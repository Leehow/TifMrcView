//! Post-processing: Fourier shell correlation between half maps, the
//! resolution where it falls below a threshold (0.143, Rosenthal &
//! Henderson, J Mol Biol 2003), an automatic soft mask, and sharpening with
//! the B factor from a Guinier fit of the shell amplitudes (Rosenthal &
//! Henderson 2003), weighted by the FSC.

use rayon::prelude::*;
use serde::Serialize;

use super::data::soft;
use super::fourier;
use super::pick::gauss;
use super::refine::lowpass;
use super::volume::mask_sphere;

fn shell_of(z: usize, y: usize, x: usize, b: usize) -> f64 {
    let (fx, fy, fz) = (fourier::freq(x, b), fourier::freq(y, b), fourier::freq(z, b));
    ((fx * fx + fy * fy + fz * fz) as f64).sqrt()
}

/// FSC per shell (0..=b/2) of two maps after a soft spherical mask of `radius` pixels.
pub fn fsc(a: &[f32], c: &[f32], b: usize, radius: f64) -> Vec<f64> {
    let (mut a, mut c) = (a.to_vec(), c.to_vec());
    mask_sphere(&mut a, b, radius, 6.0);
    mask_sphere(&mut c, b, radius, 6.0);
    fsc_raw(&a, &c, b)
}

pub fn fsc_raw(a: &[f32], c: &[f32], b: usize) -> Vec<f64> {
    let (ar, ai) = fourier::ft3(b, a);
    let (cr, ci) = fourier::ft3(b, c);
    let shells = b / 2 + 1;
    let parts: Vec<Vec<f64>> = (0..b)
        .into_par_iter()
        .map(|z| {
            let mut s = vec![0f64; 3 * shells];
            for y in 0..b {
                for x in 0..b {
                    let r = shell_of(z, y, x, b).round() as usize;
                    if r >= shells {
                        continue;
                    }
                    let j = (z * b + y) * b + x;
                    s[r] += (ar[j] * cr[j] + ai[j] * ci[j]) as f64;
                    s[shells + r] += (ar[j] * ar[j] + ai[j] * ai[j]) as f64;
                    s[2 * shells + r] += (cr[j] * cr[j] + ci[j] * ci[j]) as f64;
                }
            }
            s
        })
        .collect();
    let mut t = vec![0f64; 3 * shells];
    for p in parts {
        for (a, b) in t.iter_mut().zip(p) {
            *a += b;
        }
    }
    (0..shells)
        .map(|r| {
            if r == 0 {
                return 1.0;
            }
            let d = (t[shells + r] * t[2 * shells + r]).sqrt();
            if d > 0.0 { t[r] / d } else { 0.0 }
        })
        .collect()
}

/// Resolution (Å) of the first shell after the low ones where the FSC drops
/// below `thr` (interpolated); Nyquist when it never does.
pub fn resolution(f: &[f64], b: usize, px: f64, thr: f64) -> f64 {
    for r in 2..f.len() {
        if f[r] < thr {
            let (a, c) = (f[r - 1], f[r]);
            let t = if (a - c).abs() > 1e-12 { (a - thr) / (a - c) } else { 0.0 };
            let s = (r as f64 - 1.0 + t.clamp(0.0, 1.0)).max(1.0);
            return b as f64 * px / s;
        }
    }
    2.0 * px
}

/// Otsu threshold of the values inside a sphere.
fn otsu(v: &[f32], b: usize, radius: f64) -> f32 {
    let c = b as f64 / 2.0;
    let mut vals = vec![];
    for z in 0..b {
        for y in 0..b {
            for x in 0..b {
                let d = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2) + (z as f64 - c).powi(2)).sqrt();
                if d < radius {
                    vals.push(v[(z * b + y) * b + x]);
                }
            }
        }
    }
    if vals.is_empty() {
        return 0.0;
    }
    let lo = vals.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = vals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if hi <= lo {
        return lo;
    }
    let bins = 256;
    let mut h = vec![0f64; bins];
    for &x in &vals {
        h[(((x - lo) / (hi - lo)) * (bins - 1) as f32) as usize] += 1.0;
    }
    let total: f64 = h.iter().sum();
    let sum_all: f64 = h.iter().enumerate().map(|(i, c)| i as f64 * c).sum();
    let (mut w0, mut s0, mut best, mut at) = (0.0, 0.0, -1.0, 0);
    for (i, &c) in h.iter().enumerate() {
        w0 += c;
        s0 += i as f64 * c;
        let w1 = total - w0;
        if w0 == 0.0 || w1 == 0.0 {
            continue;
        }
        let (m0, m1) = (s0 / w0, (sum_all - s0) / w1);
        let between = w0 * w1 * (m0 - m1).powi(2);
        if between > best {
            best = between;
            at = i;
        }
    }
    lo + (at as f32 + 0.5) / (bins - 1) as f32 * (hi - lo)
}

/// A soft mask around the molecule: the map low-passed to 15 Å, thresholded
/// (Otsu inside the particle sphere), dilated by 3 pixels, with a soft edge.
pub fn auto_mask(v: &[f32], b: usize, px: f64, radius: f64) -> Vec<f32> {
    let lp = lowpass(v, b, px, 15f64.max(3.0 * px));
    let t = otsu(&lp, b, radius);
    let bin: Vec<f32> = lp.iter().map(|&x| if x > t { 1.0 } else { 0.0 }).collect();
    // Dilate and soften: blur the binary mask (separable over planes and z).
    let sigma = 3.0;
    let mut m = blur3(&bin, b, sigma);
    m.par_iter_mut().for_each(|x| *x = (*x * 3.0).min(1.0));
    let c = b as f64 / 2.0;
    m.par_chunks_mut(b * b).enumerate().for_each(|(z, plane)| {
        for y in 0..b {
            for x in 0..b {
                let d = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2) + (z as f64 - c).powi(2)).sqrt();
                plane[y * b + x] *= soft(d, b as f64 * 0.45, 3.0);
            }
        }
    });
    m
}

fn blur3(v: &[f32], b: usize, sigma: f64) -> Vec<f32> {
    // x and y within each plane, then z.
    let mut out: Vec<f32> = v.par_chunks(b * b).flat_map_iter(|p| gauss(p, b, b, sigma)).collect();
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp() as f32).collect();
    let s: f32 = k.iter().sum();
    let src = out.clone();
    out.par_chunks_mut(b * b).enumerate().for_each(|(z, plane)| {
        plane.iter_mut().for_each(|x| *x = 0.0);
        for (j, kv) in k.iter().enumerate() {
            let zz = (z as isize + j as isize - r).clamp(0, b as isize - 1) as usize;
            for (o, v) in plane.iter_mut().zip(&src[zz * b * b..(zz + 1) * b * b]) {
                *o += kv / s * v;
            }
        }
    });
    out
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sharpened {
    pub bfactor: f64,
    pub res_masked: f64,
    pub fsc_masked: Vec<f64>,
}

/// Masked FSC and sharpening: returns the sharpened map (FSC-weighted,
/// B-factor applied, low-passed at the resolution) and the numbers.
pub fn sharpen(full: &[f32], h1: &[f32], h2: &[f32], mask: &[f32], b: usize, px: f64, bfactor: Option<f64>) -> (Vec<f32>, Sharpened) {
    let m1: Vec<f32> = h1.iter().zip(mask).map(|(a, m)| a * m).collect();
    let m2: Vec<f32> = h2.iter().zip(mask).map(|(a, m)| a * m).collect();
    let f = fsc_raw(&m1, &m2, b);
    let res = resolution(&f, b, px, 0.143);
    let mf: Vec<f32> = full.iter().zip(mask).map(|(a, m)| a * m).collect();
    let (mut re, mut im) = fourier::ft3(b, &mf);
    // Guinier fit of ln|F| against 1/d² from 10 Å to the resolution.
    let shells = b / 2 + 1;
    let mut amp = vec![0f64; shells];
    let mut cnt = vec![0f64; shells];
    for z in 0..b {
        for y in 0..b {
            for x in 0..b {
                let r = shell_of(z, y, x, b).round() as usize;
                if r < shells {
                    let j = (z * b + y) * b + x;
                    amp[r] += ((re[j] * re[j] + im[j] * im[j]) as f64).sqrt();
                    cnt[r] += 1.0;
                }
            }
        }
    }
    let r_lo = (b as f64 * px / 10.0).ceil() as usize;
    let r_hi = (b as f64 * px / res).floor() as usize;
    let fitted = {
        let pts: Vec<(f64, f64)> = (r_lo.max(1)..=r_hi.min(shells - 1))
            .filter(|&r| amp[r] > 0.0)
            .map(|r| ((r as f64 / (b as f64 * px)).powi(2), (amp[r] / cnt[r]).ln()))
            .collect();
        if pts.len() >= 3 {
            let n = pts.len() as f64;
            let (sx, sy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
            let (mx, my) = (sx / n, sy / n);
            let num: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
            let den: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
            if den > 0.0 { -4.0 * num / den } else { 0.0 }
        } else {
            0.0
        }
    };
    let bf = bfactor.unwrap_or(fitted.max(0.0));
    let r_res = b as f64 * px / res;
    re.par_chunks_mut(b * b).zip(im.par_chunks_mut(b * b)).enumerate().for_each(|(z, (pr, pi))| {
        for y in 0..b {
            for x in 0..b {
                let s = shell_of(z, y, x, b);
                let r = s.round() as usize;
                let fv = f.get(r).copied().unwrap_or(0.0).max(0.0);
                let w = (2.0 * fv / (1.0 + fv)).sqrt();
                let s2 = (s / (b as f64 * px)).powi(2);
                let g = (w * (bf / 4.0 * s2).exp() * soft(s, r_res, 2.0) as f64) as f32;
                pr[y * b + x] *= g;
                pi[y * b + x] *= g;
            }
        }
    });
    let out = fourier::ift3(b, &re, &im);
    (out, Sharpened { bfactor: bf, res_masked: res, fsc_masked: f })
}

/// Flips the hand (mirror along z).
pub fn flip_hand(v: &[f32], b: usize) -> Vec<f32> {
    let mut out = vec![0f32; v.len()];
    for z in 0..b {
        let zz = (b - z) % b;
        out[z * b * b..(z + 1) * b * b].copy_from_slice(&v[zz * b * b..(zz + 1) * b * b]);
    }
    out
}
