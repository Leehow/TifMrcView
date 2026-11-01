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
//!     Noise2Noise, the deconvolution and missing-wedge correction when
//!     this computer's GPU can train it in `WEDGE_BUDGET` (about 20 min on
//!     a desktop GPU; an M1 Max would take 2.5 h at 4.5 s a step), judged
//!     from its first half minute of training; else the deconvolved
//!     tomogram is the result and `note` says why.
//!   - STA: the high resolution kept for subtomogram averaging, about
//!     4.5 Å per voxel, linear steps only (nothing learned or filtered that
//!     would change the high frequencies), and a RELION 5 folder beside
//!     the stack to go on with.
//! - Levelling and thickness (`level`): a quick bin-8 reconstruction twice
//!   as thick as likely shows where the specimen is (detail varies there,
//!   empty ice and vacuum do not); a lamella milled at an angle is laid
//!   flat at the reconstruction stage (tilt offset, depth slope along Y),
//!   and the tomogram covers the specimen with a margin, centred on it.
//! - Reconstruction: SART, 3 rounds, with phase flipping. On EMPIAR-10045
//!   it reached 36 Å by held-out FRC against 45 Å for plain WBP and 38 Å
//!   for phase-flipped WBP at bin 4 (the same order at bin 2), at a third
//!   of SIRT's time. 3D CTF made no measurable difference at these
//!   samplings, so it is left out.
//! - Display tomograms end with membrane segmentation (`crate::membrane`)
//!   into the tomogram's label map, when the model has been downloaded.
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
    /// The label map with the membranes (material "膜"), beside `path`.
    pub labels: Option<String>,
    /// Something the user should know about the result.
    pub note: Option<String>,
    /// Seconds each step took, in order.
    pub timings: Vec<(String, f64)>,
    /// The batches that chose which views to leave out.
    pub selection: Vec<super::select::Batch>,
    /// How far the specimen was turned to lie flat (°): about Y (the tilt
    /// offset) and about X (the depth slope along Y).
    pub levelled: [f64; 2],
    pub bin: usize,
    pub thickness: usize,
    pub residual: f64,
    pub defocus_um: f64,
    pub ms: f64,
}

/// Longest missing-wedge training the display tomogram waits for, seconds.
const WEDGE_BUDGET: f64 = 30.0 * 60.0;
/// Most pixels a binned view may have.
const MAX_PIXELS: f64 = 4.2e6;

/// The binning (1 to 16; 16 for super-resolution frames) closest to `voxel` Å, within `MAX_PIXELS`.
pub fn output_bin(w: usize, h: usize, pixel_a: f64, voxel: f64) -> usize {
    [1usize, 2, 4, 8, 16]
        .into_iter()
        .filter(|&b| (w * h) as f64 / (b * b) as f64 <= MAX_PIXELS || b == 16)
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
    // Membranes are segmented at the end of a display tomogram when the
    // model is on this computer (the app downloads it beforehand).
    let membranes = req.purpose == Purpose::Display && crate::membrane::model_path().is_some_and(|p| p.exists());
    // Each step's own counter restarts; the label says which step of all.
    let total = match req.purpose {
        Purpose::Survey => 5,
        Purpose::Display => 7 + membranes as usize,
        Purpose::Sta => 5,
    };
    // When each step was first and last heard from, for `timings` (steps
    // can overlap).
    let heard = std::sync::Mutex::new(Vec::<(usize, String, std::time::Instant, std::time::Instant)>::new());
    let stage = |k: usize| {
        let heard = &heard;
        move |label: &str, i: usize, n: usize| {
            let now = std::time::Instant::now();
            let mut h = heard.lock().unwrap();
            match h.iter_mut().find(|e| e.0 == k) {
                Some(e) => e.3 = now,
                None => h.push((k, label.split(['：', ' ']).next().unwrap_or(label).to_string(), now, now)),
            }
            drop(h);
            progress(&format!("第 {k}/{total} 步 · {label}"), i, n)
        }
    };
    if req.fresh {
        d.alignment = None;
        d.ctf = None;
    }
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
    let align_req = super::AlignReq { token: 0, c: 0, t: 0, angles: d.angles.clone(), start: None, step: None, axis: d.axis_hint, bin: 0, sweep: true, patch: None, excluded: vec![], groups: None, previous: None };
    match (d.alignment.is_none(), d.ctf.is_none()) {
        // With the acquisition's axis known, the CTF does not wait for the
        // alignment: both run at once (the CTF on a copy of the dataset).
        (true, true) if d.axis_hint.is_some() => {
            let mut copy = d.clone();
            let (a, c) = std::thread::scope(|s| {
                let c = s.spawn(|| pipeline::ctf(&mut copy, &params, 512, &stage(2)));
                let a = super::align(&img, &align_req, &|i, n| stage(1)("倾转对齐", i, n));
                (a, c.join().unwrap_or_else(|_| Err(anyhow::anyhow!("CTF 拟合线程崩溃"))))
            });
            d.alignment = Some(a?);
            d.ctf = Some(c?);
            d.save()?;
        }
        (need_align, need_ctf) => {
            if need_align {
                d.alignment = Some(super::align(&img, &align_req, &|i, n| stage(1)("倾转对齐", i, n))?);
                d.save()?;
            }
            check_cancel()?;
            if need_ctf {
                pipeline::ctf(d, &params, 512, &stage(2))?;
            }
        }
    }
    check_cancel()?;
    // Views that make the tomogram worse are left out, judged by quick
    // tomograms in batches (`select`); not for the quick survey.
    let mut selection = vec![];
    if req.purpose != Purpose::Survey {
        let (picked, batches) = super::select::exclude_views(&img, d, d.alignment.as_ref().unwrap(), &stage(3))?;
        if batches.iter().any(|b| b.kept.is_some()) {
            d.alignment = Some(picked);
            d.save()?;
        }
        selection = batches;
    }
    check_cancel()?;
    let a = d.alignment.clone().unwrap();
    let ctf = d.ctf.clone().unwrap();
    let bin = output_bin(w, h, px, req.purpose.voxel());
    let mut leveled = a.clone();
    let place = level(&img, &mut leveled, px, bin, req.purpose != Purpose::Sta, &stage(3))?;
    if place.tilt_offset != 0.0 {
        d.alignment = Some(leveled);
        d.save()?;
    }
    let thickness = place.thickness;
    let params = recon::Params { thickness, method: recon::Method::Sart, iterations: 3, lambda: 0.5, z_offset: place.z_offset, y_slope: place.y_slope, ..Default::default() };
    let made = pipeline::reconstruct(d, &pipeline::ReconStep { bin, params, halves: d.even.is_some() && d.odd.is_some(), phase_flip: true, ctf_3d: false }, &stage(4))?;
    let raw = made.paths[0].clone();
    let mut path = raw.clone();
    let mut relion = None;
    let mut note = None;
    if req.purpose == Purpose::Display {
        check_cancel()?;
        path = pipeline::denoise(dev, d, &pipeline::DenoiseStep { source: Some(path), steps: 1000 }, &stage(5))?.paths.remove(0);
    }
    if req.purpose != Purpose::Sta {
        check_cancel()?;
        let k = if req.purpose == Purpose::Display { 6 } else { 5 };
        path = pipeline::deconvolve(d, Some(&path), &stage(k))?.paths.remove(0);
    }
    if req.purpose == Purpose::Display {
        check_cancel()?;
        let step = pipeline::WedgeStep { source: Some(path.clone()), cfg: None, single: false, budget_s: Some(WEDGE_BUDGET) };
        match pipeline::wedge(dev, d, &step, &stage(7)) {
            Ok(mut m) => path = m.paths.remove(0),
            Err(e) => match e.downcast_ref::<pipeline::TooSlow>() {
                Some(slow) => note = Some(format!("这台电脑的 GPU 补缺失楔约需 {:.1} 小时，已跳过（可在高级设置⑤里单独运行）", slow.projected_s / 3600.0)),
                None => return Err(e),
            },
        }
    }
    let mut labels = None;
    if membranes {
        check_cancel()?;
        labels = Some(pipeline::membranes(dev, d, Some(&path), &stage(8))?.paths.remove(0));
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
    let mut heard = heard.into_inner().unwrap();
    heard.sort_by_key(|e| e.0);
    let timings = heard.iter().map(|e| (e.1.clone(), e.3.duration_since(e.2).as_secs_f64())).collect();
    Ok(AutoResult { path, raw, relion, labels, note, timings, selection, levelled: [place.tilt_offset, place.y_slope.atan().to_degrees()], bin, thickness, residual: a.residual, defocus_um: sorted[sorted.len() / 2] / 1e4, ms: now.elapsed().as_secs_f64() * 1e3 })
}

/// How the tomogram is placed: thickness and depth offset (output pixels
/// at `bin`), the slope along Y (see `recon::Params::y_slope`) and the tilt
/// offset added to the alignment (°).
#[derive(Clone, Copy, Debug, Default)]
pub struct Placement {
    pub thickness: usize,
    pub z_offset: f64,
    pub y_slope: f64,
    pub tilt_offset: f64,
}

/// Slopes below this (about half a degree) are left alone.
const LEVEL: f64 = 0.009;

/// Levels the specimen and finds its extent, the way the tomoflatten
/// method does it at the reconstruction stage (not by warping a finished
/// tomogram): a quick bin-8 WBP twice as thick as a thick lamella shows
/// where the specimen's band of detail runs; a plane fitted to its centre
/// gives the slope across the tilt axis, removed by adding a tilt offset
/// to the alignment (IMOD's OFFSET), and the slope along it, removed by
/// taking each row deeper or shallower (`y_slope`; not with `level_y`
/// false, for STA, whose RELION export cannot carry it). Checked and
/// repeated on a new probe until both are level; then the tomogram covers
/// the band with a margin, centred on it.
pub fn level(
    img: &std::sync::Arc<crate::image::OpenImage>,
    a: &mut super::Alignment,
    px: f64,
    bin: usize,
    level_y: bool,
    progress: pipeline::Progress,
) -> Result<Placement> {
    let probe = 8usize.max(bin);
    // 600 nm of depth is more than a lamella or a thin cell edge has.
    let depth = ((6000.0 / (px * probe as f64)).ceil() as usize).clamp(16, 512);
    let mut y_slope = 0.0;
    let mut offset = 0.0;
    let mut round = 0;
    let (vol, w, h) = loop {
        check_cancel()?;
        let (stack, w, h, angles) = output::aligned(img, a, probe, 0, 0, &|i, n| progress("估计厚度和倾斜", i, n))?;
        let params = recon::Params { thickness: depth, y_slope, ..Default::default() };
        let how = pipeline::Build { alignment: a, bin: probe, params: &params, ctf: None, ctf_3d: false };
        let vol = pipeline::volume(&how, stack, w, h, &angles)?;
        round += 1;
        let Some([sx, sy, _]) = plane(&vol, w, h, depth) else { break (vol, w, h) };
        let sy = if level_y { sy } else { 0.0 };
        if (sx.abs() < LEVEL && sy.abs() < LEVEL) || round == 4 {
            break (vol, w, h);
        }
        // The band rises by sx sections per column: turning every view by
        // −atan(sx) lays it flat.
        let turn = -sx.atan().to_degrees();
        for v in &mut a.views {
            v.tilt = v.refined() + turn;
        }
        a.tilt_offset += turn;
        offset += turn;
        y_slope += sy;
    };
    let (lo, hi) = specimen_extent(&vol, w, h, depth);
    let scale = probe as f64 / bin as f64;
    // A margin of a quarter of the extent (at least 20 nm) on each side.
    let margin = ((hi - lo) as f64 / 4.0).max(200.0 / (px * probe as f64));
    let extent = ((hi - lo) as f64 + 2.0 * margin) * scale;
    let thickness = (extent.round() as usize).clamp(32, 2048);
    let centre = ((lo + hi) as f64 / 2.0 - (depth as f64 - 1.0) / 2.0) * scale;
    Ok(Placement { thickness: thickness.div_ceil(8) * 8, z_offset: centre, y_slope, tilt_offset: offset })
}

/// Band-pass of one section (Gaussian σ 1 minus σ 4 pixels): particle-sized
/// detail, neither pixel noise nor slow shading.
fn band_pass(s: &[f32], w: usize, h: usize) -> Vec<f32> {
    let blur = |s: &[f32], sigma: f32| -> Vec<f32> {
        let r = (3.0 * sigma).ceil() as isize;
        let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let sum: f32 = k.iter().sum();
        let pass = |src: &[f32], dx: isize, dy: isize| -> Vec<f32> {
            (0..w * h)
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
    let (a, b) = (blur(s, 1.0), blur(s, 4.0));
    a.iter().zip(&b).map(|(a, b)| a - b).collect()
}

/// The plane z = sx·x + sy·y + c (sections, x and y in pixels from the
/// centre) through the middle of the specimen's band of detail: per block
/// of the view, the depth profile of band-passed power, its band taken
/// where it stands clearly above the quiet sections and its centre
/// weighted by the excess; blocks without a clear band are left out, and
/// the fit drops blocks far off it (local structure) twice.
pub fn plane(vol: &[f32], w: usize, h: usize, z: usize) -> Option<[f64; 3]> {
    use rayon::prelude::*;
    let plane = w * h;
    let power: Vec<Vec<f32>> = (0..z)
        .into_par_iter()
        .map(|k| band_pass(&vol[k * plane..(k + 1) * plane], w, h).into_iter().map(|v| v * v).collect())
        .collect();
    // About 12 blocks along the longer side, at least 16 pixels each.
    let side = (w.max(h) / 12).max(16);
    let (nx, ny) = ((w / side).max(1), (h / side).max(1));
    let mut points = Vec::new();
    for by in 0..ny {
        for bx in 0..nx {
            let (x0, x1, y0, y1) = (bx * w / nx, (bx + 1) * w / nx, by * h / ny, (by + 1) * h / ny);
            let raw: Vec<f64> = power
                .iter()
                .map(|p| (y0..y1).map(|y| p[y * w + x0..y * w + x1].iter().map(|&v| v as f64).sum::<f64>()).sum::<f64>())
                .collect();
            let prof: Vec<f64> = (0..z).map(|k| {
                let r = &raw[k.saturating_sub(2)..(k + 3).min(z)];
                r.iter().sum::<f64>() / r.len() as f64
            }).collect();
            let mut sorted = prof.clone();
            sorted.sort_by(f64::total_cmp);
            let median = sorted[z / 2];
            let mut dev: Vec<f64> = prof.iter().map(|p| (p - median).abs()).collect();
            dev.sort_by(f64::total_cmp);
            let spread = (dev[z / 2] * 1.4826).max(1e-12);
            let top = (0..z).max_by(|&a, &b| prof[a].total_cmp(&prof[b])).unwrap();
            if prof[top] < median + 6.0 * spread {
                continue;
            }
            // The band: the run around the peak clearly above the quiet
            // level. Not just the peak's half height: one bright surface
            // (contamination on a lamella) would then stand for the band in
            // some blocks and not in others, and tilt the fit
            // (EMPIAR-10988 TS_026: 3.6° found, 2.2° true).
            let cut = median + 3.0 * spread;
            let (mut lo, mut hi) = (top, top);
            while lo > 0 && prof[lo - 1] >= cut {
                lo -= 1;
            }
            while hi + 1 < z && prof[hi + 1] >= cut {
                hi += 1;
            }
            let (mut sum, mut at) = (0.0, 0.0);
            for k in lo..=hi {
                sum += prof[k] - median;
                at += (prof[k] - median) * k as f64;
            }
            let cx = (x0 + x1) as f64 / 2.0 - w as f64 / 2.0;
            let cy = (y0 + y1) as f64 / 2.0 - h as f64 / 2.0;
            points.push((cx, cy, at / sum, ((prof[top] - median) / spread).sqrt()));
        }
    }
    let mut keep = vec![true; points.len()];
    let mut fit = None;
    for _ in 0..3 {
        let used: Vec<&(f64, f64, f64, f64)> = points.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| p).collect();
        if used.len() < 6 {
            return None;
        }
        let f = least_squares(&used)?;
        let res: Vec<f64> = points.iter().map(|p| (p.2 - (f[0] * p.0 + f[1] * p.1 + f[2])).abs()).collect();
        let mut r: Vec<f64> = res.iter().zip(&keep).filter(|(_, k)| **k).map(|(r, _)| *r).collect();
        r.sort_by(f64::total_cmp);
        let limit = (3.0 * 1.4826 * r[r.len() / 2]).max(1.0);
        keep = res.iter().map(|r| *r <= limit).collect();
        fit = Some(f);
    }
    // Columns must spread along both axes for both slopes to mean anything.
    let xs: Vec<f64> = points.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| p.0).collect();
    let ys: Vec<f64> = points.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| p.1).collect();
    let spread = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max) - v.iter().copied().fold(f64::INFINITY, f64::min);
    let mut f = fit?;
    if spread(&xs) < w as f64 / 3.0 {
        f[0] = 0.0;
    }
    if spread(&ys) < h as f64 / 3.0 {
        f[1] = 0.0;
    }
    Some(f)
}

/// Weighted least squares of z = a·x + b·y + c over (x, y, z, weight).
fn least_squares(p: &[&(f64, f64, f64, f64)]) -> Option<[f64; 3]> {
    let mut m = [[0f64; 4]; 3];
    for &&(x, y, z, wt) in p {
        let r = [x, y, 1.0];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += wt * r[i] * r[j];
            }
            m[i][3] += wt * r[i] * z;
        }
    }
    // Gauss–Jordan with partial pivoting.
    for c in 0..3 {
        let p = (c..3).max_by(|&a, &b| m[a][c].abs().total_cmp(&m[b][c].abs()))?;
        if m[p][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, p);
        for r in 0..3 {
            if r != c {
                let f = m[r][c] / m[c][c];
                for k in c..4 {
                    m[r][k] -= f * m[c][k];
                }
            }
        }
    }
    Some([m[0][3] / m[0][0], m[1][3] / m[1][1], m[2][3] / m[2][2]])
}

/// The range of sections with structure: per section, the variance of a
/// band-passed copy (`band_pass`), smoothed along the depth; the specimen
/// is the run of sections around the peak that stay clearly above the
/// quiet sections' level (median + 3 × their spread).
pub fn specimen_extent(vol: &[f32], w: usize, h: usize, z: usize) -> (usize, usize) {
    use rayon::prelude::*;
    let plane = w * h;
    let raw: Vec<f64> = (0..z)
        .into_par_iter()
        .map(|k| {
            let d = band_pass(&vol[k * plane..(k + 1) * plane], w, h);
            let m = d.iter().map(|&v| v as f64).sum::<f64>() / d.len() as f64;
            d.iter().map(|&v| (v as f64 - m).powi(2)).sum::<f64>() / d.len() as f64
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

#[cfg(test)]
mod tests {
    #[test]
    fn plane_follows_a_tilted_band() {
        let (w, h, z) = (96usize, 64usize, 48usize);
        let mut seed = 7u64;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 1000) as f32 / 1000.0 - 0.5
        };
        let mut vol = vec![0f32; w * h * z];
        for k in 0..z {
            for y in 0..h {
                for x in 0..w {
                    // The band's centre rises 0.1 per column and falls 0.05 per row.
                    let c = 24.0 + 0.1 * (x as f64 - 48.0) - 0.05 * (y as f64 - 32.0);
                    let inside = (k as f64 - c).abs() < 3.0;
                    vol[(k * h + y) * w + x] = noise() * if inside { 1.0 } else { 0.05 };
                }
            }
        }
        let [sx, sy, c] = super::plane(&vol, w, h, z).unwrap();
        assert!((sx - 0.1).abs() < 0.02, "sx {sx}");
        assert!((sy + 0.05).abs() < 0.02, "sy {sy}");
        assert!((c - 24.0).abs() < 1.5, "c {c}");
    }
}
