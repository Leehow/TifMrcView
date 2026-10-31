//! Missing-wedge correction for tomograms (Pro), after the method of IsoNet
//! (Liu et al., Nature Communications 2022), written here from the paper.
//!
//! A tilt series covers only ±60° or so, so the tomogram's Fourier transform
//! is empty in a wedge around the beam axis: objects look stretched along
//! the beam and lose the faces that point along it. The data cannot show
//! what is missing, but the same kind of object appears in every
//! orientation, and turning a subvolume turns its wedge too. So:
//!
//! 1. Cubes are cut from the parts of the tomogram with most structure.
//! 2. Each cube is turned by one of the 20 right-angle rotations that move
//!    the wedge somewhere else, and the wedge is cut out of the turned cube
//!    again (Fourier mask). A 3D U-Net learns to map the twice-cut cube back
//!    to the turned one: to fill in a wedge from what lies around it.
//! 3. The network's result for each cube keeps the measured Fourier
//!    components of the cube and adds its own only inside the wedge; the
//!    next round trains on these filled cubes, so the targets improve too.
//! 4. Optionally, noise shaped like the data's (wedge-cut) is added to the
//!    inputs in the later rounds, so the network also learns to denoise.
//!
//! Finally the network is applied to the whole tomogram in overlapping
//! tiles. The beam axis, the tilt axis and the tilt range can be measured
//! from the tomogram's power spectrum (`detect`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use burn::module::Module;
use burn::nn::conv::{Conv3d, Conv3dConfig};
use burn::nn::PaddingConfig3d;
use burn::optim::{AdamConfig, GradientsParams};
use burn::tensor::activation::leaky_relu;
use burn::tensor::{Device, Tensor, TensorData};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::fft::Fft;
use crate::image::OpenImage;

/// Set to stop a running training or save.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// A volume, X fastest, then Y, then Z.
pub struct Vol {
    pub n: [usize; 3],
    pub v: Vec<f32>,
}

impl Vol {
    fn at(&self, p: [usize; 3]) -> f32 {
        self.v[(p[2] * self.n[1] + p[1]) * self.n[0] + p[0]]
    }

    /// The `side`³ cube at `origin`, mirrored where it leaves the volume.
    fn cube(&self, origin: [isize; 3], side: [usize; 3]) -> Vec<f32> {
        let idx: [Vec<usize>; 3] = std::array::from_fn(|a| (0..side[a]).map(|i| reflect(origin[a] + i as isize, self.n[a])).collect());
        let mut out = vec![0f32; side[0] * side[1] * side[2]];
        out.par_chunks_mut(side[0] * side[1]).enumerate().for_each(|(z, plane)| {
            for (y, row) in plane.chunks_mut(side[0]).enumerate() {
                for (x, o) in row.iter_mut().enumerate() {
                    *o = self.at([idx[0][x], idx[1][y], idx[2][z]]);
                }
            }
        });
        out
    }
}

fn reflect(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let p = 2 * n as isize - 2;
    let k = i.rem_euclid(p);
    (if k < n as isize { k } else { p - k }) as usize
}

/// Reads channel `c`, time `t` of a stack as a volume, averaged over
/// `bin`³ blocks, and standardised (mean 0, spread 1); returns it with the
/// mean and spread taken off.
pub fn load(img: &OpenImage, c: usize, t: usize, bin: usize) -> Result<(Vol, f32, f32)> {
    let info = img.info();
    let (w, h, d) = (info.width, info.height, info.size_z);
    let bin = bin.max(1);
    let n = [w / bin, h / bin, d / bin];
    if n.iter().any(|&k| k < 16) {
        bail!("体积太小（每个方向分箱后至少 16 像素）");
    }
    if n[0] * n[1] * n[2] > 800_000_000 {
        bail!("体积太大，请选更大的分箱");
    }
    let mut v = vec![0f32; n[0] * n[1] * n[2]];
    let scale = 1.0 / (bin * bin * bin) as f32;
    for z in 0..n[2] * bin {
        if CANCEL.load(Ordering::SeqCst) {
            bail!("已取消");
        }
        let s = crate::denoise::input(img, c, z, t, 0)?;
        let plane = &mut v[(z / bin) * n[0] * n[1]..][..n[0] * n[1]];
        plane.par_chunks_mut(n[0]).enumerate().for_each(|(y, row)| {
            for dy in 0..bin {
                let src = &s[(y * bin + dy) * w..];
                for (x, o) in row.iter_mut().enumerate() {
                    let mut acc = 0f32;
                    for dx in 0..bin {
                        let val = src[x * bin + dx];
                        acc += if val.is_finite() { val } else { 0.0 };
                    }
                    *o += acc * scale;
                }
            }
        });
    }
    let (mean, std) = stats(&v);
    v.par_iter_mut().for_each(|x| *x = (*x - mean) / std);
    Ok((Vol { n, v }, mean, std))
}

fn stats(v: &[f32]) -> (f32, f32) {
    let step = (v.len() / 2_000_000).max(1);
    let (mut k, mut s, mut s2) = (0f64, 0f64, 0f64);
    for &x in v.iter().step_by(step) {
        k += 1.0;
        s += x as f64;
        s2 += x as f64 * x as f64;
    }
    let mean = s / k.max(1.0);
    let sd = (s2 / k.max(1.0) - mean * mean).max(0.0).sqrt();
    (mean as f32, if sd > 1e-12 { sd as f32 } else { 1.0 })
}

/// Gaussian blur of σ voxels (separable, mirrored edges): a gentle
/// low-pass so that very noisy tomograms do not teach the network to fill
/// the wedge with noise.
pub fn smooth(vol: &Vol, sigma: f32) -> Vol {
    if sigma <= 0.0 {
        return Vol { n: vol.n, v: vol.v.clone() };
    }
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / sum).collect();
    let n = vol.n;
    let mut v = vol.v.clone();
    for axis in 0..3 {
        let stride = [1, n[0], n[0] * n[1]][axis];
        let src = v.clone();
        v.par_iter_mut().enumerate().for_each(|(i, o)| {
            let c = (i / stride) % n[axis];
            let base = i - c * stride;
            *o = k.iter().enumerate().map(|(j, w)| w * src[base + reflect(c as isize + j as isize - r, n[axis]) * stride]).sum();
        });
    }
    Vol { n, v }
}

// ---- Geometry and Fourier masks ----

/// Which volume axes (0 = X, 1 = Y, 2 = Z) the beam and the tilt axis lie
/// along, and the largest tilt (degrees, symmetric).
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Geometry {
    pub beam: usize,
    pub tilt: usize,
    pub max_tilt: f32,
    /// Most negative tilt for an asymmetric range (default −max_tilt).
    #[serde(default)]
    pub min_tilt: Option<f32>,
}

impl Geometry {
    fn check(&self) -> Result<()> {
        if self.beam > 2 || self.tilt > 2 || self.beam == self.tilt {
            bail!("电子束方向和倾转轴必须是两个不同的坐标轴");
        }
        let lo = self.min_tilt.unwrap_or(-self.max_tilt);
        if !(10.0..=89.0).contains(&self.max_tilt) || !(-89.0..=-10.0).contains(&lo) {
            bail!("倾转范围应在 ±10° 到 ±89° 之间");
        }
        Ok(())
    }

    /// The axis normal to both the beam and the tilt axis.
    fn across(&self) -> usize {
        3 - self.beam - self.tilt
    }

    /// 1 for the Fourier components a tilt series measures, 0 in the
    /// missing wedge, for a cube of side n (unshifted frequencies).
    fn mask(&self, n: usize) -> Vec<f32> {
        // A view tilted by α measures the plane through the tilt axis at
        // angle α from the plane normal to the beam; Friedel symmetry folds
        // each direction to (-90°, 90°].
        let (lo, hi) = (self.min_tilt.unwrap_or(-self.max_tilt) as f64 - 1e-6, self.max_tilt as f64 + 1e-6);
        let f = |i: usize| if i <= n / 2 { i as f64 } else { i as f64 - n as f64 };
        let mut out = vec![0f32; n * n * n];
        out.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
            for (i, m) in plane.iter_mut().enumerate() {
                let k = [f(i % n), f(i / n), f(z)];
                let (mut b, mut a) = (k[self.beam], k[self.across()]);
                if a < 0.0 || (a == 0.0 && b < 0.0) {
                    (b, a) = (-b, -a);
                }
                let angle = if a == 0.0 && b == 0.0 { 0.0 } else { b.atan2(a).to_degrees() };
                *m = if (lo..=hi).contains(&angle) { 1.0 } else { 0.0 };
            }
        });
        out
    }
}

/// 3D FFT of a cube of side n in place: transforms along X, then turns the
/// axes (X, Y, Z) → (Y, Z, X) so the next one is contiguous, three times.
fn fft3(fft: &Fft, n: usize, re: &mut Vec<f32>, im: &mut Vec<f32>, inverse: bool) {
    if inverse {
        im.par_iter_mut().for_each(|v| *v = -*v);
    }
    for _ in 0..3 {
        re.par_chunks_mut(n).zip(im.par_chunks_mut(n)).for_each(|(r, i)| fft.run(r, i));
        *re = cycle(re, n);
        *im = cycle(im, n);
    }
    if inverse {
        let s = 1.0 / (n * n * n) as f32;
        re.par_iter_mut().for_each(|v| *v *= s);
        im.par_iter_mut().for_each(|v| *v *= -s);
    }
}

/// a[z][y][x] → out[x][z][y].
fn cycle(a: &[f32], n: usize) -> Vec<f32> {
    let mut out = vec![0f32; n * n * n];
    out.par_chunks_mut(n * n).enumerate().for_each(|(x, plane)| {
        for (i, o) in plane.iter_mut().enumerate() {
            let (z, y) = (i / n, i % n);
            *o = a[(z * n + y) * n + x];
        }
    });
    out
}

/// Fourier masks and transform for one cube size.
struct Spectral {
    n: usize,
    fft: Fft,
    keep: Vec<f32>,
}

impl Spectral {
    fn new(n: usize, g: &Geometry) -> Spectral {
        Spectral { n, fft: Fft::new(n), keep: g.mask(n) }
    }

    /// `v` with only the Fourier components where `mask` is 1.
    fn filter(&self, v: &[f32], mask: &[f32]) -> Vec<f32> {
        let mut re = v.to_vec();
        let mut im = vec![0f32; v.len()];
        fft3(&self.fft, self.n, &mut re, &mut im, false);
        re.par_iter_mut().zip(im.par_iter_mut()).zip(mask).for_each(|((r, i), &m)| {
            *r *= m;
            *i *= m;
        });
        fft3(&self.fft, self.n, &mut re, &mut im, true);
        re
    }

    /// The tilt-series view of `v`: its missing wedge removed.
    fn cut(&self, v: &[f32]) -> Vec<f32> {
        self.filter(v, &self.keep)
    }

    /// The measured components of `measured`, with `guess` inside the wedge.
    fn fill(&self, measured: &[f32], guess: &[f32]) -> Vec<f32> {
        let diff: Vec<f32> = measured.iter().zip(guess).map(|(a, b)| a - b).collect();
        let d = self.cut(&diff);
        guess.iter().zip(d).map(|(g, d)| g + d).collect()
    }
}

// ---- Rotations ----

/// A right-angle rotation of a cube: output axis i reads input axis
/// `perm[i]`, mirrored where `flip[i]`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rot {
    perm: [usize; 3],
    flip: [bool; 3],
}

impl Rot {
    fn apply(&self, v: &[f32], n: usize) -> Vec<f32> {
        let mut out = vec![0f32; n * n * n];
        out.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
            for (i, o) in plane.iter_mut().enumerate() {
                let c = [i % n, i / n, z];
                let mut s = [0usize; 3];
                for a in 0..3 {
                    s[self.perm[a]] = if self.flip[a] { n - 1 - c[a] } else { c[a] };
                }
                *o = v[(s[2] * n + s[1]) * n + s[0]];
            }
        });
        out
    }
}

/// The 24 rotations of a cube, less the 4 that leave the wedge where it is.
fn rotations(g: &Geometry) -> Vec<Rot> {
    const PERMS: [([usize; 3], bool); 6] = [([0, 1, 2], true), ([1, 2, 0], true), ([2, 0, 1], true), ([0, 2, 1], false), ([2, 1, 0], false), ([1, 0, 2], false)];
    let mut out = vec![];
    for (perm, even) in PERMS {
        for bits in 0..8 {
            let flip = [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
            let odd_flips = flip.iter().filter(|&&f| f).count() % 2 == 1;
            if even == odd_flips {
                continue; // a mirror image, not a rotation
            }
            if perm[g.beam] == g.beam && perm[g.across()] == g.across() {
                continue;
            }
            out.push(Rot { perm, flip });
        }
    }
    out
}

// ---- Detecting the geometry ----

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub geometry: Geometry,
    /// How much weaker the wedge is than the rest (natural log of power);
    /// below about 0.5 the result is not to be trusted.
    pub contrast: f32,
    /// Mean log power against the angle from the measured plane, 0..90° in
    /// 5° steps, relative to the mean at each radius.
    pub profile: Vec<f32>,
}

/// Cube origins with the most structure (highest spread), at most `count`.
fn busiest(vol: &Vol, side: usize, count: usize, seed: u64) -> Vec<[isize; 3]> {
    let mut rng = Rng(seed | 1);
    let tries = (count * 4).max(64);
    let mut cands: Vec<([isize; 3], f32)> = (0..tries)
        .map(|_| {
            let o: [isize; 3] = std::array::from_fn(|a| if vol.n[a] > side { rng.below(vol.n[a] - side + 1) as isize } else { (vol.n[a] as isize - side as isize) / 2 });
            // Spread of a coarse sample of the cube.
            let (mut k, mut s, mut s2) = (0f32, 0f32, 0f32);
            for z in (0..side).step_by(2) {
                for y in (0..side).step_by(2) {
                    for x in (0..side).step_by(2) {
                        let p = [o[0] + x as isize, o[1] + y as isize, o[2] + z as isize];
                        let v = vol.at(std::array::from_fn(|a| reflect(p[a], vol.n[a])));
                        k += 1.0;
                        s += v;
                        s2 += v * v;
                    }
                }
            }
            let m = s / k;
            (o, s2 / k - m * m)
        })
        .collect();
    cands.sort_by(|a, b| b.1.total_cmp(&a.1));
    cands.truncate(count);
    cands.into_iter().map(|c| c.0).collect()
}

/// Measures the missing wedge from the power spectrum of the busiest cubes.
pub fn detect(vol: &Vol) -> Detected {
    let side = match vol.n.iter().copied().min().unwrap() {
        64.. => 64,
        32.. => 32,
        _ => 16,
    };
    let fft = Fft::new(side);
    let win: Vec<f32> = crate::fft::hann(side);
    let mut power = vec![0f32; side * side * side];
    for o in busiest(vol, side, 8, 0xD37EC7) {
        let mut re = vol.cube(o, [side; 3]);
        let mean = re.iter().sum::<f32>() / re.len() as f32;
        for (i, v) in re.iter_mut().enumerate() {
            *v = (*v - mean) * win[i % side] * win[(i / side) % side] * win[i / (side * side)];
        }
        let mut im = vec![0f32; re.len()];
        fft3(&fft, side, &mut re, &mut im, false);
        for ((p, r), i) in power.iter_mut().zip(&re).zip(&im) {
            *p += r * r + i * i;
        }
    }
    let n = side;
    let f = |i: usize| if i <= n / 2 { i as f32 } else { i as f32 - n as f32 };
    // Log power less its mean over the shell, for the band 0.2..0.9 Nyquist.
    let shells = n / 2 + 1;
    let (mut sum, mut cnt) = (vec![0f64; shells], vec![0f64; shells]);
    let mut pts: Vec<([f32; 3], usize, f32)> = vec![];
    for (i, &p) in power.iter().enumerate() {
        let k = [f(i % n), f((i / n) % n), f(i / (n * n))];
        let r = (k[0] * k[0] + k[1] * k[1] + k[2] * k[2]).sqrt();
        let shell = r.round() as usize;
        if shell >= shells || (r as f64) < 0.2 * (n / 2) as f64 || (r as f64) > 0.9 * (n / 2) as f64 || p <= 0.0 {
            continue;
        }
        let l = p.ln();
        sum[shell] += l as f64;
        cnt[shell] += 1.0;
        pts.push((k, shell, l));
    }
    let pts: Vec<([f32; 3], f32)> = pts.into_iter().map(|(k, s, l)| (k, l - (sum[s] / cnt[s]) as f32)).collect();
    let mut best: Option<Detected> = None;
    for beam in 0..3 {
        for tilt in 0..3 {
            if tilt == beam {
                continue;
            }
            let across = 3 - beam - tilt;
            let mut bins = [(0f64, 0f64); 18];
            for (k, l) in &pts {
                let (b, a) = (k[beam].abs(), k[across].abs());
                // Close to the tilt axis the angle means nothing.
                if (a * a + b * b).sqrt() < 0.5 * k[tilt].abs() {
                    continue;
                }
                let bin = ((b.atan2(a).to_degrees() / 5.0) as usize).min(17);
                bins[bin].0 += *l as f64;
                bins[bin].1 += 1.0;
            }
            let profile: Vec<f32> = bins.iter().map(|&(s, c)| if c > 0.0 { (s / c) as f32 } else { 0.0 }).collect();
            let mean = |r: std::ops::Range<usize>| profile[r.clone()].iter().sum::<f32>() / r.len() as f32;
            let (hi, lo) = (mean(0..8), mean(16..18));
            let contrast = hi - lo;
            // The first 5° step from 40° on that falls below half way.
            let half = 0.5 * (hi + lo);
            let edge = (8..18).find(|&i| profile[i] < half).unwrap_or(17);
            let max_tilt = ((edge * 5) as f32).clamp(40.0, 80.0);
            if best.as_ref().is_none_or(|b| contrast > b.contrast) {
                best = Some(Detected { geometry: Geometry { beam, tilt, max_tilt, min_tilt: None }, contrast, profile });
            }
        }
    }
    best.unwrap()
}

// ---- The network ----

/// Two 3×3×3 convolutions with leaky ReLU, no normalisation (so tiles join).
#[derive(Module, Debug)]
struct Block {
    c1: Conv3d,
    c2: Conv3d,
}

impl Block {
    fn new(i: usize, o: usize, dev: &Device) -> Block {
        let conv = |i, o| Conv3dConfig::new([i, o], [3, 3, 3]).with_padding(PaddingConfig3d::Same).init(dev);
        Block { c1: conv(i, o), c2: conv(o, o) }
    }

    fn forward(&self, x: Tensor<5>) -> Tensor<5> {
        leaky_relu(self.c2.forward(leaky_relu(self.c1.forward(x), 0.1)), 0.1)
    }
}

/// 3D U-Net: strided convolutions down, transposed ones up, widths
/// base, 2·base, … ; predicts a correction added to its input.
#[derive(Module, Debug)]
pub struct Net {
    down: Vec<Block>,
    pool: Vec<Conv3d>,
    bottom: Block,
    raise: Vec<Conv3d>,
    up: Vec<Block>,
    head: Conv3d,
}

impl Net {
    pub fn new(base: usize, levels: usize, dev: &Device) -> Net {
        let w = |l: usize| base << l;
        let down = (0..levels).map(|l| Block::new(if l == 0 { 1 } else { w(l - 1) }, w(l), dev)).collect();
        let pool = (0..levels).map(|l| Conv3dConfig::new([w(l), w(l)], [2, 2, 2]).with_stride([2, 2, 2]).init(dev)).collect();
        let bottom = Block::new(w(levels - 1), w(levels), dev);
        // Nearest-neighbour doubling then a 3×3×3 convolution: no checkerboard,
        // which transposed convolutions leave.
        let raise = (0..levels).rev().map(|l| Conv3dConfig::new([w(l + 1), w(l)], [3, 3, 3]).with_padding(PaddingConfig3d::Same).init(dev)).collect();
        let up = (0..levels).rev().map(|l| Block::new(2 * w(l), w(l), dev)).collect();
        // The head starts at zero: the network starts as the identity.
        let head = Conv3dConfig::new([base, 1], [1, 1, 1]).with_initializer(burn::nn::Initializer::Zeros).init(dev);
        Net { down, pool, bottom, raise, up, head }
    }

    /// Sides fed in are multiples of this.
    pub fn align(&self) -> usize {
        1 << self.down.len()
    }

    /// [B, 1, D, H, W] → same shape.
    pub fn forward(&self, x: Tensor<5>) -> Tensor<5> {
        let input = x.clone();
        let mut skips = vec![];
        let mut x = x;
        for (b, p) in self.down.iter().zip(&self.pool) {
            x = b.forward(x);
            skips.push(x.clone());
            x = p.forward(x);
        }
        x = self.bottom.forward(x);
        for (r, b) in self.raise.iter().zip(&self.up) {
            x = leaky_relu(r.forward(double(x)), 0.1);
            x = b.forward(Tensor::cat(vec![x, skips.pop().unwrap()], 1));
        }
        input + self.head.forward(x)
    }
}

/// [B, C, D, H, W] → [B, C, 2D, 2H, 2W], each voxel repeated.
fn double(x: Tensor<5>) -> Tensor<5> {
    let [b, c, d, h, w] = x.dims();
    let x: Tensor<6> = x.reshape([b, c, d, h, w, 1]).repeat_dim(5, 2);
    let x: Tensor<6> = x.reshape([b, c, d, h, 1, 2 * w]).repeat_dim(4, 2);
    let x: Tensor<6> = x.reshape([b, c, d, 1, 2 * h, 2 * w]).repeat_dim(3, 2);
    x.reshape([b, c, 2 * d, 2 * h, 2 * w])
}

// ---- Training ----

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct TrainConfig {
    /// Cube side (a power of two: 32 or 64).
    pub cube: usize,
    /// Cubes cut from the tomogram.
    pub count: usize,
    pub iterations: usize,
    /// Optimiser steps per iteration.
    pub steps: usize,
    pub batch: usize,
    pub lr: f64,
    /// Noise added to the inputs by the last iteration (× the data's
    /// spread; 0 = no denoising), rising from zero half way.
    pub noise: f32,
    pub base: usize,
    pub levels: usize,
    pub seed: u64,
    /// Gaussian low-pass of the volume first (σ in voxels, 0 = none).
    pub smooth: f32,
    /// With two half tomograms: weight of the missing wedge against the
    /// measured part in the loss.
    pub omega: f32,
}

impl Default for TrainConfig {
    fn default() -> Self {
        TrainConfig { cube: 64, count: 120, iterations: 10, steps: 200, batch: 4, lr: 4e-4, noise: 0.0, base: 16, levels: 3, seed: 0x1503, smooth: 0.0, omega: 2.0 }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub iteration: usize,
    pub iterations: usize,
    pub step: usize,
    pub steps: usize,
    pub loss: f32,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn gauss(&mut self) -> f32 {
        let u = ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
        let v = ((self.next() >> 11) as f64) / (1u64 << 53) as f64;
        ((-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()) as f32
    }
}

/// Trains the network on cubes of `vol` (standardised).
pub fn train(dev: &Device, vol: &Vol, g: &Geometry, cfg: &TrainConfig, mut progress: impl FnMut(Progress)) -> Result<(Net, f32)> {
    g.check()?;
    let n = cfg.cube;
    if !n.is_power_of_two() || n < 16 || !n.is_multiple_of(1 << cfg.levels) {
        bail!("子块边长须为 2 的幂");
    }
    let sp = Spectral::new(n, g);
    let rots = rotations(g);
    let orig: Vec<Vec<f32>> = busiest(vol, n, cfg.count.max(1), cfg.seed).into_iter().map(|o| vol.cube(o, [n; 3])).collect();
    let mut data = orig.clone();
    let ad = dev.clone().autodiff();
    let mut net = Net::new(cfg.base, cfg.levels, &ad);
    let mut opt = AdamConfig::new().init();
    let mut rng = Rng(cfg.seed | 1);
    let total = cfg.iterations * cfg.steps;
    let mut loss_avg = 0f32;
    let edge = n / 8;
    let inside = |i: usize| (edge..n - edge).contains(&i);
    let w: Vec<f32> = (0..n * n * n).map(|i| if inside(i % n) && inside((i / n) % n) && inside(i / (n * n)) { 1.0 } else { 0.0 }).collect();
    let inner_n = w.iter().sum::<f32>() * cfg.batch as f32;
    let inner = Tensor::<5>::from_data(TensorData::new(w.repeat(cfg.batch), [cfg.batch, 1, n, n, n]), &ad);
    for it in 0..cfg.iterations {
        let noise = if cfg.noise > 0.0 && cfg.iterations > 1 {
            let start = cfg.iterations / 2;
            if it >= start { cfg.noise * (it - start + 1) as f32 / (cfg.iterations - start) as f32 } else { 0.0 }
        } else {
            0.0
        };
        for step in 0..cfg.steps {
            if CANCEL.load(Ordering::SeqCst) {
                bail!("已取消");
            }
            let picks: Vec<(usize, Rot, u64)> = (0..cfg.batch).map(|_| (rng.below(data.len()), rots[rng.below(rots.len())], rng.next())).collect();
            let pairs: Vec<(Vec<f32>, Vec<f32>)> = picks
                .par_iter()
                .map(|(i, r, seed)| {
                    let target = r.apply(&data[*i], n);
                    let mut input = sp.cut(&target);
                    if noise > 0.0 {
                        let mut nr = Rng(*seed | 1);
                        let raw: Vec<f32> = (0..n * n * n).map(|_| nr.gauss() * noise).collect();
                        // Noise as the tomogram has it: without the wedge.
                        let shaped = sp.cut(&raw);
                        let gain = (raw.iter().map(|v| v * v).sum::<f32>() / shaped.iter().map(|v| v * v).sum::<f32>().max(1e-12)).sqrt();
                        input.iter_mut().zip(&shaped).for_each(|(v, s)| *v += s * gain);
                    }
                    (input, target)
                })
                .collect();
            let b = pairs.len();
            let (xs, ts): (Vec<Vec<f32>>, Vec<Vec<f32>>) = pairs.into_iter().unzip();
            let x = Tensor::<5>::from_data(TensorData::new(xs.concat(), [b, 1, n, n, n]), &ad);
            let t = Tensor::<5>::from_data(TensorData::new(ts.concat(), [b, 1, n, n, n]), &ad);
            // Fourier cuts ring at the cube's faces: score the middle only.
            let loss = ((net.forward(x) - t).abs() * inner.clone()).sum() / inner_n;
            let lv: f32 = loss.clone().into_scalar();
            loss_avg += if it == 0 && step == 0 { lv - loss_avg } else { 0.05 * (lv - loss_avg) };
            let grads = GradientsParams::from_grads(loss.backward(), &net);
            let k = it * cfg.steps + step;
            let lr = cfg.lr * (0.1 + 0.9 * 0.5 * (1.0 + (std::f64::consts::PI * k as f64 / total.max(1) as f64).cos()));
            net = opt.step(lr, net, grads);
            if step % 5 == 4 || step + 1 == cfg.steps {
                progress(Progress { iteration: it + 1, iterations: cfg.iterations, step: step + 1, steps: cfg.steps, loss: loss_avg });
            }
        }
        // Fill each cube's wedge with the network's guess for the next round.
        if it + 1 < cfg.iterations {
            let valid = net.valid();
            for chunk in (0..orig.len()).collect::<Vec<_>>().chunks(cfg.batch.max(1)) {
                let x = Tensor::<5>::from_data(TensorData::new(chunk.iter().flat_map(|&i| data[i].iter().copied()).collect::<Vec<f32>>(), [chunk.len(), 1, n, n, n]), dev);
                let out = valid.forward(x).into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
                let filled: Vec<Vec<f32>> = chunk.par_iter().enumerate().map(|(j, &i)| sp.fill(&orig[i], &out[j * n * n * n..(j + 1) * n * n * n])).collect();
                for (&i, f) in chunk.iter().zip(filled) {
                    data[i] = f;
                }
            }
        }
    }
    Ok((net.valid(), loss_avg))
}

/// For a reconstruction pipeline: trains on `values` (dims X, Y, Z, X
/// fastest, beam along Z, tilt axis along Y, tilts from `tilt_range[0]` to
/// `tilt_range[1]` degrees) and returns the corrected volume, same units.
/// `progress` gets training steps, then tiles as (iterations + 1, 1, done, total).
pub fn train_and_apply(dev: &Device, values: &[f32], dims: [usize; 3], tilt_range: [f32; 2], cfg: &TrainConfig, mut progress: impl FnMut(Progress)) -> Result<Vec<f32>> {
    if values.len() != dims[0] * dims[1] * dims[2] {
        bail!("体数据大小和尺寸不符");
    }
    let (mean, std) = stats(values);
    let vol = smooth(&Vol { n: dims, v: values.par_iter().map(|&x| if x.is_finite() { (x - mean) / std } else { 0.0 }).collect() }, cfg.smooth);
    let g = Geometry { beam: 2, tilt: 1, max_tilt: tilt_range[1], min_tilt: Some(tilt_range[0]) };
    let (net, _) = train(dev, &vol, &g, cfg, &mut progress)?;
    let it = cfg.iterations + 1;
    let out = apply(dev, &net, &vol, [0; 3], dims, &mut |done, total| {
        if CANCEL.load(Ordering::SeqCst) {
            bail!("已取消");
        }
        progress(Progress { iteration: it, iterations: cfg.iterations, step: done, steps: total, loss: 0.0 });
        Ok(())
    })?;
    Ok(out.into_par_iter().map(|v| v * std + mean).collect())
}

// ---- Two half tomograms ----
//
// With tomograms from the even and odd frames of each movie, the two halves
// see the same specimen through independent noise. The network then maps a
// turned, twice-cut cube of one half to the same turned cube of the other
// (after DeepDeWedge, Wiedemann & Heckel 2024, and IsoNet2): the error is
// scored in Fourier space, only where the target was measured; where the
// input was measured too it learns to denoise, where the input lacks it
// (the original wedge, turned away in the target) it learns to fill the
// wedge, weighted by `omega`. Noise in the target averages out, so no
// pre-smoothing is needed. The wedge of the inputs is not refilled with the
// network's guesses between rounds (that feeds its errors back).

/// The measured components for a tilt series with these tilts (degrees),
/// for a cube of side n: inside the tilt range, and within about a voxel of
/// some tilt's central section (so wide gaps in the series stay empty).
fn tilt_mask(n: usize, g: &Geometry, tilts: &[f32]) -> Vec<f32> {
    let mut range = g.mask(n);
    if tilts.len() < 2 {
        return range;
    }
    let f = |i: usize| if i <= n / 2 { i as f64 } else { i as f64 - n as f64 };
    range.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
        for (i, m) in plane.iter_mut().enumerate() {
            if *m == 0.0 {
                continue;
            }
            let k = [f(i % n), f(i / n), f(z)];
            let (mut b, mut a) = (k[g.beam], k[g.across()]);
            if a < 0.0 || (a == 0.0 && b < 0.0) {
                (b, a) = (-b, -a);
            }
            let angle = b.atan2(a);
            let r = a.hypot(b);
            let near = tilts.iter().map(|&t| (angle - (t as f64).to_radians()).abs()).fold(f64::MAX, f64::min);
            if r * near.sin() > 1.0 {
                *m = 0.0;
            }
        }
    });
    range
}

/// `mask` (unshifted frequencies of a cube of side n) turned with the cube.
fn turn_mask(mask: &[f32], n: usize, r: &Rot) -> Vec<f32> {
    let mut out = vec![0f32; n * n * n];
    out.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
        for (i, o) in plane.iter_mut().enumerate() {
            let c = [i % n, i / n, z];
            let mut s = [0usize; 3];
            for a in 0..3 {
                s[r.perm[a]] = if r.flip[a] { (n - c[a]) % n } else { c[a] };
            }
            *o = mask[(s[2] * n + s[1]) * n + s[0]];
        }
    });
    out
}

/// Cosine and −sine DFT matrices of side n.
fn dft(n: usize, dev: &Device) -> (Tensor<2>, Tensor<2>) {
    let at = |f: fn(f64) -> f64| -> Vec<f32> { (0..n * n).map(|i| f(2.0 * std::f64::consts::PI * ((i / n) * (i % n) % n) as f64 / n as f64) as f32).collect() };
    let c = Tensor::<2>::from_data(TensorData::new(at(f64::cos), [n, n]), dev);
    let s = Tensor::<2>::from_data(TensorData::new(at(|x| -x.sin()), [n, n]), dev);
    (c, s)
}

/// Σ w·|F e|² for e [B, 1, n, n, n] (Z, Y, X), with the weights in the
/// order the transform leaves the axes: [B, kY, kX, kZ].
fn spectral_error(e: Tensor<5>, c: &Tensor<2>, s: &Tensor<2>, w: Tensor<4>) -> Tensor<1> {
    let [b, _, n, _, _] = e.dims();
    let rows = b * n * n;
    let x = e.reshape([rows, n]);
    let (mut re, mut im) = (x.clone().matmul(c.clone()), x.matmul(s.clone()));
    for swap in [(2, 3), (1, 3)] {
        let turn = |t: Tensor<2>| -> Tensor<2> { t.reshape([b, n, n, n]).swap_dims(swap.0, swap.1).reshape([rows, n]) };
        let (r, i) = (turn(re), turn(im));
        re = r.clone().matmul(c.clone()) - i.clone().matmul(s.clone());
        im = r.matmul(s.clone()) + i.matmul(c.clone());
    }
    let p = re.clone() * re + im.clone() * im;
    (p.reshape([b, n, n, n]) * w).sum()
}

/// [kZ][kY][kX] → [kY][kX][kZ], the order `spectral_error` reads.
fn spectral_order(w: &[f32], n: usize) -> Vec<f32> {
    (0..n * n * n).map(|i| {
        let (y, x, z) = (i / (n * n), (i / n) % n, i % n);
        w[(z * n + y) * n + x]
    }).collect()
}

/// Trains on the two half tomograms `a` and `b` (standardised alike).
pub fn train_halves(dev: &Device, a: &Vol, b: &Vol, g: &Geometry, tilts: &[f32], cfg: &TrainConfig, mut progress: impl FnMut(Progress)) -> Result<(Net, f32)> {
    g.check()?;
    if a.n != b.n {
        bail!("两个半断层尺寸不同");
    }
    let n = cfg.cube;
    if !n.is_power_of_two() || n < 16 || !n.is_multiple_of(1 << cfg.levels) {
        bail!("子块边长须为 2 的幂");
    }
    let sp = Spectral { n, fft: Fft::new(n), keep: tilt_mask(n, g, tilts) };
    let rots = rotations(g);
    // Loss weights for each rotation: the part both input and target
    // measured (denoising) and the part only the target measured (the
    // wedge), each averaged over its own size, the wedge × omega.
    let weights: Vec<Vec<f32>> = rots
        .iter()
        .map(|r| {
            let target = turn_mask(&sp.keep, n, r);
            let both = target.iter().zip(&sp.keep).filter(|(&t, &k)| t > 0.0 && k > 0.0).count().max(1) as f32;
            let new = target.iter().zip(&sp.keep).filter(|(&t, &k)| t > 0.0 && k == 0.0).count().max(1) as f32;
            let w: Vec<f32> = target.iter().zip(&sp.keep).map(|(&t, &k)| t * if k > 0.0 { 1.0 / both } else { cfg.omega / new }).collect();
            spectral_order(&w, n)
        })
        .collect();
    let edge = (n / 8).max(1);
    let taper1: Vec<f32> = (0..n).map(|i| {
        let d = i.min(n - 1 - i);
        if d >= edge { 1.0 } else { (0.5 - 0.5 * (std::f32::consts::PI * (d as f32 + 0.5) / edge as f32).cos()).max(0.0) }
    }).collect();
    let taper: Vec<f32> = (0..n * n * n).map(|i| taper1[i % n] * taper1[(i / n) % n] * taper1[i / (n * n)]).collect();
    let mean = Vol { n: a.n, v: a.v.par_iter().zip(&b.v).map(|(x, y)| 0.5 * (x + y)).collect() };
    let origins = busiest(&mean, n, cfg.count.max(1), cfg.seed);
    let orig: [Vec<Vec<f32>>; 2] = [a, b].map(|h| origins.iter().map(|&o| h.cube(o, [n; 3])).collect());
    let ad = dev.clone().autodiff();
    let (c, s) = dft(n, &ad);
    let window = Tensor::<5>::from_data(TensorData::new(taper.repeat(cfg.batch), [cfg.batch, 1, n, n, n]), &ad);
    let mut net = Net::new(cfg.base, cfg.levels, &ad);
    let mut opt = AdamConfig::new().init();
    let mut rng = Rng(cfg.seed | 1);
    let total = cfg.iterations * cfg.steps;
    let mut loss_avg = 0f32;
    for it in 0..cfg.iterations {
        for step in 0..cfg.steps {
            if CANCEL.load(Ordering::SeqCst) {
                bail!("已取消");
            }
            let picks: Vec<(usize, usize, usize)> = (0..cfg.batch).map(|_| (rng.below(origins.len()), rng.below(rots.len()), rng.below(2))).collect();
            let pairs: Vec<(Vec<f32>, Vec<f32>)> = picks
                .par_iter()
                .map(|&(i, r, from)| (sp.cut(&rots[r].apply(&orig[from][i], n)), rots[r].apply(&orig[1 - from][i], n)))
                .collect();
            let (xs, ts): (Vec<Vec<f32>>, Vec<Vec<f32>>) = pairs.into_iter().unzip();
            let w: Vec<f32> = picks.iter().flat_map(|&(_, r, _)| weights[r].iter().copied()).collect();
            let norm = (cfg.batch * n * n * n) as f32 * (1.0 + cfg.omega);
            let x = Tensor::<5>::from_data(TensorData::new(xs.concat(), [cfg.batch, 1, n, n, n]), &ad);
            let t = Tensor::<5>::from_data(TensorData::new(ts.concat(), [cfg.batch, 1, n, n, n]), &ad);
            let w = Tensor::<4>::from_data(TensorData::new(w, [cfg.batch, n, n, n]), &ad);
            let loss = spectral_error((net.forward(x) - t) * window.clone(), &c, &s, w) / norm;
            let lv: f32 = loss.clone().into_scalar();
            loss_avg += if it == 0 && step == 0 { lv - loss_avg } else { 0.05 * (lv - loss_avg) };
            let grads = GradientsParams::from_grads(loss.backward(), &net);
            let k = it * cfg.steps + step;
            let lr = cfg.lr * (0.1 + 0.9 * 0.5 * (1.0 + (std::f64::consts::PI * k as f64 / total.max(1) as f64).cos()));
            net = opt.step(lr, net, grads);
            if step % 5 == 4 || step + 1 == cfg.steps {
                progress(Progress { iteration: it + 1, iterations: cfg.iterations, step: step + 1, steps: cfg.steps, loss: loss_avg });
            }
        }
    }
    Ok((net.valid(), loss_avg))
}

/// For a reconstruction pipeline with even/odd half tomograms (same layout
/// as `train_and_apply`; `tilts` = the tilt angles used, degrees): denoises
/// and fills the wedge in one network, and returns the mean of its results
/// for both halves, in the halves' units.
pub fn train_halves_and_apply(dev: &Device, even: &[f32], odd: &[f32], dims: [usize; 3], tilts: &[f32], cfg: &TrainConfig, mut progress: impl FnMut(Progress)) -> Result<Vec<f32>> {
    let len = dims[0] * dims[1] * dims[2];
    if even.len() != len || odd.len() != len {
        bail!("体数据大小和尺寸不符");
    }
    let (lo, hi) = tilts.iter().fold((f32::MAX, f32::MIN), |(l, h), &t| (l.min(t), h.max(t)));
    if tilts.len() < 2 {
        bail!("缺少倾转角");
    }
    let (m0, s0) = stats(even);
    let (m1, s1) = stats(odd);
    let (mean, std) = (0.5 * (m0 + m1), 0.5 * (s0 + s1));
    let std_vol = |v: &[f32]| Vol { n: dims, v: v.par_iter().map(|&x| if x.is_finite() { (x - mean) / std } else { 0.0 }).collect() };
    let (a, b) = (std_vol(even), std_vol(odd));
    let g = Geometry { beam: 2, tilt: 1, max_tilt: hi, min_tilt: Some(lo) };
    let (net, _) = train_halves(dev, &a, &b, &g, tilts, cfg, &mut progress)?;
    let it = cfg.iterations + 1;
    let mut outs = vec![];
    for (h, vol) in [&a, &b].into_iter().enumerate() {
        outs.push(apply(dev, &net, vol, [0; 3], dims, &mut |done, total| {
            if CANCEL.load(Ordering::SeqCst) {
                bail!("已取消");
            }
            progress(Progress { iteration: it, iterations: cfg.iterations, step: h * total + done, steps: 2 * total, loss: 0.0 });
            Ok(())
        })?);
    }
    Ok(outs[0].par_iter().zip(&outs[1]).map(|(x, y)| 0.5 * (x + y) * std + mean).collect())
}

// ---- Applying ----

/// Context read around each tile, the part of it where neighbouring tiles
/// are blended (so no seams show), and the tile side.
const MARGIN: usize = 24;
const BLEND: usize = 8;
const TILE: usize = 112;

/// The network's result for the box [origin, origin + size) of `vol`, in
/// overlapping tiles that read past the box (mirrored at the volume's
/// edges) and are cross-faded where they meet. `tick(done, total)` after
/// each tile.
pub fn apply(dev: &Device, net: &Net, vol: &Vol, origin: [usize; 3], size: [usize; 3], tick: &mut dyn FnMut(usize, usize) -> Result<()>) -> Result<Vec<f32>> {
    let al = net.align();
    let step = TILE - 2 * MARGIN;
    let starts: [Vec<usize>; 3] = std::array::from_fn(|a| (0..size[a]).step_by(step).collect());
    let total = starts.iter().map(Vec::len).product();
    let len = size[0] * size[1] * size[2];
    let (mut acc, mut wsum) = (vec![0f32; len], vec![0f32; len]);
    let mut done = 0;
    for &z0 in &starts[2] {
        for &y0 in &starts[1] {
            for &x0 in &starts[0] {
                let o = [x0, y0, z0];
                // Output span of this tile along each axis: its step plus
                // BLEND on each side, inside the box.
                let lo: [usize; 3] = std::array::from_fn(|a| o[a].saturating_sub(BLEND));
                let hi: [usize; 3] = std::array::from_fn(|a| (o[a] + step + BLEND).min(size[a]));
                let side: [usize; 3] = std::array::from_fn(|a| (hi[a] - lo[a] + 2 * (MARGIN - BLEND)).div_ceil(al) * al);
                let pad = MARGIN - BLEND;
                let corner: [isize; 3] = std::array::from_fn(|a| (origin[a] + lo[a]) as isize - pad as isize);
                let tile = vol.cube(corner, side);
                let x = Tensor::<5>::from_data(TensorData::new(tile, [1, 1, side[2], side[1], side[0]]), dev);
                let r = net.forward(x).into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
                // Weight ramps from 0 to 1 across the 2·BLEND overlap.
                let ramp = |a: usize, i: usize| -> f32 {
                    let rise = if o[a] == 0 { 1.0 } else { ((i + 1) as f32 / (2 * BLEND + 1) as f32).min(1.0) };
                    let left = hi[a] - lo[a] - 1 - i;
                    let fall = if hi[a] == size[a] { 1.0 } else { ((left + 1) as f32 / (2 * BLEND + 1) as f32).min(1.0) };
                    rise.min(fall)
                };
                for z in 0..hi[2] - lo[2] {
                    let wz = ramp(2, z);
                    for y in 0..hi[1] - lo[1] {
                        let wzy = wz * ramp(1, y);
                        let src = ((z + pad) * side[1] + y + pad) * side[0] + pad;
                        let dst = ((lo[2] + z) * size[1] + lo[1] + y) * size[0] + lo[0];
                        for x in 0..hi[0] - lo[0] {
                            let w = wzy * ramp(0, x);
                            acc[dst + x] += w * r[src + x];
                            wsum[dst + x] += w;
                        }
                    }
                }
                done += 1;
                tick(done, total)?;
            }
        }
    }
    acc.par_iter_mut().zip(&wsum).for_each(|(a, &w)| *a /= w.max(1e-12));
    Ok(acc)
}

/// A trained network with the volume it was trained on.
pub struct Trained {
    pub net: Net,
    pub vol: Arc<Vol>,
    pub geometry: Geometry,
    pub bin: usize,
    pub mean: f32,
    pub std: f32,
    pub loss: f32,
    /// Image it belongs to.
    pub token: u64,
    pub c: usize,
    pub t: usize,
}

/// The last trained network (one at a time).
pub static MODEL: Mutex<Option<Arc<Trained>>> = Mutex::new(None);

/// Loaded volume reused between `detect` and training: (token, c, t, bin).
#[allow(clippy::type_complexity)]
pub static VOLUME: Mutex<Option<((u64, usize, usize, usize), Arc<Vol>, f32, f32)>> = Mutex::new(None);

pub fn volume(img: &OpenImage, c: usize, t: usize, bin: usize) -> Result<(Arc<Vol>, f32, f32)> {
    let key = (img.token, c, t, bin);
    if let Some((k, v, m, s)) = VOLUME.lock().unwrap().as_ref() {
        if *k == key {
            return Ok((v.clone(), *m, *s));
        }
    }
    let (v, m, s) = load(img, c, t, bin)?;
    let v = Arc::new(v);
    *VOLUME.lock().unwrap() = Some((key, v.clone(), m, s));
    Ok((v, m, s))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewReq {
    pub token: u64,
    /// "xy", "xz" or "yz": the view the region is in.
    pub axis: String,
    /// Slice of that view (Z for XY, Y for XZ, X for YZ), full size.
    pub index: usize,
    /// [x, y, w, h] in that view's pixels, full size.
    pub region: [usize; 4],
}

/// Corrected slice through the middle of `region` (at most 512 binned
/// pixels a side): header (x, y, w, h of the region in view pixels; result
/// width, height as u32; ms as f32; backend name, 16 bytes) then f32 values.
pub fn preview(m: &Trained, req: &PreviewReq) -> Result<Vec<u8>> {
    if req.token != m.token {
        bail!("网络是在另一个图像上训练的");
    }
    let started = std::time::Instant::now();
    let b = m.bin;
    let n = m.vol.n;
    // View axes (u → horizontal, v → vertical) and the slice axis.
    let (u, v, s) = match req.axis.as_str() {
        "xz" => (0, 2, 1),
        "yz" => (1, 2, 0),
        _ => (0, 1, 2),
    };
    let [rx, ry, rw, rh] = req.region;
    let lim = 512;
    let (cu, cv) = ((rx + rw / 2) / b, (ry + rh / 2) / b);
    let (wu, wv) = ((rw / b).clamp(1, lim).min(n[u]), (rh / b).clamp(1, lim).min(n[v]));
    let ou = cu.saturating_sub(wu / 2).min(n[u] - wu);
    let ov = cv.saturating_sub(wv / 2).min(n[v] - wv);
    let mut origin = [0usize; 3];
    let mut size = [1usize; 3];
    origin[u] = ou;
    size[u] = wu;
    origin[v] = ov;
    size[v] = wv;
    origin[s] = (req.index / b).min(n[s] - 1);
    let (vals, backend) = crate::classify::on_device(|dev| {
        let net = m.net.clone().to_device(dev);
        apply(dev, &net, &m.vol, origin, size, &mut |_, _| Ok(()))
    })?;
    // The box is one voxel thick along s: lay it out as v rows of u.
    let mut img = vec![0f32; wu * wv];
    for j in 0..wv {
        for i in 0..wu {
            let mut p = [0usize; 3];
            p[u] = i;
            p[v] = j;
            img[j * wu + i] = vals[(p[2] * size[1] + p[1]) * size[0] + p[0]] * m.std + m.mean;
        }
    }
    let mut out = Vec::with_capacity(40 + img.len() * 4);
    for k in [ou * b, ov * b, wu * b, wv * b, wu, wv] {
        out.extend((k as u32).to_le_bytes());
    }
    out.extend((started.elapsed().as_secs_f32() * 1e3).to_le_bytes());
    let mut name = [0u8; 16];
    let bn = backend.as_bytes();
    name[..bn.len().min(16)].copy_from_slice(&bn[..bn.len().min(16)]);
    out.extend(name);
    out.extend(img.iter().flat_map(|v| v.to_le_bytes()));
    Ok(out)
}

/// Corrects the whole volume and writes it as a float32 MRC at `path`.
pub fn save(m: &Trained, path: &str, pixel_a: f64, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<&'static str> {
    use std::io::Write;
    CANCEL.store(false, Ordering::SeqCst);
    let n = m.vol.n;
    let (vals, backend) = crate::classify::on_device(|dev| {
        let net = m.net.clone().to_device(dev);
        apply(dev, &net, &m.vol, [0; 3], n, &mut |done, total| {
            if CANCEL.load(Ordering::SeqCst) {
                bail!("已取消");
            }
            progress(done, total);
            Ok(())
        })
    })?;
    let (s, k) = (m.std, m.mean);
    let real: Vec<f32> = vals.par_iter().map(|v| v * s + k).collect();
    let (lo, hi) = real.par_iter().fold(|| (f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v))).reduce(|| (f32::MAX, f32::MIN), |x, y| (x.0.min(y.0), x.1.max(y.1)));
    let (mean, sd) = stats(&real);
    let g = &m.geometry;
    let label = format!("TifMrcView: missing wedge filled, beam {} tilt {} +-{:.0} bin {}", ["X", "Y", "Z"][g.beam], ["X", "Y", "Z"][g.tilt], g.max_tilt, m.bin);
    let tmp = format!("{path}.part");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    f.write_all(&crate::align::mrc_header(n[0], n[1], n[2], pixel_a * m.bin as f64, [lo, hi, mean, sd], &label))?;
    let mut bytes = vec![0u8; real.len() * 4];
    bytes.par_chunks_mut(4).zip(&real).for_each(|(b, v)| b.copy_from_slice(&v.to_le_bytes()));
    f.write_all(&bytes)?;
    f.flush()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(backend)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Random blobs (ellipsoids of several sizes) in a cube of side n.
    fn phantom(n: usize, seed: u64) -> Vol {
        let mut rng = Rng(seed);
        let blobs: Vec<([f32; 3], f32, f32)> = (0..n / 2).map(|_| (std::array::from_fn(|_| rng.below(n * 100) as f32 / 100.0), 1.5 + rng.below(400) as f32 / 100.0, if rng.below(2) == 0 { 1.0 } else { -0.6 })).collect();
        let mut v = vec![0f32; n * n * n];
        v.par_iter_mut().enumerate().for_each(|(i, o)| {
            let p = [(i % n) as f32, ((i / n) % n) as f32, (i / (n * n)) as f32];
            for (c, r, a) in &blobs {
                let d2: f32 = (0..3).map(|k| (p[k] - c[k]).powi(2)).sum();
                *o += a * (-d2 / (2.0 * r * r)).exp();
            }
        });
        let (m, s) = stats(&v);
        Vol { n: [n; 3], v: v.iter().map(|x| (x - m) / s).collect() }
    }

    #[test]
    fn fft_round_trip_and_masks() {
        let n = 16;
        let fft = Fft::new(n);
        let v = phantom(n, 3).v;
        let (mut re, mut im) = (v.clone(), vec![0f32; v.len()]);
        fft3(&fft, n, &mut re, &mut im, false);
        fft3(&fft, n, &mut re, &mut im, true);
        assert!(re.iter().zip(&v).all(|(a, b)| (a - b).abs() < 1e-4));
        let g = Geometry { beam: 2, tilt: 1, max_tilt: 60.0, min_tilt: None };
        let sp = Spectral::new(n, &g);
        let once = sp.cut(&v);
        let twice = sp.cut(&once);
        assert!(once.iter().zip(&twice).all(|(a, b)| (a - b).abs() < 1e-4));
        // Filling with the truth gives the truth back.
        let filled = sp.fill(&once, &v);
        assert!(filled.iter().zip(&v).all(|(a, b)| (a - b).abs() < 1e-4));
        // About a third of the components are missing for ±60°.
        let missing = sp.keep.iter().filter(|&&m| m == 0.0).count() as f32 / sp.keep.len() as f32;
        assert!((0.25..0.4).contains(&missing), "{missing}");
    }

    #[test]
    fn twenty_rotations_move_the_wedge() {
        let g = Geometry { beam: 2, tilt: 1, max_tilt: 60.0, min_tilt: None };
        let rots = rotations(&g);
        assert_eq!(rots.len(), 20);
        let n = 8;
        let mask = g.mask(n);
        for r in &rots {
            // Rotating a cube rotates its spectrum; the cut part moves.
            assert_ne!(r.apply(&mask, n), mask, "{r:?}");
        }
    }

    #[test]
    fn detects_the_wedge() {
        for g in [Geometry { beam: 2, tilt: 1, max_tilt: 60.0, min_tilt: None }, Geometry { beam: 1, tilt: 0, max_tilt: 50.0, min_tilt: None }] {
            let vol = phantom(64, 11);
            let sp = Spectral::new(64, &g);
            let mut rng = Rng(5);
            let noisy: Vec<f32> = vol.v.iter().map(|v| v + 0.3 * rng.gauss()).collect();
            let cut = Vol { n: vol.n, v: sp.cut(&noisy) };
            let d = detect(&cut);
            assert_eq!((d.geometry.beam, d.geometry.tilt), (g.beam, g.tilt), "{d:?}");
            assert!((d.geometry.max_tilt - g.max_tilt).abs() <= 10.0, "{d:?}");
            assert!(d.contrast > 0.5, "{d:?}");
        }
    }

    #[test]
    fn spectral_error_matches_the_cpu_transform() {
        let n = 8;
        let dev = Device::flex();
        let mut rng = Rng(17);
        let e: Vec<f32> = (0..n * n * n).map(|_| rng.gauss()).collect();
        let w: Vec<f32> = (0..n * n * n).map(|_| rng.below(3) as f32).collect();
        let (mut re, mut im) = (e.clone(), vec![0f32; e.len()]);
        fft3(&Fft::new(n), n, &mut re, &mut im, false);
        let cpu: f32 = (0..e.len()).map(|i| w[i] * (re[i] * re[i] + im[i] * im[i])).sum();
        let (c, s) = dft(n, &dev);
        let x = Tensor::<5>::from_data(TensorData::new(e, [1, 1, n, n, n]), &dev);
        let wt = Tensor::<4>::from_data(TensorData::new(spectral_order(&w, n), [1, n, n, n]), &dev);
        let gpu: f32 = spectral_error(x, &c, &s, wt).into_scalar();
        assert!((gpu - cpu).abs() < 1e-3 * cpu, "{gpu} {cpu}");
    }

    #[test]
    fn turned_masks_match_turned_cubes() {
        let n = 16;
        let g = Geometry { beam: 2, tilt: 1, max_tilt: 60.0, min_tilt: Some(-50.0) };
        let sp = Spectral::new(n, &g);
        let v = sp.cut(&phantom(n, 8).v);
        for r in rotations(&g) {
            // A turned cut cube has only the turned mask's components.
            let turned = r.apply(&v, n);
            let worst = |m: &[f32]| sp.filter(&turned, m).iter().zip(&turned).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
            // (Nyquist components of an asymmetric range leave a little.)
            let (right, wrong) = (worst(&turn_mask(&sp.keep, n, &r)), worst(&sp.keep));
            assert!(right < 1e-2 && wrong > 10.0 * right, "{r:?} {right} {wrong}");
        }
        // A dense series measures the whole range; a gap leaves a hole.
        let dense: Vec<f32> = (-25..=30).map(|i| i as f32 * 2.0).collect();
        assert_eq!(tilt_mask(n, &g, &dense), sp.keep);
        let gappy: Vec<f32> = dense.iter().copied().filter(|t| !(10.0..30.0).contains(t)).collect();
        let holes = tilt_mask(n, &g, &gappy).iter().zip(&sp.keep).filter(|(a, b)| a != b).count();
        assert!(holes > 0);
    }

    #[test]
    fn trains_on_halves() {
        let dev = Device::flex();
        let truth = phantom(24, 4);
        let mut rng = Rng(2);
        let half = |rng: &mut Rng| -> Vec<f32> { truth.v.iter().map(|v| v + 0.5 * rng.gauss()).collect() };
        let (a, b) = (half(&mut rng), half(&mut rng));
        let tilts: Vec<f32> = (-20..=20).map(|i| i as f32 * 3.0).collect();
        let cfg = TrainConfig { cube: 16, count: 4, iterations: 2, steps: 3, batch: 2, lr: 1e-3, base: 4, levels: 2, seed: 1, ..Default::default() };
        let mut last = None;
        let out = train_halves_and_apply(&dev, &a, &b, truth.n, &tilts, &cfg, |p| last = Some(p)).unwrap();
        assert_eq!(out.len(), a.len());
        assert!(out.iter().all(|v| v.is_finite()));
        let p = last.unwrap();
        assert_eq!((p.iteration, p.step, p.steps), (3, 2, 2));
    }

    /// Halves against one tomogram, on a noisy phantom (slow on the CPU):
    /// cargo test --release --features pro --lib wedge::tests::halves_beat -- --ignored --nocapture
    /// (env as for learns_to_fill_the_wedge, WNOISE noise σ per half, WO omega,
    /// WONE set = skip the one-tomogram run). Runs on the GPU when there is one.
    #[test]
    #[ignore]
    fn halves_beat_one_tomogram() {
        let (dev, name) = crate::classify::device();
        eprintln!("device {name}");
        let env = |k: &str, d: f64| std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d);
        let n = env("WN", 64.0) as usize;
        let truth = phantom(n, 21);
        let g = Geometry { beam: 2, tilt: 1, max_tilt: 60.0, min_tilt: None };
        let sp = Spectral::new(n, &g);
        let sigma = env("WNOISE", 1.0) as f32;
        let mut rng = Rng(77);
        // Each half: the specimen plus its own noise, both wedge-cut.
        let mut half = || -> Vec<f32> { sp.cut(&truth.v.iter().map(|v| v + sigma * rng.gauss()).collect::<Vec<f32>>()) };
        let (a, b) = (half(), half());
        let full: Vec<f32> = a.iter().zip(&b).map(|(x, y)| 0.5 * (x + y)).collect();
        let cfg = TrainConfig {
            cube: env("WC", 32.0) as usize,
            count: 32,
            iterations: env("WI", 3.0) as usize,
            steps: env("WS", 150.0) as usize,
            batch: 4,
            lr: env("WL", 1e-3),
            base: env("WB", 8.0) as usize,
            levels: 2,
            seed: 9,
            omega: env("WO", 2.0) as f32,
            ..Default::default()
        };
        // Correlation with the truth, overall and inside the wedge only.
        let score = |v: &[f32]| -> (f32, f32) {
            let corr = |x: &[f32], y: &[f32]| {
                let (mx, my) = (x.iter().sum::<f32>() / x.len() as f32, y.iter().sum::<f32>() / y.len() as f32);
                let (mut sxy, mut sxx, mut syy) = (0f32, 0f32, 0f32);
                for (p, q) in x.iter().zip(y) {
                    sxy += (p - mx) * (q - my);
                    sxx += (p - mx).powi(2);
                    syy += (q - my).powi(2);
                }
                sxy / (sxx * syy).sqrt().max(1e-12)
            };
            let wedge = |x: &[f32]| -> Vec<f32> { x.iter().zip(sp.cut(x)).map(|(a, b)| a - b).collect() };
            (corr(v, &truth.v), corr(&wedge(v), &wedge(&truth.v)))
        };
        eprintln!("input (mean of halves): {:?}", score(&full));
        if std::env::var("WONE").is_err() {
        let t0 = std::time::Instant::now();
        let one = train_and_apply(&dev, &full, truth.n, [-60.0, 60.0], &TrainConfig { smooth: 1.0, ..cfg.clone() }, |_| {}).unwrap();
        eprintln!("one tomogram, σ1 pre-smooth: {:?} in {:.0?}", score(&one), t0.elapsed());
        }
        let tilts: Vec<f32> = (-20..=20).map(|i| i as f32 * 3.0).collect();
        let t0 = std::time::Instant::now();
        let two = train_halves_and_apply(&dev, &a, &b, truth.n, &tilts, &cfg, |p| if p.step == p.steps { eprintln!("round {} loss {}", p.iteration, p.loss) }).unwrap();
        eprintln!("halves: {:?} in {:.0?}", score(&two), t0.elapsed());
    }

    #[test]
    fn trains_and_fills() {
        let dev = Device::flex();
        let g = Geometry { beam: 2, tilt: 1, max_tilt: 60.0, min_tilt: None };
        let truth = phantom(24, 4);
        let values: Vec<f32> = Spectral::new(32, &g).cut(&phantom(32, 4).v)[..24 * 24 * 24].to_vec();
        let cfg = TrainConfig { cube: 16, count: 4, iterations: 2, steps: 3, batch: 2, lr: 1e-3, noise: 0.1, base: 4, levels: 2, seed: 1, smooth: 1.0, omega: 2.0 };
        let mut last = None;
        let out = train_and_apply(&dev, &values, truth.n, [-60.0, 60.0], &cfg, |p| last = Some(p)).unwrap();
        assert_eq!(out.len(), values.len());
        assert!(out.iter().all(|v| v.is_finite()));
        let p = last.unwrap();
        assert_eq!((p.iteration, p.step, p.steps), (3, 1, 1));
    }

    /// Quality on a phantom (slow on the CPU):
    /// cargo test --release --features pro --lib wedge::tests::learns -- --ignored --nocapture
    /// (WI iterations, WS steps per iteration).
    #[test]
    #[ignore]
    fn learns_to_fill_the_wedge() {
        let dev = Device::flex();
        let n = std::env::var("WN").ok().and_then(|s| s.parse().ok()).unwrap_or(32);
        let cube = std::env::var("WC").ok().and_then(|s| s.parse().ok()).unwrap_or(16);
        let truth = phantom(n, 21);
        let g = Geometry { beam: 2, tilt: 1, max_tilt: 60.0, min_tilt: None };
        let sp = Spectral::new(n, &g);
        let vol = Vol { n: truth.n, v: sp.cut(&truth.v) };
        let iters: usize = std::env::var("WI").ok().and_then(|s| s.parse().ok()).unwrap_or(6);
        let steps: usize = std::env::var("WS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
        let base = std::env::var("WB").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
        let lr = std::env::var("WL").ok().and_then(|s| s.parse().ok()).unwrap_or(2e-3);
        let cfg = TrainConfig { cube, count: 24, iterations: iters, steps, batch: 4, lr, noise: 0.0, base, levels: 2, seed: 9, smooth: 0.0, omega: 2.0 };
        let (net, _) = train(&dev, &vol, &g, &cfg, |p| if p.step == p.steps { eprintln!("round {} loss {}", p.iteration, p.loss) }).unwrap();
        let out = apply(&dev, &net, &vol, [0; 3], vol.n, &mut |_, _| Ok(())).unwrap();
        // Error inside the wedge, before (nothing there) and after.
        let wedge = |v: &[f32]| -> f32 {
            let d: Vec<f32> = v.iter().zip(&truth.v).map(|(a, b)| a - b).collect();
            let inside: Vec<f32> = d.iter().zip(sp.cut(&d)).map(|(a, b)| a - b).collect();
            inside.iter().map(|x| x * x).sum::<f32>().sqrt()
        };
        let (before, after) = (wedge(&vol.v), wedge(&out));
        eprintln!("wedge error {before} -> {after}");
        assert!(after < before * 0.9, "{after} vs {before}");
    }

    #[test]
    fn preview_matches_the_whole_volume() {
        let dev = Device::flex();
        let net = Net::new(4, 2, &dev);
        // A trained-looking net: perturb the zero head so the output differs from the input.
        let mut rng = Rng(3);
        let n = [40, 36, 30];
        let vol = Vol { n, v: (0..n[0] * n[1] * n[2]).map(|_| rng.gauss()).collect() };
        let net = {
            let mut net = net;
            net.head.weight = net.head.weight.map(|w| w.ones_like() * 0.3);
            net
        };
        let full = apply(&dev, &net, &vol, [0; 3], n, &mut |_, _| Ok(())).unwrap();
        let m = Trained { net, vol: Arc::new(vol), geometry: Geometry { beam: 2, tilt: 1, max_tilt: 60.0, min_tilt: None }, bin: 1, mean: 0.0, std: 1.0, loss: 0.0, token: 7, c: 0, t: 0 };
        for (axis, index) in [("xy", 11), ("xz", 17), ("yz", 5)] {
            let req = PreviewReq { token: 7, axis: axis.into(), index, region: [0, 0, 1000, 1000] };
            let bytes = preview(&m, &req).unwrap();
            let u32_at = |k: usize| u32::from_le_bytes(bytes[k * 4..k * 4 + 4].try_into().unwrap()) as usize;
            let (w, h) = (u32_at(4), u32_at(5));
            let vals: Vec<f32> = bytes[44..].chunks(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
            assert_eq!(vals.len(), w * h);
            for j in 0..h {
                for i in 0..w {
                    let p = match axis {
                        "xy" => [i, j, index],
                        "xz" => [i, index, j],
                        _ => [index, i, j],
                    };
                    let want = full[(p[2] * n[1] + p[1]) * n[0] + p[0]];
                    assert!((vals[j * w + i] - want).abs() < 1e-3, "{axis} ({i},{j}): {} vs {want}", vals[j * w + i]);
                }
            }
        }
    }

    #[test]
    fn apply_tiles_any_box() {
        let dev = Device::flex();
        let net = Net::new(4, 2, &dev);
        let vol = Vol { n: [70, 20, 9], v: vec![0.5; 70 * 20 * 9] };
        let out = apply(&dev, &net, &vol, [3, 0, 4], [67, 20, 1], &mut |_, _| Ok(())).unwrap();
        assert_eq!(out.len(), 67 * 20);
        let mid = out[10 * 67 + 30];
        assert!(out.iter().all(|v| v.is_finite() && (v - mid).abs() < 1e-3));
    }
}

/// Time and result on a real tomogram:
/// TMV_WEDGE_FILE=<tomogram> cargo test --release --features pro --lib wedge::bench -- --ignored --nocapture
/// TMV_WEDGE_BIN (default 1), TMV_WEDGE_ITER / TMV_WEDGE_STEPS / TMV_WEDGE_NOISE / TMV_WEDGE_SMOOTH override the
/// defaults; TMV_WEDGE_OUT=<file.mrc> saves the corrected volume.
#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    #[ignore]
    fn bench() {
        let Ok(path) = std::env::var("TMV_WEDGE_FILE") else { return };
        let env = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<f64>().ok());
        let src = crate::formats::open(std::path::Path::new(&path)).unwrap();
        let img = OpenImage::new(1, src, 1 << 30);
        let bin = env("TMV_WEDGE_BIN").unwrap_or(1.0) as usize;
        let t0 = std::time::Instant::now();
        let (vol, mean, std) = load(&img, 0, 0, bin).unwrap();
        println!("{path}: {:?} after bin {bin}, read in {:.1} s", vol.n, t0.elapsed().as_secs_f64());
        let d = detect(&vol);
        println!("  detected {:?}, contrast {:.2}, profile {:?}", d.geometry, d.contrast, d.profile.iter().map(|v| (v * 100.0).round() / 100.0).collect::<Vec<_>>());
        let mut cfg = TrainConfig::default();
        if let Some(v) = env("TMV_WEDGE_ITER") {
            cfg.iterations = v as usize;
        }
        if let Some(v) = env("TMV_WEDGE_STEPS") {
            cfg.steps = v as usize;
        }
        if let Some(v) = env("TMV_WEDGE_NOISE") {
            cfg.noise = v as f32;
        }
        if let Some(v) = env("TMV_WEDGE_SMOOTH") {
            cfg.smooth = v as f32;
        }
        let vol = smooth(&vol, cfg.smooth);
        while vol.n.iter().any(|&k| k < cfg.cube) && cfg.cube > 16 {
            cfg.cube /= 2;
        }
        let t1 = std::time::Instant::now();
        let ((net, loss), backend) = crate::classify::on_device(|dev| train(dev, &vol, &d.geometry, &cfg, |_| {})).unwrap();
        println!("  trained on {backend}: {} it × {} steps, cube {}, loss {loss:.4}, {:.1} s", cfg.iterations, cfg.steps, cfg.cube, t1.elapsed().as_secs_f64());
        let m = Trained { net, vol: Arc::new(vol), geometry: d.geometry, bin, mean, std, loss, token: 1, c: 0, t: 0 };
        // The preview must show what saving writes: compare a slice per view.
        let n = m.vol.n;
        let (full, b) = crate::classify::on_device(|dev| apply(dev, &m.net.clone().to_device(dev), &m.vol, [0; 3], n, &mut |_, _| Ok(()))).unwrap();
        for (axis, index) in [("xy", n[2] / 2), ("xz", n[1] / 2), ("yz", n[0] / 2)] {
            let bytes = preview(&m, &PreviewReq { token: 1, axis: axis.into(), index: index * bin, region: [0, 0, 1 << 20, 1 << 20] }).unwrap();
            let at = |k: usize| u32::from_le_bytes(bytes[k * 4..k * 4 + 4].try_into().unwrap()) as usize;
            let (w, h, x0, y0) = (at(4), at(5), at(0) / bin, at(1) / bin);
            let mut worst = 0f32;
            for (k, c) in bytes[44..].chunks(4).enumerate() {
                let (i, j) = (x0 + k % w, y0 + k / w);
                let p = match axis { "xy" => [i, j, index], "xz" => [i, index, j], _ => [index, i, j] };
                let want = full[(p[2] * n[1] + p[1]) * n[0] + p[0]] * m.std + m.mean;
                worst = worst.max((f32::from_le_bytes(c.try_into().unwrap()) - want).abs());
            }
            println!("  preview {axis} {w}×{h} vs whole volume on {b}: max difference {worst:.2e} (data spread {:.3})", m.std);
        }
        if let Ok(out) = std::env::var("TMV_WEDGE_OUT") {
            let t2 = std::time::Instant::now();
            let b = save(&m, &out, crate::align::pixel_a(img.info()), &|_, _| {}).unwrap();
            println!("  applied and saved on {b} in {:.1} s → {out}", t2.elapsed().as_secs_f64());
        }
    }
}
