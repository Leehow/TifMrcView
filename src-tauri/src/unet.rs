//! A small 2D U-Net trained in the app (Pro) on the image's own labels:
//! the slices that hold labels are the training set (unlabelled pixels
//! are ignored), and the trained network then labels the other slices.
//! Training and inference run with Burn on the device SAM uses (the GPU
//! through CUDA or wgpu, else the CPU), with Burn's autodiff and Adam.

use anyhow::{bail, Result};
use burn::module::Module;
use burn::nn::conv::{Conv2d, Conv2dConfig};
use burn::nn::{GroupNorm, GroupNormConfig, PaddingConfig2d};
use burn::optim::{AdamConfig, GradientsParams};
use burn::tensor::activation::{log_softmax, relu, softmax};
use burn::tensor::module::{interpolate, max_pool2d};
use burn::tensor::ops::{InterpolateMode, InterpolateOptions, MaxPoolOptions};
use burn::tensor::{Device, Int, Tensor, TensorData};

/// Set to stop a running training or inference.
pub static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Two 3×3 convolutions, each followed by group norm and ReLU.
#[derive(Module, Debug)]
struct Block {
    c1: Conv2d,
    n1: GroupNorm,
    c2: Conv2d,
    n2: GroupNorm,
}

impl Block {
    fn new(i: usize, o: usize, dev: &Device) -> Block {
        let conv = |i, o| {
            Conv2dConfig::new([i, o], [3, 3])
                .with_padding(PaddingConfig2d::Same)
                .init(dev)
        };
        let norm = |c| GroupNormConfig::new(4, c).init(dev);
        Block {
            c1: conv(i, o),
            n1: norm(o),
            c2: conv(o, o),
            n2: norm(o),
        }
    }

    fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let x = relu(self.n1.forward(self.c1.forward(x)));
        relu(self.n2.forward(self.c2.forward(x)))
    }
}

/// U-Net with `LEVELS` poolings; widths double at every level from `BASE`.
#[derive(Module, Debug)]
pub struct UNet {
    down: Vec<Block>,
    bottom: Block,
    up: Vec<Block>,
    head: Conv2d,
}

const LEVELS: usize = 3;
const BASE: usize = 16;
/// Side of a slice or tile must be a multiple of this (2^LEVELS).
pub const ALIGN: usize = 1 << LEVELS;

impl UNet {
    pub fn new(classes: usize, dev: &Device) -> UNet {
        let w = |l: usize| BASE << l;
        let down = (0..LEVELS)
            .map(|l| Block::new(if l == 0 { 1 } else { w(l - 1) }, w(l), dev))
            .collect();
        let bottom = Block::new(w(LEVELS - 1), w(LEVELS), dev);
        let up = (0..LEVELS)
            .rev()
            .map(|l| Block::new(w(l + 1) + w(l), w(l), dev))
            .collect();
        let head = Conv2dConfig::new([BASE, classes], [1, 1]).init(dev);
        UNet {
            down,
            bottom,
            up,
            head,
        }
    }

    /// Logits [B, classes, H, W] of images [B, 1, H, W] (H, W multiples of ALIGN).
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
            x = interpolate(
                x,
                InterpolateOptions::new(InterpolateMode::Nearest).with_output_size([h, w]),
            );
            x = b.forward(Tensor::cat(vec![x, skip], 1));
        }
        self.head.forward(x)
    }
}

/// Training settings.
#[derive(Clone, Debug)]
pub struct TrainConfig {
    pub steps: usize,
    pub batch: usize,
    pub patch: usize,
    pub lr: f64,
    pub seed: u64,
}

impl Default for TrainConfig {
    fn default() -> Self {
        TrainConfig {
            steps: 400,
            batch: 8,
            patch: 128,
            lr: 2e-3,
            seed: 0x5eed,
        }
    }
}

/// A trained network and the label each output class stands for.
pub struct Trained {
    pub net: UNet,
    pub labels: Vec<u16>,
    pub lo: f32,
    pub hi: f32,
    pub loss: f32,
    pub accuracy: f32,
}

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

fn scale(v: &[f32], lo: f32, hi: f32) -> Vec<f32> {
    let k = if hi > lo { 1.0 / (hi - lo) } else { 1.0 };
    v.iter()
        .map(|&x| if x.is_finite() { (x - lo) * k } else { 0.0 })
        .collect()
}

/// Masked cross-entropy over labelled pixels, and the share of them right.
fn loss_of(logits: Tensor<4>, target: Tensor<4, Int>, mask: Tensor<4>) -> (Tensor<1>, Tensor<1>) {
    let logp = log_softmax(logits.clone(), 1);
    let picked = logp.gather(1, target.clone());
    let n = mask.clone().sum().clamp_min(1.0);
    let loss = -(picked * mask.clone()).sum() / n.clone();
    let right = logits.argmax(1).equal(target).float() * mask;
    (loss, right.sum() / n)
}

/// Trains on `slices` (raw values and labels, 0 = unlabelled) of size w × h.
/// `progress(step, steps, loss)` is called every few steps.
#[allow(clippy::too_many_arguments)]
pub fn train(
    dev: &Device,
    slices: &[(Vec<f32>, Vec<u16>)],
    w: usize,
    h: usize,
    lo: f32,
    hi: f32,
    cfg: &TrainConfig,
    mut progress: impl FnMut(usize, usize, f32),
) -> Result<Trained> {
    let mut labels: Vec<u16> = slices
        .iter()
        .flat_map(|(_, l)| l.iter().copied())
        .filter(|&v| v != 0)
        .collect();
    labels.sort_unstable();
    labels.dedup();
    if labels.len() < 2 {
        bail!("先在几层上画出至少两种标签（比如物体和背景），再训练");
    }
    let classes = labels.len();
    // Patch side: a multiple of ALIGN that fits the slice.
    let p = (cfg.patch.min(w).min(h) / ALIGN * ALIGN).max(ALIGN);
    if w < ALIGN || h < ALIGN {
        bail!("图像太小");
    }
    let data: Vec<(Vec<f32>, Vec<i64>, Vec<f32>)> = slices
        .iter()
        .map(|(v, l)| {
            let cls = l
                .iter()
                .map(|&k| labels.binary_search(&k).map_or(0, |c| c as i64))
                .collect();
            let mask = l.iter().map(|&k| (k != 0) as u8 as f32).collect();
            (scale(v, lo, hi), cls, mask)
        })
        .collect();
    // Patch corners that hold some labelled pixels, per slice.
    let mut rng = Rng(cfg.seed | 1);
    let ad = dev.clone().autodiff();
    let mut net = UNet::new(classes, &ad);
    let mut opt = AdamConfig::new().init();
    let (mut loss_avg, mut acc_avg) = (0f32, 0f32);
    for step in 0..cfg.steps {
        if CANCEL.load(std::sync::atomic::Ordering::SeqCst) {
            bail!("已取消");
        }
        let (mut xs, mut ts, mut ms) = (vec![], vec![], vec![]);
        for _ in 0..cfg.batch {
            // Retry a few corners to land on labelled pixels.
            let mut pick = None;
            for _ in 0..20 {
                let s = rng.below(data.len());
                let (x0, y0) = (rng.below(w - p + 1), rng.below(h - p + 1));
                let m = &data[s].2;
                let any = (0..p).any(|y| {
                    m[(y0 + y) * w + x0..(y0 + y) * w + x0 + p]
                        .iter()
                        .any(|&v| v > 0.0)
                });
                pick = Some((s, x0, y0));
                if any {
                    break;
                }
            }
            let (s, x0, y0) = pick.unwrap();
            // Random flips and transpose.
            let (fx, fy, tr) = (rng.below(2) == 1, rng.below(2) == 1, rng.below(2) == 1);
            for y in 0..p {
                for x in 0..p {
                    let (mut sx, mut sy) = if tr { (y, x) } else { (x, y) };
                    if fx {
                        sx = p - 1 - sx;
                    }
                    if fy {
                        sy = p - 1 - sy;
                    }
                    let i = (y0 + sy) * w + x0 + sx;
                    xs.push(data[s].0[i]);
                    ts.push(data[s].1[i]);
                    ms.push(data[s].2[i]);
                }
            }
        }
        let b = cfg.batch;
        let x = Tensor::<4>::from_data(TensorData::new(xs, [b, 1, p, p]), &ad);
        let t = Tensor::<4, Int>::from_ints(TensorData::new(ts, [b, 1, p, p]), &ad);
        let m = Tensor::<4>::from_data(TensorData::new(ms, [b, 1, p, p]), &ad);
        let (loss, acc) = loss_of(net.forward(x), t, m);
        let lv: f32 = loss.clone().into_scalar();
        let av: f32 = acc.into_scalar();
        let k = if step == 0 { 1.0 } else { 0.1 };
        loss_avg += k * (lv - loss_avg);
        acc_avg += k * (av - acc_avg);
        let grads = GradientsParams::from_grads(loss.backward(), &net);
        // Cosine decay of the learning rate.
        let lr =
            cfg.lr * 0.5 * (1.0 + (std::f64::consts::PI * step as f64 / cfg.steps as f64).cos());
        net = opt.step(lr, net, grads);
        if step % 5 == 4 || step + 1 == cfg.steps {
            progress(step + 1, cfg.steps, loss_avg);
        }
    }
    Ok(Trained {
        net: net.valid(),
        labels,
        lo,
        hi,
        loss: loss_avg,
        accuracy: acc_avg,
    })
}

impl Trained {
    /// Label and confidence of every pixel of one slice. The slice is
    /// padded (reflected) to a multiple of ALIGN and processed in tiles.
    pub fn predict(
        &self,
        dev: &Device,
        values: &[f32],
        w: usize,
        h: usize,
    ) -> (Vec<u16>, Vec<f32>) {
        let v = scale(values, self.lo, self.hi);
        const TILE: usize = 512;
        const OVERLAP: usize = 32;
        let net = self.net.clone().to_device(dev);
        let (mut out, mut conf) = (vec![0u16; w * h], vec![0f32; w * h]);
        let step = TILE - 2 * OVERLAP;
        let mut y0 = 0;
        while y0 < h {
            let mut x0 = 0;
            while x0 < w {
                // Tile with a margin, clamped to the image and padded to ALIGN.
                let (ax, ay) = (x0.saturating_sub(OVERLAP), y0.saturating_sub(OVERLAP));
                let (bx, by) = ((x0 + step + OVERLAP).min(w), (y0 + step + OVERLAP).min(h));
                let (tw, th) = (bx - ax, by - ay);
                let (pw, ph) = (tw.div_ceil(ALIGN) * ALIGN, th.div_ceil(ALIGN) * ALIGN);
                let mut tile = vec![0f32; pw * ph];
                for y in 0..ph {
                    let sy = if y < th {
                        y
                    } else {
                        2 * th - 2 - y.min(2 * th - 2)
                    }
                    .min(th - 1);
                    for x in 0..pw {
                        let sx = if x < tw {
                            x
                        } else {
                            2 * tw - 2 - x.min(2 * tw - 2)
                        }
                        .min(tw - 1);
                        tile[y * pw + x] = v[(ay + sy) * w + ax + sx];
                    }
                }
                let x = Tensor::<4>::from_data(TensorData::new(tile, [1, 1, ph, pw]), dev);
                let p = softmax(net.forward(x), 1);
                let best = p
                    .clone()
                    .max_dim(1)
                    .reshape([ph * pw])
                    .into_data()
                    .convert::<f32>()
                    .try_into_vec::<f32>()
                    .unwrap();
                let arg = p
                    .argmax(1)
                    .reshape([ph * pw])
                    .into_data()
                    .convert::<i64>()
                    .try_into_vec::<i64>()
                    .unwrap();
                // Keep the tile's core (x0.., y0..).
                for y in y0..(y0 + step).min(h) {
                    for x in x0..(x0 + step).min(w) {
                        let i = (y - ay) * pw + (x - ax);
                        out[y * w + x] = self.labels[arg[i] as usize];
                        conf[y * w + x] = best[i];
                    }
                }
                x0 += step;
            }
            y0 += step;
        }
        (out, conf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Discs on a noisy background, the discs darker: labels 2 inside, 1 outside.
    fn scene(w: usize, h: usize, seed: u64) -> (Vec<f32>, Vec<u16>) {
        let mut rng = Rng(seed);
        let centers: Vec<(f32, f32, f32)> = (0..6)
            .map(|_| {
                (
                    rng.below(w) as f32,
                    rng.below(h) as f32,
                    5.0 + rng.below(6) as f32,
                )
            })
            .collect();
        let mut v = vec![0f32; w * h];
        let mut l = vec![1u16; w * h];
        for i in 0..w * h {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let inside = centers
                .iter()
                .any(|&(cx, cy, r)| (x - cx).powi(2) + (y - cy).powi(2) < r * r);
            let noise = (rng.below(1000) as f32 / 1000.0 - 0.5) * 0.3;
            v[i] = if inside { 0.3 } else { 0.7 } + noise;
            if inside {
                l[i] = 2;
            }
        }
        (v, l)
    }

    #[test]
    fn unet_learns_discs() {
        let dev = Device::flex();
        let (w, h) = (48, 40);
        let train_set: Vec<_> = (0..3).map(|s| scene(w, h, 11 + s)).collect();
        let cfg = TrainConfig {
            steps: 120,
            batch: 4,
            patch: 32,
            lr: 3e-3,
            seed: 3,
        };
        let t = train(&dev, &train_set, w, h, 0.0, 1.0, &cfg, |_, _, _| {}).unwrap();
        assert_eq!(t.labels, vec![1, 2]);
        let (v, want) = scene(w, h, 99);
        let (got, _) = t.predict(&dev, &v, w, h);
        let right = got.iter().zip(&want).filter(|(a, b)| a == b).count();
        assert!(
            right as f32 / (w * h) as f32 > 0.95,
            "{right}/{} (loss {})",
            w * h,
            t.loss
        );
    }

    #[test]
    fn prediction_covers_odd_sizes_and_tiles() {
        let dev = Device::flex();
        let net = UNet::new(3, &dev);
        let t = Trained {
            net,
            labels: vec![1, 4, 9],
            lo: 0.0,
            hi: 1.0,
            loss: 0.0,
            accuracy: 0.0,
        };
        let (w, h) = (601, 37);
        let (out, conf) = t.predict(&dev, &vec![0.5; w * h], w, h);
        assert_eq!(out.len(), w * h);
        assert!(out.iter().all(|k| [1, 4, 9].contains(k)));
        assert!(conf.iter().all(|&p| p > 0.3 && p <= 1.0));
    }
}
