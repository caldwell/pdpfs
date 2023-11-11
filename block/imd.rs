// Copyright © 2023 David Caldwell <david@porkrind.org>

use anyhow::anyhow;
use bytebuffer::ByteBuffer;


use super::{PhysicalBlockDevice, Geometry};

#[derive(Clone, Debug)]
pub struct IMD {
    pub comment: String,
    pub track: Vec<Track>,
    pub geometry: Geometry,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub mode: Mode,
    pub cylinder: u8,
    pub head: u8,
    pub sector_count: u8,
    pub sector_size: usize,
    pub sector_map: Vec<u8>,
    pub sector_data: Vec<Sector>,
}

#[derive(Clone, Debug)]
#[repr(u8)]
pub enum Mode {
    M500kbitsFM  = 0,
    M300kbitsFM  = 1,
    M250kbitsFM  = 2,
    M500kbitsMFM = 3,
    M300kbitsMFM = 4,
    M250kbitsMFM = 5,
}

#[derive(Clone, Debug)]
pub struct Sector {
    data: SectorData,
    deleted: bool,
    error: bool,
}

#[derive(Clone, Debug)]
pub enum SectorData {
    Unavailable,
    Normal(Vec<u8>),
    Compressed(u8, usize),
}

impl IMD {
    pub fn from_bytes(image: &[u8]) -> anyhow::Result<IMD> {
        let mut buf = ByteBuffer::from_bytes(image);

        let comment_len = buf.as_bytes().iter().enumerate().find(|(_,x)| **x==0x1a).ok_or(anyhow!("Couldn't find comment terminator (0x1a)"))?.0;
        let comment = String::from_utf8(buf.read_bytes(comment_len)?)?;

        assert_eq!(buf.read_u8()?, 0x1a);

        let mut tracks: Vec<Track> = vec![];
        while buf.get_rpos() < buf.len() {
            let sector_count;
            let sector_size;
            tracks.push(
                Track {
                    mode: match buf.read_u8()? {
                        0 => Mode::M500kbitsFM,
                        1 => Mode::M300kbitsFM,
                        2 => Mode::M250kbitsFM,
                        3 => Mode::M500kbitsMFM,
                        4 => Mode::M300kbitsMFM,
                        5 => Mode::M250kbitsMFM,
                        m => Err(anyhow!("Bad mode: {:02x} at track {}", m, tracks.len()))?,
                    },
                    cylinder: buf.read_u8()?,
                    head: match buf.read_u8()? {
                        1 => 1,
                        0 => 0,
                        h if h & 0b1000_0000 != 0 => Err(anyhow!("TODO: support Sector Cylinder Map (track {})", tracks.len()))?,
                        h if h & 0b0100_0000 != 0 => Err(anyhow!("TODO: support Sector head Map (track {})", tracks.len()))?,
                        h => Err(anyhow!("Bad head: {:02x} at track {}", h, tracks.len()))?,
                    },
                    sector_count: {sector_count = buf.read_u8()?; sector_count},
                    sector_size: {sector_size = match buf.read_u8()? {
                        0 =>  128,
                        1 =>  256,
                        2 =>  512,
                        3 => 1024,
                        4 => 2048,
                        5 => 4096,
                        6 => 8192,
                        s => Err(anyhow!("Bad sector size: {:02x} at track {}", s, tracks.len()))?,
                    }; sector_size},
                    sector_map: buf.read_bytes(sector_count as usize)?,
                    sector_data: (0..sector_count).map(|_| -> anyhow::Result<Sector> {
                        Ok(match buf.read_u8()? {
                            0 => Sector { deleted: false, error: false, data: SectorData::Unavailable },
                            1 => Sector { deleted: false, error: false, data: SectorData::Normal(buf.read_bytes(sector_size)?) },
                            2 => Sector { deleted: false, error: false, data: SectorData::Compressed(buf.read_u8()?, sector_size) },
                            3 => Sector { deleted: true,  error: false, data: SectorData::Normal(buf.read_bytes(sector_size)?) },
                            4 => Sector { deleted: true,  error: false, data: SectorData::Compressed(buf.read_u8()?, sector_size) },
                            5 => Sector { deleted: false, error: true,  data: SectorData::Normal(buf.read_bytes(sector_size)?) },
                            6 => Sector { deleted: false, error: true,  data: SectorData::Compressed(buf.read_u8()?, sector_size) },
                            7 => Sector { deleted: true,  error: true,  data: SectorData::Normal(buf.read_bytes(sector_size)?) },
                            8 => Sector { deleted: true,  error: true,  data: SectorData::Compressed(buf.read_u8()?, sector_size) },
                            t => Err(anyhow!("Bad sector type: {:02x} at track {}", t, tracks.len()))?,
                        })
                    }).collect::<anyhow::Result<Vec<Sector>>>()?,
                }
            );
        }

        Ok(IMD {
            comment: comment,
            geometry: Geometry { // This isn't really important to the IMD format itself, but PhysicalBlockDevice needs it and traits can't add data to structs :-(
                cylinders: tracks.len(),
                heads: if tracks.iter().find(|t| t.head == 1).is_none() { 1 } else { 2 },
                sectors: tracks[0].sector_count as usize,
                sector_size: tracks[0].sector_size,
            },
            track: tracks,
        })
    }
}

impl Sector {
    pub fn as_bytes(&self) -> anyhow::Result<Vec<u8>> {
        if self.deleted { Err(anyhow!("Reading deleted sector"))? }
        if self.error { Err(anyhow!("Reading sector with data error"))? }
        match (self.deleted, self.error, &self.data) {
            (true, _, _) => Err(anyhow!("Reading deleted sector"))?,
            (_,true, _) => Err(anyhow!("Reading sector with data error"))?,
            (false, false, SectorData::Unavailable) => Err(anyhow!("Reading unavailable sector"))?,
            (false, false, SectorData::Normal(data)) => Ok(data.clone()),
            (false, false, SectorData::Compressed(val, count)) => Ok(vec![*val; *count]),
        }
    }
}

impl PhysicalBlockDevice for IMD {
    fn sector(&self, cylinder: usize, _head: usize, sector: usize) -> anyhow::Result<Vec<u8>> {
        Ok(self.track[cylinder].sector_data[self.track[cylinder].sector_map[sector] as usize - 1].as_bytes()?)
    }
    fn geometry(&self) -> &Geometry {
        &self.geometry
    }
    fn write_sector(&mut self, cylinder: usize, head: usize, sector: usize, buf: &[u8]) -> anyhow::Result<()> {
        use pretty_hex::PrettyHex;
        println!("Writing CHS({},{},{}):\n{:?}", cylinder, head, sector, buf.hex_dump());
        todo!();
        //Ok(())
    }
    fn as_vec(&self) -> Vec<u8> {
        todo!()
    }
}
