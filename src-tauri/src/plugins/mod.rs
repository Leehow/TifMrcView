//! Plugin host.
//!
//! A plugin is a JavaScript file that calls `TifMrcView.register({...})`
//! (see `ui/plugin-host.js`). Plugins that need native code, such as the
//! MP4 recorder, also have a Rust half implementing [`NativePlugin`]; the
//! UI half reaches it through the `plugin_call` command.
//!
//! Built-in plugins live in `ui/plugins/`. Users can drop extra `.js`
//! plugins into the folder returned by `plugin_dir`; they are served to the
//! webview through the `tmvplugin://` scheme.

#[cfg(feature = "recorder")]
mod mp4;
#[cfg(feature = "recorder")]
mod recorder;
mod smartsem;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime, State, UriSchemeContext};

/// The Rust half of a plugin.
pub trait NativePlugin: Send + Sync {
    /// Matches the `id` the JavaScript half registers with.
    fn id(&self) -> &'static str;
    /// Handles `plugin_call`. `body` carries bulk binary data, e.g. frames.
    /// Runs on a blocking thread.
    fn call(&self, method: &str, args: Value, body: &[u8]) -> anyhow::Result<Value>;
}

pub struct Plugins {
    native: HashMap<&'static str, Arc<dyn NativePlugin>>,
    dir: Option<PathBuf>,
}

impl Plugins {
    #[allow(unused_mut, clippy::vec_init_then_push)] // entries depend on enabled features
    pub fn new(dir: Option<PathBuf>) -> Self {
        #[allow(unused_mut)]
        let mut list: Vec<Arc<dyn NativePlugin>> = vec![Arc::new(smartsem::SmartSem)];
        #[cfg(feature = "recorder")]
        list.push(Arc::new(recorder::Recorder::default()));
        Plugins { native: list.into_iter().map(|p| (p.id(), p)).collect(), dir }
    }
}

/// Folder for user plugins, created on demand.
pub fn user_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("plugins"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginScan {
    /// Ids of the native plugins compiled into this build.
    native: Vec<&'static str>,
    /// File names of user plugins in `dir`.
    user: Vec<String>,
    dir: String,
}

#[tauri::command]
pub fn plugin_scan(plugins: State<'_, Plugins>) -> PluginScan {
    let mut user = vec![];
    if let Some(dir) = &plugins.dir {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(rd) = std::fs::read_dir(dir) {
            user = rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".js") && !n.starts_with('.'))
                .collect();
        }
    }
    user.sort();
    let mut native: Vec<_> = plugins.native.keys().copied().collect();
    native.sort();
    PluginScan {
        native,
        user,
        dir: plugins.dir.as_ref().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

/// Calls a native plugin. Arguments travel as percent-encoded JSON in the
/// `x-args` header so the body can stay raw binary.
#[tauri::command]
pub async fn plugin_call(plugins: State<'_, Plugins>, request: tauri::ipc::Request<'_>) -> Result<Value, String> {
    let header = |k: &str| request.headers().get(k).and_then(|v| v.to_str().ok()).map(crate::percent_decode);
    let id = header("x-plugin").ok_or("missing plugin")?;
    let method = header("x-method").ok_or("missing method")?;
    let args: Value = match header("x-args") {
        Some(a) => serde_json::from_str(&a).map_err(|e| e.to_string())?,
        None => Value::Null,
    };
    let body = match request.body() {
        tauri::ipc::InvokeBody::Raw(b) => b.clone(),
        tauri::ipc::InvokeBody::Json(_) => vec![],
    };
    let plugin = plugins.native.get(id.as_str()).cloned().ok_or_else(|| format!("no native plugin {id}"))?;
    tauri::async_runtime::spawn_blocking(move || plugin.call(&method, args, &body))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

/// Shows the user plugin folder in the file manager.
#[tauri::command]
pub fn open_plugin_dir(plugins: State<'_, Plugins>) -> Result<(), String> {
    let dir = plugins.dir.as_ref().ok_or("no plugin folder")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";
    std::process::Command::new(opener).arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Serves `tmvplugin://localhost/<file>.js` from the user plugin folder.
pub fn serve<R: Runtime>(ctx: UriSchemeContext<'_, R>, req: tauri::http::Request<Vec<u8>>) -> Response<Vec<u8>> {
    let not_found = || Response::builder().status(StatusCode::NOT_FOUND).body(vec![]).unwrap();
    let name = crate::percent_decode(req.uri().path().trim_start_matches('/'));
    // Only plain file names; never walk out of the folder.
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') || !name.ends_with(".js") {
        return not_found();
    }
    let Some(dir) = ctx.app_handle().try_state::<Plugins>().and_then(|p| p.dir.clone()) else {
        return not_found();
    };
    match std::fs::read(dir.join(name)) {
        Ok(data) => Response::builder()
            .header(header::CONTENT_TYPE, "text/javascript; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store")
            .body(data)
            .unwrap(),
        Err(_) => not_found(),
    }
}
