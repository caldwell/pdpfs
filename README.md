rt11fs
======

Manipulate RT-11 Filesystems on disk images.

This is a CLI app designed to move files on and off of RT-11 filesystems. It
can read and write IMD image files and flat binary image files (often just
called ".img").

It currently supports RX-01 images and flat hard disk images over 1MB.

Find the latest version at https://porkrind.org/rt11fs

## Usage

    rt11fs -h
    rt11fs [-h] -i <image> ls [-l] [-a]
    rt11fs [-h] -i <image> cp <source-file> <dest-file>
    rt11fs [-h] -i <image> mv [-f] <source-file> <dest-file>
    rt11fs [-h] -i <image> rm <file>
    rt11fs [-h] -i <image> mkfs <device-type> <filesystem>
    rt11fs [-h] -i <image> dump [--sector]
    rt11fs [-h] -i <image> dump-home
    rt11fs [-h] -i <image> dump-dir
    rt11fs [-h] -i <image> convert <image-type> <dest-file>

### Options:

    -h --help              Show this screen.
    -i --image <image>     Use <image> as the disk image.

### Commands:

#### `ls [-l] [-a]`

    -a --all              List all entries, not just 'permanents'
    -l --long             Give a more detailed output. All directory entry fields in
                          the filesystem are printed and not just the most useful.

List files in the image.

#### `cp <source-file> <dest-file>`

`<source-file>` and `<dest-file>` specify local (host) filesystem paths if
they contain a `/` character. Otherwise they specify files on the
image. The filenames will be converted to uppercase for convenience (but
they will not be truncated or stripped of other invalid characters). A
plain `.` in the `<dest-file>` means the same name as the `<source-file>`, but
inside the image (use `./` for the local filesystem).

Examples:

    # These both copy 'file.txt' from the local machine into disk image (as FILE.TXT):
    rt11fs -i my_image.img cp ./file.txt file.txt
    rt11fs -i my_image.img cp ./file.txt .

    # This copies 'FILE.TXT' from the disk image into /tmp/FILE.TXT on the local machine:
    rt11fs -i my_image.img cp FILE.TXT /tmp

    # This copies 'FILE.TXT' from the image into './file.txt' on the local machine:
    rt11fs -i my_image.img cp file.txt ./

#### `mv [-f] <source-file> <dest-file>`

    -f --force            Overwrite destination file if it exists.

Move (rename) files on the image. `<source-file>` and `<dest-file>` specify
files on the image.

If `<dest-file>` already exists on the image an error will be indicated,
unless the `--force` option is used.

#### `rm <file>`

`<file>` will be deleted from the image.

#### `dump [--sector]`

    -s --sector            Dump by blocks instead of sectors

Print a hex dump of the logical blocks of the image, de-interleaving floppy images.

#### `dump-home`

Print a debug dump of the fields of the home block.

#### `dump-dir`

Print a debug dump of the fields of the directory segments.

#### `mkfs <device-type> <filesystem>`

Initializes a new image. The `<image>` file specified by `-i` will be created
and must _not_ already exist.

`<device-type>` must be: `rx01`

`<filesystem>` must be one of: `rt11`, `xxdp`

#### `convert <image-type> <dest-file>`

Convert the image to a different image file type.

`<image-type>` must be one of: `img`, `imd`

## Examples

List the contents of an image:

    $ rt11fs -i RT11RX01.IMD ls
    Warning: Bad checksum: computed (9f88) != on disk (0000)
    1988-03-07   0:0       80 RT11SJ.SYS
    1987-09-02   0:0       27 SWAP.SYS
    1988-03-07   0:0        2 TT.SYS
    1988-03-07   0:0        8 DU.SYS
    1988-03-07   0:0        5 DD.SYS
    1988-03-07   0:0        4 DX.SYS
    1984-09-05   0:0        4 DY.SYS
    1988-03-07   0:0        5 LS.SYS
    1987-09-02   0:0       30 PIP.SAV
    1987-09-02   0:0       49 DUP.SAV
    1987-09-02   0:0       19 DIR.SAV
    1987-09-02   0:0       58 KED.SAV
    1987-09-02   0:0       25 RESORC.SAV
    1987-09-02   0:0       17 SL.SYS
    1987-09-02   0:0       58 IND.SAV
    1985-07-16   0:0        1 STARTS.COM

    Used   392 blocks  200704 bytes  80%
    Free    94 blocks   48128 bytes  19%
    Total  486 blocks  248832 bytes

Delete a file from the image:

    rt11fs -i RT11RX01.IMD rm SWAP.SYS

Copy a file to the local computer (the file with a `/` in the name will be
interpreted as the local computer).:

    rt11fs -i RT11RX01.IMD cp ./STARTS.COM .

Copy a file to the image from the local computer:

    rt11fs -i RT11RX01.IMD cp STARTS.COM ./

Rename a file on the image:

    rt11fs -i RT11RX01.IMD mv STARTS.COM starts.bak

Initialize a new blank image (it uses the image extension to figure out the
image format):

    rt11fs -i new_image.img init rx01

Or, to create a new IMD image:

    rt11fs -i new_image.imd init rx01

Convert an existing image from IMD to IMG format:

    rt11fs -i original.imd convert img new-image.rx01

Convert an existing image from IMG to IMD format:

    rt11fs -i original.img convert imd new-image.imd


Building
--------

rt11fs is written in Rust, so you will need the [Rust
compiler](https://rust-lang.org) to be installed in order to build it.

To build it:

    cargo build

To run the automated tests:

    cargo test


License
-------

Copyright © 2022 David Caldwell <david_rt11fs@porkrind.org>

*TLDR: [GPLv3](LICENSE.md). You can redistribute the .exe (or a modified
version) as long as you ship the source code used to build it alongside.*

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program.  If not, see <https://www.gnu.org/licenses/>.
