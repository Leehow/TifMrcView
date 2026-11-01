//! Single-axis reconstruction with projectors computed on the fly: nothing
//! the size of volume × views is ever stored, so the size of a tomogram is
//! limited by one slab of Y rows, not by the geometry.
//!
//! After alignment the tilt axis is the image Y axis and each Y row of the
//! views reconstructs one XZ slice. For view θ, voxel (x, z) projects to
//! detector position d = cos θ·u + sin θ·v + (w − 1)/2 with u = x − (w − 1)/2
//! and v = z − (Z − 1)/2 + offset.
//! - Back projection B (voxel driven): each voxel takes the linearly
//!   interpolated detector value at d, summed over views.
//! - Forward projection A (ray driven, Joseph's method): each detector bin
//!   steps along the ray one depth row at a time when |θ| ≤ 45° (sampling the
//!   slice linearly in x where the ray crosses the row, path length 1/|cos θ|
//!   per row), one column at a time otherwise (linear in z, 1/|sin θ|), so
//!   steep rays are not undersampled.
//!
//! WBP: rows are ramp filtered (zero padded to twice the width, optional
//! roll-off) and weighted by the angular interval each view stands for, then
//! back projected. SIRT (Gilbert, J Theor Biol 36, 1972, row/column
//! normalised): x ← x + λ · B[(b − A x) / A1] / B1. A and B are not exact
//! transposes (as in most SIRT codes); the normalisation makes the update a
//! contraction all the same. CPU here is the reference, `recon_gpu` the same
//! method on the GPU.

use super::check_cancel;
use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Method {
    Wbp,
    Sirt,
    /// Simultaneous algebraic reconstruction (Andersen & Kak, 1984): the
    /// SIRT update made one view at a time, views taken far apart in turn.
    Sart,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Filter {
    Ramp,
    Cosine,
    Hamming,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub thickness: usize,
    pub method: Method,
    pub iterations: usize,
    pub lambda: f32,
    pub non_negative: bool,
    pub z_offset: f64,
    /// Depth change per row along Y (sections per row): each row of the
    /// tomogram is taken that much deeper, so a specimen tilted about X
    /// lies level. Whole sections only, so nothing is resampled.
    #[serde(default)]
    pub y_slope: f64,
    pub filter: Filter,
    /// auto / cpu / gpu (explicit gpu fails instead of hiding fallback).
    pub backend: String,
}
impl Default for Params {
    fn default() -> Self {
        Self {
            thickness: 128,
            method: Method::Wbp,
            iterations: 15,
            lambda: 1.0,
            non_negative: false,
            z_offset: 0.0,
            y_slope: 0.0,
            filter: Filter::Ramp,
            backend: "auto".into(),
        }
    }
}
impl Params {
    pub fn validate(&self, w: usize, views: usize) -> Result<()> {
        if w < 2
            || views < 3
            || self.thickness == 0
            || self.thickness > 8192
            || self.iterations == 0
            || self.iterations > 1000
            || !self.z_offset.is_finite()
            || !self.y_slope.is_finite()
            || self.y_slope.abs() > 1.0
            || !self.lambda.is_finite()
            || self.lambda <= 0.0
            || self.lambda > 2.0
            || !matches!(self.backend.as_str(), "auto" | "cpu" | "gpu")
        {
            bail!("重构参数无效");
        }
        Ok(())
    }
}

/// Where each view's rays go: per view (cos θ, sin θ) and the WBP weight.
#[derive(Clone, Debug)]
pub struct Geometry {
    pub w: usize,
    pub z: usize,
    pub z_offset: f64,
    pub trig: Vec<[f64; 2]>,
    /// Angular interval each view stands for, relative to the mean.
    pub weight: Vec<f64>,
}

impl Geometry {
    pub fn new(w: usize, z: usize, angles: &[f64], z_offset: f64) -> Result<Geometry> {
        if angles.len() < 3 || angles.iter().any(|a| !a.is_finite() || a.abs() >= 89.5) {
            bail!("倾转角必须在 ±89.5° 内，且至少三个");
        }
        let mut order: Vec<usize> = (0..angles.len()).collect();
        order.sort_by(|&a, &b| angles[a].total_cmp(&angles[b]));
        let mut weight = vec![0.0; angles.len()];
        for (k, &i) in order.iter().enumerate() {
            let lo = if k > 0 { angles[order[k - 1]] } else { 2.0 * angles[i] - angles[order[k + 1]] };
            let hi = if k + 1 < order.len() { angles[order[k + 1]] } else { 2.0 * angles[i] - angles[order[k - 1]] };
            weight[i] = (hi - lo) / 2.0;
        }
        let mean = weight.iter().sum::<f64>() / weight.len() as f64;
        for v in &mut weight {
            *v = (*v / mean).clamp(0.25, 4.0);
        }
        let trig = angles.iter().map(|a| { let (s, c) = a.to_radians().sin_cos(); [c, s] }).collect();
        Ok(Geometry { w, z, z_offset, trig, weight })
    }

    pub fn views(&self) -> usize {
        self.trig.len()
    }

    pub fn depth(&self, z: usize) -> f64 {
        z as f64 - (self.z - 1) as f64 / 2.0 + self.z_offset
    }

    /// Detector position of voxel (x, z) in view v.
    pub fn detector(&self, v: usize, x: usize, z: usize) -> f64 {
        let [c, s] = self.trig[v];
        let h = (self.w - 1) as f64 / 2.0;
        c * (x as f64 - h) + s * self.depth(z) + h
    }

    /// Whether rays of view v step through depth rows (else through columns).
    pub fn by_rows(&self, v: usize) -> bool {
        let [c, s] = self.trig[v];
        c.abs() >= s.abs()
    }

    /// Columns a ray of detector bin j in a column-stepping view v crosses
    /// inside the slice: the first one (may be negative) and how many.
    pub fn columns(&self, v: usize, j: usize) -> (isize, usize) {
        let [c, s] = self.trig[v];
        let h = (self.w - 1) as f64 / 2.0;
        let at = |r: f64| h + (j as f64 - h - s * (r + self.z_offset - (self.z - 1) as f64 / 2.0)) / c;
        let (a, b) = (at(-1.0), at(self.z as f64));
        (a.min(b).floor() as isize, ((self.z + 1) as f64 * (s / c).abs()).ceil() as usize + 2)
    }

    /// Ray of detector bin j in view v at step k (a depth row or a column):
    /// the position along the other axis and the path length per step.
    pub fn crossing(&self, v: usize, j: usize, k: usize) -> (f64, f64) {
        let [c, s] = self.trig[v];
        let h = (self.w - 1) as f64 / 2.0;
        let t = j as f64 - h;
        if self.by_rows(v) {
            ((t - s * self.depth(k)) / c + h, 1.0 / c.abs())
        } else {
            // Depth v at column k, back to a row index.
            let depth = (t - c * (k as f64 - h)) / s;
            (depth - self.z_offset + (self.z - 1) as f64 / 2.0, 1.0 / s.abs())
        }
    }
}

/// Linear interpolation of a row at x, zero outside it.
fn lerp(row: &[f32], x: f64) -> f32 {
    let i = x.floor();
    if i < -1.0 || i >= row.len() as f64 {
        return 0.0;
    }
    let f = (x - i) as f32;
    let at = |k: f64| if k >= 0.0 && k < row.len() as f64 { row[k as usize] } else { 0.0 };
    at(i) * (1.0 - f) + at(i + 1.0) * f
}

/// The WBP filter on the zero-padded length n (bins 0..=n/2): the ramp
/// |k| (a quarter bin at 0, the exact discrete ramp's mean there) times the
/// roll-off, scaled so a flat row keeps its level.
pub fn ramp(n: usize, f: Filter) -> Vec<f32> {
    (0..=n / 2)
        .map(|k| {
            let r = k as f32 / (n / 2) as f32;
            let window = match f {
                Filter::Ramp => 1.0,
                Filter::Cosine => (std::f32::consts::FRAC_PI_2 * r).cos(),
                Filter::Hamming => 0.54 + 0.46 * (std::f32::consts::PI * r).cos(),
            };
            (if k == 0 { 0.25 } else { k as f32 }) * 2.0 / n as f32 * window
        })
        .collect()
}

/// Ramp filtered rows (each of length w) for WBP.
pub fn filtered(b: &[f32], w: usize, f: Filter) -> Vec<f32> {
    let n = (2 * w).next_power_of_two();
    let fft = crate::fft::Fft::new(n);
    let ramp = ramp(n, f);
    b.par_chunks(w)
        .flat_map_iter(|row| {
            let mut re = vec![0.0; n];
            let mut im = vec![0.0; n];
            re[..w].copy_from_slice(row);
            fft.run(&mut re, &mut im);
            for k in 0..n {
                let g = ramp[k.min(n - k)];
                re[k] *= g;
                im[k] *= -g;
            }
            fft.run(&mut re, &mut im);
            re[..w].iter().map(|v| v / n as f32).collect::<Vec<_>>()
        })
        .collect()
}

/// B for one row: sino [views][w] → slice [z][w], `scale` per view.
pub fn back(g: &Geometry, sino: &[f32], scale: &[f64]) -> Vec<f32> {
    let w = g.w;
    let mut out = vec![0.0; g.z * w];
    for (z, row) in out.chunks_mut(w).enumerate() {
        for (x, o) in row.iter_mut().enumerate() {
            *o = (0..g.views()).map(|v| scale[v] as f32 * lerp(&sino[v * w..(v + 1) * w], g.detector(v, x, z))).sum();
        }
    }
    out
}

/// A for one row: slice [z][w] → sino [views][w].
pub fn forward(g: &Geometry, slice: &[f32]) -> Vec<f32> {
    let w = g.w;
    let columns: Vec<Vec<f32>> = (0..w).map(|x| (0..g.z).map(|z| slice[z * w + x]).collect()).collect();
    let mut out = vec![0.0; g.views() * w];
    for (v, row) in out.chunks_mut(w).enumerate() {
        let rows = g.by_rows(v);
        for (j, o) in row.iter_mut().enumerate() {
            *o = if rows {
                (0..g.z).map(|k| { let (at, len) = g.crossing(v, j, k); len as f32 * lerp(&slice[k * w..(k + 1) * w], at) }).sum()
            } else {
                let (k0, m) = g.columns(v, j);
                (k0.max(0)..(k0 + m as isize).min(w as isize))
                    .map(|k| { let (at, len) = g.crossing(v, j, k as usize); len as f32 * lerp(&columns[k as usize], at) })
                    .sum()
            };
        }
    }
    out
}

/// B for one view: row [w] → slice [z][w].
pub fn back_view(g: &Geometry, v: usize, row: &[f32]) -> Vec<f32> {
    let w = g.w;
    let mut out = vec![0.0; g.z * w];
    for (z, r) in out.chunks_mut(w).enumerate() {
        for (x, o) in r.iter_mut().enumerate() {
            *o = lerp(row, g.detector(v, x, z));
        }
    }
    out
}

/// A for one view: slice [z][w] → row [w].
pub fn forward_view(g: &Geometry, v: usize, slice: &[f32]) -> Vec<f32> {
    let mut one = g.clone();
    one.trig = vec![g.trig[v]];
    one.weight = vec![1.0];
    forward(&one, slice)
}

/// The order SART visits views in: each next view far from the last
/// (steps of about 0.618 of the series), so consecutive updates are
/// nearly independent.
pub fn sart_order(n: usize) -> Vec<usize> {
    let step = ((n as f64 * 0.618).round() as usize).max(1);
    let mut step = step;
    while gcd(step, n) != 1 {
        step += 1;
    }
    (0..n).map(|k| k * step % n).collect()
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// A1 and B1 for one row (they do not depend on Y).
pub fn normalisers(g: &Geometry) -> (Vec<f32>, Vec<f32>) {
    let rows = forward(g, &vec![1.0; g.z * g.w]);
    let cols = back(g, &vec![1.0; g.views() * g.w], &vec![1.0; g.views()]);
    (rows, cols)
}

/// SIRT updates only rays whose path through the slice is at least `SEEN`
/// of the longest path in their view: rays near the edges at high tilt
/// mostly cross specimen outside the reconstructed width, and dividing
/// their error by a short path amplifies that into streaks. Voxels seen by
/// less than `SEEN` of the best covered voxel's views take no update either.
pub const COVERED: f32 = 1e-3;
pub const SEEN: f32 = 0.5;

/// 1 / v where v ≥ `least`, else 0.
fn inverse(v: &mut [f32], least: f32) {
    for v in v {
        *v = if *v >= least.max(COVERED) { 1.0 / *v } else { 0.0 };
    }
}

/// Reconstructs one slab: sinograms [rows][views][w] → [rows][z][w].
pub fn cpu_slab(g: &Geometry, sino: &[f32], p: &Params) -> Result<Vec<f32>> {
    let (w, nv) = (g.w, g.views());
    if sino.len() % (nv * w) != 0 {
        bail!("投影尺寸不一致");
    }
    if p.method == Method::Wbp {
        let f = filtered(sino, w, p.filter);
        let scale: Vec<f64> = g.weight.iter().map(|k| k / nv as f64).collect();
        return Ok(f.par_chunks(nv * w).flat_map_iter(|s| back(g, s, &scale)).collect());
    }
    let (mut rs, mut cs) = normalisers(g);
    for view in rs.chunks_mut(w) {
        let longest = view.iter().copied().fold(0.0, f32::max);
        inverse(view, SEEN * longest);
    }
    let most = cs.iter().copied().fold(0.0, f32::max);
    inverse(&mut cs, SEEN * most);
    let ones = vec![1.0; nv];
    let order = sart_order(nv);
    let slabs: Vec<Vec<f32>> = sino
        .par_chunks(nv * w)
        .map(|b| {
            let mut x = vec![0.0f32; g.z * w];
            for _ in 0..p.iterations {
                if check_cancel().is_err() {
                    break;
                }
                if p.method == Method::Sart {
                    for &v in &order {
                        let ax = forward_view(g, v, &x);
                        let e: Vec<f32> = (0..w).map(|j| (b[v * w + j] - ax[j]) * rs[v * w + j]).collect();
                        let step = back_view(g, v, &e);
                        for (x, s) in x.iter_mut().zip(&step) {
                            *x += p.lambda * s;
                            if p.non_negative {
                                *x = x.max(0.0);
                            }
                        }
                    }
                    continue;
                }
                let ax = forward(g, &x);
                let e: Vec<f32> = b.iter().zip(&ax).zip(&rs).map(|((b, a), r)| (b - a) * r).collect();
                let step = back(g, &e, &ones);
                for ((x, s), c) in x.iter_mut().zip(&step).zip(&cs) {
                    *x += p.lambda * s * c;
                    if p.non_negative {
                        *x = x.max(0.0);
                    }
                }
            }
            x
        })
        .collect();
    check_cancel()?;
    Ok(slabs.concat())
}

/// Sinogram rows of the aligned views: `rows(y0, n)` gives [n][views][w].
pub trait Sinograms: Sync {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn rows(&self, y0: usize, n: usize) -> Result<Vec<f32>>;
}

/// Aligned views held in memory, [view][y][x].
pub struct Views<'a> {
    pub data: &'a [f32],
    pub w: usize,
    pub h: usize,
    pub views: usize,
}

impl Sinograms for Views<'_> {
    fn width(&self) -> usize {
        self.w
    }
    fn height(&self) -> usize {
        self.h
    }
    fn rows(&self, y0: usize, n: usize) -> Result<Vec<f32>> {
        let mut out = Vec::with_capacity(n * self.views * self.w);
        for y in y0..y0 + n {
            for v in 0..self.views {
                let at = (v * self.h + y) * self.w;
                out.extend_from_slice(&self.data[at..at + self.w]);
            }
        }
        Ok(out)
    }
}

/// Rows per slab, so a slab of volume stays near 2²⁴ voxels.
pub fn slab_rows(g: &Geometry, h: usize) -> usize {
    ((1usize << 24) / (g.z * g.w).max(1)).clamp(1, h.max(1))
}

/// Mean of every view. Each is subtracted before reconstruction: the
/// views' mean levels differ with the path length through the specimen
/// (and the exposure), which no density inside the reconstructed slab can
/// explain; left in, SIRT spreads them into broad shading and bright
/// borders where few rays cross.
fn view_means(src: &dyn Sinograms, views: usize, rows: usize) -> Result<Vec<f32>> {
    let (w, h) = (src.width(), src.height());
    let mut sum = vec![0f64; views];
    for y0 in (0..h).step_by(rows) {
        let n = rows.min(h - y0);
        for (i, v) in src.rows(y0, n)?.iter().enumerate() {
            sum[(i / w) % views] += *v as f64;
        }
    }
    Ok(sum.iter().map(|s| (s / (w * h) as f64) as f32).collect())
}

/// Reconstructs every row: slabs go to `sink(y0, n, [n][z][w])` in order.
/// GPU first (when there is one and `p.backend` allows); returns the backend.
pub fn reconstruct(
    src: &dyn Sinograms,
    g: &Geometry,
    p: &Params,
    sink: &mut dyn FnMut(usize, usize, Vec<f32>) -> Result<()>,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<String> {
    p.validate(g.w, g.views())?;
    if src.width() != g.w {
        bail!("投影宽度与几何不一致");
    }
    let h = src.height();
    let rows = slab_rows(g, h);
    let gpu = if p.backend == "cpu" { None } else { crate::ctf_gpu::device() };
    if p.backend == "gpu" && gpu.is_none() {
        bail!("没有可用 GPU");
    }
    let mut plan = gpu.and_then(|(dev, name)| {
        crate::ctf_gpu::guarded(|| super::recon_gpu::Plan::new(dev, g, p)).ok().and_then(Result::ok).map(|plan| (plan, *name))
    });
    let mut backend = plan.as_ref().map_or("CPU", |p| p.1).to_string();
    let means = view_means(src, g.views(), rows)?;
    for y0 in (0..h).step_by(rows) {
        check_cancel()?;
        let n = rows.min(h - y0);
        let mut sino = src.rows(y0, n)?;
        for (i, v) in sino.iter_mut().enumerate() {
            *v -= means[(i / g.w) % g.views()];
        }
        let gpu_slab = plan.as_ref().map(|(gp, _)| crate::ctf_gpu::guarded(|| gp.slab(&sino, n)));
        let slab = match gpu_slab {
            Some(Ok(Ok(v))) => v,
            Some(r) => {
                check_cancel()?;
                let e = match r {
                    Ok(Err(e)) | Err(e) => e,
                    Ok(Ok(_)) => unreachable!(),
                };
                if p.backend == "gpu" {
                    return Err(e);
                }
                eprintln!("tomo: GPU reconstruction failed ({e}); CPU from row {y0}");
                plan = None;
                backend = "CPU".into();
                cpu_slab(g, &sino, p)?
            }
            None => cpu_slab(g, &sino, p)?,
        };
        sink(y0, n, slab)?;
        progress(y0 + n, h);
    }
    Ok(backend)
}
