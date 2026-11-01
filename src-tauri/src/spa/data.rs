//! Particles and their stack: per-particle metadata (micrograph, position,
//! CTF, orientation), the particle stack as a float32 MRC file read through
//! a memory map, the CTF of a particle, and the whitened, CTF-weighted
//! Fourier coefficients that the 2D and 3D searches score.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::fourier::{self, Half};
use crate::fft::Fft;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Optics {
    pub kv: f64,
    pub cs_mm: f64,
    pub amplitude: f64,
    /// Pixel size of the micrographs, Å.
    pub pixel_a: f64,
}

impl Default for Optics {
    fn default() -> Self {
        Optics { kv: 300.0, cs_mm: 2.7, amplitude: 0.1, pixel_a: 1.0 }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Particle {
    pub mic: u32,
    /// Centre in micrograph pixels.
    pub x: f32,
    pub y: f32,
    /// Defocus (Å) along and across `angle` (degrees), as `ctf::Fit` gives them.
    pub df1: f32,
    pub df2: f32,
    pub angle: f32,
    /// Picking score.
    pub fom: f32,
    /// 2D class and in-plane angle (degrees), −1 before classification.
    #[serde(default = "minus_one")]
    pub class2d: i32,
    #[serde(default)]
    pub psi2d: f32,
    /// Orientation (rot, tilt, psi, degrees) and shift (stack pixels) of
    /// the last 3D refinement; half set 1 or 2.
    #[serde(default)]
    pub euler: [f32; 3],
    #[serde(default)]
    pub shift: [f32; 2],
    #[serde(default)]
    pub shift2d: [f32; 2],
    #[serde(default)]
    pub half: u8,
    #[serde(default)]
    pub score: f32,
}

fn minus_one() -> i32 {
    -1
}

/// CTF at frequency index k (cycles per box) of a b-pixel box at `px` Å.
pub struct CtfEval {
    a: f64,
    bb: f64,
    w: f64,
    mean: f64,
    half: f64,
    angle: f64,
    scale: f64,
}

impl CtfEval {
    pub fn new(o: &Optics, p: &Particle, b: usize, px: f64) -> CtfEval {
        let op = crate::ctf::Optics::of(o.kv, o.cs_mm, o.amplitude);
        CtfEval {
            a: op.a,
            bb: op.b,
            w: op.w,
            mean: 0.5 * (p.df1 + p.df2) as f64,
            half: 0.5 * (p.df1 - p.df2) as f64,
            angle: (p.angle as f64).to_radians(),
            scale: 1.0 / (b as f64 * px),
        }
    }

    /// sin γ, positive at low frequency (particles with protein bright).
    #[inline]
    pub fn at(&self, kx: f32, ky: f32) -> f32 {
        let (kx, ky) = (kx as f64 * self.scale, ky as f64 * self.scale);
        let k2 = kx * kx + ky * ky;
        // Same angle convention as the CTF fit: y up in the spectrum.
        let t = (-ky).atan2(kx);
        let df = self.mean + self.half * (2.0 * (t - self.angle)).cos();
        (self.a * df * k2 - self.bb * k2 * k2 + self.w).sin() as f32
    }
}

/// A particle stack: b × b × n float32 MRC, memory-mapped.
pub struct Stack {
    map: Mmap,
    offset: usize,
    pub b: usize,
    pub n: usize,
    pub pixel_a: f64,
}

impl Stack {
    pub fn open(path: &str) -> Result<Stack> {
        let f = File::open(path).with_context(|| format!("打不开颗粒叠层 {path}"))?;
        let map = unsafe { Mmap::map(&f)? };
        if map.len() < 1024 {
            bail!("颗粒叠层太短");
        }
        let i = |o: usize| i32::from_le_bytes(map[o..o + 4].try_into().unwrap());
        let f32_at = |o: usize| f32::from_le_bytes(map[o..o + 4].try_into().unwrap());
        let (w, h, n, mode, ext) = (i(0) as usize, i(4) as usize, i(8) as usize, i(12), i(92).max(0) as usize);
        if w != h || mode != 2 || w == 0 {
            bail!("颗粒叠层应为方形 float32 MRC");
        }
        let offset = 1024 + ext;
        if map.len() < offset + w * h * n * 4 {
            bail!("颗粒叠层不完整");
        }
        let mx = i(28).max(1) as f64;
        let pixel_a = f32_at(40) as f64 / mx;
        Ok(Stack { map, offset, b: w, n, pixel_a: if pixel_a > 0.0 { pixel_a } else { 1.0 } })
    }

    pub fn image(&self, i: usize) -> Vec<f32> {
        let s = self.offset + i * self.b * self.b * 4;
        self.map[s..s + self.b * self.b * 4].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
    }
}

/// Writes a b × b float32 stack one image at a time; the header (with the
/// count and statistics) is written at the end.
pub struct StackWriter {
    f: BufWriter<File>,
    b: usize,
    n: usize,
    px: f64,
    stats: [f64; 4],
    label: String,
}

impl StackWriter {
    pub fn create(path: &Path, b: usize, px: f64, label: &str) -> Result<StackWriter> {
        let mut f = BufWriter::new(File::create(path)?);
        f.write_all(&[0u8; 1024])?;
        Ok(StackWriter { f, b, n: 0, px, stats: [f64::INFINITY, f64::NEG_INFINITY, 0.0, 0.0], label: label.into() })
    }
    pub fn push(&mut self, img: &[f32]) -> Result<()> {
        if img.len() != self.b * self.b {
            bail!("颗粒尺寸不符");
        }
        for &v in img {
            let v = v as f64;
            self.stats[0] = self.stats[0].min(v);
            self.stats[1] = self.stats[1].max(v);
            self.stats[2] += v;
            self.stats[3] += v * v;
        }
        let bytes: Vec<u8> = img.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.f.write_all(&bytes)?;
        self.n += 1;
        Ok(())
    }
    pub fn finish(mut self) -> Result<usize> {
        let count = (self.b * self.b * self.n.max(1)) as f64;
        let mean = self.stats[2] / count;
        let rms = (self.stats[3] / count - mean * mean).max(0.0).sqrt();
        let lo = if self.n > 0 { self.stats[0] } else { 0.0 };
        let hi = if self.n > 0 { self.stats[1] } else { 0.0 };
        self.f.seek(SeekFrom::Start(0))?;
        self.f.write_all(&crate::align::mrc_header(self.b, self.b, self.n, self.px, [lo as f32, hi as f32, mean as f32, rms as f32], &self.label))?;
        self.f.flush()?;
        Ok(self.n)
    }
}

/// One float32 volume or image as MRC.
pub fn write_mrc(path: &Path, data: &[f32], [w, h, d]: [usize; 3], px: f64, label: &str) -> Result<()> {
    let mut f = BufWriter::new(File::create(path).with_context(|| format!("无法写入 {}", path.display()))?);
    let n = data.len().max(1) as f64;
    let mean = data.iter().map(|&v| v as f64).sum::<f64>() / n;
    let rms = (data.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / n).sqrt();
    let lo = data.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = data.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    f.write_all(&crate::align::mrc_header(w, h, d, px, [lo, hi, mean as f32, rms as f32], label))?;
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    f.write_all(&bytes)?;
    f.flush()?;
    Ok(())
}

/// Reads a float32 (or other) MRC volume through the app's readers: (data, side, pixel Å).
pub fn read_volume(path: &str) -> Result<(Vec<f32>, [usize; 3], f64)> {
    let img = crate::image::OpenImage::new(0, crate::formats::open(Path::new(path))?, 256 << 20);
    let info = img.info().clone();
    let (w, h, d) = (info.width as usize, info.height as usize, info.size_z);
    let mut out = Vec::with_capacity(w * h * d);
    for z in 0..d {
        let p = img.page(info.locate(0, z, 0).0)?;
        out.extend((0..w * h).map(|i| p[0].data.get_f64(i) as f32));
    }
    Ok((out, [w, h, d], crate::align::pixel_a(&info)))
}

/// Noise power per shell (index = radius in FFT pixels of the stack box),
/// estimated from up to `max` particles; used to whiten the scores.
pub fn noise_spectrum(stack: &Stack, ids: &[usize], max: usize) -> Vec<f32> {
    let b = stack.b;
    let step = (ids.len() / max.max(1)).max(1);
    let sample: Vec<usize> = ids.iter().step_by(step).copied().collect();
    let fft = Fft::new(b);
    let shells = b / 2 + 1;
    let sums = sample
        .par_iter()
        .map(|&i| {
            let (re, im) = fourier::ft2(&fft, b, &stack.image(i));
            let mut s = vec![0f64; shells * 2];
            for y in 0..b {
                for x in 0..b {
                    let (fx, fy) = (fourier::freq(x, b), fourier::freq(y, b));
                    let r = ((fx * fx + fy * fy) as f64).sqrt().round() as usize;
                    if r < shells {
                        let j = y * b + x;
                        s[r] += (re[j] * re[j] + im[j] * im[j]) as f64;
                        s[shells + r] += 1.0;
                    }
                }
            }
            s
        })
        .reduce(|| vec![0f64; shells * 2], |a, b| a.iter().zip(&b).map(|(x, y)| x + y).collect());
    let mut out: Vec<f32> = (0..shells).map(|r| (sums[r] / sums[shells + r].max(1.0)) as f32).collect();
    let floor = out.iter().skip(1).copied().fold(f32::INFINITY, f32::min).max(1e-12);
    for v in &mut out {
        *v = v.max(floor);
    }
    out
}

/// Fourier coefficients of particles for the scores: for each particle,
/// X·C·w (whitened and multiplied by its CTF, shifted back by its current
/// shift) on the half plane, interleaved re/im, plus C²·w for the norms.
pub struct Coeffs {
    pub k: usize,
    /// n × 2k
    pub x: Vec<f32>,
    /// n × k
    pub c2: Vec<f32>,
}

/// Raw centred transforms of particles at the stack box.
pub fn transforms(stack: &Stack, ids: &[usize]) -> Vec<(Vec<f32>, Vec<f32>)> {
    let fft = Fft::new(stack.b);
    ids.par_iter().map(|&i| fourier::ft2(&fft, stack.b, &stack.image(i))).collect()
}

/// The coefficients on `half` (a box of `half.b` pixels, Fourier-cropped
/// from the stack box `b`), with `shift` (stack pixels) undone.
pub fn coeffs(o: &Optics, px: f64, b: usize, half: &Half, noise: &[f32], parts: &[&Particle], fts: &[(Vec<f32>, Vec<f32>)], shifts: &[[f32; 2]], extra: &[[f32; 2]]) -> Coeffs {
    let k = half.len();
    let bw = half.b;
    let s = extra.len().max(1);
    let scale = (bw * bw) as f32 / (b * b) as f32;
    let ratio = (b / bw).max(1) as f32;
    let mut x = vec![0f32; parts.len() * s * 2 * k];
    let mut c2 = vec![0f32; parts.len() * k];
    x.par_chunks_mut(s * 2 * k).zip(c2.par_chunks_mut(k)).enumerate().for_each(|(n, (xr, cr))| {
        let ctf = CtfEval::new(o, parts[n], b, px);
        let (re, im) = &fts[n];
        for j in 0..k {
            let [kx, ky] = half.k[j];
            let i = fourier::index(ky as i32, b) * b + fourier::index(kx as i32, b);
            let c = ctf.at(kx, ky);
            let w = 1.0 / noise[((kx * kx + ky * ky).sqrt().round() as usize).min(noise.len() - 1)];
            let (xr0, xi0) = (re[i] * scale * c * w, im[i] * scale * c * w);
            cr[j] = c * c * w;
            for t in 0..s {
                let e = extra.get(t).copied().unwrap_or([0.0, 0.0]);
                // Undo the shift: multiply by exp(+2πi k·s / b).
                let sx = shifts[n][0] + e[0] * ratio;
                let sy = shifts[n][1] + e[1] * ratio;
                let ph = 2.0 * std::f32::consts::PI * (kx * sx + ky * sy) / b as f32;
                let (c, si) = (ph.cos(), ph.sin());
                xr[t * 2 * k + 2 * j] = xr0 * c - xi0 * si;
                xr[t * 2 * k + 2 * j + 1] = xr0 * si + xi0 * c;
            }
        }
    });
    Coeffs { k, x, c2 }
}

/// Offsets (in working-box pixels) within ±`range` on a square grid.
pub fn offsets(range: i32) -> Vec<[f32; 2]> {
    let mut v = vec![];
    for dy in -range..=range {
        for dx in -range..=range {
            if dx * dx + dy * dy <= range * range + range {
                v.push([dx as f32, dy as f32]);
            }
        }
    }
    v
}

/// Mean 0 and standard deviation 1 outside a circle of `radius` pixels.
pub fn normalise(img: &mut [f32], b: usize, radius: f64) {
    let c = b as f64 / 2.0;
    let (mut s, mut s2, mut n) = (0f64, 0f64, 0f64);
    for y in 0..b {
        for x in 0..b {
            let d = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2)).sqrt();
            if d > radius {
                let v = img[y * b + x] as f64;
                s += v;
                s2 += v * v;
                n += 1.0;
            }
        }
    }
    if n < 10.0 {
        return;
    }
    let mean = s / n;
    let sd = (s2 / n - mean * mean).max(1e-12).sqrt();
    for v in img.iter_mut() {
        *v = ((*v as f64 - mean) / sd) as f32;
    }
}

/// Soft circular (2D) or spherical (3D) mask: 1 inside `r`, cosine edge of `edge` pixels.
#[inline]
pub fn soft(d: f64, r: f64, edge: f64) -> f32 {
    if d <= r {
        1.0
    } else if d >= r + edge {
        0.0
    } else {
        (0.5 + 0.5 * (std::f64::consts::PI * (d - r) / edge).cos()) as f32
    }
}
