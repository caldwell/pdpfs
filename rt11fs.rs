mod block;
mod fs;

use std::path::{PathBuf, Path};

use block::{BlockDevice, PhysicalBlockDevice};
use block::imd::IMD;
use block::img::IMG;
use block::rx::{RX, RX01_GEOMETRY, RX02_GEOMETRY};
use fs::RT11FS;

use anyhow::{anyhow, Context};
use docopt::Docopt;
use serde::Deserialize;

use crate::block::BLOCK_SIZE;

const USAGE: &'static str = "
Usage:
  rt11fs -h
  rt11fs [-h] -i <image> ls
  rt11fs [-h] -i <image> cp <source-file> <dest-file>

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
";
#[derive(Debug, Deserialize)]
struct Args {
    flag_image:       PathBuf,
    cmd_ls:           bool,
    cmd_cp:           bool,
    arg_source_file:  PathBuf,
    arg_dest_file:    PathBuf,
}
fn main() -> anyhow::Result<()> {
    let args: Args = Docopt::new(USAGE)
        .and_then(|d| d.deserialize())
        .unwrap_or_else(|e| e.exit());

    let image = std::fs::read(&args.flag_image)?;
    match (&image[0..3], image.len()) {
        (magic, _) if magic == "IMD".as_bytes() => with_physical_dev(&args, IMD::from_bytes(&image)?),
        (_, 256256) => with_physical_dev(&args, IMG::from_vec(image, RX01_GEOMETRY)),
        (_, 512512) => with_physical_dev(&args, IMG::from_vec(image, RX02_GEOMETRY)),
        (magic, len) => return Err(anyhow!("Unknown image (magic number: {:x?}, length: {})", magic, len)),
    }
}

fn with_physical_dev<P: PhysicalBlockDevice>(args: &Args, dev: P) -> anyhow::Result<()> {
    let fs = RT11FS::new(RX(dev))?;

    if args.cmd_ls {
        ls(&fs);
    }

    if args.cmd_cp {
        match (args.arg_source_file.components().count() > 1,
               args.arg_dest_file.components().count() > 1) {
            (false, true)  => cp_from_image(&fs, &args.arg_source_file, &args.arg_dest_file)?,
            (true,  false) => cp_into_image(&fs, &args.arg_source_file, &args.arg_dest_file)?,
            (false, false) => Err(anyhow!("Image to image copy is not supported yet."))?,
            (true,  true)  => Err(anyhow!("Either the source or destination file needs to be on the image"))?,
        }
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
    let data = fs.image.block(file.block, file.length)?;
    std::fs::write(local_dest, data.as_bytes())?;
    print!("... Successfully copied {} blocks ({} bytes)\n", file.length, file.length * block::BLOCK_SIZE);
    Ok(())
}

fn cp_into_image<B: BlockDevice>(fs: &RT11FS<B>, src: &Path, dest: &Path) -> anyhow::Result<()> {
    todo!()
}
