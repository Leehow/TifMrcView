//! Android glue. The Kotlin half (gen/android/.../TmvPlugin.kt) asks for
//! all-files access and turns the content:// links that pickers and "open
//! with" hand out into file paths, so the readers can map files and browse
//! their folders as on the desktop.

use serde::Deserialize;
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime};

struct Native<R: Runtime>(PluginHandle<R>);

#[derive(Deserialize)]
struct PathReply {
    path: Option<String>,
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("tmv")
        .setup(|app, api| {
            app.manage(Native(api.register_android_plugin("com.leehow.tifmrcview", "TmvPlugin")?));
            Ok(())
        })
        .build()
}

fn call<R: Runtime>(app: &AppHandle<R>, command: &str) -> Option<String> {
    let native = app.try_state::<Native<R>>()?;
    match native.0.run_mobile_plugin::<PathReply>(command, ()) {
        Ok(r) => r.path,
        Err(e) => {
            eprintln!("{command}: {e}");
            None
        }
    }
}

/// Shows the system file picker; the chosen file's path.
pub fn pick_file<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    call(app, "pickFile")
}

/// The file another app asked us to open, once.
pub fn take_opened<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    call(app, "takeOpened")
}

/// Shows the system folder picker; the chosen folder's path.
pub fn pick_folder<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    call(app, "pickFolder")
}
