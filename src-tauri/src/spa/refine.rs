//! 3D: an ab initio model and gold-standard refinement.
//!
//! Ab initio: the model starts from particles at random orientations; then,
//! batch by batch, particles are aligned to the current model (global
//! search) and inserted into a running reconstruction that forgets old
//! batches, while the resolution limit rises — a stochastic, hard-assignment
//! version of the SGD ab initio of Punjani et al. (Nat Methods 2017).
//!
//! Refinement: the particles are split into two halves that never meet;
//! each half is aligned to its own map (global search, then local searches
//! around the previous orientation with finer steps), maps are
//! reconstructed with an FSC-derived Wiener term, and the FSC between the
//! halves sets the next resolution limit (Scheres & Chen, Nat Methods 2012;
//! Henderson et al., Structure 2012). Independent implementation.

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::data::{self, Coeffs, Optics, Particle, Stack};
use super::fourier::{self, Half};
use super::geom::{self, Mat};
use super::score::{self, Refs};
use super::volume::{self, Accum, Projector, Pt};
use super::{check_cancel, post, Progress, Rng};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InitialParams {
    pub symmetry: String,
    pub iterations: usize,
    pub batch: usize,
    pub res_start: f64,
    pub res_end: f64,
    pub box_px: usize,
    pub seed: u64,
}

impl Default for InitialParams {
    fn default() -> Self {
        InitialParams { symmetry: "C1".into(), iterations: 80, batch: 300, res_start: 35.0, res_end: 12.0, box_px: 64, seed: 7 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RefineParams {
    pub symmetry: String,
    /// Low-pass of the starting map, Å.
    pub res_init: f64,
    pub max_iterations: usize,
    /// Global-search iterations before the local ones.
    pub global_iterations: usize,
    pub global_step: f64,
    pub seed: u64,
}

impl Default for RefineParams {
    fn default() -> Self {
        RefineParams { symmetry: "C1".into(), res_init: 30.0, max_iterations: 12, global_iterations: 2, global_step: 7.5, seed: 11 }
    }
}

/// A map: side and pixel size.
#[derive(Clone)]
pub struct Map {
    pub b: usize,
    pub px: f64,
    pub v: Vec<f32>,
}

/// Particle coefficients on `half` with shifts `offs` around their current shift.
fn coeffs_for(stack: &Stack, parts: &[Particle], chunk: &[usize], optics: &Optics, half: &Half, noise: &[f32], offs: &[[f32; 2]]) -> Coeffs {
    let fts = data::transforms(stack, chunk);
    let ps: Vec<&Particle> = chunk.iter().map(|&i| &parts[i]).collect();
    let sh: Vec<[f32; 2]> = ps.iter().map(|q| q.shift).collect();
    data::coeffs(optics, stack.pixel_a, stack.b, half, noise, &ps, &fts, &sh, offs)
}

/// Global search: every orientation of `grid` for each particle of `chunk`.
fn global(c: &Coeffs, offs: &[[f32; 2]], refs: &Refs, grid: &[Mat], ratio: f32, parts: &mut [Particle], chunk: &[usize]) -> Result<()> {
    let best = score::best(c, offs.len(), refs)?;
    for (j, &i) in chunk.iter().enumerate() {
        let b = best[j];
        let q = &mut parts[i];
        let a = geom::angles(&grid[b.o as usize]);
        q.euler = [a[0] as f32, a[1] as f32, a[2] as f32];
        let o = offs[b.s as usize];
        q.shift = [q.shift[0] + o[0] * ratio, q.shift[1] + o[1] * ratio];
        q.score = b.score;
    }
    Ok(())
}

/// Local search around each particle's orientation (CPU, per particle).
fn local(c: &Coeffs, offs: &[[f32; 2]], proj: &Projector, half: &Half, deltas: &[Mat], ratio: f32, parts: &mut [Particle], chunk: &[usize]) {
    let k = c.k;
    let s = offs.len();
    let found: Vec<(Mat, usize, f32)> = chunk
        .par_iter()
        .enumerate()
        .map(|(j, &i)| {
            let q = &parts[i];
            let r0 = geom::euler(q.euler[0] as f64, q.euler[1] as f64, q.euler[2] as f64);
            let mut v = vec![0f32; 2 * k];
            let c2 = &c.c2[j * k..(j + 1) * k];
            let mut best = (r0, 0usize, f32::NEG_INFINITY);
            for d in deltas {
                let r = geom::mul(d, &r0);
                proj.slice(&r, half, &mut v);
                let mut norm = 0f32;
                for t in 0..k {
                    norm += c2[t] * (v[2 * t] * v[2 * t] + v[2 * t + 1] * v[2 * t + 1]);
                }
                let norm = norm.max(1e-20).sqrt();
                for si in 0..s {
                    let row = &c.x[(j * s + si) * 2 * k..(j * s + si + 1) * 2 * k];
                    let mut cross = 0f32;
                    for t in 0..2 * k {
                        cross += row[t] * v[t];
                    }
                    let sc = cross / norm;
                    if sc > best.2 {
                        best = (r, si, sc);
                    }
                }
            }
            best
        })
        .collect();
    for (j, &i) in chunk.iter().enumerate() {
        let (r, si, sc) = found[j];
        let a = geom::angles(&r);
        let q = &mut parts[i];
        q.euler = [a[0] as f32, a[1] as f32, a[2] as f32];
        let o = offs[si];
        q.shift = [q.shift[0] + o[0] * ratio, q.shift[1] + o[1] * ratio];
        q.score = sc;
    }
}

/// Inserts particles at their orientations (and symmetry mates).
fn insert(acc: &mut Accum, c: &Coeffs, half: &Half, sym: &[Mat], parts: &[Particle], chunk: &[usize]) {
    let k = c.k;
    let per = (4_000_000 / (k * sym.len()).max(1)).max(1);
    for (bi, sub) in chunk.chunks(per).enumerate() {
        let pts: Vec<Pt> = sub
            .par_iter()
            .enumerate()
            .flat_map_iter(|(j, &i)| {
                let jj = bi * per + j;
                let q = &parts[i];
                let r = geom::euler(q.euler[0] as f64, q.euler[1] as f64, q.euler[2] as f64);
                let mut out = Vec::with_capacity(k * sym.len());
                acc.points(&r, sym, half, &c.x[jj * 2 * k..(jj + 1) * 2 * k], &c.c2[jj * k..(jj + 1) * k], &mut out);
                out
            })
            .collect();
        acc.insert(&pts);
    }
}

/// Working box for a resolution: Nyquist at most res / 1.2, a power of two.
fn box_for(b: usize, px: f64, res: f64) -> usize {
    let want = (2.4 * b as f64 * px / res).ceil() as usize;
    want.next_power_of_two().clamp(32.min(b), b)
}

/// Fourier radius (shells of the stack box) of a resolution.
fn radius(b: usize, px: f64, res: f64) -> f64 {
    b as f64 * px / res
}

/// Low-pass with a cosine edge at `res` Å, then a soft spherical mask.
pub fn lowpass(v: &[f32], b: usize, px: f64, res: f64) -> Vec<f32> {
    let (mut re, mut im) = fourier::ft3(b, v);
    let r = b as f64 * px / res;
    re.par_chunks_mut(b * b).zip(im.par_chunks_mut(b * b)).enumerate().for_each(|(z, (pr, pi))| {
        for y in 0..b {
            for x in 0..b {
                let (fx, fy, fz) = (fourier::freq(x, b), fourier::freq(y, b), fourier::freq(z, b));
                let d = ((fx * fx + fy * fy + fz * fz) as f64).sqrt();
                let f = data::soft(d, r, 3.0);
                pr[y * b + x] *= f;
                pi[y * b + x] *= f;
            }
        }
    });
    fourier::ift3(b, &re, &im)
}

/// Resamples a map to box `b` at pixel `px` (trilinear in real space),
/// keeping the centre; for references from elsewhere.
pub fn fit_map(v: &[f32], [w, h, d]: [usize; 3], px_in: f64, b: usize, px: f64) -> Vec<f32> {
    let (cx, cy, cz) = (w as f64 / 2.0, h as f64 / 2.0, d as f64 / 2.0);
    let c = b as f64 / 2.0;
    let at = |x: isize, y: isize, z: isize| -> f32 {
        if x < 0 || y < 0 || z < 0 || x >= w as isize || y >= h as isize || z >= d as isize {
            0.0
        } else {
            v[(z as usize * h + y as usize) * w + x as usize]
        }
    };
    let mut out = vec![0f32; b * b * b];
    out.par_chunks_mut(b * b).enumerate().for_each(|(z, plane)| {
        for y in 0..b {
            for x in 0..b {
                let s = px / px_in;
                let (fx, fy, fz) = ((x as f64 - c) * s + cx, (y as f64 - c) * s + cy, (z as f64 - c) * s + cz);
                let (x0, y0, z0) = (fx.floor(), fy.floor(), fz.floor());
                let (tx, ty, tz) = ((fx - x0) as f32, (fy - y0) as f32, (fz - z0) as f32);
                let (x0, y0, z0) = (x0 as isize, y0 as isize, z0 as isize);
                let mut a = 0f32;
                for (dz, wz) in [(0, 1.0 - tz), (1, tz)] {
                    for (dy, wy) in [(0, 1.0 - ty), (1, ty)] {
                        for (dx, wx) in [(0, 1.0 - tx), (1, tx)] {
                            a += wx * wy * wz * at(x0 + dx, y0 + dy, z0 + dz);
                        }
                    }
                }
                plane[y * b + x] = a;
            }
        }
    });
    out
}

/// The ab initio model at the working box (`box_px`, Fourier-cropped).
pub fn initial(stack: &Stack, parts: &mut [Particle], ids: &[usize], optics: &Optics, diameter_a: f64, p: &InitialParams, progress: Progress) -> Result<Map> {
    if ids.len() < 50 {
        bail!("颗粒太少（至少 50 个）");
    }
    let b = stack.b;
    let px = stack.pixel_a;
    let bw = p.box_px.clamp(16, b).next_power_of_two().min(b);
    let ratio = (b / bw) as f32;
    let pxw = px * (b / bw) as f64;
    let sym = geom::symmetry(&p.symmetry)?;
    let noise = data::noise_spectrum(stack, ids, 2000);
    let mask_r = (diameter_a / 2.0 / pxw * 1.1).min(bw as f64 * 0.45);
    let pad = volume::pad_for(bw);
    let mut rng = Rng(p.seed.max(1));
    let mut order = ids.to_vec();
    shuffle(&mut order, &mut rng);
    // Random orientations for a first blob.
    let full = Half::new(bw, bw as f64 / 2.0 - 1.0);
    let start = &order[..order.len().min(1000)];
    for &i in start {
        let r = random_rotation(&mut rng);
        let a = geom::angles(&r);
        parts[i].euler = [a[0] as f32, a[1] as f32, a[2] as f32];
        parts[i].shift = [0.0, 0.0];
    }
    let mut acc = Accum::new(bw, pad);
    let c = coeffs_for(stack, parts, start, optics, &full, &noise, &[]);
    insert(&mut acc, &c, &full, &sym, parts, start);
    let res_end = p.res_end.max(2.2 * pxw);
    let mut model = volume_from(&acc, bw, pxw, p.res_start, mask_r);
    let iters = p.iterations.max(1);
    let batch = p.batch.clamp(50, ids.len());
    let mut pos = 0;
    let max_shift = (diameter_a * 0.15 / px) as f32;
    for it in 0..iters {
        check_cancel()?;
        let f = (it as f64 / (iters as f64 * 0.8)).min(1.0);
        let res = 1.0 / (1.0 / p.res_start + f * (1.0 / res_end - 1.0 / p.res_start));
        let half = Half::new(bw, radius(b, px, res).min(bw as f64 / 2.0 - 1.0));
        let step = if res > 20.0 { 15.0 } else { 10.0 };
        let grid = geom::grid(step);
        let proj = Projector::new(&model, bw, pad);
        let refs = slices(&proj, &grid, &half);
        // The next batch, wrapping around the shuffled order.
        let mut chunk = Vec::with_capacity(batch);
        for _ in 0..batch {
            chunk.push(order[pos]);
            pos = (pos + 1) % order.len();
        }
        let offs = data::offsets(2);
        let c = coeffs_for(stack, parts, &chunk, optics, &half, &noise, &offs);
        global(&c, &offs, &refs, &grid, ratio, parts, &chunk)?;
        clamp_shifts(parts, &chunk, max_shift);
        let c = coeffs_for(stack, parts, &chunk, optics, &full, &noise, &[]);
        let mut fresh = Accum::new(bw, pad);
        insert(&mut fresh, &c, &full, &sym, parts, &chunk);
        // Forget old batches: the model follows the recent alignments.
        acc.scale(0.85);
        acc.add(&fresh);
        model = volume_from(&acc, bw, pxw, res, mask_r);
        progress("初始模型", it + 1, iters + 2);
    }
    // Two passes over every particle at the final resolution.
    for pass in 0..2 {
        check_cancel()?;
        let half = Half::new(bw, radius(b, px, res_end).min(bw as f64 / 2.0 - 1.0));
        let grid = geom::grid(if pass == 0 { 10.0 } else { 7.5 });
        let proj = Projector::new(&model, bw, pad);
        let refs = slices(&proj, &grid, &half);
        let mut acc = Accum::new(bw, pad);
        let offs = data::offsets(2);
        for chunk in ids.chunks(512) {
            check_cancel()?;
            let c = coeffs_for(stack, parts, chunk, optics, &half, &noise, &offs);
            global(&c, &offs, &refs, &grid, ratio, parts, chunk)?;
            clamp_shifts(parts, chunk, max_shift);
            let c = coeffs_for(stack, parts, chunk, optics, &full, &noise, &[]);
            insert(&mut acc, &c, &full, &sym, parts, chunk);
        }
        model = volume_from(&acc, bw, pxw, res_end, mask_r);
        progress("初始模型", iters + pass + 1, iters + 2);
    }
    Ok(Map { b: bw, px: pxw, v: model })
}

fn clamp_shifts(parts: &mut [Particle], chunk: &[usize], max: f32) {
    for &i in chunk {
        let s = parts[i].shift;
        let l = s[0].hypot(s[1]);
        if l > max {
            parts[i].shift = [s[0] * max / l, s[1] * max / l];
        }
    }
}

fn volume_from(acc: &Accum, b: usize, px: f64, res: f64, mask_r: f64) -> Vec<f32> {
    let v = acc.reconstruct(None);
    let mut v = lowpass(&v, b, px, res);
    volume::mask_sphere(&mut v, b, mask_r, 4.0);
    v
}

fn slices(proj: &Projector, grid: &[Mat], half: &Half) -> Refs {
    let k = half.len();
    let rows: Vec<Vec<f32>> = grid
        .par_iter()
        .map(|r| {
            let mut out = vec![0f32; 2 * k];
            proj.slice(r, half, &mut out);
            out
        })
        .collect();
    let mut refs = Refs::new(k);
    for r in &rows {
        refs.push(r);
    }
    refs
}

fn shuffle(v: &mut [usize], rng: &mut Rng) {
    for i in (1..v.len()).rev() {
        v.swap(i, rng.below(i + 1));
    }
}

/// A uniformly random rotation (from a random unit quaternion).
pub fn random_rotation(rng: &mut Rng) -> Mat {
    let g = [rng.gauss(), rng.gauss(), rng.gauss(), rng.gauss()];
    let n = g.iter().map(|v| v * v).sum::<f64>().sqrt();
    let [w, x, y, z] = [g[0] / n, g[1] / n, g[2] / n, g[3] / n];
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Iteration {
    pub res: f64,
    pub box_px: usize,
    pub step: f64,
    pub global: bool,
}

pub struct Refined {
    pub half1: Map,
    pub half2: Map,
    pub full: Map,
    pub fsc: Vec<f64>,
    pub res: f64,
    pub history: Vec<Iteration>,
}

/// Gold-standard refinement from `start` (any box and pixel; it is
/// resampled). The final maps are at the stack's box and pixel.
pub fn refine(stack: &Stack, parts: &mut [Particle], ids: &[usize], optics: &Optics, diameter_a: f64, start: &Map, p: &RefineParams, progress: Progress) -> Result<Refined> {
    if ids.len() < 20 {
        bail!("颗粒太少（至少 20 个）");
    }
    let b = stack.b;
    let px = stack.pixel_a;
    let sym = geom::symmetry(&p.symmetry)?;
    let noise = data::noise_spectrum(stack, ids, 2000);
    // Half sets, fixed for the whole run.
    let mut rng = Rng(p.seed.max(1));
    let mut order = ids.to_vec();
    shuffle(&mut order, &mut rng);
    let (h1, h2) = order.split_at(order.len() / 2);
    let (mut h1, mut h2) = (h1.to_vec(), h2.to_vec());
    h1.sort_unstable();
    h2.sort_unstable();
    for &i in &h1 {
        parts[i].half = 1;
    }
    for &i in &h2 {
        parts[i].half = 2;
    }
    // Starting maps at the stack box, low-passed (the same for both halves).
    let start_b = if start.b == b && (start.px - px).abs() < 1e-3 * px {
        start.v.clone()
    } else if (start.b as f64 * start.px - b as f64 * px).abs() < 1e-3 * b as f64 * px {
        fourier::resample3(&start.v, start.b, b)
    } else {
        fit_map(&start.v, [start.b; 3], start.px, b, px)
    };
    let mut res = p.res_init;
    let base = lowpass(&start_b, b, px, res);
    let mut maps = [base.clone(), base];
    let mut history = vec![];
    let mask_r = (diameter_a / 2.0 / px * 1.15).min(b as f64 * 0.45);
    let max_shift = (diameter_a * 0.2 / px) as f32;
    let mut step = p.global_step;
    let mut best_res = f64::INFINITY;
    let mut stalls = 0;
    let iters = p.max_iterations.max(p.global_iterations + 1);
    let mut accs: Vec<Accum> = vec![];
    for it in 0..iters {
        check_cancel()?;
        let is_global = it < p.global_iterations;
        // Search a little beyond the current resolution.
        let r_search = radius(b, px, res) * 1.15 + 2.0;
        let bw = box_for(b, px, res / 1.15).max(32.min(b));
        let ratio = (b / bw) as f32;
        let pad = volume::pad_for(bw);
        let half = Half::new(bw, r_search.min(bw as f64 / 2.0 - 1.0));
        let full = Half::new(bw, bw as f64 / 2.0 - 1.0);
        // Angular step from the resolution: half the angle `res` spans at the particle edge.
        let want = (res / (std::f64::consts::PI * diameter_a) * 90.0).clamp(0.5, 7.5);
        if !is_global {
            step = want.min(step);
        }
        history.push(Iteration { res, box_px: bw, step, global: is_global });
        accs = vec![];
        for (hi, set) in [&h1, &h2].into_iter().enumerate() {
            let m = if bw == b { maps[hi].clone() } else { fourier::resample3(&maps[hi], b, bw) };
            let mut m = m;
            volume::mask_sphere(&mut m, bw, mask_r * bw as f64 / b as f64, 4.0);
            let proj = Projector::new(&m, bw, pad);
            let mut acc = Accum::new(bw, pad);
            let grid = if is_global { geom::grid(step) } else { vec![] };
            let refs = if is_global { Some(slices(&proj, &grid, &half)) } else { None };
            let deltas = geom::local(step * 2.0, 2);
            let offs = data::offsets(if is_global { 3 } else { 1 });
            let total = set.len();
            for (ci, chunk) in set.chunks(512).enumerate() {
                check_cancel()?;
                let c = coeffs_for(stack, parts, chunk, optics, &half, &noise, &offs);
                match &refs {
                    Some(refs) => global(&c, &offs, refs, &grid, ratio, parts, chunk)?,
                    None => local(&c, &offs, &proj, &half, &deltas, ratio, parts, chunk),
                }
                clamp_shifts(parts, chunk, max_shift);
                let c = coeffs_for(stack, parts, chunk, optics, &full, &noise, &[]);
                insert(&mut acc, &c, &full, &sym, parts, chunk);
                progress("三维精修", it * 2000 + hi * 1000 + (ci + 1) * 512 * 1000 / total.max(1), iters * 2000 + 1000);
            }
            accs.push(acc);
        }
        // FSC of the unfiltered half maps; the next references are the
        // half maps with the Wiener term from that FSC.
        let u1 = accs[0].reconstruct(None);
        let u2 = accs[1].reconstruct(None);
        let f = post::fsc(&u1, &u2, bw, mask_r * bw as f64 / b as f64);
        let new_res = post::resolution(&f, bw, px * (b / bw) as f64, 0.143);
        let v1 = accs[0].reconstruct(Some(&f));
        let v2 = accs[1].reconstruct(Some(&f));
        maps = [fourier::resample3(&v1, bw, b), fourier::resample3(&v2, bw, b)];
        if new_res < best_res * 0.98 {
            best_res = new_res;
            stalls = 0;
        } else {
            stalls += 1;
        }
        res = new_res.max(2.0 * px * 1.05);
        // Done when the resolution has stopped improving at the finest useful step.
        if it >= p.global_iterations + 2 && stalls >= 2 && step <= want * 1.01 {
            break;
        }
    }
    // Final maps at the full box from the last alignments.
    progress("三维精修 · 最终重构", iters * 2000, iters * 2000 + 1000);
    let pad = volume::pad_for(b);
    let full = Half::new(b, b as f64 / 2.0 - 1.0);
    let mut finals = vec![];
    for set in [&h1, &h2] {
        let mut acc = Accum::new(b, pad);
        for chunk in set.chunks(512) {
            check_cancel()?;
            let c = coeffs_for(stack, parts, chunk, optics, &full, &noise, &[]);
            insert(&mut acc, &c, &full, &sym, parts, chunk);
        }
        finals.push(acc);
    }
    // Unfiltered half maps and their FSC; the full map with the Wiener
    // term of the combined data.
    let v1 = finals[0].reconstruct(None);
    let v2 = finals[1].reconstruct(None);
    let f = post::fsc(&v1, &v2, b, mask_r);
    let res = post::resolution(&f, b, px, 0.143);
    let mut both = Accum::new(b, pad);
    both.add(&finals[0]);
    both.add(&finals[1]);
    let full_fsc: Vec<f64> = f.iter().map(|&x| (2.0 * x / (1.0 + x)).clamp(0.0, 1.0)).collect();
    let vf = both.reconstruct(Some(&full_fsc));
    drop(accs);
    Ok(Refined { half1: Map { b, px, v: v1 }, half2: Map { b, px, v: v2 }, full: Map { b, px, v: vf }, fsc: f, res, history })
}
