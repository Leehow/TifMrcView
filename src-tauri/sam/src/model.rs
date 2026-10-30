//! SAM 2.1 image predictor in Burn: Hiera image encoder with FPN neck,
//! prompt encoder and two-way transformer mask decoder. Field names follow
//! the PyTorch module tree of facebookresearch/sam2 so its checkpoints load
//! as they are (see `Predictor::load`).
//!
//! Ported from facebookresearch/sam2, Copyright (c) Meta Platforms, Inc. and
//! affiliates, Apache License 2.0.

use burn::module::{Module, Param};
use burn::nn::conv::{Conv2d, Conv2dConfig, ConvTranspose2d, ConvTranspose2dConfig};
use burn::nn::{Embedding, EmbeddingConfig, LayerNorm, LayerNormConfig, Linear, LinearConfig, PaddingConfig2d};
use burn::tensor::activation::{gelu, relu, sigmoid};
use burn::tensor::module::{attention, interpolate, max_pool2d};
use burn::tensor::ops::{AttentionModuleOptions, InterpolateMode, InterpolateOptions, MaxPoolOptions};
use burn::tensor::{Device, Tensor};

/// Sizes of one SAM 2.1 variant.
#[derive(Clone, Debug)]
pub struct Sam2Config {
    pub embed_dim: usize,
    pub num_heads: usize,
    pub stages: Vec<usize>,
    pub global_att_blocks: Vec<usize>,
    pub window_spec: Vec<usize>,
    pub bkg_size: [usize; 2],
}

impl Sam2Config {
    pub fn tiny() -> Self {
        Sam2Config { embed_dim: 96, num_heads: 1, stages: vec![1, 2, 7, 2], global_att_blocks: vec![5, 7, 9], window_spec: vec![8, 4, 14, 7], bkg_size: [7, 7] }
    }
    pub fn small() -> Self {
        Sam2Config { stages: vec![1, 2, 11, 2], global_att_blocks: vec![7, 10, 13], ..Self::tiny() }
    }
    pub fn base_plus() -> Self {
        Sam2Config { embed_dim: 112, num_heads: 2, stages: vec![2, 3, 16, 3], global_att_blocks: vec![12, 16, 20], window_spec: vec![8, 4, 14, 7], bkg_size: [14, 14] }
    }
    pub fn large() -> Self {
        Sam2Config { embed_dim: 144, num_heads: 2, stages: vec![2, 6, 36, 4], global_att_blocks: vec![23, 33, 43], window_spec: vec![8, 4, 16, 8], bkg_size: [7, 7] }
    }
}

pub const IMAGE_SIZE: usize = 1024;
pub(crate) const D_MODEL: usize = 256;

pub(crate) fn linear(i: usize, o: usize, dev: &Device) -> Linear {
    LinearConfig::new(i, o).init(dev)
}

pub(crate) fn conv(i: usize, o: usize, k: usize, s: usize, p: usize, dev: &Device) -> Conv2d {
    Conv2dConfig::new([i, o], [k, k]).with_stride([s, s]).with_padding(PaddingConfig2d::Explicit(p, p, p, p)).init(dev)
}

fn sdpa(q: Tensor<4>, k: Tensor<4>, v: Tensor<4>) -> Tensor<4> {
    attention(q, k, v, None, None, AttentionModuleOptions::default())
}

/// MLP with ReLU or GELU between layers (sam2_utils.MLP).
#[derive(Module, Debug)]
pub struct Mlp {
    layers: Vec<Linear>,
    #[module(skip)]
    gelu: bool,
    #[module(skip)]
    sigmoid: bool,
}

impl Mlp {
    pub(crate) fn new(i: usize, h: usize, o: usize, n: usize, gelu: bool, sigmoid: bool, dev: &Device) -> Self {
        let dims: Vec<usize> = std::iter::once(i).chain(std::iter::repeat_n(h, n - 1)).chain(std::iter::once(o)).collect();
        Mlp { layers: dims.windows(2).map(|d| linear(d[0], d[1], dev)).collect(), gelu, sigmoid }
    }

    pub(crate) fn forward<const D: usize>(&self, mut x: Tensor<D>) -> Tensor<D> {
        let n = self.layers.len();
        for (i, l) in self.layers.iter().enumerate() {
            x = l.forward(x);
            if i + 1 < n {
                x = if self.gelu { gelu(x) } else { relu(x) };
            }
        }
        if self.sigmoid { sigmoid(x) } else { x }
    }
}

/// Layer norm over the channel axis of NCHW maps.
#[derive(Module, Debug)]
pub struct LayerNorm2d {
    weight: Param<Tensor<1>>,
    bias: Param<Tensor<1>>,
}

impl LayerNorm2d {
    pub(crate) fn new(c: usize, dev: &Device) -> Self {
        LayerNorm2d { weight: Param::from_tensor(Tensor::ones([c], dev)), bias: Param::from_tensor(Tensor::zeros([c], dev)) }
    }

    pub(crate) fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let c = x.dims()[1];
        let u = x.clone().mean_dim(1);
        let x = x - u;
        let s = x.clone().powf_scalar(2.0).mean_dim(1);
        let x = x / (s + 1e-6).sqrt();
        x * self.weight.val().reshape([1, c, 1, 1]) + self.bias.val().reshape([1, c, 1, 1])
    }
}

// ---------- Hiera ----------

#[derive(Module, Debug)]
pub struct PatchEmbed {
    proj: Conv2d,
}

#[derive(Module, Debug)]
pub struct MultiScaleAttention {
    qkv: Linear,
    proj: Linear,
    #[module(skip)]
    heads: usize,
    #[module(skip)]
    q_pool: bool,
}

/// 2x2 max pooling of a BHWC map.
fn pool_bhwc(x: Tensor<4>) -> Tensor<4> {
    let x = x.permute([0, 3, 1, 2]);
    let x = max_pool2d(x, MaxPoolOptions::new([2, 2]).with_stride([2, 2]));
    x.permute([0, 2, 3, 1])
}

impl MultiScaleAttention {
    fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let [b, h, w, _] = x.dims();
        let qkv = self.qkv.forward(x);
        let c_out = qkv.dims()[3] / 3;
        let hd = c_out / self.heads;
        let qkv = qkv.reshape([b, h * w, 3, self.heads, hd]);
        let part = |i: usize| qkv.clone().narrow(2, i, 1).reshape([b, h * w, self.heads, hd]);
        let (mut q, k, v) = (part(0), part(1), part(2));
        let (mut h, mut w) = (h, w);
        if self.q_pool {
            let p = pool_bhwc(q.reshape([b, h, w, c_out]));
            [_, h, w, _] = p.dims();
            q = p.reshape([b, h * w, self.heads, hd]);
        }
        let x = sdpa(q.swap_dims(1, 2), k.swap_dims(1, 2), v.swap_dims(1, 2));
        let x = x.swap_dims(1, 2).reshape([b, h, w, c_out]);
        self.proj.forward(x)
    }
}

fn window_partition(x: Tensor<4>, ws: usize) -> (Tensor<4>, [usize; 2]) {
    let [b, h, w, c] = x.dims();
    let (ph, pw) = ((ws - h % ws) % ws, (ws - w % ws) % ws);
    let x = if ph > 0 || pw > 0 { x.pad([(0, 0), (0, ph), (0, pw), (0, 0)], burn::tensor::ops::PadMode::Constant(0.0)) } else { x };
    let (hp, wp) = (h + ph, w + pw);
    let x = x.reshape([b, hp / ws, ws, wp / ws, ws, c]).permute([0, 1, 3, 2, 4, 5]).reshape([b * (hp / ws) * (wp / ws), ws, ws, c]);
    (x, [hp, wp])
}

fn window_unpartition(x: Tensor<4>, ws: usize, [hp, wp]: [usize; 2], [h, w]: [usize; 2]) -> Tensor<4> {
    let c = x.dims()[3];
    let b = x.dims()[0] / (hp * wp / ws / ws);
    let x = x.reshape([b, hp / ws, wp / ws, ws, ws, c]).permute([0, 1, 3, 2, 4, 5]).reshape([b, hp, wp, c]);
    if hp > h || wp > w { x.slice([0..b, 0..h, 0..w, 0..c]) } else { x }
}

#[derive(Module, Debug)]
pub struct MultiScaleBlock {
    norm1: LayerNorm,
    attn: MultiScaleAttention,
    norm2: LayerNorm,
    mlp: Mlp,
    proj: Option<Linear>,
    #[module(skip)]
    window_size: usize,
    #[module(skip)]
    q_stride: bool,
}

impl MultiScaleBlock {
    fn new(dim: usize, dim_out: usize, heads: usize, q_stride: bool, window_size: usize, dev: &Device) -> Self {
        MultiScaleBlock {
            norm1: LayerNormConfig::new(dim).with_epsilon(1e-6).init(dev),
            attn: MultiScaleAttention { qkv: linear(dim, dim_out * 3, dev), proj: linear(dim_out, dim_out, dev), heads, q_pool: q_stride },
            norm2: LayerNormConfig::new(dim_out).with_epsilon(1e-6).init(dev),
            mlp: Mlp::new(dim_out, dim_out * 4, dim_out, 2, true, false, dev),
            proj: (dim != dim_out).then(|| linear(dim, dim_out, dev)),
            window_size,
            q_stride,
        }
    }

    fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let mut shortcut = x.clone();
        let x = self.norm1.forward(x);
        if let Some(p) = &self.proj {
            shortcut = p.forward(x.clone());
            if self.q_stride {
                shortcut = pool_bhwc(shortcut);
            }
        }
        let [_, mut h, mut w, _] = x.dims();
        let mut ws = self.window_size;
        let mut pad_hw = [h, w];
        let mut x = x;
        if ws > 0 {
            (x, pad_hw) = window_partition(x, ws);
        }
        x = self.attn.forward(x);
        if self.q_stride {
            ws = self.window_size / 2;
            [_, h, w, _] = shortcut.dims();
            if ws > 0 {
                pad_hw = [h + (ws - h % ws) % ws, w + (ws - w % ws) % ws];
            }
        }
        if self.window_size > 0 {
            x = window_unpartition(x, ws, pad_hw, [h, w]);
        }
        let x = shortcut + x;
        x.clone() + self.mlp.forward(self.norm2.forward(x))
    }
}

#[derive(Module, Debug)]
pub struct Hiera {
    patch_embed: PatchEmbed,
    pub pos_embed: Param<Tensor<4>>,
    pub pos_embed_window: Param<Tensor<4>>,
    blocks: Vec<MultiScaleBlock>,
    #[module(skip)]
    stage_ends: Vec<usize>,
}

impl Hiera {
    fn new(cfg: &Sam2Config, dev: &Device) -> Self {
        let mut stage_ends = vec![];
        let mut acc = 0;
        for s in &cfg.stages {
            acc += s;
            stage_ends.push(acc - 1);
        }
        let q_pool_blocks: Vec<usize> = stage_ends[..stage_ends.len() - 1].iter().map(|x| x + 1).collect();
        let (mut dim, mut heads, mut cur_stage) = (cfg.embed_dim, cfg.num_heads, 1);
        let mut blocks = vec![];
        for i in 0..acc {
            let mut dim_out = dim;
            let mut ws = cfg.window_spec[cur_stage - 1];
            if cfg.global_att_blocks.contains(&i) {
                ws = 0;
            }
            if i > 0 && stage_ends.contains(&(i - 1)) {
                dim_out = dim * 2;
                heads *= 2;
                cur_stage += 1;
            }
            blocks.push(MultiScaleBlock::new(dim, dim_out, heads, q_pool_blocks.contains(&i), ws, dev));
            dim = dim_out;
        }
        let e = cfg.embed_dim;
        let w0 = cfg.window_spec[0];
        Hiera {
            patch_embed: PatchEmbed { proj: conv(3, e, 7, 4, 3, dev) },
            pos_embed: Param::from_tensor(Tensor::zeros([1, e, cfg.bkg_size[0], cfg.bkg_size[1]], dev)),
            pos_embed_window: Param::from_tensor(Tensor::zeros([1, e, w0, w0], dev)),
            blocks,
            stage_ends,
        }
    }

    /// Features at strides 4, 8, 16, 32 (NCHW). `pos` is the BHWC position
    /// embedding for the patch grid (see `hiera_pos_embed`).
    fn forward(&self, x: Tensor<4>, pos: Tensor<4>) -> Vec<Tensor<4>> {
        let mut x = self.patch_embed.proj.forward(x).permute([0, 2, 3, 1]) + pos;
        let mut out = vec![];
        for (i, b) in self.blocks.iter().enumerate() {
            x = b.forward(x);
            if self.stage_ends.contains(&i) {
                out.push(x.clone().permute([0, 3, 1, 2]));
            }
        }
        out
    }
}

/// PyTorch bicubic resize (align_corners=False, A=-0.75) of a CxHxW array.
fn bicubic(src: &[f32], c: usize, (ih, iw): (usize, usize), (oh, ow): (usize, usize)) -> Vec<f32> {
    let cubic = |x: f32| {
        let a = -0.75f32;
        let x = x.abs();
        if x <= 1.0 {
            ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
        } else if x < 2.0 {
            ((a * x - 5.0 * a) * x + 8.0 * a) * x - 4.0 * a
        } else {
            0.0
        }
    };
    let taps = |o: usize, n_in: usize, n_out: usize| {
        let scale = n_in as f32 / n_out as f32;
        let x = (o as f32 + 0.5) * scale - 0.5;
        let f = x.floor();
        let t = x - f;
        let ws = [cubic(t + 1.0), cubic(t), cubic(1.0 - t), cubic(2.0 - t)];
        let idx: [usize; 4] = std::array::from_fn(|k| (f as isize - 1 + k as isize).clamp(0, n_in as isize - 1) as usize);
        (idx, ws)
    };
    let ys: Vec<_> = (0..oh).map(|y| taps(y, ih, oh)).collect();
    let xs: Vec<_> = (0..ow).map(|x| taps(x, iw, ow)).collect();
    let mut out = vec![0f32; c * oh * ow];
    for ch in 0..c {
        let s = &src[ch * ih * iw..(ch + 1) * ih * iw];
        for (y, (yi, yw)) in ys.iter().enumerate() {
            for (x, (xi, xw)) in xs.iter().enumerate() {
                let mut acc = 0.0;
                for a in 0..4 {
                    let mut row = 0.0;
                    for b in 0..4 {
                        row += xw[b] * s[yi[a] * iw + xi[b]];
                    }
                    acc += yw[a] * row;
                }
                out[ch * oh * ow + y * ow + x] = acc;
            }
        }
    }
    out
}

// ---------- FPN neck ----------

#[derive(Module, Debug)]
pub struct FpnNeck {
    convs: Vec<Conv2d>,
}

impl FpnNeck {
    /// Levels 0..2 (strides 4, 8, 16) after dropping the coarsest (scalp 1).
    fn forward(&self, xs: Vec<Tensor<4>>) -> Vec<Tensor<4>> {
        let n = xs.len() - 1;
        let mut out: Vec<Option<Tensor<4>>> = vec![None; xs.len()];
        let mut prev: Option<Tensor<4>> = None;
        for i in (0..=n).rev() {
            let lateral = self.convs[n - i].forward(xs[i].clone());
            // fpn_top_down_levels [2, 3]
            let cur = match prev.take() {
                Some(p) if i >= 2 => {
                    let [_, _, h, w] = p.dims();
                    lateral + interpolate(p, InterpolateOptions::new(InterpolateMode::Nearest).with_output_size([h * 2, w * 2]))
                }
                _ => lateral,
            };
            prev = Some(cur.clone());
            out[i] = Some(cur);
        }
        out.into_iter().take(n).map(Option::unwrap).collect()
    }
}

#[derive(Module, Debug)]
pub struct ImageEncoder {
    pub trunk: Hiera,
    neck: FpnNeck,
}

// ---------- prompt encoder ----------

#[derive(Module, Debug)]
pub struct PeLayer {
    pub positional_encoding_gaussian_matrix: Param<Tensor<2>>,
}

impl PeLayer {
    /// coords in 0..1, shape [.., 2] -> [.., 256]
    fn encode<const D: usize>(&self, coords: Tensor<D>) -> Tensor<D> {
        let c = coords * 2.0 - 1.0;
        let dims = c.dims();
        let n: usize = dims[..D - 1].iter().product();
        let c = c.reshape([n, 2]).matmul(self.positional_encoding_gaussian_matrix.val()) * (2.0 * std::f32::consts::PI);
        let e = Tensor::cat(vec![c.clone().sin(), c.cos()], 1);
        let mut out = dims;
        out[D - 1] = e.dims()[1];
        e.reshape(out)
    }
}

#[derive(Module, Debug)]
pub struct MaskDownscaling {
    c1: Conv2d,
    n1: LayerNorm2d,
    c2: Conv2d,
    n2: LayerNorm2d,
    c3: Conv2d,
}

#[derive(Module, Debug)]
pub struct PromptEncoder {
    pe_layer: PeLayer,
    point_embeddings: Vec<Embedding>,
    not_a_point_embed: Embedding,
    mask_downscaling: MaskDownscaling,
    no_mask_embed: Embedding,
}

impl PromptEncoder {
    fn new(dev: &Device) -> Self {
        let emb = || EmbeddingConfig::new(1, D_MODEL).init(dev);
        PromptEncoder {
            pe_layer: PeLayer { positional_encoding_gaussian_matrix: Param::from_tensor(Tensor::zeros([2, D_MODEL / 2], dev)) },
            point_embeddings: (0..4).map(|_| emb()).collect(),
            not_a_point_embed: emb(),
            mask_downscaling: MaskDownscaling {
                c1: conv(1, 4, 2, 2, 0, dev),
                n1: LayerNorm2d::new(4, dev),
                c2: conv(4, 16, 2, 2, 0, dev),
                n2: LayerNorm2d::new(16, dev),
                c3: conv(16, D_MODEL, 1, 1, 0, dev),
            },
            no_mask_embed: emb(),
        }
    }

    /// Positional encoding of the 64x64 embedding grid, [1, 256, 64, 64].
    pub fn dense_pe(&self, dev: &Device) -> Tensor<4> {
        let n = IMAGE_SIZE / 16;
        let mut g = Vec::with_capacity(n * n * 2);
        for y in 0..n {
            for x in 0..n {
                g.push((x as f32 + 0.5) / n as f32);
                g.push((y as f32 + 0.5) / n as f32);
            }
        }
        let pe = self.pe_layer.encode(Tensor::<3>::from_data(burn::tensor::TensorData::new(g, [n, n, 2]), dev));
        pe.permute([2, 0, 1]).unsqueeze_dim(0)
    }

    /// Points in 1024-pixel model coordinates with labels (1 fg, 0 bg,
    /// 2/3 box corners); a padding point is appended as SAM 2 does.
    pub fn sparse(&self, points: &[([f32; 2], i32)], dev: &Device) -> Tensor<3> {
        let mut pts: Vec<([f32; 2], i32)> = points.to_vec();
        pts.push(([0.0, 0.0], -1));
        let coords: Vec<f32> = pts.iter().flat_map(|(p, _)| [(p[0] + 0.5) / IMAGE_SIZE as f32, (p[1] + 0.5) / IMAGE_SIZE as f32]).collect();
        let pe = self.pe_layer.encode(Tensor::<3>::from_data(burn::tensor::TensorData::new(coords, [1, pts.len(), 2]), dev));
        let rows: Vec<Tensor<3>> = pts
            .iter()
            .enumerate()
            .map(|(i, (_, l))| {
                let e = pe.clone().narrow(1, i, 1);
                match *l {
                    -1 => self.not_a_point_embed.weight.val().reshape([1, 1, D_MODEL]),
                    l => e + self.point_embeddings[l as usize].weight.val().reshape([1, 1, D_MODEL]),
                }
            })
            .collect();
        Tensor::cat(rows, 1)
    }

    /// Low-res mask logits [1, 1, 256, 256] -> [1, 256, 64, 64], or the
    /// no-mask embedding.
    pub fn dense(&self, mask: Option<Tensor<4>>, dev: &Device) -> Tensor<4> {
        let n = IMAGE_SIZE / 16;
        match mask {
            Some(m) => {
                let d = &self.mask_downscaling;
                let x = gelu(d.n1.forward(d.c1.forward(m)));
                let x = gelu(d.n2.forward(d.c2.forward(x)));
                d.c3.forward(x)
            }
            None => self.no_mask_embed.weight.val().reshape([1, D_MODEL, 1, 1]).expand([1, D_MODEL, n, n]).to_device(dev),
        }
    }
}

// ---------- two-way transformer ----------

#[derive(Module, Debug)]
pub struct Attention {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    out_proj: Linear,
    #[module(skip)]
    heads: usize,
}

impl Attention {
    fn new(dim: usize, heads: usize, down: usize, dev: &Device) -> Self {
        let i = dim / down;
        Attention { q_proj: linear(dim, i, dev), k_proj: linear(dim, i, dev), v_proj: linear(dim, i, dev), out_proj: linear(i, dim, dev), heads }
    }

    fn forward(&self, q: Tensor<3>, k: Tensor<3>, v: Tensor<3>) -> Tensor<3> {
        let sep = |x: Tensor<3>| {
            let [b, n, c] = x.dims();
            x.reshape([b, n, self.heads, c / self.heads]).swap_dims(1, 2)
        };
        let out = sdpa(sep(self.q_proj.forward(q)), sep(self.k_proj.forward(k)), sep(self.v_proj.forward(v)));
        let [b, h, n, c] = out.dims();
        self.out_proj.forward(out.swap_dims(1, 2).reshape([b, n, h * c]))
    }
}

#[derive(Module, Debug)]
pub struct TwoWayBlock {
    self_attn: Attention,
    norm1: LayerNorm,
    cross_attn_token_to_image: Attention,
    norm2: LayerNorm,
    mlp: Mlp,
    norm3: LayerNorm,
    norm4: LayerNorm,
    cross_attn_image_to_token: Attention,
    #[module(skip)]
    skip_first_layer_pe: bool,
}

impl TwoWayBlock {
    fn new(skip: bool, dev: &Device) -> Self {
        let ln = || LayerNormConfig::new(D_MODEL).init(dev);
        TwoWayBlock {
            self_attn: Attention::new(D_MODEL, 8, 1, dev),
            norm1: ln(),
            cross_attn_token_to_image: Attention::new(D_MODEL, 8, 2, dev),
            norm2: ln(),
            mlp: Mlp::new(D_MODEL, 2048, D_MODEL, 2, false, false, dev),
            norm3: ln(),
            norm4: ln(),
            cross_attn_image_to_token: Attention::new(D_MODEL, 8, 2, dev),
            skip_first_layer_pe: skip,
        }
    }

    fn forward(&self, queries: Tensor<3>, keys: Tensor<3>, qpe: Tensor<3>, kpe: Tensor<3>) -> (Tensor<3>, Tensor<3>) {
        let queries = if self.skip_first_layer_pe {
            self.self_attn.forward(queries.clone(), queries.clone(), queries)
        } else {
            let q = queries.clone() + qpe.clone();
            queries.clone() + self.self_attn.forward(q.clone(), q, queries)
        };
        let queries = self.norm1.forward(queries);
        let q = queries.clone() + qpe.clone();
        let k = keys.clone() + kpe.clone();
        let queries = self.norm2.forward(queries + self.cross_attn_token_to_image.forward(q, k, keys.clone()));
        let queries = self.norm3.forward(queries.clone() + self.mlp.forward(queries));
        let q = queries.clone() + qpe;
        let k = keys.clone() + kpe;
        let keys = self.norm4.forward(keys + self.cross_attn_image_to_token.forward(k, q, queries.clone()));
        (queries, keys)
    }
}

#[derive(Module, Debug)]
pub struct TwoWayTransformer {
    layers: Vec<TwoWayBlock>,
    final_attn_token_to_image: Attention,
    norm_final_attn: LayerNorm,
}

impl TwoWayTransformer {
    fn forward(&self, image: Tensor<4>, pe: Tensor<4>, points: Tensor<3>) -> (Tensor<3>, Tensor<3>) {
        let [b, c, h, w] = image.dims();
        let keys0 = image.reshape([b, c, h * w]).permute([0, 2, 1]);
        let kpe = pe.reshape([b, c, h * w]).permute([0, 2, 1]);
        let (mut q, mut k) = (points.clone(), keys0);
        for l in &self.layers {
            (q, k) = l.forward(q, k, points.clone(), kpe.clone());
        }
        let attn = self.final_attn_token_to_image.forward(q.clone() + points, k.clone() + kpe, k.clone());
        (self.norm_final_attn.forward(q + attn), k)
    }
}

// ---------- mask decoder ----------

#[derive(Module, Debug)]
pub struct OutputUpscaling {
    dc1: ConvTranspose2d,
    ln1: LayerNorm2d,
    dc2: ConvTranspose2d,
}

#[derive(Module, Debug)]
pub struct MaskDecoder {
    transformer: TwoWayTransformer,
    iou_token: Embedding,
    mask_tokens: Embedding,
    obj_score_token: Embedding,
    output_upscaling: OutputUpscaling,
    pub conv_s0: Conv2d,
    pub conv_s1: Conv2d,
    output_hypernetworks_mlps: Vec<Mlp>,
    iou_prediction_head: Mlp,
    pred_obj_score_head: Mlp,
}

/// Mask logits [1, n, 256, 256] and predicted IoU [1, n], with the mask
/// token outputs [1, n, 256] (object pointers come from them) and the
/// object score logit [1, 1] (> 0: the object is there).
pub struct Masks {
    pub logits: Tensor<4>,
    pub iou: Tensor<2>,
    pub tokens: Tensor<3>,
    pub obj_score: Tensor<2>,
}

impl MaskDecoder {
    fn new(dev: &Device) -> Self {
        let up = |i: usize, o: usize| ConvTranspose2dConfig::new([i, o], [2, 2]).with_stride([2, 2]).init(dev);
        MaskDecoder {
            transformer: TwoWayTransformer {
                layers: vec![TwoWayBlock::new(true, dev), TwoWayBlock::new(false, dev)],
                final_attn_token_to_image: Attention::new(D_MODEL, 8, 2, dev),
                norm_final_attn: LayerNormConfig::new(D_MODEL).init(dev),
            },
            iou_token: EmbeddingConfig::new(1, D_MODEL).init(dev),
            mask_tokens: EmbeddingConfig::new(4, D_MODEL).init(dev),
            obj_score_token: EmbeddingConfig::new(1, D_MODEL).init(dev),
            output_upscaling: OutputUpscaling { dc1: up(D_MODEL, D_MODEL / 4), ln1: LayerNorm2d::new(D_MODEL / 4, dev), dc2: up(D_MODEL / 4, D_MODEL / 8) },
            conv_s0: conv(D_MODEL, D_MODEL / 8, 1, 1, 0, dev),
            conv_s1: conv(D_MODEL, D_MODEL / 4, 1, 1, 0, dev),
            output_hypernetworks_mlps: (0..4).map(|_| Mlp::new(D_MODEL, D_MODEL, D_MODEL / 8, 3, false, false, dev)).collect(),
            iou_prediction_head: Mlp::new(D_MODEL, 256, 4, 3, false, true, dev),
            pred_obj_score_head: Mlp::new(D_MODEL, D_MODEL, 1, 3, false, false, dev),
        }
    }

    /// All four masks (0 = single-mask token, 1..3 = multimask).
    pub fn forward(&self, image: Tensor<4>, pe: Tensor<4>, sparse: Tensor<3>, dense: Tensor<4>, s0: Tensor<4>, s1: Tensor<4>) -> Masks {
        let tokens = Tensor::cat(vec![self.obj_score_token.weight.val(), self.iou_token.weight.val(), self.mask_tokens.weight.val()], 0).unsqueeze_dim(0);
        let tokens = Tensor::cat(vec![tokens, sparse], 1);
        let src = image + dense;
        let [b, c, h, w] = src.dims();
        let (hs, src) = self.transformer.forward(src, pe, tokens);
        let obj_out = hs.clone().narrow(1, 0, 1).reshape([b, c]);
        let iou_out = hs.clone().narrow(1, 1, 1).reshape([b, c]);
        let mask_out = hs.narrow(1, 2, 4);
        let src = src.swap_dims(1, 2).reshape([b, c, h, w]);
        let u = &self.output_upscaling;
        let up = gelu(u.ln1.forward(u.dc1.forward(src) + s1));
        let up = gelu(u.dc2.forward(up) + s0);
        let hyper: Vec<Tensor<3>> = self.output_hypernetworks_mlps.iter().enumerate().map(|(i, m)| m.forward(mask_out.clone().narrow(1, i, 1))).collect();
        let hyper = Tensor::cat(hyper, 1);
        let [_, uc, uh, uw] = up.dims();
        let logits = hyper.matmul(up.reshape([b, uc, uh * uw])).reshape([b, 4, uh, uw]);
        let iou = self.iou_prediction_head.forward(iou_out);
        let obj_score = self.pred_obj_score_head.forward(obj_out);
        Masks { logits, iou, tokens: mask_out, obj_score }
    }
}

// ---------- whole model ----------

#[derive(Module, Debug)]
pub struct Sam2 {
    pub image_encoder: ImageEncoder,
    pub sam_prompt_encoder: PromptEncoder,
    pub sam_mask_decoder: MaskDecoder,
    pub no_mem_embed: Param<Tensor<3>>,
    // Video tracking (memory) parts, see memory.rs.
    pub memory_attention: crate::memory::MemoryAttention,
    pub memory_encoder: crate::memory::MemoryEncoder,
    pub maskmem_tpos_enc: Param<Tensor<4>>,
    pub no_obj_ptr: Param<Tensor<2>>,
    pub no_obj_embed_spatial: Param<Tensor<2>>,
    pub obj_ptr_proj: Mlp,
    pub obj_ptr_tpos_proj: Linear,
    pub mask_downsample: Conv2d,
}

/// Image features for prompting: the 64x64 embedding (with the no-memory
/// embedding added, as for a single image), the same without it (for
/// tracking with memory) and two high-res maps.
#[derive(Clone)]
pub struct Features {
    pub embed: Tensor<4>,
    pub raw: Tensor<4>,
    pub s0: Tensor<4>,
    pub s1: Tensor<4>,
}

impl Sam2 {
    pub fn new(cfg: &Sam2Config, dev: &Device) -> Self {
        let trunk = Hiera::new(cfg, dev);
        let mut chans: Vec<usize> = trunk.stage_ends.iter().map(|&i| trunk.blocks[i].attn.proj.weight.dims()[1]).collect();
        chans.reverse();
        Sam2 {
            image_encoder: ImageEncoder { trunk, neck: FpnNeck { convs: chans.iter().map(|&c| conv(c, D_MODEL, 1, 1, 0, dev)).collect() } },
            sam_prompt_encoder: PromptEncoder::new(dev),
            sam_mask_decoder: MaskDecoder::new(dev),
            no_mem_embed: Param::from_tensor(Tensor::zeros([1, 1, D_MODEL], dev)),
            memory_attention: crate::memory::MemoryAttention::new(dev),
            memory_encoder: crate::memory::MemoryEncoder::new(dev),
            maskmem_tpos_enc: Param::from_tensor(Tensor::zeros([crate::memory::NUM_MASKMEM, 1, 1, crate::memory::MEM_DIM], dev)),
            no_obj_ptr: Param::from_tensor(Tensor::zeros([1, D_MODEL], dev)),
            no_obj_embed_spatial: Param::from_tensor(Tensor::zeros([1, crate::memory::MEM_DIM], dev)),
            obj_ptr_proj: Mlp::new(D_MODEL, D_MODEL, D_MODEL, 3, false, false, dev),
            obj_ptr_tpos_proj: linear(D_MODEL, crate::memory::MEM_DIM, dev),
            mask_downsample: conv(1, 1, 4, 4, 0, dev),
        }
    }

    /// Window-tiled, bicubically resized Hiera position embedding for the
    /// 256x256 patch grid, BHWC. Computed once per loaded model.
    pub fn hiera_pos_embed(&self, dev: &Device) -> Tensor<4> {
        let t = &self.image_encoder.trunk;
        let pe = t.pos_embed.val();
        let [_, c, ih, iw] = pe.dims();
        let n = IMAGE_SIZE / 4;
        let src: Vec<f32> = pe.into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
        let big = bicubic(&src, c, (ih, iw), (n, n));
        let win = t.pos_embed_window.val();
        let ws = win.dims()[2];
        let win: Vec<f32> = win.into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
        let mut out = vec![0f32; n * n * c];
        for y in 0..n {
            for x in 0..n {
                for ch in 0..c {
                    out[(y * n + x) * c + ch] = big[ch * n * n + y * n + x] + win[ch * ws * ws + (y % ws) * ws + x % ws];
                }
            }
        }
        Tensor::from_data(burn::tensor::TensorData::new(out, [1, n, n, c]), dev)
    }

    /// Normalized image [1, 3, 1024, 1024] -> features.
    pub fn encode(&self, image: Tensor<4>, pos: Tensor<4>) -> Features {
        let xs = self.image_encoder.trunk.forward(image, pos);
        let mut f = self.image_encoder.neck.forward(xs);
        let embed = f.pop().unwrap();
        let s1 = self.sam_mask_decoder.conv_s1.forward(f.pop().unwrap());
        let s0 = self.sam_mask_decoder.conv_s0.forward(f.pop().unwrap());
        let [b, c, h, w] = embed.dims();
        let raw = embed.clone();
        let embed = embed + self.no_mem_embed.val().reshape([1, c, 1, 1]).expand([b, c, h, w]);
        Features { embed, raw, s0, s1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bicubic_keeps_constant_and_matches_size() {
        let src = vec![2.5f32; 2 * 7 * 7];
        let out = bicubic(&src, 2, (7, 7), (16, 16));
        assert_eq!(out.len(), 2 * 16 * 16);
        assert!(out.iter().all(|v| (v - 2.5).abs() < 1e-5));
        // Same size is the identity.
        let ramp: Vec<f32> = (0..25).map(|i| i as f32).collect();
        let same = bicubic(&ramp, 1, (5, 5), (5, 5));
        assert!(same.iter().zip(&ramp).all(|(a, b)| (a - b).abs() < 1e-5));
    }

    #[test]
    fn windows_round_trip_with_padding() {
        let dev = Device::default();
        let x = Tensor::<4>::from_data(burn::tensor::TensorData::new((0..2 * 10 * 9 * 3).map(|i| i as f32).collect::<Vec<f32>>(), [2, 10, 9, 3]), &dev);
        let (w, pad) = window_partition(x.clone(), 4);
        assert_eq!(pad, [12, 12]);
        assert_eq!(w.dims(), [2 * 9, 4, 4, 3]);
        let back = window_unpartition(w, 4, pad, [10, 9]);
        let a: Vec<f32> = back.into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
        let b: Vec<f32> = x.into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn decoder_shapes() {
        let dev = Device::default();
        let m = Sam2::new(&Sam2Config::tiny(), &dev);
        let pe = m.sam_prompt_encoder.dense_pe(&dev);
        assert_eq!(pe.dims(), [1, 256, 64, 64]);
        let sparse = m.sam_prompt_encoder.sparse(&[([500.0, 400.0], 1), ([10.0, 20.0], 0)], &dev);
        assert_eq!(sparse.dims(), [1, 3, 256]);
        let dense = m.sam_prompt_encoder.dense(None, &dev);
        let out = m.sam_mask_decoder.forward(
            Tensor::zeros([1, 256, 64, 64], &dev),
            pe,
            sparse,
            dense,
            Tensor::zeros([1, 32, 256, 256], &dev),
            Tensor::zeros([1, 64, 128, 128], &dev),
        );
        assert_eq!(out.logits.dims(), [1, 4, 256, 256]);
        assert_eq!(out.iou.dims(), [1, 4]);
        let masked = m.sam_prompt_encoder.dense(Some(Tensor::zeros([1, 1, 256, 256], &dev)), &dev);
        assert_eq!(masked.dims(), [1, 256, 64, 64]);
    }
}
