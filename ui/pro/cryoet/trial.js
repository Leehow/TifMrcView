"use strict";
// 「重构试验」: the reconstruction button of the quick toolbar. Lit when the
// open image is a tilt series (or a tomogram / aligned film made from one,
// tomo::trial::source); the user picks a method (WBP, SIRT, SART) and its
// settings, and each trial opens in a tab of its own, named by its settings,
// to flip between. The backend aligns the series first when it has no
// alignment and builds the tomogram with the Cryo-ET workspace's own
// reconstruction (tomo_trial → tomo::pipeline::reconstruct); `tmv recon` is
// the same on the command line.
// Shares app.js globals: S, T, invoke, $, pref, openPath, toast.
(() => {
  const W = window.TifMrcViewPro.cryoet;
  const R = { src: null, path: null, busy: false, times: new Map() };

  const btn = document.createElement("button");
  btn.id = "btn-recon";
  btn.className = "toggle";
  btn.title = "重构试验：用 WBP、SIRT、SART 等方法和不同参数快速重构当前倾转序列，每个结果在新标签页打开，方便对比（打开带倾角的倾转序列时可用）";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><path d="M4 2l5 6M12 1v7M20 2l-5 6"/><path d="M12 10l7 3.5v6.5L12 23.5 5 20v-6.5z"/><path d="M5 13.5l7 3.5 7-3.5M12 17v6.5"/></svg>';
  btn.disabled = true;
  $("btn-3d") ? $("btn-3d").before(btn) : $("quickbar").append(btn);

  const panel = document.createElement("aside");
  panel.id = "recon-panel";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head"><b>重构试验</b><button id="rt-close" class="small" title="关闭">✕</button></div>
    <p id="rt-series" class="dim"></p>
    <div class="al-opts">
      <label class="al-wide">方法<select id="rt-method">
        <option value="wbp">WBP · 加权反投影（最快）</option>
        <option value="sirt">SIRT · 同时迭代（对比度好、噪声低）</option>
        <option value="sart">SART · 逐张迭代（收敛快）</option>
      </select></label>
      <label data-for="wbp">滤波<select id="rt-filter" title="WBP 的反投影滤波：Ramp 最锐利；Cosine、Hamming 压高频噪声"><option value="ramp">Ramp</option><option value="cosine">Cosine</option><option value="hamming">Hamming</option></select></label>
      <label data-for="iter">迭代次数<input id="rt-iters" type="number" min="1" max="1000" title="SIRT 常用 10–30，SART 3–5"></label>
      <label data-for="iter">松弛因子 λ<input id="rt-lambda" type="number" min="0.05" max="2" step="0.05" title="每次更新的步长；小一些更稳，大一些收敛快"></label>
      <label>binning<select id="rt-bin"><option>1</option><option>2</option><option>4</option><option>8</option><option>16</option></select></label>
      <label>厚度（像素）<input id="rt-z" type="number" min="8" max="8192" step="8"></label>
      <label>Z 偏移（像素）<input id="rt-offset" type="number" step="1" value="0" title="样品不在中间时上下移动重构范围"></label>
      <p id="rt-size" class="dim al-wide"></p>
      <label class="al-check al-wide" data-for="iter"><input id="rt-positive" type="checkbox">非负约束</label>
      <label class="al-check al-wide" title="用已做的 CTF 估计对每张倾转图做相位翻转（在 Cryo-ET 工作区做过 CTF 后可选）"><input id="rt-flip" type="checkbox">相位翻转</label>
    </div>
    <div class="al-row">
      <button id="rt-run" class="small primary" title="按上面的设置重构，结果在新标签页打开">重构</button>
      <button id="rt-all" class="small" title="用同样的 binning 和厚度依次做 WBP、SIRT、SART，各开一个标签页">三种方法都做</button>
      <button id="rt-stop" class="small" hidden title="停下来">停止</button>
    </div>
    <p id="rt-status" class="dim" role="status"></p>
    <div id="rt-list"></div>`;
  $("stage").append(panel);

  const P = { method: "wbp", filter: "ramp", iters: { sirt: 15, sart: 3 }, lambda: { sirt: 1, sart: 0.5 }, positive: false, ...pref.get("recon-trial", {}) };
  const save = () => pref.set("recon-trial", P);
  const status = (t) => { $("rt-status").textContent = t; };
  const name = (p) => (window.TmvRemote?.displayPath(p) || p).split(/[\\/]/).pop();

  function syncMethod() {
    const m = $("rt-method").value;
    panel.querySelectorAll("[data-for=wbp]").forEach((e) => { e.hidden = m !== "wbp"; });
    panel.querySelectorAll("[data-for=iter]").forEach((e) => { e.hidden = m === "wbp"; });
    if (m !== "wbp") {
      $("rt-iters").value = P.iters[m];
      $("rt-lambda").value = P.lambda[m];
    }
  }
  function syncSize() {
    const s = R.src;
    if (!s) return;
    const bin = +$("rt-bin").value, z = +$("rt-z").value;
    const w = Math.floor(s.width / bin);
    const px = s.pixelA ? s.pixelA * bin : null;
    $("rt-size").textContent = `输出 ${w} 像素宽 × ${z} 层` + (px ? ` · ${px.toFixed(1)} Å/像素 · 厚约 ${(z * px / 10).toFixed(0)} nm` : "");
  }

  $("rt-method").value = P.method;
  $("rt-filter").value = P.filter;
  $("rt-positive").checked = !!P.positive;
  syncMethod();
  $("rt-method").onchange = () => { P.method = $("rt-method").value; save(); syncMethod(); };
  $("rt-filter").onchange = () => { P.filter = $("rt-filter").value; save(); };
  $("rt-iters").onchange = () => { P.iters[$("rt-method").value] = Math.max(1, +$("rt-iters").value || 1); save(); };
  $("rt-lambda").onchange = () => { P.lambda[$("rt-method").value] = Math.min(2, Math.max(0.05, +$("rt-lambda").value || 1)); save(); };
  $("rt-positive").onchange = () => { P.positive = $("rt-positive").checked; save(); };
  $("rt-bin").onchange = syncSize;
  $("rt-z").oninput = syncSize;

  // The series the panel works on; its settings start from the backend's
  // defaults the first time a series is shown.
  function show(src) {
    const changed = R.src?.stack !== src.stack;
    R.src = src;
    const d = src.dataset;
    $("rt-series").textContent = `倾转序列：${name(src.stack)} · ${d.angles.length} 个倾角 · ` + (d.alignment ? `已对齐（残差 ${d.alignment.residual.toFixed(2)} px）` : "还没对齐，第一次重构前会先自动对齐");
    if (changed) {
      $("rt-bin").value = src.defaults.bin;
      $("rt-z").value = src.defaults.thickness;
      $("rt-offset").value = 0;
    }
    $("rt-flip").disabled = !d.ctf;
    if (!d.ctf) $("rt-flip").checked = false;
    syncSize();
    list(d);
  }

  // What was tried on this series, newest first; a click opens it.
  function list(d) {
    const box = $("rt-list");
    box.replaceChildren();
    const trials = (d.tomograms || []).filter((t) => t.kind === "trial").reverse();
    if (!trials.length) return;
    const head = document.createElement("div");
    head.className = "dim rt-head";
    head.textContent = "已做的试验（点一下打开）";
    box.append(head);
    for (const t of trials) {
      const row = document.createElement("button");
      row.className = "rt-item";
      row.classList.toggle("on", t.path === S.info?.path);
      const time = R.times.get(t.path);
      const tag = name(t.path).replace(/\.mrc$/i, "").split("_try_")[1] || "";
      const extra = tag.split("_").slice(1).map((w) => ({ cosine: "Cosine", hamming: "Hamming", pos: "非负" }[w] || w.replace(/^off/, "偏移 ").replace(/^l(?=\d)/, "λ "))).join(" · ");
      row.innerHTML = "<b></b><span></span>";
      row.firstChild.textContent = `${t.method.toUpperCase().replace(/(\D+)(\d+)$/, "$1 × $2")}`;
      row.lastChild.textContent = [`bin ${t.bin}`, `Z ${t.thickness}`, extra, t.phaseFlipped ? "相位翻转" : "", time].filter(Boolean).join(" · ");
      row.title = t.path;
      row.onclick = () => openPath(t.path);
      box.append(row);
    }
  }

  function request(method) {
    const m = method || $("rt-method").value;
    return {
      bin: +$("rt-bin").value,
      phaseFlip: $("rt-flip").checked,
      params: {
        method: m,
        thickness: Math.max(8, Math.round(+$("rt-z").value || 8)),
        iterations: m === "wbp" ? 1 : (method ? P.iters[m] : Math.max(1, +$("rt-iters").value || 1)),
        lambda: m === "wbp" ? 1 : (method ? P.lambda[m] : +$("rt-lambda").value || 1),
        nonNegative: m !== "wbp" && $("rt-positive").checked,
        zOffset: +$("rt-offset").value || 0,
        filter: $("rt-filter").value,
        backend: "auto",
      },
    };
  }

  async function run(methods) {
    const src = R.src;
    if (!src || R.busy || W.busy) return;
    R.busy = true;
    $("rt-run").disabled = $("rt-all").disabled = true;
    $("rt-stop").hidden = false;
    let stopped = false;
    $("rt-stop").onclick = () => { stopped = true; invoke("tomo_cancel"); };
    const un = await T.event.listen("tomo-progress", (e) => { const p = e.payload; status(`${p.stage} ${p.done} / ${p.total}`); });
    try {
      for (const [i, m] of methods.entries()) {
        if (stopped) break;
        const label = methods.length > 1 ? `重构 ${i + 1}/${methods.length}（${m.toUpperCase()}）` : "重构";
        status(`${label}…`);
        const r = await W.run(label, () => invoke("tomo_trial", { req: { stack: src.stack, ...request(methods.length > 1 ? m : null) } }));
        if (!r) { status($("cryo-status")?.textContent || "没有完成"); break; }
        W.setDataset(r.dataset);
        src.dataset = r.dataset;
        const t = r.result;
        const time = `${(t.ms / 1000).toFixed(1)} s · ${t.backend}`;
        R.times.set(t.path, time);
        status(`${t.aligned ? "已自动对齐；" : ""}${t.summary} · ${time}`);
        show(src);
        // Every trial in a tab of its own (the same settings again reload theirs).
        await openPath(t.path, true);
      }
    } finally {
      un();
      R.busy = false;
      $("rt-run").disabled = $("rt-all").disabled = false;
      $("rt-stop").hidden = true;
    }
  }
  $("rt-run").onclick = () => run([$("rt-method").value]);
  $("rt-all").onclick = () => run(["wbp", "sirt", "sart"]);

  function open() {
    if (!R.src) return;
    if (typeof F !== "undefined" && F.on) setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
    panel.hidden = false;
    btn.classList.add("on");
    show(R.src);
  }
  function close() {
    panel.hidden = true;
    btn.classList.remove("on");
  }
  btn.onclick = () => (panel.hidden ? open() : close());
  $("rt-close").onclick = close;
  document.addEventListener("pro-panel", (e) => e.detail !== panel.id && !panel.hidden && close());

  // Lit only for a tilt series (or what was made from one); asked once per opened file.
  async function check() {
    const path = S.info?.path;
    if (path === R.path) return;
    R.path = path;
    // On an SSH folder the question (and every trial) goes to the server engine (remote-processing.js).
    let src = null;
    if (path && S.info.sizeZ >= 3) src = await invoke("tomo_trial_source", { path }).catch(() => null);
    if (R.path !== path) return;
    btn.disabled = !src;
    if (!src) { if (!R.busy) close(); return; }
    if (!panel.hidden || !R.src || R.src.stack !== src.stack) show(src);
    else R.src = src;
  }
  document.addEventListener("tmv-opened", check);
  document.addEventListener("tmv-folder-changed", () => { R.path = null; check(); });
  setInterval(() => { if (S.info?.path !== R.path && !R.busy) check(); }, 600);
})();
