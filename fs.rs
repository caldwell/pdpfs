// Copyright © 2023 David Caldwell <david@porkrind.org>

pub mod rt11;

use std::ops::{Deref, DerefMut};

use bytebuffer::ByteBuffer;

use crate::block::BlockDevice;
use rt11::{DirEntryIterator,RT11FileWriter,DirEntry};

pub trait FileSystem : Send + Sync {
    type BlockDevice: BlockDevice;

    fn dir_iter<'a>(&'a self) -> DirEntryIterator<'a, Self::BlockDevice>;
    fn file_iter<'a>(&'a self) -> std::iter::Filter<DirEntryIterator<'a, Self::BlockDevice>, Box<dyn FnMut(&&'a DirEntry) -> bool>>;
    fn stat<'a>(&'a self, name: &str) -> Option<&'a DirEntry>;
    fn free_blocks(&self) -> usize;
    fn used_blocks(&self) -> usize;
    fn read_file(&self, name: &str) -> anyhow::Result<ByteBuffer>;
    fn create<'a>(&'a mut self, name: &str, bytes: usize) -> anyhow::Result<RT11FileWriter<'a, Self::BlockDevice>>;
    fn delete(&mut self, name: &str) -> anyhow::Result<()>;
    fn rename(&mut self, src: &str, dest: &str) -> anyhow::Result<()>;
    fn block_device(&self) -> &Self::BlockDevice;
}

// It's really a shame this isn't automatic or derivable or something.
impl<B: BlockDevice+Send+Sync> FileSystem for Box<dyn FileSystem<BlockDevice = B>> {
    type BlockDevice = B;
    fn dir_iter<'a>(&'a self) -> DirEntryIterator<'a, Self::BlockDevice> { self.deref().dir_iter() }
    fn file_iter<'a>(&'a self) -> std::iter::Filter<DirEntryIterator<'a, Self::BlockDevice>, Box<dyn FnMut(&&'a DirEntry) -> bool>> { self.deref().file_iter() }
    fn stat<'a>(&'a self, name: &str) -> Option<&'a DirEntry> { self.deref().stat(name) }
    fn free_blocks(&self) -> usize { self.deref().free_blocks() }
    fn used_blocks(&self) -> usize { self.deref().used_blocks() }
    fn read_file(&self, name: &str) -> anyhow::Result<ByteBuffer> { self.deref().read_file(name) }
    fn create<'a>(&'a mut self, name: &str, bytes: usize) -> anyhow::Result<RT11FileWriter<'a, Self::BlockDevice>> { self.deref_mut().create(name, bytes) }
    fn delete(&mut self, name: &str) -> anyhow::Result<()> { self.deref_mut().delete(name) }
    fn rename(&mut self, src: &str, dest: &str) -> anyhow::Result<()> { self.deref_mut().rename(src, dest) }
    fn block_device(&self) -> &B { self.deref().block_device() }
}
