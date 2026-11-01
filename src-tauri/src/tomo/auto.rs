//! One-button reconstruction: everything the steps do, with the choices
//! made for the user. What it picks was measured on real data with held-out
//! projections (`quality`, examples/cryoet_quality.rs): views left out of
//! the reconstruction, projected back and compared with the measured view.
//!
//! - Alignment and CTF run when the dataset has none yet.
//! - What it makes depends on what the tomogram is for (`Purpose`):
//!   - survey: a quick look at the 3D shape, about 18 Å per voxel, SART
//!     with phase flipping and the deconvolution; no learned steps.
//!   - display (default): as clear as possible, about 9 Å per voxel,
//!     Noise2Noise and the deconvolution; missing-wedge correction when
//!     asked, since it trains a network for long (about 20 min on a
//!     desktop GPU, hours on a laptop's: 4.5 s per step on an M1 Max).
//!   - STA: the high resolution kept for subtomogram averaging, about
//!     4.5 Å per voxel, linear steps only (nothing learned or filtered that
//!     would change the high frequencies), and a RELION 5 folder beside
//!     the stack to go on with.
//! - Thickness: a quick bin-8 reconstruction twice as thick as likely
//!   shows where the specimen is (detail varies there, empty ice and
//!   vacuum do not); the tomogram covers that with a margin, centred on it.
//! - Reconstruction: SART, 3 rounds, with phase flipping. On EMPIAR-10045
//!   it reached 36 Å by held-out FRC against 45 Å for plain WBP and 38 Å
//!   for phase-flipped WBP at bin 4 (the same order at bin 2), at a third
//!   of SIRT's time. 3D CTF made no measurable difference at these
//!   samplings, so it is left out.
//! - Noise2Noise on the even/odd half tomograms when motion correction made
//!   them (else on neighbouring sections), then the Wiener-like
//!   deconvolution of Warp and IsoNet (`deconv`), which restores the low
//!   resolution contrast.

use super::{check_cancel, dataset::Dataset, output, pipeline, recon};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// What the tomogram is for.
#[derive(Clone, Copy, Deserialize, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Purpose {
    Survey,
    #[default]
    Display,
    Sta,
}

impl Purpose {
    /// Output voxel size aimed at, Å.
    fn voxel(self) -> f64 {
        match self {
            Purpose::Survey => 18.0,
            Purpose::Display => 9.0,
            Purpose::Sta => 4.5,
        }
    }
}

#[derive(Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoReq {
    /// Pixel size of the stack, Å, when neither the file nor the mdoc has it.
    pub pixel_a: Option<f64>,
    pub purpose: Purpose,
    /// For display: also correct the missing wedge (slow).
    pub wedge: bool,
    /// Start over even when the dataset has an alignment and CTF.
    pub fresh: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoResult {
    /// The tomogram to look at.
    pub path: String,
    pub raw: String,
    /// The RELION 5 folder (for STA).
    pub relion: Option<String>,
    pub bin: usize,
    pub thickness: usize,
    pub residual: f64,
    pub defocus_um: f64,
    pub ms: f64,
}

/// Most pixels a binned view may have.
const MAX_PIXELS: f64 = 4.2e6;

/// The binning (1, 2, 4 or 8) closest to `voxel` Å, within `MAX_PIXELS`.
pub fn output_bin(w: usize, h: usize, pixel_a: f64, voxel: f64) -> usize {
    [1usize, 2, 4, 8]
        .into_iter()
        .filter(|&b| (w * h) as f64 / (b * b) as f64 <= MAX_PIXELS || b == 8)
        .min_by(|&a, &b| ((pixel_a * a as f64) / voxel).ln().abs().total_cmp(&((pixel_a * b as f64) / voxel).ln().abs()))
        .unwrap()
}

pub fn run(dev: &burn::tensor::Device, d: &mut Dataset, req: &AutoReq, progress: pipeline::Progress) -> Result<AutoResult> {
    let now = std::time::Instant::now();
    let img = pipeline::open(&d.stack)?;
    let (w, h) = (img.info().width, img.info().height);
    if let Some(px) = req.pixel_a.filter(|p| *p > 0.0) {
        d.optics.pixel_a = Some(px);
    }
    let px = match d.optics.pixel_a.or_else(|| img.info().pixel_size.is_some().then(|| crate::align::pixel_a(img.info()))) {
        Some(p) => p,
        None => bail!("不知道像素大小：请填写像素大小（Å）"),
    };
    d.optics.pixel_a = Some(px);
    if d.angles.len() != img.info().size_z || d.angles.len() < 3 {
        bail!("没有找到倾转角（.mdoc / .tlt / .rawtlt），请在高级设置里给出起始角和步长");
    }
    // Each step's own counter restarts; the label says which step of all.
    let total = match req.purpose {
        Purpose::Survey => 5,
        Purpose::Display => 6 + req.wedge as usize,
        Purpose::Sta => 5,
    };
    let stage = |k: usize| move |label: &str, i: usize, n: usize| progress(&format!("第 {k}/{total} 步 · {label}"), i, n);
    if req.fresh {
        d.alignment = None;
        d.ctf = None;
    }
    if d.alignment.is_none() {
        let a = super::align(
            &img,
            &super::AlignReq { token: 0, c: 0, t: 0, angles: d.angles.clone(), start: None, step: None, axis: d.axis_hint, bin: 0, sweep: true, patch: None, excluded: vec![], groups: None, previous: None },
            &|i, n| stage(1)("倾转对齐", i, n),
        )?;
        d.alignment = Some(a);
        d.save()?;
    }
    check_cancel()?;
    if d.ctf.is_none() {
        let params = crate::ctf::Params {
            voltage_kv: d.optics.kv,
            cs_mm: d.optics.cs_mm,
            amp_contrast: d.optics.amplitude,
            pixel_a: px,
            res_low_a: 30.0,
            res_high_a: (2.5 * px).max(5.0),
            df_min_a: 5000.0,
            df_max_a: 80000.0,
            max_astig_a: 2000.0,
            phase: false,
            backend: None,
        };
        pipeline::ctf(d, &params, 512, &stage(2))?;
    }
    check_cancel()?;
    let a = d.alignment.clone().unwrap();
    let ctf = d.ctf.clone().unwrap();
    let bin = output_bin(w, h, px, req.purpose.voxel());
    let (thickness, z_offset) = estimate_thickness(&img, &a, px, bin, &stage(3))?;
    let params = recon::Params { thickness, method: recon::Method::Sart, iterations: 3, lambda: 0.5, z_offset, ..Default::default() };
    let made = pipeline::reconstruct(d, &pipeline::ReconStep { bin, params, halves: d.even.is_some() && d.odd.is_some(), phase_flip: true, ctf_3d: false }, &stage(4))?;
    let raw = made.paths[0].clone();
    let mut path = raw.clone();
    let mut relion = None;
    if req.purpose == Purpose::Display {
        check_cancel()?;
        path = pipeline::denoise(dev, d, &pipeline::DenoiseStep { source: Some(path), steps: 1000 }, &stage(5))?.paths.remove(0);
    }
    if req.purpose != Purpose::Sta {
        check_cancel()?;
        let k = if req.purpose == Purpose::Display { 6 } else { 5 };
        path = pipeline::deconvolve(d, Some(&path), &stage(k))?.paths.remove(0);
    }
    if req.purpose == Purpose::Display && req.wedge {
        check_cancel()?;
        path = pipeline::wedge(dev, d, &pipeline::WedgeStep { source: Some(path), cfg: None, single: false }, &stage(7))?.paths.remove(0);
    }
    if req.purpose == Purpose::Sta {
        check_cancel()?;
        let stem = d.stack.strip_suffix(".mrc").unwrap_or(&d.stack);
        let dir = format!("{stem}_relion5");
        std::fs::create_dir_all(&dir)?;
        super::export::run(d, "relion5", &dir, 1, &|i, n| stage(5)("导出 RELION 5", i, n))?;
        relion = Some(dir);
    }
    let dfs: Vec<f64> = ctf.tilts.iter().map(|t| 0.5 * (t.df1 + t.df2)).collect();
    let mut sorted = dfs.clone();
    sorted.sort_by(f64::total_cmp);
    Ok(AutoResult { path, raw, relion, bin, thickness, residual: a.residual, defocus_um: sorted[sorted.len() / 2] / 1e4, ms: now.elapsed().as_secs_f64() * 1e3 })
}

/// Thickness (output pixels at `bin`) and depth offset that cover the
/// specimen, from a quick bin-8 WBP twice as thick as a thick lamella.
pub fn estimate_thickness(
    img: &std::sync::Arc<crate::image::OpenImage>,
    a: &super::Alignment,
    px: f64,
    bin: usize,
    progress: pipeline::Progress,
) -> Result<(usize, f64)> {
    let probe = 8usize.max(bin);
    // 600 nm of depth is more than a lamella or a thin cell edge has.
    let depth = ((6000.0 / (px * probe as f64)).ceil() as usize).clamp(16, 512);
    let (stack, w, h, angles) = output::aligned(img, a, probe, 0, 0, &|i, n| progress("估计厚度", i, n))?;
    let params = recon::Params { thickness: depth, ..Default::default() };
    let how = pipeline::Build { alignment: a, bin: probe, params: &params, ctf: None, ctf_3d: false };
    let vol = pipeline::volume(&how, stack, w, h, &angles)?;
    let (lo, hi) = specimen_extent(&vol, w, h, depth);
    let scale = probe as f64 / bin as f64;
    // A margin of a quarter of the extent (at least 20 nm) on each side.
    let margin = ((hi - lo) as f64 / 4.0).max(200.0 / (px * probe as f64));
    let extent = ((hi - lo) as f64 + 2.0 * margin) * scale;
    let thickness = (extent.round() as usize).clamp(32, 2048);
    let centre = ((lo + hi) as f64 / 2.0 - (depth as f64 - 1.0) / 2.0) * scale;
    Ok((thickness.div_ceil(8) * 8, centre))
}

/// The range of sections with structure: per section, the variance of a
/// band-passed copy (Gaussian σ 1 minus σ 4 pixels: particle-sized detail,
/// neither pixel noise nor slow shading), smoothed along the depth; the
/// specimen is the run of sections around the peak that stay clearly above
/// the quiet sections' level (median + 3 × their spread).
pub fn specimen_extent(vol: &[f32], w: usize, h: usize, z: usize) -> (usize, usize) {
    use rayon::prelude::*;
    let plane = w * h;
    let blur = |s: &[f32], sigma: f32| -> Vec<f32> {
        let r = (3.0 * sigma).ceil() as isize;
        let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let sum: f32 = k.iter().sum();
        let pass = |src: &[f32], dx: isize, dy: isize| -> Vec<f32> {
            (0..plane)
                .map(|p| {
                    let (x, y) = ((p % w) as isize, (p / w) as isize);
                    k.iter().enumerate().map(|(j, kk)| {
                        let o = j as isize - r;
                        let (xx, yy) = ((x + o * dx).clamp(0, w as isize - 1), (y + o * dy).clamp(0, h as isize - 1));
                        kk * src[yy as usize * w + xx as usize]
                    }).sum::<f32>() / sum
                })
                .collect()
        };
        pass(&pass(s, 1, 0), 0, 1)
    };
    let raw: Vec<f64> = (0..z)
        .into_par_iter()
        .map(|k| {
            let s = &vol[k * plane..(k + 1) * plane];
            let (a, b) = (blur(s, 1.0), blur(s, 4.0));
            let d: Vec<f64> = a.iter().zip(&b).map(|(a, b)| (a - b) as f64).collect();
            let m = d.iter().sum::<f64>() / d.len() as f64;
            d.iter().map(|v| (v - m).powi(2)).sum::<f64>() / d.len() as f64
        })
        .collect();
    let detail: Vec<f64> = (0..z).map(|k| {
        let r: Vec<f64> = (k.saturating_sub(2)..(k + 3).min(z)).map(|j| raw[j]).collect();
        r.iter().sum::<f64>() / r.len() as f64
    }).collect();
    let mut sorted = detail.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[z / 2];
    let mut dev: Vec<f64> = detail.iter().map(|d| (d - median).abs()).collect();
    dev.sort_by(f64::total_cmp);
    let spread = dev[z / 2] * 1.4826;
    let peak = sorted[z - 1];
    if peak < median + 4.0 * spread {
        return (z / 4, 3 * z / 4);
    }
    // From the peak out to where the level is back near the quiet one.
    let cut = median + 3.0 * spread;
    let top = (0..z).max_by(|&a, &b| detail[a].total_cmp(&detail[b])).unwrap();
    let mut lo = top;
    while lo > 0 && detail[lo - 1] >= cut {
        lo -= 1;
    }
    let mut hi = top;
    while hi + 1 < z && detail[hi + 1] >= cut {
        hi += 1;
    }
    (lo, hi + 1)
}
