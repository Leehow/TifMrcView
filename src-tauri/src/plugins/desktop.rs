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
pub fn user_dir<R: Runtime>(_app: &AppHandle<R>) -> Option<PathBuf> {
    paths::user_dir()
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
    crate::plugin_assoc::sync_plugin_formats();
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

/// Inspect a zip before confirmation. Only the backend-fetched official index
/// can authorize an API 1 package; never accept an index from the frontend.
#[tauri::command]
pub async fn plugin_inspect_package(app: AppHandle, cache: State<'_, MarketCache>, path: String) -> Result<Value, String> {
    let version = app_version(&app);
    let index = cache.0.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = package::read_archive(Path::new(&path))?;
        let m = inspect_installable(&bytes, index.as_ref(), &version)?;
        Ok(serde_json::json!({"manifest": m.raw, "sha256": market::sha256(&bytes)}))
    }).await.map_err(|e| e.to_string())?.map_err(|e: anyhow::Error| format!("{e:#}"))
}

fn inspect_installable(bytes: &[u8], index: Option<&Value>, version: &str) -> anyhow::Result<package::Manifest> {
    let m = package::inspect(bytes, version)?;
    let entry = index.and_then(|i| market::find_entry(i, &m.id, version).ok());
    market::check_install_policy(&m, bytes, entry.as_ref())?;
    Ok(m)
}

/// Re-read and hash one snapshot after confirmation, then extract those bytes.
#[tauri::command]
pub async fn plugin_install(app: AppHandle, plugins: State<'_, Plugins>, cache: State<'_, MarketCache>, path: String, sha256: String) -> Result<Value, String> {
    let dir = plugins.dir()?.to_path_buf();
    let version = app_version(&app);
    let index = cache.0.lock().unwrap().clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let bytes = package::read_archive(Path::new(&path))?;
        install_confirmed_zip(&dir, &bytes, &sha256, index.as_ref(), &version)
    })
    .await
    .map_err(|e| e.to_string())?;
    registry::Registry::global().invalidate();
    crate::plugin_assoc::sync_plugin_formats();
    res.map(|m| m.raw).map_err(|e| format!("{e:#}"))
}

fn install_confirmed_zip(dir: &Path, bytes: &[u8], sha256: &str, index: Option<&Value>, version: &str) -> anyhow::Result<package::Manifest> {
    if market::sha256(bytes) != sha256 {
        anyhow::bail!("插件包已改变，请重新选择并确认安装");
    }
    inspect_installable(bytes, index, version)?;
    let m = package::install_data_checked(dir, bytes, version, registry::validate_package)?;
    // Only the official package may remove the plugins it replaces; any
    // other only hides them (see the registry).
    if market::is_official(index, &m, bytes, version) {
        package::retire_replaced(dir, &m);
    }
    Ok(m)
}

#[tauri::command]
pub fn plugin_uninstall(plugins: State<'_, Plugins>, id: String) -> Result<(), String> {
    let res = package::uninstall(plugins.dir()?, &id).map_err(|e| format!("{e:#}"));
    registry::Registry::global().invalidate();
    crate::plugin_assoc::sync_plugin_formats();
    res
}

/// Tells the backend which plugins the user switched off ("plugins.disabled"
/// in the UI), so format and metadata plugins follow the same switches.
#[tauri::command]
pub fn plugin_set_disabled(ids: Vec<String>) -> Result<(), String> {
    registry::Registry::global().set_disabled(ids).map_err(|e| format!("{e:#}"))?;
    crate::plugin_assoc::sync_plugin_formats();
    Ok(())
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

/// Markets sit next to either updater feed, in endpoint order.
fn market_urls(updater: Option<&Value>) -> Vec<String> {
    let mut bases = Vec::new();
    if let Some(endpoints) = updater.and_then(|u| u["endpoints"].as_array()) {
        for endpoint in endpoints.iter().filter_map(Value::as_str) {
            // latest.json, latest-direct.json, and Pro's latest-pro(-direct).json share one folder.
            let base = ["latest.json", "latest-direct.json", "latest-pro.json", "latest-pro-direct.json"]
                .iter()
                .find_map(|feed| endpoint.strip_suffix(feed));
            if let Some(base) = base.filter(|b| b.starts_with("https://") && b.ends_with('/')) {
                let url = format!("{base}plugins/");
                if !bases.contains(&url) { bases.push(url); }
            }
        }
    }
    if bases.is_empty() { bases.push(MARKET_URL.into()); }
    bases
}

/// Browser navigation uses the last (main site / Cloudflare) endpoint.
fn market_url(updater: Option<&Value>) -> String {
    market_urls(updater).pop().unwrap()
}

/// Opens the plugin market in the default browser. Only ever that one URL.
#[tauri::command]
pub fn open_market<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    open_in_browser(&market_url(app.config().plugins.0.get("updater")))
}

/// Opens a link from the About dialog. Only https: and mailto: links, so the
/// page can never start a local program or open a file.
#[tauri::command]
pub fn open_link(url: String) -> Result<(), String> {
    let ok = (url.starts_with("https://") || url.starts_with("mailto:"))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"');
    if !ok { return Err("unsupported link".into()); }
    open_in_browser(&url)
}

/// Opens the website (download, manual) from 帮助. Only ever that one URL.
#[tauri::command]
pub fn open_website() -> Result<(), String> {
    open_in_browser("https://tifmrcview.com/")
}

fn open_in_browser(url: &str) -> Result<(), String> {
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

fn market_bases<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
    market_urls(app.config().plugins.0.get("updater"))
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
            let ua = user_agent(&app);
            let index = market::fetch_index(|url, max| market::http_get(url, max, true, &ua), &market_bases(&app))
                .await.map_err(|e| format!("{e:#}"))?;
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
    approval: Value,
) -> Result<Value, String> {
    if !package::valid_id(&id) {
        return Err(format!("插件 id 不对：{id}"));
    }
    let dir = plugins.dir()?.to_path_buf();
    let version = app_version(&app);
    let ua = user_agent(&app);
    let fetch = |url, max| market::http_get(url, max, true, &ua);
    let (index, entry, bytes) =
        market::download_from_bases(fetch, &market_bases(&app), &id, &version).await.map_err(|e| format!("{e:#}"))?;
    let published = market::entries(&index).unwrap().iter().find(|p| p["id"].as_str() == Some(&id));
    if !published.is_some_and(|p| market::matches_confirmation(p, &approval)) {
        return Err("插件市场的信息已改变，请重新确认安装".into());
    }
    *cache.0.lock().unwrap() = Some(index);
    let res = tauri::async_runtime::spawn_blocking(move || market::install_bytes(&dir, &entry, &bytes, &version))
        .await
        .map_err(|e| e.to_string())?;
    registry::Registry::global().invalidate();
    crate::plugin_assoc::sync_plugin_formats();
    let m = res.map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("plugin-installed", MarketInstalled { manifest: m.raw.clone(), origin: window.label().into() });
    Ok(m.raw)
}

/// Serves the user plugin folder: `tmvplugin://localhost/<file>.js` (loose
/// plugins) and `tmvplugin://localhost/<id>/<path>` (files of a package).
pub fn serve<R: Runtime>(ctx: UriSchemeContext<'_, R>, req: tauri::http::Request<Vec<u8>>) -> Response<Vec<u8>> {
    let not_found = || Response::builder().status(StatusCode::NOT_FOUND).body(vec![]).unwrap();
    let path = crate::percent_decode(req.uri().path().trim_start_matches('/'));
    // Reserved core resources cannot be replaced by installed plugin files.
    if path == "__sandbox/runtime.js" {
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/javascript; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header("X-Content-Type-Options", "nosniff")
            .body(include_bytes!("../../../ui/sandbox/tmv-plugin-api.js").to_vec())
            .unwrap();
    }
    let Some(dir) = ctx.app_handle().try_state::<Plugins>().and_then(|p| p.dir.clone()) else {
        return not_found();
    };
    // Only the core runtime is served here. Sandbox documents are srcdoc so
    // their frame URL cannot acquire a registered custom protocol's Local IPC.
    if path.starts_with("__sandbox/") {
        return not_found();
    }
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
    fn zip_inspection_does_not_install_and_confirmation_pins_bytes() {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("plugin.json", opts).unwrap();
        zip.write_all(br#"{"id":"demo","name":"Demo","version":"1.0.0","apiVersion":2,"permissions":[]}"#).unwrap();
        zip.start_file("index.js", opts).unwrap(); zip.write_all(b"// demo").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins");
        let m = inspect_installable(&bytes, None, "0.5.0").unwrap();
        assert_eq!(m.id, "demo");
        assert!(!dir.exists(), "inspection must not create an install directory");
        assert!(install_confirmed_zip(&dir, &bytes, "old-digest", None, "0.5.0").is_err());
        assert!(!dir.exists(), "changed bytes must be rejected before extraction");
        install_confirmed_zip(&dir, &bytes, &market::sha256(&bytes), None, "0.5.0").unwrap();
        assert!(package::is_installed(&dir, "demo"));
    }

    #[test]
    fn market_next_to_update_feed() {
        let conf: Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(market_url(conf["plugins"].get("updater")), "https://tifmrcview.com/tifmrcview/plugins/");
        assert_eq!(market_urls(conf["plugins"].get("updater")), vec![
            "https://dl.tifmrcview.com:9443/tifmrcview/plugins/", MARKET_URL,
        ]);
        // Pro uses the same market.
        let pro: Value = serde_json::from_str(include_str!("../../tauri.pro.conf.json")).unwrap();
        assert_eq!(market_urls(pro["plugins"].get("updater")), market_urls(conf["plugins"].get("updater")));
        let repeated = serde_json::json!({"endpoints": [
            "https://direct.test/latest-direct.json", "https://direct.test/latest.json",
            "http://evil/latest.json", "https://main.test/latest.json",
        ]});
        assert_eq!(market_urls(Some(&repeated)), vec!["https://direct.test/plugins/", "https://main.test/plugins/"]);
        assert_eq!(market_url(Some(&repeated)), "https://main.test/plugins/");
        let other = serde_json::json!({"endpoints": ["http://evil/latest.json"]});
        assert_eq!(market_url(Some(&other)), MARKET_URL);
        assert_eq!(market_url(None), MARKET_URL);
        assert_eq!(market_urls(Some(&other)), vec![MARKET_URL]);
        assert_eq!(market_urls(None), vec![MARKET_URL]);
    }
}
