//! Shared helpers for the end-to-end plugin tests: building the WebAssembly
//! plugins from `plugins/<id>/` and installing them into a temporary
//! plugin folder.

#![allow(dead_code)]

pub mod czi;
pub mod imaris;
pub mod lasx;
pub mod lsm;
pub mod nis;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use tifmrcview_lib::plugins::registry::Registry;

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn wasm_target_installed() -> bool {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    Command::new(rustc)
        .args(["--print", "target-libdir", "--target", "wasm32-unknown-unknown"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .is_some_and(|d| d.join("libcore").exists() || std::fs::read_dir(&d).is_ok_and(|mut r| r.next().is_some()))
}

/// Builds `plugins/<id>` for wasm32 (once per test run) and returns the
/// .wasm, or `None` (with a note) when the wasm32 target is not installed.
pub fn build_plugin(id: &str) -> Option<PathBuf> {
    static BUILT: OnceLock<Mutex<std::collections::HashMap<String, Option<PathBuf>>>> = OnceLock::new();
    let mut built = BUILT.get_or_init(Default::default).lock().unwrap();
    if let Some(p) = built.get(id) {
        return p.clone();
    }
    let out = if !wasm_target_installed() {
        eprintln!("SKIPPED: the wasm32-unknown-unknown target is not installed (rustup target add wasm32-unknown-unknown)");
        None
    } else {
        let target = std::env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
            .join("wasm-plugins");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let manifest = repo().join("plugins").join(id).join("Cargo.toml");
        let mut cmd = Command::new(cargo);
        cmd.args(["build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&target)
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTFLAGS");
        // The plugin's own release profile applies, whatever the app build uses.
        for (k, _) in std::env::vars_os() {
            if k.to_string_lossy().starts_with("CARGO_PROFILE_") {
                cmd.env_remove(k);
            }
        }
        let status = cmd.status().expect("run cargo");
        assert!(status.success(), "building the {id} plugin failed");
        let name = format!("tmv_plugin_{}.wasm", id.replace('-', "_"));
        Some(target.join("wasm32-unknown-unknown/release").join(name))
    };
    built.insert(id.into(), out.clone());
    out
}

/// A plugin folder with the given plugins installed, as the app would have
/// it after "Install plugin…". `None` when the plugins cannot be built.
pub struct Plugins {
    pub dir: tempfile::TempDir,
    pub registry: Registry,
}

pub fn install(ids: &[&str]) -> Option<Plugins> {
    let dir = tempfile::tempdir().unwrap();
    for id in ids {
        let wasm = build_plugin(id)?;
        let src = repo().join("plugins").join(id);
        let dst = dir.path().join(id);
        std::fs::create_dir_all(&dst).unwrap();
        let manifest = std::fs::read_to_string(src.join("plugin.json")).unwrap();
        let m: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        std::fs::write(dst.join("plugin.json"), &manifest).unwrap();
        std::fs::copy(src.join("index.js"), dst.join("index.js")).unwrap();
        std::fs::copy(&wasm, dst.join(m["wasm"].as_str().unwrap())).unwrap();
    }
    let registry = Registry::new(Some(dir.path().to_path_buf()), env!("CARGO_PKG_VERSION"));
    Some(Plugins { dir, registry })
}
