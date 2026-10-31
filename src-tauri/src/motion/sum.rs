//! Corrected sums. The smooth part of each frame's motion, the global
//! shift plus the model's constant term, is applied as a Fourier phase
//! shift (exact, no interpolation loss); only the spatially varying rest of
//! the local model (well under a pixel to a few pixels) is resampled
//! bilinearly before the transform. Order: u(x) = f(x + l(x)), then
//! out(x) = u(x + s); l is smooth, so l(x + s) ≈ l(x). Frames add up in
//! Fourier space: even, odd and, with an exposure, Grant & Grigorieff dose
//! weights. The weighted sum is normalised by sqrt(Σq²) times sqrt(N),
//! keeping DC and the noise scale of the raw sum. Shifts wrap around at the
//! edges, so a border as wide as the largest shift is set to the mean.
//! Nyquist bins of a fractionally shifted axis are dropped (no real form).
use super::{check_cancel, dose, fourier, input::Spool, local::Model};
use anyhow::Result;
use rayon::prelude::*;
#[derive(Clone)]
pub struct Sums {
    pub unweighted: Vec<f32>,
    pub even: Vec<f32>,
    pub odd: Vec<f32>,
    pub weighted: Option<Vec<f32>>,
}

/// Full warp by the global shift and the whole local model (reference).
pub fn warp(v: &[f32], w: usize, h: usize, shift: [f64; 2], model: &Model, time: f64) -> Vec<f32> {
    warp_poly(v, w, h, shift, &model.spatial(time))
}

fn warp_poly(v: &[f32], w: usize, h: usize, shift: [f64; 2], c: &[[f32; 6]; 2]) -> Vec<f32> {
    let mut out = vec![0.0; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let yy = 2.0 * y as f64 / (h - 1) as f64 - 1.0;
        for (x, o) in row.iter_mut().enumerate() {
            let xx = 2.0 * x as f64 / (w - 1) as f64 - 1.0;
            let s = [1.0, xx, yy, xx * xx, xx * yy, yy * yy];
            let d: [f64; 2] = std::array::from_fn(|k| {
                shift[k] + (0..6).map(|j| c[k][j] as f64 * s[j]).sum::<f64>()
            });
            *o = sample(v, w, h, x as f64 + d[0], y as f64 + d[1]);
        }
    });
    out
}

/// Bilinear sample with zeros outside the image, each of the four
/// neighbours counted only when it is inside (as the GPU grid sampler does).
fn sample(v: &[f32], w: usize, h: usize, x: f64, y: f64) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
    let at = |xi: f64, yi: f64| {
        if xi < 0.0 || yi < 0.0 || xi >= w as f64 || yi >= h as f64 {
            0.0
        } else {
            v[yi as usize * w + xi as usize]
        }
    };
    (1.0 - fy) * ((1.0 - fx) * at(x0, y0) + fx * at(x0 + 1.0, y0)) + fy * ((1.0 - fx) * at(x0, y0 + 1.0) + fx * at(x0 + 1.0, y0 + 1.0))
}

/// The spatially varying part of the model at `time` and the whole-frame
/// shift (global + constant term) applied in Fourier space.
pub fn split(model: &Model, time: f64, global: [f64; 2]) -> ([[f32; 6]; 2], [f64; 2]) {
    let mut c = model.spatial(time);
    let s = [global[0] + c[0][0] as f64, global[1] + c[1][0] as f64];
    c[0][0] = 0.0;
    c[1][0] = 0.0;
    (c, s)
}

/// Whether the polynomial moves any pixel by more than 1/100 px.
pub fn varies(c: &[[f32; 6]; 2]) -> bool {
    c.iter().any(|k| k[1..].iter().map(|v| v.abs()).sum::<f32>() > 0.01)
}

/// Nyquist bins have no conjugate partner for a fractional shift; on an
/// axis shifted by a fractional amount they are dropped.
pub fn nyquist(i: usize, n: usize) -> bool {
    n > 1 && i == n / 2
}

pub fn fractional(s: f64) -> bool {
    (s - s.round()).abs() > 1e-6
}

/// Signed frequency index of bin i of n.
pub fn freq(i: usize, n: usize) -> f64 {
    if i <= n / 2 { i as f64 } else { i as f64 - n as f64 }
}

/// Sets a border of width `b` to the mean of the inside.
pub fn fill_border(v: &mut [f32], w: usize, h: usize, b: usize) {
    if b == 0 || 2 * b >= w.min(h) {
        return;
    }
    let inside = |x: usize, y: usize| x >= b && y >= b && x < w - b && y < h - b;
    let (mut sum, mut n) = (0.0f64, 0usize);
    for y in b..h - b {
        for x in b..w - b {
            sum += v[y * w + x] as f64;
            n += 1;
        }
    }
    let mean = (sum / n as f64) as f32;
    for y in 0..h {
        for x in 0..w {
            if !inside(x, y) {
                v[y * w + x] = mean;
            }
        }
    }
}

/// Border width covering the largest whole-frame shift.
pub fn border(shifts: &[[f64; 2]]) -> usize {
    shifts.iter().map(|s| s[0].abs().max(s[1].abs())).fold(0.0, f64::max).ceil() as usize
}

/// Dose weight factor per (frame exposure start, dose) at critical exposure `n`.
pub fn q(n: f64, start: f64, d: f64) -> f64 {
    let rate = 1.0 / (2.0 * n);
    let z = d * rate;
    (-start * rate).exp() * if z > 0.0 { -(-z).exp_m1() / z } else { 1.0 }
}

pub fn exposure(counts: &[usize], dose_per_fraction: f64, pre: f64) -> Vec<(f64, f64)> {
    let mut dose = pre;
    counts
        .iter()
        .map(|&n| {
            let d = dose_per_fraction * n as f64;
            let p = (dose, d);
            dose += d;
            p
        })
        .collect()
}
pub fn cpu(
    sp: &Spool,
    global: &[[f64; 2]],
    model: &Model,
    exposures: Option<&[(f64, f64)]>,
    pixel: f64,
    kv: f64,
    progress: &(dyn Fn(&str, usize, usize) + Sync),
) -> Result<Sums> {
    let (w, h) = (sp.w, sp.h);
    let (fw, fh) = (w.next_power_of_two(), h.next_power_of_two());
    let fft = fourier::Cpu::new(fw, fh);
    let critical: Vec<f64> = if exposures.is_some() {
        (0..fw * fh)
            .into_par_iter()
            .map(|i| {
                let k = (freq(i % fw, fw) / fw as f64).hypot(freq(i / fw, fh) / fh as f64) / pixel;
                dose::critical(k, kv)
            })
            .collect::<Result<_>>()?
    } else {
        vec![]
    };
    // Accumulators: even, odd, weighted (re, im), and Σq².
    let mut acc = vec![vec![0.0f32; fw * fh]; if exposures.is_some() { 6 } else { 4 }];
    let mut norm = vec![0.0f64; if exposures.is_some() { fw * fh } else { 0 }];
    let mut shifts = vec![];
    for (i, &g) in global.iter().enumerate() {
        check_cancel()?;
        let v = sp.read(i)?;
        let (c, s) = split(model, sp.times[i], g);
        shifts.push(s);
        let v = if varies(&c) { warp_poly(&v, w, h, [0.0; 2], &c) } else { v };
        let mean = (v.par_iter().map(|&a| a as f64).sum::<f64>() / v.len() as f64) as f32;
        let mut re = vec![mean; fw * fh];
        let mut im = vec![0.0; fw * fh];
        for y in 0..h {
            re[y * fw..y * fw + w].copy_from_slice(&v[y * w..(y + 1) * w]);
        }
        drop(v);
        fft.transform(&mut re, &mut im, false);
        let drop = s.map(fractional);
        let (ax, ay) = (std::f64::consts::TAU * s[0] / fw as f64, std::f64::consts::TAU * s[1] / fh as f64);
        let [er, ei, rest @ ..] = &mut acc[..] else { unreachable!() };
        let (sr, si) = if i % 2 == 0 { (er, ei) } else { let [or, oi, ..] = rest else { unreachable!() }; (or, oi) };
        let shifted: Vec<(f32, f32)> = (0..fw * fh)
            .into_par_iter()
            .map(|j| {
                if (drop[0] && nyquist(j % fw, fw)) || (drop[1] && nyquist(j / fw, fh)) {
                    return (0.0, 0.0);
                }
                let (sn, cs) = (ax * freq(j % fw, fw) + ay * freq(j / fw, fh)).sin_cos();
                let (a, b) = (re[j] as f64, im[j] as f64);
                ((a * cs - b * sn) as f32, (a * sn + b * cs) as f32)
            })
            .collect();
        sr.par_iter_mut().zip(si.par_iter_mut()).zip(&shifted).for_each(|((r, ii), (a, b))| {
            *r += a;
            *ii += b;
        });
        if let Some(exposure) = exposures {
            let (start, d) = exposure[i];
            let [_, _, _, _, wr, wi] = &mut acc[..] else { unreachable!() };
            wr.par_iter_mut()
                .zip(wi.par_iter_mut())
                .zip(norm.par_iter_mut())
                .zip(&shifted)
                .enumerate()
                .for_each(|(j, (((r, ii), nn), (a, b)))| {
                    let q = q(critical[j], start, d);
                    *r += a * q as f32;
                    *ii += b * q as f32;
                    *nn += q * q;
                });
        }
        progress("变形求和 CPU", i + 1, sp.len());
    }
    let b = border(&shifts);
    let back = |mut re: Vec<f32>, mut im: Vec<f32>| -> Vec<f32> {
        fft.transform(&mut re, &mut im, true);
        let mut out: Vec<f32> = (0..h).flat_map(|y| re[y * fw..y * fw + w].iter().copied()).collect();
        fill_border(&mut out, w, h, b);
        out
    };
    let mut acc = acc.into_iter();
    let (er, ei, or, oi) = (acc.next().unwrap(), acc.next().unwrap(), acc.next().unwrap(), acc.next().unwrap());
    let weighted = if exposures.is_some() {
        let (mut wr, mut wi) = (acc.next().unwrap(), acc.next().unwrap());
        let scale = (sp.len() as f64).sqrt();
        wr.par_iter_mut().zip(wi.par_iter_mut()).zip(&norm).for_each(|((r, i), n)| {
            let f = (scale / n.sqrt().max(1e-12)) as f32;
            *r *= f;
            *i *= f;
        });
        Some(back(wr, wi))
    } else {
        None
    };
    let ur: Vec<f32> = er.iter().zip(&or).map(|(a, b)| a + b).collect();
    let ui: Vec<f32> = ei.iter().zip(&oi).map(|(a, b)| a + b).collect();
    let unweighted = back(ur, ui);
    Ok(Sums { unweighted, even: back(er, ei), odd: back(or, oi), weighted })
}
