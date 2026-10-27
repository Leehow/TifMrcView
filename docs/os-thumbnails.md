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
`.rec/.st/.map/.ali/.preali`. Extensions declare all app association UTIs, including public.tiff and the
future-plugin UTIs below. QLThumbnailMinimumDimension is zero. The sandbox has only the plugin
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
and `.rec/.st/.map/.ali/.preali` UTIs (`mdls -name kMDItemContentType file.rec`). Registering
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
ignored `target/thumbnail-windows/`. The NSIS post-install hook copies it beside
the app executable (moving a DLL Explorer still holds aside first) and registers:

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
Registration covers core and current official plugins. Additional enabled plugin
extensions are synchronized per user as described below.

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

## Runtime format registration

`plugin_assoc::sync_plugin_formats()` runs on app startup and after local/market
installation, uninstallation, plugin-folder reload and enable/disable changes.
It uses the enabled, valid WASM packages' `formats[].ext`, deduplicates and
subtracts `tauri.conf.json`'s installer associations. Metadata-only, disabled and
broken packages do not add types. Duplicate providers retain a type until the
last enabled provider is removed. Extensions are validated before use in shell
registrations. Helpers never synchronize or write these registrations. Errors
are logged without turning a successful package install into an apparent failure;
startup/reload retries synchronization. macOS synchronization is a no-op.

### macOS pre-registration

The single future-plugin list is `packaging/macos/plugin-formats.json`.
`scripts/quicklook-plist.py` reads it and the existing Tauri associations to
produce the app's merged Info.plist and both helpers' QLSupportedContentTypes.
`bundle.macOS.infoPlist` points at that generated plist, produced by the normal
before-bundle hook. Existing document types and Tauri-generated exported UTIs
are preserved. The extra document types use Viewer / Alternate; they do not
request default-handler changes. Each declares an imported
`com.leehow.tifmrcview.<ext>` UTI conforming to public.data and public.image.
Apple recommends imported declarations for formats owned by other organizations;
exporting these would incorrectly claim ownership. This macOS-only plist does
not expand Windows/Linux plugin-only installer associations.

Accepted extensions (none are currently declared by core or official plugins):

| Extensions | Format / reason |
| --- | --- |
| dm3, dm4 | Gatan DigitalMicrograph, established EM containers |
| emi, emd | FEI TIA / Velox, established EM metadata/container types |
| zvi | Zeiss AxioVision |
| vsi, ets | cellSens dataset and tile container |
| oif | Olympus FluoView dataset |
| lei | Leica confocal dataset |
| ndpi, svs | Hamamatsu / Aperio whole-slide images |
| ids | ICS pixel-data companion (unlike .ics, no widespread calendar meaning) |
| mvd2 | Volocity dataset |
| spi | SPIDER EM images |

Conservative omissions from the suggested set:

| Extensions | Reason to omit static microscopy classification |
| --- | --- |
| ser | Also a widely used astronomy video format |
| dv, r3d | DV and RED video collisions |
| stk | Legacy generic stacks / HyperCard and other stack formats; also ambiguous between microscopy readers |
| nd | QuickBooks network configuration |
| scn | Generic scene files as well as several microscopy formats |
| pic | Generic picture formats, not specifically Bio-Rad |
| ics | iCalendar |
| sif | Setup/information formats as well as Andor |
| msr | Ambiguous measurement/report containers |
| obf | OsmAnd offline maps |
| sld | Slide/presentation/CAD files |
| em | Short ambiguous suffix; insufficient basis to classify all such files as TOM images |
| btf | General BigTIFF alias, not microscopy-specific; current core dispatch does not advertise this suffix |

`.ali/.preali` are different: they are IMOD MRC stacks. The existing MRC reader
opens their bytes; dispatch now recognizes both, with regression coverage. They
join the existing Alternate MRC association and its Linux globs/Windows thumbnail
hook, rather than the future-plugin list. No decoder was added.

No missing-plugin decode is attempted speculatively. An unsupported extension
returns an ordinary error, the C ABI returns 1 with zero/null outputs, and Swift
calls its completion handler with that error. This provides Finder's normal
failure/fallback path without leaving a callback pending. Actual Finder icon
fallback and provider choice require installed-app testing. A vendor-exported
UTI can take precedence over our imported fallback UTI; matching filename tags
alone does not prove Finder will choose this provider. An extension outside this
static list still requires an app update on macOS.

### Windows runtime additions and removal

All writes use HKCU's 64-bit registry view, matching NSIS. The installed Tauri
NSIS template passes each association's `name` as its ProgID; the existing MRC
association is therefore **MRC Image**. Runtime adds an empty value named
`MRC Image` under `Software\Classes\.<ext>\OpenWithProgids`. It never writes
the extension's default value or Explorer's UserChoice. A dev/portable copy does
not change registrations unless that installed ProgID's command points at the
running executable.

The thumbnail slot is added only when the merged HKCR slot is absent/empty and
the installed COM DLL exists. Existing per-user **and machine-wide** providers
are preserved. Journal entries under
`HKCU\Software\com.leehow.tifmrcview\PluginFormats\<ext>` record OpenWith,
Thumbnail and an optional ThumbnailBackup (distinguishing absent from empty).
Entries are journaled before shell writes. Stale owned OpenWith values are
removed; an owned thumbnail is restored/deleted only while still ours. A later
installer taking ownership of the type retains its thumbnail registration.
SHChangeNotify(SHCNE_ASSOCCHANGED) runs after shell changes.

NSIS PREUNINSTALL enumerates this same journal and removes/restores dynamic
entries before deleting the COM registration and DLL. It preserves replacements
made by other software and pre-existing OpenWith entries. Uninstall does not
require starting the app. No candidate-list extensions are pre-registered.

### Linux runtime additions and removal

The app respects XDG_DATA_HOME, XDG_CONFIG_HOME and XDG_DATA_DIRS, defaulting to
`~/.local/share`, `~/.config` and `/usr/local/share:/usr/share`. It owns:

- `mime/packages/tifmrcview-plugins.xml`: exact extension globs. Existing shared
  MIME types are discovered from `mime/globs2` (highest-weight exact glob), with
  `application/x-tifmrcview-<ext>` as fallback. Stale self-generated fallback
  types in the cache are ignored during discovery.
- `thumbnailers/tifmrcview-plugins.thumbnailer`: deduplicated types, using the
  same Exec as the system thumbnailer.
- `applications/tifmrcview.desktop`: a hidden launcher pointing at the current
  executable, only created/updated when the user has not supplied that desktop
  entry. A user-modified entry is preserved. This also covers the product-cased
  Tauri desktop filename and AppImage's lack of a system launcher.
- `com.leehow.tifmrcview/plugin-formats.json`: journal of associations introduced
  by synchronization and the exact generated desktop content.

Only `[Added Associations]` in `mimeapps.list` is edited, preserving other apps'
order, defaults, comments, and pre-existing TifMrcView associations. Explicit
`[Removed Associations]` entries are respected. Removing the final plugin removes
the generated XML, thumbnailer, owned desktop entry and journal; only owned Added
Associations tokens are removed. Files are replaced atomically and unchanged
contents are not rewritten. Optional update-mime-database and
update-desktop-database are invoked after relevant changes; missing tools are
accepted and other failures logged. The thumbnail Exec still requires
`tifmrcview` on PATH, including manual AppImage integration. The existing GNOME
sandbox limitations above still apply. Linux package removal itself cannot
clean arbitrary users' homes; remove plugins in-app before removing the package
if per-user integration should also be removed.

## Validation for runtime associations (2026-10-26)

- `cargo test --lib --offline`: 167 passed, seven existing market HTTP tests
  failed because the execution sandbox denies binding a loopback listener.
- With only those seven sandbox-blocked HTTP cases filtered out, the desktop
  lib suite passed all 167 remaining tests.
- `cargo test --lib --offline --no-default-features`: 133 passed.
- `cargo test -p tmv-thumbnail-ffi --offline`: passed, including missing-plugin
  error outputs with isolated plugin discovery; no outstanding callback work.
- `node --test scripts/*.test.mjs`: 21 passed, including generated plist parity,
  candidate-list/core/plugin disjointness and IMOD installer aliases.
- `cargo check --target x86_64-pc-windows-msvc --offline`: attempted; the full
  desktop graph stops at ring's missing MSVC C headers (`assert.h`). The same
  check with `--no-default-features` passes and compiles the new Win32 registry
  code. No target, toolchain, crate or large Internet download was installed.
- `CARGO_NET_OFFLINE=true npx --no-install tauri build --bundles app`: passed.
  Artifact: `src-tauri/target/release/bundle/macos/TifMrcView.app`. `plutil -p`
  and readback confirmed 23 document types, 14 imported UTIs, existing exports,
  and all 23 supported UTIs in each appex. `codesign --verify --deep --strict`
  passed; both appex sandbox/plugin-read entitlements were retained. Signing is
  ad-hoc, with no notarization or GUI/TCC acceptance.
- The built executable's missing-plugin `.dm4` thumbnail request exited 1 in
  about 0.53 seconds with no output PNG. The isolated C ABI test also returned
  error 1 and null/zero outputs. Swift's error branch completes the request.
  This establishes clean error handling, not actual Finder selection/fallback.
- The available LaunchServices dump and Spotlight query exposed no disposable
  TifMrcView bundle paths. No GUI copy was launched, no build copy registered,
  and `/Applications/TifMrcView.app` was not replaced. The sandbox's limited
  inventory does not establish system-wide LaunchServices uniqueness.

Real OS acceptance still needed:

1. Windows: install NSIS on leehowpc, install a test plugin with a unique extra
   extension, inspect Open With and Explorer thumbnails, verify repeated startup
   is unchanged, disable/re-enable/uninstall it. Test duplicate providers, an
   existing HKCU provider, an existing HKLM provider, a provider replaced after
   sync, and NSIS uninstall while plugins remain installed. Check defaults and
   pre-existing OpenWith values survive.
2. macOS: install only the canonical `/Applications/TifMrcView.app`, inspect
   Finder Open With and existing defaults, test a listed vendor file with/without
   its plugin, Finder's generic-icon fallback and preview completion, and UTI
   resolution when vendor software is also installed. Clear only relevant caches
   if necessary. No GUI, TCC or Finder acceptance is implied by CLI/plist tests.
3. Linux: on a real DE, use isolated XDG directories to inspect generated XML,
   desktop/thumbnailer and mimeapps contents across install/remove/restart; then
   test MIME cache refresh and actual file-manager behavior. No Linux toolchain
   was installed for this task.

Design references: [Apple imported file/data types](https://developer.apple.com/documentation/uniformtypeidentifiers/defining-file-and-data-types-for-your-app),
[Apple thumbnail extensions](https://developer.apple.com/documentation/quicklookthumbnailing/providing-thumbnails-of-your-custom-file-types),
[Microsoft application/Open With registration](https://learn.microsoft.com/en-us/windows/win32/shell/app-registration),
[Tauri NSIS installer](https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi),
[freedesktop Added Associations](https://specifications.freedesktop.org/mime-apps/latest/associations.html),
[Bio-Formats dataset extensions](https://bio-formats.readthedocs.io/en/stable/formats/dataset-table.html),
[QuickBooks ND](https://quickbooks.intuit.com/learn-support/en-ca/help-article/product-setup/nd-files-quickbooks-desktop/L21Jtw7cN_CA_en_CA),
[OsmAnd OBF](https://docs.osmand.net/docs/technical/osmand-file-formats/), and
[RED R3D](https://docs.red.com/955-0199/955-0199_V1.2_Rev_A_RED_PS_V-RAPTOR_Operation_Guide/Content/4_Menus/b_ProjSet/FileFormat.htm).
