# Handoff: Pro membrane segmentation (MemBrain-seg weights)

State on 2026-11-04, for whoever continues this work.

## What exists

Pro segments the membranes of a tomogram with the published MemBrain-seg
v10 model. The weights carry no licence, so the app neither ships nor
downloads them. The user downloads `MemBrain_seg_v10_beta.ckpt` (about
730 MB, Google Drive link in the teamtomo/membrain-seg README) and picks
it once. The app checks that it loads and remembers the path.

| Piece | Where |
|---|---|
| Network (MONAI DynUNet layout, widths 32–1024) and the `.ckpt` loader | `src-tauri/src/membrane/dynunet.rs` |
| Resampling to 10 Å, sliding windows, TTA, cleaning, label write-back | `src-tauri/src/membrane/mod.rs` (`segment`, `score`, `Pass`) |
| Picker box (official page link and file picker), shared by the labels panel and one-click | `ui/pro/membrane.js` |
| Labels-panel button "自动分割膜" | `ui/pro/labels.js` |
| Membranes as the last step of one-click display tomograms | `src-tauri/src/tomo/auto.rs` |
| Tauri commands (`membrane_status`, `membrane_choose`, `membrane_segment`, `membrane_cancel`) | `src-tauri/src/desktop.rs` |
| Command-line tool for comparisons | `src-tauri/examples/membrane_seg.rs` |
| Our own network and its training tool (kept, not used by the app) | `membrane/net.rs`, `examples/membrane_train.rs` |

The remembered choice is a text file `models/membrane-model.txt` beside
the plugin user dir. Our own `.safetensors` model also loads via `membrane::load`.

## PRs

- **#122 (merged):** official weights, the picker and the scoring path.
- **#123 (speed-up and exactness):** CI was green before a merge with main
  (#127, the live view). The conflict in `score()` has been resolved
  (live hooks kept); see the PR for its final state. It makes the
  following changes:
  - Two-stage instance norm. Burn's own took 0.38 s per 160³ window,
    the new one 0.03 s.
  - Convolutions with ≥ 64 input channels, the strided ones and the ×2
    upsampling run as one matrix product per kernel offset (`conv`, `up2`
    in `dynunet.rs`). They are 10–50× faster than Burn's direct kernel.
    The full-resolution 32-channel convolutions still use Burn's kernel.
  - Host-side gather and scatter happen in one parallel pass.
  - Mirrored copies place their windows mirrored, each copy is blended on
    its own, then the copies are averaged. This matches what MemBrain-seg
    does; without it, Dice on 512 × 512 × 128 was 0.983.
  - Standardisation uses every voxel.

## Verified numbers

- **Exactness:** the reference is MemBrain-seg 0.0.10, fp32, no TF32. On
  a 512 × 512 × 128 tomogram treated as 10 Å, every score is within
  1e-4 and 0 voxels differ (RTX 5080, Vulkan). On the 240 × 160 × 128 crop
  the same holds on the M1 Max (Metal).
- **Speed per 160³ window, eight per position:**

  | GPU | Before | After |
  |---|---|---|
  | M1 Max | 10.9 s | 2.7 s |
  | RTX 5080 | — | 0.87 s |

- **Whole tomogram:** 512 × 512 × 128 at 10 Å is 36 positions × 8
  copies. It takes 4 min on the 5080 and about 13 min on the Mac.
- **The user decided:** always eight mirrored copies, matching the
  official run. Fewer copies (`Pass::Budget`) only reach Dice 0.95
  against it.

## Open problems (priority order)

1. **Wrong results on Metal now and then.**
   - In 1 of 4 runs of the crop on the Mac (2026-11-04, after the #127
     merge) the scores were garbage: 0 components, Dice 0.003. The other
     3 runs were exact.
   - The GPU was busy with something else at the time: windows took
     2–4× longer than usual.
   - Earlier, tensors over about 1 GB gave different results from run to
     run on Metal. That one is fixed: the up-block no longer joins its
     inputs.
   - Suspects: Burn / cubecl-wgpu 0.11.3 under memory pressure or
     concurrency, or autotune.
   - Next steps:
     - Reproduce with `examples/membrane_seg` in a loop.
     - Check whether it also happens on main before #123.
     - Consider a sanity check: if a window's output is constant or NaN,
       rerun it.
2. **Mac speed.** One-click segmentation on the Mac was reported to take
   about 66 min. This was reported from another task; it was likely
   before #123 or on a large tomogram.
   - Remaining time goes to the four full-resolution 32-channel 3³
     convolutions (about 0.37 s each on the M1 Max, 0.6 TFLOPS) and to
     the ×2 upsample to 32 channels.
   - Ideas:
     - A fused custom cubecl conv kernel for 32→32 at 160³.
     - f16 (Burn's conv in f16 was only 12% faster).
     - Fewer positions with ⅓ overlap (not yet validated against the
       official run).
3. **Rescaling path not compared against the official one.** The
   comparisons used tomograms treated as 10 Å, so no resampling ran. The
   app resamples to 10 Å with trilinear interpolation after a Gaussian
   low-pass. MemBrain-seg's `--rescale-patches` instead rescales each
   patch in Fourier space. Expect small differences on non-10 Å data;
   these have not been measured.
4. **Not tested end to end in the packaged Pro app:** picker → segment →
   labels. Only the picker box was checked, in a Chromium harness with a
   mocked `invoke`.

## How to test

Unit tests:

```bash
cd src-tauri && cargo test --features pro --lib membrane
```

The `dynunet` tests compare the matrix-product conv, `up2` and the instance
norm against Burn's own modules.

Official comparison:

1. Run our side:

   ```bash
   cargo build --release --features pro --example membrane_seg
   ./target/release/examples/membrane_seg IN.mrc MemBrain_seg_v10_beta.ckpt OUT_mask.mrc --pixel 10 --scores OUT_scores.mrc
   ```

2. Run the official side:

   ```python
   from membrain_seg.segmentation.segment import segment
   segment(IN, CKPT, OUTDIR, store_probabilities=True)
   ```

   For fp32, patch `torch.cuda.amp.autocast` to a null context and set
   `torch.backends.cudnn.allow_tf32 = False`. With some typer/click
   versions the `membrain` CLI fails to start, so call the function
   directly.

3. Compare `*_scores.mrc`: max |diff|, sign agreement, and Dice of > 0.

On Windows, build with `--no-default-features --features pro`.

## Data and machines

Where the checkpoint, the test tomograms, the official MemBrain-seg
environment and the test machines are kept is in the project's private
handoff notes, not in this public repository.

## Website

The Pro page row `pr_mem` and the model note are in all 7 languages
(tifmrcview-site 62617aa). #123 changes no UI, so the site needs nothing
more.
