//! Helpers for the Imaris (.ims) plugin tests: the synthetic fixtures made
//! by `plugins/imaris/tests/make_fixtures.py` and an installed plugin.

use std::path::PathBuf;

use tifmrcview_lib::plugins::registry::Registry;

use super::{build_plugin, repo, Plugins};

/// The pixel values of the fixtures (as `pattern()` in make_fixtures.py),
/// before the cast to the file's sample type.
pub fn pattern(c: u64, t: u64, z: u64, y: u64, x: u64, modulo: u64) -> u64 {
    (x * 7 + y * 13 + z * 101 + c * 1000 + t * 3000) % modulo
}

pub fn fixture(name: &str) -> PathBuf {
    repo().join("src-tauri/tests/fixtures").join(name)
}

/// The plugin needs app 0.5.0 (recipe `skip`); install it into a registry
/// that reports at least that version.
pub fn install() -> Option<Plugins> {
    let wasm = build_plugin("imaris")?;
    let dir = tempfile::tempdir().unwrap();
    let src = repo().join("plugins/imaris");
    let dst = dir.path().join("imaris");
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::copy(src.join("plugin.json"), dst.join("plugin.json")).unwrap();
    std::fs::copy(src.join("index.js"), dst.join("index.js")).unwrap();
    std::fs::copy(&wasm, dst.join("imaris.wasm")).unwrap();
    let registry = Registry::new(Some(dir.path().to_path_buf()), app_version());
    Some(Plugins { dir, registry })
}

/// This build's version, or 0.5.0 while the app is still older.
pub fn app_version() -> &'static str {
    let v: Vec<u32> = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    if v.as_slice() < [0, 5, 0].as_slice() {
        "0.5.0"
    } else {
        env!("CARGO_PKG_VERSION")
    }
}
