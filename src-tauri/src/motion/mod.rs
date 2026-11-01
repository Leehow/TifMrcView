//! Beam-induced movie motion correction (Pro), independent implementation
//! of the published global/patch-polynomial method (Zheng et al. 2017) and
//! critical-exposure weighting (Grant & Grigorieff 2015). EER grouping is
//! native and streaming; the Thermo wasm plugin shares the event decoder.
pub mod dose;
pub mod eer;
pub mod fourier;
pub mod input;
pub mod local;
pub mod output;
pub mod series;
pub mod sum;
pub mod sum_gpu;
#[cfg(test)]
mod tests;
pub mod tiles;
use crate::image::OpenImage;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
pub static CANCEL: AtomicBool = AtomicBool::new(false);
/// How strongly the global trajectory is smoothed (see `smooth`).
const SMOOTHING: f64 = 4.0;

/// A smooth trajectory through the measured shifts (Whittaker smoother):
/// minimises Σ wᵢ|sᵢ − dᵢ|² + λ Σ |sᵢ₋₁ − 2sᵢ + sᵢ₊₁|², weights wᵢ from the
/// correlation peaks (mean 1). Specimen drift is smooth in time; frame to
/// frame jumps of a fraction of a pixel are measurement noise on groups of
/// a few electrons per pixel, and summing along them blurs the image.
pub fn smooth(d: &[[f64; 2]], cc: &[f64], lambda: f64) -> Vec<[f64; 2]> {
    let n = d.len();
    if n < 3 {
        return d.to_vec();
    }
    let mean = cc.iter().map(|c| c.max(1e-3)).sum::<f64>() / n as f64;
    let w: Vec<f64> = cc.iter().map(|c| (c.max(1e-3) / mean).powi(2)).collect();
    // (W + λ DᵀD) s = W d, D the second-difference operator.
    let mut a = vec![vec![0.0; n]; n];
    for i in 0..n {
        a[i][i] += w[i];
    }
    for i in 1..n - 1 {
        let idx = [i - 1, i, i + 1];
        let coef = [1.0, -2.0, 1.0];
        for (p, cp) in idx.iter().zip(coef) {
            for (q, cq) in idx.iter().zip(coef) {
                a[*p][*q] += lambda * cp * cq;
            }
        }
    }
    let solve = |k: usize| -> Vec<f64> {
        let mut m = a.clone();
        let mut b: Vec<f64> = (0..n).map(|i| w[i] * d[i][k]).collect();
        for i in 0..n {
            let p = m[i][i];
            for j in i + 1..n {
                let f = m[j][i] / p;
                if f != 0.0 {
                    for c in i..n {
                        m[j][c] -= f * m[i][c];
                    }
                    b[j] -= f * b[i];
                }
            }
        }
        let mut x = vec![0.0; n];
        for i in (0..n).rev() {
            x[i] = (b[i] - (i + 1..n).map(|c| m[i][c] * x[c]).sum::<f64>()) / m[i][i];
        }
        x
    };
    let (x, y) = (solve(0), solve(1));
    (0..n).map(|i| [x[i], y[i]]).collect()
}

pub fn check_cancel() -> Result<()> {
    if CANCEL.load(Ordering::Relaxed) {
        bail!("已取消");
    }
    Ok(())
}
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
    crate::align::CANCEL.store(true, Ordering::SeqCst);
}
pub fn begin() {
    CANCEL.store(false, Ordering::SeqCst);
    crate::align::CANCEL.store(false, Ordering::SeqCst);
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Params {
    pub group: usize,
    pub upsample: usize,
    pub frame_axis: String,
    pub working_size: usize,
    pub local: bool,
    pub patches: usize,
    pub dose_weighted: bool,
    pub unweighted: bool,
    pub even_odd: bool,
    pub pixel_a: Option<f64>,
    pub voltage_kv: f64,
    pub voltage_auto: bool,
    pub dose_per_frame: Option<f64>,
    pub total_dose: Option<f64>,
    pub pre_exposure: f64,
    pub gain_path: Option<String>,
    pub gain_rotate: usize,
    pub gain_flip_x: bool,
    pub gain_flip_y: bool,
    pub backend: String,
    pub benchmark: bool,
    pub diagnostics: bool,
    /// ImageSize of the mdoc that `pixel_a` came from (frames of another
    /// size get it rescaled).
    #[serde(skip)]
    pub mdoc_size: Option<[usize; 2]>,
}
impl Default for Params {
    fn default() -> Self {
        Self {
            group: 0,
            upsample: 1,
            frame_axis: "auto".into(),
            working_size: 1024,
            local: true,
            patches: 5,
            dose_weighted: true,
            unweighted: true,
            even_odd: true,
            pixel_a: None,
            voltage_kv: 300.0,
            voltage_auto: true,
            dose_per_frame: None,
            total_dose: None,
            pre_exposure: 0.0,
            gain_path: None,
            gain_rotate: 0,
            gain_flip_x: false,
            gain_flip_y: false,
            backend: "auto".into(),
            benchmark: false,
            diagnostics: false,
            mdoc_size: None,
        }
    }
}
impl Params {
    pub fn validate(&self) -> Result<()> {
        if ![1, 2].contains(&self.upsample)
            || self.group > 256
            || !(128..=2048).contains(&self.working_size)
            || !(2..=10).contains(&self.patches)
            || self.gain_rotate > 3
            || !self.pre_exposure.is_finite()
            || self.pre_exposure < 0.0
            || !self.voltage_kv.is_finite()
            || self.voltage_kv <= 0.0
            || !matches!(self.backend.as_str(), "auto" | "cpu" | "gpu")
            || self.pixel_a.is_some_and(|v| !v.is_finite() || v <= 0.0)
            || self
                .dose_per_frame
                .is_some_and(|v| !v.is_finite() || v <= 0.0)
            || self.total_dose.is_some_and(|v| !v.is_finite() || v <= 0.0)
            || (self.total_dose.is_some() && self.dose_per_frame.is_some())
        {
            bail!("运动校正参数无效；剂量应只指定总剂量或每分组帧剂量之一");
        }
        Ok(())
    }
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReq {
    pub token: Option<u64>,
    pub path: Option<String>,
    pub mdoc: Option<String>,
    pub output: String,
    #[serde(default)]
    pub c: usize,
    #[serde(default)]
    pub z: usize,
    #[serde(default)]
    pub t: usize,
    pub params: Params,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MotionResult {
    pub path: String,
    pub unweighted: Option<String>,
    pub even: Option<String>,
    pub odd: Option<String>,
    pub width: usize,
    pub height: usize,
    pub frames: usize,
    pub raw_fractions: usize,
    pub pixel_a: f64,
    pub global: Vec<[f64; 2]>,
    pub cc: Vec<f64>,
    pub local: local::Local,
    pub total_drift: f64,
    pub mean_local_motion: f64,
    pub decode_ms: f64,
    pub prepare_ms: f64,
    pub group_counts: Vec<usize>,
    pub alignment_ms: f64,
    pub sum_ms: f64,
    pub total_ms: f64,
    pub backend: String,
    pub cpu_ms: Option<f64>,
    pub gpu_ms: Option<f64>,
    pub dose_weighted: bool,
    pub warnings: Vec<String>,
}

pub fn run(
    req: &RunReq,
    img: Option<Arc<OpenImage>>,
    progress: &(dyn Fn(&str, usize, usize) + Sync),
) -> Result<MotionResult> {
    let path = req
        .path
        .as_deref()
        .or_else(|| img.as_ref().map(|i| i.info().path.as_str()))
        .ok_or_else(|| anyhow::anyhow!("需要打开电影或选择文件"))?
        .to_string();
    let mut p = req.params.clone();
    let mut warnings = vec![];
    let automatic_mdoc = if req.mdoc.is_none() {
        series::find_mdoc(&path)
    } else {
        None
    };
    if let Some(mdoc) = req.mdoc.as_ref().or(automatic_mdoc.as_ref()) {
        let doc = series::parse(&std::fs::read_to_string(mdoc)?)?;
        if let Some((view, pre)) = doc.for_movie(&path) {
            if p.pixel_a.is_none() {
                p.pixel_a = view.pixel_a.or(doc.pixel_a);
                p.mdoc_size = doc.image_size;
            }
            if p.total_dose.is_none() && p.dose_per_frame.is_none() {
                p.total_dose = view.dose.filter(|v| *v > 0.0);
            }
            p.pre_exposure += pre;
            if doc.views.iter().any(|v| v.dose.unwrap_or(0.0) <= 0.0) {
                warnings.push(
                    "mdoc 存在零/未知剂量，无法恢复已采集电影的累计前曝光；请核对手动前曝光值"
                        .into(),
                );
            }
            if p.voltage_auto {
                p.voltage_kv = doc.kv.unwrap_or(p.voltage_kv);
            }
        } else {
            warnings.push("mdoc 中未匹配到该电影，使用手动参数".into());
        }
    }
    run_movie(
        &path,
        img,
        &req.output,
        req.c,
        req.z,
        req.t,
        &p,
        warnings,
        progress,
    )
}
#[allow(clippy::too_many_arguments)]
fn run_movie(
    path: &str,
    img: Option<Arc<OpenImage>>,
    target: &str,
    c: usize,
    z: usize,
    t: usize,
    p: &Params,
    mut warnings: Vec<String>,
    progress: &(dyn Fn(&str, usize, usize) + Sync),
) -> Result<MotionResult> {
    p.validate()?;
    check_cancel()?;
    let start = std::time::Instant::now();
    for out in [
        target.to_string(),
        output::sibling(target, "unweighted"),
        output::sibling(target, "even"),
        output::sibling(target, "odd"),
    ] {
        output::protect(path, &out)?;
        if let Some(g) = &p.gain_path {
            output::protect(g, &out)?;
        }
    }
    let movie = input::Movie::open(path, img, p, c, z, t)?;
    if p.dose_weighted
        && (p.total_dose.is_some() || p.dose_per_frame.is_some())
        && p.pixel_a.or(movie.pixel_a()).is_none()
    {
        bail!("剂量加权需要真实像素尺寸（Å），请填写或关联 mdoc");
    }
    let from_mdoc = p.pixel_a.map(|v| {
        let (w, h, _) = movie.shape();
        let doc = series::Doc {
            image_size: p.mdoc_size,
            ..Default::default()
        };
        doc.frame_pixel(v, (w, h))
    });
    let pixel = from_mdoc
        .or(movie.pixel_a())
        .map(|v| {
            if matches!(movie, input::Movie::Eer(_)) {
                v / p.upsample as f64
            } else {
                v
            }
        })
        .unwrap_or_else(|| {
            warnings.push("像素标定未知，输出暂以 1 Å/px 标记".into());
            1.0
        });
    let parent = Path::new(target)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let sp = input::Spool::prepare(&movie, p, parent, progress)?;
    let raw = sp.counts.iter().sum::<usize>();
    let alignment_start = std::time::Instant::now();
    let g = crate::align::movie_planes(&sp.frames, sp.w / sp.bin, sp.h / sp.bin, &|done, total| {
        progress("全局对齐", done, total)
    });
    let (global, local, cc) = match g {
        Ok(g) => {
            let measured: Vec<_> = g
                .shifts
                .iter()
                .map(|d| d.map(|v| v * sp.bin as f64))
                .collect();
            let global = smooth(&measured, &g.cc, SMOOTHING);
            let local = local::estimate(&sp, &global, p, progress)?;
            (global, local, g.cc)
        }
        // Frames with no measurable signal (an empty hole, a blank tilt):
        // any peak would be noise, so the frames are summed as recorded.
        Err(e) if e.downcast_ref::<crate::align::NoSignal>().is_some() => {
            warnings.push(format!("{e}；按未校正直接求和"));
            let local = local::Local {
                model: local::Model::zero(&sp.times),
                patches: Vec::new(),
                global_residual: 0.0,
                local_residual: 0.0,
                rejected: 0,
                backend: String::new(),
                cpu_pair_ms: 0.0,
                gpu_pair_ms: None,
            };
            (vec![[0.0; 2]; sp.len()], local, vec![0.0; sp.len()])
        }
        Err(e) => return Err(e),
    };
    let alignment_ms = alignment_start.elapsed().as_secs_f64() * 1000.0;
    let fraction_dose = p
        .total_dose
        .map(|d| d / raw as f64)
        .or_else(|| p.dose_per_frame.map(|d| d / sp.counts[0] as f64));
    let exposure = if p.dose_weighted {
        if let Some(d) = fraction_dose {
            let _ = dose::critical(1.0, p.voltage_kv)?;
            Some(sum::exposure(&sp.counts, d, p.pre_exposure))
        } else {
            warnings.push("ExposureDose 为 0 或缺失；未应用剂量加权".into());
            None
        }
    } else {
        None
    };
    let timer = std::time::Instant::now();
    let mut cpu_ms = None;
    let mut gpu_ms = None;
    let (sums, backend) = if p.benchmark {
        let t = std::time::Instant::now();
        let cpu = sum::cpu(
            &sp,
            &global,
            &local.model,
            exposure.as_deref(),
            pixel,
            p.voltage_kv,
            progress,
        )?;
        let tc = t.elapsed().as_secs_f64() * 1000.0;
        cpu_ms = Some(tc);
        if let Some((dev, name)) = crate::ctf_gpu::device() {
            let t = std::time::Instant::now();
            match crate::ctf_gpu::guarded(|| {
                sum_gpu::run(
                    dev,
                    &sp,
                    &global,
                    &local.model,
                    exposure.as_deref(),
                    pixel,
                    p.voltage_kv,
                    progress,
                )
            }) {
                Ok(Ok(gpu)) => {
                    let tg = t.elapsed().as_secs_f64() * 1000.0;
                    gpu_ms = Some(tg);
                    let diff = gpu
                        .unweighted
                        .iter()
                        .zip(&cpu.unweighted)
                        .map(|(a, b)| (*a as f64 - *b as f64).powi(2))
                        .sum::<f64>();
                    let norm = cpu
                        .unweighted
                        .iter()
                        .map(|v| (*v as f64).powi(2))
                        .sum::<f64>();
                    let rel = (diff / norm.max(1e-20)).sqrt();
                    warnings.push(format!("CPU/GPU 求和相对 L2={rel:.3e}"));
                    if rel > 1e-3 {
                        bail!("GPU 求和与 CPU 参考不一致：{rel}");
                    }
                    if tg < tc {
                        (gpu, (*name).to_string())
                    } else {
                        (cpu, "CPU".into())
                    }
                }
                r => {
                    check_cancel()?;
                    warnings.push(format!(
                        "GPU 回退：{}",
                        match r {
                            Ok(Err(e)) | Err(e) => e.to_string(),
                            _ => unreachable!(),
                        }
                    ));
                    (cpu, "CPU".into())
                }
            }
        } else {
            warnings.push("无可用 GPU，测速仅 CPU".into());
            (cpu, "CPU".into())
        }
    } else {
        let gpu = if p.backend != "cpu" {
            crate::ctf_gpu::device()
        } else {
            None
        };
        // GPU first; the CPU path is the fallback and the reference.
        let selected_gpu = p.backend == "gpu" || (p.backend == "auto" && gpu.is_some());
        if selected_gpu {
            if let Some((dev, name)) = gpu {
                match crate::ctf_gpu::guarded(|| {
                    sum_gpu::run(
                        dev,
                        &sp,
                        &global,
                        &local.model,
                        exposure.as_deref(),
                        pixel,
                        p.voltage_kv,
                        progress,
                    )
                }) {
                    Ok(Ok(s)) => (s, (*name).into()),
                    r => {
                        check_cancel()?;
                        if p.backend == "gpu" {
                            return match r {
                                Ok(Err(e)) | Err(e) => Err(e),
                                _ => unreachable!(),
                            };
                        }
                        warnings.push("GPU 求和失败，回退 CPU".into());
                        (
                            sum::cpu(
                                &sp,
                                &global,
                                &local.model,
                                exposure.as_deref(),
                                pixel,
                                p.voltage_kv,
                                progress,
                            )?,
                            "CPU".into(),
                        )
                    }
                }
            } else {
                bail!("无可用 GPU");
            }
        } else {
            (
                sum::cpu(
                    &sp,
                    &global,
                    &local.model,
                    exposure.as_deref(),
                    pixel,
                    p.voltage_kv,
                    progress,
                )?,
                "CPU".into(),
            )
        }
    };
    let sum_ms = timer.elapsed().as_secs_f64() * 1000.0;
    if p.diagnostics {
        output::diagnostics(target, &sp.original, &sums.unweighted, sp.w, sp.h)?;
    }
    let weighted = sums.weighted.is_some();
    output::single(
        target,
        sums.weighted.as_deref().unwrap_or(&sums.unweighted),
        sp.w,
        sp.h,
        pixel,
    )?;
    let unweighted = if p.unweighted && weighted {
        let path = output::sibling(target, "unweighted");
        output::single(&path, &sums.unweighted, sp.w, sp.h, pixel)?;
        Some(path)
    } else if !weighted {
        Some(target.into())
    } else {
        None
    };
    let (even, odd) = if p.even_odd {
        let a = output::sibling(target, "even");
        let b = output::sibling(target, "odd");
        output::single(&a, &sums.even, sp.w, sp.h, pixel)?;
        output::single(&b, &sums.odd, sp.w, sp.h, pixel)?;
        (Some(a), Some(b))
    } else {
        (None, None)
    };
    let total_drift = global
        .windows(2)
        .map(|v| (v[1][0] - v[0][0]).hypot(v[1][1] - v[0][1]))
        .sum();
    let mean_local_motion =
        local.patches.iter().map(|p| p.magnitude).sum::<f64>() / local.patches.len().max(1) as f64;
    let result = MotionResult {
        path: target.into(),
        unweighted,
        even,
        odd,
        width: sp.w,
        height: sp.h,
        frames: sp.len(),
        raw_fractions: raw,
        pixel_a: pixel,
        global,
        cc,
        local,
        total_drift,
        mean_local_motion,
        decode_ms: sp.decode_ms,
        prepare_ms: sp.prepare_ms,
        group_counts: sp.counts.clone(),
        alignment_ms,
        sum_ms,
        total_ms: start.elapsed().as_secs_f64() * 1000.0,
        backend,
        cpu_ms,
        gpu_ms,
        dose_weighted: weighted,
        warnings,
    };
    output::text(
        Path::new(&format!("{target}.motion.json")),
        &serde_json::to_string_pretty(&result)?,
    )?;
    Ok(result)
}
