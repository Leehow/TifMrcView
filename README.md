**English** | [简体中文](README.zh-CN.md) | [Deutsch](README.de.md) | [日本語](README.ja.md) | [Español](README.es.md) | [Français](README.fr.md) | [Русский](README.ru.md)

# TifMrcView

A small, fast and convenient viewer for microscopy and electron microscopy images: TIFF / OME-TIFF / ImageJ, MRC for cryo-EM and cryo-ET, and through plugins the confocal formats of Zeiss (CZI, LSM), Leica (LIF, LOF), Nikon (ND2), Olympus / Evident (OIB, OIR) and Imaris (IMS), the electron-microscopy formats of Gatan DigitalMicrograph (DM3, DM4), Thermo Fisher / FEI (TIA SER, Velox EMD, EER) and TVIPS, plus Zeiss SmartSEM. Draw lines, boxes and circles with measurements in physical units, switch between XY / XZ / YZ views, and view multichannel fluorescence with per-channel toggles and colours. The installer is only a few MB, yet it opens multi-GB stacks instantly. Runs on macOS, Windows and Linux, and can be extended with plugins. Built with Rust + Tauri 2.

[![Download](https://img.shields.io/badge/download-tifmrcview.com-2f6fed)](https://tifmrcview.com/tifmrcview/#download)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)
[![GitHub stars](https://img.shields.io/github/stars/Leehow/TifMrcView?style=social)](https://github.com/Leehow/TifMrcView)

**[Download](https://tifmrcview.com/tifmrcview/#download)** · **[Plugin market](https://tifmrcview.com/tifmrcview/plugins/)** · [Website](https://tifmrcview.com/tifmrcview/)

Author: **Hao Li** · [ORCID](https://orcid.org/0009-0004-2696-3267) · [@Leehow](https://github.com/Leehow) · [lihao9x@iccas.ac.cn](mailto:lihao9x@iccas.ac.cn)

> If you find TifMrcView useful, please give it a ⭐ on GitHub — it really helps.

## Screenshots

<table>
  <tr>
    <td width="50%"><a href="site/shots/mrc-tomogram-line.webp"><img src="site/shots/mrc-tomogram-line-1200.webp" alt="Cryo-ET tomogram in MRC with a length measurement"></a><br><sub>Cryo-ET tomogram (MRC) with a length measurement and the annotation tools.</sub></td>
    <td width="50%"><a href="site/shots/mrc-xz-view-box.webp"><img src="site/shots/mrc-xz-view-box-1200.webp" alt="XZ side view with a box"></a><br><sub>XZ side view with a box showing width × height.</sub></td>
  </tr>
  <tr>
    <td width="50%"><a href="site/shots/czi-airyscan-measure.webp"><img src="site/shots/czi-airyscan-measure-1200.webp" alt="Zeiss CZI Airyscan Z-stack with measurement"></a><br><sub>Zeiss CZI Airyscan Z-stack with a measurement and scale bar.</sub></td>
    <td width="50%"><a href="site/shots/tiff-multichannel.webp"><img src="site/shots/tiff-multichannel-1200.webp" alt="3-channel fluorescence TIFF"></a><br><sub>3-channel fluorescence TIFF with per-channel toggles.</sub></td>
  </tr>
</table>

## Features

- Supported formats: TIFF (multi-page, strips/tiles, LZW/Deflate/PackBits/JPEG compression, 1–7 bit grayscale such as 1-bit binary and 4-bit, and 8/16/32-bit integer and float; uncommon depths such as 12-bit give a clear error message), OME-TIFF (reads C/Z/T dimensions, channel names and colours, pixel size), ImageJ hyperstacks (including contiguous files larger than 4 GB), palette TIFF (such as Zeiss SEM images), MRC/MRC2014 (modes 0/1/2/3/4/6/12/101, `.mrc` `.mrcs` `.rec` `.st` `.map`; mode 0 files without an IMOD flag are treated as unsigned bytes or not based on the min/max in the header), and, with the Zeiss plugin, Zeiss CZI (LSM / Airyscan etc. saved by ZEN: uncompressed, Zstd and JPEG sub-blocks, 8/16/32-bit grayscale and colour, C/Z/T dimensions, tiled mosaics, channel names and colours, pixel size). The same plugin opens Zeiss LSM; further format plugins, one per vendor, open Leica LIF / LOF (`lasx`), Nikon ND2 (`nis`), Olympus OIB / OIR (`fluoview`), Imaris IMS (`imaris`), Gatan DM3 / DM4 (`gatan`), Thermo Fisher TIA SER, Velox EMD and EER movies (`thermo`, which also reads the settings in Thermo Fisher SEM/FIB TIFFs and the FEI extended header of MRC files) and TVIPS movies (`tvips`, which also reads EM-MENU TIFF settings); a file that holds several images (a LIF project, ND2 positions) gets a picker next to the file position. `.rec` `.st` `.map` and the plugin formats only appear in "Open with" and are not set as the default app
- Also opens PNG / JPEG / BMP / GIF / WebP
- Z slider and playback; a T slider as well when there is a time dimension
- Volume EM (FIB-SEM, serial sections, SBF-SEM): a folder of slice images opens as one Z stack (drop the folder on the window, or the folder button / Ctrl/⌘+Shift+O); the slices can be in any format the app opens and are read only when needed. When a plugin recognises an acquisition project file nearby (Thermo Fisher Auto Slice & View `ProjectData.dat` with the `thermo` plugin), the thumbnail list of that folder gets an extra stack entry with the slice thickness as Z spacing; otherwise the Z spacing can be typed in under "Scale bar"
- Multichannel colour composite: tick a box to show/hide a channel, click the name to adjust that channel's contrast, click the dot to change its colour
- Brightness / contrast / gamma; drag on the histogram to pick the display range directly; automatic per-slice contrast by default (0.1% saturation)
- The ⓘ toolbar button shows image info: format, size, pixel size, and acquisition parameters read by plugins (searchable and copyable). The Zeiss plugin also reads all parameters of a Zeiss SmartSEM image (voltage, working distance, magnification, detector, stage position, vacuum, …) and shows a scale bar from the calibrated pixel size, and for CZI the microscope, objective, lasers, detectors, pinhole, scan and Airyscan processing parameters from a CZI
- A preview strip on the right lists the images in the same folder (collapsible) with file name, format and size
- Icon toolbar at the top (hover for names): save image, undo, select, move, measure, pen (the arrow beside it opens a menu: free / line / arrow / box / circle), text, crop, scale bar, brightness/contrast
- Measurements show real lengths using the pixel size; boxes and circles always show width × height on the image (diameter for a true circle)
- Select tool (default): drag empty space to pan; click an annotation to drag it, drag the handles on its border to resize, drag the endpoints of a measurement line to re-measure; Delete removes; colour and line width can be changed
- Annotations are stored per Z slice and saved automatically about 2 seconds after drawing to `<file name>.annot.json` next to the image, and read back the next time you open it
- Crop: frame a region, then double-click or press Enter to view only that area; Esc or Backspace takes it back
- An IMOD model (`.mod` / `.fid`) with the same name is overlaid automatically and can be toggled per object
- Save image: PNG at the original resolution (only the cropped area after cropping), including the visible annotations, model and scale bar
- Images with Z can switch between XY / XZ / YZ views at the top (annotations drawn in side views are not saved)
- Drag the inner edge of the left toolbar and the right preview strip to resize them
- Mouse wheel to zoom, drag to pan, double-click to fit the window; the status bar shows the pixel coordinates and raw value
- Drag a file into the window to open it; double-clicking another file in the same folder opens it in the existing window, while a file from another folder opens a new window
- The "Open folder" button at the top of the preview strip reveals the current image in Finder / Explorer
- With any other tool active, right-click on the image to switch back to the select tool
- An annotation can be shown on a single slice or span a range of Z; after selecting one, change its Z at the top, and the Z slider marks the slices that have annotations

## Plugins

Plugins can be switched on/off, installed, uninstalled and checked for updates in the toolbar's "Plugins" menu. Except for the languages, every plugin is installed separately: click "Plugin market" to open https://tifmrcview.com/tifmrcview/plugins/ in the browser and download a plugin package (.zip), then click "Load plugin…" and pick it. Installing a newer version of the same plugin simply replaces the old one. The plugin sources are in the repository under `plugins/<id>/` (`plugin.json` + `index.js`; currently `recorder`, `zeiss`, `lasx`, `nis`, `fluoview`, `imaris`, `gatan`, `thermo` and `tvips`); installed plugins live in an `<id>/` subfolder of the plugin folder.

One-click install (0.5 and later): click **Install** on a plugin card in the market, or in the "Plugin market" list inside the Plugins menu (which also offers "Update to x.y.z" when a newer version is out). The website hands the app a link of the form `tifmrcview://install-plugin?id=<id>` (optionally `&version=<x.y.z>`); the app then shows the plugin's name and description and installs nothing until you click **Install**. It only installs plugins listed on the official market (`plugins.json` next to the update feed), downloads them over HTTPS and checks size and SHA-256 before installing. Downloading the zip and using "Load plugin…" still works.

Format plugins: a plugin package can also add file formats and metadata readers as a sandboxed WebAssembly module, written for example in Rust with the SDK in `plugins/sdk`. Every vendor gets one such plugin (`zeiss` opens CZI and LSM and reads SmartSEM settings), so the core contains no vendor format code: without the Zeiss plugin, .czi files do not open and the app says which plugin to install. A module only sees the file being opened, runs with memory and time limits, and describes each page as a list of file chunks that the app decompresses and assembles natively, in parallel. The ABI, the limits and an example are in [docs/format-plugins.md](docs/format-plugins.md).

Video recorder plugin: in the "Tools" panel under "Record video (MP4)" choose Z or T, the first and last frame, frame rate, region (whole image / selection / current view), size and quality. The resulting H.264 MP4 includes the current contrast, channel colours, annotations and scale bar.

The interface language is also a built-in plugin (`ui/plugins/i18n.js`): it supports 中文, English, Deutsch, 日本語, Español, Français and Русский, follows the system language by default, and can be changed with the drop-down in the toolbar. It replaces the known Chinese texts in the interface with the chosen language, so other plugins and messages are translated as well; to add new text, add a row to `WORDS` (fixed text) or `PATTERNS` (text containing numbers or file names). Switching this plugin off returns to Chinese.

Writing your own plugin: put a `.js` file in the plugin folder and click "Reload". To distribute it, pack `plugin.json` (`id`, `name`, `version`, `description`, `main`, `apiVersion`, `minAppVersion`, optionally `author`, `i18n`, `details`; see `plugins/recorder/plugin.json` for the format) together with the script into a zip (files at the zip root). A minimal example:

```js
TifMrcView.register({
  id: "hello",
  name: "Hello plugin",
  version: "1.0.0",
  description: "Shows the current slice in the tools panel",
  activate(app) {
    const box = app.ui.addToolSection("Hello");
    app.on("slice", ({ z }) => (box.textContent = `Slice ${z + 1}`));
  },
});
```

`app` provides: `state()` (current file, Z/T, size, selection, pixel size), `on("open" | "slice" | "render", fn)`, `setZ` / `setT` / `open(path)`, `renderRegion(region, {z, t, size})` (renders to a canvas with the current display settings, including annotations and scale bar), `ui.addToolSection` / `ui.addToolbarButton` / `ui.addToolbarSelect` / `ui.toast` / `ui.busy`, `dialog.save`, `fs.write`, `settings.get/set`, and `native(method, args, bytes)` to call the Rust side. The full definition is in `ui/plugin-host.js`. Plugins that need native code (such as the H.264 encoder used by the recorder) implement `NativePlugin` in `src-tauri/src/plugins/`; the native part is compiled into the main program but only enabled once a plugin package with the same id is installed. A Cargo feature controls whether the recorder is compiled in (`recorder` is on by default; `--no-default-features` removes it and makes the installer about 1 MB smaller).

Plugins have the same permissions as the main program: only install plugins you trust.

## Automatic updates

A few seconds after start, the app silently checks `https://dl.tifmrcview.com:9443/tifmrcview/latest-direct.json`, falling back to `https://tifmrcview.com/tifmrcview/latest.json` if the direct host fails. When a new version exists, a banner appears at the top; click "Update now and restart" to download it, verify the signature, install and restart. Update packages are signed with a Tauri minisign key; the public key is in `tauri.conf.json`.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| ↑ / ↓ | Previous / next file |
| ← / →, PageUp / PageDown, `,` / `.` | Change Z |
| Home / End | First / last slice |
| Space | Play / pause Z |
| Ctrl + wheel (⌘ + wheel on macOS), Shift + wheel | Change Z |
| S / V / M / P / L / A / R / E / X / C | Select / move / measure / pen / line / arrow / box / circle / text / crop |
| Shift (while drawing a box or circle) | Square / true circle |
| Shift (while drawing a line or arrow) | Snap to 45° |
| Delete / Backspace | Delete the selected annotation; undo the crop when nothing is selected |
| Esc | Cancel the annotation being drawn or just drawn, deselect, undo the crop |
| Enter, double-click the crop box | Crop |
| Ctrl/⌘ + Z | Undo (drawing, measuring, moving, deleting and cropping can all be undone) |
| Ctrl/⌘ + Shift + Z, Ctrl/⌘ + Y | Redo |
| Ctrl/⌘ + S | Save a PNG (next to the image by default, or in the save folder chosen in the Tools panel) |
| H | Show / hide annotations |
| I | Show / hide the IMOD model |
| T | Open / close the "Tools" panel |
| B | Show / hide the preview strip |
| F, double-click (move tool) | Fit to window |
| 1 | Original size (1:1) |
| + / =, - | Zoom in / out |
| Ctrl/⌘ + O | Open file |

## Why it is fast

- Files are memory-mapped, and the strips/tiles of a page are decompressed in parallel on all CPU cores
- After a file is opened, the other Z slices are pre-decoded into memory in the background, nearest to the current slice first (using about a quarter of physical memory, 512 MB–8 GB), so dragging the slider shows images immediately
- Only the visible region is rendered, and only at screen resolution; the 8-bit RGBA is passed to the UI as binary, so zooming and panning a 4000×4000 image stays smooth
- Thumbnails are generated in a separate thread pool so they do not compete with the current image for CPU
- The UI is plain HTML/JS with no frontend framework; release builds use LTO and strip, and the installers are about 4–11 MB (the Linux AppImage bundles WebKit, so it is about 84 MB)

## Install

Download from the official site, **[tifmrcview.com/tifmrcview](https://tifmrcview.com/tifmrcview/#download)**. New versions come out often and are published only there (GitHub Releases are not used); the app also updates itself from the same site:

- **macOS** (10.15 or newer): `TifMrcView_x.y.z_universal.dmg` (Intel and Apple silicon), drag it into "Applications". Because the app has no Apple developer signature, the first launch is blocked: go to "System Settings → Privacy & Security" and click "Open Anyway", or run `xattr -dr com.apple.quarantine /Applications/TifMrcView.app` in a terminal. On first start TifMrcView makes itself the default app for .tif/.tiff/.mrc.
- **Windows**: `TifMrcView_x.y.z_x64-setup.exe`, double-click to install (no administrator rights needed). The installer registers the .tif/.tiff/.mrc associations; if you have manually chosen another program for .tif before (for example "Photos"), Windows does not let software change that silently — click "Set as default" in TifMrcView, which opens the system settings page, and pick TifMrcView there.
- **Linux**: `.deb` for Debian/Ubuntu, `.rpm` for Fedora and similar, `.AppImage` for other distributions. On first start it sets itself as the default app via `xdg-mime`.

## Build from source

You need Rust (stable) and Node.js 18+. On Linux you also need:

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libxdo-dev libssl-dev
```

Then:

```sh
npm install
npm run dev      # development mode
npm run build    # build the installer for the current OS, in src-tauri/target/release/bundle/
```

To debug decoding, use the bundled small tool; it prints metadata, per-channel pixels and decode/render timings:

```sh
cd src-tauri && cargo run --release --example probe -- /path/to/file.tif
```

## Releases

Version numbers are generated automatically: the major.minor in `tauri.conf.json` (currently 0.3) plus the total number of commits, e.g. `0.3.57`; when a `v1.2.3` tag is pushed, the tag's version is used.

Every push to `main` makes GitHub Actions build packages on macOS / Windows / Linux; the installers are in the run's Artifacts. There are two ways to publish to the download site:

- Push a tag starting with `v`: `git tag v0.3.0 && git push origin v0.3.0` uploads to the download site and creates a GitHub Release;
- or run the build manually in Actions with "publish to the download site" ticked, which publishes with the automatic version number.

When publishing, CI puts the installers and update packages in the download site's `<version>/` directory and finally updates `latest.json` and `latest-direct.json` (for auto-update) and `downloads.json` (for the download page). The download page and the plugin market (`site/`, `plugins/`) are also deployed by their own workflow (`.github/workflows/site.yml`) whenever they change on `main`. Configure these in the repository under Settings → Secrets and variables → Actions:

| Name | Type | Description |
| --- | --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Secret | Private key for signing update packages (if lost, you can no longer push updates to installed users) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Secret, optional | Password of that key, if it has one |
| `DEPLOY_SSH_KEY` | Secret | SSH private key of the download-site deploy user |
| `DEPLOY_HOST` / `DEPLOY_USER` | Secret | Host and user name of the download site |
| `UPDATE_BASE_URL` | Variable, optional | Default `https://tifmrcview.com/tifmrcview` (Cloudflare fallback) |
| `DIRECT_BASE_URL` | Variable, optional | Default `https://dl.tifmrcview.com:9443/tifmrcview` (DNS-only direct host) |
| `DEPLOY_PATH` | Variable, optional | Default `/var/www/tifmrcview` |
| `CF_PURGE_TOKEN` | Secret, optional | Cloudflare API token with Zone → Cache Purge; the deploy purges the cache after uploading |

Both hosts serve the same deploy directory: `<version>/…`, the three JSON feeds, and `plugins/`. The app tries the direct host first for updates and plugin listings/installs, then Cloudflare on network/HTTP errors; plugin integrity errors stop the install. Browser navigation to the market stays on the main site. The download page keeps relative links unless a 3-second probe of the direct host succeeds; that host must allow cross-origin JSON requests. Port 9443 may be blocked on some networks.

When running `scripts/make-release.mjs` locally, set `DIRECT_BASE_URL` or pass a final `[direct-base-url]` argument after `[notes]` to also write `latest-direct.json`. Without it only the original feeds are written; both updater feeds use the same package signatures.

## Project layout

| Path | Contents |
| --- | --- |
| `src-tauri/src/formats/` | Decoders: `tiff.rs`, `mrc.rs`, `raster.rs` (PNG/JPEG/…), `mod.rs` |
| `src-tauri/src/` | `lib.rs` (commands, window/file handling), `render.rs`, `reslice.rs` (XZ/YZ views), `image.rs`, `imod.rs` (IMOD models), `mem.rs`, `assoc.rs` (default-app registration), `update.rs` |
| `src-tauri/src/plugins/` | Native plugin parts (`recorder.rs`, `mp4.rs`), plugin packages (`package.rs`) and the WebAssembly host for format plugins (`wasm.rs`, `recipe.rs`, `registry.rs`) |
| `src-tauri/examples/probe.rs` | The decoding probe tool |
| `ui/` | Plain HTML/JS frontend: `index.html`, `app.js`, `style.css`, `plugin-host.js`; built-in language plugin in `ui/plugins/i18n.js` |
| `plugins/<id>/` | Sources of the installable plugins (`recorder`, and the format plugins `zeiss`, `lasx`, `nis`, `fluoview`, `imaris`, `gatan`, `thermo` and `tvips`, one per vendor, which are Rust crates built to WebAssembly) and the plugin SDK (`sdk`) |
| `scripts/` | `pack-plugins.mjs` (builds plugin zips and the market index), `make-release.mjs` |
| `site/` | The website and plugin market, with screenshots in `site/shots/` |
| `packaging/linux/` | Linux packaging files (MIME type for .mrc) |
| `.github/workflows/` | `build.yml` (build and release), `site.yml` (site and plugin market deploy) |

## Translations

The README is available in several languages (see the language line at the top). **The English README (`README.md`) is the source of truth**; the other files keep exactly the same sections in the same order. When you change `README.md`, please update the translations too, or open an issue if you cannot.

## Author

<!-- author -->
Created and maintained by **Hao Li** ([@Leehow](https://github.com/Leehow)), Institute of Chemistry, Chinese Academy of Sciences. ORCID: [0009-0004-2696-3267](https://orcid.org/0009-0004-2696-3267)
<!-- /author -->

## Citation

If TifMrcView helps your research, please cite it. GitHub shows the citation from [CITATION.cff](CITATION.cff) under "Cite this repository" on the repository page.

## License

This project is open source under the [Apache License 2.0](LICENSE).
