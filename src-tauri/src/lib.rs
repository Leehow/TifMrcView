mod assoc;
pub mod formats;
pub mod imod;
pub mod image;
pub mod render;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, State};

use image::{ImageInfo, OpenImage};

/// Decoded pages kept in memory for the open file.
const CACHE_BUDGET: usize = 3 << 30;

#[derive(Default)]
struct AppState {
    current: Mutex<Option<Arc<OpenImage>>>,
    next_token: AtomicU64,
    /// Files the OS asked us to open that the UI has not picked up yet.
    pending: Mutex<Vec<String>>,
    thumbs: Mutex<HashMap<String, Arc<Vec<u8>>>>,
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
fn take_pending_open(state: State<'_, AppState>) -> Option<String> {
    let mut p = state.pending.lock().unwrap();
    let last = p.pop();
    p.clear();
    last
}

#[tauri::command]
async fn open_image(state: State<'_, AppState>, path: String) -> CmdResult<Opened> {
    let token = state.next_token.fetch_add(1, Ordering::Relaxed) + 1;
    let img = blocking(move || {
        let src = formats::open(Path::new(&path))?;
        Ok(OpenImage::new(token, src, CACHE_BUDGET))
    })
    .await?;
    let info = img.info().clone();
    {
        let mut cur = state.current.lock().unwrap();
        if let Some(old) = cur.take() {
            old.cancelled.store(true, Ordering::Relaxed);
        }
        *cur = Some(img.clone());
    }
    let (center, _) = info.locate(0, info.size_z / 2, 0);
    img.prefetch(center);
    Ok(Opened { token, info })
}

fn current(state: &AppState, token: u64) -> CmdResult<Arc<OpenImage>> {
    state
        .current
        .lock()
        .unwrap()
        .clone()
        .filter(|i| i.token == token)
        .ok_or_else(|| "stale".to_string())
}

#[tauri::command]
async fn render_view(state: State<'_, AppState>, req: render::RenderReq) -> CmdResult<Response> {
    let img = current(&state, req.token)?;
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
    let open = state.current.lock().unwrap().clone().filter(|i| i.info().path == path);
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
async fn pick_save_path(app: AppHandle, dir: String, name: String) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .add_filter("PNG", &["png"])
        .set_directory(dir)
        .set_file_name(name)
        .blocking_save_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

/// First `<stem>.png`, `<stem>-2.png`, ... that does not exist yet.
#[tauri::command]
fn free_path(dir: String, stem: String) -> String {
    let dir = PathBuf::from(dir);
    let mut p = dir.join(format!("{stem}.png"));
    let mut i = 2;
    while p.exists() {
        p = dir.join(format!("{stem}-{i}.png"));
        i += 1;
    }
    p.to_string_lossy().into_owned()
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

fn percent_decode(s: &str) -> String {
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

/// Writes the raw request body to the percent-encoded path in `x-path`.
#[tauri::command]
fn save_file(request: tauri::ipc::Request<'_>) -> CmdResult<()> {
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else {
        return Err("expected binary body".into());
    };
    let path = request
        .headers()
        .get("x-path")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .ok_or("missing path")?;
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

fn queue_open(app: &AppHandle, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    app.state::<AppState>().pending.lock().unwrap().extend(paths);
    let _ = app.emit("open-pending", ());
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::default();
    state
        .pending
        .lock()
        .unwrap()
        .extend(paths_from_args(std::env::args(), std::env::current_dir().ok().as_deref()));

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            let paths = paths_from_args(args, Some(Path::new(&cwd)));
            queue_open(app, paths);
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
            }
        }));
    }
    builder
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .setup(|app| {
            let dir = app.path().app_config_dir().ok();
            std::thread::spawn(move || assoc::first_run(dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            take_pending_open,
            open_image,
            render_view,
            pixel_values,
            list_folder,
            thumbnail,
            pick_file,
            set_default_app,
            histogram,
            pick_save_path,
            free_path,
            save_file,
            read_annotations,
            write_annotations,
            find_models,
            load_model,
            pick_model,
        ])
        .build(tauri::generate_context!())
        .expect("error while building TifMrcView")
        .run(|_app, _event| {
            // macOS delivers double-clicked files as an event, not as argv.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = _event {
                let paths = urls
                    .into_iter()
                    .filter_map(|u| u.to_file_path().ok())
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                queue_open(_app, paths);
            }
        });
}
