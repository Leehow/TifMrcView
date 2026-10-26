# UI plugins: API 2 preview

API 2 packages set `"apiVersion": 2` and declare `permissions` in
`plugin.json`. Installation rejects missing/non-array permissions, unknown
permission names and any `native` field (even `false` or `null`). The permission
vocabulary is `state`, `ui`, `metadata`, `export`, `settings`; Rust documents it
in `package::SANDBOX_PERMISSIONS`, JavaScript in `scripts/plugin-manifest.mjs`.
A regression compares both with the sandbox's method-to-permission map.

Both zip and market installs show the name, version, API and human-readable
permissions before the user clicks Install. API 2 permissions are also shown
on market cards. Zip inspection does not install or execute the package, and
its SHA-256 pins the bytes approved by the user. Market installs check the
confirmed fields against the fresh official index and the actual manifest.

Third-party API 1 JavaScript packages are no longer accepted for installation.
During migration, the eight legacy packages shipped in `plugins/` remain
installable only with an exact id + version + SHA-256 (and size) match against
the official HTTPS market index. The backend keeps that index in memory for
the session; refresh the market before installing an official API 1 zip. With
no matching cached index, the zip fails closed. A manifest flag cannot grant
this exception, and neither a `wasm` field nor the content of its JS is a bypass.
Already-installed API 1 packages and loose scripts retain their existing trusted
loader, synchronous API and full privileges. WebAssembly format plugins retain
their existing sandbox and ABI.

Each API 2 plugin runs in a hidden iframe with `sandbox="allow-scripts"`
and an opaque origin. The reserved `tmvplugin://localhost/__sandbox/<id>`
route serves a core-owned frame page. Its CSP starts with `default-src 'none'`
and permits scripts only from the core runtime and that package's `main` URL.
There are no inline scripts or eval. The parent CSP permits frame navigation
only under the reserved sandbox route. This avoids srcdoc's inherited CSP and
opaque-origin `self` problems without weakening the app's script policy.
The frame cannot access the parent DOM, Tauri globals, network connections,
cookies, localStorage or IndexedDB. The runtime connects first and only then
loads the plugin script. If the WebView injects Tauri globals into the frame,
the runtime fails closed before loading third-party code. Such a WebView needs
a native platform fix before API 2 plugins can run; this PR does not weaken
the sandbox to support it.

The host verifies the frame's `event.source` at startup and transfers one
MessageChannel. Subsequent communication uses only that port:
`{id, method, args}` receives `{id, result}` or `{id, error}`;
notifications use `{event, data}`. Host callbacks never await a plugin reply.
Every method is checked against the package's declared permissions:

| Permission | Methods |
| --- | --- |
| `state` | `state`, `on` / `off`, `setZ`, `setT` |
| `ui` | `ui.toast`, `ui.showTools`, `ui.addToolbarButton`, `ui.addToolSection` |
| `metadata` | `image.addMetadata`, `image.setPixelSize`, `image.setFormat` |
| `settings` | `settings.get`, `settings.set` (keys are prefixed with the plugin id) |
| `export` | `renderRegion`, `dialog.save`, `fs.write` |

Unknown methods and undeclared permissions reject the returned promise.
Requests are limited to 1 MiB of structured data (16 MiB for binary writes),
32 levels of nesting and 16 pending calls per frame. PNG responses are limited
to 16 MiB; rendering is limited to 4096 pixels per dimension and 4 megapixels.
A plugin can add at most 32 UI elements/sections, with at most 100 panel items.

The runtime is provided by the host; plugins do not bundle or load it:

```js
TifMrcView.register({
  id: "example",
  async activate(app) {
    await app.ui.addToolbarButton({
      label: "Describe", title: "Describe the image",
      onClick: async () => {
        const state = await app.state();
        await app.ui.toast(state.path || "No image open");
      },
    });
    await app.ui.addToolSection({
      title: "Example",
      items: [{ type: "text", text: "Runs in a sandbox" }],
    });
  },
});
```

This example needs `"permissions": ["state", "ui"]`. All app methods return
promises; `await app.on("slice", callback)` returns an async unsubscribe function.
Supported events are `open`, `slice`, and `render`. Panels support `text`,
`button`, `number`, `select`, and `checkbox` items. The host creates the controls
using its own DOM and CSS; callbacks receive values, never DOM handles.
See [the type declarations](../ui/sandbox/tmv-plugin-api.d.ts) for the full API
and [the test fixture](../ui/sandbox/test/sample/index.js) for all control types.

`renderRegion` returns PNG bytes instead of an API 1 canvas. For export, call
`dialog.save({name, filter, exts})`, then `fs.write(path, bytes)` with exactly
the returned path. Only the most recent save selection authorizes a single
write; cancellation, writing or deactivation revokes it. Plugins cannot select
a directory programmatically. No arbitrary filesystem, native plugin calls,
image opening, DOM access or video encoding API is exposed.

Annotation writes are restricted by Rust to the image currently open in the
calling window and its current series: `<image>.annot.json` (or the existing
`<image>.seriesN.annot.json` convention for later series).

Recorder / format / i18n migration, language packs and developer templates
belong to later PRs. API 2 is a preview until those migration and platform acceptance steps land.

Run the local browser regression with `node scripts/test-plugin-sandbox.cjs`
using an existing `playwright` installation and Chromium cache. The test serves
the real UI with mocked Tauri commands and maps `tmvplugin:` to local fixture
files; it performs no Internet requests or package installation. `NODE_PATH`
can point at an existing Playwright package and `TMV_TEST_CHROMIUM` can select
an already installed Chromium binary if the cached revision differs. This test
validates browser isolation and the bridge, not native decoding or MP4 encoding.
