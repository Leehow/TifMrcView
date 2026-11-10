//! SAM 3.1 interactive image segmentation, implemented in Rust/Burn.
//!
//! Architecture and checkpoint layout: facebookresearch/sam3 (Meta Platforms,
//! Inc. and affiliates). SAM 3 material is governed by SAM3-LICENSE, shipped
//! alongside this crate. The existing SAM 2 prompt/mask operations are reused.
//! This module does not implement the text detector or multiplex video tracker.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use burn::module::{Module, Param};
use burn::nn::conv::{Conv2d, Conv2dConfig, ConvTranspose2d, ConvTranspose2dConfig};
use burn::nn::{Initializer, LayerNorm, LayerNormConfig, Linear, LinearConfig};
use burn::tensor::activation::gelu;
use burn::tensor::module::attention;
use burn::tensor::ops::AttentionModuleOptions;
use burn::tensor::{Device, Tensor, TensorData};
use burn_store::{ModuleSnapshot, PytorchStore};
use sha2::{Digest, Sha256};

use crate::model::{
    Features, MaskDecoder, PromptEncoder, conv, window_partition, window_unpartition,
};
use crate::{Encoded, Point, Prediction};

pub const IMAGE_SIZE: usize = 1008;
pub const GRID_SIZE: usize = 72;
pub const MASK_SIZE: usize = 288;
const WIDTH: usize = 1024;
const HEADS: usize = 16;

/// pytorch-reader 0.22 rejects complex storages even for unused buffers.
/// The published checkpoint contains one memoized ComplexFloatStorage type,
/// used for redundant RoPE tables only. On this exact verified release,
/// represent that unused metadata as an equal-width opaque f64 storage.
/// None of those buffers exist in Sam31 or are ever read as numeric tensors;
/// all learned weights remain unchanged. The temporary ZIP is removed by RAII
/// on success and errors. This workaround never modifies the original file.
fn checkpoint_view(path: &Path) -> Result<Option<tempfile::NamedTempFile>> {
    let mut archive = zip::ZipArchive::new(File::open(path)?)?;
    let names = archive
        .file_names()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let pickles: Vec<_> = names
        .into_iter()
        .filter(|n| n.ends_with("/data.pkl"))
        .map(|n| n.into_owned())
        .collect();
    if pickles.len() != 1 {
        bail!("expected one PyTorch data.pkl");
    }
    let mut pickle = vec![];
    let entry = archive.by_name(&pickles[0])?;
    if entry.size() > 4 * 1024 * 1024 {
        bail!("SAM 3.1 pickle metadata is unexpectedly large");
    }
    entry.take(4 * 1024 * 1024 + 1).read_to_end(&mut pickle)?;
    const COMPLEX: &[u8] = b"ctorch\nComplexFloatStorage\n";
    let occurrences: Vec<_> = pickle
        .windows(COMPLEX.len())
        .enumerate()
        .filter_map(|(i, s)| (s == COMPLEX).then_some(i))
        .collect();
    if occurrences.is_empty() {
        return Ok(None);
    }
    if occurrences.len() != 1 || !pickle.starts_with(&[0x80, 2]) {
        bail!("unsupported complex checkpoint metadata");
    }
    let mut hash = Sha256::new();
    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    let digest = format!("{:x}", hash.finalize());
    if digest != "0567debeec80ba4ac6369540c6c248025283cb3ff2b92827509e57e2b3541cb6" {
        bail!(
            "complex-buffer compatibility is limited to the verified official SAM 3.1 checkpoint; got SHA256 {digest}"
        );
    }
    let i = occurrences[0];
    pickle.splice(
        i..i + COMPLEX.len(),
        b"ctorch\nDoubleStorage\n".iter().copied(),
    );
    let mut view = tempfile::Builder::new()
        .prefix("tmv-sam31-")
        .suffix(".pt")
        .tempfile_in(path.parent().unwrap_or(Path::new(".")))?;
    {
        let mut writer = zip::ZipWriter::new(view.as_file_mut());
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            if entry.name()?.as_ref() == pickles[0] {
                writer.start_file(
                    &pickles[0],
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )?;
                writer.write_all(&pickle)?;
            } else {
                writer.raw_copy_file(entry)?;
            }
        }
        writer.finish()?;
    }
    view.flush()?;
    Ok(Some(view))
}

fn linear(i: usize, o: usize, dev: &Device) -> Linear {
    LinearConfig::new(i, o)
        .with_initializer(Initializer::Zeros)
        .init(dev)
}

#[derive(Module, Debug)]
struct PatchEmbed {
    proj: Conv2d,
}

#[derive(Module, Debug)]
struct Attention {
    qkv: Linear,
    proj: Linear,
}

/// Real-valued form of complex axial RoPE. It is computed, never loaded
/// from the checkpoint's redundant complex-valued buffers.
struct Rope {
    cos: Tensor<4>,
    sin: Tensor<4>,
}

impl Rope {
    fn new(side: usize, dev: &Device) -> Self {
        let dim = WIDTH / HEADS;
        let scale = 24.0 / side as f32;
        let (mut cos, mut sin) = (vec![], vec![]);
        for y in 0..side {
            for x in 0..side {
                for coordinate in [x, y] {
                    for k in 0..dim / 4 {
                        let phase =
                            coordinate as f32 * scale / 10000f32.powf((4 * k) as f32 / dim as f32);
                        cos.push(phase.cos());
                        sin.push(phase.sin());
                    }
                }
            }
        }
        let shape = [1, side * side, dim / 2, 1];
        Self {
            cos: Tensor::from_data(TensorData::new(cos, shape), dev),
            sin: Tensor::from_data(TensorData::new(sin, shape), dev),
        }
    }

    fn rotate(&self, x: Tensor<4>) -> Tensor<4> {
        let [b, heads, n, d] = x.dims();
        let x = x.reshape([b * heads, n, d / 2, 2]);
        let re = x.clone().narrow(3, 0, 1);
        let im = x.narrow(3, 1, 1);
        Tensor::cat(
            vec![
                re.clone() * self.cos.clone() - im.clone() * self.sin.clone(),
                re * self.sin.clone() + im * self.cos.clone(),
            ],
            3,
        )
        .reshape([b, heads, n, d])
    }
}

impl Attention {
    fn forward(&self, x: Tensor<4>, rope: &Rope) -> Tensor<4> {
        let [b, h, w, c] = x.dims();
        let qkv = self.qkv.forward(x).reshape([b, h * w, 3, HEADS, c / HEADS]);
        let part = |k| {
            qkv.clone()
                .narrow(2, k, 1)
                .reshape([b, h * w, HEADS, c / HEADS])
                .swap_dims(1, 2)
        };
        let out = attention(
            rope.rotate(part(0)),
            rope.rotate(part(1)),
            part(2),
            None,
            None,
            AttentionModuleOptions::default(),
        );
        self.proj.forward(out.swap_dims(1, 2).reshape([b, h, w, c]))
    }
}

#[derive(Module, Debug)]
struct Mlp {
    fc1: Linear,
    fc2: Linear,
}

#[derive(Module, Debug)]
struct Block {
    norm1: LayerNorm,
    attn: Attention,
    norm2: LayerNorm,
    mlp: Mlp,
    #[module(skip)]
    global: bool,
}

impl Block {
    fn new(global: bool, dev: &Device) -> Self {
        let norm = || LayerNormConfig::new(WIDTH).with_epsilon(1e-5).init(dev);
        Self {
            norm1: norm(),
            attn: Attention {
                qkv: linear(WIDTH, 3 * WIDTH, dev),
                proj: linear(WIDTH, WIDTH, dev),
            },
            norm2: norm(),
            mlp: Mlp {
                fc1: linear(WIDTH, 4736, dev),
                fc2: linear(4736, WIDTH, dev),
            },
            global,
        }
    }

    fn forward(&self, x: Tensor<4>, local: &Rope, global: &Rope) -> Tensor<4> {
        let y = self.norm1.forward(x.clone());
        let y = if self.global {
            self.attn.forward(y, global)
        } else {
            let [_, h, w, _] = y.dims();
            let (windows, padded) = window_partition(y, 24);
            window_unpartition(self.attn.forward(windows, local), 24, padded, [h, w])
        };
        let x = x + y;
        let y = self
            .mlp
            .fc2
            .forward(gelu(self.mlp.fc1.forward(self.norm2.forward(x.clone()))));
        x + y
    }
}

#[derive(Module, Debug)]
struct VisionTransformer {
    patch_embed: PatchEmbed,
    pos_embed: Param<Tensor<3>>,
    ln_pre: LayerNorm,
    blocks: Vec<Block>,
}

impl VisionTransformer {
    fn new(dev: &Device) -> Self {
        Self {
            patch_embed: PatchEmbed {
                proj: Conv2dConfig::new([3, WIDTH], [14, 14])
                    .with_stride([14, 14])
                    .with_bias(false)
                    .with_initializer(Initializer::Zeros)
                    .init(dev),
            },
            pos_embed: Param::from_tensor(Tensor::zeros([1, 577, WIDTH], dev)),
            ln_pre: LayerNormConfig::new(WIDTH).with_epsilon(1e-5).init(dev),
            blocks: (0..32)
                .map(|i| Block::new([7, 15, 23, 31].contains(&i), dev))
                .collect(),
        }
    }

    fn forward(&self, x: Tensor<4>, local: &Rope, global: &Rope) -> Tensor<4> {
        let pos = self
            .pos_embed
            .val()
            .narrow(1, 1, 576)
            .reshape([1, 24, 24, WIDTH])
            .repeat_dim(1, 3)
            .repeat_dim(2, 3);
        let mut x = self
            .ln_pre
            .forward(self.patch_embed.proj.forward(x).permute([0, 2, 3, 1]) + pos);
        for (i, block) in self.blocks.iter().enumerate() {
            x = block.forward(x, local, global);
            if std::env::var_os("TMV_SAM_TRACE").is_some() {
                eprintln!(
                    "SAM 3.1 encoder block {i}: {}",
                    x.clone().mean().into_scalar::<f32>()
                );
            }
        }
        x.permute([0, 3, 1, 2])
    }
}

#[derive(Module, Debug)]
struct NeckLevel {
    dconv_2x2_0: Option<ConvTranspose2d>,
    dconv_2x2_1: Option<ConvTranspose2d>,
    dconv_2x2: Option<ConvTranspose2d>,
    conv_1x1: Conv2d,
    conv_3x3: Conv2d,
}

impl NeckLevel {
    fn new(level: usize, dev: &Device) -> Self {
        let up = |i, o| {
            ConvTranspose2dConfig::new([i, o], [2, 2])
                .with_stride([2, 2])
                .with_initializer(Initializer::Zeros)
                .init(dev)
        };
        Self {
            dconv_2x2_0: (level == 0).then(|| up(1024, 512)),
            dconv_2x2_1: (level == 0).then(|| up(512, 256)),
            dconv_2x2: (level == 1).then(|| up(1024, 512)),
            conv_1x1: conv([256, 512, 1024][level], 256, 1, 1, 0, dev),
            conv_3x3: conv(256, 256, 3, 1, 1, dev),
        }
    }

    fn forward(&self, mut x: Tensor<4>) -> Tensor<4> {
        if let (Some(a), Some(b)) = (&self.dconv_2x2_0, &self.dconv_2x2_1) {
            x = b.forward(gelu(a.forward(x)));
        } else if let Some(up) = &self.dconv_2x2 {
            x = up.forward(x);
        }
        self.conv_3x3.forward(self.conv_1x1.forward(x))
    }
}

#[derive(Module, Debug)]
struct ImageEncoder {
    trunk: VisionTransformer,
    interactive_convs: Vec<NeckLevel>,
}

#[derive(Module, Debug)]
struct Sam31 {
    image_encoder: ImageEncoder,
    sam_prompt_encoder: PromptEncoder,
    sam_mask_decoder: MaskDecoder,
    no_mem_embed: Param<Tensor<3>>,
}

/// Native SAM 3.1 point/box predictor. Image embeddings can be retained
/// across refinements. All model parameters on this path must load.
pub struct Predictor {
    model: Sam31,
    local_rope: Rope,
    global_rope: Rope,
    dense_pe: Tensor<4>,
    pub device: Device,
}

impl Predictor {
    pub fn load(path: &Path, device: &Device) -> Result<Self> {
        let view = checkpoint_view(path).context("preparing native SAM 3.1 checkpoint reader")?;
        let path = view.as_ref().map(|v| v.path()).unwrap_or(path);
        let mut model = Sam31 {
            image_encoder: ImageEncoder {
                trunk: VisionTransformer::new(device),
                interactive_convs: (0..3).map(|i| NeckLevel::new(i, device)).collect(),
            },
            sam_prompt_encoder: PromptEncoder::new(device),
            sam_mask_decoder: MaskDecoder::new(device),
            no_mem_embed: Param::from_tensor(Tensor::zeros([1, 1, 256], device)),
        };
        let mut store = PytorchStore::from_file(path)
            .allow_partial(true)
            .with_key_remapping(
                r"^detector\.backbone\.vision_backbone\.trunk\.",
                "image_encoder.trunk.",
            )
            .with_key_remapping(
                r"^detector\.backbone\.vision_backbone\.interactive_convs\.",
                "image_encoder.interactive_convs.",
            )
            .with_key_remapping(
                r"^tracker\.model\.interactive_sam_prompt_encoder\.",
                "sam_prompt_encoder.",
            )
            .with_key_remapping(
                r"^tracker\.model\.interactive_sam_mask_decoder\.",
                "sam_mask_decoder.",
            )
            .with_key_remapping(
                r"^tracker\.model\.interactivity_no_mem_embed$",
                "no_mem_embed",
            )
            .with_key_remapping(
                r"(sam_mask_decoder\.transformer\.layers\.\d+\.mlp)\.lin1\.",
                "$1.layers.0.",
            )
            .with_key_remapping(
                r"(sam_mask_decoder\.transformer\.layers\.\d+\.mlp)\.lin2\.",
                "$1.layers.1.",
            )
            .with_key_remapping(r"mask_downscaling\.0\.", "mask_downscaling.c1.")
            .with_key_remapping(r"mask_downscaling\.1\.", "mask_downscaling.n1.")
            .with_key_remapping(r"mask_downscaling\.3\.", "mask_downscaling.c2.")
            .with_key_remapping(r"mask_downscaling\.4\.", "mask_downscaling.n2.")
            .with_key_remapping(r"mask_downscaling\.6\.", "mask_downscaling.c3.")
            .with_key_remapping(r"output_upscaling\.0\.", "output_upscaling.dc1.")
            .with_key_remapping(r"output_upscaling\.1\.", "output_upscaling.ln1.")
            .with_key_remapping(r"output_upscaling\.3\.", "output_upscaling.dc2.");
        let result = model
            .load_from(&mut store)
            .context("loading SAM 3.1 interactive image weights")?;
        if !result.missing.is_empty() || !result.errors.is_empty() {
            bail!("incomplete SAM 3.1 image checkpoint: {result}");
        }
        let dense_pe = model.sam_prompt_encoder.dense_pe_size(GRID_SIZE, device);
        Ok(Self {
            model,
            local_rope: Rope::new(24, device),
            global_rope: Rope::new(72, device),
            dense_pe,
            device: device.clone(),
        })
    }

    /// Image already resized/normalized to [1,3,1008,1008], in [-1,1].
    pub fn encode_tensor(&self, image: Tensor<4>, width: usize, height: usize) -> Result<Encoded> {
        if image.dims() != [1, 3, IMAGE_SIZE, IMAGE_SIZE] || width == 0 || height == 0 {
            bail!("SAM 3.1 expects [1,3,1008,1008] and nonzero original dimensions");
        }
        let encoder = &self.model.image_encoder;
        let x = encoder
            .trunk
            .forward(image, &self.local_rope, &self.global_rope);
        let levels = &encoder.interactive_convs;
        let s0 = self
            .model
            .sam_mask_decoder
            .conv_s0
            .forward(levels[0].forward(x.clone()));
        let s1 = self
            .model
            .sam_mask_decoder
            .conv_s1
            .forward(levels[1].forward(x.clone()));
        let raw = levels[2].forward(x);
        let embed = raw.clone() + self.model.no_mem_embed.val().reshape([1, 256, 1, 1]);
        Ok(Encoded {
            features: Features { raw, embed, s0, s1 },
            width,
            height,
        })
    }

    /// RGB bytes in HWC order. Native preprocessing follows torchvision's
    /// GPU path: float antialias resize, then one round-to-even byte cast.
    pub fn encode_rgb(&self, pixels: &[u8], width: usize, height: usize) -> Result<Encoded> {
        let values = preprocess_rgb(pixels, width, height)?;
        self.encode_tensor(
            Tensor::from_data(
                TensorData::new(values, [1, 3, IMAGE_SIZE, IMAGE_SIZE]),
                &self.device,
            ),
            width,
            height,
        )
    }

    pub fn encode_gray(&self, pixels: &[u8], width: usize, height: usize) -> Result<Encoded> {
        if width.checked_mul(height) != Some(pixels.len()) || width == 0 || height == 0 {
            bail!("invalid SAM 3.1 grayscale image dimensions");
        }
        let rgb: Vec<u8> = pixels.iter().flat_map(|&v| [v, v, v]).collect();
        self.encode_rgb(&rgb, width, height)
    }

    pub fn predict(
        &self,
        image: &Encoded,
        points: &[Point],
        bbox: Option<[f32; 4]>,
        mask_input: Option<Tensor<4>>,
    ) -> Result<Prediction> {
        if points.is_empty() && bbox.is_none() && mask_input.is_none() {
            bail!("provide a point, box or mask prompt");
        }
        if image.width == 0
            || image.height == 0
            || image.features.embed.dims() != [1, 256, GRID_SIZE, GRID_SIZE]
        {
            bail!("not a SAM 3.1 image embedding");
        }
        if mask_input
            .as_ref()
            .is_some_and(|m| m.dims() != [1, 1, MASK_SIZE, MASK_SIZE])
        {
            bail!("SAM 3.1 mask prompts must be 288x288");
        }
        let sx = IMAGE_SIZE as f32 / image.width as f32;
        let sy = IMAGE_SIZE as f32 / image.height as f32;
        let mut prompts = vec![];
        if let Some(b) = bbox {
            if !b.iter().all(|v| v.is_finite()) || b[0] >= b[2] || b[1] >= b[3] {
                bail!("invalid bounding box");
            }
            prompts.extend([([b[0] * sx, b[1] * sy], 2), ([b[2] * sx, b[3] * sy], 3)]);
        }
        for p in points {
            if !p.x.is_finite() || !p.y.is_finite() {
                bail!("non-finite point coordinates");
            }
            prompts.push(([p.x * sx, p.y * sy], p.positive as i32));
        }
        let pe = &self.model.sam_prompt_encoder;
        let f = &image.features;
        let masks = self.model.sam_mask_decoder.forward(
            f.embed.clone(),
            self.dense_pe.clone(),
            pe.sparse_size(&prompts, IMAGE_SIZE, &self.device),
            pe.dense_size(mask_input, GRID_SIZE, &self.device),
            f.s0.clone(),
            f.s1.clone(),
        );
        let scores: Vec<f32> = masks
            .iou
            .into_data()
            .convert::<f32>()
            .try_into_vec()
            .unwrap();
        let best = (1..4)
            .max_by(|&a, &b| scores[a].total_cmp(&scores[b]))
            .unwrap();
        let pick = if bbox.is_none() && points.len() == 1 {
            best
        } else {
            let single: Vec<f32> = masks
                .logits
                .clone()
                .narrow(1, 0, 1)
                .into_data()
                .convert::<f32>()
                .try_into_vec()
                .unwrap();
            let inner = single.iter().filter(|&&v| v > 0.05).count();
            let outer = single.iter().filter(|&&v| v > -0.05).count();
            if outer == 0 || inner as f32 / outer as f32 >= 0.98 {
                0
            } else {
                best
            }
        };
        let chosen = masks.logits.narrow(1, pick, 1);
        let low_values: Vec<f32> = chosen
            .clone()
            .into_data()
            .convert::<f32>()
            .try_into_vec()
            .unwrap();
        // Burn's backend resize uses different sampling coordinates. SAM's
        // output requires PyTorch's align_corners=false half-pixel rule.
        let logits = resize_logits(&low_values, MASK_SIZE, MASK_SIZE, image.width, image.height);
        let low_res = chosen.clamp(-32.0, 32.0);
        Ok(Prediction {
            logits,
            iou: scores[pick],
            low_res,
        })
    }
}

fn resize_logits(src: &[f32], iw: usize, ih: usize, ow: usize, oh: usize) -> Vec<f32> {
    let taps = |o: usize, input: usize, output: usize| {
        let coordinate = ((o as f32 + 0.5) * (input as f32 / output as f32) - 0.5).max(0.0);
        let first = (coordinate.floor() as usize).min(input - 1);
        (first, (first + 1).min(input - 1), coordinate - first as f32)
    };
    let xs: Vec<_> = (0..ow).map(|x| taps(x, iw, ow)).collect();
    let mut out = vec![0.0; ow * oh];
    for y in 0..oh {
        let (y0, y1, ty) = taps(y, ih, oh);
        for (x, &(x0, x1, tx)) in xs.iter().enumerate() {
            let top = src[y0 * iw + x0] * (1.0 - tx) + src[y0 * iw + x1] * tx;
            let bottom = src[y1 * iw + x0] * (1.0 - tx) + src[y1 * iw + x1] * tx;
            out[y * ow + x] = top * (1.0 - ty) + bottom * ty;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logits_resize_matches_pytorch_half_pixel_reference() {
        // torch.nn.functional.interpolate([[0,1],[2,3]], size=(4,4),
        // mode="bilinear", align_corners=False), unlike align_corners=True.
        assert_eq!(
            resize_logits(&[0., 1., 2., 3.], 2, 2, 4, 4),
            vec![
                0., 0.25, 0.75, 1., 0.5, 0.75, 1.25, 1.5, 1.5, 1.75, 2.25, 2.5, 2., 2.25, 2.75, 3.
            ]
        );
        assert_eq!(resize_logits(&[7.], 1, 1, 3, 2), vec![7.; 6]);
    }

    #[test]
    fn byte_resize_matches_torchvision_reference_samples() {
        // torchvision.transforms.v2.Resize((1008,1008)), uint8 RGB on MPS.
        // CPU has a different native uint8 kernel with intermediate byte
        // rounding; SAM3's processor moves the input to GPU before resizing.
        for (w, h, expected) in [
            (7, 5, [0u8, 53, 106, 113, 166, 76, 124, 208, 154]),
            (1011, 1009, [0u8, 101, 202, 121, 222, 140, 16, 124, 199]),
        ] {
            let pixels: Vec<_> = (0..h)
                .flat_map(|y| {
                    (0..w).flat_map(move |x| {
                        (0..3).map(move |c| ((x * 17 + y * 31 + c * 53) % 256) as u8)
                    })
                })
                .collect();
            let resized = preprocess_rgb(&pixels, w, h).unwrap();
            let coordinates = [
                (0, 0, 0),
                (1, 1, 1),
                (2, 2, 2),
                (0, 503, 503),
                (1, 504, 504),
                (2, 1007, 1007),
                (0, 1007, 0),
                (2, 0, 1007),
                (1, 213, 777),
            ];
            for ((c, y, x), value) in coordinates.into_iter().zip(expected) {
                let actual = resized[(c * IMAGE_SIZE + y) * IMAGE_SIZE + x];
                assert!(
                    (actual - (value as f32 / 127.5 - 1.0)).abs() < 1e-6,
                    "{w}x{h} at ({c},{y},{x}): {actual}, byte {value}"
                );
            }
        }
    }
}

/// Separable, half-pixel bilinear resize with the triangular antialias
/// kernel widened on downsampling, followed by uint8 rounding.
fn preprocess_rgb(pixels: &[u8], width: usize, height: usize) -> Result<Vec<f32>> {
    if width == 0
        || height == 0
        || width.checked_mul(height).and_then(|n| n.checked_mul(3)) != Some(pixels.len())
    {
        bail!("invalid SAM 3.1 RGB image dimensions");
    }
    fn taps(input: usize) -> Vec<Vec<(usize, f32)>> {
        let scale = input as f32 / IMAGE_SIZE as f32;
        let support = scale.max(1.0);
        (0..IMAGE_SIZE)
            .map(|o| {
                let center = (o as f32 + 0.5) * scale;
                let start = ((center - support + 0.5) as isize).max(0) as usize;
                let end = ((center + support + 0.5) as usize).min(input);
                let mut weights: Vec<_> = (start..end)
                    .map(|i| {
                        (
                            i,
                            (1.0 - ((i as f32 + 0.5 - center) / support).abs()).max(0.0),
                        )
                    })
                    .collect();
                let total: f32 = weights.iter().map(|v| v.1).sum();
                for v in &mut weights {
                    v.1 /= total;
                }
                weights
            })
            .collect()
    }
    let (xs, ys) = (taps(width), taps(height));
    let mut horizontal = vec![0f32; height * IMAGE_SIZE * 3];
    for y in 0..height {
        for (x, taps) in xs.iter().enumerate() {
            for c in 0..3 {
                horizontal[(y * IMAGE_SIZE + x) * 3 + c] = taps
                    .iter()
                    .map(|&(i, w)| pixels[(y * width + i) * 3 + c] as f32 * w)
                    .sum();
            }
        }
    }
    let mut out = vec![0f32; 3 * IMAGE_SIZE * IMAGE_SIZE];
    for (y, taps) in ys.iter().enumerate() {
        for x in 0..IMAGE_SIZE {
            for c in 0..3 {
                let value: f32 = taps
                    .iter()
                    .map(|&(i, w)| horizontal[(i * IMAGE_SIZE + x) * 3 + c] * w)
                    .sum();
                out[(c * IMAGE_SIZE + y) * IMAGE_SIZE + x] =
                    value.round_ties_even().clamp(0.0, 255.0) / 127.5 - 1.0;
            }
        }
    }
    Ok(out)
}
