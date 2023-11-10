mod block;
mod fs;

use std::path::PathBuf;

use block::{BlockDevice, PhysicalBlockDevice};
use block::imd::IMD;
use block::img::IMG;
use block::rx::{RX, RX01_GEOMETRY, RX02_GEOMETRY};
use fs::{RT11FS, EntryKind};

use anyhow::{anyhow, Context};
use docopt::Docopt;
use serde::Deserialize;

const USAGE: &'static str = "
Usage:
  rt11fs -h
  rt11fs [-h] -i <image> ls
  rt11fs [-h] -i <image> cp <image-file> <local-destination>

Options:
  -h --help              Show this screen.
  -i --image <image>     Use <image> as the disk image.
";
#[derive(Debug, Deserialize)]
struct Args {
    flag_image:       PathBuf,
    cmd_ls:           bool,
    cmd_cp:           bool,
    arg_image_file:   PathBuf,
    arg_local_destination: PathBuf,
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
        let local_dest = match (args.arg_local_destination.exists(), std::fs::metadata(&args.arg_local_destination)) {
            (true, Ok(m)) if m.is_dir() => args.arg_local_destination.join(args.arg_image_file.file_name()
                                                                               .ok_or(anyhow!("Bad filename: {}", args.arg_image_file.to_string_lossy()))?),
            (true, Err(e)) => Err(e).with_context(|| format!("{}", args.arg_local_destination.to_string_lossy()))?,
            (_, _) => args.arg_local_destination.clone(),
        };
        let source_file = args.arg_image_file.to_str().ok_or(anyhow!("Bad filename: {}", args.arg_image_file.to_string_lossy()))?
            .to_uppercase();
        for s in fs.dir.iter() {
            let mut block_offset = 0usize;
            for f in s.entries.iter() {
                if f.kind != EntryKind::Permanent || f.name != source_file {
                    block_offset += f.length;
                    continue;
                }
                print!("{} -> {}", f.name, local_dest.to_string_lossy());
                let data = fs.image.block(s.data_block as usize + block_offset, f.length)?;
                std::fs::write(local_dest, data.as_bytes())?;
                print!("... Successfully copied {} blocks ({} bytes)\n", f.length, f.length * block::BLOCK_SIZE);
                return Ok(())
            }
        }
        return Err(anyhow!("File not found: {}", source_file));
    }

    Ok(())
}

fn ls<B: BlockDevice>(fs: &RT11FS<B>) {
    for s in fs.dir.iter() {
        for f in s.entries.iter() {
            if f.kind != EntryKind::Permanent { continue }
            println!("{:10} {:>3}:{:<3} {:6} {}", f.creation_date.map(|d| d.to_string()).unwrap_or("<no-date>".to_string()), f.job, f.channel, f.length, f.name);
        }
    }
}
