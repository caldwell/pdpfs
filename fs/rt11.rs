// Copyright © 2023 David Caldwell <david@porkrind.org>

use std::{cmp::min, fmt::Debug, io::{self, ErrorKind}, ops::Range};

use anyhow::{Context,anyhow};
use bytebuffer::{Endian, ByteBuffer};
use chrono::NaiveDate;
use pretty_hex::PrettyHex;
use serde::Serialize;

use crate::block::{BlockDevice, BLOCK_SIZE};
use super::FileSystem;

// Things we override to make testing easier
#[cfg(not(test))] use chrono::Local;
#[cfg    (test)]  use super::test::Local;

#[cfg(not(test))] use whoami::username;
#[cfg    (test)]  use super::test::username;

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

    pub fn image_is(image: &B) -> bool {
        let Ok(home) = Self::read_homeblock(&image) else { return false };
        let Ok(dir) = Self::read_directory(&image, home.directory_start_block).collect::<anyhow::Result<Vec<DirSegment>>>() else {
            return false;
        };
        for s in dir.into_iter() {
            for e in s.entries.iter() {
                // Do a sanity check on all the directory entries. They should have sane block numbers.
                if e.length >= image.blocks() || e.block >= image.blocks() {
                    return false;
                }
            }
        }
        true
    }

    // Initialize a filesystem on this image
    pub fn mkfs(mut image: B) -> anyhow::Result<RT11FS<B>> {
        let home = HomeBlock::new();
        image.write_blocks(1, 1, &home.repr()?)?;
        let segment_count = 4; // This is RT-11's default. Should it be configurable like it is there?
        let first_data_block = DirSegment::segment_block(home.directory_start_block, segment_count+1);
        let dir_segment = DirSegment::new(1,
                                          home.directory_start_block,
                                          1..segment_count,
                                          first_data_block..image.blocks() as u16);
        image.write_blocks(home.directory_start_block as usize, 2, &dir_segment.repr()?)?;
        return Self::new(image);
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
            next_segment: Some(1),
        }
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

    fn raw_stat<'a>(&'a self, name: &str) -> Option<&'a DirEntry> {
        self.find_file_named(name).map(|(segment, entry)| &self.dir[segment].entries[entry])
    }

    fn write_directory_segment(&mut self, segment: usize) -> anyhow::Result<()> {
        self.image.write_blocks(self.dir[segment].block as usize, 2, &self.dir[segment].repr()?)
    }

    fn coalesce_empty(&mut self, segment: usize, entry: usize) {
        if entry+1 >= self.dir[segment].entries.len() ||
           self.dir[segment].entries[entry  ].kind != EntryKind::Empty ||
           self.dir[segment].entries[entry+1].kind != EntryKind::Empty {
            return;
        }

        self.dir[segment].entries[entry].length += self.dir[segment].entries[entry+1].length;
        self.dir[segment].entries.drain(entry+1..=entry+1);
    }

    fn full_dir_iter<'a>(&'a self, kind: Option<EntryKind>) -> DirEntryIterator<'a, B> {
        DirEntryIterator {
            fs: self,
            segment: 0,
            entry: 0,
            kind,
        }
    }

    fn create<'a>(&'a mut self, name: &str, bytes: usize) -> anyhow::Result<RT11FileWriter<'a, B>> {
        let blocks = (bytes + BLOCK_SIZE - 1) / BLOCK_SIZE;
        DirEntry::encode_filename(name)?;
        _ = self.delete(name); // Can only fail because file-not-found, which is a no-op here.
        let Some((segment, entry)) = self.find_empty_space(blocks) else { return Err(anyhow!("No space available in image")) };
        let (segment, entry) =
            if self.dir[segment].entries.len() + 1 > self.dir[segment].max_entries() {
                // Too many entries to fit in segment.
                self.split_directory(segment)?;
                // The segment we found may have moved so look for it again
                let Some((segment, entry)) = self.find_empty_space(blocks) else { return Err(anyhow!("No space available in image")) };
                (segment, entry)
            } else { (segment, entry) };
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
        self.write_directory_segment(segment)?;
        Ok(RT11FileWriter{
            image: &mut self.image,
            direntry: &self.dir[segment].entries[entry],
            residue: vec![],
            pos: 0,
        })
    }

    // As per the "RT–11 Volume and File Formats Manual" section 1.1.5
    fn split_directory(&mut self, segment: usize) -> anyhow::Result<()> {
        let block_range = self.dir[segment].block_range();
        let mut new_seg = self.alloc_segment(block_range.end..block_range.end)?;
        new_seg.next_segment = self.dir[segment].next_segment;
        self.dir[segment].next_segment = new_seg.segment;
        let half = self.dir[segment].entries.len()/2;
        new_seg.entries = self.dir[segment].entries.split_off(half);
        let blocks: usize = new_seg.entries.iter().map(|e| e.length).sum();
        new_seg.data_block -= blocks as u16;
        self.dir.insert(segment+1, new_seg);
        self.write_directory_segment(segment)?;
        self.write_directory_segment(segment+1)?;
        Ok(())
    }

    fn alloc_segment(&mut self, data_block: Range<u16>) -> anyhow::Result<DirSegment> {
        // The first dir segment holds the last segment used so we just increment it to allocate a new one
        if self.dir[0].last_segment == self.dir[0].segments { Err(anyhow!("Out of directory segments"))? };
        self.dir[0].last_segment += 1;
        Ok(DirSegment::new(self.dir[0].last_segment, self.home.directory_start_block,
                           self.dir[0].last_segment..self.dir[0].segments,
                           data_block))
    }
}

impl<B: BlockDevice> FileSystem for RT11FS<B> {
    type BlockDevice=B;

    fn filesystem_name(&self) -> &str {
        "RT-11"
    }

    fn dir_iter<'a>(&'a self, _path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn super::DirEntry + 'a>> + 'a>> {
        Ok(Box::new(self.full_dir_iter(None)
            .map(|e| -> Box<dyn super::DirEntry> { return Box::new(e) })))
    }

    fn read_dir<'a>(&'a self, _path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn super::DirEntry + 'a>> + 'a>> {
        Ok(Box::new(self.full_dir_iter(Some(EntryKind::Permanent))
            .map(|e| -> Box<dyn super::DirEntry> { return Box::new(e) })))
    }

    fn stat<'a>(&'a self, name: &str) -> Option<Box<dyn super::DirEntry + 'a>> {
        self.read_dir("").unwrap().find(|f| f.file_name() == name)
    }

    fn free_blocks(&self) -> usize {
        self.full_dir_iter(None).filter(|e| e.kind == EntryKind::Empty).fold(0, |acc, e| acc + e.length)
    }

    fn used_blocks(&self) -> usize {
        self.full_dir_iter(None).filter(|e| e.kind != EntryKind::Empty).fold(0, |acc, e| acc + e.length)
    }

    fn read_file(&self, name: &str) -> anyhow::Result<ByteBuffer> {
        let Some(file) = self.raw_stat(&name) else {
            return Err(anyhow!("File not found: {}", name));
        };
        self.image.read_blocks(file.block, file.length)
    }

    fn write_file(&mut self, name: &str, contents: &[u8]) -> anyhow::Result<()> {
        use std::io::Write;
        let mut fh = self.create(name, contents.len() as usize)?;
        fh.write(&contents)?;
        Ok(())
    }

    fn delete(&mut self, name: &str) -> anyhow::Result<()> {
        let Some((segment, entry)) = self.find_file_named(name) else { return Err(anyhow!("File not found")) };
        self.dir[segment].entries[entry].kind = EntryKind::Empty;
        self.coalesce_empty(segment, entry);
        if entry > 0 {
            self.coalesce_empty(segment, entry-1);
        }
        self.write_directory_segment(segment)?;
        Ok(())
    }

    fn rename_unchecked(&mut self, src: &str, dest: &str) -> anyhow::Result<()> {
        DirEntry::encode_filename(dest)?;
        let (segment, entry) = self.find_file_named(src).unwrap(/*we already checked*/);
        self.dir[segment].entries[entry].name = dest.to_owned();
        self.write_directory_segment(segment)?;
        Ok(())
    }

    fn block_device(&self) -> &B {
        &self.image
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
    pub segment: u16, // The logical segment number (1 based)
}

impl DirSegment {
    pub fn segment_block(seg_start_block: u16, segment: u16) -> u16 { seg_start_block + (segment-1) * 2 }

    pub fn new(segment: u16, seg_start_block: u16, unused_segs: Range<u16>, data_block: Range<u16>) -> DirSegment {
        let block = DirSegment::segment_block(seg_start_block, segment);
        DirSegment {
            segment,
            block,
            next_segment: 0,
            last_segment: unused_segs.start,
            segments: unused_segs.end,
            extra_bytes: 0,
            data_block: data_block.start,
            entries: vec![DirEntry::new_empty(data_block.start as usize, (data_block.end - data_block.start) as usize)],
        }
    }

    pub fn from_repr(segment: u16, my_block: u16, mut buf: ByteBuffer) -> anyhow::Result<DirSegment> {
        buf.set_endian(Endian::LittleEndian);
        let extra_bytes;
        let data_block;
        Ok(DirSegment {
            segment,
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

    /// Returns the block range this directory segment represents
    fn block_range(&self) -> Range<u16> {
        let block_count: usize = self.entries.iter().map(|e| e.length).sum();
        self.data_block..self.data_block+block_count as u16
    }
}

impl Debug for DirSegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, r#"Directory Segment #{6} @ {0} header:
    segments     : {1:#08o} {1:#06x} {1},
    next_segment : {2:#08o} {2:#06x} {2},
    last_segment : {3:#08o} {3:#06x} {3},
    extra_bytes  : {4:#08o} {4:#06x} {4},
    data_block   : {5:#08o} {5:#06x} {5}
"#, self.block, self.segments, self.next_segment, self.last_segment, self.extra_bytes, self.data_block, self.segment)?;
        if f.alternate() {
            write!(f, "entries: {}\n", self.entries.len())?;
            write!(f, "block range: {:?}\n", self.block_range())?;
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
        let block = DirSegment::segment_block(self.directory_start_block, segment);
        Ok(DirSegment::from_repr(segment, block, self.image.read_blocks(block as usize, 2)?)
            .with_context(|| format!("Bad Directory Segment #{} (@ {})", segment, block))?)
    }
}

impl<'a, B: BlockDevice> Iterator for DirSegmentIterator<'a, B> {
    type Item = anyhow::Result<DirSegment>;

    fn next(&mut self) -> Option<Self::Item> {
        let (next, segment) = match self.segment(self.next_segment?) {
            Ok(segment) => (match segment.next_segment { 0 => None, s => Some(s) },
                            Ok(segment)),
            Err(e) => (None, Err(e))
        };
        self.next_segment = next;
        Some(segment)
    }
}

#[derive(Clone, PartialEq, Serialize)]
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

#[derive(Copy, Clone, Debug, PartialEq, Serialize)]
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
        let Some((name, ext)) = name.split_once(".") else { return Err(anyhow!("filename missing extension")) };
        if name.len() > 6 || name.len() < 1 || ext.len() > 3 || ext.len() < 1 { return Err(anyhow!("filename should 1 to 6 chars, extention should be 1 to 3")) };
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
        let (age, month, day, year) = (((raw & 0b11_0000_00000_00000) >> 14) as u32,
                                       ((raw & 0b00_1111_00000_00000) >> 10) as u32,
                                       ((raw & 0b00_0000_11111_00000) >>  5) as u32,
                                       ((raw & 0b00_0000_00000_11111) >>  0) as u32);
        Ok(match raw {
            0 => None,
            _ => Some(chrono::NaiveDate::from_ymd_opt((1972 + year + age * 32) as i32, month, day)
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
                                   EntryKind::Tentative => "Tentative",
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

impl super::DirEntry for &DirEntry {
    fn path(&self)       -> &str                             { &self.name }
    fn file_name(&self)  -> &str                             { &self.name }
    fn is_dir(&self)     -> bool                             { false }
    fn is_file(&self)    -> bool                             { self.kind == EntryKind::Permanent }
    fn is_symlink(&self) -> bool                             { false }
    fn len(&self)        -> u64                              { (self.length * BLOCK_SIZE) as u64 }
    fn modified(&self)   -> anyhow::Result<super::Timestamp> { Err(anyhow!("Not available")) }
    fn accessed(&self)   -> anyhow::Result<super::Timestamp> { Err(anyhow!("Not available")) }
    fn created(&self)    -> anyhow::Result<super::Timestamp> { self.creation_date.map(|d| super::Timestamp::Date(d)).ok_or(anyhow!("Bad Date")) }
    fn blocks(&self)     -> u64                              { self.length as u64}
    fn readonly(&self)   -> bool                             { self.read_only }
}

pub struct DirEntryIterator<'a, B: BlockDevice> {
    fs: &'a RT11FS<B>,
    segment: usize,
    entry: usize,
    kind: Option<EntryKind>, // None means all. :-)
}

impl<'a, B: BlockDevice> Iterator for DirEntryIterator<'a, B> {
    type Item = &'a DirEntry;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.segment >= self.fs.dir.len() { return None }
            let entry = &self.fs.dir[self.segment].entries[self.entry];
            self.entry += 1;
            if self.entry >= self.fs.dir[self.segment].entries.len() {
                self.segment += 1;
                self.entry = 0;
            }
            if self.kind.is_none() || self.kind == Some(entry.kind) {
                return Some(entry);
            }
        }
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
    use crate::fs::test::*;
    use crate::assert_block_eq;
    use pretty_hex::PrettyHex;
    use std::io::Write;

    #[test]
    fn test_init() {
        let dev = TestDev(vec![0;512*20]);
        let fs = RT11FS::mkfs(dev).expect("Create RT-11 FS");
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

    #[test]
    fn test_write() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::mkfs(dev).expect("Create RT-11 FS");
        { fs.create("TEST.TXT", 512).expect("write test.txt"); }
        assert_block_eq!(fs.image, 6,
            vec![0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x04, 0xdb, 0x7d, 0x00, 0x7d,
                 0xd4, 0x80, 0x01, 0x00, 0x00, 0x00, 0x73, 0x46, 0x00, 0x02, 0x58, 0x21, 0xee, 0x80, 0x25, 0x3a,
                 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08],
            vec![0; 512-40]);
        assert_block_eq!(fs.image, 14, vec![0; 512]);
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::mkfs(dev).expect("Create RT-11 FS");
        { let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            f.write(&incrementing(512)).expect("write"); }
        assert_block_eq!(fs.image, 14, incrementing(512));
    }

    #[test]
    fn test_write_chunk() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::mkfs(dev).expect("Create RT-11 FS");
        {
            let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            assert_eq!(f.pos, 0);
            assert_eq!(f.residue, Vec::<u8>::new());
            f.write(&incrementing(256)).expect("write");
            assert_eq!(f.residue, incrementing(256));
            f.write(&incrementing(256)).expect("write");
        }
        assert_block_eq!(fs.image, 14, incrementing(512));
    }

    #[test]
    fn test_write_partial_block() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::mkfs(dev).expect("Create RT-11 FS");
        {
            let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            f.write(&incrementing(256)).expect("write");
        }
        assert_block_eq!(fs.image, 14, incrementing(256), vec![0; 256]);
    }

    #[test]
    fn test_overwrite_file() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::mkfs(dev).expect("Create RT-11 FS");
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
        let mut fs = RT11FS::mkfs(dev).expect("Create RT-11 FS");
        {
            let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            f.write(&incrementing(256)).expect("write");
        }
        fs.delete("TEST.TXT").expect("delete test.txt");
        assert_eq!(fs.stat("TEST.TXT").is_none(), true);
        assert_eq!(fs.used_blocks(), 0);
        assert_block_eq!(fs.image, 6,
            vec![0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x02, ____, ____, ____, ____,
                 ____, ____, 0x06, 0x00, 0x00, 0x00, ____, ____, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            vec![0; 512-32]);
    }

    #[test]
    fn test_coalesce_empty() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = RT11FS::mkfs(dev).expect("Create RT-11 FS");
        {
            let mut f = fs.create("TEST.TXT", 512).expect("write test.txt");
            f.write(&incrementing(256)).expect("write");
        }
        fs.delete("TEST.TXT").expect("delete test.txt");
        assert_eq!(fs.dir[0].entries.len(), 1);
    }

    #[test]
    fn test_split_directory_segment() {
        let dev = TestDev(vec![0;512*200]);
        let mut fs = RT11FS::mkfs(dev).expect("Create RT-11 FS");
        for i in 0..75 {
            let mut f = fs.create(&format!("TEST{i}.TXT"), 512).expect("write test.txt");
            f.write(&incrementing(256)).expect("write");
        }
        for seg in fs.dir.iter() {
            println!("{:#?}", seg)
        }
        assert_eq!(fs.dir.len(), 2);
        assert_eq!(fs.dir[1].data_block, fs.home.directory_start_block + fs.dir[0].segments*2 + fs.dir[0].max_entries() as u16/2);
        assert_eq!(fs.dir[0].last_segment, 2);
        assert_block_eq!(fs.image, 6,
                         vec![0x04, 0x00, 0x02, 0x00, 0x02, 0x00, 0x00, 0x00, 0x0e, 0x00],
                         vec![____; 512-10]);
        assert_block_eq!(fs.image, 8,
                         vec![0x04, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x31, 0x00],
                         vec![____; 512-10]);
        assert_block_eq!(fs.image, 0x0e, incrementing(256), vec![0; 256]); // First file in segment 1
        assert_block_eq!(fs.image, 0x31, incrementing(256), vec![0; 256]); // First file in segment 2
    }
}
