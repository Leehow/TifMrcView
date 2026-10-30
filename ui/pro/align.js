"use strict";
// Pro drift / motion correction: measures how far each slice of a stack
// moved (align.rs), plots the trajectory, and saves the aligned stack or
// its sum as MRC.
// Shares app.js globals: S, T, invoke, $, pref, visibleRegion, fftChannel, setZ, toast, saveDir, setSaveDir, dirname, openPath.
(() => {
  const A = { busy: false, res: null, token: null, saved: null };

  const btn = document.createElement("button");
  btn.id = "btn-align";
  btn.className = "toggle";
  btn.title = "漂移校正：对齐电影帧、倾转序列或连续切片，保存对齐后的叠层或求和";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><rect x="3" y="3" width="11" height="11" rx="1"/><rect x="10" y="10" width="11" height="11" rx="1"/><path d="M6 18l3-3M6 18h3M6 18v-3"/></svg>';
  btn.disabled = true;
  $("btn-3d") ? $("btn-3d").before(btn) : $("quickbar").append(btn);

  const panel = document.createElement("aside");
  panel.id = "align-panel";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head">
      <b>漂移校正</b>
      <button id="al-close" class="small" title="关闭">✕</button>
    </div>
    <div class="al-opts">
      <label class="al-wide">对齐方式
        <select id="al-mode">
          <option value="movie">电影帧：每帧对齐到其余帧之和</option>
          <option value="series">倾转序列 / 连续切片：逐层对齐到相邻层</option>
        </select>
      </label>
      <label>计算尺寸 <select id="al-n" title="先缩小到这个尺寸再算位移；越大越准越慢"><option>512</option><option>1024</option><option>2048</option></select></label>
      <label>最大位移 px <input type="number" id="al-max" min="0" step="10" title="只在这个范围内找位移，0 为区域的四分之一" /></label>
      <label class="al-check al-wide"><input type="checkbox" id="al-view" /> 只用当前看到的区域算位移</label>
    </div>
    <div class="al-row">
      <button id="al-run" class="small primary">计算位移</button>
      <span id="al-status" class="dim"></span>
    </div>
    <div id="al-result" hidden>
      <canvas id="al-plot" title="漂移轨迹（像素）：从蓝到红是层的顺序，红圈是当前层；点一下跳到那一层"></canvas>
      <div id="al-sum" class="dim"></div>
      <div id="al-table"></div>
      <div class="al-row">
        <button id="al-save-sum" class="small" title="对齐后求平均，存成 32 位浮点 MRC">保存求和…</button>
        <button id="al-save-stack" class="small" title="对齐后的每一层，存成 32 位浮点 MRC">保存叠层…</button>
        <button id="al-copy" class="small" title="复制每层位移（制表符分隔，可贴到 Excel）">复制</button>
      </div>
      <div class="al-row" id="al-saved" hidden><span id="al-saved-name" class="dim"></span><button id="al-open" class="small">打开</button></div>
    </div>`;
  $("stage").appendChild(panel);

  const P = { mode: "movie", n: 1024, max: 0, view: false, ...pref.get("align-params", {}) };
  $("al-mode").value = P.mode;
  $("al-n").value = P.n;
  $("al-max").value = P.max;
  $("al-view").checked = !!P.view;
  for (const id of ["al-mode", "al-n", "al-max", "al-view"]) {
    $(id).addEventListener("change", () => {
      pref.set("align-params", { mode: $("al-mode").value, n: +$("al-n").value, max: +$("al-max").value, view: $("al-view").checked });
    });
  }

  function open() {
    panel.hidden = false;
    btn.classList.add("on");
    // The right-hand panels share one spot: one at a time.
    if (F.on) setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
    if (A.token !== S.token) reset();
    // A guess at the kind of stack, until the user picks one for this file.
    if (!A.res && S.info) {
      const name = S.info.name.toLowerCase();
      if (/\.(st|ali|tlt)$|tilt|ts_/.test(name)) $("al-mode").value = "series";
      else if (/\.(mrcs|eer|tiff?)$|movie|frames|fractions/.test(name)) $("al-mode").value = "movie";
    }
    drawPlot();
  }

  function close() {
    panel.hidden = true;
    btn.classList.remove("on");
  }

  function reset() {
    A.res = null;
    A.token = S.token;
    A.saved = null;
    $("al-result").hidden = true;
    $("al-saved").hidden = true;
    $("al-status").textContent = S.info ? `${S.info.sizeZ} 层` : "";
  }

  btn.addEventListener("click", () => (panel.hidden ? open() : close()));
  $("al-close").addEventListener("click", close);
  document.addEventListener("pro-panel", (e) => e.detail !== panel.id && !panel.hidden && close());
  $("btn-fft").addEventListener("click", () => F.on && !panel.hidden && close());

  async function withProgress(label, run) {
    const un = await T.event.listen("align-progress", (ev) => {
      const { done, total } = ev.payload;
      $("al-status").textContent = `${label} ${Math.round((100 * done) / Math.max(1, total))}%`;
    });
    A.busy = true;
    $("al-run").textContent = "取消";
    for (const id of ["al-save-sum", "al-save-stack"]) $(id).disabled = true;
    try {
      return await run();
    } finally {
      un();
      A.busy = false;
      $("al-run").textContent = "计算位移";
      for (const id of ["al-save-sum", "al-save-stack"]) $(id).disabled = false;
    }
  }

  async function estimate() {
    if (A.busy) {
      invoke("align_cancel");
      return;
    }
    if (!S.info || S.info.sizeZ < 2 || S.axis !== "xy") return;
    const token = S.token;
    const req = {
      token,
      zs: [],
      c: fftChannel(),
      t: S.t,
      region: $("al-view").checked ? visibleRegion() : null,
      mode: $("al-mode").value,
      n: +$("al-n").value,
      maxShift: +$("al-max").value || 0,
    };
    try {
      const res = await withProgress("计算中", () => invoke("align_estimate", { req }));
      if (token !== S.token) return;
      A.res = { ...res, c: req.c, t: req.t };
      A.token = token;
      $("al-status").textContent = `${res.zs.length} 层 · ${res.bin > 1 ? `缩小 ${res.bin}× 后用 ${res.n}²` : `用中间 ${res.n}²（原分辨率）`} · ${(res.ms / 1000).toFixed(1)} s`;
      show();
    } catch (e) {
      $("al-status").textContent = String(e).includes("已取消") ? "已取消" : "失败：" + e;
    }
  }

  /** A length in the image's unit, moved to a unit that reads well (4220 Å → 422 nm). */
  function physical(v, unit) {
    const m = UNIT_M[unit];
    if (!m || isRecip(unit)) return `${v.toPrecision(3)} ${unit}`;
    const u = ["Å", "nm", "µm", "mm"].filter((u) => v * m >= UNIT_M[u]).pop() || "Å";
    return `${((v * m) / UNIT_M[u]).toPrecision(3)} ${u}`;
  }

  function show() {
    const r = A.res;
    $("al-result").hidden = !r;
    if (!r) return;
    const len = r.shifts.map(([x, y]) => Math.hypot(x, y));
    let path = 0;
    for (let i = 1; i < r.shifts.length; i++) path += Math.hypot(r.shifts[i][0] - r.shifts[i - 1][0], r.shifts[i][1] - r.shifts[i - 1][1]);
    const px = S.px && S.px.unit ? ` (${physical(path * S.px.x, S.px.unit)})` : "";
    const low = r.cc.filter((c) => c < 0.05).length;
    $("al-sum").textContent = `总漂移 ${path.toFixed(1)} px${px} · 最大偏离 ${Math.max(...len).toFixed(1)} px` + (low ? ` · ${low} 层相关很低，位移可能不准` : "");
    const t = document.createElement("table");
    t.innerHTML = "<thead><tr><th>层</th><th>dx px</th><th>dy px</th><th>相关</th></tr></thead>";
    const tb = document.createElement("tbody");
    r.zs.forEach((z, i) => {
      const tr = document.createElement("tr");
      tr.dataset.z = z;
      if (r.cc[i] < 0.05) tr.className = "low";
      for (const v of [z + 1, r.shifts[i][0].toFixed(2), r.shifts[i][1].toFixed(2), r.cc[i].toFixed(3)]) {
        const td = document.createElement("td");
        td.textContent = v;
        tr.appendChild(td);
      }
      tb.appendChild(tr);
    });
    t.appendChild(tb);
    t.addEventListener("click", (e) => {
      const tr = e.target.closest("tr[data-z]");
      if (tr) setZ(+tr.dataset.z);
    });
    $("al-table").replaceChildren(t);
    drawPlot();
  }

  // Plot geometry, kept for clicks.
  let plotMap = null;

  function drawPlot() {
    const c = $("al-plot");
    const r = A.res;
    if (!r || panel.hidden || $("al-result").hidden) return;
    const dpr = window.devicePixelRatio || 1;
    const b = c.getBoundingClientRect();
    c.width = Math.max(1, Math.round(b.width * dpr));
    c.height = Math.max(1, Math.round(b.height * dpr));
    const g = c.getContext("2d");
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const W = b.width, H = b.height, pad = 12;
    g.clearRect(0, 0, W, H);
    const xs = r.shifts.map((s) => s[0]), ys = r.shifts.map((s) => s[1]);
    const cx = (Math.min(...xs) + Math.max(...xs)) / 2, cy = (Math.min(...ys) + Math.max(...ys)) / 2;
    const span = Math.max(1, Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys));
    const k = (Math.min(W, H) - 2 * pad) / span;
    // Image axes: x right, y down, same scale on both.
    const X = (v) => W / 2 + (v - cx) * k, Y = (v) => H / 2 + (v - cy) * k;
    plotMap = { X, Y };
    // Grid: a scale bar of a round number of pixels.
    const step = [0.5, 1, 2, 5, 10, 20, 50, 100, 200, 500].find((s) => s * k > 30) || 1000;
    g.strokeStyle = "#d0d7de";
    g.lineWidth = 1;
    g.beginPath();
    g.moveTo(pad, H - 6);
    g.lineTo(pad + step * k, H - 6);
    g.stroke();
    g.fillStyle = "#57606a";
    g.font = "10px system-ui, sans-serif";
    g.fillText(`${step} px`, pad + step * k + 4, H - 3);
    g.strokeStyle = "#8c959f";
    g.beginPath();
    r.shifts.forEach(([x, y], i) => (i ? g.lineTo : g.moveTo).call(g, X(x), Y(y)));
    g.stroke();
    const n = r.shifts.length;
    r.shifts.forEach(([x, y], i) => {
      const f = n > 1 ? i / (n - 1) : 0;
      g.fillStyle = `rgb(${Math.round(9 + f * 198)}, ${Math.round(105 - f * 71)}, ${Math.round(218 - f * 172)})`;
      g.beginPath();
      g.arc(X(x), Y(y), 2.4, 0, 2 * Math.PI);
      g.fill();
    });
    const cur = r.zs.indexOf(S.z);
    if (cur >= 0) {
      g.strokeStyle = "#cf222e";
      g.lineWidth = 1.5;
      g.beginPath();
      g.arc(X(r.shifts[cur][0]), Y(r.shifts[cur][1]), 5, 0, 2 * Math.PI);
      g.stroke();
    }
  }

  $("al-plot").addEventListener("click", (e) => {
    const r = A.res;
    if (!r || !plotMap) return;
    const b = $("al-plot").getBoundingClientRect();
    const x = e.clientX - b.left, y = e.clientY - b.top;
    let best = 0, bd = Infinity;
    r.shifts.forEach(([sx, sy], i) => {
      const d = Math.hypot(plotMap.X(sx) - x, plotMap.Y(sy) - y);
      if (d < bd) [best, bd] = [i, d];
    });
    setZ(r.zs[best]);
  });

  async function save(what) {
    const r = A.res;
    if (!r || A.busy || A.token !== S.token) return;
    const stem = S.info.name.replace(/\.[^.]+$/, "");
    const path = await invoke("pick_save_path", { dir: saveDir(), name: `${stem}_aligned${what === "sum" ? "_sum" : ""}.mrc`, filter: "MRC", exts: ["mrc"] });
    if (!path) return;
    setSaveDir(dirname(path));
    const req = { token: S.token, zs: r.zs, shifts: r.shifts, c: r.c, t: r.t, path, what };
    try {
      await withProgress("保存中", () => invoke("align_save", { req }));
      A.saved = path;
      $("al-status").textContent = "已保存";
      $("al-saved-name").textContent = path.split(/[\\/]/).pop();
      $("al-saved").hidden = false;
    } catch (e) {
      $("al-status").textContent = String(e).includes("已取消") ? "已取消" : "保存失败：" + e;
    }
  }

  $("al-run").addEventListener("click", estimate);
  $("al-save-sum").addEventListener("click", () => save("sum"));
  $("al-save-stack").addEventListener("click", () => save("stack"));
  $("al-open").addEventListener("click", () => A.saved && openPath(A.saved));
  $("al-copy").addEventListener("click", () => {
    const r = A.res;
    if (!r) return;
    const rows = r.zs.map((z, i) => [z + 1, r.shifts[i][0].toFixed(3), r.shifts[i][1].toFixed(3), r.cc[i].toFixed(4)].join("\t"));
    navigator.clipboard.writeText(["z\tdx_px\tdy_px\tcc", ...rows].join("\n")).then(
      () => toast("已复制每层位移"),
      (e) => toast(`复制失败：${e}`),
    );
  });

  // Enable for stacks in the XY view; forget results when another image opens.
  let lastZ = null;
  setInterval(() => {
    const ok = !!(S.info && S.info.sizeZ > 1 && S.axis === "xy");
    btn.disabled = !ok;
    if (!ok && !panel.hidden && !A.busy) close();
    if (!panel.hidden && S.token !== A.token && !A.busy && S.axis === "xy") reset();
    if (S.z !== lastZ) {
      lastZ = S.z;
      drawPlot();
    }
  }, 400);
  window.addEventListener("resize", drawPlot);
})();
