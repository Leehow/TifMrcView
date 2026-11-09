# Denoising and trained-model persistence

## Authorized outcome

- Integrate Z comparison preview fixes, IMOD NAD 3D and bilingual algorithm names into `codex/handoff-integration-20261104`, preserving its existing features.
- Automatically save and reload locally trained missing-wedge, self-supervised denoising, U-Net segmentation and random-forest models with detailed effective parameters and data identity.
- Restore inference without retraining. Optimizer-state training resume is not claimed.
- Build, stable-sign, update the canonical `/Applications/TifMrcView Pro.app`, and verify real GUI behavior, retaining existing training outputs and weights.

## Source and ownership

- Current integration HEAD: `96f2401`; tracked files were clean before this task. Existing `.tmp/` and other active plans belong to earlier work and must remain intact.
- Installed executable's CDHash `29f72ed4f2862c1d1850353afacaae8b6dd44db0` exactly matches the retained installation receipt for `46d3b5e`. Integration HEAD adds only two membrane-related source changes over that installed source; preserve them.
- Prior fixes remain uncommitted in `/Users/haoli/leehow/tifview` on `codex/denoise-z-nad3d`; unrelated XYZ/i18n/branding changes there are outside this integration.
- Existing integration worktree is explicitly authorized for edits; do not adopt or remove any pre-existing worktree. No pushes or public release are authorized.
- Build/cache root: `/Volumes/samsung/tifview-build.noindex`, outside source checkouts. Use offline dependencies and at most four build jobs.

## Retention gate

- Old App PID 16567 must remain alive until its in-memory trained network has been demonstrably preserved, or the user changes that requirement.
- User explicitly requires network weights for inference without retraining, not merely the corrected MRC volume.
- Verified installed code stores this network only in `wedge::MODEL`. Existing commands export corrected voxels, with no checkpoint export command. New-version persistence alone cannot retrieve old-process weights. Do not replace this requirement with a fresh retraining run or claim an MRC is a checkpoint.
- Saved corrected volume: `/Volumes/samsung/pro-test/amira-126/ui2/sta4p55-roi_wedge.mrc`; original: `/Volumes/samsung/pro-test/handoff-integration-20261104/gui-cli-empty-result/sta4p55-roi.mrc`.

## Execution and acceptance

1. Preserve and hash existing image outputs; synchronize only scoped prior fixes, retaining integrated live-preview behavior.
2. Implement atomic per-image model artifacts, detailed training records, strict restore compatibility checks and visible save/restore failures. Preserve prior usable checkpoint on failed/cancelled training.
3. Test actual save/drop/reload/inference equality for each trained model, source mismatch/corruption rejection, failure preservation, and preview switching.
4. Prepare stable-signed package; verify signature, canonical path and source/artifact identity. Do not close old process to complete packaging.
5. Resolve old-process weight retention before any restart. Real installed-App GUI acceptance remains pending until a safe update path is proved.

## Evidence so far

- Prior source branch: 21 frontend cases, six classical-denoise cases, three NAD adapter cases including actual IMOD differential output, and one CPU live-snapshot case passed; Pro compilation passed. These are not integration-branch or installed-GUI acceptance.
- Retained references: Burn 0.22 saving/loading uses strict tensor stores and atomic SafeTensors writes; BioImage.IO records weights, architecture and training provenance. Use these practices without changing dependencies or model algorithms.

Status: implementation in progress; integration tests, packaging and GUI gates are not yet accepted.

## Implementation milestone (2026-11-09)

- Scoped NAD/preview changes synchronized; 27 integration-branch frontend/ownership cases passed, including the integration's existing membrane cases. Kept denoise/wedge live training behavior.
- Existing source, labels, corrected MRC copied and SHA-256 verified under `/Volumes/samsung/pro-test/denoise-model-persistence-20261109/preserved/manifest.json`.
- Added `model_store.rs`: per-image `.tmv-models/series-N/<kind>/cC-tT/`, immutable hashed weights, atomic readable `model.json` / `checkpoint.json`, full source-file SHA-256 and cached identity, strict architecture/source/payload checks. Completed model takes precedence over a partial checkpoint. Clearing classifier now records disable time rather than deleting weights.
- Added `trained_models.rs` adapters for neural SafeTensors and serialized forest; full effective configs, seed, normalization, class mapping, progress, metrics and provenance. Restore is inference-only. Completed/partial desktop training hooks added, partial neural callbacks now provide exact step; explicit restore command rejects obsolete window tokens.
- Added `ui/pro/model-store.js` with readable records, failure visibility and Finder reveal. Denoise/wedge auto-restore wired; label classifier/U-Net restore wiring added and still being verified.
- Core tests passed: 3 model-store integrity/failure cases and 4 actual train/save/drop/reload/equal-inference cases (one per GUI-trained model) on the CPU. Further changes since these runs require the final targeted rerun, including newly added clear/disable case.
- Build cache copied to canonical external `/Volumes/samsung/tifview-build.noindex/target`; offline integration Pro check passed before the latest frontend/metadata changes. A later check is queued behind another task's `documents::tests` build in the original `/Users/haoli/leehow/tifview` checkout; the other task is editing that original checkout, not this integration worktree. Do not absorb its documents/tabs/layers changes or touch its processes.
- Old App PID 16567 has not been closed or altered. It has no checkpoint export API; preserving that in-memory network remains a separate unresolved migration gate. No installed update or new-version GUI acceptance has occurred yet.

## Concurrent installation change and user decision

- At 00:45 the installed CDHash was found changed to `89a5df8c8c69e05f83d376919844ae5252bd60b1`; PID 16567 had exited, new PID 65820 started 00:41:49. This task did not close or replace the old process.
- Read-only evidence from chat `为 Pro 版添加图像标签页和图层合并` (01a1beb7-a493-75c2-ab8c-76af2f97db66) shows its old-App quit and debug-executable replacement at 00:41, followed by native tab/layer/export checks. Its source remains uncommitted in the original checkout; preserve it.
- Old weights have no discovered recoverable copy in the two inspected data folders. Corrected volume, raw data and labels remain verified in preserved backup. Do not claim an executable backup contains runtime weights or claim old weights migrated.
- User explicitly authorized contacting that chat, merging its tab/layer features into this integration branch and doing one unified update/GUI acceptance. A read-only handoff request was sent; that chat is asked not to install, restart or edit code further.
- Current installed features now include tabs/layers. They must be retained; do not overwrite the new installation with a package lacking them. Await handoff before importing the other owner's patch.
- Core/preview UI checks: 33 frontend cases passed; latest Pro check passed after Arc coercion fix. Final persistence tests and combined-feature tests still need the integrated final source.

## Unified integration progress

- Initial peer snapshot (16 hashed files) retained at `/Volumes/samsung/pro-test/denoise-model-persistence-20261109/peer-initial-snapshot`; tabs/layers backend and UI integrated using the original source baseline, while preserving integration live monitor, CLI and relocation modules. Existing XYZ overview and terminology are retained because the current installed App contains them.
- Current unsaved composite was saved via the native Save dialog as `preserved/合成图2-before-unified-update.tmvmerge`; selected second layer Z=2, both source paths, view and editable layer properties verified from JSON. `preserved/composite-manifest.json` hashes project and both sources.
- Combined initial Node checks: 37 passed. Combined Pro `cargo check --offline --features pro --lib` passed. Combined targeted Rust tests: model_store 5, actual model roundtrip 4, genuine IMOD NAD 3, classical denoise 6, document lifecycle/cache 2, merge output formats 1: 21 passed. These do not substitute for final-source native GUI acceptance.
- GUI test inputs are real-data central crops, with exact provenance in `gui/crop-provenance.json`; originals are untouched.
- Peer owns final requested tabs/layers UX in original checkout; no parallel native building/install/UI. Await ready-for-integration handoff before final build.
