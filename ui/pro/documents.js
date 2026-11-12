"use strict";
// Pro document ownership: source tabs and merge projects share one window.
(() => {
  const tabs = [], sources = new Map(), pins = new Map();
  const saveTimers = new Map(), saveTails = new WeakMap();
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

  function scaleBarSnapshot(source, state = null) {
    const info = source.info, ps = info.pixelSize, unit = normUnit(info.unit || "");
    const px = state && "px" in state ? state.px
      : ps?.[0] > 0 && UNIT_M[unit] && plausiblePixel(ps[0], unit, true) ? { x: ps[0], y: ps[1] > 0 ? ps[1] : ps[0], unit } : null;
    const pxM = px?.x * UNIT_M[px?.unit];
    if (!Number.isFinite(pxM) || pxM <= 0) return null;
    const scale = Math.max(0.001, state?.view?.scale || 1);
    const region = state?.crop || [0, 0, info.width, info.height];
    const length = niceLength(Math.min(120 / scale, region[2] * 0.4) * pxM);
    const labelUnit = isRecip(px.unit) ? px.unit : px.unit === "Å" && length < 1e-8 ? "Å" : length < 1e-6 ? "nm" : length < 1e-3 ? "µm" : "mm";
    return { visible: !!(state?.scaleBar ?? pref.get("scalebar-on", true)),
      pixelSize: { x: px.x, y: px.y > 0 ? px.y : px.x, unit: px.unit },
      length: length / pxM, label: fmtLength(length, labelUnit), region: [...region],
      font: 13 / scale, thickness: 4 / scale, margin: 18 / scale };
  }

  function changed(schedule = true) {
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
    if (schedule) for (const doc of tabs) if (doc.type === "merge" && doc.dirty) autosave(doc);
  }

  async function capture() {
    if (active?.type === "merge") return save(active);
    if (active?.type !== "image") return true;
    finishText(true);
    await flushAnn();
    stopPlay();
    if (S.xyz) xyz.close(true);
    if (S.axis !== "xy") await setAxis("xy");
    active.state = { ...S, view: { ...S.view }, chans: S.chans.map((c) => ({ ...c, color: [...c.color] })),
      folder: { ...S.folder }, frame: null, ov: null, fade: null, hist: null,
      op: null, picked: null, cropBox: null, inflight: false, dirty: false,
      playing: null, scrubbing: false, scrubTimer: 0, ovBusy: false, ovKey: null, loadTimer: null };
    return true;
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
    if (!await capture()) return active;
    doc.used = Date.now();
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

  // `opts.replace` shows the image in the active image tab instead of a new one.
  async function open(path, force = false, series = 0, opts = null) {
    return enqueue(async () => {
      if (/\.tmvmerge$/i.test(path)) return openProject(path);
      const existing = tabs.find((d) => d.type === "image" && d.path === path && d.series === series);
      const here = TmvTabRules.replacesActive(active, opts) && (!existing || existing === active);
      if (existing && !here && !force) return activate(existing);
      if (existing && here && !force) return existing;
      if (!here && !existing && !await makeRoom()) return null;
      if (!await capture()) return null;
      const previous = active;
      const result = await openPath(path, force || previous?.type !== "image", series, true);
      if (!result) return null;
      if (here) {
        // Same tab, new image: the old backend document goes when nothing else uses it.
        Object.assign(previous, { path, series, token: result.token, name: result.info.name, used: Date.now() });
        sources.set(result.token, result);
        await capture();
        await releaseUnused();
        changed();
        return previous;
      }
      const doc = { id: `document-${++nextId}`, type: "image", path, series, token: result.token,
        name: result.info.name, state: null, dirty: false, used: Date.now() };
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

  // At most MAX_TABS tabs: close the stalest idle image tab for a new one.
  async function makeRoom() {
    if (tabs.length < TmvTabRules.MAX_TABS) return true;
    const victim = TmvTabRules.evictionCandidate(tabs, active);
    if (!victim) {
      toast(`最多同时打开 ${TmvTabRules.MAX_TABS} 个标签页，请先关掉几个`, 5000);
      return false;
    }
    tabs.splice(tabs.indexOf(victim), 1);
    await releaseUnused();
    changed();
    return true;
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
        if (!d.path && d.autosaveToken) used.add(d.autosaveToken);
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
      if (!await capture()) return null;
      const previous = active;
      const convert = previous?.type === "image" && !startNew;
      const opened = [];
      // Decode first: a failed drop leaves the current document and view intact.
      try {
        for (const path of paths) {
          opened.push(await source(path));
        }
      } catch (e) { await releaseUnused(); throw e; }
      if (!await capture()) { await releaseUnused(); return null; }
      const before = $("stage").getBoundingClientRect();
      let doc;
      if (convert) {
        const base = sources.get(previous.token);
        doc = TmvMerge.fromImage(base, previous.state, `${stem(previous.path)}_merge`, scaleBarSnapshot(base, previous.state));
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
        added.push(TmvMerge.add(doc, TmvMerge.layer(image, { ...settings, scaleBar: scaleBarSnapshot(image, state) })));
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

  async function openProject(path, recovery = null) {
    const existing = tabs.find((d) => d.type === "merge" && (recovery && d.autosaveId === recovery.id || path && d.path === path));
    if (existing) return activate(existing);
    if (!recovery) {
      const draft = recoveryList().find((r) => r.path === path);
      if (draft) {
        // Opening a saved file also prefers its newer interrupted edit, so two
        // tabs cannot race to save an old and a recovered copy to one path.
        try { await invoke("read_merge_project", { path }); } catch (_) { path = null; }
        recovery = draft;
      }
    }
    const data = TmvMerge.validate(recovery?.project || await invoke("read_merge_project", { path }));
    if (!await capture()) return null;
    const doc = TmvMerge.create(data.name || stem(path));
    let migratedScale = false;
    doc.id = `document-${++nextId}`;
    try {
      for (const settings of data.layers) {
        let opened;
        try { opened = await source(settings.path, settings.series); }
        catch (e) { throw new Error(`无法打开工程图层「${settings.name}」：${settings.path}\n${e}。请恢复原图后重新打开工程。`); }
        if (settings.channels.some((c) => c.index >= opened.info.channels.length)) throw new Error(`图层「${settings.name}」的原图通道已变化`);
        if (settings.z >= opened.info.sizeZ || settings.t >= opened.info.sizeT) throw new Error(`图层「${settings.name}」的原图 Z/T 尺寸已变化`);
        const layer = TmvMerge.layer(opened, settings);
        if (!Object.hasOwn(settings, "scaleBar")) {
          layer.scaleBar = scaleBarSnapshot(opened, { view: { scale: (data.view?.scale || 1) * layer.width / opened.info.width } });
          migratedScale ||= !!layer.scaleBar;
        }
        doc.layers.push(layer);
      }
    } catch (e) { await releaseUnused(); throw e; }
    if (recovery?.anchor && !doc.layers.length) {
      const anchor = await source(recovery.anchor.path, recovery.anchor.series || 0);
      doc.autosaveToken = anchor.token;
      doc.autosaveSource = recovery.anchor;
    }
    Object.assign(doc, { width: data.width, height: data.height, background: data.background,
      path, dirty: !!recovery || migratedScale, selected: doc.layers[data.selected]?.id || doc.layers.at(-1)?.id,
      view: data.view || null });
    if (recovery) doc.autosaveId = recovery.id;
    tabs.push(doc);
    await activate(doc);
    rememberProject(doc);
    return doc;
  }

  function recoveryList() {
    const rows = pref.get("merge-recovery", []);
    return Array.isArray(rows) ? rows : [];
  }

  function keepRecovery(doc) {
    doc.autosaveId ||= `merge-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
    doc.autosaveToken ||= doc.layers[0]?.token;
    if (!doc.autosaveSource && doc.layers[0]) doc.autosaveSource = { path: doc.layers[0].path, series: doc.layers[0].series };
    const row = { id: doc.autosaveId, name: doc.name, path: doc.path,
      dir: dirname(doc.layers[0]?.path || doc.autosaveSource?.path || doc.path || ""), anchor: doc.autosaveSource, project: TmvMerge.project(doc) };
    try { pref.set("merge-recovery", [row, ...recoveryList().filter((r) => r.id !== row.id)]); }
    catch (e) { doc.recoveryError = String(e); }
    const saved = recoveryList().find((r) => r.id === row.id);
    if (!saved || JSON.stringify(saved.project) !== JSON.stringify(row.project) || saved.path !== row.path) doc.recoveryError = "本地恢复副本未能写入";
    else doc.recoveryError = null;
  }

  function clearRecovery(doc) {
    try { pref.set("merge-recovery", recoveryList().filter((r) => r.id !== doc.autosaveId)); }
    catch (_) { /* A redundant recovery copy is safe to retain. */ }
  }

  function autosave(doc = active) {
    if (doc?.type !== "merge") return false;
    keepRecovery(doc);
    clearTimeout(saveTimers.get(doc));
    saveTimers.set(doc, setTimeout(() => { saveTimers.delete(doc); save(doc); }, 350));
    document.dispatchEvent(new CustomEvent("tmv-save", { detail: doc }));
    return true;
  }

  function touch(doc = active) {
    if (doc?.type !== "merge") return;
    doc.dirty = true; doc.revision++;
    autosave(doc);
  }

  async function save(doc = active) {
    if (doc?.type !== "merge") return false;
    clearTimeout(saveTimers.get(doc)); saveTimers.delete(doc);
    const task = (saveTails.get(doc) || Promise.resolve()).catch(() => {}).then(async () => {
      if (!doc.dirty) return true;
      keepRecovery(doc);
      doc.saving = true; doc.saveError = null;
      document.dispatchEvent(new CustomEvent("tmv-save", { detail: doc }));
      try {
        // Serialize writes per document. Edits arriving during a write stay
        // dirty and are flushed in the next iteration, never marked saved early.
        while (doc.dirty) {
          const revision = doc.revision, project = TmvMerge.project(doc);
          if (doc.path) await invoke("save_merge_project", { path: doc.path, project });
          else {
            doc.path = await invoke("create_merge_project", { token: doc.autosaveToken, project });
            // A collision-safe allocation may append a number to the name.
            if (doc.path && doc.name !== stem(doc.path)) { doc.name = stem(doc.path); doc.revision++; }
            if (doc === active) T.window.getCurrentWindow().setTitle(`${doc.name}.tmvmerge — ${EDITION.name}`).catch(() => {});
          }
          if (!doc.path || !/\.tmvmerge$/i.test(doc.path)) throw new Error("自动保存未返回工程路径");
          keepRecovery(doc);
          if (doc.revision === revision) doc.dirty = false;
          rememberProject(doc);
        }
        clearRecovery(doc);
        doc.recoveryError = null;
        return true;
      } catch (e) {
        doc.saveError = String(e);
        keepRecovery(doc);
        return false;
      } finally {
        doc.saving = false;
        changed(false);
        document.dispatchEvent(new CustomEvent("tmv-save", { detail: doc }));
      }
    });
    saveTails.set(doc, task);
    return task;
  }

  function recover(id) {
    return enqueue(async () => {
      const row = recoveryList().find((r) => r.id === id);
      if (!row) return null;
      let path = row.path;
      if (path) { try { await invoke("read_merge_project", { path }); } catch (_) { path = null; } }
      return openProject(path, row);
    });
  }

  function rememberProject(doc) {
    if (!doc.path) return;
    const list = pref.get("merge-projects", []);
    const origin = doc.layers[0]?.path || doc.autosaveSource?.path;
    const row = { path: doc.path, name: doc.name,
      dir: origin ? dirname(origin) : (Array.isArray(list) ? list.find((p) => p.path === doc.path)?.dir : null) || dirname(doc.path),
      preview: doc.thumbnail?.toDataURL("image/png") || doc.savedPreview || null };
    // The sidebar's recent-preview cache must never turn a successful project
    // save into a reported failure when local storage is full.
    try { pref.set("merge-projects", [row, ...(Array.isArray(list) ? list : []).filter((p) => p.path !== doc.path)].slice(0, 40)); } catch (_) {}
  }

  async function mayClose(doc) {
    if (doc.type !== "merge" || await save(doc)) return true;
    toast("自动保存失败，工程已保持打开。请检查保存位置后重试。", 6500);
    return false;
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
      e.preventDefault(); e.stopImmediatePropagation(); e.target.blur?.(); save(active);
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
    tabs, sources, open, source, addPaths, save, autosave, touch, recover, recoveryList, scaleBarSnapshot, changed, enqueue, releaseUnused, rememberProject,
    current: () => active, activate: (doc) => enqueue(() => activate(doc)), close: (doc) => enqueue(() => closeTab(doc)),
    pin(tokens) {
      tokens.forEach((t) => pins.set(t, (pins.get(t) || 0) + 1));
      return () => { tokens.forEach((t) => { if (pins.get(t) > 1) pins.set(t, pins.get(t) - 1); else pins.delete(t); }); enqueue(releaseUnused); };
    },
  };
  changed();
})();
