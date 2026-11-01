//! Nikon ND2 through the "nis" WebAssembly plugin, end to end: the plugin is
//! built from `plugins/nis`, installed into a temporary plugin folder and
//! used through `formats::open_with` / `open_series_with`, as the app does.
//! Synthetic files cover the format; the real samples in
//! `/home/user/samples/nd2` are checked against the Python `nd2` package
//! when they are present.

mod common;

use std::path::Path;

use common::nis::{self, Spec};
use tifmrcview_lib::formats;
use tifmrcview_lib::image::{DType, ImageInfo, PlaneData, Source};
use tifmrcview_lib::plugins::registry::Registry;

/// The plugin needs app 0.5.0 (series); test as that version.
const APP: &str = "0.5.0";

struct Nis {
    _p: common::Plugins,
    reg: Registry,
}

macro_rules! nis {
    () => {
        match common::install(&["nis"]) {
            Some(p) => {
                let reg = Registry::new(Some(p.dir.path().to_path_buf()), APP);
                Nis { _p: p, reg }
            }
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

fn items(i: &ImageInfo, group: &str, key: &str) -> Vec<String> {
    let all = i
        .metadata
        .iter()
        .filter(|g| g.name == group)
        .flat_map(|g| g.items.iter());
    all.filter(|it| it[0] == key)
        .map(|it| it[1].clone())
        .collect()
}

/// Expected samples of channel `c` of frame `seq`.
fn want(spec: &Spec, seq: usize, c: usize) -> Vec<f64> {
    (0..spec.h)
        .flat_map(|y| (0..spec.w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let v = nis::value(seq, c, x, y);
            match spec.bits {
                8 => (v as u8) as f64,
                16 => (v as u16) as f64,
                _ => (v as f32 / 4.0) as f64,
            }
        })
        .collect()
}

/// Checks every page of `src` against frames numbered by `seq(z, t)`.
fn check_pixels(src: &dyn Source, spec: &Spec, seq: impl Fn(usize, usize) -> usize) {
    let i = src.info();
    for t in 0..i.size_t {
        for z in 0..i.size_z {
            for c in 0..i.size_c {
                let (page, s) = i.locate(c, z, t);
                let planes = src.read_page(page).unwrap();
                assert_eq!(planes.len(), i.size_c);
                assert_eq!(
                    values(&planes[s].data),
                    want(spec, seq(z, t), c),
                    "c{c} z{z} t{t}"
                );
            }
        }
    }
}

#[test]
fn stack_with_channels_padding_and_metadata() {
    let p = nis!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("stack.nd2");
    // T(2) outside Z(3), two interleaved channels, rows padded by 6 bytes.
    let mut spec = Spec::new(13, 7, 2, 16, vec![(1, 2), (4, 3)]);
    spec.row_pad = 6;
    nis::write(&path, &spec);
    let src = open(&p.reg, &path);
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
        (13, 7, 2, 3, 2, 6)
    );
    assert_eq!(i.dtype, DType::U16);
    assert!(!i.is_rgb);
    assert!(i.series.is_empty());
    assert_eq!(i.format, "Nikon ND2 (NIS-Elements AR 5.42.03)");
    assert_eq!(i.unit, "µm");
    assert_eq!(i.pixel_size, Some([0.108, 0.108, 0.25]));
    assert_eq!(
        i.channels
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["DAPI", "EGFP"]
    );
    // NIS colours are 0xBBGGRR.
    assert_eq!(i.channels[0].color, [0, 0, 255]);
    assert_eq!(i.channels[1].color, [0, 255, 0]);
    check_pixels(&*src, &spec, |z, t| t * 3 + z);
    // Channels share a page, so it is not one raw plane for the side views.
    assert!(src.raw_page(0).is_none());

    let groups: Vec<&str> = i.metadata.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(
        groups,
        [
            "NIS 图像",
            "NIS 显微镜",
            "通道 1：DAPI",
            "通道 2：EGFP",
            "NIS 全部参数"
        ]
    );
    assert_eq!(
        item(&i, "NIS 图像", "软件").as_deref(),
        Some("NIS-Elements AR 5.42.03 (Build 1234)")
    );
    assert_eq!(item(&i, "NIS 图像", "ND2 版本").as_deref(), Some("3.0"));
    assert_eq!(
        item(&i, "NIS 图像", "采集时间").as_deref(),
        Some("15/10/2026 10:12:03")
    );
    assert_eq!(
        item(&i, "NIS 图像", "尺寸").as_deref(),
        Some("X=13  Y=7  Z=3  C=2  T=2")
    );
    assert_eq!(
        item(&i, "NIS 图像", "实验循环").as_deref(),
        Some("T(2) › Z(3)")
    );
    assert_eq!(
        item(&i, "NIS 图像", "位深").as_deref(),
        Some("12（存储 16 位）")
    );
    assert_eq!(item(&i, "NIS 图像", "压缩").as_deref(), Some("无"));
    assert_eq!(item(&i, "NIS 图像", "Z 步长").as_deref(), Some("0.25 µm"));
    assert_eq!(item(&i, "NIS 图像", "时间间隔").as_deref(), Some("500 ms"));
    assert_eq!(
        item(&i, "NIS 图像", "载物台 XY (µm)").as_deref(),
        Some("12.5, -3")
    );
    assert_eq!(item(&i, "NIS 图像", "作者").as_deref(), Some("Tester"));
    assert_eq!(
        item(&i, "NIS 显微镜", "显微镜").as_deref(),
        Some("Nikon Ti2（Ti2 Microscope）")
    );
    assert_eq!(
        item(&i, "NIS 显微镜", "物镜").as_deref(),
        Some("Plan Apo λ 60x Oil")
    );
    assert_eq!(
        item(&i, "NIS 显微镜", "物镜放大倍数").as_deref(),
        Some("60×")
    );
    assert_eq!(
        item(&i, "NIS 显微镜", "数值孔径 NA").as_deref(),
        Some("1.42")
    );
    assert_eq!(item(&i, "NIS 显微镜", "折射率").as_deref(), Some("1.515"));
    assert_eq!(
        item(&i, "NIS 显微镜", "相机").as_deref(),
        Some("Photometrics Prime 95B")
    );
    assert_eq!(
        item(&i, "NIS 显微镜", "Z 驱动").as_deref(),
        Some("Ti2 ZDrive")
    );
    assert_eq!(item(&i, "NIS 显微镜", "温度").as_deref(), Some("23.0 °C"));
    assert_eq!(
        item(&i, "NIS 显微镜", "Camera Type").as_deref(),
        Some("Prime 95B")
    );
    let ch = "通道 2：EGFP";
    assert_eq!(item(&i, ch, "显示颜色").as_deref(), Some("#00FF00"));
    assert_eq!(item(&i, ch, "模式").as_deref(), Some("荧光、转盘共聚焦"));
    assert_eq!(item(&i, ch, "激发波长 (nm)").as_deref(), Some("460"));
    assert_eq!(item(&i, ch, "发射波长 (nm)").as_deref(), Some("520"));
    assert_eq!(item(&i, ch, "滤镜").as_deref(), Some("EGFP filter"));
    assert_eq!(item(&i, ch, "针孔直径 (µm)").as_deref(), Some("50"));
    assert_eq!(item(&i, ch, "曝光时间 (ms)").as_deref(), Some("101"));
    assert_eq!(item(&i, ch, "分档").as_deref(), Some("2×2"));
    assert_eq!(items(&i, ch, "激光"), ["488 nm, 21.5%"]);
    assert_eq!(item(&i, ch, "Binning").as_deref(), Some("2x2"));
    assert_eq!(
        item(&i, ch, "Nikon Ti2, FilterChanger(Turret1)").as_deref(),
        Some("2 (Quad)")
    );
    assert_eq!(items(&i, ch, "LUN-F, MultiLaser(LUN-F)").len(), 2);
    assert!(item(&i, ch, "Name").is_none());
    let all = "NIS 全部参数";
    assert_eq!(
        item(&i, all, "ImageAttributes/uiWidthBytes").as_deref(),
        Some("58")
    );
    assert_eq!(
        item(
            &i,
            all,
            "ImageMetadataSeq/sPicturePlanes/sPlaneNew/a1/sDescription"
        )
        .as_deref(),
        Some("EGFP")
    );
    let filter = "ImageMetadataSeq/sPicturePlanes/sPlaneNew/a1/pFilterPath/m_pFilter/1/m_sName";
    assert_eq!(item(&i, all, filter).as_deref(), Some("EGFP filter"));
    assert_eq!(item(&i, all, "文本信息/作者").as_deref(), Some("Tester"));
    assert_eq!(
        item(&i, all, "AppInfo/m_SWNameString").as_deref(),
        Some("NIS-Elements AR")
    );
}

#[test]
fn xy_positions_are_series() {
    let p = nis!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("multipoint.nd2");
    // T(2) › XY(4, one switched off) › Z(2): frame = (t·3 + p)·2 + z.
    let mut spec = Spec::new(5, 4, 1, 16, vec![(1, 2), (2, 4), (4, 2)]);
    spec.invalid_points = vec![1];
    nis::write(&path, &spec);
    let src = open(&p.reg, &path);
    let i = src.info().clone();
    assert_eq!((i.size_c, i.size_z, i.size_t, i.page_count), (1, 2, 2, 4));
    assert_eq!(
        i.series.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["Well 1", "Well 3", "Well 4"]
    );
    assert_eq!(i.series[0].detail, "5×4 · 2Z · 2T");
    assert_eq!(i.series_index, 0);
    assert_eq!(
        item(&i, "NIS 图像", "尺寸").as_deref(),
        Some("X=5  Y=4  Z=2  T=2  XY=3")
    );
    check_pixels(&*src, &spec, |z, t| (t * 3) * 2 + z);
    // A single uncompressed channel is read in place for the side views.
    let (raw, little) = src.raw_page(0).unwrap();
    assert!(little);
    assert_eq!(raw, &nis::frame(&spec, 0)[..]);

    for s in 1..3 {
        let src = formats::open_series_with(&path, s, &p.reg).unwrap();
        let i = src.info().clone();
        assert_eq!(i.series_index, s);
        assert_eq!(i.series.len(), 3);
        assert_eq!(
            item(&i, "NIS 图像", "位置名称").as_deref(),
            Some(["", "Well 3", "Well 4"][s])
        );
        let x = [0.0, 200.0, 300.0][s];
        assert_eq!(
            item(&i, "NIS 图像", "载物台 XY (µm)"),
            Some(format!("{x}, {}", -x / 2.0))
        );
        check_pixels(&*src, &spec, |z, t| (t * 3 + s) * 2 + z);
    }
    let e = match formats::open_series_with(&path, 3, &p.reg) {
        Ok(_) => panic!("series 3 opened"),
        Err(e) => format!("{e:#}"),
    };
    assert!(e.contains("3 个位置"), "{e}");
}

#[test]
fn compressed_frames_rgb_and_compressed_metadata() {
    let p = nis!();
    let tmp = tempfile::tempdir().unwrap();

    // Lossless (zlib) 16-bit Z stack, metadata behind a compressed record.
    let path = tmp.path().join("zlib.nd2");
    let mut spec = Spec::new(9, 6, 1, 16, vec![(4, 4)]);
    spec.compressed = true;
    spec.compressed_meta = true;
    spec.row_pad = 2;
    nis::write(&path, &spec);
    let src = open(&p.reg, &path);
    let i = src.info().clone();
    assert_eq!((i.size_z, i.size_t), (4, 1));
    assert_eq!(
        item(&i, "NIS 图像", "压缩").as_deref(),
        Some("无损（zlib）")
    );
    assert_eq!(i.channels[0].name, "DAPI");
    check_pixels(&*src, &spec, |z, _| z);
    assert!(src.raw_page(0).is_none());

    // 8-bit RGB brightfield: one plane with three components.
    let path = tmp.path().join("rgb.nd2");
    let mut spec = Spec::new(6, 3, 3, 8, vec![]);
    spec.channels = vec![("Brightfield", 0xffffff)];
    nis::write(&path, &spec);
    let src = open(&p.reg, &path);
    let i = src.info().clone();
    assert!(i.is_rgb);
    assert_eq!((i.size_c, i.samples_per_page, i.page_count), (3, 3, 1));
    assert_eq!(i.dtype, DType::U8);
    let planes = src.read_page(0).unwrap();
    for (c, plane) in planes.iter().enumerate() {
        assert_eq!(values(&plane.data), want(&spec, 0, c));
    }

    // 32-bit float.
    let path = tmp.path().join("float.nd2");
    let spec = Spec::new(4, 4, 1, 32, vec![(1, 3)]);
    nis::write(&path, &spec);
    let src = open(&p.reg, &path);
    assert_eq!(src.info().dtype, DType::F32);
    check_pixels(&*src, &spec, |_, t| t);
}

#[test]
fn version_2_xml_metadata_and_nonequidistant_time() {
    let p = nis!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("v2.nd2");
    // NE time loop: two periods, the second switched off; Z inside.
    let mut spec = Spec::new(8, 5, 2, 16, vec![(8, 3), (4, 2)]);
    spec.xml = true;
    spec.pad = false;
    nis::write(&path, &spec);
    let src = open(&p.reg, &path);
    let i = src.info().clone();
    assert_eq!((i.size_c, i.size_z, i.size_t), (2, 2, 3));
    assert_eq!(i.pixel_size, Some([0.108, 0.108, 0.25]));
    assert_eq!(
        i.channels
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["DAPI", "EGFP"]
    );
    assert_eq!(item(&i, "NIS 图像", "ND2 版本").as_deref(), Some("2.1"));
    assert_eq!(
        item(&i, "NIS 图像", "实验循环").as_deref(),
        Some("T′(3) › Z(2)")
    );
    assert_eq!(item(&i, "NIS 图像", "时间间隔").as_deref(), Some("2000 ms"));
    assert_eq!(
        item(&i, "NIS 显微镜", "物镜").as_deref(),
        Some("Plan Apo λ 60x Oil")
    );
    assert_eq!(items(&i, "通道 1：DAPI", "激光"), ["488 nm, 20.5%"]);
    assert_eq!(
        item(&i, "通道 1：DAPI", "发射波长 (nm)").as_deref(),
        Some("450")
    );
    check_pixels(&*src, &spec, |z, t| t * 2 + z);
}

#[test]
fn missing_frames_are_black() {
    let p = nis!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("stopped.nd2");
    // The time lapse promised 4 points but was stopped after 3 frames.
    let mut spec = Spec::new(4, 3, 1, 16, vec![(1, 4)]);
    spec.frames = Some(3);
    nis::write(&path, &spec);
    let src = open(&p.reg, &path);
    assert_eq!(src.info().size_t, 4);
    assert_eq!(
        values(&src.read_page(2).unwrap()[0].data),
        want(&spec, 2, 0)
    );
    assert!(values(&src.read_page(3).unwrap()[0].data)
        .iter()
        .all(|&v| v == 0.0));

    // Without an experiment, every frame is a time point.
    let path = tmp.path().join("noloops.nd2");
    let mut spec = Spec::new(4, 3, 1, 16, vec![]);
    spec.frames = Some(5);
    nis::write(&path, &spec);
    let src = open(&p.reg, &path);
    assert_eq!((src.info().size_z, src.info().size_t), (1, 5));
    check_pixels(&*src, &spec, |_, t| t);
}

#[test]
fn broken_and_unsupported_files() {
    let p = nis!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("x.nd2");

    // Old JPEG 2000-based ND2.
    let mut jp2 = vec![
        0, 0, 0, 0x0c, b'j', b'P', b' ', b' ', 0x0d, 0x0a, 0x87, 0x0a,
    ];
    jp2.resize(4096, 0);
    std::fs::write(&path, &jp2).unwrap();
    let e = err(&p.reg, &path);
    assert!(e.contains("旧版 ND2") && e.contains("JPEG 2000"), "{e}");

    std::fs::write(
        &path,
        b"not an nd2 file at all, just some text that is long enough",
    )
    .unwrap();
    assert!(err(&p.reg, &path).contains("不是 ND2"));

    // Lossy JPEG 2000 frames.
    let attrs = nis::lite(
        "SLxImageAttributes",
        &nis::level(vec![
            ("uiWidth", nis::Lv::U32(4)),
            ("uiWidthBytes", nis::Lv::U32(8)),
            ("uiHeight", nis::Lv::U32(4)),
            ("uiComp", nis::Lv::U32(1)),
            ("uiBpcInMemory", nis::Lv::U32(16)),
            ("uiSequenceCount", nis::Lv::U32(1)),
            ("eCompression", nis::Lv::U32(1)),
        ]),
    );
    let chunks = [
        ("ImageDataSeq|0!".into(), vec![0; 40]),
        ("ImageAttributesLV!".into(), attrs),
    ];
    nis::container(&path, "3.0", &chunks, true);
    assert!(err(&p.reg, &path).contains("有损"));

    // Unsupported sample size, garbage sizes.
    for (bits, w, msg) in [(12, 4, "位深"), (16, 0, "尺寸无效")] {
        let attrs = nis::lite(
            "SLxImageAttributes",
            &nis::level(vec![
                ("uiWidth", nis::Lv::U32(w)),
                ("uiHeight", nis::Lv::U32(4)),
                ("uiComp", nis::Lv::U32(1)),
                ("uiBpcInMemory", nis::Lv::U32(bits)),
            ]),
        );
        nis::container(&path, "3.0", &[("ImageAttributesLV!".into(), attrs)], true);
        assert!(err(&p.reg, &path).contains(msg), "{bits} {w}");
    }
    nis::container(&path, "3.0", &[], true);
    assert!(err(&p.reg, &path).contains("图像属性"));

    let spec = Spec::new(4, 4, 1, 16, vec![(1, 2)]);
    nis::write(&path, &spec);
    let good = std::fs::read(&path).unwrap();

    // A file cut short loses its chunk map; any cut is an error, not a crash.
    std::fs::write(&path, &good[..good.len() - 100]).unwrap();
    assert!(err(&p.reg, &path).contains("不完整"));
    for cut in (0..good.len()).step_by(997) {
        std::fs::write(&path, &good[..cut]).unwrap();
        let _ = formats::open_with(&path, &p.reg).and_then(|s| s.read_page(0));
    }
    // Corrupt bytes anywhere: open and read may fail but never crash.
    for k in (0..good.len()).step_by(211) {
        let mut b = good.clone();
        b[k] ^= 0x5a;
        std::fs::write(&path, &b).unwrap();
        let _ = formats::open_with(&path, &p.reg).and_then(|s| s.read_page(0));
    }
    // A frame cut short fails on read.
    let mut spec = Spec::new(16, 16, 1, 16, vec![]);
    spec.pad = false;
    nis::write(&path, &spec);
    let mut b = std::fs::read(&path).unwrap();
    let pos = b.windows(15).position(|w| w == b"ImageDataSeq|0!").unwrap() - 16;
    b[pos + 8..pos + 16].copy_from_slice(&100u64.to_le_bytes());
    std::fs::write(&path, &b).unwrap();
    let src = open(&p.reg, &path);
    assert!(format!("{:#}", src.read_page(0).err().unwrap()).contains("不完整"));
}

#[test]
fn needs_the_plugin_and_follows_the_switch() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.nd2");
    nis::write(&path, &Spec::new(4, 4, 1, 16, vec![]));
    let none = Registry::new(Some(tmp.path().join("no-plugins")), APP);
    // Apps that know the ND2 plugin name it; older ones just do not open the file.
    let e = err(&none, &path);
    assert!(
        e.contains("需要安装「Nikon NIS-Elements」插件") || e.contains("不支持的文件类型"),
        "{e}"
    );

    let p = nis!();
    assert!(formats::is_supported_with(&path, &p.reg));
    assert_eq!(p.reg.extensions(), ["nd2"]);
    p.reg.set_disabled(vec!["nis".into()]).unwrap();
    assert!(err(&p.reg, &path).contains("已停用"));
    p.reg.set_disabled(vec![]).unwrap();
    assert!(formats::open_with(&path, &p.reg).is_ok());

    use tifmrcview_lib::plugins::{package, registry::validate_package};
    let dir = p._p.dir.path().join("nis");
    let m = package::read_installed(&dir, APP).unwrap();
    validate_package(&dir, &m).unwrap();
}

#[test]
fn large_chunk_maps_stay_within_the_fuel_budget() {
    let p = nis!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("long.nd2");
    // 60 000 tiny frames (a long time lapse): the chunk map has 60 000 entries.
    let mut spec = Spec::new(2, 2, 1, 8, vec![(1, 300), (4, 200)]);
    spec.pad = false;
    nis::write(&path, &spec);
    let t = std::time::Instant::now();
    let src = open(&p.reg, &path);
    eprintln!("opened an ND2 with 60 000 frames in {:?}", t.elapsed());
    let i = src.info();
    assert_eq!((i.size_z, i.size_t, i.page_count), (200, 300, 60_000));
    let (page, _) = i.locate(0, 199, 299);
    assert_eq!(
        values(&src.read_page(page).unwrap()[0].data),
        want(&spec, 59_999, 0)
    );
}

// ---------------------------------------------------------------- real samples

const SAMPLES: &str = "/home/user/samples/nd2";

/// FNV-1a 64 of a plane's samples as little-endian bytes.
fn fnv(p: &PlaneData, dtype: DType) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    let mut eat = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    };
    for i in 0..p.len() {
        let v = p.get_f64(i);
        match dtype {
            DType::U8 => eat(v as u8),
            _ => (v as u16).to_le_bytes().into_iter().for_each(&mut eat),
        }
    }
    h
}

/// Against the Python `nd2` package (0.12): per channel, the sum of the
/// plane at Z = middle, T = 0; and FNV-1a of the plane at the last C, Z, T.
#[test]
fn real_samples_match_python_nd2() {
    let p = nis!();
    // Exception9_e3 has unpadded frame names, which `nd2` misreads (it assumes
    // frame data 4096 bytes into the chunk); its values come from the chunk header.
    #[rustfmt::skip]
    let cases: &[(&str, &[u64], u64)] = &[
        ("BF007.nd2", &[947969593], 0xdbe8a31b068dcbf3),
        ("Exception_2.nd2", &[72494520], 0xdd329f2cf0e5b658),
        ("Exception9_e3.nd2", &[5202812], 0xf3ae8bbdbdd4d0e9),
        ("Experiment_0001.nd2", &[365252552], 0x3c5dc649204a636f),
        ("HE_002.nd2", &[874340761, 876951196, 965043137], 0xa5086390193e7822),
        ("JJ_CropCyan.nd2", &[2749712], 0xa9121f15fa15a9db),
        ("MRAP1_KO_DN_10X03.nd2", &[913854845, 906837163, 923926059], 0xeeb0d800a3f3efea),
        ("MeOh_high_fluo_003.nd2", &[94833502], 0xeebb8c620a8fc07d),
        ("control002.nd2", &[7625162], 0xa13cb0e82f52654f),
        ("header_test2.nd2", &[98834207], 0xa0a4fdeef78e4cfd),
        ("nikonTest_two.nd2", &[184825972, 73066319], 0x4b68ce67843da911),
        ("sample_image.nd2", &[430216741, 462082542, 416210297, 220403920, 3331422063], 0x50563308b857880c),
    ];
    for &(name, sums, hash) in cases {
        let path = Path::new(SAMPLES).join(name);
        if !path.exists() {
            eprintln!("SKIPPED: {} is not there", path.display());
            continue;
        }
        let t = std::time::Instant::now();
        let src = open(&p.reg, &path);
        let i = src.info().clone();
        let dims = format!(
            "{}×{} C{} Z{} T{}",
            i.width, i.height, i.size_c, i.size_z, i.size_t
        );
        eprintln!("{name}: {} {dims} opened in {:?}", i.format, t.elapsed());
        assert_eq!(i.size_c, sums.len(), "{name}");
        for (c, &want) in sums.iter().enumerate() {
            let (page, s) = i.locate(c, i.size_z / 2, 0);
            let got: f64 = values(&src.read_page(page).unwrap()[s].data).iter().sum();
            assert_eq!(got as u64, want, "{name} c{c}");
        }
        let (page, s) = i.locate(i.size_c - 1, i.size_z - 1, i.size_t - 1);
        assert_eq!(
            fnv(&src.read_page(page).unwrap()[s].data, i.dtype),
            hash,
            "{name} last plane"
        );
    }
}

#[test]
fn real_sample_metadata() {
    let p = nis!();
    let path = Path::new(SAMPLES).join("sample_image.nd2");
    if !path.exists() {
        eprintln!("SKIPPED: {} is not there", path.display());
        return;
    }
    let src = open(&p.reg, &path);
    let i = src.info().clone();
    assert_eq!(
        (i.width, i.height, i.size_c, i.size_z, i.size_t),
        (1019, 1019, 5, 21, 1)
    );
    assert_eq!(i.format, "Nikon ND2 (NIS-Elements AR 4.50.00)");
    assert_eq!(
        i.pixel_size,
        Some([1.0153102411107793, 1.0153102411107793, 5.0])
    );
    assert!(i.series.is_empty(), "one XY position is not a series");
    let names: Vec<&str> = i.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "CSU far red RNA",
            "CSU red RNA",
            "CSU green RNA",
            "CSU blue RNA",
            "CSU BF"
        ]
    );
    assert_eq!(i.channels[1].color, [255, 0, 0]);
    assert_eq!(
        item(&i, "NIS 显微镜", "物镜").as_deref(),
        Some("Plan Apo λ 20x")
    );
    assert_eq!(
        item(&i, "NIS 显微镜", "物镜放大倍数").as_deref(),
        Some("20×")
    );
    assert_eq!(
        item(&i, "NIS 显微镜", "数值孔径 NA").as_deref(),
        Some("0.75")
    );
    assert_eq!(item(&i, "NIS 显微镜", "变焦").as_deref(), Some("1.2×"));
    assert_eq!(
        item(&i, "NIS 显微镜", "相机").as_deref(),
        Some("Andor DU-897 X-3655")
    );
    assert_eq!(
        item(&i, "NIS 显微镜", "Z 驱动").as_deref(),
        Some("NIDAQ Piezo Z (name: NanoScan-Z)")
    );
    assert_eq!(
        item(&i, "通道 2：CSU red RNA", "发射波长 (nm)").as_deref(),
        Some("607")
    );
    assert_eq!(items(&i, "通道 2：CSU red RNA", "激光"), ["561 nm, 81.0%"]);
    assert_eq!(
        item(&i, "通道 1：CSU far red RNA", "激发波长 (nm)").as_deref(),
        Some("640")
    );
    assert!(i.metadata.last().unwrap().items.len() > 1000);
    if std::env::var_os("NIS_DUMP").is_some() {
        for g in &i.metadata {
            eprintln!("== {} ({} items)", g.name, g.items.len());
            for it in g.items.iter().take(60) {
                eprintln!("   {} = {}", it[0], it[1].replace("\r\n", " ⏎ "));
            }
        }
    }
}
