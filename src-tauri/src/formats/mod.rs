pub mod mrc;
pub mod tiff;

use std::path::Path;

use anyhow::{bail, Result};

use crate::image::{DType, PlaneData, Source};

pub const EXTENSIONS: &[&str] = &["tif", "tiff", "mrc", "mrcs", "rec", "st", "map"];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn open(path: &Path) -> Result<Box<dyn Source>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "tif" | "tiff" => Ok(Box::new(tiff::TiffSource::open(path)?)),
        "mrc" | "mrcs" | "rec" | "st" | "map" => Ok(Box::new(mrc::MrcSource::open(path)?)),
        _ => bail!("不支持的文件类型: {}", path.display()),
    }
}

/// Converts raw uncompressed samples in the given byte order into a plane.
pub fn raw_to_plane(bytes: &[u8], dtype: DType, little: bool) -> PlaneData {
    use rayon::prelude::*;
    macro_rules! conv {
        ($t:ty, $n:expr, $variant:ident) => {{
            let v: Vec<$t> = bytes
                .par_chunks_exact($n)
                .map(|b| {
                    let a: [u8; $n] = b.try_into().unwrap();
                    if little { <$t>::from_le_bytes(a) } else { <$t>::from_be_bytes(a) }
                })
                .collect();
            PlaneData::$variant(v)
        }};
    }
    match dtype {
        DType::U8 => PlaneData::U8(bytes.to_vec()),
        DType::I8 => PlaneData::I8(bytes.iter().map(|&b| b as i8).collect()),
        DType::U16 => conv!(u16, 2, U16),
        DType::I16 => conv!(i16, 2, I16),
        DType::U32 => conv!(u32, 4, U32),
        DType::I32 => conv!(i32, 4, I32),
        DType::F32 => conv!(f32, 4, F32),
        DType::F64 => conv!(f64, 8, F64),
    }
}

/// Minimal attribute lookup for the flat XML snippets we care about.
pub fn xml_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    while let Some(i) = rest.find(name) {
        let before_ok = i == 0 || rest.as_bytes()[i - 1].is_ascii_whitespace();
        let after = &rest[i + name.len()..];
        if before_ok {
            if let Some(after) = after.trim_start().strip_prefix('=') {
                let after = after.trim_start();
                let q = after.chars().next()?;
                if q == '"' || q == '\'' {
                    let body = &after[1..];
                    return body.find(q).map(|e| &body[..e]);
                }
            }
        }
        rest = after;
    }
    None
}

/// Iterates over the opening tags `<name ...>` in an XML string.
pub fn xml_tags<'a>(xml: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    let open = format!("<{name}");
    let mut pos = 0;
    std::iter::from_fn(move || loop {
        let i = xml[pos..].find(&open)? + pos;
        let after = i + open.len();
        let next = xml[after..].chars().next()?;
        let end = xml[after..].find('>')? + after;
        pos = end;
        if next.is_whitespace() || next == '>' || next == '/' {
            return Some(&xml[i..end]);
        }
    })
}
