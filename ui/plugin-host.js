"use strict";

// Plugin host. Loaded after app.js and shares its globals (S, invoke, $ ...).
//
// A plugin is a script that calls
//
//   TifMrcView.register({
//     id: "my-plugin", name: "我的插件", version: "1.0.0", description: "...",
//     activate(app) { ... },     // `app` is the API below
//     deactivate() { ... },      // optional
//   });
//
// Only the plugins in BUILTIN_PLUGINS ship with the app. Others are packages
// (a .zip with plugin.json, see src-tauri/src/plugins/package.rs) installed
// with 插件 → 加载插件… into <plugin folder>/<id>/. Loose .js files dropped
// into the plugin folder (插件 → 打开插件文件夹) still load too.

const PLUGIN_API_VERSION = 1;
const BUILTIN_PLUGINS = ["plugins/i18n.js"];
const BUILTIN_IDS = ["i18n"];

const pluginHost = {
  plugins: new Map(), // id -> {def, source, pkg, active, error, cleanup, listeners}
  broken: new Map(), // package id -> {id, source, error}: packages that did not register
  listeners: new Map(), // event -> Set(fn)
  loading: null, // {source, pkg} of the script being loaded
  native: [],
  dir: "",
  appVersion: "",
};

function pluginEnabled(id) {
  return !pref.get("plugins.disabled", []).includes(id);
}

function setPluginEnabled(id, on) {
  const off = new Set(pref.get("plugins.disabled", []));
  if (on) off.delete(id);
  else off.add(id);
  pref.set("plugins.disabled", [...off]);
}

/** Calls the Rust half of a plugin. */
async function nativeCall(id, method, args = {}, bytes = null) {
  const headers = {
    "x-plugin": encodeURIComponent(id),
    "x-method": encodeURIComponent(method),
    "x-args": encodeURIComponent(JSON.stringify(args ?? null)),
  };
  return invoke("plugin_call", bytes ?? new Uint8Array(0), { headers });
}

function makeApi(rec) {
  const id = rec.def.id;
  const on = (event, fn) => {
    if (!pluginHost.listeners.has(event)) pluginHost.listeners.set(event, new Set());
    pluginHost.listeners.get(event).add(fn);
    rec.listeners.push([event, fn]);
    const off = () => pluginHost.listeners.get(event).delete(fn);
    rec.cleanup.push(off);
    return off;
  };
  const track = (el) => {
    rec.cleanup.push(() => el.remove());
    return el;
  };
  return {
    apiVersion: PLUGIN_API_VERSION,
    appVersion: pluginHost.appVersion,

    /** Snapshot of what is shown. `null` fields when nothing is open. */
    state() {
      const i = S.info;
      return {
        info: i,
        path: i ? i.path : null,
        z: S.z,
        t: S.t,
        sizeZ: i ? i.sizeZ : 0,
        sizeT: i ? i.sizeT : 0,
        width: i ? i.width : 0,
        height: i ? i.height : 0,
        selection: S.cropBox || S.crop ? [...(S.cropBox || S.crop)] : null,
        view: i ? visibleRegion() : null,
        pixelSize: S.px ? { ...S.px } : null,
      };
    },
    /** Events: "open" (info), "slice" ({z, t}), "render". Returns an unsubscribe function. */
    on,
    setZ: (z) => setZ(z),
    setT: (t) => setT(t),
    open: (path) => openPath(path),

    /**
     * Renders a region ([x, y, w, h] in image pixels) of slice z/t with the
     * current display settings, annotations and scale bar into a canvas.
     * `size` = [w, h] scales the output.
     */
    renderRegion: (region, opts) => renderRegion(region, opts),

    ui: {
      toast: (msg, ms) => toast(msg, ms),
      /** Adds a section to the tools panel; returns its body element. */
      addToolSection(title) {
        const sec = document.createElement("section");
        sec.dataset.plugin = id;
        const h = document.createElement("h3");
        h.textContent = title;
        const body = document.createElement("div");
        sec.append(h, body);
        $("tools").append(track(sec));
        return body;
      },
      /** Adds a toolbar button. */
      addToolbarButton({ label, title = "", onClick }) {
        const b = document.createElement("button");
        b.textContent = label;
        b.title = title;
        b.dataset.plugin = id;
        b.addEventListener("click", onClick);
        $("plugin-buttons").append(track(b));
        return b;
      },
      /** Adds a toolbar drop-down. `options` = [[value, label], ...]. */
      addToolbarSelect({ title = "", options, value, onChange }) {
        const sel = document.createElement("select");
        sel.title = title;
        sel.dataset.plugin = id;
        for (const [v, label] of options) sel.append(new Option(label, v));
        if (value !== undefined) sel.value = value;
        sel.addEventListener("change", () => onChange(sel.value));
        $("plugin-buttons").append(track(sel));
        return sel;
      },
      showTools: () => showPanel("all"),
      /** Shows a busy overlay with `text`; `null` hides it. */
      busy(text) {
        $("loading").textContent = text || "";
        $("loading").hidden = !text;
      },
    },

    dialog: {
      /** Asks for a save path. Returns `null` when cancelled. */
      save({ name, dir, filter, exts }) {
        return invoke("pick_save_path", {
          dir: dir ?? (S.info ? dirname(S.info.path) : ""),
          name,
          filter: filter ?? null,
          exts: exts ?? null,
        });
      },
    },

    fs: {
      write: (path, bytes) => writeFile(path, bytes),
      dirname,
      basename,
      stem,
    },

    /**
     * Adds to what is known about the open file. Each call names the `path`
     * it is about and is ignored once another file is open, so answers from
     * slow native calls cannot land on the wrong image.
     */
    image: {
      /** Appends groups `{name, items: [[label, value], ...]}` to the image-info panel. */
      addMetadata(path, groups) {
        if (!S.info || S.info.path !== path) return false;
        S.pluginMeta.push(...groups);
        renderMeta();
        return true;
      },
      /** Sets the physical pixel size `{x, y, unit}` (unit Å / nm / µm / mm), as if the file said so. */
      setPixelSize(path, { x, y, unit }) {
        if (!S.info || S.info.path !== path || !(x > 0) || !UNIT_M[unit]) return false;
        S.px = { x, y: y > 0 ? y : x, unit };
        S.pxSource = "来自文件";
        syncScaleUi();
        updateInfo();
        renderMeta();
        draw();
        return true;
      },
      /** Replaces the format name shown in the status bar, e.g. "Zeiss SmartSEM TIFF". */
      setFormat(path, name) {
        if (!S.info || S.info.path !== path) return false;
        S.info.format = name;
        updateInfo();
        renderMeta();
        return true;
      },
    },

    /** Calls the native half of this plugin. */
    native: (method, args, bytes) => nativeCall(id, method, args, bytes),
    hasNative: () => pluginHost.native.includes(id),

    /** Per-plugin persistent settings. */
    settings: {
      get: (k, d) => pref.get(`plugin.${id}.${k}`, d),
      set: (k, v) => pref.set(`plugin.${id}.${k}`, v),
    },
  };
}

function activatePlugin(rec) {
  if (rec.active) return;
  try {
    rec.def.activate(makeApi(rec));
    rec.active = true;
    rec.error = null;
    // Late joiners still learn about the open image.
    if (S.info) emitTo(rec, "open", S.info);
  } catch (e) {
    rec.error = String(e);
    console.error(`plugin ${rec.def.id}`, e);
  }
}

function deactivatePlugin(rec) {
  if (!rec.active) return;
  try {
    if (rec.def.deactivate) rec.def.deactivate();
  } catch (e) {
    console.error(`plugin ${rec.def.id}`, e);
  }
  rec.cleanup.splice(0).forEach((f) => f());
  rec.listeners = [];
  rec.active = false;
}

function emitTo(rec, event, data) {
  for (const [ev, fn] of rec.listeners) {
    if (ev !== event) continue;
    try {
      fn(data);
    } catch (e) {
      console.error(`plugin ${rec.def.id}`, e);
    }
  }
}

window.TifMrcView = {
  apiVersion: PLUGIN_API_VERSION,

  register(def) {
    const from = pluginHost.loading || { source: "?", pkg: null };
    const fail = (msg) => {
      console.error(msg);
      if (from.pkg) pluginHost.broken.set(from.pkg, { id: from.pkg, source: from.source, error: msg });
      renderPluginList();
    };
    if (from.pkg) from.registered = true;
    if (!def || !def.id || typeof def.activate !== "function") {
      return fail("TifMrcView.register: plugin needs an id and activate()");
    }
    if (from.pkg && def.id !== from.pkg) {
      return fail(`插件 id（${def.id}）和插件包（${from.pkg}）不一致`);
    }
    if (def.apiVersion && def.apiVersion > PLUGIN_API_VERSION) {
      return fail(`插件需要更新的 TifMrcView（插件接口版本 ${def.apiVersion}）`);
    }
    if (pluginHost.plugins.has(def.id)) {
      console.warn(`plugin ${def.id} registered twice; keeping the first`);
      return;
    }
    const rec = {
      def,
      source: from.source,
      pkg: from.pkg,
      active: false,
      error: null,
      cleanup: [],
      listeners: [],
    };
    pluginHost.broken.delete(def.id);
    pluginHost.plugins.set(def.id, rec);
    if (pluginEnabled(def.id)) activatePlugin(rec);
    renderPluginList();
  },

  emit(event, data) {
    const set = pluginHost.listeners.get(event);
    if (!set) return;
    for (const fn of [...set]) {
      try {
        fn(data);
      } catch (e) {
        console.error(`plugin listener for ${event}`, e);
      }
    }
  },
};

/** Loads one script; `from` = {source, pkg} tells `register` where it came from. Resolves to false on a load error. */
function loadScript(src, from) {
  return new Promise((resolve) => {
    const el = document.createElement("script");
    el.src = src;
    const done = (ok) => () => {
      pluginHost.loading = null;
      resolve(ok);
    };
    el.onload = done(true);
    el.onerror = done(false);
    pluginHost.loading = from;
    document.body.append(el);
  });
}

/** URL of a file in the user plugin folder; `path` is "file.js" or "<id>/<path>". */
function userPluginUrl(path) {
  const scheme = navigator.userAgent.includes("Windows") ? "http://tmvplugin.localhost/" : "tmvplugin://localhost/";
  return scheme + path.split("/").map(encodeURIComponent).join("/");
}

/** Loads an installed package ({id, main, error}) from `plugin_scan` or `plugin_install`. */
async function loadPackage(p) {
  if (p.error) {
    pluginHost.broken.set(p.id, { id: p.id, source: "已安装", error: p.error });
    return;
  }
  const from = { source: "已安装", pkg: p.id, registered: false };
  const ok = await loadScript(userPluginUrl(`${p.id}/${p.main}`), from);
  if (!ok) pluginHost.broken.set(p.id, { id: p.id, source: "已安装", error: `无法载入 ${p.main}` });
  else if (!from.registered) pluginHost.broken.set(p.id, { id: p.id, source: "已安装", error: `${p.main} 没有调用 TifMrcView.register` });
}

async function loadPlugins() {
  try {
    const scan = await invoke("plugin_scan");
    pluginHost.native = scan.native;
    pluginHost.dir = scan.dir;
    // Scripts load one at a time so `register` knows which file it came from.
    for (const src of BUILTIN_PLUGINS) await loadScript(src, { source: "内置", pkg: null });
    for (const p of scan.packages) await loadPackage(p);
    for (const f of scan.user) await loadScript(userPluginUrl(f), { source: f, pkg: null });
  } catch (e) {
    console.error("loading plugins", e);
  }
  renderPluginList();
}

// ---------- install / uninstall ----------

async function installPackage() {
  const path = await invoke("pick_plugin_package");
  if (!path) return;
  let m;
  try {
    m = await invoke("plugin_install", { path });
  } catch (e) {
    toast(`安装失败：${e}`, 6000);
    return;
  }
  const label = `${m.name}  v${m.version}`;
  const old = pluginHost.plugins.get(m.id);
  if (old || pluginHost.broken.has(m.id)) {
    // Its old script is already in the page; start over.
    if (old) deactivatePlugin(old);
    toast(`已更新 ${label}，正在重新加载…`);
    setTimeout(() => location.reload(), 1000);
    return;
  }
  await loadPackage({ id: m.id, main: m.main || "index.js" });
  renderPluginList();
  const err = pluginHost.broken.get(m.id);
  toast(err ? `已安装 ${label}，但出错：${err.error}` : `已安装 ${label}`, err ? 6000 : 3500);
}

async function uninstallPackage(id) {
  const rec = pluginHost.plugins.get(id);
  if (rec) deactivatePlugin(rec);
  try {
    await invoke("plugin_uninstall", { id });
  } catch (e) {
    toast(`卸载失败：${e}`, 6000);
    if (rec && pluginEnabled(id)) activatePlugin(rec);
    renderPluginList();
    return;
  }
  // The script stays in the page but is inactive; a reload drops it.
  pluginHost.plugins.delete(id);
  pluginHost.broken.delete(id);
  renderPluginList();
  toast(`已卸载 ${rec ? rec.def.name || id : id}`);
}

/** A 卸载 button that asks once more before acting. */
function uninstallButton(id) {
  const b = document.createElement("button");
  b.className = "small pl-remove";
  b.textContent = "卸载";
  b.title = "删除这个插件";
  let armed = null;
  b.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (!armed) {
      b.textContent = "确认卸载？";
      armed = setTimeout(() => {
        armed = null;
        b.textContent = "卸载";
      }, 3000);
      return;
    }
    clearTimeout(armed);
    b.disabled = true;
    uninstallPackage(id);
  });
  return b;
}

// ---------- plugin & update dialog ----------

function pluginRow({ name, version, description, source, error, rec, pkg }) {
  const row = document.createElement("label");
  row.className = "plugin-row";
  const cb = document.createElement("input");
  cb.type = "checkbox";
  if (rec) {
    cb.checked = rec.active;
    cb.addEventListener("change", () => {
      setPluginEnabled(rec.def.id, cb.checked);
      if (cb.checked) activatePlugin(rec);
      else deactivatePlugin(rec);
      renderPluginList();
    });
  } else {
    cb.disabled = true;
  }
  const text = document.createElement("div");
  text.className = "pl-text";
  const title = document.createElement("div");
  title.className = "pl-name";
  title.textContent = `${name}${version ? "  v" + version : ""}`;
  const meta = document.createElement("div");
  meta.className = "dim";
  meta.textContent = [description, source].filter(Boolean).join(" · ");
  text.append(title, meta);
  if (error) {
    const err = document.createElement("div");
    err.className = "pl-err";
    err.textContent = `出错：${error}`;
    text.append(err);
  }
  row.append(cb, text);
  if (pkg) row.append(uninstallButton(pkg));
  return row;
}

function renderPluginList() {
  const box = $("plugin-list");
  if (!box) return;
  box.innerHTML = "";
  for (const rec of pluginHost.plugins.values()) {
    const d = rec.def;
    box.append(
      pluginRow({
        name: d.name || d.id,
        version: d.version,
        description: d.description,
        source: rec.source,
        error: rec.error,
        rec,
        pkg: rec.pkg,
      }),
    );
  }
  for (const b of pluginHost.broken.values()) {
    box.append(pluginRow({ name: b.id, source: b.source, error: b.error, pkg: b.id }));
  }
  const extra = [...pluginHost.plugins.keys()].some((id) => !BUILTIN_IDS.includes(id)) || pluginHost.broken.size;
  if (!extra) {
    const hint = document.createElement("div");
    hint.className = "dim pl-hint";
    hint.textContent = "到插件市场下载更多插件（录制视频、蔡司 SmartSEM、蔡司 ZEN…），下载后点「加载插件…」安装";
    box.append(hint);
  }
}

function togglePluginDialog(force) {
  const dlg = $("plugin-dialog");
  const show = force ?? dlg.hidden;
  dlg.hidden = !show;
  $("btn-plugins").classList.toggle("on", show);
  if (show) renderPluginList();
}

// ---------- updates ----------

const updater = { info: null, busy: false };

function setUpdateStatus(text, info = null) {
  $("update-status").textContent = text;
  const b = $("update-install");
  b.hidden = !info;
  if (info) {
    b.textContent = `更新到 ${info.version}`;
    b.disabled = updater.busy;
  }
}

function showUpdate(info) {
  updater.info = info;
  const bar = $("update-bar");
  if (!info) {
    bar.hidden = true;
    setUpdateStatus("已是最新版本");
    return;
  }
  $("update-text").textContent = `发现新版本 ${info.version}（当前 ${info.current}）`;
  setUpdateStatus(`可更新到 ${info.version}`, info);
  $("update-go").disabled = false;
  bar.hidden = false;
}

async function checkUpdate(manual) {
  if (updater.busy) return;
  updater.busy = true;
  setUpdateStatus("正在检查…");
  try {
    const info = await invoke("check_update");
    updater.busy = false;
    if (info && !manual && pref.get("update.skip", "") === info.version) {
      updater.info = info;
      setUpdateStatus(`可更新到 ${info.version}（已忽略）`, info);
    } else {
      showUpdate(info);
    }
  } catch (e) {
    // The updater says only this when every endpoint failed: offline, server
    // unreachable, or no (valid) latest.json published yet.
    const msg = /valid release JSON/i.test(String(e))
      ? "无法获取更新信息：连不上更新服务器，或服务器上还没有发布版本，请稍后再试"
      : `检查失败：${e}`;
    setUpdateStatus(msg);
    if (manual) toast(msg, 6000);
  } finally {
    updater.busy = false;
  }
}

async function installUpdate() {
  if (!updater.info || updater.busy) return;
  updater.busy = true;
  const say = (text) => {
    $("update-text").textContent = text;
    $("update-status").textContent = text;
  };
  $("update-go").disabled = true;
  $("update-install").disabled = true;
  say("正在下载更新…");
  const unlisten = await T.event.listen("update-progress", (e) => {
    const { done, total } = e.payload;
    say(total ? `正在下载更新… ${Math.floor((done / total) * 100)}%` : `正在下载更新… ${fmtBytes(done)}`);
  });
  try {
    await invoke("install_update"); // restarts the app on success
  } catch (e) {
    say(`更新失败：${e}`);
    $("update-go").disabled = false;
    $("update-install").disabled = false;
  } finally {
    unlisten();
    updater.busy = false;
  }
}

$("btn-plugins").addEventListener("click", () => togglePluginDialog());
$("plugin-close").addEventListener("click", () => togglePluginDialog(false));
$("plugin-market").addEventListener("click", () =>
  invoke("open_market").catch((e) => toast(`无法打开插件市场：${e}`)),
);
$("plugin-install").addEventListener("click", () => installPackage().catch((e) => toast(`安装失败：${e}`, 6000)));
$("plugin-dir").addEventListener("click", () =>
  invoke("open_plugin_dir").catch((e) => toast(`无法打开插件文件夹：${e}`)),
);
$("plugin-reload").addEventListener("click", () => location.reload());
$("update-check").addEventListener("click", () => checkUpdate(true));
$("update-auto").addEventListener("change", (e) => pref.set("update.auto", e.target.checked));
$("update-go").addEventListener("click", installUpdate);
$("update-install").addEventListener("click", installUpdate);
$("update-later").addEventListener("click", () => {
  if (updater.info) pref.set("update.skip", updater.info.version);
  $("update-bar").hidden = true;
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !$("plugin-dialog").hidden) togglePluginDialog(false);
});

(async () => {
  try {
    pluginHost.appVersion = await T.app.getVersion();
  } catch (_) {}
  $("app-version").textContent = pluginHost.appVersion ? `TifMrcView ${pluginHost.appVersion}` : "TifMrcView";
  $("update-auto").checked = pref.get("update.auto", true);
  await loadPlugins();
  // Check quietly a little after start so opening a file is never delayed.
  if (pref.get("update.auto", true)) setTimeout(() => checkUpdate(false), 4000);
})();
