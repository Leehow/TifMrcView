# Pro image tabs and layer merging

## Authorized result

Pro opens images in tabs beneath the quick toolbar. Folder thumbnails can be
dragged into the central canvas to turn the current document into an editable
merge in place, retaining its canvas position and scale and using the same tab.
Each layer has visibility, order, position, size, opacity, color, contrast and
its own Z/T selection. Single images stay fixed while stacks change Z. Merge
projects preserve intermediate editing state for reopening; flattened exports
support TIFF, PNG and JPEG. Standard retains its existing window behavior.

The user clarified that editing happens inside Pro, and explicitly requested
saving intermediate work and resuming it later. Projects reference the original
files rather than rewriting raw microscopy data.

## User UX correction, 2026-11-09

The initial UI was rejected as cluttered. The accepted direction now is:

- Always show the two right-edge image/layer icon tabs, with one sidebar pane
  visible at a time. File browsing never shares its scroll area with layer forms.
- Dragging a source into the image keeps the current canvas/view and converts
  the current image tab to a `.tmvmerge` document; do not append another tab or
  fit/reset the view. Further drops add to this composition.
- Image browsing has a collapsible `合成图` group with live composite previews.
- The layer pane follows Photoshop's simple hierarchy: visibility, actual layer
  thumbnails and names, color, blend mode and opacity. Drag/resize on canvas;
  place numeric geometry, intensity and canvas settings in collapsed details.
- Retain saving/reopening editable projects, flattened export and independent Z.

Coordination owner: chat `01a1be8a-a941-7b40-9387-fa5a818dd55d` integrates all
features in `/Users/haoli/leehow/tifview-handoff-integration`, owns canonical
installation and GUI acceptance. This lane may edit only the Pro frontend,
minimal app hooks and its tests/plan. Do not change Rust/Cargo, i18n, XYZ,
denoise/persistence, native builds/signing/installations or the running App.
The current unsaved `合成图 2` must remain open for that owner to preserve.

Scope of screenshot audit: current file/layer sidebar and the drag-to-merge flow.
Confirmed issues: the sidebar mixes file browsing with a long form, the drop
opens a new document and changes the view, and the image icon disappears when
the sidebar is open. The user supplied the persistent icon rail and grouped
file-list references. Adobe's layer panel separates primary opacity/blend
controls and layer thumbnails/visibility from detailed properties:
https://helpx.adobe.com/photoshop/desktop/create-manage-layers/get-started-layers/work-with-the-layers-panel.html

UX revision gates: permanent icon tabs; mutually exclusive panes; in-place drop
with stable view; composite group/preview; compact layer controls; project
roundtrip; source lifetime; Standard unaffected. Native acceptance belongs to
the integration owner after this lane reports `ready-for-integration`.

## Ownership and scope

Base: `4c434e9`, working checkout `/Users/haoli/leehow/tifview`, branch
`codex/denoise-z-nad3d`. Pre-existing dirty denoise, wedge, XYZ, branding and
publication changes belong to other work and must be preserved. Do not commit,
push, publish or deploy. No disposable worktree is created. Keep implementation
in new Pro modules with narrow hooks in app.js, pro.js and desktop.rs.

## Decisions and precedent

- ImageJ's RGBStackMerge retains source channels, LUTs and contrast settings;
  its source requires matching dimensions/depth. This feature instead keeps
  source geometry and independent Z selection for manual merging.
  https://github.com/imagej/ImageJ/blob/master/ij/plugin/RGBStackMerge.java
- napari separates layer opacity, colormap, blending and geometric transforms.
  Use the same separation with normal, additive and screen blending.
  https://napari.org/stable/howtos/layers/image.html
- Retain source image handles per Pro window. Tabs restore their own image
  display/annotation/crop state; closing tabs releases unused source handles.
- Save a versioned `.tmvmerge` project with original paths and layer settings.
  Report missing sources when reopening; never silently omit a layer.

## Execution and acceptance

- [x] Inspect current image/window lifecycle and existing request-order guards.
- [x] Confirm user intent and compare ImageJ and napari primary sources.
- [x] Add Pro source retention, activation, close and project/export commands.
- [x] Add image tabs and per-tab view state without changing Standard behavior.
- [x] Add folder drag/drop, layer editor, transforms and independent Z sliders.
- [x] Add project save/reopen, dirty-close guard and flattened image export.
- [x] Run focused request-order, project, pixel and lifecycle verification.
- [x] Verify the revised UX in browser DOM/canvas, including viewport stability.
- [ ] Integration owner: canonical-App acceptance of the revised UX and model persistence.

## Validation / limitations

Implemented in new Pro modules and small lifecycle hooks. Source cache migration
finished; a different checkout's test held the cache lock briefly. Cargo queued
and resumed normally. Existing dependencies are used offline; no large Internet
download is authorized. macOS hygiene policy was read before App build/launch.

Checks completed so far:

- 14 Node tests: opening races, model save ownership, independent Z, editable
  project roundtrip, immutable saved copies and malformed-project rejection.
- `cargo check --offline --features pro` passed. New RGB TIFF and PNG encoding
  test passed, comparing decoded RGB samples with the source pixels.
- Real Chromium DOM/canvas with a mocked Tauri fixture transport: two pending
  files become two tabs; pointer drag creates a separate two-stack merge;
  independent Z values, color, opacity and geometry survive save/close/reopen.
  Before/after flattened pixel hash: `931897075` both times. Unsaved-close modal
  offers save/discard/cancel and cancelling retains the dirty document.
- Tauri documentation confirms native OS drops conflict with HTML5 drops on
  Windows. Internal thumbnail dragging therefore uses pointer capture while
  leaving native OS file-drop handling enabled.
  https://v2.tauri.app/reference/config/#windowconfig

The initial Pro debug build completed and the old UI was tested in the canonical
App. Native tests covered tabs, source drag, a 2D RGB base with Z7/Z5 stacks,
color/opacity/geometry/Z edits, project saving and TIFF/PNG exports. The initial
UI was subsequently rejected. These results do not validate the revised UX.

The initial installation quit old PID 16567 and replaced its executable. Only
the executable was backed up; no runtime model weights were exported. This was
reported accurately to the user and integration owner. Do not treat the old
executable or corrected MRC as a weights backup. The integration owner confirmed
that the current unsaved `合成图 2` was saved to
`/Volumes/samsung/pro-test/denoise-model-persistence-20261109/preserved/合成图2-before-unified-update.tmvmerge`.
This lane has not operated the App after GUI ownership was transferred.

## Revised UX verification / design QA

Final result: passed for the browser implementation; native acceptance pending
with the integration owner. Source references are the user's two sidebar
screenshots and the current product's image list, icon rail and controls. The
revised layout reuses those controls and the app's existing icon artwork.

Observed checks:

- First drop converted the selected tab in place: tab count stayed 2 and the
  document ID stayed `document-1`. Scale stayed `3.005013020833333`; the image's
  screen origin stayed `[155.19791666666669, 0]` before and after conversion.
- The file pane remains selected after a drop; a `合成图` disclosure group shows
  the composite thumbnail and `.tmvmerge` name. Open and recently saved projects
  remain discoverable after closing/reloading. The recent-project cache is UI
  metadata; the `.tmvmerge` file remains the source of editable state.
- Image/layer panes are mutually exclusive. Both icon tabs remain visible when
  expanded and collapsed; keyboard arrows switch between them.
- Default layer pane shows blend, opacity, color, eye/thumbnail/name rows, add/
  reorder/delete and save/export. Geometry, intensity and canvas details start
  collapsed. A selected lower layer moves/resizes without moving an overlapping
  upper layer. Undo restores its geometry. Image-pane canvas drags only pan.
- A 2D base plus two stacks produces two Z sliders. Independent Z 7 and 2, red/
  green colors, opacity 0.59 and geometry survive save/close/reopen. Before/after
  flattened pixel hash: `4184689833` both times.
- The saved project was reopened after a WebView reload using the persisted
  sidebar entry. Dirty-close cancel preserves the draft; discard closes it.
- The 1024×768 viewport has no horizontal body overflow. The export menu fits
  within the viewport and PNG/TIFF actions reach `save_merged_image`.
- Standard does not load the Pro tabs, rail or layer panel.
- 17 focused Node tests passed, including new conversion/crop/reframing cases;
  syntax and whitespace checks passed.

Evidence limits: browser tests use synthetic data and a mocked Tauri transport.
The initial mock's plugin-market return type was corrected; the final run had
no feature exceptions. Chromium reported two ResizeObserver loop notifications
while the emulated viewport settled; their count stayed at two during ordinary
pane use.
This is recorded as a non-blocking browser diagnostic, not hidden by a handler.
Native rendering, native dialogs, OS open routing and training/model retention
must be accepted together in the integration build. No native build, signing,
installation, restart or GUI mutation was done by this lane during the UX
revision.

Screenshots: `/tmp/tmv-pro-ux-final-images.png`,
`/tmp/tmv-pro-ux-final-layers.png`, `/tmp/tmv-pro-ux-final-1024.png`.
Integration baseline:
`/var/folders/wn/8ly53x4n6sq3jkvkptvtkrsm0000gn/T/tmv-pro-ux-20261109-p8klvt_i`.

Ready-for-integration: frontend revision complete. Freeze these seven files:
`ui/pro/documents.js`, `ui/pro/layers.js`, `ui/pro/documents.css`,
`ui/pro/merge-model.js`, `ui/app.js`, `scripts/pro-layers.test.mjs`, and this plan.
`ui/pro/pro.js` and all Rust/Cargo/i18n/XYZ/denoise/model-persistence sources are
unchanged from the initial handoff snapshot.

## Integration follow-up: source-tool suspension and Retina zoom

The integration owner found a native boundary defect: a source NAD panel stayed
open over a `.tmvmerge` document, its export still addressed the hidden source,
and closing it triggered source rendering that overwrote merge zoom labels.
The owner also requested consistent Retina zoom: source labels use CSS scale ×
DPR, whereas the merge previously displayed CSS scale alone.

The narrowly scoped fix suspends source processing surfaces in documents.js
before publishing the document-change event. It stores visibility/inert state,
hides source panels, rejects their UI events, and clears the old shared menu.
Late-mounted or asynchronously reopened source panels stay suspended. Returning
to an image restores those states; normal source metadata rules still apply
(for example, no spectrum pane for an image without spectrum metadata).
It never calls the tools' close/reset/cancel handlers or clears model data.
Global settings/help/plugin dialogs and merge menus remain available.

app.js guards source drawing, render requests, in-flight frame/overview results,
load progress and pixel status while a merge is active. Source canvas resizing
does not clamp its saved view while hidden. Opening a new source from a merge
explicitly restarts its display after ownership changes. Merge zoom labels and
the `1` shortcut now match the source DPR convention; serialized CSS geometry
and view scale are unchanged.

Precedent checked: HTML's inert subtree contract and WebKit's shipping support
confirm focus/input suspension without destroying DOM or computation state.
https://html.spec.whatwg.org/dev/interaction.html
https://webkit.org/blog/12669/new-webkit-features-in-safari-15-5/

Follow-up verification:

- 21 focused Node cases pass, including late source frame/overview/progress
  completion and DPR=1/2 zoom/physical-100% behavior.
- Browser fixtures: N2V and missing-wedge training promises both finish while
  the merge is active; no cancel commands occur. The source token, Z and view
  survive the switch, and trained-result text remains on return. This validates
  frontend continuity with mocked computation, not real weight persistence.
- Hidden source export/train/close actions dispatch no IPC. All 12 inspected
  source panel/workspace/FFT/spectrum/3D roots become hidden/inert; a newly
  appended panel and a reopened panel are also suspended. Merge export still
  reaches `save_merged_image`, and its menu survives ordinary merge edits.
- A held source render released after switching cannot change the merge's zoom
  or status. A new source opened from the merge renders successfully. Standard
  still renders without loading the Pro document UI; no browser feature errors.
- DPR=2 in-place conversion retains tab ID/count, screen origin
  `[178.40000000000003, 17.40156250000001]` and zoom `572%`; `1` sets CSS scale
  `0.5` and both zoom labels to `100%`.

Ready-for-integration again. Exact baseline is the seven-file frozen source in
`/Volumes/samsung/pro-test/tabs-layers-20261109/ux-final-20261109/source`.
The new incremental snapshot/patch is in
`/Volumes/samsung/pro-test/tabs-layers-20261109/source-panel-fix-20261109`.
No native build, signing, installation, restart or App GUI operation occurred
in this follow-up. The integration owner retains final native acceptance.

## User refinement: single layer, fitted drops and automatic saving

The next user turn authorizes: show the current image in the layer pane even
with no additional layers; fit and center dropped images to the base canvas
with source aspect ratio intact; use a Photoshop-style transform border with
eight corner/edge handles; automatically persist editing operations and remove
the manual project-save button. Delete and Backspace delete the selected layer,
support undo and remain ordinary text editing keys in inputs.

Baseline: `source-panel-fix-20261109/source`, identical to all seven frontend
files in integration HEAD `ebcabbb` at intake. This lane owns only those seven
files. The integration owner confirmed installed 0.7.173 and owns backend,
packaging and the live App. Do not touch/restart the App; preserve source/merge
and training/model state. No additional dependency or native build is needed in
this lane.

Decisions: displaying the single source layer is passive; its first layer edit
promotes the tab in place. New layers fit the canvas without stretching or
moving the base/view. Use a thin blue transform border and white square handles,
preserve the opposite anchor, and let Shift temporarily toggle aspect locking.
No rotation or skew is added. Autosave immediately keeps a recovery draft,
debounces continuous edits for 350 ms and serializes disk writes. Flush the final
revision before switching or closing; only successful writes clear dirty state.
Errors keep the draft, show status and retain the open document. Next edits or
Ctrl/⌘S retry. The footer keeps export and compact automatic-save status.

The integration owner implements/tests the agreed minimal backend contract:
`create_merge_project({token, project}) -> absolute path string`. The source
token must belong to this window. Allocate a non-overwriting `.tmvmerge` beside
the source (inside a directory source), falling back to
`app_data_dir()/merge-projects`. Fully publish before returning and register the
allowed save path. Later writes use existing atomic `save_merge_project`.

Precedent: Adobe documents proportional corner scaling and Shift as its
temporary toggle; Krita also exposes corner and edge handles. Their Shift
defaults differ, so follow Adobe as requested. Krita documents recovery across
unexpected exits; this feature saves after edits as the user explicitly asked.
https://helpx.adobe.com/photoshop/desktop/create-manage-layers/transform-manipulate-layers/scale-layers-in-photoshop.html
https://docs.krita.org/en/reference_manual/tools/transform.html
https://docs.krita.org/en/user_manual/autosave.html

Verification complete in this lane:

- 28 focused Node cases pass. Added size/aspect/anchor coverage for all eight
  handles and large/small/portrait drops; serialized saving, in-flight edits,
  failure/retry, allocation-name collisions and an empty-draft source anchor.
- Browser DOM/canvas: a source tab shows its current layer, thumbnail and
  controls without converting merely to display it. First color edit converts
  in place and preserves the entered value. Only export remains in the footer.
- Real pointer dragging a 1280×720 thumbnail into a 320×240 base creates a
  centered 320×180 layer at (0,30). Document ID `document-1`, tab count 1, CSS
  scale `3.005013020833333` and screen origin `[155.19791666666669,0]` all remain
  unchanged. A 64×256 source fits to 60×240 at (130,0).
- Corner and side handles work with opposite anchors fixed; Shift permits
  independent width changes. Other layers stay unchanged. Undo works after
  leaving a text field for a canvas gesture. Delete and Backspace remove the
  selected layer and undo restores it; inside an input they only edit text.
- Deleting the single source layer before the first save still creates exactly
  one project, retains its source handle and permits undo. Allocated names with
  numeric suffixes are reflected in the tab/project and later saves.
- No save picker is called. A held write remains dirty/`自动保存中…`; a switch
  waits until the newer opacity 0.72 and X=143 have both reached the saved copy.
  Simulated disk failure displays an error and blocks close/switch. Reloading
  restores the latest failed opacity 0.63 from the existing composite group;
  the full recovered project equals the pre-reload draft.
- Automatic save/close/reopen preserves the full project and flattened RGB
  hash `3038652657`. No close confirmation appears. PNG/TIFF exports still reach
  `save_merged_image`. DPR=2 `1` sets CSS scale 0.5, labels 100%, and saves 0.5.
- 1024×768 has no horizontal body overflow and export stays visible. Standard
  renders its source without Pro tabs/layers. No feature exceptions occurred;
  two ResizeObserver loop diagnostics appeared while viewport emulation settled
  and did not increase during ordinary pane switching. Do not suppress them or
  treat this fixture run as native acceptance.

Ready-for-integration. Freeze all seven files against
`/Volumes/samsung/pro-test/tabs-layers-20261109/source-panel-fix-20261109/source`.
New snapshot/patch/evidence:
`/Volumes/samsung/pro-test/tabs-layers-20261109/refinement-autosave-20261109`.
Browser transport is mocked Tauri with synthetic images and mocked file writes;
the backend owner separately reported five filesystem checks for actual create/
atomic-update behavior. Native build/installation/GUI acceptance remain theirs.
This lane did not touch the running App, build/sign/install, Rust, XYZ or models.

## Final addition: calibrated base scale bar

The user additionally requested the scale bar to follow the base image. The
source conversion had omitted its calibration and the compositor had no scale
bar drawing, which explains the missing bar. The fix keeps an optional per-layer
`scaleBar` record in v1: pixel calibration (including manual values), units,
physical bar length/label, crop region and image-relative text/stroke/margins.
Draw only the current bottom layer's bar, above the merged channels in neutral
white, using the same code for preview, thumbnails and flattened export. Moving
or resizing that layer transforms the bar while preserving physical length.
The existing toolbar toggle works and undo snapshots copy bar metadata deeply.

Legacy v1 projects remain readable. When the optional field is absent and the
source file has valid pixel metadata, populate it from that source and autosave.
Never invent unknown calibration. The integration owner verified the live
source's 4.5528 Å calibration is explicitly from file metadata and preserved the
old project before migration; no old in-memory manual calibration is needed for
that observed case. New manual calibration is retained in the project.

ImageJ keeps scale bars as calibrated overlays and scales overlays with images:
https://imagej.net/ij/developer/source/ij/plugin/ScaleBar.java.html
https://github.com/imagej/ImageJ/blob/master/ij/plugin/Scaler.java

Final follow-up evidence:

- 33 Node cases now pass, including bar/crop/move/resize calibration, project
  validation/roundtrip and legacy readability, deep undo copies, recovery-path
  registration, and detecting a silently failed localStorage write.
- Browser: a 20 nm bar moves with the base, shrinks from width 40 to approximately
  36 image pixels with a 10% image resize (native pointer precision), and keeps
  physical length 20 nm. Flattened bar-center pixel at 4× rendering is white
  `[255,255,255,255]`; toolbar hide/undo works without changing the hidden source's
  scale-bar state. The rendered screenshot visibly contains `20 nm`.
- A legacy project missing every scaleBar field is opened, migrated from source
  metadata and written with the bar. A strict mock resets the permitted-path
  registry on WebView restart: recovery calls `read_merge_project` first, then
  `save_merge_project`, with no new create. Its full draft, opacity 0.82 and bar
  are retained. Bar-inclusive close/reopen pixel hash is `1853937629` both times.
  PNG export reaches the ordinary flattened-image writer; Standard remains intact.

Final app.js ownership boundary: another task concurrently added filled XYZ
annotation hit-testing and `scripts/xyz-interaction.test.mjs` in this checkout.
Those changes are preserved and excluded. The handed-off app.js is projected
from the previous frozen baseline plus only this lane's `sourceChanged` draw
hook. Apply its incremental hunk, do not replace other owners' app.js changes.
All other frozen files contain only this lane's work. No commit/push or native
build/install/App operation was performed in this lane.
