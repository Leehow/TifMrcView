//! 4D-STEM (Pro): a stack of diffraction patterns, one per probe position
//! of a scan. One pass over the patterns gives, for every probe position,
//! the counts on any number of virtual detectors (disks, annuli,
//! rectangles) and the centre of mass (first moments) of the pattern, plus
//! the mean and maximum pattern. All of these are one matrix product of a
//! block of patterns with a matrix of masks, done on the GPU when there is
//! one (Pro build) and on the CPU otherwise.
//!
//! From the centre of mass the scan-sized maps follow: the shift field
//! (DPC), its rotation to the scan axes (the angle and flip that make the
//! field curl-free), its divergence (dDPC) and its integral (iDPC, the
//! projected potential up to a factor), the latter by a Fourier solve of
//! the Poisson equation on the mirror-extended field.

use std::sync::Arc;

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::image::{OpenImage, Px};

/// Where the patterns come from: `count` patterns of `w` × `h` pixels.
pub trait Frames: Sync {
    fn count(&self) -> usize;
    fn dims(&self) -> [usize; 2];
    /// Pattern `i` into `out` (w × h, row major).
    fn read(&self, i: usize, out: &mut [f32]) -> Result<()>;
}

/// The planes of an open image as patterns: Z first, then T.
pub struct ImageFrames {
    pub img: Arc<OpenImage>,
    pub c: usize,
}

impl Frames for ImageFrames {
    fn count(&self) -> usize {
        let i = self.img.info();
        i.size_z * i.size_t
    }
    fn dims(&self) -> [usize; 2] {
        let i = self.img.info();
        [i.width, i.height]
    }
    fn read(&self, i: usize, out: &mut [f32]) -> Result<()> {
        let info = self.img.info();
        let (page, sample) = info.locate(self.c, i % info.size_z, i / info.size_z);
        let planes = self.img.page(page)?;
        let p = &planes[sample.min(planes.len() - 1)];
        if p.w * p.h != out.len() {
            bail!("第 {i} 张衍射图大小不一致");
        }
        crate::with_plane!(&p.data, v => {
            for (o, x) in out.iter_mut().zip(v.iter()) {
                *o = x.to_f32();
            }
        });
        Ok(())
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Detector {
    /// Pixels with r0 ≤ distance from (cx, cy) < r1; r0 = 0 is a disk.
    Annulus { cx: f64, cy: f64, r0: f64, r1: f64 },
    Rect { x: f64, y: f64, w: f64, h: f64 },
}

impl Detector {
    fn mask(&self, w: usize, h: usize) -> Vec<f32> {
        let mut m = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let on = match *self {
                    Detector::Annulus { cx, cy, r0, r1 } => {
                        let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                        d >= r0 && d < r1
                    }
                    Detector::Rect { x: rx, y: ry, w: rw, h: rh } => px >= rx && px < rx + rw && py >= ry && py < ry + rh,
                };
                if on {
                    m[y * w + x] = 1.0;
                }
            }
        }
        m
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Req {
    pub token: u64,
    #[serde(default)]
    pub c: usize,
    /// Probe positions per scan row and rows.
    pub scan: [usize; 2],
    /// Virtual detectors; empty: a bright-field disk and an annular
    /// dark-field ring placed from the mean pattern.
    #[serde(default)]
    pub detectors: Vec<Detector>,
    /// Pattern centre for the centre-of-mass shifts; None: the centre of
    /// the bright-field disk of the mean pattern.
    #[serde(default)]
    pub center: Option<[f64; 2]>,
    /// Angle (degrees) and flip from the detector to the scan axes; None
    /// finds them.
    #[serde(default)]
    pub rotation: Option<f64>,
    #[serde(default)]
    pub flip: Option<bool>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Result4d {
    pub scan: [usize; 2],
    pub det: [usize; 2],
    pub detectors: Vec<Detector>,
    /// One scan-sized image per detector, row major.
    pub images: Vec<Vec<f32>>,
    pub mean_dp: Vec<f32>,
    pub max_dp: Vec<f32>,
    /// Bright-field disk found in the mean pattern: centre and radius, px.
    pub disk: [f64; 3],
    pub center: [f64; 2],
    /// Centre-of-mass shift in detector pixels, turned to the scan axes.
    pub com_x: Vec<f32>,
    pub com_y: Vec<f32>,
    pub rotation: f64,
    pub flip: bool,
    /// Whether the rotation was found (rather than given).
    pub rotation_found: bool,
    pub idpc: Vec<f32>,
    pub ddpc: Vec<f32>,
    pub ms: f64,
    pub gpu: bool,
}

/// Per-pattern sums: for each mask, Σ mask·I; then Σ I, Σ x·I, Σ y·I.
struct Pass {
    sums: Vec<Vec<f64>>,
    mean: Vec<f32>,
    max: Vec<f32>,
    gpu: bool,
}

fn moment_masks(w: usize, h: usize) -> [Vec<f32>; 3] {
    let one = vec![1f32; w * h];
    let xs = (0..w * h).map(|i| (i % w) as f32 + 0.5).collect();
    let ys = (0..w * h).map(|i| (i / w) as f32 + 0.5).collect();
    [one, xs, ys]
}

fn pass(src: &dyn Frames, n: usize, masks: &[Vec<f32>]) -> Result<Pass> {
    #[cfg(feature = "pro")]
    if let Some((dev, _)) = crate::ctf_gpu::device() {
        if let Ok(Ok(p)) = crate::ctf_gpu::guarded(|| gpu_pass(dev, src, n, masks)) {
            return Ok(p);
        }
    }
    cpu_pass(src, n, masks)
}

/// Patterns per block: about 64 MB of f32.
fn block(k: usize) -> usize {
    ((16 << 20) / k.max(1)).clamp(1, 4096)
}

fn cpu_pass(src: &dyn Frames, n: usize, masks: &[Vec<f32>]) -> Result<Pass> {
    let [w, h] = src.dims();
    let k = w * h;
    let m = masks.len();
    let mut sums = vec![vec![0f64; n]; m];
    let mut mean = vec![0f64; k];
    let mut max = vec![f32::MIN; k];
    let b = block(k);
    let mut start = 0;
    while start < n {
        let end = (start + b).min(n);
        // Read and reduce a block in parallel, then merge.
        let parts: Vec<Result<(Vec<Vec<f64>>, Vec<f64>, Vec<f32>)>> = (start..end)
            .into_par_iter()
            .fold(
                || Ok((Vec::new(), vec![0f64; k], vec![f32::MIN; k])),
                |acc: Result<(Vec<Vec<f64>>, Vec<f64>, Vec<f32>)>, i| {
                    let (mut rows, mut s, mut mx) = acc?;
                    let mut f = vec![0f32; k];
                    src.read(i, &mut f)?;
                    let mut row = vec![i as f64; m + 1];
                    for (j, mask) in masks.iter().enumerate() {
                        row[j + 1] = f.iter().zip(mask).map(|(a, b)| (a * b) as f64).sum();
                    }
                    rows.push(row);
                    for ((s, mx), &v) in s.iter_mut().zip(mx.iter_mut()).zip(&f) {
                        *s += v as f64;
                        if v > *mx {
                            *mx = v;
                        }
                    }
                    Ok((rows, s, mx))
                },
            )
            .collect();
        for p in parts {
            let (rows, s, mx) = p?;
            for row in rows {
                let i = row[0] as usize;
                for j in 0..m {
                    sums[j][i] = row[j + 1];
                }
            }
            mean.iter_mut().zip(&s).for_each(|(a, b)| *a += b);
            max.iter_mut().zip(&mx).for_each(|(a, &b)| *a = a.max(b));
        }
        start = end;
    }
    Ok(Pass { sums, mean: mean.iter().map(|v| (v / n as f64) as f32).collect(), max, gpu: false })
}

#[cfg(feature = "pro")]
fn gpu_pass(dev: &burn::tensor::Device, src: &dyn Frames, n: usize, masks: &[Vec<f32>]) -> Result<Pass> {
    use burn::tensor::{Tensor, TensorData};
    let [w, h] = src.dims();
    let k = w * h;
    let m = masks.len();
    // Masks as one K × M matrix.
    let mut mm = vec![0f32; k * m];
    for (j, mask) in masks.iter().enumerate() {
        for (p, v) in mask.iter().enumerate() {
            mm[p * m + j] = *v;
        }
    }
    let mt: Tensor<2> = Tensor::from_data(TensorData::new(mm, [k, m]), dev);
    let mut sums = vec![vec![0f64; n]; m];
    let mut mean = vec![0f64; k];
    let mut max = vec![f32::MIN; k];
    let b = block(k);
    let mut start = 0;
    while start < n {
        let end = (start + b).min(n);
        let rows = end - start;
        let mut buf = vec![0f32; rows * k];
        buf.par_chunks_mut(k).enumerate().try_for_each(|(r, out)| src.read(start + r, out))?;
        // The maximum is a cheap scan of the block already in memory.
        for row in buf.chunks(k) {
            max.iter_mut().zip(row).for_each(|(a, &b)| *a = a.max(b));
        }
        let t: Tensor<2> = Tensor::from_data(TensorData::new(buf, [rows, k]), dev);
        let prod: Vec<f32> = t.clone().matmul(mt.clone()).into_data().convert::<f32>().try_into_vec::<f32>().map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let s: Vec<f32> = t.sum_dim(0).into_data().convert::<f32>().try_into_vec::<f32>().map_err(|e| anyhow::anyhow!("{e:?}"))?;
        for r in 0..rows {
            for j in 0..m {
                sums[j][start + r] = prod[r * m + j] as f64;
            }
        }
        mean.iter_mut().zip(&s).for_each(|(a, &b)| *a += b as f64);
        start = end;
    }
    Ok(Pass { sums, mean: mean.iter().map(|v| (v / n as f64) as f32).collect(), max, gpu: true })
}

/// The bright-field disk of a mean pattern: centre and radius of the
/// region above halfway between the median and the maximum.
pub fn find_disk(dp: &[f32], w: usize, h: usize) -> [f64; 3] {
    let mut s: Vec<f32> = dp.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let (med, top) = (s[s.len() / 2] as f64, s[s.len() - 1] as f64);
    let thr = (med + top) / 2.0;
    let (mut a, mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0, 0.0);
    for y in 0..h {
        for x in 0..w {
            let v = dp[y * w + x] as f64;
            if v > thr {
                a += 1.0;
                let wgt = v - med;
                sx += wgt * (x as f64 + 0.5);
                sy += wgt * (y as f64 + 0.5);
                sw += wgt;
            }
        }
    }
    if sw <= 0.0 {
        return [w as f64 / 2.0, h as f64 / 2.0, 1.0];
    }
    [sx / sw, sy / sw, (a / std::f64::consts::PI).sqrt().max(1.0)]
}

/// Default detectors around a disk: the disk itself (bright field) and a
/// ring from 1.3 to 3 disk radii (annular dark field), cut at the pattern edge.
pub fn default_detectors(disk: [f64; 3], w: usize, h: usize) -> Vec<Detector> {
    let [cx, cy, r] = disk;
    let edge = cx.min(cy).min(w as f64 - cx).min(h as f64 - cy);
    let r0 = 1.3 * r;
    let r1 = (3.0 * r).min(edge).max(r0 + 2.0);
    vec![Detector::Annulus { cx, cy, r0: 0.0, r1: r }, Detector::Annulus { cx, cy, r0, r1 }]
}

pub fn analyze(src: &dyn Frames, req: &Req) -> Result<Result4d> {
    let t0 = std::time::Instant::now();
    let [sx, sy] = req.scan;
    let n = sx * sy;
    if sx == 0 || sy == 0 {
        bail!("扫描尺寸不对");
    }
    if n > src.count() {
        bail!("扫描 {sx} × {sy} = {n} 个点，但文件只有 {} 张衍射图", src.count());
    }
    let [w, h] = src.dims();
    let k = w * h;
    let mut masks: Vec<Vec<f32>> = moment_masks(w, h).into();
    let given = !req.detectors.is_empty();
    masks.extend(req.detectors.iter().map(|d| d.mask(w, h)));
    let mut p = pass(src, n, &masks)?;
    let disk = find_disk(&p.mean, w, h);
    let detectors = if given {
        req.detectors.clone()
    } else {
        // The mean pattern places the default detectors; a second pass measures them.
        let d = default_detectors(disk, w, h);
        let q = pass(src, n, &d.iter().map(|d| d.mask(w, h)).collect::<Vec<_>>())?;
        p.sums.extend(q.sums);
        d
    };
    let center = req.center.unwrap_or([disk[0], disk[1]]);
    // Centre of mass relative to the centre.
    let (mut cx, mut cy) = (vec![0f64; n], vec![0f64; n]);
    for i in 0..n {
        let s = p.sums[0][i];
        if s > 0.0 {
            cx[i] = p.sums[1][i] / s - center[0];
            cy[i] = p.sums[2][i] / s - center[1];
        }
    }
    // A constant offset (a slightly wrong centre) is not part of the field.
    let (mx, my) = (cx.iter().sum::<f64>() / n as f64, cy.iter().sum::<f64>() / n as f64);
    cx.iter_mut().for_each(|v| *v -= mx);
    cy.iter_mut().for_each(|v| *v -= my);
    let images: Vec<Vec<f32>> = p.sums[3..].iter().map(|s| s.iter().map(|&v| v as f32).collect()).collect();
    let _ = k;

    let (rotation, flip, found) = match (req.rotation, req.flip) {
        (Some(r), Some(f)) => (r, f, false),
        _ => {
            let (r, f) = find_rotation(&cx, &cy, sx, sy);
            (r, f, true)
        }
    };
    let (mut u, mut v) = rotate(&cx, &cy, rotation, flip);
    let mut phi = integrate(&u, &v, sx, sy);
    let mut rotation = rotation;
    // Curl does not tell θ from θ + 180°: take the one whose potential
    // is high where the dark-field signal is (atoms scatter to high angles).
    if found {
        if let Some(adf) = detectors.iter().position(|d| matches!(d, Detector::Annulus { r0, .. } if *r0 > 0.0)).map(|j| &images[j]) {
            if corr(&phi, adf) < 0.0 {
                rotation = (rotation + 180.0) % 360.0;
                u.iter_mut().for_each(|a| *a = -*a);
                v.iter_mut().for_each(|a| *a = -*a);
                phi.iter_mut().for_each(|a| *a = -*a);
            }
        }
    }
    let ddpc = divergence(&u, &v, sx, sy);
    Ok(Result4d {
        scan: req.scan,
        det: [w, h],
        detectors,
        images,
        mean_dp: p.mean,
        max_dp: p.max,
        disk,
        center,
        com_x: u.iter().map(|&a| a as f32).collect(),
        com_y: v.iter().map(|&a| a as f32).collect(),
        rotation,
        flip,
        rotation_found: found,
        idpc: phi.iter().map(|&a| a as f32).collect(),
        ddpc: ddpc.iter().map(|&a| a as f32).collect(),
        ms: t0.elapsed().as_secs_f64() * 1e3,
        gpu: p.gpu,
    })
}

fn corr(a: &[f64], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    let ma = a.iter().sum::<f64>() / n;
    let mb = b.iter().map(|&v| v as f64).sum::<f64>() / n;
    a.iter().zip(b).map(|(x, &y)| (x - ma) * (y as f64 - mb)).sum()
}

/// Turns the field by `deg` (after mirroring y when `flip`).
fn rotate(x: &[f64], y: &[f64], deg: f64, flip: bool) -> (Vec<f64>, Vec<f64>) {
    let (s, c) = deg.to_radians().sin_cos();
    x.iter()
        .zip(y)
        .map(|(&a, &b)| {
            let b = if flip { -b } else { b };
            (c * a - s * b, s * a + c * b)
        })
        .unzip()
}

fn curl_rms(u: &[f64], v: &[f64], w: usize, h: usize) -> f64 {
    let mut s = 0.0;
    let mut n = 0.0;
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            let c = (v[i + 1] - v[i - 1]) / 2.0 - (u[i + w] - u[i - w]) / 2.0;
            s += c * c;
            n += 1.0;
        }
    }
    if n > 0.0 { (s / n).sqrt() } else { 0.0 }
}

/// The angle in [0°, 180°) and flip that make the field most curl-free.
pub fn find_rotation(x: &[f64], y: &[f64], w: usize, h: usize) -> (f64, bool) {
    let mut cands: Vec<(f64, f64, bool)> = (0..360)
        .into_par_iter()
        .flat_map_iter(|i| {
            let deg = i as f64 * 0.5;
            [false, true].into_iter().map(move |f| {
                let (u, v) = rotate(x, y, deg, f);
                (curl_rms(&u, &v, w, h), deg, f)
            })
        })
        .collect();
    cands.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (_, mut deg, flip) = cands[0];
    // Refine to 0.05° around the best.
    let mut best = cands[0].0;
    for k in -10..=10 {
        let d = deg + k as f64 * 0.05;
        let (u, v) = rotate(x, y, d, flip);
        let c = curl_rms(&u, &v, w, h);
        if c < best {
            best = c;
            deg = d;
        }
    }
    let _ = best;
    (deg.rem_euclid(180.0), flip)
}

fn divergence(u: &[f64], v: &[f64], w: usize, h: usize) -> Vec<f64> {
    let at = |a: &[f64], x: isize, y: isize| a[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
    let mut out = vec![0.0; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            out[y as usize * w + x as usize] = (at(u, x + 1, y) - at(u, x - 1, y)) / 2.0 + (at(v, x, y + 1) - at(v, x, y - 1)) / 2.0;
        }
    }
    out
}

/// 1D DFT of length n (forward when `sign` = −1), any n.
struct Dft {
    n: usize,
    fft: Option<crate::fft::Fft>,
    cos: Vec<f64>,
    sin: Vec<f64>,
}

impl Dft {
    fn new(n: usize) -> Dft {
        let fft = (n.is_power_of_two() && n >= 2).then(|| crate::fft::Fft::new(n));
        let (cos, sin) = if fft.is_some() { (vec![], vec![]) } else { (0..n).map(|k| (2.0 * std::f64::consts::PI * k as f64 / n as f64).sin_cos()).map(|(s, c)| (c, s)).unzip() };
        Dft { n, fft, cos, sin }
    }
    /// Forward when `inverse` is false; the inverse is not scaled.
    fn run(&self, re: &mut [f64], im: &mut [f64], inverse: bool) {
        let n = self.n;
        if inverse {
            im.iter_mut().for_each(|v| *v = -*v);
        }
        if let Some(f) = &self.fft {
            let mut r: Vec<f32> = re.iter().map(|&v| v as f32).collect();
            let mut i: Vec<f32> = im.iter().map(|&v| v as f32).collect();
            f.run(&mut r, &mut i);
            re.iter_mut().zip(&r).for_each(|(a, &b)| *a = b as f64);
            im.iter_mut().zip(&i).for_each(|(a, &b)| *a = b as f64);
        } else {
            let (mut or, mut oi) = (vec![0.0; n], vec![0.0; n]);
            for k in 0..n {
                let (mut sr, mut si) = (0.0, 0.0);
                for j in 0..n {
                    let t = (j * k) % n;
                    let (c, s) = (self.cos[t], -self.sin[t]);
                    sr += re[j] * c - im[j] * s;
                    si += re[j] * s + im[j] * c;
                }
                or[k] = sr;
                oi[k] = si;
            }
            re.copy_from_slice(&or);
            im.copy_from_slice(&oi);
        }
        if inverse {
            im.iter_mut().for_each(|v| *v = -*v);
        }
    }
}

/// 2D transform of a w × h grid in place.
fn dft2(re: &mut [f64], im: &mut [f64], w: usize, h: usize, inverse: bool) {
    let (fx, fy) = (Dft::new(w), Dft::new(h));
    re.par_chunks_mut(w).zip(im.par_chunks_mut(w)).for_each(|(r, i)| fx.run(r, i, inverse));
    let cols: Vec<(Vec<f64>, Vec<f64>)> = (0..w)
        .into_par_iter()
        .map(|x| {
            let (mut r, mut i): (Vec<f64>, Vec<f64>) = (0..h).map(|y| (re[y * w + x], im[y * w + x])).unzip();
            fy.run(&mut r, &mut i, inverse);
            (r, i)
        })
        .collect();
    for (x, (r, i)) in cols.into_iter().enumerate() {
        for y in 0..h {
            re[y * w + x] = r[y];
            im[y * w + x] = i[y];
        }
    }
}

/// The potential whose gradient is closest to (u, v): mirror the field
/// (odd in its own direction, even across) to a 2w × 2h periodic one and
/// solve ∇²φ = ∇·(u, v) in Fourier space.
pub fn integrate(u: &[f64], v: &[f64], w: usize, h: usize) -> Vec<f64> {
    let (ww, hh) = (2 * w, 2 * h);
    let mut ur = vec![0.0; ww * hh];
    let mut vr = vec![0.0; ww * hh];
    for y in 0..hh {
        let (sy, ys) = if y < h { (1.0, y) } else { (-1.0, hh - 1 - y) };
        for x in 0..ww {
            let (sx, xs) = if x < w { (1.0, x) } else { (-1.0, ww - 1 - x) };
            ur[y * ww + x] = sx * u[ys * w + xs];
            vr[y * ww + x] = sy * v[ys * w + xs];
        }
    }
    let (mut ui, mut vi) = (vec![0.0; ww * hh], vec![0.0; ww * hh]);
    dft2(&mut ur, &mut ui, ww, hh, false);
    dft2(&mut vr, &mut vi, ww, hh, false);
    // φ̂ = (kx·Û + ky·V̂) / (2πi |k|²), with k in cycles per pixel.
    let (mut pr, mut pi) = (vec![0.0; ww * hh], vec![0.0; ww * hh]);
    for y in 0..hh {
        let ky = if y <= hh / 2 { y as f64 } else { y as f64 - hh as f64 } / hh as f64;
        for x in 0..ww {
            let kx = if x <= ww / 2 { x as f64 } else { x as f64 - ww as f64 } / ww as f64;
            let k2 = kx * kx + ky * ky;
            if k2 == 0.0 {
                continue;
            }
            let i = y * ww + x;
            let (nr, ni) = (kx * ur[i] + ky * vr[i], kx * ui[i] + ky * vi[i]);
            // Divide by 2πi·k²: (a + ib) / (i·c) = (b − ia) / c.
            let c = 2.0 * std::f64::consts::PI * k2;
            pr[i] = ni / c;
            pi[i] = -nr / c;
        }
    }
    dft2(&mut pr, &mut pi, ww, hh, true);
    let scale = 1.0 / (ww * hh) as f64;
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            out[y * w + x] = pr[y * ww + x] * scale;
        }
    }
    let m = out.iter().sum::<f64>() / out.len() as f64;
    out.iter_mut().for_each(|a| *a -= m);
    out
}

/// Scan shape a pattern count most likely has: the size in the file, else
/// the squarest exact factorisation (wider than tall).
pub fn guess_scan(size_z: usize, size_t: usize) -> [usize; 2] {
    if size_z > 1 && size_t > 1 {
        return [size_z, size_t];
    }
    let n = size_z * size_t;
    let mut best = [n, 1];
    let mut a = 1;
    while a * a <= n {
        if n % a == 0 {
            best = [n / a, a];
        }
        a += 1;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Synth {
        w: usize,
        frames: Vec<Vec<f32>>,
    }

    impl Frames for Synth {
        fn count(&self) -> usize {
            self.frames.len()
        }
        fn dims(&self) -> [usize; 2] {
            [self.w, self.w]
        }
        fn read(&self, i: usize, out: &mut [f32]) -> Result<()> {
            out.copy_from_slice(&self.frames[i]);
            Ok(())
        }
    }

    /// A scan over a smooth potential: each pattern is a bright disk shifted
    /// by the potential's gradient turned by `rot` degrees, plus a dark-field
    /// halo proportional to the potential.
    fn synth(sx: usize, sy: usize, w: usize, rot: f64, flip: bool) -> (Synth, Vec<f64>) {
        // No mirror or rotation symmetry, so the angle and flip are unique.
        let tau = 2.0 * std::f64::consts::PI;
        let pot = |x: f64, y: f64| 0.6 * (tau * x / 9.0).cos() + 0.5 * (tau * (x + 2.0 * y) / 13.0).sin() + 0.4 * (tau * (0.3 * x - y) / 7.0 + 1.0).cos();
        let mut frames = Vec::new();
        let mut truth = Vec::new();
        let (s, c) = (-rot).to_radians().sin_cos();
        for j in 0..sy {
            for i in 0..sx {
                let (x, y) = (i as f64, j as f64);
                let p = pot(x, y);
                truth.push(p);
                let g = [(pot(x + 0.01, y) - pot(x - 0.01, y)) / 0.02, (pot(x, y + 0.01) - pot(x, y - 0.01)) / 0.02];
                // Scan-frame shift → detector frame: undo the rotation, then the flip.
                let (dx, dy) = (c * g[0] - s * g[1], s * g[0] + c * g[1]);
                let dy = if flip { -dy } else { dy };
                let (cx, cy) = (w as f64 / 2.0 + 1.5 * dx, w as f64 / 2.0 + 1.5 * dy);
                let mut f = vec![0f32; w * w];
                for yy in 0..w {
                    for xx in 0..w {
                        let d = ((xx as f64 + 0.5 - cx).powi(2) + (yy as f64 + 0.5 - cy).powi(2)).sqrt();
                        let disk = 1.0 / (1.0 + ((d - 6.0) * 2.0).exp());
                        let halo = if d > 9.0 && d < 14.0 { 0.05 * (1.0 + p) } else { 0.0 };
                        f[yy * w + xx] = (100.0 * disk + 100.0 * halo) as f32;
                    }
                }
                frames.push(f);
            }
        }
        (Synth { w, frames }, truth)
    }

    fn detrend(t: &[f64], w: usize, h: usize) -> Vec<f64> {
        let (mut gx, mut gy) = (0.0, 0.0);
        for y in 0..h {
            for x in 0..w - 1 {
                gx += t[y * w + x + 1] - t[y * w + x];
            }
        }
        for y in 0..h - 1 {
            for x in 0..w {
                gy += t[(y + 1) * w + x] - t[y * w + x];
            }
        }
        let (gx, gy) = (gx / ((w - 1) * h) as f64, gy / (w * (h - 1)) as f64);
        t.iter().enumerate().map(|(i, v)| v - gx * (i % w) as f64 - gy * (i / w) as f64).collect()
    }

    fn pearson(a: &[f64], b: &[f32]) -> f64 {
        let n = a.len() as f64;
        let ma = a.iter().sum::<f64>() / n;
        let mb = b.iter().map(|&v| v as f64).sum::<f64>() / n;
        let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
        for (x, &y) in a.iter().zip(b) {
            let (p, q) = (x - ma, y as f64 - mb);
            sab += p * q;
            saa += p * p;
            sbb += q * q;
        }
        sab / (saa * sbb).sqrt()
    }

    #[test]
    fn recovers_rotation_and_potential() {
        for (rot, flip) in [(0.0, false), (37.0, false), (200.0, true)] {
            let (src, truth) = synth(32, 24, 40, rot, flip);
            let r = analyze(&src, &Req { token: 0, c: 0, scan: [32, 24], detectors: vec![], center: None, rotation: None, flip: None }).unwrap();
            assert!((r.disk[0] - 20.0).abs() < 0.5 && (r.disk[2] - 6.0).abs() < 1.0, "disk {:?}", r.disk);
            let d = (r.rotation - rot).rem_euclid(360.0);
            assert!(d.min(360.0 - d) < 2.0 && r.flip == flip, "rot {rot} flip {flip}: got {} {}", r.rotation, r.flip);
            // The mean shift is removed (it can't be told from a centre
            // offset), and with it the potential's mean slope over the scan.
            let cc = pearson(&detrend(&truth, 32, 24), &r.idpc);
            assert!(cc > 0.95, "iDPC correlation {cc}");
            assert_eq!(r.images.len(), 2);
        }
    }

    #[test]
    fn integrates_a_gradient() {
        let (w, h) = (20, 13);
        let f = |x: f64, y: f64| (x * 0.3).sin() + 0.02 * y * y;
        let mut u = vec![0.0; w * h];
        let mut v = vec![0.0; w * h];
        let mut t = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (x as f64, y as f64);
                u[y * w + x] = 0.3 * (a * 0.3).cos();
                v[y * w + x] = 0.04 * b;
                t[y * w + x] = f(a, b);
            }
        }
        let p = integrate(&u, &v, w, h);
        let pf: Vec<f32> = p.iter().map(|&a| a as f32).collect();
        assert!(pearson(&t, &pf) > 0.99, "{}", pearson(&t, &pf));
    }

    #[test]
    fn guesses_scans() {
        assert_eq!(guess_scan(600, 1), [25, 24]);
        assert_eq!(guess_scan(16384, 1), [128, 128]);
        assert_eq!(guess_scan(30, 20), [30, 20]);
        assert_eq!(guess_scan(1, 31), [31, 1]);
    }

    /// Merlin .mib files (one or more, in scan order) as patterns.
    struct Mib {
        files: Vec<(std::path::PathBuf, usize)>,
        head: usize,
        w: usize,
        bytes: usize,
    }

    impl Frames for Mib {
        fn count(&self) -> usize {
            self.files.iter().map(|f| f.1).sum()
        }
        fn dims(&self) -> [usize; 2] {
            [self.w, self.w]
        }
        fn read(&self, mut i: usize, out: &mut [f32]) -> Result<()> {
            use std::io::{Read, Seek, SeekFrom};
            for (p, n) in &self.files {
                if i < *n {
                    let mut f = std::fs::File::open(p)?;
                    let size = self.head + self.w * self.w * self.bytes;
                    f.seek(SeekFrom::Start((i * size + self.head) as u64))?;
                    let mut b = vec![0u8; self.w * self.w * self.bytes];
                    f.read_exact(&mut b)?;
                    for (k, o) in out.iter_mut().enumerate() {
                        *o = match self.bytes {
                            1 => b[k] as f32,
                            2 => u16::from_be_bytes([b[2 * k], b[2 * k + 1]]) as f32,
                            _ => u32::from_be_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]]) as f32,
                        };
                    }
                    return Ok(());
                }
                i -= n;
            }
            bail!("no frame")
        }
    }

    /// Runs on Merlin files in TMV_MIB_DIR (default*.mib in numeric order),
    /// scan TMV_SCAN ("128x128"); writes the result JSON to TMV_4D_OUT.
    #[test]
    #[ignore]
    fn bench_mib() {
        let dir = std::path::PathBuf::from(std::env::var("TMV_MIB_DIR").unwrap());
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "mib")).collect();
        let num = |p: &std::path::PathBuf| p.file_stem().unwrap().to_string_lossy().chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse::<usize>().unwrap_or(0);
        files.sort_by_key(num);
        let mut hdr = vec![0u8; 384];
        use std::io::Read;
        std::fs::File::open(&files[0]).unwrap().read_exact(&mut hdr).unwrap();
        let h = String::from_utf8_lossy(&hdr);
        let f: Vec<&str> = h.split(',').collect();
        let head: usize = f[2].trim().parse().unwrap();
        let w: usize = f[4].trim().parse().unwrap();
        let bytes = match f[6].trim() { "U08" => 1, "U16" => 2, _ => 4 };
        let size = head + w * w * bytes;
        let list = files.iter().map(|p| (p.clone(), std::fs::metadata(p).unwrap().len() as usize / size)).collect();
        let src = Mib { files: list, head, w, bytes };
        let scan: Vec<usize> = std::env::var("TMV_SCAN").unwrap_or("128x128".into()).split('x').map(|v| v.parse().unwrap()).collect();
        let r = analyze(&src, &Req { token: 0, c: 0, scan: [scan[0], scan[1]], detectors: vec![], center: None, rotation: None, flip: None }).unwrap();
        eprintln!("{} frames {}x{}: {:.0} ms, disk {:?}, rotation {:.2} flip {} gpu {}", src.count(), w, w, r.ms, r.disk, r.rotation, r.flip, r.gpu);
        if let Ok(out) = std::env::var("TMV_4D_OUT") {
            std::fs::write(out, serde_json::to_string(&r).unwrap()).unwrap();
        }
    }
}
