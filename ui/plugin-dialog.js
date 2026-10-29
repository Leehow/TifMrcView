"use strict";

// Plugin dialog, app updater and startup. Loaded after the host and market.

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
  if (pkg) {
    const actions = document.createElement("div");
    actions.className = "pl-actions";
    const remove = uninstallButton(pkg);
    remove.disabled = market.busy.has(pkg);
    actions.append(remove);
    const update = (market.list || []).find(
      (item) => (item.entry.id === pkg && item.state === "update") || (item.state === "replace" && item.replaces.includes(pkg)),
    );
    if (update) actions.append(marketButton(update));
    row.append(actions);
  }
  return row;
}

function renderPluginList() {
  const box = $("plugin-list");
  if (!box) return;
  box.innerHTML = "";
  for (const rec of pluginHost.plugins.values()) {
    const d = rec.def;
    // Installed packages carry their own translations in plugin.json.
    const m = (rec.pkg && pluginHost.manifests.get(rec.pkg)) || d;
    box.append(
      pluginRow({
        name: marketText(m, "name") || d.name || d.id,
        version: d.version,
        description: marketText(m, "description") || d.description,
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
  for (const r of pluginHost.replaced.values()) {
    box.append(pluginRow({ name: r.name, version: r.version, description: `已由「${r.by}」取代，可以卸载`, pkg: r.id }));
  }
  const extra =
    [...pluginHost.plugins.keys()].some((id) => !BUILTIN_IDS.includes(id)) || pluginHost.broken.size || pluginHost.replaced.size;
  if (!extra) {
    const hint = document.createElement("div");
    hint.className = "dim pl-hint";
    hint.textContent = "在下面的插件市场一键安装更多插件（录制视频、蔡司、徕卡、尼康…）";
    box.append(hint);
  }
}

new MutationObserver(renderPluginList).observe(document.documentElement, { attributeFilter: ["lang"] });

function togglePluginDialog(force) {
  const dlg = $("plugin-dialog");
  const show = force ?? dlg.hidden;
  dlg.hidden = !show;
  if (show) toggleHelp(false);
  if (show) {
    renderPluginList();
    if (market.list || market.loading) renderMarket();
    else loadMarket();
  } else hideConfirm();
}

// ---------- updates ----------

const updater = { info: null, busy: false, dismissed: false };

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
  bar.hidden = updater.dismissed;
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

// ---------- ⚙ settings menu and help ----------

const GEAR_ICONS = {
  plugins: '<path d="M4 7h5a2.5 2.5 0 1 1 5 0h4v5a2.5 2.5 0 1 1 0 5v4H4z"/>',
  lang: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>',
  help: '<circle cx="12" cy="12" r="9"/><path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.6v.6"/><path d="M12 17.5v.5"/>',
};

function openSettings() {
  const btn = $("btn-settings");
  if (closeMenu(btn)) return;
  const items = [{ value: "plugins", label: "插件与更新", icon: GEAR_ICONS.plugins, on: !$("plugin-dialog").hidden }];
  ext.settings.forEach((s, i) => {
    const cur = s.options.find(([v]) => v === s.value);
    items.push({ value: i, label: s.title, hint: cur ? cur[1] : "", icon: GEAR_ICONS.lang });
  });
  items.push({ value: "help", label: "帮助", icon: GEAR_ICONS.help, on: !$("help-dialog").hidden });
  openMenu(btn, items, (v) => {
    if (v === "plugins") togglePluginDialog();
    else if (v === "help") toggleHelp();
    else pickSetting(ext.settings[v]);
  });
  placeMenuRight(btn);
}

/** The options of one settings entry, as a second menu in the same place. */
function pickSetting(s) {
  if (!s) return;
  const btn = $("btn-settings");
  openMenu(
    btn,
    s.options.map(([value, label]) => ({ value, label, on: value === s.value })),
    (v) => {
      s.value = v;
      s.onChange(v);
    },
  );
  placeMenuRight(btn);
}

/** The ⚙ button sits at the right edge: line the menu up with it on the right. */
function placeMenuRight(btn) {
  const m = $("dropmenu");
  // Anchored by its right edge, so it stays on screen when translated labels widen it.
  m.style.left = "auto";
  m.style.right = `${Math.max(8, window.innerWidth - btn.getBoundingClientRect().right)}px`;
}

function toggleHelp(force) {
  const dlg = $("help-dialog");
  const show = force ?? dlg.hidden;
  dlg.hidden = !show;
  if (show) {
    togglePluginDialog(false);
    $("help-version").textContent = $("app-version").textContent;
  }
}

$("btn-settings").addEventListener("click", openSettings);
$("help-close").addEventListener("click", () => toggleHelp(false));
$("help-site").addEventListener("click", () => invoke("open_website").catch((e) => toast(`${e}`)));
$("plugin-close").addEventListener("click", () => togglePluginDialog(false));
$("plugin-install").addEventListener("click", () => installPackage().catch((e) => toast(`安装失败：${e}`, 6000)));
$("plugin-dir").addEventListener("click", () =>
  invoke("open_plugin_dir").catch((e) => toast(`无法打开插件文件夹：${e}`)),
);
$("plugin-reload").addEventListener("click", () => location.reload());
$("update-check").addEventListener("click", () => checkUpdate(true));
$("update-auto").addEventListener("change", (e) => pref.set("update.auto", e.target.checked));
$("update-go").addEventListener("click", installUpdate);
$("update-install").addEventListener("click", installUpdate);
$("update-close").addEventListener("click", () => {
  updater.dismissed = true;
  $("update-bar").hidden = true;
});
$("update-later").addEventListener("click", () => {
  if (updater.info) pref.set("update.skip", updater.info.version);
  $("update-bar").hidden = true;
});
window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (!$("plugin-dialog").hidden) togglePluginDialog(false);
  if (!$("help-dialog").hidden) toggleHelp(false);
});

(async () => {
  let name = "TifMrcView";
  try {
    pluginHost.appVersion = await T.app.getVersion();
    name = await T.app.getName();
  } catch (_) {}
  $("app-version").textContent = pluginHost.appVersion ? `${name} ${pluginHost.appVersion}` : name;
  $("update-auto").checked = pref.get("update.auto", true);
  await loadPlugins();
  // The empty window lists the format plugins from the market.
  if (!$("empty").hidden) loadMarket();
  // Opened (or started) by a tifmrcview://install-plugin link.
  await takeInstallRequest();
  // Check quietly a little after start so opening a file is never delayed.
  if (pref.get("update.auto", true)) {
    setTimeout(() => checkUpdate(false), 4000);
    if (pluginHost.manifests.size) setTimeout(() => migrateReplaced().catch((e) => console.warn("plugin migration", e)), 5000);
  }
})();
