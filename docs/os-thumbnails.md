# File-manager thumbnails and previews

The OS helpers render middle Z / first T, composite every channel, and use the
viewer's automatic display range. The originals are never rewritten.
`thumb::thumbnail_rgba` removes the render protocol header and returns plain
RGBA8; `thumbnail_png` encodes that result. `formats::open_pixels` skips optional
metadata enrichment. Missing/disabled format plugins fail normally.

The headless command runs before Tauri and before single-instance handling:

```sh
tifmrcview --thumbnail input.tiff output.png --size 256
```

Size defaults to 256, with a supported range of 1–2048. Errors exit 1, success
exits 0. Paths are native OS paths (no UTF-8 requirement for the CLI). The C ABI
requires UTF-8 paths and valid output pointers; free successful allocations once
with `tmv_free(pointer, length)`. Its return codes are 0 success, 1 error, 2 panic.

## Bounds and plugin discovery

The helper passes a 16 MiB decoded-page cache to `OpenImage`, never calls
`prefetch`, and rejects required channel planes above 128 MiB before decoding.
The renderer retains the selected planes while compositing, so cache size alone
would not be a memory limit. TIFF/MRC use mappings: mapping a 4 GiB stack does not
read the whole stack. One full selected plane must still be decoded. The 128 MiB
limit is a pixel allocation admission limit, not a total RSS guarantee: decoder
scratch buffers, metadata, WASM memory and the renderer add overhead. Existing
WASM limits/fuel also apply. Very large individual planes fail gracefully rather
than being sampled by a new decoder implementation.

Both the app and helpers use `plugins::paths::user_dir()`:

- macOS: `~/Library/Application Support/com.leehow.tifmrcview/plugins/`.
  Account-home lookup avoids Quick Look's container HOME.
- Windows: roaming AppData (`dirs::data_dir()`), then
  `com.leehow.tifmrcview/plugins/`, matching Tauri app_data_dir.
- Linux: `$XDG_DATA_HOME/com.leehow.tifmrcview/plugins/`, or
  `~/.local/share/com.leehow.tifmrcview/plugins/`.

Helpers only read installed plugin packages; they do not install/download any.
The registry is configured once per helper process. Persistent helpers may need
a cache/process restart after plugin installation or an enable/disable change.

## macOS

`scripts/build-quicklook.sh` builds the `tmv-thumbnail-ffi` static library with
Tauri default features off, compiles Swift with Command Line Tools, generates
Info.plists from the app's UTI associations, and signs both extensions with the
sandbox entitlements. Output is `src-tauri/target/quicklook.noindex/`.

- `TmvThumbnail.appex`: QLThumbnailProvider, macOS 10.15+ (arm64 requires macOS 11).
- `TmvPreview.appex`: data-based QLPreviewProvider returning PNG, macOS 12+.

The app associations now export `com.leehow.tifmrcview.mrc-alternate` for
`.rec/.st/.map`. Extensions declare all app UTIs, public.tiff, and the core raster
UTIs. QLThumbnailMinimumDimension is zero. The sandbox has only the plugin
folder's home-relative read exception; Quick Look supplies access to the input.
Ad-hoc codesign accepts and retains these entitlements locally. System execution
and access to the real plugin folder still require installed-app testing.

`build.beforeBundleCommand` calls `scripts/build-thumbnails.mjs` before Tauri
bundles/signs/archives. `bundle.macOS.files` destinations are relative to
**Contents**, so `PlugIns/TmvThumbnail.appex` lands in `Contents/PlugIns/`.
The Swift build signs nested code first; Tauri's outer signing is non-recursive
and should retain those signatures. Check the final packaged app, including
entitlements, when exercising the installer/update pipeline.

```sh
# Native architecture, no SDK/target downloads:
scripts/build-quicklook.sh
# CI has both targets installed; both Rust and Swift slices are combined:
scripts/build-quicklook.sh universal-apple-darwin
# Normal complete package pipeline (also used by CI):
npx tauri build --bundles app
```

After installing the resulting app at the canonical `/Applications/TifMrcView.app`
(do not register a disposable build copy), use:

```sh
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f /Applications/TifMrcView.app
pluginkit -mAvvv -p com.apple.quicklook.thumbnail
pluginkit -mAvvv -p com.apple.quicklook.preview
qlmanage -r && qlmanage -r cache
qlmanage -t -s 256 -o /tmp /absolute/path/file.tiff
qlmanage -p /absolute/path/file.mrc
codesign --verify --deep --strict /Applications/TifMrcView.app
codesign -d --entitlements - /Applications/TifMrcView.app/Contents/PlugIns/TmvThumbnail.appex
```

Also inspect Finder icons, Gallery/Space, a vendor file with its plugin installed,
and `.rec/.st/.map` UTIs (`mdls -name kMDItemContentType file.rec`). Registering
public.tiff does **not** establish that macOS chooses this provider over Apple's
built-in TIFF generator. If Finder still chooses Apple's handler, system handler
selection remains unresolved; CLI output alone cannot prove a Finder fix.
No global Quick Look/LaunchServices preferences are changed by this code.

## Windows

`tmv-thumbnail-win` is an in-process COM DLL with IThumbnailProvider and
IInitializeWithStream. Stream data is copied through a 1 MiB buffer to an owned
temporary directory, capped at 512 MiB. Larger files are skipped. Stream names
are optional; anonymous streams try core/enabled-plugin extensions against hard
links to the same snapshot. Files with a supported named extension use just that
decoder. Temporary data is deleted with the COM object's lifetime (a forcibly
terminated shell host can leave temporary data behind).

RGBA is copied into a top-down 32-bit BGRA DIB; alpha is reported as WTSAT_RGB.
COM exports are DllGetClassObject and DllCanUnloadNow. Panics are contained, class
factory aggregation is rejected, and object/server-lock counts control unloading.
There is no WebView2 initialization or Tauri dependency in this DLL. No Windows
preview-pane handler is included.

The bundle hook builds the DLL for the selected target and stages it under the
ignored `target/thumbnail-windows/`. `tauri.windows.conf.json` ships it beside the
app executable. NSIS currentUser hooks register:

- HKCU\Software\Classes\CLSID\{926e1d8f-eb25-48d9-a251-9d27bf165d8a}\InprocServer32,
  with ThreadingModel=Apartment.
- Every built-in / official vendor extension's per-user ShellEx thumbnail slot
  `{e357fccd-a995-4576-b01f-234630154e96}`.

DisableProcessIsolation is never set. Hooks notify SHChangeNotify after changes.
Previous per-user providers are backed up and restored on uninstall only if the
current provider is still ours. Removing our override reveals a machine-wide
handler. HKCU contributes to HKCR's merged view, so `.tif/.tiff` registrations
are intended to override the Windows-provided thumbnail handler per user.
Explorer selection, cache behavior and install/uninstall restoration need real
Windows acceptance; the existing default Open action is not changed by hooks.
Registration covers core and current official plugins; an arbitrary third-party
plugin adding a new extension also needs OS association/thumbnail registration.

```sh
cd src-tauri
cargo build --target x86_64-pc-windows-msvc -p tmv-thumbnail-win
cargo test --target x86_64-pc-windows-msvc -p tmv-thumbnail-win
```

CI runs those commands on windows-latest. The COM test uses an anonymous memory
stream, checks bitmap dimensions/content and rejects repeated initialization.
Test an NSIS install with TIFF + MRC + an installed vendor plugin, then uninstall
and inspect restoration. Restart Explorer/clear its thumbnail cache if needed.

## Linux

DEB/RPM install `packaging/linux/tifmrcview.thumbnailer` into
`/usr/share/thumbnailers/`. It covers image/tiff, every MIME in the microscopy
XML, and core raster formats. The MIME XML includes the alternate MRC extensions.
AppImage cannot install these system files; manual integration is required.

The command needs neither display nor network, and writes only its requested
output. CI invokes it with DISPLAY and WAYLAND_DISPLAY unset. GNOME's bubblewrap
sandbox generally hides home: if it does not expose the per-user plugin folder,
core TIFF/MRC still work and vendor thumbnails fail gracefully. A .thumbnailer
cannot grant an extra home bind mount. This implementation does not weaken that
sandbox; plugin thumbnails require a host that exposes the plugin directory
read-only. GNOME's built-in TIFF provider may also compete with this registration.
KDE's native thumbnail-plugin integration is not supplied or verified.

## Local validation (2026-10-26)

- Built both arm64 macOS extensions using the script, without downloads. Strict
  signature verification passed, including the sandbox/plugin entitlement.
- Both extension executables are about 4.5 MB; otool and Cargo dependency checks
  show no Tauri/WebView linkage in the helper.
- Ran the CLI on read-only `1-r-1-m.tiff` and `1-r-1.tiff` from
  `hopesim荧光20261026`; visually inspected both 256×256 RGBA PNGs in the task's
  scratchpad `out/`. Both show image structure instead of black.
- Core-only lib tests: 127 passed; C ABI allocation/free test: 1 passed. They
  cover low-signal u16 TIFF, plain RGBA/header removal, PNG encoding, sparse
  4 GiB middle-Z MRC, rejecting a 256 MiB plane, and more than four channels.
- The normal lib suite encountered seven existing market HTTP tests denied by
  the execution sandbox at local port binding; 160 other tests passed.
- Windows MSVC target cargo check (including COM tests) passed locally; DLL link,
  COM execution and installer hooks await Windows CI/hardware.
- cargo clippy completes with two pre-existing warnings in render/desktop code.
- Intel/universal linking, Linux runtime and packaged-app signing/registration,
  actual Finder TIFF override, and Windows Explorer acceptance remain unverified.
  No full app/DMG build or installed-app replacement was performed locally.

Design references: [Apple thumbnail extensions](https://developer.apple.com/documentation/quicklookthumbnailing/providing-thumbnails-of-your-custom-file-types),
[Apple data-based previews](https://developer.apple.com/documentation/quicklook/qlpreviewreply),
[Microsoft thumbnail handlers](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/legacy/cc144114(v=vs.85)),
[Tauri configuration](https://v2.tauri.app/reference/config/), and
[GNOME's sandbox implementation](https://github.com/GNOME/gnome-desktop/blob/master/libgnome-desktop/gnome-desktop-thumbnail-script.c).
