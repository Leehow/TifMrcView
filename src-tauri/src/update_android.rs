//! Android has no in-app updater: new versions come as an APK from the site.

use tauri::State;

#[derive(Default)]
pub struct Pending;

#[tauri::command]
pub async fn check_update(_pending: State<'_, Pending>) -> Result<Option<()>, String> {
    Ok(None)
}

#[tauri::command]
pub async fn install_update(_pending: State<'_, Pending>) -> Result<(), String> {
    Err("Android 版请到官网下载新的 APK".into())
}
