// Copyright © 2023 David Caldwell <david@porkrind.org>

use anyhow::{anyhow, Context};
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

#[derive(Clone, Debug, Copy)]
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
            tracks.push(Track::from_repr(&mut buf).with_context(|| format!("Track {}", tracks.len()))?);
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

    pub fn repr(&self) -> anyhow::Result<Vec<u8>> {
        let mut buf = ByteBuffer::new();
        buf.write_bytes(&self.comment.as_bytes());
        buf.write_u8(0x1a); // comment terminator
        for t in self.track.iter() {
            buf.write_bytes(&t.repr()?);
        }
        Ok(buf.into_vec())
    }

    pub fn from_img(img: super::img::IMG)  -> IMD {
        let g = img.geometry().clone();
        IMD {
            comment: format!("IMD 1.18: {}\nConverted from IMG by rt11fs[1]\n[1]: https://porkrind.org/rt11fs\n",
                chrono::Local::now().format("%m/%d/Y %H:%M:%S")),
            track: (0..g.cylinders).map(|c| {
                let c = c;
                (0..g.heads).map(move |h| {
                    Track {
                        mode: Mode::M250kbitsFM, // FIXME!!! Don't hardcode this!
                        cylinder: c as u8,
                        head: h as u8,
                        sector_count: g.sectors as u8,
                        sector_size: g.sector_size,
                        sector_map: (0..g.sectors).map(|s| (s+1) as u8).collect(),
                        sector_data: (0..g.sectors).map(|_| {
                            Sector {
                                data: SectorData::Compressed(0, g.sector_size),
                                deleted: false,
                                error: false,
                            }
                        }).collect(),
                    }
                })
            }).flatten().collect(),
            geometry: g,
        }
    }
}

impl Track {
    pub fn from_repr(buf: &mut ByteBuffer) -> anyhow::Result<Track> {
            let sector_count;
            let sector_size;
                Ok(Track {
                    mode: match buf.read_u8()? {
                        0 => Mode::M500kbitsFM,
                        1 => Mode::M300kbitsFM,
                        2 => Mode::M250kbitsFM,
                        3 => Mode::M500kbitsMFM,
                        4 => Mode::M300kbitsMFM,
                        5 => Mode::M250kbitsMFM, m => Err(anyhow!("Bad mode: {:02x}", m))?,
                    },
                    cylinder: buf.read_u8()?,
                    head: match buf.read_u8()? {
                        1 => 1,
                        0 => 0,
                        h if h & 0b1000_0000 != 0 => Err(anyhow!("TODO: support Sector Cylinder Map"))?,
                        h if h & 0b0100_0000 != 0 => Err(anyhow!("TODO: support Sector head Map"))?,
                        h => Err(anyhow!("Bad head: {:02x}", h))?,
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
                        s => Err(anyhow!("Bad sector size: {:02x}", s))?,
                    }; sector_size},
                    sector_map: buf.read_bytes(sector_count as usize)?,
                    sector_data: (0..sector_count).map(|_| -> anyhow::Result<Sector> {
                        Sector::from_repr(buf, sector_size)
                    }).collect::<anyhow::Result<Vec<Sector>>>()?,
                })
    }

    pub fn repr(&self) -> anyhow::Result<Vec<u8>> {
        let mut buf = ByteBuffer::new();
        buf.write_u8(self.mode as u8);
        buf.write_u8(self.cylinder);
        buf.write_u8(self.head);
        buf.write_u8(self.sector_count);
        buf.write_u8(match self.sector_size {
                         128 => 0,
                         256 => 1,
                         512 => 2,
                        1024 => 3,
                        2048 => 4,
                        4096 => 5,
                        8192 => 6,
                        s => Err(anyhow!("Bad sector size: {:02x}", s))?});
        buf.write_bytes(&self.sector_map);
        for s in self.sector_data.iter() {
            buf.write_bytes(&s.repr()?);
        }
        Ok(buf.into_vec())
    }
}

impl Sector {
    pub fn from_repr(buf: &mut ByteBuffer, sector_size: usize) -> anyhow::Result<Sector> {
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
            t => Err(anyhow!("Bad sector type: {:02x}", t))?,
        })
    }
    pub fn repr(&self) -> anyhow::Result<Vec<u8>> {
        let mut buf = ByteBuffer::new();
        buf.write_u8(match self {
            Sector { deleted: false, error: false, data: SectorData::Unavailable }      => 0,
            Sector { deleted: false, error: false, data: SectorData::Normal(_) }        => 1,
            Sector { deleted: false, error: false, data: SectorData::Compressed(_, _) } => 2,
            Sector { deleted: true,  error: false, data: SectorData::Normal(_) }        => 3,
            Sector { deleted: true,  error: false, data: SectorData::Compressed(_, _) } => 4,
            Sector { deleted: false, error: true,  data: SectorData::Normal(_) }        => 5,
            Sector { deleted: false, error: true,  data: SectorData::Compressed(_, _) } => 6,
            Sector { deleted: true,  error: true,  data: SectorData::Normal(_) }        => 7,
            Sector { deleted: true,  error: true,  data: SectorData::Compressed(_, _) } => 8,
            _ => Err(anyhow!("Can't represent sector! {:?}", self))?,
        });
        match self.data {
            SectorData::Unavailable         => {},
            SectorData::Normal(ref data)    => buf.write_bytes(&data),
            SectorData::Compressed(data, _) => buf.write_u8(data),
        }
        Ok(buf.into_vec())
    }

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
    fn read_sector(&self, cylinder: usize, _head: usize, sector: usize) -> anyhow::Result<Vec<u8>> {
        Ok(self.track[cylinder].sector_data[self.track[cylinder].sector_map[sector] as usize - 1].as_bytes()?)
    }
    fn geometry(&self) -> &Geometry {
        &self.geometry
    }
    fn write_sector(&mut self, cylinder: usize, head: usize, sector: usize, buf: &[u8]) -> anyhow::Result<()> {
        let new_sector = Sector {
            deleted: false,
            error: false,
            data: if buf.iter().all(|b| *b==buf[0]) {
                SectorData::Compressed(buf[0], self.track[cylinder].sector_size)
            } else {
                SectorData::Normal(buf.to_owned())
            },
        };
        let raw_sector_num = self.track[cylinder].sector_map[sector] as usize - 1;
        self.track[cylinder].sector_data[raw_sector_num] = new_sector;
        Ok(())
    }
    fn as_vec(&self) -> anyhow::Result<Vec<u8>> {
        self.repr()
    }
}
