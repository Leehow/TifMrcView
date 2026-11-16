# SSH remote folders

The desktop host includes **SSH 远程文件夹**, a trusted plugin that can be
enabled or disabled in the plugin dialog. When enabled, an SSH button appears
in the top toolbar. This is bundled functionality, not a separately published
market package. Sandboxed third-party plugins receive no new permissions.
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
4. Choose **加载此文件夹** to put its image files in the normal sidebar and open
   the first image. Alternatively select/double-click an individual image.

Browsing directories only reads directory metadata. Image contents transfer
when explicitly opened, with byte progress and cancellation. Unopened sidebar
items display an SSH placeholder rather than downloading files for thumbnails.
Downloaded images use local cache paths, so existing native readers, annotations
and Pro image tabs operate on the local copy. Remote data is never written.

Only connection details, authentication mode and the selected key's path are
remembered (up to four recent connections). Passwords and key passphrases are
cleared after authentication and never written to settings. Private key
contents are read locally; only SSH authentication proofs are sent. If an
encrypted key is already unlocked in the local SSH agent, the plugin can use
that identity only when it exactly matches the selected key's `.pub` sibling.
SSH and SFTP run in libssh2 through the `ssh2` crate, without shell commands,
sshpass, a remote Python installation, or credentials in process arguments.
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
