"use strict";
// Pro particle statistics: particles.rs finds the particles of the shown
// image (threshold, hole filling, watershed split of touching ones) and
// measures each; this panel shows the size distribution with a log-normal
// fit, a table of the particles and their outlines on the image, lets the
// user leave particles out and saves the table as CSV.
// Shares app.js globals: S, invoke, $, toast, ext, requestRender, toScreen, toImage, fftChannel, stem, saveDir, visibleRegion, UNIT_M, isRecip.
(() => {
  const P = { busy: false, res: null, token: null, at: [0, 0], off: new Set(), sel: 0 };

  const btn = document.createElement("button");
  btn.id = "btn-part";
  btn.className = "toggle";
  btn.title = "颗粒统计：自动找颗粒，量粒径、形状，出分布图";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><circle cx="7" cy="8" r="3.2"/><circle cx="16" cy="6.5" r="2.3"/><circle cx="15" cy="15.5" r="4"/><circle cx="6" cy="17" r="2"/></svg>';
  btn.disabled = true;
  ($("btn-4d") || $("btn-diff")) ? ($("btn-4d") || $("btn-diff")).after(btn) : $("quickbar").append(btn);

  const panel = document.createElement("aside");
  panel.id = "part-panel";
  panel.className = "pro-panel";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head">
      <b>颗粒统计</b>
      <button id="pa-close" class="small" title="关闭">✕</button>
    </div>
    <div class="al-opts">
      <label>颗粒 <select id="pa-pol">
        <option value="">自动（少的那类）</option>
        <option value="dark">比背景暗</option>
        <option value="bright">比背景亮</option>
      </select></label>
      <label title="先把图平滑，减少噪声带来的碎点">平滑 σ（像素）<input type="number" id="pa-smooth" min="0" step="0.5" value="1" /></label>
      <label title="减去这么宽的高斯模糊，去掉不均匀的背景；0 = 不去">去背景 σ（像素）<input type="number" id="pa-bg" min="0" step="5" value="0" /></label>
      <label title="比这小的颗粒不算">最小面积（像素）<input type="number" id="pa-min" min="1" step="1" value="10" /></label>
      <label class="al-check"><input type="checkbox" id="pa-split" checked /> 分开粘在一起的颗粒</label>
      <label title="越小分得越碎；距离图上的峰比相连处高出这么多像素才算单独一颗">分开灵敏度（像素）<input type="number" id="pa-h" min="0" step="0.5" value="1" /></label>
      <label class="al-check"><input type="checkbox" id="pa-fill" checked /> 填满颗粒里的洞</label>
      <label class="al-check"><input type="checkbox" id="pa-edge" checked /> 不算碰到边的颗粒</label>
      <label class="al-check"><input type="checkbox" id="pa-view" /> 只分析看到的区域</label>
    </div>
    <div class="al-row">
      <button id="pa-run" class="small primary">找颗粒</button>
      <span id="pa-status" class="dim"></span>
    </div>
    <div id="pa-result" hidden>
      <details id="pa-thr">
        <summary>阈值</summary>
        <canvas id="pa-hist" title="平滑后的灰度直方图；点一下把阈值放到这里重算"></canvas>
        <div class="al-row">
          <label>阈值 <input type="number" id="pa-t" step="any" /></label>
          <button id="pa-rethr" class="small">按此阈值重算</button>
          <button id="pa-auto" class="small" title="用 Otsu 自动阈值">自动</button>
        </div>
      </details>
      <div id="pa-sum" class="pa-sum"></div>
      <div class="al-row">
        <select id="pa-key" title="分布图画哪个量"></select>
        <label class="al-check" title="对数横轴"><input type="checkbox" id="pa-log" /> 对数</label>
      </div>
      <canvas id="pa-dist"></canvas>
      <div id="pa-fit" class="dim"></div>
      <div class="pa-table-wrap"><table id="pa-table"></table></div>
      <div class="al-row">
        <button id="pa-csv" class="small" title="每颗颗粒一行，存成 CSV（Excel 能打开）">存 CSV</button>
        <button id="pa-allon" class="small" title="把去掉的颗粒都算回来">全部算上</button>
        <label class="al-check"><input type="checkbox" id="pa-show" checked /> 图上画出</label>
      </div>
      <div class="dim">点表格里的一行或图上的一颗，它会标红；勾掉的颗粒不算进统计。</div>
    </div>`;
  $("stage").appendChild(panel);

  // Quantity: [key, label, kind] — kind "len" scales with the pixel size,
  // "area" with its square, "" has no unit.
  const KEYS = [
    ["diameter", "等效圆直径", "len"],
    ["feretMax", "最大 Feret 直径", "len"],
    ["feretMin", "最小 Feret 直径", "len"],
    ["area", "面积", "area"],
    ["perimeter", "周长", "len"],
    ["circularity", "圆度 4πA/P²", ""],
    ["aspect", "长宽比（Feret）", ""],
    ["solidity", "实心度（面积/凸包）", ""],
    ["mean", "平均灰度", ""],
  ];
  $("pa-key").innerHTML = KEYS.map(([k, t]) => `<option value="${k}">${t}</option>`).join("");

  const open = () => {
    panel.hidden = false;
    btn.classList.add("on");
    if (P.token !== S.token) reset();
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
    requestRender();
  };
  const close = () => {
    panel.hidden = true;
    btn.classList.remove("on");
    requestRender();
  };
  btn.addEventListener("click", () => (panel.hidden ? open() : close()));
  $("pa-close").addEventListener("click", close);
  document.addEventListener("pro-panel", (e) => e.detail !== panel.id && !panel.hidden && close());

  function reset() {
    P.res = null;
    P.token = S.token;
    P.off = new Set();
    P.sel = 0;
    $("pa-result").hidden = true;
    $("pa-status").textContent = "";
    requestRender();
  }

  // ---------- units ----------

  /** Metres per pixel, or null when the image has no real-space pixel size. */
  function mpp() {
    if (!S.px || isRecip(S.px.unit) || !UNIT_M[S.px.unit]) return null;
    return S.px.x * UNIT_M[S.px.unit];
  }
  /** Length unit for the results: nm below 1 µm particles, else µm. */
  function lenUnit() {
    const m = mpp();
    if (!m) return { u: "px", k: 1 };
    const typical = P.res && P.res.particles.length ? median(P.res.particles.map((q) => q.diameter)) * m : m * 10;
    if (typical < 1e-9) return { u: "Å", k: m / 1e-10 };
    if (typical < 1e-6) return { u: "nm", k: m / 1e-9 };
    if (typical < 1e-3) return { u: "µm", k: m / 1e-6 };
    return { u: "mm", k: m / 1e-3 };
  }
  function value(q, key) {
    const kind = KEYS.find((k) => k[0] === key)[2];
    const { k } = lenUnit();
    return kind === "len" ? q[key] * k : kind === "area" ? q[key] * k * k : q[key];
  }
  function unitOf(key) {
    const kind = KEYS.find((k) => k[0] === key)[2];
    const { u } = lenUnit();
    return kind === "len" ? u : kind === "area" ? `${u}²` : "";
  }

  // ---------- statistics ----------

  function median(a) {
    if (!a.length) return 0;
    const s = [...a].sort((x, y) => x - y);
    return quantile(s, 0.5);
  }
  /** Linear-interpolated quantile of a sorted array. */
  function quantile(s, q) {
    const i = (s.length - 1) * q, lo = Math.floor(i), hi = Math.ceil(i);
    return s[lo] + (s[hi] - s[lo]) * (i - lo);
  }
  function meanSd(a) {
    const n = a.length;
    const m = a.reduce((x, y) => x + y, 0) / n;
    const v = n > 1 ? a.reduce((x, y) => x + (y - m) ** 2, 0) / (n - 1) : 0;
    return [m, Math.sqrt(v)];
  }
  const fmt = (v) => (v === 0 ? "0" : Math.abs(v) >= 1000 || Math.abs(v) < 0.01 ? v.toExponential(2) : +v.toPrecision(3) + "");

  /** Distance from each centroid to its nearest neighbour, pixels. Grid
   *  buckets of about two neighbour spacings; a centroid with nothing in
   *  the 3 × 3 cells around it is checked against all. */
  function nearest(ps) {
    if (ps.length < 2) return null;
    const cell = 2 * Math.sqrt((P.res.region[2] * P.res.region[3]) / ps.length);
    const grid = new Map();
    const key = (gx, gy) => gx * 100003 + gy;
    for (const q of ps) {
      const k = key(Math.floor(q.x / cell), Math.floor(q.y / cell));
      if (!grid.has(k)) grid.set(k, []);
      grid.get(k).push(q);
    }
    return ps.map((q) => {
      const gx = Math.floor(q.x / cell), gy = Math.floor(q.y / cell);
      let best = Infinity;
      for (let dy = -1; dy <= 1; dy++)
        for (let dx = -1; dx <= 1; dx++)
          for (const o of grid.get(key(gx + dx, gy + dy)) || []) if (o !== q) best = Math.min(best, Math.hypot(o.x - q.x, o.y - q.y));
      // Only a neighbour within one cell is sure to be the nearest.
      if (best > cell) for (const o of ps) if (o !== q) best = Math.min(best, Math.hypot(o.x - q.x, o.y - q.y));
      return best;
    });
  }

  const kept = () => (P.res ? P.res.particles.filter((q) => !P.off.has(q.id)) : []);

  // ---------- run ----------

  async function run(opts = {}) {
    if (P.busy || !S.info) return;
    const token = S.token;
    const pol = $("pa-pol").value;
    const req = {
      token,
      c: fftChannel(),
      z: S.z,
      t: S.t,
      region: $("pa-view").checked ? visibleRegion() : null,
      dark: pol ? pol === "dark" : null,
      threshold: opts.threshold ?? null,
      smooth: Math.max(0, +$("pa-smooth").value || 0),
      background: Math.max(0, +$("pa-bg").value || 0),
      fillHoles: $("pa-fill").checked,
      split: $("pa-split").checked,
      splitH: Math.max(0, +$("pa-h").value || 0),
      minArea: Math.max(1, Math.round(+$("pa-min").value) || 1),
      excludeEdge: $("pa-edge").checked,
    };
    // A given threshold keeps the polarity it was set for.
    if (opts.threshold != null && P.res && req.dark == null) req.dark = P.res.dark;
    P.busy = true;
    $("pa-run").disabled = true;
    $("pa-status").textContent = "正在找…";
    try {
      const r = await invoke("particles_analyze", { req });
      if (token !== S.token) return;
      P.res = r;
      P.token = token;
      P.at = [req.z, req.t];
      P.off = new Set();
      P.sel = 0;
      const drop = [];
      if (r.droppedSmall) drop.push(`${r.droppedSmall} 颗太小`);
      if (r.droppedEdge) drop.push(`${r.droppedEdge} 颗碰边`);
      $("pa-status").textContent = `${r.particles.length} 颗 · ${(r.ms / 1000).toFixed(2)} 秒${drop.length ? `（另有 ${drop.join("、")}没算）` : ""}`;
      $("pa-t").value = +r.threshold.toPrecision(5);
      $("pa-result").hidden = false;
      const [, , rw, rh] = r.region;
      if (r.foreground > 0.35 * rw * rh && !(+$("pa-bg").value > 0)) toast("一大片都被当成了颗粒：背景可能不均匀，试试“去背景”（σ 取颗粒直径的 2–3 倍）", 6000);
      else if (!r.particles.length) toast("没找到颗粒：试试换“比背景暗/亮”、调阈值或减小最小面积");
      refresh();
    } catch (e) {
      $("pa-status").textContent = "";
      toast(`颗粒统计失败：${e}`);
    } finally {
      P.busy = false;
      $("pa-run").disabled = false;
    }
  }
  $("pa-run").addEventListener("click", () => run());
  $("pa-rethr").addEventListener("click", () => run({ threshold: +$("pa-t").value }));
  $("pa-auto").addEventListener("click", () => run());

  function refresh() {
    drawHist();
    summary();
    drawDist();
    table();
    requestRender();
  }

  // ---------- threshold histogram ----------

  function drawHist() {
    const r = P.res;
    const c = $("pa-hist");
    const W = 280, H = 70;
    c.width = W * devicePixelRatio;
    c.height = H * devicePixelRatio;
    const g = c.getContext("2d");
    g.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);
    g.clearRect(0, 0, W, H);
    const lg = r.hist.map((n) => Math.log1p(n));
    const top = Math.max(...lg) || 1;
    g.fillStyle = "#8c959f";
    lg.forEach((v, i) => g.fillRect((i / lg.length) * W, H - (v / top) * (H - 4), W / lg.length + 0.5, (v / top) * (H - 4)));
    const tx = ((r.threshold - r.min) / (r.max - r.min)) * W;
    g.fillStyle = "rgba(9,105,218,0.15)";
    if (r.dark) g.fillRect(0, 0, tx, H);
    else g.fillRect(tx, 0, W - tx, H);
    g.strokeStyle = "#cf222e";
    g.lineWidth = 1.5;
    g.beginPath();
    g.moveTo(tx, 0);
    g.lineTo(tx, H);
    g.stroke();
  }
  $("pa-hist").addEventListener("click", (e) => {
    const r = P.res;
    if (!r) return;
    const b = $("pa-hist").getBoundingClientRect();
    const t = r.min + ((e.clientX - b.left) / b.width) * (r.max - r.min);
    $("pa-t").value = +t.toPrecision(5);
    run({ threshold: t });
  });

  // ---------- summary ----------

  function summary() {
    const ps = kept();
    const el = $("pa-sum");
    if (!ps.length) {
      el.innerHTML = "<div>没有颗粒</div>";
      return;
    }
    const { u, k } = lenUnit();
    const d = ps.map((q) => q.diameter * k).sort((a, b) => a - b);
    const [m, sd] = meanSd(d);
    const areaPx = P.res.region[2] * P.res.region[3];
    const frac = ps.reduce((a, q) => a + q.area, 0) / areaPx;
    const nn = nearest(ps);
    const rows = [
      ["颗粒数", `${ps.length}${P.off.size ? `（去掉 ${P.off.size}）` : ""}`],
      [`等效直径（${u}）`, `${fmt(m)} ± ${fmt(sd)}（中位 ${fmt(quantile(d, 0.5))}）`],
      ["D10 / D50 / D90", `${fmt(quantile(d, 0.1))} / ${fmt(quantile(d, 0.5))} / ${fmt(quantile(d, 0.9))} ${u}`],
      ["范围", `${fmt(d[0])} – ${fmt(d[d.length - 1])} ${u}`],
      ["面积占比", `${(frac * 100).toFixed(1)}%`],
    ];
    if (mpp()) {
      const um2 = areaPx * (mpp() / 1e-6) ** 2;
      const per = ps.length / um2;
      rows.push(["数密度", per >= 1e4 ? `${fmt(per / 1e6)} /nm²` : per >= 0.1 ? `${fmt(per)} /µm²` : `${fmt(per * 1e6)} /mm²`]);
    }
    if (nn && nn.length) rows.push([`最近邻距离（${u}）`, `${fmt(meanSd(nn.map((v) => v * k))[0])}`]);
    const circ = meanSd(ps.map((q) => q.circularity))[0];
    const asp = meanSd(ps.map((q) => q.aspect))[0];
    rows.push(["平均圆度 / 长宽比", `${circ.toFixed(2)} / ${asp.toFixed(2)}`]);
    if (!mpp()) rows.push(["", "没有像素尺寸，长度按像素算"]);
    el.innerHTML = rows.map(([a, b]) => `<div><span>${a}</span><b>${b}</b></div>`).join("");
  }

  // ---------- distribution ----------

  /** Log-normal fit by the moments of ln x: [μ, σ] of ln x. */
  function logNormal(v) {
    const l = v.filter((x) => x > 0).map(Math.log);
    if (l.length < 3) return null;
    return meanSd(l);
  }

  function drawDist() {
    const c = $("pa-dist");
    const W = 280, H = 150, L = 34, B = 26, T = 8, R = 16;
    c.width = W * devicePixelRatio;
    c.height = H * devicePixelRatio;
    const g = c.getContext("2d");
    g.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);
    g.clearRect(0, 0, W, H);
    const key = $("pa-key").value;
    const v = kept().map((q) => value(q, key)).filter(Number.isFinite);
    $("pa-fit").textContent = "";
    if (v.length < 2) return;
    const log = $("pa-log").checked && v.every((x) => x > 0);
    const tr = log ? Math.log10 : (x) => x;
    const tv = v.map(tr).sort((a, b) => a - b);
    let lo = tv[0], hi = tv[tv.length - 1];
    if (!(hi > lo)) hi = lo + 1;
    // Freedman–Diaconis bin width, 5 to 40 bins.
    const iqr = quantile(tv, 0.75) - quantile(tv, 0.25);
    let nb = iqr > 0 ? Math.ceil((hi - lo) / (2 * iqr * Math.cbrt(1 / tv.length))) : 10;
    nb = Math.max(5, Math.min(40, nb));
    const bw = (hi - lo) / nb;
    const counts = new Array(nb).fill(0);
    for (const x of tv) counts[Math.min(nb - 1, Math.floor((x - lo) / bw))]++;
    const top = Math.max(...counts);
    const pw = W - L - R, ph = H - T - B;
    const X = (x) => L + ((x - lo) / (hi - lo)) * pw;
    const Y = (n) => T + ph - (n / top) * ph;
    g.fillStyle = "#54aeff";
    g.strokeStyle = "#0969da";
    counts.forEach((n, i) => {
      if (!n) return;
      const x0 = X(lo + i * bw), x1 = X(lo + (i + 1) * bw);
      g.fillRect(x0, Y(n), x1 - x0 - 1, T + ph - Y(n));
    });
    // Axes and ticks.
    g.strokeStyle = "#8c959f";
    g.lineWidth = 1;
    g.beginPath();
    g.moveTo(L, T);
    g.lineTo(L, T + ph);
    g.lineTo(L + pw, T + ph);
    g.stroke();
    g.fillStyle = "#57606a";
    g.font = "10px system-ui, sans-serif";
    g.textAlign = "center";
    for (let i = 0; i <= 4; i++) {
      const x = lo + ((hi - lo) * i) / 4;
      g.fillText(fmt(log ? 10 ** x : x), X(x), T + ph + 12);
    }
    const unit = unitOf(key);
    g.fillText(`${KEYS.find((k) => k[0] === key)[1]}${unit ? `（${unit}）` : ""}`, L + pw / 2, H - 2);
    g.textAlign = "right";
    g.fillText(String(top), L - 4, T + 8);
    g.fillText("0", L - 4, T + ph);
    // Log-normal fit for the sizes.
    const kind = KEYS.find((k) => k[0] === key)[2];
    if (kind) {
      const f = logNormal(v);
      if (f) {
        const [mu, s] = f;
        const n = v.length;
        g.strokeStyle = "#cf222e";
        g.lineWidth = 1.5;
        g.beginPath();
        for (let i = 0; i <= 120; i++) {
          const t = lo + ((hi - lo) * i) / 120;
          const x = log ? 10 ** t : t;
          if (x <= 0) continue;
          // Expected count per bin: n · pdf · bin width (in the plotted variable).
          const pdf = Math.exp(-((Math.log(x) - mu) ** 2) / (2 * s * s)) / (s * Math.sqrt(2 * Math.PI));
          const dens = log ? pdf * Math.LN10 : pdf / x;
          const y = Y(n * dens * bw);
          i ? g.lineTo(X(t), y) : g.moveTo(X(t), y);
        }
        g.stroke();
        $("pa-fit").textContent = `对数正态拟合：几何平均 ${fmt(Math.exp(mu))} ${unit}，几何标准差 σg = ${Math.exp(s).toFixed(3)}（众数 ${fmt(Math.exp(mu - s * s))}）`;
      }
    }
  }
  $("pa-key").addEventListener("change", drawDist);
  $("pa-log").addEventListener("change", drawDist);

  // ---------- table ----------

  function table() {
    const r = P.res;
    const { u } = lenUnit();
    const ps = r.particles.slice(0, 5000);
    const head = `<tr><th></th><th>#</th><th>直径 ${u}</th><th>Feret ${u}</th><th>圆度</th><th>长宽比</th></tr>`;
    const rows = ps
      .map((q) => {
        const on = !P.off.has(q.id);
        return `<tr data-id="${q.id}" class="${q.id === P.sel ? "sel" : ""}${on ? "" : " off"}"><td><input type="checkbox" ${on ? "checked" : ""} /></td><td>${q.id}</td><td>${fmt(value(q, "diameter"))}</td><td>${fmt(value(q, "feretMax"))}</td><td>${q.circularity.toFixed(2)}</td><td>${q.aspect.toFixed(2)}</td></tr>`;
      })
      .join("");
    $("pa-table").innerHTML = head + rows + (r.particles.length > ps.length ? `<tr><td colspan="6" class="dim">表里只列前 ${ps.length} 颗；统计和 CSV 包括全部</td></tr>` : "");
  }
  $("pa-table").addEventListener("click", (e) => {
    const tr = e.target.closest("tr[data-id]");
    if (!tr) return;
    const id = +tr.dataset.id;
    if (e.target.matches("input[type=checkbox]")) {
      e.target.checked ? P.off.delete(id) : P.off.add(id);
      tr.classList.toggle("off", !e.target.checked);
      summary();
      drawDist();
    } else {
      select(id);
    }
    requestRender();
  });
  $("pa-allon").addEventListener("click", () => {
    P.off.clear();
    refresh();
  });
  $("pa-show").addEventListener("change", requestRender);

  function select(id) {
    P.sel = P.sel === id ? 0 : id;
    for (const tr of $("pa-table").querySelectorAll("tr[data-id]")) tr.classList.toggle("sel", +tr.dataset.id === P.sel);
    // Scroll the table only, not the panel around it.
    const row = $("pa-table").querySelector(`tr[data-id="${P.sel}"]`);
    const wrap = $("pa-table").parentElement;
    if (row && (row.offsetTop < wrap.scrollTop + 20 || row.offsetTop > wrap.scrollTop + wrap.clientHeight - 20)) wrap.scrollTop = row.offsetTop - wrap.clientHeight / 2;
  }

  // A click (not a drag) on a particle in the main view selects it.
  let down = null;
  $("view").addEventListener("pointerdown", (e) => (down = [e.clientX, e.clientY]));
  $("view").addEventListener("click", (e) => {
    if (panel.hidden || !shown() || !down || Math.hypot(e.clientX - down[0], e.clientY - down[1]) > 4) return;
    const [x, y] = toImage(e);
    const q = P.res.particles.find((q) => x >= q.bbox[0] && y >= q.bbox[1] && x < q.bbox[0] + q.bbox[2] && y < q.bbox[1] + q.bbox[3] && insidePoly(q.outline, x, y));
    if (q) {
      select(q.id);
      requestRender();
    }
  });
  function insidePoly(o, x, y) {
    let c = false;
    for (let i = 0, j = o.length - 2; i < o.length; j = i, i += 2) {
      if (o[i + 1] > y !== o[j + 1] > y && x < ((o[j] - o[i]) * (y - o[i + 1])) / (o[j + 1] - o[i + 1]) + o[i]) c = !c;
    }
    return c;
  }

  // ---------- CSV ----------

  $("pa-csv").addEventListener("click", async () => {
    const r = P.res;
    if (!r) return;
    const { u, k } = lenUnit();
    const cols = [
      ["id", (q) => q.id],
      ["included", (q) => (P.off.has(q.id) ? 0 : 1)],
      ["x_px", (q) => q.x.toFixed(2)],
      ["y_px", (q) => q.y.toFixed(2)],
      [`area_${u}2`, (q) => +(q.area * k * k).toPrecision(6)],
      [`diameter_${u}`, (q) => +(q.diameter * k).toPrecision(6)],
      [`perimeter_${u}`, (q) => +(q.perimeter * k).toPrecision(6)],
      [`feret_max_${u}`, (q) => +(q.feretMax * k).toPrecision(6)],
      [`feret_min_${u}`, (q) => +(q.feretMin * k).toPrecision(6)],
      ["feret_angle_deg", (q) => q.feretAngle.toFixed(1)],
      [`ellipse_major_${u}`, (q) => +(q.major * k).toPrecision(6)],
      [`ellipse_minor_${u}`, (q) => +(q.minor * k).toPrecision(6)],
      ["ellipse_angle_deg", (q) => q.angle.toFixed(1)],
      ["circularity", (q) => q.circularity.toFixed(4)],
      ["aspect_ratio", (q) => q.aspect.toFixed(4)],
      ["solidity", (q) => q.solidity.toFixed(4)],
      ["mean_intensity", (q) => +q.mean.toPrecision(6)],
    ];
    const text = [cols.map((c) => c[0]).join(","), ...r.particles.map((q) => cols.map((c) => c[1](q)).join(","))].join("\r\n") + "\r\n";
    const name = `${stem(S.info.path)}_particles.csv`;
    const path = await invoke("pick_save_path", { dir: saveDir(), name, filter: "CSV", exts: ["csv"] }).catch(() => null);
    if (!path) return;
    try {
      await invoke("save_file", new TextEncoder().encode("﻿" + text), { headers: { "x-path": encodeURIComponent(path) } });
      toast("已保存");
    } catch (e) {
      toast(`保存失败：${e}`);
    }
  });

  // ---------- overlay ----------

  const shown = () => !!(P.res && P.token === S.token && S.axis === "xy" && P.at[0] === S.z && P.at[1] === S.t);

  ext.overlays.push((ctx) => {
    if (panel.hidden || !shown() || !$("pa-show").checked) return;
    const [ox, oy] = toScreen(0, 0), [sx] = toScreen(1, 0);
    const s = sx - ox;
    ctx.save();
    ctx.lineJoin = "round";
    const path = (o) => {
      ctx.beginPath();
      for (let i = 0; i < o.length; i += 2) {
        const x = ox + o[i] * s, y = oy + o[i + 1] * s;
        i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
      }
      ctx.closePath();
    };
    ctx.lineWidth = 1.2;
    ctx.setLineDash([]);
    ctx.strokeStyle = "#2da44e";
    for (const q of P.res.particles) {
      if (P.off.has(q.id) || q.id === P.sel) continue;
      path(q.outline);
      ctx.stroke();
    }
    ctx.setLineDash([3, 3]);
    ctx.strokeStyle = "rgba(140,149,159,0.9)";
    for (const q of P.res.particles) {
      if (!P.off.has(q.id) || q.id === P.sel) continue;
      path(q.outline);
      ctx.stroke();
    }
    ctx.setLineDash([]);
    const q = P.res.particles.find((q) => q.id === P.sel);
    if (q) {
      ctx.lineWidth = 2.2;
      ctx.strokeStyle = "#cf222e";
      path(q.outline);
      ctx.stroke();
    }
    // Numbers once particles are big enough on screen.
    if (P.res.particles.length && median(P.res.particles.slice(0, 200).map((q) => q.diameter)) * s > 22) {
      ctx.font = "11px system-ui, sans-serif";
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.lineWidth = 3;
      ctx.strokeStyle = "rgba(255,255,255,0.85)";
      ctx.fillStyle = "#1f2328";
      for (const p of P.res.particles) {
        const x = ox + p.x * s, y = oy + p.y * s;
        ctx.strokeText(p.id, x, y);
        ctx.fillText(p.id, x, y);
      }
    }
    // The analysed area when it is not the whole image.
    const [rx, ry, rw, rh] = P.res.region;
    if (rw < S.info.width || rh < S.info.height) {
      ctx.setLineDash([6, 4]);
      ctx.lineWidth = 1;
      ctx.strokeStyle = "#0969da";
      ctx.strokeRect(ox + rx * s, oy + ry * s, rw * s, rh * s);
    }
    ctx.restore();
  });

  setInterval(() => {
    const ok = !!(S.info && !S.info.isRgb);
    btn.disabled = !ok;
    if (!ok && !panel.hidden && !P.busy) close();
    if (!panel.hidden && S.token !== P.token && !P.busy) reset();
  }, 400);
})();
