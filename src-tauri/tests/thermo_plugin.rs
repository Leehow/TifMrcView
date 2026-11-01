//! Thermo Fisher formats through the "thermo" WebAssembly plugin, end to
//! end: the plugin is built from `plugins/thermo`, installed into a
//! temporary plugin folder and used through `formats::open_with` /
//! `open_series_with`, as the app does.
//!
//! TIA SER, EER, FEI TIFF and FEI MRC files are written by the tests; the
//! Velox files in `tests/fixtures/` come from
//! `plugins/thermo/tests/make_fixtures.py`. Public samples (rosettasciio
//! test data, EMPIAR-10568) are checked when present in
//! /home/user/samples/thermo; the expected values come from h5py, numpy
//! and imagecodecs.

mod common;

use std::path::{Path, PathBuf};

use tifmrcview_lib::formats;
use tifmrcview_lib::image::{DType, ImageInfo, PlaneData, Source, SpectrumInfo};
use tifmrcview_lib::plugins::registry::Registry;

macro_rules! thermo {
    () => {
        match common::install(&["thermo"]) {
            Some(p) => p,
            None => return,
        }
    };
}

fn fixture(name: &str) -> PathBuf {
    common::repo().join("src-tauri/tests/fixtures").join(name)
}

fn values(p: &PlaneData) -> Vec<f64> {
    (0..p.len()).map(|i| p.get_f64(i)).collect()
}

fn open(reg: &Registry, path: &Path) -> Box<dyn Source> {
    formats::open_with(path, reg).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()))
}

fn open_err(reg: &Registry, path: &Path) -> String {
    match formats::open_with(path, reg) {
        Ok(_) => panic!("{} opened", path.display()),
        Err(e) => format!("{e:#}"),
    }
}

/// Unit and square pixel size, to ten significant digits.
fn px(i: &ImageInfo, unit: &str, size: f64) {
    let [x, y, _] = i
        .pixel_size
        .unwrap_or_else(|| panic!("no pixel size, want {size} {unit}"));
    assert_eq!(i.unit, unit);
    for v in [x, y] {
        assert!(
            (v - size).abs() <= size * 1e-9,
            "pixel size {v} {}, want {size} {unit}",
            i.unit
        );
    }
}

fn item(i: &ImageInfo, group: &str, key: &str) -> Option<String> {
    let g = i.metadata.iter().find(|g| g.name == group)?;
    g.items
        .iter()
        .find(|it| it[0] == key)
        .map(|it| it[1].clone())
}

fn page(src: &dyn Source, p: usize) -> Vec<f64> {
    values(&src.read_page(p).unwrap()[0].data)
}

/// The pixel values of the Velox fixtures, as `pattern()` in make_fixtures.py.
fn pattern(f: u64, y: u64, x: u64, modulo: u64) -> f64 {
    ((x * 7 + y * 13 + f * 101) % modulo) as f64
}

fn grid(w: u64, h: u64, f: impl Fn(u64, u64) -> f64) -> Vec<f64> {
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (y, x)))
        .map(|(y, x)| f(y, x))
        .collect()
}

// ---------------------------------------------------------------- TIA SER

/// A TIA series of 2D elements, rows stored bottom-up as TIA does.
/// `images[k][y][x]` is the image as displayed.
fn ser(path: &Path, version: u16, delta: f64, images: &[Vec<Vec<i16>>]) {
    let wide = version >= 0x220;
    let n = images.len() as u32;
    let mut b = vec![];
    b.extend(0x4949u16.to_le_bytes());
    b.extend(0x0197u16.to_le_bytes());
    b.extend(version.to_le_bytes());
    b.extend(0x4122u32.to_le_bytes());
    b.extend(0x4152u32.to_le_bytes());
    b.extend(n.to_le_bytes());
    b.extend(n.to_le_bytes());
    let arrays_at = b.len();
    if wide {
        b.extend(0u64.to_le_bytes());
    } else {
        b.extend(0u32.to_le_bytes());
    }
    b.extend(1u32.to_le_bytes()); // one dimension
    b.extend(n.to_le_bytes());
    b.extend(0f64.to_le_bytes());
    b.extend(1f64.to_le_bytes());
    b.extend(0u32.to_le_bytes());
    b.extend(6u32.to_le_bytes());
    b.extend(b"Number");
    b.extend(0u32.to_le_bytes());
    let mut data = vec![];
    let mut tags = vec![];
    for (k, img) in images.iter().enumerate() {
        data.push(b.len() as u64);
        let (h, w) = (img.len(), img[0].len());
        for _ in 0..2 {
            b.extend((-1e-9f64).to_le_bytes());
            b.extend(delta.to_le_bytes());
            b.extend(0i32.to_le_bytes());
        }
        b.extend(5u16.to_le_bytes()); // i16
        b.extend((w as u32).to_le_bytes());
        b.extend((h as u32).to_le_bytes());
        for row in img.iter().rev() {
            for v in row {
                b.extend(v.to_le_bytes());
            }
        }
        tags.push(b.len() as u64);
        b.extend(0x4152u16.to_le_bytes());
        b.extend(0u16.to_le_bytes());
        b.extend((1456073429u32 + k as u32).to_le_bytes());
    }
    let arrays = b.len() as u64;
    for v in data.iter().chain(&tags) {
        if wide {
            b.extend(v.to_le_bytes());
        } else {
            b.extend((*v as u32).to_le_bytes());
        }
    }
    if wide {
        b[arrays_at..arrays_at + 8].copy_from_slice(&arrays.to_le_bytes());
    } else {
        b[arrays_at..arrays_at + 4].copy_from_slice(&(arrays as u32).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

fn img(w: usize, h: usize, k: i16) -> Vec<Vec<i16>> {
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| (x * 3 + y * 10) as i16 - 7 + 100 * k)
                .collect()
        })
        .collect()
}

#[test]
fn tia_ser_series_rows_flipped_and_calibrated() {
    let p = thermo!();
    let tmp = tempfile::tempdir().unwrap();
    for version in [0x210, 0x220] {
        let path = tmp.path().join(format!("a_{version:x}_1.ser"));
        ser(&path, version, 2.5e-10, &[img(4, 3, 0), img(4, 3, 1)]);
        let src = open(&p.registry, &path);
        let i = src.info().clone();
        assert_eq!(
            (i.width, i.height, i.size_t, i.page_count, i.dtype),
            (4, 3, 2, 2, DType::I16)
        );
        assert_eq!(i.format, "TIA SER");
        px(&i, "nm", 0.25);
        assert_eq!(
            item(&i, "TIA 图像", "X 标定").as_deref(),
            Some("0.25 nm/像素")
        );
        assert_eq!(
            item(&i, "TIA 图像", "采集时间（第 1 张）").as_deref(),
            Some("2016-02-21 16:50:29 UTC")
        );
        for k in 0..2 {
            let want: Vec<f64> = img(4, 3, k as i16)
                .concat()
                .into_iter()
                .map(f64::from)
                .collect();
            assert_eq!(page(&*src, k), want, "version {version:x} image {k}");
        }
    }
}

#[test]
fn tia_diffraction_in_reciprocal_units_and_emi_explains() {
    let p = thermo!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("diff_1.ser");
    ser(&path, 0x220, 4.2e7, &[img(2, 2, 0)]);
    let i = open(&p.registry, &path).info().clone();
    px(&i, "1/nm", 0.042);
    assert_eq!(
        item(&i, "TIA 图像", "标定类型").as_deref(),
        Some("倒空间（衍射）")
    );

    let emi = tmp.path().join("diff.emi");
    std::fs::write(&emi, b"anything").unwrap();
    assert!(open_err(&p.registry, &emi).contains("_1.ser"));

}

/// A TIA series of 1D elements (spectra) with `dims` series dimensions.
fn ser_spectra(path: &Path, dims: &[u32], spectra: &[Vec<i32>]) {
    let n = spectra.len() as u32;
    let mut b = vec![];
    b.extend(0x4949u16.to_le_bytes());
    b.extend(0x0197u16.to_le_bytes());
    b.extend(0x0220u16.to_le_bytes());
    b.extend(0x4120u32.to_le_bytes());
    b.extend(0x4152u32.to_le_bytes());
    b.extend(n.to_le_bytes());
    b.extend(n.to_le_bytes());
    let arrays_at = b.len();
    b.extend(0u64.to_le_bytes());
    b.extend((dims.len() as u32).to_le_bytes());
    for &d in dims {
        b.extend(d.to_le_bytes());
        b.extend(0f64.to_le_bytes());
        b.extend(1e-9f64.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(b"X");
        b.extend(0u32.to_le_bytes());
    }
    let mut data = vec![];
    for s in spectra {
        data.push(b.len() as u64);
        b.extend(100f64.to_le_bytes()); // value at the calibration element
        b.extend(2.5f64.to_le_bytes()); // eV per channel
        b.extend(4i32.to_le_bytes()); // calibration element
        b.extend(6u16.to_le_bytes()); // i32
        b.extend((s.len() as u32).to_le_bytes());
        for v in s {
            b.extend(v.to_le_bytes());
        }
    }
    let arrays = b.len() as u64;
    for v in data.iter().chain(&data) {
        b.extend(v.to_le_bytes());
    }
    b[arrays_at..arrays_at + 8].copy_from_slice(&arrays.to_le_bytes());
    std::fs::write(path, b).unwrap();
}

#[test]
fn tia_spectra_become_rows() {
    let p = thermo!();
    let tmp = tempfile::tempdir().unwrap();
    let spec = |k: i32| (0..6).map(|i| i * 10 + k).collect::<Vec<i32>>();
    // A single spectrum.
    let path = tmp.path().join("eds_1.ser");
    ser_spectra(&path, &[1], &[spec(0)]);
    let src = open(&p.registry, &path);
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.page_count, i.dtype), (6, 1, 1, DType::I32));
    assert_eq!(
        i.spectrum,
        Some(SpectrumInfo { axis: "x".into(), start: 90.0, step: 2.5, unit: "eV".into(), kind: String::new() })
    );
    assert_eq!(page(&*src, 0), [0.0, 10.0, 20.0, 30.0, 40.0, 50.0]);
    // A 3 × 2 spectrum image: one scan line per page.
    let path = tmp.path().join("si_1.ser");
    ser_spectra(&path, &[3, 2], &(0..6).map(spec).collect::<Vec<_>>());
    let src = open(&p.registry, &path);
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.page_count), (6, 3, 2));
    let row = |k: i32| spec(k).into_iter().map(f64::from).collect::<Vec<_>>();
    assert_eq!(page(&*src, 1), [row(3), row(4), row(5)].concat());
}

// ---------------------------------------------------------------- Velox EMD

#[test]
fn velox_series_frames_and_settings() {
    let p = thermo!();
    let path = fixture("thermo_velox.emd");
    let src = open(&p.registry, &path);
    let i = src.info().clone();
    let names: Vec<&str> = i.series.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["HAADF", "BM-Ceta", "DF4"]);
    assert_eq!(i.series[0].detail, "10×12 · 5 帧 · u16");
    assert_eq!(
        (i.width, i.height, i.size_t, i.dtype),
        (10, 12, 5, DType::U16)
    );
    assert_eq!(i.format, "Velox EMD (Velox 3.7.0)");
    px(&i, "nm", 0.53);
    let v = |k| item(&i, "Velox 图像", k);
    assert_eq!(v("显微镜").as_deref(), Some("Talos F200X"));
    assert_eq!(v("加速电压").as_deref(), Some("200 kV"));
    assert_eq!(v("标称放大倍数").as_deref(), Some("160000×"));
    assert_eq!(v("相机长度").as_deref(), Some("98 mm"));
    assert_eq!(v("采集时间").as_deref(), Some("2016-02-21 16:50:29 UTC"));
    assert_eq!(v("散焦").as_deref(), Some("-150 nm"));
    assert_eq!(v("α 倾转").as_deref(), Some("15°"));
    assert_eq!(v("样品台 X").as_deref(), Some("12.5 µm"));
    assert_eq!(v("压缩").as_deref(), Some("shuffle + gzip"));
    assert_eq!(
        v("说明").as_deref(),
        Some("文件里还有谱图数据（EDS / EELS），不显示")
    );
    assert_eq!(
        item(&i, "Velox 全部参数", "Note").as_deref(),
        Some("café \"quoted\"")
    );
    for f in [0, 4] {
        assert_eq!(
            page(&*src, f),
            grid(10, 12, |y, x| pattern(f as u64, y, x, 60000)),
            "HAADF frame {f}"
        );
    }

    // Three frames interleaved in one chunk.
    let src = formats::open_series_with(&path, 1, &p.registry).unwrap();
    let i = src.info().clone();
    assert_eq!(
        (i.width, i.height, i.size_t, i.dtype, i.series_index),
        (9, 6, 3, DType::I16, 1)
    );
    px(&i, "Å", 0.25);
    for f in 0..3 {
        assert_eq!(
            page(&*src, f),
            grid(9, 6, |y, x| pattern(f as u64, y, x, 30000) - 5000.0),
            "Ceta frame {f}"
        );
    }

    // 70 interleaved frames, contiguous: copied out by the plugin.
    let src = formats::open_series_with(&path, 2, &p.registry).unwrap();
    let i = src.info().clone();
    assert_eq!(
        (i.width, i.height, i.size_t, i.dtype),
        (3, 4, 70, DType::F32)
    );
    for f in [0, 1, 69] {
        assert_eq!(
            page(&*src, f),
            grid(3, 4, |y, x| pattern(f as u64, y, x, 1000) / 4.0),
            "DF4 frame {f}"
        );
    }
}

#[test]
fn velox_without_images_explains() {
    let p = thermo!();
    assert!(open_err(&p.registry, &fixture("thermo_spectrum.emd")).contains("只有谱图数据"));
    let tmp = tempfile::tempdir().unwrap();
    let junk = tmp.path().join("x.emd");
    std::fs::write(&junk, vec![7u8; 4096]).unwrap();
    assert!(open_err(&p.registry, &junk).contains("不是 Velox EMD 文件"));
}

// ---------------------------------------------------------------- EER

/// Encodes events (pixel indices, ascending) as an EER 7-bit run stream.
fn eer_stream(events: &[usize], pixels: usize) -> Vec<u8> {
    let mut bits: Vec<bool> = vec![];
    let push =
        |v: u32, n: u32, bits: &mut Vec<bool>| (0..n).for_each(|i| bits.push(v >> i & 1 == 1));
    let mut at = 0;
    for &e in events {
        let mut run = e - at;
        while run >= 127 {
            push(127, 7, &mut bits);
            run -= 127;
        }
        push(run as u32, 7, &mut bits);
        push(0b1010, 4, &mut bits);
        at = e + 1;
    }
    // Run to the end of the frame.
    let mut run = pixels - at;
    while run >= 127 {
        push(127, 7, &mut bits);
        run -= 127;
    }
    push(run as u32, 7, &mut bits);
    let mut out = vec![0u8; bits.len().div_ceil(16) * 2 + 2];
    for (i, b) in bits.iter().enumerate() {
        if *b {
            out[i / 8] |= 1 << (i % 8);
        }
    }
    out
}

/// A little-endian BigTIFF, one strip per frame, compression 65001, the
/// acquisition XML in tag 65001 of the first directory.
fn eer(path: &Path, w: u64, h: u64, frames: &[Vec<usize>]) {
    let xml = br#"<metadata><item name="exposureTime" unit="s">2.5</item><item name="numberOfFrames">3</item><item name="sensorPixelSize.width" unit="m">1.2e-10</item></metadata>"#;
    let mut b = b"II\x2b\x00\x08\x00\x00\x00".to_vec();
    b.extend(0u64.to_le_bytes()); // first IFD, patched
    let mut link = 8;
    for (k, events) in frames.iter().enumerate() {
        let strip = eer_stream(events, (w * h) as usize);
        let strip_at = b.len() as u64;
        b.extend(&strip);
        let xml_at = b.len() as u64;
        if k == 0 {
            b.extend(xml);
        }
        if b.len() % 2 == 1 {
            b.push(0);
        }
        let ifd = b.len() as u64;
        b[link..link + 8].copy_from_slice(&ifd.to_le_bytes());
        let mut tags: Vec<(u16, u16, u64, u64)> = vec![
            (256, 4, 1, w),
            (257, 4, 1, h),
            (259, 3, 1, 65001),
            (262, 3, 1, 1),
            (273, 16, 1, strip_at),
            (278, 4, 1, h),
            (279, 16, 1, strip.len() as u64),
        ];
        if k == 0 {
            tags.push((65001, 7, xml.len() as u64, xml_at));
        }
        b.extend((tags.len() as u64).to_le_bytes());
        for (tag, typ, count, value) in tags {
            b.extend(tag.to_le_bytes());
            b.extend(typ.to_le_bytes());
            b.extend(count.to_le_bytes());
            b.extend(value.to_le_bytes());
        }
        link = b.len();
        b.extend(0u64.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

#[test]
fn eer_frames_are_counted() {
    let p = thermo!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("movie.eer");
    let frames = vec![vec![0, 5, 6, 300, 399], vec![5, 200], vec![]];
    eer(&path, 20, 20, &frames);
    let src = open(&p.registry, &path);
    let i = src.info().clone();
    assert_eq!(
        (i.width, i.height, i.size_t, i.dtype),
        (20, 20, 3, DType::U16)
    );
    assert_eq!(i.format, "Thermo Fisher EER");
    assert!(i.series.is_empty(), "few frames: no summed view");
    px(&i, "nm", 0.12);
    assert_eq!(
        item(&i, "EER 采集参数", "exposureTime").as_deref(),
        Some("2.5 s")
    );
    assert_eq!(
        item(&i, "EER 电子计数", "编码").as_deref(),
        Some("65001（游程 7 位，亚像素 2+2 位）")
    );
    for (k, events) in frames.iter().enumerate() {
        let mut want = vec![0.0; 400];
        for &e in events {
            want[e] += 1.0;
        }
        assert_eq!(page(&*src, k), want, "frame {k}");
    }
}

// ---------------------------------------------------------------- metadata

/// A little-endian 8-bit gray TIFF, one strip, with extra tags above 279.
fn tiff(path: &Path, w: u32, h: u32, extra: &[(u16, u16, u32, Vec<u8>)]) {
    let mut tags: Vec<(u16, u16, u32, Vec<u8>)> = vec![
        (256, 4, 1, w.to_le_bytes().to_vec()),
        (257, 4, 1, h.to_le_bytes().to_vec()),
        (258, 3, 1, 8u16.to_le_bytes().to_vec()),
        (259, 3, 1, 1u16.to_le_bytes().to_vec()),
        (262, 3, 1, 1u16.to_le_bytes().to_vec()),
        (273, 4, 1, vec![]),
        (277, 3, 1, 1u16.to_le_bytes().to_vec()),
        (278, 4, 1, h.to_le_bytes().to_vec()),
        (279, 4, 1, (w * h).to_le_bytes().to_vec()),
    ];
    tags.extend(extra.iter().cloned());
    let mut data_at = 8 + 2 + tags.len() * 12 + 4;
    let strip_at = (data_at
        + tags
            .iter()
            .filter(|t| t.3.len() > 4)
            .map(|t| t.3.len())
            .sum::<usize>()) as u32;
    let mut out = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    out.extend((tags.len() as u16).to_le_bytes());
    let mut blobs = vec![];
    for (tag, ty, count, mut bytes) in tags {
        if tag == 273 {
            bytes = strip_at.to_le_bytes().to_vec();
        }
        out.extend(tag.to_le_bytes());
        out.extend(ty.to_le_bytes());
        out.extend(count.to_le_bytes());
        if bytes.len() <= 4 {
            bytes.resize(4, 0);
            out.extend(bytes);
        } else {
            out.extend((data_at as u32).to_le_bytes());
            data_at += bytes.len();
            blobs.push(bytes);
        }
    }
    out.extend(0u32.to_le_bytes());
    blobs.into_iter().for_each(|b| out.extend(b));
    out.extend((0..w * h).map(|i| (i % 251) as u8));
    std::fs::write(path, out).unwrap();
}

#[test]
fn fei_tiff_settings_and_pixel_size() {
    let p = thermo!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("helios.tif");
    let ini = "[User]\r\nDate=06/13/2016\r\nTime=05:06:40 PM\r\nUser=supervisor\r\n[System]\r\nType=DualBeam\r\nSystemType=Helios NanoLab\" 660\r\n[Beam]\r\nHV=5000\r\nBeam=EBeam\r\n[EBeam]\r\nHV=5000\r\nBeamCurrent=6.25e-12\r\nWD=0.00403466\r\nHFW=0.00172667\r\n[Scan]\r\nPixelWidth=3.3724e-06\r\nPixelHeight=3.3724e-06\r\nDwelltime=1e-05\r\n[Detectors]\r\nName=ETD\r\nMode=SE\r\n[PrivateFei]\r\nX=1\r\n\0";
    tiff(
        &path,
        8,
        4,
        &[(34682, 2, ini.len() as u32, ini.as_bytes().to_vec())],
    );
    let src = open(&p.registry, &path);
    let i = src.info().clone();
    assert_eq!(i.format, "Thermo Fisher TIFF (Helios NanoLab\" 660)");
    px(&i, "µm", 3.3724);
    let m = |k| item(&i, "Thermo Fisher 主要参数", k);
    assert_eq!(m("加速电压").as_deref(), Some("5 kV"));
    assert_eq!(m("束流").as_deref(), Some("6.25 pA"));
    assert_eq!(m("工作距离").as_deref(), Some("4.03466 mm"));
    assert_eq!(m("探测器").as_deref(), Some("ETD SE"));
    assert_eq!(m("驻留时间").as_deref(), Some("10 µs"));
    assert_eq!(
        item(&i, "Thermo Fisher [EBeam]", "BeamCurrent").as_deref(),
        Some("6.25e-12")
    );
    assert!(i.metadata.iter().all(|g| !g.name.contains("PrivateFei")));
    assert_eq!(page(&*src, 0)[5], 5.0, "pixels come from the core");

    let plain = tmp.path().join("plain.tif");
    tiff(&plain, 4, 4, &[]);
    assert_eq!(open(&p.registry, &plain).info().format, "TIFF");
}

/// An Auto Slice & View project: ProjectData.dat, two detectors' slices
/// (FEI TIFFs, 3 nm pixels), log images and an overview.
fn asv_project(root: &Path) {
    let xml = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<ProjectData>\r\n  <Name>worm</Name>\r\n  <TimeCreated>2015-11-10T08:29:09</TimeCreated>\r\n  <ExecutionResult>Finished</ExecutionResult>\r\n  <Results>\r\n    <Slice TargetThickness=\"1E-08\" MeasuredThickness=\"9E-09\" />\r\n    <Slice TargetThickness=\"1E-08\" MeasuredThickness=\"1.1E-08\" />\r\n    <Slice TargetThickness=\"1E-08\" MeasuredThickness=\"NaN\" />\r\n  </Results>\r\n</ProjectData>\r\n";
    std::fs::write(root.join("ProjectData.dat"), xml).unwrap();
    let ini = "[System]\r\nSystemType=Helios NanoLab\" 650\r\n[Scan]\r\nPixelWidth=3e-09\r\nPixelHeight=3e-09\r\n\0";
    let fei = [(34682u16, 2u16, ini.len() as u32, ini.as_bytes().to_vec())];
    for (dir, n) in [("Images/SEM Image", 3), ("Images/SEM Image 2", 2), ("LogImages", 4)] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        for i in 1..=n {
            tiff(&root.join(dir).join(format!("worm - SliceImage - {i:03}.tif")), 6, 4, &fei);
        }
    }
    tiff(&root.join("Images/overview.tif"), 6, 4, &fei);
}

#[test]
fn auto_slice_and_view_projects_are_z_stacks() {
    let p = thermo!();
    let tmp = tempfile::tempdir().unwrap();
    asv_project(tmp.path());
    let config = tmp.path().join("ProjectData.dat");
    assert!(p.registry.is_stack_file(&config));
    let src = open(&p.registry, &config);
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.size_z, i.page_count), (6, 4, 3, 3));
    assert!(i.format.starts_with("Thermo Auto Slice & View · "), "{}", i.format);
    let ps = i.pixel_size.unwrap();
    assert_eq!(i.unit, "nm");
    assert!((ps[0] - 3.0).abs() < 1e-9 && (ps[2] - 10.0).abs() < 1e-9, "{ps:?}");
    assert_eq!(item(&i, "Auto Slice & View 项目", "项目名称").as_deref(), Some("worm"));
    assert_eq!(item(&i, "Auto Slice & View 项目", "切片数（项目记录）").as_deref(), Some("3"));
    assert_eq!(item(&i, "Auto Slice & View 项目", "实测切片厚度").as_deref(), Some("10 nm ± 1 nm（2 片的平均）"));
    assert_eq!(item(&i, "图像序列", "文件数").as_deref(), Some("3"));
    assert!(item(&i, "图像序列", "Z 间距").unwrap().starts_with("10 nm"));
    assert_eq!(i.series.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["SEM Image", "SEM Image 2"]);
    assert_eq!(page(&*src, 2)[5], 5.0);
    assert!(i.folder.as_deref().unwrap().ends_with("Images/SEM Image"));

    // The other detector, through the series picker.
    let second = formats::open_series_with(&config, 1, &p.registry).unwrap();
    assert_eq!((second.info().size_z, second.info().series_index), (2, 1));

    // Opening an image folder of the project finds the project file two
    // levels up and shows that folder's stack.
    let dir = tmp.path().join("Images/SEM Image 2");
    let src = open(&p.registry, &dir);
    assert_eq!((src.info().size_z, src.info().series_index), (2, 1));
    assert_eq!(src.info().path, config.to_string_lossy());
    // The log images are not part of any project stack: a plain sequence.
    let logs = open(&p.registry, &tmp.path().join("LogImages"));
    assert_eq!(logs.info().size_z, 4);
    assert!(logs.info().format.starts_with("图像序列"), "{}", logs.info().format);
}

/// An MRC with an FEI2 extended header, `n` 2×2 i16 sections tilted by
/// 10° steps.
fn fei_mrc(path: &Path, n: usize) {
    const BLOCK: usize = 888;
    let mut h = vec![0u8; 1024];
    let put = |h: &mut Vec<u8>, o: usize, v: &[u8]| h[o..o + v.len()].copy_from_slice(v);
    put(&mut h, 0, &2i32.to_le_bytes());
    put(&mut h, 4, &2i32.to_le_bytes());
    put(&mut h, 8, &(n as i32).to_le_bytes());
    put(&mut h, 12, &1i32.to_le_bytes()); // mode 1: i16
    put(&mut h, 28, &2i32.to_le_bytes());
    put(&mut h, 32, &2i32.to_le_bytes());
    put(&mut h, 36, &(n as i32).to_le_bytes());
    for o in [40, 44, 48] {
        put(&mut h, o, &20f32.to_le_bytes());
    }
    for (o, v) in [(64, 1), (68, 2), (72, 3)] {
        put(&mut h, o, &(v as i32).to_le_bytes());
    }
    put(&mut h, 92, &((n * BLOCK) as i32).to_le_bytes());
    put(&mut h, 104, b"FEI2");
    put(&mut h, 108, &20140i32.to_le_bytes());
    put(&mut h, 208, b"MAP ");
    put(&mut h, 212, &[0x44, 0x44, 0, 0]);
    for k in 0..n {
        let mut e = vec![0u8; BLOCK];
        let put = |e: &mut Vec<u8>, o: usize, v: &[u8]| e[o..o + v.len()].copy_from_slice(v);
        put(&mut e, 0, &(BLOCK as i32).to_le_bytes());
        put(&mut e, 4, &2i32.to_le_bytes());
        // Timestamp, microscope, D-number, application, version, HT, α, X–Z, pixel size, defocus.
        let m1: u32 = 0b11_1111 | 1 << 7 | 0b111 << 9 | 0b11 << 14 | 1 << 22;
        put(&mut e, 8, &m1.to_le_bytes());
        put(&mut e, 12, &44900.5f64.to_le_bytes());
        put(&mut e, 20, b"Krios G4");
        put(&mut e, 36, b"3814");
        put(&mut e, 52, b"Tomography");
        put(&mut e, 68, b"5.12");
        put(&mut e, 84, &300000f64.to_le_bytes());
        put(&mut e, 100, &(-30.0 + 10.0 * k as f64).to_le_bytes());
        put(&mut e, 116, &2.5e-5f64.to_le_bytes());
        put(&mut e, 156, &1.35e-10f64.to_le_bytes());
        put(&mut e, 164, &1.35e-10f64.to_le_bytes());
        put(&mut e, 220, &(-2e-6f64).to_le_bytes());
        // Camera name and binning.
        put(&mut e, 297, &(1u32 << 17 | 1 << 19).to_le_bytes());
        put(&mut e, 427, &2i32.to_le_bytes());
        put(&mut e, 431, &2i32.to_le_bytes());
        put(&mut e, 435, b"BM-Falcon");
        h.extend(e);
    }
    for v in 0..(4 * n) as i16 {
        h.extend(v.to_le_bytes());
    }
    std::fs::write(path, h).unwrap();
}

#[test]
fn fei_mrc_extended_header() {
    let p = thermo!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("tilt.mrc");
    fei_mrc(&path, 7);
    let src = open(&p.registry, &path);
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.page_count), (2, 2, 7));
    px(&i, "nm", 0.135);
    let m = |k| item(&i, "Thermo Fisher MRC", k);
    assert_eq!(m("显微镜").as_deref(), Some("Krios G4"));
    assert_eq!(m("软件").as_deref(), Some("Tomography 5.12"));
    assert_eq!(m("时间").as_deref(), Some("2022-12-05 12:00:00"));
    assert_eq!(m("加速电压").as_deref(), Some("300 kV"));
    assert_eq!(m("α 倾转").as_deref(), Some("-30°"));
    assert_eq!(m("倾转范围").as_deref(), Some("-30° 到 30°（7 张）"));
    assert_eq!(m("散焦").as_deref(), Some("-2 µm"));
    assert_eq!(m("样品台 X").as_deref(), Some("25 µm"));
    assert_eq!(m("相机").as_deref(), Some("BM-Falcon"));
    assert_eq!(m("像素合并").as_deref(), Some("2 × 2"));
    assert_eq!(m("剂量"), None, "not flagged as valid");
    assert_eq!(page(&*src, 1), [4.0, 5.0, 6.0, 7.0]);
}

// ---------------------------------------------------------------- public samples

fn sample(name: &str) -> Option<PathBuf> {
    let p = Path::new("/home/user/samples/thermo").join(name);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("SKIPPED: {} is not there", p.display());
        None
    }
}

#[test]
fn public_samples() {
    let p = thermo!();
    if let Some(path) = sample("64x64_TEM_images_acquire_1.ser") {
        let src = open(&p.registry, &path);
        let i = src.info().clone();
        assert_eq!((i.width, i.height, i.dtype), (64, 64, DType::F32));
        px(&i, "nm", 6.281833616);
        let v = page(&*src, 0);
        assert_eq!(&v[..2], &[49154.60546875, 47970.40625]);
        assert!((v.iter().sum::<f64>() - 165050960.89013672).abs() < 1e-3);
    }
    if let Some(path) = sample("fei_emd_image.emd") {
        let src = open(&p.registry, &path);
        let i = src.info().clone();
        assert_eq!((i.width, i.height, i.dtype), (512, 512, DType::U16));
        assert_eq!(i.format, "Velox EMD (Velox 1.6.0)");
        assert_eq!(i.channels[0].name, "HAADF");
        px(&i, "nm", 5.302414838);
        assert_eq!(page(&*src, 0).iter().sum::<f64>(), 2431912854.0);
    }
    if let Some(path) = sample("fei_example_tem_stack.emd") {
        let src = open(&p.registry, &path);
        assert_eq!(
            page(&*src, 0),
            [0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0]
        );
        assert_eq!(page(&*src, 1).iter().sum::<f64>(), 81.0);
    }
    if let Some(path) = sample("HAADFscan.mrc") {
        let i = open(&p.registry, &path).info().clone();
        px(&i, "nm", 5.67913129);
        assert_eq!(
            item(&i, "Thermo Fisher MRC", "显微镜").as_deref(),
            Some("Spectra")
        );
        assert_eq!(
            item(&i, "Thermo Fisher MRC", "相机").as_deref(),
            Some("BM-Ceta")
        );
    }
    if let Some(path) = sample("FEI-Helios-Ebeam-16bits.tif") {
        let i = open(&p.registry, &path).info().clone();
        px(&i, "µm", 3.3724);
    }
    // EMPIAR-10568, Falcon 4: 721 frames of 4096 × 4096 in fractions of 13.
    if let Some(path) = sample("falcon4.eer") {
        let src = open(&p.registry, &path);
        let i = src.info().clone();
        assert_eq!((i.width, i.height, i.page_count), (4096, 4096, 56));
        assert_eq!(page(&*src, 0).iter().sum::<f64>(), 2564263.0);
        assert_eq!(page(&*src, 55).iter().sum::<f64>(), 1182077.0);
        let frames = formats::open_series_with(&path, 1, &p.registry).unwrap();
        assert_eq!(frames.info().page_count, 721);
        assert_eq!(page(&*frames, 0).iter().sum::<f64>(), 196609.0);
    }
}
