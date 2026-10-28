//! SAM 2.1 (Segment Anything 2.1, Apache-2.0) for TifMrcView Pro, in Rust
//! with Burn. Loads the official PyTorch checkpoints and segments an object
//! from clicks or a box on one image.

pub mod model;

use std::path::Path;

use anyhow::{anyhow, Result};
use burn::tensor::module::interpolate;
use burn::tensor::ops::{InterpolateMode, InterpolateOptions};
use burn::tensor::{Device, Tensor, TensorData};
use burn_store::{ModuleSnapshot, PytorchStore};

pub use model::{Features, Sam2, Sam2Config, IMAGE_SIZE};

/// The fastest backend that works here: CUDA, then the platform GPU API
/// (Metal, DX12 or Vulkan through wgpu), then the CPU. Each candidate runs a
/// small computation first, so a missing driver falls through instead of
/// failing later. Returns the device and a name to show.
pub fn pick_device() -> (Device, &'static str) {
    #[allow(unused)]
    fn works(make: impl FnOnce() -> Device + std::panic::UnwindSafe) -> Option<Device> {
        std::panic::catch_unwind(|| {
            let d = make();
            let x = Tensor::<2>::ones([8, 8], &d);
            let s: f32 = x.clone().matmul(x).sum().into_scalar();
            (s == 512.0).then_some(d)
        })
        .ok()
        .flatten()
    }
    #[cfg(feature = "cuda")]
    if let Some(d) = works(|| Device::cuda(0)) {
        return (d, "CUDA");
    }
    #[cfg(feature = "wgpu")]
    if let Some(d) = works(|| Device::wgpu(burn::tensor::DeviceKind::DefaultDevice)) {
        return (d, if cfg!(target_os = "macos") { "Metal" } else if cfg!(windows) { "GPU (DX12/Vulkan)" } else { "GPU (Vulkan)" });
    }
    #[cfg(feature = "flex")]
    return (Device::flex(), "CPU");
    #[allow(unreachable_code)]
    (Device::default(), "CPU")
}

/// A loaded model plus the tensors that only depend on its weights.
pub struct Predictor {
    pub model: Sam2,
    pos: Tensor<4>,
    dense_pe: Tensor<4>,
    pub device: Device,
}

/// Features of one image, with its original size.
#[derive(Clone)]
pub struct Encoded {
    pub features: Features,
    pub width: usize,
    pub height: usize,
}

/// One predicted mask at the image's own size.
pub struct Prediction {
    /// Mask logits, row-major `height * width`; > 0 is inside.
    pub logits: Vec<f32>,
    pub iou: f32,
    /// Low-resolution logits [1, 1, 256, 256] to refine with more clicks.
    pub low_res: Tensor<4>,
}

/// A prompt point in image pixels; `positive` false marks background.
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub positive: bool,
}

impl Predictor {
    /// Loads a facebookresearch/sam2 checkpoint (`.pt`, state dict under "model").
    pub fn load(path: &Path, cfg: &Sam2Config, device: &Device) -> Result<Self> {
        let mut model = Sam2::new(cfg, device);
        let mut store = PytorchStore::from_file(path)
            .with_top_level_key("model")
            .allow_partial(true)
            .with_key_remapping(r"^image_encoder\.neck\.convs\.(\d+)\.conv\.", "image_encoder.neck.convs.$1.")
            .with_key_remapping(r"mask_downscaling\.0\.", "mask_downscaling.c1.")
            .with_key_remapping(r"mask_downscaling\.1\.", "mask_downscaling.n1.")
            .with_key_remapping(r"mask_downscaling\.3\.", "mask_downscaling.c2.")
            .with_key_remapping(r"mask_downscaling\.4\.", "mask_downscaling.n2.")
            .with_key_remapping(r"mask_downscaling\.6\.", "mask_downscaling.c3.")
            .with_key_remapping(r"output_upscaling\.0\.", "output_upscaling.dc1.")
            .with_key_remapping(r"output_upscaling\.1\.", "output_upscaling.ln1.")
            .with_key_remapping(r"output_upscaling\.3\.", "output_upscaling.dc2.");
        let res = model.load_from(&mut store).map_err(|e| anyhow!("loading {}: {e}", path.display()))?;
        if !res.missing.is_empty() {
            return Err(anyhow!("checkpoint is missing {} tensors, e.g. {:?}", res.missing.len(), &res.missing[..res.missing.len().min(5)]));
        }
        let pos = model.hiera_pos_embed(device);
        let dense_pe = model.sam_prompt_encoder.dense_pe(device);
        Ok(Predictor { model, pos, dense_pe, device: device.clone() })
    }

    /// Encodes an already normalized [1, 3, 1024, 1024] image.
    pub fn encode_tensor(&self, image: Tensor<4>, width: usize, height: usize) -> Encoded {
        Encoded { features: self.model.encode(image, self.pos.clone()), width, height }
    }

    /// Encodes an 8-bit grayscale image (shown contrast), resized to 1024²
    /// and normalized like SAM2Transforms.
    pub fn encode_gray(&self, pixels: &[u8], width: usize, height: usize) -> Encoded {
        let x = Tensor::<4>::from_data(TensorData::new(pixels.iter().map(|&v| v as f32 / 255.0).collect::<Vec<f32>>(), [1, 1, height, width]), &self.device);
        let x = interpolate(x, InterpolateOptions::new(InterpolateMode::Bilinear).with_output_size([IMAGE_SIZE, IMAGE_SIZE]));
        let mean = [0.485f32, 0.456, 0.406];
        let std = [0.229f32, 0.224, 0.225];
        let chans: Vec<Tensor<4>> = (0..3).map(|c| (x.clone() - mean[c]) / std[c]).collect();
        self.encode_tensor(Tensor::cat(chans, 1), width, height)
    }

    /// Mask for clicks and/or a box (x0, y0, x1, y1), in image pixels.
    /// One prompt point: the best of the three multimask outputs; more:
    /// the single-mask output, falling back to the best multimask one when
    /// it is unstable (SAM 2's dynamic multimask).
    pub fn predict(&self, enc: &Encoded, points: &[Point], bbox: Option<[f32; 4]>, mask_input: Option<Tensor<4>>) -> Prediction {
        let (sx, sy) = (IMAGE_SIZE as f32 / enc.width as f32, IMAGE_SIZE as f32 / enc.height as f32);
        let mut prompts: Vec<([f32; 2], i32)> = vec![];
        if let Some(b) = bbox {
            prompts.push(([b[0] * sx, b[1] * sy], 2));
            prompts.push(([b[2] * sx, b[3] * sy], 3));
        }
        prompts.extend(points.iter().map(|p| ([p.x * sx, p.y * sy], p.positive as i32)));
        let pe = &self.model.sam_prompt_encoder;
        let sparse = pe.sparse(&prompts, &self.device);
        let dense = pe.dense(mask_input, &self.device);
        let f = &enc.features;
        let m = self.model.sam_mask_decoder.forward(f.embed.clone(), self.dense_pe.clone(), sparse, dense, f.s0.clone(), f.s1.clone());
        let iou: Vec<f32> = m.iou.clone().into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
        let best_multi = (1..4).max_by(|&a, &b| iou[a].total_cmp(&iou[b])).unwrap();
        let pick = if prompts.len() <= 1 {
            best_multi
        } else {
            // Stability of the single-mask output: area above +0.05 over area above -0.05.
            let l0: Vec<f32> = m.logits.clone().narrow(1, 0, 1).into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
            let inner = l0.iter().filter(|&&v| v > 0.05).count();
            let outer = l0.iter().filter(|&&v| v > -0.05).count();
            let stable = outer == 0 || inner as f32 / outer as f32 >= 0.98;
            if stable { 0 } else { best_multi }
        };
        let chosen = m.logits.narrow(1, pick, 1);
        let full = interpolate(chosen.clone(), InterpolateOptions::new(InterpolateMode::Bilinear).with_output_size([enc.height, enc.width]));
        Prediction { logits: full.into_data().convert::<f32>().try_into_vec::<f32>().unwrap(), iou: iou[pick], low_res: chosen.clamp(-32.0, 32.0) }
    }
}
