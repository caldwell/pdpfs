// Copyright © 2023 David Caldwell <david@porkrind.org>

use std::{mem::size_of, fmt::Debug, ops::Range};

use anyhow::anyhow;
use bytebuffer::{Endian, ByteBuffer};
use chrono::{NaiveDate, Datelike};

// Things we override to make testing easier
#[cfg(not(test))] use chrono::Local;
#[cfg    (test)]  use super::test::Local;

use crate::block::{BlockDevice,BLOCK_SIZE};
use super::FileSystem;


// CHQF SAO XXDP+ FILE STRUCT DOC: Oct 84: https://archive.org/details/bitsavers_decpdp11xx6BMCCHQFSB0XXDPFileStructDocOct84_497378/page/n3/mode/2up
//                                 Apr 81: http://bitsavers.org/pdf/dec/pdp11/xxdp/AC-S866A-M0_CHQFSA0_XXDP%2B_File_Struct_Doc_Apr81.pdf
// Same, but appears to be recreated. Much easier to read: https://github.com/rust11/xxdp/blob/main/XXDP%2B%20File%20Structure.pdf

// XXDP is based on the "DOS-11" / "Batch-11" filesystem. Info on that can be found here:
// disk operating system monitor: systems programmer's manual: https://bitsavers.org/pdf/dec/pdp11/dos-batch/DEC-11-OSPMA-A-D_PDP-11_DOS_Monitor_V004A_System_Programmers_Manual_May72.pdf
// The date format, in particular, is defined there. The rest is useful to fill in blanks left by the XXDP
// manual, which is light on detail in some areas.

const USABLE_BLOCK_SIZE : usize = BLOCK_SIZE - size_of::<u16>(); // For blocks with the next pointer in them (most data blocks)
const BITMAP_WORDS_PER_MAP_BLOCK: usize = 64 - 4; // XXDP+ File Struct Doc (April 1981), Section 4.1.3
const ENTRIES_PER_UFD_BLOCK: usize = 28; // XXDP+ File Struct Doc (April 1981), Section 4.1.2

#[derive(Clone)]
pub struct XxdpFs<B: BlockDevice> {
    pub image: B,
    pub mfd: Mfd,
    pub ufd: Vec<DirEntry>,
    pub ufd_block_list: Vec<u16>,
    pub bitmap: Vec<bool>,
    pub bitmap_block_list: Vec<u16>,
}

fn round_up(total: usize, step: usize) -> usize {
    (total + step - 1) / step * step
}

impl<B: BlockDevice> XxdpFs<B> {
    pub fn new(image: B) -> anyhow::Result<XxdpFs<B>> {
        let (mfd, ufd, ufd_block_list, bitmap, bitmap_block_list) = Self::try_new(&image)?;
        Ok(XxdpFs {
            image,
            mfd,
            ufd,
            ufd_block_list,
            bitmap,
            bitmap_block_list,
        })
    }

    pub fn image_is(image: &B) -> bool {
        Self::try_new(image).is_ok()
    }

    fn try_new(image: &B) -> anyhow::Result<(Mfd, Vec<DirEntry>, Vec<u16>, Vec<bool>, Vec<u16>)> {
        let mfd = Self::read_master_file_directory(&image)?;
        let (ufd, ufd_block_list) = Self::read_user_file_directory(&image, match mfd {
            Mfd::VariantOne(ref v1) => v1.ufd_block,
            Mfd::VariantTwo(ref v2) => v2.ufd_block,
        })?;
        let (mut bitmap, bitmap_block_list) = Self::read_bitmap(&image, match mfd {
            Mfd::VariantOne(ref v1) => v1.bitmap_block,
            Mfd::VariantTwo(ref v2) => v2.bitmap_block,
        })?;
        bitmap.truncate(image.blocks()); // The bitmap is usually a lot bigger on disk than it needs to be
        if image.blocks() > bitmap.len() {
            return Err(anyhow!("Bitmap is too short {} < {}", bitmap.len(), image.blocks()));
        }
        Ok((mfd,
            ufd,
            ufd_block_list,
            bitmap,
            bitmap_block_list))
    }

    pub fn mkfs(image: B) -> anyhow::Result<XxdpFs<B>> {
        let bitmap_entries = round_up(image.blocks(), 16 * BITMAP_WORDS_PER_MAP_BLOCK);
        let bitmap_blocks = bitmap_entries / (16 * BITMAP_WORDS_PER_MAP_BLOCK);
        const AVE_FILE_BLOCKS: usize = 4; // Basd on XXDP+ File Struct Doc Apr81 table 4.1.4, specifically the RX01 entry. The ratios seem random:
                                          // The RX01 has 494 usable blocks, 112 entries (4 ufd blocks * 28 entries/block) - 4.5:1
                                          // The RX02 has 998 usable blocks and 448 entries (16 ufd blocks) - 2:1
                                          // The RP02 has 48,000 blocks and 4760 entries. - 10:1
        let ufd_entries = round_up(image.blocks() / AVE_FILE_BLOCKS, ENTRIES_PER_UFD_BLOCK);
        let ufd_blocks = ufd_entries / ENTRIES_PER_UFD_BLOCK;

        let mut block = 1;
        let mut blocks = move |count| {
            let b = block;
            block += count;
            b
        };
        let _mfd1_block = blocks(1);
        let mfd = MfdVariantOne {
            interleave_factor: 1,
            mfd2_block: blocks(1),
            ufd_block: blocks(ufd_blocks as u16),
            bitmap_block: blocks(bitmap_blocks as u16),
            bitmap_pointer: ((blocks(0)-bitmap_blocks as u16)..blocks(0)).collect(),
        };
        let entries = (0..ufd_entries).map(|_| DirEntry::default()).collect();
        let mut bitmap = Vec::new();
        bitmap.resize(image.blocks(), false);
        for b in 0..blocks(0) as usize {
            bitmap[b] = true;
        }
        let mut fs = XxdpFs {
            image,
            bitmap,
            bitmap_block_list: mfd.bitmap_pointer.clone(),
            ufd: entries,
            ufd_block_list: (mfd.ufd_block..mfd.bitmap_block).collect(),
            mfd: Mfd::VariantOne(mfd),
        };
        fs.write_ufd()?;
        fs.write_bitmap()?;
        fs.write_mfd()?;
        Ok(fs)
    }

    pub fn read_master_file_directory(image: &B) -> anyhow::Result<Mfd> {
        let mfd_block = 1;
        let mut buf1 = image.read_blocks(mfd_block, 1)?;
        buf1.set_endian(Endian::LittleEndian);
        let next = buf1.read_u16()? as usize;
        buf1.set_rpos(0);

        let mut buf2 = image.read_blocks(next, 1)?;
        buf2.set_endian(Endian::LittleEndian);

        Ok(Mfd::VariantOne(MfdVariantOne::from_repr(mfd_block, [buf1, buf2])?))
    }

    pub fn read_user_file_directory(image: &B, start_block: u16) -> anyhow::Result<(Vec<DirEntry>, Vec<u16>)> {
        let blocklist = Self::read_chain_raw(image, start_block)?;
        let mut entries = vec![];
        let mut block_list = vec![];
        for (block, mut buf) in blocklist {
            buf.set_rpos(size_of::<u16>());
            block_list.push(block);
            while let Some(entry) = DirEntry::from_repr(&mut buf)? {
                entries.push(entry);
            }
        }
        Ok((entries, block_list))
    }

    pub fn read_bitmap(image: &B, start_block: u16) -> anyhow::Result<(Vec<bool>, Vec<u16>)> {
        let blocklist = Self::read_chain_raw(image, start_block)?;
        let mut block_list = vec![];
        let mut bitmap = vec![];
        for (block, mut buf) in blocklist.into_iter() {
            buf.set_rpos(size_of::<u16>());
            block_list.push(block);
            let bb = BitmapBlock::from_repr(&mut buf)?;
            for e in bb.entries.iter() {
                for bit in 0..16 { // 16 -> 0 ????
                    bitmap.push(e & (1<<bit) != 0)
                }
            }
        }
        Ok((bitmap, block_list))
    }

    pub fn read_chain_raw(image: &B, start_block: u16) -> anyhow::Result<Vec<(u16, ByteBuffer)>> {
        let mut seen = std::collections::HashSet::new();
        let mut blocklist: Vec<(u16, ByteBuffer)> = vec![];
        let mut block = start_block;
        while block != 0 {
            if seen.contains(&block) {
                return Err(anyhow!("Duplicate block in chain: {block}"));
            }
            let mut buf = image.read_blocks(block.into(), 1)?;
            buf.set_endian(Endian::LittleEndian);
            let next = buf.read_u16()?;
            blocklist.push((block, buf));
            seen.insert(block);
            block = next;
        }
        Ok(blocklist)
    }

    fn full_dir_iter<'a>(&'a self) -> std::slice::Iter<'_, DirEntry> {
        self.ufd.iter()
    }

    fn raw_stat<'a>(&'a self, name: &str) -> Option<(usize, &'a DirEntry)> {
        self.full_dir_iter().enumerate().find(|(_, e)| e.name.as_deref() == Some(name))
    }

    fn allocate_dir_entry(&mut self) -> anyhow::Result<usize> {
        if let Some((i, _)) = self.full_dir_iter().enumerate().find(|(_,e)| e.name.is_none()) {
            return Ok(i);
        }
        // No more room! Need to allocate a new UFD block
        let ufd_block = self.allocate_blocks(1)?[0];
        self.ufd_block_list.push(ufd_block);
        let new_dir_entry = self.ufd.len();
        self.ufd.extend((0..ENTRIES_PER_UFD_BLOCK).map(|_| DirEntry::default()));
        self.write_bitmap()?;
        self.write_ufd()?;
        Ok(new_dir_entry)
    }

    // If we cared about speed something like this would be the native data structure.
    fn calculate_bitmap_free_spans(&self) -> Vec<Range<u16>> {
        let mut spans = vec![];
        let mut start = None;
        for (i, b) in self.bitmap.iter().enumerate().map(|(i,b)| (i as u16, b)) {
            match (start, b) {
                (None,    false) => start = Some(i),
                (Some(s), true ) => { spans.push(s..i);
                                      start = None; },
                (_,       _,   ) => {},
            }
        }
        if let Some(s) = start {
            spans.push(s..self.bitmap.len() as u16)
        }
        spans
    }

    fn allocate_blocks(&mut self, blocks: u16) -> anyhow::Result<Vec<u16>> {
        let mut spans = self.calculate_bitmap_free_spans();
        fn span_sort_key(span: &Range<u16>, desired_len: u16) -> u16 { if span.len() as u16 == desired_len { u16::MAX } else { span.len() as u16 } }
        spans.sort_by(|a,b| span_sort_key(a, blocks).cmp(&span_sort_key(b,blocks)));
        let mut list = vec![];
        let mut count = blocks;
        while count > 0 {
            let Some(mut s) = spans.pop() else {
                return Err(anyhow!("No space for {} blocks", blocks));
            };
            s.end = std::cmp::min(s.end, s.start + count); // don't overrun
            for b in s {
                list.push(b);
                self.bitmap[b as usize] = true;
                count -= 1;
            }
        }
        Ok(list)
    }

    fn write_block_chain(image: &mut B, block_list: &[u16], data: &[u8]) -> anyhow::Result<()> {
        let mut iter = block_list.into_iter().map(|b| *b).peekable();
        let mut buf = Vec::with_capacity(BLOCK_SIZE);
        let mut remaining = &data[..];
        while let Some(b) = iter.next() {
            buf.truncate(0);
            let next = iter.peek().map(|b| *b);
            let mut pointer = ByteBuffer::new();
            pointer.set_endian(Endian::LittleEndian);
            pointer.write_u16(next.unwrap_or(0));
            buf.extend_from_slice(pointer.as_bytes());
            let chunk_size = std::cmp::min(USABLE_BLOCK_SIZE, remaining.len());
            buf.extend_from_slice(&remaining[..chunk_size]);
            buf.extend_from_slice(&vec![0; BLOCK_SIZE - chunk_size]);
            image.write_blocks(b as usize, 1, &buf)?;
            remaining = &remaining[chunk_size..];
        }
        Ok(())
    }

    fn write_ufd(&mut self) -> anyhow::Result<()> {
        let mut buf = ByteBuffer::new();
        buf.set_endian(Endian::LittleEndian);
        for entries in self.ufd.chunks(ENTRIES_PER_UFD_BLOCK) {
            for e in entries.iter() {
                buf.write_bytes(&e.repr()?);
            }
            buf.write_bytes(&vec![0; USABLE_BLOCK_SIZE - buf.len() % USABLE_BLOCK_SIZE]);
        }
        Self::write_block_chain(&mut self.image, &self.ufd_block_list, buf.as_bytes())
    }

    fn write_bitmap(&mut self) -> anyhow::Result<()> {
        let mut buf = ByteBuffer::new();
        buf.set_endian(Endian::LittleEndian);
        for (i, bits) in self.bitmap.chunks(BITMAP_WORDS_PER_MAP_BLOCK * 16/*bits/word*/).enumerate() {
            buf.write_bytes(&BitmapBlock {
                                 map_number: i as u16,
                                 first_bitmap: self.bitmap_block_list[0],
                                 entries: bits.chunks(16).map(|w| {
                                     w.iter().enumerate().fold(0, |acc, (n, b)| if *b { acc | 1<<n } else { acc })
                                 }).collect(),
                            }.repr()?);
        }
        Self::write_block_chain(&mut self.image, &self.bitmap_block_list, buf.as_bytes())
    }

    fn write_mfd(&mut self) -> anyhow::Result<()> {
        let mfd2_block = match self.mfd {
            Mfd::VariantOne(ref v1) => v1.mfd2_block,
            Mfd::VariantTwo(ref v2) => v2.other_mfd_block,
        } as usize;
        let (buf1, buf2) = self.mfd.repr();
        self.image.write_blocks(1,          1, &buf1)?;
        self.image.write_blocks(mfd2_block, 1, &buf2)?;
        Ok(())
    }
}

impl<B: BlockDevice> FileSystem for XxdpFs<B> {
    type BlockDevice=B;

    fn filesystem_name(&self) -> &str {
        "XXDP"
    }

    fn dir_iter<'a>(&'a self, path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn super::DirEntry + 'a>> + 'a>> {
        if path != "/" { return Err(anyhow!("Bad path")) }
        Ok(Box::new(self.full_dir_iter()
            .map(|e| -> Box<dyn super::DirEntry> { return Box::new(e) })))
    }

    fn read_dir<'a>(&'a self, path: &str) -> anyhow::Result<Box<dyn Iterator<Item=Box<dyn super::DirEntry + 'a>> + 'a>> {
        if path != "/" { return Err(anyhow!("Bad path")) }
        Ok(Box::new(self.full_dir_iter().filter(|e| e.name.is_some())
            .map(|e| -> Box<dyn super::DirEntry> { return Box::new(e) })))
    }

    fn stat<'a>(&'a self, name: &str) -> Option<Box<dyn super::DirEntry + 'a>> {
        self.read_dir("/").unwrap().find(|f| f.file_name() == name)
    }

    fn free_blocks(&self) -> usize {
        self.bitmap[0..self.image.blocks()].iter().fold(0, |acc,b| if !b { acc + 1 } else { acc })
    }

    fn used_blocks(&self) -> usize {
        self.image.blocks() - self.free_blocks()
    }

    fn read_file(&self, name: &str) -> anyhow::Result<ByteBuffer> {
        let Some((_, entry)) = self.raw_stat(name) else {
            return Err(anyhow!("File not found: {}", name));
        };
        let mut contents = Vec::with_capacity(entry.length * BLOCK_SIZE);
        for (_, b) in Self::read_chain_raw(&self.image, entry.first_block as u16)? {
            contents.extend_from_slice(&b.as_bytes()[2..]);
        }
        Ok(ByteBuffer::from_vec(contents))
    }

    fn write_file(&mut self, name: &str, contents: &[u8]) -> anyhow::Result<()> {
        DirEntry::encode_filename(name)?;
        _ = self.delete(name); // Can only fail because file-not-found, which is a no-op here.
        let entry_num = self.allocate_dir_entry()?;
        let blocks = (contents.len() + USABLE_BLOCK_SIZE - 1) / USABLE_BLOCK_SIZE;
        let block_list = self.allocate_blocks(blocks as u16)?;

        self.ufd[entry_num] = DirEntry {
            name: Some(name.to_owned()),
            date: Some(Local::now().date_naive()),
            first_block: block_list[0] as usize,
            length: blocks,
            last_block: *block_list.last().unwrap() as usize, // allocate_blocks() should never return an empty array.
        };

        self.write_ufd()?;
        self.write_bitmap()?;

        Self::write_block_chain(&mut self.image, &block_list, contents)?;
        Ok(())
    }

    fn delete(&mut self, name: &str) -> anyhow::Result<()> {
        let Some((entry_num, _)) = self.raw_stat(name) else {
            return Err(anyhow!("File not found: {}", name));
        };
        for (block_num, _) in Self::read_chain_raw(&self.image, self.ufd[entry_num].first_block as u16)?.into_iter() {
            self.bitmap[block_num as usize] = false;
        }
        self.ufd[entry_num].name = None;
        self.write_ufd()?;
        self.write_bitmap()?;
        Ok(())
    }

    fn rename_unchecked(&mut self, src: &str, dest: &str) -> anyhow::Result<()> {
        DirEntry::encode_filename(dest)?;
        let (entry_num, _) = self.raw_stat(src).unwrap(/*we already checked*/);
        self.ufd[entry_num].name = Some(dest.to_owned());
        self.write_ufd()?;
        self.write_bitmap()?; // Might have deleted something
        Ok(())
    }

    fn block_device(&self) -> &Self::BlockDevice {
        &self.image
    }
}


impl<B: BlockDevice> Debug for XxdpFs<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, r#"XXDP FS
Image: blocks={}, bytes={}
MFD:
{:#?}
UFD:
"#,
                &self.image.blocks(), &self.image.blocks() * BLOCK_SIZE, &self.mfd)?;
            for d in self.ufd.iter() {
                write!(f, "{:#?}\n", d)?;
            }
            write!(f, "Bitmap:\n")?;
            for (i, b) in self.bitmap.iter().enumerate() {
                if i > 0 && (i % 64 == 0) { write!(f, "\n")? }
                write!(f, "{}", if *b { "X" } else { "_" })?;
            }
            Ok(())
        } else {
            f.debug_struct("XxdpFs")
                .field("mfd",    &self.mfd    )
                .field("ufd",    &self.ufd    )
                .field("bitmap", &self.bitmap )
                .finish()
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum Mfd {
    VariantOne(MfdVariantOne),
    VariantTwo(MfdVariantTwo),
}

impl Mfd {
    pub fn repr(&self) -> ([u8; BLOCK_SIZE], [u8; BLOCK_SIZE]) {
        match self {
            Mfd::VariantOne(ref v1) => v1.repr(),
            Mfd::VariantTwo(ref v2) => v2.repr(),
        }
    }
}

#[derive(Clone)]
pub struct MfdVariantOne {
    mfd2_block: u16,
    interleave_factor: u16,
    bitmap_block: u16,
    bitmap_pointer: Vec<u16>,
    ufd_block: u16,
}

#[derive(Clone)]
pub struct MfdVariantTwo {
    ufd_block: u16,
    ufd_block_count: u16,
    bitmap_block: u16,
    bitmap_block_count: u16,
    other_mfd_block: u16,
    support_blocks: u16,
    preallocated_blocks: u16,
    interleave_factor: u16,
    monitor_core_image_block: u16,
    bad_sector_file_track: u8,
    bad_sector_file_sector: u8,
    bad_sector_file_cylinder: u16,
}

impl MfdVariantOne {
    pub fn from_repr(_my_block: usize, mut buf: [ByteBuffer; 2]) -> anyhow::Result<MfdVariantOne> {
        buf[0].set_endian(Endian::LittleEndian);
        buf[1].set_endian(Endian::LittleEndian);

        Ok(MfdVariantOne {
            mfd2_block: buf[0].read_u16()?,
            interleave_factor: buf[0].read_u16()?,
            bitmap_block: buf[0].read_u16()?,
            bitmap_pointer: {
                let mut b = vec![];
                while let Some(word) = match buf[0].read_u16()? {
                    0 => None,
                    w => Some(w),
                } { b.push(word); }
                b
            },
            ufd_block: {
                buf[1].set_rpos(2 * size_of::<u16>());
                buf[1].read_u16()?
            },
        })
    }

    pub fn repr(&self) -> ([u8; BLOCK_SIZE], [u8; BLOCK_SIZE]) {
        let mut repr1 = ByteBuffer::new();
        let mut repr2 = ByteBuffer::new();
        repr1.set_endian(Endian::LittleEndian);
        repr2.set_endian(Endian::LittleEndian);

        repr1.write_u16(self.mfd2_block);
        repr1.write_u16(self.interleave_factor);
        repr1.write_u16(self.bitmap_block);
        for b in self.bitmap_pointer.iter() {
            repr1.write_u16(*b);
        }

        repr2.write_u16(0);              // Link zero--no more mfds.
        repr2.write_u16(0o401);          // DOS-11 UIC [1,1].
        repr2.write_u16(self.ufd_block); // pointer to first UFD block
        repr2.write_u16(9);              // Number of words in each UFD entry
        repr2.write_u16(0);              // Terminator?

        repr1.write_bytes(&vec![0; BLOCK_SIZE - repr1.len()]);
        repr2.write_bytes(&vec![0; BLOCK_SIZE - repr2.len()]);
        (repr1.into_vec().try_into().expect("can't happen"),
         repr2.into_vec().try_into().expect("can't happen"))
    }
}

impl MfdVariantTwo {
    #[allow(dead_code)]
    pub fn from_repr(_my_block: u16, mut buf: [ByteBuffer; 2]) -> anyhow::Result<MfdVariantTwo> {
        buf[0].set_endian(Endian::LittleEndian);
        buf[1].set_endian(Endian::LittleEndian);

        buf[0].set_rpos(2);
        Ok(MfdVariantTwo {
            ufd_block: buf[0].read_u16()?,
            ufd_block_count: buf[0].read_u16()?,
            bitmap_block: buf[0].read_u16()?,
            bitmap_block_count: buf[0].read_u16()?,
            other_mfd_block: buf[0].read_u16()?,
            support_blocks: { buf[0].set_rpos(6 * size_of::<u16>()); buf[0].read_u16()? },
            preallocated_blocks: buf[0].read_u16()?,
            interleave_factor: buf[0].read_u16()?,
            monitor_core_image_block: { buf[0].set_rpos(11 * size_of::<u16>()); buf[0].read_u16()? },
            bad_sector_file_track: { buf[0].set_rpos(13 * size_of::<u16>()); buf[0].read_u8()? },
            bad_sector_file_sector: buf[0].read_u8()?,
            bad_sector_file_cylinder: buf[0].read_u16()?,
        })
    }

    pub fn repr(&self) -> ([u8; BLOCK_SIZE], [u8; BLOCK_SIZE]) {
        let mut repr1 = ByteBuffer::new();
        repr1.set_endian(Endian::LittleEndian);

        repr1.write_u16(0);
        repr1.write_u16(self.ufd_block               );
        repr1.write_u16(self.ufd_block_count         );
        repr1.write_u16(self.bitmap_block            );
        repr1.write_u16(self.bitmap_block_count      );
        repr1.write_u16(self.other_mfd_block         ); // Word 5
        repr1.write_u16(0);
        repr1.write_u16(self.support_blocks          );
        repr1.write_u16(self.preallocated_blocks     );
        repr1.write_u16(self.interleave_factor       );
        repr1.write_u16(0);
        repr1.write_u16(self.monitor_core_image_block);
        repr1.write_u16(0);
        repr1.write_u8 (self.bad_sector_file_track   );
        repr1.write_u8 (self.bad_sector_file_sector  );
        repr1.write_u16(self.bad_sector_file_cylinder);
        // I doubt this is right. It says the 2nd one is for double density. But why 2? You can't be both
        // single and double density at the same time, can you? I'm just duping it and hoping for the best.
        repr1.write_u8 (self.bad_sector_file_track   );
        repr1.write_u8 (self.bad_sector_file_sector  );
        repr1.write_u16(self.bad_sector_file_cylinder);

        repr1.write_bytes(&vec![0; BLOCK_SIZE - repr1.len()]);

        // For Variety #2, MDF1 and MFD2 are basically the same (they point to each other in word 8).
        let mut repr2 = repr1.clone();
        repr2.set_endian(Endian::LittleEndian);
        repr2.set_wpos(size_of::<[u16; 5]>());
        repr2.write_u16(1); // other_mfd_block

        (repr1.into_vec().try_into().expect("can't happen"),
         repr2.into_vec().try_into().expect("can't happen"))
    }
}

impl Debug for MfdVariantOne {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, r#"MFD Variant One
mfd2_block        : {},
interleave_factor : {},
bitmap_block      : {},
bitmap_pointer    : {:?},
ufd_block         : {},
"#,
                    &self.mfd2_block,
                    &self.interleave_factor,
                    &self.bitmap_block,
                    &self.bitmap_pointer,
                    &self.ufd_block)
        } else {
            f.debug_struct("MfdVariantOne")
                .field("mfd2_block",        &self.mfd2_block)
                .field("interleave_factor", &self.interleave_factor)
                .field("bitmap_block",      &self.bitmap_block)
                .field("bitmap_pointer",    &self.bitmap_pointer)
                .field("ufd_block",         &self.ufd_block)
                .finish()
        }
    }
}

impl Debug for MfdVariantTwo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, r#"MFD Variant Two
ufd_block                : {}
ufd_block_count          : {}
bitmap_block             : {}
bitmap_block_count       : {}
other_mfd_block          : {}
support_blocks           : {}
preallocated_blocks      : {}
interleave_factor        : {}
monitor_core_image_block : {}
bad_sector_file_track    : {}
bad_sector_file_sector   : {}
bad_sector_file_cylinder : {}
"#,
                &self.ufd_block                ,
                &self.ufd_block_count          ,
                &self.bitmap_block             ,
                &self.bitmap_block_count       ,
                &self.other_mfd_block          ,
                &self.support_blocks           ,
                &self.preallocated_blocks      ,
                &self.interleave_factor        ,
                &self.monitor_core_image_block ,
                &self.bad_sector_file_track    ,
                &self.bad_sector_file_sector   ,
                &self.bad_sector_file_cylinder)
        } else {
            f.debug_struct("MfdVariantTwo")
                .field("ufd_block",                &self.ufd_block               )
                .field("ufd_block_count",          &self.ufd_block_count         )
                .field("bitmap_block",             &self.bitmap_block            )
                .field("bitmap_block_count",       &self.bitmap_block_count      )
                .field("other_mfd_block",          &self.other_mfd_block         )
                .field("support_blocks",           &self.support_blocks          )
                .field("preallocated_blocks",      &self.preallocated_blocks     )
                .field("interleave_factor",        &self.interleave_factor       )
                .field("monitor_core_image_block", &self.monitor_core_image_block)
                .field("bad_sector_file_track",    &self.bad_sector_file_track   )
                .field("bad_sector_file_sector",   &self.bad_sector_file_sector  )
                .field("bad_sector_file_cylinder", &self.bad_sector_file_cylinder)
                .finish()
        }
    }
}

#[derive(Clone, Default)]
pub struct DirEntry {
    name: Option<String>,
    date: Option<chrono::NaiveDate>,
    first_block: usize,
    length: usize,
    last_block: usize,
}

impl DirEntry {
    pub fn from_repr(buf: &mut ByteBuffer) -> anyhow::Result<Option<DirEntry>> {
        if buf.get_rpos() + size_of::<[u16; 9]>() > buf.len() { return Ok(None) }
        buf.set_endian(Endian::LittleEndian);
        let r50_name = [buf.read_u16()?, buf.read_u16()?, buf.read_u16()?];
        let entry = Ok(Some(DirEntry {
            name: if r50_name[0] == 0 && r50_name[1] == 0 && r50_name[2] == 0 {
                None
            } else {
                let raw = radix50::pdp11::decode(&r50_name);
                let (name, ext) = raw.split_at(6);
                Some(format!("{}.{}", name.trim(), ext.trim()))
            },
            date: Self::decode_date(buf.read_u16()?)?,
            first_block: { buf.read_u16()?/* unused */; buf.read_u16()?.into() },
            length: buf.read_u16()?.into(),
            last_block: buf.read_u16()?.into(),
        }));
        buf.read_u16()?; // ACT-11 Logical 52??
        entry
    }

    pub fn repr(&self) -> anyhow::Result<[u8; size_of::<[u16; 9]>()]> {
        let mut repr = ByteBuffer::new();
        repr.set_endian(Endian::LittleEndian);
        for r50 in if let Some(ref name) = self.name { Self::encode_filename(name)? } else { [0,0,0] } {
            repr.write_u16(r50);
        }
        repr.write_u16(Self::encode_date(self.date)?);
        repr.write_u16(0); // Unused
        repr.write_u16(self.first_block as u16);
        repr.write_u16(self.length as u16);
        repr.write_u16(self.last_block as u16);
        repr.write_u16(0); // Unused
        Ok(repr.as_bytes().try_into()?)
    }

    // https://bitsavers.org/pdf/dec/pdp11/dos-batch/DEC-11-OSPMA-A-D_PDP-11_DOS_Monitor_V004A_System_Programmers_Manual_May72.pdf
    // 1000(Year-70)+Date of the Year
    pub fn encode_date(date: Option<NaiveDate>) -> anyhow::Result<u16> {
        let Some(date) = date else { return Ok(0) };
        let yoff = date.year() - 1970;
        if yoff      < 0 { return Err(anyhow!("Date {} is before 1970", date.to_string())) }
        if yoff * 1000 > (1 << 16) { return Err(anyhow!("Date {} is after {}", date.to_string(), 1970 + (0xFFFF / 1000 - 1))) }
        Ok((yoff as u32 * 1000 + date.ordinal()) as u16)
    }
    pub fn decode_date(raw: u16) -> anyhow::Result<Option<NaiveDate>> {
        let year = raw / 1000;
        let julian = raw % 1000;
        Ok(match raw {
            0 => None,
            _ => Some(chrono::NaiveDate::from_yo_opt(1970 + year as i32, julian as u32)
                .ok_or(anyhow!("Invalid date: {:04} + {} days [{:#05x}/{:#012b}]", 1970 + year, julian, raw, raw))?),
        })
    }

    pub fn encode_filename(name: &str) -> anyhow::Result<[u16; 3]> {
        super::rt11::DirEntry::encode_filename(if name == "" { "      .   " } else { name }) // Same as RT-11: 6.3, radix-50 encoded.
    }

}

impl Debug for DirEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "{:<10} [{:#5x}] {:5} @ {:<5} -> {:<5} {}",
                   self.date.map(|d| format!("{}", d)).unwrap_or(format!(" No Date")), DirEntry::encode_date(self.date).unwrap_or(0xfff),
                   self.length,
                   self.first_block,
                   self.last_block,
                   if let Some(name) = &self.name { name } else { " --deleted--" })
        } else {
            write!(f, "{:10} {:6} {}",
                   self.date.map(|d| d.to_string()).unwrap_or(" No Date".to_string()),
                   self.length,
                   if let Some(name) = &self.name { name } else { " --deleted--" })
        }
    }
}

impl super::DirEntry for &DirEntry {
    fn path(&self)       -> &str                             { self.name.as_deref().unwrap_or("") }
    fn file_name(&self)  -> &str                             { self.name.as_deref().unwrap_or("") }
    fn is_dir(&self)     -> bool                             { false }
    fn is_file(&self)    -> bool                             { self.name.is_some() }
    fn is_symlink(&self) -> bool                             { false }
    fn len(&self)        -> u64                              { (self.length * BLOCK_SIZE) as u64 }
    fn modified(&self)   -> anyhow::Result<super::Timestamp> { Err(anyhow!("Not available")) }
    fn accessed(&self)   -> anyhow::Result<super::Timestamp> { Err(anyhow!("Not available")) }
    fn created(&self)    -> anyhow::Result<super::Timestamp> { self.date.map(|d| super::Timestamp::Date(d)).ok_or(anyhow!("Bad Date")) }
    fn blocks(&self)     -> u64                              { self.length as u64}
    fn readonly(&self)   -> bool                             { false }
}


struct BitmapBlock {
    map_number: u16,
    first_bitmap: u16,
    entries: Vec<u16>,
}

impl BitmapBlock {
    fn from_repr(buf: &mut ByteBuffer) -> anyhow::Result<BitmapBlock> {
        buf.set_endian(Endian::LittleEndian);
        Ok(BitmapBlock {
            map_number: buf.read_u16()?,
            first_bitmap: {
                // Map length comes before first_bitmap, but the spec says it's always 60 so we don't bother storing it.
                let map_length = buf.read_u16()?;
                if map_length != BITMAP_WORDS_PER_MAP_BLOCK as u16 { return Err(anyhow!("Map length was {} and not {}", map_length, BITMAP_WORDS_PER_MAP_BLOCK)) }
                buf.read_u16()? // link to first bitmap
            },
            entries: (0..BITMAP_WORDS_PER_MAP_BLOCK).map(|_| buf.read_u16()).collect::<Result<Vec<u16>,_>>()?,
        })
    }

    fn repr(&self) -> anyhow::Result<[u8; USABLE_BLOCK_SIZE]> {
        let mut repr = ByteBuffer::new();
        repr.set_endian(Endian::LittleEndian);

        repr.write_u16(self.map_number);
        repr.write_u16(BITMAP_WORDS_PER_MAP_BLOCK as u16);
        repr.write_u16(self.first_bitmap);
        for w in self.entries.iter() {
            repr.write_u16(*w);
        }
        repr.write_bytes(&vec![0; USABLE_BLOCK_SIZE - repr.len()]);
        Ok(repr.into_vec().try_into().expect("can't happen"))
    }
}

// // Used like: deindent(r#"aaaa
// //                        bbbb
// //                       ");
// // Note the last line is the number of spaces to remove from the other lines (except the first)
// fn deindent(s: &str) -> String {
//     let mut si = s.split('\n');
//     let Some(blank) = si.next_back() else {
//         return s.to_owned();
//     };
//     if blank.len() == 0 { return s.to_owned() }
//     si.map(|line:&str| if line.len() > blank.len() { &line[blank.len()..] } else { line }).collect::<Vec<_>>().join("\n")
// }

#[cfg(test)]
mod test {
    use super::*;
    use crate::fs::test::*;
    use crate::assert_block_eq;

    use pretty_hex::PrettyHex;

    #[test]
    fn test_date() {
        for (y,m,d, encoded) in [(1982,12,08,0x3036),
                                 (1982,12,08,0x3036),
                                 (1989,03,01,0x4a74),
                                 (1982,12,08,0x3036),
                                 (1982,12,08,0x3036),
                                 (1977,12,29,0x1cc3),
                                 (1985,11,22,0x3bde)].into_iter() {
            assert_eq!(NaiveDate::from_ymd_opt(y,m,d), DirEntry::decode_date(encoded).expect("date error"));
            assert_eq!(encoded, DirEntry::encode_date(NaiveDate::from_ymd_opt(y,m,d)).expect("date error"));
        }
    }

    #[test]
    fn test_mkfs() {
        let dev = TestDev(vec![0;512*20]);
        let fs = XxdpFs::mkfs(dev).expect("Create XXDP FS");
        for b in 0..20 {
            match b {
                1 => assert_block_eq!(fs.image, 1,  // MFD1
                                      vec![0x02, 0x00, 0x01, 0x00, 0x04, 0x00, 0x04, 0x00, 0x00, 0x00],
                                      vec![0; 512-10]),
                2 => assert_block_eq!(fs.image, 2,  // MFD2
                                      vec![0x00, 0x00, 0x01, 0x01, 0x03, 0x00, 0x09, 0x00, 0x00, 0x00],
                                      vec![0; 512-10]),
                3 => assert_block_eq!(fs.image, 3,  // UFD
                                      vec![0; 512]),
                4 => assert_block_eq!(fs.image, 4,  // Bitmap
                                      vec![0x00, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, 0x00, 0x1F, 0x00],
                                      vec![0; 512-10]),
                _ => assert_block_eq!(fs.image, b, vec![0; 512]),
            }
        }
    }

    pub(crate) const ______: u32 = 0xffff0000;
    fn words(words: &[u32]) -> Vec<u16> {
        let mut bytes = vec![];
        for w in words {
            bytes.push((w >>  8 & 0xff00 | w >> 0 & 0xff) as u16);
            bytes.push((w >> 16 & 0xff00 | w >> 8 & 0xff) as u16);
        }
        bytes
    }

    #[test]
    fn test_free_spans() {
        let dev = TestDev(vec![0;512*20]);
        assert_eq!(20, dev.blocks());
        let fs = XxdpFs::mkfs(dev).expect("Create XXDP FS");
        let spans = fs.calculate_bitmap_free_spans();
        assert_eq!(vec![5..20], spans);
    }

    #[test]
    fn test_allocate_dir_entry() {
        let dev = TestDev(vec![0;512*20]);
        assert_eq!(20, dev.blocks());
        let mut fs = XxdpFs::mkfs(dev).expect("Create XXDP FS");
        let entry_num = fs.allocate_dir_entry().expect("allocate_dir_entry failed");
        assert_eq!(0, entry_num);
    }

    #[test]
    fn test_write() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = XxdpFs::mkfs(dev).expect("Create XXDP FS");
        fs.write_file("TEST.TST", &incrementing(510)).expect("write_file failed");
        assert_eq!(Some("TEST.TST"), fs.ufd[0].name.as_deref());
        assert_block_eq!(fs.image, 3,  // UFD
                         words(&vec![0x0000, 0x7ddb, 0x7d00, 0x800c, 0xcf1b, 0x0000, 0x0005, 0x0001, 0x0005]),
                         vec![0; 512-18]);
        assert_block_eq!(fs.image, 4,  // Bitmap
                         vec![0x00, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, 0x00, 0x3F, 0x00],
                         vec![0; 512-10]);
        assert_block_eq!(fs.image, 5,  // Data
                         vec![0x00, 0x00], // Next block pointer
                         incrementing(510));
    }

    #[test]
    fn test_overwrite_file() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = XxdpFs::mkfs(dev).expect("Create XXDP FS");
        fs.write_file("TEST.TST", &incrementing(510)).expect("write_file failed");
        fs.write_file("TEST.TST", "david rules".as_bytes()).expect("write_file 2 failed");
        assert_eq!(Some("TEST.TST"), fs.ufd[0].name.as_deref());
        assert_block_eq!(fs.image, 3,  // UFD
                         words(&vec![0x0000, 0x7ddb, 0x7d00, 0x800c, 0xcf1b, 0x0000, 0x0005, 0x0001, 0x0005]),
                         vec![0; 512-18]);
        assert_block_eq!(fs.image, 4,  // Bitmap
                         vec![0x00, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, 0x00, 0x3F, 0x00],
                         vec![0; 512-10]);
        assert_block_eq!(fs.image, 5,  // Data
                         vec![0x00, 0x00], // Next block pointer
                         Vec::from("david rules".as_bytes()));
    }

    #[test]
    fn test_extend_directory() {
        let dev = TestDev(vec![0;512*40]);
        let mut fs = XxdpFs::mkfs(dev).expect("Create XXDP FS");
        for i in 0..28 {
            fs.write_file(&format!("TEST{}.TST",i), &incrementing(510)).expect("write_file failed");
        }
        assert_block_eq!(fs.image, 4,  // Bitmap
                         vec![0x00, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x01],
                         vec![0; 512-13]);
        fs.write_file("TEST28.TST", &incrementing(510)).expect("write_file failed");
        assert_block_eq!(fs.image, 4,  // Bitmap
                         vec![0x00, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x07],
                         vec![0; 512-13]);
        assert_block_eq!(fs.image, 3,  // UFD
                         words(&vec![0x0021, 0x7ddb, ______, 0x800c, 0xcf1b, 0x0000, ______, 0x0001, ______]),
                         vec![____; 512-18]);
        assert_block_eq!(fs.image, 0x21, // New UFD
                         words(&vec![0x0000, 0x7ddb, 0x8226, 0x800c, 0xcf1b, 0x0000, ______, 0x0001, ______]),
                         vec![____; 512-18]);
    }

    #[test]
    fn test_delete() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = XxdpFs::mkfs(dev).expect("Create XXDP FS");
        fs.write_file("TEST.TST", &incrementing(510)).expect("write_file failed");
        fs.write_file("TEST2.TST", &incrementing(510)).expect("write_file failed");
        fs.delete("TEST.TST").expect("Delete failed");
        assert_block_eq!(fs.image, 4,  // Bitmap
                         vec![0x00, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, 0x00, 0x5F, 0x00],
                         vec![0; 512-10]);
        assert_block_eq!(fs.image, 3,  // UFD
                         words(&vec![0x0000, 0x0000, 0x0000, 0x0000, ______, 0x0000, ______, ______, ______]),
                         words(&vec![0x0000, 0x7ddb, 0x8200, 0x800c, 0xcf1b, 0x0000, 0x0006, 0x0001, 0x0006]),
                         vec![0; 512-18*2]);
    }

    #[test]
    fn test_rename() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = XxdpFs::mkfs(dev).expect("Create XXDP FS");
        fs.write_file("TEST.TST", &incrementing(510)).expect("write_file failed");
        fs.rename("TEST.TST", "DAVID.RUL").expect("rename failed");
        assert_block_eq!(fs.image, 3,  // UFD
                         words(&vec![0x0000, 0x193e, 0x38e0, 0x73d4, ______, 0x0000, ______, ______, ______]),
                         vec![0; 512-18*1]);
    }

    #[test]
    fn test_rename_overwrite() {
        let dev = TestDev(vec![0;512*20]);
        let mut fs = XxdpFs::mkfs(dev).expect("Create XXDP FS");
        fs.write_file("TEST.TST", &incrementing(510)).expect("write_file failed");
        fs.write_file("DAVID.RUL", "david rules".as_bytes()).expect("write_file failed");
        fs.rename("TEST.TST", "DAVID.RUL").expect("rename failed");
        assert_block_eq!(fs.image, 3,  // UFD
                         words(&vec![0x0000, 0x193e, 0x38e0, 0x73d4, ______, 0x0000, 0x0005, 0x0001, 0x0005]),
                         words(&vec![0x0000, 0x0000, 0x0000, 0x0000, ______, 0x0000, ______, ______, ______]),
                         vec![0; 512-18*2]);
        assert_block_eq!(fs.image, 5,  // File data
                         words(&vec![0x0000]), // Next block pointer
                         incrementing(510));
    }
}
