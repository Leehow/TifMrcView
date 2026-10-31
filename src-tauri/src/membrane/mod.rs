//! Membrane segmentation for tomograms (Pro), modelled on MemBrain-seg, the
//! segmentation part of MemBrain v2 (Lamm et al., bioRxiv 2024), written here
//! from the paper and its documentation; no code or weights are taken from it.
//!
//! As there: the tomogram is rescaled to 10 Å per voxel and standardised, a
//! residual 3D U-Net (instance norm, deep supervision; `net`) scores every
//! voxel in overlapping 160³ windows weighted by a Gaussian, the scores of
//! mirrored copies are averaged (test-time augmentation), and voxels with a
//! positive score are membrane. Small specks are removed and the result is
//! taken back to the tomogram's own grid.
//!
//! Our own network is trained by `examples/membrane_train.rs` on the
//! MemBrain-seg training patches (Zenodo 15089686, CC BY 4.0), with heavy
//! augmentation (`augment`). The user does not choose anything: the pixel
//! size comes from the file, the number of mirrored copies from the
//! measured speed.

pub mod augment;
pub mod net;
pub mod nifti;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{anyhow, bail, Result};
use burn::tensor::{Device, Tensor, TensorData};
use burn_store::{ModuleSnapshot, SafetensorsStore};
use rayon::prelude::*;
use serde::Serialize;

pub use net::{MemNet, NetConfig};

/// Voxel size the network works at (Å).
pub const PIXEL_A: f32 = 10.0;
/// Sliding window side for inference (voxels at `PIXEL_A`).
pub const WINDOW: usize = 160;
/// Membrane patches smaller than this (nm³) are dropped.
pub const MIN_NM3: f32 = 2000.0;

pub const MODEL_FILE: &str = "membrane-v1.safetensors";
/// Tried in order: the site, then its China mirror.
pub const MODEL_URLS: [&str; 2] = ["https://tifmrcview.com/models/membrane-v1.safetensors", "https://dl.deepwood.cn/tifmrcview/models/membrane-v1.safetensors"];

/// Size and SHA-256 of the published model (0 / empty: not checked yet).
pub const MODEL_SIZE: u64 = 0;
pub const MODEL_SHA256: &str = "";
/// Seconds the app spends on extra mirrored copies.
pub const BUDGET_S: f64 = 60.0;
/// Material the result is written to.
pub const MATERIAL: &str = "膜";
pub const COLOR: [u8; 3] = [255, 196, 0];

pub fn verify(len: u64, sha256_hex: &str) -> Result<()> {
    if (MODEL_SIZE != 0 && len != MODEL_SIZE) || (!MODEL_SHA256.is_empty() && sha256_hex != MODEL_SHA256) {
        bail!("膜分割模型文件校验失败");
    }
    Ok(())
}

/// Set to stop a running segmentation.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

fn check_cancel() -> Result<()> {
    if CANCEL.load(Ordering::SeqCst) {
        bail!("已取消");
    }
    Ok(())
}

pub fn model_path() -> Option<PathBuf> {
    Some(crate::plugins::paths::user_dir()?.parent()?.join("models").join(MODEL_FILE))
}

pub fn load(path: &Path, dev: &Device) -> Result<MemNet> {
    let mut net = MemNet::new(&NetConfig::default(), dev);
    let mut store = SafetensorsStore::from_file(path);
    let r = net.load_from(&mut store).map_err(|e| anyhow!("读取膜分割模型失败：{e:?}"))?;
    if !r.missing.is_empty() {
        bail!("膜分割模型不完整（缺 {} 个参数）", r.missing.len());
    }
    Ok(net)
}

pub fn save(net: &MemNet, path: &Path) -> Result<()> {
    let mut store = SafetensorsStore::from_file(path).overwrite(true);
    net.save_into(&mut store).map_err(|e| anyhow!("保存模型失败：{e:?}"))
}

// ---- Resampling ----

/// Trilinear resampling of a volume (X fastest) from `n` to `m` voxels,
/// voxel centres aligned; low-passed first when shrinking.
pub fn resample(v: &[f32], n: [usize; 3], m: [usize; 3]) -> Vec<f32> {
    if n == m {
        return v.to_vec();
    }
    let f: [f32; 3] = std::array::from_fn(|a| n[a] as f32 / m[a] as f32);
    let sigma = f.iter().cloned().fold(0f32, f32::max);
    let src;
    let v = if sigma > 1.2 {
        // σ ≈ 0.4 × the shrink factor keeps aliasing down.
        src = crate::wedge::smooth(&crate::wedge::Vol { n, v: v.to_vec() }, 0.4 * sigma).v;
        &src[..]
    } else {
        v
    };
    let axis = |a: usize| -> Vec<(usize, usize, f32)> {
        (0..m[a])
            .map(|i| {
                let s = ((i as f32 + 0.5) * f[a] - 0.5).clamp(0.0, (n[a] - 1) as f32);
                let i0 = s.floor() as usize;
                let i1 = (i0 + 1).min(n[a] - 1);
                (i0, i1, s - i0 as f32)
            })
            .collect()
    };
    let (ax, ay, az) = (axis(0), axis(1), axis(2));
    let mut out = vec![0f32; m[0] * m[1] * m[2]];
    out.par_chunks_mut(m[0] * m[1]).enumerate().for_each(|(z, plane)| {
        let (z0, z1, tz) = az[z];
        for (y, row) in plane.chunks_mut(m[0]).enumerate() {
            let (y0, y1, ty) = ay[y];
            let at = |x: usize, y: usize, z: usize| v[(z * n[1] + y) * n[0] + x];
            for (x, o) in row.iter_mut().enumerate() {
                let (x0, x1, tx) = ax[x];
                let l = |x| {
                    let a = at(x, y0, z0) * (1.0 - ty) + at(x, y1, z0) * ty;
                    let b = at(x, y0, z1) * (1.0 - ty) + at(x, y1, z1) * ty;
                    a * (1.0 - tz) + b * tz
                };
                *o = l(x0) * (1.0 - tx) + l(x1) * tx;
            }
        }
    });
    out
}

/// Grid size at `PIXEL_A` for `n` voxels of `pixel_a` Å.
pub fn grid_at(n: [usize; 3], pixel_a: f32) -> [usize; 3] {
    std::array::from_fn(|a| ((n[a] as f32 * pixel_a / PIXEL_A).round() as usize).max(1))
}

// ---- Inference ----

/// Mirror sets for test-time augmentation, in the order they are used.
const FLIPS: [&[usize]; 8] = [&[], &[4], &[3], &[2], &[3, 4], &[2, 4], &[2, 3], &[2, 3, 4]];

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Mirrored copies averaged.
    pub copies: usize,
    pub windows: usize,
    /// Grid the network saw.
    pub grid: [usize; 3],
    pub ms: f64,
}

fn flip(x: Tensor<5>, axes: &[usize]) -> Tensor<5> {
    match axes {
        [] => x,
        [a] => x.flip([*a]),
        [a, b] => x.flip([*a, *b]),
        _ => x.flip([2, 3, 4]),
    }
}

/// Starts of windows of `side` covering `n` with about half overlap.
fn starts(n: usize, side: usize) -> Vec<usize> {
    if n <= side {
        return vec![0];
    }
    let k = (n - side).div_ceil(side / 2) + 1;
    (0..k).map(|i| ((n - side) as f64 * i as f64 / (k - 1) as f64).round() as usize).collect()
}

/// Membrane scores (logits) of a standardised volume `v` (dims `n`, X
/// fastest) already at `PIXEL_A`. Spends about `budget_s` seconds on mirrored
/// copies beyond the first (at least one copy, at most eight).
pub fn score(dev: &Device, net: &MemNet, v: &[f32], n: [usize; 3], budget_s: f64, progress: &mut dyn FnMut(Progress)) -> Result<(Vec<f32>, Stats)> {
    let t0 = Instant::now();
    let al = net.align();
    let side: [usize; 3] = std::array::from_fn(|a| WINDOW.min(n[a].div_ceil(al) * al).max(al));
    let st: [Vec<usize>; 3] = std::array::from_fn(|a| starts(n[a], side[a]));
    let windows = st[0].len() * st[1].len() * st[2].len();
    // Gaussian importance (σ = side/8) so window edges count little.
    let g: [Vec<f32>; 3] = std::array::from_fn(|a| {
        let s = side[a] as f32 / 8.0;
        let c = (side[a] as f32 - 1.0) / 2.0;
        (0..side[a]).map(|i| (-((i as f32 - c).powi(2)) / (2.0 * s * s)).exp().max(1e-3)).collect()
    });
    let vol = crate::wedge::Vol { n, v: v.to_vec() };
    let len = n[0] * n[1] * n[2];
    let (mut acc, mut wsum) = (vec![0f32; len], vec![0f32; len]);
    let mut copies = 8;
    let mut done = 0;
    let mut c = 0;
    while c < copies {
        for &z0 in &st[2] {
            for &y0 in &st[1] {
                for &x0 in &st[0] {
                    check_cancel()?;
                    let tile = vol.cube([x0 as isize, y0 as isize, z0 as isize], side);
                    let x = Tensor::<5>::from_data(TensorData::new(tile, [1, 1, side[2], side[1], side[0]]), dev);
                    let r = flip(net.forward(flip(x, FLIPS[c])), FLIPS[c]);
                    let r: Vec<f32> = r.into_data().convert::<f32>().try_into_vec::<f32>().map_err(|e| anyhow!("{e:?}"))?;
                    for z in 0..side[2].min(n[2] - z0) {
                        for y in 0..side[1].min(n[1] - y0) {
                            let wzy = g[2][z] * g[1][y];
                            let src = (z * side[1] + y) * side[0];
                            let dst = ((z0 + z) * n[1] + y0 + y) * n[0] + x0;
                            for x in 0..side[0].min(n[0] - x0) {
                                let w = wzy * g[0][x];
                                acc[dst + x] += w * r[src + x];
                                wsum[dst + x] += w;
                            }
                        }
                    }
                    done += 1;
                    progress(Progress { done, total: windows * copies });
                }
            }
        }
        if c == 0 {
            // From the first pass's time, as many copies as fit the budget.
            let per = t0.elapsed().as_secs_f64().max(1e-3);
            copies = match (budget_s / per).floor() as usize + 1 {
                0..=1 => 1,
                2..=3 => 2,
                4..=7 => 4,
                _ => 8,
            };
        }
        c += 1;
    }
    acc.par_iter_mut().zip(&wsum).for_each(|(a, &w)| *a /= w.max(1e-12));
    Ok((acc, Stats { copies, windows, grid: n, ms: t0.elapsed().as_secs_f64() * 1e3 }))
}

/// Keeps the 26-connected components of `mask` with at least `min` voxels.
/// Returns the kept mask and the number of components kept.
pub fn clean(mask: &[bool], n: [usize; 3], min: usize) -> (Vec<bool>, usize) {
    let len = mask.len();
    let mut parent: Vec<u32> = (0..len as u32).collect();
    fn find(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            p[i as usize] = p[p[i as usize] as usize];
            i = p[i as usize];
        }
        i
    }
    let idx = |x: usize, y: usize, z: usize| (z * n[1] + y) * n[0] + x;
    for z in 0..n[2] {
        for y in 0..n[1] {
            for x in 0..n[0] {
                let i = idx(x, y, z);
                if !mask[i] {
                    continue;
                }
                // Earlier neighbours only (13 of 26).
                for dz in -1isize..=0 {
                    for dy in -1isize..=1 {
                        for dx in -1isize..=1 {
                            if (dz, dy, dx) >= (0, 0, 0) {
                                continue;
                            }
                            let (xx, yy, zz) = (x as isize + dx, y as isize + dy, z as isize + dz);
                            if xx < 0 || yy < 0 || zz < 0 || xx >= n[0] as isize || yy >= n[1] as isize {
                                continue;
                            }
                            let j = idx(xx as usize, yy as usize, zz as usize);
                            if mask[j] {
                                let (a, b) = (find(&mut parent, i as u32), find(&mut parent, j as u32));
                                if a != b {
                                    parent[a.max(b) as usize] = a.min(b);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut size = vec![0u32; len];
    for i in 0..len {
        if mask[i] {
            let r = find(&mut parent, i as u32);
            size[r as usize] += 1;
        }
    }
    let kept = size.iter().filter(|&&s| s as usize >= min).count();
    let out = (0..len).map(|i| mask[i] && size[find(&mut parent, i as u32) as usize] as usize >= min).collect();
    (out, kept)
}

/// Result on the tomogram's own grid.
pub struct Segmentation {
    /// Logits; membrane where positive.
    pub score: Vec<f32>,
    /// Membrane after removing specks.
    pub mask: Vec<bool>,
    pub components: usize,
    pub stats: Stats,
}

/// Segments the membranes of `values` (dims `n`, X fastest, `pixel_a` Å per
/// voxel). `budget_s`: seconds to spend on extra mirrored copies.
pub fn segment(dev: &Device, net: &MemNet, values: &[f32], n: [usize; 3], pixel_a: f32, budget_s: f64, progress: &mut dyn FnMut(Progress)) -> Result<Segmentation> {
    if values.len() != n[0] * n[1] * n[2] {
        bail!("体数据大小和尺寸不符");
    }
    if !(pixel_a.is_finite() && pixel_a > 0.5) {
        bail!("需要像素大小（Å）");
    }
    let (mean, std) = crate::wedge::stats(values);
    let m = grid_at(n, pixel_a);
    if m.iter().any(|&k| k < 8) {
        bail!("体积太小：按 10 Å 体素算每边不到 8 个");
    }
    let norm: Vec<f32> = values.par_iter().map(|&x| if x.is_finite() { (x - mean) / std } else { 0.0 }).collect();
    let small = resample(&norm, n, m);
    drop(norm);
    // Standardise again at the network's scale (the low-pass narrows it).
    let (m2, s2) = crate::wedge::stats(&small);
    let small: Vec<f32> = small.into_par_iter().map(|x| (x - m2) / s2).collect();
    check_cancel()?;
    let (logits, stats) = score(dev, net, &small, m, budget_s, progress)?;
    let min = (MIN_NM3 / (PIXEL_A / 10.0).powi(3)).round() as usize;
    let (kept, components) = clean(&logits.iter().map(|&s| s > 0.0).collect::<Vec<_>>(), m, min);
    let score = resample(&logits, m, n);
    // Keep a voxel when its score is positive and the nearest network voxel
    // belongs to a kept component.
    let near: [Vec<usize>; 3] = std::array::from_fn(|a| (0..n[a]).map(|i| (((i as f32 + 0.5) * m[a] as f32 / n[a] as f32) as usize).min(m[a] - 1)).collect());
    let mut mask = vec![false; score.len()];
    mask.par_chunks_mut(n[0] * n[1]).enumerate().for_each(|(z, plane)| {
        for (y, row) in plane.chunks_mut(n[0]).enumerate() {
            for (x, o) in row.iter_mut().enumerate() {
                let i = (z * n[1] + y) * n[0] + x;
                *o = score[i] > 0.0 && kept[(near[2][z] * m[1] + near[1][y]) * m[0] + near[0][x]];
            }
        }
    });
    Ok(Segmentation { score, mask, components, stats })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_cover() {
        assert_eq!(starts(100, 160), vec![0]);
        assert_eq!(starts(160, 160), vec![0]);
        let s = starts(400, 160);
        assert_eq!(*s.last().unwrap(), 240);
        assert!(s.windows(2).all(|w| w[1] - w[0] <= 80));
    }

    #[test]
    fn clean_drops_small() {
        let n = [10, 10, 10];
        let mut m = vec![false; 1000];
        for x in 0..8 {
            for y in 0..8 {
                m[(5 * 10 + y) * 10 + x] = true;
            }
        }
        m[999] = true;
        let (k, c) = clean(&m, n, 10);
        assert_eq!(c, 1);
        assert!(!k[999]);
        assert_eq!(k.iter().filter(|&&b| b).count(), 64);
    }

    #[test]
    fn resample_keeps_constant() {
        let v = vec![3.0f32; 20 * 20 * 20];
        let r = resample(&v, [20; 3], [7, 9, 11]);
        assert!(r.iter().all(|&x| (x - 3.0).abs() < 1e-4));
    }

    #[test]
    fn net_shapes() {
        let dev = Device::flex();
        let cfg = NetConfig { widths: vec![4, 8, 8], blocks: vec![1, 1, 1], heads: 2 };
        let net = MemNet::new(&cfg, &dev);
        assert_eq!(net.align(), 4);
        let x = Tensor::<5>::zeros([1, 1, 8, 12, 16], &dev);
        let o = net.forward_all(x.clone());
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].dims(), [1, 1, 8, 12, 16]);
        assert_eq!(o[1].dims(), [1, 1, 4, 6, 8]);
        assert_eq!(net.forward(x).dims(), [1, 1, 8, 12, 16]);
        let l = net::loss(o[0].clone(), Tensor::<5>::zeros([1, 1, 8, 12, 16], &dev));
        assert!(l.into_scalar::<f32>().is_finite());
        let y = Tensor::<5>::ones([1, 1, 8, 12, 16], &dev);
        let s = net::skeleton_loss(o[0].clone(), y);
        assert!(s.into_scalar::<f32>().is_finite());
    }
}
