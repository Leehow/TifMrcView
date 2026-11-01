//! Offline real-data smoke: tomo_run INPUT [BIN=4] [THICKNESS=64] [AXIS=90]
//! [OUTPUT.mrc]. Prints alignment and matched CPU/GPU timings and relative error.
use anyhow::{Context, Result};
use std::{path::Path, sync::atomic::Ordering, time::Instant};
use tifmrcview_lib::{
    formats,
    image::OpenImage,
    tomo::{self, output, recon},
};
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .context("usage: tomo_run INPUT [BIN=4] [Z=64] [AXIS=90] [OUTPUT.mrc]")?;
    let bin = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(4);
    let z = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(64);
    let axis = args.get(4).map(|s| s.parse()).transpose()?;
    let img = OpenImage::new(1, formats::open(Path::new(path))?, 128 << 20);
    eprintln!(
        "input {}x{}x{}, bin {bin}",
        img.info().width,
        img.info().height,
        img.info().size_z
    );
    tomo::CANCEL.store(false, Ordering::Relaxed);
    let req = tomo::AlignReq {
        token: 1,
        c: 0,
        t: 0,
        angles: vec![],
        start: None,
        step: None,
        axis,
        bin,
        sweep: true,
        patch: None,
        excluded: vec![],
        groups: None,
        previous: None,
    };
    let a = tomo::align(&img, &req, &|_, _| {})?;
    println!(
        "alignment residual={:.6} px axis={:.3} grid={}x{} patch={} bin={} time={:.3}s flagged={}",
        a.residual,
        a.axis,
        a.patch.nx,
        a.patch.ny,
        a.patch.size,
        a.bin,
        a.ms / 1000.0,
        a.views.iter().filter(|v| v.flagged).count()
    );
    println!(
        "chosen tracking={:.3}s backend={} cachedCPU_pair={:.3}ms GPU_pair={:?}ms",
        a.tracks.ms / 1000.0,
        a.tracks.backend,
        a.tracks.cpu_pair_ms,
        a.tracks.gpu_pair_ms
    );
    let mut angles = vec![0.0; a.views.len()];
    for v in &a.views {
        angles[v.file_index] = v.angle;
    }
    let working = tomo::read_stack(&img, 0, 0, a.bin, &angles)?;
    for backend in ["cpu", "gpu"] {
        let tr = tomo::track::track_backend(&working, a.axis, a.patch, backend, &|_, _| {})?;
        println!(
            "tracking forced_{backend}={:.3}s actual={} pairCPU={:.3}ms pairGPU={:?}ms",
            tr.ms / 1000.0,
            tr.backend,
            tr.cpu_pair_ms,
            tr.gpu_pair_ms
        );
    }
    if let Some(path) = args.get(5) {
        std::fs::write(
            format!("{path}.alignment.json"),
            serde_json::to_vec_pretty(&a)?,
        )?;
    }
    let now = Instant::now();
    let (stack, w, h, angles) = output::aligned(&img, &a, a.bin, 0, 0, &|_, _| {})?;
    println!("resample={:.3}s shape={w}x{h}x{z}", now.elapsed().as_secs_f64());
    let g = recon::Geometry::new(w, z, &angles, 0.0)?;
    let views = recon::Views { data: &stack, w, h, views: angles.len() };
    for method in [recon::Method::Wbp, recon::Method::Sirt, recon::Method::Sart] {
        let mut results = vec![];
        for backend in ["cpu", "gpu"] {
            let p = recon::Params { thickness: z, method, iterations: if method == recon::Method::Sart { 3 } else { 15 }, lambda: if method == recon::Method::Sart { 0.5 } else { 1.0 }, backend: backend.into(), ..Default::default() };
            let mut out = vec![0.0f32; h * z * w];
            let now = Instant::now();
            let name = match recon::reconstruct(&views, &g, &p, &mut |y0, n, slab| {
                out[y0 * z * w..(y0 + n) * z * w].copy_from_slice(&slab);
                Ok(())
            }, &|_, _| {}) {
                Ok(name) => name,
                Err(e) => {
                    println!("{method:?} {backend}: {e}");
                    continue;
                }
            };
            println!("{method:?} {name}={:.3}s", now.elapsed().as_secs_f64());
            results.push(out);
        }
        if let [cpu, gpu] = &results[..] {
            let diff = gpu.iter().zip(cpu).map(|(a, b)| (*a as f64 - *b as f64).powi(2)).sum::<f64>();
            let norm = cpu.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
            println!("{method:?} relL2={:.6}", (diff / norm.max(1e-20)).sqrt());
        }
        if let (Some(path), Some(data)) = (args.get(5), results.first()) {
            let stem = format!("{path}.{}", format!("{method:?}").to_lowercase());
            // [y][z][x] slabs: XY section at mid Z, XZ slice at mid Y.
            let xy: Vec<f32> = (0..h).flat_map(|y| data[(y * z + z / 2) * w..(y * z + z / 2 + 1) * w].iter().copied()).collect();
            let xz = data[(h / 2) * z * w..(h / 2 + 1) * z * w].to_vec();
            save_png(&format!("{stem}.xy.png"), &xy, w, h)?;
            save_png(&format!("{stem}.xz.png"), &xz, w, z)?;
        }
    }
    Ok(())
}

fn save_png(path: &str, v: &[f32], w: usize, h: usize) -> Result<()> {
    let mut sorted = v.to_vec();
    sorted.sort_by(f32::total_cmp);
    let lo = sorted[sorted.len() / 100];
    let hi = sorted[sorted.len() * 99 / 100];
    let pixels = v
        .iter()
        .map(|&v| ((v - lo) / (hi - lo).max(1e-20) * 255.0).clamp(0.0, 255.0) as u8)
        .collect();
    image::GrayImage::from_raw(w as u32, h as u32, pixels)
        .context("PNG shape")?
        .save(path)?;
    Ok(())
}
