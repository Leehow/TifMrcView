"use strict";
// Pro document ownership: source tabs and merge projects share one window.
(() => {
  const tabs = [], sources = new Map(), pins = new Map();
  let active = null, nextId = 0, tail = Promise.resolve();
  const bar = document.createElement("nav");
  bar.id = "document-tabs";
  bar.setAttribute("aria-label", "打开的图像和合成工程");
  bar.setAttribute("role", "tablist");
  $("toolbar").after(bar);

  // Suspend source tools without their close/reset/cancel handlers: training,
  // model weights, results and the source view still belong to the image.
  const sourcePanels = ".pro-panel, .pro-workspace, #align-panel, #cryoet-workspace, #fft-panel, #spec-panel, #vol3d, #vol-panel, .mem-model";
  const sourceControls = "#stage > #view, #tools, #tools-resizer, .edge-rail, #stage > #size-out, #text-edit, #ann-menu, #loading, #zbar";
  const sourceSurfaces = `${sourcePanels}, ${sourceControls}`;
  const suspended = new Map();
  let sourceUiSuspended = false;
  function suspendSurface(el) {
    if (!suspended.has(el)) suspended.set(el, { hidden: el.getAttribute("hidden"), inert: el.inert });
    if (el.contains(document.activeElement)) document.activeElement.blur();
    if (el.matches(sourcePanels) && !el.hidden) el.hidden = true;
    el.inert = true;
    el.setAttribute("data-merge-suspended", "");
  }
  const sourceObserver = new MutationObserver((records) => {
    if (!sourceUiSuspended) return;
    for (const r of records) {
      if (r.type === "attributes" && r.target.matches(sourcePanels)) suspendSurface(r.target);
      for (const node of r.addedNodes || []) if (node.nodeType === 1) {
        if (node.matches(sourceSurfaces)) suspendSurface(node);
        node.querySelectorAll(sourceSurfaces).forEach(suspendSurface);
      }
    }
  });
  function syncSourceUi() {
    if (active?.type === "merge") {
      if (!sourceUiSuspended) {
        sourceUiSuspended = true;
        closeMenu();
        $("ann-menu").hidden = true;
        clearTimeout(S.loadTimer); S.loadTimer = null;
        sourceObserver.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["hidden"] });
      }
      document.querySelectorAll(sourceSurfaces).forEach(suspendSurface);
    } else if (sourceUiSuspended) {
      sourceObserver.disconnect();
      sourceUiSuspended = false;
      closeMenu();
      for (const [el, saved] of suspended) {
        el.inert = saved.inert;
        el.removeAttribute("data-merge-suspended");
        if (el.matches(sourcePanels)) {
          if (saved.hidden === null) el.removeAttribute("hidden");
          else el.setAttribute("hidden", saved.hidden);
        }
      }
      suspended.clear();
    }
  }
  // Inert covers real focus/pointer input; capture also rejects queued or
  // programmatic UI events before a hidden tool can export the source.
  for (const type of ["click", "dblclick", "pointerdown", "pointerup", "input", "change", "keydown", "wheel", "contextmenu"]) window.addEventListener(type, (e) => {
    if (active?.type !== "merge" || !e.target.closest?.(sourceSurfaces)) return;
    e.preventDefault(); e.stopImmediatePropagation();
  }, { capture: true, passive: false });
  document.addEventListener("pro-panel", (e) => {
    if (active?.type !== "merge") return;
    syncSourceUi();
    e.stopImmediatePropagation();
  }, true);

  function enqueue(work) {
    const result = tail.then(work).catch((e) => { toast(String(e), 6500); return null; });
    tail = result;
    return result;
  }

  function changed() {
    bar.replaceChildren();
    for (const doc of tabs) {
      const item = document.createElement("div");
      item.className = `document-tab${doc === active ? " selected" : ""}`;
      item.dataset.document = doc.id;
      const select = document.createElement("button");
      select.setAttribute("role", "tab");
      select.setAttribute("aria-selected", String(doc === active));
      const name = doc.type === "merge" && !/\.tmvmerge$/i.test(doc.name) ? `${doc.name}.tmvmerge` : doc.name;
      select.textContent = `${name}${doc.dirty ? " •" : ""}`;
      select.title = doc.path || doc.name;
      select.onclick = () => enqueue(() => activate(doc));
      const close = document.createElement("button");
      close.className = "document-close";
      close.textContent = "×";
      close.title = `关闭 ${doc.name}`;
      close.setAttribute("aria-label", close.title);
      close.onclick = () => enqueue(() => closeTab(doc));
      item.append(select, close);
      item.onauxclick = (e) => { if (e.button === 1) { e.preventDefault(); enqueue(() => closeTab(doc)); } };
      bar.append(item);
    }
    const add = document.createElement("button");
    add.id = "document-open";
    add.textContent = "+";
    add.title = "打开图像（新标签页）";
    add.onclick = openAny;
    bar.append(add);
    bar.querySelector(".selected")?.scrollIntoView({ inline: "nearest", block: "nearest" });
    document.body.classList.toggle("merge-mode", active?.type === "merge");
    syncSourceUi();
    document.dispatchEvent(new CustomEvent("tmv-document", { detail: active }));
  }

  async function capture() {
    if (active?.type !== "image") return;
    finishText(true);
    await flushAnn();
    stopPlay();
    if (S.xyz) xyz.close(true);
    if (S.axis !== "xy") await setAxis("xy");
    active.state = { ...S, view: { ...S.view }, chans: S.chans.map((c) => ({ ...c, color: [...c.color] })),
      folder: { ...S.folder }, frame: null, ov: null, fade: null, hist: null,
      op: null, picked: null, cropBox: null, inflight: false, dirty: false,
      playing: null, scrubbing: false, scrubTimer: 0, ovBusy: false, ovKey: null, loadTimer: null };
  }

  function restore(doc) {
    clearTimeout(S.scrubTimer);
    clearTimeout(S.loadTimer);
    Object.assign(S, doc.state);
    S.frame = null;
    S.ov = null;
    S.inflight = false;
    S.dirty = false;
    $("empty").hidden = true;
    $("loading").hidden = true;
    $("gamma").value = S.gamma;
    $("gamma-v").textContent = S.gamma.toFixed(2);
    $("st-pixel").textContent = "";
    buildChannels(); buildSeries(); setupSliders(); syncScaleUi(); updateInfo(); updateSaveButtons();
    setTool(S.tool);
    toggleAnn(S.annShow);
    $("mod-show").checked = S.modelShow;
    $("meta-list").dataset.file = "";
    renderMeta(); specOpened(); buildStrip(); markStrip();
    $("folder-name").textContent = basename(S.folder.dir || "");
    $("folder-name").title = S.folder.dir || "";
    setModel(S.model);
    clampView(); draw(); requestRender(); watchLoad();
    T.window.getCurrentWindow().setTitle(`${doc.name} — ${EDITION.name}`).catch(() => {});
    pluginEmit("open", S.info);
    document.dispatchEvent(new CustomEvent("tmv-opened"));
  }

  async function activate(doc) {
    if (doc === active) return doc;
    await capture();
    if (doc.type === "image") await invoke("activate_document", { token: doc.token });
    active = doc;
    changed();
    if (doc.type === "image") { restore(doc); changed(); }
    else {
      stopPlay();
      $("empty").hidden = true;
      $("zbar").hidden = true;
      T.window.getCurrentWindow().setTitle(`${doc.name.replace(/\.tmvmerge$/i, "")}.tmvmerge — ${EDITION.name}`).catch(() => {});
    }
    return doc;
  }

  async function open(path, force = false, series = 0) {
    return enqueue(async () => {
      if (/\.tmvmerge$/i.test(path)) return openProject(path);
      const existing = tabs.find((d) => d.type === "image" && d.path === path && d.series === series);
      if (existing && !force) return activate(existing);
      await capture();
      const previous = active;
      const result = await openPath(path, force || previous?.type !== "image", series, true);
      if (!result) return null;
      const doc = { id: `document-${++nextId}`, type: "image", path, series, token: result.token,
        name: result.info.name, state: null, dirty: false };
      sources.set(result.token, result);
      tabs.push(doc);
      active = doc;
      await capture();
      changed();
      // An explicit reload replaces this tab only; a merge may still use the old source.
      if (existing) {
        tabs.splice(tabs.indexOf(existing), 1);
        await releaseUnused();
        changed();
      }
      if (previous?.type === "merge") {
        setupSliders(); updateInfo(); draw(); requestRender(); watchLoad();
      }
      return doc;
    });
  }

  async function source(path, series = 0) {
    const tab = tabs.find((d) => d.type === "image" && d.path === path && d.series === series);
    if (tab && sources.has(tab.token)) return sources.get(tab.token);
    const found = [...sources.values()].find((s) => s.info.path === path && (s.info.seriesIndex || 0) === series);
    if (found) return found;
    const opened = await invoke("open_layer_source", { path, series });
    sources.set(opened.token, opened);
    return opened;
  }

  async function releaseUnused() {
    const used = new Set(pins.keys());
    for (const d of tabs) {
      if (d.type === "image") used.add(d.token);
      else {
        d.layers.forEach((l) => used.add(l.token));
        for (const h of [...(d.history || []), ...(d.future || [])]) h.layers.forEach((l) => used.add(l.token));
      }
    }
    for (const [token] of sources) if (!used.has(token)) {
      await invoke("close_document", { token });
      sources.delete(token);
    }
  }

  function newMerge() {
    const doc = TmvMerge.create(`合成图 ${tabs.filter((d) => d.type === "merge").length + 1}`);
    doc.id = `document-${++nextId}`;
    tabs.push(doc);
    return doc;
  }

  async function addPaths(paths, startNew = false) {
    return enqueue(async () => {
      await capture();
      const previous = active;
      const convert = previous?.type === "image" && !startNew;
      const opened = [];
      // Decode first: a failed drop leaves the current document and view intact.
      try {
        for (const path of paths) {
          opened.push(await source(path));
        }
      } catch (e) { await releaseUnused(); throw e; }
      await capture();
      const before = $("stage").getBoundingClientRect();
      let doc;
      if (convert) {
        const base = sources.get(previous.token);
        doc = TmvMerge.fromImage(base, previous.state, `${stem(previous.path)}_merge`);
        doc.id = previous.id;
        doc.folder = previous.state.folder;
      } else doc = !startNew && previous?.type === "merge" ? previous : newMerge();
      const added = [];
      for (const image of opened) {
        const imageTab = tabs.find((d) => d.type === "image" && d.token === image.token);
        const state = imageTab?.state;
        const settings = state ? { z: state.z, t: state.t, gamma: state.gamma,
          channels: state.chans.flatMap((c, index) => c.visible ? [{ index, color: c.color, min: c.auto ? null : c.min, max: c.auto ? null : c.max }] : []) } : {};
        if (settings.channels && !settings.channels.length) delete settings.channels;
        added.push(TmvMerge.add(doc, TmvMerge.layer(image, settings)));
      }
      if (convert) {
        tabs.splice(tabs.indexOf(previous), 1, doc);
        active = doc;
        stopPlay();
        $("zbar").hidden = true;
        changed();
        doc.view = TmvMerge.reframe(doc.view, before, $("stage").getBoundingClientRect());
        T.window.getCurrentWindow().setTitle(`${doc.name}.tmvmerge — ${EDITION.name}`).catch(() => {});
        window.TifMrcViewPro.layers?.request();
      } else if (doc !== active) await activate(doc);
      else {
        const view = doc.view && { ...doc.view };
        changed();
        if (view) doc.view = TmvMerge.reframe(view, before, $("stage").getBoundingClientRect());
        window.TifMrcViewPro.layers?.request();
      }
      if (!doc.view) window.TifMrcViewPro.layers?.fit(doc);
      return { doc, added };
    });
  }

  async function openProject(path) {
    const existing = tabs.find((d) => d.type === "merge" && d.path === path);
    if (existing) return activate(existing);
    const data = TmvMerge.validate(await invoke("read_merge_project", { path }));
    await capture();
    const doc = TmvMerge.create(data.name || stem(path));
    doc.id = `document-${++nextId}`;
    try {
      for (const settings of data.layers) {
        let opened;
        try { opened = await source(settings.path, settings.series); }
        catch (e) { throw new Error(`无法打开工程图层「${settings.name}」：${settings.path}\n${e}。请恢复原图后重新打开工程。`); }
        if (settings.channels.some((c) => c.index >= opened.info.channels.length)) throw new Error(`图层「${settings.name}」的原图通道已变化`);
        if (settings.z >= opened.info.sizeZ || settings.t >= opened.info.sizeT) throw new Error(`图层「${settings.name}」的原图 Z/T 尺寸已变化`);
        doc.layers.push(TmvMerge.layer(opened, settings));
      }
    } catch (e) { await releaseUnused(); throw e; }
    Object.assign(doc, { width: data.width, height: data.height, background: data.background,
      path, dirty: false, selected: doc.layers[data.selected]?.id || doc.layers.at(-1)?.id,
      view: data.view || null });
    tabs.push(doc);
    await activate(doc);
    rememberProject(doc);
    return doc;
  }

  async function save(doc = active, saveAs = false) {
    if (doc?.type !== "merge") return false;
    try {
      let path = doc.path;
      if (!path || saveAs) path = await invoke("pick_save_path", {
        dir: doc.path ? dirname(doc.path) : doc.layers[0] ? dirname(doc.layers[0].path) : "",
        name: `${doc.name.replace(/\.tmvmerge$/i, "")}.tmvmerge`, filter: "TifMrcView 合成工程", exts: ["tmvmerge"],
      });
      if (!path) return false;
      if (!/\.tmvmerge$/i.test(path)) throw new Error("合成工程请使用 .tmvmerge 扩展名");
      const revision = doc.revision;
      const project = TmvMerge.project(doc);
      project.name = stem(path);
      await invoke("save_merge_project", { path, project });
      doc.path = path;
      doc.name = project.name;
      if (doc.revision === revision) doc.dirty = false;
      rememberProject(doc);
      changed();
      if (doc === active) T.window.getCurrentWindow().setTitle(`${doc.name}.tmvmerge — ${EDITION.name}`).catch(() => {});
      toast(`已保存工程：${basename(path)}`);
      return !doc.dirty;
    } catch (e) { toast(`工程保存失败：${e}`, 6500); return false; }
  }

  function rememberProject(doc) {
    if (!doc.path) return;
    const list = pref.get("merge-projects", []);
    const row = { path: doc.path, name: doc.name,
      dir: dirname(doc.layers[0]?.path || doc.path),
      preview: doc.thumbnail?.toDataURL("image/png") || doc.savedPreview || null };
    // The sidebar's recent-preview cache must never turn a successful project
    // save into a reported failure when local storage is full.
    try { pref.set("merge-projects", [row, ...(Array.isArray(list) ? list : []).filter((p) => p.path !== doc.path)].slice(0, 40)); } catch (_) {}
  }

  function askSave(doc) {
    return new Promise((resolve) => {
      const dialog = document.createElement("dialog");
      dialog.className = "merge-close-dialog";
      const title = document.createElement("h3");
      title.textContent = `保存「${doc.name}」的修改？`;
      const body = document.createElement("p");
      body.textContent = "保存工程后，下次可以继续调整各个图层。";
      const row = document.createElement("div");
      for (const [text, action] of [["取消", "cancel"], ["不保存", "discard"], ["保存工程", "save"]]) {
        const button = document.createElement("button");
        button.textContent = text;
        if (action === "save") button.className = "primary";
        button.onclick = () => { dialog.close(); dialog.remove(); resolve(action); };
        row.append(button);
      }
      dialog.addEventListener("cancel", () => { dialog.remove(); resolve("cancel"); });
      dialog.append(title, body, row);
      document.body.append(dialog);
      dialog.showModal();
    });
  }

  async function mayClose(doc) {
    if (doc.type !== "merge" || !doc.dirty) return true;
    const action = await askSave(doc);
    return action === "discard" || action === "save" && await save(doc);
  }

  async function closeTab(doc) {
    if (!await mayClose(doc)) return;
    if (doc === active) await capture();
    const index = tabs.indexOf(doc);
    tabs.splice(index, 1);
    if (doc === active) {
      active = null;
      const next = tabs[Math.min(index, tabs.length - 1)];
      if (next) await activate(next);
      else {
        stopPlay();
        clearTimeout(S.loadTimer);
        Object.assign(S, { token: 0, info: null, frame: null, ov: null, ann: [], hist: null, model: null, base: null, axis: "xy", chans: [] });
        $("empty").hidden = false;
        $("zbar").hidden = true;
        buildChannels(); buildSeries(); updateInfo(); updateSaveButtons(); draw();
        T.window.getCurrentWindow().setTitle(EDITION.name).catch(() => {});
      }
    }
    await releaseUnused();
    changed();
  }

  window.addEventListener("keydown", (e) => {
    const mod = e.metaKey || e.ctrlKey;
    const input = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName);
    const sidebarControl = e.target.closest("#right-sidebar-tabs, #sidebar, #dropmenu");
    if (mod && e.key.toLowerCase() === "w" && active) {
      e.preventDefault(); e.stopImmediatePropagation(); enqueue(() => closeTab(active));
    } else if (mod && e.key === "Tab" && tabs.length) {
      e.preventDefault(); e.stopImmediatePropagation();
      const doc = tabs[(tabs.indexOf(active) + (e.shiftKey ? tabs.length - 1 : 1)) % tabs.length];
      enqueue(() => activate(doc));
    } else if (mod && e.key.toLowerCase() === "s" && active?.type === "merge") {
      e.preventDefault(); e.stopImmediatePropagation(); e.target.blur?.(); save(active, e.shiftKey);
    } else if (active?.type === "merge" && !input && !(mod && e.key.toLowerCase() === "o") && (mod || !sidebarControl)) {
      // Image tools and slice shortcuts must not edit the hidden source tab.
      e.stopImmediatePropagation();
      window.TifMrcViewPro.layers?.key(e);
    }
  }, true);

  let closing = false;
  T.window.getCurrentWindow().onCloseRequested(async (e) => {
    if (!tabs.some((d) => d.type === "merge" && d.dirty)) { await flushAnn(); return; }
    e.preventDefault();
    if (closing) return;
    closing = true;
    try {
      for (const doc of tabs) if (!await mayClose(doc)) return;
      await flushAnn();
      await invoke("close_pro_window");
    } finally { closing = false; }
  }).catch(() => {});

  window.TifMrcViewPro.documents = {
    tabs, sources, open, source, addPaths, save, changed, enqueue, releaseUnused, rememberProject,
    current: () => active, activate: (doc) => enqueue(() => activate(doc)), close: (doc) => enqueue(() => closeTab(doc)),
    pin(tokens) {
      tokens.forEach((t) => pins.set(t, (pins.get(t) || 0) + 1));
      return () => { tokens.forEach((t) => { if (pins.get(t) > 1) pins.set(t, pins.get(t) - 1); else pins.delete(t); }); enqueue(releaseUnused); };
    },
  };
  changed();
})();
