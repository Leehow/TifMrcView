# Notes for contributors (and coding agents)

## Keep the website in step with the app

The website (`site/`, deployed to https://tifmrcview.com by
`.github/workflows/site.yml` on every merge to `main`) shows the app, so a
change to the app is not finished until the site matches it.

- **Live demo.** The home page runs the real viewer UI from `ui/` in an
  iframe: `scripts/build-site-demo.mjs` copies `ui/` into `demo/app/` at
  deploy time and swaps the Tauri backend for `site/demo/tauri-mock.js`.
  UI changes therefore reach the demo by themselves, but:
  - if the UI calls a new backend command (`invoke("…")`) that the demo
    needs, add it to `commands` in `site/demo/tauri-mock.js`, mirroring the
    Rust code; unknown commands answer "not in the web demo";
  - if `ui/index.html` gains a new `<script>` or file, add it to the file
    list in `scripts/build-site-demo.mjs`;
  - hide features that need a computer (file dialogs, saving, plugins) in
    `site/demo/demo.css`;
  - check it locally: `node scripts/build-site-demo.mjs site`, then serve
    `site/` (`python3 -m http.server -d site`) and open the home page.
- **Feature text.** When a feature is added or changes, update its
  description on the home page (`site/index.html`) and every language in
  `site/i18n.js` (zh, en, de, ja, es, fr, ru), and the plugin market text in
  `site/plugins/` for plugins. Replace screenshots in `site/shots/` when the
  UI they show has changed visibly.
- **Demo samples** come from the maintainer's own Zeiss Airyscan data and
  are rebuilt with `scripts/make-demo-data.py`. Use only our own data there.
