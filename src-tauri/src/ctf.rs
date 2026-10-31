//! CTF fitting (Pro): defocus, astigmatism, phase shift and the resolution
//! up to which the Thon rings agree with the fit, in the manner of CTFFIND4
//! (Rohou & Grigorieff 2015). Written here from the published method.
//!
//! 1. The Welch-averaged power spectrum of the region (`fft::spectrum`) is
//!    turned into an amplitude spectrum; a box-filtered copy is subtracted as
//!    the background and each ring is scaled to unit RMS, so every spatial
//!    frequency in the fitting band counts the same.
//! 2. The score of a CTF is the correlation between that spectrum and
//!    CTF² over the band, using half of the plane (Friedel symmetry).
//! 3. Search: the mean defocus (and phase shift) on the 1D ring average,
//!    then astigmatism on a grid, then a Nelder–Mead refinement of all
//!    parameters. The background is redone once the ring spacing is known.
//! 4. Fit resolution: the spectrum is averaged along lines of equal CTF phase
//!    (so astigmatic rings line up) and correlated with the fitted CTF² in a
//!    window of one ring; the fit holds up to where that correlation first
//!    drops below `FIT_CC`.
//!
//! Defocus is positive for underfocus. Angles are degrees counterclockwise
//! from the image X axis as seen on screen.

use std::f64::consts::PI;
use std::sync::Arc;

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::fft;
use crate::image::OpenImage;

#[cfg(feature = "pro")]
type Dev = burn::tensor::Device;
#[cfg(not(feature = "pro"))]
type Dev = ();

/// Window correlation below which the Thon rings no longer follow the fit.
const FIT_CC: f64 = 0.5;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub voltage_kv: f64,
    pub cs_mm: f64,
    pub amp_contrast: f64,
    /// Pixel size in Å.
    pub pixel_a: f64,
    /// Fitting band: low- and high-resolution limits in Å.
    pub res_low_a: f64,
    pub res_high_a: f64,
    /// Defocus search range in Å.
    pub df_min_a: f64,
    pub df_max_a: f64,
    /// Largest astigmatism (df1 − df2) searched, in Å.
    pub max_astig_a: f64,
    /// Also fit a phase shift (phase plate).
    pub phase: bool,
    /// "cpu": all on the CPU; "gpu": all on the GPU; otherwise (auto) the
    /// spectrum on the GPU when there is one (Pro) and the fit on the CPU.
    #[serde(default)]
    pub backend: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Fit {
    /// Defocus along the major and minor axes (df1 ≥ df2), Å.
    pub df1: f64,
    pub df2: f64,
    /// Direction of df1, degrees in [0, 180).
    pub angle: f64,
    /// Phase shift, degrees in [0, 180).
    pub phase: f64,
    /// Correlation with CTF² over the fitting band.
    pub score: f64,
    /// Resolution (Å) up to which the Thon rings follow the fit.
    pub fit_res: f64,
    /// FFT size the fit was made on.
    pub n: usize,
    /// 1D curves over radius 0..=n/2 (FFT pixels): the spectrum averaged
    /// along equal CTF phase, the fitted CTF² (centred and scaled to it),
    /// and the window correlation; 0 where undefined.
    pub epa: Vec<f32>,
    pub model: Vec<f32>,
    pub cc: Vec<f32>,
    /// Fitting band in FFT pixels.
    pub band: [f64; 2],
    /// CTF zeros as closed polylines of (du, dv) offsets in FFT pixels from
    /// the centre (dv down, like the spectrum image), innermost first.
    pub zeros: Vec<Vec<[f32; 2]>>,
    /// Where it ran ("CPU", "Metal", …) and how long it took (spectrum and fit), ms.
    pub backend: String,
    pub ms: f64,
}

/// Electron wavelength in Å at `kv` kilovolts (relativistic).
pub fn wavelength(kv: f64) -> f64 {
    let v = kv * 1e3;
    12.264_259 / (v * (1.0 + 0.978_476e-6 * v)).sqrt()
}

/// The parts of the CTF phase γ(k, θ) = A·Δf(θ)·k² − B·k⁴ + C.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Optics {
    /// π λ
    pub a: f64,
    /// π/2 · Cs · λ³ (Cs in Å)
    pub b: f64,
    /// Amplitude contrast as a phase.
    pub w: f64,
}

impl Optics {
    fn new(p: &Params) -> Optics {
        Optics::of(p.voltage_kv, p.cs_mm, p.amp_contrast)
    }

    pub(crate) fn of(kv: f64, cs_mm: f64, amp: f64) -> Optics {
        let l = wavelength(kv);
        let amp = amp.clamp(0.0, 0.999);
        Optics { a: PI * l, b: 0.5 * PI * cs_mm * 1e7 * l * l * l, w: (amp / (1.0 - amp * amp).sqrt()).atan() }
    }

    #[cfg(feature = "pro")]
    /// Sign of sin γ at spatial frequency (kx, ky) in 1/Å for defocus df1
    /// along `angle` (radians) and df2 across it: what phase flipping
    /// multiplies by (+1 at low frequencies, where γ ≈ the amplitude phase).
    pub(crate) fn flip(&self, df1: f64, df2: f64, angle: f64, kx: f64, ky: f64) -> f32 {
        let k2 = kx * kx + ky * ky;
        let t = ky.atan2(kx) - angle;
        let df = 0.5 * (df1 + df2) + 0.5 * (df1 - df2) * (2.0 * t).cos();
        if self.gamma(df, k2, 0.0).sin() >= 0.0 { 1.0 } else { -1.0 }
    }

    fn gamma(&self, df: f64, k2: f64, phase: f64) -> f64 {
        self.a * df * k2 - self.b * k2 * k2 + self.w + phase
    }

    /// Smallest k² ≥ 0 where γ = g at defocus df, if any.
    fn k2_at(&self, df: f64, g: f64) -> Option<f64> {
        let (a, b) = (self.a * df, self.b);
        let g = g - self.w;
        let disc = a * a - 4.0 * b * g;
        if disc < 0.0 || a <= 0.0 {
            return None;
        }
        let q = 2.0 * g / (a + disc.sqrt());
        (q >= 0.0).then_some(q)
    }
}

/// CTF model: mean defocus plus the astigmatic part as (p, q) =
/// half difference × (cos 2α, sin 2α), which has no wrap-around, and phase.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Model {
    pub mean: f64,
    pub p: f64,
    pub q: f64,
    pub phase: f64,
}

impl Model {
    fn df(&self, c2: f64, s2: f64) -> f64 {
        self.mean + self.p * c2 + self.q * s2
    }
    fn half_diff(&self) -> f64 {
        self.p.hypot(self.q)
    }
    fn from_astig(mean: f64, half: f64, angle: f64, phase: f64) -> Model {
        Model { mean, p: half * (2.0 * angle).cos(), q: half * (2.0 * angle).sin(), phase }
    }
}

/// One polar bin of the half plane (1 FFT pixel in radius, `ANGLES` bins
/// over 180°): the mean over its pixels, and how many there are.
#[derive(Clone, Copy)]
pub(crate) struct Px {
    pub k2: f32,
    pub c2: f32,
    pub s2: f32,
    /// Background-subtracted, ring-normalised amplitude.
    pub x: f32,
    /// Radius in FFT pixels.
    pub r: f32,
    /// Pixels in the bin.
    pub w: f32,
}

/// Angular bins over the half plane. Within one 2° bin the astigmatic part
/// of the defocus hardly changes, and the scores cost a fraction of the
/// pixel count (about 6× fewer at n = 1024).
const ANGLES: usize = 90;

struct Prepared {
    /// Pixels inside the fitting band.
    band: Vec<Px>,
    /// Every pixel from the band's inner edge out to Nyquist (for the fit resolution).
    outer: Vec<Px>,
}

/// Amplitude spectrum (centred n × n) → background-free, ring-normalised
/// pixels. `boxw` is the background filter width in pixels (odd).
fn prepare(amp: &[f32], n: usize, pixel_a: f64, band: [f64; 2], boxw: usize) -> Prepared {
    let half = n as i64 / 2;
    let bg = box_filter(amp, n, boxw);
    let rmax = half as usize;
    // Ring RMS of the background-subtracted amplitude, for normalisation.
    let mut ring = vec![(0f64, 0usize); rmax + 2];
    let mut raw = Vec::new();
    for v in 0..n as i64 {
        for u in 0..n as i64 {
            let (du, dv) = (u - half, v - half);
            // Half plane: dv < 0, or dv == 0 with du > 0.
            if dv > 0 || (dv == 0 && du <= 0) {
                continue;
            }
            let r = ((du * du + dv * dv) as f64).sqrt();
            if r < band[0] || r > rmax as f64 {
                continue;
            }
            let i = (v as usize) * n + u as usize;
            let x = (amp[i] - bg[i]) as f64;
            let ri = r.round() as usize;
            ring[ri].0 += x * x;
            ring[ri].1 += 1;
            // θ counterclockwise on screen: y up = −dv.
            let th = (-(dv as f64)).atan2(du as f64);
            let k = r / (n as f64 * pixel_a);
            raw.push((r, ri, k * k, (2.0 * th).cos(), (2.0 * th).sin(), x, th.rem_euclid(PI)));
        }
    }
    // Smooth the ring RMS over a few radii.
    let rms: Vec<f64> = (0..ring.len())
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(3), (i + 3).min(ring.len() - 1));
            let (s, c) = ring[lo..=hi].iter().fold((0.0, 0), |(s, c), &(a, b)| (s + a, c + b));
            if c == 0 {
                1.0
            } else {
                (s / c as f64).sqrt().max(1e-20)
            }
        })
        .collect();
    // Sums per (radius, angle) bin: w, k², cos 2θ, sin 2θ, x, r.
    let mut bins = vec![[0f64; 6]; (rmax + 2) * ANGLES];
    for (r, ri, k2, c2, s2, x, th) in raw {
        // Clip outliers (crystal spots, hot pixels).
        let x = (x / rms[ri]).clamp(-4.0, 4.0);
        let a = ((th / PI * ANGLES as f64) as usize).min(ANGLES - 1);
        let b = &mut bins[ri * ANGLES + a];
        for (o, v) in b.iter_mut().zip([1.0, k2, c2, s2, x, r]) {
            *o += v;
        }
    }
    let mut out = Prepared { band: Vec::new(), outer: Vec::new() };
    for b in bins.iter().filter(|b| b[0] > 0.0) {
        let w = b[0];
        let px = Px { k2: (b[1] / w) as f32, c2: (b[2] / w) as f32, s2: (b[3] / w) as f32, x: (b[4] / w) as f32, r: (b[5] / w) as f32, w: w as f32 };
        if px.r as f64 <= band[1] {
            out.band.push(px);
        }
        out.outer.push(px);
    }
    out
}

/// Mean over a w × w box (w odd), clamped at the edges; via a summed-area table.
fn box_filter(a: &[f32], n: usize, w: usize) -> Vec<f32> {
    let h = (w / 2) as i64;
    let mut sat = vec![0f64; (n + 1) * (n + 1)];
    for y in 0..n {
        let mut row = 0f64;
        for x in 0..n {
            row += a[y * n + x] as f64;
            sat[(y + 1) * (n + 1) + x + 1] = sat[y * (n + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0f32; n * n];
    out.par_chunks_mut(n).enumerate().for_each(|(y, row)| {
        let y0 = (y as i64 - h).max(0) as usize;
        let y1 = (y as i64 + h + 1).min(n as i64) as usize;
        for (x, o) in row.iter_mut().enumerate() {
            let x0 = (x as i64 - h).max(0) as usize;
            let x1 = (x as i64 + h + 1).min(n as i64) as usize;
            let s = sat[y1 * (n + 1) + x1] - sat[y0 * (n + 1) + x1] - sat[y1 * (n + 1) + x0] + sat[y0 * (n + 1) + x0];
            *o = (s / ((y1 - y0) * (x1 - x0)) as f64) as f32;
        }
    });
    out
}

/// Pearson correlation from running sums.
pub(crate) fn corr(n: f64, sx: f64, sm: f64, sxx: f64, smm: f64, sxm: f64) -> f64 {
    let vx = sxx - sx * sx / n;
    let vm = smm - sm * sm / n;
    if vx <= 0.0 || vm <= 0.0 {
        return 0.0;
    }
    (sxm - sx * sm / n) / (vx * vm).sqrt()
}

/// Correlation between the pixels and CTF² of `m`, using every `step`th pixel.
fn score(px: &[Px], o: &Optics, m: &Model, step: usize) -> f64 {
    let s = px
        .par_chunks(4096)
        .map(|chunk| {
            let mut s = [0f64; 6];
            for p in chunk.iter().step_by(step) {
                let df = m.df(p.c2 as f64, p.s2 as f64);
                let g = o.gamma(df, p.k2 as f64, m.phase).sin();
                let (x, v, w) = (p.x as f64, g * g, p.w as f64);
                s[0] += w;
                s[1] += w * x;
                s[2] += w * v;
                s[3] += w * x * x;
                s[4] += w * v * v;
                s[5] += w * x * v;
            }
            s
        })
        .reduce(|| [0f64; 6], |a, b| std::array::from_fn(|i| a[i] + b[i]));
    corr(s[0], s[1], s[2], s[3], s[4], s[5])
}

/// Mean pixel value per integer radius.
fn ring_average(px: &[Px], len: usize) -> Vec<(f64, usize)> {
    let mut ring = vec![(0f64, 0usize); len];
    for p in px {
        let r = (p.r.round() as usize).min(len - 1);
        ring[r].0 += (p.x * p.w) as f64;
        ring[r].1 += p.w as usize;
    }
    ring.iter().map(|&(s, c)| (if c > 0 { s / c as f64 } else { 0.0 }, c)).collect()
}

/// 1D search of mean defocus (and phase) on the ring average: the best few
/// local maxima, best first.
fn search_1d(px: &[Px], o: &Optics, p: &Params, n: usize) -> Vec<Model> {
    let ring = ring_average(px, n / 2 + 2);
    let rows: Vec<(f64, f64)> = ring
        .iter()
        .enumerate()
        .filter(|(_, &(_, c))| c > 0)
        .map(|(r, &(x, _))| {
            let k = r as f64 / (n as f64 * p.pixel_a);
            (k * k, x)
        })
        .collect();
    let phases: Vec<f64> = if p.phase { (0..18).map(|i| i as f64 * PI / 18.0).collect() } else { vec![0.0] };
    // Steps fine enough that a ring at the band's edge moves well under a period.
    let kmax2 = rows.last().map(|r| r.0).unwrap_or(1.0);
    let step = (0.15 / (o.a * kmax2)).max(5.0);
    let count = (((p.df_max_a - p.df_min_a) / step).ceil() as usize + 1).min(20_000);
    let mut grid: Vec<(f64, Model)> = phases
        .par_iter()
        .flat_map_iter(|&ph| {
            let rows = &rows;
            (0..count).map(move |i| {
                let df = p.df_min_a + (p.df_max_a - p.df_min_a) * i as f64 / (count - 1).max(1) as f64;
                let mut s = [0f64; 6];
                for &(k2, x) in rows {
                    let g = o.gamma(df, k2, ph).sin();
                    let v = g * g;
                    s[0] += 1.0;
                    s[1] += x;
                    s[2] += v;
                    s[3] += x * x;
                    s[4] += v * v;
                    s[5] += x * v;
                }
                (corr(s[0], s[1], s[2], s[3], s[4], s[5]), Model { mean: df, p: 0.0, q: 0.0, phase: ph })
            })
        })
        .collect();
    // Local maxima along defocus, for each phase.
    let mut peaks: Vec<(f64, Model)> = Vec::new();
    for row in grid.chunks(count) {
        for i in 0..row.len() {
            let left = i == 0 || row[i - 1].0 <= row[i].0;
            let right = i + 1 == row.len() || row[i + 1].0 < row[i].0;
            if left && right {
                peaks.push(row[i]);
            }
        }
    }
    if peaks.is_empty() {
        peaks.append(&mut grid);
    }
    peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
    peaks.truncate(4);
    peaks.into_iter().map(|(_, m)| m).collect()
}

/// Picks the best grid model for each of several spectra with the same
/// geometry (same n and band, so the same bins): on the CPU, or on the GPU
/// (`ctf_gpu::GpuGrid`), where every spectrum is scored against the
/// candidates of all of them at once.
pub(crate) trait Grid: Sync {
    /// `px[s]`: the bins of spectrum s; `cands[s]`: its candidates.
    fn best(&self, px: &[&[Px]], o: Optics, cands: &[Vec<Model>]) -> Vec<Model>;
}

pub(crate) struct CpuGrid;

impl Grid for CpuGrid {
    fn best(&self, px: &[&[Px]], o: Optics, cands: &[Vec<Model>]) -> Vec<Model> {
        px.par_iter()
            .zip(cands)
            .map(|(px, c)| {
                let step = (px.len() / 30_000).max(1);
                let v: Vec<f64> = c.par_iter().map(|m| score(px, &o, m, step)).collect();
                c[(0..c.len()).max_by(|&a, &b| v[a].total_cmp(&v[b])).unwrap_or(0)]
            })
            .collect()
    }
}

/// Astigmatism grid around each 1D candidate.
fn grid_candidates(p: &Params, starts: &[Model]) -> Vec<Model> {
    let maxh = (p.max_astig_a / 2.0).max(0.0);
    let hs: Vec<f64> = (0..=8).map(|i| maxh * i as f64 / 8.0).collect();
    let mut cands = Vec::new();
    for s in starts {
        for &h in &hs {
            let angles = if h == 0.0 { 1 } else { 12 };
            for ai in 0..angles {
                let ang = ai as f64 * PI / 12.0;
                for dm in [-1.0, 0.0, 1.0] {
                    // Astigmatism spreads the 1D peak, so let the mean move by up to half of it.
                    cands.push(Model::from_astig(s.mean + dm * h * 0.5, h, ang, s.phase));
                }
            }
        }
    }
    cands
}

/// Score minus a penalty for leaving the requested ranges.
fn objective(p: &Params, m: &Model, score: f64) -> f64 {
    let over = (m.mean - p.df_max_a).max(p.df_min_a - m.mean).max(0.0) + (2.0 * m.half_diff() - p.max_astig_a).max(0.0);
    score - over * 1e-3
}

/// Nelder–Mead maximisation of the score (dimensions 3, or 4 with phase).
fn refine(px: &[Px], o: &Optics, p: &Params, start: Model) -> Model {
    let dim = if p.phase { 4 } else { 3 };
    let to = |v: &[f64]| Model { mean: v[0], p: v[1], q: v[2], phase: if dim == 4 { v[3] } else { start.phase } };
    let f = |v: &[f64]| {
        let m = to(v);
        -objective(p, &m, score(px, o, &m, 1))
    };
    let x0 = [start.mean, start.p, start.q, start.phase];
    let scale = [(start.mean * 0.02).max(100.0), 100.0, 100.0, 0.2];
    let mut simplex: Vec<Vec<f64>> = vec![x0[..dim].to_vec()];
    for i in 0..dim {
        let mut v = x0[..dim].to_vec();
        v[i] += scale[i];
        simplex.push(v);
    }
    let mut vals: Vec<f64> = simplex.iter().map(|v| f(v)).collect();
    for _ in 0..400 {
        let mut idx: Vec<usize> = (0..=dim).collect();
        idx.sort_by(|&a, &b| vals[a].total_cmp(&vals[b]));
        simplex = idx.iter().map(|&i| simplex[i].clone()).collect();
        vals = idx.iter().map(|&i| vals[i]).collect();
        if (vals[dim] - vals[0]).abs() < 1e-7 {
            break;
        }
        let cen: Vec<f64> = (0..dim).map(|j| simplex[..dim].iter().map(|v| v[j]).sum::<f64>() / dim as f64).collect();
        let along = |t: f64| -> Vec<f64> { (0..dim).map(|j| cen[j] + t * (simplex[dim][j] - cen[j])).collect() };
        let xr = along(-1.0);
        let fr = f(&xr);
        if fr < vals[0] {
            let xe = along(-2.0);
            let fe = f(&xe);
            if fe < fr {
                simplex[dim] = xe;
                vals[dim] = fe;
            } else {
                simplex[dim] = xr;
                vals[dim] = fr;
            }
        } else if fr < vals[dim - 1] {
            simplex[dim] = xr;
            vals[dim] = fr;
        } else {
            let xc = if fr < vals[dim] { along(-0.5) } else { along(0.5) };
            let fc = f(&xc);
            if fc < vals[dim].min(fr) {
                simplex[dim] = xc;
                vals[dim] = fc;
            } else {
                // Shrink toward the best vertex.
                for i in 1..=dim {
                    simplex[i] = (0..dim).map(|j| simplex[0][j] + 0.5 * (simplex[i][j] - simplex[0][j])).collect();
                    vals[i] = f(&simplex[i]);
                }
            }
        }
    }
    let best = (0..=dim).min_by(|&a, &b| vals[a].total_cmp(&vals[b])).unwrap();
    to(&simplex[best])
}

/// Background box width: about two ring periods at the band's inner edge,
/// within n/32..n/6.
fn box_width(o: &Optics, m: Option<&Model>, n: usize, pixel_a: f64, r_in: f64) -> usize {
    let w = match m {
        Some(m) => {
            let k = r_in / (n as f64 * pixel_a);
            let slope = 2.0 * o.a * m.mean * k - 4.0 * o.b * k * k * k;
            if slope > 0.0 {
                2.0 * PI / slope * n as f64 * pixel_a
            } else {
                n as f64 / 16.0
            }
        }
        None => n as f64 / 16.0,
    };
    let w = (w.clamp(n as f64 / 32.0, n as f64 / 6.0).round() as usize).max(3);
    w | 1
}

/// Fits the CTF to a centred n × n power spectrum (as `fft::Spectrum::log`, log10)
/// on the CPU.
pub fn fit_spectrum(log: &[f32], n: usize, p: &Params) -> Result<Fit> {
    Ok(fit_many(&[log], n, p, &CpuGrid)?.pop().unwrap())
}

/// Checks the parameters for an n × n spectrum; returns the fitting band in FFT pixels.
fn check_params(n: usize, p: &Params) -> Result<[f64; 2]> {
    if !(p.pixel_a > 0.0) || !(p.voltage_kv > 0.0) {
        bail!("需要像素大小和加速电压");
    }
    if p.df_max_a <= p.df_min_a || p.df_min_a < 0.0 {
        bail!("离焦范围不对");
    }
    let nyq = n as f64 / 2.0;
    let to_px = |res: f64| n as f64 * p.pixel_a / res;
    let band = [to_px(p.res_low_a).max(2.0), to_px(p.res_high_a).min(nyq * 0.98)];
    if band[1] - band[0] < 4.0 {
        bail!("拟合范围太窄：低分辨率限要大于高分辨率限，且高分辨率限不能小于 2 倍像素（{:.2} Å）", 2.0 * p.pixel_a);
    }
    Ok(band)
}

/// Fits several spectra of one size (a stack or tilt series), with the
/// grid searches of all of them done together by `grid`.
pub(crate) fn fit_many(logs: &[&[f32]], n: usize, p: &Params, grid: &dyn Grid) -> Result<Vec<Fit>> {
    let band = check_params(n, p)?;
    let o = Optics::new(p);
    let amps: Vec<Vec<f32>> = logs.par_iter().map(|log| log.iter().map(|&l| 10f32.powf(l * 0.5)).collect()).collect();
    let w0 = box_width(&o, None, n, p.pixel_a, band[0]);
    let mut preps: Vec<Prepared> = amps.par_iter().map(|a| prepare(a, n, p.pixel_a, band, w0)).collect();
    let cands: Vec<Vec<Model>> = preps.par_iter().map(|pr| grid_candidates(p, &search_1d(&pr.band, &o, p, n))).collect();
    let mut best = grid.best(&bands(&preps), o, &cands);
    // Redo the background for the ring spacing found, then search again.
    let redo: Vec<Option<Prepared>> = (0..preps.len())
        .into_par_iter()
        .map(|s| {
            let w = box_width(&o, Some(&best[s]), n, p.pixel_a, band[0]);
            (w != w0).then(|| prepare(&amps[s], n, p.pixel_a, band, w))
        })
        .collect();
    if redo.iter().any(Option::is_some) {
        let cands: Vec<Vec<Model>> = redo
            .par_iter()
            .zip(&best)
            .map(|(r, b)| match r {
                Some(pr) => {
                    let mut starts = search_1d(&pr.band, &o, p, n);
                    starts.push(Model { p: 0.0, q: 0.0, ..*b });
                    grid_candidates(p, &starts)
                }
                None => vec![*b],
            })
            .collect();
        for (pr, r) in preps.iter_mut().zip(redo) {
            if let Some(r) = r {
                *pr = r;
            }
        }
        best = grid.best(&bands(&preps), o, &cands);
    }
    Ok(preps
        .par_iter()
        .zip(best)
        .map(|(prep, start)| {
            let m = refine(&prep.band, &o, p, start);
            let sc = score(&prep.band, &o, &m, 1);
            let h = m.half_diff();
            let mut angle = 0.5 * m.q.atan2(m.p);
            if angle < 0.0 {
                angle += PI;
            }
            let (epa, model, cc, fit_r) = fit_resolution(&prep.outer, &o, &m, n, p.pixel_a, band);
            Fit {
                df1: m.mean + h,
                df2: m.mean - h,
                angle: if h < 1e-6 { 0.0 } else { angle.to_degrees() % 180.0 },
                phase: if p.phase { m.phase.rem_euclid(PI).to_degrees() } else { 0.0 },
                score: sc,
                fit_res: n as f64 * p.pixel_a / fit_r,
                n,
                epa,
                model,
                cc,
                band,
                zeros: zeros(&o, &m, n, p.pixel_a),
                backend: "CPU".into(),
                ms: 0.0,
            }
        })
        .collect())
}

fn bands(preps: &[Prepared]) -> Vec<&[Px]> {
    preps.iter().map(|pr| &pr.band[..]).collect()
}

/// Equal-phase average, fitted CTF² on it, window correlations, and the
/// radius (FFT pixels) where the fit stops holding.
fn fit_resolution(px: &[Px], o: &Optics, m: &Model, n: usize, pixel_a: f64, band: [f64; 2]) -> (Vec<f32>, Vec<f32>, Vec<f32>, f64) {
    let len = n / 2 + 1;
    let scale = n as f64 * pixel_a; // radius in px = k × scale
    let mut sum = vec![(0f64, 0f64); len];
    for p in px {
        // The radius at which the mean-defocus CTF has this pixel's phase.
        let df = m.df(p.c2 as f64, p.s2 as f64);
        let g = o.gamma(df, p.k2 as f64, m.phase);
        let Some(k2) = o.k2_at(m.mean, g - m.phase) else { continue };
        let r = (k2.sqrt() * scale).round() as usize;
        if r < len {
            sum[r].0 += (p.x * p.w) as f64;
            sum[r].1 += p.w as f64;
        }
    }
    let epa: Vec<f64> = sum.iter().map(|&(s, c)| if c > 0.0 { s / c } else { f64::NAN }).collect();
    let ctf2: Vec<f64> = (0..len)
        .map(|r| {
            let k = r as f64 / scale;
            o.gamma(m.mean, k * k, m.phase).sin().powi(2)
        })
        .collect();
    let mut cc = vec![0f64; len];
    let r0 = band[0].ceil() as usize;
    let mut fit_r = (len - 1) as f64;
    let mut found = false;
    for r in r0..len {
        let k = r as f64 / scale;
        let slope = 2.0 * o.a * m.mean * k - 4.0 * o.b * k * k * k; // dγ/dk
        if slope <= 0.0 {
            if !found {
                fit_r = r as f64;
            }
            break;
        }
        let period = PI / slope * scale;
        let hw = ((period / 2.0).round() as usize).max(2);
        let (lo, hi) = (r.saturating_sub(hw).max(r0), (r + hw).min(len - 1));
        let mut s = [0f64; 6];
        for i in lo..=hi {
            if epa[i].is_nan() {
                continue;
            }
            let (x, v) = (epa[i], ctf2[i]);
            s[0] += 1.0;
            s[1] += x;
            s[2] += v;
            s[3] += x * x;
            s[4] += v * v;
            s[5] += x * v;
        }
        cc[r] = corr(s[0], s[1], s[2], s[3], s[4], s[5]);
        if !found && r as f64 > band[0] + hw as f64 && cc[r] < FIT_CC {
            fit_r = r as f64;
            found = true;
        }
    }
    // Scale CTF² to the spectrum in the band, for plotting.
    let (mut sx, mut sv, mut sxx, mut svv, mut c) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for r in r0..(band[1] as usize).min(len) {
        if !epa[r].is_nan() {
            sx += epa[r];
            sxx += epa[r] * epa[r];
            sv += ctf2[r];
            svv += ctf2[r] * ctf2[r];
            c += 1.0;
        }
    }
    let (mx, mv) = (sx / c.max(1.0), sv / c.max(1.0));
    let sdx = (sxx / c.max(1.0) - mx * mx).max(0.0).sqrt();
    let sdv = (svv / c.max(1.0) - mv * mv).max(1e-12).sqrt();
    let model = ctf2.iter().map(|&v| (mx + (v - mv) * sdx / sdv) as f32).collect();
    let epa = epa.iter().map(|&v| if v.is_nan() { 0.0 } else { v as f32 }).collect();
    let cc = cc.iter().map(|&v| v as f32).collect();
    (epa, model, cc, fit_r)
}

/// CTF zeros (γ = jπ) out to Nyquist as polylines of 180 points.
fn zeros(o: &Optics, m: &Model, n: usize, pixel_a: f64) -> Vec<Vec<[f32; 2]>> {
    let scale = n as f64 * pixel_a;
    let nyq = n as f64 / 2.0;
    let mut out = Vec::new();
    for j in 1..400 {
        let g = j as f64 * PI - m.phase;
        let mut ring = Vec::with_capacity(181);
        let mut inside = false;
        for i in 0..=180 {
            let th = i as f64 * 2.0 * PI / 180.0;
            let df = m.df((2.0 * th).cos(), (2.0 * th).sin());
            let Some(k2) = o.k2_at(df, g) else { continue };
            let r = k2.sqrt() * scale;
            inside |= r <= nyq;
            ring.push([(r * th.cos()) as f32, (-r * th.sin()) as f32]);
        }
        if !inside || ring.len() < 181 {
            if !inside {
                break;
            }
            continue;
        }
        out.push(ring);
    }
    out
}

/// Fit on a region of one slice; the spectrum is computed like the FFT panel's.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitReq {
    pub token: u64,
    pub z: usize,
    pub t: usize,
    pub c: usize,
    pub region: [f64; 4],
    pub n: usize,
    pub params: Params,
}

/// Fit on the same region of many slices: a tilt series or a stack of
/// micrographs. `zs` empty means every slice.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackReq {
    pub token: u64,
    #[serde(default)]
    pub zs: Vec<usize>,
    pub t: usize,
    pub c: usize,
    pub region: [f64; 4],
    pub n: usize,
    pub params: Params,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackFit {
    pub zs: Vec<usize>,
    /// Tilt angle per slice (degrees), from an IMOD .rawtlt/.tlt beside the file.
    pub tilts: Option<Vec<f64>>,
    /// Per slice, without the per-radius curves and zeros (ask `ctf_fit` for one).
    pub fits: Vec<Fit>,
    pub backend: String,
    pub ms: f64,
}

/// Set to stop a running stack fit; cleared when one starts.
pub static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn fit_region(img: &Arc<OpenImage>, req: &FitReq) -> Result<Fit> {
    let (mut fits, backend, ms) = fit_slices(img, &[req.z], req.c, req.t, req.region, req.n, &req.params, &|_, _| ())?;
    let mut f = fits.pop().unwrap();
    f.backend = backend;
    f.ms = ms;
    Ok(f)
}

pub fn fit_stack(img: &Arc<OpenImage>, req: &StackReq, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<StackFit> {
    let nz = img.info().size_z;
    let zs: Vec<usize> = if req.zs.is_empty() { (0..nz).collect() } else { req.zs.iter().copied().filter(|&z| z < nz).collect() };
    if zs.is_empty() {
        bail!("没有可拟合的层");
    }
    CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
    let (mut fits, backend, ms) = fit_slices(img, &zs, req.c, req.t, req.region, req.n, &req.params, progress)?;
    for f in &mut fits {
        f.epa = Vec::new();
        f.model = Vec::new();
        f.cc = Vec::new();
        f.zeros = Vec::new();
    }
    let tilts = tilt_angles(&img.info().path, nz).map(|t| zs.iter().map(|&z| t[z]).collect());
    Ok(StackFit { zs, tilts, fits, backend, ms })
}

/// IMOD's tilt angle file next to a tilt series (`x.st` → `x.rawtlt` or
/// `x.tlt`), one angle per line, when it has one per slice.
pub fn tilt_angles(path: &str, nz: usize) -> Option<Vec<f64>> {
    let p = std::path::Path::new(path);
    let read = |f: &std::path::Path| -> Option<Vec<f64>> {
        let text = std::fs::read_to_string(f).ok()?;
        let t: Vec<f64> = text.split_whitespace().map(|v| v.parse().ok()).collect::<Option<_>>()?;
        (t.len() == nz).then_some(t)
    };
    if let Some(t) = ["rawtlt", "tlt"].iter().find_map(|ext| read(&p.with_extension(ext))) {
        return Some(t);
    }
    // A binned or aligned copy (TS21_bin2.mrc) next to the series' own
    // angle file (TS21.tlt): the longest name the stack's name starts with.
    let stem = p.file_stem()?.to_str()?;
    let mut found: Vec<(usize, std::path::PathBuf)> = std::fs::read_dir(p.parent()?)
        .ok()?
        .filter_map(|e| {
            let f = e.ok()?.path();
            let ext = f.extension()?.to_str()?;
            let s = f.file_stem()?.to_str()?;
            let len = s.len();
            (matches!(ext, "rawtlt" | "tlt") && len > 0 && stem.starts_with(s)).then_some(len).map(|l| (l, f.clone()))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.iter().find_map(|(_, f)| read(f))
}

#[cfg(feature = "pro")]
/// Fits per slice, each on its own region (slices sharing a region share
/// one batch): what the tilt-series CTF uses, with strips along the axis.
pub(crate) fn fit_regions(
    img: &Arc<OpenImage>,
    items: &[(usize, [f64; 4])],
    c: usize,
    t: usize,
    n: usize,
    params: &Params,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Vec<Fit>> {
    let mut out: Vec<Option<Fit>> = vec![None; items.len()];
    let mut regions: Vec<[f64; 4]> = items.iter().map(|i| i.1).collect();
    regions.sort_by(|a, b| a.partial_cmp(b).unwrap());
    regions.dedup();
    let done = std::sync::atomic::AtomicUsize::new(0);
    for r in regions {
        let idx: Vec<usize> = (0..items.len()).filter(|&i| items[i].1 == r).collect();
        let zs: Vec<usize> = idx.iter().map(|&i| items[i].0).collect();
        let before = done.load(std::sync::atomic::Ordering::Relaxed);
        let (fits, _, _) = fit_slices(img, &zs, c, t, r, n, params, &|k, _| progress((before + k).min(items.len()), items.len()))?;
        done.fetch_add(zs.len(), std::sync::atomic::Ordering::Relaxed);
        for (i, f) in idx.into_iter().zip(fits) {
            out[i] = Some(f);
        }
    }
    Ok(out.into_iter().map(Option::unwrap).collect())
}

/// Spectra of a region on each slice, then `fit_many`. The spectra run on
/// the GPU when there is one; the grid search too for "gpu". Returns the
/// fits, the backend's name and the time taken.
#[allow(clippy::too_many_arguments)]
fn fit_slices(
    img: &Arc<OpenImage>,
    zs: &[usize],
    c: usize,
    t: usize,
    r: [f64; 4],
    want_n: usize,
    params: &Params,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<(Vec<Fit>, String, f64)> {
    let start = std::time::Instant::now();
    let info = img.info();
    let x0 = r[0].max(0.0).floor() as usize;
    let y0 = r[1].max(0.0).floor() as usize;
    let x1 = ((r[0] + r[2]).ceil() as usize).min(info.width);
    let y1 = ((r[1] + r[3]).ceil() as usize).min(info.height);
    if x1 <= x0 || y1 <= y0 {
        bail!("区域在图像外");
    }
    let region = [x0, y0, x1 - x0, y1 - y0];
    // Bad parameters fail here, before any GPU work.
    let (n, _, _) = fft::tiling(region, want_n)?;
    check_params(n, params)?;
    let total = zs.len() + 1;
    let spectra = |gpu: Option<&Dev>| -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(zs.len());
        for (i, &z) in zs.iter().enumerate() {
            if CANCEL.load(std::sync::atomic::Ordering::SeqCst) {
                bail!("已取消");
            }
            let (page, sample) = info.locate(c, z, t);
            let planes = img.page(page)?;
            let p = &planes[sample.min(planes.len() - 1)];
            let values = |x: usize, y: usize| p.data.get_f64(y * p.w + x) as f32;
            out.push(match gpu {
                #[cfg(feature = "pro")]
                Some(d) => crate::ctf_gpu::spectrum(d, values, region, want_n)?.log,
                _ => fft::spectrum(values, region, want_n)?.log,
            });
            progress(i + 1, total);
        }
        Ok(out)
    };
    let done = |logs: Vec<Vec<f32>>, grid: &dyn Grid| -> Result<Vec<Fit>> {
        let refs: Vec<&[f32]> = logs.iter().map(|l| &l[..]).collect();
        let fits = fit_many(&refs, n, params, grid)?;
        progress(total, total);
        Ok(fits)
    };
    #[cfg(feature = "pro")]
    if params.backend.as_deref() != Some("cpu") {
        if let Some((dev, name)) = crate::ctf_gpu::device() {
            // "auto": spectra on the GPU; the grid search too for several
            // slices, on the CPU for one. Measured on an M1 Max: one slice
            // 0.18 s either way (Metal slower until warmed up), 41 slices
            // 1.4 s CPU vs 0.75–0.84 s Metal. "gpu": always on the GPU.
            let all = params.backend.as_deref() == Some("gpu") || zs.len() > 1;
            let tried = crate::ctf_gpu::guarded(|| -> Result<Vec<Fit>> {
                let logs = spectra(Some(dev))?;
                if all {
                    done(logs, &crate::ctf_gpu::GpuGrid(dev))
                } else {
                    done(logs, &CpuGrid)
                }
            });
            match tried.and_then(|r| r) {
                Ok(f) => {
                    let backend = if all { (*name).into() } else { format!("{name} 谱 + CPU 拟合") };
                    return Ok((f, backend, start.elapsed().as_secs_f64() * 1e3));
                }
                Err(e) if CANCEL.load(std::sync::atomic::Ordering::SeqCst) => return Err(e),
                Err(e) => eprintln!("CTF on {name} failed, using the CPU: {e:#}"),
            }
        }
    }
    let fits = done(spectra(None)?, &CpuGrid)?;
    Ok((fits, "CPU".into(), start.elapsed().as_secs_f64() * 1e3))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// xorshift64*, uniform in [0, 1).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f64 {
            let (a, b) = (self.next().max(1e-300), self.next());
            (-2.0 * a.ln()).sqrt() * (2.0 * PI * b).cos()
        }
    }

    pub fn params_pub() -> Params { params() }
    pub fn synth_pub(n: usize, p: &Params, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: u64) -> Vec<f32> { synth(n, p, a, b, c, d, e, f, g) }
    fn params() -> Params {
        Params {
            voltage_kv: 300.0,
            cs_mm: 2.7,
            amp_contrast: 0.07,
            pixel_a: 1.0,
            res_low_a: 30.0,
            res_high_a: 4.0,
            df_min_a: 5000.0,
            df_max_a: 50000.0,
            max_astig_a: 3000.0,
            phase: false,
            backend: None,
        }
    }

    /// Synthetic log10 power spectrum: falling background plus an enveloped
    /// CTF², with multiplicative noise.
    fn synth(n: usize, p: &Params, df1: f64, df2: f64, angle_deg: f64, phase_deg: f64, bfac: f64, noise: f64, seed: u64) -> Vec<f32> {
        let o = Optics::new(p);
        let h = (df1 - df2) / 2.0;
        let m = Model::from_astig((df1 + df2) / 2.0, h, angle_deg.to_radians(), phase_deg.to_radians());
        let mut rng = Rng(seed);
        let half = n as f64 / 2.0;
        let mut out = vec![0f32; n * n];
        for v in 0..n {
            for u in 0..n {
                let (du, dv) = (u as f64 - half, v as f64 - half);
                let k = (du * du + dv * dv).sqrt() / (n as f64 * p.pixel_a);
                let th = (-dv).atan2(du);
                let df = m.df((2.0 * th).cos(), (2.0 * th).sin());
                let g = o.gamma(df, k * k, m.phase).sin();
                let env = (-bfac * k * k / 4.0).exp();
                let bg = 1.0 / (1.0 + 400.0 * k * k) + 0.2;
                let a = bg + env * g * g * 0.6;
                // Power with exponential-ish noise from averaging a few tiles.
                let pw = a * a * (1.0 + noise * rng.gauss()).max(0.05);
                out[v * n + u] = pw.log10() as f32;
            }
        }
        out
    }

    #[test]
    fn wavelength_at_300_kv() {
        assert!((wavelength(300.0) - 0.019_687).abs() < 1e-5);
        assert!((wavelength(200.0) - 0.025_079).abs() < 1e-5);
    }

    #[test]
    fn zeros_sit_where_the_ctf_vanishes() {
        let p = params();
        let o = Optics::new(&p);
        let m = Model::from_astig(15000.0, 500.0, 0.3, 0.0);
        let z = zeros(&o, &m, 512, p.pixel_a);
        assert!(z.len() > 5);
        for ring in &z[..3] {
            for &[du, dv] in ring.iter().step_by(17) {
                let (du, dv) = (du as f64, dv as f64);
                let k = du.hypot(dv) / 512.0;
                let th = (-dv).atan2(du);
                let df = m.df((2.0 * th).cos(), (2.0 * th).sin());
                assert!(o.gamma(df, k * k, 0.0).sin().abs() < 1e-3);
            }
        }
    }

    fn check(fit: &Fit, df1: f64, df2: f64, angle: f64) {
        let tol = 0.01 * df1.max(1000.0);
        assert!((fit.df1 - df1).abs() < tol, "df1 {} vs {df1}", fit.df1);
        assert!((fit.df2 - df2).abs() < tol, "df2 {} vs {df2}", fit.df2);
        if df1 - df2 > 300.0 {
            let d = (fit.angle - angle).rem_euclid(180.0);
            assert!(d.min(180.0 - d) < 3.0, "angle {} vs {angle}", fit.angle);
        }
    }

    #[test]
    fn fits_defocus_and_astigmatism() {
        let p = params();
        for (i, &(df1, df2, ang)) in [(12000.0, 11000.0, 30.0), (25000.0, 24600.0, 125.0), (8000.0, 8000.0, 0.0), (32000.0, 30500.0, 80.0)].iter().enumerate() {
            let s = synth(512, &p, df1, df2, ang, 0.0, 60.0, 0.4, 7 + i as u64);
            let fit = fit_spectrum(&s, 512, &p).unwrap();
            check(&fit, df1, df2, ang);
            assert!(fit.score > 0.3, "score {}", fit.score);
            assert!(!fit.zeros.is_empty());
        }
    }

    /// A tilt series: defocus changes from slice to slice, one noisy slice.
    #[test]
    fn fits_a_series_together() {
        let p = params();
        let truth = [(14000.0, 13400.0, 40.0), (16500.0, 15900.0, 42.0), (11800.0, 11200.0, 38.0), (19000.0, 18400.0, 41.0)];
        let specs: Vec<Vec<f32>> = truth.iter().enumerate().map(|(i, &(a, b, c))| synth(256, &p, a, b, c, 0.0, 60.0, if i == 2 { 0.9 } else { 0.4 }, 30 + i as u64)).collect();
        let refs: Vec<&[f32]> = specs.iter().map(|s| &s[..]).collect();
        let fits = fit_many(&refs, 256, &p, &CpuGrid).unwrap();
        for ((f, &(a, b, c)), s) in fits.iter().zip(&truth).zip(&specs) {
            check(f, a, b, c);
            let one = fit_spectrum(s, 256, &p).unwrap();
            assert!((one.df1 - f.df1).abs() < 60.0, "alone {one:?} together {f:?}");
        }
    }

    #[test]
    fn reads_imod_tilt_angles() {
        let dir = std::env::temp_dir().join(format!("tmv-tlt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = dir.join("ts.st");
        assert!(tilt_angles(st.to_str().unwrap(), 3).is_none());
        std::fs::write(dir.join("ts.rawtlt"), "-3.0\n 0.0\n3.0\n").unwrap();
        assert_eq!(tilt_angles(st.to_str().unwrap(), 3), Some(vec![-3.0, 0.0, 3.0]));
        // Wrong count: not this series'.
        assert!(tilt_angles(st.to_str().unwrap(), 4).is_none());
        // A binned copy finds the series' own file by its name's start.
        std::fs::write(dir.join("TS21.tlt"), "-2\n0\n2\n4\n").unwrap();
        let bin = dir.join("TS21_bin2.mrc");
        assert_eq!(tilt_angles(bin.to_str().unwrap(), 4), Some(vec![-2.0, 0.0, 2.0, 4.0]));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fits_phase_shift() {
        let mut p = params();
        p.phase = true;
        let s = synth(512, &p, 6000.0, 5600.0, 60.0, 90.0, 60.0, 0.4, 3);
        let fit = fit_spectrum(&s, 512, &p).unwrap();
        check(&fit, 6000.0, 5600.0, 60.0);
        assert!((fit.phase - 90.0).abs() < 8.0, "phase {}", fit.phase);
    }

    #[test]
    fn fit_resolution_follows_the_envelope() {
        let p = params();
        // Strong damping: rings fade out well before Nyquist (2 Å).
        let sharp = fit_spectrum(&synth(512, &p, 15000.0, 14500.0, 10.0, 0.0, 20.0, 0.5, 11), 512, &p).unwrap();
        let soft = fit_spectrum(&synth(512, &p, 15000.0, 14500.0, 10.0, 0.0, 400.0, 0.5, 11), 512, &p).unwrap();
        assert!(sharp.fit_res < 4.0, "sharp {}", sharp.fit_res);
        assert!(soft.fit_res > sharp.fit_res + 1.0, "soft {} sharp {}", soft.fit_res, sharp.fit_res);
        assert!(soft.fit_res < 20.0, "soft {}", soft.fit_res);
    }

    /// An image whose Fourier transform is CTF-filtered noise (astigmatism
    /// `half` Å at `angle_deg`).
    pub fn ctf_image(size: usize, p: &Params, mean: f64, half: f64, angle_deg: f64, seed: u64) -> Vec<f32> {
        let o = Optics::new(p);
        let m = Model::from_astig(mean, half, angle_deg.to_radians(), 0.0);
        let f = fft::Fft::new(size);
        let mut rng = Rng(seed);
        let mut re: Vec<f32> = (0..size * size).map(|_| rng.gauss() as f32).collect();
        let mut im = vec![0f32; size * size];
        let fft2 = |re: &mut Vec<f32>, im: &mut Vec<f32>| {
            re.par_chunks_mut(size).zip(im.par_chunks_mut(size)).for_each(|(r, i)| f.run(r, i));
            let cols: Vec<(Vec<f32>, Vec<f32>)> = (0..size)
                .into_par_iter()
                .map(|x| {
                    let (mut cr, mut ci): (Vec<f32>, Vec<f32>) = (0..size).map(|y| (re[y * size + x], im[y * size + x])).unzip();
                    f.run(&mut cr, &mut ci);
                    (cr, ci)
                })
                .collect();
            for (x, (cr, ci)) in cols.into_iter().enumerate() {
                for y in 0..size {
                    re[y * size + x] = cr[y];
                    im[y * size + x] = ci[y];
                }
            }
        };
        fft2(&mut re, &mut im);
        for v in 0..size {
            for u in 0..size {
                let fu = if u < size / 2 { u as f64 } else { u as f64 - size as f64 };
                let fv = if v < size / 2 { v as f64 } else { v as f64 - size as f64 };
                let k = fu.hypot(fv) / (size as f64 * p.pixel_a);
                let th = (-fv).atan2(fu);
                let df = m.df((2.0 * th).cos(), (2.0 * th).sin());
                let ctf = -o.gamma(df, k * k, 0.0).sin() * (-30.0 * k * k).exp();
                let i = v * size + u;
                re[i] *= ctf as f32;
                im[i] *= ctf as f32;
            }
        }
        // Inverse by conjugating: only the real part is kept.
        for v in im.iter_mut() {
            *v = -*v;
        }
        fft2(&mut re, &mut im);
        re
    }

    /// Whole path: a CTF-filtered image through the FFT panel's spectrum.
    #[test]
    fn fits_an_image() {
        let p = params();
        let img = ctf_image(512, &p, 14000.0, 400.0, 50.0, 5);
        let s = fft::spectrum(|x, y| img[y * 512 + x], [0, 0, 512, 512], 256).unwrap();
        let fit = fit_spectrum(&s.log, s.n, &p).unwrap();
        check(&fit, 14400.0, 13600.0, 50.0);
    }
}

/// Writes a synthetic spectrum and its fit for the UI check:
/// TMV_CTF_DUMP=dir cargo test --release --lib ctf::dump -- --ignored
#[cfg(test)]
mod dump {
    #[test]
    #[ignore]
    fn dump() {
        let dir = std::env::var("TMV_CTF_DUMP").unwrap();
        let mut p = super::tests::params_pub();
        p.res_high_a = 5.0;
        let log = super::tests::synth_pub(512, &p, 14500.0, 13700.0, 35.0, 0.0, 80.0, 0.5, 4);
        let fit = super::fit_spectrum(&log, 512, &p).unwrap();
        let mut b = Vec::new();
        b.extend_from_slice(&512u32.to_le_bytes());
        b.extend_from_slice(&9u32.to_le_bytes());
        for v in &log {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for _ in 0..257 {
            b.extend_from_slice(&0f32.to_le_bytes());
        }
        std::fs::write(format!("{dir}/spectrum.bin"), b).unwrap();
        std::fs::write(format!("{dir}/fit.json"), serde_json::to_string(&fit).unwrap()).unwrap();
    }
}
