//! Training augmentation for the membrane network, after the list in the
//! MemBrain-seg paper (itself after nnU-Net): random rotation, right-angle
//! turns, mirroring and axis swaps, zoom, blur, noise, brightness, contrast,
//! low resolution, gamma with inversion, blanked boxes, brightness gradients,
//! local gamma, sharpening, and a random change of the amplitude of the
//! middle and high frequencies (their "Fourier amplitude" augmentation, as a
//! cheap band-pass gain). Out-of-volume voxels become "ignore" in the labels.

use rayon::prelude::*;

use crate::wedge::{smooth, Vol};

/// Label values: background, membrane, ignore.
pub const BG: u8 = 0;
pub const MEM: u8 = 1;
pub const IGNORE: u8 = 2;

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f32 {
        ((self.next() >> 40) as f32) / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    pub fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }
    pub fn gauss(&mut self) -> f32 {
        let u = self.f().max(1e-7);
        let v = self.f();
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f32::consts::PI * v).cos()
    }
}

type M3 = [[f32; 3]; 3];

fn mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn rot(axis: usize, t: f32) -> M3 {
    let (s, c) = t.sin_cos();
    let mut m = [[0.0; 3]; 3];
    m[axis][axis] = 1.0;
    let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
    m[i][i] = c;
    m[i][j] = -s;
    m[j][i] = s;
    m[j][j] = c;
    m
}

/// A random training sample of side `p` from a source patch: image
/// (standardised) and labels, both X fastest.
pub fn sample(img: &[f32], lab: &[u8], n: [usize; 3], p: usize, rng: &mut Rng) -> (Vec<f32>, Vec<u8>) {
    // Spatial: signed axis permutation (turns, mirrors, swaps), a small
    // rotation and zoom.
    let mut m: M3 = [[0.0; 3]; 3];
    let perm = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]][rng.below(6)];
    for (i, &j) in perm.iter().enumerate() {
        m[i][j] = if rng.chance(0.5) { -1.0 } else { 1.0 };
    }
    if rng.chance(0.75) {
        let a = 30f32.to_radians();
        for axis in 0..3 {
            m = mul(&m, &rot(axis, rng.range(-a, a)));
        }
    }
    let zoom = if rng.chance(0.3) { rng.range(0.7f32.ln(), 1.43f32.ln()).exp() } else { 1.0 };
    m.iter_mut().for_each(|r| r.iter_mut().for_each(|v| *v /= zoom));
    // Centre: anywhere the window fits, biased towards membrane voxels
    // half of the time so thin structures are seen often.
    let reach = p as f32 * 0.5 / zoom;
    let centre: [f32; 3] = {
        let mem: Vec<usize> = if rng.chance(0.5) { lab.iter().enumerate().filter(|(_, &l)| l == MEM).map(|(i, _)| i).step_by(97).collect() } else { vec![] };
        if !mem.is_empty() {
            let i = mem[rng.below(mem.len())];
            let c = [i % n[0], (i / n[0]) % n[1], i / (n[0] * n[1])];
            std::array::from_fn(|a| (c[a] as f32 + rng.range(-reach * 0.5, reach * 0.5)).clamp(0.0, n[a] as f32 - 1.0))
        } else {
            std::array::from_fn(|a| {
                let lo = reach.min(n[a] as f32 / 2.0);
                let hi = (n[a] as f32 - reach).max(n[a] as f32 / 2.0);
                rng.range(lo, hi)
            })
        }
    };
    let half = (p as f32 - 1.0) / 2.0;
    let mut out = vec![0f32; p * p * p];
    let mut lout = vec![IGNORE; p * p * p];
    let at = |x: usize, y: usize, z: usize| (z * n[1] + y) * n[0] + x;
    out.par_chunks_mut(p * p).zip(lout.par_chunks_mut(p * p)).enumerate().for_each(|(z, (plane, lplane))| {
        for y in 0..p {
            for x in 0..p {
                let q = [x as f32 - half, y as f32 - half, z as f32 - half];
                let s: [f32; 3] = std::array::from_fn(|i| centre[i] + m[i][0] * q[0] + m[i][1] * q[1] + m[i][2] * q[2]);
                let o = y * p + x;
                if s.iter().zip(&n).any(|(&v, &k)| v < -0.5 || v > k as f32 - 0.5) {
                    continue;
                }
                let c: [usize; 3] = std::array::from_fn(|a| s[a].round().clamp(0.0, n[a] as f32 - 1.0) as usize);
                lplane[o] = lab[at(c[0], c[1], c[2])];
                let f: [f32; 3] = std::array::from_fn(|a| s[a].clamp(0.0, n[a] as f32 - 1.0));
                let i0: [usize; 3] = std::array::from_fn(|a| f[a].floor() as usize);
                let i1: [usize; 3] = std::array::from_fn(|a| (i0[a] + 1).min(n[a] - 1));
                let t: [f32; 3] = std::array::from_fn(|a| f[a] - i0[a] as f32);
                let mut v = 0.0;
                for (dz, wz) in [(i0[2], 1.0 - t[2]), (i1[2], t[2])] {
                    for (dy, wy) in [(i0[1], 1.0 - t[1]), (i1[1], t[1])] {
                        for (dx, wx) in [(i0[0], 1.0 - t[0]), (i1[0], t[0])] {
                            v += wz * wy * wx * img[at(dx, dy, dz)];
                        }
                    }
                }
                plane[o] = v;
            }
        }
    });
    intensity(&mut out, p, rng);
    (out, lout)
}

fn blur(v: &[f32], p: usize, sigma: f32) -> Vec<f32> {
    smooth(&Vol { n: [p; 3], v: v.to_vec() }, sigma).v
}

fn mean_std(v: &[f32]) -> (f32, f32) {
    let n = v.len() as f64;
    let m = v.par_iter().map(|&x| x as f64).sum::<f64>() / n;
    let s = (v.par_iter().map(|&x| (x as f64 - m).powi(2)).sum::<f64>() / n).sqrt();
    (m as f32, (s as f32).max(1e-6))
}

/// A smooth random blob in [0, 1] (Gaussian of random centre and width).
fn blob(p: usize, rng: &mut Rng) -> Vec<f32> {
    let c: [f32; 3] = std::array::from_fn(|_| rng.range(-0.5, 1.5) * p as f32);
    let s: [f32; 3] = std::array::from_fn(|_| rng.range((p as f32 / 6.0).ln(), (p as f32).ln()).exp());
    let mut out = vec![0f32; p * p * p];
    out.par_chunks_mut(p * p).enumerate().for_each(|(z, plane)| {
        for y in 0..p {
            for x in 0..p {
                let d = [(x as f32 - c[0]) / s[0], (y as f32 - c[1]) / s[1], (z as f32 - c[2]) / s[2]];
                plane[y * p + x] = (-0.5 * (d[0] * d[0] + d[1] * d[1] + d[2] * d[2])).exp();
            }
        }
    });
    let mx = out.iter().cloned().fold(1e-6, f32::max);
    out.par_iter_mut().for_each(|v| *v /= mx);
    out
}

fn gamma(v: &mut [f32], g: f32, invert: bool, weight: Option<&[f32]>) {
    let (m, s) = mean_std(v);
    let lo = v.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = v.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let r = (hi - lo).max(1e-6);
    v.par_iter_mut().enumerate().for_each(|(i, x)| {
        let mut u = (*x - lo) / r;
        if invert {
            u = 1.0 - u;
        }
        let mut y = u.powf(g);
        if invert {
            y = 1.0 - y;
        }
        let y = y * r + lo;
        *x = match weight {
            Some(w) => *x * (1.0 - w[i]) + y * w[i],
            None => y,
        };
    });
    let (m2, s2) = mean_std(v);
    v.par_iter_mut().for_each(|x| *x = (*x - m2) / s2 * s + m);
}

fn intensity(v: &mut Vec<f32>, p: usize, rng: &mut Rng) {
    // Blur, light or strong (strong stands in for their median filter).
    if rng.chance(0.3) {
        *v = blur(v, p, rng.range(0.3, 1.5));
    } else if rng.chance(0.2) {
        *v = blur(v, p, rng.range(1.0, 2.5));
    }
    // Band-pass gain: changes the amplitude of mid/high frequencies.
    if rng.chance(0.3) {
        let low = blur(v, p, rng.range(1.0, 4.0));
        let a = rng.range(-0.7, 1.5);
        v.par_iter_mut().zip(&low).for_each(|(x, l)| *x += a * (*x - l));
    }
    if rng.chance(0.4) {
        let sd = rng.range(0.0, 0.7);
        let seeds: Vec<u64> = (0..p).map(|_| rng.next() | 1).collect();
        v.par_chunks_mut(p * p).zip(seeds).for_each(|(plane, s)| {
            let mut r = Rng(s);
            plane.iter_mut().for_each(|x| *x += sd * r.gauss());
        });
    }
    if rng.chance(0.3) {
        let b = 0.5 * rng.gauss();
        v.par_iter_mut().for_each(|x| *x += b);
    }
    if rng.chance(0.3) {
        let f = if rng.chance(0.5) { rng.range(0.5, 1.0) } else { rng.range(1.0, 2.0) };
        let (m, _) = mean_std(v);
        let (lo, hi) = (v.iter().cloned().fold(f32::INFINITY, f32::min), v.iter().cloned().fold(f32::NEG_INFINITY, f32::max));
        let keep = rng.chance(0.5);
        v.par_iter_mut().for_each(|x| {
            *x = (*x - m) * f + m;
            if keep {
                *x = x.clamp(lo, hi);
            }
        });
    }
    // Low resolution: shrink by nearest neighbour, grow back trilinearly.
    if rng.chance(0.35) {
        let k = rng.range(0.25, 1.0);
        let q = ((p as f32 * k).round() as usize).max(4);
        if q < p {
            let small: Vec<f32> = (0..q * q * q)
                .map(|i| {
                    let c = [i % q, (i / q) % q, i / (q * q)];
                    let s: [usize; 3] = std::array::from_fn(|a| (((c[a] as f32 + 0.5) * p as f32 / q as f32) as usize).min(p - 1));
                    v[(s[2] * p + s[1]) * p + s[0]]
                })
                .collect();
            *v = super::resample(&small, [q; 3], [p; 3]);
        }
    }
    if rng.chance(0.25) {
        for _ in 0..2 {
            let g = if rng.chance(0.5) { rng.range(0.7, 1.0) } else { rng.range(1.0, 1.5) };
            let inv = rng.chance(0.5);
            gamma(v, g, inv, None);
        }
    }
    if rng.chance(0.4) {
        let (m, _) = mean_std(v);
        for _ in 0..1 + rng.below(5) {
            let side: [usize; 3] = std::array::from_fn(|_| (p / 10 + rng.below(p / 3 - p / 10 + 1)).max(1));
            let o: [usize; 3] = std::array::from_fn(|a| rng.below(p - side[a] + 1));
            for z in o[2]..o[2] + side[2] {
                for y in o[1]..o[1] + side[1] {
                    let row = (z * p + y) * p;
                    v[row + o[0]..row + o[0] + side[0]].fill(m);
                }
            }
        }
    }
    if rng.chance(0.3) {
        let b = blob(p, rng);
        let s = if rng.chance(0.5) { rng.range(-5.0, -1.0) } else { rng.range(1.0, 5.0) };
        let (_, sd) = mean_std(v);
        // Strength relative to the spread (their scale is the raw range).
        let s = s * sd * 0.2;
        v.par_iter_mut().zip(&b).for_each(|(x, w)| *x += s * w);
    }
    if rng.chance(0.3) {
        let b = blob(p, rng);
        let g = if rng.chance(0.5) { rng.range(0.01, 0.8) } else { rng.range(1.5, 4.0) };
        gamma(v, g, false, Some(&b));
    }
    if rng.chance(0.2) {
        let low = blur(v, p, 1.0);
        let s = rng.range(0.1, 1.0);
        v.par_iter_mut().zip(&low).for_each(|(x, l)| *x += s * (*x - l));
    }
    // Back to mean 0, spread 1, as at inference.
    let (m, s) = mean_std(v);
    v.par_iter_mut().for_each(|x| *x = (*x - m) / s);
}

/// Labels at half the resolution: membrane if any of the 2³ block is,
/// else ignore if any is, else background.
pub fn halve(lab: &[u8], p: usize) -> Vec<u8> {
    let q = p / 2;
    (0..q * q * q)
        .map(|i| {
            let c = [i % q, (i / q) % q, i / (q * q)];
            let mut any_ign = false;
            for dz in 0..2 {
                for dy in 0..2 {
                    for dx in 0..2 {
                        match lab[((2 * c[2] + dz) * p + 2 * c[1] + dy) * p + 2 * c[0] + dx] {
                            MEM => return MEM,
                            IGNORE => any_ign = true,
                            _ => {}
                        }
                    }
                }
            }
            if any_ign { IGNORE } else { BG }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_shapes_and_labels() {
        let n = [40, 40, 40];
        let img: Vec<f32> = (0..n[0] * n[1] * n[2]).map(|i| (i % 7) as f32).collect();
        let lab: Vec<u8> = (0..n[0] * n[1] * n[2]).map(|i| if (i / 40) % 40 == 20 { MEM } else { BG }).collect();
        let mut r = Rng(7);
        for _ in 0..5 {
            let (v, l) = sample(&img, &lab, n, 32, &mut r);
            assert_eq!(v.len(), 32 * 32 * 32);
            assert!(v.iter().all(|x| x.is_finite()));
            assert!(l.iter().all(|&x| x <= IGNORE));
        }
        assert_eq!(halve(&vec![MEM; 8], 2), vec![MEM]);
    }
}
