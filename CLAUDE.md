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
  installers, `latest*.json`, `downloads*.json` and the plugin packages to
  the same server; `plugins.yml` uploads plugin packages when `plugins/`
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
- The private website repository has no Actions minutes: its workflows
  cannot start, so its pages are uploaded from a local machine instead
  (see that repository's CLAUDE.md).
