//! Gatan DigitalMicrograph (.dm3, .dm4) through the "gatan" WebAssembly
//! plugin, end to end: the plugin is built from `plugins/gatan`, installed
//! into a temporary plugin folder and used through `formats::open_with` /
//! `open_series_with`, as the app does.
//!
//! The synthetic files in `tests/fixtures/` come from
//! `plugins/gatan/tests/make_fixtures.py` (checked against ncempy); the real
//! samples from the OME sample repository are used when present in
//! /home/user/samples/gatan.

mod common;

use std::path::{Path, PathBuf};

use tifmrcview_lib::formats;
use tifmrcview_lib::image::{DType, ImageInfo, PlaneData, Source, SpectrumInfo};
use tifmrcview_lib::plugins::registry::Registry;

macro_rules! gatan {
    () => {
        match common::install(&["gatan"]) {
            Some(p) => p,
            None => return,
        }
    };
}

fn fixture(name: &str) -> PathBuf {
    common::repo().join("src-tauri/tests/fixtures").join(name)
}

/// The pixel values of the fixtures, as `pattern()` in make_fixtures.py.
fn pattern(z: u64, y: u64, x: u64, modulo: u64) -> f64 {
    ((x * 7 + y * 13 + z * 101) % modulo) as f64
}

fn values(p: &PlaneData) -> Vec<f64> {
    (0..p.len()).map(|i| p.get_f64(i)).collect()
}

fn open(reg: &Registry, path: &Path) -> Box<dyn Source> {
    formats::open_with(path, reg).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()))
}

fn item(i: &ImageInfo, group: &str, key: &str) -> Option<String> {
    let g = i.metadata.iter().find(|g| g.name == group)?;
    g.items.iter().find(|it| it[0] == key).map(|it| it[1].clone())
}

fn grid(w: u64, h: u64, f: impl Fn(u64, u64) -> f64) -> Vec<f64> {
    (0..h).flat_map(|y| (0..w).map(move |x| (y, x))).map(|(y, x)| f(y, x)).collect()
}

#[test]
fn dm3_u16_preview_skipped_calibration_and_tags() {
    let p = gatan!();
    let src = open(&p.registry, &fixture("gatan_u16.dm3"));
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.size_c, i.size_z, i.size_t, i.page_count), (19, 12, 1, 1, 1, 1));
    assert_eq!(i.dtype, DType::U16);
    assert_eq!(i.format, "Gatan DM3 (GMS 3.50.3584.0)");
    assert_eq!((i.unit.as_str(), i.pixel_size), ("nm", Some([0.25, 0.25, 0.0])));
    assert!(i.series.is_empty(), "the preview is not a series");
    assert_eq!(i.channels[0].name, "u16 image");
    let groups: Vec<&str> = i.metadata.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(groups, ["Gatan 图像", "Gatan 显微镜", "Gatan 采集", "Gatan 全部参数"]);
    let img = |k| item(&i, "Gatan 图像", k);
    assert_eq!(img("文件格式").as_deref(), Some("DigitalMicrograph DM3"));
    assert_eq!(img("尺寸").as_deref(), Some("19 × 12"));
    assert_eq!(img("像素类型").as_deref(), Some("16 位无符号整数（DataType 10）"));
    assert_eq!(img("X 标定").as_deref(), Some("0.25 nm/像素（原点 1.5）"));
    assert_eq!(img("Y 标定").as_deref(), Some("0.25 nm/像素"));
    assert_eq!(img("采集日期").as_deref(), Some("2024/5/17"));
    assert_eq!(img("采集时间").as_deref(), Some("14:03:22"));
    assert_eq!(img("强度标定"), None);
    let mic = |k| item(&i, "Gatan 显微镜", k);
    assert_eq!(mic("显微镜").as_deref(), Some("JEOL JEM-2100F"));
    assert_eq!(mic("加速电压").as_deref(), Some("200 kV"));
    assert_eq!(mic("标称放大倍数").as_deref(), Some("50000×"));
    assert_eq!(mic("实际放大倍数").as_deref(), Some("61234.5×"));
    assert_eq!(mic("球差 Cs").as_deref(), Some("2.7 mm"));
    assert_eq!(mic("样品台 X").as_deref(), Some("101.25"));
    assert_eq!(mic("样品台 α").as_deref(), Some("-12.5"));
    let acq = |k| item(&i, "Gatan 采集", k);
    assert_eq!(acq("相机").as_deref(), Some("OneView"));
    assert_eq!(acq("曝光时间").as_deref(), Some("0.5 s"));
    assert_eq!(acq("像素合并").as_deref(), Some("2, 2"));
    assert_eq!(acq("处理方式").as_deref(), Some("Gain normalized"));
    let all = |k| item(&i, "Gatan 全部参数", k);
    assert_eq!(all("Big table").as_deref(), Some("（5000 个值）"));
    assert_eq!(all("Unnamed/[1]").as_deref(), Some("8"));
    assert_eq!(all("Acquisition/Device/Active Size (pixels)").as_deref(), Some("4096, 4096"));
    let page = src.read_page(0).unwrap();
    assert_eq!(values(&page[0].data), grid(19, 12, |y, x| pattern(0, y, x, 65536)));
}

#[test]
fn dm4_f32_stack() {
    let p = gatan!();
    let src = open(&p.registry, &fixture("gatan_stack.dm4"));
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.size_z, i.page_count), (7, 5, 3, 3));
    assert_eq!(i.dtype, DType::F32);
    assert_eq!(i.format, "Gatan DM4 (GMS 3.60.4441.0)");
    assert_eq!((i.unit.as_str(), i.pixel_size), ("µm", Some([0.125, 0.125, 0.25])));
    assert_eq!(item(&i, "Gatan 图像", "Z 标定").as_deref(), Some("250 nm/像素"));
    for z in 0..3 {
        let (page, s) = i.locate(0, z, 0);
        let planes = src.read_page(page).unwrap();
        assert_eq!(values(&planes[s].data), grid(7, 5, |y, x| pattern(z as u64, y, x, 1000) / 4.0), "z{z}");
    }
}

#[test]
fn dm3_complex_as_two_channels_in_reciprocal_units() {
    let p = gatan!();
    let src = open(&p.registry, &fixture("gatan_complex.dm3"));
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.size_c, i.samples_per_page), (4, 3, 2, 2));
    assert_eq!(i.dtype, DType::F32);
    assert_eq!((i.unit.as_str(), i.pixel_size), ("1/nm", Some([0.5, 0.5, 0.0])));
    assert_eq!(i.channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["实部", "虚部"]);
    assert_eq!(item(&i, "Gatan 图像", "X 标定").as_deref(), Some("0.5 1/nm/像素"));
    let planes = src.read_page(0).unwrap();
    let re = grid(4, 3, |y, x| pattern(0, y, x, 100));
    assert_eq!(values(&planes[0].data), re);
    assert_eq!(values(&planes[1].data), re.iter().map(|v| v * 2.0 + 1.0).collect::<Vec<_>>());
}

#[test]
fn dm4_rgba() {
    let p = gatan!();
    let src = open(&p.registry, &fixture("gatan_rgb.dm4"));
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.size_c, i.samples_per_page, i.is_rgb), (5, 4, 3, 3, true));
    assert_eq!(i.dtype, DType::U8);
    assert_eq!((i.unit.as_str(), i.pixel_size), ("mm", Some([2.0, 2.0, 0.0])));
    let planes = src.read_page(0).unwrap();
    assert_eq!(values(&planes[0].data), grid(5, 4, |y, x| (x + y) as f64), "red");
    assert_eq!(values(&planes[1].data), grid(5, 4, |y, _| (y * 20) as f64), "green");
    assert_eq!(values(&planes[2].data), grid(5, 4, |_, x| (x * 10) as f64), "blue");
}

#[test]
fn dm3_big_endian_two_images_are_series() {
    let p = gatan!();
    let path = fixture("gatan_be.dm3");
    let src = open(&p.registry, &path);
    let i = src.info().clone();
    assert_eq!(
        i.series.iter().map(|s| (s.name.as_str(), s.detail.as_str())).collect::<Vec<_>>(),
        [("first", "6×4 · i16"), ("second", "3×2 · i16")]
    );
    assert_eq!(i.series_index, 0);
    assert_eq!((i.width, i.height, i.dtype), (6, 4, DType::I16));
    // pm become Å.
    assert_eq!((i.unit.as_str(), i.pixel_size), ("Å", Some([0.03, 0.03, 0.0])));
    assert_eq!(item(&i, "Gatan 图像", "字节序").as_deref(), Some("大端"));
    assert_eq!(item(&i, "Gatan 显微镜", "加速电压").as_deref(), Some("300 kV"));
    let planes = src.read_page(0).unwrap();
    assert_eq!(values(&planes[0].data), grid(6, 4, |y, x| pattern(0, y, x, 300) - 150.0));

    let src = formats::open_series_with(&path, 1, &p.registry).unwrap();
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.series_index), (3, 2, 1));
    assert_eq!(i.pixel_size, None);
    let planes = src.read_page(0).unwrap();
    assert_eq!(values(&planes[0].data), grid(3, 2, |y, x| -pattern(0, y, x, 50)));
    assert!(formats::open_series_with(&path, 2, &p.registry).is_err());
}

#[test]
fn broken_files_give_errors() {
    let p = gatan!();
    let tmp = tempfile::tempdir().unwrap();
    let good = std::fs::read(fixture("gatan_u16.dm3")).unwrap();
    let err = |name: &str, bytes: &[u8]| -> String {
        let path = tmp.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        match formats::open_with(&path, &p.registry) {
            Ok(_) => panic!("{name} opened"),
            Err(e) => format!("{e:#}"),
        }
    };
    assert!(err("empty.dm3", b"").contains("不是 DigitalMicrograph"));
    assert!(err("v5.dm4", &[0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]).contains("版本号 5"));
    // An unsupported real image is reported, not replaced by the preview.
    let packed = std::fs::read(fixture("gatan_packed.dm4")).unwrap();
    let e = err("packed.dm4", &packed);
    assert!(e.contains("FFT 压缩复数") && e.contains("DataType 27"), "{e}");
    let cut = err("cut.dm3", &good[..good.len() / 2]);
    assert!(cut.contains("文件不完整"), "{cut}");
    // Every truncation either opens or fails cleanly.
    for n in (0..good.len()).step_by(97) {
        let path = tmp.path().join("t.dm3");
        std::fs::write(&path, &good[..n]).unwrap();
        let _ = formats::open_with(&path, &p.registry).map(|s| drop(s.read_page(0)));
    }
}

/// Real DM4 files of the OME sample set (images/Gatan/imagesc-36590, CC BY 4.0;
/// not in the repository): sizes, metadata and sums from ncempy.
#[test]
fn samples_ome_dm4() {
    let dir = Path::new("/home/user/samples/gatan");
    let small = dir.join("SmallMontage0000.dm4");
    if !small.exists() {
        eprintln!("SKIPPED: {} is not there", small.display());
        return;
    }
    let p = gatan!();
    let src = open(&p.registry, &small);
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.page_count, i.dtype), (1024, 1024, 1, DType::F32));
    assert!(i.series.is_empty());
    assert_eq!(i.unit, "µm");
    assert!((i.pixel_size.unwrap()[0] - 0.02170467).abs() < 1e-7, "{:?}", i.pixel_size);
    assert_eq!(item(&i, "Gatan 显微镜", "加速电压").as_deref(), Some("120 kV"));
    assert_eq!(item(&i, "Gatan 显微镜", "显微镜").as_deref(), Some("FEI Tecnai Remote TCPIP"));
    assert_eq!(item(&i, "Gatan 显微镜", "标称放大倍数").as_deref(), Some("1900×"));
    assert_eq!(item(&i, "Gatan 显微镜", "球差 Cs"), None);
    assert_eq!(item(&i, "Gatan 采集", "相机").as_deref(), Some("OneView"));
    assert_eq!(item(&i, "Gatan 采集", "曝光时间").as_deref(), Some("2 s"));
    assert_eq!(item(&i, "Gatan 图像", "采集日期").as_deref(), Some("5/11/2020"));
    let v = values(&src.read_page(0).unwrap()[0].data);
    assert_eq!(&v[..4], &[342.85675048828125, 172.43435668945312, -523.5900268554688, -55.61932373046875]);
    assert!((v.iter().sum::<f64>() - 3058841.71673131).abs() < 1e-3);

    let big = dir.join("191041a_ctl_M2_3VBSED_0045.dm4");
    if !big.exists() {
        return;
    }
    let src = open(&p.registry, &big);
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.dtype), (10000, 10000, DType::U16));
    assert_eq!(item(&i, "Gatan 显微镜", "加速电压").as_deref(), Some("3 kV"));
    assert_eq!(item(&i, "Gatan 显微镜", "工作距离").as_deref(), Some("6.0239196 mm"));
    assert_eq!(item(&i, "Gatan 采集", "信号").as_deref(), Some("3VBSED"));
    let v = values(&src.read_page(0).unwrap()[0].data);
    assert_eq!(&v[..4], &[36497.0, 36314.0, 36997.0, 37726.0]);
    assert_eq!(v.iter().sum::<f64>(), 2155673292753.0);
}

/// The DigitalMicrograph test files of rosettasciio (GPL test data, not in the
/// repository; e.g. from sources.debian.org python-rosettasciio
/// rsciio/tests/data/digitalmicrograph): every file opens, and pixel sums
/// were checked against ncempy and rosettasciio.
#[test]
fn samples_rosettasciio() {
    let dir = Path::new("/home/user/samples/dm3");
    if !dir.join("2D/test_STEM_image.dm3").exists() {
        eprintln!("SKIPPED: {} is not there", dir.display());
        return;
    }
    let p = gatan!();
    let mut n = 0;
    for sub in ["", "1D", "2D", "3D"] {
        for e in std::fs::read_dir(dir.join(sub)).unwrap() {
            let path = e.unwrap().path();
            if !path.is_file() {
                continue;
            }
            n += 1;
            let r = formats::open_with(&path, &p.registry);
            if path.ends_with("test_fft_packed_complex8.dm4") {
                let e = format!("{:#}", r.err().expect("packed complex opened"));
                assert!(e.contains("DataType 27"), "{e}");
                continue;
            }
            let src = r.unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
            let i = src.info().clone();
            for k in 0..i.page_count {
                src.read_page(k).unwrap_or_else(|e| panic!("{} page {k}: {e:#}", path.display()));
            }
        }
    }
    assert!(n > 100, "{n} files");
    let open1 = |name: &str| open(&p.registry, &dir.join(name));
    let i = open1("2D/test_STEM_image.dm3").info().clone();
    assert_eq!((i.width, i.dtype, i.unit.as_str()), (68, DType::U32, "nm"));
    assert_eq!(item(&i, "Gatan 图像", "GMS 版本").as_deref(), Some("2.31.734.0"));
    let i = open1("2D/test_diffraction_pattern.dm3").info().clone();
    assert_eq!(i.unit, "1/nm");
    assert!(item(&i, "Gatan 图像", "X 标定").unwrap().contains("1/nm"));
    let i = open1("3D/EELS_SI.dm4").info().clone();
    assert_eq!((i.width, i.height, i.size_z), (2, 2, 2048));
    let sp = |axis: &str, start: f64, step: f64, unit: &str, kind: &str| {
        Some(SpectrumInfo { axis: axis.into(), start, step, unit: unit.into(), kind: kind.into() })
    };
    // Spectrum axes as rosettasciio reads them.
    assert_eq!(i.spectrum, sp("z", 300.0, 1.0, "eV", "EELS"));
    assert_eq!(i.unit, "µm");
    let i = open1("1D/test-EELS_spectrum.dm3").info().clone();
    assert_eq!((i.width, i.height, i.spectrum), (2048, 1, sp("x", -100.0, 0.5, "eV", "EELS")));
    let i = open1("1D/test-EDS_spectrum.dm3").info().clone();
    assert_eq!(i.spectrum, sp("x", -0.478, 0.005, "keV", "EDS"));
    let i = open1("2D/test-MonoCL_spectrum-SI.dm4").info().clone();
    assert_eq!((i.width, i.height), (1336, 67));
    let s = i.spectrum.unwrap();
    assert_eq!((s.axis.as_str(), s.unit.as_str(), s.kind.as_str()), ("x", "nm", "CL"));
    assert!((s.start - 811.0256562658906).abs() < 1e-3 && (s.step - 0.0934361144900322).abs() < 1e-6, "{s:?}");
    assert_eq!(i.pixel_size, None, "the wavelength axis is not a pixel size");
    let i = open1("1D/test-1.dm3").info().clone();
    assert_eq!(i.spectrum, sp("x", 0.0, 1.0, "", ""));
    assert_eq!(open1("2D/test_STEM_image.dm3").info().spectrum, None);
    let i = open1("2D/multi_signal.dm3").info().clone();
    assert_eq!(i.series.len(), 2);
    assert_eq!(values(&open1("2D/test-23.dm4").read_page(0).unwrap()[0].data), [1.0, 2.0, 3.0, 4.0]);
}
