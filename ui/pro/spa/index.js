"use strict";
// The single-particle workspace (spa::project): a folder of micrographs or
// movies carried through motion correction, CTF, picking, extraction, 2D
// classification, an ab initio model and gold-standard 3D refinement. The
// project lives in <folder>/tifmrcview-spa; every step works on it by path.
// The step modules (mics.js, ctf.js, pick.js, class2d.js, initial.js,
// refine.js) fill `sections` and call `register` with their hooks:
// `show(project)` after every change, `enter()` when the step is opened.
// Shares app.js globals: S, T, F, invoke, $, setFft, openPath, saveDir.
(() => {
  // Step 6 (auto.js) is the one-click mode, shown first; 0–5 are the steps by hand.
  const STEPS = ["① 显微图", "② CTF", "③ 挑选", "④ 二维分类", "⑤ 初始模型", "⑥ 三维精修", "一键"];
  const AUTO = 6;
  const W = window.TifMrcViewPro.spa = { project: null, busy: false, current: AUTO, sections: [], buttons: [], hooks: new Map() };

  const button = document.createElement("button");
  button.id = "btn-spa";
  button.className = "toggle";
  button.title = "单颗粒工作区：运动校正、CTF、挑选、二维分类、初始模型、三维精修";
  button.innerHTML = '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3.2"/><circle cx="5" cy="6" r="2"/><circle cx="19" cy="7" r="2"/><circle cx="6" cy="18" r="2"/><circle cx="18" cy="18" r="2"/></svg><span>单颗粒</span>';
  $("quickbar").append(button);

  const panel = document.createElement("section");
  panel.id = "spa-workspace";
  panel.className = "pro-workspace";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head"><b>单颗粒重构 · Single particle</b><button id="spa-close" class="small" title="关闭">✕</button></div>
    <div class="ws-dataset">
      <span id="spa-name" class="dim">还没有工程：选择显微图或电影所在的文件夹</span>
      <button id="spa-folder" class="small">选择文件夹…</button>
      <button id="spa-here" class="small" title="用当前打开文件所在的文件夹">当前文件夹</button>
      <button id="spa-export" class="small" title="把 CTF、挑选的坐标、颗粒和三维图导出给其他软件" disabled>导出…</button>
    </div>
    <div id="spa-export-box" class="al-opts" hidden>
      <label class="al-wide">格式<select id="spa-export-format">
        <option value="relion">RELION / cryoSPARC 文件夹（STAR + 颗粒叠层 + 坐标 + 三维图）</option>
        <option value="table">表格 CSV（每个颗粒的坐标、离焦、类别、取向）</option>
      </select></label>
      <div class="al-row"><button id="spa-export-go" class="small primary">选择文件夹并导出…</button></div>
    </div>
    <nav class="ws-steps" id="spa-steps" aria-label="单颗粒步骤"></nav>
    <div class="ws-body" id="spa-body"></div>
    <div class="ws-foot"><span id="spa-status" role="status" class="dim"></span><button id="spa-cancel" class="small" hidden>取消</button></div>`;
  $("stage").append(panel);
  W.panel = panel;
  for (const [i, name] of STEPS.entries()) {
    const b = document.createElement("button");
    b.textContent = name;
    b.onclick = () => W.select(i);
    const s = document.createElement("section");
    s.hidden = true;
    if (i === AUTO) {
      b.className = "ws-auto";
      b.title = "一键模式：自动做完全部步骤，只需要从找到的几种颗粒里点出你的";
      $("spa-steps").prepend(b);
      $("spa-body").prepend(s);
    } else {
      $("spa-steps").append(b);
      $("spa-body").append(s);
    }
    W.sections.push(s);
    W.buttons.push(b);
  }

  W.register = (i, hooks) => W.hooks.set(i, hooks);
  W.status = (t) => { $("spa-status").textContent = t; };
  W.name = (p) => p?.split(/[\\/]/).pop() || "";
  W.fmt = (v, d = 1) => (v == null || !isFinite(v) ? "—" : (+v).toFixed(d));

  W.select = (i) => {
    W.current = i;
    W.sections.forEach((s, j) => {
      s.hidden = i !== j;
      const b = W.buttons[j];
      b.classList.toggle("on", i === j);
      b.setAttribute("aria-current", i === j ? "step" : "false");
    });
    if (!panel.hidden) W.hooks.get(i)?.enter?.();
  };

  // Which steps have results, shown as ✓ on the step bar.
  function marks() {
    const p = W.project;
    const done = [
      !!p && (!p.movies || p.mics.every((m) => m.path !== m.movie)),
      !!p?.mics.some((m) => m.ctf),
      !!p?.mics.some((m) => m.picks),
      !!p?.class2d,
      !!p?.initial,
      !!p?.refine,
      !!p?.refine,
    ];
    W.buttons.forEach((b, i) => { b.textContent = STEPS[i] + (done[i] ? " ✓" : ""); });
  }

  // Makes `p` the workspace's project and lets every step show it.
  W.show = (p) => {
    W.project = p;
    marks();
    $("spa-export").disabled = !p?.mics.some((m) => m.ctf);
    if (!p) return;
    $("spa-name").textContent = `${W.name(p.source)} · ${p.mics.length} 张${p.movies ? "电影" : "显微图"}`;
    for (const h of W.hooks.values()) h.show?.(p);
  };

  W.load = async (source) => {
    try {
      W.show(await invoke("spa_open", { source }));
      W.status("已打开工程");
    } catch (e) { W.status(`无法打开：${e}`); }
  };
  $("spa-folder").onclick = async () => {
    const dir = await invoke("pick_folder", { dir: S.info ? saveDir() : "" });
    if (dir) W.load(dir);
  };
  $("spa-here").onclick = () => {
    if (!S.info) return W.status("请先打开一张显微图");
    W.load(S.info.path);
  };

  // Saves the settings of step ① (and `extra`) and shows the project.
  W.settings = async (extra = {}) => {
    if (!W.project) return null;
    const p = await invoke("spa_settings", { req: { dir: W.project.dir, diameterA: +$("spa-diam").value, pixelA: +$("spa-pixel").value, ...extra } });
    W.show(p);
    return p;
  };

  // Runs a long call with progress in the status line and a cancel button;
  // the workspace's buttons are disabled meanwhile.
  W.busyRun = async (label, call) => {
    if (W.busy) return null;
    W.busy = true;
    $("spa-cancel").hidden = false;
    $("spa-cancel").onclick = () => invoke("spa_cancel");
    const buttons = [...panel.querySelectorAll(".ws-body button, .ws-dataset button, #spa-export-box button")];
    const was = buttons.map((b) => b.disabled);
    buttons.forEach((b) => { b.disabled = true; });
    const t0 = performance.now();
    const un = await T.event.listen("spa-progress", (e) => {
      const p = e.payload;
      const s = (performance.now() - t0) / 1000;
      W.status(`${p.stage} ${p.done} / ${p.total} · ${s < 120 ? s.toFixed(0) + " s" : (s / 60).toFixed(1) + " min"}`);
    });
    W.status(`${label}…`);
    try {
      const r = await call();
      W.status(`${label}完成 · ${((performance.now() - t0) / 1000).toFixed(0)} s`);
      return r;
    } catch (e) {
      W.status(`${label}失败：${e}`);
      return null;
    } finally {
      un();
      W.busy = false;
      $("spa-cancel").hidden = true;
      buttons.forEach((b, i) => { b.disabled = was[i]; });
      if (W.project) W.show(W.project);
    }
  };

  // One project step (`spa_run`): the project comes back and is shown;
  // returns the step's result (true when it has none), null on failure.
  W.run = async (label, step, params) => {
    if (!W.project) { W.status("请先选择文件夹"); return null; }
    if (W.busy) return null;
    try { await W.settings(); } catch (e) { W.status(`${e}`); return null; }
    const r = await W.busyRun(label, () => invoke("spa_run", { req: { dir: W.project.dir, step, params } }));
    if (!r) return null;
    W.show(r.project);
    return r.result ?? true;
  };

  $("spa-export").onclick = () => { $("spa-export-box").hidden = !$("spa-export-box").hidden; };
  $("spa-export-go").onclick = async () => {
    if (!W.project) return W.status("请先选择文件夹");
    const out = await invoke("pick_folder", { dir: W.project.source });
    if (!out) return;
    const files = await W.busyRun("导出", () => invoke("spa_export", { dir: W.project.dir, format: $("spa-export-format").value, out }));
    if (files) W.status(`已导出 ${files.length} 个文件：${files.length ? files[0].replace(/[\\/][^\\/]*$/, "") : out}`);
  };

  function close() {
    panel.hidden = true;
    button.classList.remove("on");
  }
  button.onclick = () => {
    if (!panel.hidden) return close();
    if (F.on) setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
    panel.hidden = false;
    button.classList.add("on");
    W.select(W.current);
  };
  $("spa-close").onclick = close;
  document.addEventListener("pro-panel", (e) => { if (e.detail !== panel.id && !panel.hidden) close(); });
  $("btn-fft").addEventListener("click", () => { if (F.on) close(); });
  W.select(AUTO);
})();
