// Copyright © 2023 David Caldwell <david@porkrind.org>

// Logical Devices
pub mod rx;
pub mod flat;

// Physical Images
pub mod img;
pub mod imd;

use bytebuffer::ByteBuffer;

pub const BLOCK_SIZE: usize = 512; // This seems baked into the format, and unrelated to sector size, interestingly (which is 128 bytes on an RX-01).

pub trait BlockDevice {
    fn read_blocks(&self, block: usize, count: usize) -> anyhow::Result<ByteBuffer> {
        let ssz = self.sector_size();
        let mut buf = vec![];
        for b in block*BLOCK_SIZE/ssz..(block+count)*BLOCK_SIZE/ssz {
            buf.extend(self.read_sector(b)?);
        }
        Ok(ByteBuffer::from_bytes(&buf))
    }

    fn write_blocks(&mut self, block: usize, blocks: usize, buf: &[u8]) -> anyhow::Result<()> {
        let ssz = self.sector_size();
        for s in 0..blocks*BLOCK_SIZE/ssz {
            self.write_sector(block*BLOCK_SIZE/ssz + s, &buf[s * ssz..(s+1) * ssz])?;
        }
        Ok(())
    }

    fn blocks(&self) -> usize {
        self.sectors() * self.sector_size() / BLOCK_SIZE
    }
    fn read_sector(&self, sector: usize) -> anyhow::Result<Vec<u8>>;
    fn write_sector(&mut self, sector: usize, buf: &[u8]) -> anyhow::Result<()>;
    fn sector_size(&self) -> usize;
    fn sectors(&self) -> usize;
    fn physical_device(&self) -> &impl PhysicalBlockDevice;
}

pub trait PhysicalBlockDevice {
    fn geometry(&self) -> &Geometry;
    fn total_bytes(&self) -> usize {
        let g = self.geometry();
        g.cylinders * g.heads * g.sectors * g.sector_size
    }
    fn read_sector(&self, cylinder: usize, head: usize, sector: usize) -> anyhow::Result<Vec<u8>>;
    fn write_sector(&mut self, cylinder: usize, head: usize, sector: usize, buf: &[u8]) -> anyhow::Result<()>;
    fn as_vec(&self) -> anyhow::Result<Vec<u8>>;

    fn from_raw(data: Vec<u8>, geometry: Geometry) -> Self;

    fn to_raw(&self) -> anyhow::Result<(Geometry, Vec<u8>)> {
        let g = self.geometry();
        let mut raw = Vec::with_capacity(g.cylinders * g.heads * g.sectors * g.sector_size);
        for c in 0..g.cylinders {
            for h in 0..g.heads {
                for s in 0..g.sectors {
                    raw.extend(self.read_sector(c,h,s)?.iter());
                }
            }
        }
        Ok((*g, raw))
    }
}

#[derive(Clone, Debug, Copy)]
pub struct Geometry {
    pub cylinders: usize,
    pub heads: usize,
    pub sectors: usize,
    pub sector_size: usize,
}
