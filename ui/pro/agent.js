"use strict";
// Agents (Codex, Claude Code…) driving this window: `tmv app …` and
// `tmv mcp` post a call to the app (src/app_control.rs), which hands it here
// as an "agent-call" event; the answer goes back with `agent_reply`.
// Everything an agent does goes through the same buttons, fields and calls
// a person uses, so the window shows each step as it happens: the control
// is outlined and a strip at the bottom says what the agent is doing. The
// user allows an agent once (or always, or never: ⚙ → AI 助手) and can stop
// it from the strip.
// Which feature is where on both sides (window and `tmv`) is in
// pro/features.js; scripts/agent-parity.test.mjs keeps it complete.
// Shares app.js globals: S, T, invoke, $, pref, toast, openPath, setZ, setT, showPanel, ext.
(() => {
  if (!T?.webviewWindow) return; // the web demo has no app behind it
  const thisWindow = T.webviewWindow.getCurrentWebviewWindow();
  const Pro = window.TifMrcViewPro;
  const features = window.TmvFeatures?.features || [];

  // ---------- helpers ----------

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const clip = (s, n = 120) => { s = String(s ?? "").replace(/\s+/g, " ").trim(); return s.length > n ? s.slice(0, n - 1) + "…" : s; };
  const fail = (msg) => { throw new Error(msg); };

  function visible(el) {
    if (!el || !el.isConnected || el.closest("[inert], [hidden]")) return false;
    if (!el.getClientRects().length) return false;
    const cs = getComputedStyle(el);
    return cs.visibility !== "hidden" && cs.display !== "none";
  }

  let nextRef = 0;
  /** A name to find `el` again: its id, else a number it keeps while it lives. */
  function refOf(el) {
    if (el.id) return el.id;
    if (!el.dataset.agentRef) el.dataset.agentRef = `a${++nextRef}`;
    return el.dataset.agentRef;
  }

  function labelOf(el) {
    const own = el.getAttribute("aria-label");
    if (own) return clip(own);
    if (el.labels && el.labels.length) return clip(el.labels[0].textContent);
    const text = clip(el.textContent);
    if (text) return text;
    return clip(el.title || el.placeholder || el.getAttribute("alt") || el.value || "");
  }

  /** The panel an element sits in (its nearest ancestor with an id). */
  function placeOf(el) {
    const p = el.parentElement?.closest("[id]");
    return p ? p.id : "";
  }

  const INTERACTIVE = "button, input, select, textarea, summary, a[href], [role=button], [role=radio], [role=tab], [role=checkbox], [tabindex='0']";

  function controls(scope) {
    const root = scope ? document.getElementById(scope) || document.querySelector(scope) : document.body;
    if (!root) fail(`没有找到 ${scope}`);
    return [...root.querySelectorAll(INTERACTIVE)].filter(visible);
  }

  function describe(el) {
    const d = { ref: refOf(el), kind: el.tagName.toLowerCase(), label: labelOf(el) };
    if (el.type && el.tagName === "INPUT") d.kind = `input:${el.type}`;
    if (el.title && clip(el.title) !== d.label) d.hint = clip(el.title, 160);
    if (el.disabled) d.disabled = true;
    if (el.type === "checkbox" || el.type === "radio") d.checked = el.checked;
    else if (el.getAttribute("role") === "radio" || el.getAttribute("aria-checked")) d.checked = el.getAttribute("aria-checked") === "true";
    else if (el.tagName === "SELECT") {
      d.value = el.value;
      d.options = [...el.options].map((o) => (o.value === o.textContent ? o.value : `${o.value}=${clip(o.textContent, 60)}`));
    } else if (el.tagName === "INPUT" || el.tagName === "TEXTAREA") d.value = el.value;
    if (el.classList.contains("on")) d.on = true;
    if (el.tagName === "SUMMARY") d.open = !!el.parentElement?.open;
    d.in = placeOf(el);
    return d;
  }

  /** The visible status lines (what the window says it is doing). */
  function statusLines() {
    const out = [];
    for (const el of document.querySelectorAll("[role=status], #toast, #loading")) {
      if (!visible(el)) continue;
      const t = clip(el.textContent, 300);
      if (t) out.push({ in: el.id || placeOf(el), text: t });
    }
    return out;
  }

  function find({ ref, text, in: scope, selector }) {
    if (ref) {
      const el = document.getElementById(ref) || document.querySelector(`[data-agent-ref="${CSS.escape(ref)}"]`);
      if (!el) fail(`没有找到 ref=${ref}（界面可能已经变了，先用 ui 再看一次）`);
      return el;
    }
    if (selector) {
      const el = [...document.querySelectorAll(selector)].find(visible);
      if (!el) fail(`没有看得见的 ${selector}`);
      return el;
    }
    if (text) {
      const all = controls(scope);
      const t = String(text).trim();
      const el = all.find((e) => labelOf(e) === t) || all.find((e) => labelOf(e).includes(t)) || all.find((e) => (e.title || "").includes(t));
      if (!el) fail(`界面上没有看得见的「${t}」${scope ? `（在 ${scope} 里）` : ""}；先用 ui 看看有什么`);
      return el;
    }
    fail("要给 ref、text 或 selector");
  }

  // ---------- showing the user what happens ----------

  const strip = document.createElement("div");
  strip.id = "agent-strip";
  strip.hidden = true;
  strip.setAttribute("role", "status");
  strip.innerHTML = '<span class="agent-dot"></span><b>AI 助手</b><span id="agent-doing"></span><button id="agent-stop" class="small" title="不再让 AI 助手操作，直到下次允许">停止</button>';
  document.body.append(strip);
  const css = document.createElement("style");
  css.textContent = `
    #agent-strip{position:fixed;left:50%;bottom:14px;transform:translateX(-50%);z-index:9998;display:flex;gap:8px;align-items:center;
      max-width:min(720px,90vw);padding:6px 8px 6px 12px;border-radius:999px;background:var(--panel,#222);color:var(--fg,#eee);
      box-shadow:0 4px 18px rgba(0,0,0,.35);font-size:12px;border:1px solid var(--accent,#4c8dff)}
    #agent-strip #agent-doing{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;opacity:.85}
    #agent-strip .agent-dot{width:8px;height:8px;border-radius:50%;background:var(--accent,#4c8dff);animation:agent-pulse 1.2s infinite}
    @keyframes agent-pulse{50%{opacity:.3}}
    .agent-touch{outline:3px solid var(--accent,#4c8dff)!important;outline-offset:2px;transition:outline-color .3s}
    #agent-ask{position:fixed;inset:0;z-index:9999;display:flex;align-items:center;justify-content:center;background:rgba(0,0,0,.45)}
    #agent-ask>div,#agent-setup>div{background:var(--panel,#222);color:var(--fg,#eee);padding:18px 20px;border-radius:10px;max-width:560px;box-shadow:0 8px 30px rgba(0,0,0,.5)}
    #agent-ask .row,#agent-setup .row{display:flex;gap:8px;justify-content:flex-end;margin-top:14px;flex-wrap:wrap}
    #agent-setup{position:fixed;inset:0;z-index:9999;display:flex;align-items:center;justify-content:center;background:rgba(0,0,0,.45)}
    #agent-setup code{display:block;user-select:all;white-space:pre-wrap;word-break:break-all;background:rgba(127,127,127,.15);padding:6px 8px;border-radius:6px;margin:4px 0 10px;font-size:12px}
    #agent-setup h4{margin:10px 0 2px}`;
  document.head.append(css);
  let idleTimer = 0;
  function doing(text) {
    clearTimeout(idleTimer);
    strip.hidden = false;
    $("agent-doing").textContent = text;
    idleTimer = setTimeout(() => (strip.hidden = true), 8000);
  }
  function touch(el) {
    try { el.scrollIntoView({ block: "nearest", inline: "nearest" }); } catch (_) {}
    el.classList.add("agent-touch");
    setTimeout(() => el.classList.remove("agent-touch"), 1400);
  }

  // ---------- permission ----------

  // "ask": the first call of each run of the app asks; "always"; "off".
  const mode = () => pref.get("agent.allow", "ask");
  // Automated tests start the app with TMV_AGENT_ALLOW=always (agent_info).
  let envAllow = null;
  invoke("agent_info").then((i) => { envAllow = i.envAllow || null; }).catch(() => {});
  let allowed = false, asking = null;
  $("agent-stop").onclick = () => { allowed = false; strip.hidden = true; toast("已停止 AI 助手；它下次操作时会再问你"); };

  function ask() {
    if (asking) return asking;
    asking = new Promise((resolve) => {
      const box = document.createElement("div");
      box.id = "agent-ask";
      box.innerHTML = `<div><b>让 AI 助手操作 TifMrcView Pro？</b>
        <p>这台电脑上的 AI 助手（如 Codex、Claude Code）想打开文件、点按钮来帮你处理数据。它做的每一步你都能在窗口里看到，随时可以在底部点「停止」。</p>
        <div class="row"><button id="agent-no" class="small">拒绝</button><button id="agent-always" class="small">总是允许</button><button id="agent-yes" class="small primary">允许</button></div></div>`;
      document.body.append(box);
      const done = (ok, always) => {
        box.remove();
        asking = null;
        if (always) pref.set("agent.allow", "always");
        allowed = ok;
        resolve(ok);
      };
      $("agent-yes").onclick = () => done(true, false);
      $("agent-always").onclick = () => done(true, true);
      $("agent-no").onclick = () => done(false, false);
      thisWindow.setFocus().catch(() => {});
    });
    return asking;
  }

  async function permitted() {
    const m = mode();
    if (m === "off") fail("用户在 ⚙ → AI 助手 里关闭了 AI 助手操作");
    if (m === "always" || allowed || envAllow === "always") return;
    if (!await ask()) fail("用户没有允许 AI 助手操作这个窗口");
  }

  // ---------- state ----------

  const docs = () => Pro.documents;
  function tabList() {
    const d = docs();
    if (!d) return [];
    const cur = d.current();
    return d.tabs.map((t, i) => ({ index: i, type: t.type, name: t.name, path: t.path || null, active: t === cur, unsaved: !!t.dirty }));
  }

  function busy() {
    const b = {};
    if (Pro.cryoet?.busy) b.cryoet = true;
    if (Pro.spa?.busy) b.spa = true;
    if (Pro.sta?.busy) b.sta = true;
    const lab = Pro.labels?.state?.();
    if (lab?.busy) b.labels = true;
    if ($("lab-busy")?.textContent.trim()) b.labels = clip($("lab-busy").textContent);
    if (visible($("loading"))) b.loading = clip($("loading").textContent) || true;
    // A visible 取消 / 停止 button means a job is running.
    for (const el of document.querySelectorAll("button[id$='-cancel'], button[id$='-stop']")) {
      if (el.id !== "agent-stop" && visible(el) && !el.disabled) b[el.id] = true;
    }
    return b;
  }

  function openPanels() {
    return [...document.querySelectorAll(".pro-workspace, .pro-panel, #cryoet-workspace, #align-panel, #vol3d, #fft-panel, #tools")]
      .filter((el) => !el.hidden && visible(el)).map((el) => el.id).filter(Boolean);
  }

  function status() {
    const i = S.info;
    return {
      image: i ? { path: i.path, name: i.name, width: i.width, height: i.height, sizeZ: i.sizeZ, sizeT: i.sizeT, z: S.z, t: S.t, axis: S.axis,
        pixelSize: S.px ? { ...S.px } : null, format: i.format } : null,
      tabs: tabList(),
      panels: openPanels(),
      busy: busy(),
      status: statusLines(),
      labels: Pro.labels?.state ? Pro.labels.state() : null,
    };
  }

  // ---------- doing ----------

  async function open(path) {
    if (!path) fail("要给 path（文件或文件夹的完整路径）");
    doing(`打开 ${path}`);
    if (docs()) await docs().open(path);
    else await openPath(path);
    for (let i = 0; i < 300 && visible($("loading")); i++) await sleep(100);
    return status();
  }

  async function press(el, what) {
    if (el.disabled) fail(`「${labelOf(el)}」现在不能按（灰着）；先看看 ui 和 status`);
    doing(what || `点「${labelOf(el)}」`);
    touch(el);
    await sleep(250);
    el.click();
    await sleep(300);
  }

  async function setValue(el, value) {
    doing(`把「${labelOf(el)}」设为 ${clip(JSON.stringify(value), 60)}`);
    touch(el);
    await sleep(200);
    if (el.type === "checkbox" || el.type === "radio") {
      const on = value === true || value === "true" || value === 1 || value === "1" || value === "on";
      if (el.checked !== on) el.click();
    } else if (el.tagName === "SELECT") {
      const opt = [...el.options].find((o) => o.value === String(value)) || [...el.options].find((o) => o.textContent.trim() === String(value)) || [...el.options].find((o) => o.textContent.includes(String(value)));
      if (!opt) fail(`「${labelOf(el)}」没有选项 ${value}；可选：${[...el.options].map((o) => o.value).join(", ")}`);
      el.value = opt.value;
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
    } else if ("value" in el) {
      el.focus();
      el.value = String(value);
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      el.blur();
    } else fail(`「${labelOf(el)}」不是可以填写的控件`);
    await sleep(200);
  }

  /** Opens the panel of feature `id` (its tool button), unless it is open. */
  async function workspace(name, close = false) {
    const f = features.find((x) => x.id === name || x.ui?.button === name);
    if (!f || !f.ui?.button) fail(`没有叫 ${name} 的工具；可以用的：${features.filter((x) => x.ui?.button).map((x) => x.id).join(", ")}`);
    const b = $(f.ui.button);
    if (!b) fail(`${f.name} 在这个窗口里不可用`);
    const on = b.classList.contains("on");
    if (on !== !close) await press(b, `${close ? "关闭" : "打开"}${f.name}`);
    return f;
  }

  async function ensurePanel(button, panel) {
    if ($(panel) && !$(panel).hidden && visible($(panel))) return;
    if (!$(button)) fail(`窗口里没有 ${button}`);
    await press($(button));
  }

  async function snapshotOf(scope) {
    return controls(scope).slice(0, 200).map(describe);
  }

  async function waitIdle(args, timeoutMs) {
    const limit = Math.max(1000, Math.min(args.timeoutMs || timeoutMs - 3000, timeoutMs - 3000));
    const t0 = Date.now();
    await sleep(Math.min(1500, limit / 4));
    for (;;) {
      const b = busy();
      const textOk = args.text ? statusLines().some((s) => s.text.includes(args.text)) || document.body.innerText.includes(args.text) : null;
      const done = args.text ? textOk : !Object.keys(b).length;
      if (done) return { ...status(), waited: Math.round((Date.now() - t0) / 1000) };
      if (Date.now() - t0 > limit) return { ...status(), waited: Math.round((Date.now() - t0) / 1000), stillBusy: true, note: "还没做完；再调用 wait 继续等" };
      await sleep(700);
    }
  }

  const needPath = async (a) => { if (a.path) await open(a.path); if (!S.info) fail("先打开数据（给 path）"); };

  // name: {args, desc, run(args, call)}; `help` lists them.
  const ACTIONS = {
    help: { args: "", desc: "这张表：每个动作和参数，以及 features（界面上的入口和对应的 tmv 命令）", run: () => ({
      actions: Object.entries(ACTIONS).map(([name, a]) => ({ name, args: a.args, desc: a.desc })),
      features: features.map((f) => ({ id: f.id, name: f.name, app: f.app, cli: f.cli ? `tmv ${f.cli}` : null, cliWhy: f.cliWhy })),
      tips: "像用户那样操作：status 看现状，ui 看看得见的按钮（ref、文字、in=所在面板），click / set 操作，wait 等待长任务，screenshot 看图。参数用 tmv app <动作> key=value。要平均或挑选什么由用户决定：把候选给用户看，问他。",
    }) },
    status: { args: "", desc: "打开的图像、标签页、打开的面板、正在运行的任务、状态栏文字、标签图层", run: () => status() },
    open: { args: "path=<文件或文件夹>", desc: "像用户一样打开文件（新标签页）或文件夹", run: (a) => open(a.path) },
    tabs: { args: "", desc: "所有标签页", run: () => ({ tabs: tabList() }) },
    tab: { args: "index=N | path=… | name=…", desc: "切到一个标签页", run: async (a) => {
      const d = docs() || fail("这个窗口没有标签页");
      const t = d.tabs.find((x, i) => i === a.index || (a.path && x.path === a.path) || (a.name && x.name === a.name)) || fail("没有这个标签页（看 tabs）");
      doing(`切到 ${t.name}`);
      await d.activate(t);
      return { tabs: tabList() };
    } },
    close_tab: { args: "index=N", desc: "关闭一个标签页（有未保存的改动时会问用户）", run: async (a) => {
      const d = docs() || fail("这个窗口没有标签页");
      const t = d.tabs[a.index ?? d.tabs.indexOf(d.current())] || fail("没有这个标签页");
      doing(`关闭 ${t.name}`);
      await d.close(t);
      return { tabs: tabList() };
    } },
    slice: { args: "z=N [t=N]", desc: "翻到第 z 层（从 0 数）", run: (a) => {
      if (!S.info) fail("没有打开的图像");
      doing(`翻到第 ${a.z ?? S.z} 层`);
      if (a.t != null) setT(a.t);
      if (a.z != null) setZ(a.z);
      return status().image;
    } },
    ui: { args: "[in=面板 id] [find=文字]", desc: "看得见的按钮、勾选框、输入框和下拉框：ref、文字、状态、所在面板", run: async (a) => {
      let list = await snapshotOf(a.in);
      if (a.find) list = list.filter((c) => c.label.includes(a.find) || (c.hint || "").includes(a.find));
      return { panels: openPanels(), controls: list, status: statusLines() };
    } },
    click: { args: "ref=… | text=按钮文字 [in=面板 id]", desc: "点一个按钮、卡片、勾选框或折叠标题，和用户点一样", run: async (a) => {
      const el = find(a);
      await press(el);
      return { clicked: labelOf(el), busy: busy(), status: statusLines() };
    } },
    set: { args: "ref=…|text=… value=…", desc: "填输入框、选下拉框、勾或取消勾选框", run: async (a) => {
      if (!("value" in a)) fail("要给 value");
      const el = find(a);
      await setValue(el, a.value);
      return describe(el);
    } },
    key: { args: "key=Escape|Enter|…", desc: "按一个键（Escape 停止正在运行的任务）", run: async (a) => {
      doing(`按 ${a.key}`);
      const t = document.activeElement || document.body;
      for (const type of ["keydown", "keyup"]) t.dispatchEvent(new KeyboardEvent(type, { key: a.key, bubbles: true }));
      await sleep(200);
      return { busy: busy() };
    } },
    wait: { args: "[text=等这段文字出现] [timeoutMs=…]", desc: "等到没有任务在运行（或出现某段文字），再返回 status", run: (a, call) => waitIdle(a, call.timeoutMs || 60000) },
    screenshot: { args: "", desc: "用户看到的图像视图（PNG 文件）", run: () => {
      doing("看一下图像");
      // On the window's background, as the user sees it (the canvas is clear around the image).
      const v = $("view"), c = document.createElement("canvas");
      c.width = v.width; c.height = v.height;
      const g = c.getContext("2d");
      g.fillStyle = getComputedStyle($("stage") || document.body).backgroundColor || "#000";
      g.fillRect(0, 0, c.width, c.height);
      g.drawImage(v, 0, 0);
      const out = { view: c.toDataURL("image/png") };
      const v3 = document.querySelector("#vol3d canvas");
      if (v3 && visible(v3)) try { out.view3d = v3.toDataURL("image/png"); } catch (_) {}
      return out;
    } },
    workspace: { args: "name=<features 里的 id> [close=true]", desc: "打开（或关闭）一个工具面板，例如 cryoet 用 tomo、spa、sta、denoise、align、wedge、volume、figure", run: async (a) => {
      const f = await workspace(a.name, a.close === true);
      const panel = (f.ui.ids || []).find((id) => $(id) && visible($(id)));
      return { feature: f.id, panels: openPanels(), controls: panel ? await snapshotOf(panel) : [] };
    } },
    labels: { args: "", desc: "标签图层：每个标签的编号、名字、颜色、像素数、是否隐藏，当前标签和工具", run: () => {
      const L = Pro.labels || fail("这个窗口没有标签工具");
      return { ...L.state(), labels: L.used().map((k) => ({ label: k, name: L.nameOf(k), color: L.colorOf(k), pixels: L.count(k), hidden: L.hidden(k) })) };
    } },
    label_set: { args: "label=N [name=…] [color=[r,g,b]] [hidden=true|false] [active=true]", desc: "改标签的名字、颜色、显示，或设为当前标签", run: async (a) => {
      const L = Pro.labels || fail("这个窗口没有标签工具");
      if (!(a.label > 0)) fail("要给 label（从 1 起）");
      const patch = {};
      for (const k of ["name", "color", "hidden", "locked"]) if (k in a) patch[k] = a[k];
      doing(`标签 ${a.label}${patch.name ? ` 叫「${patch.name}」` : ""}`);
      if (Object.keys(patch).length) await L.setMaterial(a.label, patch);
      if (a.active) L.setActive(a.label);
      return ACTIONS.labels.run();
    } },
    label_paint: { args: "label=N z=N polygon=[[x,y],…] | stroke=[[x,y],…] radius=px [erase=true]", desc: "在第 z 层画标签（图像像素坐标）：polygon 填满多边形，stroke 沿线画；能撤销", run: async (a) => {
      const L = Pro.labels || fail("这个窗口没有标签工具");
      if (!S.info) fail("没有打开的图像");
      const z = a.z ?? S.z;
      if (z !== S.z) setZ(z);
      if (S.tool !== "label") setTool("label");
      for (let i = 0; i < 50 && !L.state().ready; i++) await sleep(100);
      const label = a.erase ? 0 : (a.label ?? L.state().active);
      const pts = a.polygon || a.stroke || fail("要给 polygon 或 stroke");
      doing(`在第 ${z} 层画标签 ${label}`);
      const op = a.polygon ? { kind: "polygon", z, pts, label, guard: 0 } : { kind: "stroke", z, pts, radius: a.radius || 6, label, guard: 0 };
      const st = await L.edit(op);
      if (!st) fail(L.state().error || "标签现在不能画（图像还在加载，或不在 XY 视图）");
      return ACTIONS.labels.run();
    } },

    // The `tmv` commands, done in the window (features.js: app = cli).
    run: { args: "path=…", desc: "打开数据，说明它是什么、接着能做什么（对应 tmv run）", run: async (a) => {
      await needPath(a);
      const i = S.info;
      const next = i.sizeZ >= 3 ? "断层或倾转序列：tomo（重构）、sta（子断层平均）、membranes（膜分割）、denoise" : "显微图或电影：spa（单颗粒）、ctf、denoise";
      return { ...status(), next };
    } },
    tomo: { args: "[path=倾转序列/.mdoc] [for=display|survey] [all=true]", desc: "Cryo-ET 面板里重构勾选的序列（对应 tmv tomo）；all=true 先全选", run: async (a) => {
      if (a.for === "sta") fail("子断层平均用 sta 动作：它自己做需要的断层图");
      if (a.path) await open(a.path);
      await workspace("tomo");
      const host = $("cryo-session");
      const use = host.querySelector(`.cs-use[data-purpose="${a.for || "display"}"]`);
      if (use && use.getAttribute("aria-checked") !== "true") await press(use);
      if (a.all) await press(host.querySelector(".cs-select-all"));
      await press(host.querySelector(".cs-finish"));
      return { ...status(), next: "用 wait 等它做完；结果在 Cryo-ET 面板右侧栏" };
    } },
    sta: { args: "[path=断层图]", desc: "子断层平均：做出候选卡片后停下（对应 tmv sta）。卡片给用户看，由用户选，再 click 那张卡片和「找出全部」", run: async (a) => {
      if (a.path) await open(a.path);
      if (Pro.sta?.show && Pro.sta.hosted) Pro.sta.show();
      else await workspace("sta");
      await press($("sta-run") || fail("没有「找候选」按钮"));
      return { ...status(), next: "wait 等卡片做好，ui in=sta-cards 看卡片，screenshot 给用户看；用户选定后 click 那张卡片，再 click text=找出全部" };
    } },
    spa: { args: "[path=显微图或电影]", desc: "单颗粒一键开始（对应 tmv spa）；找到几种颗粒后停下由用户选", run: async (a) => {
      await needPath(a);
      await workspace("spa");
      await press($("spa-a-start"));
      return { ...status(), next: "wait 等它找出颗粒种类（spa-a-kinds），给用户看，用户选定后 click 那种，再 click spa-a-go" };
    } },
    ctf: { args: "[path=显微图或电影]", desc: "拟合文件夹里全部显微图的 CTF（对应 tmv ctf）", run: async (a) => {
      await needPath(a);
      await workspace("spa");
      $("spa-advanced").open = true;
      Pro.spa.select(1);
      await press($("spa-run-ctf"));
      return { ...status(), next: "wait 等它做完" };
    } },
    membranes: { args: "[path=断层图]", desc: "分割膜、囊泡（对应 tmv membranes），结果是标签图层", run: async (a) => {
      await needPath(a);
      if ($("tools").hidden || $("tools").classList.contains("only-contrast") || $("tools").classList.contains("only-info")) showPanel("all");
      if (S.tool !== "label") setTool("label");
      $("lab-mem").open = true;
      await sleep(300);
      await press($("lab-mem-run"));
      return { ...status(), next: "wait 等它做完；没有模型时面板会说怎么装" };
    } },
    denoise: { args: "[path=…] [method=learned|nlm|…]", desc: "打开降噪面板并开始（对应 tmv denoise 的自监督降噪）；保存要用户在保存对话框里选位置", run: async (a) => {
      await needPath(a);
      await workspace("denoise");
      await setValue($("dn-method"), a.method || "learned");
      if ((a.method || "learned") === "learned") await press($("dn-train"));
      return { ...status(), controls: await snapshotOf("denoise-panel") };
    } },
    info: { args: "", desc: "版本和显卡（对应 tmv info）", run: async () => ({
      app: await T.app.getName(), version: await T.app.getVersion(),
      gpu: await invoke("gpu_info").catch((e) => String(e)), sam: await invoke("sam_status").catch(() => null),
    }) },
    setup: { args: "sam=true", desc: "下载 SAM 2.1 模型（对应 tmv setup --sam）；MemBrain-seg 模型由用户在标签面板里选", run: async (a) => {
      if (!a.sam) fail("这里只能 sam=true；MemBrain-seg 模型请用户在「标签分割 → 自动分割膜」里选自己下载的 .ckpt");
      doing("下载 SAM 2.1 模型");
      await invoke("sam_download");
      return { sam: await invoke("sam_status").catch(() => null) };
    } },
  };

  // ---------- the calls ----------

  let tail = Promise.resolve();
  thisWindow.listen("agent-call", (e) => {
    const call = e.payload;
    // One at a time, in order, like a person.
    tail = tail.then(() => handle(call)).catch(() => {});
  });

  async function handle(call) {
    let reply;
    try {
      const a = ACTIONS[call.action];
      if (!a) fail(`没有动作 ${call.action}；tmv app help 列出全部`);
      await permitted();
      if (typeof editionReady !== "undefined") await editionReady;
      const r = await a.run(call.args || {}, call);
      reply = { ok: true, action: call.action, ...(r && typeof r === "object" && !Array.isArray(r) ? r : { result: r }) };
    } catch (err) {
      reply = { ok: false, action: call.action, error: err?.message || String(err), status: statusLines() };
    }
    await invoke("agent_reply", { id: call.id, reply }).catch(() => {});
  }

  // ---------- ⚙ → AI 助手 ----------

  const SETTING = { plugin: "agent", title: "AI 助手", value: mode(), options: [["ask", "连接时先问我"], ["always", "总是允许"], ["off", "关闭"], ["setup", "怎样连接 Codex / Claude Code…"]],
    onChange(v) {
      if (v === "setup") { SETTING.value = mode(); return showSetup(); }
      pref.set("agent.allow", v);
      if (v === "off") allowed = false;
    } };
  ext.settings.push(SETTING);

  async function showSetup() {
    const info = await invoke("agent_info").catch(() => ({}));
    const q = (p) => (/\s/.test(p) ? `"${p}"` : p);
    const exe = q(info.exe || "TifMrcView Pro");
    const tmv = info.linked ? "tmv" : `${exe} tmv`;
    const box = document.createElement("div");
    box.id = "agent-setup";
    box.innerHTML = `<div><b>让 Codex / Claude Code 操作 TifMrcView Pro</b>
      <p class="dim">App 开着时，AI 助手通过本机接口（只在这台电脑上，带口令）操作这个窗口，你能看到每一步。在终端里运行一次下面的命令（二选一）：</p>
      <h4>Claude Code</h4><code></code>
      <h4>Codex</h4><code></code>
      <h4>或者直接用命令行</h4><code></code>
      <p class="dim">之后对 AI 助手说，例如：「在 TifMrcView Pro 里打开 ~/data/ts01.mrc，重构断层」「把这个断层里的膜分割出来」。</p>
      <div class="row"><button class="small" id="agent-link"></button><button class="small" id="agent-copy">复制 Claude Code 命令</button><button class="small primary" id="agent-close">好</button></div></div>`;
    const codes = box.querySelectorAll("code");
    codes[0].textContent = `claude mcp add tifmrcview -- ${exe} tmv mcp`;
    codes[1].textContent = `codex mcp add tifmrcview -- ${exe} tmv mcp`;
    codes[2].textContent = `${tmv} app status\n${tmv} app help`;
    document.body.append(box);
    const link = $("agent-link");
    if (!info.link) link.remove();
    else {
      link.textContent = info.linked ? `已安装 tmv 命令（${info.link}）` : "安装 tmv 命令";
      link.disabled = !!info.linked;
      link.title = `在 ${info.link} 放一个指向本程序的 tmv`;
      link.onclick = async () => {
        try { const p = await invoke("agent_install_tmv"); link.textContent = `已安装：${p}`; link.disabled = true; codes[2].textContent = "tmv app status\ntmv app help"; }
        catch (e) { toast(String(e), 6000); }
      };
    }
    $("agent-copy").onclick = () => navigator.clipboard.writeText(codes[0].textContent).then(() => toast("已复制")).catch(() => {});
    $("agent-close").onclick = () => box.remove();
  }
  Pro.agent = { ACTIONS, showSetup };
})();
