//! CPU reference: voxel-driven linear splatting A and its exact transpose
//! Aᵀ, ramp-filtered backprojection and row/column-normalised SIRT (Gilbert).
//! Layout is [Y][Z][X] internally, sinograms [Y][view][detector X].

use super::{check_cancel, CANCEL};
use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Method {
    Wbp,
    Sirt,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Filter {
    Ramp,
    Cosine,
    Hamming,
    SirtLike,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub thickness: usize,
    pub method: Method,
    pub iterations: usize,
    pub lambda: f32,
    pub non_negative: bool,
    pub z_offset: f64,
    pub filter: Filter,
    /// auto / cpu / gpu (explicit gpu fails instead of hiding fallback).
    pub backend: String,
}
impl Default for Params {
    fn default() -> Self {
        Self {
            thickness: 128,
            method: Method::Wbp,
            iterations: 15,
            lambda: 1.0,
            non_negative: false,
            z_offset: 0.0,
            filter: Filter::Ramp,
            backend: "auto".into(),
        }
    }
}
impl Params {
    pub fn validate(&self, w: usize, rows: usize, nv: usize) -> Result<()> {
        if w < 2
            || rows == 0
            || nv < 3
            || self.thickness == 0
            || self.thickness > 4096
            || self.iterations == 0
            || self.iterations > 1000
            || !self.z_offset.is_finite()
            || !self.lambda.is_finite()
            || self.lambda <= 0.0
            || self.lambda > 1.0
            || !matches!(self.backend.as_str(), "auto" | "cpu" | "gpu")
        {
            bail!("重构参数无效");
        }
        let bytes = w
            .checked_mul(rows)
            .and_then(|n| n.checked_mul(self.thickness))
            .and_then(|n| n.checked_mul(4))
            .unwrap_or(usize::MAX);
        if bytes > 8usize << 30 {
            bail!("输出体积超过 8 GiB，请提高 binning");
        }
        Ok(())
    }
}

/// Each voxel contributes to two detector bins. Out-of-field weights are zero.
pub struct Operator {
    pub w: usize,
    pub z: usize,
    pub views: usize,
    pub indices: Vec<usize>,
    pub weights: Vec<f32>,
}
impl Operator {
    pub fn new(w: usize, z: usize, angles: &[f64], z_offset: f64) -> Result<Self> {
        if angles.iter().any(|a| !a.is_finite() || a.abs() >= 90.0) {
            bail!("倾转角必须在 (-90, 90) 内");
        }
        let vox = w
            .checked_mul(z)
            .ok_or_else(|| anyhow::anyhow!("体积过大"))?;
        if vox.saturating_mul(angles.len()).saturating_mul(2) > 1 << 28 {
            bail!("投影几何过大，请提高 binning");
        }
        let mut indices = Vec::with_capacity(2 * vox * angles.len());
        let mut weights = Vec::with_capacity(indices.capacity());
        for (i, &a) in angles.iter().enumerate() {
            let (s, c) = a.to_radians().sin_cos();
            for k in 0..vox {
                let u = (k % w) as f64 - (w - 1) as f64 / 2.0;
                let v = (k / w) as f64 - (z - 1) as f64 / 2.0 + z_offset;
                let d = c * u + s * v + (w - 1) as f64 / 2.0;
                let lo = d.floor() as isize;
                let f = (d - d.floor()) as f32;
                for (j, wt) in [(lo, 1.0 - f), (lo + 1, f)] {
                    let valid = j >= 0 && j < w as isize;
                    indices.push(i * w + j.clamp(0, w as isize - 1) as usize);
                    weights.push(if valid { wt } else { 0.0 });
                }
            }
        }
        Ok(Self {
            w,
            z,
            views: angles.len(),
            indices,
            weights,
        })
    }
    pub fn forward(&self, x: &[f32]) -> Vec<f32> {
        let vox = self.w * self.z;
        let mut b = vec![0.0; self.w * self.views];
        for i in 0..self.indices.len() {
            b[self.indices[i]] += self.weights[i] * x[(i / 2) % vox];
        }
        b
    }
    pub fn back(&self, b: &[f32]) -> Vec<f32> {
        let vox = self.w * self.z;
        let mut x = vec![0.0; vox];
        for i in 0..self.indices.len() {
            x[(i / 2) % vox] += self.weights[i] * b[self.indices[i]];
        }
        x
    }
}

pub fn filtered(b: &[f32], w: usize, f: Filter, iterations: usize) -> Vec<f32> {
    let n = (2 * w).next_power_of_two();
    let fft = crate::fft::Fft::new(n);
    b.par_chunks(w)
        .flat_map_iter(|row| {
            let mut re = vec![0.0; n];
            let mut im = vec![0.0; n];
            re[..w].copy_from_slice(row);
            fft.run(&mut re, &mut im);
            for k in 0..n {
                let r = k.min(n - k) as f32 / (n / 2) as f32;
                let window = match f {
                    Filter::Ramp => 1.0,
                    Filter::Cosine => (std::f32::consts::FRAC_PI_2 * r).cos(),
                    Filter::Hamming => 0.54 + 0.46 * (std::f32::consts::PI * r).cos(),
                    Filter::SirtLike => {
                        // Idealised AᵀA has a 1/|k| spectrum: low frequencies
                        // converge first. This is a spectral approximation,
                        // not the finite-angle iterative SIRT operator.
                        let sigma = 1.0 / (1.0 + k.min(n - k) as f32);
                        1.0 - (1.0 - sigma).powi(iterations as i32)
                    }
                };
                re[k] *= r * window;
                im[k] *= -r * window;
            }
            fft.run(&mut re, &mut im);
            re[..w].iter().map(|v| v / n as f32).collect::<Vec<_>>()
        })
        .collect()
}

pub struct Reconstruction {
    pub data: Vec<f32>,
    /// SIRT's row-normalised objective, as weighted RMS on illuminated rays.
    /// Unweighted RMS need not decrease for a row/column-normalised update.
    pub residuals: Vec<f64>,
    pub backend: String,
}

pub fn cpu(
    b: &[f32],
    rows: usize,
    op: &Operator,
    p: &Params,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Reconstruction> {
    p.validate(op.w, rows, op.views)?;
    let rays = op.w * op.views;
    let vox = op.w * op.z;
    if b.len() != rows * rays {
        bail!("投影尺寸不一致");
    }
    let source = if p.method == Method::Wbp {
        filtered(b, op.w, p.filter, p.iterations)
    } else {
        b.to_vec()
    };
    let row_sum = op.forward(&vec![1.0; vox]);
    let col_sum = op.back(&vec![1.0; rays]);
    let metric: Vec<f64> = row_sum
        .iter()
        .map(|&v| if v > 1e-6 { 1.0 / v as f64 } else { 0.0 })
        .collect();
    let metric_sum = metric.iter().sum::<f64>() * rows as f64;
    let mut x = vec![0.0; rows * vox];
    let mut residuals = Vec::new();
    let steps = if p.method == Method::Wbp {
        1
    } else {
        p.iterations
    };
    for it in 0..steps {
        check_cancel()?;
        let err: f64 = x
            .par_chunks_mut(vox)
            .zip(source.par_chunks(rays))
            .map(|(xx, bb)| {
                if p.method == Method::Wbp {
                    let v = op.back(bb);
                    for (x, v) in xx.iter_mut().zip(v) {
                        *x = v / op.views as f32;
                    }
                    return 0.0;
                }
                let ax = op.forward(xx);
                let e: Vec<f32> = bb.iter().zip(ax).map(|(b, a)| b - a).collect();
                let r = e
                    .iter()
                    .zip(&metric)
                    .map(|(&v, &w)| w * (v as f64).powi(2))
                    .sum::<f64>();
                let norm: Vec<f32> = e
                    .iter()
                    .zip(&row_sum)
                    .map(|(e, s)| e / s.max(1e-6))
                    .collect();
                let step = op.back(&norm);
                for j in 0..vox {
                    xx[j] += p.lambda * step[j] / col_sum[j].max(1e-6);
                    if p.non_negative {
                        xx[j] = xx[j].max(0.0);
                    }
                }
                r
            })
            .sum();
        if p.method == Method::Sirt {
            residuals.push((err / metric_sum.max(1e-12)).sqrt());
        }
        progress(it + 1, steps);
    }
    if CANCEL.load(Ordering::Relaxed) {
        check_cancel()?;
    }
    Ok(Reconstruction {
        data: x,
        residuals,
        backend: "CPU".into(),
    })
}

pub fn reconstruct(
    b: &[f32],
    rows: usize,
    op: &Operator,
    p: &Params,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Reconstruction> {
    p.validate(op.w, rows, op.views)?;
    if p.backend != "cpu" {
        if let Some((dev, name)) = crate::ctf_gpu::device() {
            match crate::ctf_gpu::guarded(|| {
                super::recon_gpu::reconstruct(dev, b, rows, op, p, progress)
            }) {
                Ok(Ok(mut r)) => {
                    r.backend = (*name).into();
                    return Ok(r);
                }
                Ok(Err(e)) if p.backend == "gpu" => return Err(e),
                Err(e) if p.backend == "gpu" => return Err(e),
                r => {
                    check_cancel()?;
                    eprintln!(
                        "tomo GPU fallback: {}",
                        match r {
                            Ok(Err(e)) | Err(e) => e.to_string(),
                            _ => unreachable!(),
                        }
                    );
                }
            }
        } else if p.backend == "gpu" {
            bail!("没有可用 GPU");
        }
    }
    cpu(b, rows, op, p, progress)
}
