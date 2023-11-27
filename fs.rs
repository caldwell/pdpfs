// Copyright © 2023 David Caldwell <david@porkrind.org>

pub mod rt11;

use std::{ops::{Deref, DerefMut}, fmt::Debug};

use bytebuffer::ByteBuffer;

use crate::block::BlockDevice;

pub trait FileSystem : Send + Sync {
    type BlockDevice: BlockDevice;

    fn dir_iter<'a>(&'a self, path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn DirEntry + 'a>> + 'a>>;
    fn read_dir<'a>(&'a self, path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn DirEntry + 'a>> + 'a>>;
    fn stat<'a>(&'a self, name: &str) -> Option<Box<dyn DirEntry + 'a>>;
    fn free_blocks(&self) -> usize;
    fn used_blocks(&self) -> usize;
    fn read_file(&self, name: &str) -> anyhow::Result<ByteBuffer>;
    fn write_file(&mut self, name: &str, contents: &Vec<u8>) -> anyhow::Result<()>;
    fn delete(&mut self, name: &str) -> anyhow::Result<()>;
    fn rename(&mut self, src: &str, dest: &str) -> anyhow::Result<()>;
    fn block_device(&self) -> &Self::BlockDevice;
}

// It's really a shame this isn't automatic or derivable or something.
impl<B: BlockDevice+Send+Sync> FileSystem for Box<dyn FileSystem<BlockDevice = B>> {
    type BlockDevice = B;
    fn dir_iter<'a>(&'a self, path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn DirEntry + 'a>> + 'a>> { self.deref().dir_iter(&path) }
    fn read_dir<'a>(&'a self, path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn DirEntry + 'a>> + 'a>> { self.deref().read_dir(&path) }
    fn stat<'a>(&'a self, name: &str) -> Option<Box<dyn DirEntry + 'a>> { self.deref().stat(name) }
    fn free_blocks(&self) -> usize { self.deref().free_blocks() }
    fn used_blocks(&self) -> usize { self.deref().used_blocks() }
    fn read_file(&self, name: &str) -> anyhow::Result<ByteBuffer> { self.deref().read_file(name) }
    fn write_file(&mut self, name: &str, contents: &Vec<u8>) -> anyhow::Result<()> { self.deref_mut().write_file(name, contents) }
    fn delete(&mut self, name: &str) -> anyhow::Result<()> { self.deref_mut().delete(name) }
    fn rename(&mut self, src: &str, dest: &str) -> anyhow::Result<()> { self.deref_mut().rename(src, dest) }
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
