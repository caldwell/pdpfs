// Copyright © 2023 David Caldwell <david@porkrind.org>

use std::{cmp::min, io, io::ErrorKind};

use anyhow::{Context,anyhow};
use bytebuffer::{Endian, ByteBuffer};
use chrono::NaiveDate;

use crate::block::{BlockDevice, BLOCK_SIZE};

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

    pub fn find_empty_space<'a>(&'a self, blocks: usize) -> Option<(usize, usize)> {
        for (s, seg) in self.dir.iter().enumerate() {
            for (e, f) in seg.entries.iter().enumerate() {
                if f.kind != EntryKind::Empty || f.length < blocks { continue }
                return Some((s, e))
            }
        }
        None
    }

    pub fn create<'a>(&'a mut self, name: &str, bytes: usize) -> anyhow::Result<RT11FileWriter<'a, B>> {
        let blocks = (bytes + BLOCK_SIZE - 1) / BLOCK_SIZE;
        DirEntry::encode_filename(name)?;
        let Some((segment, entry)) = self.find_empty_space(blocks) else { return Err(anyhow!("No space available in image")) };
        if self.dir[segment].entries.len() + 1 > self.dir[segment].max_entries() {
            // Too many entries to fit in segment.
            // We _should_ split the segment here but I don't want to write that yet :-)
            return Err(anyhow!("No more entries in the segment and splitting segments is not implemented yet!"));
        }
        let mut new_free = self.dir[segment].entries[entry].clone();
        self.dir[segment].entries[entry].name = name.to_owned();
        self.dir[segment].entries[entry].length = blocks;
        self.dir[segment].entries[entry].kind = EntryKind::Permanent;
        self.dir[segment].entries[entry].read_only = false;
        self.dir[segment].entries[entry].protected = false;
        self.dir[segment].entries[entry].job = 0;
        self.dir[segment].entries[entry].channel = 0;
        self.dir[segment].entries[entry].creation_date = Some(chrono::Local::now().date_naive());
        new_free.block += blocks;
        new_free.length -= blocks;
        self.dir[segment].entries.insert(entry+1, new_free);
        self.image.write_blocks(self.dir[segment].block as usize, 2, &self.dir[segment].repr()?)?;
        Ok(RT11FileWriter{
            image: &mut self.image,
            direntry: &self.dir[segment].entries[entry],
            residue: vec![],
            pos: 0,
        })
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

    pub fn repr(&self) -> anyhow::Result<[u8; 2 * BLOCK_SIZE]> {
        let mut repr = ByteBuffer::new();
        repr.set_endian(Endian::LittleEndian);
        repr.write_u16(self.segments);
        repr.write_u16(self.next_segment);
        repr.write_u16(self.last_segment);
        repr.write_u16(self.extra_bytes);
        repr.write_u16(self.data_block);
        for entry in self.entries.iter() {
            repr.write_bytes(&entry.repr()?);
        }
        repr.write_u16(STATUS_E_EOS);
        repr.resize(2 * BLOCK_SIZE);
        Ok(repr.as_bytes().try_into()?)
    }

    fn max_entries(&self) -> usize {
        const SEGMENT_BLOCKS: usize = 2;
        const SEGMENT_HEADER_BYTES: usize = std::mem::size_of::<[u16; 5]>();
        const DIR_ENTRY_BYTES: usize = std::mem::size_of::<[u16; 7]>();
        const SEGMENT_END_MARKER_BYTES: usize = std::mem::size_of::<u16>();
        const RESERVED_ENTRIES: usize = 3 - 1; // See NOTE, below.
        // This is slightly more complicated than you'd expect because:
        //   a) each segment is allowed to have extra bytes per dir entry
        //   b) the end of segment marker doesn't have to have a full
        //      directory entry's worth of space--it only needs 1 word
        // Each segment is defined as 2 blocks, the contents of which are:
        //   segment_header + dir_entries[N] + segment_end_marker

        // NOTE: The "RT–11 Volume and File Formats Manual" says in section
        // 1.1.4 to reserve 3 directory entries when calculating the max
        // number--however, one of those is the end-of-segment marker. This
        // would mean that the end-of-segment marker consumes an entire
        // directory entries worth of bytes. However, in Table 1-3 of
        // section 1.1.2.2 it says "Note that an end-of-segment marker can
        // appear as the last word of a segment."  That means their
        // calculations in section 1.1.4 are slightly off. I've tweaked the
        // calculation to account for the short end-of-marker entry by
        // subtracting it off the top and then only having 2 reserved
        // entries. I believe this is more correct.
        (BLOCK_SIZE * SEGMENT_BLOCKS - SEGMENT_HEADER_BYTES - SEGMENT_END_MARKER_BYTES) / (DIR_ENTRY_BYTES + self.extra_bytes as usize) - RESERVED_ENTRIES
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

    pub fn repr(&self) -> anyhow::Result<Vec<u8>> {
        let mut repr = ByteBuffer::new();
        repr.set_endian(Endian::LittleEndian);
        repr.write_u16(0 | match self.kind {
                               EntryKind::Empty     => STATUS_E_MPTY,
                               EntryKind::Tentative => STATUS_E_TENT,
                               EntryKind::Permanent => STATUS_E_PERM,
                           }
                         | if self.read_only    { STATUS_E_READ } else { 0 }
                         | if self.read_only    { STATUS_E_PROT } else { 0 }
                         | if self.prefix_block { STATUS_E_PRE  } else { 0 });
        for r50 in Self::encode_filename(&self.name)? {
            repr.write_u16(r50);
        }
        repr.write_u16(self.length as u16);
        repr.write_u8(self.job);
        repr.write_u8(self.channel);
        repr.write_u16(Self::encode_date(self.creation_date)?);
        for e in self.extra.iter() {
            repr.write_u16(*e);
        }
        Ok(repr.into_vec())
    }

    pub fn encode_filename(name: &str) -> anyhow::Result<[u16; 3]> {
        let Some((name, ext)) = name.split_once(".") else { return Err(anyhow!("{}: missing extension", name)) };
        if name.len() > 6 || name.len() < 1 || ext.len() > 3 || ext.len() < 1 { return Err(anyhow!("{}: name should 1 to 6 chars, extention should be 1 to 3", name)) };
        let name_w = radix50::pdp11::encode(&format!("{:<6}", name))?;
        let ext_w  = radix50::pdp11::encode_word(&format!("{:<3}", ext))?;
        Ok([name_w[0], name_w[1], ext_w])
    }

    pub fn encode_date(date: Option<NaiveDate>) -> anyhow::Result<u16> {
        use chrono::Datelike;
        let Some(date) = date else { return Ok(0) };
        let yoff = date.year() - 1972;
        if yoff      < 0 { return Err(anyhow!("Date {} is before 1972", date.to_string())) }
        if yoff / 32 > 3 { return Err(anyhow!("Date {} is after {}", date.to_string(), 1972 + 3 * 32)) }

        Ok(0 | ((yoff as u16 / 32)    << 14) & 0b11_0000_00000_00000
             | ((date.month() as u16) << 10) & 0b00_1111_00000_00000
             | ((date.day()   as u16) <<  5) & 0b00_0000_11111_00000
             | ((yoff as u16)         <<  0) & 0b00_0000_00000_11111)
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

pub struct RT11FileWriter<'a, B:BlockDevice> {
    image: &'a mut B,
    direntry: &'a DirEntry,
    residue: Vec<u8>,
    pos: usize,
}

impl<'a, B: BlockDevice> std::io::Write for RT11FileWriter<'a, B> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.pos == self.direntry.length { return Err(io::Error::from(ErrorKind::OutOfMemory /*FileTooLarge, once it's stabilized*/)) }
        let truncated = &buf[0..min(buf.len(), (self.direntry.length - self.pos) * BLOCK_SIZE - self.residue.len())];
        let remains = if self.residue.len() > 0 {
            let (residue_fill, remains) = truncated.split_at(min(truncated.len(), BLOCK_SIZE - self.residue.len()));
            self.residue.extend_from_slice(&residue_fill);
            if self.residue.len() == BLOCK_SIZE {
                self.image.write_blocks(self.direntry.block + self.pos, 1, &self.residue).map_err(|e| io::Error::new(ErrorKind::Other, e))?;
                self.pos += 1;
                self.residue.clear();
            }
            remains
        } else {
            truncated
        };
        let blocks = remains.len() / BLOCK_SIZE;
        if blocks > 0 {
            let (chunk, residue) = remains.split_at(blocks * BLOCK_SIZE);
            self.image.write_blocks(self.direntry.block + self.pos, blocks, &chunk).map_err(|e| io::Error::new(ErrorKind::Other, e))?;
            self.pos += blocks;
            self.residue.extend_from_slice(residue);
        }
        Ok(truncated.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        todo!()
    }
}
