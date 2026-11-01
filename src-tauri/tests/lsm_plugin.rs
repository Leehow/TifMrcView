//! Zeiss LSM through the "zeiss" WebAssembly plugin, end to end: the plugin
//! is built from `plugins/zeiss`, installed into a temporary plugin folder
//! and used through `formats::open_with` / `open_series_with`, exactly as the
//! app does.

mod common;

use std::path::Path;

use common::lsm::{self, Comp, Spec};
use tifmrcview_lib::formats;
use tifmrcview_lib::image::{DType, ImageInfo, PlaneData, Source};
use tifmrcview_lib::plugins::registry::Registry;

macro_rules! zeiss {
    () => {
        match common::install(&["zeiss"]) {
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
    i.metadata
        .iter()
        .find(|g| g.name == group)?
        .items
        .iter()
        .find(|it| it[0] == key)
        .map(|it| it[1].clone())
}

/// Checks every (c, z, t) plane of `src` against the synthetic pattern; `first` is the index of
/// the first image of this series.
fn check_pixels(src: &dyn Source, s: &Spec, first: usize, what: &str) {
    let i = src.info();
    for t in 0..s.t {
        for z in 0..s.z {
            for c in 0..s.c {
                let (page, sample) = i.locate(c, z, t);
                let planes = src
                    .read_page(page)
                    .unwrap_or_else(|e| panic!("{what} page {page}: {e:#}"));
                let want: Vec<f64> = (0..s.h)
                    .flat_map(|y| {
                        (0..s.w)
                            .map(move |x| lsm::pattern(first + t * s.z + z, c, x, y, s.bits) as f64)
                    })
                    .collect();
                assert_eq!(values(&planes[sample].data), want, "{what}: c{c} z{z} t{t}");
            }
        }
    }
}

fn write(dir: &Path, name: &str, s: &Spec) -> std::path::PathBuf {
    let path = dir.join(name);
    lsm::write(&path, s);
    path
}

#[test]
fn stack_info_metadata_and_pixels() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    // Two channels (with the empty third entry of the strip arrays that LSM writes), three Z planes,
    // strips of five rows (the last one has one), thumbnails between the images.
    let mut s = Spec::new(24, 16, 2, 3, 1);
    s.rows_per_strip = 5;
    let path = write(tmp.path(), "stack.lsm", &s);
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
        (24, 16, 2, 3, 1, 3)
    );
    assert_eq!(i.samples_per_page, 2);
    assert_eq!(i.dtype, DType::U8);
    assert_eq!(i.format, "Zeiss LSM");
    assert!(i.series.is_empty());
    assert_eq!(i.unit, "µm");
    let ps = i.pixel_size.unwrap();
    assert!(
        (ps[0] - 0.25).abs() < 1e-9 && (ps[1] - 0.25).abs() < 1e-9 && (ps[2] - 1.5).abs() < 1e-9,
        "{ps:?}"
    );
    assert_eq!(
        i.channels
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["Ch1-T1", "Ch2-T2"]
    );
    assert_eq!(i.channels[0].color, [0, 255, 0]);
    assert_eq!(i.channels[1].color, [255, 0, 0]);

    let groups: Vec<&str> = i.metadata.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(
        groups,
        [
            "ZEN black 图像",
            "ZEN black 显微镜",
            "通道 1：Ch1-T1",
            "通道 2：Ch2-T2",
            "ZEN black 全部参数"
        ]
    );
    let img = "ZEN black 图像";
    assert_eq!(
        item(i, img, "记录名称").as_deref(),
        Some("Synthetic recording")
    );
    assert_eq!(item(i, img, "用户").as_deref(), Some("tester"));
    assert_eq!(
        item(i, img, "采集时间").as_deref(),
        Some("2005-05-19 12:00:00")
    );
    assert_eq!(item(i, img, "扫描类型").as_deref(), Some("Z 栈（XYZ）"));
    assert_eq!(item(i, img, "尺寸").as_deref(), Some("X=24 Y=16  Z=3  C=2"));
    assert_eq!(item(i, img, "像素类型").as_deref(), Some("8 位无符号整数"));
    assert_eq!(item(i, img, "像素尺寸 X").as_deref(), Some("0.2500 µm"));
    assert_eq!(item(i, img, "Z 步长").as_deref(), Some("1.5000 µm"));
    assert_eq!(item(i, img, "扫描缩放").as_deref(), Some("2"));
    assert_eq!(item(i, img, "视场").as_deref(), Some("127.5 × 127.5 µm"));
    assert!(item(i, img, "时间间隔").is_none(), "no time axis");
    let mic = "ZEN black 显微镜";
    assert_eq!(
        item(i, mic, "物镜").as_deref(),
        Some("Plan-Apochromat 63x/1.40 Oil")
    );
    assert_eq!(
        item(i, mic, "激光器 Argon/2").as_deref(),
        Some("启用，功率 25.5")
    );
    assert_eq!(
        item(i, mic, "激光器 HeNe1").as_deref(),
        Some("关闭，功率 1")
    );
    assert_eq!(
        item(i, mic, "分束器 HT").as_deref(),
        Some("HFT UV/488/543/633")
    );
    let ch1 = "通道 1：Ch1-T1";
    assert_eq!(item(i, ch1, "轨道").as_deref(), Some("1：Track A"));
    assert_eq!(item(i, ch1, "检测器").as_deref(), Some("Pmt2"));
    assert_eq!(item(i, ch1, "针孔 (µm)").as_deref(), Some("123"));
    assert_eq!(item(i, ch1, "检测器增益").as_deref(), Some("812"));
    assert_eq!(item(i, ch1, "激发 (nm)").as_deref(), Some("488，透过率 2%"));
    assert_eq!(item(i, ch1, "滤光片").as_deref(), Some("EF2 BP 500-550 IR"));
    assert_eq!(item(i, ch1, "采样").as_deref(), Some("帧平均 × 4（平均）"));
    assert_eq!(item(i, ch1, "像素驻留 (µs)").as_deref(), Some("1.28"));
    assert_eq!(item(i, ch1, "位深").as_deref(), Some("8"));
    assert_eq!(item(i, ch1, "颜色").as_deref(), Some("#00FF00"));
    let ch2 = "通道 2：Ch2-T2";
    assert_eq!(item(i, ch2, "轨道").as_deref(), Some("2：Track B"));
    assert_eq!(item(i, ch2, "检测器").as_deref(), Some("Pmt3"));
    assert_eq!(
        item(i, ch2, "激发 (nm)").as_deref(),
        Some("543，透过率 22%")
    );
    let all = "ZEN black 全部参数";
    assert_eq!(
        item(i, all, "LSMINFO/DimensionChannels").as_deref(),
        Some("2")
    );
    assert_eq!(
        item(
            i,
            all,
            "Recording/Track[2]/DetectionChannel[2]/PointDetectorName"
        )
        .as_deref(),
        Some("Pmt3")
    );
    assert_eq!(
        item(i, all, "Recording/Laser[1]/Power").as_deref(),
        Some("25.5")
    );
    assert_eq!(
        item(i, all, "Recording/Entry0x10000058").as_deref(),
        Some("7")
    );
    // Every thumbnail was skipped, whatever the channel count.
    check_pixels(&*src, &s, 0, "planar");
}

#[test]
fn single_channel_raw_pages_are_read_in_place() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Spec::new(16, 16, 1, 2, 1);
    s.rows_per_strip = 4;
    s.bits = 16;
    let path = write(tmp.path(), "one.lsm", &s);
    let src = open(&p.registry, &path);
    let i = src.info();
    assert_eq!((i.size_c, i.samples_per_page, i.dtype), (1, 1, DType::U16));
    // The four strips of a page follow each other: one chunk, so side views can read it directly.
    for z in 0..2 {
        let (raw, little) = src.raw_page(z).expect("raw page");
        assert!(little);
        assert_eq!(raw, &lsm::plane_bytes(&s, z, 0)[..]);
    }
    assert_eq!(i.channels[0].color, [0, 255, 0]);
    check_pixels(&*src, &s, 0, "one channel");
}

#[test]
fn compression_predictor_and_time_series() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    for (comp, bits, name) in [
        (Comp::Lzw, 8, "lzw8"),
        (Comp::Lzw, 16, "lzw16"),
        (Comp::LzwPredictor, 8, "pred8"),
        (Comp::LzwPredictor, 16, "pred16"),
    ] {
        // Two channels, 2 Z x 3 T, strips of five rows.
        let mut s = Spec::new(33, 16, 2, 2, 3);
        s.comp = comp;
        s.bits = bits;
        s.rows_per_strip = 5;
        let path = write(tmp.path(), &format!("{name}.lsm"), &s);
        let src = open(&p.registry, &path);
        let i = src.info();
        assert_eq!(
            (i.size_c, i.size_z, i.size_t, i.page_count),
            (2, 2, 3, 6),
            "{name}"
        );
        assert_eq!(i.dtype, if bits == 8 { DType::U8 } else { DType::U16 });
        assert!(src.raw_page(0).is_none());
        check_pixels(&*src, &s, 0, name);
        // Time series information.
        assert_eq!(
            item(i, "ZEN black 图像", "扫描类型").as_deref(),
            Some("时间序列 Z 栈（XYZ-T）")
        );
        assert_eq!(
            item(i, "ZEN black 图像", "时间间隔").as_deref(),
            Some("2.5 s")
        );
        assert_eq!(
            item(i, "ZEN black 图像", "实测总时长").as_deref(),
            Some("12.5 s")
        );
        assert_eq!(
            item(i, "ZEN black 图像", "实测平均间隔").as_deref(),
            Some("2.5 s")
        );
    }
}

#[test]
fn interleaved_samples() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    for (comp, bits) in [
        (Comp::None, 8),
        (Comp::None, 16),
        (Comp::LzwPredictor, 8),
        (Comp::LzwPredictor, 16),
    ] {
        // Three channels as chunky samples; the LSMINFO has no scan information
        // for the third one (only two tracks are on), so it is matched by name.
        let mut s = Spec::new(20, 12, 3, 2, 1);
        s.planar = false;
        s.comp = comp;
        s.bits = bits;
        s.rows_per_strip = 4;
        let path = write(tmp.path(), &format!("chunky_{comp:?}_{bits}.lsm"), &s);
        let src = open(&p.registry, &path);
        let i = src.info();
        assert_eq!((i.size_c, i.size_z, i.samples_per_page), (3, 2, 3));
        assert_eq!(i.channels[2].name, "ChS1-T3");
        assert_eq!(i.channels[2].color, [10, 20, 30]);
        assert!(item(i, "通道 3：ChS1-T3", "轨道").is_none());
        assert_eq!(
            item(i, "通道 2：Ch2-T2", "轨道").as_deref(),
            Some("2：Track B")
        );
        check_pixels(&*src, &s, 0, &format!("{comp:?} {bits}"));
    }
}

#[test]
fn positions_and_tiles_are_series() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    // 2 positions x 3 tiles, 2 Z planes each: tile is the outer loop, then position.
    let mut s = Spec::new(16, 8, 2, 2, 1);
    s.positions = 2;
    s.tiles = 3;
    let path = write(tmp.path(), "multi.lsm", &s);
    let src = open(&p.registry, &path);
    let i = src.info();
    assert_eq!(i.series.len(), 6);
    assert_eq!(i.series_index, 0);
    assert_eq!(i.series[0].name, "拼块 1 · 位置 1");
    assert_eq!(i.series[1].name, "拼块 1 · 位置 2");
    assert_eq!(i.series[5].name, "拼块 3 · 位置 2");
    assert_eq!(i.series[3].detail, "16×8 · 2C · 2Z · 1T");
    assert_eq!(i.page_count, 2);
    assert_eq!(
        item(i, "ZEN black 图像", "位置").as_deref(),
        Some("1 / 2（100，5.5，0）")
    );
    assert_eq!(
        item(i, "ZEN black 图像", "拼块").as_deref(),
        Some("1 / 3（200，5.5，0）")
    );
    check_pixels(&*src, &Spec { z: 2, ..s.clone() }, 0, "series 0");
    for k in 1..6usize {
        let src = formats::open_series_with(&path, k, &p.registry)
            .unwrap_or_else(|e| panic!("series {k}: {e:#}"));
        let i = src.info();
        assert_eq!(i.series_index, k);
        assert_eq!(i.series.len(), 6);
        assert_eq!((i.size_z, i.page_count), (2, 2));
        assert_eq!(
            item(i, "ZEN black 图像", "位置")
                .as_deref()
                .unwrap()
                .split(' ')
                .next(),
            Some(&*(k % 2 + 1).to_string())
        );
        check_pixels(&*src, &s, k * 2, &format!("series {k}"));
    }
    assert!(formats::open_series_with(&path, 6, &p.registry).is_err());

    // Only positions: "位置 n".
    let mut s = Spec::new(16, 8, 1, 1, 1);
    s.positions = 3;
    let path = write(tmp.path(), "pos.lsm", &s);
    let src = open(&p.registry, &path);
    assert_eq!(
        src.info()
            .series
            .iter()
            .map(|x| x.name.as_str())
            .collect::<Vec<_>>(),
        ["位置 1", "位置 2", "位置 3"]
    );
}

#[test]
fn aborted_acquisition_shows_the_saved_images() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    // Four Z planes announced, three written.
    let mut s = Spec::new(16, 8, 2, 4, 1);
    s.pairs_written = Some(3);
    let path = write(tmp.path(), "short.lsm", &s);
    let src = open(&p.registry, &path);
    let i = src.info();
    assert_eq!((i.size_z, i.size_t, i.page_count), (3, 1, 3));
    assert!(item(i, "ZEN black 图像", "提示")
        .unwrap()
        .contains("不完整"));
    check_pixels(&*src, &Spec { z: 3, ..s.clone() }, 0, "aborted");

    // Time series: 2 Z x 3 T announced, 2 time points written.
    let mut s = Spec::new(16, 8, 1, 2, 3);
    s.pairs_written = Some(4);
    let path = write(tmp.path(), "short_t.lsm", &s);
    let src = open(&p.registry, &path);
    assert_eq!((src.info().size_z, src.info().size_t), (2, 2));
}

/// Files over 4 GiB: StripOffsets are 32-bit and wrap. A sparse file keeps the test cheap.
#[cfg(unix)]
#[test]
fn offsets_beyond_4_gib_are_unwrapped() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    // The data starts below 4 GiB, so strips on both sides of the wrap occur (and for two
    // channels the empty third entry points at the end of the data). In the first case the
    // very first image and its thumbnail straddle the wrap.
    for (comp, below, z, name) in [
        (Comp::None, 700, 4, "big_raw.lsm"),
        (Comp::Lzw, 700, 4, "big_lzw.lsm"),
        (Comp::None, 2000, 12, "big_raw2.lsm"),
        (Comp::Lzw, 2000, 12, "big_lzw2.lsm"),
    ] {
        let mut s = Spec::new(24, 16, 2, z, 1);
        s.comp = comp;
        s.rows_per_strip = 8;
        s.data_at = Some((1u64 << 32) - below);
        let path = write(tmp.path(), name, &s);
        if std::fs::metadata(&path).unwrap().len() < 1 << 32 {
            panic!("not a big file");
        }
        let src = open(&p.registry, &path);
        let i = src.info();
        assert_eq!((i.size_c, i.size_z), (2, z));
        check_pixels(&*src, &s, 0, name);
    }
}

#[test]
fn broken_and_unsupported_files() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();

    // A TIFF without CZ_LSMINFO is not an LSM.
    let tiff = tmp.path().join("plain.lsm");
    let mut data = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    let entries: [(u16, u16, u32, u32); 6] = [
        (256, 3, 1, 2),
        (257, 3, 1, 2),
        (258, 3, 1, 8),
        (259, 3, 1, 1),
        (273, 4, 1, 100),
        (279, 4, 1, 4),
    ];
    data.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (t, ty, c, v) in entries {
        data.extend_from_slice(&t.to_le_bytes());
        data.extend_from_slice(&ty.to_le_bytes());
        data.extend_from_slice(&c.to_le_bytes());
        data.extend_from_slice(&v.to_le_bytes());
    }
    data.extend_from_slice(&0u32.to_le_bytes());
    data.resize(104, 0);
    std::fs::write(&tiff, &data).unwrap();
    let e = err(&p.registry, &tiff);
    assert!(e.contains("不是 Zeiss LSM"), "{e}");

    // Not a TIFF at all.
    let junk = tmp.path().join("junk.lsm");
    std::fs::write(&junk, b"this is not a tiff file at all").unwrap();
    assert!(err(&p.registry, &junk).contains("LSM"));

    // Different bit depths per channel are not supported (and say so).
    let mut s = Spec::new(16, 8, 3, 1, 1);
    s.planar = true;
    let path = write(tmp.path(), "mixed.lsm", &s);
    let mut bytes = std::fs::read(&path).unwrap();
    // BitsPerSample of the first IFD is an array of three SHORTs; make the last one 16.
    let at = bytes
        .windows(6)
        .position(|w| w == [8, 0, 8, 0, 8, 0])
        .expect("bits array");
    bytes[at + 4] = 16;
    std::fs::write(&path, &bytes).unwrap();
    let e = err(&p.registry, &path);
    assert!(e.contains("位深不同"), "{e}");

    // Unsupported compression.
    let s = Spec::new(16, 8, 1, 1, 1);
    let path = write(tmp.path(), "jpeg.lsm", &s);
    let mut bytes = std::fs::read(&path).unwrap();
    // Compression tag (259, SHORT, 1): value 1 -> 7 (JPEG).
    let at = bytes
        .windows(8)
        .position(|w| w == [3, 1, 3, 0, 1, 0, 0, 0])
        .expect("compression entry");
    bytes[at + 8] = 7;
    std::fs::write(&path, &bytes).unwrap();
    assert!(err(&p.registry, &path).contains("压缩方式 7"));

    // Truncated files and garbage are errors, not crashes.
    let mut s = Spec::new(24, 16, 2, 2, 1);
    s.comp = Comp::Lzw;
    let path = write(tmp.path(), "cut.lsm", &s);
    let full = std::fs::read(&path).unwrap();
    for cut in [5, 100, 400, 900, full.len() / 2, full.len() - 20] {
        std::fs::write(&path, &full[..cut]).unwrap();
        let _ = formats::open_with(&path, &p.registry).and_then(|src| {
            (0..src.info().page_count).try_for_each(|k| src.read_page(k).map(|_| ()))
        });
    }
    // Corrupt strips inside an intact structure.
    let mut bytes = full.clone();
    let n = bytes.len();
    for b in &mut bytes[n - 300..] {
        *b = 0xFF;
    }
    std::fs::write(&path, &bytes).unwrap();
    let _ = formats::open_with(&path, &p.registry)
        .map(|src| (0..src.info().page_count).for_each(|k| drop(src.read_page(k))));
}

/// Real LSM files of the OME sample set (not in the repository): sizes, metadata and checksums
/// from tifffile.
#[test]
fn sample_cntrl1() {
    let path = Path::new("/home/user/samples/lsm/Cntrl1.lsm");
    if !path.exists() {
        eprintln!("SKIPPED: {} is not there", path.display());
        return;
    }
    let p = zeiss!();
    let src = open(&p.registry, path);
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
        (1024, 1024, 2, 1, 1, 1)
    );
    assert_eq!(i.dtype, DType::U8);
    assert_eq!(i.unit, "µm");
    let ps = i.pixel_size.unwrap();
    assert!(
        (ps[0] - 1.2724649418062058).abs() < 1e-9 && (ps[2] - 1.3793103448275862).abs() < 1e-9,
        "{ps:?}"
    );
    assert_eq!(
        i.channels
            .iter()
            .map(|c| (c.name.as_str(), c.color))
            .collect::<Vec<_>>(),
        [("Ch3-T1", [255, 0, 0]), ("Ch2-T2", [0, 255, 0])]
    );
    let img = "ZEN black 图像";
    assert_eq!(item(i, img, "记录名称").as_deref(), Some("Cntrl 2"));
    assert_eq!(
        item(i, "ZEN black 显微镜", "物镜").as_deref(),
        Some("Plan-Neofluar 10x/0.3")
    );
    assert_eq!(
        item(i, "通道 1：Ch3-T1", "轨道").as_deref(),
        Some("1：z* RHOD for dual with FITC PMT")
    );
    assert_eq!(item(i, "通道 1：Ch3-T1", "检测器").as_deref(), Some("Pmt3"));
    assert_eq!(
        item(i, "通道 1：Ch3-T1", "针孔 (µm)").as_deref(),
        Some("454")
    );
    assert_eq!(item(i, "通道 2：Ch2-T2", "检测器").as_deref(), Some("Pmt2"));
    assert_eq!(
        item(i, "通道 2：Ch2-T2", "激发 (nm)").as_deref(),
        Some("488，透过率 2%")
    );
    // Sums and a few pixels as tifffile reads them.
    let want = [
        (21994936.0, [28.0, 43.0, 9.0, 0.0]),
        (32546241.0, [32.0, 28.0, 23.0, 16.0]),
    ];
    let planes = src.read_page(0).unwrap();
    assert_eq!(planes.len(), 2);
    for (c, (sum, px)) in want.iter().enumerate() {
        let v = values(&planes[c].data);
        assert_eq!(v.iter().sum::<f64>(), *sum, "channel {c}");
        let got: Vec<f64> = [(0, 0), (500, 300), (1023, 1023), (17, 900)]
            .iter()
            .map(|&(x, y)| v[y * 1024 + x])
            .collect();
        assert_eq!(&got, px, "channel {c}");
    }
}
