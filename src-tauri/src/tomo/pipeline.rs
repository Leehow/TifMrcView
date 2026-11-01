//! The Cryo-ET steps after alignment, driven by the dataset: they read what
//! earlier steps left in it and record what they make. Outputs are written
//! beside the stack, named after it; a file the dataset already lists is
//! replaced, any other existing file is left alone (a number is added).
//!
//! - reconstruct: the full stack and, when motion correction left them, the
//!   even and odd half stacks, with the same alignment (and phase flipping).
//! - denoise: Noise2Noise between the even and odd half tomograms (the
//!   network maps one half's section to the other's; both halves are
//!   denoised and averaged), or between neighbouring sections when there
//!   are no halves.
//! - wedge: missing-wedge correction (IsoNet method, `crate::wedge`) with
//!   the tilt range actually used.

use super::{check_cancel, dataset::{Dataset, Tomogram}, output, recon, CANCEL};
use crate::image::OpenImage;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{atomic::Ordering, Arc};

/// Progress with the name of the stage.
pub type Progress<'a> = &'a (dyn Fn(&str, usize, usize) + Sync);

pub fn open(path: &str) -> Result<Arc<OpenImage>> {
    Ok(OpenImage::new(0, crate::formats::open(Path::new(path))?, 256 << 20))
}

/// `<stem>_<suffix>.mrc` beside the stack; an unlisted existing file is kept.
fn output_path(d: &Dataset, suffix: &str) -> String {
    let p = Path::new(&d.stack);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("stack");
    for k in 1.. {
        let name = if k == 1 { format!("{stem}_{suffix}.mrc") } else { format!("{stem}_{suffix}_{k}.mrc") };
        let f = p.with_file_name(name).to_string_lossy().into_owned();
        if !Path::new(&f).exists() || d.tomograms.iter().any(|t| t.path == f) {
            return f;
        }
    }
    unreachable!()
}

fn pixel_a(d: &Dataset, img: &Arc<OpenImage>) -> f64 {
    d.optics.pixel_a.unwrap_or_else(|| output::pixel_a(img))
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconStep {
    pub bin: usize,
    pub params: recon::Params,
    /// Also reconstruct the even and odd halves (for Noise2Noise).
    pub halves: bool,
    pub phase_flip: bool,
    /// 3D CTF correction (WBP only): the volume is built in slabs along the
    /// depth, each from views phase flipped for that depth.
    #[serde(default)]
    pub ctf_3d: bool,
}

/// Largest defocus change across one 3D-CTF slab, Å.
const SLAB_DEFOCUS: f64 = 150.0;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Made {
    pub paths: Vec<String>,
    pub backend: String,
    pub ms: f64,
}

pub fn reconstruct(d: &mut Dataset, req: &ReconStep, progress: Progress) -> Result<Made> {
    let now = std::time::Instant::now();
    let Some(a) = d.alignment.clone() else { bail!("先完成倾转对齐") };
    let flip = if req.phase_flip {
        let Some(ctf) = d.ctf.as_ref() else { bail!("先完成 CTF 估计，或关闭相位翻转") };
        Some(ctf.clone())
    } else {
        None
    };
    let mut sources = vec![("full", d.stack.clone())];
    if req.halves {
        match (&d.even, &d.odd) {
            (Some(e), Some(o)) => sources.extend([("even", e.clone()), ("odd", o.clone())]),
            _ => bail!("没有奇偶帧半叠层（运动校正时生成），无法重构两半"),
        }
    }
    // The method is in the name, so WBP and SIRT of the same data both stay.
    let method = match req.params.method {
        recon::Method::Wbp => "wbp".to_string(),
        recon::Method::Sirt => format!("sirt{}", req.params.iterations),
        recon::Method::Sart => format!("sart{}", req.params.iterations),
    };
    let three_d = req.ctf_3d && flip.is_some();
    if req.ctf_3d && req.params.method != recon::Method::Wbp {
        bail!("3D CTF 校正只用于 WBP（SIRT 的正投影要整块体积）");
    }
    if req.ctf_3d && flip.is_none() {
        bail!("3D CTF 校正需要先完成 CTF 估计并勾选相位翻转");
    }
    let tag = format!("{method}_b{}{}", req.bin, if three_d { "_ctf3d" } else if flip.is_some() { "_flip" } else { "" });
    let mut made: Vec<Tomogram> = Vec::new();
    let mut backend = String::new();
    for (kind, src) in sources {
        check_cancel()?;
        let img = open(&src)?;
        let px = pixel_a(d, &img);
        output::validate(&img, &a, req.bin)?;
        let stage = format!("重构 {}", match kind { "even" => "偶数帧半", "odd" => "奇数帧半", _ => "全部" });
        let path = output_path(d, &if kind == "full" { tag.clone() } else { format!("{tag}_{kind}") });
        let (stack, w, h, angles) = output::aligned(&img, &a, req.bin, 0, 0, &|i, n| progress(&format!("{stage}：对齐叠层"), i, n))?;
        let mut out = output::VolumeFile::create(&path, [w, h, req.params.thickness], px * req.bin as f64)?;
        let how = Build { alignment: &a, bin: req.bin, params: &req.params, ctf: flip.as_ref().map(|c| (c, &d.optics, px)), ctf_3d: three_d };
        backend = build(&how, stack, w, h, &angles, &mut |y0, n, z0, nz, block| out.write_block(y0, n, z0, nz, block), &|label, i, n| progress(&format!("{stage}{label}"), i, n))?;
        out.finish()?;
        made.push(Tomogram {
            path,
            kind: kind.into(),
            bin: req.bin,
            thickness: req.params.thickness,
            method: format!("{:?}{}", req.params.method, if three_d { "+ctf3d" } else { "" }).to_lowercase(),
            phase_flipped: flip.is_some(),
            halves: None,
        });
    }
    if made.len() == 3 {
        made[0].halves = Some([made[1].path.clone(), made[2].path.clone()]);
    }
    let paths = made.iter().map(|t| t.path.clone()).collect();
    for t in made {
        d.add_tomogram(t);
    }
    d.save()?;
    Ok(Made { paths, backend, ms: now.elapsed().as_secs_f64() * 1e3 })
}

/// Per-tilt CTF of the stack, kept in the dataset.
pub fn ctf(d: &mut Dataset, params: &crate::ctf::Params, n: usize, progress: Progress) -> Result<super::dataset::Ctf> {
    let img = open(&d.stack)?;
    let mut params = params.clone();
    // The pixel size given with the request wins (and is kept); else the dataset's or the header's.
    if params.pixel_a > 0.0 {
        d.optics.pixel_a = Some(params.pixel_a);
    } else {
        params.pixel_a = pixel_a(d, &img);
    }
    d.optics.kv = params.voltage_kv;
    d.optics.cs_mm = params.cs_mm;
    d.optics.amplitude = params.amp_contrast;
    let fits = super::ctf::fit_series(&img, d, &params, n, &|i, n| progress("CTF 拟合", i, n))?;
    d.ctf = Some(fits.clone());
    d.save()?;
    Ok(fits)
}

/// How a tomogram is built from aligned views.
pub struct Build<'a> {
    pub alignment: &'a super::Alignment,
    pub bin: usize,
    pub params: &'a recon::Params,
    /// Phase flip with this CTF (2D, or 3D with `ctf_3d`).
    pub ctf: Option<output::FlipWith<'a>>,
    pub ctf_3d: bool,
}

/// Reconstructs aligned, unflipped views (`output::aligned`); blocks of
/// [n rows][nz sections][w] go to `sink(y0, n, z0, nz, block)`.
pub fn build(
    how: &Build,
    mut stack: Vec<f32>,
    w: usize,
    h: usize,
    angles: &[f64],
    sink: &mut dyn FnMut(usize, usize, usize, usize, &[f32]) -> Result<()>,
    progress: &(dyn Fn(&str, usize, usize) + Sync),
) -> Result<String> {
    let p = how.params;
    if p.y_slope != 0.0 {
        return sheared(how, stack, w, h, angles, sink, progress);
    }
    let thickness = p.thickness;
    match how.ctf {
        Some(ctf) if how.ctf_3d => {
            let px_out = ctf.2 * how.bin as f64;
            // Slabs thin enough that the defocus changes by at most SLAB_DEFOCUS
            // across one, even at the steepest tilt kept.
            let steep = angles.iter().map(|t| t.to_radians().cos()).fold(1.0, f64::min);
            let slab = ((SLAB_DEFOCUS * steep / px_out).floor() as usize).clamp(1, thickness);
            let slabs: Vec<usize> = (0..thickness).step_by(slab).collect();
            let mut backend = String::new();
            for (k, &z0) in slabs.iter().enumerate() {
                check_cancel()?;
                let nz = slab.min(thickness - z0);
                // Depth of this slab's centre: its offset in the slab's own geometry.
                let centre = z0 as f64 + (nz as f64 - 1.0) / 2.0 - (thickness as f64 - 1.0) / 2.0 + p.z_offset;
                let mut flipped = stack.clone();
                output::flip_views(&mut flipped, w, h, how.alignment, how.bin, ctf, centre);
                let g = recon::Geometry::new(w, nz, angles, centre)?;
                let views = recon::Views { data: &flipped, w, h, views: angles.len() };
                let label = format!("：3D CTF 第 {}/{} 层", k + 1, slabs.len());
                backend = recon::reconstruct(&views, &g, p, &mut |y0, n, block| sink(y0, n, z0, nz, &block), &|i, n| progress(&label, i, n))?;
            }
            Ok(backend)
        }
        ctf => {
            if let Some(ctf) = ctf {
                output::flip_views(&mut stack, w, h, how.alignment, how.bin, ctf, 0.0);
            }
            let g = recon::Geometry::new(w, thickness, angles, p.z_offset)?;
            let views = recon::Views { data: &stack, w, h, views: angles.len() };
            recon::reconstruct(&views, &g, p, &mut |y0, n, block| sink(y0, n, 0, thickness, &block), &|i, n| progress("", i, n))
        }
    }
}

/// `build` with `y_slope`: a tomogram thicker by the depth the slope spans,
/// of which each row keeps the window shifted by its whole-section offset.
fn sheared(
    how: &Build,
    stack: Vec<f32>,
    w: usize,
    h: usize,
    angles: &[f64],
    sink: &mut dyn FnMut(usize, usize, usize, usize, &[f32]) -> Result<()>,
    progress: &(dyn Fn(&str, usize, usize) + Sync),
) -> Result<String> {
    let p = how.params;
    let span = p.y_slope.abs() * (h - 1) as f64 / 2.0;
    let half = span.ceil() as usize;
    let params = recon::Params { thickness: p.thickness + 2 * half, y_slope: 0.0, ..p.clone() };
    let inner = Build { alignment: how.alignment, bin: how.bin, params: &params, ctf: how.ctf, ctf_3d: how.ctf_3d };
    let centre = (h as f64 - 1.0) / 2.0;
    let start = |y: usize| half as isize + (p.y_slope * (y as f64 - centre)).round() as isize;
    let t = p.thickness as isize;
    build(&inner, stack, w, h, angles, &mut |y0, n, z0, nz, block| {
        for r in 0..n {
            let s = start(y0 + r);
            let (lo, hi) = ((z0 as isize - s).max(0), (z0 as isize + nz as isize - s).min(t));
            if lo >= hi {
                continue;
            }
            let k0 = (lo + s) as usize - z0;
            let rows: Vec<f32> = (0..(hi - lo) as usize).flat_map(|k| block[(r * nz + k0 + k) * w..(r * nz + k0 + k + 1) * w].iter().copied()).collect();
            sink(y0 + r, 1, lo as usize, (hi - lo) as usize, &rows)?;
        }
        Ok(())
    }, progress)
}

/// A tomogram in memory as [z][y][x] (sections along Z), for comparisons.
pub fn volume(how: &Build, stack: Vec<f32>, w: usize, h: usize, angles: &[f64]) -> Result<Vec<f32>> {
    let z = how.params.thickness;
    let mut v = vec![0f32; w * h * z];
    build(how, stack, w, h, angles, &mut |y0, n, z0, nz, block| {
        for y in 0..n {
            for k in 0..nz {
                let dst = ((z0 + k) * h + y0 + y) * w;
                v[dst..dst + w].copy_from_slice(&block[(y * nz + k) * w..(y * nz + k + 1) * w]);
            }
        }
        Ok(())
    }, &|_, _, _| {})?;
    Ok(v)
}

/// Wiener-like deconvolution (`deconv`) of a phase-flipped tomogram at the
/// series' median defocus; written as `<name>_deconv.mrc`.
pub fn deconvolve(d: &mut Dataset, source: Option<&str>, progress: Progress) -> Result<Made> {
    let now = std::time::Instant::now();
    let src = self::source(d, source)?.clone();
    let Some(ctf) = d.ctf.as_ref() else { bail!("先完成 CTF 估计") };
    let mut df: Vec<f64> = ctf.tilts.iter().map(|t| 0.5 * (t.df1 + t.df2)).collect();
    df.sort_by(f64::total_cmp);
    progress("反卷积", 0, 1);
    let (mut v, shape, px) = load(&src.path)?;
    let optics = crate::ctf::Optics::of(d.optics.kv, d.optics.cs_mm, d.optics.amplitude);
    super::deconv::deconvolve(&mut v, shape, px, df[df.len() / 2], &optics);
    let path = output_path(d, &format!("{}_deconv", stem_suffix(&src.path, &d.stack)));
    output::write_mrc(&path, &v, shape, px, false)?;
    progress("反卷积", 1, 1);
    d.add_tomogram(Tomogram { path: path.clone(), kind: "deconvolved".into(), method: "wiener".into(), halves: None, ..src });
    d.save()?;
    Ok(Made { paths: vec![path], backend: "CPU".into(), ms: now.elapsed().as_secs_f64() * 1e3 })
}

/// All sections of a volume, [z][y][x], with its shape and pixel size.
fn load(path: &str) -> Result<(Vec<f32>, [usize; 3], f64)> {
    let img = open(path)?;
    let info = img.info();
    let (w, h, nz) = (info.width, info.height, info.size_z);
    let mut v = Vec::with_capacity(w * h * nz);
    for z in 0..nz {
        check_cancel()?;
        let (page, s) = info.locate(0, z, 0);
        let p = img.page(page)?;
        let p = &p[s.min(p.len() - 1)];
        v.extend((0..w * h).map(|i| p.data.get_f64(i) as f32));
    }
    Ok((v, [w, h, nz], output::pixel_a(&img)))
}

/// The tomogram a later step works on: the one given, else the latest full one.
fn source<'a>(d: &'a Dataset, path: Option<&str>) -> Result<&'a Tomogram> {
    match path {
        Some(p) => d.tomograms.iter().find(|t| t.path == p),
        None => d.tomograms.iter().rev().find(|t| t.kind == "full"),
    }
    .ok_or_else(|| anyhow::anyhow!("先完成重构"))
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenoiseStep {
    #[serde(default)]
    pub source: Option<String>,
    pub steps: usize,
}

pub fn denoise(dev: &burn::tensor::Device, d: &mut Dataset, req: &DenoiseStep, progress: Progress) -> Result<Made> {
    use crate::denoise_learn::{train, Learned, Mode, TrainConfig};
    let now = std::time::Instant::now();
    let full = source(d, req.source.as_deref())?.clone();
    let present = full.halves.clone().filter(|[e, o]| Path::new(e).exists() && Path::new(o).exists());
    let (shape, px, halves) = match present {
        Some([e, o]) => {
            let (ev, shape, px) = load(&e)?;
            let (od, s2, _) = load(&o)?;
            if s2 != shape {
                bail!("奇偶半断层尺寸不一致");
            }
            (shape, px, Some((ev, od)))
        }
        None => {
            let (_, shape, px) = load(&full.path)?;
            (shape, px, None)
        }
    };
    let [w, h, nz] = shape;
    let plane = w * h;
    // Training pairs: at most 64 sections spread over the depth.
    let picks: Vec<usize> = (0..nz).step_by((nz / 64).max(1)).collect();
    let (pairs, mode, inputs): (Vec<(Vec<f32>, Option<Vec<f32>>)>, Mode, Vec<Vec<f32>>) = match halves {
        Some((ev, od)) => {
            let mut pairs = Vec::new();
            for &z in &picks {
                let (a, b) = (ev[z * plane..(z + 1) * plane].to_vec(), od[z * plane..(z + 1) * plane].to_vec());
                pairs.push((a.clone(), Some(b.clone())));
                pairs.push((b, Some(a)));
            }
            (pairs, Mode::Frames, vec![ev, od])
        }
        None => {
            let (v, _, _) = load(&full.path)?;
            let pairs = picks.iter().filter(|&&z| z + 1 < nz).map(|&z| (v[z * plane..(z + 1) * plane].to_vec(), Some(v[(z + 1) * plane..(z + 2) * plane].to_vec()))).collect();
            (pairs, Mode::Neighbours, vec![v])
        }
    };
    crate::denoise_learn::CANCEL.store(false, Ordering::SeqCst);
    let cfg = TrainConfig { steps: req.steps.clamp(20, 20000), ..Default::default() };
    let (net, mean, std, loss) = train(dev, &pairs, w, h, mode, &cfg, |s, n, _| progress("Noise2Noise 训练", s, n))?;
    let model = Learned { net, mode, mean, std, loss, source: full.path.clone(), bin: 1 };
    let mut out = vec![0.0f32; plane * nz];
    for (k, v) in inputs.iter().enumerate() {
        for z in 0..nz {
            check_cancel()?;
            let r = model.apply(dev, &v[z * plane..(z + 1) * plane], w, h);
            for (o, x) in out[z * plane..(z + 1) * plane].iter_mut().zip(r) {
                *o += x / inputs.len() as f32;
            }
            progress("去噪", k * nz + z + 1, inputs.len() * nz);
        }
    }
    let path = output_path(d, &format!("{}_n2n", stem_suffix(&full.path, &d.stack)));
    output::write_mrc(&path, &out, shape, px, false)?;
    d.add_tomogram(Tomogram { path: path.clone(), kind: "denoised".into(), method: if mode == Mode::Frames { "n2n-halves" } else { "n2n-neighbours" }.into(), halves: None, ..full });
    d.save()?;
    Ok(Made { paths: vec![path], backend: "GPU".into(), ms: now.elapsed().as_secs_f64() * 1e3 })
}

/// The part of a tomogram's name after the stack's stem (`rec_b4_flip`).
fn stem_suffix(path: &str, stack: &str) -> String {
    let stem = Path::new(stack).file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let name = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("");
    name.strip_prefix(stem).map(|s| s.trim_start_matches('_').to_string()).unwrap_or_else(|| name.to_string())
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WedgeStep {
    #[serde(default)]
    pub source: Option<String>,
    /// Training settings; None = chosen here (the UI sends none: one
    /// button, the settings that did best in the held-out-tilt tests).
    #[serde(default)]
    pub cfg: Option<crate::wedge::TrainConfig>,
    /// Train on the tomogram alone even when it has even/odd halves.
    #[serde(default)]
    pub single: bool,
    /// Give up when training looks set to take longer than this, seconds
    /// (judged after the first half minute); the error is `TooSlow`.
    #[serde(default)]
    pub budget_s: Option<f64>,
}

/// `wedge` stopped because its training would overrun `budget_s`.
#[derive(Debug)]
pub struct TooSlow {
    /// The projected training time, seconds.
    pub projected_s: f64,
}

impl std::fmt::Display for TooSlow {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "缺失楔补偿预计要 {:.0} 分钟，超出时限", self.projected_s / 60.0)
    }
}

impl std::error::Error for TooSlow {}

/// Even/odd halves: cube 48, trained on the halves for two thirds of the
/// steps and with ICECREAM's equivariance term for the last third. It
/// did best in the held-out-tilt tests (EMPIAR-10045, all three folds)
/// and matched full ICECREAM on EMPIAR-10164 at under a third of its
/// time; about 25 min on an RTX 5080.
fn halves_config() -> crate::wedge::TrainConfig {
    crate::wedge::TrainConfig { cube: 48, iterations: 3, steps: 1000, equivariant: 2.0, equi_from: 0.67, ..Default::default() }
}

/// One tomogram: the IsoNet method with a σ1 pre-smooth (noise is not
/// filled into the wedge).
fn single_config() -> crate::wedge::TrainConfig {
    crate::wedge::TrainConfig { smooth: 1.0, ..Default::default() }
}

pub fn wedge(dev: &burn::tensor::Device, d: &mut Dataset, req: &WedgeStep, progress: Progress) -> Result<Made> {
    let now = std::time::Instant::now();
    let Some(a) = d.alignment.as_ref() else { bail!("先完成倾转对齐") };
    let used: Vec<f64> = a.views.iter().filter(|v| !v.excluded).map(|v| v.refined()).collect();
    let range = [used.iter().copied().fold(f64::INFINITY, f64::min) as f32, used.iter().copied().fold(f64::NEG_INFINITY, f64::max) as f32];
    let src = source(d, req.source.as_deref())?.clone();
    crate::wedge::CANCEL.store(false, Ordering::SeqCst);
    let started = std::time::Instant::now();
    let projected = std::sync::Mutex::new(None::<f64>);
    let tick = |p: crate::wedge::Progress| {
        if let (Some(budget), true) = (req.budget_s, p.iteration <= p.iterations) {
            let done = ((p.iteration.max(1) - 1) * p.steps + p.step) as f64 / (p.iterations * p.steps).max(1) as f64;
            let t = started.elapsed().as_secs_f64();
            if t > 30.0 && done > 0.0 && t / done > budget {
                *projected.lock().unwrap() = Some(t / done);
                crate::wedge::CANCEL.store(true, Ordering::SeqCst);
            }
        }
        if p.iteration > p.iterations {
            progress("缺失楔补偿：应用", p.step, p.steps)
        } else {
            progress(&format!("缺失楔补偿：第 {}/{} 轮", p.iteration, p.iterations), p.step, p.steps)
        }
    };
    // With even/odd halves one network denoises and fills the wedge
    // (Noise2Noise across the halves); else the IsoNet method on the one.
    let trained = (|| -> Result<_> { Ok(match (&src.halves, req.single) {
        (Some([even, odd]), false) => {
            let (a, shape, px) = load(even)?;
            let (b, ..) = load(odd)?;
            let tilts: Vec<f32> = used.iter().map(|&t| t as f32).collect();
            let cfg = req.cfg.clone().unwrap_or_else(halves_config);
            let method = if cfg.equivariant > 0.0 { "icecream" } else { "halves" };
            (crate::wedge::train_halves_and_apply(dev, &a, &b, shape, &tilts, &cfg, tick)?, shape, px, method)
        }
        _ => {
            let (v, shape, px) = load(&src.path)?;
            let cfg = req.cfg.clone().unwrap_or_else(single_config);
            (crate::wedge::train_and_apply(dev, &v, shape, range, &cfg, tick)?, shape, px, "isonet")
        }
    }) })();
    if let Some(projected_s) = *projected.lock().unwrap() {
        crate::wedge::CANCEL.store(false, Ordering::SeqCst);
        return Err(TooSlow { projected_s }.into());
    }
    let (out, shape, px, method) = trained?;
    if CANCEL.load(Ordering::SeqCst) {
        bail!("已取消");
    }
    let path = output_path(d, &format!("{}_wedge", stem_suffix(&src.path, &d.stack)));
    output::write_mrc(&path, &out, shape, px, false)?;
    d.add_tomogram(Tomogram { path: path.clone(), kind: "wedge".into(), method: method.into(), halves: None, ..src });
    d.save()?;
    Ok(Made { paths: vec![path], backend: "GPU".into(), ms: now.elapsed().as_secs_f64() * 1e3 })
}
