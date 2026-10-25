//! Developer tool: decode a file and print metadata, checksums and timings.
//! `cargo run --release --example probe -- file.tif [...]`
use std::time::Instant;

use tifview_lib::image::{OpenImage, PlaneData};
use tifview_lib::{formats, render, with_plane};

fn main() {
    for path in std::env::args().skip(1) {
        let t0 = Instant::now();
        let src = match formats::open(path.as_ref()) {
            Ok(s) => s,
            Err(e) => {
                println!("{path}: ERROR {e}");
                continue;
            }
        };
        let img = OpenImage::new(1, src, usize::MAX);
        let info = img.info().clone();
        let t_open = t0.elapsed();
        println!(
            "{}: {} {}x{} C{} Z{} T{} {:?} rgb={} px={:?}{} channels={:?}",
            info.name, info.format, info.width, info.height, info.size_c, info.size_z, info.size_t,
            info.dtype, info.is_rgb, info.pixel_size, info.unit,
            info.channels.iter().map(|c| (c.name.clone(), c.color)).collect::<Vec<_>>()
        );
        let t1 = Instant::now();
        let mut sums = vec![];
        for c in 0..info.size_c {
            let z = info.size_z / 2;
            let (page, s) = info.locate(c, z, 0);
            let planes = img.page(page).unwrap();
            let p: &PlaneData = &planes[s].data;
            let sum: f64 = with_plane!(p, v => v.iter().map(|x| *x as f64).sum());
            sums.push(sum);
        }
        let t_page = t1.elapsed();
        let t2 = Instant::now();
        let n = info.page_count;
        let all: Vec<_> = {
            use rayon::prelude::*;
            (0..n).into_par_iter().map(|p| img.page(p).is_ok()).collect()
        };
        let t_all = t2.elapsed();
        let t3 = Instant::now();
        let req = render::RenderReq {
            token: 1, z: info.size_z / 2, t: 0,
            channels: info.channels.iter().enumerate().map(|(i, c)| render::ChanReq { index: i, color: c.color, min: None, max: None }).collect(),
            region: [0.0, 0.0, info.width as f64, info.height as f64],
            out_w: 1600.min(info.width), out_h: (1600.min(info.width) * info.height / info.width).max(1),
            gamma: 1.0, invert: false,
        };
        let out = render::render(&img, &req).unwrap();
        let t_render = t3.elapsed();
        let t4 = Instant::now();
        let th = render::thumbnail(&img, 128).unwrap();
        println!(
            "   mid-z sums per channel {:?}; open {:?}, first page {:?}, all {} pages {:?} ok={}, render {:?} ({} B), thumb {:?} ({} B)",
            sums, t_open, t_page, n, t_all, all.iter().all(|&b| b), t_render, out.len(), t4.elapsed(), th.len()
        );
        if let Ok(dir) = std::env::var("PROBE_PNG") {
            let w = u32::from_le_bytes(out[0..4].try_into().unwrap());
            let h = u32::from_le_bytes(out[4..8].try_into().unwrap());
            let nch = u32::from_le_bytes(out[8..12].try_into().unwrap()) as usize;
            let px = &out[12 + nch * 16..];
            let name = format!("{dir}/{}.ppm", info.name);
            let mut ppm = format!("P6 {w} {h} 255\n").into_bytes();
            for c in px.chunks_exact(4) { ppm.extend_from_slice(&c[..3]); }
            std::fs::write(name, ppm).unwrap();
        }
    }
}
