# Notes for contributors (and coding agents)

## Keep the website in step with the app

The website (https://tifmrcview.com) is **not** in this repository: its
source is the private repo `Leehow/tifmrcview-site`, which deploys itself
(static files, no build) on every push to its `main`. Never add website
pages, site text or site screenshots to this public repo.

A change to the app is not finished until the site matches it, so a PR
here that changes the UI or features needs a companion change there:

- **Live demo.** The home page runs this repo's `ui/` in an iframe; the
  site repo copies `ui/` from `main` here at deploy time (also daily) and
  swaps the Tauri backend for its `site/demo/tauri-mock.js`. If the UI
  calls a new backend command (`invoke("…")`) the demo needs, or
  `ui/index.html` gains a new `<script>` or file, update the site repo
  (see its CLAUDE.md).
- **Feature text.** Update the feature description on the site's home page
  and all its languages, and the plugin market text for plugins.
- **What stays here.** The release workflow (`build.yml`) still uploads
  installers, `latest*.json`, `downloads*.json`, Pro's SSH engine
  (`tmv-engine_<version>_linux-x86_64.tar.gz`, downloaded by Pro on first
  remote processing) and the plugin packages to the same server; `plugins.yml` uploads plugin packages when `plugins/`
  changes. README screenshots are in `docs/shots/`.

## CI and releases

This repository is public, so its GitHub Actions are free and always
available; use them.

- **Release.** Run the `build` workflow (`build.yml`) on `main` by hand
  with `publish` on. It builds macOS, Windows, Linux, the CLI and Android,
  signs them and uploads everything to the download site. Do not build a
  release on a local machine or ask the owner for keys, licences or
  installs to do so; a local release is a fallback only when Actions is
  down.
- **While developing**, put `[skip ci]` in the commit messages of pushes
  that do not need the full matrix (it takes over an hour), and test on a
  local machine. Let CI run when a change is ready to merge.
- **Local test builds** that are installed for the owner to try (in
  /Applications, standard or Pro) carry the *next* release's version with a
  `-local.N` suffix, e.g. `0.7.336-local.1` when 0.7.335 is the newest
  release: newer than the release, so plugins' `minAppVersion` checks pass,
  but older than the real 0.7.336, so the updater still offers it. Never
  give a local build a plain number above the release (a local 0.7.344
  hides every release up to 0.7.344). Build with
  `TAURI_CONFIG='{"version":"<v>"}'` and `TMV_VERSION`/`TMV_APP_VERSION`
  set to `<v>`; a plain `cargo build` reports the crate's 0.7.0. Several
  threads may install into the same app: merge the other active branches
  into the build, and keep one copy of each edition.
- The private website repository has no Actions minutes: its workflows
  cannot start, so its pages are uploaded from a local machine instead
  (see that repository's CLAUDE.md).

## Pro's SSH engine

Pro installers do not carry the Linux engine for SSH processing; build.rs
pins its size and SHA-256 (`TMV_SSH_ENGINE_PACKAGE`) and Pro downloads it
from its version's folder on first use. A local test build whose engine is
not on the download site must set `TMV_SSH_ENGINE_EMBED=1` to embed it.

## Reuse before building

Use what the app already has before writing a new one: one label brush
(the viewer's Labels panel) is where users mark things, and workspaces
(STA, single particles) read its layers instead of adding brushes of their
own; single-particle processing calls the cryo-ET motion, CTF and denoising
code instead of keeping copies. Before adding a tool, an engine or a step,
look for one that does the job and extend it.
