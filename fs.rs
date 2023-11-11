// Copyright © 2023 David Caldwell <david@porkrind.org>

use anyhow::{Context,anyhow};
use bytebuffer::{Endian, ByteBuffer};
use chrono::NaiveDate;

use crate::block::BlockDevice;

#[derive(Clone, Debug)]
pub struct RT11FS<B: BlockDevice> {
    pub image: B,
    pub home: HomeBlock,
    pub dir: Vec<DirSegment>,
}

#[derive(Clone, Debug)]
pub struct HomeBlock {
    pub bad_block_replacement_table: [u8; 130],
    pub init_restore: [u8; 38],
    pub bup_volume: Option<u8>,
    pub pack_cluster_size: u16,
    pub directory_start_block: u16,
    pub system_version: String,
    pub volume_id: String,
    pub owner_name: String,
    pub system_id: String,
}

impl<B: BlockDevice> RT11FS<B> {
    pub fn new(image: B) -> anyhow::Result<RT11FS<B>> {
        let home = Self::read_homeblock(&image)?;
        let dir = Self::read_directory(&image, &home)?;
        Ok(RT11FS {
            image,
            home,
            dir,
        })
    }

    pub fn read_homeblock(image: &B) -> anyhow::Result<HomeBlock> {
        let mut buf = image.block(1, 1)?;
        buf.set_endian(Endian::LittleEndian);

        let computed_sum = {
            let mut sum=0u16;
            for _ in 0..255 {
                sum = sum.wrapping_add(buf.read_u16()?);
            }
            sum
        };

        buf.set_rpos(0);
        let hb = HomeBlock {
            bad_block_replacement_table: buf.read_bytes(0o202)?.try_into().unwrap(),
            init_restore: buf.read_bytes(0o252-0o204)?.try_into().unwrap(),
            bup_volume: match String::from_utf8(buf.read_bytes(0o266 - 0o252)?) {
                Ok(s) if s == "BUQ         " => Some(buf.read_u8()?), // what about 0o267-0o273??
                _ => None,
            },
            pack_cluster_size: { buf.set_rpos(0o722); buf.read_u16()? },
            directory_start_block: buf.read_u16()?,
            system_version: radix50::pdp11::decode(&[buf.read_u16()?]),
            volume_id: {
                let b = buf.read_bytes(0o744 - 0o730)?;
                String::from_utf8(b.clone()).with_context(|| format!("volume id {:?}", b))? },
            owner_name: String::from_utf8(buf.read_bytes(0o760 - 0o744)?).with_context(|| "owner name")?,
            system_id: String::from_utf8(buf.read_bytes(0o774 - 0o760)?).with_context(|| "system id")?,
        };

        assert_eq!(0o774, buf.get_rpos());
        buf.set_rpos(0o776);
        let expected = buf.read_u16().with_context(|| format!("checksum"))?;
        if computed_sum != expected {
            println!("Warning: Bad checksum: computed ({:04x}) != on disk ({:04x})", computed_sum, expected);
            // Really should be this, but _every_ disk image I've tried has a checksum error, so maybe no one uses it (or I calculate it incorrectly?):
            // return Err(anyhow!("Bad checksum: computed ({:04x}) != on disk ({:04x})", computed_sum, expected));
        }
        Ok(hb)
    }

    fn read_directory(image: &B, home: &HomeBlock) -> anyhow::Result<Vec<DirSegment>> {
        let mut segments = vec![];
        let mut segment = home.directory_start_block;
        while segment != 0 {
            let next = DirSegment::from_repr(segment, image.block(segment as usize, 2)?)
                .with_context(|| format!("Bad Directory Segment #{} (@ {})", segments.len(), segment))?;
            segment = next.next_segment;
            segments.push(next);
        }
        Ok(segments)
    }

    pub fn dir_iter<'a>(&'a self) -> DirEntryIterator<'a, B> {
        DirEntryIterator {
            fs: self,
            segment: 0,
            entry: 0,
        }
    }

    pub fn file_iter<'a>(&'a self) -> impl Iterator<Item = &'a DirEntry> + 'a {
        self.dir_iter().filter(|e| e.kind == EntryKind::Permanent)
    }

    pub fn file_named<'a>(&'a self, name: &str) -> Option<&'a DirEntry> {
        self.file_iter().find(|f| f.name == name)
    }

    pub fn free_blocks(&self) -> usize {
        self.dir_iter().filter(|e| e.kind == EntryKind::Empty).fold(0, |acc, e| acc + e.length)
    }

    pub fn used_blocks(&self) -> usize {
        self.dir_iter().filter(|e| e.kind != EntryKind::Empty).fold(0, |acc, e| acc + e.length)
    }
}

const STATUS_E_TENT: u16 = 0o000400;
const STATUS_E_MPTY: u16 = 0o001000;
const STATUS_E_PERM: u16 = 0o002000;
const STATUS_E_EOS:  u16 = 0o004000;
const STATUS_E_READ: u16 = 0o040000;
const STATUS_E_PROT: u16 = 0o100000;
const STATUS_E_PRE:  u16 = 0o000020;

#[derive(Clone, Debug)]
pub struct DirSegment {
    pub segments: u16,
    pub next_segment: u16,
    pub last_segment: u16,
    pub extra_bytes: u16,
    pub data_block: u16,
    pub entries: Vec<DirEntry>,

    // Not part of the format.
    pub block: u16, // The block number of _this_ segment
}

impl DirSegment {
    pub fn from_repr(my_block: u16, mut buf: ByteBuffer) -> anyhow::Result<DirSegment> {
        buf.set_endian(Endian::LittleEndian);
        let extra_bytes;
        let data_block;
        Ok(DirSegment {
            block: my_block,
            segments: buf.read_u16()?,
            next_segment: buf.read_u16()?,
            last_segment: buf.read_u16()?,
            extra_bytes: { extra_bytes = buf.read_u16()?;
                           if extra_bytes & 1 == 1 { return Err(anyhow!("Image has odd number of extra bytes: {}", extra_bytes)) }
                           extra_bytes },
            data_block: { data_block = buf.read_u16()?; data_block },
            entries: {
                let mut entries = vec![];
                let mut block = data_block as usize;
                while let Some(entry) = DirEntry::from_repr(block, extra_bytes, &mut buf)? {
                    block += entry.length;
                    entries.push(entry);
                }
                if entries.len() < 2 { return Err(anyhow!("Too few directory entries: {} (should be >=2)", entries.len())) }
                entries
            },
        })
    }
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub kind: EntryKind,
    pub read_only: bool,
    pub protected: bool,
    pub prefix_block: bool,
    pub name: String,
    pub length: usize,
    pub job: u8,
    pub channel: u8,
    pub creation_date: Option<NaiveDate>,
    pub extra: Vec<u16>,

    // Not part of the on-disk structure. Precalculated for convenience.
    pub block: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntryKind {
    Tentative,
    Empty,
    Permanent,
}

impl DirEntry {
    pub fn from_repr(data_block: usize, extra_bytes: u16, buf: &mut ByteBuffer) -> anyhow::Result<Option<DirEntry>> {
        let status = buf.read_u16()?;
        let length;
        Ok(Some(DirEntry {
            kind: match status {
                status if status & STATUS_E_EOS  != 0 => return Ok(None), // end of segment marker
                status if status & STATUS_E_TENT != 0 => EntryKind::Tentative,
                status if status & STATUS_E_MPTY != 0 => EntryKind::Empty,
                status if status & STATUS_E_PERM != 0 => EntryKind::Permanent,
                status => Err(anyhow!("Bad status {:06o}", status))?,
            },
            read_only: status & STATUS_E_READ != 0,
            protected: status & STATUS_E_PROT != 0,
            prefix_block: status & STATUS_E_PRE != 0,
            name: {
                let raw = radix50::pdp11::decode(&[buf.read_u16()?, buf.read_u16()?, buf.read_u16()?]);
                let (name, ext) = raw.split_at(6);
                format!("{}.{}", name.trim(), ext.trim())
            },
            length: { length = buf.read_u16()? as usize; length },
            job: buf.read_u8()?,
            channel: buf.read_u8()?,
            creation_date: DirEntry::decode_date(buf.read_u16()?)?,
            extra: (0..extra_bytes/2).map(|_| -> anyhow::Result<u16> { Ok(buf.read_u16()?) }).collect::<anyhow::Result<Vec<u16>>>()?,

            // Pre-compute block addresses of files for convenience
            block: data_block,
        }))
    }

    pub fn decode_date(raw: u16) -> anyhow::Result<Option<NaiveDate>> {
        let (age, month, day, year) = (((raw & 0b11_0000_00000_00000) >> 14) as i32,
                                       ((raw & 0b00_1111_00000_00000) >> 10) as u32,
                                       ((raw & 0b00_0000_11111_00000) >>  5) as u32,
                                       ((raw & 0b00_0000_00000_11111) >>  0) as i32);
        Ok(match raw {
            0 => None,
            _ => Some(chrono::NaiveDate::from_ymd_opt(1972 + year + age * 32, month, day)
                          .ok_or(anyhow!("Invalid date: {:04}-{:02}-{:02} [{}/{:#06x}/{:#018b}]", year, month, day, raw, raw, raw))?),
           })
    }
}

pub struct DirEntryIterator<'a, B: BlockDevice> {
    fs: &'a RT11FS<B>,
    segment: usize,
    entry: usize,
}

impl<'a, B: BlockDevice> Iterator for DirEntryIterator<'a, B> {
    type Item = &'a DirEntry;
    fn next(&mut self) -> Option<Self::Item> {
        if self.segment >= self.fs.dir.len() { return None }
        let entry = &self.fs.dir[self.segment].entries[self.entry];
        self.entry += 1;
        if self.entry >= self.fs.dir[self.segment].entries.len() {
            self.segment += 1;
            self.entry = 0;
        }
        Some(entry)
    }
}
