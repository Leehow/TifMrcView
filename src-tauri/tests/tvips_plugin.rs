//! TVIPS formats through the "tvips" WebAssembly plugin, end to end: the
//! plugin is built from `plugins/tvips`, installed into a temporary plugin
//! folder and used through `formats::open_with`, as the app does. The
//! files are written by the tests.

mod common;

use std::path::Path;

use tifmrcview_lib::formats;
use tifmrcview_lib::image::{DType, ImageInfo};

macro_rules! tvips {
    () => {
        match common::install(&["tvips"]) {
            Some(p) => p,
            None => return,
        }
    };
}

fn item(i: &ImageInfo, group: &str, key: &str) -> Option<String> {
    let g = i.metadata.iter().find(|g| g.name == group)?;
    g.items
        .iter()
        .find(|it| it[0] == key)
        .map(|it| it[1].clone())
}

fn put(b: &mut [u8], o: usize, v: &[u8]) {
    b[o..o + v.len()].copy_from_slice(v);
}

/// A version 2 stream of `frames` 6 × 4 u16 frames with 128-byte frame
/// headers; pixel `i` of frame `k` is `1000 k + i`.
fn stream(path: &Path, frames: u16) {
    let mut b = vec![0u8; 256];
    for (o, v) in [
        (0, 256),
        (4, 2),
        (8, 6),
        (12, 4),
        (16, 16),
        (20, 512),
        (24, 256),
        (28, 2),
        (32, 2),
        (36, 3),
        (40, 300),
        (44, 59000),
        (48, 128),
    ] {
        put(&mut b, o, &(v as i32).to_le_bytes());
    }
    for k in 0..frames {
        let mut f = vec![0u8; 128];
        put(&mut f, 0, &(k as u32 + 1).to_le_bytes());
        put(&mut f, 4, &1456073429u32.to_le_bytes());
        put(&mut f, 8, &250u32.to_le_bytes());
        put(&mut f, 20, &59000u32.to_le_bytes());
        put(&mut f, 28, &1500f32.to_le_bytes());
        put(&mut f, 40, &(-20.5f32).to_le_bytes());
        b.extend(f);
        b.extend((0..24u16).flat_map(|i| (1000 * k + i).to_le_bytes()));
    }
    std::fs::write(path, b).unwrap();
}

#[test]
fn tvips_stream_frames_and_header() {
    let p = tvips!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("movie_000.tvips");
    stream(&path, 3);
    let src = formats::open_with(&path, &p.registry).unwrap();
    let i = src.info().clone();
    assert_eq!(
        (i.width, i.height, i.size_t, i.page_count, i.dtype),
        (6, 4, 3, 3, DType::U16)
    );
    assert_eq!(i.format, "TVIPS");
    assert_eq!(
        (i.unit.as_str(), i.pixel_size),
        ("nm", Some([3.0, 3.0, 0.0]))
    );
    let m = |k| item(&i, "TVIPS 影像", k);
    assert_eq!(m("加速电压").as_deref(), Some("300 kV"));
    assert_eq!(m("像素合并").as_deref(), Some("2 × 2"));
    assert_eq!(
        m("第 1 帧时间").as_deref(),
        Some("2016-02-21 16:50:29 UTC（+250 ms）")
    );
    assert_eq!(m("α / β 倾转").as_deref(), Some("-20.5° / 0°"));
    for k in [0u16, 2] {
        let page = src.read_page(k as usize).unwrap();
        let d = &page[0].data;
        let got: Vec<f64> = (0..d.len()).map(|i| d.get_f64(i)).collect();
        let want: Vec<f64> = (0..24).map(|i| f64::from(1000 * k + i)).collect();
        assert_eq!(got, want, "frame {k}");
    }

    let cont = tmp.path().join("movie_001.tvips");
    std::fs::write(&cont, vec![9u8; 4096]).unwrap();
    let err = match formats::open_with(&cont, &p.registry) {
        Ok(_) => panic!("continuation opened"),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("_000"), "{err}");
}

/// A little-endian 8-bit 4 × 2 TIFF with a version 2 TemData block that
/// tag 37706 points at.
fn emmenu_tiff(path: &Path) {
    let mut t = vec![0u8; 6312];
    put(&mut t, 0, &2i32.to_le_bytes());
    let text = |t: &mut [u8], o: usize, s: &str| {
        for (k, c) in s.encode_utf16().enumerate() {
            put(t, o + 2 * k, &c.to_le_bytes());
        }
    };
    text(&mut t, 244, "grid 3");
    put(
        &mut t,
        584,
        &((2017 << 16 | 4 << 8 | 12) as i32).to_le_bytes(),
    );
    put(&mut t, 588, &32477i32.to_le_bytes());
    put(&mut t, 2968, &0.4208f32.to_le_bytes());
    put(&mut t, 2972, &0.4208f32.to_le_bytes());
    text(&mut t, 3112, "JEM-2100");
    put(&mut t, 3272, &200000f32.to_le_bytes());
    put(&mut t, 3536, &40000f32.to_le_bytes());
    text(&mut t, 3768, "XF416");
    put(&mut t, 3944, &1i32.to_le_bytes());
    put(&mut t, 3948, &1i32.to_le_bytes());
    put(&mut t, 3952, &500f32.to_le_bytes());
    put(&mut t, 6308, &0xAAAA_AAAAu32.to_le_bytes());

    let tags: [(u16, u16, u32); 10] = [
        (256, 4, 4),
        (257, 4, 2),
        (258, 3, 8),
        (259, 3, 1),
        (262, 3, 1),
        (273, 4, 0),
        (277, 3, 1),
        (278, 4, 2),
        (279, 4, 8),
        (37706, 4, 0),
    ];
    let ifd_end = 8 + 2 + tags.len() * 12 + 4;
    let (strip, tem) = (ifd_end as u32, ifd_end as u32 + 8);
    let mut b = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    b.extend((tags.len() as u16).to_le_bytes());
    for (tag, ty, v) in tags {
        let v = match tag {
            273 => strip,
            37706 => tem,
            _ => v,
        };
        b.extend(tag.to_le_bytes());
        b.extend(ty.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(v.to_le_bytes());
    }
    b.extend(0u32.to_le_bytes());
    b.extend(0..8u8);
    b.extend(t);
    std::fs::write(path, b).unwrap();
}

#[test]
fn emmenu_tiff_settings_and_pixel_size() {
    let p = tvips!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("grid.tif");
    emmenu_tiff(&path);
    let src = formats::open_with(&path, &p.registry).unwrap();
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.dtype), (4, 2, DType::U8));
    assert_eq!(i.format, "TVIPS EM-MENU TIFF (XF416)");
    assert_eq!(i.unit, "nm");
    let [x, y, _] = i.pixel_size.unwrap();
    assert!(
        (x - 0.4208).abs() < 1e-9 && (y - 0.4208).abs() < 1e-9,
        "{x} {y}"
    );
    let m = |k| item(&i, "TVIPS EM-MENU", k);
    assert_eq!(m("图像名称").as_deref(), Some("grid 3"));
    assert_eq!(m("日期").as_deref(), Some("2017-04-12"));
    assert_eq!(m("时间").as_deref(), Some("09:01:17"));
    assert_eq!(m("显微镜").as_deref(), Some("JEM-2100"));
    assert_eq!(m("加速电压").as_deref(), Some("200 kV"));
    assert_eq!(m("放大倍数").as_deref(), Some("40000×"));
    assert_eq!(m("曝光时间").as_deref(), Some("500 ms"));
    let d = &src.read_page(0).unwrap()[0].data;
    assert_eq!(
        (0..d.len()).map(|i| d.get_f64(i)).collect::<Vec<_>>(),
        [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]
    );
}
