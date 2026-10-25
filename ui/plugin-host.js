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
// Built-in plugins are listed in BUILTIN_PLUGINS. Users drop extra .js files
// into the plugin folder (插件 → 打开插件文件夹).

const PLUGIN_API_VERSION = 1;
const BUILTIN_PLUGINS = ["plugins/recorder.js"];

const pluginHost = {
  plugins: new Map(), // id -> {def, source, active, error, cleanup, listeners}
  listeners: new Map(), // event -> Set(fn)
  loading: null, // source of the script being loaded
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
        selection: S.selRect ? [...S.selRect] : null,
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
      showTools: () => toggleTools(true),
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
    if (!def || !def.id || typeof def.activate !== "function") {
      console.error("TifMrcView.register: plugin needs an id and activate()");
      return;
    }
    if (def.apiVersion && def.apiVersion > PLUGIN_API_VERSION) {
      console.error(`plugin ${def.id} needs API ${def.apiVersion}`);
      return;
    }
    if (pluginHost.plugins.has(def.id)) {
      console.warn(`plugin ${def.id} registered twice; keeping the first`);
      return;
    }
    const rec = { def, source: pluginHost.loading || "?", active: false, error: null, cleanup: [], listeners: [] };
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

function loadScript(src, source) {
  return new Promise((resolve) => {
    const el = document.createElement("script");
    el.src = src;
    el.onload = el.onerror = () => {
      pluginHost.loading = null;
      resolve();
    };
    pluginHost.loading = source;
    document.body.append(el);
  });
}

function userPluginUrl(file) {
  const scheme = navigator.userAgent.includes("Windows") ? "http://tmvplugin.localhost/" : "tmvplugin://localhost/";
  return scheme + encodeURIComponent(file);
}

async function loadPlugins() {
  try {
    const scan = await invoke("plugin_scan");
    pluginHost.native = scan.native;
    pluginHost.dir = scan.dir;
    // Scripts load one at a time so `register` knows which file it came from.
    for (const src of BUILTIN_PLUGINS) await loadScript(src, "内置");
    for (const f of scan.user) await loadScript(userPluginUrl(f), f);
  } catch (e) {
    console.error("loading plugins", e);
  }
  renderPluginList();
}

// ---------- plugin & update dialog ----------

function renderPluginList() {
  const box = $("plugin-list");
  if (!box) return;
  box.innerHTML = "";
  if (!pluginHost.plugins.size) {
    box.innerHTML = `<div class="dim">没有插件</div>`;
    return;
  }
  for (const rec of pluginHost.plugins.values()) {
    const d = rec.def;
    const row = document.createElement("label");
    row.className = "plugin-row";
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = rec.active;
    cb.addEventListener("change", () => {
      setPluginEnabled(d.id, cb.checked);
      if (cb.checked) activatePlugin(rec);
      else deactivatePlugin(rec);
      renderPluginList();
    });
    const text = document.createElement("div");
    const title = document.createElement("div");
    title.className = "pl-name";
    title.textContent = `${d.name || d.id}${d.version ? "  v" + d.version : ""}`;
    const meta = document.createElement("div");
    meta.className = "dim";
    meta.textContent = [d.description, rec.source === "内置" ? "内置" : rec.source].filter(Boolean).join(" · ");
    text.append(title, meta);
    if (rec.error) {
      const err = document.createElement("div");
      err.className = "pl-err";
      err.textContent = `出错：${rec.error}`;
      text.append(err);
    }
    row.append(cb, text);
    box.append(row);
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

function showUpdate(info) {
  updater.info = info;
  const bar = $("update-bar");
  if (!info) {
    bar.hidden = true;
    $("update-status").textContent = "已是最新版本";
    return;
  }
  $("update-text").textContent = `发现新版本 ${info.version}（当前 ${info.current}）`;
  $("update-status").textContent = `可更新到 ${info.version}`;
  $("update-go").disabled = false;
  bar.hidden = false;
}

async function checkUpdate(manual) {
  if (updater.busy) return;
  updater.busy = true;
  $("update-status").textContent = "正在检查…";
  try {
    const info = await invoke("check_update");
    if (info && !manual && pref.get("update.skip", "") === info.version) {
      updater.info = info;
      $("update-status").textContent = `可更新到 ${info.version}（已忽略）`;
    } else {
      showUpdate(info);
    }
  } catch (e) {
    // The updater reports a missing or unreachable latest.json this way.
    const msg = /valid release JSON/i.test(String(e)) ? "暂无可用更新（更新服务器上还没有发布版本，或暂时连不上）" : `检查失败：${e}`;
    $("update-status").textContent = msg;
    if (manual) toast(msg, 6000);
  } finally {
    updater.busy = false;
  }
}

async function installUpdate() {
  if (!updater.info || updater.busy) return;
  updater.busy = true;
  $("update-go").disabled = true;
  $("update-text").textContent = "正在下载更新…";
  const unlisten = await T.event.listen("update-progress", (e) => {
    const { done, total } = e.payload;
    $("update-text").textContent = total
      ? `正在下载更新… ${Math.floor((done / total) * 100)}%`
      : `正在下载更新… ${fmtBytes(done)}`;
  });
  try {
    await invoke("install_update"); // restarts the app on success
  } catch (e) {
    $("update-text").textContent = `更新失败：${e}`;
    $("update-go").disabled = false;
  } finally {
    unlisten();
    updater.busy = false;
  }
}

$("btn-plugins").addEventListener("click", () => togglePluginDialog());
$("plugin-close").addEventListener("click", () => togglePluginDialog(false));
$("plugin-dir").addEventListener("click", () =>
  invoke("open_plugin_dir").catch((e) => toast(`无法打开插件文件夹：${e}`)),
);
$("plugin-reload").addEventListener("click", () => location.reload());
$("update-check").addEventListener("click", () => checkUpdate(true));
$("update-auto").addEventListener("change", (e) => pref.set("update.auto", e.target.checked));
$("update-go").addEventListener("click", installUpdate);
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
