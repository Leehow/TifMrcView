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
        mag_group: 0,
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
    let nv = angles.len();
    let mut sino = vec![0.0; stack.len()];
    for y in 0..h {
        for i in 0..nv {
            sino[(y * nv + i) * w..(y * nv + i + 1) * w]
                .copy_from_slice(&stack[(i * h + y) * w..(i * h + y + 1) * w]);
        }
    }
    println!(
        "resample={:.3}s shape={w}x{h}x{z}",
        now.elapsed().as_secs_f64()
    );
    let op = recon::Operator::new(w, z, &angles, 0.0)?;
    for method in [recon::Method::Wbp, recon::Method::Sirt] {
        let p = recon::Params {
            thickness: z,
            method,
            ..Default::default()
        };
        let now = Instant::now();
        let cpu = recon::cpu(&sino, h, &op, &p, &|_, _| {})?;
        println!(
            "{method:?} CPU={:.3}s residuals={:?}",
            now.elapsed().as_secs_f64(),
            cpu.residuals
        );
        if let Some((dev, name)) = tifmrcview_lib::ctf_gpu::device() {
            let now = Instant::now();
            let gpu = tifmrcview_lib::ctf_gpu::guarded(|| {
                tomo::recon_gpu::reconstruct(dev, &sino, h, &op, &p, &|_, _| {})
            })??;
            let diff = gpu
                .data
                .iter()
                .zip(&cpu.data)
                .map(|(a, b)| (*a as f64 - *b as f64).powi(2))
                .sum::<f64>();
            let norm = cpu.data.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
            println!(
                "{method:?} {name}={:.3}s relL2={:.6}",
                now.elapsed().as_secs_f64(),
                (diff / norm.max(1e-20)).sqrt()
            );
        } else {
            println!("GPU: unavailable (skipped)");
        }
        if method == recon::Method::Wbp {
            if let Some(path) = args.get(5) {
                output::write_mrc(
                    path,
                    &cpu.data,
                    [w, h, z],
                    output::pixel_a(&img) * bin as f64,
                    true,
                )?;
                output::export(&a, path)?;
                let xy = cpu
                    .data
                    .chunks(w * z)
                    .flat_map(|row| row[(z / 2) * w..(z / 2 + 1) * w].iter().copied())
                    .collect::<Vec<_>>();
                let xz = cpu.data[(h / 2) * w * z..(h / 2 + 1) * w * z].to_vec();
                save_png(&format!("{path}.xy.png"), &xy, w, h)?;
                save_png(&format!("{path}.xz.png"), &xz, w, z)?;
            }
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
