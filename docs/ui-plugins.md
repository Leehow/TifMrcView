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
and a host-built `srcdoc` document (`about:srcdoc`, opaque origin). It inherits
the parent's CSP. A meta CSP at the start of the document further restricts it
to `default-src 'none'` and exactly two script URLs: the core runtime at
`tmvplugin://localhost/__sandbox/runtime.js` and that package's main script.
Windows uses `http://tmvplugin.localhost` for those script URLs. There are no
inline scripts or eval. In particular, `connect-src` falls back to `'none'`, so
a plugin cannot fetch its own or another plugin's files, despite the parent
being allowed to fetch tmvplugin resources. Other plugins' scripts are also
outside the child's script allowlist. The parent sets `frame-src 'none'`: srcdoc
is allowed, but subsequent URL navigation (including to privileged app or
registered-protocol pages) is blocked. The old `__sandbox/<id>` HTML endpoint
and `frame.html` template are removed; only the reserved runtime script is served.
The MessageChannel payload, permission, size and lifecycle checks are unchanged.

The frame cannot access the parent DOM, cookies, localStorage or IndexedDB.
The runtime connects first and only then loads the plugin script. WebView2
injects Tauri startup scripts into subframes regardless of its main-frame-only
setting (`wry-0.57.1/src/lib.rs:1029`, `src/webview2/mod.rs:516`). The runtime
removes configurable globals best-effort and tolerates native nonconfigurable
ones. This cleanup does not grant or revoke native authority.

**Native IPC boundary (stock Tauri 2.12.5):** `is_local_url` in
`tauri/src/webview/mod.rs:1960` trusts registered custom protocols. Therefore
serving the frame document from tmvplugin would incorrectly make its native IPC
Local. Its scripts may come from tmvplugin; its document must remain srcdoc.
`about:srcdoc` is not the app URL or a registered protocol. In `on_message`
(`webview/mod.rs:2044-2105`), it is Remote: app commands require an explicit
remote capability, and core/plugin commands require matching capabilities too.
Our capabilities have no `remote` entries and no app ACL manifest grants access.
The Rust regression `ipc_config_tests` checks source and compiled capability
configuration plus the generated manifest and the frame navigation policy.
No Tauri fork, dependency patch or native transport replacement is used; the
main UI retains stock fetch IPC and its postMessage fallback.

Fetch IPC takes the browser-controlled `Origin` header
(`tauri/src/ipc/protocol.rs:486`); an opaque frame's `Origin: null` is not a valid
URL and is rejected before dispatch, even if CSP allowed the request. Native
postMessage uses the WebView2 event's `Source` (`wry/src/webview2/mod.rs:979`),
not a source claimed in the JSON body. Wry registers a webview-level
`WebMessageReceived` handler, not frame-level handlers; whether an iframe message
is delivered at all must be recorded on the real Windows runtime. If delivered,
its source must remain non-local and app/core requests must fail ACL checks.

There is one stock-Tauri exception to a blanket no-IPC claim: the internal
`plugin:__TAURI_CHANNEL__|fetch` command bypasses the general ACL branch
(`webview/mod.rs:2080`). The public plugin bridge exposes no Tauri channels.
Do not infer that every internal command is ACL-rejected from the config test
or toolbar label: Windows acceptance must verify frame-message delivery and
absence of native command execution. On Linux, wry's raw-message adapter reports
the top URL (`src/webkitgtk/mod.rs:743`); its init scripts are main-frame-only
(`:826`), so retaining that platform's key separation is also essential. These
are platform acceptance boundaries, not properties proven by Chromium mocks.

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


### Native IPC probe (Windows acceptance)

`ui/sandbox/test/probe/` is a manual API 2 fixture (`sandbox-ipc-probe`, only
`ui` permission). It is not installed or scanned automatically. Close the app,
copy that directory to `%APPDATA%\com.leehow.tifmrcview\plugins\sandbox-ipc-probe\`
(the exact plugin directory is available with Open plugin folder in the app),
then reopen the app and enable **Sandbox native IPC probe**. This development
copy avoids the market's API 1/signature approval path; use a disposable test
profile and remove that exact copied directory afterwards.

The expected toolbar label is `inv0 ipc0 wv0 f10 f20 core0`. Hover or click for
details; the frame console logs the report and whether it captured a valid
Tauri invoke key. The probe tries injected `invoke('plugin_market_list')`, both
native postMessage objects, POST to both IPC URL forms, and a core app-version
command. Any `!` is a native success response and a failure of acceptance.
When `keyCaptured` is true, raw probes reuse the key from injected invoke,
so a missing-key rejection cannot mask a valid-key transport bypass. If it is
false, treat raw probes as weaker evidence; confirm the native request/key in
a debugger instead of attributing failure to the ACL.

A `0` means unavailable, rejected, or no reply; **it alone does not prove that
a fire-and-forget postMessage failed to execute**. `postMessage` returns void,
and stock Tauri evaluates native callbacks in the top frame. For raw `ipc`/`wv`
probes, use a debugger on wry's `WebMessageReceived` handler and Tauri's
`Webview::on_message`: record whether the frame message arrives, its native
`Source`, and any ACL rejection before dispatch. Set a breakpoint in the app's
`plugin_market_list` handler to confirm the frame did not execute it. Check core
command dispatch too. There is no custom "native postMessage disabled" diagnostic
in this design. Record no-event and unavailable separately from an ACL rejection.
CSP normally blocks fetch probes before transport; that is additional protection,
not evidence that the Rust parser or ACL ran. Do not weaken shipped CSP to test.

On Windows, confirm the frame console report says `frameUrl: "about:srcdoc"`
and `frameOrigin: "null"`, not a tmvplugin document URL. Stock Tauri must treat
any native request that arrives from that frame as non-local. Because internal
channel fetch has an ACL exception, also verify that frame-origin messages do
not reach that internal dispatch; do not assume the general app/core ACL proof
covers it. Run `cargo test --offline --lib` there to guard the actual build's
capabilities and manifest. Record the Windows and WebView2 versions.

Also verify on the real Windows app: an ordinary API 2 plugin activates, its
permitted toolbar/state/metadata/export/settings operations work, undeclared
operations still reject, and other-plugin file reads/scripts remain blocked.
Check normal main-UI plugin listing/local installation, annotation read/save
and export, including reload and a secondary window. Native main-window IPC
fallback remains available. Chromium tests use mocked native transport and
cannot substitute for native acceptance on Windows, macOS or Linux.
