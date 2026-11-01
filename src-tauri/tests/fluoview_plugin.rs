//! Olympus FluoView OIB / OIR through the "fluoview" WebAssembly plugin,
//! end to end: the plugin is built from `plugins/fluoview`, installed into a
//! temporary plugin folder and used through `formats::open_with`, exactly
//! as the app does. Synthetic files come from `common::fluoview`; the tests
//! against real samples in /home/user/samples skip quietly when the files
//! are absent.

mod common;

use std::path::Path;

use common::fluoview::{self as fv, OibSpec, OirSpec};
use tifmrcview_lib::formats;
use tifmrcview_lib::image::{ImageInfo, PlaneData, Source};
use tifmrcview_lib::plugins::registry::Registry;

macro_rules! plugin {
    () => {
        match common::install(&["fluoview"]) {
            Some(p) => p,
            None => return,
        }
    };
}

fn values(p: &PlaneData) -> Vec<f64> {
    (0..p.len()).map(|i| p.get_f64(i)).collect()
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

fn item(i: &ImageInfo, group: &str, key: &str) -> Option<String> {
    let g = i.metadata.iter().find(|g| g.name == group)?;
    g.items
        .iter()
        .find(|it| it[0] == key)
        .map(|it| it[1].clone())
}

fn groups(i: &ImageInfo) -> Vec<&str> {
    i.metadata.iter().map(|g| g.name.as_str()).collect()
}

/// Checks every page against the synthetic pattern.
fn check_pattern(
    src: &dyn Source,
    nc: usize,
    nz: usize,
    nt: usize,
    missing: &[(usize, usize, usize)],
) {
    let i = src.info();
    let (w, h) = (i.width, i.height);
    for t in 0..nt {
        for z in 0..nz {
            for c in 0..nc {
                let (page, s) = i.locate(c, z, t);
                let got = values(&src.read_page(page).unwrap()[s].data);
                let want: Vec<f64> = if missing.contains(&(c, z, t)) {
                    vec![0.0; w * h]
                } else {
                    fv::plane(c, z, t, w, h)
                        .into_iter()
                        .map(f64::from)
                        .collect()
                };
                assert_eq!(got, want, "c{c} z{z} t{t}");
            }
        }
    }
}

// ---------------------------------------------------------------- OIB

#[test]
fn oib_stack_info_metadata_and_pixels() {
    let p = plugin!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("stack.oib");
    let spec = OibSpec::default();
    fv::write_oib(&path, &spec);
    let src = open(&p.registry, &path);
    let i = src.info();
    assert_eq!(
        (
            i.width,
            i.height,
            i.size_c,
            i.size_z,
            i.size_t,
            i.page_count
        ),
        (64, 48, 2, 3, 1, 6)
    );
    assert_eq!(i.dtype, tifmrcview_lib::image::DType::U16);
    assert_eq!(i.format, "Olympus FluoView OIB (FV1000 4.2.4.5)");
    assert_eq!(i.unit, "µm");
    let ps = i.pixel_size.unwrap();
    // "0,125.0" is 0.125 written with a decimal comma; Z: 150 nm steps.
    assert!(
        (ps[0] - 0.125).abs() < 1e-9 && (ps[1] - 0.125).abs() < 1e-9 && (ps[2] - 0.15).abs() < 1e-9,
        "{ps:?}"
    );
    let names: Vec<&str> = i.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["DAPI", "Alexa Fluor 594"]);
    assert_eq!(i.channels[0].color, [0, 0, 255]);
    assert_eq!(i.channels[1].color, [255, 0, 0]);
    assert_eq!(
        groups(i),
        [
            "FluoView 图像",
            "FluoView 显微镜",
            "通道 1：DAPI",
            "通道 2：Alexa Fluor 594",
            "FluoView 全部参数"
        ]
    );
    assert_eq!(
        item(i, "FluoView 图像", "采集时间").as_deref(),
        Some("2022-08-24 14:31:02")
    );
    assert_eq!(
        item(i, "FluoView 图像", "像素尺寸 X").as_deref(),
        Some("0.1250 µm")
    );
    assert_eq!(
        item(i, "FluoView 显微镜", "物镜").as_deref(),
        Some("UPLSAPO 60X O NA:1.35")
    );
    assert_eq!(
        item(i, "FluoView 显微镜", "数值孔径 NA").as_deref(),
        Some("1.35")
    );
    assert_eq!(
        item(i, "FluoView 显微镜", "像素驻留 (µs)").as_deref(),
        Some("8")
    );
    assert_eq!(
        item(i, "FluoView 显微镜", "平均（Kalman / 行）").as_deref(),
        Some("Line Kalman ×2")
    );
    assert_eq!(item(i, "FluoView 显微镜", "扫描缩放").as_deref(), Some("2"));
    assert_eq!(
        item(i, "通道 1：DAPI", "激光").as_deref(),
        Some("405 nm, 5.3%")
    );
    assert_eq!(
        item(i, "通道 2：Alexa Fluor 594", "激光").as_deref(),
        Some("543 nm, 12.5%")
    );
    assert_eq!(
        item(i, "通道 2：Alexa Fluor 594", "探测器电压 HV").as_deref(),
        Some("509")
    );
    assert_eq!(
        item(i, "通道 2：Alexa Fluor 594", "偏移 (%)").as_deref(),
        Some("3")
    );
    assert_eq!(item(i, "通道 1：DAPI", "针孔 (µm)").as_deref(), Some("110"));
    assert_eq!(
        item(i, "FluoView 全部参数", "Version Info/SystemName").as_deref(),
        Some("FLUOVIEW FV1000")
    );
    check_pattern(src.as_ref(), 2, 3, 1, &[]);
    // Strips stored back to back become one chunk the host reads in place.
    let (raw, little) = src.raw_page(0).expect("contiguous plane is read in place");
    assert!(little);
    let want: Vec<u8> = fv::plane(0, 0, 0, 64, 48)
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert_eq!(raw, &want[..]);
}

#[test]
fn oib_scattered_sectors_big_endian_time_and_missing_planes() {
    let p = plugin!();
    let tmp = tempfile::tempdir().unwrap();
    // Sectors of the plane streams alternate, so strips and rows cross
    // non-contiguous sectors; big-endian TIFFs; a time series with a
    // missing plane.
    let spec = OibSpec {
        w: 50,
        h: 57,
        nz: 2,
        nt: 3,
        rps: 5,
        big_endian: true,
        interleave: true,
        missing: vec![(1, 1, 2)],
        ..OibSpec::default()
    };
    let path = tmp.path().join("scattered.oib");
    fv::write_oib(&path, &spec);
    let src = open(&p.registry, &path);
    let i = src.info();
    assert_eq!(
        (i.width, i.height, i.size_c, i.size_z, i.size_t),
        (50, 57, 2, 2, 3)
    );
    assert_eq!(
        item(i, "FluoView 图像", "时间间隔 (s)").as_deref(),
        Some("2.5")
    );
    check_pattern(src.as_ref(), 2, 2, 3, &[(1, 1, 2)]);
    assert!(src.raw_page(0).is_none());

    // Planes small enough for the mini stream (64-byte mini sectors).
    let spec = OibSpec {
        w: 20,
        h: 9,
        nz: 2,
        rps: 4,
        ..OibSpec::default()
    };
    let path = tmp.path().join("mini.oib");
    fv::write_oib(&path, &spec);
    let src = open(&p.registry, &path);
    check_pattern(src.as_ref(), 2, 2, 1, &[]);
}

#[test]
fn oir_stack_info_metadata_pixels_and_reference() {
    let p = plugin!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("stack.oir");
    let spec = OirSpec::default();
    fv::write_oir(&path, &spec);
    let src = open(&p.registry, &path);
    let i = src.info().clone();
    assert_eq!(
        (
            i.width,
            i.height,
            i.size_c,
            i.size_z,
            i.size_t,
            i.page_count
        ),
        (40, 30, 2, 3, 2, 12)
    );
    assert_eq!(i.format, "Olympus FluoView OIR (FV4000 3.1.1.67)");
    let ps = i.pixel_size.unwrap();
    assert!(
        (ps[0] - 0.25).abs() < 1e-9 && (ps[2] - 1.5).abs() < 1e-9,
        "{ps:?}"
    );
    // Channels follow their `order`, not the order the XML lists them in.
    let names: Vec<&str> = i.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["CH2", "CH3"]);
    assert_eq!(i.channels[0].color, [0, 255, 0]);
    assert_eq!(i.channels[1].color, [255, 0, 0]);
    assert_eq!(
        groups(&i),
        [
            "FluoView 图像",
            "FluoView 显微镜",
            "通道 1：CH2 (FITC)",
            "通道 2：CH3 (mCherry)",
            "FluoView 全部参数"
        ]
    );
    assert_eq!(
        item(&i, "FluoView 图像", "采集时间").as_deref(),
        Some("2024-12-02 09:52:10 +08:00")
    );
    assert_eq!(
        item(&i, "FluoView 图像", "时间间隔 (s)").as_deref(),
        Some("30")
    );
    assert_eq!(
        item(&i, "FluoView 显微镜", "物镜").as_deref(),
        Some("UPLXAPO10X")
    );
    assert_eq!(
        item(&i, "FluoView 显微镜", "数值孔径 NA").as_deref(),
        Some("0.4")
    );
    assert_eq!(
        item(&i, "FluoView 显微镜", "扫描器").as_deref(),
        Some("Galvano")
    );
    assert_eq!(
        item(&i, "FluoView 显微镜", "像素驻留 (µs)").as_deref(),
        Some("2")
    );
    assert_eq!(
        item(&i, "FluoView 显微镜", "平均（Kalman / 行）").as_deref(),
        Some("Line ×4")
    );
    assert_eq!(
        item(&i, "通道 1：CH2 (FITC)", "激光").as_deref(),
        Some("488 nm, 0.6%")
    );
    assert_eq!(
        item(&i, "通道 1：CH2 (FITC)", "检测范围 (nm)").as_deref(),
        Some("500–540")
    );
    assert_eq!(
        item(&i, "通道 2：CH3 (mCherry)", "探测器电压 HV").as_deref(),
        Some("601")
    );
    assert_eq!(
        item(&i, "通道 2：CH3 (mCherry)", "探测器").as_deref(),
        Some("BSD2")
    );
    check_pattern(src.as_ref(), 2, 3, 2, &[]);

    // The reference image is the second series.
    assert_eq!(i.series.len(), 2);
    assert_eq!(
        (i.series[0].detail.as_str(), i.series[1].name.as_str()),
        ("40×30 · 2C · 3Z · 2T", "参考图像")
    );
    let r = formats::open_series_with(&path, 1, &p.registry).unwrap();
    let ri = r.info();
    assert_eq!(
        (ri.size_c, ri.size_z, ri.size_t, ri.series_index),
        (2, 1, 1, 1)
    );
    for c in 0..2 {
        let want: Vec<f64> = fv::plane(c, 99, 0, 40, 30)
            .into_iter()
            .map(f64::from)
            .collect();
        assert_eq!(values(&r.read_page(c).unwrap()[0].data), want);
    }
    assert!(formats::open_series_with(&path, 2, &p.registry).is_err());
}

#[test]
fn oir_unaligned_strips_lambda_and_no_index() {
    let p = plugin!();
    let tmp = tempfile::tempdir().unwrap();
    // Strips of 700 bytes cut rows of 80 bytes; λ planes become channels;
    // without a block index the blocks are walked.
    let spec = OirSpec {
        nc: 2,
        nz: 2,
        nt: 1,
        nl: 3,
        strip: 700,
        no_index: true,
        reference: false,
        ..OirSpec::default()
    };
    let path = tmp.path().join("lambda.oir");
    fv::write_oir(&path, &spec);
    let src = open(&p.registry, &path);
    let i = src.info();
    assert_eq!((i.size_c, i.size_z, i.size_t), (6, 2, 1));
    assert!(i.series.is_empty());
    let names: Vec<&str> = i.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "CH2 420 nm",
            "CH3 420 nm",
            "CH2 470 nm",
            "CH3 470 nm",
            "CH2 520 nm",
            "CH3 520 nm"
        ]
    );
    check_pattern(src.as_ref(), 6, 2, 1, &[]);
}

#[test]
fn broken_and_foreign_files() {
    let p = plugin!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("junk.oir");
    std::fs::write(&path, b"not an Olympus file at all").unwrap();
    assert!(err(&p.registry, &path).contains("不是 Olympus FluoView 文件"));
    let path = tmp.path().join("junk.oib");
    std::fs::write(
        &path,
        [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1, 0, 0],
    )
    .unwrap();
    assert!(err(&p.registry, &path).contains("OIB"));

    // Truncated files are errors (or blank pages), never crashes.
    for (name, write) in [
        (
            "t.oib",
            Box::new(|p: &Path| fv::write_oib(p, &OibSpec::default())) as Box<dyn Fn(&Path)>,
        ),
        (
            "t.oir",
            Box::new(|p: &Path| fv::write_oir(p, &OirSpec::default())),
        ),
    ] {
        let path = tmp.path().join(name);
        write(&path);
        let full = std::fs::read(&path).unwrap();
        for cut in [
            20,
            100,
            600,
            full.len() / 3,
            full.len() / 2,
            full.len() - 100,
        ] {
            std::fs::write(&path, &full[..cut]).unwrap();
            if let Ok(s) = formats::open_with(&path, &p.registry) {
                for page in 0..s.info().page_count {
                    let _ = s.read_page(page);
                }
            }
        }
        // Flipped bytes, too.
        for k in (0..full.len()).step_by(full.len() / 40) {
            let mut b = full.clone();
            for x in &mut b[k..(k + 16).min(full.len())] {
                *x ^= 0x5a;
            }
            std::fs::write(&path, &b).unwrap();
            if let Ok(s) = formats::open_with(&path, &p.registry) {
                let _ = s.read_page(0);
            }
        }
    }
}

#[test]
fn needs_the_plugin() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.oir");
    fv::write_oir(&path, &OirSpec::default());
    let none = Registry::new(Some(tmp.path().join("no-plugins")), "0.4.0");
    assert!(formats::open_with(&path, &none).is_err());
    assert!(!formats::is_supported_with(&path, &none));
    let p = plugin!();
    assert!(formats::is_supported_with(&path, &p.registry));
    let mut ext = p.registry.extensions();
    ext.sort();
    assert_eq!(ext, ["oib", "oir"]);
}

// ---------------------------------------------------------------- real samples

const SAMPLES: &str = "/home/user/samples";

/// (sum, Σ v·(i mod 251 + 1)) of a plane, as computed with oiffile / oirfile.
fn sig(p: &PlaneData) -> (u64, u64) {
    let mut s = 0u64;
    let mut w = 0u64;
    for i in 0..p.len() {
        let v = p.get_f64(i) as u64;
        s += v;
        w = w.wrapping_add(v * ((i as u64 % 251) + 1));
    }
    (s, w)
}

fn sample(rel: &str) -> Option<std::path::PathBuf> {
    let p = Path::new(SAMPLES).join(rel);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("SKIPPED: {} not found", p.display());
        None
    }
}

/// Opens a sample, prints how long it took, and checks planes (c, z, t)
/// against checksums from the Python readers.
fn check_sample(
    reg: &Registry,
    path: &Path,
    dims: (usize, usize, usize, usize, usize),
    want: &[((usize, usize, usize), (u64, u64))],
) -> Box<dyn Source> {
    let t = std::time::Instant::now();
    let src = open(reg, path);
    let i = src.info();
    eprintln!(
        "{}: {} {}×{} C{} Z{} T{} opened in {:?}",
        path.display(),
        i.format,
        i.width,
        i.height,
        i.size_c,
        i.size_z,
        i.size_t,
        t.elapsed()
    );
    assert_eq!((i.width, i.height, i.size_c, i.size_z, i.size_t), dims);
    // Shown with --nocapture, to eyeball the info panel.
    for g in i.metadata.iter().filter(|g| g.name != "FluoView 全部参数") {
        eprintln!(
            "  [{}] {}",
            g.name,
            g.items
                .iter()
                .map(|it| format!("{}={}", it[0], it[1]))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    for &((c, z, t), s) in want {
        let (page, k) = i.locate(c, z, t);
        assert_eq!(
            sig(&src.read_page(page).unwrap()[k].data),
            s,
            "c{c} z{z} t{t}"
        );
    }
    src
}

#[test]
fn sample_oib_fv1000() {
    let Some(path) = sample("oib/20220824_4492_cord_dapi__iba568_60x.oib") else {
        return;
    };
    let p = plugin!();
    let src = check_sample(
        &p.registry,
        &path,
        (1024, 1024, 2, 6, 1),
        &[
            ((0, 0, 0), (258699873, 32613067089)),
            ((1, 3, 0), (279200409, 35190043713)),
            ((0, 5, 0), (246796922, 31073818775)),
        ],
    );
    let i = src.info();
    assert_eq!(i.format, "Olympus FluoView OIB (FV1000 4.2.4.5)");
    assert!(
        (i.pixel_size.unwrap()[0] - 0.207).abs() < 1e-9
            && (i.pixel_size.unwrap()[2] - 0.51).abs() < 1e-9
    );
    assert_eq!(
        item(i, "FluoView 显微镜", "物镜").as_deref(),
        Some("UPLSAPO 60X O NA:1.35")
    );
    assert_eq!(
        item(i, "通道 1：DAPI", "激光").as_deref(),
        Some("405 nm, 5.3%")
    );
    assert!(
        src.raw_page(0).is_some(),
        "contiguous plane streams are read in place"
    );
}

/// The checksums come from the OME-TIFF twins of these files (read with
/// tifffile; the app cannot open them itself, they are 16-bit palette TIFFs).
#[test]
fn sample_oir_matches_its_ome_tiff() {
    let p = plugin!();
    if let Some(path) = sample("oir/venus stack.oir") {
        let src = check_sample(
            &p.registry,
            &path,
            (512, 512, 1, 67, 1),
            &[
                ((0, 0, 0), (47010130, 5898030327)),
                ((0, 33, 0), (89382326, 11253013897)),
                ((0, 66, 0), (101658010, 12879353034)),
            ],
        );
        let i = src.info();
        assert_eq!(
            i.format,
            "Olympus FluoView OIR (FluoView Software 1.3.1.83)"
        );
        assert!((i.pixel_size.unwrap()[2] - 1.49).abs() < 1e-9);
        assert_eq!(
            item(i, "FluoView 显微镜", "数值孔径 NA").as_deref(),
            Some("1.05")
        );
    }
    if let Some(path) = sample("oir/coupe shg stack_0001.oir") {
        check_sample(
            &p.registry,
            &path,
            (512, 512, 1, 35, 1),
            &[
                ((0, 0, 0), (29611193, 3719685221)),
                ((0, 17, 0), (95978366, 12082921001)),
                ((0, 34, 0), (225661344, 28437723373)),
            ],
        );
    }
}

#[test]
fn sample_oir_fv4000_series_and_lambda() {
    let p = plugin!();
    if let Some(path) = sample("oir/1202-interval_30sec_sequence_frame_z stack.oir") {
        let src = check_sample(
            &p.registry,
            &path,
            (512, 512, 2, 8, 5),
            &[
                ((0, 0, 0), (52438629, 6642112635)),
                ((1, 7, 4), (74363263, 9351960174)),
                ((1, 3, 2), (143238147, 18141211959)),
            ],
        );
        let i = src.info();
        assert_eq!(i.format, "Olympus FluoView OIR (FV4000 3.1.1.67)");
        assert_eq!(
            i.channels.iter().map(|c| c.color).collect::<Vec<_>>(),
            [[0, 255, 0], [255, 0, 0]]
        );
        assert_eq!(
            item(i, "FluoView 显微镜", "物镜").as_deref(),
            Some("UPLXAPO10X")
        );
        assert_eq!(
            item(i, "FluoView 图像", "时间间隔 (s)").as_deref(),
            Some("30")
        );
        assert_eq!(i.series.len(), 2);
        let r = formats::open_series_with(&path, 1, &p.registry).unwrap();
        assert_eq!(
            sig(&r.read_page(0).unwrap()[0].data),
            (99428490, 12499837354)
        );
    }
    if let Some(path) = sample("oir/DAPI-mCherry_3T_4Z_5Lambda-420-630-w10nm-s50nm.oir") {
        // T3 λ5 Z4: λ becomes the channel axis.
        let src = check_sample(
            &p.registry,
            &path,
            (512, 512, 5, 4, 3),
            &[
                ((0, 0, 0), (55608921, 6918923257)),
                ((4, 3, 2), (30581620, 3850331915)),
                ((2, 1, 1), (280616613, 34909001472)),
            ],
        );
        assert_eq!(src.info().channels[1].name, "CH1 470 nm");
    }
    if let Some(path) = sample("oir/tiling-sample-brain-section_A01_G001_0001.oir") {
        check_sample(
            &p.registry,
            &path,
            (512, 512, 2, 53, 1),
            &[
                ((0, 0, 0), (8837351, 1112353771)),
                ((1, 52, 0), (340010, 43025090)),
            ],
        );
    }
    if let Some(path) = sample("oir/venus stack.oir") {
        let r = formats::open_series_with(&path, 1, &p.registry).unwrap();
        assert_eq!(
            sig(&r.read_page(0).unwrap()[0].data),
            (110639144, 13966665288)
        );
    }
}

/// Writes the synthetic files to `$FLUOVIEW_OUT`, to check them with other
/// readers (oiffile, oirfile):
/// `FLUOVIEW_OUT=dir cargo test --test fluoview_plugin -- --ignored write_synthetic`
#[test]
#[ignore]
fn write_synthetic_files() {
    let Some(dir) = std::env::var_os("FLUOVIEW_OUT") else {
        return;
    };
    let dir = Path::new(&dir);
    fv::write_oib(&dir.join("stack.oib"), &OibSpec::default());
    let scattered = OibSpec {
        w: 50,
        h: 57,
        nz: 2,
        nt: 3,
        rps: 5,
        big_endian: true,
        interleave: true,
        ..OibSpec::default()
    };
    fv::write_oib(&dir.join("scattered.oib"), &scattered);
    // Worst case for the recipe builder: 1024² planes in alternating 512-byte sectors.
    let big = OibSpec {
        w: 1024,
        h: 1024,
        nz: 1,
        rps: 1024,
        interleave: true,
        ..OibSpec::default()
    };
    fv::write_oib(&dir.join("scattered-1024.oib"), &big);
    fv::write_oir(&dir.join("stack.oir"), &OirSpec::default());
    fv::write_oir(
        &dir.join("lambda.oir"),
        &OirSpec {
            nl: 3,
            strip: 700,
            reference: false,
            ..OirSpec::default()
        },
    );
}
