// Copyright © 2023 David Caldwell <david@porkrind.org>

#![feature(return_position_impl_trait_in_trait)]

mod block;
mod fs;
mod ops;

use std::path::PathBuf;

use block::{BlockDevice, PhysicalBlockDevice};
use block::imd::IMD;
use block::img::IMG;
use block::rx::{RX, RX01_GEOMETRY, RX02_GEOMETRY};
use block::flat::Flat;
use fs::RT11FS;
use ops::*;

use anyhow::anyhow;
use docopt::Docopt;
use serde::Deserialize;

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
