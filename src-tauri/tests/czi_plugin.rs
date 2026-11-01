//! Zeiss CZI through the "zeiss" WebAssembly plugin, end to end: the plugin
//! is built from `plugins/zeiss`, installed into a temporary plugin folder
//! and used through `formats::open_with`, exactly as the app does.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::czi::{self, Block};
use tifmrcview_lib::formats;
use tifmrcview_lib::image::{DType, OpenImage, PlaneData, Source};
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

#[test]
fn stack_info_metadata_and_pixels() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("stack.czi");
    czi::stack(&path, 64, 48, 2, 3, 0, 0);
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
    assert_eq!(i.dtype, DType::U16);
    assert_eq!(i.format, "Zeiss ZEN CZI (ZEN 3.8.2.0)");
    assert_eq!(i.unit, "µm");
    let ps = i.pixel_size.unwrap();
    assert!(
        (ps[0] - 0.045).abs() < 1e-9 && (ps[1] - 0.045).abs() < 1e-9 && (ps[2] - 0.15).abs() < 1e-9
    );
    assert_eq!(
        i.channels
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["EGFP", "mCherry"]
    );
    assert_eq!(i.channels[0].color, [0, 255, 0x5b]);
    assert_eq!(i.channels[1].color, [255, 0, 0]);
    let groups: Vec<&str> = i.metadata.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(
        groups,
        [
            "ZEN 图像",
            "ZEN 显微镜",
            "通道 1：EGFP",
            "通道 2：mCherry",
            "ZEN 处理",
            "ZEN 全部参数"
        ]
    );
    let item = |g: usize, k: &str| {
        i.metadata[g]
            .items
            .iter()
            .find(|it| it[0] == k)
            .map(|it| it[1].clone())
    };
    assert_eq!(
        item(0, "采集时间").as_deref(),
        Some("2026-10-15 10:12:03 +08:00")
    );
    assert_eq!(item(0, "像素尺寸 X").as_deref(), Some("0.0450 µm"));
    assert_eq!(
        item(1, "物镜").as_deref(),
        Some("Plan-Apochromat 63x/1.40 Oil")
    );
    assert_eq!(item(2, "激发波长 (nm)").as_deref(), Some("488"));
    assert_eq!(item(2, "探测器").as_deref(), Some("Airyscan"));
    assert_eq!(item(2, "像素驻留 (µs)").as_deref(), Some("1.03"));
    assert_eq!(item(4, "Airyscan 强度（通道 1）").as_deref(), Some("5.2"));

    for z in 0..3 {
        for c in 0..2 {
            let (page, s) = i.locate(c, z, 0);
            assert_eq!(s, 0);
            let planes = src.read_page(page).unwrap();
            assert_eq!(planes.len(), 1);
            let want: Vec<f64> = (0..48)
                .flat_map(|y| (0..64).map(move |x| czi::pattern(c, z, x, y) as f64))
                .collect();
            assert_eq!(values(&planes[0].data), want, "c{c} z{z}");
            // Uncompressed single-tile pages are readable in place (side views).
            let (raw, little) = src.raw_page(page).unwrap();
            assert!(little);
            assert_eq!(raw, &czi::u16_tile(c, z, 0, 0, 64, 48)[..]);
        }
    }
}

#[test]
fn compressed_subblocks() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    let (w, h) = (40, 30);
    let raw = czi::u16_tile(0, 0, 0, 0, w, h);
    let want: Vec<f64> = raw
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]) as f64)
        .collect();
    let tile = || Block::tile(1, 0, 0, w as i32, h as i32, 0, 0, 0, 0, vec![]);
    for (name, block) in [
        ("zstd0", tile().compressed(5, czi::zstd(&raw))),
        ("zstd1-hilo", tile().compressed(6, czi::zstd1_hilo(&raw))),
        ("zstd1", tile().compressed(6, czi::zstd1_plain(&raw))),
    ] {
        let path = tmp.path().join(format!("{name}.czi"));
        czi::write(&path, &[block], "<ImageDocument/>", 0);
        let src = open(&p.registry, &path);
        assert_eq!(values(&src.read_page(0).unwrap()[0].data), want, "{name}");
        assert!(src.raw_page(0).is_none(), "{name} is not stored raw");
        assert_eq!(src.info().format, "Zeiss ZEN CZI");
    }

    // JPEG gray 8-bit.
    let gray: Vec<u8> = (0..w * h).map(|i| (i % w * 6) as u8).collect();
    let mut jpg = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 100)
        .encode(&gray, w as u32, h as u32, image::ExtendedColorType::L8)
        .unwrap();
    let path = tmp.path().join("jpeg.czi");
    czi::write(
        &path,
        &[Block::tile(0, 0, 0, w as i32, h as i32, 0, 0, 0, 0, vec![]).compressed(1, jpg)],
        "<ImageDocument/>",
        0,
    );
    let src = open(&p.registry, &path);
    assert_eq!(src.info().dtype, DType::U8);
    let got = values(&src.read_page(0).unwrap()[0].data);
    let diff = got
        .iter()
        .zip(&gray)
        .map(|(a, &b)| (a - b as f64).abs())
        .fold(0.0, f64::max);
    assert!(diff <= 3.0, "jpeg max diff {diff}");
}

#[test]
fn mosaic_rgb_pyramid_and_extra_dimensions() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();

    // Two 4x2 tiles overlapping by 2 columns, origin at negative coordinates;
    // M=1 is drawn over M=0. A pyramid level and a second phase (H=1) are ignored.
    let a: Vec<u8> = [1u16; 8].iter().flat_map(|v| v.to_le_bytes()).collect();
    let b: Vec<u8> = [2u16; 8].iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut pyramid = Block::tile(1, -10, -5, 8, 4, 0, 0, 0, 0, vec![9; 16]);
    pyramid.pyramid = 1;
    pyramid.dims[0] = ("X", -10, 8, 4);
    let mut other_phase = Block::tile(1, -10, -5, 4, 2, 0, 0, 0, 0, vec![7; 16]);
    other_phase.dims.push(("H", 1, 1, 1));
    let mut first_phase = Block::tile(1, -8, -5, 4, 2, 0, 0, 0, 1, b.clone());
    first_phase.dims.push(("H", 0, 1, 1));
    let blocks = vec![
        first_phase,
        Block::tile(1, -10, -5, 4, 2, 0, 0, 0, 0, a),
        pyramid,
        other_phase,
    ];
    let path = tmp.path().join("mosaic.czi");
    czi::write(&path, &blocks, "<ImageDocument/>", 0);
    let src = open(&p.registry, &path);
    assert_eq!(
        (src.info().width, src.info().height, src.info().page_count),
        (6, 2, 1)
    );
    assert_eq!(
        values(&src.read_page(0).unwrap()[0].data),
        [1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]
    );

    // BGR 8-bit -> RGB, BGRA drops alpha.
    for (code, n) in [(3, 3), (9, 4)] {
        let px: Vec<u8> = (0..2)
            .flat_map(|i| [10 + i, 20 + i, 30 + i, 255][..n].to_vec())
            .collect();
        let path = tmp.path().join(format!("bgr{n}.czi"));
        czi::write(
            &path,
            &[Block::tile(code, 0, 0, 2, 1, 0, 0, 0, 0, px)],
            "<ImageDocument/>",
            0,
        );
        let src = open(&p.registry, &path);
        let i = src.info();
        assert!(i.is_rgb);
        assert_eq!((i.size_c, i.samples_per_page), (3, 3));
        let planes = src.read_page(0).unwrap();
        assert_eq!(planes.len(), 3);
        assert_eq!(values(&planes[0].data), [30.0, 31.0]);
        assert_eq!(values(&planes[2].data), [10.0, 11.0]);
    }

    // C/Z/T indices with gaps are ranked; T is the slow axis.
    let blocks: Vec<Block> = [(3, 0, 5), (3, 0, 9), (7, 0, 5), (7, 0, 9)]
        .iter()
        .map(|&(c, z, t)| Block::tile(0, 0, 0, 1, 1, c, z, t, 0, vec![(c * 10 + t) as u8]))
        .collect();
    let path = tmp.path().join("ranks.czi");
    czi::write(&path, &blocks, "<ImageDocument/>", 0);
    let src = open(&p.registry, &path);
    let i = src.info().clone();
    assert_eq!((i.size_c, i.size_z, i.size_t), (2, 1, 2));
    assert_eq!(
        values(&src.read_page(i.locate(1, 0, 1).0).unwrap()[0].data),
        [79.0]
    );
}

#[test]
fn broken_and_unsupported_files() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    let tile = |px: i32| Block::tile(px, 0, 0, 2, 2, 0, 0, 0, 0, vec![0; 8]);
    let case = |name: &str, blocks: &[Block], part: i32| {
        let path = tmp.path().join(format!("{name}.czi"));
        czi::write(&path, blocks, "<ImageDocument/>", part);
        path
    };
    let e = err(&p.registry, &case("part", &[tile(1)], 1));
    assert!(e.contains("分卷"), "{e}");
    assert!(err(&p.registry, &case("empty", &[], 0)).contains("没有图像"));
    assert!(err(&p.registry, &case("pixel", &[tile(77)], 0)).contains("像素类型 77"));
    assert!(err(
        &p.registry,
        &case(
            "mixed",
            &[tile(1), Block::tile(0, 2, 0, 2, 2, 0, 0, 0, 0, vec![0; 4])],
            0
        )
    )
    .contains("混有"));

    // JPEG XR opens but its pages cannot be read.
    let path = case("jxr", &[tile(1).compressed(4, vec![0; 8])], 0);
    let src = open(&p.registry, &path);
    assert!(format!("{:#}", src.read_page(0).err().unwrap()).contains("JPEG XR"));

    // Truncated data and garbage are errors, not crashes.
    let path = case("short", &[tile(1)], 0);
    let mut bytes = std::fs::read(&path).unwrap();
    let full = bytes.clone();
    for cut in [10, 100, 600, full.len() - 50] {
        bytes.truncate(cut);
        std::fs::write(&path, &bytes).unwrap();
        let _ = formats::open_with(&path, &p.registry).and_then(|s| s.read_page(0));
        bytes = full.clone();
    }
    std::fs::write(&path, b"ZISRAWFILE but nothing else").unwrap();
    assert!(err(&p.registry, &path).contains("CZI"));
    // A corrupt zstd stream fails on read.
    let path = case("badzstd", &[tile(1).compressed(5, vec![1, 2, 3, 4])], 0);
    assert!(open(&p.registry, &path).read_page(0).is_err());
}

#[test]
fn install_validation_checks_the_module() {
    use tifmrcview_lib::plugins::{package, registry::validate_package};
    let p = zeiss!();
    let dir = p.dir.path().join("zeiss");
    let m = package::read_installed(&dir, env!("CARGO_PKG_VERSION")).unwrap();
    validate_package(&dir, &m).unwrap();
    // A module that is not WebAssembly is refused.
    std::fs::write(dir.join("zeiss.wasm"), b"\0asm garbage").unwrap();
    assert!(validate_package(&dir, &m).is_err());
}

#[test]
fn needs_the_plugin_and_follows_the_switch() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.czi");
    czi::stack(&path, 8, 8, 1, 1, 0, 0);
    let none = Registry::new(Some(tmp.path().join("no-plugins")), "0.4.0");
    assert_eq!(err(&none, &path), "需要安装「Zeiss」插件才能打开 .czi 文件");
    assert!(!formats::is_supported_with(&path, &none));

    let p = zeiss!();
    assert!(formats::is_supported_with(&path, &p.registry));
    assert_eq!(p.registry.extensions(), ["czi", "lsm"]);
    p.registry.set_disabled(vec!["zeiss".into()]).unwrap();
    assert!(err(&p.registry, &path).contains("已停用"));
    assert!(!formats::is_supported_with(&path, &p.registry));
    p.registry.set_disabled(vec![]).unwrap();
    assert!(formats::open_with(&path, &p.registry).is_ok());
}

#[test]
fn pages_decode_in_parallel_threads() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("par.czi");
    czi::stack(&path, 128, 96, 2, 8, 6, 0);
    let img = OpenImage::new(1, open(&p.registry, &path), usize::MAX);
    let img = Arc::new(img);
    std::thread::scope(|s| {
        for k in 0..4 {
            let img = img.clone();
            s.spawn(move || {
                for page in (0..16).map(|i| (i + k * 3) % 16) {
                    let planes = img.source.read_page(page).unwrap();
                    let (c, z) = (page % 2, page / 2);
                    assert_eq!(planes[0].data.get_f64(5) as u16, czi::pattern(c, z, 5, 0));
                }
            });
        }
    });
}

#[test]
fn large_metadata_stays_within_the_fuel_budget() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bigxml.czi");
    // About 3 MB of XML, more than real ZEN files carry.
    czi::stack(&path, 16, 16, 1, 1, 0, 40_000);
    let t = std::time::Instant::now();
    let src = open(&p.registry, &path);
    eprintln!(
        "opened a CZI with {} KB of XML in {:?}",
        std::fs::metadata(&path).unwrap().len() >> 10,
        t.elapsed()
    );
    let all = src.info().metadata.last().unwrap();
    assert_eq!(all.name, "ZEN 全部参数");
    assert!(all.items.len() > 80_000);
}

/// Open and read every page, like the prefetcher (two readers).
/// `cargo test --release --test czi_plugin -- --ignored --nocapture`
#[test]
#[ignore]
fn benchmark_open_and_read_all_pages() {
    let p = zeiss!();
    let tmp = tempfile::tempdir().unwrap();
    for (name, comp, extra) in [
        ("uncompressed", 0, 0),
        ("uncompressed", 0, 2000),
        ("zstd", 5, 2000),
        ("zstd hi/lo", 6, 2000),
    ] {
        let path = tmp.path().join(format!("{comp}-{extra}.czi"));
        czi::stack(&path, 1024, 1024, 2, 20, comp, extra);
        let mb = std::fs::metadata(&path).unwrap().len() >> 20;
        let mut t_open = std::time::Duration::MAX;
        let mut read = std::time::Duration::MAX;
        for _ in 0..6 {
            let t = std::time::Instant::now();
            let img = OpenImage::new(1, open(&p.registry, &path), usize::MAX);
            t_open = t_open.min(t.elapsed());
            let t = std::time::Instant::now();
            let n = img.info().page_count;
            std::thread::scope(|s| {
                for k in 0..2 {
                    let img = &img;
                    s.spawn(move || {
                        (k..n)
                            .step_by(2)
                            .for_each(|page| drop(img.page(page).unwrap()))
                    });
                }
            });
            read = read.min(t.elapsed());
        }
        println!("{name:>13} ({mb} MB, 40 pages, extra XML {extra:>4}): open {t_open:>10.2?}, read all {read:>10.2?}");
    }
}
