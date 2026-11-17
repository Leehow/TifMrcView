# SSH remote folders

**SSH 远程文件夹** is an optional trusted plugin installed from the plugin
market. It is not included or loaded by default. After installation, it can be
enabled, disabled or uninstalled in the plugin dialog. When enabled, an SSH
button appears in the top toolbar. Its native backend requires the installed
package. Sandboxed third-party plugins receive no new permissions.
The SSH UI inherits the edition's accent: standard blue and Pro red, including
toolbar/folder/image icons, buttons, focus, selection and transfer progress.

On macOS, the first local-network TCP operation can fail immediately while
NECP resolves the app's local-network identity/permission. The retained native
failure had errno 65 (`EHOSTUNREACH`), zero connection duration and an explicit
`reason: NECP` kernel drop, before any SSH handshake. It was previously
misreported as a timeout. The TCP connector now preserves the actual error and
waits up to 15 seconds for this specific error on private/link-local addresses,
checking other resolved addresses first. Password/key authentication and host
trust failures are never retried. Other platforms, public addresses, localhost
and refused connections retain their immediate error behavior.

The generated macOS app plist includes `NSLocalNetworkUsageDescription` for
both editions. When connecting, the UI explains the initial permission request;
if access remains blocked, the error points to Local Network settings. The
application does not change system network policies or privacy settings.
This follows [Apple TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy),
which recommends appropriate retry logic for BSD sockets because the initial
operation can be denied before permission processing completes.

1. Click SSH and enter the host/IP, port (22 by default) and username. Choose
   password login or local-key login. For a key, use **选择…** to load a private
   key (not its `.pub` file), or enter its local path; `~/` is supported.
   Enter the key's passphrase if it is encrypted and not already unlocked.
2. For an unknown server, compare its SHA-256 fingerprint with the administrator's
   fingerprint and explicitly choose **信任并连接**. A changed known key is rejected.
3. Navigate using double-click, Enter, breadcrumbs, parent/home buttons or the
   editable path. Filter the current directory or show hidden entries as needed.
4. Choose **加载此文件夹** to show all immediate directories and files in the
   sidebar, even when there are no images at this level. Loading a directory
   does not download or open a first image. Enter child directories, go up,
   refresh, filter, or show another 100 entries at a time. Click an image to
   fetch and open that individual file; Ctrl/Command-click keeps Pro tab routing.
5. MDOC and other recognized text files open in a read-only viewer (up to
   256 KiB). MDOC shows tilt-record/movie counts and lets the user locate
   a referenced movie by basename, handling Windows source paths and looking
   in the MDOC directory, its parent and immediate child directories. Multiple
   matches require the user to choose a directory. Resolving a reference does
   not download its movie. Unknown binary files remain visible with properties.
   Falcon `.gain` files are opened as TIFF or MRC according to their header.
   Merely opening a movie/reference does not apply gain correction or run a
   Cryo-ET processing pipeline.

Directory navigation reads metadata, then visible image rows request 256-pixel
previews. The normal sidebar requests the same previews for unopened files.
Only PNG previews cross the connection; full image contents transfer when
explicitly opened, with byte progress and cancellation. Preview failures keep
a placeholder and do not prevent opening the image.
Downloaded images use local cache paths, so existing native readers, annotations
and Pro image tabs operate on the local copy. Original remote images are never written.

Only connection details, authentication mode and the selected key's path are
remembered (up to four recent connections). Passwords and key passphrases are
cleared after authentication and never written to settings. Private key
contents are read locally; only SSH authentication proofs are sent. If an
encrypted key is already unlocked in the local SSH agent, the plugin can use
that identity only when it exactly matches the selected key's `.pub` sibling.
SSH and SFTP run in libssh2 through the `ssh2` crate. Previews use a bundled
Linux x86_64 musl binary (about 5 MB), built from the same `thumb` and format
readers without desktop, GTK or GPU features. It requires no Python, package
installation, shared C runtime or background service. On first use it is uploaded
with mode 0700 into `~/.cache/tifmrcview/ssh-preview/`; its SHA-256 is checked
before every invocation. Remote writes remain limited to this private cache. Image paths travel as
JSON over stdin, not interpolated shell commands; credentials never enter
process arguments. The helper generates one PNG and exits, with a 12-second
watchdog, 768 MiB address-space cap, the existing 128 MiB plane decode limit
and a 512 KiB output limit. The app serializes preview requests, prioritizes
explicit transfers over queued previews, and caches by helper version, path,
file size and mtime. It rejects changed files.

The helper covers built-in TIFF/gain, MRC/REC/ST/MAP/ALI and
PNG/JPEG/GIF/BMP/WebP readers. For another format, the client provisions only
the already enabled, validated local format package that handles that extension
into a content-addressed cache. The helper verifies the manifest/WASM hashes
and the original minimum-app-version requirement before using the existing
WASM sandbox. JavaScript is not executed. It does not discover or load plugins
from the server environment. Formats without a matching enabled local plugin
and non-x86_64 Linux servers keep placeholders. Preview cache identity also
includes the exact format-package content. An `noexec` home/cache or SFTP-only account also
keeps placeholders. The full-image open path is unaffected.

Build the helper using `cargo build --release --no-default-features --target
x86_64-unknown-linux-musl --bin tmv-ssh-preview`. Set `TMV_SSH_PREVIEW_HELPER`
to that ELF file while building the desktop app; build.rs embeds it in the
executable, so updates include the matching helper. CI builds the helper before
all desktop bundles. Local release packaging requires the helper path.

Click outside the dialog (both press and release) or press Escape to close it
without disconnecting. Dragging from inside onto the backdrop does not close it.
Closing while a host-trust prompt is pending rejects that prompt; it never
implicitly trusts a server.

The backend checks OpenSSH known_hosts and its own trusted-host file before
sending the password. Trust confirmation and authentication use the same SSH
session. Host key mismatches cannot be overridden in the connection dialog.

The app's config directory contains `com.leehow.tifmrcview/ssh_known_hosts`.
Image snapshots are stored below the platform cache directory in
`com.leehow.tifmrcview/ssh/`. Caches are retained after disconnect, so images
already open remain usable. They can be removed with the app closed. Cache
identity includes the connection, remote path, file size and modification time.
If a remote file changes after listing, refresh the remote directory before
opening its new snapshot. Partial/cancelled transfers are removed.

This initial implementation opens individual self-contained images in a folder.
It does not mount a network drive, recursively download a directory, assemble
a remote folder into a Z stack, fetch multi-file format dependencies, or sync
local edits back to the server. Servers must support SFTP and either password
or public-key login. Unrestricted agent login, agent forwarding and interactive
multi-factor prompts are not exposed.

## Validation

- `cargo check --offline --locked --manifest-path src-tauri/Cargo.toml --lib`
  checks the desktop integration.
- `cargo test --offline --manifest-path src-tauri/Cargo.toml --lib
  --no-default-features --features ssh plugins::ssh::tests` runs transport unit tests.
- `cargo test --offline --manifest-path src-tauri/Cargo.toml --lib
  --no-default-features --features ssh plugins::ssh::tcp::tests` deterministically
  replays the observed errno 65, delayed permission resolution, a persistent
  denial deadline, address fallback, and non-retryable failures without sleeps.
- `python3 scripts/test-ssh-native.py` uses an existing Paramiko installation
  to start an isolated read-only loopback fixture. It verifies real password
  login, unencrypted/encrypted private-key login, rejection of a wrong key
  passphrase, trust confirmation, mismatching keys, UTF-8 names, directory/file links,
  byte-exact transfer, cached MRC decoding and pixel values, cache reuse,
  stale metadata rejection, cancellation and
  disconnect. It never touches the user's servers or trusted-host records.
- `node --test scripts/image-ownership.test.mjs scripts/tab-rules.test.mjs`
  covers local opens, asynchronous remote reads and Pro tab routing rules.
- With existing Playwright and Chromium, run the normal sandbox browser harness
  with `TMV_TEST_SSH_ONLY=1 node scripts/test-plugin-sandbox.cjs`. Set `NODE_PATH`
  and `TMV_TEST_CHROMIUM` to their installed locations if needed.
  `TMV_SSH_SCREENSHOT_DIR` optionally saves connection/browser screenshots.
  The SSH scenario uses mocked native replies; it validates UI and lazy-load
  routing, not native image decoding or a platform WebView.

The full existing sandbox harness currently expects the old combined settings
menu item “插件与更新”; current main separates 更新 / 插件 / 关于, so that
unrelated menu assertion fails before reaching later sandbox scenarios. The
SSH-only path uses the same startup and real host/plugin code and skips that
obsolete menu expectation. No installed-app, Windows/Linux runtime, or live
Pro GUI acceptance is claimed by these checks.

Implementation precedent: [ssh2 SFTP API](https://docs.rs/ssh2/latest/ssh2/struct.Sftp.html)
and [libssh2's SFTP example](https://libssh2.org/examples/sftp.html) both separate
host-key verification, authentication and SFTP reads. The browser follows that
ordering and keeps the verified session alive through the trust prompt.
