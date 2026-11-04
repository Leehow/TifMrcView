# Handoff: Pro GPU selection on Apple Silicon (PR #129)

Status 2026-11-04: fix written and pushed, cross-compiled for Linux / macOS /
Windows; real-Mac check was started but not finished when work was paused.

## Problem reported

On the user's M1 Max every Pro process logged

    Unable to acquire wgpu device: AdapterUnavailable { DiscreteGpu(0) … No Discrete GPU device found }
    thread 'tokio-rt-worker' panicked … RecvError
    GPU: Metal backend failed

and a membrane segmentation of a 256×256×128 crop took 66 min
(log line: `membrane: 3 components, 5998 voxels, 8 copies, Metal, 3961.9 s`).
Logs on the Mac: `/Users/haoli/leehow/tifview-live-shots/app-mem.log`,
`app-dn.log`; raw frames in `/Volumes/samsung/pro-test/live-127/`.

## Cause (of the panic)

wgpu reports Apple Silicon GPUs as `IntegratedGpu` (unified memory,
wgpu-hal `metal/adapter.rs`). `tmv_sam::backends()` (src-tauri/sam/src/lib.rs)
pushed Metal `DiscreteGpu(0)` first. cubecl-wgpu's `DeviceService::init`
`expect`s the adapter, so it panics on the device runner thread; the caller's
oneshot `recv().unwrap()` then panics with `RecvError`. `pick_device`'s
`guarded` catches it and the next candidate (`IntegratedGpu`) works. So the
GPU **was** used — the panic was noise plus a wasted attempt, not a CPU
fallback.

## Fix (PR #129, branch `claude/project-thread-0xa9be`)

- New `gpu_kinds(apis)` lists wgpu adapters per graphics API once (cached)
  and returns the real-GPU kinds present (discrete → integrated → virtual;
  never software renderers).
- Metal, Vulkan (replaces `vulkan_gpu()`), DX12 and the generic wgpu path
  only push kinds that exist. Order is unchanged:
  Vulkan discrete → CUDA → Vulkan integrated/virtual → DX12 → Metal → CPU.
  `TMV_GPU=…` still forces one.
- `pick_device` logs `GPU: using <backend>`.
- Checked: `cargo check -p tmv-sam` with `wgpu,flex`, `vulkan,flex` (Linux),
  `--target aarch64-apple-darwin --features metal,flex`,
  `--target x86_64-pc-windows-msvc --features vulkan,flex`.

## Not yet verified

1. Real M1 Max run on the branch: stderr should show `GPU: using Metal` and no
   `AdapterUnavailable` / `RecvError`. Quick ways (in `src-tauri`):
   `cargo test --release --features pro --lib ctf_gpu -- --nocapture`, or
   `cargo run --release --example gpu_bench --features pro`, or open Pro and
   run any GPU task.
2. Windows iGPU-only and Linux iGPU laptops (should go straight to Vulkan
   integrated; DX12 only tried for kinds DX12 lists).
3. CI on the PR (test macos/windows/ubuntu) — merge when green.

## Open: why membrane segmentation took 66 min (not fixed here)

It ran on Metal with GPU at 100%. Hypotheses, unverified:
- A second Pro instance (`/Volumes/samsung/pro-test/amira-126`, pid 16020)
  was using the GPU at the same time → contention.
- The crop's pixel size > 10 Å: `membrane::grid_at` rescales to 10 Å voxels,
  so a coarse crop is *upsampled* into a much larger grid (e.g. 20 Å → 8×
  voxels), times 8× TTA.
Reference speed (PR #123): full tomogram ~13 min on the Mac, ~4 min on the
RTX 5080. To check: log the input pixel size and grid size, rerun alone on an
idle GPU, compare with `TMV_GPU=metal` vs CPU.
