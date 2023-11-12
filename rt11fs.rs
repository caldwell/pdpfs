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
use fs::RT11FS;

use anyhow::{anyhow, Context};
use docopt::Docopt;
use pretty_hex::PrettyHex;
use serde::Deserialize;

use crate::block::BLOCK_SIZE;

const USAGE: &'static str = "
Usage:
  rt11fs -h
  rt11fs [-h] -i <image> ls
  rt11fs [-h] -i <image> cp <source-file> <dest-file>
  rt11fs [-h] -i <image> rm <file>
  rt11fs [-h] -i <image> init <device-type>
  rt11fs [-h] -i <image> dump [--sector]

Options:
  -h --help              Show this screen.
  -i --image <image>     Use <image> as the disk image.

 cp:
   <source-file> and <dest-file> specify local (host) filesystem paths if they
   contain a `/` character. Otherwise they specify files on the image. The
   filenames will be converted to uppercase for convenience (but they will not be
   truncated or stripped of other invalid characters).

   Examples:
     # This copies 'file.txt' from the local machine into disk image (as FILE.TXT):
     rt11fs -i my_image.img cp ./file.txt file.txt

     # This copies 'FILE.TXT' from the disk image into /tmp/FILE.TXT on the local machine:
     rt11fs -i my_image.img cp FILE.TXT /tmp

 rm:
   <file> will be deleted from the image.

 dump:
   -s --sector            Dump by blocks instead of sectors

   Dumps the image, de-interleaving floppy images.

 init:
   Initializes a new image. The <image> file specified by `-i` will be created
   and must _not_ already exist.

   <device-type> must be: rx01
";

#[derive(Debug, Deserialize)]
struct Args {
    flag_image:       PathBuf,
    flag_sector:      bool,
    cmd_ls:           bool,
    cmd_cp:           bool,
    cmd_rm:           bool,
    cmd_dump:         bool,
    cmd_init:         bool,
    arg_source_file:  PathBuf,
    arg_dest_file:    PathBuf,
    arg_file:         PathBuf,
    arg_device_type:  Option<DeviceType>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DeviceType {
    RX01,
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
        (magic, _) if magic == "IMD".as_bytes() => with_physical_dev(&args, IMD::from_bytes(&image)?),
        (_, 256256) => with_physical_dev(&args, IMG::from_vec(image, RX01_GEOMETRY)),
        (_, 512512) => with_physical_dev(&args, IMG::from_vec(image, RX02_GEOMETRY)),
        (magic, len) => return Err(anyhow!("Unknown image (magic number: {:x?}, length: {})", magic, len)),
    }
}

fn with_physical_dev<P: PhysicalBlockDevice>(args: &Args, dev: P) -> anyhow::Result<()> {
    let rx = RX(dev);

    // Do this early so we can dump corrupt images (since RT11FS::new() might die).
    if args.cmd_dump {
        return dump(&rx, args.flag_sector);
    }

    let mut fs = RT11FS::new(rx)?;

    if args.cmd_ls {
        ls(&fs);
    }

    if args.cmd_cp {
        match (args.arg_source_file.components().count() > 1,
               args.arg_dest_file.components().count() > 1) {
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

fn ls<B: BlockDevice>(fs: &RT11FS<B>) {
    for f in fs.file_iter() {
        println!("{:10} {:>3}:{:<3} {:6} {}", f.creation_date.map(|d| d.to_string()).unwrap_or("<no-date>".to_string()), f.job, f.channel, f.length, f.name);
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
    let mut fh = fs.create(&path_to_rt11_filename(dest)?,
                           m.len() as usize)?;
    let buf = std::fs::read(src)?;
    fh.write(&buf)?;
    Ok(())
}

fn save_image<P: PhysicalBlockDevice>(dev: &P, filename: &Path) -> anyhow::Result<()> {
    let new_image = dev.as_vec();
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

fn rm<B: BlockDevice>(fs: &mut RT11FS<B>, file: &Path) -> anyhow::Result<()> {
    fs.delete(&path_to_rt11_filename(file)?)
}

fn init(image: &Path, dtype: DeviceType) -> anyhow::Result<()> {
    let ext = image.extension().and_then(|oss| oss.to_str());
    match (dtype, ext) {
        (DeviceType::RX01, Some("img")) => return init_fs(image, RX(IMG::from_bytes(&[0; 256256], RX01_GEOMETRY))),
        (DeviceType::RX01, Some("imd")) => return init_fs(image, RX(IMD::from_img(IMG::from_bytes(&[0; 256256], RX01_GEOMETRY)))),
        (DeviceType::RX01, Some(ext)) => return Err(anyhow!("Unknown image type {}", ext)),
        (DeviceType::RX01, None)      => return Err(anyhow!("Unknown image type for {}", image.to_string_lossy())),
    }
}

fn init_fs<B: BlockDevice>(path: &Path, image: B) -> anyhow::Result<()> {
    let fs = RT11FS::init(image)?;
    save_image(fs.image.physical_device(), path)?;
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
