//! Patch leave-one-out alignment after global correction, following the
//! method of Zheng et al., Nature Methods 14, 331–332 (2017). Our model is
//! [1,x,y,x²,xy,y²] times [t-mean(t),t²-mean(t²),t³-mean(t³)]: 18 terms per
//! displacement component, in normalized image/time coordinates. Tukey
//! IRLS rejects weak and inconsistent measurements. No external code used.
use super::{check_cancel, input::Spool, Params};
use crate::tomo::{correlation_gpu::Batch, median, track::sample};
use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
const TERMS: usize = 18;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub coeff: [[f64; TERMS]; 2],
    pub time_mean: [f64; 3],
}
impl Model {
    pub fn zero(times: &[f64]) -> Self {
        Self {
            coeff: [[0.0; TERMS]; 2],
            time_mean: std::array::from_fn(|k| {
                times.iter().map(|t| t.powi(k as i32 + 1)).sum::<f64>() / times.len() as f64
            }),
        }
    }
    fn basis(&self, x: f64, y: f64, t: f64) -> [f64; TERMS] {
        let s = [1.0, x, y, x * x, x * y, y * y];
        std::array::from_fn(|i| s[i / 3] * (t.powi((i % 3 + 1) as i32) - self.time_mean[i % 3]))
    }
    pub fn displacement(&self, x: f64, y: f64, t: f64) -> [f64; 2] {
        let b = self.basis(x, y, t);
        std::array::from_fn(|k| self.coeff[k].iter().zip(b).map(|(a, b)| a * b).sum())
    }
    pub fn spatial(&self, t: f64) -> [[f32; 6]; 2] {
        std::array::from_fn(|k| {
            std::array::from_fn(|j| {
                (0..3)
                    .map(|v| self.coeff[k][j * 3 + v] * (t.powi(v as i32 + 1) - self.time_mean[v]))
                    .sum::<f64>() as f32
            })
        })
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    pub centre: [f64; 2],
    pub motion: Vec<[f64; 2]>,
    pub cc: Vec<f64>,
    pub magnitude: f64,
    pub vector: [f64; 2],
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Local {
    pub model: Model,
    pub patches: Vec<Patch>,
    pub global_residual: f64,
    pub local_residual: f64,
    pub rejected: usize,
    pub backend: String,
    pub cpu_pair_ms: f64,
    pub gpu_pair_ms: Option<f64>,
}

fn solve(mut a: [[f64; TERMS]; TERMS], mut b: [f64; TERMS]) -> [f64; TERMS] {
    let ridge = (0..TERMS).map(|i| a[i][i]).sum::<f64>() * 1e-7 + 1e-8;
    for (i, row) in a.iter_mut().enumerate() {
        row[i] += ridge;
    }
    for i in 0..TERMS {
        let p = (i..TERMS)
            .max_by(|&j, &k| a[j][i].abs().total_cmp(&a[k][i].abs()))
            .unwrap();
        a.swap(i, p);
        b.swap(i, p);
        let d = a[i][i];
        if d.abs() < 1e-15 {
            return [0.0; TERMS];
        }
        for v in &mut a[i][i..] {
            *v /= d;
        }
        b[i] /= d;
        let pivot = a[i];
        for j in 0..TERMS {
            if j == i {
                continue;
            }
            let d = a[j][i];
            for (v, p) in a[j][i..].iter_mut().zip(&pivot[i..]) {
                *v -= d * p;
            }
            b[j] -= d * b[i];
        }
    }
    b
}

pub fn fit(
    centres: &[[f64; 2]],
    times: &[f64],
    motion: &[[f64; 2]],
    cc: &[f64],
) -> (Model, Vec<f64>) {
    let mut model = Model::zero(times);
    let nf = times.len();
    let base: Vec<f64> = cc.iter().map(|v| v.max(0.0).powi(2)).collect();
    let mut weights = base.clone();
    for it in 0..8 {
        let mut a = [[0.0; TERMS]; TERMS];
        let mut b = [[0.0; TERMS]; 2];
        for (i, (d, &wt)) in motion.iter().zip(&weights).enumerate() {
            let c = centres[i / nf];
            let phi = model.basis(c[0], c[1], times[i % nf]);
            for j in 0..TERMS {
                for k in 0..TERMS {
                    a[j][k] += wt * phi[j] * phi[k];
                }
                for k in 0..2 {
                    b[k][j] += wt * phi[j] * d[k];
                }
            }
        }
        model.coeff = [solve(a, b[0]), solve(a, b[1])];
        if it == 7 {
            break;
        }
        let errors: Vec<_> = motion
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let c = centres[i / nf];
                let p = model.displacement(c[0], c[1], times[i % nf]);
                (d[0] - p[0]).hypot(d[1] - p[1])
            })
            .collect();
        let scale = (median(&errors) * 1.4826).max(0.03);
        for i in 0..weights.len() {
            let r = errors[i] / (4.685 * scale);
            weights[i] = if r < 1.0 {
                base[i] * (1.0 - r * r).powi(2)
            } else {
                0.0
            };
        }
    }
    (model, weights)
}

pub fn estimate(
    sp: &Spool,
    global: &[[f64; 2]],
    p: &Params,
    progress: &(dyn Fn(&str, usize, usize) + Sync),
) -> Result<Local> {
    let nf = sp.len();
    let (w, h) = (sp.w / sp.bin, sp.h / sp.bin);
    let ngrid = p.patches;
    let centres: Vec<_> = (0..ngrid * ngrid)
        .map(|i| {
            [
                2.0 * (i % ngrid) as f64 / ngrid as f64 + 1.0 / ngrid as f64 - 1.0,
                2.0 * (i / ngrid) as f64 / ngrid as f64 + 1.0 / ngrid as f64 - 1.0,
            ]
        })
        .collect();
    if !p.local {
        return Ok(Local {
            model: Model::zero(&sp.times),
            patches: vec![],
            global_residual: 0.0,
            local_residual: 0.0,
            rejected: 0,
            backend: "关闭局部校正".into(),
            cpu_pair_ms: 0.0,
            gpu_pair_ms: None,
        });
    }
    let avail = w.min(h) / ngrid * 4 / 5;
    if avail < 32 {
        bail!("局部网格过密，请减少 patches 或提高计算尺寸");
    }
    let n = (1usize << avail.ilog2()).clamp(32, 256);
    let len = n * n;
    let patches = centres.len();
    let base: Vec<Vec<f32>> = centres
        .par_iter()
        .flat_map_iter(|c| {
            let cx = (c[0] + 1.0) * (w - 1) as f64 / 2.0;
            let cy = (c[1] + 1.0) * (h - 1) as f64 / 2.0;
            (0..nf).map(move |i| {
                (0..len)
                    .map(|j| {
                        sample(
                            &sp.frames[i],
                            w,
                            h,
                            cx + (j % n) as f64 - (n - 1) as f64 / 2.0
                                + global[i][0] / sp.bin as f64,
                            cy + (j / n) as f64 - (n - 1) as f64 / 2.0
                                + global[i][1] / sp.bin as f64,
                        )
                    })
                    .collect()
            })
        })
        .collect();
    let mut shifts = vec![[0.0; 2]; patches * nf];
    let mut ccs = vec![1.0; shifts.len()];
    let max = n as f64 / 8.0;
    let mut gpu = None;
    let mut use_gpu = false;
    let mut cpu_pair_ms = 0.0;
    let mut gpu_pair_ms = None;
    for it in 0..4 {
        check_cancel()?;
        let aligned: Vec<_> = base
            .par_iter()
            .zip(&shifts)
            .map(|(v, &d)| crate::align::movie_shift(v, n, n, d))
            .collect();
        let sums: Vec<Vec<f32>> = aligned
            .par_chunks(nf)
            .map(|chunk| {
                let mut sum = vec![0.0; len];
                for v in chunk {
                    for (o, &v) in sum.iter_mut().zip(v) {
                        *o += v;
                    }
                }
                sum
            })
            .collect();
        let pairs: Vec<_> = base
            .par_iter()
            .enumerate()
            .map(|(i, v)| {
                let reference = sums[i / nf]
                    .iter()
                    .zip(&aligned[i])
                    .map(|(a, b)| (a - b) / (nf - 1) as f32)
                    .collect();
                (reference, v.clone())
            })
            .collect();
        let cpu = |q: &[(Vec<f32>, Vec<f32>)]| {
            q.par_iter()
                .map(|(a, b)| crate::align::patch_shift(a, b, n, max))
                .collect::<Vec<_>>()
        };
        if it == 0 {
            let q = &pairs[..pairs.len().min(32)];
            let _ = cpu(q);
            let t = std::time::Instant::now();
            let reference = cpu(q);
            cpu_pair_ms = t.elapsed().as_secs_f64() * 1000.0;
            if p.backend != "cpu" {
                if let Some((dev, _)) = crate::ctf_gpu::device() {
                    if let Ok(Ok((b, peaks, ms))) = crate::ctf_gpu::guarded(|| -> Result<_> {
                        let b = Batch::new(dev, n);
                        let _ = b.correlate(q, max)?;
                        let t = std::time::Instant::now();
                        let r = b.correlate(q, max)?;
                        Ok((b, r, t.elapsed().as_secs_f64() * 1000.0))
                    }) {
                        gpu_pair_ms = Some(ms);
                        use_gpu = peaks
                            .iter()
                            .zip(reference)
                            .all(|(a, b)| (a.0[0] - b.0[0]).hypot(a.0[1] - b.0[1]) < 0.05)
                            && (p.backend == "gpu" || ms < cpu_pair_ms);
                        gpu = Some(b);
                    }
                }
            }
        }
        let peaks = if use_gpu {
            match crate::ctf_gpu::guarded(|| gpu.as_ref().unwrap().correlate(&pairs, max)) {
                Ok(Ok(v)) => v,
                _ => {
                    use_gpu = false;
                    cpu(&pairs)
                }
            }
        } else {
            cpu(&pairs)
        };
        let mut change = 0.0f64;
        for (j, (d, cc)) in peaks.into_iter().enumerate() {
            change = change.max((d[0] - shifts[j][0]).hypot(d[1] - shifts[j][1]));
            shifts[j] = d;
            ccs[j] = cc;
        }
        for chunk in shifts.chunks_mut(nf) {
            let mean: [f64; 2] =
                std::array::from_fn(|k| chunk.iter().map(|d| d[k]).sum::<f64>() / nf as f64);
            for d in chunk {
                for k in 0..2 {
                    d[k] -= mean[k];
                }
            }
        }
        progress("局部轨迹", it + 1, 4);
        if it > 0 && change < 0.02 {
            break;
        }
    }
    for i in 0..nf {
        let med = median(&(0..patches).map(|j| ccs[j * nf + i]).collect::<Vec<_>>());
        for j in 0..patches {
            if ccs[j * nf + i] < (0.5 * med).max(0.08) {
                ccs[j * nf + i] = 0.0;
            }
        }
    }
    let shifts: Vec<_> = shifts
        .into_iter()
        .map(|v| v.map(|v| v * sp.bin as f64))
        .collect();
    let (model, weights) = fit(&centres, &sp.times, &shifts, &ccs);
    let used = weights.iter().sum::<f64>();
    if used < 1e-8 {
        bail!("局部运动相关不足，请关闭局部校正或调整分组");
    }
    let mut before = 0.0;
    let mut after = 0.0;
    for (j, c) in centres.iter().enumerate() {
        for i in 0..nf {
            let k = j * nf + i;
            let d = model.displacement(c[0], c[1], sp.times[i]);
            before += weights[k] * shifts[k][0].hypot(shifts[k][1]);
            after += weights[k] * (shifts[k][0] - d[0]).hypot(shifts[k][1] - d[1]);
        }
    }
    let patches = centres
        .iter()
        .enumerate()
        .map(|(j, c)| {
            let motion: Vec<_> = sp
                .times
                .iter()
                .map(|&t| model.displacement(c[0], c[1], t))
                .collect();
            let vector = [
                motion[nf - 1][0] - motion[0][0],
                motion[nf - 1][1] - motion[0][1],
            ];
            let magnitude = motion.iter().map(|d| d[0].hypot(d[1])).sum::<f64>() / nf as f64;
            Patch {
                centre: [
                    (c[0] + 1.0) * (sp.w - 1) as f64 / 2.0,
                    (c[1] + 1.0) * (sp.h - 1) as f64 / 2.0,
                ],
                motion,
                cc: ccs[j * nf..(j + 1) * nf].to_vec(),
                magnitude,
                vector,
            }
        })
        .collect();
    let rejected = weights.iter().filter(|&&w| w == 0.0).count();
    Ok(Local {
        model,
        patches,
        global_residual: before / used,
        local_residual: after / used,
        rejected,
        backend: if use_gpu { "GPU" } else { "CPU" }.into(),
        cpu_pair_ms,
        gpu_pair_ms,
    })
}
