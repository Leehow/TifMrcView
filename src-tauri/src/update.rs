//! Auto-update through `tauri-plugin-updater`.
//!
//! The feed (`plugins.updater.endpoints` in tauri.conf.json) is a
//! `latest.json` that CI publishes to the download site.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    current: String,
    version: String,
    notes: Option<String>,
    date: Option<String>,
}

#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    pending: State<'_, Pending>,
) -> Result<Option<UpdateInfo>, String> {
    let update = app
        .updater_builder()
        .timeout(Duration::from_secs(300))
        .configure_client(|client| {
            client
                .connect_timeout(Duration::from_secs(20))
                .read_timeout(Duration::from_secs(60))
        })
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let info = update.as_ref().map(|u| UpdateInfo {
        current: u.current_version.clone(),
        version: u.version.clone(),
        notes: u.body.clone(),
        date: u.date.map(|d| d.to_string()),
    });
    *pending.0.lock().unwrap() = update;
    Ok(info)
}

#[derive(Clone, Serialize)]
struct Progress {
    done: u64,
    total: Option<u64>,
}

/// Downloads and installs the update found by `check_update`, then restarts.
#[tauri::command]
pub async fn install_update(app: AppHandle, pending: State<'_, Pending>) -> Result<(), String> {
    let update = pending.0.lock().unwrap().take().ok_or("没有可安装的更新")?;
    let mut done = 0u64;
    let progress_app = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                done += chunk as u64;
                let _ = progress_app.emit("update-progress", Progress { done, total });
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}
