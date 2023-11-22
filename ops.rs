// Copyright © 2023 David Caldwell <david@porkrind.org>

// Various operations we can do on disk image file systems

use crate::block::{BlockDevice, PhysicalBlockDevice, BLOCK_SIZE};
use crate::block::imd::IMD;
use crate::block::img::IMG;
use crate::block::rx::{RX, RX01_GEOMETRY};
use crate::fs::{DirEntry,DirSegment,RT11FS};

use std::fs::rename;
use std::io::Write;
use std::path::{PathBuf, Path};

use anyhow::{anyhow, Context};
use pretty_hex::PrettyHex;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    RX01,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ImageType {
    IMD,
    IMG,
}

pub fn ls(fs: &impl FileSystem, long: bool, all: bool) {
    for f in if all { Box::new(fs.dir_iter()) as Box<dyn Iterator<Item = &DirEntry>> }
             else   { Box::new(fs.file_iter()) as Box<dyn Iterator<Item = &DirEntry>> } {
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
    let Some(file) = fs.file_named(&source_file) else {
        return Err(anyhow!("File not found: {}", source_file));
    };
    print!("{} -> {}", file.name, local_dest.to_string_lossy());
    let data = fs.image.read_blocks(file.block, file.length)?;
    std::fs::write(local_dest, data.as_bytes())?;
    print!("... Successfully copied {} blocks ({} bytes)\n", file.length, file.length * BLOCK_SIZE);
    Ok(())
}

pub fn cp_into_image(fs: &mut impl FileSystem, src: &Path, dest: &Path) -> anyhow::Result<()> {
    let m = src.metadata()?;
    let dest = match dest {
        d if d == Path::new(".") => Path::new(src.file_name().ok_or_else(|| anyhow!("Need source filename to use '.'"))?),
        d => d,
    };
    let mut fh = fs.create(&path_to_rt11_filename(dest)?,
                           m.len() as usize)?;
    let buf = std::fs::read(src)?;
    fh.write(&buf)?;
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

pub fn init(image: &Path, dtype: DeviceType) -> anyhow::Result<()> {
    let ext = image.extension().and_then(|oss| oss.to_str());
    match (dtype, ext) {
        (DeviceType::RX01, Some("img")) => return init_fs(image, RX(IMG::from_raw(vec![0; 256256], RX01_GEOMETRY))),
        (DeviceType::RX01, Some("imd")) => return init_fs(image, RX(IMD::from_raw(vec![0; 256256], RX01_GEOMETRY))),
        (DeviceType::RX01, Some(ext)) => return Err(anyhow!("Unknown image type {}", ext)),
        (DeviceType::RX01, None)      => return Err(anyhow!("Unknown image type for {}", image.to_string_lossy())),
    }
}

pub fn init_fs<B: BlockDevice>(path: &Path, image: B) -> anyhow::Result<()> {
    let fs = RT11FS::init(image)?;
    save_image(fs.image.physical_device(), path)?;
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
