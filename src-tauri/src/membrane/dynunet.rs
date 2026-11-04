//! The network of the published MemBrain-seg models (v10), so a checkpoint
//! the user downloaded from the MemBrain-seg authors can be loaded as it
//! is: a residual 3D U-Net in the layout of MONAI's DynUNet (six levels,
//! widths 32–1024, 3³ kernels, instance norm with scale and shift, leaky
//! ReLU 0.01, transposed-convolution upsampling, convolutions without
//! bias). Written here from that layout; field names follow the
//! checkpoint's so its tensors map one to one. Only the full-resolution
//! output is used; the deep-supervision heads are left out.
//!
//! Checked against MemBrain-seg 0.0.10 (PyTorch) on `MemBrain_seg_v10_beta.ckpt`:
//! all 91 tensors load, and on a 64³ input every voxel has the same sign,
//! largest logit difference 5e-4 on Metal.

use burn::module::Module;
use burn::nn::conv::{Conv3d, Conv3dConfig, ConvTranspose3d, ConvTranspose3dConfig};
use burn::nn::{InstanceNorm, InstanceNormConfig, PaddingConfig3d};
use burn::tensor::activation::leaky_relu;
use burn::tensor::module::conv3d;
use burn::tensor::ops::ConvOptions;
use burn::tensor::{Device, Slice, Tensor};
use burn_store::{ModuleSnapshot, PytorchStore};

const SLOPE: f64 = 0.01;
pub const WIDTHS: [usize; 6] = [32, 64, 128, 256, 512, 1024];

/// A convolution as the checkpoint stores it (`<name>.conv.weight`).
#[derive(Module, Debug)]
struct Conv {
    conv: Conv3d,
}

impl Conv {
    fn new(i: usize, o: usize, k: usize, stride: usize, bias: bool, dev: &Device) -> Conv {
        let p = k / 2;
        Conv {
            conv: Conv3dConfig::new([i, o], [k; 3])
                .with_stride([stride; 3])
                .with_padding(PaddingConfig3d::Explicit(p, p, p))
                .with_bias(bias)
                .init(dev),
        }
    }
}

impl Conv {
    fn forward(&self, x: Tensor<5>) -> Tensor<5> {
        conv(x, self.conv.weight.val(), self.conv.bias.as_ref().map(|b| b.val()), self.conv.stride[0])
    }
}

/// A cubic convolution (side 1 or 3, padding side/2, stride 1 or 2). Burn's
/// direct kernel is slow wherever there are many channels (Metal, M1 Max:
/// 1024 channels on 5³ took 0.54 s), so those are one matrix product per
/// kernel offset, summed: 10–50× faster. Its kernel stays for the
/// full-resolution 32-channel convolutions, except the measured Metal
/// matrix-instruction path below.
fn conv(x: Tensor<5>, w: Tensor<5>, bias: Option<Tensor<1>>, stride: usize) -> Tensor<5> {
    let [b, c, d, h, wd] = x.dims();
    let [o, _, k, ..] = w.dims();
    let p = k / 2;
    // Only the four measured full-resolution convolutions use this path.
    // Other shapes, backends, precision and autodiff keep their existing ops.
    #[cfg(all(target_os = "macos", not(feature = "train-fusion")))]
    if [b, c, d, h, wd] == [1, 32, 160, 160, 160] && w.dims() == [32, 32, 3, 3, 3] && stride == 1 && bias.is_none() {
        if let Some(y) = metal_conv32(&x, &w) {
            return y;
        }
    }
    if c <= 32 && stride == 1 && k == 3 {
        return conv3d(x, w, bias, ConvOptions::new([1; 3], [p; 3], [1; 3], 1));
    }
    let out = |n: usize| (n + 2 * p - k) / stride + 1;
    let (od, oh, ow) = (out(d), out(h), out(wd));
    let x = if p > 0 { x.pad([(0, 0), (0, 0), (p, p), (p, p), (p, p)], burn::tensor::ops::PadMode::Constant(0.0)) } else { x };
    let span = |at: usize, n: usize| Slice { start: at as isize, end: Some((at + stride * (n - 1) + 1) as isize), step: stride as isize };
    let n = od * oh * ow;
    let mut acc: Option<Tensor<3>> = None;
    for kz in 0..k {
        for ky in 0..k {
            for kx in 0..k {
                let xs = x.clone().slice([Slice::from(0..b), Slice::from(0..c), span(kz, od), span(ky, oh), span(kx, ow)]).reshape([b, c, n]);
                let wk = w.clone().slice([0..o, 0..c, kz..kz + 1, ky..ky + 1, kx..kx + 1]).reshape([1, o, c]);
                let r = wk.repeat_dim(0, b).matmul(xs);
                acc = Some(match acc {
                    None => r,
                    Some(a) => a + r,
                });
            }
        }
    }
    let mut y = acc.unwrap();
    if let Some(bias) = bias {
        y = y + bias.reshape([1, o, 1]);
    }
    y.reshape([b, o, od, oh, ow])
}

/// Reuse Burn's fp32 implicit-GEMM convolution rather than materializing a
/// multi-gigabyte im2col tensor. Metal's matrix instructions make the measured
/// 32→32,160³ convolution about twice as fast as its default direct kernel.
#[cfg(all(target_os = "macos", not(feature = "train-fusion")))]
fn metal_conv32(x: &Tensor<5>, w: &Tensor<5>) -> Option<Tensor<5>> {
    use burn::tensor::DType;
    use burn_cubecl::{CubeBackend, CubeDevice, cubecl::ir::{ElemType, FloatKind}, kernel::conv::{conv_forward, ConvStrategy}};
    // Conversion rejects CPU and autodiff tensors without touching
    // the original values. CubeBackend also covers CUDA, so test the actual
    // runtime instead of assuming the Rust backend type identifies Metal.
    let x = x.clone().try_into_primitive::<CubeBackend>().ok()?;
    let w = w.clone().try_into_primitive::<CubeBackend>().ok()?;
    // Auto-selected devices keep backend=Auto even after choosing Metal.
    // CubeCL assigns this runtime name only to actual Metal using MSL.
    if !matches!(&x.device, CubeDevice::Wgpu(_)) || x.client.name() != "wgpu<msl>" || x.dtype != DType::F32 || w.dtype != DType::F32 {
        return None;
    }
    let f32 = ElemType::Float(FloatKind::F32);
    if !x.client.properties().features.matmul.cmma.iter().any(|config| config.a_type == f32 && config.b_type == f32 && config.cd_type == f32) {
        return None;
    }
    // A launch/device failure is not an unsupported-device fallback: let the
    // existing read_window guard report it before any result can be saved.
    let y = conv_forward::<3>(x, w, None, ConvOptions::new([1; 3], [1; 3], [1; 3], 1), ConvStrategy::ImplicitGemm)
        .unwrap_or_else(|e| panic!("膜卷积矩阵计算失败：{e:?}"));
    Some(Tensor::from_primitive::<CubeBackend>(y))
}

/// The ×2 transposed convolution (side 2, stride 2) as eight matrix
/// products, one per output parity, interleaved (as `conv`, for speed).
fn up2(x: Tensor<5>, w: Tensor<5>) -> Tensor<5> {
    let [b, c, d, h, wd] = x.dims();
    let o = w.dims()[1];
    let n = d * h * wd;
    let x = x.reshape([b, c, n]);
    let mut parts = Vec::with_capacity(8);
    for a in 0..2 {
        for e in 0..2 {
            for f in 0..2 {
                let wk = w.clone().slice([0..c, 0..o, a..a + 1, e..e + 1, f..f + 1]).reshape([1, c, o]).swap_dims(1, 2).repeat_dim(0, b);
                parts.push(wk.matmul(x.clone()).reshape([b, o, d, h, wd]));
            }
        }
    }
    // [2, 2, 2, b, o, d, h, w] → [b, o, d, 2, h, 2, w, 2].
    let t: Tensor<8> = Tensor::stack::<6>(parts, 0).reshape([2, 2, 2, b, o, d, h, wd]);
    t.permute([3, 4, 5, 0, 6, 1, 7, 2]).reshape([b, o, 2 * d, 2 * h, 2 * wd])
}

fn norm(c: usize, dev: &Device) -> InstanceNorm {
    InstanceNormConfig::new(c).with_affine(true).init(dev)
}

/// Instance norm with the statistics reduced in two stages: Burn's own
/// reduces each channel's millions of voxels in one pass and took as long
/// as a convolution on a 160³ window (0.38 s against 0.03 s, Metal).
fn inorm(n: &InstanceNorm, x: Tensor<5>) -> Tensor<5> {
    let d = x.dims();
    let (bc, len) = (d[0] * d[1], d[2] * d[3] * d[4]);
    // Blocks of the largest power of two (up to 4096) dividing the length.
    let k = (1usize << len.trailing_zeros().min(12)).min(len);
    let mean = |t: Tensor<2>| t.reshape([bc, k, len / k]).mean_dim(2).mean_dim(1).reshape([bc, 1]);
    let x = x.reshape([bc, len]);
    let c = x.clone() - mean(x);
    let var = mean(c.clone().powi_scalar(2));
    let y = c / (var + n.epsilon).sqrt();
    let affine = |p: &Option<burn::module::Param<Tensor<1>>>| p.as_ref().map(|p| p.val().reshape([1, d[1]]).repeat_dim(0, d[0]).reshape([bc, 1]));
    match (affine(&n.gamma), affine(&n.beta)) {
        (Some(g), Some(b)) => (y * g + b).reshape(d),
        _ => y.reshape(d),
    }
}

/// conv–norm–act–conv–norm plus the input (through a strided 1³
/// convolution and norm when the shape changes), then act.
#[derive(Module, Debug)]
struct ResBlock {
    conv1: Conv,
    norm1: InstanceNorm,
    conv2: Conv,
    norm2: InstanceNorm,
    conv3: Option<Conv>,
    norm3: Option<InstanceNorm>,
}

impl ResBlock {
    fn new(i: usize, o: usize, stride: usize, dev: &Device) -> ResBlock {
        let project = i != o || stride != 1;
        ResBlock {
            conv1: Conv::new(i, o, 3, stride, false, dev),
            norm1: norm(o, dev),
            conv2: Conv::new(o, o, 3, 1, false, dev),
            norm2: norm(o, dev),
            conv3: project.then(|| Conv::new(i, o, 1, stride, false, dev)),
            norm3: project.then(|| norm(o, dev)),
        }
    }

    fn forward(&self, x: Tensor<5>) -> Tensor<5> {
        let h = leaky_relu(inorm(&self.norm1, self.conv1.forward(x.clone())), SLOPE);
        let h = inorm(&self.norm2, self.conv2.forward(h));
        let s = match (&self.conv3, &self.norm3) {
            (Some(c), Some(n)) => inorm(n, c.forward(x)),
            _ => x,
        };
        leaky_relu(h + s, SLOPE)
    }
}

/// conv–norm–act twice.
#[derive(Module, Debug)]
struct BasicBlock {
    conv1: Conv,
    norm1: InstanceNorm,
    conv2: Conv,
    norm2: InstanceNorm,
}

impl BasicBlock {
    fn new(i: usize, o: usize, dev: &Device) -> BasicBlock {
        BasicBlock { conv1: Conv::new(i, o, 3, 1, false, dev), norm1: norm(o, dev), conv2: Conv::new(o, o, 3, 1, false, dev), norm2: norm(o, dev) }
    }

    /// The block on the channels of `a` and then `b` (the upsampled input
    /// and the skip), without joining them: at the finest level of a 160³
    /// window the joined tensor passes 1 GB, and on Metal (M1 Max) such
    /// tensors gave different results from run to run. The first kernel is
    /// split by input channel instead and the two halves summed.
    fn forward_pair(&self, a: Tensor<5>, b: Tensor<5>) -> Tensor<5> {
        let w = self.conv1.conv.weight.val();
        let [_, i, ..] = w.dims();
        let ca = a.dims()[1];
        let h = conv(a, w.clone().narrow(1, 0, ca), None, 1) + conv(b, w.narrow(1, ca, i - ca), None, 1);
        let h = leaky_relu(inorm(&self.norm1, h), SLOPE);
        leaky_relu(inorm(&self.norm2, self.conv2.forward(h)), SLOPE)
    }
}

#[derive(Module, Debug)]
struct TransConv {
    conv: ConvTranspose3d,
}

/// Upsample ×2, join the skip, two convolutions.
#[derive(Module, Debug)]
struct UpBlock {
    transp_conv: TransConv,
    conv_block: BasicBlock,
}

impl UpBlock {
    fn new(i: usize, o: usize, dev: &Device) -> UpBlock {
        UpBlock {
            transp_conv: TransConv { conv: ConvTranspose3dConfig::new([i, o], [2; 3]).with_stride([2; 3]).with_bias(false).init(dev) },
            conv_block: BasicBlock::new(2 * o, o, dev),
        }
    }

    fn forward(&self, x: Tensor<5>, skip: Tensor<5>) -> Tensor<5> {
        let up = up2(x, self.transp_conv.conv.weight.val());
        self.conv_block.forward_pair(up, skip)
    }
}

/// The 1³ output convolution (`output_block.conv.conv`).
#[derive(Module, Debug)]
struct OutBlock {
    conv: Conv,
}

#[derive(Module, Debug)]
pub struct DynUNet {
    input_block: ResBlock,
    downsamples: Vec<ResBlock>,
    bottleneck: ResBlock,
    /// Deepest first.
    upsamples: Vec<UpBlock>,
    output_block: OutBlock,
}

impl DynUNet {
    pub fn new(dev: &Device) -> DynUNet {
        let w = WIDTHS;
        let n = w.len();
        DynUNet {
            input_block: ResBlock::new(1, w[0], 1, dev),
            downsamples: (1..n - 1).map(|l| ResBlock::new(w[l - 1], w[l], 2, dev)).collect(),
            bottleneck: ResBlock::new(w[n - 2], w[n - 1], 2, dev),
            upsamples: (1..n).rev().map(|l| UpBlock::new(w[l], w[l - 1], dev)).collect(),
            output_block: OutBlock { conv: Conv::new(w[0], 1, 1, 1, true, dev) },
        }
    }

    /// Input sides must be multiples of this.
    pub fn align(&self) -> usize {
        1 << (WIDTHS.len() - 1)
    }

    /// Membrane logits, [n, 1, d, h, w] for input [n, 1, d, h, w].
    pub fn forward(&self, x: Tensor<5>) -> Tensor<5> {
        let mut skips = vec![self.input_block.forward(x)];
        for d in &self.downsamples {
            let h = d.forward(skips.last().unwrap().clone());
            skips.push(h);
        }
        let mut h = self.bottleneck.forward(skips.last().unwrap().clone());
        for up in &self.upsamples {
            h = up.forward(h, skips.pop().unwrap());
        }
        self.output_block.conv.forward(h)
    }
}

/// Loads a MemBrain-seg checkpoint (PyTorch Lightning: the weights under
/// `state_dict`, prefixed `_model.`; instance norms store `weight`/`bias`
/// where Burn has `gamma`/`beta`). Tensors the network does not use (the
/// deep-supervision heads, optimiser state) are ignored; a missing one is an
/// error, so a checkpoint of another network is refused.
pub fn load(path: &std::path::Path, dev: &Device) -> anyhow::Result<DynUNet> {
    let mut net = DynUNet::new(dev);
    let mut store = PytorchStore::from_file(path)
        .with_top_level_key("state_dict")
        .allow_partial(true)
        .with_key_remapping(r"^_model\.", "")
        .with_key_remapping(r"(norm\d)\.weight$", "$1.gamma")
        .with_key_remapping(r"(norm\d)\.bias$", "$1.beta");
    let r = net.load_from(&mut store).map_err(|e| anyhow::anyhow!("读不了这个 MemBrain-seg 模型文件：{e}"))?;
    if !r.missing.is_empty() || !r.errors.is_empty() {
        anyhow::bail!("这个文件不是 MemBrain-seg v10 模型（缺 {} 个参数，{} 个形状不符）", r.missing.len(), r.errors.len());
    }
    Ok(net)
}

#[cfg(test)]
mod tests {
    use super::*;
    use burn::tensor::Distribution;

    #[cfg(all(target_os = "macos", not(feature = "train-fusion")))]
    #[test]
    fn metal_matrix_path_preserves_cpu_fallback() {
        let dev = Device::flex();
        let x = Tensor::<5>::ones([1, 32, 2, 2, 2], &dev);
        let w = Tensor::<5>::ones([32, 32, 3, 3, 3], &dev);
        assert!(metal_conv32(&x, &w).is_none());
        assert_eq!(x.sum().into_scalar::<f32>(), 256.0);
    }

    #[cfg(all(target_os = "macos", not(feature = "train-fusion")))]
    #[test]
    #[ignore = "requires a Metal GPU and full 160-cubed activation buffers"]
    fn metal_matrix_convolution_matches_direct_with_strided_weights() {
        let dev = Device::wgpu_options()
            .graphics_api(burn::tensor::wgpu::WgpuBackend::Metal)
            .device_kind(burn::tensor::DeviceKind::IntegratedGpu(0))
            .init().unwrap();
        dev.seed(0x29c784b1);
        let x = Tensor::<5>::random([1, 32, 160, 160, 160], Distribution::Normal(0.0, 1.0), &dev);
        // The decoder's split skip convolution uses this non-contiguous view.
        let w = Tensor::<5>::random([32, 64, 3, 3, 3], Distribution::Normal(0.0, 0.1), &dev).narrow(1, 32, 32);
        let want = conv3d(x.clone(), w.clone(), None, ConvOptions::new([1; 3], [1; 3], [1; 3], 1));
        assert!(want.clone().abs().max().into_scalar::<f32>() > 0.1);
        let got = metal_conv32(&x, &w).expect("fp32 Metal matrix path unavailable");
        let max = (got - want).abs().max().into_scalar::<f32>();
        assert!(max.is_finite() && max < 1e-4, "maximum difference {max}");
    }

    #[test]
    fn matrix_convolutions_match_burn() {
        let dev = Device::flex();
        let x = Tensor::<5>::random([2, 40, 6, 5, 7], Distribution::Normal(0.0, 1.0), &dev);
        for (k, stride) in [(3, 1), (3, 2), (1, 1), (1, 2)] {
            let w = Tensor::<5>::random([24, 40, k, k, k], Distribution::Normal(0.0, 0.1), &dev);
            let b = Tensor::<1>::random([24], Distribution::Normal(0.0, 0.1), &dev);
            let want = conv3d(x.clone(), w.clone(), Some(b.clone()), ConvOptions::new([stride; 3], [k / 2; 3], [1; 3], 1));
            let got = conv(x.clone(), w, Some(b), stride);
            assert_eq!(got.dims(), want.dims());
            assert!((got - want).abs().max().into_scalar::<f32>() < 1e-4, "side {k} stride {stride}");
        }
        let up = ConvTranspose3dConfig::new([40, 16], [2; 3]).with_stride([2; 3]).with_bias(false).init(&dev);
        let want = up.forward(x.clone());
        let got = up2(x, up.weight.val());
        assert_eq!(got.dims(), want.dims());
        assert!((got - want).abs().max().into_scalar::<f32>() < 1e-4);
    }

    #[test]
    fn instance_norm_matches_burn() {
        let dev = Device::flex();
        let mut n = norm(6, &dev);
        n.gamma = Some(burn::module::Param::from_tensor(Tensor::<1>::random([6], Distribution::Normal(1.0, 0.3), &dev)));
        n.beta = Some(burn::module::Param::from_tensor(Tensor::<1>::random([6], Distribution::Normal(0.0, 0.3), &dev)));
        let x = Tensor::<5>::random([2, 6, 8, 6, 10], Distribution::Normal(3.0, 2.0), &dev);
        assert!((inorm(&n, x.clone()) - n.forward(x)).abs().max().into_scalar::<f32>() < 1e-4);
    }
}
