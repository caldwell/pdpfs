// Copyright © 2023 David Caldwell <david@porkrind.org>

#![feature(return_position_impl_trait_in_trait)]

mod block;
mod fs;

use std::fs::rename;
use std::io::Write;
use std::path::{PathBuf, Path};

use block::{BlockDevice, PhysicalBlockDevice};
use block::imd::IMD;
use block::img::IMG;
use block::rx::{RX, RX01_GEOMETRY, RX02_GEOMETRY};
use block::flat::Flat;
use fs::RT11FS;

use anyhow::{anyhow, Context};
use docopt::Docopt;
use pretty_hex::PrettyHex;
use serde::Deserialize;

use crate::block::BLOCK_SIZE;

const USAGE: &'static str = "
Usage:
  rt11fs -h
  rt11fs [-h] -i <image> ls [-l] [-a]
  rt11fs [-h] -i <image> cp <source-file> <dest-file>
  rt11fs [-h] -i <image> rm <file>
  rt11fs [-h] -i <image> init <device-type>
  rt11fs [-h] -i <image> dump [--sector]
  rt11fs [-h] -i <image> dump-home
  rt11fs [-h] -i <image> dump-dir
  rt11fs [-h] -i <image> convert <image-type> <dest-file>

Options:
  -h --help              Show this screen.
  -i --image <image>     Use <image> as the disk image.

 ls:
   -a --all              List all entries, not just 'permanents'
   -l --long             Give a more detailed output. All directory entry fields in
                         the filesystem are printed and not just the most useful.

   List files in the image.

 cp:
   <source-file> and <dest-file> specify local (host) filesystem paths if they
   contain a `/` character. Otherwise they specify files on the image. The
   filenames will be converted to uppercase for convenience (but they will not
   be truncated or stripped of other invalid characters). A plain `.` in the
   <dest-file> means the same name as the <source-file>, but inside the image
   (use `./` for the local filesystem).

   Examples:
     # These both copy 'file.txt' from the local machine into disk image (as FILE.TXT):
     rt11fs -i my_image.img cp ./file.txt file.txt
     rt11fs -i my_image.img cp ./file.txt .

     # This copies 'FILE.TXT' from the disk image into /tmp/FILE.TXT on the local machine:
     rt11fs -i my_image.img cp FILE.TXT /tmp

     # This copies 'FILE.TXT' from the image into './file.txt' on the local machine:
     rt11fs -i my_image.img cp file.txt ./

 rm:
   <file> will be deleted from the image.

 dump:
   -s --sector            Dump by blocks instead of sectors

   Dumps the image, de-interleaving floppy images.

 init:
   Initializes a new image. The <image> file specified by `-i` will be created
   and must _not_ already exist.

   <device-type> must be: rx01

 convert:
   Convert the image to a different image file type.

   <image-type> must be one of: img, imd
";

#[derive(Debug, Deserialize)]
struct Args {
    flag_image:       PathBuf,
    flag_sector:      bool,
    flag_long:        bool,
    flag_all:         bool,
    cmd_ls:           bool,
    cmd_cp:           bool,
    cmd_rm:           bool,
    cmd_dump:         bool,
    cmd_dump_home:    bool,
    cmd_dump_dir:     bool,
    cmd_init:         bool,
    cmd_convert:      bool,
    arg_source_file:  PathBuf,
    arg_dest_file:    PathBuf,
    arg_file:         PathBuf,
    arg_device_type:  Option<DeviceType>,
    arg_image_type:   Option<ImageType>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DeviceType {
    RX01,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum ImageType {
    IMD,
    IMG,
}

fn main() -> anyhow::Result<()> {
    let args: Args = Docopt::new(USAGE)
        .and_then(|d| d.deserialize())
        .unwrap_or_else(|e| e.exit());

    // Do this very early since we normally die if the image file doesn't exist
    if args.cmd_init {
        return init(&args.flag_image, args.arg_device_type.unwrap());
    }

    let image = std::fs::read(&args.flag_image)?;
    match (&image[0..3], image.len()) {
        (magic, _) if magic == "IMD".as_bytes() => {
            let imd = IMD::from_bytes(&image)?;
            match imd.total_bytes() {
                bytes if bytes < 1024*1024 => with_block_dev(&args, RX(imd)),
                _                          => with_block_dev(&args, Flat(imd))
            }
        },
        (_, 256256) => with_block_dev(&args, RX(IMG::from_vec(image, RX01_GEOMETRY))),
        (_, 512512) => with_block_dev(&args, RX(IMG::from_vec(image, RX02_GEOMETRY))),
        (_, len) if len >= 1024*1024 => with_block_dev(&args, Flat(IMG::from_vec(image, block::Geometry {
            cylinders: 1,
            heads: 1,
            sectors: len/512,
            sector_size: 512,
        }))),
        (magic, len) => return Err(anyhow!("Unknown image type (magic number: {:x?}, length: {})", magic, len)),
    }
}

fn with_block_dev<B: BlockDevice>(args: &Args, dev: B) -> anyhow::Result<()> {
    // Do this early so we can dump corrupt images (since RT11FS::new() might die).
    if args.cmd_dump {
        return dump(&dev, args.flag_sector);
    }

    if args.cmd_dump_home {
        return dump_home(&dev);
    }

    if args.cmd_dump_dir {
        return dump_dir(&dev);
    }

    if args.cmd_convert {
        return convert(&dev, args.arg_image_type.unwrap(), &args.arg_dest_file);
    }

    let mut fs = RT11FS::new(dev)?;

    if args.cmd_ls {
        ls(&fs, args.flag_long, args.flag_all);
    }

    if args.cmd_cp {
        match (args.arg_source_file.to_string_lossy().chars().find(|c| std::path::is_separator(*c)).is_some(),
               args.arg_dest_file  .to_string_lossy().chars().find(|c| std::path::is_separator(*c)).is_some()) {
            (false, true)  => cp_from_image(&fs, &args.arg_source_file, &args.arg_dest_file)?,
            (true,  false) => { cp_into_image(&mut fs, &args.arg_source_file, &args.arg_dest_file)?;
                                save_image(fs.image.physical_device(), &args.flag_image)? },
            (false, false) => Err(anyhow!("Image to image copy is not supported yet."))?,
            (true,  true)  => Err(anyhow!("Either the source or destination file needs to be on the image"))?,
        }
    }

    if args.cmd_rm {
        rm(&mut fs, &args.arg_file)?;
        save_image(fs.image.physical_device(), &args.flag_image)?;
    }

    Ok(())
}

fn ls<B: BlockDevice>(fs: &RT11FS<B>, long: bool, all: bool) {
    for f in if all { Box::new(fs.dir_iter()) as Box<dyn Iterator<Item = &fs::DirEntry>> }
             else   { Box::new(fs.file_iter()) as Box<dyn Iterator<Item = &fs::DirEntry>> } {
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

fn cp_from_image<B: BlockDevice>(fs: &RT11FS<B>, src: &Path, dest: &Path) -> anyhow::Result<()> {
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
    print!("... Successfully copied {} blocks ({} bytes)\n", file.length, file.length * block::BLOCK_SIZE);
    Ok(())
}

fn cp_into_image<B: BlockDevice>(fs: &mut RT11FS<B>, src: &Path, dest: &Path) -> anyhow::Result<()> {
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

fn save_image<P: PhysicalBlockDevice>(dev: &P, filename: &Path) -> anyhow::Result<()> {
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

fn dump<B: BlockDevice>(image: &B, by_sector: bool) -> anyhow::Result<()> {
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

fn dump_home<B: BlockDevice>(image: &B) -> anyhow::Result<()> {
    let home = RT11FS::read_homeblock(image)?;
    println!("{:#?}", home);
    Ok(())
}

fn dump_dir<B: BlockDevice>(image: &B) -> anyhow::Result<()> {
    let segment_block = RT11FS::read_homeblock(image).map(|home| home.directory_start_block).unwrap_or(6);

    for (num, segment) in fs::RT11FS::read_directory(image, segment_block).enumerate() {
        match segment {
            Ok(segment) => println!("{:#?}", segment),
            Err(e) => {
                // This is for debug purposes. Try to dump as much possible without erroring out
                println!("Error reading segment {}: {:#}. Raw Dump:", num, e);

                let mut buf = image.read_blocks((segment_block + num as u16 * 2) as usize, 2)?;
                buf.set_endian(bytebuffer::Endian::LittleEndian);

                let seg = fs::DirSegment {
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

fn rm<B: BlockDevice>(fs: &mut RT11FS<B>, file: &Path) -> anyhow::Result<()> {
    fs.delete(&path_to_rt11_filename(file)?)
}

fn init(image: &Path, dtype: DeviceType) -> anyhow::Result<()> {
    let ext = image.extension().and_then(|oss| oss.to_str());
    match (dtype, ext) {
        (DeviceType::RX01, Some("img")) => return init_fs(image, RX(IMG::from_raw(vec![0; 256256], RX01_GEOMETRY))),
        (DeviceType::RX01, Some("imd")) => return init_fs(image, RX(IMD::from_raw(vec![0; 256256], RX01_GEOMETRY))),
        (DeviceType::RX01, Some(ext)) => return Err(anyhow!("Unknown image type {}", ext)),
        (DeviceType::RX01, None)      => return Err(anyhow!("Unknown image type for {}", image.to_string_lossy())),
    }
}

fn init_fs<B: BlockDevice>(path: &Path, image: B) -> anyhow::Result<()> {
    let fs = RT11FS::init(image)?;
    save_image(fs.image.physical_device(), path)?;
    Ok(())
}

fn convert<B: BlockDevice>(image: &B, image_type: ImageType, dest: &Path) -> anyhow::Result<()> {
    let (geometry, data) = image.physical_device().to_raw()?;
    match image_type {
        ImageType::IMG => save_image(&IMG::from_raw(data, geometry), dest)?,
        ImageType::IMD => save_image(&IMD::from_raw(data, geometry), dest)?,
    }
    Ok(())
}

fn path_to_rt11_filename(p: &Path) -> anyhow::Result<String> {
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
