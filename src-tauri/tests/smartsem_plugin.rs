//! Zeiss SmartSEM TIFF metadata through the "smartsem" WebAssembly plugin,
//! end to end (built from `plugins/smartsem`, installed into a temporary
//! plugin folder, used through `formats::open_with`).

mod common;

use std::path::Path;

use tifmrcview_lib::formats;
use tifmrcview_lib::plugins::registry::Registry;

/// A little-endian 8-bit gray TIFF, one strip, with optional extra tags
/// (tag, type, count, bytes), which must have numbers above 279.
fn tiff(path: &Path, w: u32, h: u32, extra: &[(u16, u16, u32, Vec<u8>)]) {
    let pixels: Vec<u8> = (0..w * h).map(|i| (i % 251) as u8).collect();
    let mut tags: Vec<(u16, u16, u32, Vec<u8>)> = vec![
        (256, 4, 1, w.to_le_bytes().to_vec()),
        (257, 4, 1, h.to_le_bytes().to_vec()),
        (258, 3, 1, 8u16.to_le_bytes().to_vec()),
        (259, 3, 1, 1u16.to_le_bytes().to_vec()),
        (262, 3, 1, 1u16.to_le_bytes().to_vec()),
        (273, 4, 1, vec![]), // patched: strip offset
        (277, 3, 1, 1u16.to_le_bytes().to_vec()),
        (278, 4, 1, h.to_le_bytes().to_vec()),
        (279, 4, 1, (w * h).to_le_bytes().to_vec()),
    ];
    tags.extend(extra.iter().cloned());
    let ifd_len = 2 + tags.len() * 12 + 4;
    let mut data_at = 8 + ifd_len;
    let mut blobs = vec![];
    let mut out = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    let strip_at = {
        let big: usize = tags.iter().filter(|t| t.3.len() > 4).map(|t| t.3.len()).sum();
        (data_at + big) as u32
    };
    for (tag, ty, count, mut bytes) in tags {
        if tag == 273 {
            bytes = strip_at.to_le_bytes().to_vec();
        }
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        if bytes.len() <= 4 {
            let mut v = bytes.clone();
            v.resize(4, 0);
            out.extend_from_slice(&v);
        } else {
            out.extend_from_slice(&(data_at as u32).to_le_bytes());
            data_at += bytes.len();
            blobs.push(bytes);
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    for b in blobs {
        out.extend(b);
    }
    assert_eq!(out.len(), strip_at as usize);
    out.extend(pixels);
    std::fs::write(path, out).unwrap();
}

/// CZ_SEM text as SmartSEM writes it (Latin-1).
fn cz_sem() -> Vec<u8> {
    let mut t = b"0\r\n 0\r\n 0\r\n1.5625e-008\r\n".to_vec();
    for (code, line) in [
        ("AP_WD", "WD =  2.7 mm"),
        ("AP_MAG", "Mag =  20.00 K X"),
        ("AP_IMAGE_PIXEL_SIZE", "Image Pixel Size = 15.63 nm"),
        ("DP_DETECTOR_CHANNEL", "Signal A = InLens"),
        ("SV_VERSION", "Version = V07.04.00.00 : 10-May-23"),
        ("AP_DATE", "Date :22 Oct 2026"),
        ("AP_ACTUALKV", "EHT =  1.50 kV"),
        ("AP_STAGE_AT_X", "Stage at X = 10.5 \u{b5}m"),
        ("DP_EMPTY", "Empty Value ="),
    ] {
        t.extend_from_slice(code.as_bytes());
        t.extend_from_slice(b"\r\n");
        // Latin-1, as SmartSEM writes it.
        t.extend(line.chars().map(|c| c as u32 as u8));
        t.extend_from_slice(b"\r\n");
    }
    t.push(0);
    t
}

fn plugins() -> Option<common::Plugins> {
    common::install(&["smartsem"])
}

fn items(g: &tifmrcview_lib::image::MetaGroup) -> Vec<(String, String)> {
    g.items.iter().map(|[a, b]| (a.clone(), b.clone())).collect()
}

#[test]
fn smartsem_tiff_gets_settings_pixel_size_and_format() {
    let Some(p) = plugins() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sem.tif");
    let tag = cz_sem();
    tiff(&path, 1024, 2, &[(34118, 1, tag.len() as u32, tag)]);
    let src = formats::open_with(&path, &p.registry).unwrap();
    let i = src.info();
    assert_eq!(i.format, "Zeiss SmartSEM TIFF");
    assert_eq!((i.pixel_size, i.unit.as_str()), (Some([15.625, 15.625, 0.0]), "nm"));
    let names: Vec<&str> = i.metadata.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["SmartSEM 主要参数", "全部数值参数 (AP)", "全部状态 (DP)", "系统信息 (SV)"]);
    let s = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(
        items(&i.metadata[0]),
        [
            s("Date", "22 Oct 2026"),
            s("EHT", "1.50 kV"),
            s("WD", "2.7 mm"),
            s("Mag", "20.00 K X"),
            s("Image Pixel Size", "15.63 nm"),
            s("Signal A", "InLens"),
            s("Stage at X", "10.5 µm"),
            s("Version", "V07.04.00.00 : 10-May-23"),
        ]
    );
    assert_eq!(items(&i.metadata[2]), [s("Empty Value", ""), s("Signal A", "InLens")]);
    // Pixels still come from the core reader.
    assert_eq!(src.read_page(0).unwrap()[0].data.get_f64(3), 3.0);

    // A narrower store scales the full-precision calibration.
    let tag = cz_sem();
    let path = tmp.path().join("sem512.tif");
    tiff(&path, 512, 1, &[(34118, 1, tag.len() as u32, tag)]);
    let i = formats::open_with(&path, &p.registry).unwrap().info().clone();
    assert_eq!(i.pixel_size, Some([15.63, 15.63, 0.0]), "header value disagrees: the nominal size wins");
}

#[test]
fn other_tiffs_are_untouched_and_the_switch_is_respected() {
    let Some(p) = plugins() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain.tif");
    tiff(&plain, 4, 4, &[]);
    let junk = tmp.path().join("junk.tif");
    tiff(&junk, 4, 4, &[(34118, 2, 6, b"hello\0".to_vec())]);
    for path in [&plain, &junk] {
        let i = formats::open_with(path, &p.registry).unwrap().info().clone();
        assert!(i.format.contains("TIFF") && !i.format.contains("SmartSEM"), "{}", i.format);
        assert!(i.metadata.is_empty() && i.pixel_size.is_none());
    }

    let sem = tmp.path().join("sem.tif");
    let tag = cz_sem();
    tiff(&sem, 1024, 1, &[(34118, 1, tag.len() as u32, tag)]);
    p.registry.set_disabled(vec!["smartsem".into()]).unwrap();
    assert!(formats::open_with(&sem, &p.registry).unwrap().info().metadata.is_empty());
    p.registry.set_disabled(vec![]).unwrap();
    assert!(!formats::open_with(&sem, &p.registry).unwrap().info().metadata.is_empty());

    // Without the plugin the TIFF opens as before.
    let none = Registry::new(Some(tmp.path().join("none")), "0.4.0");
    assert!(formats::open_with(&sem, &none).unwrap().info().metadata.is_empty());
}
