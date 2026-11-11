//! Membrane segmentation for tomograms (Pro), modelled on MemBrain-seg, the
//! segmentation part of MemBrain v2 (Lamm et al., bioRxiv 2024), written here
//! from the paper, its documentation and the layout of its checkpoints; no
//! code or weights are taken from it.
//!
//! As there: the tomogram is standardised; native-grid patches are Fourier
//! rescaled to the 10 Å network scale. A
//! residual 3D U-Net (instance norm, deep supervision) scores every
//! voxel in overlapping 160³ windows weighted by a Gaussian, the scores of
//! eight mirrored copies are averaged (test-time augmentation), and voxels with a
//! positive score are membrane. Small specks are removed and the result is
//! kept on the tomogram's own grid.
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
mod rescale;
pub mod vesicles;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
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
    // The command-line tool (`crate::cli`) can name the model for one run.
    if let Some(p) = std::env::var_os("TMV_MEMBRANE_MODEL").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
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
    resample_linear(v, n, m, true)
}

fn resample_linear(v: &[f32], n: [usize; 3], m: [usize; 3], antialias: bool) -> Vec<f32> {
    if n == m {
        return v.to_vec();
    }
    let f: [f32; 3] = std::array::from_fn(|a| n[a] as f32 / m[a] as f32);
    let sigma = f.iter().cloned().fold(0f32, f32::max);
    let src;
    let v = if antialias && sigma > 1.2 {
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
    /// Why fewer copies were averaged than set out for, when the GPU
    /// failed after at least one whole copy: what the finished copies gave
    /// is the result (each is a whole segmentation; more only steady it),
    /// not hours of work thrown away.
    pub cut_short: Option<String>,
    pub windows: usize,
    /// Grid the network saw.
    pub grid: [usize; 3],
    pub ms: f64,
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
    /// 512 × 512 × 128 tomogram (36 windows) every voxel's score was within
    /// 1e-4 of MemBrain-seg 0.0.10 in single precision, no voxel decided
    /// differently.
    Official,
    /// As `Official`, with this many of the eight mirrored copies (1 to
    /// 8): the same windows on the same grid, an eighth of the time per
    /// copy left out.
    Copies(usize),
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
    score_patches(dev, net, v, n, pass, WINDOW, progress)
}

fn score_patches(dev: &Device, net: &Model, v: &[f32], n: [usize; 3], pass: Pass, patch: usize, progress: &mut dyn FnMut(Progress)) -> Result<(Vec<f32>, Stats)> {
    // Clear the busy marker on success, cancellation and a failed GPU read.
    struct ClearBusy;
    impl Drop for ClearBusy {
        fn drop(&mut self) {
            crate::live::with(|lv| lv.busy = None);
        }
    }
    let _clear_busy = ClearBusy;
    let t0 = Instant::now();
    if !WINDOW.is_multiple_of(net.align()) {
        bail!("模型的窗口对齐不符");
    }
    let official = matches!(pass, Pass::Official | Pass::Copies(_));
    let side = [patch; 3];
    let resize = (patch != WINDOW).then(|| rescale::FourierResize::new(patch, WINDOW));
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
    // Where tile voxel (x, y, z) sits in the network's input and output,
    // mirrored by `m`: in its axis order (`Model::xyz`), so the tile is
    // gathered and scattered in one pass on the host.
    let xyz = net.xyz();
    let at = |m: [bool; 3], x: usize, y: usize, z: usize| -> usize {
        let f = |a: usize, i: usize| if m[a] { side[a] - 1 - i } else { i };
        let (x, y, z) = (f(0, x), f(1, y), f(2, z));
        if xyz {
            (x * side[1] + y) * side[2] + z
        } else {
            (z * side[1] + y) * side[0] + x
        }
    };
    let tile = |o: [isize; 3], m: [bool; 3]| -> Vec<f32> {
        // Filled in the network's order: its slowest axis in parallel.
        let ax = if xyz { [0, 1, 2] } else { [2, 1, 0] };
        let f = |a: usize, i: usize| if m[a] { side[a] - 1 - i } else { i };
        let mut t = vec![0f32; side[0] * side[1] * side[2]];
        t.par_chunks_mut(side[ax[1]] * side[ax[2]]).enumerate().for_each(|(i, slab)| {
            let mut l = [0usize; 3];
            l[ax[0]] = f(ax[0], i);
            for j in 0..side[ax[1]] {
                l[ax[1]] = f(ax[1], j);
                for k in 0..side[ax[2]] {
                    l[ax[2]] = f(ax[2], k);
                    if let (Some(xx), Some(yy), Some(zz)) = (inside(0, o[0], l[0]), inside(1, o[1], l[1]), inside(2, o[2], l[2])) {
                        slab[j * side[ax[2]] + k] = v[(zz * n[1] + yy) * n[0] + xx];
                    }
                }
            }
        });
        t
    };
    let fine_tile = |o: [isize; 3], mirrored: [bool; 3]| -> Vec<f32> {
        // Array axes are fast→slow. MemBrain tensors are [x,y,z], so their
        // fastest array axis is physical Z rather than physical X.
        let axes = if xyz { [2, 1, 0] } else { [0, 1, 2] };
        let bounds = std::array::from_fn(|q| {
            let a = axes[q];
            let start = (-o[a]).clamp(0, patch as isize) as usize;
            let end = (n[a] as isize - o[a]).clamp(0, patch as isize) as usize;
            if mirrored[a] { patch - end..patch - start } else { start..end }
        });
        resize.as_ref().unwrap().shrink_window(bounds, |i, j, k| {
            let mut local = [0; 3];
            for (q, value) in [i, j, k].into_iter().enumerate() {
                let a = axes[q];
                local[a] = if mirrored[a] { patch - 1 - value } else { value };
            }
            let [x, y, z] = std::array::from_fn(|a| (o[a] + local[a] as isize) as usize);
            v[(z * n[1] + y) * n[0] + x]
        })
    };
    // Interpolate only native voxels actually covered by this window, rather
    // than allocating a fine-grid patch³ logit cube (which can exceed100GB).
    let fine_axis: Vec<_> = if patch > WINDOW {
        let factor = WINDOW as f32 / patch as f32;
        (0..patch).map(|i| {
            let p = ((i as f32 + 0.5) * factor - 0.5).clamp(0.0, (WINDOW - 1) as f32);
            let lo = p.floor() as usize;
            (lo, (lo + 1).min(WINDOW - 1), p - lo as f32)
        }).collect()
    } else { Vec::new() };
    let len = n[0] * n[1] * n[2];
    let (mut acc, mut wsum) = (vec![0f32; len], vec![0f32; len]);
    // Each copy is blended on its own and the copies averaged (as
    // MemBrain-seg): mirrored windows sit elsewhere, so their weights
    // differ from copy to copy.
    let mut total = vec![0f32; len];
    let mut copies = if let Pass::Copies(k) = pass { k.clamp(1, 8) } else { 8 };
    let mut done = 0;
    let mut c = 0;
    let mut cut_short = None;
    'copies: while c < copies {
        // MemBrain-seg mirrors the whole volume and then places its
        // windows, so along a mirrored axis they sit mirrored too (the same
        // places only when they are spread symmetrically).
        let m = FLIPS[c];
        let sc: [Vec<isize>; 3] = std::array::from_fn(|a| st[a].iter().map(|&s| if m[a] { n[a] as isize - side[a] as isize - s } else { s }).collect());
        for &z0 in &sc[2] {
            for &y0 in &sc[1] {
                for &x0 in &sc[0] {
                    check_cancel()?;
                    let clip = |a: usize, o: isize| -> (usize, usize) { (o.max(0) as usize, ((o + side[a] as isize).max(0) as usize).min(n[a])) };
                    let ((bx0, bx1), (by0, by1), (bz0, bz1)) = (clip(0, x0), clip(1, y0), clip(2, z0));
                    crate::live::with(|lv| {
                        if lv.grid == n {
                            lv.busy = Some([bx0, by0, bz0, bx1, by1, bz1]);
                        }
                    });
                    let dims = [1, 1, WINDOW, WINDOW, WINDOW];
                    let r = read_window(|| {
                        let input = if patch > WINDOW {
                            fine_tile([x0, y0, z0], m)
                        } else {
                            let input = tile([x0, y0, z0], m);
                            if let Some(resize) = &resize { resize.run(&input) } else { input }
                        };
                        let x = Tensor::<5>::from_data(TensorData::new(input, dims), dev);
                        net.forward(x).try_into_data().map_err(|e| anyhow!("{e}"))
                    });
                    #[cfg(test)]
                    let r = if tests::FAIL_AT.with(|f| f.get()) == Some(done) { Err(anyhow!("the GPU went away")) } else { r };
                    let r = match r {
                        Ok(r) => r,
                        // The copies already finished are a result.
                        Err(e) if c >= 1 => {
                            cut_short = Some(format!("膜分割在第 {}/{copies} 遍中途计算失败（窗口 {}/{}），用的是前 {c} 遍的结果：{e:#}", c + 1, done + 1, windows * copies));
                            break 'copies;
                        }
                        Err(e) => return Err(e).with_context(|| format!("膜分割计算失败（窗口 {}/{}），结果未保存", done + 1, windows * copies)),
                    };
                    let r = if patch < WINDOW { resample_linear(&r, [WINDOW; 3], side, false) } else { r };
                    let value = |x: usize, y: usize, z: usize| {
                        if patch <= WINDOW { return r[at(m, x, y, z)]; }
                        let mut q = [x, y, z];
                        for a in 0..3 { if m[a] { q[a] = patch - 1 - q[a]; } }
                        if xyz { q.swap(0, 2); }
                        let [(x0, x1, tx), (y0, y1, ty), (z0, z1, tz)] = q.map(|i| fine_axis[i]);
                        let at = |x, y, z| r[(z * WINDOW + y) * WINDOW + x];
                        let l = |x| {
                            let a = at(x, y0, z0) * (1.0 - ty) + at(x, y1, z0) * ty;
                            let b = at(x, y0, z1) * (1.0 - ty) + at(x, y1, z1) * ty;
                            a * (1.0 - tz) + b * tz
                        };
                        l(x0) * (1.0 - tx) + l(x1) * tx
                    };
                    // Each volume plane the window covers, in parallel.
                    let plane = n[0] * n[1];
                    acc.par_chunks_mut(plane).zip(wsum.par_chunks_mut(plane)).enumerate().for_each(|(zz, (pa, pw))| {
                        let z = zz as isize - z0;
                        if z < 0 || z >= side[2] as isize {
                            return;
                        }
                        let z = z as usize;
                        let yr = if patch > WINDOW { (by0 as isize - y0) as usize..(by1 as isize - y0) as usize } else { 0..side[1] };
                        let xr = if patch > WINDOW { (bx0 as isize - x0) as usize..(bx1 as isize - x0) as usize } else { 0..side[0] };
                        for y in yr {
                            let Some(yy) = inside(1, y0, y) else { continue };
                            let wzy = g[2][z] * g[1][y];
                            for x in xr.clone() {
                                if let Some(xx) = inside(0, x0, x) {
                                    let w = (wzy * g[0][x]).max(1e-3);
                                    pa[yy * n[0] + xx] += w * value(x, y, z);
                                    pw[yy * n[0] + xx] += w;
                                }
                            }
                        }
                    });
                    // What the window found so far, for the viewer (`live`).
                    crate::live::with(|lv| {
                        if lv.grid != n {
                            return;
                        }
                        for z in bz0..bz1 {
                            for y in by0..by1 {
                                let i = (z * n[1] + y) * n[0];
                                for x in bx0..bx1 {
                                    lv.cells[i + x] = u8::from(acc[i + x] > 0.0);
                                }
                            }
                        }
                    });
                    done += 1;
                    progress(Progress { done, total: windows * copies });
                }
            }
        }
        total.par_iter_mut().zip(acc.par_iter_mut().zip(wsum.par_iter_mut())).for_each(|(t, (a, w))| {
            *t += *a / w.max(1e-12);
            (*a, *w) = (0.0, 0.0);
        });
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
    if cut_short.is_some() {
        copies = c;
    }
    total.par_iter_mut().for_each(|t| *t /= copies as f32);
    Ok((total, Stats { copies, cut_short, windows, grid: n, ms: t0.elapsed().as_secs_f64() * 1e3 }))
}

/// A failed readback must remain an error, before blending or label writes.
/// Burn can also panic during a launch, before its fallible readback runs.
fn read_window(forward: impl FnOnce() -> Result<TensorData>) -> Result<Vec<f32>> {
    let data = crate::ctf_gpu::guarded(forward)??;
    let values: Vec<f32> = data.convert::<f32>().try_into_vec().map_err(|e| anyhow!("{e:?}"))?;
    if values.iter().any(|v| !v.is_finite()) {
        bail!("膜分割产生了非有限数值");
    }
    Ok(values)
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

/// Mean and spread of every voxel (sums in f64).
fn exact_stats(v: &[f32]) -> (f32, f32) {
    let (s, s2) = v.par_iter().fold(|| (0f64, 0f64), |(a, b), &x| (a + x as f64, b + x as f64 * x as f64)).reduce(|| (0.0, 0.0), |p, q| (p.0 + q.0, p.1 + q.1));
    let k = v.len().max(1) as f64;
    let mean = s / k;
    let sd = (s2 / k - mean * mean).max(0.0).sqrt();
    (mean as f32, if sd > 1e-12 { sd as f32 } else { 1.0 })
}

/// NumPy rounds to nearest (ties to even), then MemBrain rounds odd sides
/// upward so Fourier padding is symmetric. The validated pixel size >0.5 Å
/// bounds the native side to3200; do not silently clamp fine data to160.
fn physical_patch(pixel_a: f32) -> usize {
    let rounded = (WINDOW as f64 * PIXEL_A as f64 / pixel_a as f64).round_ties_even() as usize;
    (rounded + rounded % 2).max(2)
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
    let m = grid_at(n, pixel_a);
    if m.iter().any(|&k| k < 8) {
        bail!("体积太小：按 10 Å 体素算每边不到 8 个");
    }
    // MemBrain's rescale-patches operates on the native grid: normalize the
    // original tomogram once, FFT-resize each physical patch to 160³, and
    // resize its logits back before Gaussian blending. Whole-volume spatial
    // interpolation changes both the input spectrum and window positions.
    if matches!(net, Model::MemBrain(_)) && matches!(pass, Pass::Official | Pass::Copies(_)) && pixel_a != PIXEL_A {
        let patch = physical_patch(pixel_a);
        let (mean, std) = exact_stats(values);
        let norm: Vec<f32> = values.par_iter().map(|&x| if x.is_finite() { (x-mean)/std } else { 0.0 }).collect();
        // The desktop initially allocates its historical 10 Å preview grid.
        // Keep live windows and final labels on this inference grid instead.
        crate::live::with(|map| {
            if map.full == n && map.grid != n {
                map.grid = n;
                map.cells = vec![0; values.len()];
                map.busy = None;
            }
        });
        let (score, stats) = score_patches(dev, net, &norm, n, pass, patch, progress)?;
        let min = (MIN_NM3 / (pixel_a/10.0).powi(3)).round().max(1.0) as usize;
        let (mask, components) = clean(&score.iter().map(|&s| s>0.0).collect::<Vec<_>>(), n, min);
        return Ok(Segmentation { score, mask, components, stats });
    }
    let (mean, std) = crate::wedge::stats(values);
    let norm: Vec<f32> = values.par_iter().map(|&x| if x.is_finite() { (x - mean) / std } else { 0.0 }).collect();
    let small = resample(&norm, n, m);
    drop(norm);
    // Standardise again at the network's scale (the low-pass narrows it),
    // over every voxel: a sample (every 16th of 512² × 128) shifted the
    // scores enough to cost 0.01 Dice against MemBrain-seg.
    let (m2, s2) = exact_stats(&small);
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
/// Always writes a stack, including when there are no labels.
pub fn write_labels(image: &str, n: [usize; 3], mask: &[bool], tubes: Option<&[bool]>, pixel_a: f32) -> Result<String> {
    Ok(write_labels_found(image, n, mask, tubes, pixel_a)?.0)
}

/// `write_labels`, also returning the vesicles found.
fn write_labels_found(image: &str, n: [usize; 3], mask: &[bool], tubes: Option<&[bool]>, pixel_a: f32) -> Result<(String, Vec<vesicles::Vesicle>)> {
    let path = crate::labels::labels_path(image, 0);
    let mut l = if Path::new(&path).exists() {
        crate::labels::Labels::load(Path::new(&path), n[0], n[1], n[2])?
    } else {
        crate::labels::Labels::new(n[0], n[1], n[2])
    };
    let (_, _, ves) = apply(&mut l, mask, tubes, n, pixel_a, 0);
    l.write_stack(Path::new(&path))?;
    Ok((path, ves))
}

/// What `segment_file` found and wrote.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    /// The tomogram segmented.
    pub image: String,
    /// Its label map (materials 膜, 囊泡, 微管), which the app opens with it.
    pub labels: String,
    /// `<image>.membranes.json`: this result.
    pub summary: String,
    /// `<image>.vesicles.csv`: one vesicle per row.
    pub vesicles_csv: String,
    pub pixel_a: f64,
    pub shape: [usize; 3],
    pub components: usize,
    pub voxels: usize,
    pub vesicles: Vec<vesicles::Vesicle>,
    /// Lengths of the microtubules found, nm.
    pub microtubules: Vec<f32>,
    pub seconds: f64,
    /// Mirrored copies the segmentation averaged (8 when all went well).
    pub copies: usize,
    /// Set when the GPU failed part of the way and the result is from the
    /// copies finished before (`Stats::cut_short`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Segments the membranes of the tomogram file `image` with the model
/// file `model`, finds vesicles and microtubules, and writes the label map
/// beside it with a summary (`<image>.membranes.json`) and a vesicle table
/// (`<image>.vesicles.csv`). `progress` gets (stage, done, total).
pub fn segment_file(dev: &Device, model: &Path, image: &str, progress: &(dyn Fn(&str, usize, usize) + Sync)) -> Result<FileResult> {
    let now = Instant::now();
    CANCEL.store(false, Ordering::SeqCst);
    progress("膜分割：读取", 0, 1);
    let (v, shape, px) = crate::tomo::pipeline::load(image)?;
    if shape[2] < 8 {
        bail!("膜分割需要三维断层（至少 8 层）");
    }
    if (px - 1.0).abs() < 1e-6 {
        bail!("文件里没有像素大小，无法换算到 10 Å 体素");
    }
    let net = load(model, dev)?;
    crate::live::show_section("膜分割：中间层", &v, shape, shape[2] / 2, &[]);
    let seg = segment(dev, &net, &v, shape, px as f32, Pass::Official, &mut |p| progress("膜分割", p.done, p.total))?;
    crate::live::show_section("膜分割：中间层", &v, shape, shape[2] / 2, &seg.mask);
    progress("膜分割：微管", 0, 1);
    let fils = crate::filaments::find(&v, shape, px as f32);
    let tubes = crate::filaments::mask(&fils, shape, px as f32);
    drop(v);
    progress("膜分割：囊泡", 0, 1);
    let (labels, vesicles) = write_labels_found(image, shape, &seg.mask, Some(&tubes), px as f32)?;
    let summary = format!("{image}.membranes.json");
    let vesicles_csv = format!("{image}.vesicles.csv");
    let mut csv = String::from("id,x_px,y_px,z_px,diameter_nm,voxels\n");
    for (i, ve) in vesicles.iter().enumerate() {
        csv += &format!("{},{:.1},{:.1},{:.1},{:.2},{}\n", i + 1, ve.center[0], ve.center[1], ve.center[2], ve.diameter, ve.voxels);
    }
    std::fs::write(&vesicles_csv, csv)?;
    let r = FileResult {
        image: image.into(),
        labels,
        summary: summary.clone(),
        vesicles_csv,
        pixel_a: px,
        shape,
        components: seg.components,
        voxels: seg.mask.iter().filter(|&&b| b).count(),
        vesicles,
        microtubules: fils.iter().map(|f| f.length).collect(),
        seconds: now.elapsed().as_secs_f64(),
        copies: seg.stats.copies,
        note: seg.stats.cut_short.clone(),
    };
    std::fs::write(&summary, serde_json::to_vec_pretty(&r)?)?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_output_empty_prediction_is_loadable() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("empty.mrc");
        let path = write_labels(image.to_str().unwrap(), [9, 7, 5], &[false; 315], None, 10.0).unwrap();
        let labels = crate::labels::Labels::load(Path::new(&path), 9, 7, 5)
            .expect("the returned label path must contain a readable stack even without detections");
        assert!(labels.is_empty());
        assert!(crate::labels::Labels::load(Path::new(&path), 9, 7, 6).is_err(), "the stack must have exactly five slices");
    }

    #[test]
    fn file_output_empty_prediction_preserves_existing_labels() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("annotated.mrc");
        let mut existing = crate::labels::Labels::new(9, 7, 5);
        let mut annotation = [false; 315];
        annotation[314] = true;
        existing.apply_volume_mask(&annotation, 1234, 0);
        let path = crate::labels::labels_path(image.to_str().unwrap(), 0);
        existing.save(Path::new(&path)).unwrap();

        let path = write_labels(image.to_str().unwrap(), [9, 7, 5], &[false; 315], None, 10.0).unwrap();
        let labels = crate::labels::Labels::load(Path::new(&path), 9, 7, 5).unwrap();
        assert_eq!(labels.get(8, 6, 4), 1234);
        assert_eq!(labels.counts(), existing.counts());
    }

    #[test]
    fn window_failure_is_an_error_before_results_are_used() {
        let failed = read_window(|| bail!("device lost during Metal command"));
        assert!(format!("{:#}", failed.unwrap_err()).contains("device lost"));
        let panicked = read_window(|| panic!("failed GPU launch"));
        assert!(format!("{:#}", panicked.unwrap_err()).contains("failed GPU launch"));
        assert!(read_window(|| Ok(TensorData::new(vec![f32::NAN], [1]))).is_err());
        // An empty membrane is legitimate. Do not reject constant finite logits
        // in place of propagating the actual execution failure.
        assert_eq!(read_window(|| Ok(TensorData::new(vec![-1.0f32; 8], [8]))).unwrap(), vec![-1.0; 8]);
    }

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
    fn native_patch_side_matches_reference_rounding() {
        assert_eq!(physical_patch(13.4807968), 120);
        assert_eq!(physical_patch(15.252), 106);
        assert_eq!(physical_patch(10.0), 160);
        assert_eq!(physical_patch(4.5527935), 352);
        assert_eq!(physical_patch(8.0), 200);
        assert_eq!(physical_patch(0.5001), 3200);
        assert_eq!(physical_patch(12.8), 126);
        assert_eq!(physical_patch(32.0), 50);
        assert_eq!(physical_patch(128.0), 12); // exact12.5: NumPy ties-to-even
        assert_eq!(physical_patch(f32::from_bits(10.0f32.to_bits()+1)), 160);
        assert_eq!(physical_patch(f32::MAX), 2);
        for pixel in [10.00001,12.0,15.252,100.0,1000.0,f32::MAX] {
            let side=physical_patch(pixel);
            assert!((2..=WINDOW).contains(&side) && side.is_multiple_of(2));
        }
    }

    #[test]
    fn patch_logits_use_half_pixel_linear_without_antialias() {
        let input: Vec<_>=(0..64).map(|i| (i%4+10*(i/4%4)+100*(i/16)) as f32).collect();
        let out=resample_linear(&input,[4;3],[2;3],false);
        assert_eq!(out,vec![55.5,57.5,75.5,77.5,255.5,257.5,275.5,277.5]);
        let up=resample_linear(&[0.0,1.0,10.0,11.0,100.0,101.0,110.0,111.0],[2;3],[4;3],false);
        assert_eq!(up[0],0.0);
        assert_eq!(up[21],27.75);
        assert_eq!(up[63],111.0);
    }

    #[test]
    fn resample_keeps_constant() {
        let v = vec![3.0f32; 20 * 20 * 20];
        let r = resample(&v, [20; 3], [7, 9, 11]);
        assert!(r.iter().all(|&x| (x - 3.0).abs() < 1e-4));
    }

    thread_local! {
        /// The window (counted from 0 over all copies) at which `score`
        /// is made to fail on this thread.
        pub(super) static FAIL_AT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }

    /// A GPU failure after a whole mirrored copy leaves that copy's
    /// segmentation as the result; one in the first copy is an error.
    #[test]
    fn a_failure_after_the_first_copy_keeps_what_is_finished() {
        let _one = crate::live::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dev = Device::flex();
        let cfg = NetConfig { widths: vec![2, 4, 4], blocks: vec![1, 1, 1], heads: 1 };
        let net = Model::Own(MemNet::new(&cfg, &dev));
        let n = [WINDOW, WINDOW, 8];
        let v: Vec<f32> = (0..n[0] * n[1] * n[2]).map(|i| ((i % 7) as f32 - 3.0) / 3.0 + ((i / n[0]) % 5) as f32 * 0.2).collect();
        let (whole, stats) = score(&dev, &net, &v, n, Pass::Budget(0.0), &mut |_| {}).unwrap();
        assert_eq!((stats.windows, stats.copies, stats.cut_short.is_some()), (1, 1, false));
        // All eight copies set out for; the second one's window fails.
        FAIL_AT.with(|f| f.set(Some(1)));
        let (part, stats) = score(&dev, &net, &v, n, Pass::Official, &mut |_| {}).unwrap();
        assert_eq!(stats.copies, 1);
        let why = stats.cut_short.expect("said");
        assert!(why.contains("第 2/8 遍") && why.contains("前 1 遍"), "{why}");
        assert_eq!(part, whole);
        FAIL_AT.with(|f| f.set(Some(0)));
        let failed = score(&dev, &net, &v, n, Pass::Official, &mut |_| {});
        FAIL_AT.with(|f| f.set(None));
        assert!(format!("{:#}", failed.err().expect("an error")).contains("结果未保存"));
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
