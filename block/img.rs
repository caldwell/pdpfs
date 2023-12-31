// Copyright © 2023 David Caldwell <david@porkrind.org>

use super::{Geometry, PhysicalBlockDevice};

use anyhow::anyhow;

#[derive(Clone)]
pub struct IMG {
    pub data: Vec<u8>,
    pub geometry: Geometry,
}

impl IMG {
    pub fn from_vec(image: Vec<u8>, geometry: Geometry) -> IMG {
        IMG { data: image, geometry }
    }
}

impl PhysicalBlockDevice for IMG {
    fn from_raw(data: Vec<u8>, geometry: Geometry) -> Self {
        Self::from_vec(data, geometry)
    }

    fn read_sector(&self, cylinder: usize, head: usize, sector: usize) -> anyhow::Result<Vec<u8>> {
        let start = cylinder * self.geometry.sectors * self.geometry.heads
                          + head   * self.geometry.sectors
                          + sector;
        let start_bytes = start*self.geometry.sector_size;
        if start_bytes >= self.data.len() || start_bytes + self.geometry.sector_size > self.data.len() {
            return Err(anyhow!("Read past end of device {}..{} > {}", start, start+1, self.data.len() / self.geometry.sector_size))
        }
        Ok(self.data[start_bytes..start_bytes + self.geometry.sector_size].to_owned())
    }

    fn write_sector(&mut self, cylinder: usize, head: usize, sector: usize, buf: &[u8]) -> anyhow::Result<()> {
        if buf.len() != self.geometry.sector_size { return Err(anyhow!("Sector {}: Can't write partial sector ({} len)", sector, buf.len())) }
        let start = cylinder * self.geometry.sectors * self.geometry.heads
                          + head   * self.geometry.sectors
                          + sector;
        let start_bytes = start*self.geometry.sector_size;
        if start_bytes >= self.data.len() || start_bytes + self.geometry.sector_size > self.data.len() {
            return Err(anyhow!("Write past end of device {}..{} > {}", start, start+1, self.data.len() / self.geometry.sector_size))
        }
        self.data.splice(start_bytes..start_bytes + self.geometry.sector_size,
            buf.into_iter().map(|b| *b));
        Ok(())
    }

    fn geometry(&self) -> &Geometry {
        &self.geometry
    }

    fn as_vec(&self) -> anyhow::Result<Vec<u8>> {
        Ok(self.data.clone())
    }
}
