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
}

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
    };
    let tag = format!("{method}_b{}{}", req.bin, if flip.is_some() { "_flip" } else { "" });
    let mut made: Vec<Tomogram> = Vec::new();
    let mut backend = String::new();
    for (kind, src) in sources {
        check_cancel()?;
        let img = open(&src)?;
        let px = pixel_a(d, &img);
        output::validate(&img, &a, req.bin)?;
        let stage = format!("重构 {}", match kind { "even" => "偶数帧半", "odd" => "奇数帧半", _ => "全部" });
        let (stack, w, h, angles) = output::aligned_flipped(&img, &a, req.bin, 0, 0, flip.as_ref().map(|c| (c, &d.optics, px)), &|i, n| progress(&format!("{stage}：对齐叠层"), i, n))?;
        let g = recon::Geometry::new(w, req.params.thickness, &angles, req.params.z_offset)?;
        let views = recon::Views { data: &stack, w, h, views: angles.len() };
        let path = output_path(d, &if kind == "full" { tag.clone() } else { format!("{tag}_{kind}") });
        let mut out = output::VolumeFile::create(&path, [w, h, req.params.thickness], px * req.bin as f64)?;
        backend = recon::reconstruct(&views, &g, &req.params, &mut |y0, n, slab| out.write_rows(y0, n, &slab), &|i, n| progress(&stage, i, n))?;
        out.finish()?;
        made.push(Tomogram {
            path,
            kind: kind.into(),
            bin: req.bin,
            thickness: req.params.thickness,
            method: format!("{:?}", req.params.method).to_lowercase(),
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
    params.pixel_a = pixel_a(d, &img);
    d.optics.kv = params.voltage_kv;
    d.optics.cs_mm = params.cs_mm;
    d.optics.amplitude = params.amp_contrast;
    let fits = super::ctf::fit_series(&img, d, &params, n, &|i, n| progress("CTF 拟合", i, n))?;
    d.ctf = Some(fits.clone());
    d.save()?;
    Ok(fits)
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
}

/// Even/odd halves: cube 48 did best in the held-out-tilt tests
/// (EMPIAR-10045); about 20 min on an RTX 5080.
fn halves_config() -> crate::wedge::TrainConfig {
    crate::wedge::TrainConfig { cube: 48, iterations: 3, steps: 1000, ..Default::default() }
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
    let tick = |p: crate::wedge::Progress| {
        if p.iteration > p.iterations {
            progress("缺失楔补偿：应用", p.step, p.steps)
        } else {
            progress(&format!("缺失楔补偿：第 {}/{} 轮", p.iteration, p.iterations), p.step, p.steps)
        }
    };
    // With even/odd halves one network denoises and fills the wedge
    // (Noise2Noise across the halves); else the IsoNet method on the one.
    let (out, shape, px, method) = match (&src.halves, req.single) {
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
    };
    if CANCEL.load(Ordering::SeqCst) {
        bail!("已取消");
    }
    let path = output_path(d, &format!("{}_wedge", stem_suffix(&src.path, &d.stack)));
    output::write_mrc(&path, &out, shape, px, false)?;
    d.add_tomogram(Tomogram { path: path.clone(), kind: "wedge".into(), method: method.into(), halves: None, ..src });
    d.save()?;
    Ok(Made { paths: vec![path], backend: "GPU".into(), ms: now.elapsed().as_secs_f64() * 1e3 })
}
