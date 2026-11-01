//! The membrane network: a residual 3D U-Net with instance norm and deep
//! supervision, the kind of network nnU-Net and MemBrain-seg use, written
//! here from their papers' descriptions.

use burn::module::Module;
use burn::nn::conv::{Conv3d, Conv3dConfig, ConvTranspose3d, ConvTranspose3dConfig};
use burn::nn::{InstanceNorm, InstanceNormConfig, PaddingConfig3d};
use burn::tensor::activation::leaky_relu;
use burn::tensor::{Device, Tensor};
use serde::{Deserialize, Serialize};

const SLOPE: f64 = 0.01;

/// Widths and residual blocks per level, finest first.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetConfig {
    pub widths: Vec<usize>,
    pub blocks: Vec<usize>,
    /// Decoder levels (finest first) with an output head; level 0 always.
    pub heads: usize,
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig { widths: vec![16, 32, 64, 128, 256, 320], blocks: vec![1, 2, 2, 3, 3, 3], heads: 3 }
    }
}

fn conv(i: usize, o: usize, k: usize, stride: usize, dev: &Device) -> Conv3d {
    let p = k / 2;
    Conv3dConfig::new([i, o], [k; 3]).with_stride([stride; 3]).with_padding(PaddingConfig3d::Explicit(p, p, p)).init(dev)
}

fn norm(c: usize, dev: &Device) -> InstanceNorm {
    InstanceNormConfig::new(c).init(dev)
}

/// conv–norm–act–conv–norm plus the input (projected when the shape
/// changes), then act.
#[derive(Module, Debug)]
struct Res {
    c1: Conv3d,
    n1: InstanceNorm,
    c2: Conv3d,
    n2: InstanceNorm,
    skip: Option<Conv3d>,
    skip_n: Option<InstanceNorm>,
}

impl Res {
    fn new(i: usize, o: usize, stride: usize, dev: &Device) -> Res {
        let project = i != o || stride != 1;
        Res {
            c1: conv(i, o, 3, stride, dev),
            n1: norm(o, dev),
            c2: conv(o, o, 3, 1, dev),
            n2: norm(o, dev),
            skip: project.then(|| conv(i, o, 1, stride, dev)),
            skip_n: project.then(|| norm(o, dev)),
        }
    }

    fn forward(&self, x: Tensor<5>) -> Tensor<5> {
        let h = leaky_relu(self.n1.forward(self.c1.forward(x.clone())), SLOPE);
        let h = self.n2.forward(self.c2.forward(h));
        let s = match (&self.skip, &self.skip_n) {
            (Some(c), Some(n)) => n.forward(c.forward(x)),
            _ => x,
        };
        leaky_relu(h + s, SLOPE)
    }
}

#[derive(Module, Debug)]
struct Stage {
    blocks: Vec<Res>,
}

impl Stage {
    fn new(i: usize, o: usize, n: usize, stride: usize, dev: &Device) -> Stage {
        Stage { blocks: (0..n.max(1)).map(|k| if k == 0 { Res::new(i, o, stride, dev) } else { Res::new(o, o, 1, dev) }).collect() }
    }

    fn forward(&self, mut x: Tensor<5>) -> Tensor<5> {
        for b in &self.blocks {
            x = b.forward(x);
        }
        x
    }
}

#[derive(Module, Debug)]
pub struct MemNet {
    stem: Conv3d,
    stem_n: InstanceNorm,
    enc: Vec<Stage>,
    up: Vec<ConvTranspose3d>,
    dec: Vec<Stage>,
    heads: Vec<Conv3d>,
}

impl MemNet {
    pub fn new(cfg: &NetConfig, dev: &Device) -> MemNet {
        let w = &cfg.widths;
        let levels = w.len();
        let stem = conv(1, w[0], 3, 1, dev);
        let enc = (0..levels).map(|l| Stage::new(if l == 0 { w[0] } else { w[l - 1] }, w[l], cfg.blocks[l], if l == 0 { 1 } else { 2 }, dev)).collect();
        // Decoder level l (0 = finest) joins the upsampled level l+1 with
        // the encoder's level l.
        let up = (0..levels - 1).map(|l| ConvTranspose3dConfig::new([w[l + 1], w[l]], [2; 3]).with_stride([2; 3]).init(dev)).collect();
        let dec = (0..levels - 1).map(|l| Stage::new(2 * w[l], w[l], 1, 1, dev)).collect();
        let heads = (0..cfg.heads.clamp(1, levels - 1)).map(|l| conv(w[l], 1, 1, 1, dev)).collect();
        MemNet { stem, stem_n: norm(w[0], dev), enc, up, dec, heads }
    }

    /// Input sides must be multiples of this.
    pub fn align(&self) -> usize {
        1 << (self.enc.len() - 1)
    }

    /// Membrane logits for [B, 1, D, H, W]: full resolution first, then
    /// half, quarter… (as many as there are heads).
    pub fn forward_all(&self, x: Tensor<5>) -> Vec<Tensor<5>> {
        let mut x = leaky_relu(self.stem_n.forward(self.stem.forward(x)), SLOPE);
        let mut skips = Vec::with_capacity(self.enc.len());
        for s in &self.enc {
            x = s.forward(x);
            skips.push(x.clone());
        }
        let mut x = skips.pop().unwrap();
        let mut outs = vec![];
        for l in (0..self.dec.len()).rev() {
            let up = self.up[l].forward(x);
            x = self.dec[l].forward(Tensor::cat(vec![up, skips.pop().unwrap()], 1));
            if l < self.heads.len() {
                outs.push(self.heads[l].forward(x.clone()));
            }
        }
        outs.reverse();
        outs
    }

    /// Full-resolution membrane logits only.
    pub fn forward(&self, x: Tensor<5>) -> Tensor<5> {
        let mut x = leaky_relu(self.stem_n.forward(self.stem.forward(x)), SLOPE);
        let mut skips = Vec::with_capacity(self.enc.len());
        for s in &self.enc {
            x = s.forward(x);
            skips.push(x.clone());
        }
        let mut x = skips.pop().unwrap();
        for l in (0..self.dec.len()).rev() {
            let up = self.up[l].forward(x);
            x = self.dec[l].forward(Tensor::cat(vec![up, skips.pop().unwrap()], 1));
        }
        self.heads[0].forward(x)
    }
}

/// Masked Dice + binary cross-entropy of `logits` against `labels`
/// (0 background, 1 membrane, 2 ignore), as one scalar.
pub fn loss(logits: Tensor<5>, labels: Tensor<5>) -> Tensor<1> {
    let keep = labels.clone().lower_elem(1.5).float();
    let t = labels.equal_elem(1.0).float();
    let n = keep.clone().sum().clamp_min(1.0);
    // Stable BCE with logits: max(x,0) − x·t + ln(1 + e^−|x|).
    let bce = logits.clone().clamp_min(0.0) - logits.clone() * t.clone() + (logits.clone().abs().neg().exp() + 1.0).log();
    let bce = (bce * keep.clone()).sum() / n;
    let p = burn::tensor::activation::sigmoid(logits) * keep.clone();
    let tk = t * keep;
    let inter = (p.clone() * tk.clone()).sum();
    let dice = (inter * 2.0 + 1.0) / (p.sum() + tk.sum() + 1.0);
    bce + dice.neg() + 1.0
}

/// Max over each voxel and its two neighbours along `axis` (edges repeated).
fn shift_max(x: Tensor<5>, axis: usize) -> Tensor<5> {
    let n = x.dims()[axis];
    if n < 2 {
        return x;
    }
    let left = Tensor::cat(vec![x.clone().narrow(axis, 0, 1), x.clone().narrow(axis, 0, n - 1)], axis);
    let right = Tensor::cat(vec![x.clone().narrow(axis, 1, n - 1), x.clone().narrow(axis, n - 1, 1)], axis);
    x.max_pair(left).max_pair(right)
}

/// Soft erosion: the smallest of the three 3-voxel minima along the axes.
fn erode(x: Tensor<5>) -> Tensor<5> {
    let m = |a| shift_max(x.clone().neg(), a).neg();
    m(2).min_pair(m(3)).min_pair(m(4))
}

/// Soft dilation: 3×3×3 maximum.
fn dilate(x: Tensor<5>) -> Tensor<5> {
    shift_max(shift_max(shift_max(x, 2), 3), 4)
}

/// Soft skeleton (Shit et al., clDice, 2021) of probabilities in [0, 1].
fn soft_skel(x: Tensor<5>, iters: usize) -> Tensor<5> {
    let open = |x: Tensor<5>| dilate(erode(x));
    let mut x = x;
    let mut skel = (x.clone() - open(x.clone())).clamp_min(0.0);
    for _ in 0..iters {
        x = erode(x);
        let delta = (x.clone() - open(x.clone())).clamp_min(0.0);
        skel = skel.clone() + (delta.clone() - skel * delta).clamp_min(0.0);
    }
    skel
}

/// Masked skeleton ("surface") Dice loss: how much of each skeleton lies
/// in the other mask; keeps membranes thin and unbroken, as MemBrain-seg's
/// surface-Dice term does.
pub fn skeleton_loss(logits: Tensor<5>, labels: Tensor<5>) -> Tensor<1> {
    let keep = labels.clone().lower_elem(1.5).float();
    let t = labels.equal_elem(1.0).float() * keep.clone();
    let p = burn::tensor::activation::sigmoid(logits) * keep.clone();
    let sp = soft_skel(p.clone(), 5) * keep.clone();
    let st = soft_skel(t.clone(), 5) * keep;
    let prec = ((sp.clone() * t).sum() + 1.0) / (sp.sum() + 1.0);
    let sens = ((st.clone() * p).sum() + 1.0) / (st.sum() + 1.0);
    (prec.clone() * sens.clone() * 2.0 / (prec + sens)).neg() + 1.0
}
