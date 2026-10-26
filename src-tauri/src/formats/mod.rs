pub mod mrc;
pub mod raster;
pub mod tiff;

use std::path::Path;

use anyhow::{bail, Result};

use crate::image::{DType, PlaneData, Source};
use crate::plugins::registry::Registry;

/// Formats built into the app. Others (Zeiss CZI…) come from WebAssembly
/// plugins, see `plugins::registry`.
pub const CORE_EXTENSIONS: &[&str] =
    &["tif", "tiff", "mrc", "mrcs", "rec", "st", "map", "png", "jpg", "jpeg", "gif", "bmp", "webp"];

fn ext_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

pub fn is_supported(path: &Path) -> bool {
    is_supported_with(path, Registry::global())
}

pub fn is_supported_with(path: &Path, reg: &Registry) -> bool {
    let ext = ext_of(path);
    !ext.is_empty() && (CORE_EXTENSIONS.contains(&ext.as_str()) || reg.opens(&ext))
}

/// Every extension that opens now: the core's plus enabled plugins'.
pub fn extensions() -> Vec<String> {
    let mut v: Vec<String> = CORE_EXTENSIONS.iter().map(|e| e.to_string()).collect();
    v.extend(Registry::global().extensions());
    v
}

/// Opens a file, including what metadata plugins add.
pub fn open(path: &Path) -> Result<Box<dyn Source>> {
    open_with(path, Registry::global())
}

/// Opens a file for its pixels only (thumbnails): metadata plugins are skipped.
pub fn open_pixels(path: &Path) -> Result<Box<dyn Source>> {
    open_pixels_with(path, Registry::global()).map(|(s, _)| s)
}

pub fn open_with(path: &Path, reg: &Registry) -> Result<Box<dyn Source>> {
    let (src, by) = open_pixels_with(path, reg)?;
    Ok(reg.add_metadata(path, &ext_of(path), src, by.as_deref()))
}

/// The source, and the id of the plugin that opened it (if one did).
fn open_pixels_with(path: &Path, reg: &Registry) -> Result<(Box<dyn Source>, Option<String>)> {
    let ext = ext_of(path);
    let core: Box<dyn Source> = match ext.as_str() {
        "tif" | "tiff" => Box::new(tiff::TiffSource::open(path)?),
        "mrc" | "mrcs" | "rec" | "st" | "map" => Box::new(mrc::MrcSource::open(path)?),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" => Box::new(raster::RasterSource::open(path)?),
        _ => {
            return match reg.open(path, &ext) {
                Some(r) => r.map(|(s, id)| (s, Some(id))),
                None => match reg.missing(&ext) {
                    Some(msg) => bail!("{msg}"),
                    None => bail!("不支持的文件类型: {}", path.display()),
                },
            }
        }
    };
    Ok((core, None))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions() {
        for f in ["a.tif", "a.TIFF", "b.mrc", "b.MRCS", "c.rec", "c.st", "c.map", "d.png", "d.JPG", "d.webp"] {
            assert!(is_supported(Path::new(f)), "{f}");
        }
        for f in ["a.txt", "noext", "a.tif.bak", ".tif", "a.mod", ""] {
            assert!(!is_supported(Path::new(f)), "{f}");
        }
        assert!(is_supported(Path::new("/some/dir.v2/x.tif")));
    }

    #[test]
    fn open_rejects_unknown_extension() {
        assert!(open(Path::new("x.xyz")).is_err());
        assert!(open(Path::new("/no/such/file.tif")).is_err());
    }

    fn f64s(p: &PlaneData) -> Vec<f64> {
        (0..p.len()).map(|i| p.get_f64(i)).collect()
    }

    #[test]
    fn raw_to_plane_u8_i8() {
        assert_eq!(f64s(&raw_to_plane(&[0, 255], DType::U8, true)), [0.0, 255.0]);
        assert_eq!(f64s(&raw_to_plane(&[0x80, 0x7f, 0xff], DType::I8, false)), [-128.0, 127.0, -1.0]);
    }

    #[test]
    fn raw_to_plane_both_endiannesses() {
        let le = |v: &[u8]| raw_to_plane(v, DType::U16, true);
        let be = |v: &[u8]| raw_to_plane(v, DType::U16, false);
        assert_eq!(f64s(&le(&[0x34, 0x12])), [0x1234 as f64]);
        assert_eq!(f64s(&be(&[0x12, 0x34])), [0x1234 as f64]);
        assert_eq!(f64s(&raw_to_plane(&(-2i16).to_le_bytes(), DType::I16, true)), [-2.0]);
        assert_eq!(f64s(&raw_to_plane(&(-2i16).to_be_bytes(), DType::I16, false)), [-2.0]);
        assert_eq!(f64s(&raw_to_plane(&70000u32.to_le_bytes(), DType::U32, true)), [70000.0]);
        assert_eq!(f64s(&raw_to_plane(&70000u32.to_be_bytes(), DType::U32, false)), [70000.0]);
        assert_eq!(f64s(&raw_to_plane(&(-70000i32).to_le_bytes(), DType::I32, true)), [-70000.0]);
        assert_eq!(f64s(&raw_to_plane(&(-70000i32).to_be_bytes(), DType::I32, false)), [-70000.0]);
        assert_eq!(f64s(&raw_to_plane(&1.5f32.to_le_bytes(), DType::F32, true)), [1.5]);
        assert_eq!(f64s(&raw_to_plane(&1.5f32.to_be_bytes(), DType::F32, false)), [1.5]);
        let p = raw_to_plane(&(-0.25f64).to_le_bytes(), DType::F64, true);
        assert!(matches!(p, PlaneData::F64(ref v) if v == &[-0.25]));
        let p = raw_to_plane(&(-0.25f64).to_be_bytes(), DType::F64, false);
        assert!(matches!(p, PlaneData::F64(ref v) if v == &[-0.25]));
    }

    #[test]
    fn raw_to_plane_ignores_trailing_partial_sample() {
        assert_eq!(raw_to_plane(&[1, 0, 2, 0, 3], DType::U16, true).len(), 2);
        assert_eq!(raw_to_plane(&[], DType::F32, true).len(), 0);
    }

    #[test]
    fn xml_attr_basic_and_quotes() {
        let tag = r#"<Pixels SizeC="2" Name='a b' Empty="">"#;
        assert_eq!(xml_attr(tag, "SizeC"), Some("2"));
        assert_eq!(xml_attr(tag, "Name"), Some("a b"));
        assert_eq!(xml_attr(tag, "Empty"), Some(""));
        assert_eq!(xml_attr(tag, "Missing"), None);
    }

    #[test]
    fn xml_attr_whitespace_around_equals() {
        assert_eq!(xml_attr(r#"<a x = "1">"#, "x"), Some("1"));
    }

    #[test]
    fn xml_attr_does_not_match_name_suffix() {
        let tag = r#"<Pixels PhysicalSizeC="9" SizeC="2">"#;
        assert_eq!(xml_attr(tag, "SizeC"), Some("2"));
        assert_eq!(xml_attr(r#"<Pixels PhysicalSizeC="9">"#, "SizeC"), None);
        assert_eq!(xml_attr(r#"<Pixels SizeC="2">"#, "PhysicalSizeC"), None);
    }

    #[test]
    fn xml_attr_longer_name_with_same_prefix() {
        // "SizeCX" starts with "SizeC" but is a different attribute.
        assert_eq!(xml_attr(r#"<P SizeCX="1" SizeC="2">"#, "SizeC"), Some("2"));
        assert_eq!(xml_attr(r#"<P SizeCX="1">"#, "SizeC"), None);
    }

    #[test]
    fn xml_attr_unquoted_is_none() {
        assert_eq!(xml_attr("<a x=1>", "x"), None);
    }

    #[test]
    fn xml_tags_iterates_opening_tags() {
        let xml = r#"<OME><Pixels SizeC="2"><Channel ID="a"/><Channel ID="b"></Channel><ChannelX/><Pixel/></Pixels></OME>"#;
        let ch: Vec<_> = xml_tags(xml, "Channel").collect();
        assert_eq!(ch, [r#"<Channel ID="a"/"#, r#"<Channel ID="b""#]);
        assert_eq!(xml_tags(xml, "Pixels").next(), Some(r#"<Pixels SizeC="2""#));
        assert_eq!(xml_tags(xml, "Nope").count(), 0);
        assert_eq!(xml_tags("<A>", "A").count(), 1);
        assert_eq!(xml_tags("<A/>", "A").count(), 1);
    }
}
