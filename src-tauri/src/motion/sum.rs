//! Corrected sums, built tile by tile (see `tiles`): every frame's tiles
//! are moved by the frame's motion at their centres, whole pixels by where
//! they are cut, the fraction by a Fourier phase shift, so nothing is
//! resampled and no spatial frequency is damped. Tiles add up over frames
//! in Fourier space: even, odd and, with an exposure, Grant & Grigorieff
//! dose weights; the weighted sum is normalised by sqrt(Σq²) times sqrt(N),
//! keeping DC and the noise scale of the raw sum. Nyquist bins of a
//! fractionally shifted axis are dropped (a fractional shift has no real
//! form there). `warp` is the resampling reference the tests compare with.
use super::{check_cancel, dose, fourier, input::Spool, local::Model, tiles::Plan};
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
    let plan = Plan::new(sp.w, sp.h);
    let t = plan.t;
    let fft = fourier::Cpu::new(t, t);
    let critical: Vec<f64> = if exposures.is_some() {
        (0..t * t)
            .map(|i| dose::critical((freq(i % t, t)).hypot(freq(i / t, t)) / t as f64 / pixel, kv))
            .collect::<Result<_>>()?
    } else {
        vec![]
    };
    let n = plan.len();
    let zeros = || vec![vec![0.0f32; t * t]; n];
    // Per tile: even (re, im), odd (re, im), weighted (re, im).
    let mut acc: Vec<[Vec<Vec<f32>>; 2]> = (0..if exposures.is_some() { 3 } else { 2 }).map(|_| [zeros(), zeros()]).collect();
    let mut norm = vec![0.0f64; if exposures.is_some() { t * t } else { 0 }];
    for (i, &g) in global.iter().enumerate() {
        check_cancel()?;
        let frame = sp.read(i)?;
        let mean = (frame.par_iter().map(|&a| a as f64).sum::<f64>() / frame.len() as f64) as f32;
        let q: Vec<f64> = match exposures {
            Some(e) => critical.iter().map(|&c| q(c, e[i].0, e[i].1)).collect(),
            None => vec![],
        };
        // Each tile's shifted spectrum.
        let spectra: Vec<(Vec<f32>, Vec<f32>)> = (0..n)
            .into_par_iter()
            .map(|k| {
                let s = plan.shift(k, model, sp.times[i], g);
                let mut re = vec![0.0; t * t];
                plan.cut(&frame, k, s, mean, &mut re);
                let mut im = vec![0.0; t * t];
                fft.transform(&mut re, &mut im, false);
                let f = [s[0] - s[0].round(), s[1] - s[1].round()];
                let drop = f.map(fractional);
                let (ax, ay) = (std::f64::consts::TAU * f[0] / t as f64, std::f64::consts::TAU * f[1] / t as f64);
                for j in 0..t * t {
                    if (drop[0] && nyquist(j % t, t)) || (drop[1] && nyquist(j / t, t)) {
                        re[j] = 0.0;
                        im[j] = 0.0;
                        continue;
                    }
                    let (sn, cs) = (ax * freq(j % t, t) + ay * freq(j / t, t)).sin_cos();
                    let (a, b) = (re[j] as f64, im[j] as f64);
                    re[j] = (a * cs - b * sn) as f32;
                    im[j] = (a * sn + b * cs) as f32;
                }
                (re, im)
            })
            .collect();
        drop(frame);
        let parity = i % 2;
        for (k, (re, im)) in spectra.iter().enumerate() {
            let [ar, ai] = &mut acc[parity];
            ar[k].iter_mut().zip(re).for_each(|(a, b)| *a += b);
            ai[k].iter_mut().zip(im).for_each(|(a, b)| *a += b);
            if exposures.is_some() {
                let [wr, wi] = &mut acc[2];
                for j in 0..t * t {
                    wr[k][j] += re[j] * q[j] as f32;
                    wi[k][j] += im[j] * q[j] as f32;
                }
            }
        }
        for (nn, qq) in norm.iter_mut().zip(&q) {
            *nn += qq * qq;
        }
        progress("求和 CPU", i + 1, sp.len());
    }
    let back = |re: &[Vec<f32>], im: &[Vec<f32>], scale: Option<&[f32]>| -> Vec<f32> {
        let tiles: Vec<Vec<f32>> = re
            .par_iter()
            .zip(im)
            .map(|(r, m)| {
                let (mut r, mut m) = (r.clone(), m.clone());
                if let Some(sc) = scale {
                    r.iter_mut().zip(sc).for_each(|(a, b)| *a *= b);
                    m.iter_mut().zip(sc).for_each(|(a, b)| *a *= b);
                }
                fft.transform(&mut r, &mut m, true);
                r
            })
            .collect();
        plan.blend(&tiles)
    };
    let [er, ei] = &acc[0];
    let [or, oi] = &acc[1];
    let ur: Vec<Vec<f32>> = er.iter().zip(or).map(|(a, b)| a.iter().zip(b).map(|(x, y)| x + y).collect()).collect();
    let ui: Vec<Vec<f32>> = ei.iter().zip(oi).map(|(a, b)| a.iter().zip(b).map(|(x, y)| x + y).collect()).collect();
    let weighted = exposures.map(|_| {
        let n = (sp.len() as f64).sqrt();
        let scale: Vec<f32> = norm.iter().map(|v| (n / v.sqrt().max(1e-12)) as f32).collect();
        let [wr, wi] = &acc[2];
        back(wr, wi, Some(&scale))
    });
    Ok(Sums { unweighted: back(&ur, &ui, None), even: back(er, ei, None), odd: back(or, oi, None), weighted })
}
