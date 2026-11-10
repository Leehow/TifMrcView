# SAM 3.1 native Rust support

## Release continuation (2026-11-10)

The user subsequently authorized committing, rebuilding, packaging and
uploading. This supersedes the implementation phase's no-publishing boundary
below. Scope remains SAM 3.1 and the packaging/license changes required to
distribute it; unrelated dirty work is preserved.

- Source commit: `8eb9eb7` on the original checkout.
- Clean integration: `/Users/haoli/leehow/tifview-sam31-release`, branch
  `codex/sam31-native-release`, based on published main `4b3d347` (0.7.192).
  Cherry-pick `f7760a1` preserves main's later runtime/UI work.
- Existing GitHub Actions `build.yml` owns cross-platform tests, installers,
  updater signatures and upload to the established download site. Release
  verification will inspect the resulting artifact metadata and public feeds.
- Pro and CLI packages must carry SAM3-LICENSE and the mixed-license notice.
- No unrelated in-progress feature is included. No model checkpoint or
  microscopy image is published as part of the code/package upload.
- Pending: clean-branch validation, integration, package/upload completion,
  public-feed verification and task-owned worktree closeout.

## Authorized objective and acceptance

Implement SAM 3.1 interactive image segmentation in the existing Rust/Burn
runtime. No Python service, subprocess, LibTorch or ONNX Runtime dependency.
Load the already-downloaded official `sam3.1_multiplex.pt` directly in Rust.
Support positive/negative points, boxes, cached image features and mask logits.
Verify native results against the retained image references, including the
cryo-ET slice that motivated this request. Report execution separately from
App/GUI acceptance and SAM 3.1 multiplex video tracking.

## Scope and ownership

- Integration checkout: `/Users/haoli/leehow/tifview`, branch
  `codex/denoise-z-nad3d`, initial HEAD `4c434e9`.
- Existing unrelated dirty files must be preserved. The SAM crate was clean.
- Editable: `src-tauri/sam/`, a scoped Rust example, and this plan. App SAM
  routing may be adapted only as required to expose supported native behavior.
- No publishing, packaging, unrelated cleanup, model retraining, or dependency
  upgrades. Text/concept detection and multiplex video are separate paths and
  are not implied by interactive-image acceptance.

## Evidence and decisions

- Existing `tmv-sam` uses Burn 0.22.1 with Metal/Vulkan/CUDA/CPU device selection.
- Official SAM 3.1 image path: ViT-L/14 (1008 pixels, 72x72 embedding),
  interactive FPN neck, SAM prompt/mask heads. Existing SAM 2 prompt/mask
  algebra can be reused after parameterizing resolution and prompt padding.
- Hugging Face's Apache-2.0 `sam3_tracker` implementation independently reuses
  SAM 2 prompt/mask classes with the SAM 3 vision backbone. An inspected Rust
  ONNX implementation uses a separate native runtime; keeping Burn avoids that
  dependency and matches the user's Rust-only requirement.
- Official checkpoint: revision `daa63191845a41281374e725f4c9e51c7a824460`,
  SHA256 `0567debeec80ba4ac6369540c6c248025283cb3ff2b92827509e57e2b3541cb6`.
- Local reference: `/Volumes/samsung/sam-bench/results/sam31-feasibility-20261110/`.
  Python reference used an MPS compatibility copy and skipped optional
  hole/sprinkle postprocessing; native parity will use the same raw-mask scope.

## Steps and current state

1. Inspect architecture, checkpoint layout, license and existing Burn API — done.
2. Implement native vision encoder and reuse resolution-aware prompt/mask heads — done.
3. Load all required interactive-image parameters, reject missing weights — done.
4. Compile offline, run meaningful unit/regression checks and native real-image
   parity checks (points, negative points, boxes, mask feedback) — done.
5. Document actual supported surface and remaining App/propagation acceptance — done.

## Validation

- Final offline native executable build and headless Pro library check passed.
  Desktop packaging and GUI acceptance are separate and were not performed.
- Nine SAM crate unit/regression tests passed, including the new half-pixel
  output interpolation and GPU-reference byte-preprocessing tests.
- Final pure Rust/Metal inference passed on cryo-ET and fluorescence PNGs,
  with positive point, negative point, box and mask feedback on each image.
- All eight 288x288 thresholded masks matched the MPS reference exactly.
  Seven full-resolution masks were identical; fluorescence mask refinement
  differed by one pixel (148,574 vs 148,573 foreground pixels).
  Minimum full-resolution mask IoU: **0.9999932693472613**.
  Maximum absolute predicted-score difference: **0.00004285573959350586**.
- Burn's output interpolation convention differs from PyTorch's; SAM 3.1 now
  uses a scoped native half-pixel output resize. Input preprocessing matches
  torchvision's float GPU path with one final ties-to-even byte rounding.
- These measurements establish implementation parity on these samples, not
  segmentation accuracy against biological/manual ground truth.
- `node --check ui/pro/labels.js` and scoped `git diff --check` passed.
  Dynamic-library inspection found only macOS system libraries/frameworks;
  no Python, LibTorch or ONNX Runtime is linked into the native example.
- Native metadata compatibility: pytorch-reader 0.22 rejects complex buffers.
  For the exact SHA-verified official checkpoint only, a temporary Rust-created
  ZIP aliases unused complex64 RoPE storage metadata to equal-width opaque f64.
  Learned weights remain unchanged; the native model never reads those buffers
  and recomputes its real-valued RoPE tables. RAII removes the temporary ZIP on
  success/error. Final inspection found no temporary ZIP in either the
  checkpoint directory or standard Pro model directory.
- App source routes to the native interactive backend when the official
  `sam3.1_multiplex.pt` (3,502,755,717 bytes) is installed next to SAM 2.1.
  Status identifies the actual loaded variant. The 3.1 video-memory button is
  hidden; the backend rejects that unsupported path instead of using SAM 2.
- Final evidence directory: `/Volumes/samsung/sam-bench/results/sam31-rust-20261110/`.
  `verification.json` contains all eight case metrics, acceptance thresholds,
  limitations and the SHA256 of the exact final native source.

## Runtime use and references

The native API is `tmv_sam::sam31::Predictor`: load the checkpoint with a Burn
device, call `encode_rgb` / `encode_gray`, retain `Encoded`, and call `predict`
with points/box/optional 288x288 mask logits. The batch example is
`sam/examples/sam31_image.rs`; it accepts checkpoint, PNG/JPEG, JSON requests
and an output directory, with no Python process or model export.

The standard Pro model directory now contains a link named
`sam3.1_multiplex.pt` to the verified checkpoint on Samsung. SAM 2.1's existing
link is preserved. New App source selects 3.1 before first model load and
keeps the chosen model for that process. The installed App binary has not
been rebuilt or replaced in this task.

Primary implementation comparisons:

- [Meta SAM 3 vision encoder](https://github.com/facebookresearch/sam3/blob/main/sam3/model/vitdet.py)
  and [model builder](https://github.com/facebookresearch/sam3/blob/main/sam3/model_builder.py):
  exact image architecture and official checkpoint names.
- [Hugging Face SAM 3 tracker](https://github.com/huggingface/transformers/blob/main/src/transformers/models/sam3_tracker/modular_sam3_tracker.py):
  independently confirms reuse of SAM 2 prompt/mask heads at the new resolution.
- [Rust ONNX implementation](https://github.com/baseweight/sam3_rust): confirms
  the encoder/interactive-head split, but its native ONNX Runtime dependency
  differs from this project's existing Burn runtime and was not adopted.
- Installed torchvision `transforms/v2/functional/_geometry.py` lines 316–360:
  GPU uint8 input is cast to float32 for resizing and rounded once afterwards;
  CPU may use an integer resize with intermediate byte rounding. Native input
  preprocessing deliberately follows the GPU reference used by the SAM 3.1
  oracle. Output resizing follows `align_corners=false` half-pixel coordinates.

## Release validation continuation (2026-11-10)

PR #136 integrated the native implementation and bundled license notices into
main 4c1f16c. Build run 42851571001 compiled the Linux Pro installer and CLI,
but its Pro test target found an old ignored SAM 2 sphere fixture assigning
`Predictor` directly where the new wrapper requires `NativePredictor`.
The fixture now explicitly wraps SAM 2.1; production inference code is unchanged.
No artifacts from the failed revision are published. Cross-platform CI and
public feed checks remain required before release completion.
Local offline `cargo check --tests --no-default-features --features pro`
passed after the fixture repair, including compilation of ignored test bodies.
