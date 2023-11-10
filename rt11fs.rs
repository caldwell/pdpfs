mod block;
mod fs;

use std::path::PathBuf;

use block::{BlockDevice, PhysicalBlockDevice};
use block::imd::IMD;
use block::img::IMG;
use block::rx::{RX, RX01_GEOMETRY, RX02_GEOMETRY};
use fs::{RT11FS, EntryKind};

use anyhow::anyhow;
use docopt::Docopt;
use serde::Deserialize;

const USAGE: &'static str = "
Usage:
  rt11fs -h
  rt11fs [-h] -i <image> ls

Options:
  -h --help              Show this screen.
  -i --image <image>     Use <image> as the disk image.
";
#[derive(Debug, Deserialize)]
struct Args {
    flag_image:       PathBuf,
    cmd_ls:           bool,
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
