//! A minimal TIFF / BigTIFF directory reader for plugins whose formats are
//! TIFF underneath (Zeiss LSM, the TIFFs inside Olympus OIB…).
//!
//! It only walks the IFD chain and decodes tag values; what the tags mean
//! (strips, tiles, compression) is up to the plugin, which turns them into
//! recipe chunks. Bytes come from a caller-supplied reader, so the same
//! code works on the opened file ([`file_reader`]) or on a TIFF held in
//! memory or embedded in a container.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{bail, file, Result};

/// Values longer than this are not loaded (their offset is still known).
const MAX_VALUE: u64 = 64 << 20;
/// Guards against loops in the IFD chain and absurd files.
const MAX_IFDS: usize = 1 << 20;

/// Field types, with their size in bytes.
pub fn type_size(typ: u16) -> u64 {
    match typ {
        1 | 2 | 6 | 7 => 1,              // BYTE, ASCII, SBYTE, UNDEFINED
        3 | 8 => 2,                      // SHORT, SSHORT
        4 | 9 | 11 | 13 => 4,            // LONG, SLONG, FLOAT, IFD
        5 | 10 | 12 | 16 | 17 | 18 => 8, // RATIONAL, SRATIONAL, DOUBLE, LONG8, SLONG8, IFD8
        _ => 0,
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u64,
    /// Where the value is stored, when it does not fit in the entry.
    pub offset: Option<u64>,
    /// The raw value bytes (in the file's byte order); empty when longer than 64 MiB.
    pub value: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Ifd {
    /// Where this directory starts.
    pub offset: u64,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
pub struct Tiff {
    pub little: bool,
    pub big: bool,
    pub ifds: Vec<Ifd>,
}

/// Reads `len` bytes at `offset` of the opened file (see [`parse`]).
pub fn file_reader(offset: u64, len: usize) -> Result<Vec<u8>> {
    file::read(offset, len)
}

struct Endian(bool);

impl Endian {
    fn u16(&self, b: &[u8], o: usize) -> u16 {
        let a = [b[o], b[o + 1]];
        if self.0 {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        }
    }
    fn u32(&self, b: &[u8], o: usize) -> u32 {
        let a = [b[o], b[o + 1], b[o + 2], b[o + 3]];
        if self.0 {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    }
    fn u64(&self, b: &[u8], o: usize) -> u64 {
        let a: [u8; 8] = b[o..o + 8].try_into().unwrap();
        if self.0 {
            u64::from_le_bytes(a)
        } else {
            u64::from_be_bytes(a)
        }
    }
}

/// Parses the header and every IFD of the chain. `read(offset, len)` must
/// return exactly `len` bytes or fail; `size` is the size of the TIFF.
/// `max_ifds` limits how many directories are read (0 = all).
pub fn parse(
    read: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>>,
    size: u64,
    max_ifds: usize,
) -> Result<Tiff> {
    let head = read(0, 16.min(size as usize))?;
    if head.len() < 8 {
        bail!("不是 TIFF 文件");
    }
    let little = match &head[..2] {
        b"II" => true,
        b"MM" => false,
        _ => bail!("不是 TIFF 文件"),
    };
    let e = Endian(little);
    let (big, mut next) = match e.u16(&head, 2) {
        42 => (false, e.u32(&head, 4) as u64),
        43 if head.len() >= 16 => (true, e.u64(&head, 8)),
        _ => bail!("不是 TIFF 文件"),
    };
    let (count_size, entry_size, off_size) = if big {
        (8usize, 20usize, 8usize)
    } else {
        (2, 12, 4)
    };
    let limit = if max_ifds == 0 {
        MAX_IFDS
    } else {
        max_ifds.min(MAX_IFDS)
    };
    let mut ifds = Vec::new();
    let mut seen = Vec::new();
    while next != 0 && next < size && ifds.len() < limit {
        if seen.contains(&next) {
            break;
        }
        seen.push(next);
        let cb = read(next, count_size)?;
        let n = if big {
            e.u64(&cb, 0)
        } else {
            e.u16(&cb, 0) as u64
        };
        if n == 0 || n > 1 << 16 {
            bail!("TIFF 目录损坏（{n} 项）");
        }
        let body = read(next + count_size as u64, n as usize * entry_size + off_size)?;
        let mut entries = Vec::with_capacity(n as usize);
        for k in 0..n as usize {
            let b = &body[k * entry_size..(k + 1) * entry_size];
            let tag = e.u16(b, 0);
            let typ = e.u16(b, 2);
            let count = if big { e.u64(b, 4) } else { e.u32(b, 4) as u64 };
            let field = if big { &b[12..20] } else { &b[8..12] };
            let len = count.saturating_mul(type_size(typ));
            let (offset, value) = if len <= off_size as u64 {
                (None, field[..len as usize].to_vec())
            } else {
                let at = if big {
                    e.u64(field, 0)
                } else {
                    e.u32(field, 0) as u64
                };
                let v = if len <= MAX_VALUE && at.checked_add(len).is_some_and(|end| end <= size) {
                    read(at, len as usize)?
                } else {
                    Vec::new()
                };
                (Some(at), v)
            };
            entries.push(Entry {
                tag,
                typ,
                count,
                offset,
                value,
            });
        }
        let nb = &body[n as usize * entry_size..];
        next = if big {
            e.u64(nb, 0)
        } else {
            e.u32(nb, 0) as u64
        };
        ifds.push(Ifd {
            offset: seen[seen.len() - 1],
            entries,
        });
    }
    if ifds.is_empty() {
        bail!("TIFF 文件里没有图像");
    }
    Ok(Tiff { little, big, ifds })
}

impl Tiff {
    /// The integer values of an entry (BYTE … LONG8, signed ones as their bits).
    pub fn uints(&self, entry: &Entry) -> Vec<u64> {
        let e = Endian(self.little);
        let size = type_size(entry.typ) as usize;
        if size == 0 || entry.value.len() < entry.count as usize * size {
            return Vec::new();
        }
        (0..entry.count as usize)
            .map(|i| match size {
                1 => entry.value[i] as u64,
                2 => e.u16(&entry.value, i * 2) as u64,
                4 => e.u32(&entry.value, i * 4) as u64,
                _ => e.u64(&entry.value, i * 8),
            })
            .collect()
    }

    /// The floating-point values of an entry (also converts integers and rationals).
    pub fn floats(&self, entry: &Entry) -> Vec<f64> {
        let e = Endian(self.little);
        let v = &entry.value;
        let n = entry.count as usize;
        if v.len() < n * type_size(entry.typ) as usize {
            return Vec::new();
        }
        match entry.typ {
            11 => (0..n)
                .map(|i| f32::from_bits(e.u32(v, i * 4)) as f64)
                .collect(),
            12 => (0..n).map(|i| f64::from_bits(e.u64(v, i * 8))).collect(),
            5 => (0..n)
                .map(|i| e.u32(v, i * 8) as f64 / e.u32(v, i * 8 + 4).max(1) as f64)
                .collect(),
            10 => (0..n)
                .map(|i| e.u32(v, i * 8) as i32 as f64 / (e.u32(v, i * 8 + 4) as i32).max(1) as f64)
                .collect(),
            6 => v.iter().map(|&b| b as i8 as f64).collect(),
            8 => (0..n).map(|i| e.u16(v, i * 2) as i16 as f64).collect(),
            9 => (0..n).map(|i| e.u32(v, i * 4) as i32 as f64).collect(),
            17 => (0..n).map(|i| e.u64(v, i * 8) as i64 as f64).collect(),
            _ => self.uints(entry).into_iter().map(|x| x as f64).collect(),
        }
    }
}

impl Ifd {
    pub fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }

    /// First integer value of `tag`.
    pub fn uint(&self, tiff: &Tiff, tag: u16) -> Option<u64> {
        self.get(tag).and_then(|e| tiff.uints(e).first().copied())
    }

    /// All integer values of `tag` (empty when missing).
    pub fn uints(&self, tiff: &Tiff, tag: u16) -> Vec<u64> {
        self.get(tag).map(|e| tiff.uints(e)).unwrap_or_default()
    }

    /// An ASCII value, without the trailing NULs.
    pub fn string(&self, tag: u16) -> Option<String> {
        let e = self.get(tag)?;
        let end = e.value.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
        let s = String::from_utf8_lossy(&e.value[..end]).into_owned();
        (!s.trim().is_empty()).then_some(s)
    }
}

/// Common tags.
pub mod tag {
    pub const NEW_SUBFILE_TYPE: u16 = 254;
    pub const IMAGE_WIDTH: u16 = 256;
    pub const IMAGE_LENGTH: u16 = 257;
    pub const BITS_PER_SAMPLE: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC: u16 = 262;
    pub const IMAGE_DESCRIPTION: u16 = 270;
    pub const STRIP_OFFSETS: u16 = 273;
    pub const SAMPLES_PER_PIXEL: u16 = 277;
    pub const ROWS_PER_STRIP: u16 = 278;
    pub const STRIP_BYTE_COUNTS: u16 = 279;
    pub const PLANAR_CONFIG: u16 = 284;
    pub const PREDICTOR: u16 = 317;
    pub const TILE_WIDTH: u16 = 322;
    pub const TILE_LENGTH: u16 = 323;
    pub const TILE_OFFSETS: u16 = 324;
    pub const TILE_BYTE_COUNTS: u16 = 325;
    pub const SAMPLE_FORMAT: u16 = 339;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A little-endian classic TIFF with one 2x1 u16 image and a long description.
    fn sample() -> Vec<u8> {
        let mut b = vec![];
        b.extend_from_slice(b"II");
        b.extend_from_slice(&42u16.to_le_bytes());
        b.extend_from_slice(&8u32.to_le_bytes());
        let n = 4u16;
        b.extend_from_slice(&n.to_le_bytes());
        let desc_at = 8 + 2 + 12 * 4 + 4;
        let entry = |b: &mut Vec<u8>, tag: u16, typ: u16, count: u32, v: u32| {
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&typ.to_le_bytes());
            b.extend_from_slice(&count.to_le_bytes());
            b.extend_from_slice(&v.to_le_bytes());
        };
        entry(&mut b, 256, 3, 1, 2);
        entry(&mut b, 257, 4, 1, 1);
        entry(&mut b, 258, 3, 1, 16);
        entry(&mut b, 270, 2, 7, desc_at as u32);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"hello!\0");
        b
    }

    #[test]
    fn reads_entries_inline_and_by_offset() {
        let data = sample();
        let size = data.len() as u64;
        let mut read = |o: u64, l: usize| -> Result<Vec<u8>> {
            data.get(o as usize..o as usize + l)
                .map(|s| s.to_vec())
                .ok_or_else(|| "eof".into())
        };
        let t = parse(&mut read, size, 0).unwrap();
        assert!(t.little && !t.big);
        assert_eq!(t.ifds.len(), 1);
        let ifd = &t.ifds[0];
        assert_eq!(ifd.uint(&t, tag::IMAGE_WIDTH), Some(2));
        assert_eq!(ifd.uint(&t, tag::IMAGE_LENGTH), Some(1));
        assert_eq!(ifd.uint(&t, tag::BITS_PER_SAMPLE), Some(16));
        assert_eq!(
            ifd.string(tag::IMAGE_DESCRIPTION).as_deref(),
            Some("hello!")
        );
        assert!(ifd.get(tag::IMAGE_DESCRIPTION).unwrap().offset.is_some());
    }

    #[test]
    fn rejects_garbage() {
        let data = b"not a tiff at all".to_vec();
        let mut read = |o: u64, l: usize| -> Result<Vec<u8>> {
            data.get(o as usize..o as usize + l)
                .map(|s| s.to_vec())
                .ok_or_else(|| "eof".into())
        };
        assert!(parse(&mut read, 17, 0).is_err());
    }
}
