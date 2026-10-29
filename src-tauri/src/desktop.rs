use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use formats::sequence::natural_cmp;
use image::{ImageInfo, OpenImage};

#[derive(Default)]
struct AppState {
    /// The file shown in each window, by window label.
    current: Mutex<HashMap<String, Arc<OpenImage>>>,
    next_token: AtomicU64,
    next_window: AtomicU64,
    /// Files the OS asked us to open that a window has not picked up yet.
    pending: Mutex<HashMap<String, Vec<String>>>,
    /// The XZ / YZ side view of each window's file, when one is shown.
    side: Mutex<HashMap<String, Arc<OpenImage>>>,
    /// The file each window shows or is loading, by window label.
    showing: Mutex<HashMap<String, String>>,
    thumbs: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    save_paths: SavePaths,
}

/// Paths the user chose (or quick-save picked) to write to; `save_file`
/// writes nowhere else.
#[derive(Default)]
struct SavePaths(Mutex<HashSet<PathBuf>>);

impl SavePaths {
    fn allow(&self, path: &str) {
        let mut set = self.0.lock().unwrap();
        if set.len() >= 1000 {
            set.clear();
        }
        set.insert(PathBuf::from(path));
    }

    fn allowed(&self, path: &str) -> bool {
        self.0.lock().unwrap().contains(Path::new(path))
    }
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?.map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Opened {
    token: u64,
    info: ImageInfo,
}

#[tauri::command]
fn take_pending_open(window: WebviewWindow, state: State<'_, AppState>) -> Option<String> {
    let mut p = state.pending.lock().unwrap();
    let path = p.remove(window.label())?.into_iter().last()?;
    state.showing.lock().unwrap().insert(window.label().to_string(), path.clone());
    Some(path)
}

#[tauri::command]
async fn open_image(
    window: WebviewWindow,
    state: State<'_, AppState>,
    path: String,
    series: Option<usize>,
) -> CmdResult<Opened> {
    let token = state.next_token.fetch_add(1, Ordering::Relaxed) + 1;
    state.showing.lock().unwrap().insert(window.label().to_string(), path.clone());
    // Each window gets an equal share of the page cache.
    let others = state.current.lock().unwrap().keys().filter(|l| *l != window.label()).count();
    let budget = mem::cache_budget() / (others + 1);
    let img = blocking(move || {
        let src = formats::open_series(Path::new(&path), series.unwrap_or(0))?;
        Ok(OpenImage::new(token, src, budget))
    })
    .await?;
    let info = img.info().clone();
    {
        let mut cur = state.current.lock().unwrap();
        // If the window closed while the file was opening, nothing would ever
        // cancel the prefetch below. Tauri unregisters a window before our
        // Destroyed handler takes this lock, so the check cannot race it.
        if window.app_handle().get_webview_window(window.label()).is_none() {
            return Err("window closed".into());
        }
        if let Some(old) = cur.insert(window.label().to_string(), img.clone()) {
            old.cancelled.store(true, Ordering::Relaxed);
        }
        state.side.lock().unwrap().remove(window.label());
    }
    let (center, _) = info.locate(0, info.size_z / 2, 0);
    img.prefetch(center);
    Ok(Opened { token, info })
}

/// Opens the XZ or YZ side view of the file shown under `token`.
#[tauri::command]
fn open_reslice(window: WebviewWindow, state: State<'_, AppState>, token: u64, axis: String) -> CmdResult<Opened> {
    let base = current(&state, token)?;
    let axis = match axis.as_str() {
        "xz" => reslice::Axis::Xz,
        "yz" => reslice::Axis::Yz,
        _ => return Err(format!("unknown view {axis}")),
    };
    let token = state.next_token.fetch_add(1, Ordering::Relaxed) + 1;
    let img = OpenImage::new(token, Box::new(reslice::Reslice::new(base, axis)), mem::cache_budget() / 8);
    let info = img.info().clone();
    state.side.lock().unwrap().insert(window.label().to_string(), img);
    Ok(Opened { token, info })
}

/// Pages of an image already in memory: (loaded, total, whole file fits).
#[tauri::command]
fn load_progress(state: State<'_, AppState>, token: u64) -> CmdResult<(usize, usize, bool)> {
    Ok(current(&state, token)?.load_progress())
}

fn current(state: &AppState, token: u64) -> CmdResult<Arc<OpenImage>> {
    state
        .current
        .lock()
        .unwrap()
        .values()
        .find(|i| i.token == token)
        .cloned()
        .or_else(|| state.side.lock().unwrap().values().find(|i| i.token == token).cloned())
        .ok_or_else(|| "stale".to_string())
}

#[tauri::command]
async fn render_view(state: State<'_, AppState>, req: render::RenderReq) -> CmdResult<Response> {
    let img = current(&state, req.token)?;
    img.set_focus(img.info().locate(0, req.z, req.t).0);
    let bytes = blocking(move || render::render(&img, &req)).await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
async fn pixel_values(
    state: State<'_, AppState>,
    token: u64,
    z: usize,
    t: usize,
    x: usize,
    y: usize,
) -> CmdResult<Vec<f64>> {
    let img = current(&state, token)?;
    // Only answer from cache; never block the UI on a decode for a tooltip.
    let (page, _) = img.info().locate(0, z, t);
    if img.cached(page).is_none() {
        return Ok(vec![]);
    }
    blocking(move || render::pixel_values(&img, z, t, x, y)).await
}

#[derive(Serialize)]
struct Folder {
    files: Vec<String>,
    sizes: Vec<u64>,
    index: Option<usize>,
    /// Z stacks an acquisition project file makes of this folder's slices.
    stacks: Vec<StackEntry>,
}

#[derive(Serialize)]
struct StackEntry {
    /// What to open: the project file, or this folder when the project
    /// file is in a folder above it.
    path: String,
    name: String,
    slices: usize,
}

/// The stack a plugin reads from a project file for the slices in `dir`.
fn folder_stacks(dir: &Path) -> Vec<StackEntry> {
    let reg = plugins::registry::Registry::global();
    let Some(config) = reg.find_stack_file(dir) else { return vec![] };
    let here = config.parent() == Some(dir);
    match reg.stack(&config, 0, Some(dir)) {
        Some(Ok(plan)) if here || plan.files.iter().all(|f| f.parent() == Some(dir)) => {
            let path = if here { config.clone() } else { dir.to_path_buf() };
            let name = plan.format.clone().unwrap_or_else(|| "堆叠".into());
            vec![StackEntry { path: path.to_string_lossy().into_owned(), name, slices: plan.files.len() }]
        }
        Some(Err(e)) => {
            eprintln!("{}: {e:#}", config.display());
            vec![]
        }
        _ => vec![],
    }
}

#[tauri::command]
async fn list_folder(path: String) -> CmdResult<Folder> {
    blocking(move || {
        let p = PathBuf::from(&path);
        let dir = if p.is_dir() { p.clone() } else { p.parent().map(Path::to_path_buf).unwrap_or_default() };
        let mut files: Vec<String> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && formats::is_supported(p))
            .filter(|p| !p.file_name().map(|n| n.to_string_lossy().starts_with("._")).unwrap_or(false))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        files.sort_by(|a, b| natural_cmp(a, b));
        let index = files.iter().position(|f| Path::new(f) == p);
        let sizes = files.iter().map(|f| std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)).collect();
        let dir: PathBuf = dir.components().collect();
        Ok(Folder { files, sizes, index, stacks: folder_stacks(&dir) })
    })
    .await
}

#[tauri::command]
async fn thumbnail(state: State<'_, AppState>, path: String, size: usize) -> CmdResult<Response> {
    let mtime = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| format!("{t:?}"))
        .unwrap_or_default();
    let key = format!("{path}|{mtime}|{size}");
    if let Some(t) = state.thumbs.lock().unwrap().get(&key) {
        return Ok(Response::new(t.as_ref().clone()));
    }
    // Reuse the decoded pages when the thumbnail is for the open file.
    let open = state.current.lock().unwrap().values().find(|i| i.info().path == path).cloned();
    let bytes = blocking(move || {
        image::bg_pool().install(|| {
            let img = match open {
                Some(i) => i,
                None => OpenImage::new(0, formats::open_pixels(Path::new(&path))?, usize::MAX),
            };
            render::thumbnail(&img, size)
        })
    })
    .await?;
    let bytes = Arc::new(bytes);
    let mut thumbs = state.thumbs.lock().unwrap();
    if thumbs.len() > 2000 {
        thumbs.clear();
    }
    thumbs.insert(key, bytes.clone());
    Ok(Response::new(bytes.as_ref().clone()))
}

#[tauri::command]
async fn pick_file(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let exts = formats::extensions();
    let exts: Vec<&str> = exts.iter().map(String::as_str).collect();
    app.dialog()
        .file()
        .add_filter("TIFF / MRC", &exts)
        .blocking_pick_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

#[derive(Serialize)]
struct Histogram {
    bins: Vec<u32>,
    min: f32,
    max: f32,
}

#[tauri::command]
async fn histogram(state: State<'_, AppState>, token: u64, z: usize, t: usize, c: usize) -> CmdResult<Histogram> {
    let img = current(&state, token)?;
    blocking(move || {
        let (page, sample) = img.info().locate(c, z, t);
        let planes = img.page(page)?;
        let st = img.stats(page, sample, &planes);
        Ok(Histogram { bins: st.bins(256), min: st.min, max: st.max })
    })
    .await
}

/// Power spectrum of a region of one slice; see `fft::power_spectrum`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn power_spectrum(
    state: State<'_, AppState>,
    token: u64,
    z: usize,
    t: usize,
    c: usize,
    region: [f64; 4],
    n: usize,
) -> CmdResult<Response> {
    let img = current(&state, token)?;
    let bytes = blocking(move || fft::power_spectrum(&img, c, z, t, region, n)).await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
async fn pick_save_path(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    name: String,
    filter: Option<String>,
    exts: Option<Vec<String>>,
) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let exts = exts.unwrap_or_else(|| vec!["png".into()]);
    let exts: Vec<&str> = exts.iter().map(String::as_str).collect();
    let path = app
        .dialog()
        .file()
        .add_filter(filter.as_deref().unwrap_or("PNG"), &exts)
        .set_directory(dir)
        .set_file_name(name)
        .blocking_save_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned());
    if let Some(p) = &path {
        state.save_paths.allow(p);
    }
    Ok(path)
}

#[tauri::command]
async fn pick_folder(app: AppHandle, dir: String) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .set_directory(dir)
        .blocking_pick_folder()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Opens a folder in Finder / Explorer / the file manager.
#[tauri::command]
fn open_folder(path: String) -> CmdResult<()> {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(windows)]
    let mut cmd = Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");
    cmd.arg(&path).spawn().map(|_| ()).map_err(err)
}

/// First `<stem>.png`, `<stem>-2.png`, ... that does not exist yet.
#[tauri::command]
fn free_path(state: State<'_, AppState>, dir: String, stem: String) -> String {
    let dir = PathBuf::from(dir);
    let mut p = dir.join(format!("{stem}.png"));
    let mut i = 2;
    while p.exists() {
        p = dir.join(format!("{stem}-{i}.png"));
        i += 1;
    }
    let p = p.to_string_lossy().into_owned();
    state.save_paths.allow(&p);
    p
}

/// Annotations drawn on an image are kept next to it, e.g.
/// `cell.tif` → `cell.tif.annot.json`; the other images of a file that holds
/// several get their own file (`project.lif.series3.annot.json` for the third).
fn annot_path(image: &str, series: Option<usize>) -> PathBuf {
    match series {
        Some(s) if s > 0 => PathBuf::from(format!("{image}.series{}.annot.json", s + 1)),
        _ => PathBuf::from(format!("{image}.annot.json")),
    }
}

#[tauri::command]
fn read_annotations(image: String, series: Option<usize>) -> Option<String> {
    std::fs::read_to_string(annot_path(&image, series)).ok()
}

/// Writes the annotations file, or removes it when `json` is empty.
#[tauri::command]
fn write_annotations(window: WebviewWindow, state: State<'_, AppState>, image: String, series: Option<usize>, json: String) -> CmdResult<()> {
    let current = state.current.lock().unwrap();
    let info = current.get(window.label()).map(|img| img.info());
    let path = checked_annot_path(info.map(|i| (i.path.as_str(), i.series_index)), &image, series)?;
    write_annotations_at(&path, &json)
}

fn checked_annot_path(opened: Option<(&str, usize)>, image: &str, series: Option<usize>) -> CmdResult<PathBuf> {
    match opened {
        Some((path, index)) if path == image && index == series.unwrap_or(0) => Ok(annot_path(path, Some(index))),
        _ => Err("只能保存当前窗口已打开图像的标注".into()),
    }
}

fn write_annotations_at(path: &Path, json: &str) -> CmdResult<()> {
    if json.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(err(e)),
            _ => Ok(()),
        };
    }
    // Write to a temporary file first so a crash never leaves half a file.
    let tmp = path.with_extension("json.tmp");
    // Never follow a pre-existing temporary-file symlink.
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(err)?;
    let result = file.write_all(json.as_bytes()).and_then(|_| {
        drop(file);
        std::fs::rename(&tmp, path)
    });
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result.map_err(err)
}

pub(crate) fn percent_decode(s: &str) -> String {
    fn hex(c: u8) -> Option<u8> {
        (c as char).to_digit(16).map(|d| d as u8)
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Writes the raw request body to the percent-encoded path in `x-path`,
/// which must come from `pick_save_path` or `free_path`.
#[tauri::command]
fn save_file(state: State<'_, AppState>, request: tauri::ipc::Request<'_>) -> CmdResult<()> {
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err("expected binary body".into());
    };
    let path = request
        .headers()
        .get("x-path")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .ok_or("missing path")?;
    if !state.save_paths.allowed(&path) {
        return Err(format!("不允许写入：{path}"));
    }
    std::fs::write(&path, data).map_err(err)
}

#[tauri::command]
async fn find_models(path: String) -> Vec<String> {
    imod::find_for(Path::new(&path))
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
async fn load_model(path: String) -> CmdResult<imod::Model> {
    blocking(move || imod::load(Path::new(&path))).await
}

#[tauri::command]
async fn pick_model(app: AppHandle, dir: String) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .add_filter("IMOD 模型", &["mod", "fid", "imod", "seed", "3dmod"])
        .set_directory(dir)
        .blocking_pick_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Shows `path` in Finder / Explorer, or opens its folder elsewhere.
#[tauri::command]
fn reveal_in_folder(path: String) -> CmdResult<()> {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg("-R").arg(&path);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("explorer");
        c.arg(format!("/select,{path}"));
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(Path::new(&path).parent().unwrap_or(Path::new("/")));
        c
    };
    cmd.spawn().map(|_| ()).map_err(err)
}

#[tauri::command]
async fn set_default_app() -> CmdResult<String> {
    assoc::set_default()
}

/// Paths passed on the command line (Windows and Linux double-click). The
/// Linux launcher passes `file://` URIs (`%U`, so `tifmrcview://` links
/// reach us too).
fn paths_from_args(args: impl IntoIterator<Item = String>, cwd: Option<&Path>) -> Vec<String> {
    args.into_iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(|a| {
            let p = match a.starts_with("file://") {
                true => tauri::Url::parse(&a).ok().and_then(|u| u.to_file_path().ok()).unwrap_or_default(),
                false => PathBuf::from(&a),
            };
            match cwd {
                Some(c) if p.is_relative() => c.join(p),
                _ => p,
            }
        })
        .filter(|p| p.is_file() || p.is_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Opens files the OS handed us. A file goes to the window that already
/// shows it or another file from its folder, else to a window with nothing
/// open yet, else to a new window.
fn queue_open(app: &AppHandle, paths: Vec<String>) {
    for path in paths {
        let label = target_window(app, &path);
        let state = app.state::<AppState>();
        state.pending.lock().unwrap().entry(label.clone()).or_default().push(path);
        match app.get_webview_window(&label) {
            Some(w) => {
                let _ = app.emit_to(&label, "open-pending", ());
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
            // The main window is still being set up; it picks the file up when it loads.
            None if label == "main" => {}
            // The new window picks the file up when its page loads.
            None => {
                if let Err(e) = new_window(app, &label) {
                    eprintln!("could not open a window: {e}");
                }
            }
        }
    }
}

fn target_window(app: &AppHandle, path: &str) -> String {
    let state = app.state::<AppState>();
    let dir = Path::new(path).parent();
    let showing = state.showing.lock().unwrap();
    let pending = state.pending.lock().unwrap();
    let windows = app.webview_windows();
    // On a cold start macOS hands over the file before the main window
    // exists; it is about to be created, so give the file to it.
    if windows.is_empty() {
        return "main".into();
    }
    let mut labels: Vec<&String> = windows.keys().collect();
    labels.sort();
    let shown = |l: &String| {
        pending.get(l).and_then(|p| p.last()).or_else(|| showing.get(l)).map(PathBuf::from)
    };
    if let Some(l) = labels.iter().find(|l| shown(l).is_some_and(|p| p.parent() == dir)) {
        return l.to_string();
    }
    if let Some(l) = labels.iter().find(|l| shown(l).is_none()) {
        return l.to_string();
    }
    format!("w{}", state.next_window.fetch_add(1, Ordering::Relaxed) + 2)
}

/// The newest `tifmrcview://install-plugin` link the UI has not picked up.
#[derive(Default)]
struct InstallRequest(Mutex<Option<plugins::market::InstallLink>>);

#[tauri::command]
fn take_install_request(state: State<'_, InstallRequest>) -> Option<plugins::market::InstallLink> {
    state.0.lock().unwrap().take()
}

/// Handles URLs the OS opened us with: an install link brings the main
/// window to the front, where the UI asks before installing anything.
/// Everything else (files on macOS, malformed links) is ignored here.
fn handle_links(app: &AppHandle, urls: impl IntoIterator<Item = String>) {
    let Some(link) = urls.into_iter().find_map(|u| plugins::market::parse_install_link(&u)) else {
        return;
    };
    *app.state::<InstallRequest>().0.lock().unwrap() = Some(link);
    let windows = app.webview_windows();
    let mut labels: Vec<&String> = windows.keys().collect();
    labels.sort_by_key(|l| (l.as_str() != "main", l.as_str()));
    // With no window yet, the main window picks the link up when it loads.
    if let Some(w) = labels.first().and_then(|l| windows.get(*l)) {
        let _ = app.emit_to(w.label(), "install-request", ());
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn new_window(app: &AppHandle, label: &str) -> tauri::Result<WebviewWindow> {
    // Cascade from the window in front so the new one doesn't hide it.
    let pos = app
        .webview_windows()
        .values()
        .find(|w| w.is_focused().unwrap_or(false))
        .and_then(|w| Some((w.outer_position().ok()?, w.scale_factor().ok()?)));
    let mut b = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title("TifMrcView")
        .inner_size(1280.0, 800.0)
        .min_inner_size(640.0, 480.0);
    b = match pos {
        Some((p, k)) => b.position(p.x as f64 / k + 28.0, p.y as f64 / k + 28.0),
        None => b.center(),
    };
    b.build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::default();
    let args = paths_from_args(std::env::args(), std::env::current_dir().ok().as_deref());
    if !args.is_empty() {
        state.pending.lock().unwrap().insert("main".into(), args);
    }

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            let paths = paths_from_args(args, Some(Path::new(&cwd)));
            queue_open(app, paths);
        }));
    }
    builder
        // After single-instance: it forwards a second instance's link here.
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state)
        .manage(update::Pending::default())
        .manage(InstallRequest::default())
        .manage(plugins::MarketCache::default())
        .register_uri_scheme_protocol("tmvplugin", plugins::serve)
        .setup(|app| {
            let dir = plugins::user_dir(app.handle());
            plugins::registry::Registry::global().configure(dir.clone(), &app.package_info().version.to_string());
            app.manage(plugins::Plugins::new(dir));
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                // Windows / Linux: a link to a running app comes through the
                // single-instance plugin; macOS: through the open-URL event.
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |e| handle_links(&handle, e.urls().into_iter().map(String::from)));
                // Cold start: Windows / Linux pass the link as the only argument.
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    handle_links(app.handle(), urls.into_iter().map(String::from));
                }
                // Installers register the scheme (NSIS registry key, .deb/.rpm
                // desktop file, macOS Info.plist). Development builds and
                // AppImages are not installed, so register them at runtime.
                #[cfg(any(windows, target_os = "linux"))]
                {
                    #[cfg(target_os = "linux")]
                    let unpacked = cfg!(debug_assertions) || app.env().appimage.is_some();
                    #[cfg(windows)]
                    let unpacked = cfg!(debug_assertions);
                    if unpacked {
                        if let Err(e) = app.deep_link().register_all() {
                            eprintln!("could not register tifmrcview:// links: {e}");
                        }
                    }
                }
            }
            let dir = app.path().app_config_dir().ok();
            std::thread::spawn(move || {
                crate::plugin_assoc::sync_plugin_formats();
                assoc::first_run(dir);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            take_pending_open,
            open_image,
            open_reslice,
            load_progress,
            render_view,
            pixel_values,
            list_folder,
            thumbnail,
            pick_file,
            set_default_app,
            reveal_in_folder,
            histogram,
            power_spectrum,
            pick_save_path,
            pick_folder,
            open_folder,
            free_path,
            save_file,
            read_annotations,
            write_annotations,
            plugins::plugin_scan,
            plugins::plugin_call,
            plugins::open_plugin_dir,
            plugins::pick_plugin_package,
            plugins::plugin_install,
            plugins::plugin_inspect_package,
            plugins::plugin_uninstall,
            plugins::plugin_set_disabled,
            plugins::open_market,
            plugins::plugin_market_list,
            plugins::plugin_install_from_market,
            take_install_request,
            update::check_update,
            update::install_update,
            find_models,
            load_model,
            pick_model,
        ])
        .build(tauri::generate_context!())
        .expect("error while building TifMrcView")
        .run(|_app, _event| {
            // Free a window's file when it closes.
            if let tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } = &_event {
                let state = _app.state::<AppState>();
                if let Some(img) = state.current.lock().unwrap().remove(label) {
                    img.cancelled.store(true, Ordering::Relaxed);
                }
                state.pending.lock().unwrap().remove(label);
                state.showing.lock().unwrap().remove(label);
                state.side.lock().unwrap().remove(label);
            }
            // macOS delivers double-clicked files as an event, not as argv.
            // tifmrcview:// links arrive here too; the deep-link plugin
            // passes them to `handle_links`.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = _event {
                let paths = urls
                    .into_iter()
                    .filter(|u| u.scheme() == "file")
                    .filter_map(|u| u.to_file_path().ok())
                    .filter(|p| p.is_file() || p.is_dir())
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                queue_open(_app, paths);
            }
        });
}

#[cfg(test)]
mod save_path_tests {
    use super::*;

    #[test]
    fn annotations_only_for_current_image_and_series() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("cell.tif").to_string_lossy().into_owned();
        let other = dir.path().join("other.tif").to_string_lossy().into_owned();
        let current = Some((image.as_str(), 0));
        assert!(checked_annot_path(None, &image, None).is_err());
        assert!(checked_annot_path(current, &other, None).is_err());
        assert!(checked_annot_path(current, &format!("{image}/../other.tif"), None).is_err());
        assert!(checked_annot_path(current, &image, Some(1)).is_err());
        let path = checked_annot_path(current, &image, None).unwrap();
        assert_eq!(path, PathBuf::from(format!("{image}.annot.json")));
        write_annotations_at(&path, "{\"items\":[]}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"items\":[]}");
        write_annotations_at(&path, "updated").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "updated");
        write_annotations_at(&path, "").unwrap();
        write_annotations_at(&path, "").unwrap();
        assert!(!path.exists());
        let series = checked_annot_path(Some((&image, 2)), &image, Some(2)).unwrap();
        assert_eq!(series, PathBuf::from(format!("{image}.series3.annot.json")));
        assert!(checked_annot_path(Some((&image, 2)), &image, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn annotations_do_not_follow_temporary_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("unrelated");
        std::fs::write(&target, "original").unwrap();
        let path = dir.path().join("cell.tif.annot.json");
        let tmp = path.with_extension("json.tmp");
        std::os::unix::fs::symlink(&target, &tmp).unwrap();
        assert!(write_annotations_at(&path, "changed").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    }

    #[test]
    fn save_paths_allow_only_handed_out_paths() {
        let s = SavePaths::default();
        assert!(!s.allowed("/tmp/a.png"));
        s.allow("/tmp/a.png");
        assert!(s.allowed("/tmp/a.png"));
        assert!(s.allowed("/tmp/a.png"));
        assert!(!s.allowed("/tmp/b.png"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn natural_cmp_numbers_compare_by_value() {
        assert_eq!(natural_cmp("img2", "img10"), Ordering::Less);
        assert_eq!(natural_cmp("img10", "img2"), Ordering::Greater);
        assert_eq!(natural_cmp("a1b20", "a1b3"), Ordering::Greater);
    }

    #[test]
    fn natural_cmp_leading_zeros() {
        assert_eq!(natural_cmp("img002", "img10"), Ordering::Less);
        // Leading zeros carry no weight: equal values compare equal.
        assert_eq!(natural_cmp("img02", "img2"), Ordering::Equal);
        assert_eq!(natural_cmp("img0", "img00"), Ordering::Equal);
        assert_eq!(natural_cmp("img007", "img7"), Ordering::Equal);
        assert_eq!(natural_cmp("img007", "img8"), Ordering::Less);
    }

    #[test]
    fn natural_cmp_ignores_ascii_case() {
        assert_eq!(natural_cmp("ABC", "abc"), Ordering::Equal);
        assert_eq!(natural_cmp("a.tif", "B.tif"), Ordering::Less);
    }

    #[test]
    fn natural_cmp_prefixes_and_equal() {
        assert_eq!(natural_cmp("", ""), Ordering::Equal);
        assert_eq!(natural_cmp("img", "img1"), Ordering::Less);
        assert_eq!(natural_cmp("img1", "img"), Ordering::Greater);
        assert_eq!(natural_cmp("img1.tif", "img1.tif"), Ordering::Equal);
        // Digits (0x31) sort before letters.
        assert_eq!(natural_cmp("a1", "ab"), Ordering::Less);
    }

    #[test]
    fn natural_cmp_sorts_a_list() {
        let mut v = vec!["f10", "f2", "F1", "f01x", "f1"];
        v.sort_by(|a, b| natural_cmp(a, b));
        // "F1" and "f1" compare equal, so the stable sort keeps input order.
        assert_eq!(v, ["F1", "f1", "f01x", "f2", "f10"]);
    }

    #[test]
    fn percent_decode_ascii_and_escapes() {
        assert_eq!(percent_decode("plain.tif"), "plain.tif");
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("%41%62"), "Ab");
        assert_eq!(percent_decode(""), "");
    }

    #[test]
    fn percent_decode_utf8_multibyte() {
        assert_eq!(percent_decode("%E4%B8%AD.tif"), "中.tif");
        assert_eq!(percent_decode("日本%E8%AA%9E"), "日本語");
    }

    #[test]
    fn percent_decode_lone_or_invalid_percent() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%4"), "a%4");
        assert_eq!(percent_decode("%zz1"), "%zz1");
        assert_eq!(percent_decode("%G1%41"), "%G1A");
    }

    #[test]
    fn percent_decode_multibyte_after_percent() {
        assert_eq!(percent_decode("%a€"), "%a€");
    }

    #[test]
    fn percent_decode_rejects_plus_sign() {
        assert_eq!(percent_decode("%+4x"), "%+4x");
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn paths_from_args_filters_and_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.tif");
        let b = dir.path().join("b.mrc");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let abs_b = format!("file://{}", b.display());
        let got = paths_from_args(
            args(&["prog", "-x", "a.tif", &abs_b, "missing.tif", "--flag", "sub"]),
            Some(dir.path()),
        );
        // A folder opens as an image sequence (a Z stack of its slices).
        let sub = dir.path().join("sub");
        assert_eq!(got, vec![a.to_string_lossy(), b.to_string_lossy(), sub.to_string_lossy()]);
    }

    #[test]
    fn paths_from_args_skips_argv0_and_handles_missing_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.tif");
        std::fs::write(&a, b"x").unwrap();
        let abs = a.to_string_lossy().into_owned();
        // argv[0] is never treated as a file, even if it exists.
        assert!(paths_from_args(args(&[&abs]), None).is_empty());
        assert_eq!(paths_from_args(args(&["prog", &abs]), None), vec![abs.clone()]);
        assert!(paths_from_args(args(&["prog", "definitely-not-here.tif"]), None).is_empty());
    }
}
