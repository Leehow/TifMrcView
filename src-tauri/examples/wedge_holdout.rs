//! Held-out-tilt validation of missing-wedge correction:
//! wedge_holdout STACK TLT LO HI [THICKNESS=128] [STEPS=2000] [CUBE=48] [OUT_DIR]
//!
//! STACK holds aligned views (one per section, in TLT order). The views
//! with tilts in [LO, HI] are left out of everything: the tomograms are
//! reconstructed (WBP) from the rest, every method is trained on those, and
//! each result is projected back to the left-out tilts and compared with
//! the measured views there (Fourier ring correlation in frequency bands,
//! middle half of the width, where rays stay inside the slab). Without
//! movie frames the halves are the kept views alternately.
//! Methods: WBP, WBP + Gaussian σ1, IsoNet on the one tomogram, halves,
//! halves then ICECREAM-style equivariance for the last third
//! (HOLDOUT_EQUI_FROM, default 0.67), full ICECREAM with HOLDOUT_FULL_ICECREAM=1.
use anyhow::{bail, Context, Result};
use std::{path::Path, time::Instant};
use tifmrcview_lib::{fft::Fft, formats, image::OpenImage, tomo::recon, wedge};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: wedge_holdout STACK TLT LO HI [THICKNESS=128] [STEPS=2000] [CUBE=48] [OUT_DIR]";
    let (stack, tlt) = (args.get(1).context(usage)?, args.get(2).context(usage)?);
    let lo: f64 = args.get(3).context(usage)?.parse()?;
    let hi: f64 = args.get(4).context(usage)?.parse()?;
    let z: usize = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(128);
    let steps: usize = args.get(6).map(|s| s.parse()).transpose()?.unwrap_or(2000);
    let cube: usize = args.get(7).map(|s| s.parse()).transpose()?.unwrap_or(48);
    let out = args.get(8);

    let img = OpenImage::new(1, formats::open(Path::new(stack))?, 1 << 30);
    let info = img.info();
    let (w, h, n) = (info.width, info.height, info.size_z);
    let angles: Vec<f64> = std::fs::read_to_string(tlt)?.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    if angles.len() != n {
        bail!("{n} views but {} tilts", angles.len());
    }
    let mut views = Vec::with_capacity(n * w * h);
    for v in 0..n {
        let (page, s) = info.locate(0, v, 0);
        let p = img.page(page)?;
        let p = &p[s.min(p.len() - 1)];
        views.extend((0..w * h).map(|i| p.data.get_f64(i) as f32));
    }
    let held: Vec<usize> = (0..n).filter(|&v| (lo..=hi).contains(&angles[v])).collect();
    let mut kept: Vec<usize> = (0..n).filter(|v| !held.contains(v)).collect();
    kept.sort_by(|&a, &b| angles[a].total_cmp(&angles[b]));
    if held.len() < 3 || kept.len() < 6 {
        bail!("need at least 3 left-out and 6 kept views ({} / {})", held.len(), kept.len());
    }
    println!("{w}×{h}, {n} views; leaving out {} ({lo}°..{hi}°), keeping {}, thickness {z}", held.len(), kept.len());

    // WBP of a set of views → volume X fastest, then Y, then Z (beam).
    let wbp = |set: &[usize]| -> Result<Vec<f32>> {
        let data: Vec<f32> = set.iter().flat_map(|&v| views[v * w * h..(v + 1) * w * h].iter().copied()).collect();
        let a: Vec<f64> = set.iter().map(|&v| angles[v]).collect();
        let g = recon::Geometry::new(w, z, &a, 0.0)?;
        let p = recon::Params { thickness: z, ..Default::default() };
        let mut yzx = vec![0f32; h * z * w];
        recon::reconstruct(&recon::Views { data: &data, w, h, views: set.len() }, &g, &p, &mut |y0, k, slab| {
            yzx[y0 * z * w..(y0 + k) * z * w].copy_from_slice(&slab);
            Ok(())
        }, &|_, _| {})?;
        Ok(to_zyx(&yzx, w, h, z))
    };
    let t0 = Instant::now();
    let full = wbp(&kept)?;
    let even: Vec<usize> = kept.iter().copied().step_by(2).collect();
    let odd: Vec<usize> = kept.iter().copied().skip(1).step_by(2).collect();
    let (a, b) = (wbp(&even)?, wbp(&odd)?);
    println!("WBP ×3 in {:.1} s", t0.elapsed().as_secs_f64());

    let dims = [w, h, z];
    let kept_tilts: Vec<f32> = kept.iter().map(|&v| angles[v] as f32).collect();
    let range = [kept_tilts.iter().cloned().fold(f32::MAX, f32::min), kept_tilts.iter().cloned().fold(f32::MIN, f32::max)];
    let (dev, backend) = tifmrcview_lib::classify::device();
    println!("training on {backend}");
    let mut methods: Vec<(&str, Vec<f32>)> = vec![("WBP", full.clone())];
    let gauss = wedge::smooth(&wedge::Vol { n: dims, v: full.clone() }, 1.0).v;
    methods.push(("WBP + Gauss σ1", gauss));
    let t1 = Instant::now();
    let single_cfg = wedge::TrainConfig { cube: 32, iterations: 3, steps: steps / 3, smooth: 1.0, ..Default::default() };
    methods.push(("IsoNet (one tomogram)", wedge::train_and_apply(&dev, &full, dims, range, &single_cfg, |_| {})?));
    println!("IsoNet in {:.0} s", t1.elapsed().as_secs_f64());
    let t2 = Instant::now();
    let halves_cfg = wedge::TrainConfig { cube, iterations: 3, steps: steps / 3, ..Default::default() };
    methods.push(("halves", wedge::train_halves_and_apply(&dev, &a, &b, dims, &kept_tilts, &halves_cfg, |_| {})?));
    println!("halves in {:.0} s", t2.elapsed().as_secs_f64());
    // Halves first, equivariance for the last third (half the pairs per step).
    let from: f32 = std::env::var("HOLDOUT_EQUI_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0.67);
    let t3 = Instant::now();
    let warm = wedge::TrainConfig { equivariant: 2.0, equi_from: from, ..halves_cfg.clone() };
    methods.push(("halves → equivariant", wedge::train_halves_and_apply(&dev, &a, &b, dims, &kept_tilts, &warm, |_| {})?));
    println!("halves → equivariant (from {from}) in {:.0} s", t3.elapsed().as_secs_f64());
    if std::env::var("HOLDOUT_FULL_ICECREAM").is_ok() {
        let t4 = Instant::now();
        let cfg = wedge::TrainConfig { equivariant: 2.0, ..halves_cfg };
        methods.push(("ICECREAM", wedge::train_halves_and_apply(&dev, &a, &b, dims, &kept_tilts, &cfg, |_| {})?));
        println!("ICECREAM in {:.0} s", t4.elapsed().as_secs_f64());
    }

    // Project each result to the left-out tilts and compare.
    let held_angles: Vec<f64> = held.iter().map(|&v| angles[v]).collect();
    let g = recon::Geometry::new(w, z, &held_angles, 0.0)?;
    let side = (w / 2).min(h).next_power_of_two() / if (w / 2).min(h).is_power_of_two() { 1 } else { 2 };
    let (x0, y0) = ((w - side) / 2, (h - side) / 2);
    const BANDS: [(f64, f64); 4] = [(0.02, 0.05), (0.05, 0.1), (0.1, 0.2), (0.2, 0.3)];
    println!("\nFRC with the measured left-out views (cycles/px bands {BANDS:?}), crop {side}² at ({x0}, {y0}):");
    for (name, vol) in &methods {
        let proj = project(vol, &g, w, h, z);
        let mut sums = vec![[0f64; 3]; BANDS.len()];
        for (k, &v) in held.iter().enumerate() {
            let crop = |src: &[f32]| -> Vec<f32> { (0..side * side).map(|i| src[(y0 + i / side) * w + x0 + i % side]).collect() };
            let p = crop(&proj[k * w * h..(k + 1) * w * h]);
            let m = crop(&views[v * w * h..(v + 1) * w * h]);
            frc_add(&p, &m, side, &BANDS, &mut sums);
        }
        let frc: Vec<String> = sums.iter().map(|[pq, pp, qq]| format!("{:.3}", pq / (pp * qq).sqrt().max(1e-30))).collect();
        println!("  {name:<24} {}", frc.join("  "));
        if let Some(dir) = out {
            let file = format!("{dir}/{}.xz.png", name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>());
            let xz: Vec<f32> = (0..z).flat_map(|zz| vol[(zz * h + h / 2) * w..(zz * h + h / 2 + 1) * w].iter().copied()).collect();
            save_png(&file, &xz, w, z)?;
        }
    }
    Ok(())
}

/// [y][z][x] → [z][y][x].
fn to_zyx(v: &[f32], w: usize, h: usize, z: usize) -> Vec<f32> {
    let mut out = vec![0f32; v.len()];
    for y in 0..h {
        for zz in 0..z {
            out[(zz * h + y) * w..(zz * h + y + 1) * w].copy_from_slice(&v[(y * z + zz) * w..(y * z + zz + 1) * w]);
        }
    }
    out
}

/// Projections [view][y][x] of a volume [z][y][x] for the views of `g`.
fn project(vol: &[f32], g: &recon::Geometry, w: usize, h: usize, z: usize) -> Vec<f32> {
    use rayon::prelude::*;
    let rows: Vec<Vec<f32>> = (0..h)
        .into_par_iter()
        .map(|y| {
            let slice: Vec<f32> = (0..z).flat_map(|zz| vol[(zz * h + y) * w..(zz * h + y + 1) * w].iter().copied()).collect();
            recon::forward(g, &slice)
        })
        .collect();
    let nv = g.views();
    let mut out = vec![0f32; nv * h * w];
    for (y, sino) in rows.iter().enumerate() {
        for v in 0..nv {
            out[(v * h + y) * w..(v * h + y + 1) * w].copy_from_slice(&sino[v * w..(v + 1) * w]);
        }
    }
    out
}

/// Adds Σ Re(P Q*), Σ |P|², Σ |Q|² per ring band of two side² images.
fn frc_add(p: &[f32], q: &[f32], side: usize, bands: &[(f64, f64)], sums: &mut [[f64; 3]]) {
    let f = |v: &[f32]| -> (Vec<f32>, Vec<f32>) {
        let m = v.iter().sum::<f32>() / v.len() as f32;
        let fft = Fft::new(side);
        let (mut re, mut im): (Vec<f32>, Vec<f32>) = (v.iter().map(|x| x - m).collect(), vec![0f32; v.len()]);
        for r in 0..side {
            fft.run(&mut re[r * side..(r + 1) * side], &mut im[r * side..(r + 1) * side]);
        }
        for c in 0..side {
            let (mut cr, mut ci): (Vec<f32>, Vec<f32>) = ((0..side).map(|r| re[r * side + c]).collect(), (0..side).map(|r| im[r * side + c]).collect());
            fft.run(&mut cr, &mut ci);
            for r in 0..side {
                re[r * side + c] = cr[r];
                im[r * side + c] = ci[r];
            }
        }
        (re, im)
    };
    let ((pr, pi), (qr, qi)) = (f(p), f(q));
    let fr = |i: usize| if i <= side / 2 { i as f64 } else { i as f64 - side as f64 };
    for i in 0..side * side {
        let k = fr(i % side).hypot(fr(i / side)) / side as f64;
        if let Some(b) = bands.iter().position(|&(a, b)| (a..b).contains(&k)) {
            sums[b][0] += (pr[i] * qr[i] + pi[i] * qi[i]) as f64;
            sums[b][1] += (pr[i] * pr[i] + pi[i] * pi[i]) as f64;
            sums[b][2] += (qr[i] * qr[i] + qi[i] * qi[i]) as f64;
        }
    }
}

fn save_png(path: &str, v: &[f32], w: usize, h: usize) -> Result<()> {
    let mut sorted = v.to_vec();
    sorted.sort_by(f32::total_cmp);
    let (lo, hi) = (sorted[sorted.len() / 100], sorted[sorted.len() * 99 / 100]);
    let pixels = v.iter().map(|&v| ((v - lo) / (hi - lo).max(1e-20) * 255.0).clamp(0.0, 255.0) as u8).collect();
    image::GrayImage::from_raw(w as u32, h as u32, pixels).context("PNG shape")?.save(path)?;
    Ok(())
}
