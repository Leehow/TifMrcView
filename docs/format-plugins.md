# Format plugins (WebAssembly), ABI v1

TifMrcView reads TIFF / OME-TIFF / ImageJ, MRC and the common raster formats
itself. Everything else — vendor formats such as Zeiss CZI, or vendor
metadata hidden in TIFF tags such as Zeiss SmartSEM — comes from **format
plugins**: a WebAssembly module inside an ordinary plugin package. The
"zeiss" plugin in `plugins/` (CZI, LSM and SmartSEM metadata) and the other
vendor plugins next to it are built this way. One vendor gets one plugin:
a module can open several file types (dispatch on the extension of the path
in `open`) and add metadata in the same package.

A format plugin can do two things:

- **Open a file type** (`formats`): the app asks the plugin to describe the
  file (size, channels, pixel type, metadata) and, for each page, *where the
  pixels are* in the file. The app then reads, decompresses and assembles
  the pixels itself, natively and in parallel. Plugin code only builds an
  index; the heavy lifting runs at native speed.
- **Add metadata** (`metadata`) to files the core (or another plugin)
  opens: format name, pixel size, groups of settings for the image-info
  panel.

## Sandbox and limits

Modules run in [wasmi](https://github.com/wasmi-labs/wasmi), a pure-Rust
WebAssembly interpreter, with nothing but the imports listed below. A
plugin cannot touch the file system, the network, the environment or the
UI; it can only read the one file it was asked about.

| Limit | Value |
| --- | --- |
| Linear memory per instance | 512 MiB |
| Fuel for `tmv_open`, `tmv_info`, `tmv_metadata` | 3·10⁹ (a few seconds) |
| Fuel for `tmv_page` and the small calls | 5·10⁸ |
| Largest buffer returned (info JSON, recipe) | 256 MiB |
| Largest output of the `decompress` import | 256 MiB |
| Largest page (width × height × planes × sample size) | 16 GiB |

Fuel counts executed WebAssembly operators; a release build runs roughly
0.5–1.3·10⁹ per second. Running out of fuel, trapping (including a panic),
returning bad data or reading out of bounds makes that one call fail with
an error message — it never crashes or hangs the app.

Each opened file gets its own instance. The host calls one instance from
one thread at a time; while a page is being decoded, other pages of the
same file can be described (calls are short and serialised) and decoded
(outside the plugin) in parallel.

A plugin cannot take over the core's extensions (`tif`, `tiff`, `mrc`,
`mrcs`, `rec`, `st`, `map`, `png`, `jpg`, `jpeg`, `gif`, `bmp`, `webp`); use
`metadata` to add to those files instead.

## Package

A format plugin is a normal plugin package (see the developer docs on the
plugin market) with a `.wasm` file and three more fields in `plugin.json`:

```json
{
  "id": "zeiss",
  "name": "Zeiss",
  "version": "1.0.0",
  "description": "…",
  "main": "index.js",
  "apiVersion": 2,
  "permissions": [],
  "minAppVersion": "0.6.0",
  "wasm": "zeiss.wasm",
  "formats": [{ "ext": ["czi"], "name": "Zeiss CZI" }, { "ext": ["lsm"], "name": "Zeiss LSM" }],
  "metadata": { "ext": ["tif", "tiff"] },
  "replaces": ["zen", "zenblack", "smartsem"]
}
```

| Field | Meaning |
| --- | --- |
| `wasm` | Path of the module inside the package. |
| `formats` | File types the module opens: `ext` (lower-case, no dot, `[a-z0-9]{1,16}`) and a display `name` (used as the format name when the plugin gives none). Needs the format exports. |
| `metadata` | Extensions whose files `tmv_metadata` should see. Needs the metadata export. |
| `stacks` | Optional. Project files of acquisition programs that describe a Z stack of separate slice images (FIB-SEM, serial sections): `[{ "files": ["ProjectData.dat"], "name": "Thermo Auto Slice & View" }]`. `files` are file names, `*` matches any run of characters, case is ignored. Needs the stack export (below) and app 0.7.0. |
| `replaces` | Optional ids of older plugins this one takes over, e.g. when several plugins merge into one. While both are installed the old ones never run. Installing the official market package (from the market, or the same zip by hand) also removes them, and at start-up the app installs such a replacement for users who have the old plugins, when it asks for no permissions. The market entry must list the same ids. |

`main` (a JavaScript file) is still required: it registers the plugin so it
shows up in the plugin window, where the user can switch it off. With
`"apiVersion": 2` and no permissions it runs in the sandboxed frame like any
other API 2 plugin, and for a pure format plugin it can be as small as

```js
TifMrcView.register({ id: "zeiss", activate() {} });
```

On install the app compiles the module, checks that it has the exports
`plugin.json` promises and that `tmv_abi()` returns 1, and refuses the
package otherwise. A switched-off plugin opens nothing; opening a file it
would handle says so.

## ABI v1

Plain C ABI on `wasm32`, little-endian, all lengths in bytes. Pointers are
offsets into the module's exported `memory`.

**Returned buffers** (`tmv_info`, `tmv_page`, `tmv_metadata`) are an `i64`
holding `ptr | len << 32`. `0` means failure. The buffer must stay valid
until the next call into the module; the host copies it right away.

**Errors**: before returning a failure (`0`, or a negative handle), call the
`error` import with a message for the user (the app's language is Chinese;
English is fine too).

### Exports

| Export | Signature | |
| --- | --- | --- |
| `memory` | memory | The linear memory. |
| `tmv_abi` | `() -> i32` | Must return `1`. |
| `tmv_alloc` | `(len: i32) -> i32` | Allocates `len` bytes; `0` on failure. The host uses it to pass the path. |
| `tmv_free` | `(ptr: i32, len: i32)` | Frees what `tmv_alloc` returned. |
| `tmv_open` | `(path_ptr: i32, path_len: i32) -> i32` | Opens the file (UTF-8 path, for information only — read the file through the imports). Returns a handle ≥ 0, or < 0 on error. |
| `tmv_open_series` | `(path_ptr: i32, path_len: i32, series: i32) -> i32` | Optional. Opens image `series` (≥ 1) of a file that holds several (see `series` below); like `tmv_open` otherwise. |
| `tmv_info` | `(handle: i32) -> i64` | Image description, JSON (below). Called once, right after `tmv_open`. |
| `tmv_page` | `(handle: i32, page: i32) -> i64` | Recipe for one page (below). `page` is `0 … pageCount-1`. |
| `tmv_close` | `(handle: i32)` | The file was closed. |
| `tmv_metadata` | `(path_ptr: i32, path_len: i32) -> i64` | Metadata JSON for a file another reader opened, or `null`. |
| `tmv_stack` | `(path_ptr: i32, path_len: i32, list_ptr: i32, list_len: i32, series: i32) -> i64` | Stack JSON for a project file named in `stacks` (below). |

`tmv_open`, `tmv_info`, `tmv_page` and `tmv_close` are needed for
`formats`; `tmv_metadata` for `metadata`. A module may have both.

### Imports (module `tmv`)

| Import | Signature | |
| --- | --- | --- |
| `file_size` | `() -> i64` | Size of the file. |
| `read` | `(offset: i64, len: i32, dst: i32) -> i32` | Copies up to `len` bytes at `offset` into memory at `dst`. Returns the number of bytes read (less at the end of the file), or < 0 if `dst..dst+len` is outside memory. |
| `decompress` | `(codec: i32, src: i32, src_len: i32, dst: i32, dst_len: i32) -> i32` | Decompresses `src` natively into `dst` (at most `dst_len` bytes). Returns the output length, or < 0. Any codec below except JPEG. Meant for metadata blocks; pixels go through recipes. |
| `log` | `(ptr: i32, len: i32)` | Writes a line to the app's log (stderr). |
| `error` | `(ptr: i32, len: i32)` | Sets the error message of the current call. |

A module importing anything else is refused.

### `tmv_info` JSON

```json
{
  "width": 1024, "height": 1024,
  "sizeC": 2, "sizeZ": 20, "sizeT": 1,
  "dtype": "u16",
  "isRgb": false,
  "samplesPerPage": 1,
  "order": "CZT",
  "pageCount": 40,
  "channels": [{ "name": "EGFP", "color": [0, 255, 91] }],
  "pixelSize": [0.045, 0.045, 0.15],
  "unit": "µm",
  "format": "Zeiss ZEN CZI (ZEN 3.8.2.0)",
  "metadata": [{ "name": "ZEN 图像", "items": [["软件", "ZEN 3.8"], ["位深", "12"]] }]
}
```

| Field | |
| --- | --- |
| `width`, `height` | Pixels; required. |
| `sizeC`, `sizeZ`, `sizeT` | Default 1. |
| `dtype` | `u8`, `i8`, `u16`, `i16`, `u32`, `i32`, `f32` or `f64`; required. |
| `samplesPerPage` | 1 (one channel per page) or `sizeC` (every page holds all channels, e.g. RGB). Default 1. |
| `isRgb` | 8-bit RGB shown as colours rather than as channels (needs `samplesPerPage` ≥ 3). |
| `order` | Page order of C, Z, T, fastest first (`CZT`, `CTZ`, `ZCT`, `ZTC`, `TCZ`, `TZC`), when `samplesPerPage` is 1. Default `CZT`. With `samplesPerPage` = `sizeC`, page = `z + sizeZ·t`. |
| `pageCount` | Default `sizeC·sizeZ·sizeT` (or `sizeZ·sizeT`). |
| `channels` | Names and colours; missing entries get defaults. |
| `pixelSize`, `unit` | `[x, y, z]` in `Å`, `nm`, `µm` or `mm`, or for diffraction patterns in `1/nm`, `1/Å` or `1/µm`; `null` or 0 when unknown. |
| `format` | Shown in the status bar; default: the `formats[].name`. |
| `metadata` | Groups for the image-info panel. |
| `series`, `seriesIndex` | Optional, for files that hold several images (a Leica LIF project, ND2 positions…): one `{ "name", "detail" }` per image, in order, and which one this is. With two or more entries the viewer shows a picker and reopens the file through `tmv_open_series`. Older versions of the app ignore them and show image 0. |
| `spectrum` | Optional, for spectra (EELS, EDS, CL…): `{ "axis", "start", "step", "unit", "kind" }`. `axis` `"x"`: every image row is a spectrum (a single spectrum is a 1-pixel-high image); `"z"`: a spectrum image, each pixel's spectrum runs through the Z slices. Channel `i` is at `start + i·step` `unit`; `kind` names it (e.g. `EELS`). The viewer then plots the spectra; older versions ignore it. In Rust: `Info::spectrum`. |

### `tmv_metadata` JSON

`null` when the file is not one the plugin knows, else

```json
{ "format": "Zeiss SmartSEM TIFF", "pixelSize": [15.625, 15.625, 0], "unit": "nm",
  "groups": [{ "name": "SmartSEM 主要参数", "items": [["EHT", "1.50 kV"]] }] }
```

All fields are optional. `format` replaces the format name, `pixelSize`
(with `unit`) replaces the pixel size, `groups` are appended after the
groups the opener found. Errors from `tmv_metadata` are logged and
otherwise ignored: the image still opens.

### Image stacks from project files (`tmv_stack`)

Volume-EM programs write one image per slice and keep what ties them
together (order, slice thickness, detectors) in a project file. The app
always opens a folder of slices as one Z stack by itself (the largest
group of alike-named images, in natural order); a plugin adds what the
project file knows.

The plugin is bound to the project file (the `read` import reads it) and
gets the files around it as text: one path per line, relative to the
project file's folder, `/`-separated, in natural order, hidden files
skipped, up to four folders deep and 200 000 files. It returns:

```json
{
  "files": ["Images/SEM Image/run - SliceImage - 001.tif", "…"],
  "format": "Thermo Auto Slice & View",
  "zSpacing": 10, "unit": "nm",
  "metadata": [{ "name": "Auto Slice & View 项目", "items": [["项目名称", "worm"]] }],
  "series": [{ "name": "SEM Image", "detail": "320 张", "dir": "Images/SEM Image" }],
  "seriesIndex": 0
}
```

| Field | |
| --- | --- |
| `files` | The slices in Z order, each one a line of the listing; the app opens them with the core or other plugins, so they can be in any format it reads. Required. |
| `zSpacing`, `unit` | Distance between slices; the X/Y pixel size comes from the slices. |
| `format`, `metadata` | Status-bar name (default: the `stacks` entry's `name`) and groups shown before the first slice's own. |
| `series`, `seriesIndex` | Several stacks in one project (one per detector…): the picker reopens with `series`. `dir` is the folder of that stack's slices, so opening that folder picks it. |

The app asks for it when the user opens the project file, opens a folder
that holds one or whose project file is one or two folders up (and the
stack is that folder's slices), and when it lists a folder for the
thumbnails: the stack then shows as an extra entry at the top. With the
SDK, implement `Stacks` and use `export_stack!` (see
`plugins/thermo/src/asv.rs`).

### Page recipes (`tmv_page`)

A recipe tells the host how to build one page: a list of **chunks**, each
a byte range plus how to decode it and which rectangle of the page it
fills. The page starts as zeros; chunks are applied in order, so later
chunks overwrite earlier ones where they overlap (mosaics: sort by tile
order). Chunks are decoded in parallel.

Binary, little-endian:

```
offset  size  header
0       4     magic "TMVR"
4       4     u32 version = 1 (48-byte chunks) or 2 (56-byte chunks)
8       4     u32 chunk count N
12      4     reserved, 0
16      S·N   chunks (S = 48 or 56)
16+S·N  …     inline payload
```

Each chunk (48 bytes; version 2 adds `skip`):

| Offset | Type | Field |
| --- | --- | --- |
| 0 | u8 | `source`: 0 = file, 1 = inline payload |
| 1 | u8 | `codec` (below) |
| 2 | u8 | `dtype` code (below), or `0xff` = the page's dtype; must match the page |
| 3 | u8 | `flags` (below) |
| 4 | u32 | `samples`: interleaved samples per pixel in the decoded data (1–64) |
| 8 | u64 | `offset`: in the file (`source` 0) or in the inline payload (`source` 1) |
| 16 | u64 | `length` of the stored (compressed) bytes |
| 24 | u32 ×4 | destination rectangle `x, y, w, h` in page pixels (clipped to the page; chunks entirely outside are skipped) |
| 40 | u32 | `plane`: page plane that sample 0 goes to; sample *s* goes to `plane + s`, samples landing past `samplesPerPage` are dropped |
| 44 | u32 | `stride`: bytes per decoded row, 0 = `w · samples · size` |
| 48 | u64 | `skip` (version 2 only): decoded bytes to drop before the first row, e.g. `z · h · stride` to take one Z slice of a compressed 3D block (HDF5 chunks). Not with hi/lo or JPEG. Needs app 0.5.0; the SDK writes version 2 only when a chunk uses it. |

Codecs (`codec`, also for the `decompress` import):

| Code | |
| --- | --- |
| 0 | raw (read in place from the memory-mapped file) |
| 1 | zlib (TIFF compression 8) |
| 2 | raw deflate |
| 3 | LZW, TIFF flavour (MSB first; old-style LZW is accepted too) |
| 4 | Zstandard |
| 5 | baseline JPEG (`dtype` u8, `samples` 1 = gray or 3 = RGB; recipes only) |
| 6 | PackBits |
| 7 | LZ4 in the framing of the HDF5 LZ4 filter 32004: decoded size (u64 BE), block size (u32 BE), then per block its stored size (u32 BE) and bytes (stored as-is when not smaller). App 0.5.0. |

`dtype` codes: 0 `u8`, 1 `i8`, 2 `u16`, 3 `i16`, 4 `u32`, 5 `i32`, 6 `f32`,
7 `f64`.

Flags (combine with `|`):

| Bit | |
| --- | --- |
| 1 | big-endian samples |
| 2 | "hi/lo": 16-bit samples stored as all low bytes, then all high bytes (CZI zstd1) |
| 4 | BGR: the first three samples are blue, green, red (sample 0 → `plane + 2`) |
| 8 | horizontal differencing (TIFF predictor 2), undone per row |
| 16 | HDF5 byte shuffle: the whole decoded chunk holds all first bytes of its samples, then all second bytes…; undone before `skip`. Not with hi/lo or JPEG. App 0.5.0. |

Decoded data shorter than `stride · (h - 1) + w · samples · size` is an
error ("图像数据不完整"). For odd formats the plugin can decode a chunk
itself and pass the pixels inline (`source` 1, codec 0), at interpreter
speed.

A page made of one uncompressed, single-sample chunk covering the whole
page is also read in place for the XZ / YZ side views, without decoding
whole pages.

## The Rust SDK

`plugins/sdk` (`tifmrcview-plugin-sdk`, `no_std` + `alloc`) wraps all of
the above. A plugin implements `Format` and/or `Metadata` and exports them
with `export_format!` / `export_metadata!`; the SDK provides `tmv_abi`,
`tmv_alloc` and `tmv_free`, the handle table, the JSON writers (`Info`,
`Meta`, `Group` — written straight to JSON, so even tens of thousands of
items stay cheap) and the recipe builder (`Recipe`, `Chunk`, `Codec`,
`flags`). Errors are `Error(String)`; `bail!` and `Context::context` help.
Panics abort the call (the module is built with `panic = "abort"`).

### Example: a trivial raw format

For a copyable crate with host tests, a sample generator, a one-command
build and a standalone GitHub Actions workflow, start with
[`templates/format-plugin`](../templates/format-plugin/README.md)
([简体中文](../templates/format-plugin/README.zh-CN.md)). It targets plugin
API 1 / ABI 1 and app 0.5.0. Run
`node scripts/tmv-plugin.mjs check templates/format-plugin` to check the
manifest before building; the same command accepts an installable zip
and checks the module's static ABI. Source crates without a built module
get a manifest-only check.

`.r16` files: `"R16\0"`, then width, height and depth as little-endian u32,
then the planes as little-endian u16.

`Cargo.toml`:

```toml
[package]
name = "r16"
version = "1.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
tifmrcview-plugin-sdk = { path = "../sdk" }

[profile.release]
opt-level = "s"
lto = true
panic = "abort"
strip = true

# Not part of any workspace.
[workspace]
```

`src/lib.rs`:

```rust
use tifmrcview_plugin_sdk::*;

struct R16 {
    w: u32,
    h: u32,
    depth: u32,
}

impl Format for R16 {
    fn open(_path: &str) -> Result<R16> {
        let head = file::read(0, 16)?;
        if &head[..4] != b"R16\0" {
            bail!("不是 R16 文件");
        }
        let (w, h, depth) = (le_u32(&head, 4), le_u32(&head, 8), le_u32(&head, 12));
        if w == 0 || h == 0 || depth == 0 {
            bail!("R16 文件头不对");
        }
        Ok(R16 { w, h, depth })
    }

    fn info(&mut self) -> Result<Info> {
        let mut info = Info::new(self.w, self.h, DType::U16, "R16 stack");
        info.size_z = self.depth;
        info.page_count = self.depth;
        let mut g = Group::new("R16");
        g.add_str("Planes", &format!("{}", self.depth));
        info.metadata.push(g);
        Ok(info)
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        let bytes = self.w as u64 * self.h as u64 * 2;
        let mut r = Recipe::new();
        r.push(Chunk::file(16 + page as u64 * bytes, bytes, [0, 0, self.w, self.h]));
        Ok(r)
    }
}

export_format!(R16);
```

`plugin.json`:

```json
{
  "id": "r16", "name": "R16", "version": "1.0.0", "description": "Opens .r16 stacks",
  "main": "index.js", "apiVersion": 1, "minAppVersion": "0.4.0",
  "wasm": "r16.wasm", "formats": [{ "ext": ["r16"], "name": "R16 stack" }]
}
```

`index.js`:

```js
TifMrcView.register({ id: "r16", name: "R16", version: "1.0.0", description: "Opens .r16 stacks", activate() {} });
```

A compressed or tiled format only changes `page`: push one chunk per tile
with `.codec(Codec::Zstd)`, `.samples(3).flags(flags::BGR)` and so on. A
metadata plugin implements `Metadata::metadata(path) -> Result<Option<Meta>>`
and uses `export_metadata!` (see `plugins/zeiss/src/smartsem.rs`; one crate may
use both macros, as `plugins/zeiss/src/lib.rs` does).

A file holding several images sets `Info::series` (and `series_index`) and
implements `Format::open_series(path, series)`; `export_format!` exports
`tmv_open_series` for every plugin, and series 0 always goes to `open`.

The SDK also has two parsers that vendor formats keep needing:
`tifmrcview_plugin_sdk::xml`, a fast, lenient XML tree for metadata
documents, and `tifmrcview_plugin_sdk::tiff`, a TIFF / BigTIFF directory
reader over any byte source, for formats that are TIFF underneath.

### Build and package

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
mkdir pkg
cp plugin.json index.js pkg/
cp target/wasm32-unknown-unknown/release/r16.wasm pkg/
cd pkg && zip -r ../r16.zip .
```

`plugin.json` must be at the root of the zip. Install it with **Plugins →
Install plugin…**. In this repository, `node scripts/pack-plugins.mjs out`
builds every plugin folder that has a `Cargo.toml` and packs the module
(without the Rust sources) next to `plugin.json`.

### Testing and debugging

- `cargo run --release --example probe -- file.r16` in `src-tauri`, with
  `TMV_PLUGIN_DIR` pointing at a folder that holds the unpacked package in
  `r16/`, opens a file through the plugin and prints the info and timings.
- `TMV_PLUGIN_DEBUG=1` prints the fuel each call used; the `log` import
  (`tifmrcview_plugin_sdk::log`) writes to the same output.
- `src-tauri/tests/czi_plugin.rs` shows end-to-end tests that build a
  plugin, install it into a temporary folder and open files through it.
- Code inside the plugin is interpreted (tens of times slower than native):
  keep the per-pixel work in recipes, read headers in a few large `read`s
  rather than many small ones, and avoid per-item allocations when
  building large metadata groups.

## Compatibility

The ABI is versioned by `tmv_abi()`. Version 1 will not change; additions
that old plugins can ignore (new codecs, flags or JSON fields) may appear,
and an incompatible change would come as version 2, with the app stating
which versions it accepts.
