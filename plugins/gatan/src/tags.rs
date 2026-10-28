//! The DigitalMicrograph tag tree of DM3 and DM4 files.
//!
//! Layout (written from the public descriptions of the format by Chris
//! Boothroyd and Greg Jefferis, and the ncempy documentation; no code
//! taken):
//!
//! ```text
//! header   u32 version (3 or 4), root length (u32 / u64), u32 byte order
//!          of the data (1 = little-endian, 0 = big-endian)
//! dir      u8 sorted, u8 closed, tag count (u32 / u64)
//! tag      u8 kind (20 = dir, 21 = data, 0 = end), u16 label length,
//!          label, [DM4: u64 size of the rest of the tag], dir or data
//! data     "%%%%", info count (u32 / u64), info words (u32 / u64), bytes
//! ```
//!
//! Everything but the data bytes is big-endian. The info words give the
//! type: a simple type code, 15 (struct: name length, field count, then a
//! name length and type per field), 18 (string: length) or 20 (array:
//! element type, then the struct description when the elements are
//! structs, then the element count). Arrays hold the pixels, so large data
//! is never read here; it is kept as an offset.

use tifmrcview_plugin_sdk::{bail, file, Result};

/// Data larger than this is not read while parsing (pixels, big tables).
const EAGER: u64 = 4096;
const MAX_DEPTH: u32 = 64;
const BLOCK: usize = 256 << 10;

pub const T_I16: u64 = 2;
pub const T_I32: u64 = 3;
pub const T_U16: u64 = 4;
pub const T_U32: u64 = 5;
pub const T_F32: u64 = 6;
pub const T_F64: u64 = 7;
pub const T_BOOL: u64 = 8;
pub const T_I8: u64 = 9;
pub const T_U8: u64 = 10;
pub const T_I64: u64 = 11;
pub const T_U64: u64 = 12;
const T_STRUCT: u64 = 15;
const T_STRING: u64 = 18;
const T_ARRAY: u64 = 20;

/// Size of a simple type, `None` for anything else.
pub fn simple_size(t: u64) -> Option<u64> {
    Some(match t {
        T_I16 | T_U16 => 2,
        T_I32 | T_U32 | T_F32 => 4,
        T_F64 | T_I64 | T_U64 => 8,
        T_BOOL | T_I8 | T_U8 => 1,
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub enum Kind {
    Simple(u64),
    Str,
    Struct(Vec<u64>),
    Array(u64, u64),
    StructArray(Vec<u64>, u64),
}

#[derive(Debug)]
pub struct Data {
    pub kind: Kind,
    /// Absolute offset and length of the bytes in the file.
    pub offset: u64,
    pub len: u64,
    /// The bytes, when small enough to read while parsing.
    pub bytes: Option<Vec<u8>>,
}

#[derive(Debug)]
pub enum Node {
    Dir(Vec<(String, Node)>),
    Data(Data),
}

/// Buffered big-endian reader over the host file.
struct Rd {
    buf: Vec<u8>,
    base: u64,
    pos: u64,
    size: u64,
    v4: bool,
}

impl Rd {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.pos.checked_add(n as u64).filter(|&e| e <= self.size);
        let Some(end) = end else { bail!("文件不完整（标签读到了文件末尾之后）") };
        if self.pos < self.base || end > self.base + self.buf.len() as u64 {
            self.base = self.pos;
            self.buf = file::read_at_most(self.pos, n.max(BLOCK))?;
            if self.buf.len() < n {
                bail!("文件不完整");
            }
        }
        let s = (self.pos - self.base) as usize;
        self.pos = end;
        Ok(&self.buf[s..s + n])
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes(b.try_into().unwrap()))
    }
    /// A count or size field: u32 in DM3, u64 in DM4.
    fn len(&mut self) -> Result<u64> {
        if self.v4 {
            self.u64()
        } else {
            Ok(self.u32()? as u64)
        }
    }
    fn remaining(&self) -> u64 {
        self.size.saturating_sub(self.pos)
    }

    fn dir(&mut self, depth: u32) -> Result<Vec<(String, Node)>> {
        if depth > MAX_DEPTH {
            bail!("标签嵌套太深，文件可能已损坏");
        }
        self.take(2)?; // sorted, closed
        let n = self.len()?;
        // Every tag takes at least 3 bytes.
        if n > self.remaining() / 3 {
            bail!("标签数量不对（{n}），文件可能已损坏");
        }
        let mut out = Vec::with_capacity(n.min(4096) as usize);
        for _ in 0..n {
            let kind = self.u8()?;
            if kind == 0 {
                break;
            }
            let ll = self.u16()? as usize;
            let label = latin1(self.take(ll)?);
            if self.v4 {
                self.u64()?;
            }
            let node = match kind {
                20 => Node::Dir(self.dir(depth + 1)?),
                21 => Node::Data(self.data()?),
                k => bail!("未知的标签类型 {k}，文件可能已损坏"),
            };
            out.push((label, node));
        }
        Ok(out)
    }

    fn data(&mut self) -> Result<Data> {
        if self.take(4)? != b"%%%%" {
            bail!("标签数据缺少 %%%% 标记，文件可能已损坏");
        }
        let n = self.len()?;
        if n == 0 || n > 4096 {
            bail!("标签类型描述长度不对（{n}）");
        }
        let mut info = Vec::with_capacity(n as usize);
        for _ in 0..n {
            info.push(self.len()?);
        }
        let at = |i: usize| -> Result<u64> {
            match info.get(i) {
                Some(&v) => Ok(v),
                None => bail!("标签类型描述不完整"),
            }
        };
        let fields = |count_at: usize| -> Result<Vec<u64>> {
            let nf = at(count_at)? as usize;
            if nf > 1024 {
                bail!("结构体字段太多");
            }
            let mut f = Vec::with_capacity(nf);
            for i in 0..nf {
                let t = at(count_at + 2 + 2 * i)?;
                if simple_size(t).is_none() {
                    bail!("不支持的结构体字段类型 {t}");
                }
                f.push(t);
            }
            Ok(f)
        };
        let t = info[0];
        let (kind, len) = if let Some(s) = simple_size(t) {
            (Kind::Simple(t), s)
        } else if t == T_STRING {
            (Kind::Str, at(1)?)
        } else if t == T_STRUCT {
            let f = fields(2)?;
            let s = f.iter().map(|&t| simple_size(t).unwrap()).sum();
            (Kind::Struct(f), s)
        } else if t == T_ARRAY {
            let et = at(1)?;
            if et == T_STRUCT {
                let f = fields(3)?;
                let count = at(4 + 2 * f.len())?;
                let s: u64 = f.iter().map(|&t| simple_size(t).unwrap()).sum();
                (Kind::StructArray(f, count), s.saturating_mul(count))
            } else if let Some(s) = simple_size(et) {
                let count = at(2)?;
                (Kind::Array(et, count), s.saturating_mul(count))
            } else {
                bail!("不支持的数组元素类型 {et}");
            }
        } else {
            bail!("未知的标签数据类型 {t}");
        };
        if len > self.remaining() {
            bail!("文件不完整（标签数据超出文件末尾）");
        }
        let offset = self.pos;
        let bytes = if len <= EAGER {
            Some(self.take(len as usize)?.to_vec())
        } else {
            self.pos += len;
            None
        };
        Ok(Data { kind, offset, len, bytes })
    }
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

pub struct Tree {
    pub version: u32,
    pub little_endian: bool,
    pub root: Vec<(String, Node)>,
}

pub fn parse() -> Result<Tree> {
    let size = file::size();
    let head = file::read_at_most(0, 16)?;
    if head.len() < 16 {
        bail!("不是 DigitalMicrograph 文件（太短）");
    }
    let version = u32::from_be_bytes(head[0..4].try_into().unwrap());
    let (v4, order_at) = match version {
        3 => (false, 8),
        4 => (true, 12),
        _ => bail!("不是 DigitalMicrograph 3/4 文件（版本号 {version}）"),
    };
    let order = u32::from_be_bytes(head[order_at..order_at + 4].try_into().unwrap());
    if order > 1 {
        bail!("不是 DigitalMicrograph 文件（字节序标记 {order}）");
    }
    let mut rd = Rd { buf: Vec::new(), base: 0, pos: order_at as u64 + 4, size, v4 };
    let root = rd.dir(0)?;
    Ok(Tree { version, little_endian: order == 1, root })
}

// ------------------------------------------------------------- reading values

pub fn get<'a>(dir: &'a [(String, Node)], name: &str) -> Option<&'a Node> {
    dir.iter().find(|(k, _)| k == name).map(|(_, n)| n)
}

pub fn path<'a>(dir: &'a [(String, Node)], names: &[&str]) -> Option<&'a Node> {
    let (last, parents) = names.split_last()?;
    let mut d = dir;
    for n in parents {
        d = get(d, n)?.dir()?;
    }
    get(d, last)
}

impl Node {
    pub fn dir(&self) -> Option<&[(String, Node)]> {
        match self {
            Node::Dir(d) => Some(d),
            Node::Data(_) => None,
        }
    }
    pub fn data(&self) -> Option<&Data> {
        match self {
            Node::Data(d) => Some(d),
            Node::Dir(_) => None,
        }
    }
    pub fn num(&self, le: bool) -> Option<f64> {
        let d = self.data()?;
        match d.kind {
            Kind::Simple(t) => Some(scalar(t, d.bytes.as_deref()?, le)),
            _ => None,
        }
        .filter(|v| v.is_finite())
    }
    pub fn text(&self, le: bool) -> Option<String> {
        let d = self.data()?;
        match d.kind {
            Kind::Array(T_U16, _) | Kind::Str => Some(d.display(le)).filter(|s| !s.is_empty()),
            _ => None,
        }
    }
}

fn scalar(t: u64, b: &[u8], le: bool) -> f64 {
    let n = simple_size(t).unwrap() as usize;
    let mut a = [0u8; 8];
    a[..n].copy_from_slice(&b[..n]);
    if !le {
        a[..n].reverse();
    }
    match t {
        T_I16 => i16::from_le_bytes([a[0], a[1]]) as f64,
        T_U16 => u16::from_le_bytes([a[0], a[1]]) as f64,
        T_I32 => i32::from_le_bytes(a[..4].try_into().unwrap()) as f64,
        T_U32 => u32::from_le_bytes(a[..4].try_into().unwrap()) as f64,
        T_F32 => f32::from_le_bytes(a[..4].try_into().unwrap()) as f64,
        T_F64 => f64::from_le_bytes(a),
        T_I64 => i64::from_le_bytes(a) as f64,
        T_U64 => u64::from_le_bytes(a) as f64,
        T_I8 => a[0] as i8 as f64,
        _ => a[0] as f64,
    }
}

/// A value as text; f32 keeps its own shortest form.
fn scalar_text(t: u64, b: &[u8], le: bool) -> String {
    match t {
        T_F32 => {
            let mut a: [u8; 4] = b[..4].try_into().unwrap();
            if !le {
                a.reverse();
            }
            format!("{}", f32::from_le_bytes(a))
        }
        T_BOOL => (if b[0] != 0 { "true" } else { "false" }).into(),
        _ => format!("{}", scalar(t, b, le)),
    }
}

/// Small arrays and structs are shown in full; larger ones by their size.
const MAX_SHOWN: u64 = 16;

impl Data {
    pub fn display(&self, le: bool) -> String {
        let Some(b) = self.bytes.as_deref() else {
            return match self.kind {
                Kind::Array(_, n) | Kind::StructArray(_, n) => format!("（{n} 个值）"),
                _ => format!("（{} 字节）", self.len),
            };
        };
        match &self.kind {
            Kind::Simple(t) => scalar_text(*t, b, le),
            Kind::Str => String::from_utf8_lossy(b).trim_end_matches('\0').into(),
            Kind::Array(T_U16, _) => {
                let u: Vec<u16> = b
                    .chunks_exact(2)
                    .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
                    .collect();
                String::from_utf16_lossy(&u).trim_end_matches('\0').into()
            }
            Kind::Array(t, n) if *n <= MAX_SHOWN => {
                let s = simple_size(*t).unwrap() as usize;
                b.chunks_exact(s).map(|c| scalar_text(*t, c, le)).collect::<Vec<_>>().join(", ")
            }
            Kind::Struct(f) => format!("({})", fields_text(f, b, le)),
            Kind::StructArray(f, n) if *n <= MAX_SHOWN => {
                let s: usize = f.iter().map(|&t| simple_size(t).unwrap() as usize).sum();
                b.chunks_exact(s.max(1)).map(|c| format!("({})", fields_text(f, c, le))).collect::<Vec<_>>().join(" ")
            }
            Kind::Array(_, n) | Kind::StructArray(_, n) => format!("（{n} 个值）"),
        }
    }
}

fn fields_text(f: &[u64], b: &[u8], le: bool) -> String {
    let mut out = Vec::with_capacity(f.len());
    let mut o = 0;
    for &t in f {
        let s = simple_size(t).unwrap() as usize;
        out.push(scalar_text(t, &b[o..o + s], le));
        o += s;
    }
    out.join(", ")
}
