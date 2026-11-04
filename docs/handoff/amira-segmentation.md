# Handoff: Amira-style segmentation rendering in Pro

State on 2026-11-04. Goal (user, 2026-11-03): make reconstructed tomograms look like
the middle panels of Houcek et al. 2026, *Structure*, Fig. 4. Those panels show coloured
surfaces of membranes, synaptic vesicles, ER, mitochondria and microtubules over a
tomogram slice, made in Amira. The paper's method: AreTomo2 → IsoNet →
MemBrain-seg (binary membranes) + Amira cylinder tracing (microtubules) → hand
editing and colouring in Amira; vesicle diameters with CryoVesNet.

Product rules that apply: one button, no parameters for the user, GPU first, all
code our own Rust (methods may be reimplemented, code and weights are not copied),
and the website (private repo `Leehow/tifmrcview-site`) updated with every feature.

## What is merged

| PR | What |
|---|---|
| #121 | `src-tauri/src/surfaces.rs`, `ui/pro/surfaces.js`: label map → per-object smooth meshes (surface nets + Taubin), WebGL lit render over a movable XY slice on white (3D display mode 「分割表面」, labels-panel button 「3D 表面」), click to pick, hide/delete/relabel (undoable), OBJ+MTL and PNG export |
| #122 | (other thread) MemBrain-seg: `membrane/dynunet.rs` loads the `.ckpt` the user downloads from the official links (weights have no licence, so we do not ship them); picker box `ui/pro/membrane.js`; labels-panel button 「自动分割膜」 |
| #124 | `membrane/vesicles.rs`: a membrane piece lying on a sphere is a vesicle (LSQ sphere fit, spread ≤ 2 nm + 8 % r, r 8–100 nm, membrane goes round ≥ 60 % of directions outside the missing-wedge band); written as 「囊泡」 in the same undo step as 「膜」 (`membrane::apply`); panel shows count, mean ± SD diameter, histogram, CSV export |
| #125 | touching vesicles split with RANSAC; tubes (esp. along the beam) rejected; 3D pick panel has one-click class buttons (囊泡, 其他囊泡, 质膜, 线粒体, 内质网, 微管, 突触后致密区, 其他膜) coloured like the paper |
| #126 | `src-tauri/src/filaments.rs`: microtubules without seeds: 2 nm voxels, Hessian rod score at the 12.5 nm scale, ridge lines at several thresholds (8/6/5/4/3 robust SD, pooled strictest first), joined end to end, kept when ≥ 100 nm, straight, and hollow (lumen t statistic ≥ 3 at ≤ 16 Å/px); drawn as 「微管」 tubes in the same undo step |
| #130 | (this PR) failure text stays on the membrane panel; 3D right-drag / Control-drag pans (Mac trackpad); `button_path_on_file` test |

Website: Pro page rows `pr_mem` (membranes + vesicles + microtubules) and `pr_surf`
(3D surfaces), all 7 languages, pushed to the site repo (commits 1faeaec, e56cdc2).

## Open / not done

1. **Speed of membrane inference.** PR #123 (other thread, open): 4–12× faster
   MemBrain-seg on the GPU. Without it a 256×256×128 tomogram at 13.5 Å took
   4034 s on the M1 Max in the GUI test. Merge it after its own review.
2. **Microtubule recall.** On CryoET portal 10000/TS_041 one of two annotated
   microtubules is found (voxel precision 0.94, recall 0.63); the missed one is
   short and along the beam. In a 480×464×200 crop of the same tomogram nothing is
   found (the lumen test rejects the line). Tried and dropped: recentring the line
   on the most tube-like cross-section (290 false finds) and seeded tracing along
   the local axis (lost the real one). Ideas not tried: cylinder template matching
   on the GPU (what Amira does), or a learned filament detector.
3. **Mitochondria / plasma membrane / ER automatically.** Not attempted; geometric
   rules are unreliable. Would need a trained classifier. CryoET Data Portal
   dataset 10000 (S. pombe) has per-tomogram annotations of membrane,
   mitochondrion, ER, vesicle, microtubule, Golgi, etc. (but its 「membrane」 masks
   leave out vesicle membranes, so they are no good for judging vesicles). Today the
   user assigns these classes by clicking objects in 3D — as the paper's authors did
   in Amira. The user has not decided on training (it needs tens of GB of data and
   days of GPU time).
4. **Our own membrane model** (#117, closed): Dice 0.80 vs MemBrain v10 0.86; user
   wants to keep improving it in parallel. Evaluation notes:
   `~/leehow/cryoet-notes/membrane-final-20261102/`. A licence request email to
   the MemBrain-seg authors was drafted; the user sends it.
5. Vesicle false positive seen on real data: the closed tip of a tube (Nav110).

## How to test

- Unit tests: `cd src-tauri && cargo test --release --features desktop,pro --lib -- surfaces vesicles membrane filaments`.
- Ignored file tests (release build), all in `src-tauri/src`:
  - `filaments_on_file`: `FIL_MRC=<tomogram> [FIL_GT=<microtubule mask>] [FIL_OUT=<mask.mrc>]`
  - `vesicles_on_file`: `VES_MRC=<membrane mask>`
  - `segmentation_on_file`: `SEG_TOMO SEG_MEM [SEG_VES] [SEG_LABELS]` (vesicles vs annotation; writes a label file for GUI tests)
  - `button_path_on_file`: `SEG_TOMO SEG_MODEL [SEG_LABELS]` (the whole button backend)
  - `tomogram_sized_timing` (surfaces.rs): synthetic 960×960×300 meshing time.
- GUI: build Pro locally (`npx tauri build --config src-tauri/tauri.pro.conf.json --bundles app`;
  touch `src-tauri/src/lib.rs` first if only `ui/` changed, or the old assets stay
  embedded), copy to `/Volumes/samsung/pro-test/amira-126/`, re-sign with the
  "TifMrcView Dev" certificate (see that folder's `*.ent.plist` and the
  pro-test-signing notes), start it with stderr to a log, and give Codex (terra) a
  brief **in the Codex desktop app** — `codex exec` and remote Claude sessions are
  refused computer-use access. Briefs: `codex-brief.md` (full) and
  `codex-brief2.md` (segmentation re-test). Don't run GPU jobs alongside a GUI
  test (the first run broke off at 4 % while another process used the GPU).
- The 3D UI can be checked in Chromium with a stub harness (volume.js + surfaces.js
  with a fake `invoke`); one was used during development but not committed.

## Data

- `/Volumes/samsung/tmv-cryoet-scratch/czp10000/TS_041/`: portal 10000/TS_041 tomogram
  (960×928×500, 13.48 Å, int16), microtubule / membrane / vesicle annotation masks,
  `B_*` crops (x 240–720, y 400–864, z 180–380).
- `/Volumes/samsung/pro-test/amira-126/`: test app, `TS_041_crop.mrc` (256×256×128),
  `TS_041_B.mrc` (+ labels), `TS_041_small.mrc`, `MemBrain_seg_v10_beta.ckpt`
  (sha256 e5236991…, from the official Google Drive link, id 1hruug1GbO4V8C4bkE5DZJeybDyOxZ7PX),
  GUI test results in `ui/` and `ui2/`.
- Nav110 MemBrain-seg output (amax): `/home/lihao/leehow/cryo-et/experiment-E02ai-nav110-polarity-20261014/output/Nav110/`.
- amax (LAN `192.168.31.153`) has the MemBrain-seg venv at
  `/home/lihao/leehow/cryo-et/membrain-generalization-20261008/.venv` for reference runs.

## Known log lines

`No Discrete GPU device found` / `RecvError` / `GPU: Metal backend failed` at
start-up come from GPU probing (the discrete-GPU try fails on a Mac and the app
falls back to the integrated GPU). They are expected.
