// Included only by the Pro desktop build, alongside desktop.rs.

#[tauri::command]
fn close_pro_window(window: WebviewWindow) -> CmdResult<()> { window.destroy().map_err(err) }

#[tauri::command]
async fn pick_merge_project(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog().file().add_filter("TifMrcView 合成工程", &["tmvmerge"])
        .blocking_pick_file().and_then(|f| f.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
fn activate_document(window: WebviewWindow, state: State<'_, AppState>, token: u64) -> CmdResult<()> {
    let image = state.documents.get(window.label(), token).ok_or("stale")?;
    state.current.lock().unwrap().insert(window.label().to_string(), image.clone());
    state.showing.lock().unwrap().insert(window.label().to_string(), image.info().path.clone());
    state.side.lock().unwrap().remove(window.label());
    Ok(())
}

#[tauri::command]
fn close_document(window: WebviewWindow, state: State<'_, AppState>, token: u64) -> CmdResult<()> {
    // A window cannot release a different window's sources.
    if state.documents.get(window.label(), token).is_none() { return Err("stale".into()); }
    let mut current = state.current.lock().unwrap();
    if current.get(window.label()).is_some_and(|image| image.token == token) {
        current.remove(window.label());
        state.showing.lock().unwrap().remove(window.label());
        state.side.lock().unwrap().remove(window.label());
    }
    drop(current);
    state.documents.remove(window.label(), token);
    state.documents.rebalance(mem::cache_budget());
    prune_labels(&state);
    Ok(())
}

/// Open a merge source without replacing the active image or its annotations.
#[tauri::command]
async fn open_layer_source(window: WebviewWindow, state: State<'_, AppState>, path: String, series: usize) -> CmdResult<Opened> {
    if let Some(image) = state.documents.all().into_iter().find(|image|
        image.info().path == path && image.info().series_index == series && state.documents.get(window.label(), image.token).is_some()) {
        return Ok(Opened { token: image.token, info: image.info().clone() });
    }
    let token = state.next_token.fetch_add(1, Ordering::Relaxed) + 1;
    let budget = mem::cache_budget() / (state.documents.all().len() + 1);
    let image = blocking(move || Ok(OpenImage::new(token, formats::open_series(Path::new(&path), series)?, budget))).await?;
    let info = image.info().clone();
    // Share the close/open lock so a closed window cannot acquire orphan sources.
    let current = state.current.lock().unwrap();
    if window.app_handle().get_webview_window(window.label()).is_none() { return Err("window closed".into()); }
    state.documents.keep(window.label(), image);
    drop(current);
    state.documents.rebalance(mem::cache_budget());
    Ok(Opened { token, info })
}

#[tauri::command]
fn read_merge_project(state: State<'_, AppState>, path: String) -> CmdResult<serde_json::Value> {
    if std::fs::metadata(&path).map_err(err)?.len() > 4 * 1024 * 1024 { return Err("合成工程文件过大".into()); }
    let project: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).map_err(err)?).map_err(err)?;
    if project["format"] != "tifmrcview-merge" || project["version"] != 1 { return Err("不支持的合成工程格式或版本".into()); }
    state.save_paths.allow(&path);
    Ok(project)
}

#[tauri::command]
async fn create_merge_project(window: WebviewWindow, state: State<'_, AppState>, app: AppHandle, token: u64, project: serde_json::Value) -> CmdResult<String> {
    let source = state.documents.get(window.label(), token).ok_or("stale")?;
    let source = PathBuf::from(&source.info().path);
    let fallback = app.path().app_data_dir().ok().map(|p| p.join("merge-projects"));
    let path = blocking(move || crate::merge_project::create(&source, fallback.as_deref(), &project)).await?;
    let path = path.to_string_lossy().into_owned();
    state.save_paths.allow(&path);
    Ok(path)
}

#[tauri::command]
async fn save_merge_project(state: State<'_, AppState>, path: String, project: serde_json::Value) -> CmdResult<()> {
    if !state.save_paths.allowed(&path) { return Err("请选择工程保存位置".into()); }
    blocking(move || crate::merge_project::save(Path::new(&path), &project)).await
}

#[tauri::command]
async fn save_merged_image(state: State<'_, AppState>, request: tauri::ipc::Request<'_>) -> CmdResult<()> {
    let tauri::ipc::InvokeBody::Raw(data) = request.body() else { return Err("expected PNG body".into()); };
    let path = request.headers().get("x-path").and_then(|v| v.to_str().ok()).map(percent_decode).ok_or("missing path")?;
    if !state.save_paths.allowed(&path) { return Err("请选择图像保存位置".into()); }
    let data = data.clone();
    blocking(move || {
        let bytes = crate::merge_export::encode(Path::new(&path), &data)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }).await
}
