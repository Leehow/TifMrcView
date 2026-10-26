//! Temporary: the zen WebAssembly plugin against the built-in CZI reader it
//! replaces, on the same files (same info, same pixels), plus a benchmark.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::czi::{self, Block};
use tifmrcview_lib::formats::{self, czi::CziSource};
use tifmrcview_lib::image::{OpenImage, Source};

fn same(native: &dyn Source, plugin: &dyn Source, what: &str) {
    let (a, b) = (native.info(), plugin.info());
    assert_eq!(
        (a.width, a.height, a.size_c, a.size_z, a.size_t, a.dtype, a.is_rgb, a.samples_per_page, a.page_count),
        (b.width, b.height, b.size_c, b.size_z, b.size_t, b.dtype, b.is_rgb, b.samples_per_page, b.page_count),
        "{what}"
    );
    assert_eq!(a.pixel_size, b.pixel_size, "{what}");
    assert_eq!(a.unit, b.unit, "{what}");
    let ch = |i: &tifmrcview_lib::image::ImageInfo| i.channels.iter().map(|c| (c.name.clone(), c.color)).collect::<Vec<_>>();
    assert_eq!(ch(a), ch(b), "{what}");
    for page in 0..a.page_count {
        let (pa, pb) = (native.read_page(page).unwrap(), plugin.read_page(page).unwrap());
        assert_eq!(pa.len(), pb.len(), "{what} page {page}");
        for (x, y) in pa.iter().zip(&pb) {
            assert_eq!(x.data.as_bytes(), y.data.as_bytes(), "{what} page {page}");
        }
    }
}

#[test]
fn plugin_matches_the_native_reader() {
    let Some(p) = common::install(&["zen"]) else { return };
    let tmp = tempfile::tempdir().unwrap();
    let mut files = vec![];
    for (i, comp) in [0, 5, 6].into_iter().enumerate() {
        let path = tmp.path().join(format!("s{i}.czi"));
        czi::stack(&path, 50, 30, 3, 4, comp, 20);
        files.push(path);
    }
    // A mosaic with overlapping tiles and an RGB file.
    let t = |x, m, v: u16| Block::tile(1, x, 3, 10, 5, 0, 0, 0, m, [v; 50].iter().flat_map(|v| v.to_le_bytes()).collect());
    let path = tmp.path().join("mosaic.czi");
    czi::write(&path, &[t(4, 1, 2), t(0, 0, 1), t(12, 2, 3)], "<ImageDocument/>", 0);
    files.push(path);
    let path = tmp.path().join("rgb.czi");
    czi::write(&path, &[Block::tile(9, 0, 0, 3, 2, 0, 0, 0, 0, (0..24).collect())], "<ImageDocument/>", 0);
    files.push(path);

    for f in &files {
        let native = CziSource::open(f).unwrap();
        let plugin = formats::open_with(f, &p.registry).unwrap();
        same(&native, plugin.as_ref(), &f.display().to_string());
    }
}

fn read_all(src: Box<dyn Source>) -> Duration {
    let img = OpenImage::new(1, src, usize::MAX);
    let t = Instant::now();
    // Like the prefetcher: pages on parallel readers.
    let n = img.info().page_count;
    std::thread::scope(|s| {
        for k in 0..2 {
            let img = &img;
            s.spawn(move || {
                for page in (k..n).step_by(2) {
                    img.page(page).unwrap();
                }
            });
        }
    });
    t.elapsed()
}

fn best<T>(runs: usize, mut f: impl FnMut() -> (Duration, T)) -> Duration {
    (0..runs).map(|_| f().0).min().unwrap()
}

/// `cargo test --release --test czi_vs_native -- --ignored --nocapture`
#[test]
#[ignore]
fn benchmark_open_and_read_all_pages() {
    let Some(p) = common::install(&["zen"]) else { return };
    let tmp = tempfile::tempdir().unwrap();
    for (name, comp, extra) in [("uncompressed", 0, 0), ("uncompressed", 0, 2000), ("zstd", 5, 2000), ("zstd hi/lo", 6, 2000)] {
        let path = tmp.path().join(format!("{comp}-{extra}.czi"));
        czi::stack(&path, 1024, 1024, 2, 20, comp, extra);
        let xml_kb = czi::xml(&[("a", "#FF00FF00"), ("b", "#FFFF0000")], 1e-7, 1e-7, extra).len() >> 10;
        let size = std::fs::metadata(&path).unwrap().len() >> 20;
        let open_native = |path: &Path| -> Box<dyn Source> { Box::new(CziSource::open(path).unwrap()) };
        let open_plugin = |path: &Path| formats::open_with(path, &p.registry).unwrap();
        // Warm the page cache and the compiled module.
        read_all(open_native(&path));
        read_all(open_plugin(&path));
        for (who, open) in [("native", &open_native as &dyn Fn(&Path) -> Box<dyn Source>), ("plugin", &open_plugin)] {
            let t_open = best(5, || {
                let t = Instant::now();
                let s = open(&path);
                (t.elapsed(), s)
            });
            let t_read = best(5, || (read_all(open(&path)), ()));
            println!("{name:>13} ({size} MB, 40 pages, {xml_kb:>3} KB XML) {who:>6}: open {:>7.2} ms, read all {:>8.2} ms", t_open.as_secs_f64() * 1e3, t_read.as_secs_f64() * 1e3);
        }
    }
}
