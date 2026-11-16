//! Plugin host.
//!
//! A plugin is a JavaScript file that calls `TifMrcView.register({...})`
//! (see `ui/plugin-host.js`). Plugins that need native code, such as the
//! MP4 recorder, also have a Rust half implementing [`NativePlugin`]; the
//! UI half reaches it through the `plugin_call` command.
//!
//! `ui/plugins/i18n.js` and the optional SSH browser are built in. Other plugins are installed as
//! packages (see [`package`]) into `<plugin dir>/<id>/`; loose `.js` files in
//! the plugin dir still load too. Both are served to the webview through the
//! `tmvplugin://` scheme. The native halves are compiled in but stay dormant
//! until a package with their id is installed. SSH is the explicit built-in
//! exception; its connection state is created only when the user connects.
//!
//! Packages can also carry a sandboxed WebAssembly module that opens new
//! file formats or reads vendor metadata (see [`registry`] and
//! `docs/format-plugins.md`); that is how Zeiss CZI and SmartSEM support
//! ship, so no vendor format code lives in the core.

#[cfg(feature = "recorder")]
mod mp4;
#[cfg(feature = "desktop")]
pub mod market;
pub mod package;
pub mod recipe;
#[cfg(feature = "recorder")]
pub(crate) mod recorder;
pub mod registry;
pub mod wasm;

pub mod paths;

#[cfg(all(feature = "ssh", not(target_os = "android")))]
pub mod ssh;

#[cfg(feature = "desktop")]
include!("desktop.rs");
