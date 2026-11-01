//! Power spectrum of a region of one slice, for looking at lattice
//! periodicities and the Thon rings of the contrast transfer function.
//!
//! The region is cut into overlapping N × N tiles (N a power of two); each
//! tile has its mean removed, is multiplied by a Hann window and Fourier
//! transformed, and the squared magnitudes are averaged (Welch's method), so
//! a large region gives a smooth spectrum instead of a noisy one. The FFT is
//! an iterative radix-2 Cooley–Tukey transform written here.

use std::f64::consts::PI;
use std::sync::Arc;

use anyhow::{bail, Result};
use rayon::prelude::*;

use crate::image::OpenImage;

/// Smallest and largest tile size.
pub const MIN_N: usize = 32;
pub const MAX_N: usize = 2048;
/// Most tiles averaged (for N ≤ 512); beyond this the stride grows.
const MAX_TILES: usize = 64;

/// Precomputed twiddle factors and bit reversal for one size.
pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<u32>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        let (cos, sin) = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .unzip();
        Fft { n, cos, sin, rev }
    }

    /// Forward transform in place.
    pub fn run(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

/// |FFT|² of an N × N tile (row major), unshifted.
pub(crate) fn power_2d(fft: &Fft, tile: &[f32]) -> Vec<f32> {
    let n = fft.n;
    let mut re = tile.to_vec();
    let mut im = vec![0f32; n * n];
    re.par_chunks_mut(n).zip(im.par_chunks_mut(n)).for_each(|(r, i)| fft.run(r, i));
    // Columns: transpose, transform the rows, and read back transposed.
    let (mut tr, mut ti) = (transpose(&re, n), transpose(&im, n));
    tr.par_chunks_mut(n).zip(ti.par_chunks_mut(n)).for_each(|(r, i)| fft.run(r, i));
    // tr[u][v] holds frequency (row v, column u).
    let mut out = vec![0f32; n * n];
    out.par_chunks_mut(n).enumerate().for_each(|(v, row)| {
        for (u, o) in row.iter_mut().enumerate() {
            let (a, b) = (tr[u * n + v], ti[u * n + v]);
            *o = a * a + b * b;
        }
    });
    out
}

fn transpose(a: &[f32], n: usize) -> Vec<f32> {
    let mut out = vec![0f32; n * n];
    out.par_chunks_mut(n).enumerate().for_each(|(x, row)| {
        for (y, o) in row.iter_mut().enumerate() {
            *o = a[y * n + x];
        }
    });
    out
}

pub(crate) fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32).collect()
}

/// Tile origins along one axis: overlapping by half, at most `max` of them.
fn origins(start: usize, len: usize, n: usize, max: usize) -> Vec<usize> {
    if len <= n {
        return vec![start];
    }
    let span = len - n;
    let count = (span / (n / 2) + 1).clamp(1, max.max(1));
    if count == 1 {
        return vec![start + span / 2];
    }
    (0..count).map(|k| start + k * span / (count - 1)).collect()
}

pub struct Spectrum {
    pub n: usize,
    pub tiles: usize,
    /// log10 of the mean power, centred (zero frequency at n/2, n/2).
    pub log: Vec<f32>,
    /// log10 of the power averaged over rings of radius 0..n/2.
    pub radial: Vec<f32>,
}

/// Tile size and tile origins along X and Y for `region` = [x0, y0, w, h].
pub(crate) fn tiling(region: [usize; 4], want_n: usize) -> Result<(usize, Vec<usize>, Vec<usize>)> {
    let [x0, y0, w, h] = region;
    let side = w.min(h);
    if side < MIN_N {
        bail!("区域太小（至少 {MIN_N} × {MIN_N} 像素）");
    }
    let n = want_n.clamp(MIN_N, MAX_N).min(1 << side.ilog2());
    // Keep the work about that of 64 tiles of 512: fewer tiles when they are larger.
    let per_axis = (((MAX_TILES * 512 * 512) / (n * n)) as f64).sqrt().max(2.0) as usize;
    Ok((n, origins(x0, w, n, per_axis), origins(y0, h, n, per_axis)))
}

/// Power spectrum of `values` (w × h, row major) inside `region`.
pub fn spectrum(values: impl Fn(usize, usize) -> f32 + Sync, region: [usize; 4], want_n: usize) -> Result<Spectrum> {
    let (n, xs, ys) = tiling(region, want_n)?;
    let fft = Fft::new(n);
    let win = hann(n);
    let mut sum = vec![0f64; n * n];
    for &ty in &ys {
        for &tx in &xs {
            let mut tile: Vec<f32> = vec![0f32; n * n];
            tile.par_chunks_mut(n).enumerate().for_each(|(y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    *o = values(tx + x, ty + y);
                }
            });
            let mean = tile.iter().map(|&v| v as f64).sum::<f64>() / (n * n) as f64;
            tile.par_chunks_mut(n).enumerate().for_each(|(y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    *o = ((*o as f64 - mean) as f32) * win[x] * win[y];
                }
            });
            let p = power_2d(&fft, &tile);
            for (s, v) in sum.iter_mut().zip(p) {
                *s += v as f64;
            }
        }
    }
    Ok(finish(&sum, n, xs.len() * ys.len()))
}

/// Centres the summed (unshifted) power of `tiles` tiles, takes log10 and
/// the ring average.
pub(crate) fn finish(sum: &[f64], n: usize, tiles: usize) -> Spectrum {
    let half = n / 2;
    let mut log = vec![0f32; n * n];
    let mut ring = vec![(0f64, 0usize); half + 1];
    for v in 0..n {
        for u in 0..n {
            let p = sum[v * n + u] / tiles as f64;
            let (cu, cv) = ((u + half) % n, (v + half) % n);
            log[cv * n + cu] = (p + 1e-12).log10() as f32;
            let (fu, fv) = (cu as f64 - half as f64, cv as f64 - half as f64);
            let r = (fu * fu + fv * fv).sqrt().round() as usize;
            if r <= half {
                ring[r].0 += p;
                ring[r].1 += 1;
            }
        }
    }
    let radial = ring.iter().map(|&(s, c)| (s / c.max(1) as f64 + 1e-12).log10() as f32).collect();
    Spectrum { n, tiles, log, radial }
}

/// Reply of the `power_spectrum` command: `u32 n, u32 tiles`, then n × n
/// f32 (log10 power, centred), then n/2 + 1 f32 (radial average).
pub fn power_spectrum(img: &Arc<OpenImage>, c: usize, z: usize, t: usize, region: [f64; 4], n: usize) -> Result<Vec<u8>> {
    let info = img.info();
    let (page, sample) = info.locate(c, z, t);
    let planes = img.page(page)?;
    let p = &planes[sample.min(planes.len() - 1)];
    let x0 = region[0].max(0.0).floor() as usize;
    let y0 = region[1].max(0.0).floor() as usize;
    let x1 = ((region[0] + region[2]).ceil() as usize).min(p.w);
    let y1 = ((region[1] + region[3]).ceil() as usize).min(p.h);
    if x1 <= x0 || y1 <= y0 {
        bail!("区域在图像外");
    }
    let s = spectrum(|x, y| p.data.get_f64(y * p.w + x) as f32, [x0, y0, x1 - x0, y1 - y0], n)?;
    let mut out = Vec::with_capacity(8 + (s.log.len() + s.radial.len()) * 4);
    out.extend_from_slice(&(s.n as u32).to_le_bytes());
    out.extend_from_slice(&(s.tiles as u32).to_le_bytes());
    for v in s.log.iter().chain(&s.radial) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plain O(n²) DFT to check the FFT against.
    fn dft(re: &[f32], im: &[f32]) -> (Vec<f64>, Vec<f64>) {
        let n = re.len();
        (0..n)
            .map(|k| {
                (0..n).fold((0.0, 0.0), |(sr, si), j| {
                    let a = -2.0 * PI * (k * j) as f64 / n as f64;
                    let (c, s) = (a.cos(), a.sin());
                    (sr + re[j] as f64 * c - im[j] as f64 * s, si + re[j] as f64 * s + im[j] as f64 * c)
                })
            })
            .unzip()
    }

    #[test]
    fn fft_matches_dft() {
        for n in [2, 4, 8, 64, 256] {
            let re: Vec<f32> = (0..n).map(|i| ((i * 37 % 11) as f32 - 5.0) * 0.3).collect();
            let im: Vec<f32> = (0..n).map(|i| ((i * 13 % 7) as f32 - 3.0) * 0.2).collect();
            let (wr, wi) = dft(&re, &im);
            let (mut r, mut i) = (re.clone(), im.clone());
            Fft::new(n).run(&mut r, &mut i);
            for k in 0..n {
                assert!((r[k] as f64 - wr[k]).abs() < 1e-3 * n as f64, "n {n} re[{k}]");
                assert!((i[k] as f64 - wi[k]).abs() < 1e-3 * n as f64, "n {n} im[{k}]");
            }
        }
    }

    fn peak(s: &Spectrum) -> (usize, usize) {
        let half = s.n / 2;
        let (mut best, mut at) = (f32::MIN, (0, 0));
        for v in 0..s.n {
            for u in 0..s.n {
                if (u, v) == (half, half) {
                    continue;
                }
                if s.log[v * s.n + u] > best {
                    best = s.log[v * s.n + u];
                    at = (u, v);
                }
            }
        }
        at
    }

    #[test]
    fn lattice_peak_lands_at_its_frequency() {
        // Stripes along X with a period of 8 px: peaks at ±n/8 on the u axis.
        let f = |x: usize, _y: usize| (2.0 * PI * x as f64 / 8.0).cos() as f32 * 100.0 + 500.0;
        let s = spectrum(f, [0, 0, 128, 128], 128).unwrap();
        assert_eq!((s.n, s.tiles), (128, 1));
        let (u, v) = peak(&s);
        assert_eq!(v, 64);
        assert_eq!((u as i64 - 64).abs(), 16);
        // The radial average peaks at the same radius.
        let r = (1..s.radial.len()).max_by(|&a, &b| s.radial[a].total_cmp(&s.radial[b])).unwrap();
        assert_eq!(r, 16);
    }

    #[test]
    fn large_regions_are_averaged_over_tiles() {
        let f = |x: usize, y: usize| (2.0 * PI * (x + y) as f64 / 16.0).sin() as f32;
        let s = spectrum(f, [3, 5, 1000, 700], 256).unwrap();
        assert_eq!(s.n, 256);
        assert!(s.tiles > 4 && s.tiles <= MAX_TILES, "{} tiles", s.tiles);
        let (u, v) = peak(&s);
        assert_eq!(((u as i64 - 128).abs(), (v as i64 - 128).abs()), (16, 16));
        // A small region shrinks the tile to fit.
        assert_eq!(spectrum(f, [0, 0, 100, 300], 512).unwrap().n, 64);
        assert!(spectrum(f, [0, 0, 20, 300], 512).is_err());
    }
}
