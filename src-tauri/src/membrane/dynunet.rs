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
use burn::tensor::{Device, Tensor};
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

fn norm(c: usize, dev: &Device) -> InstanceNorm {
    InstanceNormConfig::new(c).with_affine(true).init(dev)
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
        let h = leaky_relu(self.norm1.forward(self.conv1.conv.forward(x.clone())), SLOPE);
        let h = self.norm2.forward(self.conv2.conv.forward(h));
        let s = match (&self.conv3, &self.norm3) {
            (Some(c), Some(n)) => n.forward(c.conv.forward(x)),
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
        let opt = || ConvOptions::new([1; 3], [1; 3], [1; 3], 1);
        let h = conv3d(a, w.clone().narrow(1, 0, ca), None, opt()) + conv3d(b, w.narrow(1, ca, i - ca), None, opt());
        let h = leaky_relu(self.norm1.forward(h), SLOPE);
        leaky_relu(self.norm2.forward(self.conv2.conv.forward(h)), SLOPE)
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
        let up = self.transp_conv.conv.forward(x);
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
        self.output_block.conv.conv.forward(h)
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
