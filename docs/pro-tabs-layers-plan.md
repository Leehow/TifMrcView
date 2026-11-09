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
