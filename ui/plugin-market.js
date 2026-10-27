"use strict";

// Market, installation links and package actions. Classic script; shares pluginHost.

// ---------- plugin market (one-click install) ----------

const market = {
  list: null, // [{entry, installed, state}] from plugin_market_list
  error: null,
  loading: null, // promise while fetching
  busy: new Set(), // ids being installed
  request: null, // {id, version} of the tifmrcview:// link being confirmed
};

const thisWindow = T.webviewWindow.getCurrentWebviewWindow();

/** The language the i18n plugin shows, for the market's own translations. */
function uiLang() {
  const rec = pluginHost.plugins.get("i18n");
  if (!rec || !rec.active) return "zh";
  return (document.documentElement.lang || "zh").split("-")[0];
}

/** A plugins.json text field in the interface language (as the market page picks it). */
function marketText(p, key) {
  const lang = uiLang();
  const tr = p.i18n || {};
  if (lang !== "zh") {
    if (tr[lang] && tr[lang][key]) return tr[lang][key];
    if (tr.en && tr.en[key]) return tr.en[key];
  }
  return p[key] || "";
}

// Human descriptions are translated by the built-in i18n DOM observer.
const PERMISSION_LABELS = Object.freeze({
  state: "读取当前图像的信息并切换层和帧",
  ui: "添加工具栏按钮和面板",
  metadata: "给图像添加元数据并设置像素尺寸和格式",
  export: "导出图片到你选择的位置",
  settings: "保存自己的设置",
});

function permissionDetails(p) {
  const box = document.createElement("div");
  box.className = "dim pl-permissions";
  if ((p.apiVersion ?? 1) !== 2) {
    box.textContent = "官方 API 1 插件使用完整权限";
    return box;
  }
  const label = document.createElement("div");
  label.textContent = "插件权限";
  box.append(label);
  const list = document.createElement("ul");
  const labels = (p.permissions || []).map((p) => PERMISSION_LABELS[p] || p);
  for (const text of labels.length ? labels : ["无额外权限"]) {
    const item = document.createElement("li");
    item.textContent = text;
    list.append(item);
  }
  box.append(list);
  return box;
}

function showConfirmDetails(p) {
  $("mc-api").textContent = `API ${p.apiVersion ?? 1}`;
  $("mc-permissions").replaceChildren(permissionDetails(p));
  $("market-confirm").scrollIntoView({ block: "start" });
}

/** Fetches the market list (the backend caches it for the session unless `refresh`). */
function loadMarket(refresh = false) {
  if (market.loading) return market.loading;
  market.loading = invoke("plugin_market_list", { refresh })
    .then((list) => {
      market.list = list;
      market.error = null;
    })
    .catch((e) => {
      market.error = String(e);
    })
    .finally(() => {
      market.loading = null;
      renderMarket();
      renderPluginList();
    });
  renderMarket();
  return market.loading;
}

function marketButton(item) {
  const p = item.entry;
  const b = document.createElement("button");
  b.className = "small";
  if (market.busy.has(p.id)) {
    b.textContent = item.state === "update" || item.state === "replace" ? "正在更新…" : "正在安装…";
    b.disabled = true;
    return b;
  }
  switch (item.state) {
    case "install":
      b.textContent = "安装";
      b.classList.add("primary");
      break;
    case "update":
      b.textContent = `更新到 ${p.version}`;
      b.classList.add("primary");
      break;
    case "replace":
      b.textContent = `换成 ${marketText(p, "name") || p.id}`;
      b.classList.add("primary");
      break;
    case "needsApp":
      b.textContent = `需要 TifMrcView ${p.minAppVersion}`;
      b.title = "请先在下面「检查更新」";
      b.disabled = true;
      return b;
    default:
      b.textContent = "已安装";
      b.disabled = true;
      return b;
  }
  b.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    confirmInstall({ id: p.id });
  });
  return b;
}

function renderMarket() {
  const box = $("market-list");
  if (!box) return;
  box.innerHTML = "";
  const note = (text, cls = "dim") => {
    const d = document.createElement("div");
    d.className = cls;
    d.textContent = text;
    box.append(d);
  };
  if (!market.list) {
    if (market.loading) note("正在读取插件市场…");
    else if (market.error) {
      note(market.error, "dim pl-err");
      note("也可以点「插件市场」在浏览器里下载 zip，再点「加载插件…」安装");
    }
    return;
  }
  // Updates and replacements show on the installed plugin's row instead.
  const available = market.list.filter((l) => !["builtin", "installed", "update", "replace"].includes(l.state));
  const query = $("market-search").value.trim().toLowerCase();
  const list = available.filter(({ entry: p }) =>
    [p.id, p.name, p.description, ...Object.values(p.i18n || {}).flatMap((tr) => [tr.name, tr.description])]
      .some((s) => (s || "").toLowerCase().includes(query)),
  );
  if (!list.length) {
    if (query) note("没有匹配的插件");
    else if (market.list.some((l) => ["installed", "update", "replace"].includes(l.state))) note("插件市场上的插件都已安装");
    else note("插件市场上暂时没有插件");
  }
  for (const item of list) {
    const p = item.entry;
    const row = document.createElement("div");
    row.className = "market-row";
    const text = document.createElement("div");
    text.className = "pl-text";
    const title = document.createElement("div");
    title.className = "pl-name";
    title.textContent = `${marketText(p, "name") || p.id}  v${p.version}`;
    const meta = document.createElement("div");
    meta.className = "dim";
    meta.textContent = marketText(p, "description");
    text.append(title, meta);
    if (p.apiVersion === 2) text.append(permissionDetails(p));
    row.append(text, marketButton(item));
    box.append(row);
  }
}

/** Downloads, checks and installs `id` from the official market. Resolves to an error message, or null. */
async function installFromMarket(id, quiet = false, approval) {
  if (market.busy.has(id)) return "busy";
  market.busy.add(id);
  renderMarket();
  renderPluginList();
  let error = null;
  try {
    // The backend's plugin-installed event then loads it (see below).
    await invoke("plugin_install_from_market", { id, approval });
  } catch (e) {
    error = String(e);
    if (!quiet) toast(`安装失败：${error}`, 8000);
  }
  market.busy.delete(id);
  await loadMarket();
  return error;
}

/** Names of the installed plugins a "replace" item takes over. */
function replacedNames(item) {
  return item.replaces.map((id) => pluginHost.plugins.get(id)?.def.name || id).join(" / ");
}

/**
 * Moves users of plugins that were merged into one (e.g. the three Zeiss
 * plugins into "zeiss") over to the new plugin, quietly, once the market
 * list shows a replacement. Only replacements that ask for no permissions
 * are installed without asking; others keep their button on the old row.
 */
async function migrateReplaced() {
  await loadMarket();
  for (const item of market.list || []) {
    const p = item.entry;
    if (item.state !== "replace" || p.apiVersion !== 2 || (p.permissions || []).length) continue;
    await installFromMarket(p.id, true, p);
  }
}

// ---------- tifmrcview://install-plugin links ----------

function confirmMsg(text, isError = false) {
  const el = $("mc-msg");
  el.textContent = text;
  el.classList.toggle("pl-err", isError);
}

function hideConfirm() {
  market.request = null;
  $("market-confirm").hidden = true;
}

/** Shows the link's plugin with 安装 / 取消. Nothing is installed until 安装 is clicked. */
async function confirmInstall(req) {
  market.request = req;
  togglePluginDialog(true);
  $("market-confirm").hidden = false;
  $("market-confirm").scrollIntoView({ block: "start" });
  $("mc-title").textContent = `从 TifMrcView 插件市场安装「${req.id}」？`;
  $("mc-desc").textContent = "";
  $("mc-api").textContent = "";
  $("mc-permissions").replaceChildren();
  $("mc-install").disabled = true;
  confirmMsg("正在读取插件市场…");
  // Always read the current list: the link may name a plugin published after the cached copy.
  if (market.loading) await market.loading;
  await loadMarket(true);
  if (market.request !== req) return;
  const item = (market.list || []).find((l) => l.entry.id === req.id);
  if (!item) {
    confirmMsg(market.error || `插件市场上没有插件 ${req.id}`, true);
    return;
  }
  const p = item.entry;
  const name = marketText(p, "name") || p.id;
  $("mc-title").textContent = `从 TifMrcView 插件市场安装「${name}」 ${p.version}？`;
  $("mc-desc").textContent = marketText(p, "description");
  req.approval = p;
  showConfirmDetails(p);
  if (item.state === "builtin") return confirmMsg(`${p.id} 是内置插件，不能另外安装`, true);
  if (item.state === "needsApp") {
    const cur = pluginHost.appVersion;
    return confirmMsg(`「${name}」需要 TifMrcView ${p.minAppVersion} 或更新版本（当前 ${cur}），请先在下面「检查更新」`, true);
  }
  if (item.state === "installed") return confirmMsg(`已安装 ${name}  v${item.installed}，已是最新版本`);
  if (item.state === "replace") confirmMsg(`将取代已安装的 ${replacedNames(item)}`);
  else confirmMsg(item.state === "update" ? `已安装 ${item.installed}，将更新到 ${p.version}` : "");
  $("mc-install").disabled = false;
}

async function takeInstallRequest() {
  const req = await invoke("take_install_request").catch(() => null);
  if (req) confirmInstall(req);
}

/** Zip inspection has no install side effects. The digest pins the reviewed bytes. */
async function confirmPackage(path) {
  togglePluginDialog(true);
  const req = { path };
  market.request = req;
  $("market-confirm").hidden = false;
  $("market-confirm").scrollIntoView({ block: "start" });
  $("mc-title").textContent = "正在读取插件包…";
  $("mc-desc").textContent = "";
  $("mc-api").textContent = "";
  $("mc-permissions").replaceChildren();
  $("mc-install").disabled = true;
  confirmMsg("");
  // Populate the backend's official-index cache when available. Offline API 2
  // zips still work; API 1 needs a previously fetched matching official entry.
  if (market.loading) await market.loading;
  else if (!market.list) await loadMarket();
  try {
    const inspected = await invoke("plugin_inspect_package", { path });
    if (market.request !== req) return;
    req.sha256 = inspected.sha256;
    const p = inspected.manifest;
    $("mc-title").textContent = `${p.name}  v${p.version}`;
    $("mc-desc").textContent = p.description || "";
    showConfirmDetails(p);
    $("mc-install").disabled = false;
  } catch (e) {
    if (market.request === req) confirmMsg(`安装失败：${e}`, true);
  }
}

$("mc-install").addEventListener("click", async () => {
  const req = market.request;
  if (!req) return;
  $("mc-install").disabled = true;
  confirmMsg(req.path ? "正在安装…" : "正在下载并安装…");
  let error;
  if (req.path) {
    try {
      const m = await invoke("plugin_install", { path: req.path, sha256: req.sha256 });
      await afterInstall(m, true);
      if (market.list) loadMarket();
    } catch (e) { error = String(e); }
  } else error = await installFromMarket(req.id, true, req.approval);
  if (market.request !== req) return;
  if (error) {
    confirmMsg(`安装失败：${error}`, true);
    $("mc-install").disabled = false;
  } else hideConfirm();
});
$("mc-cancel").addEventListener("click", hideConfirm);
$("market-refresh").addEventListener("click", () => loadMarket(true));
$("market-search").addEventListener("input", renderMarket);
new MutationObserver(renderMarket).observe(document.documentElement, { attributeFilter: ["lang"] });
thisWindow.listen("install-request", takeInstallRequest);
T.event.listen("plugin-installed", (e) => {
  const { manifest, origin } = e.payload;
  afterInstall(manifest, origin === thisWindow.label);
  if (market.list && origin !== thisWindow.label) loadMarket();
});

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

$("plugin-market").addEventListener("click", () =>
  invoke("open_market").catch((e) => toast(`无法打开插件市场：${e}`)),
);
