// Copyright © 2023 David Caldwell <david@porkrind.org>

use super::{Geometry, PhysicalBlockDevice};

pub struct IMG {
    pub data: Vec<u8>,
    pub geometry: Geometry,
}

impl IMG {
    pub fn from_bytes(image: &[u8], geometry: Geometry) -> IMG {
        Self::from_vec(image.to_owned(), geometry)
    }
    pub fn from_vec(image: Vec<u8>, geometry: Geometry) -> IMG {
        IMG { data: image, geometry }
    }
}

impl PhysicalBlockDevice for IMG {
    fn sector(&self, cylinder: usize, head: usize, sector: usize) -> anyhow::Result<Vec<u8>> {
        let start = cylinder * self.geometry.sectors * self.geometry.heads
                          + head   * self.geometry.sectors
                          + sector;
        Ok(self.data[start*self.geometry.sector_size..(start + 1)*self.geometry.sector_size].to_owned())
    }

    fn geometry(&self) -> &Geometry {
        &self.geometry
    }
}
