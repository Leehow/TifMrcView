//! Fourier transforms for particle boxes and volumes, with the phase origin
//! at the box centre (b/2), so a centred object has a real transform and a
//! rotation of the spectrum about index 0 rotates the object about its centre.
//! Indices are unshifted: index i holds frequency i for i < n/2, i − n above.

use crate::fft::Fft;
use rayon::prelude::*;

/// Signed frequency of index `i` in a transform of size `n`.
#[inline]
pub fn freq(i: usize, n: usize) -> i32 {
    if i < n / 2 { i as i32 } else { i as i32 - n as i32 }
}

/// Index of signed frequency `f`.
#[inline]
pub fn index(f: i32, n: usize) -> usize {
    f.rem_euclid(n as i32) as usize
}

/// (−1)^(kx+ky): moves the phase origin from index 0 to the centre.
#[inline]
fn sign(i: usize) -> f32 {
    if i & 1 == 0 { 1.0 } else { -1.0 }
}

/// Forward 2D transform of a b × b image (one thread; callers parallelise
/// over particles).
pub fn ft2(fft: &Fft, b: usize, img: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut re = img.to_vec();
    let mut im = vec![0f32; b * b];
    rows_cols(fft, b, &mut re, &mut im);
    for y in 0..b {
        for x in 0..b {
            let s = sign(x + y);
            re[y * b + x] *= s;
            im[y * b + x] *= s;
        }
    }
    (re, im)
}

/// Inverse of `ft2`, the real part.
pub fn ift2(fft: &Fft, b: usize, re: &[f32], im: &[f32]) -> Vec<f32> {
    let mut r = re.to_vec();
    // Conjugate, forward, conjugate: the inverse up to 1/n².
    let mut i: Vec<f32> = im.iter().map(|v| -v).collect();
    for y in 0..b {
        for x in 0..b {
            let s = sign(x + y);
            r[y * b + x] *= s;
            i[y * b + x] *= s;
        }
    }
    rows_cols(fft, b, &mut r, &mut i);
    let s = 1.0 / (b * b) as f32;
    r.iter_mut().for_each(|v| *v *= s);
    r
}

fn rows_cols(fft: &Fft, b: usize, re: &mut [f32], im: &mut [f32]) {
    for (r, i) in re.chunks_mut(b).zip(im.chunks_mut(b)) {
        fft.run(r, i);
    }
    let (mut cr, mut ci) = (vec![0f32; b], vec![0f32; b]);
    for x in 0..b {
        for y in 0..b {
            cr[y] = re[y * b + x];
            ci[y] = im[y * b + x];
        }
        fft.run(&mut cr, &mut ci);
        for y in 0..b {
            re[y * b + x] = cr[y];
            im[y * b + x] = ci[y];
        }
    }
}

/// Forward 3D transform of an n³ volume (z, y, x order), in parallel.
pub fn ft3(n: usize, vol: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let fft = Fft::new(n);
    let mut re = vol.to_vec();
    let mut im = vec![0f32; n * n * n];
    lines3(&fft, n, &mut re, &mut im);
    centre3(n, &mut re, &mut im);
    (re, im)
}

/// Inverse of `ft3`, the real part.
pub fn ift3(n: usize, re: &[f32], im: &[f32]) -> Vec<f32> {
    let fft = Fft::new(n);
    let mut r = re.to_vec();
    let mut i: Vec<f32> = im.par_iter().map(|v| -v).collect();
    centre3(n, &mut r, &mut i);
    lines3(&fft, n, &mut r, &mut i);
    let s = 1.0 / (n * n * n) as f32;
    r.par_iter_mut().for_each(|v| *v *= s);
    r
}

fn centre3(n: usize, re: &mut [f32], im: &mut [f32]) {
    re.par_chunks_mut(n * n).zip(im.par_chunks_mut(n * n)).enumerate().for_each(|(z, (r, i))| {
        for y in 0..n {
            for x in 0..n {
                let s = sign(x + y + z);
                r[y * n + x] *= s;
                i[y * n + x] *= s;
            }
        }
    });
}

/// Transforms along x, y and z (each pass on contiguous copies).
fn lines3(fft: &Fft, n: usize, re: &mut [f32], im: &mut [f32]) {
    re.par_chunks_mut(n).zip(im.par_chunks_mut(n)).for_each(|(r, i)| fft.run(r, i));
    // Along y, one z plane at a time.
    re.par_chunks_mut(n * n).zip(im.par_chunks_mut(n * n)).for_each(|(r, i)| {
        let (mut cr, mut ci) = (vec![0f32; n], vec![0f32; n]);
        for x in 0..n {
            for y in 0..n {
                cr[y] = r[y * n + x];
                ci[y] = i[y * n + x];
            }
            fft.run(&mut cr, &mut ci);
            for y in 0..n {
                r[y * n + x] = cr[y];
                i[y * n + x] = ci[y];
            }
        }
    });
    // Along z, one y row of columns at a time.
    let plane = n * n;
    let cols: Vec<(Vec<f32>, Vec<f32>)> = (0..plane)
        .into_par_iter()
        .map(|p| {
            let mut cr: Vec<f32> = (0..n).map(|z| re[z * plane + p]).collect();
            let mut ci: Vec<f32> = (0..n).map(|z| im[z * plane + p]).collect();
            fft.run(&mut cr, &mut ci);
            (cr, ci)
        })
        .collect();
    for (p, (cr, ci)) in cols.into_iter().enumerate() {
        for z in 0..n {
            re[z * plane + p] = cr[z];
            im[z * plane + p] = ci[z];
        }
    }
}

/// The half plane of a b × b transform out to radius `r_max` (FFT pixels),
/// DC left out: for each coefficient its index and frequency. The other
/// half follows from Hermitian symmetry, so a score over these counts each
/// pair once.
#[derive(Clone)]
pub struct Half {
    pub b: usize,
    pub idx: Vec<u32>,
    pub k: Vec<[f32; 2]>,
    /// Radius shell (rounded), for per-shell weights.
    pub shell: Vec<u16>,
}

impl Half {
    pub fn new(b: usize, r_max: f64) -> Half {
        let r_max = r_max.min(b as f64 / 2.0 - 1.0);
        let r2 = r_max * r_max;
        let lim = r_max.floor() as i32;
        let (mut idx, mut k, mut shell) = (vec![], vec![], vec![]);
        for ky in -lim..=lim {
            for kx in 0..=lim {
                if kx == 0 && ky <= 0 {
                    continue;
                }
                let d2 = (kx * kx + ky * ky) as f64;
                if d2 > r2 {
                    continue;
                }
                idx.push((index(ky, b) * b + index(kx, b)) as u32);
                k.push([kx as f32, ky as f32]);
                shell.push(d2.sqrt().round() as u16);
            }
        }
        Half { b, idx, k, shell }
    }
    pub fn len(&self) -> usize {
        self.idx.len()
    }
}

/// Fourier crop (or pad) of a b × b transform to c × c, keeping the
/// frequencies both share; values are rescaled so real-space intensities stay.
pub fn resize2(re: &[f32], im: &[f32], b: usize, c: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut or, mut oi) = (vec![0f32; c * c], vec![0f32; c * c]);
    let lim = (b.min(c) / 2) as i32;
    let s = (c * c) as f32 / (b * b) as f32;
    for fy in -lim..lim {
        for fx in -lim..lim {
            let i = index(fy, b) * b + index(fx, b);
            let o = index(fy, c) * c + index(fx, c);
            or[o] = re[i] * s;
            oi[o] = im[i] * s;
        }
    }
    (or, oi)
}

/// Real-space resampling of a b × b image to c × c through Fourier space.
pub fn resample2(img: &[f32], b: usize, c: usize) -> Vec<f32> {
    if b == c {
        return img.to_vec();
    }
    let (re, im) = ft2(&Fft::new(b), b, img);
    let (re, im) = resize2(&re, &im, b, c);
    ift2(&Fft::new(c), c, &re, &im)
}

/// Fourier resampling of an n³ volume to m³.
pub fn resample3(vol: &[f32], n: usize, m: usize) -> Vec<f32> {
    if n == m {
        return vol.to_vec();
    }
    let (re, im) = ft3(n, vol);
    let (mut or, mut oi) = (vec![0f32; m * m * m], vec![0f32; m * m * m]);
    let lim = (n.min(m) / 2) as i32;
    let s = (m * m * m) as f32 / (n * n * n) as f32;
    for fz in -lim..lim {
        for fy in -lim..lim {
            for fx in -lim..lim {
                let i = (index(fz, n) * n + index(fy, n)) * n + index(fx, n);
                let o = (index(fz, m) * m + index(fy, m)) * m + index(fx, m);
                or[o] = re[i] * s;
                oi[o] = im[i] * s;
            }
        }
    }
    ift3(m, &or, &oi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centred_transform_is_real_for_symmetric_objects() {
        let b = 16;
        let mut img = vec![0f32; b * b];
        for y in 0..b {
            for x in 0..b {
                let (dx, dy) = (x as f32 - 8.0, y as f32 - 8.0);
                img[y * b + x] = (-(dx * dx + dy * dy) / 8.0).exp();
            }
        }
        let (re, im) = ft2(&Fft::new(b), b, &img);
        assert!(im.iter().all(|v| v.abs() < 1e-4));
        assert!(re[0] > 0.0);
        let back = ift2(&Fft::new(b), b, &re, &im);
        assert!(back.iter().zip(&img).all(|(a, b)| (a - b).abs() < 1e-4));
    }

    #[test]
    fn volume_round_trip() {
        let n = 8;
        let v: Vec<f32> = (0..n * n * n).map(|i| ((i * 37) % 11) as f32).collect();
        let (re, im) = ft3(n, &v);
        let back = ift3(n, &re, &im);
        assert!(back.iter().zip(&v).all(|(a, b)| (a - b).abs() < 1e-3));
    }
}
