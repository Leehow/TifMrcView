//! Denoising (Pro): classical filters for noisy EM images and stacks, with
//! a preview of the visible region and a float32 MRC of the whole result.
//! The image itself is never changed.
//!
//! Methods, from cheapest to best:
//! - "gaussian": Gaussian blur of `strength` pixels.
//! - "median": median of a square of radius `strength` (rounded); removes
//!   hot pixels and salt-and-pepper noise.
//! - "diffusion": Perona–Malik anisotropic diffusion. Smooths along edges
//!   but not across them: flow between neighbours falls off with their
//!   difference as 1 / (1 + (d / K)²), with K = `strength` × noise σ,
//!   repeated `iterations` times (time step 0.2).
//! - "nlm": non-local means. Each pixel becomes the weighted mean of the
//!   pixels within `search` of it, weighted by how alike the patches of
//!   radius `patch` around the two are: exp(−max(d² − 2σ², 0) / h²) with
//!   d² the mean squared patch difference and h = `strength` × σ. Done per
//!   offset over the whole image, with the patch sums as box filters.
//!
//! σ, the noise level, is estimated from each slice (after any Z averaging)
//! by the median absolute response to the Laplacian-difference mask
//! [1 −2 1; −2 4 −2; 1 −2 1], which mostly cancels smooth signal. With
//! `slab` > 0, each slice is first averaged with `slab` neighbours on each
//! side (stacks of thin slices, tomograms).
//!
//! NLM runs on the GPU (Burn, `denoise_gpu`) when there is one, in tiles;
//! the CPU code here is the reference and fallback.

use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::Deserialize;

use crate::image::OpenImage;

/// Set to stop a running save; cleared when one starts.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// Pixels the self-supervised network reads around a region
/// (`denoise_learn::MARGIN`).
const LEARNED_MARGIN: usize = 32;

/// Largest preview side, in image pixels (the middle of the view).
const PREVIEW_MAX: usize = 2048;

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    Gaussian,
    Median,
    Diffusion,
    Nlm,
    /// The self-supervised network last trained (`denoise_learn`, Pro);
    /// `strength` mixes its result with the input (1 = all network).
    Learned,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub method: Method,
    /// Gaussian σ (px), median radius (px), diffusion K or NLM h (both in
    /// noise σ).
    pub strength: f32,
    /// Diffusion steps.
    #[serde(default = "ten")]
    pub iterations: usize,
    /// NLM patch radius.
    #[serde(default = "one")]
    pub patch: usize,
    /// NLM search radius.
    #[serde(default = "seven")]
    pub search: usize,
    /// Slices averaged on each side first (0 = none).
    #[serde(default)]
    pub slab: usize,
    /// "cpu" keeps everything on the CPU, "gpu" uses the GPU for NLM whenever
    /// there is one, anything else ("auto") where it is faster.
    #[serde(default)]
    pub device: String,
}

fn ten() -> usize {
    10
}
fn one() -> usize {
    1
}
fn seven() -> usize {
    7
}

impl Params {
    /// Pixels around a region that its result depends on.
    fn margin(&self) -> usize {
        match self.method {
            Method::Gaussian => kernel_radius(self.strength) + 1,
            Method::Median => self.median_radius(),
            Method::Diffusion => self.iterations.min(200) + 1,
            Method::Nlm => self.search.min(20) + self.patch.min(5) + 1,
            Method::Learned => LEARNED_MARGIN * learned_bin(),
        }
    }

    fn median_radius(&self) -> usize {
        (self.strength.round().max(1.0) as usize).min(7)
    }
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PreviewReq {
    pub token: u64,
    pub z: usize,
    pub c: usize,
    pub t: usize,
    /// Visible region [x, y, w, h]; the preview covers at most
    /// `PREVIEW_MAX` pixels of each side, around its middle.
    pub region: [f64; 4],
    pub params: Params,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SaveReq {
    pub token: u64,
    /// Slices to write, in order; empty means all.
    #[serde(default)]
    pub zs: Vec<usize>,
    pub c: usize,
    pub t: usize,
    pub path: String,
    pub params: Params,
}

/// One denoised region and how it was made.
pub struct Denoised {
    pub rect: [usize; 4],
    pub values: Vec<f32>,
    pub sigma: f32,
    pub ms: f64,
    pub backend: &'static str,
}

impl Denoised {
    /// Reply layout: `u32 x, y, w, h`, `f32 ms, sigma`, 16 bytes of backend
    /// name (ASCII, zero padded), then w × h f32 values, all little-endian.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(40 + self.values.len() * 4);
        for v in self.rect {
            b.extend_from_slice(&(v as u32).to_le_bytes());
        }
        b.extend_from_slice(&(self.ms as f32).to_le_bytes());
        b.extend_from_slice(&self.sigma.to_le_bytes());
        let mut name = [0u8; 16];
        let n = self.backend.len().min(16);
        name[..n].copy_from_slice(&self.backend.as_bytes()[..n]);
        b.extend_from_slice(&name);
        for v in &self.values {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }
}

/// One slice of channel `c`, as f32.
fn plane(img: &OpenImage, c: usize, z: usize, t: usize) -> Result<Vec<f32>> {
    let info = img.info();
    let (page, s) = info.locate(c, z, t);
    let planes = img.page(page)?;
    let p = &planes[s.min(planes.len() - 1)];
    Ok((0..p.w * p.h)
        .into_par_iter()
        .map(|i| p.data.get_f64(i) as f32)
        .collect())
}

/// Slice `z`, averaged with `slab` slices on each side that exist.
pub(crate) fn input(
    img: &OpenImage,
    c: usize,
    z: usize,
    t: usize,
    slab: usize,
) -> Result<Vec<f32>> {
    let nz = img.info().size_z;
    let (a, b) = (z.saturating_sub(slab), (z + slab).min(nz - 1));
    let mut acc = plane(img, c, a, t)?;
    for k in a + 1..=b {
        let v = plane(img, c, k, t)?;
        acc.par_iter_mut().zip(&v).for_each(|(a, &v)| *a += v);
    }
    let n = (b - a + 1) as f32;
    if n > 1.0 {
        acc.par_iter_mut().for_each(|a| *a /= n);
    }
    Ok(acc)
}

fn crop(v: &[f32], w: usize, [x, y, cw, ch]: [usize; 4]) -> Vec<f32> {
    let mut out = Vec::with_capacity(cw * ch);
    for r in y..y + ch {
        out.extend_from_slice(&v[r * w + x..r * w + x + cw]);
    }
    out
}

/// Denoises the middle of the visible region of one slice.
pub fn preview(img: &Arc<OpenImage>, req: &PreviewReq) -> Result<Denoised> {
    let t0 = Instant::now();
    let info = img.info();
    let (w, h) = (info.width, info.height);
    let [rx, ry, rw, rh] = req.region;
    let x0 = rx.floor().clamp(0.0, w as f64) as usize;
    let y0 = ry.floor().clamp(0.0, h as f64) as usize;
    let x1 = (rx + rw).ceil().clamp(0.0, w as f64) as usize;
    let y1 = (ry + rh).ceil().clamp(0.0, h as f64) as usize;
    if x1 <= x0 || y1 <= y0 {
        bail!("区域在图像外");
    }
    let (cw, ch) = ((x1 - x0).min(PREVIEW_MAX), (y1 - y0).min(PREVIEW_MAX));
    let (cx, cy) = (x0 + (x1 - x0 - cw) / 2, y0 + (y1 - y0 - ch) / 2);
    let (values, sigma, backend) = run(img, req.c, req.z, req.t, &req.params, [cx, cy, cw, ch])?;
    Ok(Denoised {
        rect: [cx, cy, cw, ch],
        values,
        sigma,
        ms: t0.elapsed().as_secs_f64() * 1e3,
        backend,
    })
}

/// Whether the method is the trained network in movie mode, whose input is
/// the even and odd frame means rather than one slice. A single image (a
/// movie's saved result, say) goes through the same network as it is.
fn uses_frames(img: &OpenImage, p: &Params) -> bool {
    #[cfg(feature = "pro")]
    if p.method == Method::Learned && img.info().size_z > 1 {
        if let Some(m) = crate::denoise_learn::MODEL.lock().unwrap().as_ref() {
            return m.mode == crate::denoise_learn::Mode::Frames;
        }
    }
    let _ = (img, p);
    false
}

/// Means of the even and of the odd slices (frames 1, 3, 5… and 2, 4, 6…).
pub fn frame_halves(img: &OpenImage, c: usize, t: usize) -> Result<[Vec<f32>; 2]> {
    let nz = img.info().size_z;
    if nz < 2 {
        bail!("需要至少两帧");
    }
    let mut out = [Vec::new(), Vec::new()];
    for (k, half) in out.iter_mut().enumerate() {
        let zs: Vec<usize> = (k..nz).step_by(2).collect();
        let mut acc = plane(img, c, zs[0], t)?;
        for &z in &zs[1..] {
            let v = plane(img, c, z, t)?;
            acc.par_iter_mut().zip(&v).for_each(|(a, &v)| *a += v);
        }
        let n = zs.len() as f32;
        acc.par_iter_mut().for_each(|a| *a /= n);
        *half = acc;
    }
    Ok(out)
}

/// Denoises the `rect` of slice `z` (of the movie in frames mode): the
/// input with the margin the method reads, each part filtered, the parts
/// averaged. Returns the values, the noise σ and the backend.
fn run(
    img: &OpenImage,
    c: usize,
    z: usize,
    t: usize,
    p: &Params,
    rect: [usize; 4],
) -> Result<(Vec<f32>, f32, &'static str)> {
    let info = img.info();
    let (w, h) = (info.width, info.height);
    let parts: Vec<Vec<f32>> = if uses_frames(img, p) {
        frame_halves(img, c, t)?.into()
    } else {
        vec![input(img, c, z, t, p.slab)?]
    };
    let sigma = noise_sigma(&parts[0], w, h);
    let [cx, cy, cw, ch] = rect;
    let m = p.margin();
    let (ex, ey) = (cx.saturating_sub(m), cy.saturating_sub(m));
    let (ew, eh) = ((cx + cw + m).min(w) - ex, (cy + ch + m).min(h) - ey);
    let (mut sum, mut backend) = (vec![0f32; cw * ch], "CPU");
    for full in &parts {
        let part = if [ex, ey, ew, eh] == [0, 0, w, h] {
            full.clone()
        } else {
            crop(full, w, [ex, ey, ew, eh])
        };
        let (out, b) = filter(part, ew, eh, p, sigma)?;
        backend = b;
        sum.par_iter_mut()
            .zip(crop(&out, ew, [cx - ex, cy - ey, cw, ch]))
            .for_each(|(s, v)| *s += v);
    }
    let n = parts.len() as f32;
    if n > 1.0 {
        sum.par_iter_mut().for_each(|s| *s /= n);
    }
    Ok((sum, sigma, backend))
}

/// Denoises one whole image of `w` × `h` with noise level `sigma`; returns
/// the result and the backend that made it.
pub fn filter(
    v: Vec<f32>,
    w: usize,
    h: usize,
    p: &Params,
    sigma: f32,
) -> Result<(Vec<f32>, &'static str)> {
    let sigma = sigma.max(f32::MIN_POSITIVE);
    // Only NLM goes to the GPU: diffusion is a few cheap passes that run as
    // fast on the CPU (RTX 5080 even, M1 Max Metal 10–30× slower).
    #[cfg(feature = "pro")]
    if p.device != "cpu" && p.method == Method::Nlm {
        // On Apple GPUs (M1 Max) these unfused tensor passes are 1.5–2×
        // slower than the CPU code, so "auto" keeps NLM on the CPU there;
        // "gpu" forces it.
        if let Some((dev, name)) =
            crate::ctf_gpu::device().filter(|(_, n)| p.device == "gpu" || *n != "Metal")
        {
            match crate::ctf_gpu::guarded(|| nlm_gpu_tiled(dev, &v, w, h, p, sigma)) {
                Ok(out) => return Ok((out, name)),
                Err(e) => eprintln!("denoise: GPU failed, using the CPU: {e:#}"),
            }
        }
    }
    if p.method == Method::Learned {
        return learned(v, w, h, p);
    }
    let out = match p.method {
        Method::Learned => unreachable!(),
        Method::Gaussian => gaussian(&v, w, h, p.strength),
        Method::Median => median(&v, w, h, p.median_radius()),
        Method::Diffusion => diffusion(v, w, h, p.strength * sigma, p.iterations.min(200)),
        Method::Nlm => nlm(
            &v,
            w,
            h,
            p.patch.min(5),
            p.search.min(20),
            p.strength * sigma,
            sigma,
        ),
    };
    Ok((out, "CPU"))
}

/// The trained network's result, mixed with the input by `strength`.
/// How much the trained model bins first (its margin grows by as much).
fn learned_bin() -> usize {
    #[cfg(feature = "pro")]
    if let Some(m) = crate::denoise_learn::MODEL.lock().unwrap().as_ref() {
        return m.bin.max(1);
    }
    1
}

fn learned(v: Vec<f32>, w: usize, h: usize, p: &Params) -> Result<(Vec<f32>, &'static str)> {
    #[cfg(feature = "pro")]
    {
        let Some(m) = crate::denoise_learn::MODEL.lock().unwrap().clone() else {
            bail!("先训练自监督模型")
        };
        let (mut out, backend) = if p.device == "cpu" {
            (m.apply(&burn::tensor::Device::flex(), &v, w, h), "CPU")
        } else {
            crate::classify::on_device(|dev| Ok(m.apply(dev, &v, w, h)))?
        };
        let k = p.strength.clamp(0.0, 1.0);
        if k < 1.0 {
            out.par_iter_mut()
                .zip(&v)
                .for_each(|(o, &x)| *o = k * *o + (1.0 - k) * x);
        }
        Ok((out, backend))
    }
    #[cfg(not(feature = "pro"))]
    {
        let _ = (v, w, h, p);
        bail!("自监督降噪只在 Pro 版里有")
    }
}

/// NLM on the GPU in tiles of at most 1024² pixels plus the margin they
/// read, so each tile batches many offsets: one offset at a time over a
/// 4096² image made Metal (M1 Max) 3× slower than the CPU.
#[cfg(feature = "pro")]
fn nlm_gpu_tiled(
    dev: &burn::tensor::Device,
    v: &[f32],
    w: usize,
    h: usize,
    p: &Params,
    sigma: f32,
) -> Vec<f32> {
    tiled(dev, v, w, h, p, sigma, 1024)
}

#[cfg(feature = "pro")]
fn tiled(
    dev: &burn::tensor::Device,
    v: &[f32],
    w: usize,
    h: usize,
    p: &Params,
    sigma: f32,
    tile: usize,
) -> Vec<f32> {
    let m = p.margin();
    let mut out = vec![0f32; w * h];
    for ty in (0..h).step_by(tile) {
        for tx in (0..w).step_by(tile) {
            let (cw, ch) = (tile.min(w - tx), tile.min(h - ty));
            let (ex, ey) = (tx.saturating_sub(m), ty.saturating_sub(m));
            let (ew, eh) = ((tx + cw + m).min(w) - ex, (ty + ch + m).min(h) - ey);
            let part = crop(v, w, [ex, ey, ew, eh]);
            let res = crate::denoise_gpu::filter(dev, &part, ew, eh, p, sigma);
            for r in 0..ch {
                let src = (ty - ey + r) * ew + (tx - ex);
                out[(ty + r) * w + tx..(ty + r) * w + tx + cw].copy_from_slice(&res[src..src + cw]);
            }
        }
    }
    out
}

/// Noise σ from the median absolute Laplacian-difference response (whose
/// weights square-sum to 36), on at most about a million pixels.
pub fn noise_sigma(v: &[f32], w: usize, h: usize) -> f32 {
    if w < 3 || h < 3 {
        return 0.0;
    }
    let step = (((w - 2) * (h - 2)) as f64 / 1e6).sqrt().ceil().max(1.0) as usize;
    let mut r: Vec<f32> = (1..h - 1)
        .step_by(step)
        .flat_map(|y| (1..w - 1).step_by(step).map(move |x| (x, y)))
        .map(|(x, y)| {
            let at = |dx: isize, dy: isize| {
                v[(y as isize + dy) as usize * w + (x as isize + dx) as usize]
            };
            (at(-1, -1) + at(1, -1) + at(-1, 1) + at(1, 1)
                - 2.0 * (at(0, -1) + at(0, 1) + at(-1, 0) + at(1, 0))
                + 4.0 * at(0, 0))
            .abs()
        })
        .filter(|x| x.is_finite())
        .collect();
    if r.is_empty() {
        return 0.0;
    }
    let k = r.len() / 2;
    let (_, med, _) = r.select_nth_unstable_by(k, f32::total_cmp);
    1.4826 * *med / 6.0
}

/// How far the noise departs from independent pixels: the correlation of
/// horizontally neighbouring high-pass residuals (value minus the mean of
/// its 3 × 3 neighbourhood), minus what white noise gives (−1/6).
/// Filtered or binned-with-interpolation data shows values far from 0,
/// which defeats Noise2Void.
pub fn noise_correlation(v: &[f32], w: usize, h: usize) -> f32 {
    if w < 4 || h < 3 {
        return 0.0;
    }
    let step = (((w - 2) * (h - 2)) as f64 / 1e6).sqrt().ceil().max(1.0) as usize;
    let res = |x: usize, y: usize| {
        let mut s = 0f32;
        for dy in 0..3 {
            for dx in 0..3 {
                s += v[(y + dy - 1) * w + x + dx - 1];
            }
        }
        v[y * w + x] - s / 9.0
    };
    let (mut sab, mut saa, mut sbb) = (0f64, 0f64, 0f64);
    for y in (1..h - 1).step_by(step) {
        for x in (1..w - 2).step_by(step) {
            let (a, b) = (res(x, y) as f64, res(x + 1, y) as f64);
            if a.is_finite() && b.is_finite() {
                sab += a * b;
                saa += a * a;
                sbb += b * b;
            }
        }
    }
    let r = if saa > 0.0 && sbb > 0.0 {
        (sab / (saa * sbb).sqrt()) as f32
    } else {
        -1.0 / 6.0
    };
    // White noise gives −12/81 ÷ 72/81 = −1/6.
    r + 1.0 / 6.0
}

pub fn kernel_radius(sigma: f32) -> usize {
    ((3.0 * sigma).ceil() as usize).clamp(1, 64)
}

fn gauss_kernel(sigma: f32) -> Vec<f32> {
    let r = kernel_radius(sigma) as isize;
    let s = sigma.max(1e-3);
    let k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * s * s)).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    k.into_iter().map(|x| x / sum).collect()
}

#[inline]
fn clampi(i: isize, n: usize) -> usize {
    i.clamp(0, n as isize - 1) as usize
}

/// Separable convolution with `k` (odd length), edges repeated.
fn separable(v: &[f32], w: usize, h: usize, k: &[f32]) -> Vec<f32> {
    let r = (k.len() / 2) as isize;
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &v[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            *o = k
                .iter()
                .enumerate()
                .map(|(j, &kj)| kj * src[clampi(x as isize + j as isize - r, w)])
                .sum();
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (j, &kj) in k.iter().enumerate() {
            let s = clampi(y as isize + j as isize - r, h);
            for (o, &t) in row.iter_mut().zip(&tmp[s * w..(s + 1) * w]) {
                *o += kj * t;
            }
        }
    });
    out
}

pub fn gaussian(v: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.0 {
        return v.to_vec();
    }
    separable(v, w, h, &gauss_kernel(sigma))
}

pub fn median(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let r = r as isize;
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w)
        .enumerate()
        .for_each_init(Vec::new, |win, (y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                win.clear();
                for dy in -r..=r {
                    let yy = clampi(y as isize + dy, h);
                    for dx in -r..=r {
                        win.push(v[yy * w + clampi(x as isize + dx, w)]);
                    }
                }
                let k = win.len() / 2;
                *o = *win.select_nth_unstable_by(k, f32::total_cmp).1;
            }
        });
    out
}

/// Perona–Malik diffusion with conductance 1 / (1 + (d / k)²), 4 neighbours,
/// no flow across the image edge.
pub fn diffusion(mut u: Vec<f32>, w: usize, h: usize, k: f32, iterations: usize) -> Vec<f32> {
    const DT: f32 = 0.2;
    let inv_k2 = 1.0 / (k * k).max(f32::MIN_POSITIVE);
    let flow = move |d: f32| d / (1.0 + d * d * inv_k2);
    let mut next = vec![0f32; w * h];
    for _ in 0..iterations {
        next.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let c = &u[y * w..(y + 1) * w];
            let up = &u[y.saturating_sub(1) * w..][..w];
            let dn = &u[(y + 1).min(h - 1) * w..][..w];
            for x in 0..w {
                let v = c[x];
                let l = c[x.saturating_sub(1)];
                let r = c[(x + 1).min(w - 1)];
                row[x] = v + DT * (flow(up[x] - v) + flow(dn[x] - v) + flow(l - v) + flow(r - v));
            }
        });
        std::mem::swap(&mut u, &mut next);
    }
    u
}

/// Running box mean of radius `r` along rows then columns, edges repeated.
fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let k = vec![1.0 / (2 * r + 1) as f32; 2 * r + 1];
    separable(v, w, h, &k)
}

/// Non-local means; see the module notes.
pub fn nlm(
    v: &[f32],
    w: usize,
    h: usize,
    patch: usize,
    search: usize,
    hh: f32,
    sigma: f32,
) -> Vec<f32> {
    let inv_h2 = 1.0 / (hh * hh).max(f32::MIN_POSITIVE);
    let two_s2 = 2.0 * sigma * sigma;
    let mut num = vec![0f32; w * h];
    let mut den = vec![0f32; w * h];
    let s = search as isize;
    let mut d = vec![0f32; w * h];
    for oy in -s..=s {
        for ox in -s..=s {
            d.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                let q = clampi(y as isize + oy, h);
                for (x, o) in row.iter_mut().enumerate() {
                    let e = v[y * w + x] - v[q * w + clampi(x as isize + ox, w)];
                    *o = e * e;
                }
            });
            let dd = box_mean(&d, w, h, patch);
            num.par_chunks_mut(w)
                .zip(den.par_chunks_mut(w))
                .enumerate()
                .for_each(|(y, (nr, dr))| {
                    let q = clampi(y as isize + oy, h);
                    for x in 0..w {
                        let wt = (-(dd[y * w + x] - two_s2).max(0.0) * inv_h2).exp();
                        nr[x] += wt * v[q * w + clampi(x as isize + ox, w)];
                        dr[x] += wt;
                    }
                });
        }
    }
    num.par_iter_mut().zip(&den).for_each(|(n, &d)| *n /= d);
    num
}

/// Writes the denoised slices as a float32 MRC (the pixel size is kept).
pub fn save(
    img: &Arc<OpenImage>,
    req: &SaveReq,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<&'static str> {
    CANCEL.store(false, Ordering::SeqCst);
    let info = img.info();
    let (w, h) = (info.width, info.height);
    // A movie denoised from its frame halves gives one image.
    let zs: Vec<usize> = if uses_frames(img, &req.params) {
        vec![0]
    } else if req.zs.is_empty() {
        (0..info.size_z).collect()
    } else {
        req.zs.clone()
    };
    if zs.iter().any(|&z| z >= info.size_z) {
        bail!("层号超出范围");
    }
    let tmp = format!("{}.part", req.path);
    let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    f.write_all(&[0u8; 1024])?;
    let (mut lo, mut hi, mut total, mut total2) = (f64::MAX, f64::MIN, 0f64, 0f64);
    let mut backend = "CPU";
    for (i, &z) in zs.iter().enumerate() {
        if CANCEL.load(Ordering::SeqCst) {
            drop(f);
            let _ = std::fs::remove_file(&tmp);
            bail!("已取消");
        }
        let (out, _, b) = run(img, req.c, z, req.t, &req.params, [0, 0, w, h])?;
        backend = b;
        let (a, b, c, d) = out
            .par_iter()
            .fold(
                || (f64::MAX, f64::MIN, 0f64, 0f64),
                |(a, b, c, d), &v| {
                    let v = v as f64;
                    (a.min(v), b.max(v), c + v, d + v * v)
                },
            )
            .reduce(
                || (f64::MAX, f64::MIN, 0f64, 0f64),
                |x, y| (x.0.min(y.0), x.1.max(y.1), x.2 + y.2, x.3 + y.3),
            );
        lo = lo.min(a);
        hi = hi.max(b);
        total += c;
        total2 += d;
        let mut bytes = vec![0u8; out.len() * 4];
        bytes
            .par_chunks_mut(4)
            .zip(&out)
            .for_each(|(b, v)| b.copy_from_slice(&v.to_le_bytes()));
        f.write_all(&bytes)?;
        progress(i + 1, zs.len());
    }
    let count = (w * h * zs.len()) as f64;
    let mean = total / count;
    let rms = (total2 / count - mean * mean).max(0.0).sqrt();
    let p = &req.params;
    let label = match p.method {
        Method::Gaussian => format!("TifMrcView: denoised, gaussian {:.2} px", p.strength),
        Method::Median => format!("TifMrcView: denoised, median r {}", p.median_radius()),
        Method::Diffusion => format!(
            "TifMrcView: denoised, diffusion K {:.2} sd x{}",
            p.strength, p.iterations
        ),
        Method::Nlm => format!(
            "TifMrcView: denoised, NLM h {:.2} sd p{} s{}",
            p.strength, p.patch, p.search
        ),
        Method::Learned => format!(
            "TifMrcView: denoised, self-supervised{} mix {:.2}",
            if uses_frames(img, p) {
                " N2N frames"
            } else {
                ""
            },
            p.strength
        ),
    } + &if p.slab > 0 {
        format!(" z+-{}", p.slab)
    } else {
        String::new()
    };
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&crate::align::mrc_header(
        w,
        h,
        zs.len(),
        crate::align::pixel_a(info),
        [lo as f32, hi as f32, mean as f32, rms as f32],
        &label,
    ))?;
    f.flush()?;
    drop(f);
    std::fs::rename(&tmp, &req.path)?;
    Ok(backend)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn mix(mut z: u64) -> u64 {
        // splitmix64
        z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Standard normal noise (Box–Muller) from a hash of (seed, i).
    pub fn gauss(seed: u64, i: u64) -> f32 {
        let u = |k: u64| {
            ((mix(seed.wrapping_mul(0x1000_0000_01b3) ^ k) >> 11) as f64 + 0.5)
                / (1u64 << 53) as f64
        };
        ((-2.0 * u(2 * i).ln()).sqrt() * (2.0 * std::f64::consts::PI * u(2 * i + 1)).cos()) as f32
    }

    /// Discs and a bar on a flat background (0 and 1), plus noise of σ `s`.
    pub fn phantom(w: usize, h: usize, s: f32) -> (Vec<f32>, Vec<f32>) {
        let clean: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let disc = |cx: f32, cy: f32, r: f32| ((x - cx).powi(2) + (y - cy).powi(2)) < r * r;
                let bar =
                    (x - w as f32 * 0.7).abs() < 3.0 && y > h as f32 * 0.2 && y < h as f32 * 0.8;
                if disc(w as f32 * 0.3, h as f32 * 0.3, 12.0)
                    || disc(w as f32 * 0.35, h as f32 * 0.7, 7.0)
                    || bar
                {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let noisy = clean
            .iter()
            .enumerate()
            .map(|(i, &c)| c + s * gauss(7, i as u64))
            .collect();
        (clean, noisy)
    }

    pub fn rmse(a: &[f32], b: &[f32]) -> f32 {
        (a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f32>() / a.len() as f32).sqrt()
    }

    #[test]
    fn estimates_the_noise_level() {
        let (_, noisy) = phantom(256, 256, 0.3);
        let s = noise_sigma(&noisy, 256, 256);
        assert!((s - 0.3).abs() < 0.03, "{s}");
        let (clean, _) = phantom(64, 64, 0.0);
        assert!(noise_sigma(&clean, 64, 64) < 1e-6);
    }

    #[test]
    fn every_method_lowers_the_error() {
        let (w, h) = (128, 96);
        let (clean, noisy) = phantom(w, h, 0.3);
        let sigma = noise_sigma(&noisy, w, h);
        let before = rmse(&clean, &noisy);
        let p = |method, strength| Params {
            method,
            strength,
            iterations: 15,
            patch: 2,
            search: 6,
            slab: 0,
            device: "cpu".into(),
        };
        let mut errs = Vec::new();
        for (m, s) in [
            (Method::Gaussian, 1.0),
            (Method::Median, 1.0),
            (Method::Diffusion, 2.0),
            (Method::Nlm, 0.8),
        ] {
            let (out, b) = filter(noisy.clone(), w, h, &p(m, s), sigma).unwrap();
            assert_eq!(b, "CPU");
            let e = rmse(&clean, &out);
            errs.push(e);
            assert!(e < before * 0.75, "{m:?}: {e} vs {before}");
        }
        // Non-local means is the best of them on piecewise flat images.
        assert!(errs[3] < errs[0] && errs[3] < errs[1], "{errs:?}");
    }

    #[test]
    fn flat_images_stay_flat_and_median_drops_outliers() {
        let (w, h) = (20, 10);
        let mut v = vec![2.5f32; w * h];
        for m in [Method::Gaussian, Method::Diffusion, Method::Nlm] {
            let p = Params {
                method: m,
                strength: 1.0,
                iterations: 5,
                patch: 1,
                search: 3,
                slab: 0,
                device: "cpu".into(),
            };
            let (out, _) = filter(v.clone(), w, h, &p, 0.1).unwrap();
            assert!(out.iter().all(|&x| (x - 2.5).abs() < 1e-5), "{m:?}");
        }
        v[55] = 1000.0;
        let out = median(&v, w, h, 1);
        assert_eq!(out[55], 2.5);
    }

    /// Tiles see the margin they read, so they join without seams.
    #[cfg(feature = "pro")]
    #[test]
    fn gpu_tiles_match_the_whole_image() {
        let dev = burn::tensor::Device::flex();
        let (w, h) = (50, 37);
        let (_, noisy) = phantom(w, h, 0.3);
        let p = Params {
            method: Method::Nlm,
            strength: 0.8,
            iterations: 0,
            patch: 1,
            search: 3,
            slab: 0,
            device: String::new(),
        };
        let whole = crate::denoise_gpu::filter(&dev, &noisy, w, h, &p, 0.3);
        let parts = tiled(&dev, &noisy, w, h, &p, 0.3, 16);
        let worst = whole
            .iter()
            .zip(&parts)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn correlated_noise_is_flagged() {
        let (w, h) = (128, 128);
        let mut seed = 1u64;
        let noisy: Vec<f32> = (0..w * h)
            .map(|_| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (seed >> 40) as f32 / (1u64 << 24) as f32
            })
            .collect();
        assert!(
            noise_correlation(&noisy, w, h).abs() < 0.08,
            "{}",
            noise_correlation(&noisy, w, h)
        );
        let blurred = gaussian(&noisy, w, h, 1.0);
        assert!(
            noise_correlation(&blurred, w, h) > 0.2,
            "{}",
            noise_correlation(&blurred, w, h)
        );
    }

    #[test]
    fn preview_bytes_have_the_documented_layout() {
        let d = Denoised {
            rect: [1, 2, 2, 1],
            values: vec![0.5, -1.0],
            sigma: 0.25,
            ms: 3.0,
            backend: "CPU",
        };
        let b = d.to_bytes();
        assert_eq!(b.len(), 40 + 8);
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), 2);
        assert_eq!(f32::from_le_bytes(b[20..24].try_into().unwrap()), 0.25);
        assert_eq!(&b[24..27], b"CPU");
        assert_eq!(f32::from_le_bytes(b[44..48].try_into().unwrap()), -1.0);
    }
}

/// Error and time of every method on a noisy synthetic image:
/// cargo test --release --lib denoise::bench -- --ignored --nocapture
/// (TMV_DENOISE_N sets the size, default 1024). With TMV_DENOISE_FILE=<image>
/// it times the middle slice of that file instead and reports the noise σ
/// left in each result.
#[cfg(test)]
mod bench {
    use super::tests::{phantom, rmse};
    use super::*;

    #[test]
    #[ignore]
    fn bench() {
        if let Ok(path) = std::env::var("TMV_DENOISE_FILE") {
            let src = crate::formats::open(std::path::Path::new(&path)).unwrap();
            let info = src.info().clone();
            let (w, h) = (info.width, info.height);
            let (page, s) = info.locate(0, info.size_z / 2, 0);
            let planes = src.read_page(page).unwrap();
            let p = &planes[s.min(planes.len() - 1)];
            let v: Vec<f32> = (0..w * h).map(|i| p.data.get_f64(i) as f32).collect();
            let sigma = noise_sigma(&v, w, h);
            println!(
                "{path}: {w}×{h}, slice {} of {}, noise σ {sigma:.4}",
                info.size_z / 2,
                info.size_z
            );
            for (m, s, it) in [
                (Method::Gaussian, 1.5, 0),
                (Method::Median, 2.0, 0),
                (Method::Diffusion, 2.0, 20),
                (Method::Nlm, 0.8, 0),
            ] {
                for device in ["cpu", "auto"] {
                    if device == "auto" && m != Method::Nlm {
                        continue;
                    }
                    let p = Params {
                        method: m,
                        strength: s,
                        iterations: it,
                        patch: 2,
                        search: 7,
                        slab: 0,
                        device: device.into(),
                    };
                    // Twice: the first GPU call includes compiling the kernels.
                    for run in 0..2 {
                        let t0 = Instant::now();
                        let (out, b) = filter(v.clone(), w, h, &p, sigma).unwrap();
                        println!(
                            "  {m:?} {s} on {b} (run {run}): {:.0} ms, σ left {:.4}",
                            t0.elapsed().as_secs_f64() * 1e3,
                            noise_sigma(&out, w, h)
                        );
                    }
                }
            }
            return;
        }
        let n: usize = std::env::var("TMV_DENOISE_N")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1024);
        for noise in [0.3f32, 1.0] {
            let (clean, noisy) = phantom(n, n, noise);
            let sigma = noise_sigma(&noisy, n, n);
            println!(
                "{n}² noise σ {noise} (estimated {sigma:.3}): input RMSE {:.3}",
                rmse(&clean, &noisy)
            );
            for (m, s, it) in [
                (Method::Gaussian, 1.5, 0),
                (Method::Median, 2.0, 0),
                (Method::Diffusion, 2.0, 20),
                (Method::Nlm, 0.8, 0),
            ] {
                for device in ["cpu", "auto"] {
                    if device == "auto" && m != Method::Nlm {
                        continue;
                    }
                    let p = Params {
                        method: m,
                        strength: s,
                        iterations: it,
                        patch: 2,
                        search: 7,
                        slab: 0,
                        device: device.into(),
                    };
                    let t0 = Instant::now();
                    let (out, b) = filter(noisy.clone(), n, n, &p, sigma).unwrap();
                    println!(
                        "  {m:?} {s} on {b}: RMSE {:.3} in {:.0} ms",
                        rmse(&clean, &out),
                        t0.elapsed().as_secs_f64() * 1e3
                    );
                }
            }
        }
    }
}
