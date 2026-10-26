//! NIS-Elements metadata values ("variants") as one small tree.
//!
//! ND2 metadata chunks come in two encodings of the same typed key/value
//! tree. Files from NIS-Elements 4 and later (ND2 3.0) use the binary "lite
//! variant": each record is a type byte, a name length, the name in UTF-16
//! and the value; a "level" record holds a count of children, a byte length
//! and, after the children, a table of their offsets (which we skip). Older
//! files (ND2 2.x) write the tree as XML, `<name runtype="lx_uint32"
//! value="…"/>` for scalars and plain elements for levels. Both become a
//! [`V`]. Lists have empty names (binary) or `no_name` (XML); lookups by name
//! simply never match them, and [`V::items`] walks them in order.

use tifmrcview_plugin_sdk::xml::{self, Node};
use tifmrcview_plugin_sdk::{decompress, le_u32, le_u64, Codec};

#[derive(Clone, Debug)]
pub enum V {
    Bool(bool),
    Int(i64),
    UInt(u64),
    F(f64),
    Str(String),
    Bytes(Vec<u8>),
    Map(Vec<(String, V)>),
}

static EMPTY: [(String, V); 0] = [];

impl V {
    /// The first child called `key`.
    pub fn get(&self, key: &str) -> Option<&V> {
        self.items().iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// `get` along a `/`-separated path.
    pub fn at(&self, path: &str) -> Option<&V> {
        path.split('/').try_fold(self, |v, k| v.get(k))
    }

    /// Children of a level, in order (empty for scalars).
    pub fn items(&self) -> &[(String, V)] {
        match self {
            V::Map(m) => m,
            _ => &EMPTY,
        }
    }

    pub fn f64(&self) -> Option<f64> {
        match *self {
            V::Bool(b) => Some(b as u8 as f64),
            V::Int(i) => Some(i as f64),
            V::UInt(u) => Some(u as f64),
            V::F(f) => Some(f),
            V::Str(ref s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn u64(&self) -> Option<u64> {
        match *self {
            V::Bool(b) => Some(b as u64),
            V::Int(i) => u64::try_from(i).ok(),
            V::UInt(u) => Some(u),
            V::F(f) if f >= 0.0 && f.is_finite() => Some(f as u64),
            V::Str(ref s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            V::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            V::Str(s) => matches!(s.trim(), "true" | "True" | "1"),
            v => v.f64().is_some_and(|f| f != 0.0),
        }
    }

    /// Strips the wrappers around a chunk's content: a single level such as
    /// `SLxImageAttributes` (binary) or `variant/no_name` (XML).
    pub fn unwrap_single(&self) -> &V {
        let mut v = self;
        while let V::Map(m) = v {
            match m.as_slice() {
                [(_, inner @ V::Map(_))] => v = inner,
                _ => break,
            }
        }
        v
    }

    /// The scalar as display text (`None` for levels and byte arrays).
    pub fn text(&self) -> Option<String> {
        Some(match self {
            V::Bool(b) => (if *b { "true" } else { "false" }).into(),
            V::Int(i) => format!("{i}"),
            V::UInt(u) => format!("{u}"),
            V::F(f) => num(*f),
            V::Str(s) => s.trim().to_string(),
            V::Bytes(_) | V::Map(_) => return None,
        })
    }

    /// Flags of a list (`pItemValid`, `pPeriodValid`): a byte array in binary
    /// metadata, a level of booleans or a base64 byte array in XML.
    pub fn flags(&self) -> Vec<bool> {
        match self {
            V::Bytes(b) => b.iter().map(|&x| x != 0).collect(),
            V::Map(m) => m.iter().map(|(_, v)| v.truthy()).collect(),
            V::Str(s) => base64(s).iter().map(|&x| x != 0).collect(),
            _ => vec![],
        }
    }
}

/// A number without float noise: 0.12863494437945 -> "0.1286", 5000 -> "5000".
pub fn num(v: f64) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let r = format!("{v:.4}");
    let r = r.trim_end_matches('0').trim_end_matches('.');
    if r == "0" || r == "-0" {
        format!("{v:.3e}")
    } else {
        r.to_string()
    }
}

fn base64(s: &str) -> Vec<u8> {
    let mut out = vec![];
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Decodes a metadata chunk, XML or binary.
pub fn decode(data: &[u8]) -> V {
    let start = data.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(0);
    if data.get(start) == Some(&b'<') {
        from_xml(data)
    } else {
        let mut out = vec![];
        lite(data, &mut 0, usize::MAX, &mut out, 0);
        V::Map(out)
    }
}

// ---------------------------------------------------------------- binary "lite variant"

const BOOL: u8 = 1;
const INT32: u8 = 2;
const UINT32: u8 = 3;
const INT64: u8 = 4;
const UINT64: u8 = 5;
const DOUBLE: u8 = 6;
const VOIDPOINTER: u8 = 7;
const STRING: u8 = 8;
const BYTEARRAY: u8 = 9;
const LEVEL: u8 = 11;
const COMPRESS: u8 = 76;

/// UTF-16LE up to the first NUL (or the end); returns the text and the bytes used.
fn utf16(b: &[u8], max_units: usize) -> (String, usize) {
    let mut units = Vec::with_capacity(max_units.min(b.len() / 2).min(256));
    let mut used = 0;
    while used + 2 <= b.len() && units.len() < max_units {
        let u = u16::from_le_bytes([b[used], b[used + 1]]);
        used += 2;
        if u == 0 {
            break;
        }
        units.push(u);
    }
    // Most names and values are ASCII: skip the general decoder for them.
    let s = if units.iter().all(|&u| u < 0x80) {
        units.iter().map(|&u| u as u8 as char).collect()
    } else {
        char::decode_utf16(units.iter().copied()).map(|c| c.unwrap_or('\u{fffd}')).collect()
    };
    (s, used)
}

/// Reads up to `count` records from `b[*pos..]` into `out`. Stops quietly at
/// anything malformed: metadata is best effort, pixels do not depend on it.
fn lite(b: &[u8], pos: &mut usize, count: usize, out: &mut Vec<(String, V)>, depth: u32) {
    if depth > 64 {
        return;
    }
    let mut n = 0;
    while n < count && *pos + 2 <= b.len() {
        let rec = *pos;
        let (ty, name_len) = (b[rec], b[rec + 1] as usize);
        if ty == COMPRESS {
            // Type, name length, 10 more bytes, then a zlib stream with the records.
            let Some(z) = b.get(rec + 12..) else { return };
            let mut cap = (z.len() * 8).max(1 << 16);
            loop {
                match decompress(Codec::Zlib, z, cap) {
                    Ok(d) if d.len() < cap || cap >= 1 << 28 => {
                        lite(&d, &mut 0, usize::MAX, out, depth + 1);
                        break;
                    }
                    Ok(_) => cap *= 4,
                    Err(_) => break,
                }
            }
            *pos = b.len();
            return;
        }
        let (name, _) = utf16(&b[rec + 2..], name_len);
        let mut p = rec + 2 + name_len * 2;
        let fixed = |p: usize, k: usize| b.get(p..p + k);
        let v = match ty {
            BOOL => match fixed(p, 1) {
                Some(x) => {
                    p += 1;
                    V::Bool(x[0] != 0)
                }
                None => return,
            },
            INT32 | UINT32 => match fixed(p, 4) {
                Some(x) => {
                    p += 4;
                    if ty == INT32 { V::Int(le_u32(x, 0) as i32 as i64) } else { V::UInt(le_u32(x, 0) as u64) }
                }
                None => return,
            },
            INT64 | UINT64 | VOIDPOINTER | DOUBLE => match fixed(p, 8) {
                Some(x) => {
                    p += 8;
                    let u = le_u64(x, 0);
                    match ty {
                        INT64 => V::Int(u as i64),
                        DOUBLE => V::F(f64::from_bits(u)),
                        _ => V::UInt(u),
                    }
                }
                None => return,
            },
            STRING => {
                let (s, used) = utf16(&b[p.min(b.len())..], usize::MAX);
                p += used;
                V::Str(s)
            }
            BYTEARRAY => {
                let Some(x) = fixed(p, 8) else { return };
                let len = le_u64(x, 0);
                p += 8;
                let Some(bytes) = usize::try_from(len).ok().and_then(|l| b.get(p..p.checked_add(l)?)) else { return };
                p += bytes.len();
                V::Bytes(bytes.to_vec())
            }
            LEVEL => {
                let Some(x) = fixed(p, 12) else { return };
                let items = le_u32(x, 0) as usize;
                let len = le_u64(x, 4);
                p += 12;
                let end = match usize::try_from(len).ok().and_then(|l| rec.checked_add(l)) {
                    Some(e) if e >= p && e <= b.len() => e,
                    _ => return,
                };
                let mut kids = Vec::with_capacity(items.min(4096));
                let mut q = p;
                lite(&b[..end], &mut q, items, &mut kids, depth + 1);
                // The children are followed by a table of their offsets.
                p = end.saturating_add(items.saturating_mul(8)).min(b.len());
                V::Map(kids)
            }
            _ => return,
        };
        out.push((name, v));
        *pos = p;
        n += 1;
    }
}

// ---------------------------------------------------------------- XML variant

fn from_xml(data: &[u8]) -> V {
    let doc = xml::Doc::parse(data);
    match doc.root() {
        Some(r) => V::Map(vec![(r.name().into_owned(), xml_node(r, 0))]),
        None => V::Map(vec![]),
    }
}

fn xml_node(n: Node, depth: u32) -> V {
    if let Some(rt) = n.attribute("runtype") {
        let val = n.attribute("value");
        let s = val.as_deref().unwrap_or("");
        match &*rt {
            "bool" => return V::Bool(matches!(s.trim(), "true" | "True" | "1")),
            "double" | "float" => return V::F(s.trim().parse().unwrap_or(f64::NAN)),
            "lx_int32" | "lx_int64" => return V::Int(s.trim().parse().unwrap_or(0)),
            "lx_uint32" | "lx_uint64" => return V::UInt(s.trim().parse().unwrap_or(0)),
            "CLxStringW" | "CLxByteArray" | "unknown" => return V::Str(s.to_string()),
            _ => {}
        }
    }
    if depth > 64 {
        return V::Map(vec![]);
    }
    V::Map(n.children().map(|c| (c.name().into_owned(), xml_node(c, depth + 1))).collect())
}

/// Calls `f(path, value)` for every scalar below `v`, with paths like
/// `sPicturePlanes/sPlaneNew/a0/sDescription`. List items (no name) are
/// numbered from 1.
pub fn leaves(v: &V, f: &mut impl FnMut(&str, &str)) {
    let mut path = String::new();
    walk(v, &mut path, f, 0);
}

fn walk(v: &V, path: &mut String, f: &mut impl FnMut(&str, &str), depth: u32) {
    if depth > 64 {
        return;
    }
    let mut k = 0;
    for (name, child) in v.items() {
        let len = path.len();
        if !path.is_empty() {
            path.push('/');
        }
        if name.is_empty() || name == "no_name" {
            k += 1;
            path.push_str(&format!("{k}"));
        } else {
            path.push_str(name);
        }
        match child {
            V::Map(_) => walk(child, path, f, depth + 1),
            V::Bytes(_) => {}
            _ => {
                if let Some(t) = child.text().filter(|t| !t.is_empty()) {
                    f(path, &t);
                }
            }
        }
        path.truncate(len);
    }
}
