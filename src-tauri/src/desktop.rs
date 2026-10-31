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
    #[cfg_attr(target_os = "android", allow(dead_code))]
    next_window: AtomicU64,
    /// Files the OS asked us to open that a window has not picked up yet.
    pending: Mutex<HashMap<String, Vec<String>>>,
    /// The XZ / YZ side view of each window's file, when one is shown.
    side: Mutex<HashMap<String, Arc<OpenImage>>>,
    /// The file each window shows or is loading, by window label.
    showing: Mutex<HashMap<String, String>>,
    thumbs: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    save_paths: SavePaths,
    /// Pro label maps of the open images.
    labels: crate::labels::Store,
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
    #[cfg(target_os = "android")]
    if let Some(path) = android::take_opened(window.app_handle()) {
        state.pending.lock().unwrap().entry(window.label().to_string()).or_default().push(path);
    }
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

/// The current stack as an 8-bit volume for the Pro 3D view.
#[tauri::command]
async fn volume_data(state: State<'_, AppState>, req: crate::volume::VolumeReq) -> CmdResult<Response> {
    let img = current(&state, req.token)?;
    let bytes = blocking(move || crate::volume::volume(&img, &req)).await?;
    Ok(Response::new(bytes))
}

// ---------- Pro label maps ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LabelsState {
    /// The label file next to the image.
    path: String,
    /// Voxels per label, indexed by label.
    counts: Vec<u64>,
    /// Main label of each slice (0 = none), for the marks on the Z slider.
    slices: Vec<u16>,
    /// Slices that changed.
    zs: Vec<usize>,
    materials: Vec<crate::labels::Material>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum LabelOp {
    Stroke { z: usize, pts: Vec<[f32; 2]>, radius: f32, label: u16, guard: u16 },
    Wand { x: usize, y: usize, z: usize, c: usize, t: usize, tol: f32, label: u16, guard: u16, three_d: bool },
    Threshold { z0: usize, z1: usize, c: usize, t: usize, lo: f32, hi: f32, label: u16, guard: u16, split: bool, min_size: usize },
    Watershed { z0: usize, z1: usize, c: usize, t: usize },
    Polygon { z: usize, pts: Vec<[f32; 2]>, label: u16, guard: u16 },
    Interpolate { label: u16, guard: u16 },
    Delete { label: u16 },
    MoveRegion { x: usize, y: usize, z: usize, dx: i64, dy: i64, three_d: bool },
    DeleteRegion { x: usize, y: usize, z: usize, three_d: bool },
    Materials { materials: Vec<crate::labels::Material> },
    Undo,
    Redo,
}

/// Saves and drops the label maps of images no window shows any more.
fn prune_labels(state: &AppState) {
    let open: HashSet<u64> = state.current.lock().unwrap().values().map(|i| i.token).collect();
    let gone: Vec<_> = {
        let mut store = state.labels.lock().unwrap();
        let tokens: Vec<u64> = store.keys().copied().filter(|t| !open.contains(t)).collect();
        tokens.into_iter().filter_map(|t| store.remove(&t)).collect()
    };
    for e in gone {
        let mut e = e.lock().unwrap();
        if e.labels.dirty {
            let path = PathBuf::from(&e.path);
            if let Err(err) = e.labels.save(&path) {
                eprintln!("saving {}: {err}", path.display());
            }
        }
    }
}

fn labels_state(e: &crate::labels::Entry, zs: Vec<usize>) -> LabelsState {
    let l = &e.labels;
    LabelsState { path: e.path.clone(), counts: l.counts(), slices: l.slice_labels(), zs, materials: l.materials.clone() }
}

fn labels_entry(state: &AppState, token: u64) -> CmdResult<Arc<Mutex<crate::labels::Entry>>> {
    state.labels.lock().unwrap().get(&token).cloned().ok_or_else(|| "stale".to_string())
}

/// Loads (or starts) the label map of an open image.
#[tauri::command]
async fn labels_open(state: State<'_, AppState>, token: u64) -> CmdResult<LabelsState> {
    prune_labels(&state);
    if let Ok(e) = labels_entry(&state, token) {
        let e = e.lock().unwrap();
        return Ok(labels_state(&e, vec![]));
    }
    let img = state.current.lock().unwrap().values().find(|i| i.token == token).cloned().ok_or("stale")?;
    let entry = blocking(move || {
        let info = img.info();
        let path = crate::labels::labels_path(&info.path, info.series_index);
        let labels = if Path::new(&path).exists() {
            crate::labels::Labels::load(Path::new(&path), info.width, info.height, info.size_z)?
        } else {
            crate::labels::Labels::new(info.width, info.height, info.size_z)
        };
        Ok(crate::labels::Entry::new(labels, path))
    })
    .await?;
    let out = labels_state(&entry, vec![]);
    state.labels.lock().unwrap().entry(token).or_insert_with(|| Arc::new(Mutex::new(entry)));
    Ok(out)
}

/// One slice of the label map as little-endian u16.
#[tauri::command]
async fn labels_slice(state: State<'_, AppState>, token: u64, z: usize) -> CmdResult<Response> {
    let e = labels_entry(&state, token)?;
    let bytes = blocking(move || Ok(e.lock().unwrap().labels.slice_bytes(z))).await?;
    Ok(Response::new(bytes))
}

#[tauri::command]
async fn labels_edit(state: State<'_, AppState>, token: u64, op: LabelOp) -> CmdResult<LabelsState> {
    let e = labels_entry(&state, token)?;
    let img = current(&state, token)?;
    blocking(move || {
        use crate::labels::ChannelValues;
        let mut e = e.lock().unwrap();
        let values = |c, t| ChannelValues { img: img.clone(), c, t };
        let l = &mut e.labels;
        let zs = match op {
            LabelOp::Stroke { z, pts, radius, label, guard } => l.stroke(z, &pts, radius, label, guard),
            LabelOp::Wand { x, y, z, c, t, tol, label, guard, three_d } => l.wand(&values(c, t), x, y, z, tol, label, guard, three_d)?,
            LabelOp::Threshold { z0, z1, c, t, lo, hi, label, guard, split, min_size } => {
                l.threshold(&values(c, t), z0..z1, lo, hi, label, guard, split, min_size)?
            }
            LabelOp::Watershed { z0, z1, c, t } => l.watershed(&values(c, t), z0..z1)?,
            LabelOp::Polygon { z, pts, label, guard } => l.polygon(z, &pts, label, guard),
            LabelOp::Interpolate { label, guard } => l.interpolate(label, guard),
            LabelOp::Delete { label } => l.delete_label(label),
            LabelOp::MoveRegion { x, y, z, dx, dy, three_d } => l.move_region(x, y, z, dx, dy, three_d),
            LabelOp::DeleteRegion { x, y, z, three_d } => l.delete_region(x, y, z, three_d),
            LabelOp::Materials { materials } => {
                l.set_materials(materials);
                vec![]
            }
            LabelOp::Undo => l.undo(),
            LabelOp::Redo => l.redo(),
        };
        Ok(labels_state(&e, zs))
    })
    .await
}

/// Writes the label file when it changed; returns whether a file exists now.
#[tauri::command]
async fn labels_save(state: State<'_, AppState>, token: u64) -> CmdResult<bool> {
    let e = labels_entry(&state, token)?;
    blocking(move || {
        let mut e = e.lock().unwrap();
        let path = PathBuf::from(&e.path);
        if !e.labels.dirty {
            return Ok(path.exists());
        }
        e.labels.save(&path)
    })
    .await
}

/// The label map reduced like `volume_data` for the 3D view: u32 width,
/// height, depth, then one byte per voxel.
#[tauri::command]
async fn labels_volume(state: State<'_, AppState>, token: u64, max_voxels: usize) -> CmdResult<Response> {
    let e = labels_entry(&state, token)?;
    let bytes = blocking(move || {
        let e = e.lock().unwrap();
        let l = &e.labels;
        let (f, fz) = crate::volume::factors(l.w, l.h, l.d, max_voxels);
        let (w, h, d, v) = l.reduced(f, fz);
        let mut out = Vec::with_capacity(12 + v.len());
        for n in [w, h, d] {
            out.extend_from_slice(&(n as u32).to_le_bytes());
        }
        out.extend_from_slice(&v);
        Ok(out)
    })
    .await?;
    Ok(Response::new(bytes))
}

// ---------- Pro SAM segmentation ----------

#[tauri::command]
fn sam_status(sam: State<'_, crate::sam::Sam>) -> crate::sam::Status {
    sam.status()
}

/// Downloads the SAM model once, reporting `sam-download` {done, total}.
#[tauri::command]
async fn sam_download(app: AppHandle) -> CmdResult<()> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::sync::atomic::Ordering::SeqCst;
    let sam = app.state::<crate::sam::Sam>();
    if !crate::edition::PRO {
        return Err("SAM 分割只在 TifMrcView Pro 里有".into());
    }
    if sam.downloading.swap(true, SeqCst) {
        return Err("模型正在下载".into());
    }
    let result = async {
        let path = crate::sam::model_path().ok_or("找不到存放模型的文件夹")?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
        let part = path.with_extension("part");
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let client = reqwest::Client::builder()
            .user_agent(format!("{}/{}", crate::edition::NAME, env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(err)?;
        let mut last_err = String::new();
        for url in crate::sam::MODEL_URLS {
            let attempt = async {
                let mut resp = client.get(url).send().await.map_err(err)?;
                if !resp.status().is_success() {
                    return Err(format!("服务器返回 {}", resp.status().as_u16()));
                }
                let total = resp.content_length().unwrap_or(crate::sam::MODEL_SIZE);
                let mut file = std::io::BufWriter::new(std::fs::File::create(&part).map_err(err)?);
                let mut hash = Sha256::new();
                let (mut done, mut shown) = (0u64, 0u64);
                while let Some(chunk) = resp.chunk().await.map_err(err)? {
                    file.write_all(&chunk).map_err(err)?;
                    hash.update(&chunk);
                    done += chunk.len() as u64;
                    if done - shown > 1 << 20 || done == total {
                        shown = done;
                        let _ = app.emit("sam-download", serde_json::json!({ "done": done, "total": total }));
                    }
                }
                file.flush().map_err(err)?;
                drop(file);
                let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
                crate::sam::verify(done, &hex).map_err(err)?;
                std::fs::rename(&part, &path).map_err(err)
            };
            match attempt.await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let _ = std::fs::remove_file(&part);
                    last_err = e;
                }
            }
        }
        Err(format!("模型下载失败：{last_err}"))
    }
    .await;
    sam.downloading.store(false, SeqCst);
    result
}

/// Segments the object under the prompt on one slice. Reply: u32 width,
/// u32 height, f32 predicted IoU, then one byte per pixel (1 = inside).
#[tauri::command]
async fn sam_predict(app: AppHandle, req: crate::sam::PredictReq) -> CmdResult<Response> {
    let img = current(&app.state::<AppState>(), req.token)?;
    let app2 = app.clone();
    let bytes = blocking(move || {
        let sam = app2.state::<crate::sam::Sam>();
        let info = img.info();
        let (w, h) = (info.width, info.height);
        let vals = crate::labels::ChannelValues { img: img.clone(), c: req.c, t: req.t };
        let values = |z| {
            use crate::labels::Values;
            vals.slice(z)
        };
        let (mask, iou) = sam.predict(&req, (w, h, info.size_z), values)?;
        let mut out = Vec::with_capacity(12 + mask.len());
        out.extend_from_slice(&(w as u32).to_le_bytes());
        out.extend_from_slice(&(h as u32).to_le_bytes());
        out.extend_from_slice(&iou.to_le_bytes());
        out.extend_from_slice(&mask);
        Ok(out)
    })
    .await?;
    Ok(Response::new(bytes))
}

/// Paints the last SAM mask into the label map.
#[tauri::command]
async fn sam_apply(app: AppHandle, token: u64, z: usize, label: u16, guard: u16, trim: Option<bool>) -> CmdResult<LabelsState> {
    let e = labels_entry(&app.state::<AppState>(), token)?;
    let app2 = app.clone();
    blocking(move || {
        let sam = app2.state::<crate::sam::Sam>();
        let last = sam.last.lock().unwrap();
        let Some(m) = last.as_ref().filter(|m| m.token == token && m.z == z) else {
            anyhow::bail!("没有可以应用的分割结果");
        };
        let mut e = e.lock().unwrap();
        let zs = e.labels.apply_mask(z, &m.mask, label, guard, trim.unwrap_or(false));
        Ok(labels_state(&e, zs))
    })
    .await
}

/// Carries the last SAM mask along Z and paints every slice it reaches,
/// as one undo step; reports `sam-propagate` {done, total}.
#[tauri::command]
async fn sam_propagate(app: AppHandle, req: crate::sam::PropagateReq, label: u16, guard: u16) -> CmdResult<LabelsState> {
    let img = current(&app.state::<AppState>(), req.token)?;
    let e = labels_entry(&app.state::<AppState>(), req.token)?;
    let app2 = app.clone();
    blocking(move || {
        let sam = app2.state::<crate::sam::Sam>();
        let info = img.info();
        let d = e.lock().unwrap().labels.d;
        let vals = crate::labels::ChannelValues { img: img.clone(), c: req.c, t: req.t };
        let values = |z| {
            use crate::labels::Values;
            vals.slice(z)
        };
        let progress = |done, total| {
            let _ = app2.emit("sam-propagate", serde_json::json!({ "done": done, "total": total }));
        };
        let masks = sam.propagate(&req, (info.width, info.height, d), values, progress)?;
        let refs: Vec<(usize, &[bool])> = masks.iter().map(|(z, m)| (*z, m.as_slice())).collect();
        let mut e = e.lock().unwrap();
        let zs = e.labels.apply_masks(&refs, label, guard, false);
        Ok(labels_state(&e, zs))
    })
    .await
}

#[tauri::command]
fn sam_cancel(sam: State<'_, crate::sam::Sam>) {
    sam.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
}

// ---------- Pro pixel classifier ----------

/// Which channel the classifier reads and the contrast it scales it with.
#[derive(serde::Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(feature = "pro"), allow(dead_code))]
struct ClassifyReq {
    token: u64,
    c: usize,
    t: usize,
    lo: f32,
    hi: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClassifierInfo {
    /// Labels it was trained on.
    labels: Vec<u16>,
    samples: usize,
    /// Out-of-bag accuracy, 0..1.
    accuracy: f32,
    backend: String,
    seconds: f32,
}

#[cfg(not(feature = "pro"))]
fn pro_only<T>() -> CmdResult<T> {
    Err("仅 Pro 版提供".into())
}

/// Paints (label 0: erases) a classifier training stroke; returns the
/// scribble slice as little-endian u16.
#[tauri::command]
async fn classify_scribble(state: State<'_, AppState>, token: u64, z: usize, pts: Vec<[f32; 2]>, radius: f32, label: u16) -> CmdResult<Response> {
    let e = labels_entry(&state, token)?;
    let bytes = blocking(move || {
        let mut e = e.lock().unwrap();
        let s = e.scribbles();
        s.stroke(z, &pts, radius, label, 0);
        Ok(s.slice_bytes(z))
    })
    .await?;
    Ok(Response::new(bytes))
}

/// One scribble slice as little-endian u16 (zeros when there are none).
#[tauri::command]
async fn classify_scribbles(state: State<'_, AppState>, token: u64, z: usize) -> CmdResult<Response> {
    let e = labels_entry(&state, token)?;
    let bytes = blocking(move || {
        let e = e.lock().unwrap();
        Ok(match &e.scribbles {
            Some(s) => s.slice_bytes(z),
            None => vec![0; e.labels.w * e.labels.h * 2],
        })
    })
    .await?;
    Ok(Response::new(bytes))
}

/// Drops all scribbles and the trained classifier.
#[tauri::command]
async fn classify_clear(state: State<'_, AppState>, token: u64) -> CmdResult<()> {
    let e = labels_entry(&state, token)?;
    let mut e = e.lock().unwrap();
    e.scribbles = None;
    #[cfg(feature = "pro")]
    {
        e.classifier = None;
    }
    Ok(())
}

/// Trains a random forest on the scribbled pixels of every slice.
#[tauri::command]
async fn classify_train(state: State<'_, AppState>, req: ClassifyReq) -> CmdResult<ClassifierInfo> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = (state, req);
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        let e = labels_entry(&state, req.token)?;
        let img = current(&state, req.token)?;
        blocking(move || {
            use crate::labels::Values;
            let started = std::time::Instant::now();
            let scrib: Vec<(usize, Vec<u16>)> = {
                let e = e.lock().unwrap();
                let Some(s) = &e.scribbles else { anyhow::bail!("先用画笔在图上标几笔，至少两种标签") };
                s.used_slices().into_iter().filter_map(|z| s.slice(z).map(|v| (z, v.to_vec()))).collect()
            };
            let info = img.info();
            let (w, h) = (info.width, info.height);
            let vals = crate::labels::ChannelValues { img: img.clone(), c: req.c, t: req.t };
            let mut slices = Vec::with_capacity(scrib.len());
            for (z, s) in &scrib {
                slices.push((vals.slice(*z)?, s.as_slice()));
            }
            let cfg = crate::classify::ForestConfig::default();
            let (c, backend) = crate::classify::on_device(|dev| crate::classify::train(dev, &slices, w, h, req.lo, req.hi, &cfg))?;
            let out = ClassifierInfo {
                labels: c.labels.clone(),
                samples: c.samples,
                accuracy: c.forest.oob_accuracy,
                backend: backend.to_string(),
                seconds: started.elapsed().as_secs_f32(),
            };
            eprintln!("classifier: {} samples, {} labels, oob {:.3}, {} in {:.2} s", out.samples, out.labels.len(), out.accuracy, out.backend, out.seconds);
            e.lock().unwrap().classifier = Some(Arc::new(c));
            Ok(out)
        })
        .await
    }
}

/// Classifies one slice. Reply: u32 width, u32 height, then u16 label per
/// pixel (little-endian), then one byte of confidence (0..255) per pixel.
#[tauri::command]
async fn classify_predict(state: State<'_, AppState>, req: ClassifyReq, z: usize) -> CmdResult<Response> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = (state, req, z);
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        let e = labels_entry(&state, req.token)?;
        let img = current(&state, req.token)?;
        let bytes = blocking(move || {
            use crate::labels::Values;
            let Some(c) = e.lock().unwrap().classifier.clone() else { anyhow::bail!("还没有训练分类器") };
            let info = img.info();
            let (w, h) = (info.width, info.height);
            let values = crate::labels::ChannelValues { img: img.clone(), c: req.c, t: req.t }.slice(z)?;
            let ((labels, conf), _) = crate::classify::on_device(|dev| Ok(c.predict(dev, &values, w, h)))?;
            let mut out = Vec::with_capacity(8 + labels.len() * 3);
            out.extend_from_slice(&(w as u32).to_le_bytes());
            out.extend_from_slice(&(h as u32).to_le_bytes());
            for v in &labels {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend(conf.iter().map(|p| (p.clamp(0.0, 1.0) * 255.0).round() as u8));
            Ok(out)
        })
        .await?;
        Ok(Response::new(bytes))
    }
}

/// Classifies slices `z0..z1` and writes the result into the label map as
/// one undo step, skipping pixels below `min_conf`; reports
/// `classify-apply` {done, total}.
#[tauri::command]
async fn classify_apply(app: AppHandle, req: ClassifyReq, z0: usize, z1: usize, guard: u16, min_conf: f32) -> CmdResult<LabelsState> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = (app, req, z0, z1, guard, min_conf);
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        let state = app.state::<AppState>();
        let e = labels_entry(&state, req.token)?;
        let img = current(&state, req.token)?;
        let app2 = app.clone();
        crate::classify::CANCEL.store(false, Ordering::SeqCst);
        blocking(move || {
            use crate::labels::Values;
            let Some(c) = e.lock().unwrap().classifier.clone() else { anyhow::bail!("还没有训练分类器") };
            let info = img.info();
            let (w, h) = (info.width, info.height);
            let z1 = z1.min(info.size_z);
            let vals = crate::labels::ChannelValues { img: img.clone(), c: req.c, t: req.t };
            let total = z1.saturating_sub(z0);
            let mut out = Vec::with_capacity(total);
            for (i, z) in (z0..z1).enumerate() {
                if crate::classify::CANCEL.load(Ordering::SeqCst) {
                    anyhow::bail!("已取消");
                }
                let values = vals.slice(z)?;
                let ((mut labels, conf), _) = crate::classify::on_device(|dev| Ok(c.predict(dev, &values, w, h)))?;
                for (l, p) in labels.iter_mut().zip(&conf) {
                    if *p < min_conf {
                        *l = 0;
                    }
                }
                out.push((z, labels));
                let _ = app2.emit("classify-apply", serde_json::json!({ "done": i + 1, "total": total }));
            }
            let mut e = e.lock().unwrap();
            let zs = e.labels.apply_classes(&out, guard);
            Ok(labels_state(&e, zs))
        })
        .await
    }
}

// ---------- Pro U-Net ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnetInfo {
    labels: Vec<u16>,
    /// Labelled slices it was trained on.
    slices: usize,
    loss: f32,
    accuracy: f32,
    backend: String,
    seconds: f32,
}

/// Trains a U-Net on the slices of the label map that hold labels;
/// reports `unet-train` {step, steps, loss}.
#[tauri::command]
async fn unet_train(app: AppHandle, req: ClassifyReq, steps: usize) -> CmdResult<UnetInfo> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = (app, req, steps);
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        let state = app.state::<AppState>();
        let e = labels_entry(&state, req.token)?;
        let img = current(&state, req.token)?;
        let app2 = app.clone();
        crate::unet::CANCEL.store(false, Ordering::SeqCst);
        blocking(move || {
            use crate::labels::Values;
            let started = std::time::Instant::now();
            let labelled: Vec<(usize, Vec<u16>)> = {
                let e = e.lock().unwrap();
                e.labels.used_slices().into_iter().filter_map(|z| e.labels.slice(z).map(|v| (z, v.to_vec()))).collect()
            };
            if labelled.is_empty() {
                anyhow::bail!("先在几层上画出至少两种标签（比如物体和背景），再训练");
            }
            let info = img.info();
            let (w, h) = (info.width, info.height);
            let vals = crate::labels::ChannelValues { img: img.clone(), c: req.c, t: req.t };
            let mut slices = Vec::with_capacity(labelled.len());
            for (z, l) in labelled {
                slices.push((vals.slice(z)?, l));
            }
            let cfg = crate::unet::TrainConfig { steps: steps.clamp(10, 20000), ..Default::default() };
            let (t, backend) = crate::classify::on_device(|dev| {
                crate::unet::train(dev, &slices, w, h, req.lo, req.hi, &cfg, |step, steps, loss| {
                    let _ = app2.emit("unet-train", serde_json::json!({ "step": step, "steps": steps, "loss": loss }));
                })
            })?;
            let out = UnetInfo {
                labels: t.labels.clone(),
                slices: slices.len(),
                loss: t.loss,
                accuracy: t.accuracy,
                backend: backend.to_string(),
                seconds: started.elapsed().as_secs_f32(),
            };
            eprintln!("unet: {} slices, {} labels, loss {:.3}, acc {:.3}, {} in {:.1} s", out.slices, out.labels.len(), out.loss, out.accuracy, out.backend, out.seconds);
            e.lock().unwrap().unet = Some(Arc::new(t));
            Ok(out)
        })
        .await
    }
}

/// Like `classify_predict`, with the trained U-Net.
#[tauri::command]
async fn unet_predict(state: State<'_, AppState>, req: ClassifyReq, z: usize) -> CmdResult<Response> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = (state, req, z);
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        let e = labels_entry(&state, req.token)?;
        let img = current(&state, req.token)?;
        let bytes = blocking(move || {
            use crate::labels::Values;
            let Some(t) = e.lock().unwrap().unet.clone() else { anyhow::bail!("还没有训练 U-Net") };
            let info = img.info();
            let (w, h) = (info.width, info.height);
            let values = crate::labels::ChannelValues { img: img.clone(), c: req.c, t: req.t }.slice(z)?;
            let ((labels, conf), _) = crate::classify::on_device(|dev| Ok(t.predict(dev, &values, w, h)))?;
            let mut out = Vec::with_capacity(8 + labels.len() * 3);
            out.extend_from_slice(&(w as u32).to_le_bytes());
            out.extend_from_slice(&(h as u32).to_le_bytes());
            for v in &labels {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend(conf.iter().map(|p| (p.clamp(0.0, 1.0) * 255.0).round() as u8));
            Ok(out)
        })
        .await?;
        Ok(Response::new(bytes))
    }
}

/// Labels slices `z0..z1` with the U-Net as one undo step; with
/// `only_empty`, slices that already hold labels are left alone. Reports
/// `unet-apply` {done, total}.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn unet_apply(app: AppHandle, req: ClassifyReq, z0: usize, z1: usize, guard: u16, min_conf: f32, only_empty: bool) -> CmdResult<LabelsState> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = (app, req, z0, z1, guard, min_conf, only_empty);
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        let state = app.state::<AppState>();
        let e = labels_entry(&state, req.token)?;
        let img = current(&state, req.token)?;
        let app2 = app.clone();
        crate::unet::CANCEL.store(false, Ordering::SeqCst);
        blocking(move || {
            use crate::labels::Values;
            let (t, skip) = {
                let e = e.lock().unwrap();
                let Some(t) = e.unet.clone() else { anyhow::bail!("还没有训练 U-Net") };
                let skip: HashSet<usize> = if only_empty { e.labels.used_slices().into_iter().collect() } else { HashSet::new() };
                (t, skip)
            };
            let info = img.info();
            let (w, h) = (info.width, info.height);
            let zs: Vec<usize> = (z0..z1.min(info.size_z)).filter(|z| !skip.contains(z)).collect();
            let vals = crate::labels::ChannelValues { img: img.clone(), c: req.c, t: req.t };
            let mut out = Vec::with_capacity(zs.len());
            for (i, &z) in zs.iter().enumerate() {
                if crate::unet::CANCEL.load(Ordering::SeqCst) {
                    anyhow::bail!("已取消");
                }
                let values = vals.slice(z)?;
                let ((mut labels, conf), _) = crate::classify::on_device(|dev| Ok(t.predict(dev, &values, w, h)))?;
                for (l, p) in labels.iter_mut().zip(&conf) {
                    if *p < min_conf {
                        *l = 0;
                    }
                }
                out.push((z, labels));
                let _ = app2.emit("unet-apply", serde_json::json!({ "done": i + 1, "total": zs.len() }));
            }
            let mut e = e.lock().unwrap();
            let zs = e.labels.apply_classes(&out, guard);
            Ok(labels_state(&e, zs))
        })
        .await
    }
}

#[tauri::command]
fn unet_cancel() {
    #[cfg(feature = "pro")]
    crate::unet::CANCEL.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn classify_cancel() {
    #[cfg(feature = "pro")]
    crate::classify::CANCEL.store(true, Ordering::SeqCst);
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
            .filter(|p| p.is_file() && formats::is_supported(p) && !crate::labels::is_labels_file(p))
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
    // Android: the system picker hands out content:// links; the Kotlin side
    // turns them into paths the readers can open and map.
    #[cfg(target_os = "android")]
    {
        android::pick_file(&app)
    }
    #[cfg(not(target_os = "android"))]
    {
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

/// Mean spectrum of a region; see `spectrum::values`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn spectrum_values(
    state: State<'_, AppState>,
    token: u64,
    axis: String,
    c: usize,
    z: usize,
    t: usize,
    region: [f64; 4],
) -> CmdResult<Vec<f64>> {
    let img = current(&state, token)?;
    blocking(move || spectrum::values(&img, &axis, c, z, t, region)).await
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

/// CTF fit (Pro) on the power spectrum of a region; see `ctf::fit_region`.
#[tauri::command]
async fn ctf_fit(state: State<'_, AppState>, req: crate::ctf::FitReq) -> CmdResult<crate::ctf::Fit> {
    let img = current(&state, req.token)?;
    blocking(move || crate::ctf::fit_region(&img, &req)).await
}

/// CTF fit (Pro) on the same region of every slice (tilt series, stack);
/// reports `ctf-stack` {done, total}.
#[tauri::command]
async fn ctf_fit_stack(app: AppHandle, req: crate::ctf::StackReq) -> CmdResult<crate::ctf::StackFit> {
    let img = current(&app.state::<AppState>(), req.token)?;
    blocking(move || {
        let progress = |done, total| {
            let _ = app.emit("ctf-stack", serde_json::json!({ "done": done, "total": total }));
        };
        crate::ctf::fit_stack(&img, &req, &progress)
    })
    .await
}

#[tauri::command]
fn ctf_cancel() {
    crate::ctf::CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Drift / motion correction (Pro): shifts of every slice; reports
/// `align-progress` {done, total}.
#[tauri::command]
async fn align_estimate(app: AppHandle, req: crate::align::AlignReq) -> CmdResult<crate::align::Aligned> {
    #[cfg(feature = "pro")] let job=MotionJob::begin(false)?;
    let img = current(&app.state::<AppState>(), req.token)?;
    blocking(move || {
        let progress = |done, total| {
            let _ = app.emit("align-progress", serde_json::json!({ "done": done, "total": total }));
        };
        #[cfg(feature = "pro")] let _job=job;
        crate::align::estimate(&img, &req, &progress)
    })
    .await
}

/// Beam-induced movie correction, sharing the global movie solver.
#[cfg(feature = "pro")]
#[tauri::command]
async fn motion_run(app: AppHandle, req: crate::motion::RunReq) -> CmdResult<crate::motion::MotionResult> {
    let img = {
        let state=app.state::<AppState>();
        if !state.save_paths.allowed(&req.output) {return Err("输出路径必须由保存对话框选择".into());}
        req.token.map(|token|current(&state,token)).transpose()?
    };
    let job=MotionJob::begin(true)?;
    blocking(move || {let _job=job;crate::motion::run(&req,img,&|stage,done,total|{let _=app.emit("motion-progress",serde_json::json!({"stage":stage,"done":done,"total":total}));})}).await
}

#[cfg(feature = "pro")]
#[tauri::command]
async fn motion_series(app: AppHandle, req: crate::motion::series::SeriesReq) -> CmdResult<crate::motion::series::SeriesResult> {
    if !app.state::<AppState>().save_paths.allowed(&req.output) {return Err("输出路径必须由保存对话框选择".into());}
    let job=MotionJob::begin(true)?;
    blocking(move || {let _job=job;crate::motion::series::run(&req,&|stage,done,total|{let _=app.emit("motion-progress",serde_json::json!({"stage":stage,"done":done,"total":total}));})}).await
}

#[cfg(feature = "pro")]
#[tauri::command]
fn motion_cancel() {if MOTION_RUNNING.load(Ordering::SeqCst){crate::motion::cancel();}}

#[cfg(feature = "pro")]
#[tauri::command]
async fn motion_pick(app:AppHandle,dir:String,kind:String)->Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let exts=match kind.as_str(){"mdoc"=>vec!["mdoc"],"gain"=>vec!["mrc","mrcs","tif","tiff"],_=>vec!["eer","mrc","mrcs","tif","tiff"]};
    app.dialog().file().set_directory(dir).add_filter("Cryo-ET",&exts).blocking_pick_file().and_then(|f|f.into_path().ok()).map(|p|p.to_string_lossy().into_owned())
}

#[cfg(feature = "pro")]
static MOTION_ALIGN_BUSY:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "pro")]
static MOTION_RUNNING:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "pro")]
struct MotionJob;
#[cfg(feature = "pro")]
impl MotionJob {
    fn begin(motion:bool)->CmdResult<Self> {
        MOTION_ALIGN_BUSY.compare_exchange(false,true,Ordering::SeqCst,Ordering::SeqCst).map_err(|_|"电影/漂移校正正在运行".to_string())?;
        MOTION_RUNNING.store(motion,Ordering::SeqCst);
        if motion {crate::motion::begin();}Ok(Self)
    }
}
#[cfg(feature = "pro")]
impl Drop for MotionJob {fn drop(&mut self){MOTION_RUNNING.store(false,Ordering::SeqCst);MOTION_ALIGN_BUSY.store(false,Ordering::SeqCst);}}

/// Tomography commands exist only in Pro builds.

/// The Cryo-ET dataset of a tilt series (`tomo::dataset`): read from
/// beside the stack, or started from its metadata. The workspace keeps
/// working on it by path while other files (its tomograms) are open.
#[cfg(feature = "pro")]
fn tomo_open(stack: &str) -> anyhow::Result<(Arc<OpenImage>, crate::tomo::dataset::Dataset)> {
    let img = crate::tomo::pipeline::open(stack)?;
    let info = img.info();
    let px = info.pixel_size.is_some().then(|| crate::align::pixel_a(info));
    let d = crate::tomo::dataset::Dataset::open(&info.path, info.size_z, px)?;
    Ok((img, d))
}

#[cfg(feature = "pro")]
#[tauri::command]
async fn tomo_dataset(stack: String) -> CmdResult<crate::tomo::dataset::Dataset> {
    blocking(move || Ok(tomo_open(&stack)?.1)).await
}

/// Saves edits made in the workspace (angles, optics).
#[cfg(feature = "pro")]
#[tauri::command]
async fn tomo_dataset_save(dataset: crate::tomo::dataset::Dataset) -> CmdResult<()> {
    blocking(move || {
        let (_, mut d) = tomo_open(&dataset.stack)?;
        if dataset.angles.len() != d.angles.len() && !d.angles.is_empty() {
            anyhow::bail!("倾转角数与叠层不一致");
        }
        d.angles = dataset.angles;
        d.optics = dataset.optics;
        d.axis_hint = dataset.axis_hint;
        d.save()
    }).await
}

#[cfg(feature = "pro")]
fn tomo_emit(app: &AppHandle) -> impl Fn(&str, usize, usize) + Sync + '_ {
    move |stage, done, total| { let _ = app.emit("tomo-progress", serde_json::json!({"stage":stage,"done":done,"total":total})); }
}

#[cfg(feature = "pro")]
#[tauri::command]
async fn tomo_align(app: AppHandle, stack: String, req: crate::tomo::AlignReq) -> CmdResult<crate::tomo::dataset::Dataset> {
    let _job = TomoJob::begin()?;
    blocking(move || {
        let _job = _job;
        let (img, mut d) = tomo_open(&stack)?;
        let emit = tomo_emit(&app);
        let mut req = req;
        if req.angles.is_empty() && req.start.is_none() {
            req.angles = d.angles.clone();
        }
        if req.axis.is_none() && req.previous.is_none() {
            req.axis = d.axis_hint;
        }
        // Tracking runs once per starting axis and patch configuration, each
        // counting views from the start: number the rounds.
        let round = std::sync::atomic::AtomicUsize::new(0);
        let last = std::sync::atomic::AtomicUsize::new(usize::MAX);
        let a = crate::tomo::align(&img, &req, &|done, total| {
            use std::sync::atomic::Ordering::Relaxed;
            if done < last.swap(done, Relaxed) { round.fetch_add(1, Relaxed); }
            emit(&format!("倾转对齐 · 第 {} 轮跟踪", round.load(Relaxed)), done, total)
        })?;
        let mut angles = vec![0.0; a.views.len()];
        for v in &a.views { angles[v.file_index] = v.angle; }
        d.angles = angles;
        d.alignment = Some(a);
        d.save()?;
        Ok(d)
    }).await
}

#[cfg(feature = "pro")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TomoStepReq<T> { stack: String, #[serde(flatten)] step: T }

/// Runs a step on a tilt series' dataset; returns its result and the dataset after it.
#[cfg(feature = "pro")]
async fn tomo_step<T: Send + 'static, R: Serialize + Send + 'static>(
    app: AppHandle,
    req: TomoStepReq<T>,
    run: impl FnOnce(&mut crate::tomo::dataset::Dataset, &T, &(dyn Fn(&str, usize, usize) + Sync)) -> anyhow::Result<R> + Send + 'static,
) -> CmdResult<serde_json::Value> {
    let _job = TomoJob::begin()?;
    blocking(move || {
        let _job = _job;
        let (_, mut d) = tomo_open(&req.stack)?;
        let emit = tomo_emit(&app);
        let r = run(&mut d, &req.step, &emit)?;
        Ok(serde_json::json!({ "result": r, "dataset": d }))
    }).await
}

#[cfg(feature = "pro")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TomoCtfStep { n: usize, params: crate::ctf::Params }

#[cfg(feature = "pro")]
#[tauri::command]
async fn tomo_ctf(app: AppHandle, req: TomoStepReq<TomoCtfStep>) -> CmdResult<serde_json::Value> {
    tomo_step(app, req, |d, s, p| crate::tomo::pipeline::ctf(d, &s.params, s.n, p)).await
}

#[cfg(feature = "pro")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TomoExportStep { format: String, dir: String, #[serde(default = "one")] bin: usize }

#[cfg(feature = "pro")]
fn one() -> usize { 1 }

/// Writes the dataset for other software (`tomo::export`) into a folder the user picked.
#[cfg(feature = "pro")]
#[tauri::command]
async fn tomo_export(app: AppHandle, req: TomoStepReq<TomoExportStep>) -> CmdResult<serde_json::Value> {
    tomo_step(app, req, |d, s, p| crate::tomo::export::run(d, &s.format, &s.dir, s.bin, &|i, n| p("导出", i, n))).await
}

#[cfg(feature = "pro")]
#[tauri::command]
async fn tomo_reconstruct(app: AppHandle, req: TomoStepReq<crate::tomo::pipeline::ReconStep>) -> CmdResult<serde_json::Value> {
    tomo_step(app, req, |d, s, p| crate::tomo::pipeline::reconstruct(d, s, p)).await
}

#[cfg(feature = "pro")]
#[tauri::command]
async fn tomo_denoise(app: AppHandle, req: TomoStepReq<crate::tomo::pipeline::DenoiseStep>) -> CmdResult<serde_json::Value> {
    tomo_step(app, req, |d, s, p| {
        let Some((dev, _)) = crate::ctf_gpu::device() else { anyhow::bail!("没有可用 GPU") };
        crate::ctf_gpu::guarded(|| crate::tomo::pipeline::denoise(dev, d, s, p))?
    }).await
}

#[cfg(feature = "pro")]
#[tauri::command]
async fn tomo_wedge(app: AppHandle, req: TomoStepReq<crate::tomo::pipeline::WedgeStep>) -> CmdResult<serde_json::Value> {
    tomo_step(app, req, |d, s, p| {
        let Some((dev, _)) = crate::ctf_gpu::device() else { anyhow::bail!("没有可用 GPU") };
        crate::ctf_gpu::guarded(|| crate::tomo::pipeline::wedge(dev, d, s, p))?
    }).await
}

#[cfg(feature = "pro")]
#[tauri::command]
async fn tomo_save_aligned(app: AppHandle, req: crate::tomo::output::SaveReq) -> CmdResult<()> {
    let img = {
        let state = app.state::<AppState>();
        if !state.save_paths.allowed(&req.path) { return Err("输出路径必须由保存对话框选择".into()); }
        current(&state, req.token)?
    };
    let _job = TomoJob::begin()?;
    blocking(move || {
        let _job = _job;
        crate::tomo::output::save(&img, &req, &|done, total| { let _ = app.emit("tomo-progress", serde_json::json!({"done":done,"total":total})); })
    }).await
}

#[cfg(feature = "pro")]
#[tauri::command]
fn tomo_cancel() {
    use std::sync::atomic::Ordering;
    crate::tomo::CANCEL.store(true, Ordering::SeqCst);
    crate::denoise_learn::CANCEL.store(true, Ordering::SeqCst);
    crate::wedge::CANCEL.store(true, Ordering::SeqCst);
    crate::ctf::CANCEL.store(true, Ordering::SeqCst);
}

#[cfg(feature = "pro")]
static TOMO_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "pro")]
struct TomoJob;
#[cfg(feature = "pro")]
impl TomoJob {
    fn begin() -> CmdResult<Self> {
        use std::sync::atomic::Ordering;
        TOMO_BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).map_err(|_| "断层任务正在运行".to_string())?;
        crate::tomo::CANCEL.store(false, Ordering::SeqCst);
        Ok(Self)
    }
}
#[cfg(feature = "pro")]
impl Drop for TomoJob {
    fn drop(&mut self) { TOMO_BUSY.store(false, std::sync::atomic::Ordering::SeqCst); }
}

/// Single-particle analysis (Pro): a project folder next to the
/// micrographs (`spa::project`); each step runs on it by path.
#[cfg(feature = "pro")]
#[tauri::command]
async fn spa_open(source: String) -> CmdResult<crate::spa::project::Project> {
    blocking(move || crate::spa::project::Project::open(&source)).await
}

/// Settings edited in the workspace: particle diameter, pixel size, excluded micrographs.
#[cfg(feature = "pro")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpaSettings {
    dir: String,
    diameter_a: Option<f64>,
    pixel_a: Option<f64>,
    excluded: Option<Vec<usize>>,
}

#[cfg(feature = "pro")]
#[tauri::command]
async fn spa_settings(req: SpaSettings) -> CmdResult<crate::spa::project::Project> {
    blocking(move || {
        let mut p = crate::spa::project::Project::load(&req.dir)?;
        if let Some(d) = req.diameter_a.filter(|d| *d > 0.0) { p.diameter_a = d; }
        if let Some(px) = req.pixel_a.filter(|v| *v > 0.0) { p.optics.pixel_a = px; }
        if let Some(ex) = req.excluded {
            for (i, m) in p.mics.iter_mut().enumerate() { m.excluded = ex.contains(&i); }
        }
        p.save()?;
        Ok(p)
    }).await
}

#[cfg(feature = "pro")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpaStep {
    dir: String,
    step: String,
    #[serde(default)]
    params: serde_json::Value,
}

/// Runs one step; reports `spa-progress` {stage, done, total} and returns
/// {project, result}.
#[cfg(feature = "pro")]
#[tauri::command]
async fn spa_run(app: AppHandle, req: SpaStep) -> CmdResult<serde_json::Value> {
    use crate::spa::project as sp;
    let job = SpaJob::begin()?;
    blocking(move || {
        let _job = job;
        let emit = |stage: &str, done: usize, total: usize| { let _ = app.emit("spa-progress", serde_json::json!({"stage":stage,"done":done,"total":total})); };
        let mut p = sp::Project::load(&req.dir)?;
        let v = req.params;
        let result = match req.step.as_str() {
            "motion" => { let _m = MotionJob::begin(true).map_err(anyhow::Error::msg)?; sp::motion(&mut p, &serde_json::from_value(v)?, &emit)?; serde_json::Value::Null }
            "ctf" => {
                let n = v.get("n").and_then(|n| n.as_u64()).unwrap_or(512) as usize;
                crate::ctf::CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
                sp::ctf(&mut p, &serde_json::from_value(v["params"].clone())?, n, &emit)?;
                serde_json::Value::Null
            }
            "pick" => serde_json::json!(sp::pick_all(&mut p, &serde_json::from_value(v)?, &emit)?),
            "extract" => {
                let max_box = v.get("maxBox").and_then(|n| n.as_u64()).unwrap_or(256) as usize;
                let invert = v.get("invert").and_then(|n| n.as_bool()).unwrap_or(true);
                serde_json::to_value(sp::extract_all(&mut p, max_box, invert, &emit)?)?
            }
            "class2d" => serde_json::to_value(sp::class2d(&mut p, &serde_json::from_value(v)?, &emit)?)?,
            "select" => serde_json::json!(sp::select(&mut p, serde_json::from_value(v)?)?),
            "initial" => serde_json::to_value(sp::initial(&mut p, &serde_json::from_value(v)?, &emit)?)?,
            "flip" => { sp::flip_initial(&mut p)?; serde_json::Value::Null }
            "refine" => {
                let reference = v.get("reference").and_then(|r| r.as_str()).map(str::to_string);
                serde_json::to_value(sp::refine(&mut p, &serde_json::from_value(v["params"].clone())?, reference.as_deref(), &emit)?)?
            }
            other => anyhow::bail!("未知步骤 {other}"),
        };
        Ok(serde_json::json!({"project": p, "result": result}))
    }).await
}

/// A micrograph with picks circled: {png (base64), picks, scale}; with
/// `params` the picks are made now (preview), otherwise the saved ones.
#[cfg(feature = "pro")]
#[tauri::command]
async fn spa_preview(dir: String, mic: usize, params: Option<crate::spa::pick::PickParams>) -> CmdResult<serde_json::Value> {
    blocking(move || {
        let p = crate::spa::project::Project::load(&dir)?;
        let picks = match params {
            Some(pp) => crate::spa::project::pick_one(&p, mic, &pp)?,
            None => p.picks()?.get(mic).cloned().unwrap_or_default(),
        };
        let m = p.mics.get(mic).ok_or_else(|| anyhow::anyhow!("没有这张显微图"))?;
        let (img, w, h) = crate::spa::project::read_mic(&m.path)?;
        let (png, scale) = crate::spa::render::mic_preview(&img, w, h, &picks, p.diameter_a / 2.0 / p.optics.pixel_a, 900)?;
        Ok(serde_json::json!({"png": base64(&png), "picks": picks.len(), "scale": scale}))
    }).await
}

/// The 2D class averages as one PNG grid: {png, cols, cell, counts, scores}.
#[cfg(feature = "pro")]
#[tauri::command]
async fn spa_classes(dir: String) -> CmdResult<serde_json::Value> {
    blocking(move || {
        let p = crate::spa::project::Project::load(&dir)?;
        let c = p.class2d.as_ref().ok_or_else(|| anyhow::anyhow!("还没有二维分类"))?;
        let (v, dims, _) = crate::spa::data::read_volume(&c.path)?;
        let b = dims[0];
        let avgs: Vec<Vec<f32>> = v.chunks(b * b).map(|a| a.to_vec()).collect();
        let cols = 8;
        let cell = 72;
        let png = crate::spa::render::gallery(&avgs, b, cols, cell)?;
        Ok(serde_json::json!({"png": base64(&png), "cols": cols, "cell": cell, "counts": c.counts, "scores": c.scores, "res": c.res, "selected": c.selected}))
    }).await
}

#[cfg(feature = "pro")]
fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() { s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char); } else { s.push('='); }
        }
    }
    s
}

#[cfg(feature = "pro")]
#[tauri::command]
fn spa_cancel() {
    use std::sync::atomic::Ordering;
    crate::spa::CANCEL.store(true, Ordering::SeqCst);
    crate::ctf::CANCEL.store(true, Ordering::SeqCst);
    if SPA_BUSY.load(Ordering::SeqCst) && MOTION_RUNNING.load(Ordering::SeqCst) { crate::motion::cancel(); }
}

#[cfg(feature = "pro")]
static SPA_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "pro")]
struct SpaJob;
#[cfg(feature = "pro")]
impl SpaJob {
    fn begin() -> CmdResult<Self> {
        use std::sync::atomic::Ordering;
        SPA_BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).map_err(|_| "单颗粒任务正在运行".to_string())?;
        crate::spa::CANCEL.store(false, Ordering::SeqCst);
        Ok(Self)
    }
}
#[cfg(feature = "pro")]
impl Drop for SpaJob {
    fn drop(&mut self) { SPA_BUSY.store(false, std::sync::atomic::Ordering::SeqCst); }
}

/// 4D-STEM (Pro): virtual detectors, centre of mass, iDPC; see `stem4d::analyze`.
#[tauri::command]
async fn stem4d_analyze(app: AppHandle, req: crate::stem4d::Req) -> CmdResult<crate::stem4d::Result4d> {
    let img = current(&app.state::<AppState>(), req.token)?;
    blocking(move || crate::stem4d::analyze(&crate::stem4d::ImageFrames { img, c: req.c }, &req)).await
}

/// Finds and measures the particles of one image (Pro); see `particles`.
#[tauri::command]
async fn particles_analyze(app: AppHandle, req: crate::particles::Req) -> CmdResult<crate::particles::Result2> {
    let img = current(&app.state::<AppState>(), req.token)?;
    blocking(move || crate::particles::analyze(&img, &req)).await
}

/// EDS quantification (Pro) of a spectrum; see `spectro::eds`.
#[tauri::command]
async fn eds_quant(req: crate::spectro::EdsReq) -> CmdResult<crate::spectro::EdsResult> {
    blocking(move || crate::spectro::eds(&req)).await
}

/// EELS edge quantification (Pro) of a spectrum; see `spectro::eels`.
#[tauri::command]
async fn eels_quant(req: crate::spectro::EelsReq) -> CmdResult<crate::spectro::EelsResult> {
    blocking(move || crate::spectro::eels(&req)).await
}

/// Finds the spots, centre and 2D lattice of a diffraction pattern.
#[tauri::command]
async fn diffraction_find(app: AppHandle, req: crate::diffraction::FindReq) -> CmdResult<crate::diffraction::Pattern> {
    let img = current(&app.state::<AppState>(), req.token)?;
    blocking(move || crate::diffraction::find(&img, &req)).await
}

/// Zone axes of a unit cell whose spot pattern matches a measured lattice.
#[tauri::command]
async fn diffraction_zones(req: crate::diffraction::ZoneReq) -> CmdResult<Vec<crate::diffraction::Zone>> {
    blocking(move || crate::diffraction::zones(&req)).await
}

/// Writes the aligned stack or sum to a path from `pick_save_path`.
#[tauri::command]
async fn align_save(app: AppHandle, req: crate::align::SaveReq) -> CmdResult<()> {
    #[cfg(feature = "pro")] let job=MotionJob::begin(false)?;
    let img = {
        let state = app.state::<AppState>();
        if !state.save_paths.allowed(&req.path) {
            return Err(format!("不允许写入：{}", req.path));
        }
        current(&state, req.token)?
    };
    blocking(move || {
        let progress = |done, total| {
            let _ = app.emit("align-progress", serde_json::json!({ "done": done, "total": total }));
        };
        #[cfg(feature = "pro")] let _job=job;
        crate::align::save(&img, &req, &progress)
    })
    .await
}

#[tauri::command]
fn align_cancel() {
    #[cfg(feature = "pro")] if MOTION_RUNNING.load(Ordering::SeqCst) {return;}
    crate::align::CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Denoising (Pro): the middle of the visible region of one slice, as
/// `denoise::Denoised::to_bytes`.
#[tauri::command]
async fn denoise_preview(state: State<'_, AppState>, req: crate::denoise::PreviewReq) -> CmdResult<Response> {
    let img = current(&state, req.token)?;
    let d = blocking(move || crate::denoise::preview(&img, &req)).await?;
    Ok(Response::new(d.to_bytes()))
}

/// Writes the denoised slices to a path from `pick_save_path`; reports
/// `denoise-progress` {done, total} and returns the backend used.
#[tauri::command]
async fn denoise_save(app: AppHandle, req: crate::denoise::SaveReq) -> CmdResult<String> {
    let img = {
        let state = app.state::<AppState>();
        if !state.save_paths.allowed(&req.path) {
            return Err(format!("不允许写入：{}", req.path));
        }
        current(&state, req.token)?
    };
    blocking(move || {
        let progress = |done, total| {
            let _ = app.emit("denoise-progress", serde_json::json!({ "done": done, "total": total }));
        };
        crate::denoise::save(&img, &req, &progress).map(String::from)
    })
    .await
}

#[tauri::command]
fn denoise_cancel() {
    crate::denoise::CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
    #[cfg(feature = "pro")]
    crate::denoise_learn::CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DenoiseTrainReq {
    token: u64,
    c: usize,
    t: usize,
    /// "n2v", "frames" or "neighbours".
    mode: String,
    steps: usize,
    /// Slices averaged on each side first (n2v only).
    #[serde(default)]
    slab: usize,
    /// Pixels averaged in each direction first (1 = none).
    #[serde(default)]
    bin: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DenoiseTrained {
    mode: String,
    /// Images (or pairs) it was trained on.
    images: usize,
    loss: f32,
    backend: String,
    seconds: f32,
    /// Set when the data does not suit the method (correlated noise).
    warning: Option<String>,
}

/// Trains the self-supervised denoiser (Pro) on the open image; reports
/// `denoise-train` {step, steps, loss}. The model then serves the
/// "learned" method of `denoise_preview` and `denoise_save`.
#[tauri::command]
async fn denoise_train(app: AppHandle, req: DenoiseTrainReq) -> CmdResult<DenoiseTrained> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = (app, req);
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        use crate::denoise_learn::{Mode, CANCEL, MODEL};
        let img = current(&app.state::<AppState>(), req.token)?;
        CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
        blocking(move || {
            let started = std::time::Instant::now();
            let info = img.info();
            let (w, h, nz) = (info.width, info.height, info.size_z);
            let mode = match req.mode.as_str() {
                "n2v" => Mode::N2v,
                "frames" => Mode::Frames,
                "neighbours" => Mode::Neighbours,
                m => anyhow::bail!("未知的训练方式 {m}"),
            };
            if mode != Mode::N2v && nz < 2 {
                anyhow::bail!("这种方式需要叠层（至少两层）");
            }
            // At most 16 slices and about 64 M pixels, spread over the stack.
            let most = (64_000_000 / (w * h).max(1)).clamp(1, 16);
            let spread = |n: usize| -> Vec<usize> { let k = n.min(most); (0..k).map(|i| i * n / k).collect() };
            let pairs: Vec<(Vec<f32>, Option<Vec<f32>>)> = match mode {
                Mode::N2v => spread(nz).into_iter().map(|z| Ok((crate::denoise::input(&img, req.c, z, req.t, req.slab)?, None))).collect::<anyhow::Result<_>>()?,
                Mode::Frames => {
                    let [a, b] = crate::denoise::frame_halves(&img, req.c, req.t)?;
                    vec![(a, Some(b))]
                }
                // Neighbouring slices, unaveraged so that their noise stays independent.
                Mode::Neighbours => spread(nz - 1)
                    .into_iter()
                    .map(|z| Ok((crate::denoise::input(&img, req.c, z, req.t, 0)?, Some(crate::denoise::input(&img, req.c, z + 1, req.t, 0)?))))
                    .collect::<anyhow::Result<_>>()?,
            };
            let k = req.bin.clamp(1, 8);
            let (bw, bh) = ((w / k).max(1), (h / k).max(1));
            let shrink = |v: Vec<f32>| crate::denoise_learn::bin(&v, w, h, k).0;
            let pairs: Vec<_> = pairs.into_iter().map(|(a, b)| (shrink(a), b.map(shrink))).collect();
            let cfg = crate::denoise_learn::TrainConfig { steps: req.steps.clamp(20, 20000), ..Default::default() };
            let app2 = app.clone();
            let ((net, mean, std, loss), backend) = crate::classify::on_device(|dev| {
                crate::denoise_learn::train(dev, &pairs, bw, bh, mode, &cfg, |step, steps, loss| {
                    let _ = app2.emit("denoise-train", serde_json::json!({ "step": step, "steps": steps, "loss": loss }));
                })
            })?;
            *MODEL.lock().unwrap() = Some(Arc::new(crate::denoise_learn::Learned { net, mode, mean, std, loss, source: info.name.clone(), bin: k }));
            // Self-supervision assumes noise independent from pixel to pixel.
            let corr = crate::denoise::noise_correlation(&pairs[0].0, bw, bh);
            let warning = (corr.abs() > 0.15).then(|| format!("这份数据的噪声在相邻像素间相关（{corr:+.2}），可能已经滤过波或插值过；自监督降噪对这种数据效果有限"));
            let out = DenoiseTrained { mode: req.mode, images: pairs.len(), loss, backend: backend.to_string(), seconds: started.elapsed().as_secs_f32(), warning };
            eprintln!("denoise_train: {} on {} images, loss {:.4}, {} in {:.1} s", out.mode, out.images, out.loss, out.backend, out.seconds);
            Ok(out)
        })
        .await
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(feature = "pro"), allow(dead_code))]
struct WedgeReq {
    token: u64,
    c: usize,
    t: usize,
    bin: usize,
    #[cfg(feature = "pro")]
    #[serde(default)]
    geometry: Option<crate::wedge::Geometry>,
    #[cfg(feature = "pro")]
    #[serde(default)]
    config: Option<crate::wedge::TrainConfig>,
}

/// Missing-wedge correction (Pro): measures the beam axis, tilt axis and
/// tilt range from the open stack's power spectrum.
#[tauri::command]
async fn wedge_detect(app: AppHandle, req: WedgeReq) -> CmdResult<serde_json::Value> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = (app, req);
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        let img = current(&app.state::<AppState>(), req.token)?;
        crate::wedge::CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
        blocking(move || {
            let (vol, _, _) = crate::wedge::volume(&img, req.c, req.t, req.bin)?;
            Ok(serde_json::to_value(crate::wedge::detect(&vol))?)
        })
        .await
    }
}

/// Trains the missing-wedge network on the open stack; reports
/// `wedge-train` (`wedge::Progress`). Then `wedge_preview` and `wedge_save`.
#[tauri::command]
async fn wedge_train(app: AppHandle, req: WedgeReq) -> CmdResult<serde_json::Value> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = (app, req);
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        use crate::wedge::{CANCEL, MODEL};
        let img = current(&app.state::<AppState>(), req.token)?;
        CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
        blocking(move || {
            let started = std::time::Instant::now();
            let Some(g) = req.geometry else { anyhow::bail!("缺少倾转几何") };
            let cfg = req.config.unwrap_or_default();
            let (vol, mean, std) = crate::wedge::volume(&img, req.c, req.t, req.bin)?;
            let vol = if cfg.smooth > 0.0 { Arc::new(crate::wedge::smooth(&vol, cfg.smooth)) } else { vol };
            // Cubes no larger than the volume's thinnest side allows.
            let mut cfg = cfg;
            while cfg.cube > 16 && vol.n.iter().any(|&k| k < cfg.cube) {
                cfg.cube /= 2;
            }
            let app2 = app.clone();
            let ((net, loss), backend) = crate::classify::on_device(|dev| {
                crate::wedge::train(dev, &vol, &g, &cfg, |p| {
                    let _ = app2.emit("wedge-train", p);
                })
            })?;
            *MODEL.lock().unwrap() = Some(Arc::new(crate::wedge::Trained { net, vol: vol.clone(), geometry: g, bin: req.bin.max(1), mean, std, loss, token: req.token, c: req.c, t: req.t }));
            let seconds = started.elapsed().as_secs_f32();
            eprintln!("wedge_train: {:?} cube {} × {} {} it × {} steps, loss {loss:.4}, {backend} in {seconds:.1} s", vol.n, cfg.cube, cfg.count, cfg.iterations, cfg.steps);
            Ok(serde_json::json!({ "loss": loss, "backend": backend, "seconds": seconds, "dims": vol.n, "cube": cfg.cube }))
        })
        .await
    }
}

/// A corrected slice of the trained stack, as `wedge::preview` lays it out.
#[tauri::command]
async fn wedge_preview(req: serde_json::Value) -> CmdResult<Response> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = req;
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        let req: crate::wedge::PreviewReq = serde_json::from_value(req).map_err(err)?;
        let Some(m) = crate::wedge::MODEL.lock().unwrap().clone() else { return Err("还没有训练网络".into()) };
        let bytes = blocking(move || crate::wedge::preview(&m, &req)).await?;
        Ok(Response::new(bytes))
    }
}

/// Writes the whole corrected volume to a path from `pick_save_path`;
/// reports `wedge-progress` {done, total}; returns the backend.
#[tauri::command]
async fn wedge_save(app: AppHandle, path: String) -> CmdResult<String> {
    #[cfg(not(feature = "pro"))]
    {
        let _ = (app, path);
        pro_only()
    }
    #[cfg(feature = "pro")]
    {
        if !app.state::<AppState>().save_paths.allowed(&path) {
            return Err(format!("不允许写入：{path}"));
        }
        let Some(m) = crate::wedge::MODEL.lock().unwrap().clone() else { return Err("还没有训练网络".into()) };
        let pixel_a = current(&app.state::<AppState>(), m.token).map(|img| crate::align::pixel_a(img.info())).unwrap_or(1.0);
        blocking(move || {
            let progress = |done, total| {
                let _ = app.emit("wedge-progress", serde_json::json!({ "done": done, "total": total }));
            };
            crate::wedge::save(&m, &path, pixel_a, &progress).map(String::from)
        })
        .await
    }
}

#[tauri::command]
fn wedge_cancel() {
    #[cfg(feature = "pro")]
    crate::wedge::CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
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
    // Android: the system tree picker; the Kotlin side turns it into a path.
    #[cfg(target_os = "android")]
    {
        let _ = dir;
        android::pick_folder(&app)
    }
    #[cfg(not(target_os = "android"))]
    {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .set_directory(dir)
        .blocking_pick_folder()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
    }
}

/// The folder button: an image or a folder of slices from one panel. Only the
/// macOS panel can choose both; elsewhere the UI asks first and this is the
/// plain file dialog.
#[tauri::command]
async fn pick_open(app: AppHandle, dir: String) -> Option<String> {
    #[cfg(target_os = "macos")]
    return pick_open_mac(&app, dir);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = dir;
        pick_file(app).await
    }
}

#[cfg(target_os = "macos")]
fn pick_open_mac(app: &AppHandle, dir: String) -> Option<String> {
    let exts = formats::extensions();
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
        use objc2_foundation::{NSArray, NSString, NSURL};
        let Some(mtm) = MainThreadMarker::new() else { return };
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(true);
        panel.setAllowsMultipleSelection(false);
        // Only image files are selectable; every folder stays selectable too.
        let types: Vec<_> = exts.iter().map(|e| NSString::from_str(e)).collect();
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&types)));
        if !dir.is_empty() {
            panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(&dir))));
        }
        let picked = if panel.runModal() == NSModalResponseOK { panel.URL() } else { None };
        let _ = tx.send(picked.and_then(|u| u.path()).map(|p| p.to_string()));
    })
    .ok()?;
    rx.recv().ok().flatten()
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
#[cfg_attr(target_os = "android", allow(dead_code))]
fn queue_open(app: &AppHandle, paths: Vec<String>) {
    for path in paths {
        let label = target_window(app, &path);
        let state = app.state::<AppState>();
        state.pending.lock().unwrap().entry(label.clone()).or_default().push(path);
        match app.get_webview_window(&label) {
            Some(w) => {
                let _ = app.emit_to(&label, "open-pending", ());
                #[cfg(not(target_os = "android"))]
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

#[cfg_attr(target_os = "android", allow(dead_code))]
fn target_window(app: &AppHandle, path: &str) -> String {
    let state = app.state::<AppState>();
    let dir = Path::new(path).parent();
    let showing = state.showing.lock().unwrap();
    let pending = state.pending.lock().unwrap();
    let windows = app.webview_windows();
    // On a cold start macOS hands over the file before the main window
    // exists; it is about to be created, so give the file to it. Android
    // shows a single window.
    if windows.is_empty() || cfg!(target_os = "android") {
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
        #[cfg(not(target_os = "android"))]
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[cfg(target_os = "android")]
#[allow(dead_code)]
fn new_window(app: &AppHandle, label: &str) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into())).build()
}

#[cfg(not(target_os = "android"))]
fn new_window(app: &AppHandle, label: &str) -> tauri::Result<WebviewWindow> {
    // Cascade from the window in front so the new one doesn't hide it.
    let pos = app
        .webview_windows()
        .values()
        .find(|w| w.is_focused().unwrap_or(false))
        .and_then(|w| Some((w.outer_position().ok()?, w.scale_factor().ok()?)));
    let mut b = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(crate::edition::NAME)
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
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            let paths = paths_from_args(args, Some(Path::new(&cwd)));
            queue_open(app, paths);
        }));
    }
    #[cfg(not(target_os = "android"))]
    {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    builder = builder
        // After single-instance: it forwards a second instance's link here.
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init());
    #[cfg(target_os = "android")]
    {
        builder = builder.plugin(android::init());
    }
    builder
        .manage(state)
        .manage(update::Pending::default())
        .manage(InstallRequest::default())
        .manage(plugins::MarketCache::default())
        .manage(crate::sam::Sam::default())
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
            volume_data,
            labels_open,
            labels_slice,
            labels_edit,
            labels_save,
            labels_volume,
            sam_status,
            sam_download,
            sam_predict,
            sam_apply,
            sam_propagate,
            sam_cancel,
            classify_scribble,
            classify_scribbles,
            classify_clear,
            classify_train,
            classify_predict,
            classify_apply,
            classify_cancel,
            unet_train,
            unet_predict,
            unet_apply,
            unet_cancel,
            pixel_values,
            list_folder,
            thumbnail,
            pick_file,
            set_default_app,
            reveal_in_folder,
            histogram,
            power_spectrum,
            ctf_fit,
            ctf_fit_stack,
            ctf_cancel,
            align_estimate,
            #[cfg(feature = "pro")] motion_run,
            #[cfg(feature = "pro")] motion_series,
            #[cfg(feature = "pro")] motion_cancel,
            #[cfg(feature = "pro")] motion_pick,
            #[cfg(feature = "pro")] tomo_dataset,
            #[cfg(feature = "pro")] tomo_dataset_save,
            #[cfg(feature = "pro")] tomo_ctf,
            #[cfg(feature = "pro")] tomo_denoise,
            #[cfg(feature = "pro")] tomo_wedge,
            #[cfg(feature = "pro")] tomo_export,
            #[cfg(feature = "pro")] tomo_align,
            #[cfg(feature = "pro")] tomo_save_aligned,
            #[cfg(feature = "pro")] tomo_reconstruct,
            #[cfg(feature = "pro")] tomo_cancel,
            #[cfg(feature = "pro")] spa_open,
            #[cfg(feature = "pro")] spa_settings,
            #[cfg(feature = "pro")] spa_run,
            #[cfg(feature = "pro")] spa_preview,
            #[cfg(feature = "pro")] spa_classes,
            #[cfg(feature = "pro")] spa_cancel,
            diffraction_find,
            diffraction_zones,
            eds_quant,
            stem4d_analyze,
            particles_analyze,
            eels_quant,
            align_save,
            align_cancel,
            denoise_preview,
            denoise_save,
            denoise_cancel,
            denoise_train,
            wedge_detect,
            wedge_train,
            wedge_preview,
            wedge_save,
            wedge_cancel,
            spectrum_values,
            pick_save_path,
            pick_folder,
            pick_open,
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
            plugins::open_link,
            plugins::open_website,
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
                prune_labels(&state);
            }
            if let tauri::RunEvent::Exit = &_event {
                let state = _app.state::<AppState>();
                state.current.lock().unwrap().clear();
                prune_labels(&state);
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
