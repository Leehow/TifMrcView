# R16 format plugin template

[简体中文](README.zh-CN.md)

A small Apache-2.0 Rust plugin for **TifMrcView 0.5.0+**, using the current
plugin API **1** and WebAssembly ABI **1**. It implements `Format::open`,
image info with metadata, and one raw page recipe per Z slice through
`plugins/sdk`. It does not need npm packages. `index.js` is still required
by today's app to register the plugin in the plugin window.

## Copy and rename

Inside a TifMrcView checkout:

```sh
cp -R templates/format-plugin plugins/my-format
cd plugins/my-format
```

Set `plugin.json`'s `id` to `my-format` (matching the folder name). Update
the registration in `index.js` to match its id, name, version and description.
Use a unique id before submitting to the market. Rename the Cargo package,
`wasm` filename and extension declarations as needed; `build.mjs` reads the
Cargo package name and copies its output to the manifest's `wasm` path.

For a **standalone repository**, copy the template and the shared tooling
from a TifMrcView checkout (run these commands from that checkout):

```sh
cp -R templates/format-plugin ../my-format
mkdir -p ../my-format/vendor ../my-format/tools
cp -R plugins/sdk ../my-format/vendor/sdk
cp scripts/tmv-plugin.mjs scripts/plugin-manifest.mjs scripts/plugin-zip.mjs ../my-format/tools/
cd ../my-format
```

Change the SDK dependency in `Cargo.toml` to
`tifmrcview-plugin-sdk = { path = "vendor/sdk" }`, then rename the id and
JavaScript registration as above. Commit the vendored SDK and `tools/`
alongside your plugin when creating its repository. The included
`.github/workflows/build.yml` is for this standalone layout; it tests,
builds, checks and uploads the zip without publishing it.

Alternatively replace the path dependency with the commented **git**
dependency in `Cargo.toml`, pinning `rev` to a reviewed commit of
<https://github.com/Leehow/TifMrcView>. Cargo finds the SDK package in that
repository.
Copy `tools/` in either case. You can also set
`TMV_PLUGIN_TOOLS` to an absolute directory containing those shared scripts.
The default in-repository path is `../../scripts`; nothing is downloaded
automatically by `build.mjs`.

## Edit and test

Edit `src/lib.rs` for your format and `plugin.json` for its extensions.
Keep file access in the SDK's `file` functions; describe pixel locations
with `Recipe` / `Chunk` so the app reads pixels natively. Do not use
`std::fs` in the wasm plugin. Metadata in `Info::metadata` appears in image
info; this example does not invent a physical pixel size.

R16 is a made-up, uncompressed format:

| Byte offset | Contents |
| --- | --- |
| 0 | Four magic bytes `52 31 36 00` (`R16\0`) |
| 4 | Width: little-endian u32 |
| 8 | Height: little-endian u32 |
| 12 | Depth: little-endian u32 |
| 16 | `width × height × depth` little-endian u16 pixels |

Pixels are row-major (X fastest), then Y, then Z, one grayscale channel
and one time point. Dimensions must be nonzero; pages are limited to the
app's 16 GiB limit and depth to 2²⁴. Short headers, bad magic, arithmetic
overflow and truncated pixels are rejected. Trailing bytes are ignored.

```sh
cargo test
cargo run --example make_sample -- sample.r16
```

The generated file is 4 × 3 × 2 (64 bytes); flattened pixel `i` is `i × 1000`
for `i = 0..23`. Host tests use this same generator and parser, verify
metadata and page recipes, recover the expected pixels, and reject invalid
inputs. Host SDK file imports are stubs, so these tests do not execute a
wasm instance. See `src-tauri/tests/*_plugin.rs` in TifMrcView for app-side
install/open/decode tests.

## Build and install

Prerequisites: Node 22+, Rust, the `wasm32-unknown-unknown` target and `unzip`
on PATH (for checking zips). On an unmetered network, prepare the target
once with `rustup target add wasm32-unknown-unknown`. Then:

```sh
node build.mjs
```

This validates the source manifest, runs
`cargo build --target wasm32-unknown-unknown --release`, validates
the compiled module, and writes `dist/<id>-<version>.zip`. The zip contains
`plugin.json` at the root, `index.js`, and the module at the manifest's
`wasm` path, using the same reproducible zip writer as the market packer.
Rust sources, SDK, build tools and sample files are excluded.

In TifMrcView, open **插件 → 加载插件…** (**Plugins → Install plugin…**),
select the zip, confirm the plugin is enabled, then open `sample.r16`.
Check the two Z slices and the R16 metadata in image info.

The checker also runs separately:

```sh
# In the main checkout:
node scripts/tmv-plugin.mjs check templates/format-plugin
node scripts/tmv-plugin.mjs check templates/format-plugin/dist/format-plugin-1.0.0.zip
# In a standalone plugin repository:
node tools/tmv-plugin.mjs check .
node tools/tmv-plugin.mjs check dist/my-format-1.0.0.zip
```

It shares the market packer's manifest rules and additionally checks
current-app compatibility (default 0.5.0; override with `--app-version X.Y.Z`),
paths, core extension conflicts and wasm ABI signatures. Source crates may
omit the wasm and get a **manifest-only** result; packaged zips may not.
Unknown imports produce warnings saying the app will refuse them. The
checker never executes plugin code: the app checks `tmv_abi() == 1` and
runtime compatibility during installation. A static pass alone does not
prove image decoding works.

## Submit to the market

Submit a **pull request to <https://github.com/Leehow/TifMrcView>**, adding
your renamed source folder under `plugins/<id>/`. Use the in-repository SDK
path, keep the JavaScript registration consistent, include tests and
describe a sample file and its expected output in the PR. Review the SDK
and format documentation in `docs/format-plugins.md`.

Maintainers run `node scripts/pack-plugins.mjs <out-dir>` to build the crates
under `plugins/`, write `<out-dir>/plugins/<id>-<version>.zip` and `<id>.zip`,
and generate `<out-dir>/plugins/plugins.json`. Each downloadable market
entry is the manifest plus `file` (versioned zip filename), `size` (zip
bytes), and `sha256` (zip digest). Do not hand-edit that generated index.
The template stays under `templates/` and is never published by the default
market packer.
