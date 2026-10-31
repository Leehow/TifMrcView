//! Reference-free 2D classification: classes start as averages of random
//! subsets at random in-plane angles; each iteration every particle is
//! compared with every class at every in-plane angle and nearby shifts
//! (CTF-weighted, whitened correlation, see `score`), assigned to the
//! best, and the classes are re-averaged in Fourier space with a Wiener
//! term. The resolution limit rises over the iterations (as in the
//! multi-reference alignment of Penczek et al., Ultramicroscopy 1992, and
//! RELION's 2D classification, Scheres 2012). Independent implementation.

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::data::{self, Coeffs, Optics, Particle, Stack};
use super::fourier::Half;
use super::plane::{self, Accum2, Projector2};
use super::score::{self, Refs};
use super::{check_cancel, Progress, Rng};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Class2dParams {
    pub classes: usize,
    pub iterations: usize,
    /// In-plane angular step, degrees.
    pub psi_step: f64,
    /// Resolution limits at the start and the end, Å.
    pub res_start: f64,
    pub res_end: f64,
    /// Largest shift from the box centre, Å.
    pub max_shift_a: f64,
    /// Working box (Fourier-cropped), pixels.
    pub box_px: usize,
    pub seed: u64,
}

impl Default for Class2dParams {
    fn default() -> Self {
        Class2dParams { classes: 20, iterations: 20, psi_step: 6.0, res_start: 30.0, res_end: 8.0, max_shift_a: 30.0, box_px: 64, seed: 1 }
    }
}

pub struct Class2dResult {
    pub b: usize,
    pub pixel_a: f64,
    /// Class averages sorted by size, b × b each.
    pub averages: Vec<Vec<f32>>,
    pub counts: Vec<usize>,
    /// Mean correlation of each class's particles.
    pub scores: Vec<f32>,
    /// Resolution of each average, Å: where the FRC of the averages of
    /// its even and odd particles falls below 0.143.
    pub res: Vec<f64>,
}

/// Runs the classification on `ids` (indices into `parts`), updating their
/// class2d, psi2d and shift2d.
pub fn run(stack: &Stack, parts: &mut [Particle], ids: &[usize], optics: &Optics, diameter_a: f64, p: &Class2dParams, progress: Progress) -> Result<Class2dResult> {
    if ids.len() < p.classes.max(1) * 3 {
        bail!("颗粒太少：每类至少需要 3 个");
    }
    let b = stack.b;
    let bw = p.box_px.clamp(16, b).next_power_of_two().min(b);
    let ratio = (b / bw) as f32;
    let px = stack.pixel_a;
    let pxw = px * (b / bw) as f64;
    let radius = (diameter_a / 2.0 / pxw * 1.1).min(bw as f64 * 0.45);
    let k_classes = p.classes.max(1);
    let psis = (360.0 / p.psi_step.clamp(1.0, 90.0)).round() as usize;
    let noise = data::noise_spectrum(stack, ids, 2000);
    let mut rng = Rng(p.seed.max(1));
    // Start: random classes and angles.
    for &i in ids {
        parts[i].class2d = rng.below(k_classes) as i32;
        parts[i].psi2d = (rng.below(psis) as f64 * 360.0 / psis as f64) as f32;
        parts[i].shift2d = [0.0, 0.0];
    }
    let max_shift = (p.max_shift_a / px) as f32;
    let full = Half::new(bw, bw as f64 / 2.0 - 1.0);
    let mut averages = vec![vec![0f32; bw * bw]; k_classes];
    let mut counts = vec![0usize; k_classes];
    let mut scores = vec![0f32; k_classes];
    let iters = p.iterations.max(1);
    let total = iters + 1;
    let mut halves: Vec<Accum2> = vec![];
    for it in 0..=iters {
        check_cancel()?;
        // Resolution: from res_start to res_end over the first two thirds.
        let f = (it as f64 / (iters as f64 * 0.67).max(1.0)).min(1.0);
        let res = 1.0 / (1.0 / p.res_start + f * (1.0 / p.res_end.max(2.0 * pxw) - 1.0 / p.res_start));
        let r_max = (bw as f64 * pxw / res).min(bw as f64 / 2.0 - 1.0);
        let half = Half::new(bw, r_max);
        let k = half.len();
        // References: every class at every angle.
        let refs = if it == 0 {
            None
        } else {
            let mut refs = Refs::new(k);
            let rows: Vec<Vec<f32>> = averages
                .par_iter()
                .flat_map_iter(|a| {
                    let pr = Projector2::new(a, bw, 2);
                    let half = &half;
                    (0..psis).map(move |j| {
                        let mut out = vec![0f32; 2 * k];
                        pr.rotated(j as f64 * 360.0 / psis as f64, half, &mut out);
                        out
                    })
                })
                .collect();
            for r in &rows {
                refs.push(r);
            }
            Some(refs)
        };
        let range = if it <= 2 { 3 } else { 1 };
        let offs = data::offsets(range);
        let mut acc = vec![Accum2::new(bw); k_classes];
        // In the last pass also the averages of even and odd particles.
        if it == iters {
            halves = vec![Accum2::new(bw); 2 * k_classes];
        }
        let mut sums = vec![0f64; k_classes];
        let block = 512;
        for (bi, chunk) in ids.chunks(block).enumerate() {
            check_cancel()?;
            let fts = data::transforms(stack, chunk);
            if let Some(refs) = &refs {
                let ps: Vec<&Particle> = chunk.iter().map(|&i| &parts[i]).collect();
                let sh: Vec<[f32; 2]> = ps.iter().map(|q| q.shift2d).collect();
                let c = data::coeffs(optics, px, b, &half, &noise, &ps, &fts, &sh, &offs);
                let best = score::best(&c, offs.len(), refs)?;
                for (j, &i) in chunk.iter().enumerate() {
                    let bst = best[j];
                    let q = &mut parts[i];
                    q.class2d = (bst.o as usize / psis) as i32;
                    q.psi2d = ((bst.o as usize % psis) as f64 * 360.0 / psis as f64) as f32;
                    let o = offs[bst.s as usize];
                    let mut s = [q.shift2d[0] + o[0] * ratio, q.shift2d[1] + o[1] * ratio];
                    let len = s[0].hypot(s[1]);
                    if len > max_shift {
                        s = [s[0] * max_shift / len, s[1] * max_shift / len];
                    }
                    q.shift2d = s;
                    q.score = bst.score;
                    sums[q.class2d as usize] += bst.score as f64;
                }
            }
            // Insert with the new assignments at full resolution.
            let ps: Vec<&Particle> = chunk.iter().map(|&i| &parts[i]).collect();
            let sh: Vec<[f32; 2]> = ps.iter().map(|q| q.shift2d).collect();
            let c = data::coeffs(optics, px, b, &full, &noise, &ps, &fts, &sh, &[]);
            insert_all(&mut acc, &c, &full, &ps);
            if !halves.is_empty() {
                let parity: Vec<usize> = chunk.iter().map(|&i| i % 2).collect();
                insert_halves(&mut halves, &c, &full, &ps, &parity);
            }
            progress("二维分类", it * 1000 + (bi + 1) * 1000 * block / ids.len().max(1), total * 1000);
        }
        // New averages; empty or tiny classes restart from a split of the largest.
        let r_avg = if it == iters { bw as f64 / 2.0 - 1.0 } else { r_max };
        averages = acc.par_iter().map(|a| a.average(0.2, r_avg, radius)).collect();
        counts = acc.iter().map(|a| a.count).collect();
        scores = (0..k_classes).map(|c| (sums[c] / counts[c].max(1) as f64) as f32).collect();
        // Centre the classes.
        for a in &mut averages {
            let d = plane::centre_of_mass(a, bw);
            if d[0].hypot(d[1]) > 0.5 {
                *a = plane::shift(a, bw, d);
            }
        }
        if it < iters {
            let min = (ids.len() / k_classes / 10).max(2);
            for c in 0..k_classes {
                if counts[c] < min {
                    let big = (0..k_classes).max_by_key(|&j| counts[j]).unwrap();
                    let src = averages[big].clone();
                    let pr = Projector2::new(&src, bw, 2);
                    // A rotated copy: the next iteration splits the big class.
                    let mut out = vec![0f32; 2 * full.len()];
                    pr.rotated(rng.below(360) as f64, &full, &mut out);
                    averages[c] = from_half(&full, &out);
                    counts[c] = counts[big] / 2;
                    counts[big] -= counts[c];
                }
            }
        }
        progress("二维分类", (it + 1) * 1000, total * 1000);
    }
    let res: Vec<f64> = (0..k_classes)
        .into_par_iter()
        .map(|c| {
            let (a, b) = (&halves[2 * c], &halves[2 * c + 1]);
            if a.count < 3 || b.count < 3 {
                return UNRESOLVED;
            }
            let r = bw as f64 / 2.0 - 1.0;
            frc_res(&a.average(0.2, r, radius), &b.average(0.2, r, radius), bw, pxw)
        })
        .collect();
    // Sort by size; renumber the particles.
    let mut order: Vec<usize> = (0..k_classes).collect();
    order.sort_by_key(|&c| std::cmp::Reverse(counts[c]));
    let mut new_id = vec![0i32; k_classes];
    for (n, &c) in order.iter().enumerate() {
        new_id[c] = n as i32;
    }
    for &i in ids {
        let c = parts[i].class2d;
        if c >= 0 {
            parts[i].class2d = new_id[c as usize];
        }
    }
    Ok(Class2dResult {
        b: bw,
        pixel_a: pxw,
        averages: order.iter().map(|&c| averages[c].clone()).collect(),
        counts: order.iter().map(|&c| counts[c]).collect(),
        scores: order.iter().map(|&c| scores[c]).collect(),
        res: order.iter().map(|&c| res[c]).collect(),
    })
}

/// Inserts a block's particles into their classes, in parallel over classes.
fn insert_all(acc: &mut [Accum2], c: &Coeffs, half: &Half, ps: &[&Particle]) {
    let k = c.k;
    acc.par_iter_mut().enumerate().for_each(|(cls, a)| {
        for (j, q) in ps.iter().enumerate() {
            if q.class2d as usize == cls {
                a.insert(q.psi2d as f64, half, &c.x[j * 2 * k..(j + 1) * 2 * k], &c.c2[j * k..(j + 1) * k]);
            }
        }
    });
}

/// Resolution of a class without signal (finite, so it survives JSON).
pub const UNRESOLVED: f64 = 999.0;

/// Like `insert_all`, into the even (`2·class`) or odd (`2·class + 1`) average.
fn insert_halves(acc: &mut [Accum2], c: &Coeffs, half: &Half, ps: &[&Particle], parity: &[usize]) {
    let k = c.k;
    acc.par_iter_mut().enumerate().for_each(|(slot, a)| {
        for (j, q) in ps.iter().enumerate() {
            if q.class2d as usize * 2 + parity[j] == slot {
                a.insert(q.psi2d as f64, half, &c.x[j * 2 * k..(j + 1) * 2 * k], &c.c2[j * k..(j + 1) * k]);
            }
        }
    });
}

/// Resolution (Å) where the Fourier ring correlation of two images falls
/// below 0.143; `UNRESOLVED` when it never rises above it.
pub fn frc_res(a: &[f32], b: &[f32], n: usize, px: f64) -> f64 {
    let fft = crate::fft::Fft::new(n);
    let (ar, ai) = super::fourier::ft2(&fft, n, a);
    let (br, bi) = super::fourier::ft2(&fft, n, b);
    let shells = n / 2;
    let (mut x, mut p, mut q) = (vec![0f64; shells], vec![0f64; shells], vec![0f64; shells]);
    for y in 0..n {
        for xx in 0..n {
            let (fx, fy) = (super::fourier::freq(xx, n), super::fourier::freq(y, n));
            let r = ((fx * fx + fy * fy) as f64).sqrt().round() as usize;
            if r < shells {
                let j = y * n + xx;
                x[r] += (ar[j] * br[j] + ai[j] * bi[j]) as f64;
                p[r] += (ar[j] * ar[j] + ai[j] * ai[j]) as f64;
                q[r] += (br[j] * br[j] + bi[j] * bi[j]) as f64;
            }
        }
    }
    match (1..shells).find(|&r| x[r] / (p[r] * q[r]).sqrt().max(1e-30) < 0.143) {
        Some(1) => UNRESOLVED,
        Some(r) => n as f64 * px / (r as f64 - 0.5),
        None => 2.0 * px,
    }
}

/// The classes to keep by default: those whose average resolves within
/// `slack` × the best class's resolution (among classes holding at least
/// 1 % of the particles), as junk and blurred classes resolve much less.
pub fn auto_select(counts: &[usize], res: &[f64], slack: f64) -> Vec<usize> {
    let total: usize = counts.iter().sum();
    let big = |c: usize| counts[c] * 100 >= total;
    let best = (0..counts.len()).filter(|&c| big(c)).map(|c| res[c]).fold(f64::INFINITY, f64::min);
    if best >= UNRESOLVED {
        return vec![];
    }
    (0..counts.len()).filter(|&c| big(c) && res[c] <= best * slack).collect()
}

/// Real-space image from half-plane coefficients.
fn from_half(half: &Half, v: &[f32]) -> Vec<f32> {
    let b = half.b;
    let (mut re, mut im) = (vec![0f32; b * b], vec![0f32; b * b]);
    for (j, &[kx, ky]) in half.k.iter().enumerate() {
        let i = super::fourier::index(ky as i32, b) * b + super::fourier::index(kx as i32, b);
        re[i] = v[2 * j];
        im[i] = v[2 * j + 1];
        let m = super::fourier::index(-(ky as i32), b) * b + super::fourier::index(-(kx as i32), b);
        re[m] = v[2 * j];
        im[m] = -v[2 * j + 1];
    }
    super::fourier::ift2(&crate::fft::Fft::new(b), b, &re, &im)
}
