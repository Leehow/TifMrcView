//! Membrane segmentation for tomograms (Pro), modelled on MemBrain-seg, the
//! segmentation part of MemBrain v2 (Lamm et al., bioRxiv 2024), written here
//! from the paper, its documentation and the layout of its checkpoints; no
//! code or weights are taken from it.
//!
//! As there: the tomogram is rescaled to 10 Å per voxel and standardised, a
//! residual 3D U-Net (instance norm, deep supervision) scores every
//! voxel in overlapping 160³ windows weighted by a Gaussian, the scores of
//! eight mirrored copies are averaged (test-time augmentation), and voxels with a
//! positive score are membrane. Small specks are removed and the result is
//! taken back to the tomogram's own grid.
//!
//! The network is the published MemBrain-seg model (`dynunet`), from a
//! checkpoint the user downloads from its authors: the weights carry no
//! licence, so the app does not ship them. The user picks the file once and
//! it is remembered. Our own network (`net`), trained by
//! `examples/membrane_train.rs` on the MemBrain-seg training patches
//! (Zenodo 15089686, CC BY 4.0) with heavy augmentation (`augment`), loads
//! the same way from its .safetensors file. Nothing else is chosen: the
//! pixel size comes from the file.

pub mod augment;
pub mod dynunet;
pub mod net;
pub mod nifti;
pub mod vesicles;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{anyhow, bail, Result};
use burn::tensor::{Device, Tensor, TensorData};
use burn_store::{ModuleSnapshot, SafetensorsStore};
use rayon::prelude::*;
use serde::Serialize;

pub use dynunet::DynUNet;
pub use net::{MemNet, NetConfig};

/// Voxel size the network works at (Å).
pub const PIXEL_A: f32 = 10.0;
/// Sliding window side for inference (voxels at `PIXEL_A`).
pub const WINDOW: usize = 160;
/// Membrane patches smaller than this (nm³) are dropped.
pub const MIN_NM3: f32 = 2000.0;

/// Where the user gets the MemBrain-seg models (the README lists the
/// download links of each version).
pub const MODEL_PAGE: &str = "https://github.com/teamtomo/membrain-seg";
/// Material the result is written to.
pub const MATERIAL: &str = "膜";
pub const COLOR: [u8; 3] = [255, 196, 0];

/// Set to stop a running segmentation.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

fn check_cancel() -> Result<()> {
    if CANCEL.load(Ordering::SeqCst) {
        bail!("已取消");
    }
    Ok(())
}

/// The file that remembers which model the user picked.
fn choice_file() -> Option<PathBuf> {
    Some(crate::plugins::paths::user_dir()?.parent()?.join("models").join("membrane-model.txt"))
}

/// The model file the user picked, while it is still there.
pub fn model_path() -> Option<PathBuf> {
    let p = PathBuf::from(std::fs::read_to_string(choice_file()?).ok()?.trim());
    p.is_file().then_some(p)
}

/// Remembers `path` as the model after checking that it loads.
pub fn choose(path: &Path, dev: &Device) -> Result<()> {
    load(path, dev)?;
    let f = choice_file().ok_or_else(|| anyhow!("找不到存放设置的文件夹"))?;
    std::fs::create_dir_all(f.parent().unwrap())?;
    std::fs::write(f, path.to_string_lossy().as_bytes())?;
    Ok(())
}

/// A loaded membrane network.
pub enum Model {
    /// The published MemBrain-seg model.
    MemBrain(DynUNet),
    /// Ours.
    Own(MemNet),
}

impl Model {
    pub fn align(&self) -> usize {
        match self {
            Model::MemBrain(n) => n.align(),
            Model::Own(n) => n.align(),
        }
    }

    /// Whether the network takes volumes as [x, y, z] (MemBrain-seg reads
    /// tomograms that way; its kernels are not symmetric, the missing wedge
    /// being along z) rather than our [z, y, x].
    pub fn xyz(&self) -> bool {
        matches!(self, Model::MemBrain(_))
    }

    /// Logits for [n, 1, …] in the network's axis order (`xyz`).
    pub fn forward(&self, x: Tensor<5>) -> Tensor<5> {
        match self {
            Model::MemBrain(n) => n.forward(x),
            Model::Own(n) => n.forward(x),
        }
    }
}

/// Reverses the axis order of a volume (dims `n`, first axis fastest).
fn transpose(v: &[f32], n: [usize; 3]) -> Vec<f32> {
    let mut out = vec![0f32; v.len()];
    out.par_chunks_mut(n[2] * n[1]).enumerate().for_each(|(x, plane)| {
        for y in 0..n[1] {
            for z in 0..n[2] {
                plane[y * n[2] + z] = v[(z * n[1] + y) * n[0] + x];
            }
        }
    });
    out
}

/// Whether a safetensors file stores half-precision weights (the published
/// model does, to halve the download).
fn is_half(path: &Path) -> Result<bool> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut len = [0u8; 8];
    f.read_exact(&mut len)?;
    let n = u64::from_le_bytes(len).min(16 << 20) as usize;
    let mut head = vec![0u8; n];
    f.read_exact(&mut head)?;
    Ok(String::from_utf8_lossy(&head).contains("\"F16\""))
}

/// Loads a MemBrain-seg checkpoint (.ckpt) or our weights (.safetensors).
pub fn load(path: &Path, dev: &Device) -> Result<Model> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "ckpt" | "pt" | "pth" => Ok(Model::MemBrain(dynunet::load(path, dev)?)),
        "safetensors" => Ok(Model::Own(load_own(path, dev)?)),
        _ => bail!("膜分割模型应是 MemBrain-seg 的 .ckpt 文件"),
    }
}

/// Loads weights saved by `save` (single or half precision).
pub fn load_own(path: &Path, dev: &Device) -> Result<MemNet> {
    let mut net = MemNet::new(&NetConfig::default(), dev);
    let mut store = SafetensorsStore::from_file(path);
    if is_half(path)? {
        store = store.with_from_adapter(burn_store::HalfPrecisionAdapter::new());
    }
    let r = net.load_from(&mut store).map_err(|e| anyhow!("读取膜分割模型失败：{e:?}"))?;
    if !r.missing.is_empty() {
        bail!("膜分割模型不完整（缺 {} 个参数）", r.missing.len());
    }
    Ok(net)
}

/// Saves the weights, as half precision with `half` (for publishing).
pub fn save(net: &MemNet, path: &Path, half: bool) -> Result<()> {
    let mut store = SafetensorsStore::from_file(path).overwrite(true);
    if half {
        store = store.with_to_adapter(burn_store::HalfPrecisionAdapter::new());
    }
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
/// (Mirrored axes x, y, z.)
const FLIPS: [[bool; 3]; 8] = [
    [false, false, false],
    [true, false, false],
    [false, true, false],
    [false, false, true],
    [true, true, false],
    [true, false, true],
    [false, true, true],
    [true, true, true],
];

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

/// Mirrors a volume (dims `n`, x fastest) along the axes in `m`.
fn mirror(v: Vec<f32>, n: [usize; 3], m: [bool; 3]) -> Vec<f32> {
    if m == [false; 3] {
        return v;
    }
    let mut out = vec![0f32; v.len()];
    out.par_chunks_mut(n[0] * n[1]).enumerate().for_each(|(z, plane)| {
        let sz = if m[2] { n[2] - 1 - z } else { z };
        for y in 0..n[1] {
            let sy = if m[1] { n[1] - 1 - y } else { y };
            let src = &v[(sz * n[1] + sy) * n[0]..][..n[0]];
            let dst = &mut plane[y * n[0]..][..n[0]];
            if m[0] {
                dst.iter_mut().zip(src.iter().rev()).for_each(|(d, s)| *d = *s);
            } else {
                dst.copy_from_slice(src);
            }
        }
    });
    out
}

/// How thoroughly `score` goes over the volume.
#[derive(Clone, Copy, Debug)]
pub enum Pass {
    /// Windows overlapping by about a third (about 30% fewer), as many
    /// mirrored copies beyond the first as fit in this many seconds (at most
    /// eight in all). Faster, but fewer copies agree noticeably less with
    /// MemBrain-seg (two copies: Dice 0.95), so only for comparisons.
    Budget(f64),
    /// As MemBrain-seg's own `segment`: windows overlapping by half (placed
    /// as MONAI's sliding window does), all eight copies. The app's: on a
    /// 240 × 160 × 128 tomogram every voxel's score was within 0.015 of
    /// MemBrain-seg 0.0.10 (Dice 0.99986).
    Official,
}

/// Starts of windows of `side` covering `n`, overlapping by about a third;
/// negative when the side is shorter than a window (centred, the rest
/// zeros, as in MemBrain-seg).
fn starts(n: usize, side: usize) -> Vec<isize> {
    if n <= side {
        return vec![-(((side - n) / 2) as isize)];
    }
    let k = (n - side).div_ceil(side * 2 / 3) + 1;
    (0..k).map(|i| ((n - side) as f64 * i as f64 / (k - 1) as f64).round() as isize).collect()
}

/// Starts as MONAI's sliding window (overlap ½) places them.
fn starts_official(n: usize, side: usize) -> Vec<isize> {
    if n <= side {
        return starts(n, side);
    }
    let step = side / 2;
    let k = (n - side).div_ceil(step) + 1;
    (0..k).map(|i| (i * step).min(n - side) as isize).collect()
}

/// Membrane scores (logits) of a standardised volume `v` (dims `n`, X
/// fastest) already at `PIXEL_A`, in `WINDOW`³ windows padded with zeros
/// where the volume is thinner (as MemBrain-seg; tomograms on the 10 Å
/// grid are often thinner than a window).
pub fn score(dev: &Device, net: &Model, v: &[f32], n: [usize; 3], pass: Pass, progress: &mut dyn FnMut(Progress)) -> Result<(Vec<f32>, Stats)> {
    let t0 = Instant::now();
    if !WINDOW.is_multiple_of(net.align()) {
        bail!("模型的窗口对齐不符");
    }
    let official = matches!(pass, Pass::Official);
    let side = [WINDOW; 3];
    let st: [Vec<isize>; 3] = std::array::from_fn(|a| if official { starts_official(n[a], side[a]) } else { starts(n[a], side[a]) });
    let windows = st[0].len() * st[1].len() * st[2].len();
    // Gaussian importance (σ = side/8) so window edges count little, at
    // least 1e-3 (as MONAI's sliding window, which MemBrain-seg uses: the
    // floor is on the 3D weight, not on each axis).
    let g: [Vec<f32>; 3] = std::array::from_fn(|a| {
        let s = side[a] as f32 / 8.0;
        let c = (side[a] as f32 - 1.0) / 2.0;
        (0..side[a]).map(|i| (-((i as f32 - c).powi(2)) / (2.0 * s * s)).exp()).collect()
    });
    let inside = |a: usize, o: isize, i: usize| -> Option<usize> { usize::try_from(o + i as isize).ok().filter(|&k| k < n[a]) };
    let tile = |o: [isize; 3]| -> Vec<f32> {
        let mut t = vec![0f32; side[0] * side[1] * side[2]];
        for z in 0..side[2] {
            let Some(zz) = inside(2, o[2], z) else { continue };
            for y in 0..side[1] {
                let Some(yy) = inside(1, o[1], y) else { continue };
                for x in 0..side[0] {
                    if let Some(xx) = inside(0, o[0], x) {
                        t[(z * side[1] + y) * side[0] + x] = v[(zz * n[1] + yy) * n[0] + xx];
                    }
                }
            }
        }
        t
    };
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
                    let clip = |a: usize, o: isize| -> (usize, usize) { (o.max(0) as usize, ((o + side[a] as isize).max(0) as usize).min(n[a])) };
                    let ((bx0, bx1), (by0, by1), (bz0, bz1)) = (clip(0, x0), clip(1, y0), clip(2, z0));
                    crate::live::with(|m| {
                        if m.grid == n {
                            m.busy = Some([bx0, by0, bz0, bx1, by1, bz1]);
                        }
                    });
                    // Mirrored and put in the network's axis order on the host
                    // (cheap next to the network).
                    let t = mirror(tile([x0, y0, z0]), side, FLIPS[c]);
                    let x = if net.xyz() {
                        Tensor::<5>::from_data(TensorData::new(transpose(&t, side), [1, 1, side[0], side[1], side[2]]), dev)
                    } else {
                        Tensor::<5>::from_data(TensorData::new(t, [1, 1, side[2], side[1], side[0]]), dev)
                    };
                    let r: Vec<f32> = net.forward(x).into_data().convert::<f32>().try_into_vec::<f32>().map_err(|e| anyhow!("{e:?}"))?;
                    let r = mirror(if net.xyz() { transpose(&r, [side[2], side[1], side[0]]) } else { r }, side, FLIPS[c]);
                    for z in 0..side[2] {
                        let Some(zz) = inside(2, z0, z) else { continue };
                        for y in 0..side[1] {
                            let Some(yy) = inside(1, y0, y) else { continue };
                            let wzy = g[2][z] * g[1][y];
                            let src = (z * side[1] + y) * side[0];
                            let dst = (zz * n[1] + yy) * n[0];
                            for x in 0..side[0] {
                                if let Some(xx) = inside(0, x0, x) {
                                    let w = (wzy * g[0][x]).max(1e-3);
                                    acc[dst + xx] += w * r[src + x];
                                    wsum[dst + xx] += w;
                                }
                            }
                        }
                    }
                    // What the window found so far, for the viewer (`live`).
                    crate::live::with(|m| {
                        if m.grid != n {
                            return;
                        }
                        for z in bz0..bz1 {
                            for y in by0..by1 {
                                let i = (z * n[1] + y) * n[0];
                                for x in bx0..bx1 {
                                    m.cells[i + x] = u8::from(acc[i + x] > 0.0);
                                }
                            }
                        }
                    });
                    done += 1;
                    progress(Progress { done, total: windows * copies });
                }
            }
        }
        if let (0, Pass::Budget(budget_s)) = (c, pass) {
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
    crate::live::with(|m| m.busy = None);
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
/// voxel).
pub fn segment(dev: &Device, net: &Model, values: &[f32], n: [usize; 3], pixel_a: f32, pass: Pass, progress: &mut dyn FnMut(Progress)) -> Result<Segmentation> {
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
    let (logits, stats) = score(dev, net, &small, m, pass, progress)?;
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

/// Material the vesicles found among the membranes go to.
pub const VESICLE_MATERIAL: &str = "囊泡";
pub const VESICLE_COLOR: [u8; 3] = [92, 190, 60];

/// Material microtubules (`crate::filaments`) go to.
pub const TUBE_MATERIAL: &str = "微管";
pub const TUBE_COLOR: [u8; 3] = [240, 110, 20];

/// Writes `mask` (dims `n`, `pixel_a` Å per voxel) into `l` in one undo step:
/// vesicles (`vesicles::find`) as "囊泡", the other membranes as "膜", and
/// the microtubule voxels `tubes` (when given) as "微管". Returns the changed
/// slices, the membrane label and the vesicles.
pub fn apply(l: &mut crate::labels::Labels, mask: &[bool], tubes: Option<&[bool]>, n: [usize; 3], pixel_a: f32, guard: u16) -> (Vec<usize>, u16, Vec<vesicles::Vesicle>) {
    let (ves, is_ves) = vesicles::find(mask, n, pixel_a);
    let label = l.material_label(MATERIAL, COLOR);
    let vlabel = if ves.is_empty() { 0 } else { l.material_label(VESICLE_MATERIAL, VESICLE_COLOR) };
    let tubes = tubes.filter(|t| t.iter().any(|&b| b));
    let tlabel = if tubes.is_some() { l.material_label(TUBE_MATERIAL, TUBE_COLOR) } else { 0 };
    let plane = n[0] * n[1];
    let tube = |i: usize| tubes.is_some_and(|t| t[i]);
    let classes: Vec<(usize, Vec<u16>)> = (0..n[2])
        .filter(|&z| (z * plane..(z + 1) * plane).any(|i| mask[i] || tube(i)))
        .map(|z| {
            let class = |i: usize| if is_ves[i] { vlabel } else if mask[i] { label } else if tube(i) { tlabel } else { 0 };
            (z, (z * plane..(z + 1) * plane).map(class).collect())
        })
        .collect();
    (l.apply_classes(&classes, guard), label, ves)
}

/// Writes `mask` and `tubes` (dims `n`) into the label map beside `image`
/// (see `apply`), keeping labels already there; returns the label file.
pub fn write_labels(image: &str, n: [usize; 3], mask: &[bool], tubes: Option<&[bool]>, pixel_a: f32) -> Result<String> {
    let path = crate::labels::labels_path(image, 0);
    let mut l = if Path::new(&path).exists() {
        crate::labels::Labels::load(Path::new(&path), n[0], n[1], n[2])?
    } else {
        crate::labels::Labels::new(n[0], n[1], n[2])
    };
    apply(&mut l, mask, tubes, n, pixel_a, 0);
    l.save(Path::new(&path))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_half() {
        let dev = Device::flex();
        let net = MemNet::new(&NetConfig::default(), &dev);
        let dir = std::env::temp_dir();
        let (a, b) = (dir.join("tmv-mem-f32.safetensors"), dir.join("tmv-mem-f16.safetensors"));
        save(&net, &a, false).unwrap();
        save(&net, &b, true).unwrap();
        assert!(!is_half(&a).unwrap() && is_half(&b).unwrap());
        assert!(std::fs::metadata(&b).unwrap().len() * 3 < std::fs::metadata(&a).unwrap().len() * 2);
        let x = Tensor::<5>::random([1, 1, 32, 32, 32], burn::tensor::Distribution::Normal(0.0, 1.0), &dev);
        let (p, q) = (load_own(&a, &dev).unwrap().forward(x.clone()), load_own(&b, &dev).unwrap().forward(x));
        assert!((p - q).abs().max().into_scalar::<f32>() < 0.05);
    }

    #[test]
    fn transpose_reverses_axes() {
        let n = [4, 3, 2];
        let v: Vec<f32> = (0..24).map(|i| i as f32).collect();
        let t = transpose(&v, n);
        // v[x + 4 (y + 3 z)] lands at t[z + 2 (y + 3 x)].
        assert_eq!(t[1 + 2 * (2 + 3 * 3)], v[3 + 4 * (2 + 3)]);
        assert_eq!(transpose(&t, [2, 3, 4]), v);
    }

    #[test]
    fn mirror_twice_is_identity() {
        let n = [5, 4, 3];
        let v: Vec<f32> = (0..60).map(|i| i as f32).collect();
        for m in FLIPS {
            assert_eq!(mirror(mirror(v.clone(), n, m), n, m), v);
        }
        assert_eq!(mirror(v.clone(), n, [true, false, true])[0], v[4 + 5 * 4 * 2]);
    }

    #[test]
    fn starts_cover() {
        assert_eq!(starts(100, 160), vec![-30]);
        assert_eq!(starts(160, 160), vec![0]);
        let s = starts(400, 160);
        assert_eq!(*s.last().unwrap(), 240);
        assert!(s.windows(2).all(|w| w[1] - w[0] <= 107));
        assert_eq!(starts(837, 160).len(), 8);
        assert_eq!(starts_official(128, 160), vec![-16]);
        assert_eq!(starts_official(400, 160), vec![0, 80, 160, 240]);
        assert_eq!(starts_official(410, 160), vec![0, 80, 160, 240, 250]);
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

    /// `score` draws what each window found into the live map (`live`) and
    /// marks the window it is reading.
    #[test]
    fn score_fills_the_live_map() {
        let _one = crate::live::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dev = Device::flex();
        let cfg = NetConfig { widths: vec![2, 4, 4], blocks: vec![1, 1, 1], heads: 1 };
        let net = Model::Own(MemNet::new(&cfg, &dev));
        let n = [WINDOW + 40, WINDOW, 8];
        let v: Vec<f32> = (0..n[0] * n[1] * n[2]).map(|i| ((i % 7) as f32 - 3.0) / 3.0 + ((i / n[0]) % 5) as f32 * 0.2).collect();
        crate::live::start(1, n, n, vec![[0; 4], [255, 196, 0, 255]]);
        let mut boxes = vec![];
        let (logits, stats) = score(&dev, &net, &v, n, Pass::Budget(0.0), &mut |_| {
            boxes.push(crate::live::MAP.lock().unwrap().as_ref().unwrap().busy);
        })
        .unwrap();
        assert_eq!((stats.windows, stats.copies), (2, 1));
        // The windows read, in order, clipped to the volume.
        assert_eq!(boxes, vec![Some([0, 0, 0, WINDOW, WINDOW, 8]), Some([40, 0, 0, WINDOW + 40, WINDOW, 8])]);
        let map = crate::live::MAP.lock().unwrap().take().unwrap();
        assert_eq!(map.busy, None);
        let want: Vec<u8> = logits.iter().map(|&s| u8::from(s > 0.0)).collect();
        assert_eq!(map.cells, want);
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
        assert_eq!(net.forward(x.clone()).dims(), [1, 1, 8, 12, 16]);
        let l = net::loss(o[0].clone(), Tensor::<5>::zeros([1, 1, 8, 12, 16], &dev));
        assert!(l.into_scalar::<f32>().is_finite());
        let y = Tensor::<5>::ones([1, 1, 8, 12, 16], &dev);
        let s = net::skeleton_loss(o[0].clone(), y);
        assert!(s.into_scalar::<f32>().is_finite());
    }
}
