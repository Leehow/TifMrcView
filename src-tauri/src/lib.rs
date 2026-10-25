mod assoc;
pub mod formats;
pub mod imod;
pub mod image;
mod mem;
pub mod plugins;
pub mod render;
pub mod reslice;
mod update;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

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
async fn open_image(window: WebviewWindow, state: State<'_, AppState>, path: String) -> CmdResult<Opened> {
    let token = state.next_token.fetch_add(1, Ordering::Relaxed) + 1;
    state.showing.lock().unwrap().insert(window.label().to_string(), path.clone());
    // Each window gets an equal share of the page cache.
    let others = state.current.lock().unwrap().keys().filter(|l| *l != window.label()).count();
    let budget = mem::cache_budget() / (others + 1);
    let img = blocking(move || {
        let src = formats::open(Path::new(&path))?;
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
}

/// Sorts "img2" before "img10".
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let nb = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let (da, db) = (&a[..na], &b[..nb]);
                let ta = da.iter().skip_while(|&&c| c == b'0').count();
                let tb = db.iter().skip_while(|&&c| c == b'0').count();
                let ord = ta.cmp(&tb).then_with(|| da[na - ta..].cmp(&db[nb - tb..]));
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
                a = &a[na..];
                b = &b[nb..];
            }
            (Some(x), Some(y)) => {
                let ord = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
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
        Ok(Folder { files, sizes, index })
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
                None => OpenImage::new(0, formats::open(Path::new(&path))?, usize::MAX),
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
    let exts: Vec<&str> = formats::EXTENSIONS.to_vec();
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
/// `cell.tif` → `cell.tif.annot.json`.
fn annot_path(image: &str) -> PathBuf {
    PathBuf::from(format!("{image}.annot.json"))
}

#[tauri::command]
fn read_annotations(image: String) -> Option<String> {
    std::fs::read_to_string(annot_path(&image)).ok()
}

/// Writes the annotations file, or removes it when `json` is empty.
#[tauri::command]
fn write_annotations(image: String, json: String) -> CmdResult<()> {
    let path = annot_path(&image);
    if json.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(err(e)),
            _ => Ok(()),
        };
    }
    // Write to a temporary file first so a crash never leaves half a file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(err)?;
    std::fs::rename(&tmp, &path).map_err(err)
}

pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
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

/// Paths passed on the command line (Windows and Linux double-click).
fn paths_from_args(args: impl IntoIterator<Item = String>, cwd: Option<&Path>) -> Vec<String> {
    args.into_iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(|a| {
            let a = a.strip_prefix("file://").map(str::to_string).unwrap_or(a);
            let p = PathBuf::from(&a);
            match cwd {
                Some(c) if p.is_relative() => c.join(p),
                _ => p,
            }
        })
        .filter(|p| p.is_file())
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state)
        .manage(update::Pending::default())
        .register_uri_scheme_protocol("tmvplugin", plugins::serve)
        .setup(|app| {
            app.manage(plugins::Plugins::new(plugins::user_dir(app.handle())));
            let dir = app.path().app_config_dir().ok();
            std::thread::spawn(move || assoc::first_run(dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            take_pending_open,
            open_image,
            open_reslice,
            render_view,
            pixel_values,
            list_folder,
            thumbnail,
            pick_file,
            set_default_app,
            reveal_in_folder,
            histogram,
            pick_save_path,
            free_path,
            save_file,
            read_annotations,
            write_annotations,
            plugins::plugin_scan,
            plugins::plugin_call,
            plugins::open_plugin_dir,
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
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = _event {
                let paths = urls
                    .into_iter()
                    .filter_map(|u| u.to_file_path().ok())
                    .filter(|p| p.is_file())
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                queue_open(_app, paths);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

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
