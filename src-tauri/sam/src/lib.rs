//! SAM 2.1 (Segment Anything 2.1, Apache-2.0) for TifMrcView Pro, in Rust
//! with Burn. Loads the official PyTorch checkpoints and segments an object
//! from clicks or a box on one image, and tracks it through a stack with
//! SAM 2's memory (`track_start` / `track_next`).

pub mod memory;
pub mod model;

use std::path::Path;

use anyhow::{anyhow, Result};
use burn::tensor::module::interpolate;
use burn::tensor::ops::{InterpolateMode, InterpolateOptions};
use burn::tensor::{Device, Tensor, TensorData};
use burn_store::{ModuleSnapshot, PytorchStore};

pub use model::{Features, Sam2, Sam2Config, IMAGE_SIZE};

/// One way to run Burn on this machine, with a name to show.
pub struct Backend {
    pub name: &'static str,
    make: fn() -> Result<Device>,
}

impl Backend {
    pub fn device(&self) -> Result<Device> {
        guarded(self.make).and_then(|r| r)
    }
}

/// The backends to try, fastest first: Vulkan (SPIR-V kernels), then CUDA
/// for NVIDIA machines without a GPU Vulkan driver (WSL, compute servers),
/// then DX12 on Windows; Metal on a Mac; the CPU last. On an RTX 5080
/// (`examples/gpu_bench.rs`, Windows, driver 617) Vulkan was fastest in 7
/// of 10 Pro tasks, CUDA up to 20x slower (SART) and only ahead in SAM
/// encoding, DX12 in between. `TMV_GPU=cuda|vulkan|dx12|metal|cpu` keeps
/// only that one (and the CPU after it), for benchmarks and as a way out
/// when a driver misbehaves.
pub fn backends() -> Vec<Backend> {
    #[allow(unused_mut)]
    let mut v: Vec<Backend> = vec![];
    // Real GPUs only, by kind: wgpu's default device can be a software
    // renderer (lavapipe on Linux without a GPU Vulkan driver, as in WSL),
    // which is slower than the CPU backend and fails on large buffers.
    #[cfg(feature = "vulkan")]
    if vulkan_gpu() {
        use burn::tensor::DeviceKind;
        v.push(Backend { name: "Vulkan", make: || Ok(Device::vulkan(DeviceKind::DiscreteGpu(0))) });
    }
    // A discrete NVIDIA card without a Vulkan driver beats an integrated GPU.
    #[cfg(feature = "cuda")]
    v.push(Backend { name: "CUDA", make: || Ok(Device::cuda(0)) });
    #[cfg(feature = "vulkan")]
    if vulkan_gpu() {
        use burn::tensor::DeviceKind;
        v.push(Backend { name: "Vulkan", make: || Ok(Device::vulkan(DeviceKind::IntegratedGpu(0))) });
        v.push(Backend { name: "Vulkan", make: || Ok(Device::vulkan(DeviceKind::VirtualGpu(0))) });
    }
    #[cfg(all(feature = "wgpu", windows))]
    {
        v.push(Backend { name: "DX12", make: || dx12(0) });
        v.push(Backend { name: "DX12", make: || dx12(1) });
    }
    #[cfg(all(feature = "metal", target_os = "macos"))]
    {
        use burn::tensor::DeviceKind;
        v.push(Backend { name: "Metal", make: || Ok(Device::wgpu(DeviceKind::DiscreteGpu(0))) });
        v.push(Backend { name: "Metal", make: || Ok(Device::wgpu(DeviceKind::IntegratedGpu(0))) });
        v.push(Backend { name: "Metal", make: || Ok(Device::wgpu(DeviceKind::VirtualGpu(0))) });
    }
    // A build with wgpu but no pinned API for this platform (e.g. Linux
    // without the vulkan feature): leave the API to wgpu.
    #[cfg(all(feature = "wgpu", not(feature = "vulkan"), not(windows), not(all(feature = "metal", target_os = "macos"))))]
    {
        use burn::tensor::DeviceKind;
        v.push(Backend { name: "GPU", make: || Ok(Device::wgpu(DeviceKind::DiscreteGpu(0))) });
        v.push(Backend { name: "GPU", make: || Ok(Device::wgpu(DeviceKind::IntegratedGpu(0))) });
    }
    #[cfg(feature = "flex")]
    v.push(Backend { name: "CPU", make: || Ok(Device::flex()) });
    if !v.iter().any(|b| b.name == "CPU") {
        v.push(Backend { name: "CPU", make: || Ok(Device::default()) });
    }
    if let Ok(want) = std::env::var("TMV_GPU") {
        let want = want.trim().to_ascii_lowercase();
        if !want.is_empty() && want != "auto" {
            v.retain(|b| b.name.eq_ignore_ascii_case(&want) || b.name == "CPU");
        }
    }
    v
}

/// Whether Vulkan reaches a real GPU (not only a software renderer such as
/// llvmpipe in WSL). Asked once, so machines without one skip Vulkan
/// instead of failing (noisily) on each kind of device.
#[cfg(feature = "vulkan")]
fn vulkan_gpu() -> bool {
    use burn::tensor::wgpu::wgpu;
    static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAS.get_or_init(|| {
        // WSL has no GPU Vulkan driver, and listing its software adapters
        // took about a minute on leehowpc: use CUDA there straight away.
        if in_wsl() {
            return false;
        }
        guarded(|| {
            let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
            desc.backends = wgpu::Backends::VULKAN;
            let adapters = block_on(wgpu::Instance::new(desc).enumerate_adapters(wgpu::Backends::VULKAN));
            adapters.iter().any(|a| matches!(a.get_info().device_type, wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::VirtualGpu))
        })
        .unwrap_or(false)
    })
}

#[cfg(feature = "vulkan")]
fn in_wsl() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var_os("WSL_DISTRO_NAME").is_some()
            || std::fs::read_to_string("/proc/sys/kernel/osrelease").is_ok_and(|r| r.to_ascii_lowercase().contains("microsoft")))
}

/// Runs a future to completion on this thread (wgpu's native futures are
/// ready at once or soon).
#[cfg(feature = "vulkan")]
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

/// A DX12 device on the discrete (0) or integrated (1) GPU. wgpu's own
/// choice of API on Windows is Vulkan first, so DX12 has to be asked for;
/// a device can be set up only once, so it is kept.
#[cfg(all(feature = "wgpu", windows))]
fn dx12(which: usize) -> Result<Device> {
    use burn::tensor::wgpu::WgpuBackend;
    use burn::tensor::DeviceKind;
    use std::sync::{Mutex, OnceLock};
    static DEVS: OnceLock<Mutex<[Option<Option<Device>>; 2]>> = OnceLock::new();
    let mut devs = DEVS.get_or_init(|| Mutex::new([None, None])).lock().unwrap_or_else(|e| e.into_inner());
    let slot = &mut devs[which];
    if slot.is_none() {
        let kind = if which == 0 { DeviceKind::DiscreteGpu(0) } else { DeviceKind::IntegratedGpu(0) };
        *slot = Some(Device::wgpu_options().device_kind(kind).graphics_api(WgpuBackend::Dx12).init().ok());
    }
    slot.clone().flatten().ok_or_else(|| anyhow!("no DX12 GPU"))
}

/// Runs `f`, turning a panic (how Burn reports a failed GPU kernel or
/// allocation) into an error.
fn guarded<T>(f: impl FnOnce() -> T) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|e| {
        let msg = e.downcast_ref::<String>().map(String::as_str).or_else(|| e.downcast_ref::<&str>().copied()).unwrap_or("panic");
        anyhow!("{}", msg.lines().next().unwrap_or(msg))
    })
}

/// The fastest backend where a small computation works. Returns the
/// device and a name to show.
pub fn pick_device() -> (Device, &'static str) {
    for b in backends() {
        let ok = guarded(|| {
            let d = b.device().ok()?;
            let x = Tensor::<2>::ones([8, 8], &d);
            let s: f32 = x.clone().matmul(x).sum().into_scalar();
            (s == 512.0).then_some(d)
        });
        match ok {
            Ok(Some(d)) => return (d, b.name),
            Ok(None) => eprintln!("GPU: {} backend failed", b.name),
            Err(e) => eprintln!("GPU: {} backend failed: {e:#}", b.name),
        }
    }
    (Device::default(), "CPU")
}

/// Loads the model on the fastest backend that can really run it: each
/// candidate encodes a blank image and decodes one click first, so a GPU
/// with too little memory or too small buffer limits falls through to the
/// next one instead of failing on the user's first click. A GPU candidate
/// that has not answered within `gpu_timeout` (a hung driver) is given up
/// on as well; its thread is left behind.
pub fn load_best(path: &Path, cfg: &Sam2Config, gpu_timeout: std::time::Duration) -> Result<(Predictor, &'static str)> {
    let mut last = anyhow!("no backend");
    let list = backends();
    let n = list.len();
    for (i, b) in list.into_iter().enumerate() {
        let (make, name) = (b.make, b.name);
        let (tx, rx) = std::sync::mpsc::channel();
        let (path, cfg) = (path.to_path_buf(), cfg.clone());
        std::thread::spawn(move || {
            let tried = guarded(|| -> Result<Predictor> {
                let p = Predictor::load(&path, &cfg, &make()?)?;
                let enc = p.encode_gray(&[0u8; 64 * 64], 64, 64);
                p.predict(&enc, &[Point { x: 32.0, y: 32.0, positive: true }], None, None);
                Ok(p)
            });
            let _ = tx.send(tried.and_then(|r| r));
        });
        // The last candidate (the CPU) gets all the time it needs.
        let got = if i + 1 < n { rx.recv_timeout(gpu_timeout).map_err(|_| anyhow!("no answer within {} s", gpu_timeout.as_secs())) } else { rx.recv().map_err(|e| anyhow!("{e}")) };
        match got.and_then(|r| r) {
            Ok(p) => return Ok((p, name)),
            Err(e) => {
                eprintln!("SAM: {name} backend failed: {e:#}");
                last = e.context(format!("{name} backend"));
            }
        }
    }
    Err(last)
}

/// A loaded model plus the tensors that only depend on its weights.
pub struct Predictor {
    pub model: Sam2,
    pos: Tensor<4>,
    dense_pe: Tensor<4>,
    /// Position encodings of the 64×64 grid for tracking: of the image
    /// features [1, 4096, 256] and of a memory [1, 4096, 64], plus the
    /// rotary tables of memory attention.
    grid_pos: Tensor<3>,
    mem_pos: Tensor<3>,
    rope: memory::Rope,
    pub device: Device,
}

/// What tracking keeps of one frame: its memory [1, 4096, 64] and the
/// object pointer [1, 256].
#[derive(Clone)]
pub struct Memory {
    feats: Tensor<3>,
    ptr: Tensor<2>,
}

/// A tracked frame: the mask, the frame's memory for the next ones, and
/// the object score (> 0: the model sees the object on this frame).
pub struct Tracked {
    pub prediction: Prediction,
    pub memory: Memory,
    pub obj_score: f32,
}

/// [1, c, h, w] -> [1, h*w, c]
fn tokens(x: Tensor<4>) -> Tensor<3> {
    let [b, c, h, w] = x.dims();
    x.reshape([b, c, h * w]).swap_dims(1, 2)
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

impl Prediction {
    /// [`Prediction::low_res`] as plain numbers, to keep for a later mask prompt.
    pub fn low_res_values(&self) -> Vec<f32> {
        self.low_res.clone().into_data().convert::<f32>().try_into_vec::<f32>().unwrap()
    }
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
            .with_key_remapping(r"output_upscaling\.3\.", "output_upscaling.dc2.")
            .with_key_remapping(r"mask_downsampler\.encoder\.0\.", "mask_downsampler.convs.0.")
            .with_key_remapping(r"mask_downsampler\.encoder\.3\.", "mask_downsampler.convs.1.")
            .with_key_remapping(r"mask_downsampler\.encoder\.6\.", "mask_downsampler.convs.2.")
            .with_key_remapping(r"mask_downsampler\.encoder\.9\.", "mask_downsampler.convs.3.")
            .with_key_remapping(r"mask_downsampler\.encoder\.1\.", "mask_downsampler.norms.0.")
            .with_key_remapping(r"mask_downsampler\.encoder\.4\.", "mask_downsampler.norms.1.")
            .with_key_remapping(r"mask_downsampler\.encoder\.7\.", "mask_downsampler.norms.2.")
            .with_key_remapping(r"mask_downsampler\.encoder\.10\.", "mask_downsampler.norms.3.")
            .with_key_remapping(r"mask_downsampler\.encoder\.12\.", "mask_downsampler.proj.");
        let res = model.load_from(&mut store).map_err(|e| anyhow!("loading {}: {e}", path.display()))?;
        if !res.missing.is_empty() {
            return Err(anyhow!("checkpoint is missing {} tensors, e.g. {:?}", res.missing.len(), &res.missing[..res.missing.len().min(5)]));
        }
        let pos = model.hiera_pos_embed(device);
        let dense_pe = model.sam_prompt_encoder.dense_pe(device);
        let n = IMAGE_SIZE / 16;
        let grid_pos = tokens(memory::sine_pe(model::D_MODEL, n, n, device));
        let mem_pos = tokens(memory::sine_pe(memory::MEM_DIM, n, n, device));
        let rope = memory::Rope::new(n, device);
        Ok(Predictor { model, pos, dense_pe, grid_pos, mem_pos, rope, device: device.clone() })
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

    /// A mask prompt from [`Prediction::low_res_values`] of an earlier
    /// prediction, for `predict`'s `mask_input`.
    pub fn mask_prompt(&self, low_res: &[f32]) -> Tensor<4> {
        Tensor::<4>::from_data(TensorData::new(low_res.to_vec(), [1, 1, 256, 256]), &self.device)
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

    /// Mask logits at 1024² for the memory encoder: sigmoid, scaled to ±10.
    fn memory_of(&self, raw: Tensor<4>, high_res: Tensor<4>, present: bool) -> Tensor<3> {
        let m = burn::tensor::activation::sigmoid(high_res) * 20.0 - 10.0;
        let mut feats = tokens(self.model.memory_encoder.forward(raw, m));
        if !present {
            feats = feats + self.model.no_obj_embed_spatial.val().unsqueeze_dim::<3>(0);
        }
        feats
    }

    /// Starts tracking from a mask on one frame (image size, row-major),
    /// used as it is (SAM 2's mask input), and returns that frame's memory.
    pub fn track_start(&self, enc: &Encoded, mask: &[bool]) -> Memory {
        let m = Tensor::<4>::from_data(TensorData::new(mask.iter().map(|&v| v as u8 as f32).collect::<Vec<f32>>(), [1, 1, enc.height, enc.width]), &self.device);
        let m = interpolate(m, InterpolateOptions::new(InterpolateMode::Bilinear).with_output_size([IMAGE_SIZE, IMAGE_SIZE]));
        let present = mask.iter().any(|&v| v);
        let high = m.clone() * 20.0 - 10.0;
        // The object pointer comes from the decoder prompted with the mask.
        let pe = &self.model.sam_prompt_encoder;
        let dense = pe.dense(Some(self.model.mask_downsample.forward(m)), &self.device);
        let sparse = pe.sparse(&[([0.0, 0.0], -1)], &self.device);
        let f = &enc.features;
        let out = self.model.sam_mask_decoder.forward(f.embed.clone(), self.dense_pe.clone(), sparse, dense, f.s0.clone(), f.s1.clone());
        let ptr = if present { self.model.obj_ptr_proj.forward(out.tokens.narrow(1, 0, 1).squeeze_dim::<2>(1)) } else { self.model.no_obj_ptr.val() };
        Memory { feats: self.memory_of(f.raw.clone(), high, present), ptr }
    }

    /// The object on the frame `k` steps from the start frame, from the
    /// start frame's memory and those of the frames since (`prev`, nearest
    /// last). `frames` is the length of the stack.
    pub fn track_next(&self, enc: &Encoded, start: &Memory, prev: &[Memory], k: usize, frames: usize) -> Tracked {
        let m = &self.model;
        let tpos = |i: usize| m.maskmem_tpos_enc.val().narrow(0, i, 1).reshape([1, 1, memory::MEM_DIM]);
        // Spatial memories: the start frame, then up to six frames before this one.
        let (mut mem, mut mem_pos) = (vec![start.feats.clone()], vec![self.mem_pos.clone() + tpos(memory::NUM_MASKMEM - 1)]);
        for back in (1..memory::NUM_MASKMEM).rev() {
            if prev.len() >= back {
                mem.push(prev[prev.len() - back].feats.clone());
                mem_pos.push(self.mem_pos.clone() + tpos(back - 1));
            }
        }
        // Object pointers: the start frame's and those of up to 15 frames before.
        let most = frames.min(memory::MAX_OBJ_PTRS);
        let mut ptrs = vec![start.ptr.clone()];
        let mut dist = vec![k as f32];
        for back in 1..most {
            if prev.len() >= back {
                ptrs.push(prev[prev.len() - back].ptr.clone());
                dist.push(back as f32);
            }
        }
        let scale = (most.max(2) - 1) as f32;
        let dist: Vec<f32> = dist.iter().map(|d| d / scale).collect();
        let n = ptrs.len();
        let split = model::D_MODEL / memory::MEM_DIM;
        let ptr_pos = m.obj_ptr_tpos_proj.forward(memory::sine_1d(&dist, model::D_MODEL, &self.device));
        let ptr_pos = ptr_pos.reshape([n, 1, memory::MEM_DIM]).expand([n, split, memory::MEM_DIM]).reshape([1, n * split, memory::MEM_DIM]);
        let ptr_tok = Tensor::cat(ptrs, 0).reshape([1, n * split, memory::MEM_DIM]);
        mem.push(ptr_tok);
        mem_pos.push(ptr_pos);
        let f = &enc.features;
        let [b, c, h, w] = f.raw.dims();
        let x = m.memory_attention.forward(tokens(f.raw.clone()), self.grid_pos.clone(), Tensor::cat(mem, 1), Tensor::cat(mem_pos, 1), &self.rope, n * split);
        let cond = x.swap_dims(1, 2).reshape([b, c, h, w]);
        // Decode without prompts and keep the best of the multimask outputs.
        let pe = &m.sam_prompt_encoder;
        let out = m.sam_mask_decoder.forward(cond, self.dense_pe.clone(), pe.sparse(&[([0.0, 0.0], -1)], &self.device), pe.dense(None, &self.device), f.s0.clone(), f.s1.clone());
        let iou: Vec<f32> = out.iou.clone().into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
        let best = (1..4).max_by(|&a, &b| iou[a].total_cmp(&iou[b])).unwrap();
        let obj_score = out.obj_score.into_data().convert::<f32>().try_into_vec::<f32>().unwrap()[0];
        let present = obj_score > 0.0;
        let mut low = out.logits.narrow(1, best, 1);
        if !present {
            low = low.zeros_like() - 1024.0;
        }
        let ptr = if present { m.obj_ptr_proj.forward(out.tokens.narrow(1, best, 1).squeeze_dim::<2>(1)) } else { m.no_obj_ptr.val() };
        let high = interpolate(low.clone(), InterpolateOptions::new(InterpolateMode::Bilinear).with_output_size([IMAGE_SIZE, IMAGE_SIZE]));
        let memory = Memory { feats: self.memory_of(f.raw.clone(), high, present), ptr };
        let full = interpolate(low.clone(), InterpolateOptions::new(InterpolateMode::Bilinear).with_output_size([enc.height, enc.width]));
        let prediction = Prediction { logits: full.into_data().convert::<f32>().try_into_vec::<f32>().unwrap(), iou: iou[best], low_res: low.clamp(-32.0, 32.0) };
        Tracked { prediction, memory, obj_score }
    }
}
