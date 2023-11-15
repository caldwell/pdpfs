// Copyright © 2023 David Caldwell <david@porkrind.org>

use std::{cmp::min, io, io::ErrorKind, fmt::Debug};

use anyhow::{Context,anyhow};
use bytebuffer::{Endian, ByteBuffer};
use chrono::NaiveDate;
use pretty_hex::PrettyHex;

use crate::block::{BlockDevice, BLOCK_SIZE};

// Things we override to make testing easier
#[cfg(not(test))] use chrono::Local;
#[cfg    (test)]  use test::Local;

#[cfg(not(test))] use whoami::username;
#[cfg    (test)]  use test::username;

#[derive(Clone, Debug)]
pub struct RT11FS<B: BlockDevice> {
    pub image: B,
    pub home: HomeBlock,
    pub dir: Vec<DirSegment>,
}

impl<B: BlockDevice> RT11FS<B> {
    pub fn new(image: B) -> anyhow::Result<RT11FS<B>> {
        let home = Self::read_homeblock(&image)?;
        let dir = Self::read_directory(&image, home.directory_start_block).collect::<anyhow::Result<Vec<DirSegment>>>()?;
        Ok(RT11FS {
            image,
            home,
            dir,
        })
    }

    pub fn read_homeblock(image: &B) -> anyhow::Result<HomeBlock> {
        let mut buf = image.read_blocks(1, 1)?;
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

    pub fn read_directory<'a>(image: &'a B, directory_start_block: u16) -> DirSegmentIterator<'a, B> {
        DirSegmentIterator {
            image,
            directory_start_block: directory_start_block,
            next_segment: Some(0),
        }
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

    fn find<F>(&self, predicate: F) -> Option<(usize, usize)>
    where F: Fn(&DirEntry) -> bool
    {
        for (s, seg) in self.dir.iter().enumerate() {
            for (e, f) in seg.entries.iter().enumerate() {
                if !predicate(&f) { continue }
                return Some((s, e))
            }
        }
        None
    }

    fn find_empty_space<'a>(&'a self, blocks: usize) -> Option<(usize, usize)> {
        self.find(|f| f.kind == EntryKind::Empty && f.length >= blocks)
    }

    fn find_file_named(&self, name: &str) -> Option<(usize, usize)> {
        self.find(|f| f.kind == EntryKind::Permanent && f.name == name)
    }

    pub fn create<'a>(&'a mut self, name: &str, bytes: usize) -> anyhow::Result<RT11FileWriter<'a, B>> {
        let blocks = (bytes + BLOCK_SIZE - 1) / BLOCK_SIZE;
        DirEntry::encode_filename(name)?;
        _ = self.delete(name); // Can only fail because file-not-found, which is a no-op here.
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
        self.dir[segment].entries[entry].creation_date = Some(Local::now().date_naive());
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

    pub fn delete(&mut self, name: &str) -> anyhow::Result<()> {
        let Some((segment, entry)) = self.find_file_named(name) else { return Err(anyhow!("File not found")) };
        self.dir[segment].entries[entry].kind = EntryKind::Empty;
        self.coalesce_empty(segment, entry);
        if entry > 0 {
            self.coalesce_empty(segment, entry-1);
        }
        self.image.write_blocks(self.dir[segment].block as usize, 2, &self.dir[segment].repr()?)?;
        Ok(())
    }

    pub fn coalesce_empty(&mut self, segment: usize, entry: usize) {
        if entry+1 >= self.dir[segment].entries.len() ||
           self.dir[segment].entries[entry  ].kind != EntryKind::Empty ||
           self.dir[segment].entries[entry+1].kind != EntryKind::Empty {
            return;
        }

        self.dir[segment].entries[entry].length += self.dir[segment].entries[entry+1].length;
        self.dir[segment].entries.drain(entry+1..=entry+1);
    }

    // Initialize a filesystem on this image
    pub fn init(mut image: B) -> anyhow::Result<RT11FS<B>> {
        let home = HomeBlock::new();
        image.write_blocks(1, 1, &home.repr()?)?;
        let dir_segment = DirSegment::new(home.directory_start_block, 4, image.blocks() as u16);
        image.write_blocks(home.directory_start_block as usize, 2, &dir_segment.repr()?)?;
        return Self::new(image);
    }
}

#[derive(Clone)]
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

impl HomeBlock {
    pub fn new() -> HomeBlock {
        HomeBlock {
            bad_block_replacement_table: [0; 0o202],
            init_restore: [0; 0o252-0o204],
            bup_volume: None,
            pack_cluster_size: 1 /* what is this?? */,
            directory_start_block: 6,
            system_version: "V3A".to_string(),
            volume_id: "RT11FS DC".to_string(), // FIXME: Make this settable?
            owner_name: username(),
            system_id: "DECRT11A".to_string(),
        }
    }

    pub fn repr(&self) -> anyhow::Result<[u8; BLOCK_SIZE]> {
        let mut repr = ByteBuffer::new();
        repr.set_endian(Endian::LittleEndian);
        repr.write_bytes(&self.bad_block_replacement_table);
        repr.write_bytes(&self.init_restore);
        match self.bup_volume {
            Some(num) => { repr.write_bytes(format!("{:<12}", "BUQ").as_bytes());
                           repr.write_u8(num); },
            None => {},
        };
        repr.resize(0o722);
        repr.set_wpos(0o722);
        repr.write_u16(self.pack_cluster_size);
        repr.write_u16(self.directory_start_block);
        repr.write_u16(radix50::pdp11::encode_word(&self.system_version)?);
        repr.write_bytes(format!("{:<12.12}", self.volume_id).as_bytes());
        repr.write_bytes(format!("{:<12.12}", self.owner_name).as_bytes());
        repr.write_bytes(format!("{:<12.12}", self.system_id).as_bytes());
        repr.write_u16(0); // unused
        let mut checksum = 0u16;
        while let Ok(word) = repr.read_u16() {
            checksum = checksum.wrapping_add(word);
        }
        repr.write_u16(checksum);
        Ok(repr.into_vec().try_into().expect("Can't happen."))
    }
}

impl Debug for HomeBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, r#"bad_block_replacement_table:
{:?}
init_restore:
{:?}
bup_volume            : {:?},
pack_cluster_size     : {:#08o} {:#06x} {},
directory_start_block : {:#08o} {:#06x} {},
system_version        : {:#08o} {:?},
volume_id             : {:?},
owner_name            : {:?},
system_id             : {:?},
"#,
            &self.bad_block_replacement_table.hex_dump(),
            &self.init_restore.hex_dump(),
            &self.bup_volume,
            &self.pack_cluster_size, &self.pack_cluster_size, &self.pack_cluster_size,
            &self.directory_start_block, &self.directory_start_block, &self.directory_start_block,
            radix50::pdp11::encode_word(&self.system_version).unwrap(), &self.system_version,
            &self.volume_id,
            &self.owner_name,
            &self.system_id)
        } else {
            f.debug_struct("HomeBlock")
                .field("bad_block_replacement_table", &self.bad_block_replacement_table)
                .field("init_restore",                &self.init_restore          )
                .field("bup_volume",                  &self.bup_volume            )
                .field("pack_cluster_size",           &self.pack_cluster_size     )
                .field("directory_start_block",       &self.directory_start_block )
                .field("system_version",              &self.system_version        )
                .field("volume_id",                   &self.volume_id             )
                .field("owner_name",                  &self.owner_name            )
                .field("system_id",                   &self.system_id             )
                .finish()
        }
    }
}

const STATUS_E_TENT: u16 = 0o000400;
const STATUS_E_MPTY: u16 = 0o001000;
const STATUS_E_PERM: u16 = 0o002000;
const STATUS_E_EOS:  u16 = 0o004000;
const STATUS_E_READ: u16 = 0o040000;
const STATUS_E_PROT: u16 = 0o100000;
const STATUS_E_PRE:  u16 = 0o000020;

#[derive(Clone)]
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
    // This is for initializing a new set of segments on a blank disk
    pub fn new(my_block: u16, segments: u16, total_blocks: u16) -> DirSegment {
        let data_block = my_block + segments * 2;
        let segments = 4; // This is RT-11's default. Should it be configurable like it is there?
        DirSegment {
            block: my_block,
            segments,
            next_segment: 0,
            last_segment:  1,
            extra_bytes: 0,
            data_block,
            entries: vec![DirEntry::new_empty(data_block as usize, (total_blocks - data_block) as usize)],
        }
    }

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
                if entries.len() < 1 { return Err(anyhow!("Too few directory entries: {} (should be >=1)", entries.len())) }
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

impl Debug for DirSegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, r#"Directory Segment {0} header:
    segments     : {1:#08o} {1:#06x} {1},
    next_segment : {2:#08o} {2:#06x} {2},
    last_segment : {3:#08o} {3:#06x} {3},
    extra_bytes  : {4:#08o} {4:#06x} {4},
    data_block   : {5:#08o} {5:#06x} {5}
"#, self.block/2, self.segments, self.next_segment, self.last_segment, self.extra_bytes, self.data_block)?;
        if f.alternate() {
            write!(f, "entries: {}\n", self.entries.len())?;
            for e in self.entries.iter() {
                write!(f, "{:#?}\n", e)?;
            }
        }
        Ok(())
    }
}
pub struct DirSegmentIterator<'a, B: BlockDevice> {
    image: &'a B,
    directory_start_block: u16,
    next_segment: Option<u16>,
}

impl<'a, B: BlockDevice> DirSegmentIterator<'a, B> {
    fn segment(&self, segment: u16) -> anyhow::Result<DirSegment> {
        let block = self.directory_start_block + segment*2;
        Ok(DirSegment::from_repr(block, self.image.read_blocks(block as usize, 2)?)
            .with_context(|| format!("Bad Directory Segment #{} (@ {})", segment, self.directory_start_block + segment*2))?)
    }
}

impl<'a, B: BlockDevice> Iterator for DirSegmentIterator<'a, B> {
    type Item = anyhow::Result<DirSegment>;

    fn next(&mut self) -> Option<Self::Item> {
        let Some(next_segment) = self.next_segment else {
            return None
        };
        let (next, segment) = match self.segment(next_segment) {
            Ok(segment) => (match segment.next_segment { 0 => None, s => Some(s) },
                            Ok(segment)),
            Err(e) => (None, Err(e))
        };
        self.next_segment = next;
        Some(segment)
    }
}

#[derive(Clone, PartialEq)]
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
    pub fn new_empty(data_block: usize, blocks: usize) -> DirEntry {
        DirEntry {
            kind: EntryKind::Empty,
            name: "EMPTYF.ILE".to_string(),
            length: blocks,
            block: data_block,
            read_only: false, protected: false, prefix_block: false, job: 0, channel: 0, creation_date: None, extra: vec![],
        }
    }

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

impl Debug for DirEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            use join_string::Join;
            write!(f, "{:<9} {}{}{} {:>3}:{:<3} {:<10} [{:#06x}] {:5} @ {:<5} {}",
                 match self.kind { EntryKind::Permanent => "Permanent",
                                   EntryKind::Empty     => "Empty",
                                   EntryKind::Tentative => "Tentative"
                 },
                 if self.read_only    { "R" } else { "-" },
                 if self.protected    { "P" } else { "-" },
                 if self.prefix_block { "p" } else { "-" },
                 self.job,
                 self.channel,
                 self.creation_date.map(|d| format!("{}", d)).unwrap_or(format!(" No Date")), DirEntry::encode_date(self.creation_date).unwrap_or(0xffff),
                 self.length,
                 self.block,
                 if self.extra.is_empty() { format!("{}", self.name) } else { format!("{:<10} [{}]", self.name, self.extra.iter().map(|e| format!("{:#6x}", e)).join(",")) }
            )
        } else {
            write!(f, "{:10} {:6} {}", self.creation_date.map(|d| d.to_string()).unwrap_or(" No Date".to_string()),
                self.length,
                match self.kind { EntryKind::Permanent => format!("{}", self.name),
                                  EntryKind::Empty     => format!(" <empty>  was {}", self.name),
                                  EntryKind::Tentative => format!("{:10} (tentative)", self.name),
                })
        }
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

impl <'a, B: BlockDevice> RT11FileWriter<'a, B> {
    #[allow(unused)]
    pub fn close(mut self) -> anyhow::Result<()> {
        return self._close()
    }
    fn _close(&mut self) -> anyhow::Result<()> {
        use std::io::Write;
        self.flush()?;
        Ok(())
    }
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
        } else {
            self.residue.extend_from_slice(remains);
        }
        Ok(truncated.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.residue.len() > 0 {
            if self.pos == self.direntry.length { return Err(io::Error::from(ErrorKind::OutOfMemory /*FileTooLarge, once it's stabilized*/)) }
            self.residue.extend_from_slice(&vec![0; BLOCK_SIZE - self.residue.len()]);
            self.image.write_blocks(self.direntry.block + self.pos, 1, &self.residue).map_err(|e| io::Error::new(ErrorKind::Other, e))?;
            self.pos += 1;
            self.residue.clear();
        }
        Ok(())
    }
}

impl<'a, B: BlockDevice> Drop for RT11FileWriter<'a, B> {
    fn drop(&mut self) {
        _ = self._close();
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::block::PhysicalBlockDevice;
    use pretty_hex::PrettyHex;
    use std::io::Write;

    // Replacement for chrono::Local::now() so that tests are consistent
    #[allow(non_snake_case)]
    pub(crate) mod Local {
        pub(crate) fn now() -> chrono::DateTime<chrono::FixedOffset> {
            chrono::DateTime::<chrono::FixedOffset>::parse_from_rfc3339("2023-01-19 12:13:14+08:00").unwrap()
        }
    }

    pub fn username() -> String { "test-user".into() }

    struct TestDev(Vec<u8>);
    impl BlockDevice for TestDev {
        fn read_sector(&self, sector: usize) -> anyhow::Result<Vec<u8>> {
            Ok(self.0[sector*512..(sector+1)*512].into())
        }
        fn write_sector(&mut self, sector: usize, buf: &[u8]) -> anyhow::Result<()> {
            self.0.splice(sector*512..(sector+1)*512, buf.into_iter().map(|b| *b));
            Ok(())
        }
        fn sector_size(&self) -> usize { 512 }
        fn sectors(&self) -> usize { self.0.len()/512 }
        fn physical_device(&self) -> &impl crate::block::PhysicalBlockDevice {
            self
        }
    }
    impl PhysicalBlockDevice for TestDev {
        fn geometry(&self) -> &crate::block::Geometry {unimplemented!()}
        fn read_sector(&self, _cylinder: usize, _head: usize, _sector: usize) -> anyhow::Result<Vec<u8>> {unimplemented!()}
        fn write_sector(&mut self, _cylinder: usize, _head: usize, _sector: usize, _buf: &[u8]) -> anyhow::Result<()> {unimplemented!()}
        fn as_vec(&self) -> anyhow::Result<Vec<u8>> {unimplemented!()}
        fn from_raw(_data: Vec<u8>, _geometry: crate::block::Geometry) -> Self { unimplemented!() }
    }

    macro_rules! assert_block_eq {
        ($image:expr, $block_num:expr, $( $expected_and_mask:expr ),*) => {
            {
                let got = $image.read_blocks($block_num, 1).expect(&format!("block {}", $block_num));
                let mut expected_and_mask: Vec<u16> = Vec::new();
                $( {
                    expected_and_mask.extend($expected_and_mask.into_iter().map(|v| v as u16));
                } )*
                // We encode the inverse mask into the high byte of a u16--that way simple u8s are treated as 0xff.
                // The ____ const, below, sets the high byte of the mask so that is becomes 0x00 when we invert it.
                let expected: Vec<u8> = expected_and_mask.iter().map(|x| (x & 0xff) as u8).collect();
                let mask: Vec<u8> = expected_and_mask.iter().map(|x| !(x >> 8) as u8).collect();
                let masked: Vec<u8> = got.as_bytes().iter().zip(&mask).map(|(data, mask)| data & mask).collect();
                if masked != expected {
                    panic!("assertion `block {0} == expected` failed\n  Block {0} was:\n{1:?} \n Expected:\n{2:?}\n Mask:\n{3:?}",
                        $block_num, got.as_bytes().hex_dump(), expected.hex_dump(), mask.hex_dump());
                }
            }
        };
    }
    const ____: u16 = 0xff00;

    #[test]
    fn test_init() {
        let dev = TestDev(vec![0;512*20]);
        let fs = RT11FS::init(dev).expect("Create RT-11 FS");
        for b in 0..20 {
            match b {
                1 => assert_block_eq!(fs.image, 1,
                    vec![0; 512-48],
                    vec![0x00, 0x00, 0x01, 0x00, 0x06, 0x00, 0xa9, 0x8e, 0x52, 0x54, 0x31, 0x31 ,0x46, 0x53, 0x20, 0x44,
                         0x43, 0x20, 0x20, 0x20, 0x74, 0x65, 0x73, 0x74, 0x2d, 0x75, 0x73, 0x65, 0x72, 0x20, 0x20, 0x20,
                         0x44, 0x45, 0x43, 0x52, 0x54, 0x31, 0x31, 0x41, 0x20, 0x20, 0x20, 0x20 ,0x00, 0x00, 0x61, 0x2b]),
                6 => assert_block_eq!(fs.image, 6,
                    vec![0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x02, 0x58, 0x21, 0xee, 0x80,
                         0x25, 0x3a, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                    vec![0; 512-32]),
                _ => assert_block_eq!(fs.image, b, vec![0; 512]),
            }
        }
    }

    fn incrementing(count: usize) -> Vec<u8> {
        (0..count).map(|x| x as u8).collect::<Vec<u8>>()
    }

    #[test]
    fn test_write() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::init(dev).expect("Create RT-11 FS");
        { fs.create("TEST.TXT", 512).expect("write test.txt"); }
        assert_block_eq!(fs.image, 6,
            vec![0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x04, 0xdb, 0x7d, 0x00, 0x7d,
                 0xd4, 0x80, 0x01, 0x00, 0x00, 0x00, 0x73, 0x46, 0x00, 0x02, 0x58, 0x21, 0xee, 0x80, 0x25, 0x3a,
                 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08],
            vec![0; 512-40]);
        assert_block_eq!(fs.image, 14, vec![0; 512]);
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::init(dev).expect("Create RT-11 FS");
        { let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            f.write(&incrementing(512)).expect("write"); }
        assert_block_eq!(fs.image, 14, incrementing(512));
    }

    #[test]
    fn test_write_chunk() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::init(dev).expect("Create RT-11 FS");
        {
            let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            assert_eq!(f.pos, 0);
            assert_eq!(f.residue, vec![]);
            f.write(&incrementing(256)).expect("write");
            assert_eq!(f.residue, incrementing(256));
            f.write(&incrementing(256)).expect("write");
        }
        assert_block_eq!(fs.image, 14, incrementing(512));
    }

    #[test]
    fn test_write_partial_block() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::init(dev).expect("Create RT-11 FS");
        {
            let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            f.write(&incrementing(256)).expect("write");
        }
        assert_block_eq!(fs.image, 14, incrementing(256), vec![0; 256]);
    }

    #[test]
    fn test_overwrite_file() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::init(dev).expect("Create RT-11 FS");
        {
            let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            f.write(&incrementing(256)).expect("write");
        }
        {
            let mut f = fs.create("TEST.TXT", 1024).expect("write test.txt");
            f.write(&vec![0x55; 1024]).expect("write");
        }
        assert_eq!(fs.dir[0].entries.len(), 2);
        assert_block_eq!(fs.image, 14, vec![0x55; 512]);
        assert_block_eq!(fs.image, 15, vec![0x55; 512]);
    }

    #[test]
    fn test_remove_file() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::init(dev).expect("Create RT-11 FS");
        {
            let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            f.write(&incrementing(256)).expect("write");
        }
        fs.delete("TEST.TXT").expect("delete test.txt");
        assert_eq!(fs.file_named("TEST.TXT"), None);
        assert_eq!(fs.used_blocks(), 0);
        assert_block_eq!(fs.image, 6,
            vec![0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x02, ____, ____, ____, ____,
                 ____, ____, 0x06, 0x00, 0x00, 0x00, ____, ____, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            vec![0; 512-32]);
    }

    #[test]
    fn test_coalesce_empty() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::init(dev).expect("Create RT-11 FS");
        {
            let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            f.write(&incrementing(256)).expect("write");
        }
        fs.delete("TEST.TXT").expect("delete test.txt");
        assert_eq!(fs.dir[0].entries.len(), 1);
    }
}
