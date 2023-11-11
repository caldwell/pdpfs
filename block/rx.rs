// Copyright © 2023 David Caldwell <david@porkrind.org>

use super::{Geometry, PhysicalBlockDevice, BlockDevice};

pub const RX01_GEOMETRY: Geometry = Geometry {
    cylinders: 77,
    heads: 1,
    sectors: 26,
    sector_size: 128,
};

pub const RX02_GEOMETRY: Geometry = Geometry {
    cylinders: 77,
    heads: 1,
    sectors: 26,
    sector_size: 256,
};

pub struct RX<B: PhysicalBlockDevice>(pub B);

impl<B: PhysicalBlockDevice> BlockDevice for RX<B> {
    fn sector(&self, sector: usize) -> anyhow::Result<Vec<u8>> {
        let (c,h,s) = self.physical_from_logical(sector);
        self.0.sector(c+1,h,s) // RT-11 skips track 0 on RX devices (for IBM interchange compatibility)
    }

    fn sector_size(&self) -> usize {
        self.0.geometry().sector_size
    }

    fn sectors(&self) -> usize {
        let g = self.0.geometry();
        (g.cylinders - 1) * g.heads * g.sectors // don't include track 0 in the sector count (see above)
    }

    fn physical_device(&self) -> &impl PhysicalBlockDevice {
        &self.0
    }
}

impl<B: PhysicalBlockDevice> RX<B> {
    pub fn physical_from_logical(&self, sector: usize) -> (usize/*Cylinder*/, usize/*Head*/, usize/*Sector*/) {
        let g = self.0.geometry();
        // RT-11 interleaves floppy sectors in the RX-01 driver. (They are _not_ physically interleaved on the
        // disk--that is, the format has the physical blocks labelled in a non-interleaved fashion and RT-11
        // does the interleaving in the software layer).
        let cyl = sector / g.sectors;
        let mut sec = sector % g.sectors;
        sec *= 2; // 2:1 interleave
        sec += if sec >= g.sectors { 1 } else { 0 } + cyl * 6 /* 6 block skew per track */;
        sec %= g.sectors;
        (cyl, 0, sec)
    }
}


