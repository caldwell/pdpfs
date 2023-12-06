// Copyright © 2023 David Caldwell <david@porkrind.org>

mod block;
mod fs;
mod ops;

use std::path::PathBuf;

use block::BlockDevice;
use ops::*;

use anyhow::anyhow;
use docopt::Docopt;
use serde::Deserialize;
use strum::VariantNames;

use crate::fs::FileSystem;

fn usage() -> String {
    format!(r#"
Usage:
  pdpfs -h
  pdpfs [-h] -i <image> ls [-l] [-a]
  pdpfs [-h] -i <image> cp <source-file> <dest-file>
  pdpfs [-h] -i <image> mv [-f] <source-file> <dest-file>
  pdpfs [-h] -i <image> rm <file>
  pdpfs [-h] -i <image> cat <file>
  pdpfs [-h] -i <image> mkfs <device-type> <filesystem>
  pdpfs [-h] -i <image> dump [--sector] [<file>]
  pdpfs [-h] -i <image> dump-home
  pdpfs [-h] -i <image> dump-dir
  pdpfs [-h] -i <image> convert <image-type> <dest-file>

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
     pdpfs -i my_image.img cp ./file.txt file.txt
     pdpfs -i my_image.img cp ./file.txt .

     # This copies 'FILE.TXT' from the disk image into /tmp/FILE.TXT on the local machine:
     pdpfs -i my_image.img cp FILE.TXT /tmp

     # This copies 'FILE.TXT' from the image into './file.txt' on the local machine:
     pdpfs -i my_image.img cp file.txt ./

 mv:
   -f --force            Overwrite destination file if it exists.

   Move (rename) files on the image. <source-file> and <dest-file> specify files
   on the image.

   If <dest-file> already exists on the image an error will be indicated, unless
   the --force option is used.

 rm:
   <file> will be deleted from the image.

 cat:
   Prints the contents of <file> to stdout.

 dump:
   -s --sector            Dump by blocks instead of sectors

   Dumps the image, de-interleaving floppy images.

   If <file> is specified, dumps the file instead of the image.

 mkfs:
   Initializes a new image. The <image> file specified by `-i` will be created
   and must _not_ already exist.

   <device-type> must be one of: {}

   <filesystem> must be one of: {}

 convert:
   Convert the image to a different image file type.

   <image-type> must be one of: {}
"#,
    DeviceType::VARIANTS.iter().map(|s| *s).filter(|t| *t != "flat").collect::<Vec<&str>>().join(", "),
    FileSystemType::VARIANTS.join(", "),
    ImageType::VARIANTS.join(", "))
}

#[derive(Debug, Deserialize)]
struct Args {
    flag_image:       PathBuf,
    flag_sector:      bool,
    flag_long:        bool,
    flag_all:         bool,
    flag_force:       bool,
    cmd_ls:           bool,
    cmd_cp:           bool,
    cmd_mv:           bool,
    cmd_rm:           bool,
    cmd_dump:         bool,
    cmd_dump_home:    bool,
    cmd_dump_dir:     bool,
    cmd_mkfs:         bool,
    cmd_cat:          bool,
    cmd_convert:      bool,
    arg_source_file:  PathBuf,
    arg_dest_file:    PathBuf,
    arg_file:         Option<PathBuf>,
    arg_device_type:  Option<DeviceType>,
    arg_image_type:   Option<ImageType>,
    arg_filesystem:   Option<FileSystemType>,
}

fn main() -> anyhow::Result<()> {
    let args: Args = Docopt::new(usage())
        .and_then(|d| d.deserialize())
        .unwrap_or_else(|e| e.exit());

    // Do this very early since we normally die if the image file doesn't exist
    if args.cmd_mkfs {
        let fs = create_image(ImageType::from_file_ext(&args.flag_image)?, args.arg_device_type.unwrap(), args.arg_filesystem.unwrap())?;
        return save_image(fs.block_device().physical_device(), &args.flag_image);
    }

    let dev = open_device(&args.flag_image)?;

    // Do this early so we can dump corrupt images (since RT11FS::new() might die).
    if args.cmd_dump && args.arg_file.is_none() {
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

    let mut fs = open_fs(dev)?;

    if args.cmd_ls {
        ls(&fs, args.flag_long, args.flag_all);
    }

    if args.cmd_cp {
        match (args.arg_source_file.to_string_lossy().chars().find(|c| std::path::is_separator(*c)).is_some(),
               args.arg_dest_file  .to_string_lossy().chars().find(|c| std::path::is_separator(*c)).is_some()) {
            (false, true)  => cp_from_image(&fs, &args.arg_source_file, &args.arg_dest_file)?,
            (true,  false) => { cp_into_image(&mut fs, &args.arg_source_file, &args.arg_dest_file)?;
                                save_image(fs.block_device().physical_device(), &args.flag_image)? },
            (false, false) => Err(anyhow!("Image to image copy is not supported yet."))?,
            (true,  true)  => Err(anyhow!("Either the source or destination file needs to be on the image"))?,
        }
    }

    if args.cmd_rm {
        rm(&mut fs, &args.arg_file.unwrap())?;
        save_image(fs.block_device().physical_device(), &args.flag_image)?;
        return Ok(())
    }

    if args.cmd_mv {
        mv(&mut fs, &args.arg_source_file, &args.arg_dest_file, args.flag_force)?;
        save_image(fs.block_device().physical_device(), &args.flag_image)?;
    }

    if args.cmd_dump && args.arg_file.is_some() {
        dump_file(&fs, &args.arg_file.unwrap(), args.flag_sector)?;
        return Ok(())
    }

    if args.cmd_cat {
        use std::io::Write;
        let data = fs.read_file(&ops::path_to_rt11_filename(&args.arg_file.unwrap())?)?;
        std::io::stdout().write_all(data.as_bytes())?;
    }

    Ok(())
}
