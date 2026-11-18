//! Self-supervised denoising (Pro): a small U-Net trained on the open
//! image itself, with no clean data, then used like the other denoisers.
//!
//! Three ways to get a training signal from noisy data alone:
//! - "n2v" (Noise2Void): any image. In every training patch a few pixels
//!   (one per 8 × 8 cell) are replaced by a random neighbour within two
//!   pixels, and the network learns to predict their original values from
//!   the surroundings. Noise that is independent from pixel to pixel cannot
//!   be predicted that way, so what it learns is the signal.
//! - "frames" (Noise2Noise on a movie): the mean of the even frames is
//!   mapped to the mean of the odd ones and back. Both see the same signal
//!   with independent noise, so the best guess of one from the other is the
//!   signal. The result is the mean of the two halves, each denoised.
//! - "neighbours" (Noise2Noise along Z): slice z is mapped to slice z ± 1,
//!   for tomograms and stacks whose slices are thinner than the detail.
//!
//! Values are standardised with the training data's mean and spread; the
//! loss is the mean squared error (over the masked pixels for n2v).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use burn::module::Module;
use burn::nn::conv::{Conv2d, Conv2dConfig};
use burn::nn::PaddingConfig2d;
use burn::optim::AdamConfig;
use burn::tensor::activation::relu;
use burn::tensor::module::{interpolate, max_pool2d};
use burn::tensor::ops::{InterpolateMode, InterpolateOptions, MaxPoolOptions};
use burn::tensor::{Device, Tensor, TensorData};
use serde::{Deserialize, Serialize};

/// Two 3×3 convolutions with ReLU. No normalisation layers: they would make
/// each pixel depend on the whole tile, which breaks tiled inference.
#[derive(Module, Debug)]
struct Block {
    c1: Conv2d,
    c2: Conv2d,
}

impl Block {
    fn new(i: usize, o: usize, dev: &Device) -> Block {
        let conv = |i, o| Conv2dConfig::new([i, o], [3, 3]).with_padding(PaddingConfig2d::Same).init(dev);
        Block { c1: conv(i, o), c2: conv(o, o) }
    }

    fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        relu(self.c2.forward(relu(self.c1.forward(x))))
    }
}

/// U-Net of depth 2 (as Noise2Void uses), widths 32, 64, 128. Its
/// receptive field reaches about 20 pixels, inside `MARGIN`.
#[derive(Module, Debug)]
pub struct Net {
    down: Vec<Block>,
    bottom: Block,
    up: Vec<Block>,
    head: Conv2d,
}

const LEVELS: usize = 2;
const BASE: usize = 32;
/// Sides fed to the network are multiples of this (2^LEVELS).
const ALIGN: usize = 1 << LEVELS;

impl Net {
    pub fn new(dev: &Device) -> Net {
        let w = |l: usize| BASE << l;
        let down = (0..LEVELS).map(|l| Block::new(if l == 0 { 1 } else { w(l - 1) }, w(l), dev)).collect();
        let bottom = Block::new(w(LEVELS - 1), w(LEVELS), dev);
        let up = (0..LEVELS).rev().map(|l| Block::new(w(l + 1) + w(l), w(l), dev)).collect();
        Net { down, bottom, up, head: Conv2dConfig::new([BASE, 1], [1, 1]).init(dev) }
    }

    /// [B, 1, H, W] → [B, 1, H, W], H and W multiples of ALIGN.
    pub fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let mut skips = vec![];
        let mut x = x;
        for b in &self.down {
            x = b.forward(x);
            skips.push(x.clone());
            x = max_pool2d(x, MaxPoolOptions::new([2, 2]).with_stride([2, 2]));
        }
        x = self.bottom.forward(x);
        for b in &self.up {
            let skip = skips.pop().unwrap();
            let [_, _, h, w] = skip.dims();
            x = interpolate(x, InterpolateOptions::new(InterpolateMode::Nearest).with_output_size([h, w]));
            x = b.forward(Tensor::cat(vec![x, skip], 1));
        }
        self.head.forward(x)
    }
}

/// Set to stop a running training.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// The last trained model (one at a time, usable on any open image).
pub static MODEL: Mutex<Option<Arc<Learned>>> = Mutex::new(None);

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    N2v,
    Frames,
    Neighbours,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainConfig {
    pub steps: usize,
    pub batch: usize,
    pub patch: usize,
    pub lr: f64,
    pub seed: u64,
}

impl Default for TrainConfig {
    fn default() -> Self {
        TrainConfig { steps: 1000, batch: 8, patch: 64, lr: 1e-3, seed: 0x5eed }
    }
}

/// A trained denoiser.
pub struct Learned {
    pub net: Net,
    pub mode: Mode,
    pub mean: f32,
    pub std: f32,
    pub loss: f32,
    /// File it was trained on, to show.
    pub source: String,
    /// Pixels averaged in each direction before the network (1 = none):
    /// counting-mode movies have too few electrons per pixel to denoise at
    /// full resolution.
    pub bin: usize,
}

/// Averages `k` × `k` blocks (the remainder at the right and bottom is dropped).
pub fn bin(v: &[f32], w: usize, h: usize, k: usize) -> (Vec<f32>, usize, usize) {
    if k <= 1 {
        return (v.to_vec(), w, h);
    }
    let (bw, bh) = ((w / k).max(1), (h / k).max(1));
    let mut out = vec![0f32; bw * bh];
    for by in 0..bh {
        for bx in 0..bw {
            let (mut s, mut n) = (0f32, 0f32);
            for y in by * k..((by + 1) * k).min(h) {
                for x in bx * k..((bx + 1) * k).min(w) {
                    s += v[y * w + x];
                    n += 1.0;
                }
            }
            out[by * bw + bx] = s / n.max(1.0);
        }
    }
    (out, bw, bh)
}

/// Bilinear enlargement of a `bin`-binned image back to `w` × `h`.
fn unbin(v: &[f32], bw: usize, bh: usize, k: usize, w: usize, h: usize) -> Vec<f32> {
    let src = |i: usize, n: usize| {
        let f = ((i as f32 + 0.5) / k as f32 - 0.5).clamp(0.0, (n - 1) as f32);
        let a = (f as usize).min(n.saturating_sub(2));
        (a, (a + 1).min(n - 1), f - a as f32)
    };
    let xs: Vec<_> = (0..w).map(|x| src(x, bw)).collect();
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let (y0, y1, fy) = src(y, bh);
        for (x, &(x0, x1, fx)) in xs.iter().enumerate() {
            let top = v[y0 * bw + x0] * (1.0 - fx) + v[y0 * bw + x1] * fx;
            let bot = v[y1 * bw + x0] * (1.0 - fx) + v[y1 * bw + x1] * fx;
            out[y * w + x] = top * (1.0 - fy) + bot * fy;
        }
    }
    out
}

/// Pixels around a region that the network's result depends on (enough
/// for its receptive field in practice).
pub const MARGIN: usize = 32;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Mean and standard deviation of the finite values, from a sample.
fn stats(images: &[&[f32]]) -> (f32, f32) {
    let (mut n, mut s, mut s2) = (0f64, 0f64, 0f64);
    for v in images {
        let step = (v.len() / 200_000).max(1);
        for &x in v.iter().step_by(step).filter(|x| x.is_finite()) {
            n += 1.0;
            s += x as f64;
            s2 += (x as f64) * (x as f64);
        }
    }
    let mean = s / n.max(1.0);
    let std = (s2 / n.max(1.0) - mean * mean).max(0.0).sqrt();
    (mean as f32, if std > 0.0 { std as f32 } else { 1.0 })
}

fn standardise(v: &[f32], mean: f32, std: f32) -> Vec<f32> {
    v.iter().map(|&x| if x.is_finite() { (x - mean) / std } else { 0.0 }).collect()
}

/// Trains on `pairs` of w × h images: (input, target) for Noise2Noise,
/// (image, None) for Noise2Void. `progress(step, steps, loss)` is called
/// every few steps.
#[allow(clippy::too_many_arguments)]
pub fn train(
    dev: &Device,
    pairs: &[(Vec<f32>, Option<Vec<f32>>)],
    w: usize,
    h: usize,
    mode: Mode,
    cfg: &TrainConfig,
    progress: impl FnMut(usize, usize, f32),
) -> Result<(Net, f32, f32, f32)> {
    train_live(dev, pairs, w, h, mode, cfg, progress, &mut |_, _, _, _, _| {})
}

/// As `train`, also handing the network as it is so far to `snapshot`
/// (net, mean, std) every few seconds, for the viewer to show the result
/// improving while it trains.
#[allow(clippy::too_many_arguments)]
pub fn train_live(
    dev: &Device,
    pairs: &[(Vec<f32>, Option<Vec<f32>>)],
    w: usize,
    h: usize,
    mode: Mode,
    cfg: &TrainConfig,
    progress: impl FnMut(usize, usize, f32),
    snapshot: &mut dyn FnMut(Net, f32, f32, usize, f32),
) -> Result<(Net, f32, f32, f32)> {
    let images: Vec<Img> = pairs.iter().map(|(a, b)| Img { a, b: b.as_deref(), w, h }).collect();
    let f = fit(&[dev.clone()], &images, mode, cfg, &Fitting::default(), progress, snapshot)?;
    Ok((f.net, f.mean, f.std, f.loss))
}

/// One training image (and its Noise2Noise partner), w × h.
#[derive(Clone, Copy)]
pub struct Img<'a> {
    pub a: &'a [f32],
    pub b: Option<&'a [f32]>,
    pub w: usize,
    pub h: usize,
}

/// How `fit` chooses its training patches and when it stops.
#[derive(Clone, Default)]
pub struct Fitting {
    /// Patches only where the images show signal: the image is cut into
    /// cells, each scored by what the two noisy looks share (the covariance
    /// of input and target; the variance for Noise2Void), cells with
    /// outliers (gold, ice crystals: values far past the usual cell's) or
    /// no content (padding) dropped, and the best `signal_keep` of the
    /// rest kept. Off: anywhere.
    pub signal: bool,
    pub signal_keep: f32,
    /// Hold out every eighth cell, score the network on them every few
    /// steps (the same loss, nothing learned from them), and end with the
    /// network that scored best.
    pub validate: bool,
    /// With `validate`: stop when the held-out score has not improved for
    /// this many checks (0 = run all the steps), after at least a third of
    /// the steps.
    pub patience: usize,
    /// Start from this network instead of a new one (fine-tuning).
    pub start: Option<Net>,
    /// Standardise with this mean and spread instead of the data's own (a
    /// network carried over from other data keeps its scale).
    pub scale: Option<(f32, f32)>,
    /// Extra held-out data scored like the held-out cells (other images
    /// than the training ones), for comparing ways of training.
    pub holdout: Vec<(Vec<f32>, Option<Vec<f32>>, usize, usize)>,
}

/// What `fit` made.
pub struct Fit {
    pub net: Net,
    pub mean: f32,
    pub std: f32,
    /// Running training loss at the end.
    pub loss: f32,
    /// Best held-out loss (with `validate`), and the step it came at.
    pub val: Option<(f32, usize)>,
    /// Held-out loss on `Fitting::holdout`, of the network returned.
    pub holdout: Option<f32>,
    /// Steps run (fewer than asked when it stopped early).
    pub steps: usize,
    /// (step, held-out loss) of every check.
    pub history: Vec<(usize, f32)>,
    pub cards: usize,
}

/// A training cell: image, top-left corner.
type Cell = (usize, usize, usize);

/// Cells of side p over every image, with their signal score, without the
/// ones holding outliers or nothing.
fn scored_cells(data: &[(Vec<f32>, Option<Vec<f32>>, usize, usize)], p: usize) -> Vec<(Cell, f32)> {
    use rayon::prelude::*;
    let mut cells: Vec<(Cell, f32, f32, f32)> = data
        .par_iter()
        .enumerate()
        .flat_map_iter(|(i, (a, b, w, h))| {
            let (w, h) = (*w, *h);
            (0..h / p).flat_map(move |cy| (0..w / p).map(move |cx| (cx * p, cy * p))).map(move |(x0, y0)| {
                let (mut n, mut sa, mut sb, mut sab, mut saa, mut peak) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f32);
                for y in (y0..y0 + p).step_by(2) {
                    for x in (x0..x0 + p).step_by(2) {
                        let u = a[y * w + x];
                        let v = b.as_ref().map_or(u, |b| b[y * w + x]);
                        n += 1.0;
                        sa += u as f64;
                        sb += v as f64;
                        sab += (u * v) as f64;
                        saa += (u * u) as f64;
                        peak = peak.max((0.5 * (u + v)).abs());
                    }
                }
                let var = saa / n - (sa / n).powi(2);
                let score = (sab / n - sa / n * sb / n) as f32;
                ((i, x0, y0), score, peak, var as f32)
            })
        })
        .collect();
    if cells.is_empty() {
        return vec![];
    }
    let mut peaks: Vec<f32> = cells.iter().map(|c| c.2).collect();
    peaks.sort_by(f32::total_cmp);
    let usual = peaks[peaks.len() / 2].max(1e-6);
    cells.retain(|c| c.3 > 1e-6 && c.2 <= 3.0 * usual);
    cells.into_iter().map(|c| (c.0, c.1)).collect()
}

/// Trains a denoiser on `images` (all standardised alike) on the cards
/// `devs` (plain devices, the first the primary; the batch is split over
/// them each step).
pub fn fit(
    devs: &[Device],
    images: &[Img],
    mode: Mode,
    cfg: &TrainConfig,
    how: &Fitting,
    mut progress: impl FnMut(usize, usize, f32),
    snapshot: &mut dyn FnMut(Net, f32, f32, usize, f32),
) -> Result<Fit> {
    if images.is_empty() {
        bail!("没有训练数据");
    }
    let small = images.iter().map(|i| i.w.min(i.h)).min().unwrap();
    if small < ALIGN * 2 {
        bail!("图像太小");
    }
    if (mode == Mode::N2v) != images.iter().all(|p| p.b.is_none()) {
        bail!("训练数据和方式对不上");
    }
    let all: Vec<&[f32]> = images.iter().flat_map(|i| std::iter::once(i.a).chain(i.b)).collect();
    let (mean, std) = how.scale.unwrap_or_else(|| stats(&all));
    let data: Vec<(Vec<f32>, Option<Vec<f32>>, usize, usize)> = images.iter().map(|i| (standardise(i.a, mean, std), i.b.map(|b| standardise(b, mean, std)), i.w, i.h)).collect();
    let holdout: Vec<(Vec<f32>, Option<Vec<f32>>, usize, usize)> = how.holdout.iter().map(|(a, b, w, h)| (standardise(a, mean, std), b.as_ref().map(|b| standardise(b, mean, std)), *w, *h)).collect();
    let p = (cfg.patch.min(small) / ALIGN * ALIGN).max(ALIGN);
    let mut rng = Rng(cfg.seed | 1);
    // Where patches come from: cells ranked by signal (else all cells),
    // every eighth of them held out.
    let mut cells: Vec<(Cell, f32)> = if how.signal { scored_cells(&data, p) } else { data.iter().enumerate().flat_map(|(i, (_, _, w, h))| (0..h / p).flat_map(move |cy| (0..w / p).map(move |cx| ((i, cx * p, cy * p), 0.0)))).collect() };
    if how.signal {
        cells.sort_by(|a, b| b.1.total_cmp(&a.1));
        let keep = ((cells.len() as f32 * how.signal_keep.clamp(0.05, 1.0)).ceil() as usize).max(1);
        cells.truncate(keep);
    } else {
        // Every eighth of a shuffled list: spread over the images.
        for i in (1..cells.len()).rev() {
            cells.swap(i, rng.below(i + 1));
        }
    }
    let (mut train_cells, mut val_cells) = (vec![], vec![]);
    for (k, c) in cells.into_iter().enumerate() {
        if how.validate && k % 8 == 4 { val_cells.push(c.0) } else { train_cells.push(c.0) }
    }
    if train_cells.is_empty() {
        bail!("没有可训练的区域");
    }
    val_cells.truncate(96);
    let held: std::collections::HashSet<Cell> = val_cells.iter().copied().collect();
    let cards = devs.len().max(1);
    let ads: Vec<Device> = devs.iter().map(|d| d.clone().autodiff()).collect();
    let mut net = match &how.start {
        Some(n) => n.clone().fork(&ads[0]),
        None => Net::new(&ads[0]),
    };
    let mut opt = AdamConfig::new().init();
    let mut loss_avg = 0f32;
    const CELL: usize = 8;
    // One sample: input, target, mask of the scored pixels.
    let sample = |rng: &mut Rng, src_img: &[f32], dst_img: Option<&[f32]>, w: usize, x0: usize, y0: usize, aug: bool, xs: &mut Vec<f32>, ts: &mut Vec<f32>, ms: &mut Vec<f32>| {
        let (fx, fy, tr) = if aug { (rng.below(2) == 1, rng.below(2) == 1, rng.below(2) == 1) } else { (false, false, false) };
        let at = |img: &[f32], x: usize, y: usize| {
            let (mut sx, mut sy) = if tr { (y, x) } else { (x, y) };
            if fx {
                sx = p - 1 - sx;
            }
            if fy {
                sy = p - 1 - sy;
            }
            img[(y0 + sy) * w + x0 + sx]
        };
        let base = xs.len();
        for y in 0..p {
            for x in 0..p {
                xs.push(at(src_img, x, y));
                ts.push(at(dst_img.unwrap_or(src_img), x, y));
            }
        }
        match dst_img {
            Some(_) => ms.extend(std::iter::repeat_n(1f32, p * p)),
            None => {
                // Blind spots: one per cell, its value from a random neighbour.
                ms.extend(std::iter::repeat_n(0f32, p * p));
                for cy in (0..p).step_by(CELL) {
                    for cx in (0..p).step_by(CELL) {
                        let (x, y) = (cx + rng.below(CELL.min(p - cx)), cy + rng.below(CELL.min(p - cy)));
                        let (nx, ny) = loop {
                            let (dx, dy) = (rng.below(5) as isize - 2, rng.below(5) as isize - 2);
                            if dx != 0 || dy != 0 {
                                break ((x as isize + dx).clamp(0, p as isize - 1) as usize, (y as isize + dy).clamp(0, p as isize - 1) as usize);
                            }
                        };
                        xs[base + y * p + x] = ts[base + ny * p + nx];
                        ms[base + y * p + x] = 1.0;
                    }
                }
            }
        }
    };
    let loss_of = |net: &Net, (xs, ts, ms, b): (Vec<f32>, Vec<f32>, Vec<f32>, usize), dev: &Device| -> Tensor<1> {
        let x = Tensor::<4>::from_data(TensorData::new(xs, [b, 1, p, p]), dev);
        let t = Tensor::<4>::from_data(TensorData::new(ts, [b, 1, p, p]), dev);
        let m = Tensor::<4>::from_data(TensorData::new(ms, [b, 1, p, p]), dev);
        let e = net.forward(x) - t;
        (e.clone() * e * m.clone()).sum() / m.sum().clamp_min(1.0)
    };
    // Held-out score: the same loss on fixed samples (both directions of
    // each pair, no augmentation, blind spots from a fixed seed).
    let score = |net: &Net, set: &[(&[f32], Option<&[f32]>, usize, usize, usize)]| -> f32 {
        if set.is_empty() {
            return f32::NAN;
        }
        let mut r = Rng(0x7a11d);
        let (mut total, mut n) = (0f64, 0usize);
        for chunk in set.chunks(16) {
            let (mut xs, mut ts, mut ms) = (vec![], vec![], vec![]);
            for &(a, b, w, x0, y0) in chunk {
                sample(&mut r, a, b, w, x0, y0, false, &mut xs, &mut ts, &mut ms);
            }
            let l: f32 = loss_of(net, (xs, ts, ms, chunk.len()), &devs[0]).into_scalar();
            total += l as f64 * chunk.len() as f64;
            n += chunk.len();
        }
        (total / n as f64) as f32
    };
    let val_set: Vec<(&[f32], Option<&[f32]>, usize, usize, usize)> = val_cells
        .iter()
        .flat_map(|&(i, x0, y0)| {
            let (a, b, w, _) = &data[i];
            let one = (a.as_slice(), b.as_deref(), *w, x0, y0);
            let other = b.as_deref().map(|b| (b, Some(a.as_slice()), *w, x0, y0));
            std::iter::once(one).chain(other)
        })
        .collect();
    let hold_set: Vec<(&[f32], Option<&[f32]>, usize, usize, usize)> = holdout
        .iter()
        .flat_map(|(a, b, w, h)| {
            let (w, h) = (*w, *h);
            (0..h / p).flat_map(move |cy| (0..w / p).map(move |cx| (cx * p, cy * p))).flat_map(move |(x0, y0)| {
                let one = (a.as_slice(), b.as_deref(), w, x0, y0);
                let other = b.as_deref().map(|b| (b, Some(a.as_slice()), w, x0, y0));
                std::iter::once(one).chain(other)
            })
        })
        .collect();
    let every = (cfg.steps / 40).max(25);
    let mut best: Option<(f32, usize, Net)> = None;
    let mut since_best = 0;
    let mut history = vec![];
    let mut shot = std::time::Instant::now();
    let mut ran = cfg.steps;
    for step in 0..cfg.steps {
        if CANCEL.load(Ordering::SeqCst) {
            bail!("已取消");
        }
        let b = cfg.batch.max(cards);
        let mut parts = Vec::with_capacity(cards);
        for k in 0..cards {
            let nk = crate::multi_gpu::share(b, cards, k);
            let (mut xs, mut ts, mut ms) = (Vec::with_capacity(nk * p * p), Vec::with_capacity(nk * p * p), Vec::with_capacity(nk * p * p));
            for _ in 0..nk {
                let (i, cx, cy) = train_cells[rng.below(train_cells.len())];
                let (a, bimg, w, h) = &data[i];
                // A random shift within half a cell, unless it would reach
                // into a held-out cell.
                let jitter = |r: &mut Rng, c: usize, n: usize| (c as isize + r.below(p + 1) as isize - (p / 2) as isize).clamp(0, (n - p) as isize) as usize;
                let (mut x0, mut y0) = (jitter(&mut rng, cx, *w), jitter(&mut rng, cy, *h));
                if !held.is_empty() {
                    let touches = [(x0 / p, y0 / p), ((x0 + p - 1) / p, y0 / p), (x0 / p, (y0 + p - 1) / p), ((x0 + p - 1) / p, (y0 + p - 1) / p)]
                        .iter()
                        .any(|&(gx, gy)| held.contains(&(i, gx * p, gy * p)));
                    if touches {
                        (x0, y0) = (cx, cy);
                    }
                }
                // Noise2Noise pairs work both ways.
                let swap = rng.below(2) == 1;
                let (src, dst) = match bimg {
                    Some(bb) if swap => (bb.as_slice(), Some(a.as_slice())),
                    _ => (a.as_slice(), bimg.as_deref()),
                };
                sample(&mut rng, src, dst, *w, x0, y0, true, &mut xs, &mut ts, &mut ms);
            }
            parts.push(((xs, ts, ms, nk), nk));
        }
        let (grads, lv) = crate::multi_gpu::step_grads(&net, &ads, parts, &|n: &Net, part, k| loss_of(n, part, &ads[k]));
        loss_avg += if step == 0 { lv - loss_avg } else { 0.05 * (lv - loss_avg) };
        // Cosine decay of the learning rate.
        let lr = cfg.lr * 0.5 * (1.0 + (std::f64::consts::PI * step as f64 / cfg.steps as f64).cos());
        net = opt.step(lr, net, grads);
        if step % 10 == 9 || step + 1 == cfg.steps {
            progress(step + 1, cfg.steps, loss_avg);
        }
        if !val_set.is_empty() && (step % every == every - 1 || step + 1 == cfg.steps) {
            let v = net.valid();
            let s = score(&v, &val_set);
            history.push((step + 1, s));
            if best.as_ref().is_none_or(|b| s < b.0 * (1.0 - 1e-3)) {
                best = Some((s, step + 1, v));
                since_best = 0;
            } else {
                since_best += 1;
                if how.patience > 0 && since_best >= how.patience && step + 1 >= cfg.steps / 3 {
                    ran = step + 1;
                    break;
                }
            }
        }
        if step + 1 < cfg.steps && shot.elapsed().as_secs_f32() > 2.5 {
            snapshot(net.valid(), mean, std, step + 1, loss_avg);
            shot = std::time::Instant::now();
        }
    }
    let (val, net) = match best {
        Some((s, at, v)) => (Some((s, at)), v),
        None => (None, net.valid()),
    };
    let held_out = (!hold_set.is_empty()).then(|| score(&net, &hold_set));
    Ok(Fit { net, mean, std, loss: loss_avg, val, holdout: held_out, steps: ran, history, cards })
}

impl Learned {
    /// Denoises one w × h image in overlapping tiles (edges reflected to a
    /// multiple of ALIGN).
    pub fn apply(&self, dev: &Device, values: &[f32], w: usize, h: usize) -> Vec<f32> {
        if self.bin > 1 {
            let (b, bw, bh) = bin(values, w, h, self.bin);
            return unbin(&self.tiles(dev, &b, bw, bh), bw, bh, self.bin, w, h);
        }
        self.tiles(dev, values, w, h)
    }

    /// The training data's scale, unless `values` are on a clearly different
    /// one (a model trained on another image): then their own mean and
    /// standard deviation, so the network sees what it was trained on.
    fn scale_for(&self, values: &[f32]) -> (f32, f32) {
        let (mut n, mut s, mut s2) = (0f64, 0f64, 0f64);
        for &x in values.iter().filter(|x| x.is_finite()) {
            n += 1.0;
            s += x as f64;
            s2 += x as f64 * x as f64;
        }
        if n < 2.0 {
            return (self.mean, self.std);
        }
        let mean = (s / n) as f32;
        let std = ((s2 / n - (s / n).powi(2)).max(0.0).sqrt() as f32).max(f32::MIN_POSITIVE);
        let ratio = std / self.std;
        if (0.25..=4.0).contains(&ratio) && (mean - self.mean).abs() <= 4.0 * self.std {
            (self.mean, self.std)
        } else {
            (mean, std)
        }
    }

    fn tiles(&self, dev: &Device, values: &[f32], w: usize, h: usize) -> Vec<f32> {
        const TILE: usize = 512;
        let (mean, std) = self.scale_for(values);
        let v = standardise(values, mean, std);
        let net = self.net.clone().to_device(dev);
        let mut out = vec![0f32; w * h];
        let step = TILE - 2 * MARGIN;
        for y0 in (0..h).step_by(step) {
            for x0 in (0..w).step_by(step) {
                let (ax, ay) = (x0.saturating_sub(MARGIN), y0.saturating_sub(MARGIN));
                let (bx, by) = ((x0 + step + MARGIN).min(w), (y0 + step + MARGIN).min(h));
                let (tw, th) = (bx - ax, by - ay);
                let (pw, ph) = (tw.div_ceil(ALIGN).max(2) * ALIGN, th.div_ceil(ALIGN).max(2) * ALIGN);
                let reflect = |i: usize, n: usize| if n == 1 { 0 } else { let k = i % (2 * n - 2); if k < n { k } else { 2 * n - 2 - k } };
                let mut tile = vec![0f32; pw * ph];
                for y in 0..ph {
                    let sy = reflect(y, th);
                    for x in 0..pw {
                        tile[y * pw + x] = v[(ay + sy) * w + ax + reflect(x, tw)];
                    }
                }
                let x = Tensor::<4>::from_data(TensorData::new(tile, [1, 1, ph, pw]), dev);
                let r = net.forward(x).into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
                for y in y0..(y0 + step).min(h) {
                    for x in x0..(x0 + step).min(w) {
                        out[y * w + x] = r[(y - ay) * pw + (x - ax)] * std + mean;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::denoise::tests::{gauss, phantom, rmse};

    fn denoised(mode: Mode, steps: usize) -> (f32, f32) {
        let dev = Device::flex();
        let (w, h) = (64, 64);
        let (clean, noisy) = phantom(w, h, 0.4);
        let other: Vec<f32> = clean.iter().enumerate().map(|(i, &c)| c + 0.4 * gauss(99, i as u64)).collect();
        let pairs = match mode {
            Mode::N2v => vec![(noisy.clone(), None)],
            _ => vec![(noisy.clone(), Some(other.clone()))],
        };
        let cfg = TrainConfig { steps, batch: 4, patch: 32, lr: 2e-3, seed: 7 };
        let (net, mean, std, loss) = train(&dev, &pairs, w, h, mode, &cfg, |_, _, _| {}).unwrap();
        let l = Learned { net, mode, mean, std, loss, source: String::new(), bin: 1 };
        let out = match mode {
            Mode::N2v => l.apply(&dev, &noisy, w, h),
            _ => {
                let (a, b) = (l.apply(&dev, &noisy, w, h), l.apply(&dev, &other, w, h));
                a.iter().zip(&b).map(|(a, b)| 0.5 * (a + b)).collect()
            }
        };
        let before = match mode {
            Mode::N2v => rmse(&clean, &noisy),
            _ => rmse(&clean, &noisy.iter().zip(&other).map(|(a, b)| 0.5 * (a + b)).collect::<Vec<_>>()),
        };
        (before, rmse(&clean, &out))
    }

    #[test]
    fn noise2noise_learns_the_signal() {
        let (before, after) = denoised(Mode::Frames, 60);
        assert!(after < before * 0.7, "{after} vs {before}");
    }

    #[test]
    fn noise2void_learns_the_signal() {
        let (before, after) = denoised(Mode::N2v, 120);
        assert!(after < before * 0.8, "{after} vs {before}");
    }

    #[test]
    fn other_scales_get_their_own_standardisation() {
        let dev = Device::flex();
        let l = Learned { net: Net::new(&dev), mode: Mode::N2v, mean: 0.5, std: 0.1, loss: 0.0, source: String::new(), bin: 1 };
        let near: Vec<f32> = (0..100).map(|i| 0.5 + 0.1 * ((i % 7) as f32 - 3.0) / 2.0).collect();
        assert_eq!(l.scale_for(&near), (0.5, 0.1));
        let far: Vec<f32> = near.iter().map(|x| 1000.0 + 300.0 * x).collect();
        let (m, s) = l.scale_for(&far);
        assert!((m - 1150.0).abs() < 1.0 && s > 10.0, "{m} {s}");
    }

    #[test]
    fn binning_round_trips_smooth_images() {
        let (w, h) = (37, 29);
        let v: Vec<f32> = (0..w * h).map(|i| (i % w) as f32 * 0.5 + (i / w) as f32).collect();
        let (b, bw, bh) = bin(&v, w, h, 2);
        assert_eq!((bw, bh), (18, 14));
        assert!((b[0] - (0.0 + 0.5 + 1.0 + 1.5) / 4.0).abs() < 1e-6);
        let back = unbin(&b, bw, bh, 2, w, h);
        // A linear ramp survives inside the binned area.
        for y in 1..2 * bh - 1 {
            for x in 1..2 * bw - 1 {
                assert!((back[y * w + x] - v[y * w + x]).abs() < 1e-4, "{x},{y}");
            }
        }
    }

    #[test]
    fn apply_covers_odd_sizes_and_tiles() {
        let dev = Device::flex();
        let l = Learned { net: Net::new(&dev), mode: Mode::N2v, mean: 3.0, std: 2.0, loss: 0.0, source: String::new(), bin: 1 };
        let (w, h) = (601, 21);
        let out = l.apply(&dev, &vec![1.0; w * h], w, h);
        assert_eq!(out.len(), w * h);
        assert!(out.iter().all(|v| v.is_finite()));
        // A flat input gives a flat result away from the edges.
        let mid = out[10 * w + 300];
        assert!(out[10 * w + 100..10 * w + 500].iter().all(|v| (v - mid).abs() < 1e-3));
    }
}

/// Training time and result on a real stack:
/// TMV_DENOISE_FILE=<stack> cargo test --release --features pro --lib denoise_learn::bench -- --ignored --nocapture
/// (TMV_DENOISE_STEPS sets the steps, default 1000; TMV_DENOISE_BIN bins
/// first; TMV_DENOISE_MOVIE=1 treats the stack as frames of one exposure
/// and works on their mean). Reports the noise σ
/// estimate of the middle slice before and after, per mode.
#[cfg(test)]
mod bench {
    use super::*;
    use crate::denoise::noise_sigma;

    #[test]
    #[ignore]
    fn bench() {
        let Ok(path) = std::env::var("TMV_DENOISE_FILE") else { return };
        let steps: usize = std::env::var("TMV_DENOISE_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
        let src = crate::formats::open(std::path::Path::new(&path)).unwrap();
        let img = crate::image::OpenImage::new(1, src, 1 << 30);
        let info = img.info().clone();
        let (w, h, nz) = (info.width, info.height, info.size_z);
        let mid = nz / 2;
        let k: usize = std::env::var("TMV_DENOISE_BIN").ok().and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
        // A movie: everything works on the frame mean, and neighbours make no sense.
        let movie = std::env::var("TMV_DENOISE_MOVIE").is_ok();
        let slice = if movie {
            let [a, b] = crate::denoise::frame_halves(&img, 0, 0).unwrap();
            a.iter().zip(&b).map(|(a, b)| 0.5 * (a + b)).collect()
        } else {
            crate::denoise::input(&img, 0, mid, 0, 0).unwrap()
        };
        println!("{path}: {w}×{h}×{nz}, slice {mid} noise σ {:.4}", noise_sigma(&slice, w, h));
        let spread: Vec<usize> = (0..nz.min(16)).map(|i| i * nz / nz.min(16)).collect();
        for mode in [Mode::N2v, Mode::Neighbours, Mode::Frames] {
            if (mode != Mode::N2v && nz < 2) || (movie && mode == Mode::Neighbours) {
                continue;
            }
            let pairs: Vec<(Vec<f32>, Option<Vec<f32>>)> = match mode {
                Mode::N2v if movie => vec![(slice.clone(), None)],
                Mode::N2v => spread.iter().map(|&z| (crate::denoise::input(&img, 0, z, 0, 0).unwrap(), None)).collect(),
                Mode::Neighbours => spread.iter().filter(|&&z| z + 1 < nz).map(|&z| (crate::denoise::input(&img, 0, z, 0, 0).unwrap(), Some(crate::denoise::input(&img, 0, z + 1, 0, 0).unwrap()))).collect(),
                Mode::Frames => {
                    let [a, b] = crate::denoise::frame_halves(&img, 0, 0).unwrap();
                    vec![(a, Some(b))]
                }
            };
            let (bw, bh) = ((w / k).max(1), (h / k).max(1));
            let pairs: Vec<_> = pairs.into_iter().map(|(a, b)| (bin(&a, w, h, k).0, b.map(|b| bin(&b, w, h, k).0))).collect();
            let t0 = std::time::Instant::now();
            let cfg = TrainConfig { steps, ..Default::default() };
            let ((net, mean, std, loss), backend) = crate::classify::on_device(|dev| train(dev, &pairs, bw, bh, mode, &cfg, |_, _, _| {})).unwrap();
            let trained = t0.elapsed().as_secs_f64();
            let l = Learned { net, mode, mean, std, loss, source: String::new(), bin: k };
            let t1 = std::time::Instant::now();
            let (out, _) = crate::classify::on_device(|dev| Ok(l.apply(dev, &slice, w, h))).unwrap();
            println!("  {mode:?} on {backend}: train {trained:.1} s ({steps} steps, loss {loss:.4}), apply {:.0} ms, σ left {:.4}", t1.elapsed().as_secs_f64() * 1e3, noise_sigma(&out, w, h));
            if let Ok(dir) = std::env::var("TMV_DENOISE_OUT") {
                let mut bytes = crate::align::mrc_header(w, h, 1, 1.0, [0.0, 1.0, 0.0, 1.0], &format!("{mode:?}"));
                bytes.extend(out.iter().flat_map(|v| v.to_le_bytes()));
                std::fs::write(format!("{dir}/denoised_{mode:?}.mrc"), bytes).unwrap();
            }
        }
    }
}
