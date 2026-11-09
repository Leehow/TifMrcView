"use strict";
// Pro layer editor and display compositor. Raw sources are never modified.
(() => {
  const D = window.TifMrcViewPro.documents, M = window.TmvMerge;
  const cache = new Map();
  const stage = $("stage");
  const preview = document.createElement("canvas");
  preview.id = "merge-view";
  preview.tabIndex = 0;
  preview.hidden = true;
  stage.append(preview);
  const zbars = document.createElement("section");
  zbars.id = "merge-zbars";
  zbars.hidden = true;
  $("zbar").after(zbars);
  const panel = document.createElement("section");
  panel.id = "merge-panel";
  panel.setAttribute("role", "tabpanel");
  panel.setAttribute("aria-label", "图层");
  panel.innerHTML = `
    <div class="merge-heading"><b>图层</b><span class="merge-heading-space"></span><button id="merge-more" title="合成图操作" aria-label="合成图更多操作">更多</button><button id="merge-panel-close" class="panel-close" title="收起侧栏" aria-label="收起图层面板"></button></div>
    <div id="merge-intro" class="merge-empty"><p>将图片拖到画布，即可叠加图层。</p><button id="merge-empty-add" class="small">添加图层…</button></div>
    <div id="merge-editor" hidden>
      <div id="merge-properties">
        <label>混合 <select id="merge-mode"><option value="normal">正常</option><option value="add">荧光相加</option><option value="screen">滤色</option></select></label>
        <label>不透明度 <input id="merge-opacity" type="range" min="0" max="100" value="100" /><output id="merge-opacity-value">100%</output></label>
        <label>颜色 <select id="merge-color"><option value="">原图颜色</option><option value="#ffffff">灰度 / 白</option><option value="#ff0000">红</option><option value="#00ff00">绿</option><option value="#0000ff">蓝</option><option value="#ffff00">黄</option><option value="#ff00ff">品红</option><option value="#00ffff">青</option><option value="custom">自定义</option></select><input id="merge-custom-color" type="color" value="#00ff00" aria-label="自定义图层颜色" /></label>
      </div>
      <div id="merge-list" role="list" aria-label="图层列表（上层在前）"></div>
      <div class="merge-order"><button id="merge-add">添加</button><button id="merge-up" title="图层上移">上移</button><button id="merge-down" title="图层下移">下移</button><button id="merge-delete">删除</button></div>
      <details id="merge-transform" class="merge-detail"><summary>位置与大小</summary>
        <label class="merge-field">名称 <input id="merge-name" type="text" /></label>
        <div class="merge-geometry"><label>X <input id="merge-x" type="number" step="1" /></label><label>Y <input id="merge-y" type="number" step="1" /></label><label>宽 <input id="merge-width" type="number" min="1" step="1" /></label><label>高 <input id="merge-height" type="number" min="1" step="1" /></label></div>
        <label class="merge-check"><input id="merge-aspect" type="checkbox" checked />锁定宽高比</label>
      </details>
      <details id="merge-levels" class="merge-detail"><summary>显示范围</summary>
        <div class="merge-geometry"><label>灰度下限 <input id="merge-min" type="number" step="any" placeholder="自动" /></label><label>灰度上限 <input id="merge-max" type="number" step="any" placeholder="自动" /></label></div>
        <button id="merge-auto" class="small">自动显示范围</button>
        <div class="merge-layer-time" id="merge-time-wrap" hidden><label>T <input id="merge-time" type="number" min="1" step="1" /></label></div>
        <div id="merge-source" class="dim"></div>
      </details>
      <details class="merge-canvas-settings merge-detail"><summary>画布设置</summary><div class="merge-geometry"><label>宽 <input id="merge-canvas-width" type="number" min="1" /></label><label>高 <input id="merge-canvas-height" type="number" min="1" /></label></div><label>背景 <input id="merge-background" type="color" value="#000000" /></label></details>
      <div class="merge-footer"><div class="merge-actions"><button id="merge-export" class="primary" title="导出当前合成的图像">导出图像…</button></div><div id="merge-save-status" class="dim" role="status"></div></div>
    </div>`;
  const imagesPane = document.createElement("section");
  imagesPane.id = "image-browser-pane";
  imagesPane.setAttribute("role", "tabpanel");
  imagesPane.setAttribute("aria-label", "图片");
  const projects = document.createElement("details");
  projects.id = "merge-projects";
  projects.open = pref.get("merge-group-open", true);
  const projectHeading = document.createElement("summary");
  projectHeading.innerHTML = '<span>合成图</span><span class="merge-project-count"></span>';
  const projectList = document.createElement("div");
  projectList.id = "merge-project-list";
  projects.append(projectHeading, projectList);
  projects.ontoggle = () => pref.set("merge-group-open", projects.open);
  $("sidebar").append(imagesPane, panel);
  imagesPane.append($("side-head"), projects, $("strip"));
  $("merge-panel-close").append($("sidebar-close").querySelector("svg").cloneNode(true));
  const rail = document.createElement("nav");
  rail.id = "right-sidebar-tabs";
  rail.setAttribute("role", "tablist");
  rail.setAttribute("aria-orientation", "vertical");
  rail.setAttribute("aria-label", "右侧栏");
  const show = document.createElement("button");
  show.id = "btn-show-layers";
  show.className = "panel-open";
  show.title = "图层";
  show.setAttribute("aria-label", "图层面板");
  // Reuse the application's existing layer icon.
  show.innerHTML = '<svg viewBox="0 0 24 24"><path d="m12 3 10 5-10 5L2 8zM2 12l10 5 10-5M2 16l10 5 10-5"/></svg>';
  rail.append($("btn-show-sidebar"), show);
  stage.append(rail);

  const current = () => D.current()?.type === "merge" ? D.current() : null;
  const selected = (doc = current()) => doc?.layers.find((l) => l.id === doc.selected);
  const busyButtons = new Map();
  let scheduled = false, rendering = false, again = false, drag = null, zkey = "", exportBusy = false, sourceScheduled = false, promoting = null;
  let sideMode = pref.get("pro-sidebar-pane", "images");
  if (!["images", "layers"].includes(sideMode)) sideMode = "images";

  function setPanel(mode = sideMode, visible = true) {
    const doc = current(), before = stage.getBoundingClientRect(), view = doc?.view && { ...doc.view };
    sideMode = mode;
    $("sidebar").classList.toggle("collapsed", !visible);
    $("sidebar-resizer").hidden = !visible;
    imagesPane.hidden = mode !== "images";
    panel.hidden = mode !== "layers";
    document.body.dataset.sidebarPane = mode;
    preview.title = mode === "layers" ? "拖动图层移动；拖动边角控制点缩放；Shift 切换等比；Alt 或中键平移" : "拖动画布平移；滚轮缩放";
    for (const [button, name, target] of [[$("btn-show-sidebar"), "images", imagesPane], [show, "layers", panel]]) {
      button.hidden = false;
      button.setAttribute("role", "tab");
      button.setAttribute("aria-selected", String(visible && name === mode));
      button.setAttribute("aria-controls", target.id);
      button.tabIndex = !visible || name === mode ? 0 : -1;
    }
    $("btn-show-sidebar").title = "图片";
    $("btn-show-sidebar").setAttribute("aria-label", "图片面板");
    pref.set("sidebar", visible);
    pref.set("pro-sidebar-pane", mode);
    if (view) doc.view = M.reframe(view, before, stage.getBoundingClientRect());
    request();
  }
  $("btn-show-sidebar").addEventListener("click", (e) => { e.stopImmediatePropagation(); setPanel("images"); }, true);
  show.onclick = () => setPanel("layers");
  $("merge-panel-close").onclick = () => setPanel(sideMode, false);
  rail.addEventListener("keydown", (e) => {
    if (!["ArrowUp", "ArrowDown", "Home", "End"].includes(e.key)) return;
    e.preventDefault(); e.stopPropagation();
    const mode = e.key === "Home" ? "images" : e.key === "End" ? "layers" : sideMode === "images" ? "layers" : "images";
    setPanel(mode);
    (mode === "images" ? $("btn-show-sidebar") : show).focus();
  });

  function contain(output, source) {
    if (!source || !source.width || !source.height) return;
    const g = output.getContext("2d"), k = Math.min(output.width / source.width, output.height / source.height);
    g.clearRect(0, 0, output.width, output.height);
    g.drawImage(source, (output.width - source.width * k) / 2, (output.height - source.height * k) / 2, source.width * k, source.height * k);
  }

  function syncProjects() {
    const open = D.tabs.filter((d) => d.type === "merge");
    const paths = new Set(open.map((d) => d.path).filter(Boolean));
    const recovery = D.recoveryList().filter((r) => r?.project && !open.some((d) => d.autosaveId === r.id || r.path && d.path === r.path)
      && (!S.folder.dir || r.dir === S.folder.dir));
    const recoveryPaths = new Set(recovery.map((r) => r.path).filter(Boolean));
    const stored = pref.get("merge-projects", []);
    const saved = (Array.isArray(stored) ? stored : []).filter((p) => p?.path && !paths.has(p.path) && !recoveryPaths.has(p.path)
      && (!S.folder.dir || p.dir === S.folder.dir || dirname(p.path) === S.folder.dir));
    const entries = [...open.map((doc) => ({ key: doc.id, doc, ...doc })),
      ...recovery.map((r) => ({ ...r, key: r.id, recovery: true })), ...saved.map((p) => ({ key: p.path, ...p }))];
    projects.hidden = entries.length === 0;
    if (!$("folder-name").textContent) $("folder-name").textContent = "图片";
    projectHeading.querySelector(".merge-project-count").textContent = entries.length;
    const previous = new Map([...projectList.children].map((row) => [row.dataset.project, row]));
    for (const entry of entries) {
      let row = previous.get(entry.key);
      previous.delete(entry.key);
      if (!row) {
        row = document.createElement("div"); row.className = "thumb merge-project-card";
        row.dataset.project = entry.key; row.tabIndex = 0; row.setAttribute("role", "button");
        const box = document.createElement("div"); box.className = "box";
        const thumbnail = document.createElement("canvas"); thumbnail.width = 164; thumbnail.height = 116;
        thumbnail.setAttribute("aria-hidden", "true"); box.append(thumbnail);
        const name = document.createElement("div"); name.className = "nm";
        const meta = document.createElement("div"); meta.className = "meta";
        row.append(box, name, meta);
      }
      row.classList.toggle("cur", entry.doc === D.current());
      row.querySelector(".nm").textContent = `${entry.name.replace(/\.tmvmerge$/i, "")}.tmvmerge`;
      row.querySelector(".meta").textContent = entry.recovery ? "恢复上次编辑" : entry.doc
        ? `${entry.layers.length} 个图层${entry.saveError ? " · 自动保存失败" : entry.dirty ? " · 自动保存中…" : ""}` : "合成工程";
      row.title = entry.path || "未保存的合成图";
      row.setAttribute("aria-label", row.querySelector(".nm").textContent);
      const select = () => entry.doc ? D.activate(entry.doc) : entry.recovery ? D.recover(entry.id) : D.open(entry.path);
      row.onclick = select;
      row.onkeydown = (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); e.stopPropagation(); select(); } };
      const thumbnail = row.querySelector("canvas");
      if (entry.doc?.thumbnail) contain(thumbnail, entry.doc.thumbnail);
      else if (typeof entry.preview === "string" && entry.preview.startsWith("data:image/png;base64,") && row.dataset.preview !== entry.preview) {
        row.dataset.preview = entry.preview;
        const image = new Image(); image.onload = () => contain(thumbnail, image); image.src = entry.preview;
      }
      projectList.append(row);
    }
    previous.forEach((row) => row.remove());
  }

  function syncLayerThumbs(doc) {
    for (const row of $("merge-list").children) {
      const layer = doc.layers.find((l) => l.id === row.dataset.layer);
      if (!layer) continue;
      const fallback = [...$("strip").querySelectorAll(".thumb")].find((t) => t.dataset.path === layer.path)?.querySelector("canvas");
      contain(row.querySelector(".merge-layer-thumb"), cache.get(layer.id)?.canvas || fallback);
    }
  }

  function updateThumbnail(doc) {
    syncLayerThumbs(doc);
    if (doc._thumbnailRevision === doc.revision) return;
    const output = document.createElement("canvas");
    const k = Math.min(164 / doc.width, 116 / doc.height);
    output.width = Math.max(1, Math.round(doc.width * k)); output.height = Math.max(1, Math.round(doc.height * k));
    const g = output.getContext("2d");
    g.fillStyle = doc.background; g.fillRect(0, 0, output.width, output.height);
    for (const l of doc.layers) {
      const image = cache.get(l.id)?.canvas;
      if (!image || !l.visible || l.opacity <= 0) continue;
      g.globalAlpha = l.opacity; g.globalCompositeOperation = M.operation(l.mode);
      g.drawImage(image, l.x * k, l.y * k, l.width * k, l.height * k);
    }
    paintScaleBar(doc, g, k);
    doc.thumbnail = output; doc._thumbnailRevision = doc.revision;
    if (doc.path && !doc.dirty) D.rememberProject(doc);
    syncProjects();
  }

  function state(doc) {
    return { width: doc.width, height: doc.height, background: doc.background, selected: doc.selected,
      layers: doc.layers.map((l) => ({ ...l, channels: M.copy(l.channels), scaleBar: l.scaleBar && M.copy(l.scaleBar) })) };
  }
  function remember(doc) {
    (doc.history ||= []).push(state(doc));
    if (doc.history.length > 60) doc.history.shift();
    doc.future = [];
  }
  async function edit(change, history = true) {
    const doc = current();
    if (!doc) {
      // Viewing a single layer stays a source tab. Its first layer edit creates
      // the editable composition in place, with the source view intact.
      if (D.current()?.type === "image") {
        promoting ||= D.addPaths([]).finally(() => { promoting = null; });
        const result = await promoting;
        if (result?.doc === D.current()) return edit(change, true);
      }
      return;
    }
    if (history) remember(doc);
    change(doc, selected(doc));
    doc.revision++;
    doc.dirty = true;
    D.changed();
    request();
  }
  function undo(redo = false) {
    const doc = current();
    if (!doc) return;
    const from = redo ? doc.future : doc.history;
    if (!from?.length) return;
    (redo ? (doc.history ||= []) : (doc.future ||= [])).push(state(doc));
    Object.assign(doc, from.pop());
    doc.dirty = true; doc.revision++;
    D.changed(); request();
  }

  function syncValue(id, value) { if (document.activeElement !== $(id)) $(id).value = value; }
  function syncSaveState() {
    const doc = current(), status = $("merge-save-status");
    status.hidden = !doc;
    status.textContent = !doc ? "" : doc.saveError ? doc.recoveryError ? "自动保存失败，请保持工程打开" : "自动保存失败，修改已保留" : doc.dirty || doc.saving ? "自动保存中…" : "已自动保存";
    status.classList.toggle("error", !!doc?.saveError);
    status.title = doc?.saveError || doc?.path || "";
  }
  function imageDocument() {
    const raw = D.current();
    if (raw?.type !== "image" || !S.info) return null;
    const source = { token: S.token, info: S.info };
    const doc = M.fromImage(source, S, stem(raw.path), D.scaleBarSnapshot(source, S));
    doc.id = raw.id;
    doc.layers[0].id = `source-${S.token}`;
    doc.selected = doc.layers[0].id;
    return doc;
  }
  function updateZoom(doc) {
    if (!doc?.view) return;
    const pct = `${Math.round(doc.view.scale * (window.devicePixelRatio || 1) * 100)}%`;
    $("btn-zoom").textContent = pct;
    $("st-zoom").textContent = pct;
  }
  function sync() {
    const doc = current();
    const shown = doc || imageDocument();
    syncSaveState();
    preview.hidden = !doc;
    zbars.hidden = !doc;
    $("merge-intro").hidden = !!shown;
    $("merge-editor").hidden = !shown;
    $("merge-empty-add").disabled = !S.info && !doc;
    syncProjects();
    // Keep the original image tools from applying to a hidden source tab.
    for (const button of document.querySelectorAll("#toolbar button, #channels input, #channels select, #series")) {
      if (["btn-open", "btn-save", "btn-settings", "btn-scalebar", "btn-zoom", "btn-zoomin", "btn-zoomout", "btn-undo", "btn-redo", "scroll-left", "scroll-right"].includes(button.id)) continue;
      if (doc) { if (!busyButtons.has(button)) busyButtons.set(button, button.disabled); button.disabled = true; }
      else if (busyButtons.has(button)) { button.disabled = busyButtons.get(button); busyButtons.delete(button); }
    }
    if (!shown) { zkey = ""; return; }
    $("btn-scalebar").disabled = !!doc && !doc.layers[0]?.scaleBar;
    if (doc) $("btn-scalebar").classList.toggle("on", !!doc.layers[0]?.scaleBar?.visible);
    if (doc) {
      $("st-info").textContent = `合成 · ${doc.width}×${doc.height} · ${doc.layers.length} 个图层`;
      $("st-pixel").textContent = "";
      $("st-cache").textContent = "";
      $("btn-save").disabled = false;
      $("strip").querySelectorAll(".thumb.cur").forEach((el) => el.classList.remove("cur"));
    }
    const list = $("merge-list");
    list.replaceChildren();
    [...shown.layers].reverse().forEach((l) => {
      const row = document.createElement("div");
      row.className = `merge-layer-row${l.id === shown.selected ? " selected" : ""}`;
      row.dataset.layer = l.id;
      row.setAttribute("role", "listitem");
      const visible = document.createElement("button");
      visible.className = "merge-eye";
      visible.setAttribute("aria-pressed", String(l.visible));
      visible.setAttribute("aria-label", `显示 ${l.name}`);
      visible.append($("btn-annshow").querySelector("svg").cloneNode(true));
      visible.classList.toggle("off", !l.visible);
      visible.onclick = () => edit((target, activeLayer) => { const item = target.layers.find((item) => item.id === l.id) || activeLayer; if (item) item.visible = !item.visible; });
      const pick = document.createElement("button");
      pick.className = "merge-layer-pick";
      pick.setAttribute("aria-pressed", String(l.id === shown.selected));
      const thumb = document.createElement("canvas");
      thumb.className = "merge-layer-thumb"; thumb.width = 40; thumb.height = 32;
      thumb.setAttribute("aria-hidden", "true");
      const name = document.createElement("span"); name.textContent = l.name;
      pick.append(thumb, name); pick.title = l.path;
      pick.onclick = () => { if (doc) { doc.selected = l.id; D.touch(doc); sync(); request(); } };
      pick.ondblclick = () => { $("merge-transform").open = true; $("merge-name").focus(); $("merge-name").select(); };
      row.append(visible, pick); list.append(row);
    });
    const l = selected(shown);
    $("merge-properties").hidden = !l;
    $("merge-transform").hidden = !l;
    $("merge-levels").hidden = !l;
    for (const id of ["merge-up", "merge-down", "merge-delete"]) $(id).disabled = !l;
    if (l) {
      const index = shown.layers.indexOf(l);
      $("merge-up").disabled = index === shown.layers.length - 1;
      $("merge-down").disabled = index === 0;
      syncValue("merge-name", l.name); syncValue("merge-mode", l.mode);
      const known = ["#ffffff", "#ff0000", "#00ff00", "#0000ff", "#ffff00", "#ff00ff", "#00ffff"];
      syncValue("merge-color", l.color ? known.includes(l.color) ? l.color : "custom" : "");
      syncValue("merge-custom-color", l.color || "#00ff00");
      $("merge-custom-color").hidden = !l.color;
      syncValue("merge-opacity", Math.round(l.opacity * 100));
      $("merge-opacity-value").textContent = `${Math.round(l.opacity * 100)}%`;
      for (const key of ["x", "y", "width", "height"]) syncValue(`merge-${key}`, +l[key].toFixed(3));
      $("merge-aspect").checked = l.lockAspect !== false;
      syncValue("merge-min", l.channels[0]?.min ?? ""); syncValue("merge-max", l.channels[0]?.max ?? "");
      $("merge-time-wrap").hidden = l.info.sizeT <= 1;
      $("merge-time").max = l.info.sizeT;
      syncValue("merge-time", l.t + 1);
      $("merge-source").textContent = `${l.info.width} × ${l.info.height} · ${l.info.sizeZ > 1 ? `Z ${l.z + 1}/${l.info.sizeZ}` : "单张（固定）"}`;
      $("merge-source").title = l.path;
    }
    syncValue("merge-canvas-width", shown.width); syncValue("merge-canvas-height", shown.height);
    syncValue("merge-background", shown.background);
    if (!doc) {
      contain(list.querySelector(".merge-layer-thumb"), S.ov ? ovCanvas : S.frame ? frameCanvas : null);
      zkey = "";
      return;
    }
    syncZBars(doc);
    if (!doc.view) fit(doc);
    updateZoom(doc);
    request();
  }

  function syncZBars(doc) {
    const stacks = doc.layers.filter((l) => l.info.sizeZ > 1);
    zbars.hidden = stacks.length === 0;
    const key = `${doc.id}|${stacks.map((l) => `${l.id}:${l.info.sizeZ}`).join("|")}`;
    if (key !== zkey) {
      zkey = key; zbars.replaceChildren();
      for (const l of stacks) {
        const row = document.createElement("label");
        row.className = "merge-zrow"; row.dataset.layer = l.id;
        const title = document.createElement("span"); title.className = "merge-zname";
        const input = document.createElement("input");
        input.type = "range"; input.min = 0; input.max = l.info.sizeZ - 1; input.step = 1;
        input.setAttribute("aria-label", `${l.name} Z`);
        input.disabled = l.info.sizeZ <= 1;
        input.onpointerdown = () => remember(doc);
        input.onkeydown = (e) => { if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) remember(doc); };
        input.oninput = () => edit(() => M.setZ(l, +input.value), false);
        const value = document.createElement("output");
        row.append(title, input, value); zbars.append(row);
      }
    }
    for (const row of zbars.children) {
      const l = doc.layers.find((l) => l.id === row.dataset.layer);
      row.classList.toggle("selected", l.id === doc.selected);
      row.firstChild.textContent = l.name;
      row.firstChild.title = l.name;
      row.querySelector("input").value = l.z;
      row.lastChild.textContent = l.info.sizeZ <= 1 ? "单张 · 固定" : `${l.z + 1} / ${l.info.sizeZ}`;
    }
  }

  function fit(doc = current()) {
    if (!doc || !doc.width || !doc.height) return;
    const rect = stage.getBoundingClientRect();
    doc.view = { cx: doc.width / 2, cy: doc.height / 2, scale: Math.max(0.001, Math.min((rect.width - 70) / doc.width, (rect.height - 60) / doc.height)) };
    request();
  }

  async function pixels(l, scale, full) {
    const factor = Math.min(1, l.width * scale / l.info.width, l.height * scale / l.info.height);
    const limit = full ? 16384 : 3072;
    const k = Math.min(factor, limit / Math.max(l.info.width, l.info.height));
    const outW = Math.max(1, Math.round(l.info.width * k)), outH = Math.max(1, Math.round(l.info.height * k));
    const req = { token: l.token, z: l.z, t: l.t, channels: l.channels,
      region: [0, 0, l.info.width, l.info.height], outW, outH, gamma: l.gamma, invert: false };
    const key = JSON.stringify([req, l.color]);
    if (cache.get(l.id)?.key === key) return cache.get(l.id).canvas;
    const frame = parseFrame(await invoke("render_view", { req }));
    if (l.color) {
      const rgb = [1, 3, 5].map((i) => parseInt(l.color.slice(i, i + 2), 16) / 255);
      const values = frame.image.data;
      for (let i = 0; i < values.length; i += 4) {
        const intensity = Math.max(values[i], values[i + 1], values[i + 2]);
        for (let c = 0; c < 3; c++) values[i + c] = Math.round(intensity * rgb[c]);
      }
    }
    const image = document.createElement("canvas"); image.width = frame.w; image.height = frame.h;
    image.getContext("2d").putImageData(frame.image, 0, 0);
    if (!full) cache.set(l.id, { key, canvas: image });
    return image;
  }

  function paintScaleBar(doc, g, scale) {
    const bar = M.scaleBarGeometry(doc.layers[0]);
    if (!bar) return;
    g.save();
    g.globalCompositeOperation = "source-over"; g.globalAlpha = 1;
    g.fillStyle = "#fff"; g.shadowColor = "#000"; g.shadowBlur = Math.max(1, bar.font * scale * 0.23);
    g.fillRect(bar.x * scale, bar.y * scale, bar.width * scale, bar.height * scale);
    g.font = `600 ${bar.font * scale}px -apple-system, "Segoe UI", sans-serif`;
    g.textAlign = "center"; g.textBaseline = "bottom";
    g.fillText(bar.label, (bar.x + bar.width / 2) * scale, (bar.y - bar.font * 0.3) * scale);
    g.restore();
  }

  async function paint(doc, output, origin, scale, full = false) {
    const g = output.getContext("2d");
    g.clearRect(0, 0, output.width, output.height);
    g.save();
    g.translate(origin[0], origin[1]);
    g.beginPath(); g.rect(0, 0, doc.width * scale, doc.height * scale); g.clip();
    g.fillStyle = doc.background; g.fillRect(0, 0, doc.width * scale, doc.height * scale);
    g.imageSmoothingEnabled = true;
    try {
      for (const l of doc.layers) {
        if (!l.visible || l.opacity <= 0) continue;
        const image = await pixels(l, scale, full);
        g.globalCompositeOperation = M.operation(l.mode);
        g.globalAlpha = l.opacity;
        g.drawImage(image, l.x * scale, l.y * scale, l.width * scale, l.height * scale);
      }
      // Keep the base image's calibrated bar neutral and above the merged
      // channels. Its geometry is attached to that layer, not the viewport.
      paintScaleBar(doc, g, scale);
    } finally { g.restore(); }
  }

  function request() {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => { scheduled = false; render(); });
  }
  async function render() {
    const doc = current();
    if (!doc || !doc.view) return;
    if (rendering) { again = true; return; }
    rendering = true;
    const revision = doc.revision, view = { ...doc.view };
    const dpr = window.devicePixelRatio || 1;
    const rect = stage.getBoundingClientRect();
    const output = document.createElement("canvas");
    output.width = Math.max(1, Math.round(rect.width * dpr)); output.height = Math.max(1, Math.round(rect.height * dpr));
    const scale = view.scale * dpr;
    const origin = [output.width / 2 - view.cx * scale, output.height / 2 - view.cy * scale];
    try {
      await paint(doc, output, origin, scale);
      if (current() !== doc || doc.revision !== revision || JSON.stringify(doc.view) !== JSON.stringify(view)) { again = true; return; }
      const l = selected(doc);
      if (l?.visible && sideMode === "layers" && !$("sidebar").classList.contains("collapsed")) {
        const g = output.getContext("2d");
        const x = origin[0] + l.x * scale, y = origin[1] + l.y * scale;
        const w = l.width * scale, h = l.height * scale;
        g.strokeStyle = "#1686ff"; g.lineWidth = dpr; g.strokeRect(x, y, w, h);
        const size = 6 * dpr;
        g.fillStyle = "#ffffff";
        for (const [, hx, hy] of M.handles) {
          const left = x + hx * w - size / 2, top = y + hy * h - size / 2;
          g.fillRect(left, top, size, size); g.strokeRect(left, top, size, size);
        }
      }
      preview.width = output.width; preview.height = output.height;
      preview.getContext("2d").drawImage(output, 0, 0);
      updateZoom(doc);
      $("st-info").textContent = `合成 · ${doc.width}×${doc.height} · ${doc.layers.length} 个图层`;
      updateThumbnail(doc);
    } catch (e) { if (current() === doc) toast(`合成预览失败：${e}`, 6500); }
    finally { rendering = false; if (again) { again = false; request(); } }
  }

  async function exportImage(doc = current() || imageDocument(), extension = "tif") {
    if (!doc || exportBusy) return;
    exportBusy = true;
    const snapshot = { ...doc, layers: doc.layers.map((l) => ({ ...l, channels: M.copy(l.channels), scaleBar: l.scaleBar && M.copy(l.scaleBar) })) };
    const unpin = D.pin(snapshot.layers.map((l) => l.token));
    try {
      if (snapshot.width > 16384 || snapshot.height > 16384 || snapshot.width * snapshot.height > 100000000) throw new Error("导出画布最大边长为 16384，且不超过 1 亿像素；请先调整画布尺寸");
      const path = await invoke("pick_save_path", { dir: doc.path ? dirname(doc.path) : dirname(doc.layers[0]?.path || ""),
        name: `${doc.name}.${extension}`, filter: "合并图像", exts: [extension] });
      if (!path) return;
      // The export owns a snapshot even if the user switches tabs or edits again.
      const output = document.createElement("canvas"); output.width = snapshot.width; output.height = snapshot.height;
      await paint(snapshot, output, [0, 0], 1, true);
      const blob = await new Promise((resolve) => output.toBlob(resolve, "image/png"));
      if (!blob) throw new Error("无法生成导出图像");
      await invoke("save_merged_image", new Uint8Array(await blob.arrayBuffer()), { headers: { "x-path": encodeURIComponent(path) } });
      toast(`已导出：${basename(path)}`);
    } catch (e) { toast(`导出失败：${e}`, 6500); }
    finally { exportBusy = false; unpin(); }
  }

  const addLayer = async () => { const path = await invoke("pick_file"); if (path) D.addPaths([path]); };
  $("merge-add").onclick = addLayer;
  $("merge-empty-add").onclick = addLayer;
  $("merge-export").onclick = () => {
    if (closeMenu($("merge-export"))) return;
    const doc = current() || imageDocument();
    openMenu($("merge-export"), [["tif", "TIFF"], ["png", "PNG"], ["jpg", "JPEG"], ["bmp", "BMP"], ["webp", "WebP"]].map(([value, label]) => ({ value, label })),
      (format) => exportImage(doc, format));
    const menu = $("dropmenu"), r = $("merge-export").getBoundingClientRect();
    if (r.bottom + 6 + menu.offsetHeight > window.innerHeight - 8) menu.style.top = `${Math.max(8, r.top - menu.offsetHeight - 6)}px`;
  };
  $("merge-more").onclick = () => {
    if (closeMenu($("merge-more"))) return;
    openMenu($("merge-more"), [
      { value: "open", label: "打开合成工程…" },
    ], async (action) => {
      const path = await invoke("pick_merge_project"); if (path) D.open(path);
    });
  };
  for (const [id, key] of [["merge-name", "name"], ["merge-mode", "mode"]]) $(id).onchange = (e) => {
    const value = e.target.value; edit((_doc, l) => { if (l) l[key] = value; });
  };
  $("merge-color").onchange = (e) => {
    const value = e.target.value === "custom" ? $("merge-custom-color").value : e.target.value || null;
    edit((_doc, l) => { if (l) l.color = value; });
  };
  $("merge-custom-color").onchange = (e) => { const value = e.target.value; edit((_doc, l) => { if (l) l.color = value; }); };
  $("merge-opacity").onpointerdown = () => current() && remember(current());
  $("merge-opacity").onkeydown = () => current() && remember(current());
  $("merge-opacity").oninput = (e) => { const value = +e.target.value / 100; edit((_doc, l) => { if (l) l.opacity = value; }, false); };
  $("merge-aspect").onchange = (e) => { const value = e.target.checked; edit((_doc, l) => { if (l) l.lockAspect = value; }); };
  for (const key of ["x", "y", "width", "height"]) $(`merge-${key}`).onchange = (e) => {
    const value = +e.target.value;
    if (!Number.isFinite(value) || Math.abs(value) > 1000000 || (["width", "height"].includes(key) && value <= 0)) return sync();
    edit((_doc, l) => {
      if (!l) return;
      if (l.lockAspect !== false && key === "width") l.height *= value / l.width;
      if (l.lockAspect !== false && key === "height") l.width *= value / l.height;
      l[key] = value;
    });
  };
  for (const [id, key] of [["merge-min", "min"], ["merge-max", "max"]]) $(id).onchange = (e) => {
    const value = e.target.value === "" ? null : Number(e.target.value);
    if (value !== null && !Number.isFinite(value)) return sync();
    edit((_doc, l) => { l?.channels.forEach((c) => { c[key] = value; }); });
  };
  $("merge-auto").onclick = () => edit((_doc, l) => l?.channels.forEach((c) => { c.min = c.max = null; }));
  $("merge-time").onchange = (e) => { const value = Math.round(+e.target.value - 1); edit((_doc, l) => { if (l) l.t = M.clamp(value, 0, l.info.sizeT - 1); }); };
  for (const key of ["width", "height"]) $(`merge-canvas-${key}`).onchange = (e) => {
    const value = +e.target.value;
    if (Number.isInteger(value) && value > 0 && value <= 1000000) edit((doc) => { doc[key] = value; });
    else sync();
  };
  $("merge-background").onchange = (e) => { const value = e.target.value; edit((doc) => { doc.background = value; }); };
  for (const [id, delta] of [["merge-up", 1], ["merge-down", -1]]) $(id).onclick = () => edit((doc, l) => {
    if (!l) return;
    const i = doc.layers.indexOf(l), j = M.clamp(i + delta, 0, doc.layers.length - 1);
    [doc.layers[i], doc.layers[j]] = [doc.layers[j], doc.layers[i]];
  });
  const deleteLayer = () => edit((doc, l) => {
    if (!l) return;
    doc.layers.splice(doc.layers.indexOf(l), 1);
    doc.selected = doc.layers.at(-1)?.id;
    // Undo may need this source; all handles are released when its tab closes.
  });
  $("merge-delete").onclick = deleteLayer;

  function point(e, doc) {
    const rect = preview.getBoundingClientRect(), v = doc.view;
    return [(e.clientX - rect.left - rect.width / 2) / v.scale + v.cx, (e.clientY - rect.top - rect.height / 2) / v.scale + v.cy];
  }
  function handleAt(l, p, scale) {
    if (!l?.visible) return null;
    return M.handles.find(([, x, y]) => Math.hypot(p[0] - l.x - x * l.width, p[1] - l.y - y * l.height) * scale <= 9)?.[0] || null;
  }
  const handleCursor = (handle) => ({ n: "ns-resize", s: "ns-resize", e: "ew-resize", w: "ew-resize",
    nw: "nwse-resize", se: "nwse-resize", ne: "nesw-resize", sw: "nesw-resize" })[handle] || "move";
  preview.onpointerdown = (e) => {
    const doc = current(); if (!doc?.view || e.button > 1) return;
    preview.focus({ preventScroll: true });
    const p = point(e, doc), l = selected(doc);
    const resize = handleAt(l, p, doc.view.scale);
    const contains = (l) => l.visible && p[0] >= l.x && p[0] <= l.x + l.width && p[1] >= l.y && p[1] <= l.y + l.height;
    const hit = resize || l && contains(l) ? l : [...doc.layers].reverse().find(contains);
    const pan = e.button === 1 || e.altKey || !hit || sideMode !== "layers" || $("sidebar").classList.contains("collapsed");
    if (!pan) {
      if (doc.selected !== hit.id) { doc.selected = hit.id; D.touch(doc); }
      remember(doc);
    }
    drag = { doc, start: p, client: [e.clientX, e.clientY], pan, resize, layer: hit, view: { ...doc.view }, original: hit ? { x: hit.x, y: hit.y, width: hit.width, height: hit.height } : null, moved: false };
    preview.setPointerCapture(e.pointerId);
    e.preventDefault(); sync();
  };
  preview.onpointermove = (e) => {
    if (!drag) {
      const doc = current();
      preview.style.cursor = doc?.view && sideMode === "layers" && !$("sidebar").classList.contains("collapsed")
        ? handleCursor(handleAt(selected(doc), point(e, doc), doc.view.scale)) : "grab";
      return;
    }
    if (current() !== drag.doc) return;
    const d = drag, v = d.doc.view;
    const dx = (e.clientX - d.client[0]) / v.scale, dy = (e.clientY - d.client[1]) / v.scale;
    d.moved ||= Math.hypot(dx, dy) > 0;
    if (d.pan) { v.cx = d.view.cx - dx; v.cy = d.view.cy - dy; D.touch(d.doc); request(); return; }
    if (d.resize) {
      Object.assign(d.layer, M.resize(d.original, d.resize, dx, dy, (d.layer.lockAspect !== false) !== e.shiftKey));
    } else { d.layer.x = d.original.x + dx; d.layer.y = d.original.y + dy; }
    edit(() => {}, false);
  };
  const endDrag = () => {
    if (drag && !drag.moved && !drag.pan) drag.doc.history?.pop();
    if (drag?.moved && current() === drag.doc) D.touch(drag.doc);
    drag = null;
  };
  preview.onpointerup = endDrag; preview.onpointercancel = endDrag;
  preview.oncontextmenu = (e) => e.preventDefault();
  preview.onmousedown = (e) => { if (e.button === 1) e.preventDefault(); };
  preview.ondblclick = () => { fit(); D.touch(); };
  function zoom(factor, e) {
    const doc = current(); if (!doc?.view) return;
    const p = e ? point(e, doc) : null;
    doc.view.scale = M.clamp(doc.view.scale * factor, 0.001, 100);
    if (p) { const q = point(e, doc); doc.view.cx += p[0] - q[0]; doc.view.cy += p[1] - q[1]; }
    D.touch(doc);
    request();
  }
  preview.addEventListener("wheel", (e) => { e.preventDefault(); zoom(Math.exp(-e.deltaY * 0.002), e); }, { passive: false });
  for (const [id, action] of [["btn-save", () => $("merge-export").click()], ["btn-scalebar", () => edit((doc) => { const bar = doc.layers[0]?.scaleBar; if (bar) bar.visible = !bar.visible; })], ["btn-undo", () => undo()], ["btn-redo", () => undo(true)], ["btn-zoom", () => { fit(); D.touch(); }], ["btn-zoomin", () => zoom(1.25)], ["btn-zoomout", () => zoom(0.8)]]) $(id).addEventListener("click", (e) => {
    if (!current()) return;
    e.preventDefault(); e.stopImmediatePropagation(); action();
  }, true);
  $("toolbar").addEventListener("click", (e) => {
    if (!current()) return;
    const button = e.target.closest("button");
    if (button && ["btn-open", "btn-save", "btn-settings", "btn-scalebar", "btn-undo", "btn-redo", "btn-zoom", "btn-zoomin", "btn-zoomout", "scroll-left", "scroll-right"].includes(button.id)) return;
    e.preventDefault(); e.stopImmediatePropagation();
  }, true);
  for (const pane of [imagesPane, panel]) pane.addEventListener("keydown", (e) => {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === "o") return;
    if (!mod && ["Delete", "Backspace"].includes(e.key) && !/^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) && e.target.closest("#merge-list")) {
      e.preventDefault(); e.stopPropagation(); deleteLayer(); return;
    }
    if (!mod) e.stopPropagation();
  });

  stage.addEventListener("dragover", (e) => {
    if (!e.dataTransfer.types.includes("application/x-tmv-image")) return;
    e.preventDefault(); e.dataTransfer.dropEffect = "copy"; stage.classList.add("merge-drop-target");
  });
  stage.addEventListener("dragleave", (e) => { if (!stage.contains(e.relatedTarget)) stage.classList.remove("merge-drop-target"); });
  stage.addEventListener("drop", (e) => {
    const path = e.dataTransfer.getData("application/x-tmv-image"); if (!path) return;
    e.preventDefault(); e.stopPropagation(); stage.classList.remove("merge-drop-target");
    D.addPaths([path]);
  });
  // Keep native OS-file drops enabled. WebView2 replaces HTML5 drop handlers
  // when native drops are on, so thumbnail drags use pointer events instead.
  let fileDrag = null;
  const ghost = document.createElement("div");
  ghost.id = "merge-file-drag"; ghost.hidden = true; document.body.append(ghost);
  $("strip").addEventListener("pointerdown", (e) => {
    const item = e.target.closest(".thumb[data-path]");
    if (!item || e.button !== 0) return;
    fileDrag = { path: item.dataset.path, x: e.clientX, y: e.clientY, moving: false };
  });
  $("strip").addEventListener("dragstart", (e) => e.preventDefault());
  const insideStage = (e) => {
    const r = stage.getBoundingClientRect();
    return e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom;
  };
  document.addEventListener("pointermove", (e) => {
    if (!fileDrag) return;
    if (!fileDrag.moving && Math.hypot(e.clientX - fileDrag.x, e.clientY - fileDrag.y) > 7) {
      fileDrag.moving = true;
      $("strip").setPointerCapture(e.pointerId);
    }
    if (!fileDrag.moving) return;
    ghost.hidden = false; ghost.textContent = `＋ ${basename(fileDrag.path)}`;
    ghost.style.left = `${e.clientX + 12}px`; ghost.style.top = `${e.clientY + 12}px`;
    stage.classList.toggle("merge-drop-target", insideStage(e));
    e.preventDefault();
  });
  document.addEventListener("pointerup", (e) => {
    const drop = fileDrag; fileDrag = null; ghost.hidden = true;
    stage.classList.remove("merge-drop-target");
    if (!drop?.moving) return;
    e.preventDefault();
    // Suppress only the click generated by this completed drag.
    const block = (click) => { click.preventDefault(); click.stopImmediatePropagation(); window.removeEventListener("click", block, true); };
    window.addEventListener("click", block, true);
    setTimeout(() => window.removeEventListener("click", block, true), 0);
    if (insideStage(e)) D.addPaths([drop.path]);
  });
  $("strip").addEventListener("pointercancel", () => { fileDrag = null; ghost.hidden = true; stage.classList.remove("merge-drop-target"); });
  document.addEventListener("tmv-document", () => {
    const live = new Set(D.tabs.flatMap((d) => d.type === "merge" ? d.layers.map((l) => l.id) : []));
    for (const key of cache.keys()) if (!live.has(key)) cache.delete(key);
    sync();
  });
  document.addEventListener("tmv-save", () => { syncSaveState(); syncProjects(); });
  new ResizeObserver(request).observe(stage);
  window.TifMrcViewPro.layers = { fit, exportImage, paint, sync, request, setPanel,
    sourceChanged() {
      if (current() || sideMode !== "layers" || sourceScheduled) return;
      sourceScheduled = true;
      requestAnimationFrame(() => {
        sourceScheduled = false;
        if (current()) return;
        const row = $("merge-list").querySelector(`[data-layer="source-${S.token}"]`);
        // A late source frame must not steal keyboard focus from its layer row.
        if (row?.contains(document.activeElement)) contain(row.querySelector(".merge-layer-thumb"), S.ov ? ovCanvas : S.frame ? frameCanvas : null);
        else sync();
      });
    },
    toggleSidebar(force) { setPanel(sideMode, force ?? $("sidebar").classList.contains("collapsed")); },
    key(e) {
    const doc = current(); if (!doc) return;
    const key = e.key.toLowerCase(), mod = e.metaKey || e.ctrlKey;
    if (mod && (key === "z" || key === "y")) undo(key === "y" || e.shiftKey);
    else if (!mod && (key === "delete" || key === "backspace")) deleteLayer();
    else if (key === "f") { fit(); D.touch(doc); }
    else if (key === "+" || key === "=") zoom(1.25);
    else if (key === "-") zoom(0.8);
    else if (key === "1") { doc.view.scale = 1 / (window.devicePixelRatio || 1); D.touch(doc); request(); }
    else if (key === "b") toggleSidebar();
    else if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(e.key)) edit((_doc, l) => {
      if (!l) return; const step = e.shiftKey ? 10 : 1;
      if (e.key === "ArrowLeft") l.x -= step;
      if (e.key === "ArrowRight") l.x += step;
      if (e.key === "ArrowUp") l.y -= step;
      if (e.key === "ArrowDown") l.y += step;
    });
    else return;
    e.preventDefault();
  } };
  setPanel(sideMode, !$("sidebar").classList.contains("collapsed"));
  sync();
})();
