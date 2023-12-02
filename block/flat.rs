// Copyright © 2023 David Caldwell <david@porkrind.org>

use super::{PhysicalBlockDevice, BlockDevice};

#[derive(Clone, Debug)]
pub struct Flat<B: PhysicalBlockDevice>(pub B);

impl<B: PhysicalBlockDevice> BlockDevice for Flat<B> {
    fn read_sector(&self, sector: usize) -> anyhow::Result<Vec<u8>> {
        let (c,h,s) = self.physical_from_logical(sector);
        self.0.read_sector(c,h,s)
    }

    fn write_sector(&mut self, sector: usize, buf: &[u8]) -> anyhow::Result<()> {
        let (c,h,s) = self.physical_from_logical(sector);
        self.0.write_sector(c,h,s, buf)
    }

    fn sector_size(&self) -> usize {
        self.0.geometry().sector_size
    }

    fn sectors(&self) -> usize {
        self.0.geometry().sectors()
    }

    fn physical_device(&self) -> Box<&dyn PhysicalBlockDevice> {
        Box::new(&self.0)
    }
}

impl<B: PhysicalBlockDevice> Flat<B> {
    pub fn physical_from_logical(&self, sector: usize) -> (usize/*Cylinder*/, usize/*Head*/, usize/*Sector*/) {
        let g = self.0.geometry();
        let c = sector / g.sectors / g.heads;
        let h = sector / g.sectors % g.heads;
        let s = sector % g.sectors;
        (c, h, s)
    }
}
