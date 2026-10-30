//! SAM 2.1 video tracking parts in Burn: the memory encoder (a predicted
//! mask fused with the frame's features into a 64-channel memory), memory
//! attention (the current frame's features attend to the memories of
//! earlier frames and their object pointers) and the position encodings
//! they use. Field names follow the PyTorch module tree so checkpoints
//! load as they are.
//!
//! Written after the method of facebookresearch/sam2 (Meta Platforms, Inc.
//! and affiliates, Apache License 2.0).

use burn::module::{Module, Param};
use burn::nn::conv::{Conv2d, Conv2dConfig};
use burn::nn::{LayerNorm, LayerNormConfig, Linear, PaddingConfig2d};
use burn::tensor::activation::{gelu, relu, softmax};
use burn::tensor::{Device, Tensor, TensorData};

use crate::model::{conv, linear, LayerNorm2d, D_MODEL};

/// Memories kept: the prompted frame and the six frames before the current one.
pub const NUM_MASKMEM: usize = 7;
/// Channels of a memory.
pub const MEM_DIM: usize = 64;
/// Object pointers of at most this many earlier frames.
pub const MAX_OBJ_PTRS: usize = 16;

// ---------- position encodings ----------

/// 2D sine position encoding [1, c, h, w]: the first half of the channels
/// encodes y, the second x, each as interleaved sin/cos of the normalized
/// coordinate (DETR style, scale 2π).
pub fn sine_pe(c: usize, h: usize, w: usize, dev: &Device) -> Tensor<4> {
    let npf = c / 2;
    let scale = 2.0 * std::f32::consts::PI;
    let dim_t: Vec<f32> = (0..npf).map(|i| 10000f32.powf(2.0 * (i / 2) as f32 / npf as f32)).collect();
    let mut out = vec![0f32; c * h * w];
    for y in 0..h {
        for x in 0..w {
            let ye = (y + 1) as f32 / (h as f32 + 1e-6) * scale;
            let xe = (x + 1) as f32 / (w as f32 + 1e-6) * scale;
            for (i, d) in dim_t.iter().enumerate() {
                let (py, px) = (ye / d, xe / d);
                let (vy, vx) = if i % 2 == 0 { (py.sin(), px.sin()) } else { (py.cos(), px.cos()) };
                out[i * h * w + y * w + x] = vy;
                out[(npf + i) * h * w + y * w + x] = vx;
            }
        }
    }
    Tensor::from_data(TensorData::new(out, [1, c, h, w]), dev)
}

/// 1D sine encoding of `pos` (already scaled), `dim` channels: sines then cosines.
pub fn sine_1d(pos: &[f32], dim: usize, dev: &Device) -> Tensor<2> {
    let half = dim / 2;
    let mut out = Vec::with_capacity(pos.len() * dim);
    for &p in pos {
        let args: Vec<f32> = (0..half).map(|i| p / 10000f32.powf(2.0 * (i / 2) as f32 / half as f32)).collect();
        out.extend(args.iter().map(|a| a.sin()));
        out.extend(args.iter().map(|a| a.cos()));
    }
    Tensor::from_data(TensorData::new(out, [pos.len(), dim]), dev)
}

/// Axial 2D rotary embedding for a `side`×`side` grid and `dim` channels,
/// as (cos, sin) [n, dim/2]: the first quarter of the channel pairs rotate
/// with x, the second with y.
pub fn rope_tables(dim: usize, side: usize) -> (Vec<f32>, Vec<f32>) {
    let q = dim / 4;
    let freqs: Vec<f32> = (0..q).map(|i| 1.0 / 10000f32.powf((4 * i) as f32 / dim as f32)).collect();
    let n = side * side;
    let (mut cos, mut sin) = (Vec::with_capacity(n * dim / 2), Vec::with_capacity(n * dim / 2));
    for t in 0..n {
        let (tx, ty) = ((t % side) as f32, (t / side) as f32);
        for &f in &freqs {
            cos.push((tx * f).cos());
            sin.push((tx * f).sin());
        }
        for &f in &freqs {
            cos.push((ty * f).cos());
            sin.push((ty * f).sin());
        }
    }
    (cos, sin)
}

/// Rotates consecutive channel pairs of `x` [b, n, dim] by the tables
/// [1, n, dim/2, 1].
fn rotate(x: Tensor<3>, cos: Tensor<4>, sin: Tensor<4>) -> Tensor<3> {
    let [b, n, d] = x.dims();
    let x = x.reshape([b, n, d / 2, 2]);
    let (re, im) = (x.clone().narrow(3, 0, 1), x.narrow(3, 1, 1));
    let out = Tensor::cat(vec![re.clone() * cos.clone() - im.clone() * sin.clone(), re * sin + im * cos], 3);
    out.reshape([b, n, d])
}

// ---------- memory attention ----------

/// Single-head attention with rotary position encoding on queries and keys.
#[derive(Module, Debug)]
pub struct RopeAttention {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    out_proj: Linear,
}

/// Rotary tables of the 64×64 feature grid, [1, 4096, 128, 1] each.
#[derive(Clone)]
pub struct Rope {
    pub cos: Tensor<4>,
    pub sin: Tensor<4>,
}

impl Rope {
    pub fn new(side: usize, dev: &Device) -> Self {
        let (c, s) = rope_tables(D_MODEL, side);
        let n = side * side;
        Rope { cos: Tensor::from_data(TensorData::new(c, [1, n, D_MODEL / 2, 1]), dev), sin: Tensor::from_data(TensorData::new(s, [1, n, D_MODEL / 2, 1]), dev) }
    }
}

impl RopeAttention {
    fn new(kv_dim: usize, dev: &Device) -> Self {
        RopeAttention { q_proj: linear(D_MODEL, D_MODEL, dev), k_proj: linear(kv_dim, D_MODEL, dev), v_proj: linear(kv_dim, D_MODEL, dev), out_proj: linear(D_MODEL, D_MODEL, dev) }
    }

    /// Keys beyond the first `k.len - exclude` are object pointer tokens
    /// and get no rotation; the rest are whole grids, each rotated like the
    /// queries.
    fn forward(&self, q: Tensor<3>, k: Tensor<3>, v: Tensor<3>, rope: &Rope, exclude: usize) -> Tensor<3> {
        let q = rotate(self.q_proj.forward(q), rope.cos.clone(), rope.sin.clone());
        let k = self.k_proj.forward(k);
        let v = self.v_proj.forward(v);
        let [b, nq, d] = q.dims();
        let nk = k.dims()[1];
        let grids = (nk - exclude) / nq;
        let rot = rotate(k.clone().narrow(1, 0, nk - exclude), rope.cos.clone().repeat_dim(1, grids), rope.sin.clone().repeat_dim(1, grids));
        let k = if exclude > 0 { Tensor::cat(vec![rot, k.narrow(1, nk - exclude, exclude)], 1) } else { rot };
        // Queries in chunks keep the attention matrix small (4096 queries
        // against up to seven 4096-token memories).
        let kt = k.swap_dims(1, 2);
        let scale = 1.0 / (d as f32).sqrt();
        let chunk = 1024;
        let outs: Vec<Tensor<3>> = (0..nq.div_ceil(chunk))
            .map(|i| {
                let n = chunk.min(nq - i * chunk);
                let a = softmax(q.clone().narrow(1, i * chunk, n).matmul(kt.clone()) * scale, 2);
                a.matmul(v.clone())
            })
            .collect();
        let _ = b;
        self.out_proj.forward(Tensor::cat(outs, 1))
    }
}

#[derive(Module, Debug)]
pub struct MemoryAttentionLayer {
    self_attn: RopeAttention,
    cross_attn_image: RopeAttention,
    linear1: Linear,
    linear2: Linear,
    norm1: LayerNorm,
    norm2: LayerNorm,
    norm3: LayerNorm,
}

impl MemoryAttentionLayer {
    fn new(dev: &Device) -> Self {
        let ln = || LayerNormConfig::new(D_MODEL).init(dev);
        MemoryAttentionLayer {
            self_attn: RopeAttention::new(D_MODEL, dev),
            cross_attn_image: RopeAttention::new(MEM_DIM, dev),
            linear1: linear(D_MODEL, 2048, dev),
            linear2: linear(2048, D_MODEL, dev),
            norm1: ln(),
            norm2: ln(),
            norm3: ln(),
        }
    }

    fn forward(&self, x: Tensor<3>, memory: Tensor<3>, memory_pos: Tensor<3>, rope: &Rope, ptr_tokens: usize) -> Tensor<3> {
        let t = self.norm1.forward(x.clone());
        let x = x + self.self_attn.forward(t.clone(), t.clone(), t, rope, 0);
        let t = self.norm2.forward(x.clone());
        let x = x + self.cross_attn_image.forward(t, memory.clone() + memory_pos, memory, rope, ptr_tokens);
        let t = self.norm3.forward(x.clone());
        x + self.linear2.forward(relu(self.linear1.forward(t)))
    }
}

#[derive(Module, Debug)]
pub struct MemoryAttention {
    layers: Vec<MemoryAttentionLayer>,
    norm: LayerNorm,
}

impl MemoryAttention {
    pub fn new(dev: &Device) -> Self {
        MemoryAttention { layers: (0..4).map(|_| MemoryAttentionLayer::new(dev)).collect(), norm: LayerNormConfig::new(D_MODEL).init(dev) }
    }

    /// Conditions the current features `x` [1, 4096, 256] (with position
    /// encoding `pos`) on `memory` [1, m, 64]; its last `ptr_tokens` tokens
    /// are object pointers.
    pub fn forward(&self, x: Tensor<3>, pos: Tensor<3>, memory: Tensor<3>, memory_pos: Tensor<3>, rope: &Rope, ptr_tokens: usize) -> Tensor<3> {
        let mut x = x + pos * 0.1;
        for l in &self.layers {
            x = l.forward(x, memory.clone(), memory_pos.clone(), rope, ptr_tokens);
        }
        self.norm.forward(x)
    }
}

// ---------- memory encoder ----------

/// Mask 1024² -> 256 channels at 64² (four stride-2 convs, then 1×1).
#[derive(Module, Debug)]
pub struct MaskDownSampler {
    convs: Vec<Conv2d>,
    norms: Vec<LayerNorm2d>,
    proj: Conv2d,
}

/// ConvNeXt block: depthwise 7×7, layer norm, MLP, layer scale, residual.
#[derive(Module, Debug)]
pub struct CxBlock {
    gamma: Param<Tensor<1>>,
    dwconv: Conv2d,
    norm: LayerNorm2d,
    pwconv1: Linear,
    pwconv2: Linear,
}

impl CxBlock {
    fn new(dev: &Device) -> Self {
        CxBlock {
            gamma: Param::from_tensor(Tensor::ones([D_MODEL], dev)),
            dwconv: Conv2dConfig::new([D_MODEL, D_MODEL], [7, 7]).with_groups(D_MODEL).with_padding(PaddingConfig2d::Explicit(3, 3, 3, 3)).init(dev),
            norm: LayerNorm2d::new(D_MODEL, dev),
            pwconv1: linear(D_MODEL, 4 * D_MODEL, dev),
            pwconv2: linear(4 * D_MODEL, D_MODEL, dev),
        }
    }

    fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let y = self.norm.forward(self.dwconv.forward(x.clone())).permute([0, 2, 3, 1]);
        let y = self.pwconv2.forward(gelu(self.pwconv1.forward(y))) * self.gamma.val().reshape([1, 1, 1, D_MODEL]);
        x + y.permute([0, 3, 1, 2])
    }
}

#[derive(Module, Debug)]
pub struct Fuser {
    layers: Vec<CxBlock>,
}

#[derive(Module, Debug)]
pub struct MemoryEncoder {
    mask_downsampler: MaskDownSampler,
    pix_feat_proj: Conv2d,
    fuser: Fuser,
    out_proj: Conv2d,
}

impl MemoryEncoder {
    pub fn new(dev: &Device) -> Self {
        let chans = [1, 4, 16, 64, 256];
        MemoryEncoder {
            mask_downsampler: MaskDownSampler {
                convs: chans.windows(2).map(|c| conv(c[0], c[1], 3, 2, 1, dev)).collect(),
                norms: chans[1..].iter().map(|&c| LayerNorm2d::new(c, dev)).collect(),
                proj: conv(D_MODEL, D_MODEL, 1, 1, 0, dev),
            },
            pix_feat_proj: conv(D_MODEL, D_MODEL, 1, 1, 0, dev),
            fuser: Fuser { layers: (0..2).map(|_| CxBlock::new(dev)).collect() },
            out_proj: conv(D_MODEL, MEM_DIM, 1, 1, 0, dev),
        }
    }

    /// Frame features [1, 256, 64, 64] and mask logits for memory
    /// (already scaled, [1, 1, 1024, 1024]) -> memory [1, 64, 64, 64].
    pub fn forward(&self, pix_feat: Tensor<4>, mask: Tensor<4>) -> Tensor<4> {
        let d = &self.mask_downsampler;
        let mut m = mask;
        for (c, n) in d.convs.iter().zip(&d.norms) {
            m = gelu(n.forward(c.forward(m)));
        }
        let m = d.proj.forward(m);
        let mut x = self.pix_feat_proj.forward(pix_feat) + m;
        for l in &self.fuser.layers {
            x = l.forward(x);
        }
        self.out_proj.forward(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rope_keeps_norms_and_starts_at_identity() {
        let (c, s) = rope_tables(8, 3);
        assert_eq!(c.len(), 9 * 4);
        // Position 0 is not rotated.
        assert!(c[..4].iter().all(|v| (v - 1.0).abs() < 1e-6) && s[..4].iter().all(|v| v.abs() < 1e-6));
        // Position 1 (x = 1, y = 0) rotates the x pairs only.
        assert!((c[4] - 1f32.cos()).abs() < 1e-6 && (c[6] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn sine_pe_layout() {
        let dev = Device::default();
        let pe = sine_pe(8, 2, 3, &dev);
        assert_eq!(pe.dims(), [1, 8, 2, 3]);
        let v: Vec<f32> = pe.into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
        // Channel 0 is sin of y: row 1 is the last, so 2π · 1 → ~0.
        let y1 = v[3];
        assert!(y1.abs() < 1e-3, "{y1}");
        // Channel 1 is cos of y.
        assert!((v[6] - (std::f32::consts::PI).cos()).abs() < 1e-3);
    }

    #[test]
    fn memory_shapes() {
        let dev = Device::default();
        let enc = MemoryEncoder::new(&dev);
        let mem = enc.forward(Tensor::zeros([1, D_MODEL, 64, 64], &dev), Tensor::zeros([1, 1, 1024, 1024], &dev));
        assert_eq!(mem.dims(), [1, MEM_DIM, 64, 64]);
        let att = MemoryAttention::new(&dev);
        let rope = Rope::new(8, &dev);
        let x = att.forward(Tensor::zeros([1, 64, D_MODEL], &dev), Tensor::zeros([1, 64, D_MODEL], &dev), Tensor::zeros([1, 2 * 64 + 4, MEM_DIM], &dev), Tensor::zeros([1, 2 * 64 + 4, MEM_DIM], &dev), &rope, 4);
        assert_eq!(x.dims(), [1, 64, D_MODEL]);
    }
}
