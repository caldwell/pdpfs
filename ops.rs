// Copyright © 2023 David Caldwell <david@porkrind.org>

// Various operations we can do on disk image file systems

use crate::block::{BlockDevice, PhysicalBlockDevice, BLOCK_SIZE, Geometry};
use crate::block::flat::Flat;
use crate::block::imd::IMD;
use crate::block::img::IMG;
use crate::block::rx::{RX, RX01_GEOMETRY, RX02_GEOMETRY};
use crate::fs::xxdp::XxdpFs;
use crate::fs::{FileSystem,DirEntry};
use crate::fs::rt11::{DirSegment,RT11FS};

use std::cmp::min;
use std::fs::rename;
use std::ops::Range;
use std::path::{PathBuf, Path};

use anyhow::{anyhow, Context};
use pretty_hex::PrettyHex;
use serde::Deserialize;
use strum::{EnumVariantNames, EnumString, Display};
pub use strum;

#[derive(Debug, Deserialize, EnumVariantNames, EnumString, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum DeviceType {
    RX01,
    RX02,
    Flat(usize),
}

impl DeviceType {
    pub fn geometry(&self) -> Geometry {
        match self {
            DeviceType::RX01 => RX01_GEOMETRY,
            DeviceType::RX02 => RX02_GEOMETRY,
            DeviceType::Flat(size) => Geometry {
                cylinders: 1,
                heads: 1,
                sectors: size/512,
                sector_size: 512,
            },
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy, EnumVariantNames, EnumString, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ImageType {
    IMD,
    IMG,
}

impl ImageType {
    pub fn from_file_ext(path: &Path) -> anyhow::Result<ImageType> {
        let ext = path.extension().and_then(|oss| oss.to_str());
        match ext {
            Some("img") => Ok(ImageType::IMG),
            Some("imd") => Ok(ImageType::IMD),
            Some(ext) => Err(anyhow!("Unknown image type for extention {}", ext)),
            None        => Err(anyhow!("Unknown image type for {}", path.display())),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy, EnumVariantNames, EnumString, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum FileSystemType {
    RT11,
    XXDP,
}

pub fn open_device(image_file: &Path) -> anyhow::Result<Box<dyn BlockDevice>> {
    let image = std::fs::read(image_file)?;
    Ok(match (&image[0..3], image.len()) {
        (magic, _) if magic == "IMD".as_bytes() => {
            let imd = IMD::from_bytes(&image).with_context(|| "Malformed IMD file")?;
            match imd.total_bytes() {
                bytes if bytes < 1024*1024 => Box::new(RX(imd)),
                _                          => Box::new(Flat(imd))
            }
        },
        (_, 256256) => Box::new(RX(IMG::from_vec(image, RX01_GEOMETRY))),
        (_, 512512) => Box::new(RX(IMG::from_vec(image, RX02_GEOMETRY))),
        (_, len) if len >= 1024*1024 => Box::new(Flat(IMG::from_vec(image, Geometry {
            cylinders: 1,
            heads: 1,
            sectors: len/512,
            sector_size: 512,
        }))),
        (magic, len) => return Err(anyhow!("Unknown image type (magic number: {:x?}, length: {})", magic, len)),
    })
}

pub fn open_fs(dev: Box<dyn BlockDevice>) -> anyhow::Result<Box<dyn FileSystem<BlockDevice = Box<dyn BlockDevice>>>> {
    let fs: Box<dyn FileSystem<BlockDevice=Box<dyn BlockDevice>>> =
        if XxdpFs::image_is(&dev) {
            Box::new(XxdpFs::new(dev)?)
        } else if RT11FS::image_is(&dev) {
            Box::new(RT11FS::new(dev)?)
        } else {
            return Err(anyhow!("Unknown filesystem on image"));
        };
    Ok(fs)
}

pub fn ls(fs: &impl FileSystem, long: bool, all: bool) {
    for f in if all { Box::new(fs.dir_iter("/").expect("fixme")) as Box<dyn Iterator<Item = Box<dyn DirEntry>>> }
             else   { fs.read_dir("/").expect("fixme") } {
        match long {
            false => println!("{:?}", f),
            true  => println!("{:#?}", f),
        }
    }
    let free_blocks = fs.free_blocks();
    let used_blocks = fs.used_blocks();
    println!("\nUsed  {:4} blocks {:7} bytes {:3}%\nFree  {:4} blocks {:7} bytes {:3}%\nTotal {:4} blocks {:7} bytes",
             used_blocks, used_blocks * BLOCK_SIZE, used_blocks * 100 / (used_blocks + free_blocks),
             free_blocks, free_blocks * BLOCK_SIZE, free_blocks * 100 / (used_blocks + free_blocks),
             used_blocks + free_blocks, (used_blocks + free_blocks) * BLOCK_SIZE);
}

pub fn cp_from_image(fs: &impl FileSystem, src: &Path, dest: &Path) -> anyhow::Result<()> {
    let local_dest = match (dest.exists(), std::fs::metadata(&dest)) {
        (true, Ok(m)) if m.is_dir() => dest.join(src.file_name().ok_or(anyhow!("Bad filename: {}", src.to_string_lossy()))?),
        (true, Err(e)) => Err(e).with_context(|| format!("{}", dest.to_string_lossy()))?,
        (_, _) => dest.to_owned(),
    };
    let source_file = src.to_str().ok_or(anyhow!("Bad filename: {}", src.to_string_lossy()))?
        .to_uppercase();
    let data = fs.read_file(&source_file)?;
    let file = fs.stat(&source_file).unwrap();
    print!("{} -> {}", file.file_name(), local_dest.to_string_lossy());
    std::fs::write(local_dest, data.as_bytes())?;
    print!("... Successfully copied {} blocks ({} bytes)\n", file.blocks(), file.len());
    Ok(())
}

pub fn cp_into_image(fs: &mut impl FileSystem, src: &Path, dest: &Path) -> anyhow::Result<()> {
    let dest = path_to_rt11_filename(match dest {
        d if d == Path::new(".") => Path::new(src.file_name().ok_or_else(|| anyhow!("Need source filename to use '.'"))?),
        d => d,
    })?;
    let buf = std::fs::read(src).with_context(|| format!("Reading \"{}\" failed", src.display()))?;
    fs.write_file(&dest, &buf).with_context(|| format!("Creating \"{}\" on disk image failed", dest))?;
    Ok(())
}

pub fn save_image(dev: Box<&dyn PhysicalBlockDevice>, filename: &Path) -> anyhow::Result<()> {
    let new_image = dev.as_vec()?;
    let newname = filename.append(".new");
    let bakname = filename.append(".bak");
    std::fs::write(&newname, &new_image).with_context(|| format!("{}", newname.to_string_lossy()))?;
    if filename.exists() {
        rename(filename, &bakname)?;
    }
    rename(&newname, filename)?;
    Ok(())
}

pub fn dump(image: &Box<dyn BlockDevice>, by_sector: bool, range: Option<Range<usize>>) -> anyhow::Result<()> {
    let range = range.unwrap_or(0..usize::MAX);
    if by_sector {
        for s in range.start..min(range.end,image.sectors()) {
            println!("Sector {}\n{:?}", s, image.read_sector(s)?.hex_dump());
        }
    } else {
        for b in range.start..min(range.end,image.blocks()) {
            println!("Block {}\n{:?}", b, image.read_blocks(b, 1)?.as_bytes().hex_dump());
        }
    }
    Ok(())
}

pub fn dump_file(fs: &impl FileSystem, file: &Path, by_sector: bool, range: Option<Range<usize>>) -> anyhow::Result<()> {
    let range = range.unwrap_or(0..usize::MAX);
    let file = path_to_rt11_filename(&file)?;
    let data = fs.read_file(&file)?;
    let chunk_size = if by_sector { fs.block_device().sector_size() } else { crate::block::BLOCK_SIZE };
    for c in range.start..min(range.end, data.len()/chunk_size) {
        println!("{} Logical {} {}\n{:?}", file, // It would be really nice to be able to print the physical block here...
            if by_sector { "Sector" } else { "Block" },
            c, data.as_bytes()[c*chunk_size..(c+1)*chunk_size].hex_dump());
    }
    Ok(())
}

pub fn rt11_dump_home(image: &Box<dyn BlockDevice>) -> anyhow::Result<()> {
    let home = RT11FS::read_homeblock(image)?;
    println!("{:#?}", home);
    Ok(())
}

pub fn rt11_dump_dir(image: &Box<dyn BlockDevice>) -> anyhow::Result<()> {
    let segment_start_block = RT11FS::read_homeblock(image).map(|home| home.directory_start_block).unwrap_or(6);
    let mut segment_num: u16 = 1;

    for segment in RT11FS::read_directory(image, segment_start_block) {
        match segment {
            Ok(segment) => {
                println!("{:#?}", segment);
                segment_num = segment.next_segment;
            },
            Err(e) => {
                // This is for debug purposes. Try to dump as much possible without erroring out
                let segment_block = crate::fs::rt11::DirSegment::segment_block(segment_start_block, segment_num);
                println!("Error reading segment #{}: {:#}. Raw Dump @ {}:", segment_num, e, segment_block);

                let mut buf = image.read_blocks(segment_block as usize, 2)?;
                buf.set_endian(bytebuffer::Endian::LittleEndian);

                let seg = DirSegment {
                    segments: buf.read_u16()?,
                    next_segment: buf.read_u16()?,
                    last_segment: buf.read_u16()?,
                    extra_bytes: buf.read_u16()?,
                    data_block: buf.read_u16()?,
                    entries: vec![],
                    segment: segment_num,
                    block: segment_block,
                };
                println!("{:#?}", seg);
                for entry in 0..(512-5)/7 {
                    print!("Directory Entry {}: ", entry);
                    for w in 0..7 {
                        print!("{}{:#08o}", if w == 0 { "" } else { "," }, buf.read_u16()?);
                    }
                    println!("");
                }
            }
        }
    }
    Ok(())
}

pub fn rm(fs: &mut impl FileSystem, file: &Path) -> anyhow::Result<()> {
    fs.delete(&path_to_rt11_filename(file)?)
}

pub fn mv(fs: &mut impl FileSystem, src: &Path, dest: &Path, overwrite_dest: bool) -> anyhow::Result<()> {
    if !overwrite_dest && fs.stat(&path_to_rt11_filename(dest)?).is_some() { return Err(anyhow!("Destination file already exists")) }
    fs.rename(&path_to_rt11_filename(src)?, &path_to_rt11_filename(dest)?)
}

pub fn create_image(imtype: ImageType, dtype: DeviceType, fstype: FileSystemType) -> anyhow::Result<Box<dyn FileSystem<BlockDevice = Box<dyn BlockDevice>>>> {
    let geometry = dtype.geometry();

    fn create_device<'a, P: PhysicalBlockDevice + 'a>(dtype: DeviceType, phys: P) -> Box<dyn BlockDevice+'a> {
        match dtype {
            DeviceType::RX01    |
            DeviceType::RX02    => Box::new(RX(phys)),
            DeviceType::Flat(_) => Box::new(Flat(phys)),
        }
    }

    let dev = match imtype {
        ImageType::IMD => create_device(dtype, IMD::from_raw(vec![0; geometry.bytes()], geometry)),
        ImageType::IMG => create_device(dtype, IMG::from_raw(vec![0; geometry.bytes()], geometry)),
    };

    Ok(match fstype {
        FileSystemType::RT11 => Box::new(RT11FS::mkfs(dev)?) as Box<dyn FileSystem<BlockDevice = Box<dyn BlockDevice>>>,
        FileSystemType::XXDP => Box::new(XxdpFs::mkfs(dev)?) as Box<dyn FileSystem<BlockDevice = Box<dyn BlockDevice>>>,
    })
}

pub fn convert(image: &Box<dyn BlockDevice>, image_type: ImageType, dest: &Path) -> anyhow::Result<()> {
    let (geometry, data) = image.physical_device().to_raw()?;
    match image_type {
        ImageType::IMG => save_image(Box::new(&IMG::from_raw(data, geometry)), dest)?,
        ImageType::IMD => save_image(Box::new(&IMD::from_raw(data, geometry)), dest)?,
    }
    Ok(())
}

pub fn path_to_rt11_filename(p: &Path) -> anyhow::Result<String> {
    Ok(p.to_str().ok_or(anyhow!("Bad filename: {}", p.to_string_lossy()))?
        .to_uppercase())
}

// Stolen^H^H^H^H^H^H Adapted from https://internals.rust-lang.org/t/pathbuf-has-set-extension-but-no-add-extension-cannot-cleanly-turn-tar-to-tar-gz/14187/10
// WHY ISN"T THIS IN STDLIB?!?!?!?!?!?!???!?!111
use std::ffi::{OsString, OsStr};
trait Append {
    fn append(&self, ext: impl AsRef<OsStr>) -> PathBuf;
}

impl Append for Path {
    fn append(&self, ext: impl AsRef<OsStr>) -> PathBuf {
        let mut os_string: OsString = self.to_owned().into();
        os_string.push(ext.as_ref());
        os_string.into()
    }
}
