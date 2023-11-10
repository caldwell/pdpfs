// Copyright © 2023 David Caldwell <david@porkrind.org>

// Logical Devices
pub mod rx;

// Physical Images
pub mod img;
pub mod imd;

use bytebuffer::ByteBuffer;

pub const BLOCK_SIZE: usize = 512; // This seems baked into the format, and unrelated to sector size, interestingly (which is 128 bytes on an RX-01).

pub trait BlockDevice {
    fn block(&self, block: usize, count: usize) -> anyhow::Result<ByteBuffer> {
        let ssz = self.sector_size();
        let mut buf = vec![];
        for b in block*BLOCK_SIZE/ssz..(block+count)*BLOCK_SIZE/ssz {
            buf.extend(self.sector(b)?);
        }
        Ok(ByteBuffer::from_bytes(&buf))
    }

    fn sector(&self, sector: usize) -> anyhow::Result<Vec<u8>>;
    fn sector_size(&self) -> usize;
}

pub trait PhysicalBlockDevice {
    fn geometry(&self) -> &Geometry;
    fn sector(&self, cylinder: usize, head: usize, sector: usize) -> anyhow::Result<Vec<u8>>;
}

#[derive(Clone, Debug)]
pub struct Geometry {
    pub cylinders: usize,
    pub heads: usize,
    pub sectors: usize,
    pub sector_size: usize,
}
