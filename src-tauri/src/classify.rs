//! Pixel classifier for the Pro segmentation tools, like ilastik's or
//! Weka's: the user scribbles a few strokes of each label, every pixel gets
//! a stack of filter responses (smoothed intensity, gradient, Laplacian and
//! Hessian eigenvalues at several scales), and a random forest trained on
//! the scribbled pixels labels the rest.
//!
//! Features are computed with Burn on the device SAM uses (the GPU through
//! wgpu or CUDA, else the CPU), as separable convolutions of a reflected
//! copy of the slice. The forest is trained on the CPU (a few thousand
//! samples) and evaluated on the same device, all trees at once, by
//! following node indices with gathers.

use anyhow::{bail, Result};
use burn::tensor::module::conv2d;
use burn::tensor::ops::ConvOptions;
use burn::tensor::{Device, Int, Tensor, TensorData};
use rayon::prelude::*;

/// Set to stop a running `classify_apply`.
pub static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Gaussian scales of the filters, in pixels.
pub const SIGMAS: [f32; 4] = [1.0, 2.0, 4.0, 8.0];
/// Filters per scale: smoothed, gradient magnitude, Laplacian, Hessian λ1, λ2.
const PER_SIGMA: usize = 5;
/// Number of features per pixel (the raw intensity first).
pub const FEATURES: usize = 1 + SIGMAS.len() * PER_SIGMA;
/// Feature names, for logs and tests.
pub fn feature_names() -> Vec<String> {
    let mut v = vec!["raw".to_string()];
    for s in SIGMAS {
        for f in ["gauss", "gradient", "laplacian", "hessian1", "hessian2"] {
            v.push(format!("{f}{s}"));
        }
    }
    v
}

fn radius(sigma: f32) -> usize {
    (3.0 * sigma).ceil() as usize
}

/// Gaussian, first and second derivative kernels of `sigma`, each 2r+1 taps.
/// Derivatives are scale-normalised (×σ, ×σ²) so scales compare.
fn kernels(sigma: f32) -> [Vec<f32>; 3] {
    let r = radius(sigma) as i32;
    let s2 = sigma * sigma;
    let g: Vec<f32> = (-r..=r)
        .map(|x| (-(x * x) as f32 / (2.0 * s2)).exp())
        .collect();
    let sum: f32 = g.iter().sum();
    let g: Vec<f32> = g.iter().map(|v| v / sum).collect();
    let d1 = (-r..=r)
        .zip(&g)
        .map(|(x, v)| -(x as f32) / s2 * v * sigma)
        .collect();
    let d2: Vec<f32> = (-r..=r)
        .zip(&g)
        .map(|(x, v)| ((x * x) as f32 / (s2 * s2) - 1.0 / s2) * v * s2)
        .collect();
    // The sampled second derivative should sum to 0 (no response to a flat image).
    let mean = d2.iter().sum::<f32>() / d2.len() as f32;
    let d2 = d2.iter().map(|v| v - mean).collect();
    [g, d1, d2]
}

/// `pixels` padded by `pad` on every side, reflecting at the border.
fn reflect(pixels: &[f32], w: usize, h: usize, pad: usize) -> Vec<f32> {
    let ref_idx = |i: isize, n: usize| -> usize {
        let n = n as isize;
        if n == 1 {
            return 0;
        }
        let period = 2 * (n - 1);
        let mut k = i.rem_euclid(period);
        if k >= n {
            k = period - k;
        }
        k as usize
    };
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let mut out = vec![0f32; pw * ph];
    out.par_chunks_mut(pw).enumerate().for_each(|(y, row)| {
        let sy = ref_idx(y as isize - pad as isize, h);
        for (x, o) in row.iter_mut().enumerate() {
            *o = pixels[sy * w + ref_idx(x as isize - pad as isize, w)];
        }
    });
    out
}

fn upload<const D: usize>(v: Vec<f32>, shape: [usize; D], dev: &Device) -> Tensor<D> {
    Tensor::from_data(TensorData::new(v, shape), dev)
}

/// Features of one slice, feature-major: `out[f * w * h + i]`. `pixels` are
/// already scaled to about 0..1.
pub fn features(dev: &Device, pixels: &[f32], w: usize, h: usize) -> Vec<f32> {
    features_tensor(dev, pixels, w, h)
        .into_data()
        .convert::<f32>()
        .try_into_vec::<f32>()
        .unwrap()
}

/// `features` left on the device, shape [FEATURES, w * h].
pub fn features_tensor(dev: &Device, pixels: &[f32], w: usize, h: usize) -> Tensor<2> {
    let pad = radius(*SIGMAS.last().unwrap());
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let img = upload(reflect(pixels, w, h, pad), [1, 1, ph, pw], dev);
    let n = w * h;
    let mut out = vec![img
        .clone()
        .slice([0..1, 0..1, pad..pad + h, pad..pad + w])
        .reshape([1, n])];
    for sigma in SIGMAS {
        let r = radius(sigma);
        let k = 2 * r + 1;
        let [g, d1, d2] = kernels(sigma);
        // Along x: channels g, d1, d2.
        let wx: Vec<f32> = [&g, &d1, &d2].into_iter().flatten().copied().collect();
        let rows = conv2d(
            img.clone(),
            upload(wx, [3, 1, 1, k], dev),
            None,
            ConvOptions::new([1, 1], [0, 0], [1, 1], 1),
        );
        // Along y, in groups: G = g·g, Gx = d1·g, Gy = g·d1, Gxx = d2·g, Gyy = g·d2, Gxy = d1·d1.
        let pick =
            Tensor::<1, Int>::from_ints(TensorData::new(vec![0i64, 1, 0, 2, 0, 1], [6]), dev);
        let six = rows.select(1, pick);
        let wy: Vec<f32> = [&g, &g, &d1, &g, &d2, &d1]
            .into_iter()
            .flatten()
            .copied()
            .collect();
        let all = conv2d(
            six,
            upload(wy, [6, 1, k, 1], dev),
            None,
            ConvOptions::new([1, 1], [0, 0], [1, 1], 6),
        );
        // `all` is (ph - 2r) × (pw - 2r); the slice starts at pad - r.
        let o = pad - r;
        let all = all.slice([0..1, 0..6, o..o + h, o..o + w]).reshape([6, n]);
        let ch = |i: usize| all.clone().slice([i..i + 1, 0..n]);
        let (gs, gx, gy, gxx, gyy, gxy) = (ch(0), ch(1), ch(2), ch(3), ch(4), ch(5));
        let grad = (gx.clone() * gx + gy.clone() * gy).sqrt();
        let trace = gxx.clone() + gyy.clone();
        let half = (gxx - gyy) * 0.5;
        let disc = (half.clone() * half + gxy.clone() * gxy).sqrt();
        let mid = trace.clone() * 0.5;
        out.extend([gs, grad, trace, mid.clone() + disc.clone(), mid - disc]);
    }
    Tensor::cat(out, 0)
}

/// The GPU SAM and the CTF fit use, else the CPU, with its name.
pub fn device() -> (Device, &'static str) {
    match crate::ctf_gpu::device() {
        Some((d, name)) => (d.clone(), name),
        None => (Device::flex(), "CPU"),
    }
}

/// Runs `f` on the GPU and, if a kernel fails there, again on the CPU.
pub fn on_device<T>(f: impl Fn(&Device) -> Result<T>) -> Result<(T, &'static str)> {
    let (dev, name) = device();
    if name != "CPU" {
        match crate::ctf_gpu::guarded(|| f(&dev)) {
            Ok(Ok(v)) => return Ok((v, name)),
            Ok(Err(e)) => return Err(e),
            Err(e) => eprintln!("classifier: {e}; using the CPU"),
        }
    }
    Ok((f(&Device::flex())?, "CPU"))
}

/// Tiny deterministic generator (xorshift64*), so training is repeatable.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// One node of a tree. A leaf has `feature == LEAF` and its class
/// probabilities at `left` in `Tree::leaves`.
#[derive(Clone, Copy, Debug)]
struct Node {
    feature: u16,
    threshold: f32,
    left: u32,
    right: u32,
}

const LEAF: u16 = u16::MAX;

#[derive(Clone, Debug)]
struct Tree {
    nodes: Vec<Node>,
    /// Class probabilities of the leaves, `classes` per leaf.
    leaves: Vec<f32>,
    depth: usize,
}

/// Training settings.
#[derive(Clone, Debug)]
pub struct ForestConfig {
    pub trees: usize,
    pub max_depth: usize,
    pub min_leaf: usize,
    /// Candidate thresholds per feature (quantiles of the training data).
    pub bins: usize,
    pub seed: u64,
}

impl Default for ForestConfig {
    fn default() -> Self {
        ForestConfig {
            trees: 64,
            max_depth: 14,
            min_leaf: 2,
            bins: 32,
            seed: 0x5eed,
        }
    }
}

/// A random forest over `features` inputs and `classes` outputs.
#[derive(Clone, Debug)]
pub struct Forest {
    trees: Vec<Tree>,
    pub classes: usize,
    pub features: usize,
    /// Fraction of the training samples the trees that did not see them got right.
    pub oob_accuracy: f32,
}

impl Forest {
    /// Trains on `x` (sample-major, `features` per sample) and classes `y`
    /// (0..classes).
    pub fn train(
        x: &[f32],
        y: &[u16],
        features: usize,
        classes: usize,
        cfg: &ForestConfig,
    ) -> Result<Forest> {
        let n = y.len();
        if n == 0 || x.len() != n * features {
            bail!("no training samples");
        }
        if classes < 2 {
            bail!("先用画笔在图上标几笔，至少两种标签");
        }
        // Candidate thresholds: quantiles of each feature.
        let cuts: Vec<Vec<f32>> = (0..features)
            .into_par_iter()
            .map(|f| {
                let mut v: Vec<f32> = (0..n)
                    .map(|i| x[i * features + f])
                    .filter(|v| v.is_finite())
                    .collect();
                v.sort_by(|a, b| a.total_cmp(b));
                let mut c: Vec<f32> = (1..cfg.bins)
                    .filter_map(|k| v.get(k * v.len() / cfg.bins).copied())
                    .collect();
                c.dedup();
                c
            })
            .collect();
        // Bin of every sample in every feature (index of the first cut above it).
        let binned: Vec<u8> = (0..n * features)
            .into_par_iter()
            .map(|i| cuts[i % features].partition_point(|&c| c < x[i]) as u8)
            .collect();
        let mtry = ((features as f32).sqrt().round() as usize).clamp(1, features);
        let built: Vec<(Tree, Vec<usize>)> = (0..cfg.trees)
            .into_par_iter()
            .map(|t| {
                let mut rng =
                    Rng(cfg.seed ^ (t as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
                let mut seen = vec![false; n];
                let mut idx: Vec<usize> = (0..n).map(|_| rng.below(n)).collect();
                for &i in &idx {
                    seen[i] = true;
                }
                let mut b = Builder {
                    binned: &binned,
                    cuts: &cuts,
                    y,
                    features,
                    classes,
                    cfg,
                    mtry,
                    rng,
                    nodes: vec![],
                    leaves: vec![],
                    depth: 0,
                };
                b.grow(&mut idx, 0);
                let oob = (0..n).filter(|&i| !seen[i]).collect();
                (
                    Tree {
                        nodes: b.nodes,
                        leaves: b.leaves,
                        depth: b.depth,
                    },
                    oob,
                )
            })
            .collect();
        let trees: Vec<Tree> = built.iter().map(|(t, _)| t.clone()).collect();
        // Out-of-bag accuracy.
        let mut votes = vec![0f32; n * classes];
        for (tree, oob) in &built {
            for &i in oob {
                let p = tree.leaf(&x[i * features..(i + 1) * features], classes);
                for c in 0..classes {
                    votes[i * classes + c] += p[c];
                }
            }
        }
        let (mut right, mut counted) = (0usize, 0usize);
        for i in 0..n {
            let v = &votes[i * classes..(i + 1) * classes];
            if v.iter().any(|&p| p > 0.0) {
                counted += 1;
                right += (argmax(v) == y[i] as usize) as usize;
            }
        }
        let oob_accuracy = if counted > 0 {
            right as f32 / counted as f32
        } else {
            0.0
        };
        Ok(Forest {
            trees,
            classes,
            features,
            oob_accuracy,
        })
    }

    /// Class probabilities of one sample.
    pub fn predict_one(&self, sample: &[f32]) -> Vec<f32> {
        let mut p = vec![0f32; self.classes];
        for t in &self.trees {
            for (a, b) in p.iter_mut().zip(t.leaf(sample, self.classes)) {
                *a += b;
            }
        }
        p.iter_mut().for_each(|v| *v /= self.trees.len() as f32);
        p
    }

    /// Classes and confidences (the winning probability) of `n` samples
    /// stored feature-major (`feats[f * n + i]`), on the CPU.
    pub fn predict_cpu(&self, feats: &[f32], n: usize) -> (Vec<u16>, Vec<f32>) {
        (0..n)
            .into_par_iter()
            .map(|i| {
                let s: Vec<f32> = (0..self.features).map(|f| feats[f * n + i]).collect();
                let p = self.predict_one(&s);
                let c = argmax(&p);
                (c as u16, p[c])
            })
            .unzip()
    }

    /// `predict_cpu` on the device, for features from `features_tensor`.
    pub fn predict_device(&self, dev: &Device, feats: Tensor<2>) -> (Vec<u16>, Vec<f32>) {
        let n = feats.dims()[1];
        let t = self.trees.len();
        // All trees in one table; a leaf points at itself so extra steps stay put.
        let (mut feat, mut thr, mut left, mut right, mut probs) =
            (vec![], vec![], vec![], vec![], vec![]);
        for tree in &self.trees {
            let base = feat.len() as i64;
            for (k, node) in tree.nodes.iter().enumerate() {
                let me = base + k as i64;
                if node.feature == LEAF {
                    feat.push(0i64);
                    thr.push(f32::INFINITY);
                    left.push(me);
                    right.push(me);
                    probs.extend_from_slice(
                        &tree.leaves[node.left as usize * self.classes
                            ..(node.left as usize + 1) * self.classes],
                    );
                } else {
                    feat.push(node.feature as i64);
                    thr.push(node.threshold);
                    left.push(base + node.left as i64);
                    right.push(base + node.right as i64);
                    probs.extend(std::iter::repeat_n(0.0, self.classes));
                }
            }
        }
        let m = feat.len();
        let ints = |v: Vec<i64>| Tensor::<1, Int>::from_ints(TensorData::new(v, [m]), dev);
        let (feat, left, right) = (ints(feat), ints(left), ints(right));
        let thr = upload(thr, [m], dev);
        let probs = upload(probs, [m, self.classes], dev);
        let roots: Vec<i64> = {
            let mut r = vec![];
            let mut base = 0i64;
            for tree in &self.trees {
                r.push(base);
                base += tree.nodes.len() as i64;
            }
            r
        };
        let depth = self.trees.iter().map(|t| t.depth).max().unwrap_or(0);
        // Pixels in chunks to bound memory (trees × chunk indices).
        let chunk = (1usize << 22) / t.max(1);
        let flat = feats.reshape([self.features * n]);
        let (mut classes, mut conf) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let mut start = 0;
        while start < n {
            let len = chunk.min(n - start);
            let tn = t * len;
            // Node index of every (tree, pixel), and the pixel's column.
            let mut start_idx = Vec::with_capacity(tn);
            let mut col = Vec::with_capacity(tn);
            for &r in &roots {
                start_idx.extend(std::iter::repeat_n(r, len));
                col.extend((start..start + len).map(|i| i as i64));
            }
            let mut idx = Tensor::<1, Int>::from_ints(TensorData::new(start_idx, [tn]), dev);
            let col = Tensor::<1, Int>::from_ints(TensorData::new(col, [tn]), dev);
            for _ in 0..depth {
                let f = feat.clone().gather(0, idx.clone());
                let v = flat.clone().gather(0, f * n as i64 + col.clone());
                let go_left = v.lower_equal(thr.clone().gather(0, idx.clone()));
                let l = left.clone().gather(0, idx.clone());
                let r = right.clone().gather(0, idx);
                idx = r.mask_where(go_left, l);
            }
            let p = probs
                .clone()
                .select(0, idx)
                .reshape([t, len, self.classes])
                .mean_dim(0)
                .reshape([len, self.classes]);
            let best = p.clone().max_dim(1).reshape([len]);
            let arg = p.argmax(1).reshape([len]);
            classes.extend(
                arg.into_data()
                    .convert::<i64>()
                    .try_into_vec::<i64>()
                    .unwrap()
                    .into_iter()
                    .map(|c| c as u16),
            );
            conf.extend(
                best.into_data()
                    .convert::<f32>()
                    .try_into_vec::<f32>()
                    .unwrap(),
            );
            start += len;
        }
        (classes, conf)
    }
}

fn argmax(v: &[f32]) -> usize {
    (0..v.len()).fold(0, |b, i| if v[i] > v[b] { i } else { b })
}

impl Tree {
    fn leaf(&self, s: &[f32], classes: usize) -> &[f32] {
        let mut k = 0usize;
        loop {
            let n = self.nodes[k];
            if n.feature == LEAF {
                let l = n.left as usize;
                return &self.leaves[l * classes..(l + 1) * classes];
            }
            k = if s[n.feature as usize] <= n.threshold {
                n.left
            } else {
                n.right
            } as usize;
        }
    }
}

struct Builder<'a> {
    binned: &'a [u8],
    cuts: &'a [Vec<f32>],
    y: &'a [u16],
    features: usize,
    classes: usize,
    cfg: &'a ForestConfig,
    mtry: usize,
    rng: Rng,
    nodes: Vec<Node>,
    leaves: Vec<f32>,
    depth: usize,
}

impl Builder<'_> {
    fn leaf(&mut self, idx: &[usize]) -> u32 {
        let mut p = vec![0f32; self.classes];
        for &i in idx {
            p[self.y[i] as usize] += 1.0;
        }
        let total = idx.len().max(1) as f32;
        let at = (self.leaves.len() / self.classes) as u32;
        self.leaves.extend(p.iter().map(|v| v / total));
        self.nodes.push(Node {
            feature: LEAF,
            threshold: 0.0,
            left: at,
            right: at,
        });
        (self.nodes.len() - 1) as u32
    }

    /// Grows the subtree for the samples `idx` and returns its node index.
    fn grow(&mut self, idx: &mut [usize], depth: usize) -> u32 {
        self.depth = self.depth.max(depth);
        let pure = idx.iter().all(|&i| self.y[i] == self.y[idx[0]]);
        if pure || depth >= self.cfg.max_depth || idx.len() < 2 * self.cfg.min_leaf {
            return self.leaf(idx);
        }
        let Some((f, bin)) = self.best_split(idx) else {
            return self.leaf(idx);
        };
        // Partition in place: bin <= `bin` goes left.
        let mut l = 0;
        for k in 0..idx.len() {
            if self.binned[idx[k] * self.features + f] as usize <= bin {
                idx.swap(l, k);
                l += 1;
            }
        }
        let me = self.nodes.len();
        self.nodes.push(Node {
            feature: f as u16,
            threshold: self.cuts[f][bin],
            left: 0,
            right: 0,
        });
        let (a, b) = idx.split_at_mut(l);
        let left = self.grow(a, depth + 1);
        let right = self.grow(b, depth + 1);
        self.nodes[me].left = left;
        self.nodes[me].right = right;
        me as u32
    }

    /// The (feature, bin) split of `mtry` random features with the lowest
    /// Gini impurity, if any split leaves `min_leaf` samples on both sides.
    fn best_split(&mut self, idx: &[usize]) -> Option<(usize, usize)> {
        let c = self.classes;
        let gini = |counts: &[f32], total: f32| -> f32 {
            if total <= 0.0 {
                return 0.0;
            }
            total - counts.iter().map(|v| v * v).sum::<f32>() / total
        };
        let mut best: Option<(f32, usize, usize)> = None;
        let mut tried = vec![false; self.features];
        for _ in 0..self.mtry {
            let mut f = self.rng.below(self.features);
            while tried[f] {
                f = (f + 1) % self.features;
            }
            tried[f] = true;
            let nb = self.cuts[f].len();
            if nb == 0 {
                continue;
            }
            // Class counts per bin (bins 0..=nb).
            let mut hist = vec![0f32; (nb + 1) * c];
            for &i in idx {
                hist[self.binned[i * self.features + f] as usize * c + self.y[i] as usize] += 1.0;
            }
            let mut total = vec![0f32; c];
            for b in 0..=nb {
                for k in 0..c {
                    total[k] += hist[b * c + k];
                }
            }
            let n = idx.len() as f32;
            let mut left = vec![0f32; c];
            let mut nl = 0f32;
            for b in 0..nb {
                for k in 0..c {
                    left[k] += hist[b * c + k];
                    nl += hist[b * c + k];
                }
                let nr = n - nl;
                if nl < self.cfg.min_leaf as f32 || nr < self.cfg.min_leaf as f32 {
                    continue;
                }
                let right: Vec<f32> = (0..c).map(|k| total[k] - left[k]).collect();
                let score = gini(&left, nl) + gini(&right, nr);
                if best.is_none_or(|(s, _, _)| score < s) {
                    best = Some((score, f, b));
                }
            }
        }
        best.map(|(_, f, b)| (f, b))
    }
}

/// A trained classifier for one image: the forest and the label each class
/// stands for, with the contrast the features were computed at.
pub struct Classifier {
    pub forest: Forest,
    pub labels: Vec<u16>,
    pub lo: f32,
    pub hi: f32,
    pub samples: usize,
}

/// Scales raw values to about 0..1 with the view's contrast.
pub fn scale(values: &[f32], lo: f32, hi: f32) -> Vec<f32> {
    let k = if hi > lo { 1.0 / (hi - lo) } else { 1.0 };
    values
        .iter()
        .map(|&v| if v.is_finite() { (v - lo) * k } else { 0.0 })
        .collect()
}

/// Most samples taken per class, so big scribbles don't slow training.
pub const MAX_PER_CLASS: usize = 20_000;

/// Training data from scribbled slices: for every slice, its pixel values and
/// its scribble labels (0 = not scribbled). Returns the classifier.
pub fn train(
    dev: &Device,
    slices: &[(Vec<f32>, &[u16])],
    w: usize,
    h: usize,
    lo: f32,
    hi: f32,
    cfg: &ForestConfig,
) -> Result<Classifier> {
    // Which labels were scribbled, and where.
    let mut labels: Vec<u16> = slices
        .iter()
        .flat_map(|(_, s)| s.iter().copied())
        .filter(|&v| v != 0)
        .collect();
    labels.sort_unstable();
    labels.dedup();
    if labels.len() < 2 {
        bail!("先用画笔在图上标几笔，至少两种标签");
    }
    let class_of = |v: u16| labels.binary_search(&v).ok();
    // Sample positions per class across slices (evenly thinned past MAX_PER_CLASS).
    let mut per_class: Vec<Vec<(usize, usize)>> = vec![vec![]; labels.len()];
    for (si, (_, s)) in slices.iter().enumerate() {
        for (i, &v) in s.iter().enumerate() {
            if let Some(c) = (v != 0).then(|| class_of(v)).flatten() {
                per_class[c].push((si, i));
            }
        }
    }
    let mut rng = Rng(cfg.seed | 1);
    for v in per_class.iter_mut() {
        while v.len() > MAX_PER_CLASS {
            let k = rng.below(v.len());
            v.swap_remove(k);
        }
    }
    let n: usize = per_class.iter().map(Vec::len).sum();
    let mut x = vec![0f32; n * FEATURES];
    let mut y = vec![0u16; n];
    let mut row = 0;
    for (si, (values, _)) in slices.iter().enumerate() {
        let wanted: Vec<(usize, usize)> = per_class
            .iter()
            .enumerate()
            .flat_map(|(c, v)| v.iter().filter(|p| p.0 == si).map(move |p| (c, p.1)))
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let f = features(dev, &scale(values, lo, hi), w, h);
        let np = w * h;
        for (c, i) in wanted {
            for k in 0..FEATURES {
                x[row * FEATURES + k] = f[k * np + i];
            }
            y[row] = c as u16;
            row += 1;
        }
    }
    let forest = Forest::train(&x, &y, FEATURES, labels.len(), cfg)?;
    Ok(Classifier {
        forest,
        labels,
        lo,
        hi,
        samples: n,
    })
}

impl Classifier {
    /// Label and confidence of every pixel of one slice.
    pub fn predict(
        &self,
        dev: &Device,
        values: &[f32],
        w: usize,
        h: usize,
    ) -> (Vec<u16>, Vec<f32>) {
        let f = features_tensor(dev, &scale(values, self.lo, self.hi), w, h);
        let (c, p) = self.forest.predict_device(dev, f);
        (c.into_iter().map(|c| self.labels[c as usize]).collect(), p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev() -> Device {
        Device::flex()
    }

    /// Plain CPU reference of the features, for checking the device path.
    fn reference(px: &[f32], w: usize, h: usize) -> Vec<f32> {
        let n = w * h;
        let mut out = px.to_vec();
        for sigma in SIGMAS {
            let r = radius(sigma);
            let p = reflect(px, w, h, r);
            let pw = w + 2 * r;
            let [g, d1, d2] = kernels(sigma);
            let conv = |kx: &[f32], ky: &[f32]| -> Vec<f32> {
                let mut o = vec![0f32; n];
                for y in 0..h {
                    for x in 0..w {
                        let mut s = 0f32;
                        for (j, wy) in ky.iter().enumerate() {
                            for (i, wx) in kx.iter().enumerate() {
                                s += wy * wx * p[(y + j) * pw + x + i];
                            }
                        }
                        o[y * w + x] = s;
                    }
                }
                o
            };
            let (gs, gx, gy, gxx, gyy, gxy) = (
                conv(&g, &g),
                conv(&d1, &g),
                conv(&g, &d1),
                conv(&d2, &g),
                conv(&g, &d2),
                conv(&d1, &d1),
            );
            out.extend(gs.iter());
            out.extend((0..n).map(|i| (gx[i] * gx[i] + gy[i] * gy[i]).sqrt()));
            out.extend((0..n).map(|i| gxx[i] + gyy[i]));
            for sign in [1.0f32, -1.0] {
                out.extend((0..n).map(|i| {
                    let half = (gxx[i] - gyy[i]) * 0.5;
                    (gxx[i] + gyy[i]) * 0.5 + sign * (half * half + gxy[i] * gxy[i]).sqrt()
                }));
            }
        }
        out
    }

    fn test_image(w: usize, h: usize) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let ring = (((x - 30.0).powi(2) + (y - 20.0).powi(2)).sqrt() - 12.0).abs() < 2.0;
                let noise = ((i as u64).wrapping_mul(2654435761) % 1000) as f32 / 1000.0 - 0.5;
                (if ring { 0.2 } else { 0.7 }) + 0.1 * noise + 0.002 * x
            })
            .collect()
    }

    #[test]
    fn device_features_match_reference() {
        let (w, h) = (53, 41);
        let px = test_image(w, h);
        let got = features(&dev(), &px, w, h);
        let want = reference(&px, w, h);
        assert_eq!(got.len(), FEATURES * w * h);
        assert_eq!(want.len(), got.len());
        let names = feature_names();
        for f in 0..FEATURES {
            let worst = (0..w * h)
                .map(|i| (got[f * w * h + i] - want[f * w * h + i]).abs())
                .fold(0f32, f32::max);
            assert!(worst < 1e-4, "{}: off by {worst}", names[f]);
        }
    }

    #[test]
    fn flat_image_has_no_edges() {
        let (w, h) = (20, 17);
        let f = features(&dev(), &vec![0.5; w * h], w, h);
        for s in 0..SIGMAS.len() {
            let base = 1 + s * PER_SIGMA;
            for i in 0..w * h {
                assert!((f[base * w * h + i] - 0.5).abs() < 1e-4);
                for k in 1..PER_SIGMA {
                    assert!(
                        f[(base + k) * w * h + i].abs() < 1e-4,
                        "feature {} not flat",
                        base + k
                    );
                }
            }
        }
    }

    #[test]
    fn forest_learns_xor_and_device_agrees() {
        // Two features, class = xor of their signs, plus noise features.
        let mut rng = Rng(7);
        let n = 2000;
        let f = 5;
        let mut x = vec![0f32; n * f];
        let mut y = vec![0u16; n];
        for i in 0..n {
            for k in 0..f {
                x[i * f + k] = (rng.below(2001) as f32 - 1000.0) / 1000.0;
            }
            y[i] = ((x[i * f] > 0.0) ^ (x[i * f + 1] > 0.0)) as u16;
        }
        let forest = Forest::train(
            &x,
            &y,
            f,
            2,
            &ForestConfig {
                trees: 24,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(forest.oob_accuracy > 0.9, "oob {}", forest.oob_accuracy);
        // Feature-major test grid.
        let m = 400;
        let mut feats = vec![0f32; f * m];
        let mut want = vec![0u16; m];
        for i in 0..m {
            let (a, b) = ((i % 20) as f32 / 10.0 - 0.95, (i / 20) as f32 / 10.0 - 0.95);
            feats[i] = a;
            feats[m + i] = b;
            want[i] = ((a > 0.0) ^ (b > 0.0)) as u16;
        }
        let (cpu, pc) = forest.predict_cpu(&feats, m);
        let right = cpu.iter().zip(&want).filter(|(a, b)| a == b).count();
        assert!(right as f32 / m as f32 > 0.9, "{right}/{m}");
        let d = dev();
        let (gpu, pg) = forest.predict_device(&d, upload(feats, [f, m], &d));
        assert_eq!(cpu, gpu);
        for (a, b) in pc.iter().zip(&pg) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn classifier_segments_a_ring_from_scribbles() {
        let (w, h) = (64, 48);
        let px = test_image(w, h);
        // Scribbles: an arc on the ring (label 3) and a few background strokes (label 1).
        let mut s = vec![0u16; w * h];
        for a in 0..30 {
            let t = a as f32 * 0.05;
            let (x, y) = (
                (30.0 + 12.0 * t.cos()).round() as usize,
                (20.0 + 12.0 * t.sin()).round() as usize,
            );
            s[y * w + x] = 3;
        }
        for x in 4..60 {
            s[3 * w + x] = 1;
            s[44 * w + x] = 1;
        }
        for y in 15..26 {
            s[y * w + 30] = 1;
            s[y * w + 52] = 1;
        }
        let d = dev();
        let c = train(
            &d,
            &[(px.clone(), &s)],
            w,
            h,
            0.0,
            1.0,
            &ForestConfig::default(),
        )
        .unwrap();
        assert_eq!(c.labels, vec![1, 3]);
        let (out, _) = c.predict(&d, &px, w, h);
        // Pixels on the ring's edge (within a pixel of it) may go either way.
        let (mut right, mut total) = (0, 0);
        for i in 0..w * h {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let d = (((x - 30.0).powi(2) + (y - 20.0).powi(2)).sqrt() - 12.0).abs();
            if (d - 2.0).abs() < 1.0 {
                continue;
            }
            total += 1;
            right += (out[i] == if d < 2.0 { 3 } else { 1 }) as usize;
        }
        assert!(right as f32 / total as f32 > 0.97, "{right}/{total}");
    }
}
