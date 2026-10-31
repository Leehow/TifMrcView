//! Cryo-ET single-axis alignment and reconstruction (Pro). Published
//! methods: patch tracking / bundle adjustment (Kremer, Mastronarde &
//! McIntosh, J Struct Biol 116, 1996; Mastronarde & Held, 2017), Fourier
//! ramp WBP and simultaneous iterative reconstruction (Gilbert, 1972).
//! This is an independent Rust implementation, not translated software.
//! See geometry for axis conventions, model for gauges, recon for A/Aᵀ.

pub mod correlation_gpu;
pub mod geometry;
pub mod model;
pub mod output;
pub mod recon;
pub mod recon_gpu;
#[cfg(test)]
mod tests;
pub mod track;

use crate::image::OpenImage;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use track::{PatchConfig, Tracks};

pub static CANCEL: AtomicBool = AtomicBool::new(false);
pub fn check_cancel() -> Result<()> {
    if CANCEL.load(Ordering::Relaxed) {
        bail!("已取消");
    }
    Ok(())
}
pub fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    (v[(v.len() - 1) / 2] + v[v.len() / 2]) / 2.0
}

pub struct Stack {
    pub w: usize,
    pub h: usize,
    pub views: Vec<Vec<f32>>,
    pub angles: Vec<f64>,
    pub file_order: Vec<usize>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignReq {
    pub token: u64,
    #[serde(default)]
    pub c: usize,
    #[serde(default)]
    pub t: usize,
    #[serde(default)]
    pub angles: Vec<f64>,
    pub start: Option<f64>,
    pub step: Option<f64>,
    pub axis: Option<f64>,
    pub bin: usize,
    pub sweep: bool,
    pub patch: Option<PatchConfig>,
    #[serde(default)]
    pub excluded: Vec<usize>,
    /// Shared rotation / tilt / magnification groups; None picks them from the series length.
    #[serde(default)]
    pub groups: Option<model::Groups>,
    /// Reuse measured contours when only exclusions change.
    pub previous: Option<Alignment>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub file_index: usize,
    pub angle: f64,
    pub residual: f64,
    pub shift: [f64; 2],
    pub mag: f64,
    /// In-plane rotation added to the axis (°).
    #[serde(default)]
    pub rotation: f64,
    /// Refined tilt angle (°), used for reconstruction.
    #[serde(default)]
    pub tilt: f64,
    pub cc: f64,
    pub contours: usize,
    pub flagged: bool,
    pub excluded: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alignment {
    pub width: usize,
    pub height: usize,
    pub bin: usize,
    pub axis: f64,
    pub patch: PatchConfig,
    pub residual: f64,
    pub quality: String,
    pub views: Vec<View>,
    pub tracks: Tracks,
    pub ms: f64,
}

impl View {
    /// The refined tilt angle (alignments saved before refinement have none).
    pub fn refined(&self) -> f64 {
        if self.tilt != 0.0 { self.tilt } else { self.angle }
    }
}

pub fn quality(r: f64) -> &'static str {
    if r < 3.0 {
        "<3 px"
    } else if r < 4.0 {
        "3–4 px"
    } else if r < 7.0 {
        "4–7 px"
    } else if r < 10.0 {
        "7–10 px"
    } else if r < 15.0 {
        "10–15 px"
    } else {
        ">15 px"
    }
}
pub fn validate_bin(bin: usize) -> Result<()> {
    if ![1, 2, 4, 8].contains(&bin) {
        bail!("binning 必须为 1/2/4/8");
    }
    Ok(())
}

pub fn read_stack(
    img: &Arc<OpenImage>,
    c: usize,
    t: usize,
    bin: usize,
    angles: &[f64],
) -> Result<Stack> {
    validate_bin(bin)?;
    let info = img.info();
    if c >= info.size_c
        || t >= info.size_t
        || angles.len() != info.size_z
        || angles.len() < 3
        || angles.iter().any(|a| !a.is_finite() || a.abs() >= 89.5)
    {
        bail!("层数、通道或倾转角无效（倾角需在 ±89.5° 内）");
    }
    let (w, h) = (info.width / bin, info.height / bin);
    if w.min(h) < 16 || w.saturating_mul(h).saturating_mul(angles.len()) > 1 << 29 {
        bail!("计算叠层过大或过小，请调整 binning");
    }
    let mut order: Vec<usize> = (0..angles.len()).collect();
    order.sort_by(|&a, &b| angles[a].total_cmp(&angles[b]));
    let mut views = Vec::with_capacity(order.len());
    for &z in &order {
        check_cancel()?;
        let (page, sample) = info.locate(c, z, t);
        let p = img.page(page)?;
        let p = &p[sample.min(p.len() - 1)];
        let mut v = vec![0.0; w * h];
        use rayon::prelude::*;
        v.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let mut sum = 0.0;
                for dy in 0..bin {
                    for dx in 0..bin {
                        sum += p.data.get_f64(
                            ((info.height - h * bin) / 2 + y * bin + dy) * p.w
                                + (info.width - w * bin) / 2
                                + x * bin
                                + dx,
                        );
                    }
                }
                *o = (sum / (bin * bin) as f64) as f32;
            }
        });
        if v.iter().any(|v| !v.is_finite()) {
            bail!("输入含 NaN/Inf，无法对齐");
        }
        views.push(v);
    }
    Ok(Stack {
        w,
        h,
        views,
        angles: order.iter().map(|&i| angles[i]).collect(),
        file_order: order,
    })
}

fn filtered_tracks(tr: &Tracks, order: &[usize], excluded: &[usize]) -> Result<Tracks> {
    if tr.points > 4096
        || tr.cc.len() != order.len()
        || tr.counts.len() != order.len()
        || tr.observations.iter().any(|o| {
            o.point >= tr.points
                || o.view >= order.len()
                || !o.cc.is_finite()
                || !(0.0..=1.0).contains(&o.cc)
                || o.xy.iter().any(|v| !v.is_finite())
        })
    {
        bail!("轮廓数据无效");
    }
    let mut tr = tr.clone();
    tr.observations
        .retain(|o| !excluded.contains(&order[o.view]));
    let mut lengths = vec![0usize; tr.points];
    for o in &tr.observations {
        lengths[o.point] += 1;
    }
    let minimum = (order.len() / 4).max(3);
    tr.observations.retain(|o| lengths[o.point] >= minimum);
    let mut per_view = vec![0usize; order.len()];
    let mut active = vec![false; tr.points];
    for o in &tr.observations {
        per_view[o.view] += 1;
        active[o.point] = true;
    }
    let included = order.iter().filter(|i| !excluded.contains(i)).count();
    let covered = per_view
        .iter()
        .enumerate()
        .filter(|(i, n)| !excluded.contains(&order[*i]) && **n >= 4)
        .count();
    if active.iter().filter(|&&v| v).count() < 4 || 4 * covered < 3 * included {
        bail!("长轮廓或视图覆盖不足，无法比较该 patch 配置");
    }
    Ok(tr)
}

pub fn align(
    img: &Arc<OpenImage>,
    req: &AlignReq,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Alignment> {
    let now = std::time::Instant::now();
    let manual = !req.angles.is_empty() || (req.start.is_some() && req.step.is_some());
    let meta = match geometry::metadata(&img.info().path, img.info().size_z) {
        Ok(meta) => meta,
        Err(_) if manual => geometry::Metadata::default(),
        Err(e) => return Err(e),
    };
    let angles = if !req.angles.is_empty() {
        req.angles.clone()
    } else if let (Some(start), Some(step)) = (req.start, req.step) {
        (0..img.info().size_z)
            .map(|i| start + step * i as f64)
            .collect()
    } else {
        meta.angles
    };
    let axis = req.axis.or(meta.axis).unwrap_or(90.0);
    if !axis.is_finite() {
        bail!("倾转轴角无效");
    }
    let bin = if req.bin == 0 {
        auto_bin(img.info().width, img.info().height)
    } else {
        req.bin
    };
    let st = read_stack(img, req.c, req.t, bin, &angles)?;
    let groups = req.groups.unwrap_or_else(|| model::Groups::default_for(st.angles.len()));
    if st
        .file_order
        .iter()
        .filter(|i| !req.excluded.contains(i))
        .count()
        < 3
    {
        bail!("至少保留三个倾角");
    }
    let (mut chosen, mut tracks, mut solution);
    if let Some(prev) = &req.previous {
        if prev.bin != bin
            || prev.width != img.info().width
            || prev.height != img.info().height
            || prev.views.len() != st.angles.len()
            || prev
                .views
                .iter()
                .zip(&st.file_order)
                .any(|(v, &i)| v.file_index != i || (v.angle - angles[i]).abs() > 1e-8)
        {
            bail!("原对齐与输入几何不一致，请重新对齐");
        }
        chosen = prev.patch;
        tracks = prev.tracks.clone();
        solution = model::solve(
            &filtered_tracks(&tracks, &st.file_order, &req.excluded)?,
            &st.angles,
            prev.axis,
            groups,
        )?;
    } else {
        chosen = req
            .patch
            .unwrap_or_else(|| PatchConfig::default_for(st.w, st.h));
        // Starting axes: the one asked for; else the mdoc's under both
        // conventions in use (they differ by a mirror, see geometry); else a
        // coarse turn through 180°. The lowest residual wins.
        let starts: Vec<f64> = match (req.axis, meta.axis) {
            (Some(a), _) => vec![a],
            (None, Some(a)) if req.sweep => vec![a, -a],
            (None, Some(a)) => vec![a],
            (None, None) if req.sweep => (0..12).map(|i| i as f64 * 15.0).collect(),
            (None, None) => vec![axis],
        };
        let mut best: Option<(Tracks, model::Solution)> = None;
        for start in starts {
            check_cancel()?;
            match track_and_solve(&st, start, chosen, &req.excluded, groups, req.sweep, progress) {
                Ok(r) if best.as_ref().is_none_or(|b| r.1.residual < b.1.residual) => best = Some(r),
                Ok(_) => {}
                Err(e) if CANCEL.load(Ordering::Relaxed) => return Err(e),
                Err(_) => {}
            }
        }
        (tracks, solution) = best.ok_or_else(|| anyhow::anyhow!("各起始轴角都无法对齐，请检查倾转角或手动给出轴角"))?;
        if req.sweep {
            let fixed_axis = solution.axis;
            for patch in PatchConfig::candidates(st.w, st.h) {
                check_cancel()?;
                let tr = match track::track(&st, fixed_axis, patch, progress) {
                    Ok(tr) => tr,
                    Err(_) if !CANCEL.load(Ordering::Relaxed) => continue,
                    Err(e) => return Err(e),
                };
                let Ok(fit) = filtered_tracks(&tr, &st.file_order, &req.excluded) else {
                    continue;
                };
                if fit.observations.len() < 12 {
                    continue;
                }
                let s = model::solve(&fit, &st.angles, fixed_axis, groups)?;
                if s.residual < solution.residual {
                    solution = s;
                    chosen = patch;
                    tracks = tr;
                }
            }
        }
    }
    let med = median(
        &solution
            .per_view
            .iter()
            .enumerate()
            .filter(|(i, _)| !req.excluded.contains(&st.file_order[*i]))
            .map(|(_, v)| *v)
            .collect::<Vec<_>>(),
    );
    let med_count = median(&tracks.counts.iter().map(|&n| n as f64).collect::<Vec<_>>());
    let med_cc = median(&tracks.cc);
    let views = (0..st.angles.len())
        .map(|i| View {
            file_index: st.file_order[i],
            angle: st.angles[i],
            residual: solution.per_view[i] * bin as f64,
            shift: solution.shifts[i].map(|v| v * bin as f64),
            mag: solution.magnifications[i],
            rotation: solution.rotations[i],
            tilt: st.angles[i] + solution.tilts[i],
            cc: tracks.cc[i],
            contours: tracks.counts[i],
            flagged: solution.per_view[i] > (3.0 * med).max(0.5)
                || (tracks.counts[i] as f64) < (med_count * 0.25).max(4.0)
                || tracks.cc[i] < med_cc * 0.4,
            excluded: req.excluded.contains(&st.file_order[i]),
        })
        .collect();
    let residual = solution.residual * bin as f64;
    Ok(Alignment {
        width: img.info().width,
        height: img.info().height,
        bin,
        axis: solution.axis,
        patch: chosen,
        residual,
        quality: quality(residual).into(),
        views,
        tracks,
        ms: now.elapsed().as_secs_f64() * 1e3,
    })
}

/// Re-measure at the updated axis: compression depends on that axis, so
/// profiling one frozen contour set alone can preserve initial-angle bias.
pub fn track_and_solve(
    st: &Stack,
    axis: f64,
    patch: PatchConfig,
    excluded: &[usize],
    groups: model::Groups,
    automatic: bool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<(Tracks, model::Solution)> {
    let mut tr = track::track(st, axis, patch, progress)?;
    let fit = filtered_tracks(&tr, &st.file_order, excluded)?;
    let mut s = if automatic {
        model::sweep(&fit, &st.angles, axis, groups)?
    } else {
        model::solve(&fit, &st.angles, axis, groups)?
    };
    if automatic && (s.axis - axis).abs() > 0.2 {
        let current = s.axis;
        tr = track::track(st, current, patch, progress)?;
        let fit = filtered_tracks(&tr, &st.file_order, excluded)?;
        s = model::refine(&fit, &st.angles, current, groups)?;
    }
    Ok((tr, s))
}

/// Closest supported bin to a 1024-pixel working short side; zero is UI auto.
pub fn auto_bin(w: usize, h: usize) -> usize {
    [1usize, 2, 4, 8]
        .into_iter()
        .min_by(|&a, &b| {
            ((w.min(h) as f64 / a as f64 / 1024.0).ln().abs())
                .total_cmp(&((w.min(h) as f64 / b as f64 / 1024.0).ln().abs()))
        })
        .unwrap()
}
