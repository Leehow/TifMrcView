//! Imaris .ims files through the "imaris" WebAssembly plugin, end to end:
//! the plugin is built from `plugins/imaris`, installed into a temporary
//! plugin folder and used through `formats::open_with`, as the app does.
//!
//! The synthetic files in `tests/fixtures/` come from
//! `plugins/imaris/tests/make_fixtures.py` (h5py); the real samples from
//! the OME sample repository are used when present in /home/user/samples.

mod common;

use std::path::Path;

use common::imaris::{self, fixture, pattern};
use tifmrcview_lib::formats;
use tifmrcview_lib::image::{DType, PlaneData, Source};
use tifmrcview_lib::plugins::registry::Registry;

macro_rules! plugin {
    () => {
        match imaris::install() {
            Some(p) => p,
            None => return,
        }
    };
}

fn values(p: &PlaneData) -> Vec<f64> {
    (0..p.len()).map(|i| p.get_f64(i)).collect()
}

/// Position-weighted sum, the same as the h5py oracle computes.
fn checksum(p: &PlaneData) -> u64 {
    (0..p.len()).map(|i| p.get_f64(i) as u64 * (i as u64 % 251 + 1)).sum()
}

fn open(reg: &Registry, path: &Path) -> Box<dyn Source> {
    formats::open_with(path, reg).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()))
}

fn err(reg: &Registry, path: &Path) -> String {
    match formats::open_with(path, reg) {
        Ok(_) => panic!("{} opened", path.display()),
        Err(e) => format!("{e:#}"),
    }
}

fn item(i: &tifmrcview_lib::image::ImageInfo, group: &str, key: &str) -> Option<String> {
    let g = i.metadata.iter().find(|g| g.name == group)?;
    g.items.iter().find(|it| it[0] == key).map(|it| it[1].clone())
}

/// Every page against the pattern, cropped to the real size.
fn check_pixels(src: &dyn Source, modulo: u64) {
    let i = src.info().clone();
    for t in 0..i.size_t {
        for z in 0..i.size_z {
            for c in 0..i.size_c {
                let (page, s) = i.locate(c, z, t);
                let planes = src.read_page(page).unwrap();
                let want: Vec<f64> = (0..i.height as u64)
                    .flat_map(|y| (0..i.width as u64).map(move |x| pattern(c as u64, t as u64, z as u64, y, x, modulo) as f64))
                    .collect();
                assert_eq!(values(&planes[s].data), want, "c{c} z{z} t{t}");
            }
        }
    }
}

#[test]
fn old_style_hdf5_gzip_chunks_metadata_and_pixels() {
    let p = plugin!();
    let src = open(&p.registry, &fixture("imaris_v0.ims"));
    let i = src.info().clone();
    // 19×12×5 stored as 32×16×8 with 4×8×16 chunks: padding is cut off.
    assert_eq!((i.width, i.height, i.size_c, i.size_z, i.size_t, i.page_count), (19, 12, 2, 5, 2, 20));
    assert_eq!(i.dtype, DType::U16);
    assert_eq!(i.format, "Imaris IMS (Imaris 10.2)");
    assert_eq!(i.unit, "µm");
    assert_eq!(i.pixel_size, Some([0.25, 0.25, 1.5]));
    assert_eq!(i.channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["EGFP", "mCherry"]);
    assert_eq!((i.channels[0].color, i.channels[1].color), ([0, 255, 0], [255, 0, 0]));
    let groups: Vec<&str> = i.metadata.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(groups, ["Imaris 图像", "通道 1：EGFP", "通道 2：mCherry", "Imaris 全部参数"]);
    let img = |k: &str| item(&i, "Imaris 图像", k);
    assert_eq!(img("文件格式").as_deref(), Some("Imaris 5.5.0"));
    assert_eq!(img("采集时间").as_deref(), Some("2026-10-26 09:30:00.000"));
    assert_eq!(img("尺寸").as_deref(), Some("X=19  Y=12  Z=5  C=2  T=2"));
    assert_eq!(img("压缩").as_deref(), Some("gzip（级别 2）"));
    assert_eq!(img("分块 (Z×Y×X)").as_deref(), Some("4×8×16"));
    assert_eq!(img("分辨率层级").as_deref(), Some("2（显示最高分辨率）"));
    assert_eq!(img("Z 步长").as_deref(), Some("1.5 µm"));
    assert_eq!(img("时间间隔（平均）").as_deref(), Some("2.5 s"));
    assert_eq!(item(&i, "通道 1：EGFP", "激发波长").as_deref(), Some("488 nm"));
    assert_eq!(item(&i, "通道 2：mCherry", "颜色 (RGB)").as_deref(), Some("1.000 0.000 0.000"));
    assert_eq!(item(&i, "Imaris 全部参数", "Image/ExtMax0").as_deref(), Some("3.25"));
    assert_eq!(item(&i, "Imaris 全部参数", "NumberOfDataSets").as_deref(), Some("1"));
    // Gzip pages take one Z slice out of each 4-deep chunk; channel 1 at
    // t=1 is stored uncompressed.
    check_pixels(src.as_ref(), 65536);
}

#[test]
fn new_style_hdf5_dense_groups_fixed_array_and_single_chunk() {
    let p = plugin!();
    let src = open(&p.registry, &fixture("imaris_v110.ims"));
    let i = src.info().clone();
    assert_eq!((i.width, i.height, i.size_c, i.size_z, i.size_t), (14, 12, 2, 3, 1));
    assert_eq!(i.dtype, DType::U8);
    assert_eq!(i.pixel_size, Some([0.25, 0.25, 1.5]));
    // A huge attribute (stored outside the heap) is shown, shortened.
    let settings = item(&i, "Imaris 全部参数", "CustomData/Settings").unwrap();
    assert!(settings.starts_with("{xxx") && settings.ends_with('…') && settings.len() < 4100, "{}", settings.len());
    assert_eq!(item(&i, "Imaris 全部参数", "Vendor 8/Index").as_deref(), Some("8"));
    check_pixels(src.as_ref(), 256);
}

#[test]
fn broken_and_unsupported_files() {
    let p = plugin!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.ims");

    let mut ims3 = b"II*\0\x86\xc4\x0c\0ImarisDataSet\0\0\0".to_vec();
    ims3.resize(256, 0);
    std::fs::write(&path, &ims3).unwrap();
    assert!(err(&p.registry, &path).contains("IMS3"));
    std::fs::write(&path, b"not an hdf5 file at all").unwrap();
    assert!(err(&p.registry, &path).contains("不是 Imaris 文件"));

    // Truncated files are errors, not crashes.
    let full = std::fs::read(fixture("imaris_v0.ims")).unwrap();
    for cut in [8, 100, 1000, 5000, full.len() / 2, full.len() - 100] {
        std::fs::write(&path, &full[..cut]).unwrap();
        if let Ok(s) = formats::open_with(&path, &p.registry) {
            for page in 0..s.info().page_count {
                let _ = s.read_page(page);
            }
        }
    }
    // Garbage in the middle of the file.
    let mut bad = full.clone();
    for b in &mut bad[2000..6000] {
        *b = 0xa5;
    }
    std::fs::write(&path, &bad).unwrap();
    if let Ok(s) = formats::open_with(&path, &p.registry) {
        let _ = s.read_page(0);
    }
}

#[test]
fn needs_the_plugin() {
    let none = Registry::new(Some(tempfile::tempdir().unwrap().path().join("none")), imaris::app_version());
    // Says "install the Imaris plugin" once `ims` is in the registry's KNOWN list.
    let e = err(&none, &fixture("imaris_v0.ims"));
    assert!(e.contains("不支持的文件类型") || e.contains("「Imaris」"), "{e}");
    let p = plugin!();
    assert_eq!(p.registry.extensions(), ["ims"]);
}

/// Real files from the OME sample repository (skipped when absent), against
/// checksums computed with h5py.
#[test]
fn real_samples_match_h5py() {
    let p = plugin!();
    let dir = Path::new("/home/user/samples/ims");
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, (usize, usize, usize, usize, usize), DType, &[(usize, usize, usize, u64)])] = &[
        (
            "Convallaria_3C_1T_confocal.ims",
            (1024, 1024, 3, 1, 1),
            DType::U16,
            &[(0, 0, 0, 247462866077), (1, 0, 0, 1024720187633), (2, 0, 0, 236154393461)],
        ),
        (
            "Convallaria_3C_10T_confocal.ims",
            (1024, 1024, 3, 1, 10),
            DType::U16,
            &[(0, 0, 0, 242607045609), (1, 0, 5, 1035275465302), (2, 0, 9, 240213743003)],
        ),
        (
            "Convallaria_3C_1T_2x2grid_confocal.ims",
            (1949, 1949, 3, 1, 1),
            DType::U16,
            &[(0, 0, 0, 608960300782), (1, 0, 0, 2285241743298), (2, 0, 0, 808462496380)],
        ),
        ("TransFactors_2C_SRRF.ims", (4096, 4096, 2, 1, 1), DType::U16, &[(0, 0, 0, 92836262225), (1, 0, 0, 93013473301)]),
        (
            "performanceTestImage.ims",
            (1024, 1024, 2, 98, 1),
            DType::U8,
            &[(0, 0, 0, 2066047531), (1, 49, 0, 1763280769), (1, 97, 0, 1576229271)],
        ),
        ("retina_large.ims", (2048, 1567, 2, 64, 1), DType::U8, &[(1, 32, 0, 4051665040), (1, 63, 0, 54299227)]),
        (
            "2016-07-26_17.11.03_Sample_Prefix_5P298C2_Protocol.ims",
            (1024, 1024, 1, 47, 1),
            DType::U16,
            &[(0, 0, 0, 14489365484), (0, 23, 0, 14710767233), (0, 46, 0, 14513402097)],
        ),
    ];
    for (name, dims, dtype, planes) in cases {
        let path = dir.join(name);
        if !path.exists() {
            eprintln!("SKIPPED: {} not found", path.display());
            continue;
        }
        let t = std::time::Instant::now();
        let src = open(&p.registry, &path);
        let i = src.info().clone();
        eprintln!("{name}: {} opened in {:?}", i.format, t.elapsed());
        assert_eq!((i.width, i.height, i.size_c, i.size_z, i.size_t), *dims, "{name}");
        assert_eq!(i.dtype, *dtype, "{name}");
        assert_eq!(i.unit, "µm", "{name}");
        assert!(i.pixel_size.unwrap()[0] > 0.0, "{name}");
        assert!(i.metadata.iter().any(|g| g.name == "Imaris 全部参数"), "{name}");
        for &(c, z, t, want) in *planes {
            let (page, s) = i.locate(c, z, t);
            assert_eq!(checksum(&src.read_page(page).unwrap()[s].data), want, "{name} c{c} z{z} t{t}");
        }
    }

    // Imaris 10 LZ4 / shuffle and the old IMS3 format are refused with a reason.
    for (name, msg) in [
        ("croppedRetinaLz4.ims", "LZ4"),
        ("croppedCDM3DLz4.ims", "shuffle"),
        ("Convallaria_3C_1T_confocal_IMS3.ims", "IMS3"),
    ] {
        let path = dir.join(name);
        if path.exists() {
            let e = err(&p.registry, &path);
            assert!(e.contains(msg), "{name}: {e}");
        }
    }
}
