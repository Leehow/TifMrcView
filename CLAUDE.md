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
