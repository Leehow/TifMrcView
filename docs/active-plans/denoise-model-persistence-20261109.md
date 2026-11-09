# Denoising and trained-model persistence

## Native NAD replacement — user correction, 2026-11-09

Status: CPU and actual Metal GPU numerical gates passed; prior0.7.178 remains running. Preparing the unified release and native GUI acceptance.

Objective: replace the external IMOD NAD3D adapter with an in-process Rust implementation of the referenced NAD/EED algorithm; compare actual numeric outputs with original software, then update the canonical installed App and perform real GUI acceptance. Retain source/results/models and the installed tabs/layers/autosave/scale-bar/channel-visibility features. No website publishing, remote deployment, dependency refresh or unrelated cleanup.

Current source: integration branchcodex/handoff-integration-20261104, initialHEAD8cb4250; only pre-existing untracked.tmp/ and handoff-workers plan. Installed0.7.178 receipt points to3c8d94d(channel visibility), owned in a clean other checkout. Reviewed/cherry-picked that14-line app.js feature as4eee7e4, excluding its website documentation follow-up. No worktree created/adopted. Scope: denoise_nad Rust kernel/adapter, required denoise UI/backend labels, regression/reference fixtures and this plan.

Evidence: the installed IMOD shell wrapper withoutIMOD_DIR reproduces screenshotexit127 (/bin/realbin/nad_eed_3d). The actual prepare() regression `IMOD_NAD_PATH=/nonexistent/tmv-regression-no-imod ... cargo test --offline --no-default-features --features pro --lib denoise_nad::tests::native_nad_works_without_imod_installation -- --exact` has been run and FAILS on the real processing seam in0.01s after warm compile. This is a known implementation-design correction; no speculative environment-repair hypothesis loop is needed.

References retained in `/Volumes/samsung/pro-test/native-nad-rust-20261109`: original paper DOI10.1006/jsbi.2001.4406 (published equations/discretization), official https://bio3d.colorado.edu/imod/doc/man/nad_eed_3d.html (EED-only CLI semantics), official source/license page and public source mirror. Independently implement math in Rust; do not embedGPL C program/runtime or translate copied C source into the Apache project. Original executable is a development-only numeric oracle. No large downloads; paper636680bytes, selected C source37105bytes; existing compiler/dependency caches only.

Acceptance gates:
1. Existing no-IMOD production prepare/preview/export test passes, and production module has no executable lookup/subprocess calls. Deterministic true3D math with explicit boundary behavior, validation, cancellation and bounded memory/cache lifecycle.
2. Original IMOD comparisons on synthetic flat/step/diagonal/noisy/small/asymmetric volumes and real cryo-ET crops across K and iteration settings. Record absolute/SD-normalized error and boundary/interior, all Z planes; explain any meaningful deviation. Also establish noise/edge capability on known signal; numeric agreement alone is not biological validation.
3. Z0/middle/last preview and saved outputs use the same full-volume result; no stale cache after parameter/source/channel change. Preserve32bit MRC calibration and source pixels, cancel cleanup.
4.56+ relevant existing tests, appropriate Pro compilation/build; offline canonical external build root/jobs4/stable certificate. Before replacing current0.7.178 inspect live jobs/unsaved documents and preserve weights/results. Native GUI succeeds without an IMOD runtime/environment; save/restart and latest existing features retained. Package/sign/LS audit and precise evidence.

Completed: in-process native kernel and disk cache integrated. No executable search/subprocess remains.7 backend tests passed with deliberately invalidIMOD_NAD_PATH, including actual prepare/preview/export,18 original-program golden volumes, cancellation/invalid-input/Z coupling and cache retention.56 existing frontend regressions and JS syntax/whitespace passed.18 synthetic volumes and4 real128x128x32 reference runs all match IMOD5.1.3 exactly, all voxels including boundaries (maxdelta0), n up to20. The fixed UI contract remains sigma0/dt0.1; other originalCLI options are not claimed. Initial Jacobi/algebraic shortcuts exposed floating-order sensitivity of the nearly rank-one tensor; independent Householder/QL math and roundoff-compatible evaluation resolved it. `docs/nad3d-native.md` records the algorithm, limits and provenance. Known-signal metrics are retained, including an over-smoothed diagonal case also seen in the original; no universal denoising/biological benefit claimed.

Preservation baseline:10 file hashes (model records/weights and current source/result/project), installed0.7.178 receipt and existing signed ZIP reference under evidence/preserved. Installed-source manifest compared with integration: only the four intended denoise/module/UI files differ; channel visibility and all other existing features match. Native current tabs: original128x128x32 NAD test crop and old NAD export; sourceZ17/32,923%, current methodMedian radius1, split54, CPUfalse, saved foldertabs-autosave.../gui. No unsaved merge marker visible. Inspect live tasks again before quit.

User follow-up: remove the denoise CPU-only checkbox and always request the existing automatic backend policy, ignoring a legacy saved CPU override. Verified current owning code: learned training/inference use GPU with CPU fallback; NLM has GPU support but automatic selection retains CPU on Metal; Gaussian, median, 2D diffusion and native NAD3D currently run on CPU. No GPU NAD implementation is claimed or added. This small UI removal needs no external design research; existing backend policy and diagnostic/API overrides remain intact.

User expansion: add a genuine GPU implementation of NAD3D, automatically selected, with CPU fallback and original-program numerical validation. The CPU release compile completed; installation is deferred until GPU integration and acceptance. Existing cached Burn/CubeCL is the preferred Rust kernel route, without dependency downloads. Research: official W3C WGSL and NVIDIA floating-point documentation confirm operation reassociation/FMA can change results; ITK's GPU finite-difference filters confirm separate previous/next iterates and per-iteration synchronization. Local CubeCL/Metal reports no native f64. The unsmoothed reference tensor is ill-conditioned, so GPU precision must be validated separately rather than assuming the CPU goldens transfer.

First actual Metal run failed the strict original-program gate (real low-K 5-step RMSE0.058, max0.563); retained `gpu-first-hardware-failure.txt` and generated shader. Shader inspection directly showed implicit FMA replacing the evolution's separately rounded product/add. The cached CubeCL C++/MSL compiler applies InstCombine unconditionally, and Metal passthrough uses default fast math. Required narrow repair: vendor the same cached cubecl-cpp0.11.3 version, skip only this pass for the unique `tmv_nad_eed_` entrypoint prefix, and disable Metal fast math only for that prefix in the existing wgpu-hal patch. No other kernel policy/version changes or Internet payloads. GPU remains a documented hybrid: CPU f64 tensor eigensystem, GPU XYZ stencil. Rerun the complete original-program gate before acceptance.

GPU final numerical gate passed: all18 original-program synthetic and4 real-volume outputs now equal every reference voxel, including20 iterations. After disabling FMA, rare compensated-weight rounding still seeded amplified differences; conservative weight intervals now mark only potentially changed fp32 updates for the shared CPU step before the next tensor. All8 NAD tests (including actual Metal),6 classical-denoise tests,57 frontend/Metal-completion tests and Standard core check passed. Runtime device/compiler qualification precedes accelerated user data. Optimized kernel probes repaired0.13–0.23% of voxel steps on the real128x128x32 cases. Small-volume GPU/CPU timings are close; no broad speedup claim. A full256x256x128 existing corrected volume also has CPU/GPU maxdelta0; GPU median about0.99s versusCPU1.02s for2 iterations, so this is compatibility-first hybrid execution rather than a substantial measured acceleration.

Next: freeze source including UI simplification and GPU qualification, build offline/stable-sign, preserve current native state and update; verify native NAD full Z, absent CPU checkbox and exported voxels against original, retained features/models and final package/hygiene.



## Follow-up authorized after 0.7.173

Status: complete. Optimized stable-signed0.7.176 installed, restarted and real GUI accepted. Local ZIP verified; no public distribution.

Verified actual user messages in chat01a1beb7-a493-75c2-ab8c-76af2f97db66, IDs01a1bf69-d43f-70f2-bd4b-cce7a2351504 and01a1bf6a-f9c4-7b92-946c-d3866ff4b874: a single image appears as a layer; incoming layers fit the base proportionally/centered; familiar edge/corner transform handles; each edit automatically saves the editable project with no Save button; Delete/Backspace deletes selected layer with Undo while text fields keep normal editing.

- Original chat owns its same seven frontend/test/plan files and will deliver a frozen incremental snapshot. It must not build, sign, install, restart or manipulate the native App. This integration checkout owns backend, integration and final native acceptance.
- Required new native protocol: `create_merge_project({token, project}) -> absolute path`, verified against this window's retained source token; safe auto filename, no overwrite even under name races; prefer source directory, fallback to App data merge-projects; register successful path for subsequent saves. Existing `save_merge_project` interface continues, with complete same-folder atomic replacement off the UI thread.
- Precedent: Rust OpenOptions create_new guarantees atomic reservation (https://doc.rust-lang.org/std/fs/struct.OpenOptions.html); Qt QSaveFile stages beside destination and renames (https://doc.qt.io/qt-6/qsavefile.html). The previous annotation writer renames over the destination, so a mere exists check cannot safely create first projects. New project persistence uses a complete synced unique stage and no-clobber publication, with normal atomic replacement for edits; no dependency changes.
- Backend checks cover safe Unicode/basename names, file/symlink collisions, concurrent first creates, primary-path failure/App-data fallback and failed-save preservation/temporary cleanup. Native tests will verify actual first save without picker, edited/deleted/undone state after close/reopen, fit/handles, and preserve existing denoising/model state.
- Before any next native update: verify actual current App/model/job/unsaved-document state again; do not assume it is unchanged from the prior completed acceptance. Keep the current signed package, all weight records, user images and projects.
- Additional actual user message01a1bf84-195c-7a01-bfae-2d9c589d2ac8 in the same authorized chat requests the scale bar follow the base image. Include its calibration/geometry persistence and export in this frontend batch; no unrelated XYZ edits from the other owner.
- At08:58 UTC, the running0.7.173 had a newly edited single-layer user merge. Saved it through the native dialog to `/Volumes/samsung/pro-test/amira-126/ui2/sta4p55-roi_wedge_merge.tmvmerge` (previously absent), then copied/hash-verified it in `/Volumes/samsung/pro-test/tabs-autosave-20261109/preserved/`. SHA2567a6e7b9508f93bdea1b014e29b112e4534c022ca518aeda102353f737e49fbf1. Source tab observed Z79/128,211%; merge Z65/128,304%, selected bottom layer with slightly translated geometry. Denoise/wedge panels showed no active train/cancel job. Returned to saved merge. Inspect again immediately before the eventual quit.

- Frozen peer snapshot `/Volumes/samsung/pro-test/tabs-layers-20261109/refinement-autosave-20261109` applied with `git apply --whitespace=error`; all seven baseline/final hashes verified. The only app.js hunk is sourceChanged at draw end. Combined frontend/model/preview/ownership suite:56 passed. Backend create/update tests:5 passed on both Mac temporary and Samsung filesystems. Native follow-up acceptance remains pending.

## Follow-up final outcome — 0.7.176

Implementation8bcdf70 includes frozen UI and backend8c2817b.56 combined Node tests passed;5 backend filesystem tests passed on Mac and Samsung; final optimized offline build passed. Canonical stable-signed App and running executable verified, ZIP integrity and all bundle files match. Native acceptance verified single-image layer/promotion, real first automatic project creation, fit geometry,8handles/corner drag, Delete/Backspace/Undo/input isolation, base-calibrated scale transformation, immediate-close final-save, full App restart/project/export equality and own-trained N2V/wedge restore+inference exact equality4096voxels each. Original raw/labels/corrected/composite sources and6 model records/weights hash unchanged. User merge layers/Z/geometry/zoom retained, with small viewport navigation change<0.1px recorded. Current final UI is user mergeZ65/128,304%,20nm.

Authoritative follow-up report `/Volumes/samsung/pro-test/tabs-autosave-20261109/acceptance-report.md`; receipts and native GUI outputs alongside. ZIP `/Volumes/samsung/tifview-build.noindex/packages/TifMrcView-Pro-0.7.176-macos-arm64.zip` SHA256e0af64b941ed5c4815381caf0e03f2e50d60480e8bab571163287d78f066acf2. Final LaunchServices only canonical App; no remaining task stages. Historical memory-only legacy weight loss is not repaired or disguised. No push/public updater or server changes.

## Previous delivery outcome — 0.7.173 (2026-11-09)

Local software delivery is complete. Optimized, stable-signed `/Applications/TifMrcView Pro.app` is version0.7.173, packaged from implementation commit83f1c22 (with integrated tabs/layers, XYZ, NAD/Z fixes and four-model persistence). The final package and running executable identity match; no push or public updater/site release occurred.

Authoritative final evidence: `/Volumes/samsung/pro-test/denoise-model-persistence-20261109/acceptance-report.md`, `final-source-manifest.json`, `final-install-receipt.json`, `package-receipt.json`, `app-hygiene-audit.json` and `preservation-final-check.json`. Signed ZIP: `/Volumes/samsung/tifview-build.noindex/packages/TifMrcView-Pro-0.7.173-macos-arm64.zip`.

44 combined frontend cases passed; 21 targeted backend cases passed. Native GUI verified N2V/wedge automatic weights/parameters, restart/update inference equality (maximum delta0), real IMOD all32-plane export equality, editable merge save/reopen and PNG/TIFF pixel equality, independent Z, selected-layer dragging/Undo, Retina labels/100% shortcut and real2,000-step Metal training completing across a merge switch. Final one-line cube16 preference compatibility was rechecked on a fresh native restart. Original user save/training preferences restored, original corrected volume reopened at Z65/128.

The historical legacy-model retention gate below could not be met: another chat exited the memory-only old process before export. No verified recoverable old network copy was found. This limitation was reported; original image/labels/corrected voxels and saved composite remain unchanged. Newly trained models demonstrably survive process exit, version update and reopen. Do not describe the preserved corrected MRC as an old network checkpoint.

Task stages and task-copied cache App removed. LaunchServices audit resolves only the canonical App; pre-existing unowned test/source App files remain on disk (Spotlight can see them) and were precisely unregistered. Original checkout changes and pre-existing integration `.tmp/` / handoff plan preserved.

The sections below are chronological execution evidence and superseded gates, retained for audit.

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

Historical status at initial planning: implementation in progress; integration tests, packaging and GUI gates not yet accepted.

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
