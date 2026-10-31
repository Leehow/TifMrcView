//! Bilinear polynomial warping and G&G Fourier sums. Odd/even corrected
//! groups remain unweighted. Weighted output is normalized by sqrt(sum q²)
//! times sqrt(N), preserving DC and the noise scale of the raw summed movie.
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

pub fn warp(v: &[f32], w: usize, h: usize, shift: [f64; 2], model: &Model, time: f64) -> Vec<f32> {
    let c = model.spatial(time);
    let mut out = vec![0.0; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let yy = 2.0 * y as f64 / (h - 1) as f64 - 1.0;
        for (x, o) in row.iter_mut().enumerate() {
            let xx = 2.0 * x as f64 / (w - 1) as f64 - 1.0;
            let s = [1.0, xx, yy, xx * xx, xx * yy, yy * yy];
            let d: [f64; 2] = std::array::from_fn(|k| {
                shift[k] + (0..6).map(|j| c[k][j] as f64 * s[j]).sum::<f64>()
            });
            *o = crate::tomo::track::sample(v, w, h, x as f64 + d[0], y as f64 + d[1]);
        }
    });
    out
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
    let mut even = vec![0.0; w * h];
    let mut odd = vec![0.0; w * h];
    let (fw, fh) = (w.next_power_of_two(), h.next_power_of_two());
    let fft = fourier::Cpu::new(fw, fh);
    let mut sr = if exposures.is_some() {
        vec![0.0; fw * fh]
    } else {
        vec![]
    };
    let mut si = vec![0.0; sr.len()];
    let mut norm = vec![0.0f64; sr.len()];
    let critical: Vec<f64> = if exposures.is_some() {
        (0..fw * fh)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % fw, i / fw);
                let k = (x.min(fw - x) as f64 / fw as f64).hypot(y.min(fh - y) as f64 / fh as f64)
                    / pixel;
                dose::critical(k, kv)
            })
            .collect::<Result<_>>()?
    } else {
        vec![]
    };
    for (i, &g) in global.iter().enumerate() {
        check_cancel()?;
        let v = sp.read(i)?;
        let corrected = warp(&v, w, h, g, model, sp.times[i]);
        drop(v);
        let out = if i % 2 == 0 { &mut even } else { &mut odd };
        out.par_iter_mut()
            .zip(&corrected)
            .for_each(|(a, b)| *a += b);
        if let Some(exposure) = exposures {
            let mut re = vec![0.0; fw * fh];
            let mut im = vec![0.0; fw * fh];
            for y in 0..h {
                re[y * fw..y * fw + w].copy_from_slice(&corrected[y * w..(y + 1) * w]);
            }
            fft.transform(&mut re, &mut im, false);
            let (start, d) = exposure[i];
            sr.par_iter_mut()
                .zip(si.par_iter_mut())
                .zip(norm.par_iter_mut())
                .enumerate()
                .for_each(|(j, ((r, ii), norm))| {
                    let rate = 1.0 / (2.0 * critical[j]);
                    let z = d * rate;
                    let q = (-start * rate).exp() * if z > 0.0 { -(-z).exp_m1() / z } else { 1.0 };
                    *r += re[j] * q as f32;
                    *ii += im[j] * q as f32;
                    *norm += q * q;
                });
        }
        progress("变形求和 CPU", i + 1, sp.len());
    }
    let weighted = if exposures.is_some() {
        let scale = (sp.len() as f64).sqrt();
        sr.par_iter_mut()
            .zip(si.par_iter_mut())
            .zip(&norm)
            .for_each(|((r, i), n)| {
                let f = (scale / n.sqrt().max(1e-12)) as f32;
                *r *= f;
                *i *= f;
            });
        fft.transform(&mut sr, &mut si, true);
        Some(
            (0..h)
                .flat_map(|y| sr[y * fw..y * fw + w].iter().copied())
                .collect(),
        )
    } else {
        None
    };
    let unweighted = even.par_iter().zip(&odd).map(|(a, b)| a + b).collect();
    Ok(Sums {
        unweighted,
        even,
        odd,
        weighted,
    })
}
