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

use std::fs::rename;
use std::path::{PathBuf, Path};

use anyhow::{anyhow, Context};
use pretty_hex::PrettyHex;
use serde::Deserialize;
use strum::EnumVariantNames;

#[derive(Debug, Deserialize, EnumVariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum DeviceType {
    RX01,
}

#[derive(Debug, Deserialize, Clone, Copy, EnumVariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ImageType {
    IMD,
    IMG,
}

#[derive(Debug, Deserialize, Clone, Copy, EnumVariantNames)]
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
            let imd = IMD::from_bytes(&image)?;
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
    let dest = match dest {
        d if d == Path::new(".") => Path::new(src.file_name().ok_or_else(|| anyhow!("Need source filename to use '.'"))?),
        d => d,
    };
    let buf = std::fs::read(src)?;
    fs.write_file(&path_to_rt11_filename(dest)?, &buf)?;
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

pub fn dump(image: &Box<dyn BlockDevice>, by_sector: bool) -> anyhow::Result<()> {
    if by_sector {
        for s in 0..image.sectors() {
            println!("Sector {}\n{:?}", s, image.read_sector(s)?.hex_dump());
        }
    } else {
        for b in 0..image.blocks() {
            println!("Block {}\n{:?}", b, image.read_blocks(b, 1)?.as_bytes().hex_dump());
        }
    }
    Ok(())
}

pub fn dump_home(image: &Box<dyn BlockDevice>) -> anyhow::Result<()> {
    let home = RT11FS::read_homeblock(image)?;
    println!("{:#?}", home);
    Ok(())
}

pub fn dump_dir(image: &Box<dyn BlockDevice>) -> anyhow::Result<()> {
    let segment_block = RT11FS::read_homeblock(image).map(|home| home.directory_start_block).unwrap_or(6);

    for (num, segment) in RT11FS::read_directory(image, segment_block).enumerate() {
        match segment {
            Ok(segment) => println!("{:#?}", segment),
            Err(e) => {
                // This is for debug purposes. Try to dump as much possible without erroring out
                println!("Error reading segment {}: {:#}. Raw Dump:", num, e);

                let mut buf = image.read_blocks((segment_block + num as u16 * 2) as usize, 2)?;
                buf.set_endian(bytebuffer::Endian::LittleEndian);

                let seg = DirSegment {
                    segments: buf.read_u16()?,
                    next_segment: buf.read_u16()?,
                    last_segment: buf.read_u16()?,
                    extra_bytes: buf.read_u16()?,
                    data_block: buf.read_u16()?,
                    entries: vec![],
                    block: segment_block,
                };
                println!("{:?}", seg);
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

pub fn create_image(image: &Path, dtype: DeviceType, fstype: FileSystemType) -> anyhow::Result<()> {
    let ext = image.extension().and_then(|oss| oss.to_str());
    match (dtype, ext) {
        (DeviceType::RX01, Some("img")) => return mkfs(image, fstype, RX(IMG::from_raw(vec![0; 256256], RX01_GEOMETRY))),
        (DeviceType::RX01, Some("imd")) => return mkfs(image, fstype, RX(IMD::from_raw(vec![0; 256256], RX01_GEOMETRY))),
        (DeviceType::RX01, Some(ext)) => return Err(anyhow!("Unknown image type {}", ext)),
        (DeviceType::RX01, None)      => return Err(anyhow!("Unknown image type for {}", image.to_string_lossy())),
    }
}

pub fn mkfs<B: BlockDevice+ 'static>(path: &Path, fstype: FileSystemType, image: B) -> anyhow::Result<()> {
    let fs: Box<dyn FileSystem<BlockDevice = B>> = match fstype {
        FileSystemType::RT11 => Box::new(RT11FS::mkfs(image)?),
        FileSystemType::XXDP => Box::new(XxdpFs::mkfs(image)?),
    };
    save_image(fs.block_device().physical_device(), path)?;
    Ok(())
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
