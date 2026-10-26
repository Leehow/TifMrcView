//! A minimal, read-only HDF5 reader: just enough of the format to walk
//! groups, read attributes and find the chunks of a dataset. Written from
//! the public "HDF5 File Format Specification Version 3.0" (The HDF Group).
//!
//! Supported: superblock versions 0–3; object headers version 1 and 2 with
//! continuation blocks; old-style groups (symbol table: v1 B-tree + local
//! heap) and new-style groups (link messages, or dense storage in a
//! fractal heap indexed by a v2 B-tree); attributes, compact and dense;
//! datasets with compact, contiguous or chunked layout, with chunks indexed
//! by a v1 B-tree (layout v1–v3) or, for layout v4, as a single chunk, an
//! implicit index or a fixed array. Not supported: extensible-array and
//! v2-B-tree chunk indexes (datasets with unlimited dimensions), filtered
//! fractal heaps, huge heap objects, shared/committed datatypes in
//! attributes, variable-length strings, external and virtual storage.
//!
//! Every read goes through [`Source`], so the same code runs in the plugin
//! (host file reads) and in native unit tests (a byte vector).

use std::cell::RefCell;

use tifmrcview_plugin_sdk::{bail, Error, Result};

const SIGNATURE: &[u8; 8] = b"\x89HDF\r\n\x1a\n";

/// Where the bytes come from.
pub trait Source {
    fn size(&self) -> u64;
    /// Up to `len` bytes at `off` (fewer only at the end of the file).
    fn read(&self, off: u64, len: usize) -> Result<Vec<u8>>;
}

impl Source for Vec<u8> {
    fn size(&self) -> u64 {
        self.len() as u64
    }
    fn read(&self, off: u64, len: usize) -> Result<Vec<u8>> {
        let s = (off as usize).min(self.len());
        Ok(self[s..(s + len).min(self.len())].to_vec())
    }
}

fn corrupt() -> Error {
    Error("HDF5 文件结构损坏".into())
}

// ---------------------------------------------------------------- reading

const PAGE: u64 = 1 << 16;
const CACHED_PAGES: usize = 64;

/// Metadata is scattered in small pieces; read it in 64 KiB pages and keep
/// the last few, so walking a group costs a handful of host reads.
struct Cache<S> {
    src: S,
    pages: RefCell<Vec<(u64, Vec<u8>)>>,
}

impl<S: Source> Cache<S> {
    fn read(&self, off: u64, len: usize) -> Result<Vec<u8>> {
        let end = off.checked_add(len as u64).filter(|&e| e <= self.src.size());
        let Some(end) = end else { bail!("HDF5 文件被截断（需要读取位置 {off} 起的 {len} 字节）") };
        if len as u64 > 4 * PAGE {
            let v = self.src.read(off, len)?;
            if v.len() != len {
                return Err(corrupt());
            }
            return Ok(v);
        }
        let mut out = Vec::with_capacity(len);
        let mut pages = self.pages.borrow_mut();
        let mut p = off;
        while p < end {
            let pg = p / PAGE;
            let i = match pages.iter().position(|(k, _)| *k == pg) {
                Some(i) => i,
                None => {
                    if pages.len() >= CACHED_PAGES {
                        pages.remove(0);
                    }
                    pages.push((pg, self.src.read(pg * PAGE, PAGE as usize)?));
                    pages.len() - 1
                }
            };
            let data = &pages[i].1;
            let s = (p - pg * PAGE) as usize;
            let e = ((end - pg * PAGE) as usize).min(data.len());
            if s >= e {
                return Err(corrupt());
            }
            out.extend_from_slice(&data[s..e]);
            p += (e - s) as u64;
        }
        Ok(out)
    }

    /// Up to `len` bytes, fewer at the end of the file.
    fn read_most(&self, off: u64, len: usize) -> Result<Vec<u8>> {
        let len = len.min(self.src.size().saturating_sub(off) as usize);
        self.read(off, len)
    }
}

/// Little-endian integer of `b.len()` bytes.
pub fn uv(b: &[u8]) -> u64 {
    b.iter().rev().fold(0u64, |a, &x| a << 8 | x as u64)
}

/// A bounds-checked cursor over a byte slice.
pub struct Cur<'a> {
    pub b: &'a [u8],
    pub p: usize,
}

impl<'a> Cur<'a> {
    pub fn new(b: &'a [u8]) -> Cur<'a> {
        Cur { b, p: 0 }
    }
    pub fn at(b: &'a [u8], p: usize) -> Cur<'a> {
        Cur { b, p }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n).ok_or_else(corrupt)?).ok_or_else(corrupt)?;
        self.p += n;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(uv(self.take(2)?) as u16)
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(uv(self.take(4)?) as u32)
    }
    pub fn uv(&mut self, n: usize) -> Result<u64> {
        if n > 8 {
            return Err(corrupt());
        }
        Ok(uv(self.take(n)?))
    }
    pub fn left(&self) -> usize {
        self.b.len().saturating_sub(self.p)
    }
}

/// floor(log2(n)), 0 for 0.
fn log2(n: u64) -> u32 {
    63 - n.max(1).leading_zeros()
}

/// Bytes needed to encode values up to `n` (H5 "limit encode size").
fn enc_size(n: u64) -> usize {
    (log2(n) / 8 + 1) as usize
}

// ---------------------------------------------------------------- types

/// One header message.
pub struct Msg {
    pub typ: u16,
    pub flags: u8,
    pub data: Vec<u8>,
}

pub const MSG_DATASPACE: u16 = 0x01;
pub const MSG_LINK_INFO: u16 = 0x02;
pub const MSG_DATATYPE: u16 = 0x03;
pub const MSG_LINK: u16 = 0x06;
pub const MSG_LAYOUT: u16 = 0x08;
pub const MSG_FILTERS: u16 = 0x0b;
pub const MSG_ATTRIBUTE: u16 = 0x0c;
pub const MSG_CONTINUATION: u16 = 0x10;
pub const MSG_SYMBOL_TABLE: u16 = 0x11;
pub const MSG_ATTRIBUTE_INFO: u16 = 0x15;

/// An object header's messages.
pub struct Object {
    pub msgs: Vec<Msg>,
}

impl Object {
    pub fn msg(&self, typ: u16) -> Option<&Msg> {
        self.msgs.iter().find(|m| m.typ == typ)
    }
}

/// A datatype (only what we need: class, size, sign and byte order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Datatype {
    /// 0 fixed-point, 1 floating-point, 3 string, 9 variable-length…
    pub class: u8,
    pub bits: u32,
    pub size: u32,
}

impl Datatype {
    pub fn parse(d: &[u8]) -> Result<Datatype> {
        let mut c = Cur::new(d);
        let cv = c.u8()?;
        let bits = uv(c.take(3)?) as u32;
        let size = c.u32()?;
        Ok(Datatype { class: cv & 0x0f, bits, size })
    }
    pub fn big_endian(&self) -> bool {
        matches!(self.class, 0 | 1) && self.bits & 1 != 0
    }
    pub fn signed(&self) -> bool {
        self.class == 0 && self.bits & 8 != 0
    }
}

/// Dimension sizes of a dataspace; `None` for a null dataspace.
pub fn parse_dataspace(d: &[u8], sl: usize) -> Result<Option<Vec<u64>>> {
    let mut c = Cur::new(d);
    let ver = c.u8()?;
    let rank = c.u8()? as usize;
    let _flags = c.u8()?;
    match ver {
        1 => c.skip(5)?,
        2 => {
            if c.u8()? == 2 {
                return Ok(None);
            }
        }
        _ => bail!("不支持的 HDF5 数据空间版本 {ver}"),
    }
    (0..rank).map(|_| c.uv(sl)).collect::<Result<Vec<u64>>>().map(Some)
}

/// How a dataset's chunks are indexed.
#[derive(Clone, Debug)]
pub enum Index {
    /// Version 1 B-tree (layout versions 1–3).
    BTree1(u64),
    /// Layout v4, one chunk: address, stored size (0 = unfiltered) and filter mask.
    Single(u64, u64, u32),
    /// Layout v4, chunks stored back to back in row-major order.
    Implicit(u64),
    /// Layout v4, fixed array.
    FixedArray(u64),
    /// Layout v4 extensible array / v2 B-tree (not supported).
    Other(u8),
}

#[derive(Clone, Debug)]
pub enum Layout {
    Compact,
    /// Address (`None`: never written).
    Contiguous(Option<u64>),
    /// Chunk dimensions (dataset rank, without the element-size dimension).
    Chunked(Vec<u64>, Index),
}

/// One filter of a pipeline: id and client values.
#[derive(Clone, Debug)]
pub struct Filter {
    pub id: u16,
    pub values: Vec<u32>,
}

pub const FILTER_DEFLATE: u16 = 1;
pub const FILTER_SHUFFLE: u16 = 2;
pub const FILTER_FLETCHER32: u16 = 3;
pub const FILTER_LZ4: u16 = 32004;

#[derive(Clone, Debug)]
pub struct Dataset {
    pub dims: Vec<u64>,
    pub dtype: Datatype,
    pub layout: Layout,
    pub filters: Vec<Filter>,
}

/// One stored chunk: its first element, absolute file offset, stored size
/// and filter mask (bit i set: filter i was not applied).
#[derive(Clone, Debug)]
pub struct ChunkRef {
    pub start: Vec<u64>,
    pub addr: u64,
    pub size: u64,
    pub mask: u32,
}

/// A fractal heap (the parts needed to find managed and tiny objects).
struct Heap {
    id_len: usize,
    /// v2 B-tree of huge objects.
    huge: u64,
    width: u64,
    start: u64,
    max_direct_rows: u64,
    root: u64,
    root_rows: u64,
    off_size: usize,
    len_size: usize,
}

// ---------------------------------------------------------------- file

pub struct H5<S> {
    r: Cache<S>,
    /// Size of offsets (addresses) and lengths, in bytes.
    pub so: usize,
    pub sl: usize,
    /// Base address; all addresses in the file are relative to it.
    pub base: u64,
    /// Root group object header.
    pub root: u64,
}

impl<S: Source> H5<S> {
    pub fn open(src: S) -> Result<H5<S>> {
        // The superblock is at 0, 512, 1024, 2048…
        let size = src.size();
        let mut at = 0u64;
        loop {
            if at + 8 > size {
                bail!("不是 HDF5 文件");
            }
            if src.read(at, 8)? == SIGNATURE {
                break;
            }
            at = if at == 0 { 512 } else { at * 2 };
        }
        let r = Cache { src, pages: RefCell::new(vec![]) };
        let b = r.read_most(at, 256)?;
        let ver = *b.get(8).ok_or_else(corrupt)?;
        let (so, sl, mut c) = match ver {
            0 | 1 => (b[13] as usize, b[14] as usize, Cur::at(&b, if ver == 0 { 24 } else { 28 })),
            2 | 3 => (b[9] as usize, b[10] as usize, Cur::at(&b, 12)),
            _ => bail!("不支持的 HDF5 超级块版本 {ver}"),
        };
        if ![2, 4, 8].contains(&so) || ![2, 4, 8].contains(&sl) {
            return Err(corrupt());
        }
        let base = c.uv(so)?;
        let root = if ver < 2 {
            // Free-space info, end of file, driver info, then the root
            // group's symbol table entry (link name offset, header address).
            c.skip(3 * so)?;
            c.uv(so)?;
            c.uv(so)?
        } else {
            // Superblock extension, end of file, root group header.
            c.skip(2 * so)?;
            c.uv(so)?
        };
        Ok(H5 { r, so, sl, base, root })
    }

    fn undef(&self, a: u64) -> bool {
        a == if self.so >= 8 { u64::MAX } else { (1u64 << (8 * self.so)) - 1 }
    }

    /// `len` bytes at relative address `addr`.
    fn rd(&self, addr: u64, len: usize) -> Result<Vec<u8>> {
        self.r.read(self.base.checked_add(addr).ok_or_else(corrupt)?, len)
    }

    fn rd_most(&self, addr: u64, len: usize) -> Result<Vec<u8>> {
        self.r.read_most(self.base.checked_add(addr).ok_or_else(corrupt)?, len)
    }

    /// Absolute file offset of a relative address.
    pub fn abs(&self, addr: u64) -> u64 {
        self.base + addr
    }

    // ------------------------------------------------------------ objects

    /// Reads the object header at `addr`, following continuation blocks.
    pub fn object(&self, addr: u64) -> Result<Object> {
        let head = self.rd_most(addr, 32)?;
        let mut msgs = vec![];
        let mut conts = vec![];
        let v2 = head.starts_with(b"OHDR");
        let mut corder = false;
        if v2 {
            let mut c = Cur::at(&head, 4);
            if c.u8()? != 2 {
                return Err(corrupt());
            }
            let flags = c.u8()?;
            if flags & 0x20 != 0 {
                c.skip(16)?;
            }
            if flags & 0x10 != 0 {
                c.skip(4)?;
            }
            corder = flags & 0x04 != 0;
            let n = c.uv(1 << (flags & 3))? as usize;
            let data = self.rd(addr + c.p as u64, n)?;
            self.messages_v2(&data, corder, &mut msgs, &mut conts)?;
        } else if head.first() == Some(&1) {
            let n = uv(head.get(8..12).ok_or_else(corrupt)?) as usize;
            let data = self.rd(addr + 16, n)?;
            self.messages_v1(&data, &mut msgs, &mut conts)?;
        } else {
            bail!("HDF5 对象头格式未知（位置 {addr}）");
        }
        let mut i = 0;
        while i < conts.len() {
            if i > 10_000 {
                return Err(corrupt());
            }
            let (a, len) = conts[i];
            let d = self.rd(a, len as usize)?;
            if v2 {
                if !d.starts_with(b"OCHK") || d.len() < 8 {
                    return Err(corrupt());
                }
                self.messages_v2(&d[4..d.len() - 4], corder, &mut msgs, &mut conts)?;
            } else {
                self.messages_v1(&d, &mut msgs, &mut conts)?;
            }
            i += 1;
        }
        Ok(Object { msgs })
    }

    fn messages_v1(&self, d: &[u8], msgs: &mut Vec<Msg>, conts: &mut Vec<(u64, u64)>) -> Result<()> {
        let mut c = Cur::new(d);
        while c.left() >= 8 {
            let typ = c.u16()?;
            let size = c.u16()? as usize;
            let flags = c.u8()?;
            c.skip(3)?;
            let data = c.take(size)?;
            self.push_msg(typ, flags, data, msgs, conts)?;
        }
        Ok(())
    }

    fn messages_v2(&self, d: &[u8], corder: bool, msgs: &mut Vec<Msg>, conts: &mut Vec<(u64, u64)>) -> Result<()> {
        let mut c = Cur::new(d);
        let head = if corder { 6 } else { 4 };
        while c.left() >= head {
            let typ = c.u8()? as u16;
            let size = c.u16()? as usize;
            let flags = c.u8()?;
            if corder {
                c.skip(2)?;
            }
            if size > c.left() {
                break;
            }
            let data = c.take(size)?;
            self.push_msg(typ, flags, data, msgs, conts)?;
        }
        Ok(())
    }

    fn push_msg(&self, typ: u16, flags: u8, data: &[u8], msgs: &mut Vec<Msg>, conts: &mut Vec<(u64, u64)>) -> Result<()> {
        match typ {
            0 => {}
            MSG_CONTINUATION => {
                let mut c = Cur::new(data);
                let a = c.uv(self.so)?;
                let len = c.uv(self.sl)?;
                conts.push((a, len));
            }
            _ => msgs.push(Msg { typ, flags, data: data.to_vec() }),
        }
        Ok(())
    }

    // ------------------------------------------------------------ groups

    /// The hard links of a group: (name, object header address), in no
    /// particular order. Soft and external links are left out.
    pub fn links(&self, obj: &Object) -> Result<Vec<(String, u64)>> {
        let mut out = vec![];
        for m in &obj.msgs {
            match m.typ {
                MSG_SYMBOL_TABLE => {
                    let mut c = Cur::new(&m.data);
                    let btree = c.uv(self.so)?;
                    let heap = c.uv(self.so)?;
                    self.symbol_table(btree, heap, &mut out)?;
                }
                MSG_LINK => self.parse_link(&m.data, &mut out)?,
                MSG_LINK_INFO => {
                    let mut c = Cur::new(&m.data);
                    c.u8()?;
                    let flags = c.u8()?;
                    if flags & 1 != 0 {
                        c.skip(8)?;
                    }
                    let heap = c.uv(self.so)?;
                    let names = c.uv(self.so)?;
                    if !self.undef(heap) && !self.undef(names) {
                        let h = self.heap(heap)?;
                        for rec in self.btree2(names)? {
                            // Link name index record: name hash, heap ID.
                            let obj = self.heap_object(&h, rec.get(4..).ok_or_else(corrupt)?)?;
                            self.parse_link(&obj, &mut out)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    fn parse_link(&self, d: &[u8], out: &mut Vec<(String, u64)>) -> Result<()> {
        let mut c = Cur::new(d);
        if c.u8()? != 1 {
            bail!("不支持的 HDF5 链接消息版本");
        }
        let flags = c.u8()?;
        let typ = if flags & 0x08 != 0 { c.u8()? } else { 0 };
        if flags & 0x04 != 0 {
            c.skip(8)?;
        }
        if flags & 0x10 != 0 {
            c.skip(1)?;
        }
        let n = c.uv(1 << (flags & 3))? as usize;
        let name = text(c.take(n)?);
        if typ == 0 {
            out.push((name, c.uv(self.so)?));
        }
        Ok(())
    }

    /// Old-style group: a v1 B-tree of symbol table nodes, names in a local heap.
    fn symbol_table(&self, btree: u64, heap: u64, out: &mut Vec<(String, u64)>) -> Result<()> {
        let h = self.rd(heap, 8 + 2 * self.sl + self.so)?;
        if !h.starts_with(b"HEAP") {
            return Err(corrupt());
        }
        let mut c = Cur::at(&h, 8);
        let size = c.uv(self.sl)? as usize;
        c.uv(self.sl)?;
        let data_addr = c.uv(self.so)?;
        let names = self.rd(data_addr, size)?;
        let mut nodes = vec![];
        self.btree1(btree, 0, 0, &mut |_key, child| nodes.push(child))?;
        let entry = 2 * self.so + 24;
        for node in nodes {
            let head = self.rd(node, 8)?;
            if !head.starts_with(b"SNOD") {
                return Err(corrupt());
            }
            let n = uv(&head[6..8]) as usize;
            let d = self.rd(node + 8, n * entry)?;
            for e in d.chunks_exact(entry) {
                let name_off = uv(&e[..self.so]) as usize;
                let obj = uv(&e[self.so..2 * self.so]);
                let rest = names.get(name_off..).ok_or_else(corrupt)?;
                let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
                out.push((text(&rest[..end]), obj));
            }
        }
        Ok(())
    }

    /// Walks a version 1 B-tree of node type `typ` (0 group, 1 chunks) and
    /// calls `f(key, child)` for every leaf entry. `ndims` is the chunk
    /// key's dimensionality (type 1).
    fn btree1(&self, addr: u64, typ: u8, ndims: usize, f: &mut dyn FnMut(&[u8], u64)) -> Result<()> {
        let key = if typ == 0 { self.sl } else { 8 + 8 * ndims };
        let mut stack = vec![(addr, 0u32)];
        while let Some((a, depth)) = stack.pop() {
            if depth > 64 {
                return Err(corrupt());
            }
            let head = self.rd(a, 8 + 2 * self.so)?;
            if !head.starts_with(b"TREE") || head[4] != typ {
                return Err(corrupt());
            }
            let level = head[5];
            let n = uv(&head[6..8]) as usize;
            let body = self.rd(a + 8 + 2 * self.so as u64, n * (key + self.so) + key)?;
            for i in 0..n {
                let k = &body[i * (key + self.so)..][..key];
                let child = uv(&body[i * (key + self.so) + key..][..self.so]);
                if level == 0 {
                    f(k, child);
                } else {
                    stack.push((child, depth + 1));
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ v2 B-trees and fractal heaps

    /// All records of a version 2 B-tree.
    fn btree2(&self, addr: u64) -> Result<Vec<Vec<u8>>> {
        let h = self.rd(addr, 16 + self.so + 2 + self.sl + 4)?;
        if !h.starts_with(b"BTHD") {
            return Err(corrupt());
        }
        let mut c = Cur::at(&h, 6);
        let node_size = c.u32()? as u64;
        let rec = c.u16()? as u64;
        let depth = c.u16()? as usize;
        c.skip(2)?;
        let root = c.uv(self.so)?;
        let nroot = c.u16()? as usize;
        if rec == 0 || node_size <= 10 || depth > 16 {
            return Err(corrupt());
        }
        // Sizes of the record counts in internal nodes.
        let so = self.so as u64;
        let max_leaf = (node_size - 10) / rec;
        let nrec_size = enc_size(max_leaf);
        let mut cum = vec![max_leaf];
        let mut cum_size = vec![0usize];
        for d in 1..=depth {
            let ptr = so + nrec_size as u64 + if d > 1 { cum_size[d - 1] as u64 } else { 0 };
            let max = (node_size - 10).saturating_sub(ptr) / (rec + ptr);
            cum.push((max + 1) * cum[d - 1] + max);
            cum_size.push(enc_size(cum[d]));
        }
        let mut out = vec![];
        if self.undef(root) {
            return Ok(out);
        }
        let mut stack = vec![(root, nroot, depth)];
        while let Some((a, n, d)) = stack.pop() {
            if out.len() > 10_000_000 {
                return Err(corrupt());
            }
            let ptr = self.so + nrec_size + if d > 1 { cum_size[d - 1] } else { 0 };
            let len = 6 + n * rec as usize + if d > 0 { (n + 1) * ptr } else { 0 };
            let b = self.rd(a, len)?;
            let sig: &[u8] = if d > 0 { b"BTIN" } else { b"BTLF" };
            if !b.starts_with(sig) {
                return Err(corrupt());
            }
            for i in 0..n {
                out.push(b[6 + i * rec as usize..][..rec as usize].to_vec());
            }
            if d > 0 {
                let mut c = Cur::at(&b, 6 + n * rec as usize);
                for _ in 0..=n {
                    let child = c.uv(self.so)?;
                    let cn = c.uv(nrec_size)? as usize;
                    if d > 1 {
                        c.uv(cum_size[d - 1])?;
                    }
                    stack.push((child, cn, d - 1));
                }
            }
        }
        Ok(out)
    }

    fn heap(&self, addr: u64) -> Result<Heap> {
        let (so, sl) = (self.so, self.sl);
        let len = 4 + 1 + 2 + 2 + 1 + 4 + 12 * sl + 3 * so + 2 + 2 + 2 + 2;
        let h = self.rd(addr, len)?;
        if !h.starts_with(b"FRHP") {
            return Err(corrupt());
        }
        let mut c = Cur::at(&h, 5);
        let id_len = c.u16()? as usize;
        let filters = c.u16()?;
        let _flags = c.u8()?;
        let max_man = c.u32()? as u64;
        c.skip(sl)?;
        let huge = c.uv(so)?;
        // Free space, free-space manager, managed space, allocated, iterator
        // offset, managed/huge/tiny counts and sizes.
        c.skip(sl + so + 8 * sl)?;
        let width = c.u16()? as u64;
        let start = c.uv(sl)?;
        let max_direct = c.uv(sl)?;
        let max_bits = c.u16()? as u32;
        let _start_rows = c.u16()?;
        let root = c.uv(so)?;
        let root_rows = c.u16()? as u64;
        if filters != 0 {
            bail!("不支持压缩过的 HDF5 堆");
        }
        if width == 0 || start == 0 || max_direct < start {
            return Err(corrupt());
        }
        Ok(Heap {
            id_len,
            huge,
            width,
            start,
            max_direct_rows: (log2(max_direct) - log2(start) + 2) as u64,
            root,
            root_rows,
            off_size: max_bits.div_ceil(8) as usize,
            len_size: ((log2(max_direct) as usize).div_ceil(8)).min(enc_size(max_man)),
        })
    }

    /// The bytes of a fractal heap object, given its heap ID.
    fn heap_object(&self, h: &Heap, id: &[u8]) -> Result<Vec<u8>> {
        let first = *id.first().ok_or_else(corrupt)?;
        match (first >> 4) & 3 {
            0 => {
                let mut c = Cur::at(id, 1);
                let off = c.uv(h.off_size)?;
                let len = c.uv(h.len_size)? as usize;
                let addr = self.heap_address(h, off)?;
                self.rd(addr, len)
            }
            1 => {
                // Huge object, stored on its own: its address and length are
                // in the ID when they fit, else in the huge-object B-tree
                // (records: address, length, ID).
                let (so, sl) = (self.so, self.sl);
                let (addr, len) = if h.id_len > so + sl {
                    let mut c = Cur::at(id, 1);
                    (c.uv(so)?, c.uv(sl)?)
                } else {
                    let key = uv(id.get(1..h.id_len.min(9)).ok_or_else(corrupt)?);
                    let rec = self.btree2(h.huge)?.into_iter().find(|r| r.len() >= so + 2 * sl && uv(&r[so + sl..so + 2 * sl]) == key);
                    let rec = rec.ok_or_else(corrupt)?;
                    (uv(&rec[..so]), uv(&rec[so..so + sl]))
                };
                self.rd(addr, len as usize)
            }
            2 => {
                let len = (first & 0x0f) as usize + 1;
                Ok(id.get(1..1 + len).ok_or_else(corrupt)?.to_vec())
            }
            _ => Err(corrupt()),
        }
    }

    /// File address of heap offset `off`, through the doubling table.
    fn heap_address(&self, h: &Heap, off: u64) -> Result<u64> {
        if h.root_rows == 0 {
            return Ok(h.root + off);
        }
        let (mut block, mut rows, mut base) = (h.root, h.root_rows, 0u64);
        for _ in 0..64 {
            let hdr = 5 + self.so + h.off_size;
            let direct = rows.min(h.max_direct_rows) * h.width;
            let indirect = rows * h.width - direct;
            let b = self.rd(block, hdr + (direct + indirect) as usize * self.so)?;
            if !b.starts_with(b"FHIB") {
                return Err(corrupt());
            }
            let mut pos = base;
            let mut next = None;
            'rows: for r in 0..rows {
                let size = if r == 0 { h.start } else { h.start << (r - 1) };
                for col in 0..h.width {
                    let k = (r * h.width + col) as usize;
                    if off >= pos && off < pos + size {
                        let a = uv(&b[hdr + k * self.so..][..self.so]);
                        if self.undef(a) {
                            return Err(corrupt());
                        }
                        if r < h.max_direct_rows {
                            return Ok(a + (off - pos));
                        }
                        let child_rows = (log2(size) - log2(h.start * h.width) + 1) as u64;
                        next = Some((a, child_rows, pos));
                        break 'rows;
                    }
                    pos += size;
                }
            }
            (block, rows, base) = next.ok_or_else(corrupt)?;
        }
        Err(corrupt())
    }

    // ------------------------------------------------------------ attributes

    /// Attributes of an object: (name, value as text). Values of types we
    /// do not decode are `None`.
    pub fn attributes(&self, obj: &Object) -> Result<Vec<(String, Option<String>)>> {
        let mut out = vec![];
        for m in &obj.msgs {
            match m.typ {
                MSG_ATTRIBUTE => out.push(self.parse_attribute(&m.data)?),
                MSG_ATTRIBUTE_INFO => {
                    let mut c = Cur::new(&m.data);
                    c.u8()?;
                    let flags = c.u8()?;
                    if flags & 1 != 0 {
                        c.skip(2)?;
                    }
                    let heap = c.uv(self.so)?;
                    let names = c.uv(self.so)?;
                    if !self.undef(heap) && !self.undef(names) {
                        let h = self.heap(heap)?;
                        for rec in self.btree2(names)? {
                            // Attribute name index record: heap ID (8), flags, order, hash.
                            let a = self.heap_object(&h, rec.get(..8).ok_or_else(corrupt)?)?;
                            out.push(self.parse_attribute(&a)?);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    fn parse_attribute(&self, d: &[u8]) -> Result<(String, Option<String>)> {
        let mut c = Cur::new(d);
        let ver = c.u8()?;
        let flags = c.u8()?;
        let nlen = c.u16()? as usize;
        let tlen = c.u16()? as usize;
        let slen = c.u16()? as usize;
        let pad = |n: usize| if ver == 1 { n.div_ceil(8) * 8 } else { n };
        match ver {
            1 | 2 => {}
            3 => c.skip(1)?,
            _ => bail!("不支持的 HDF5 属性版本 {ver}"),
        }
        let name = c.take(pad(nlen))?;
        let name = text(&name[..nlen.min(name.len())]);
        let name = name.trim_end_matches('\0').to_string();
        let dt = c.take(pad(tlen))?;
        let ds = c.take(pad(slen))?;
        if flags & 3 != 0 {
            // Shared datatype or dataspace.
            return Ok((name, None));
        }
        let dtype = Datatype::parse(dt)?;
        let count = match parse_dataspace(ds, self.sl)? {
            Some(dims) => dims.iter().product::<u64>(),
            None => 0,
        };
        let data = &d[c.p.min(d.len())..];
        Ok((name, attribute_text(&dtype, data, count)))
    }

    // ------------------------------------------------------------ datasets

    pub fn dataset(&self, obj: &Object) -> Result<Dataset> {
        let ds = obj.msg(MSG_DATASPACE).ok_or_else(corrupt)?;
        let dims = parse_dataspace(&ds.data, self.sl)?.unwrap_or_default();
        let dt = obj.msg(MSG_DATATYPE).ok_or_else(corrupt)?;
        if dt.flags & 2 != 0 {
            bail!("不支持共享的 HDF5 数据类型");
        }
        let dtype = Datatype::parse(&dt.data)?;
        let layout = self.parse_layout(&obj.msg(MSG_LAYOUT).ok_or_else(corrupt)?.data, dims.len())?;
        let filters = match obj.msg(MSG_FILTERS) {
            Some(m) => parse_filters(&m.data)?,
            None => vec![],
        };
        Ok(Dataset { dims, dtype, layout, filters })
    }

    fn parse_layout(&self, d: &[u8], rank: usize) -> Result<Layout> {
        let (so, sl) = (self.so, self.sl);
        let mut c = Cur::new(d);
        let ver = c.u8()?;
        // Chunk dimensions carry the element size as an extra last one.
        let chunk_dims = |v: Vec<u64>| -> Result<Vec<u64>> {
            if v.len() == rank + 1 {
                Ok(v[..rank].to_vec())
            } else if v.len() == rank {
                Ok(v)
            } else {
                Err(corrupt())
            }
        };
        match ver {
            1 | 2 => {
                let nd = c.u8()? as usize;
                let class = c.u8()?;
                c.skip(5)?;
                let addr = if class != 0 { Some(c.uv(so)?) } else { None };
                let dims = (0..nd).map(|_| c.u32().map(|v| v as u64)).collect::<Result<Vec<_>>>()?;
                match class {
                    0 => Ok(Layout::Compact),
                    1 => {
                        let a = addr.unwrap();
                        Ok(Layout::Contiguous((!self.undef(a)).then_some(a)))
                    }
                    2 => Ok(Layout::Chunked(chunk_dims(dims)?, Index::BTree1(addr.unwrap()))),
                    _ => Err(corrupt()),
                }
            }
            3 | 4 => {
                let class = c.u8()?;
                match class {
                    0 => Ok(Layout::Compact),
                    1 => {
                        let a = c.uv(so)?;
                        Ok(Layout::Contiguous((!self.undef(a)).then_some(a)))
                    }
                    2 if ver == 3 => {
                        let nd = c.u8()? as usize;
                        let a = c.uv(so)?;
                        let dims = (0..nd).map(|_| c.u32().map(|v| v as u64)).collect::<Result<Vec<_>>>()?;
                        Ok(Layout::Chunked(chunk_dims(dims)?, Index::BTree1(a)))
                    }
                    2 => {
                        let flags = c.u8()?;
                        let nd = c.u8()? as usize;
                        let enc = c.u8()? as usize;
                        let dims = (0..nd).map(|_| c.uv(enc)).collect::<Result<Vec<_>>>()?;
                        let index = match c.u8()? {
                            1 => {
                                let (size, mask) = if flags & 2 != 0 { (c.uv(sl)?, c.u32()?) } else { (0, 0) };
                                Index::Single(c.uv(so)?, size, mask)
                            }
                            2 => Index::Implicit(c.uv(so)?),
                            3 => {
                                c.skip(1)?;
                                Index::FixedArray(c.uv(so)?)
                            }
                            t => Index::Other(t),
                        };
                        Ok(Layout::Chunked(chunk_dims(dims)?, index))
                    }
                    3 => bail!("不支持 HDF5 虚拟数据集"),
                    _ => Err(corrupt()),
                }
            }
            _ => bail!("不支持的 HDF5 数据布局版本 {ver}"),
        }
    }

    /// The stored chunks of a chunked dataset. Chunks never written are
    /// missing (the fill value, zero for Imaris).
    pub fn chunks(&self, ds: &Dataset) -> Result<Vec<ChunkRef>> {
        let Layout::Chunked(cd, index) = &ds.layout else { bail!("数据集不是分块存储") };
        let rank = ds.dims.len();
        let elem = ds.dtype.size as u64;
        let chunk_bytes = cd.iter().product::<u64>() * elem;
        // Row-major grid of chunks, for the indexes that store them in order.
        let grid: Vec<u64> = ds.dims.iter().zip(cd).map(|(&d, &c)| d.div_ceil(c.max(1))).collect();
        let start_of = |mut k: u64| -> Vec<u64> {
            let mut s = vec![0; rank];
            for i in (0..rank).rev() {
                s[i] = k % grid[i].max(1) * cd[i];
                k /= grid[i].max(1);
            }
            s
        };
        let mut out = vec![];
        match *index {
            Index::BTree1(a) => {
                if self.undef(a) {
                    return Ok(out);
                }
                let mut err = None;
                self.btree1(a, 1, rank + 1, &mut |key, child| {
                    let mut c = Cur::new(key);
                    let (Ok(size), Ok(mask)) = (c.u32(), c.u32()) else { return };
                    let start: Result<Vec<u64>> = (0..rank).map(|_| c.uv(8)).collect();
                    match start {
                        Ok(start) => out.push(ChunkRef { start, addr: self.abs(child), size: size as u64, mask }),
                        Err(e) => err = Some(e),
                    }
                })?;
                if let Some(e) = err {
                    return Err(e);
                }
            }
            Index::Single(a, size, mask) => {
                if !self.undef(a) {
                    let size = if size == 0 { chunk_bytes } else { size };
                    out.push(ChunkRef { start: vec![0; rank], addr: self.abs(a), size, mask });
                }
            }
            Index::Implicit(a) => {
                if !self.undef(a) {
                    let n: u64 = grid.iter().product();
                    for k in 0..n {
                        out.push(ChunkRef { start: start_of(k), addr: self.abs(a + k * chunk_bytes), size: chunk_bytes, mask: 0 });
                    }
                }
            }
            Index::FixedArray(a) => {
                if self.undef(a) {
                    return Ok(out);
                }
                let h = self.rd(a, 8 + self.sl + self.so + 4)?;
                if !h.starts_with(b"FAHD") {
                    return Err(corrupt());
                }
                let filtered = h[5] == 1;
                let esize = h[6] as usize;
                let page_bits = h[7] as u32;
                let mut c = Cur::at(&h, 8);
                let n = c.uv(self.sl)? as usize;
                let block = c.uv(self.so)?;
                let fixed = if filtered { self.so + 4 } else { self.so };
                if esize < fixed || n > 1 << 28 {
                    return Err(corrupt());
                }
                let mut entries = Vec::with_capacity(n * esize);
                let head = 6 + self.so;
                let page_n = 1usize << page_bits.min(31);
                if n > page_n {
                    let pages = n.div_ceil(page_n);
                    let mut pos = block + (head + pages.div_ceil(8) + 4) as u64;
                    for p in 0..pages {
                        let k = page_n.min(n - p * page_n);
                        entries.extend_from_slice(&self.rd(pos, k * esize)?);
                        pos += (k * esize + 4) as u64;
                    }
                } else {
                    entries = self.rd(block + head as u64, n * esize)?;
                }
                for (k, e) in entries.chunks_exact(esize).enumerate() {
                    let a = uv(&e[..self.so]);
                    if self.undef(a) || (k as u64) >= grid.iter().product::<u64>() {
                        continue;
                    }
                    let (size, mask) =
                        if filtered { (uv(&e[self.so..esize - 4]), uv(&e[esize - 4..]) as u32) } else { (chunk_bytes, 0) };
                    out.push(ChunkRef { start: start_of(k as u64), addr: self.abs(a), size, mask });
                }
            }
            Index::Other(t) => bail!("不支持的 HDF5 分块索引类型 {t}（可扩展数组或 v2 B 树）"),
        }
        Ok(out)
    }
}

fn parse_filters(d: &[u8]) -> Result<Vec<Filter>> {
    let mut c = Cur::new(d);
    let ver = c.u8()?;
    let n = c.u8()? as usize;
    if ver == 1 {
        c.skip(6)?;
    }
    let mut out = vec![];
    for _ in 0..n {
        let id = c.u16()?;
        let name_len = if ver == 1 || id >= 256 { c.u16()? as usize } else { 0 };
        let _flags = c.u16()?;
        let nv = c.u16()? as usize;
        c.skip(if ver == 1 { name_len.div_ceil(8) * 8 } else { name_len })?;
        let values = (0..nv).map(|_| c.u32()).collect::<Result<Vec<_>>>()?;
        if ver == 1 && nv % 2 == 1 {
            c.skip(4)?;
        }
        out.push(Filter { id, values });
    }
    Ok(out)
}

/// UTF-8, or Latin-1 when the bytes are not valid UTF-8.
pub fn text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

/// An attribute value as text: strings joined (Imaris stores them as
/// arrays of one-character strings), numbers separated by spaces.
fn attribute_text(t: &Datatype, data: &[u8], count: u64) -> Option<String> {
    let size = t.size as usize;
    let n = (count as usize).min(data.len() / size.max(1));
    let elems = data[..n * size].chunks_exact(size.max(1));
    match t.class {
        3 if size == 1 => {
            let b = &data[..n];
            let end = b.iter().position(|&c| c == 0).unwrap_or(n);
            Some(text(&b[..end]).trim_end().to_string())
        }
        3 => {
            let mut b = Vec::with_capacity(n * size);
            for e in elems {
                let end = e.iter().position(|&c| c == 0).unwrap_or(e.len());
                b.extend_from_slice(&e[..end]);
            }
            Some(text(&b).trim_end().to_string())
        }
        0 | 1 if size <= 8 => {
            let mut s = String::new();
            for e in elems.take(256) {
                let mut v = [0u8; 8];
                v[..size].copy_from_slice(e);
                if t.big_endian() {
                    v[..size].reverse();
                }
                let raw = u64::from_le_bytes(v);
                if !s.is_empty() {
                    s.push(' ');
                }
                let x = match (t.class, size) {
                    (1, 4) => format!("{}", f32::from_bits(raw as u32)),
                    (1, 8) => format!("{}", f64::from_bits(raw)),
                    (1, _) => return None,
                    _ if t.signed() => {
                        let sh = 64 - 8 * size as u32;
                        format!("{}", ((raw << sh) as i64) >> sh)
                    }
                    _ => format!("{raw}"),
                };
                s.push_str(&x);
            }
            Some(s)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(name: &str) -> H5<Vec<u8>> {
        let path = format!("{}/../../src-tauri/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        H5::open(std::fs::read(path).unwrap()).unwrap()
    }

    fn walk(h: &H5<Vec<u8>>, path: &str) -> Object {
        let mut addr = h.root;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let links = h.links(&h.object(addr).unwrap()).unwrap();
            addr = links.iter().find(|(n, _)| n == part).unwrap_or_else(|| panic!("{part} in {links:?}")).1;
        }
        h.object(addr).unwrap()
    }

    fn attr(h: &H5<Vec<u8>>, o: &Object, name: &str) -> Option<String> {
        h.attributes(o).unwrap().into_iter().find(|(k, _)| k == name).and_then(|(_, v)| v)
    }

    #[test]
    fn old_style_groups_and_btree1_chunks() {
        let h = open("imaris_v0.ims");
        let root = h.object(h.root).unwrap();
        assert!(root.msg(MSG_SYMBOL_TABLE).is_some());
        assert_eq!(attr(&h, &root, "ImarisVersion").as_deref(), Some("5.5.0"));
        assert_eq!(attr(&h, &root, "NumberOfDataSets").as_deref(), Some("1"));
        let info = walk(&h, "DataSetInfo/Image");
        assert_eq!(attr(&h, &info, "X").as_deref(), Some("19"));
        let ds = h.dataset(&walk(&h, "DataSet/ResolutionLevel 0/TimePoint 0/Channel 0/Data")).unwrap();
        assert_eq!(ds.dims, [8, 16, 32]);
        assert_eq!((ds.dtype.class, ds.dtype.size), (0, 2));
        assert!(matches!(&ds.layout, Layout::Chunked(c, Index::BTree1(_)) if c == &[4, 8, 16]));
        assert_eq!(ds.filters.iter().map(|f| f.id).collect::<Vec<_>>(), [FILTER_DEFLATE]);
        let chunks = h.chunks(&ds).unwrap();
        assert_eq!(chunks.len(), 2 * 2 * 2);
        assert!(chunks.iter().any(|c| c.start == [4, 8, 16]));
        let raw = h.dataset(&walk(&h, "DataSet/ResolutionLevel 0/TimePoint 1/Channel 1/Data")).unwrap();
        assert!(raw.filters.is_empty());
        assert!(h.chunks(&raw).unwrap().iter().all(|c| c.size == 4 * 8 * 16 * 2));
    }

    #[test]
    fn new_style_groups_dense_attributes_and_fixed_array() {
        let h = open("imaris_v110.ims");
        let info = walk(&h, "DataSetInfo");
        assert!(info.msg(MSG_LINK_INFO).is_some() && info.msg(MSG_LINK).is_none(), "dense links");
        assert_eq!(h.links(&info).unwrap().len(), 15);
        let image = walk(&h, "DataSetInfo/Image");
        assert!(image.msg(MSG_ATTRIBUTE_INFO).is_some() && image.msg(MSG_ATTRIBUTE).is_none(), "dense attributes");
        assert_eq!(h.attributes(&image).unwrap().len(), 16);
        assert_eq!(attr(&h, &image, "Unit").as_deref(), Some("um"));
        // Larger than the heap's biggest managed object: a huge object.
        let custom = walk(&h, "DataSetInfo/CustomData");
        assert_eq!(attr(&h, &custom, "Settings").map(|s| s.len()), Some(6002));
        let ds = h.dataset(&walk(&h, "DataSet/ResolutionLevel 0/TimePoint 0/Channel 0/Data")).unwrap();
        assert!(matches!(&ds.layout, Layout::Chunked(c, Index::FixedArray(_)) if c == &[2, 8, 8]));
        let chunks = h.chunks(&ds).unwrap();
        assert_eq!(chunks.len(), 2 * 2 * 2);
        assert_eq!(chunks.iter().filter(|c| c.start == [2, 8, 0]).count(), 1);
        let single = h.dataset(&walk(&h, "DataSet/ResolutionLevel 0/TimePoint 0/Channel 1/Data")).unwrap();
        assert!(matches!(&single.layout, Layout::Chunked(_, Index::Single(..))));
        assert_eq!(h.chunks(&single).unwrap().len(), 1);
    }
}
