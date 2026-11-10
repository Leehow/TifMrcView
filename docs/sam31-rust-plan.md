# SAM 3.1 native Rust support

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

## Authorized release continuation (2026-11-10)

User authorized commit, rebuild, packaging and upload. Source commit 8eb9eb7
contains only this task's SAM changes. Other pre-existing dirty files remain
untouched. A canonical task-owned clean worktree was created at
`/Users/haoli/leehow/tifview-sam31-release`, branch `codex/sam31-native-release`,
from published main 4b3d347 (0.7.192). Native change f7760a1 and packaging
license change b0ae046 were integrated through PR #136 into main
4c1f16c7198dd04b13137456e6ba6c8889abb24e (candidate version 0.7.195).

Candidate packaging/build run: https://github.com/Leehow/TifMrcView/actions/runs/42851571001
It was superseded by the final-main run below after the SAM fixture repair.
The two redundant same-tree PR runs were canceled by this task; unrelated
PR/workflow runs remain untouched.

Pro installers and CLI archives now include SAM 3 license notices. Model
weights and microscopy images are excluded from the public packages. Local
package/release tests passed 10/10; the native source hash is unchanged from
the retained eight-case parity verification.

The candidate CI found a stale ignored SAM 2 sphere test fixture assigning
`Predictor` directly instead of the new `NativePredictor::Sam21` wrapper.
One fixture line was corrected; production inference code is unchanged.
Offline `cargo check --tests --no-default-features --features pro` passed,
including the ignored test body. Fix commit 6b17e8f was merged through PR #137
into main 97e7721c643c296274be0d7bc7fbefedbc20540c (version 0.7.197).
The failed 0.7.195 candidate is not published.

Final-main CI: https://github.com/Leehow/TifMrcView/actions/runs/42857292837
Final main CI completed with a green workflow status and all release artifacts.
Linux and macOS full jobs passed; SAM crate tests passed 9/9 on all three OSes.
Raw Windows logs revealed one pre-existing fixture failure hidden by the
PowerShell multi-command step: `model_store::tests::corrupt_or_different_source_is_rejected_and_first_checkpoint_is_recoverable`
tries to truncate a memory-mapped MRC, raising Windows error 1224 at line 433.
The published 0.7.192 baseline run 42745883500 has the identical failure, and
`model_store.rs` is unchanged between that baseline and this release. The
remaining Windows Pro tests passed (414). This is retained as an explicit
legacy test exception; the unrelated model-store fixture/CI shell are outside
the SAM upgrade and were not changed. No claim of a fully clean Windows Pro
test suite is made. Raw final/baseline logs are retained in the evidence folder.

Publication job succeeded in run 42872320870:
https://github.com/Leehow/TifMrcView/actions/runs/42872320870
Its reuse log confirms artifacts came from final-main run 42857292837, exactly
commit 97e7721c643c296274be0d7bc7fbefedbc20540c. The publish job is complete;
the dispatch workflow also starts redundant tests of that already-built SHA,
which were still running at the time of website verification.

Website verification passed: eight ordinary/direct download and updater feeds
report 0.7.197; 26 artifact URLs return HTTP 200 with nonzero content lengths,
matching declared download sizes where present. Twenty signature sidecars
match updater manifest values, and the CLI SHA256 sidecar matches its download
manifest. These are metadata checks; full installers were not downloaded to
the Mac for checksum or GUI validation. Evidence: `release-verification.json`
and retained public feed snapshots in the evidence directory above.

Task-owned worktree closeout is `closed`; the final lifecycle audit reported
`audit_ok`, zero pending entries, no registered worktree and no local task
branch. The pre-existing checkout and unrelated dirty files remain intact.
The local SAM fixture and Pro license-resource mapping are synchronized with
the published implementation. CI rebuilt release installers; the existing
installed App was not replaced, and live local GUI acceptance is not claimed.
