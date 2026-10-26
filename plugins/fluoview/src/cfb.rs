//! A small reader for Microsoft Compound File Binary (OLE2) containers,
//! which Olympus OIB files are. Written from the public [MS-CFB]
//! specification.
//!
//! The file is an array of sectors (512 or 4096 bytes) after a 512-byte
//! header. The FAT links each sector to the next one of its stream; the
//! sectors of the FAT itself are listed by the DIFAT (109 entries in the
//! header, more in a chain of DIFAT sectors). Streams shorter than the
//! cutoff (4096 bytes) live in 64-byte mini sectors inside the root
//! entry's stream, linked by the mini FAT. The directory is a red-black
//! tree of 128-byte entries; we only need each stream's name, start and size.
//!
//! Nothing is read eagerly except the FAT, the mini FAT and the directory:
//! a stream is described as a list of file extents, so pixel data can be
//! handed to the host as file ranges.

use tifmrcview_plugin_sdk::{bail, file, le_u16, le_u32, le_u64, Result};

use crate::common::Piece;

const MAGIC: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const ENDOFCHAIN: u32 = 0xffff_fffe;
const MAXREGSECT: u32 = 0xffff_fffa;
const NOSTREAM: u32 = 0xffff_ffff;

pub fn is_cfb(head: &[u8]) -> bool {
    head.len() >= 8 && head[..8] == MAGIC
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    /// 1 storage, 2 stream, 5 root.
    pub kind: u8,
    pub start: u32,
    pub size: u64,
}

pub struct Cfb {
    sector: u64,
    mini: u64,
    cutoff: u64,
    fat: Vec<u32>,
    minifat: Vec<u32>,
    pub entries: Vec<Entry>,
    /// Where the mini stream (the root entry's stream) lies in the file.
    ministream: Vec<Piece>,
}

impl Cfb {
    pub fn open() -> Result<Cfb> {
        let h = file::read(0, 512).map_err(|_| "OIB 文件头不完整")?;
        if !is_cfb(&h) {
            bail!("不是 OIB 文件（缺少 OLE2 文件头）");
        }
        if le_u16(&h, 0x1c) != 0xfffe {
            bail!("OIB 文件的字节序不支持");
        }
        let shift = le_u16(&h, 0x1e) as u32;
        let mini_shift = le_u16(&h, 0x20) as u32;
        if !(9..=16).contains(&shift) || !(1..shift).contains(&mini_shift) {
            bail!("OIB 文件头损坏（扇区大小）");
        }
        let sector = 1u64 << shift;
        let n_fat = le_u32(&h, 0x2c) as usize;
        let dir_start = le_u32(&h, 0x30);
        let cutoff = le_u32(&h, 0x38) as u64;
        let minifat_start = le_u32(&h, 0x3c);
        let n_minifat = le_u32(&h, 0x40) as usize;
        let mut difat_next = le_u32(&h, 0x44);
        let n_difat = le_u32(&h, 0x48) as usize;
        let n_sectors = file::size().saturating_sub(sector).div_ceil(sector);
        if n_fat as u64 > n_sectors + 1 {
            bail!("OIB 文件头损坏（FAT 扇区数）");
        }

        // DIFAT: where the FAT sectors are.
        let mut difat: Vec<u32> = (0..109).map(|i| le_u32(&h, 0x4c + i * 4)).collect();
        for _ in 0..n_difat {
            if difat.len() >= n_fat || difat_next > MAXREGSECT {
                break;
            }
            let s = read_sector(sector, difat_next)?;
            let k = sector as usize / 4;
            difat.extend((0..k - 1).map(|i| le_u32(&s, i * 4)));
            difat_next = le_u32(&s, (k - 1) * 4);
        }
        difat.truncate(n_fat);
        if difat.len() < n_fat {
            bail!("OIB 文件损坏（DIFAT 不完整）");
        }

        // FAT, read in runs of consecutive sectors.
        let per = sector as usize / 4;
        let mut fat = Vec::with_capacity(n_fat * per);
        let mut i = 0;
        while i < difat.len() {
            let mut j = i + 1;
            while j < difat.len() && difat[j] == difat[j - 1].wrapping_add(1) && j - i < 256 {
                j += 1;
            }
            if difat[i] > MAXREGSECT {
                bail!("OIB 文件损坏（FAT 扇区号无效）");
            }
            let b = file::read((difat[i] as u64 + 1) * sector, (j - i) * sector as usize)
                .map_err(|_| "OIB 文件被截断（FAT）")?;
            fat.extend(b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])));
            i = j;
        }

        let mut cfb =
            Cfb { sector, mini: 1 << mini_shift, cutoff, fat, minifat: vec![], entries: vec![], ministream: vec![] };

        // Directory.
        let dir = cfb.chain_pieces(dir_start, u64::MAX)?;
        let mut raw = vec![];
        for p in &dir {
            raw.extend(file::read(p.file, p.len as usize).map_err(|_| "OIB 文件被截断（目录）")?);
        }
        for e in raw.chunks_exact(128) {
            let name_len = (le_u16(e, 64) as usize).min(64);
            let units = e[..name_len.saturating_sub(2)].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
            let name: String = char::decode_utf16(units).map(|c| c.unwrap_or('\u{fffd}')).collect();
            let mut size = le_u64(e, 120);
            if shift == 9 {
                // Version 3 files only use the low 32 bits.
                size &= 0xffff_ffff;
            }
            cfb.entries.push(Entry {
                name,
                kind: e[66],
                start: le_u32(e, 116),
                size,
            });
        }
        let Some(root) = cfb.entries.first().cloned().filter(|r| r.kind == 5) else {
            bail!("OIB 文件损坏（没有根目录）");
        };

        // Mini FAT and mini stream.
        let mf = cfb.chain_pieces(minifat_start, n_minifat as u64 * sector)?;
        for p in &mf {
            let b = file::read(p.file, p.len as usize).map_err(|_| "OIB 文件被截断（mini FAT）")?;
            cfb.minifat.extend(b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])));
        }
        cfb.ministream = cfb.chain_pieces(root.start, root.size)?;
        Ok(cfb)
    }

    /// The file extents of a sector chain, merged where consecutive, up to `size` bytes.
    fn chain_pieces(&self, start: u32, size: u64) -> Result<Vec<Piece>> {
        let mut out: Vec<Piece> = vec![];
        let mut s = start;
        let mut at = 0u64;
        let mut steps = 0usize;
        while s != ENDOFCHAIN && at < size {
            if s > MAXREGSECT || s as usize >= self.fat.len() {
                if s == NOSTREAM && out.is_empty() {
                    break;
                }
                bail!("OIB 文件损坏（扇区链无效）");
            }
            steps += 1;
            if steps > self.fat.len() {
                bail!("OIB 文件损坏（扇区链循环）");
            }
            let pos = (s as u64 + 1) * self.sector;
            let len = self.sector.min(size - at);
            match out.last_mut() {
                Some(p) if p.file + p.len == pos => p.len += len,
                _ => out.push(Piece { at, file: pos, len }),
            }
            at += len;
            s = self.fat[s as usize];
        }
        Ok(out)
    }

    /// Where the bytes of a stream are in the file (logical offset `at` within the stream).
    pub fn pieces(&self, e: &Entry) -> Result<Vec<Piece>> {
        let size = e.size;
        let pieces = if size < self.cutoff {
            let mut out: Vec<Piece> = vec![];
            let mut s = e.start;
            let mut at = 0u64;
            let mut steps = 0usize;
            while s != ENDOFCHAIN && at < size {
                if s as usize >= self.minifat.len() {
                    bail!("OIB 文件损坏（mini 扇区链无效）");
                }
                steps += 1;
                if steps > self.minifat.len() {
                    bail!("OIB 文件损坏（mini 扇区链循环）");
                }
                let len = self.mini.min(size - at);
                let pos = locate(&self.ministream, s as u64 * self.mini).ok_or("OIB 文件损坏（mini 流越界）")?;
                match out.last_mut() {
                    Some(p) if p.file + p.len == pos => p.len += len,
                    _ => out.push(Piece { at, file: pos, len }),
                }
                at += len;
                s = self.minifat[s as usize];
            }
            out
        } else {
            self.chain_pieces(e.start, size)?
        };
        let have: u64 = pieces.iter().map(|p| p.len).sum();
        if have < size {
            bail!("OIB 文件损坏：流「{}」不完整", e.name);
        }
        Ok(pieces)
    }

    /// The first stream called `name`.
    pub fn stream(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.kind == 2 && e.name == name)
    }

    /// Reads a whole stream (at most `max` bytes).
    pub fn read_stream(&self, e: &Entry, max: usize) -> Result<Vec<u8>> {
        let pieces = self.pieces(e)?;
        read_pieces(&pieces, 0, (e.size as usize).min(max))
    }
}

fn read_sector(sector: u64, s: u32) -> Result<Vec<u8>> {
    file::read((s as u64 + 1) * sector, sector as usize).map_err(|_| "OIB 文件被截断".into())
}

/// File offset of logical offset `at`.
pub fn locate(pieces: &[Piece], at: u64) -> Option<u64> {
    let i = pieces.partition_point(|p| p.at + p.len <= at);
    let p = pieces.get(i)?;
    (p.at <= at).then(|| p.file + (at - p.at))
}

/// The pieces covering logical bytes `at..at+len`, re-based so that `at` is 0.
pub fn sub_pieces(pieces: &[Piece], at: u64, len: u64) -> Vec<Piece> {
    let end = at + len;
    let i = pieces.partition_point(|p| p.at + p.len <= at);
    let mut out = vec![];
    for p in &pieces[i..] {
        if p.at >= end {
            break;
        }
        let (a, b) = (p.at.max(at), (p.at + p.len).min(end));
        if a < b {
            out.push(Piece { at: a - at, file: p.file + (a - p.at), len: b - a });
        }
    }
    out
}

/// Reads logical bytes `at..at+len` (fewer at the end of the data).
pub fn read_pieces(pieces: &[Piece], at: u64, len: usize) -> Result<Vec<u8>> {
    let sub = sub_pieces(pieces, at, len as u64);
    let total = sub.last().map_or(0, |p| p.at + p.len) as usize;
    let mut out = vec![0u8; total.min(len)];
    for p in sub {
        let n = file::read_into(p.file, &mut out[p.at as usize..(p.at + p.len) as usize])?;
        if n < p.len as usize {
            bail!("文件被截断");
        }
    }
    Ok(out)
}
