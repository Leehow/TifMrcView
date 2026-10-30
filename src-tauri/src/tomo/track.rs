//! Native patch contours following neighbouring tilts outward from zero.
//! The reference patch is compressed perpendicular to the axis by the
//! ratio of cosines before correlation (published patch-tracking method,
//! Mastronarde / Kremer et al.). No external alignment code is used.

use super::{check_cancel, median, Stack};
use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PatchConfig {
    pub nx: usize,
    pub ny: usize,
    pub size: usize,
}

impl PatchConfig {
    pub fn validate(self, w: usize, h: usize) -> Result<Self> {
        if self.nx == 0
            || self.ny == 0
            || self.nx * self.ny < 4
            || self.nx > 128
            || self.ny > 128
            || self.size < 16
            || self.size > w / self.nx
            || self.size > h / self.ny
        {
            bail!("patch 大小必须 ≤ X/Y 网格间距，至少四个 patch");
        }
        Ok(self)
    }
    pub fn default_for(w: usize, h: usize) -> Self {
        let size = 80.min(w.min(h) / 2).max(16);
        Self {
            nx: ((w as f64 / 100.0).round() as usize).max(2).min(w / size),
            ny: ((h as f64 / 100.0).round() as usize).max(2).min(h / size),
            size,
        }
    }
    pub fn candidates(w: usize, h: usize) -> Vec<Self> {
        let mut out = vec![Self::default_for(w, h)];
        for (spacing, size) in [(170, 150), (128, 100), (128, 128), (100, 80), (85, 60)] {
            let p = Self {
                nx: (w / spacing).max(1),
                ny: (h / spacing).max(1),
                size,
            };
            if p.validate(w, h).is_ok() && !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub view: usize,
    pub point: usize,
    pub xy: [f64; 2],
    pub cc: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tracks {
    pub observations: Vec<Observation>,
    pub points: usize,
    pub cc: Vec<f64>,
    pub counts: Vec<usize>,
    #[serde(default)]
    pub ms: f64,
    #[serde(default)]
    pub cpu_pair_ms: f64,
    #[serde(default)]
    pub gpu_pair_ms: Option<f64>,
    #[serde(default)]
    pub backend: String,
}

pub fn sample(v: &[f32], w: usize, h: usize, x: f64, y: f64) -> f32 {
    if x < 0.0 || y < 0.0 || x > (w - 1) as f64 || y > (h - 1) as f64 {
        return 0.0;
    }
    let (ix, iy) = (x.floor() as usize, y.floor() as usize);
    let (fx, fy) = ((x - ix as f64) as f32, (y - iy as f64) as f32);
    let (jx, jy) = ((ix + 1).min(w - 1), (iy + 1).min(h - 1));
    (1.0 - fy) * ((1.0 - fx) * v[iy * w + ix] + fx * v[iy * w + jx])
        + fy * ((1.0 - fx) * v[jy * w + ix] + fx * v[jy * w + jx])
}

fn stretch(d: [f64; 2], ratio: f64, axis: f64) -> [f64; 2] {
    let (s, c) = (-axis).to_radians().sin_cos();
    let u = -s * d[0] + c * d[1];
    [d[0] - s * (ratio - 1.0) * u, d[1] + c * (ratio - 1.0) * u]
}

// Mean-fill clipped patches before tapering; black edges bias the peak.
fn patch_sample(v: &[f32], w: usize, h: usize, x: f64, y: f64) -> f32 {
    if x < 0.0 || y < 0.0 || x > (w - 1) as f64 || y > (h - 1) as f64 {
        f32::NAN
    } else {
        sample(v, w, h, x, y)
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_pair(
    st: &Stack,
    a: usize,
    b: usize,
    p: [f64; 2],
    q: [f64; 2],
    side: f64,
    n: usize,
    axis: f64,
    max: f64,
) -> (Vec<f32>, Vec<f32>, f64) {
    let ratio = st.angles[b].to_radians().cos() / st.angles[a].to_radians().cos();
    let step = side / n as f64;
    let mut va = vec![0.0; n * n];
    let mut vb = vec![0.0; n * n];
    for i in 0..n * n {
        let d = [
            ((i % n) as f64 - (n - 1) as f64 / 2.0) * step,
            ((i / n) as f64 - (n - 1) as f64 / 2.0) * step,
        ];
        let da = stretch(d, 1.0 / ratio, axis);
        va[i] = patch_sample(&st.views[a], st.w, st.h, p[0] + da[0], p[1] + da[1]);
        vb[i] = patch_sample(&st.views[b], st.w, st.h, q[0] + d[0], q[1] + d[1]);
    }
    for v in [&mut va, &mut vb] {
        let count = v.iter().filter(|v| v.is_finite()).count();
        let mean = v
            .iter()
            .filter(|v| v.is_finite())
            .map(|&v| v as f64)
            .sum::<f64>()
            / count.max(1) as f64;
        for x in v {
            if !x.is_finite() {
                *x = mean as f32;
            }
        }
    }
    let _ = max;
    (va, vb, step)
}

#[allow(clippy::too_many_arguments)]
fn correlate(
    st: &Stack,
    a: usize,
    b: usize,
    p: [f64; 2],
    q: [f64; 2],
    side: f64,
    n: usize,
    axis: f64,
    max: f64,
) -> ([f64; 2], f64) {
    let (a, b, step) = extract_pair(st, a, b, p, q, side, n, axis, max);
    let (d, cc) = crate::align::patch_shift(&a, &b, n, max / step);
    ([d[0] * step, d[1] * step], cc)
}

pub fn track(
    st: &Stack,
    axis: f64,
    config: PatchConfig,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Tracks> {
    track_backend(st, axis, config, "auto", progress)
}

pub fn track_backend(
    st: &Stack,
    axis: f64,
    config: PatchConfig,
    backend: &str,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Tracks> {
    config.validate(st.w, st.h)?;
    let now = std::time::Instant::now();
    let n_patch = config.size.next_power_of_two().max(32);
    let mut gpu = None;
    let mut chosen_gpu = false;
    let mut calibrated = false;
    let mut cpu_pair_ms = 0.0;
    let mut gpu_pair_ms = None;
    let count = st.views.len();
    let zero = (0..count)
        .min_by(|&a, &b| st.angles[a].abs().total_cmp(&st.angles[b].abs()))
        .unwrap();
    let centre = [(st.w - 1) as f64 / 2.0, (st.h - 1) as f64 / 2.0];
    let points = config.nx * config.ny;
    let seeds: Vec<[f64; 2]> = (0..points)
        .map(|i| {
            [
                ((i % config.nx) as f64 + 0.5) * st.w as f64 / config.nx as f64,
                ((i / config.nx) as f64 + 0.5) * st.h as f64 / config.ny as f64,
            ]
        })
        .collect();
    let mut obs: Vec<Observation> = seeds
        .iter()
        .enumerate()
        .map(|(j, p)| Observation {
            view: zero,
            point: j,
            xy: [p[0] - centre[0], p[1] - centre[1]],
            cc: 1.0,
        })
        .collect();
    let mut ccs = vec![1.0; count];
    let mut counts = vec![0; count];
    counts[zero] = points;
    let mut done = 1;
    for direction in [-1isize, 1] {
        let mut pos: Vec<Option<[f64; 2]>> = seeds.iter().copied().map(Some).collect();
        let mut a = zero;
        loop {
            let bi = a as isize + direction;
            if bi < 0 || bi >= count as isize {
                break;
            }
            check_cancel()?;
            let b = bi as usize;
            let side = st.w.min(st.h) as f64 * 0.8;
            let n = (side as usize).next_power_of_two().clamp(32, 256);
            let (coarse, _) = correlate(st, a, b, centre, centre, side, n, axis, side * 0.2);
            let ratio = st.angles[b].to_radians().cos() / st.angles[a].to_radians().cos();
            let active: Vec<_> = pos
                .par_iter()
                .enumerate()
                .filter_map(|(j, p)| {
                    let p = (*p)?;
                    let d = stretch([p[0] - centre[0], p[1] - centre[1]], ratio, axis);
                    let q = [centre[0] + d[0] + coarse[0], centre[1] + d[1] + coarse[1]];
                    let (va, vb, step) =
                        extract_pair(st, a, b, p, q, config.size as f64, n_patch, axis, 0.0);
                    Some((j, q, (va, vb), step))
                })
                .collect();
            let pairs: Vec<_> = active.iter().map(|v| v.2.clone()).collect();
            let max = config.size as f64 * 0.22 / (config.size as f64 / n_patch as f64);
            let cpu = || {
                pairs
                    .par_iter()
                    .map(|(a, b)| crate::align::patch_shift(a, b, n_patch, max))
                    .collect::<Vec<_>>()
            };
            let peaks = if !calibrated && pairs.len() >= 4 {
                let _ = cpu();
                let t = std::time::Instant::now();
                let reference = cpu();
                cpu_pair_ms = t.elapsed().as_secs_f64() * 1000.0;
                if let Some((dev, _)) = crate::ctf_gpu::device().filter(|_| backend != "cpu") {
                    let result = crate::ctf_gpu::guarded(|| {
                        let batch = super::correlation_gpu::Batch::new(dev, n_patch);
                        let _ = batch.correlate(&pairs, max)?;
                        let t = std::time::Instant::now();
                        let peaks = batch.correlate(&pairs, max)?;
                        Ok::<_, anyhow::Error>((batch, peaks, t.elapsed().as_secs_f64() * 1000.0))
                    });
                    if let Ok(Ok((batch, peaks, ms))) = result {
                        gpu_pair_ms = Some(ms);
                        let agree = peaks.iter().zip(&reference).all(|(a, b)| {
                            (a.0[0] - b.0[0]).hypot(a.0[1] - b.0[1]) < 0.05
                                && (a.1 - b.1).abs() < 0.01
                        });
                        chosen_gpu = agree && (backend == "gpu" || ms < cpu_pair_ms);
                        gpu = Some(batch);
                    }
                }
                calibrated = true;
                if chosen_gpu {
                    gpu.as_ref().unwrap().correlate(&pairs, max)?
                } else {
                    reference
                }
            } else if chosen_gpu {
                match crate::ctf_gpu::guarded(|| gpu.as_ref().unwrap().correlate(&pairs, max)) {
                    Ok(Ok(peaks)) => peaks,
                    _ => {
                        chosen_gpu = false;
                        cpu()
                    }
                }
            } else {
                cpu()
            };
            let mut result = vec![None; points];
            for ((j, q, _, step), (off, cc)) in active.into_iter().zip(peaks) {
                let q = [q[0] + off[0] * step, q[1] + off[1] * step];
                let margin = 0.0;
                if q[0] >= margin
                    && q[1] >= margin
                    && q[0] < st.w as f64 - margin
                    && q[1] < st.h as f64 - margin
                {
                    result[j] = Some((q, cc));
                }
            }
            let med = median(&result.iter().flatten().map(|v| v.1).collect::<Vec<_>>());
            ccs[b] = med;
            for (j, v) in result.into_iter().enumerate() {
                pos[j] = v
                    .filter(|(_, cc)| *cc > (med * 0.5).max(0.08))
                    .map(|(q, cc)| {
                        obs.push(Observation {
                            view: b,
                            point: j,
                            xy: [q[0] - centre[0], q[1] - centre[1]],
                            cc,
                        });
                        counts[b] += 1;
                        q
                    });
            }
            done += 1;
            progress(done, count);
            a = b;
        }
    }
    if counts.iter().filter(|&&n| n >= 4).count() < 3 {
        bail!("有效 patch 轮廓不足，请调整倾转轴角或计算 binning");
    }
    Ok(Tracks {
        observations: obs,
        points,
        cc: ccs,
        counts,
        ms: now.elapsed().as_secs_f64() * 1000.0,
        cpu_pair_ms,
        gpu_pair_ms,
        backend: if chosen_gpu { "GPU" } else { "CPU" }.into(),
    })
}
