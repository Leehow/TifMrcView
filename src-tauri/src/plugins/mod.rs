//! Plugin host.
//!
//! A plugin is a JavaScript file that calls `TifMrcView.register({...})`
//! (see `ui/plugin-host.js`). Plugins that need native code, such as the
//! MP4 recorder, also have a Rust half implementing [`NativePlugin`]; the
//! UI half reaches it through the `plugin_call` command.
//!
//! Only `ui/plugins/i18n.js` is built in. Other plugins are installed as
//! packages (see [`package`]) into `<plugin dir>/<id>/`; loose `.js` files in
//! the plugin dir still load too. Both are served to the webview through the
//! `tmvplugin://` scheme. The native halves are compiled in but stay dormant
//! until a package with their id is installed.
//!
//! Packages can also carry a sandboxed WebAssembly module that opens new
//! file formats or reads vendor metadata (see [`registry`] and
//! `docs/format-plugins.md`); that is how Zeiss CZI and SmartSEM support
//! ship, so no vendor format code lives in the core.

#[cfg(feature = "recorder")]
mod mp4;
pub mod market;
pub mod package;
pub mod recipe;
#[cfg(feature = "recorder")]
mod recorder;
pub mod registry;
pub mod wasm;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, Runtime, State, UriSchemeContext};

/// Where 插件市场 points when the updater endpoint gives no hint.
const MARKET_URL: &str = "https://tifmrcview.com/tifmrcview/plugins/";

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
        let mut list: Vec<Arc<dyn NativePlugin>> = vec![];
        #[cfg(feature = "recorder")]
        list.push(Arc::new(recorder::Recorder::default()));
        Plugins { native: list.into_iter().map(|p| (p.id(), p)).collect(), dir }
    }

    fn dir(&self) -> Result<&Path, String> {
        self.dir.as_deref().ok_or_else(|| "no plugin folder".into())
    }
}

/// Folder for user plugins, created on demand.
pub fn user_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("plugins"))
}

fn app_version<R: Runtime>(app: &AppHandle<R>) -> String {
    app.package_info().version.to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginScan {
    /// Ids of the native plugins compiled into this build.
    native: Vec<&'static str>,
    /// Installed packages, including broken ones (with `error`).
    packages: Vec<package::Installed>,
    /// File names of loose (legacy) user plugins in `dir`.
    user: Vec<String>,
    dir: String,
}

#[tauri::command]
pub fn plugin_scan<R: Runtime>(app: AppHandle<R>, plugins: State<'_, Plugins>) -> PluginScan {
    // A reload may follow files copied into the folder by hand.
    registry::Registry::global().invalidate();
    let mut user = vec![];
    let mut packages = vec![];
    if let Some(dir) = &plugins.dir {
        let _ = std::fs::create_dir_all(dir);
        packages = package::scan(dir, &app_version(&app));
        if let Ok(rd) = std::fs::read_dir(dir) {
            user = rd
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
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
        packages,
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
    // Native code only runs for plugins the user installed.
    if !plugins.dir.as_deref().is_some_and(|d| package::is_installed(d, &id)) {
        return Err(format!("插件 {id} 没有安装，请先在插件市场下载安装"));
    }
    tauri::async_runtime::spawn_blocking(move || plugin.call(&method, args, &body))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

/// Asks for a plugin package (.zip).
#[tauri::command]
pub async fn pick_plugin_package(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .add_filter("TifMrcView 插件包", &["zip"])
        .blocking_pick_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Installs or upgrades the package at `path`; returns its plugin.json.
#[tauri::command]
pub async fn plugin_install(app: AppHandle, plugins: State<'_, Plugins>, path: String) -> Result<Value, String> {
    let dir = plugins.dir()?.to_path_buf();
    let version = app_version(&app);
    let res = tauri::async_runtime::spawn_blocking(move || {
        package::install_checked(&dir, Path::new(&path), &version, registry::validate_package)
    })
    .await
    .map_err(|e| e.to_string())?;
    registry::Registry::global().invalidate();
    res.map(|m| m.raw).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn plugin_uninstall(plugins: State<'_, Plugins>, id: String) -> Result<(), String> {
    let res = package::uninstall(plugins.dir()?, &id).map_err(|e| format!("{e:#}"));
    registry::Registry::global().invalidate();
    res
}

/// Tells the backend which plugins the user switched off ("plugins.disabled"
/// in the UI), so format and metadata plugins follow the same switches.
#[tauri::command]
pub fn plugin_set_disabled(ids: Vec<String>) -> Result<(), String> {
    registry::Registry::global().set_disabled(ids).map_err(|e| format!("{e:#}"))
}

/// Shows the user plugin folder in the file manager.
#[tauri::command]
pub fn open_plugin_dir(plugins: State<'_, Plugins>) -> Result<(), String> {
    let dir = plugins.dir()?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";
    std::process::Command::new(opener).arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// The plugin market sits next to the updater feed: `…/latest.json` → `…/plugins/`.
fn market_url(updater: Option<&Value>) -> String {
    let endpoint = updater.and_then(|u| u["endpoints"][0].as_str());
    match endpoint.and_then(|e| e.strip_suffix("latest.json")) {
        Some(base) if base.starts_with("https://") && base.ends_with('/') => format!("{base}plugins/"),
        _ => MARKET_URL.into(),
    }
}

/// Opens the plugin market in the default browser. Only ever that one URL.
#[tauri::command]
pub fn open_market<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let url = market_url(app.config().plugins.0.get("updater"));
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    // `explorer <url>` can open a folder window instead; url.dll hands the URL
    // to the default browser without going through a shell.
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// The market's `plugins.json`, fetched once per session.
#[derive(Default)]
pub struct MarketCache(Mutex<Option<Value>>);

fn market_base<R: Runtime>(app: &AppHandle<R>) -> String {
    market_url(app.config().plugins.0.get("updater"))
}

fn user_agent<R: Runtime>(app: &AppHandle<R>) -> String {
    format!("TifMrcView/{}", app_version(app))
}

/// The market's packages and whether each is installed, up to date or
/// needs a newer app. The list is cached unless `refresh` is set.
#[tauri::command]
pub async fn plugin_market_list(
    app: AppHandle,
    plugins: State<'_, Plugins>,
    cache: State<'_, MarketCache>,
    refresh: Option<bool>,
) -> Result<Vec<market::Listed>, String> {
    let cached = if refresh == Some(true) { None } else { cache.0.lock().unwrap().clone() };
    let index = match cached {
        Some(i) => i,
        None => {
            let url = format!("{}plugins.json", market_base(&app));
            let bytes = market::http_get(url, market::MAX_INDEX, true, &user_agent(&app))
                .await
                .map_err(|e| format!("{e:#}"))?;
            let index = market::parse_index(&bytes).map_err(|e| format!("{e:#}"))?;
            *cache.0.lock().unwrap() = Some(index.clone());
            index
        }
    };
    let dir = plugins.dir.clone();
    let version = app_version(&app);
    tauri::async_runtime::spawn_blocking(move || market::list(&index, dir.as_deref(), &version))
        .await
        .map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
struct MarketInstalled {
    manifest: Value,
    /// Label of the window that asked; it reloads if the plugin was loaded.
    origin: String,
}

/// Downloads `id` from the official market, checks it against the index
/// (size, SHA-256, minAppVersion) and installs it like 加载插件… does. Every
/// window then gets a `plugin-installed` event to load it.
#[tauri::command]
pub async fn plugin_install_from_market(
    app: AppHandle,
    window: tauri::WebviewWindow,
    plugins: State<'_, Plugins>,
    cache: State<'_, MarketCache>,
    id: String,
) -> Result<Value, String> {
    if !package::valid_id(&id) {
        return Err(format!("插件 id 不对：{id}"));
    }
    let dir = plugins.dir()?.to_path_buf();
    let version = app_version(&app);
    let ua = user_agent(&app);
    let fetch = |url, max| market::http_get(url, max, true, &ua);
    let (index, entry, bytes) =
        market::download(fetch, &market_base(&app), &id, &version).await.map_err(|e| format!("{e:#}"))?;
    *cache.0.lock().unwrap() = Some(index);
    let res = tauri::async_runtime::spawn_blocking(move || market::install_bytes(&dir, &entry, &bytes, &version))
        .await
        .map_err(|e| e.to_string())?;
    registry::Registry::global().invalidate();
    let m = res.map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("plugin-installed", MarketInstalled { manifest: m.raw.clone(), origin: window.label().into() });
    Ok(m.raw)
}

/// Serves the user plugin folder: `tmvplugin://localhost/<file>.js` (loose
/// plugins) and `tmvplugin://localhost/<id>/<path>` (files of a package).
pub fn serve<R: Runtime>(ctx: UriSchemeContext<'_, R>, req: tauri::http::Request<Vec<u8>>) -> Response<Vec<u8>> {
    let not_found = || Response::builder().status(StatusCode::NOT_FOUND).body(vec![]).unwrap();
    let path = crate::percent_decode(req.uri().path().trim_start_matches('/'));
    let Some(dir) = ctx.app_handle().try_state::<Plugins>().and_then(|p| p.dir.clone()) else {
        return not_found();
    };
    // Never walks out of the folder (see `package::resolve`).
    let Some((file, mime)) = package::resolve(&dir, &path) else {
        return not_found();
    };
    match std::fs::read(file) {
        Ok(data) => Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "no-store")
            // The page (tauri://localhost) is another origin; lets plugins fetch their own files.
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(data)
            .unwrap(),
        Err(_) => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_next_to_update_feed() {
        let conf: Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(market_url(conf["plugins"].get("updater")), "https://tifmrcview.com/tifmrcview/plugins/");
        let other = serde_json::json!({"endpoints": ["http://evil/latest.json"]});
        assert_eq!(market_url(Some(&other)), MARKET_URL);
        assert_eq!(market_url(None), MARKET_URL);
    }
}
