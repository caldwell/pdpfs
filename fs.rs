// Copyright © 2023 David Caldwell <david@porkrind.org>

pub mod rt11;
pub mod xxdp;

use std::{ops::{Deref, DerefMut}, fmt::Debug};

use anyhow::anyhow;
use bytebuffer::ByteBuffer;

use crate::block::BlockDevice;

pub trait FileSystem : Send + Sync {
    type BlockDevice: BlockDevice;

    fn filesystem_name(&self) -> &str;
    fn dir_iter<'a>(&'a self, path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn DirEntry + 'a>> + 'a>>;
    fn read_dir<'a>(&'a self, path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn DirEntry + 'a>> + 'a>>;
    fn stat<'a>(&'a self, name: &str) -> Option<Box<dyn DirEntry + 'a>>;
    fn free_blocks(&self) -> usize;
    fn used_blocks(&self) -> usize;
    fn read_file(&self, name: &str) -> anyhow::Result<ByteBuffer>;
    fn write_file(&mut self, name: &str, contents: &[u8]) -> anyhow::Result<()>;
    fn delete(&mut self, name: &str) -> anyhow::Result<()>;
    fn block_device(&self) -> &Self::BlockDevice;
    fn rename(&mut self, src: &str, dest: &str) -> anyhow::Result<()> {
        if src == dest { return Ok(()) } // Gotta check for this otherwise we'd delete ourselves below!
        // Can't combine this with find_file_named() below because the (segment,entry)
        // might change after we delete the dest (due to coalesce_empty()). And we don't
        // want to delete the dest _before_ error checking.
        if !self.stat(src).is_some() { return Err(anyhow!("File not found")) };
        if self.stat(dest).is_some() {
            self.delete(dest)?;
        }
        self.rename_unchecked(src, dest)
    }
    fn rename_unchecked(&mut self, src: &str, dest: &str) -> anyhow::Result<()>;
}

// It's really a shame this isn't automatic or derivable or something.
impl<B: BlockDevice+Send+Sync> FileSystem for Box<dyn FileSystem<BlockDevice = B>> {
    type BlockDevice = B;
    fn filesystem_name(&self) -> &str { self.deref().filesystem_name() }
    fn dir_iter<'a>(&'a self, path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn DirEntry + 'a>> + 'a>> { self.deref().dir_iter(&path) }
    fn read_dir<'a>(&'a self, path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn DirEntry + 'a>> + 'a>> { self.deref().read_dir(&path) }
    fn stat<'a>(&'a self, name: &str) -> Option<Box<dyn DirEntry + 'a>> { self.deref().stat(name) }
    fn free_blocks(&self) -> usize { self.deref().free_blocks() }
    fn used_blocks(&self) -> usize { self.deref().used_blocks() }
    fn read_file(&self, name: &str) -> anyhow::Result<ByteBuffer> { self.deref().read_file(name) }
    fn write_file(&mut self, name: &str, contents: &[u8]) -> anyhow::Result<()> { self.deref_mut().write_file(name, contents) }
    fn delete(&mut self, name: &str) -> anyhow::Result<()> { self.deref_mut().delete(name) }
    fn rename_unchecked(&mut self, src: &str, dest: &str) -> anyhow::Result<()> { self.deref_mut().rename_unchecked(src, dest) }
    fn block_device(&self) -> &B { self.deref().block_device() }
}

#[allow(dead_code)]
pub enum Timestamp {
    Date(chrono::NaiveDate),
    DateTime(chrono::NaiveDateTime),
}

pub trait DirEntry : Debug {
    fn path(&self) -> &str;
    fn file_name(&self) -> &str;

    fn is_dir(&self) -> bool;
    fn is_file(&self) -> bool;
    fn is_symlink(&self) -> bool;

    fn len(&self) -> u64;
    fn modified(&self) -> anyhow::Result<Timestamp>;
    fn accessed(&self) -> anyhow::Result<Timestamp>;
    fn created(&self)  -> anyhow::Result<Timestamp>;

    fn blocks(&self) -> u64;

    fn readonly(&self) -> bool;
}


#[cfg(test)]
mod test { // No tests here, just helpful stuff that filesystem tests can use
    use super::*;
    use crate::block::PhysicalBlockDevice;

    // Replacement for chrono::Local::now() so that tests are consistent
    #[allow(non_snake_case)]
    pub(crate) mod Local {
        pub(crate) fn now() -> chrono::DateTime<chrono::FixedOffset> {
            chrono::DateTime::<chrono::FixedOffset>::parse_from_rfc3339("2023-01-19 12:13:14+08:00").unwrap()
        }
    }

    pub(crate) fn username() -> String { "test-user".into() }

    pub(crate) struct TestDev(pub Vec<u8>);
    impl BlockDevice for TestDev {
        fn read_sector(&self, sector: usize) -> anyhow::Result<Vec<u8>> {
            Ok(self.0[sector*512..(sector+1)*512].into())
        }
        fn write_sector(&mut self, sector: usize, buf: &[u8]) -> anyhow::Result<()> {
            self.0.splice(sector*512..(sector+1)*512, buf.into_iter().map(|b| *b));
            Ok(())
        }
        fn sector_size(&self) -> usize { 512 }
        fn sectors(&self) -> usize { self.0.len()/512 }
        fn physical_device(&self) -> Box<&dyn crate::block::PhysicalBlockDevice> {
            Box::new(self)
        }
    }
    impl PhysicalBlockDevice for TestDev {
        fn geometry(&self) -> &crate::block::Geometry {unimplemented!()}
        fn read_sector(&self, _cylinder: usize, _head: usize, _sector: usize) -> anyhow::Result<Vec<u8>> {unimplemented!()}
        fn write_sector(&mut self, _cylinder: usize, _head: usize, _sector: usize, _buf: &[u8]) -> anyhow::Result<()> {unimplemented!()}
        fn as_vec(&self) -> anyhow::Result<Vec<u8>> {unimplemented!()}
        fn from_raw(_data: Vec<u8>, _geometry: crate::block::Geometry) -> Self { unimplemented!() }
    }

    #[macro_export] macro_rules! assert_block_eq {
        ($image:expr, $block_num:expr, $( $expected_and_mask:expr ),*) => {
            {
                let got = $image.read_blocks($block_num, 1).expect(&format!("block {}", $block_num));
                let mut expected_and_mask: Vec<u16> = Vec::new();
                $( {
                    expected_and_mask.extend($expected_and_mask.into_iter().map(|v| v as u16));
                } )*
                // We encode the inverse mask into the high byte of a u16--that way simple u8s are treated as 0xff.
                // The ____ const, below, sets the high byte of the mask so that is becomes 0x00 when we invert it.
                let expected: Vec<u8> = expected_and_mask.iter().map(|x| (x & 0xff) as u8).collect();
                let mask: Vec<u8> = expected_and_mask.iter().map(|x| !(x >> 8) as u8).collect();
                let masked: Vec<u8> = got.as_bytes().iter().zip(&mask).map(|(data, mask)| data & mask).collect();
                if masked != expected {
                    panic!("assertion `block {0} == expected` failed\n  Block {0} was:\n{1:?} \n Expected:\n{2:?}\n Mask:\n{3:?}",
                        $block_num, got.as_bytes().hex_dump(), expected.hex_dump(), mask.hex_dump());
                }
            }
        };
    }
    pub(crate) const ____: u16 = 0xff00;

    pub(crate) fn incrementing(count: usize) -> Vec<u8> {
        (0..count).map(|x| x as u8).collect::<Vec<u8>>()
    }
}
