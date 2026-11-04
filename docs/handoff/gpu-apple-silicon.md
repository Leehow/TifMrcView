# Handoff: Pro GPU selection on Apple Silicon (PR #129)

Status 2026-11-04: fix pushed, cross-compiled for Linux / macOS / Windows,
and verified on the user's M1 Max (see "Mac test" below).

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

## Mac test (M1 Max, 2026-11-04)

- Old logs (app-mem.log, app-dn.log): one `Metal backend failed … RecvError`
  each, no "using the CPU"; membrane line says Metal → the integrated GPU was
  used after the panic.
- Branch build: `cargo build --release --no-default-features --features pro
  --example membrane_seg` (6 min, reusing the main checkout's target). Run on
  `TS_041_small.mrc`: stderr `GPU: using Metal`, no AdapterUnavailable/panic.
- Timing (official MemBrain_seg_v10_beta.ckpt, Pass::Official): volume
  160×160×96 at 13.48 Å → grid [216, 216, 129], 4 windows × 8 copies = 32
  network passes; network 521 s, total 522 s, CPU user time only 4 s (so it
  ran on the GPU). ≈16 s per pass. Another session was compiling at the same
  time (load ~24), so slightly pessimistic.

## Not yet verified

1. (done, see above)
2. Windows iGPU-only and Linux iGPU laptops (should go straight to Vulkan
   integrated; DX12 only tried for kinds DX12 lists).
3. CI on the PR (test macos/windows/ubuntu) — merge when green.

## Open: why membrane segmentation took 66 min (not fixed here)

It ran on Metal with GPU at 100%. The Mac test above shows the main reason:
Metal itself is slow for this 3D U-Net, ~16 s per window pass, × 8 TTA copies
× number of windows. The second Pro instance (pid 16020) was idle (0% CPU)
during the test, so contention is probably not the cause. Other factors:
- A second Pro instance (`/Volumes/samsung/pro-test/amira-126`, pid 16020)
  was using the GPU at the same time → contention.
- The crop's pixel size > 10 Å: `membrane::grid_at` rescales to 10 Å voxels,
  so a coarse crop is *upsampled* into a much larger grid (e.g. 20 Å → 8×
  voxels), times 8× TTA.
Ideas: profile the Metal kernels of the 3D conv (MSL vs WGSL, workgroup
sizes, fusion), fewer TTA copies on slow GPUs, or larger/fewer windows.
Reference speed (PR #123): full tomogram ~13 min on the Mac, ~4 min on the
RTX 5080. To check: log the input pixel size and grid size, rerun alone on an
idle GPU, compare with `TMV_GPU=metal` vs CPU.
