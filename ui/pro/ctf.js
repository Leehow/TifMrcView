"use strict";
// Pro CTF fitting, inside the FFT panel: fits defocus, astigmatism and
// phase shift to the panel's power spectrum (ctf.rs) and draws the fitted
// CTF zeros over the spectrum plus the 1D fit below it.
// Shares app.js globals: S, F, T, invoke, $, pref, visibleRegion, fftChannel, metaGroups, UNIT_M, isRecip, setZ, toast.
(() => {
  const C = { busy: false, fit: null, key: "" };
  const panel = $("fft-panel");

  const box = document.createElement("section");
  box.id = "ctf-box";
  box.innerHTML = `
    <div class="ctf-head">
      <b>CTF 拟合</b>
      <button id="ctf-run" class="small primary" title="对当前 FFT 区域拟合离焦、像散（和相位板相移）">拟合</button>
      <button id="ctf-stack" class="small" title="在每一层的同一区域拟合（倾转序列、多张照片），给出离焦随倾转角 / 层的变化" hidden>全部层</button>
      <button id="ctf-opts-btn" class="small" title="显微镜参数和搜索范围">参数</button>
    </div>
    <div id="ctf-opts" hidden>
      <label>电压 kV <input type="number" id="ctf-kv" step="10" min="1" /></label>
      <label>Cs mm <input type="number" id="ctf-cs" step="0.1" min="0" /></label>
      <label>振幅衬度 <input type="number" id="ctf-amp" step="0.01" min="0" max="0.5" /></label>
      <label>像素 Å <input type="number" id="ctf-px" step="0.01" min="0" title="默认取图像的像素尺寸" /></label>
      <label>分辨率 低 Å <input type="number" id="ctf-rlo" step="1" min="1" /></label>
      <label>高 Å <input type="number" id="ctf-rhi" step="0.1" min="0.1" /></label>
      <label>离焦 最小 µm <input type="number" id="ctf-dmin" step="0.1" min="0" /></label>
      <label>最大 µm <input type="number" id="ctf-dmax" step="0.1" min="0" /></label>
      <label>最大像散 µm <input type="number" id="ctf-astig" step="0.05" min="0" /></label>
      <label class="ctf-check ctf-wide"><input type="checkbox" id="ctf-phase" /> 相位板（拟合相移）</label>
      <label class="ctf-wide">计算 <select id="ctf-backend"><option value="auto">自动（单层 GPU 算谱 + CPU 拟合，全部层都用 GPU）</option><option value="gpu">全部用 GPU</option><option value="cpu">只用 CPU</option></select></label>
    </div>
    <div id="ctf-out" class="dim">按“拟合”在当前看到的区域上拟合 CTF</div>
    <canvas id="ctf-plot" title="蓝：等相位平均的谱；红：拟合的 CTF²；绿：逐环相关系数（0–1）"></canvas>
    <div id="ctf-series" hidden>
      <div class="ctf-head"><span id="ctf-series-sum" class="dim"></span><button id="ctf-copy" class="small" title="复制每层结果（制表符分隔，可贴到 Excel）">复制</button></div>
      <canvas id="ctf-trend" title="平均离焦（线）和 Δf1/Δf2（阴影）随倾转角或层变化；空心点为相关较低的层。点一下跳到那一层"></canvas>
      <div id="ctf-table"></div>
    </div>`;
  panel.appendChild(box);

  // The zeros are drawn on a canvas laid over the spectrum.
  const img = $("fft-img");
  const wrap = document.createElement("div");
  wrap.id = "ctf-wrap";
  img.replaceWith(wrap);
  wrap.appendChild(img);
  const over = document.createElement("canvas");
  over.id = "ctf-over";
  wrap.appendChild(over);

  const DEFAULTS = { kv: 300, cs: 2.7, amp: 0.07, rlo: 30, rhi: 5, dmin: 0.3, dmax: 5, astig: 0.3, phase: false, backend: "auto" };
  const saved = pref.get("ctf-params", {});
  const P = { ...DEFAULTS, ...saved };
  const fields = { kv: "ctf-kv", cs: "ctf-cs", amp: "ctf-amp", rlo: "ctf-rlo", rhi: "ctf-rhi", dmin: "ctf-dmin", dmax: "ctf-dmax", astig: "ctf-astig" };
  for (const [k, id] of Object.entries(fields)) {
    $(id).value = P[k];
    $(id).addEventListener("change", () => save());
  }
  $("ctf-phase").checked = !!P.phase;
  $("ctf-backend").value = P.backend;
  for (const id of ["ctf-phase", "ctf-backend"]) $(id).addEventListener("change", () => save());
  function save() {
    const v = {};
    for (const [k, id] of Object.entries(fields)) v[k] = +$(id).value;
    v.phase = $("ctf-phase").checked;
    v.backend = $("ctf-backend").value;
    pref.set("ctf-params", v);
  }

  $("ctf-opts-btn").addEventListener("click", () => {
    $("ctf-opts").hidden = !$("ctf-opts").hidden;
    $("ctf-opts-btn").classList.toggle("on", !$("ctf-opts").hidden);
  });

  /** A number from the file's metadata, e.g. "300 kV" under 加速电压. */
  function metaNumber(names) {
    for (const g of metaGroups()) {
      for (const [k, v] of g.items || []) {
        if (names.some((n) => String(k).includes(n))) {
          const m = String(v).match(/[-+]?\d*\.?\d+(?:e[-+]?\d+)?/i);
          if (m) return +m[0];
        }
      }
    }
    return null;
  }

  /** Pixel size in Å from the image calibration, or null. */
  function pixelA() {
    if (!S.px || isRecip(S.px.unit) || !UNIT_M[S.px.unit]) return null;
    return (S.px.x * UNIT_M[S.px.unit]) / 1e-10;
  }

  // On a new file, take voltage, Cs and pixel size from it when it says.
  let lastToken = null;
  function fillFromFile() {
    if (!S.info || S.token === lastToken) return;
    lastToken = S.token;
    const kv = metaNumber(["加速电压"]);
    if (kv) $("ctf-kv").value = kv;
    const cs = metaNumber(["球差"]);
    if (cs) $("ctf-cs").value = cs;
    const px = pixelA();
    $("ctf-px").value = px ? +px.toPrecision(4) : "";
    clearFit();
  }

  function clearFit(msg) {
    C.fit = null;
    C.key = "";
    drawOverlay();
    drawPlot();
    $("ctf-out").textContent = msg || "按“拟合”在当前看到的区域上拟合 CTF";
  }

  function readParams(px) {
    return {
      voltageKv: +$("ctf-kv").value,
      csMm: +$("ctf-cs").value,
      ampContrast: +$("ctf-amp").value,
      pixelA: px,
      resLowA: +$("ctf-rlo").value,
      resHighA: Math.max(+$("ctf-rhi").value, 2 * px),
      dfMinA: +$("ctf-dmin").value * 1e4,
      dfMaxA: +$("ctf-dmax").value * 1e4,
      maxAstigA: +$("ctf-astig").value * 1e4,
      phase: $("ctf-phase").checked,
      backend: $("ctf-backend").value,
    };
  }

  /** The pixel size to fit with, or null after telling the user to set one. */
  function needPixel() {
    const px = +$("ctf-px").value;
    if (px > 0) return px;
    $("ctf-out").textContent = "需要像素尺寸：在参数里填像素 Å，或先给图像设定像素尺寸";
    $("ctf-opts").hidden = false;
    return null;
  }

  async function run() {
    fillFromFile();
    const region = visibleRegion();
    if (!S.info || !region || C.busy) return;
    const px = needPixel();
    if (!px) return;
    const n = +$("fft-n").value;
    const c = fftChannel();
    const params = readParams(px);
    const key = JSON.stringify([S.token, S.z, S.t, c, region, n]);
    const token = S.token;
    C.busy = true;
    $("ctf-run").disabled = true;
    $("ctf-out").textContent = "拟合中…";
    try {
      const fit = await invoke("ctf_fit", { req: { token, z: S.z, t: S.t, c, region, n, params } });
      if (token !== S.token) return;
      C.fit = fit;
      C.key = key;
      showFit();
    } catch (e) {
      if (String(e) !== "stale") clearFit("拟合失败：" + e);
    } finally {
      C.busy = false;
      $("ctf-run").disabled = false;
    }
  }

  const um = (a) => (a / 1e4).toFixed(3);

  function showFit() {
    const f = C.fit;
    if (!f) return;
    const astig = f.df1 - f.df2;
    const lines = [
      `离焦 ${um((f.df1 + f.df2) / 2)} µm（Δf1 ${um(f.df1)} / Δf2 ${um(f.df2)}）`,
      `像散 ${astig < 1 ? "0" : `${(astig / 10).toFixed(1)} nm @ ${f.angle.toFixed(1)}°`}` + ($("ctf-phase").checked ? ` · 相移 ${f.phase.toFixed(1)}°` : ""),
      `Thon 环可信到 ${f.fitRes.toFixed(2)} Å · 相关 ${f.score.toFixed(3)}`,
      `${f.backend} · ${(f.ms / 1000).toFixed(2)} s`,
    ];
    const out = $("ctf-out");
    out.textContent = "";
    for (const l of lines) {
      const d = document.createElement("div");
      d.textContent = l;
      out.appendChild(d);
    }
    out.lastChild.className = "dim";
    out.classList.remove("dim");
    drawOverlay();
    drawPlot();
  }

  /** CTF zeros on the right half of the spectrum (the left half stays bare
   * to compare), out to the fit resolution, plus the fitting band. */
  function drawOverlay() {
    const f = C.fit;
    const fresh = f && F.data && C.key && F.key === C.key && F.data.n === f.n;
    over.width = over.height = F.data ? F.data.n : 1;
    const g = over.getContext("2d");
    g.clearRect(0, 0, over.width, over.height);
    if (!fresh) return;
    const n = f.n;
    const rFit = (n * +$("ctf-px").value) / f.fitRes;
    g.lineWidth = Math.max(1, 1.5 * (n / Math.max(1, img.getBoundingClientRect().width)));
    g.strokeStyle = "rgba(46, 204, 64, 0.85)";
    // Pixels of the spectrum per screen pixel.
    const per = n / Math.max(1, img.getBoundingClientRect().width);
    let prev = 0;
    for (const ring of f.zeros) {
      const r = Math.min(...ring.map(([u, v]) => Math.hypot(u, v)));
      // Stop where rings come closer than 4 screen pixels or the fit no longer holds.
      if (r > rFit || r - prev < 4 * per) break;
      prev = r;
      g.beginPath();
      let pen = false;
      for (const [u, v] of ring) {
        if (u < 0) {
          pen = false;
          continue;
        }
        (pen ? g.lineTo : g.moveTo).call(g, n / 2 + u, n / 2 + v);
        pen = true;
      }
      g.stroke();
    }
    g.setLineDash([n / 100, n / 100]);
    g.strokeStyle = "rgba(255, 210, 0, 0.7)";
    g.beginPath();
    g.moveTo(n / 2, 0);
    g.lineTo(n / 2, n);
    g.stroke();
    for (const r of f.band) {
      g.beginPath();
      g.arc(n / 2, n / 2, r, -Math.PI / 2, Math.PI / 2);
      g.stroke();
    }
  }

  function drawPlot() {
    const c = $("ctf-plot");
    const f = C.fit;
    c.hidden = !f;
    if (!f) return;
    const dpr = window.devicePixelRatio || 1;
    const r = c.getBoundingClientRect();
    c.width = Math.max(1, Math.round(r.width * dpr));
    c.height = Math.max(1, Math.round(r.height * dpr));
    const g = c.getContext("2d");
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const W = r.width, H = r.height;
    g.clearRect(0, 0, W, H);
    const lo = Math.max(1, Math.floor(f.band[0]));
    const hi = f.epa.length - 1;
    const x = (i) => ((i - lo) / Math.max(1, hi - lo)) * W;
    const vals = [];
    for (let i = lo; i <= hi; i++) vals.push(f.epa[i], f.model[i]);
    vals.sort((a, b) => a - b);
    const vlo = vals[Math.floor(vals.length * 0.01)], vhi = vals[Math.floor(vals.length * 0.99)];
    const y = (v) => H - 4 - ((v - vlo) / Math.max(1e-6, vhi - vlo)) * (H - 8);
    const line = (arr, color, map) => {
      g.strokeStyle = color;
      g.beginPath();
      for (let i = lo; i <= hi; i++) (i === lo ? g.moveTo : g.lineTo).call(g, x(i), map(arr[i]));
      g.stroke();
    };
    g.lineWidth = 1;
    line(f.cc, "rgba(26, 127, 55, 0.8)", (v) => H - 2 - Math.max(0, Math.min(1, v)) * (H - 4));
    line(f.epa, "#0969da", y);
    line(f.model, "#cf222e", y);
    // Fit resolution and the band's outer edge.
    const n = f.n;
    const rFit = (n * +$("ctf-px").value) / f.fitRes;
    g.setLineDash([3, 3]);
    g.strokeStyle = "#57606a";
    for (const rr of [f.band[1], rFit]) {
      g.beginPath();
      g.moveTo(x(rr), 0);
      g.lineTo(x(rr), H);
      g.stroke();
    }
    g.setLineDash([]);
  }

  // ---- Every slice: tilt series and stacks of micrographs ----

  const SER = { busy: false, res: null, token: null };

  function syncStackButton() {
    const show = !!S.info && S.info.sizeZ > 1 && S.axis === "xy";
    $("ctf-stack").hidden = !show;
    if (SER.res && SER.token !== S.token) {
      SER.res = null;
      $("ctf-series").hidden = true;
    }
  }

  async function runStack() {
    if (SER.busy) {
      invoke("ctf_cancel");
      return;
    }
    fillFromFile();
    const region = visibleRegion();
    if (!S.info || !region || C.busy) return;
    const px = needPixel();
    if (!px) return;
    const token = S.token;
    const req = { token, zs: [], t: S.t, c: fftChannel(), region, n: +$("fft-n").value, params: readParams(px) };
    SER.busy = true;
    $("ctf-stack").textContent = "取消";
    $("ctf-run").disabled = true;
    $("ctf-out").textContent = `拟合全部 ${S.info.sizeZ} 层…`;
    const un = await T.event.listen("ctf-stack", (ev) => {
      const { done, total } = ev.payload;
      $("ctf-out").textContent = done < total ? `算谱 ${done} / ${total - 1} 层…` : "拟合中…";
    });
    try {
      const res = await invoke("ctf_fit_stack", { req });
      if (token !== S.token) return;
      SER.res = res;
      SER.token = token;
      showStack();
      $("ctf-out").textContent = `${res.fits.length} 层 · ${res.backend} · ${(res.ms / 1000).toFixed(1)} s`;
      $("ctf-out").classList.add("dim");
    } catch (e) {
      $("ctf-out").textContent = String(e).includes("已取消") ? "已取消" : "拟合失败：" + e;
    } finally {
      un();
      SER.busy = false;
      $("ctf-stack").textContent = "全部层";
      $("ctf-run").disabled = false;
    }
  }

  /** Per slice: [x value, label for x]. Tilt angle when known, else Z. */
  function xAxis() {
    const r = SER.res;
    return r.tilts ? { name: "倾转角 °", v: r.tilts } : { name: "层", v: r.zs.map((z) => z + 1) };
  }

  // Fits below this correlation are drawn hollow and flagged in the table.
  const LOW_CC = 0.1;

  function showStack() {
    const r = SER.res;
    $("ctf-series").hidden = !r;
    if (!r) return;
    const mean = r.fits.map((f) => (f.df1 + f.df2) / 2);
    const lo = Math.min(...mean), hi = Math.max(...mean);
    const good = r.fits.filter((f) => f.score >= LOW_CC).length;
    $("ctf-series-sum").textContent = `离焦 ${um(lo)}–${um(hi)} µm · ${good}/${r.fits.length} 层相关 ≥ ${LOW_CC}`;
    const ax = xAxis();
    const t = document.createElement("table");
    t.innerHTML = `<thead><tr><th>${r.tilts ? "倾转 °" : "层"}</th><th>离焦 µm</th><th>像散 nm</th><th>角 °</th>${$("ctf-phase").checked ? "<th>相移 °</th>" : ""}<th>可信 Å</th><th>相关</th></tr></thead>`;
    const tb = document.createElement("tbody");
    r.fits.forEach((f, i) => {
      const tr = document.createElement("tr");
      tr.dataset.z = r.zs[i];
      if (f.score < LOW_CC) tr.className = "low";
      const cells = [
        r.tilts ? ax.v[i].toFixed(1) : String(ax.v[i]),
        um(mean[i]),
        ((f.df1 - f.df2) / 10).toFixed(1),
        f.angle.toFixed(0),
        ...($("ctf-phase").checked ? [f.phase.toFixed(0)] : []),
        f.fitRes.toFixed(1),
        f.score.toFixed(3),
      ];
      for (const v of cells) {
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
    $("ctf-table").replaceChildren(t);
    drawTrend();
  }

  // Plot geometry, kept for clicks.
  let trendX = null;

  function drawTrend() {
    const c = $("ctf-trend");
    const r = SER.res;
    if (!r || $("ctf-series").hidden) return;
    const dpr = window.devicePixelRatio || 1;
    const b = c.getBoundingClientRect();
    c.width = Math.max(1, Math.round(b.width * dpr));
    c.height = Math.max(1, Math.round(b.height * dpr));
    const g = c.getContext("2d");
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const W = b.width, H = b.height, L = 34, B = 14;
    g.clearRect(0, 0, W, H);
    const xs = xAxis().v;
    const order = xs.map((_, i) => i).sort((a, b) => xs[a] - xs[b]);
    const ys = r.fits.flatMap((f) => [f.df1, f.df2]);
    let ylo = Math.min(...ys), yhi = Math.max(...ys);
    if (yhi - ylo < 100) { ylo -= 50; yhi += 50; }
    const xlo = Math.min(...xs), xhi = Math.max(...xs);
    const X = (v) => L + ((v - xlo) / Math.max(1e-9, xhi - xlo)) * (W - L - 6);
    const Y = (v) => 4 + (1 - (v - ylo) / (yhi - ylo)) * (H - B - 8);
    trendX = { X, xs };
    g.font = "10px system-ui, sans-serif";
    g.fillStyle = "#57606a";
    g.textAlign = "right";
    g.fillText(um(yhi), L - 3, 12);
    g.fillText(um(ylo), L - 3, H - B);
    g.textAlign = "center";
    g.fillText(`${xlo.toFixed(r.tilts ? 0 : 0)}`, L + 6, H - 2);
    g.fillText(xAxis().name, (L + W) / 2, H - 2);
    g.fillText(`${xhi.toFixed(0)}`, W - 10, H - 2);
    // Δf1..Δf2 band, then the mean.
    g.fillStyle = "rgba(9, 105, 218, 0.15)";
    g.beginPath();
    order.forEach((i, k) => (k ? g.lineTo : g.moveTo).call(g, X(xs[i]), Y(r.fits[i].df1)));
    [...order].reverse().forEach((i) => g.lineTo(X(xs[i]), Y(r.fits[i].df2)));
    g.fill();
    g.strokeStyle = "#0969da";
    g.lineWidth = 1.2;
    g.beginPath();
    order.forEach((i, k) => (k ? g.lineTo : g.moveTo).call(g, X(xs[i]), Y((r.fits[i].df1 + r.fits[i].df2) / 2)));
    g.stroke();
    for (const i of order) {
      const f = r.fits[i];
      g.beginPath();
      g.arc(X(xs[i]), Y((f.df1 + f.df2) / 2), r.zs[i] === S.z ? 3.5 : 2.2, 0, 2 * Math.PI);
      if (f.score < LOW_CC) {
        g.fillStyle = "#fff";
        g.fill();
        g.strokeStyle = "#cf222e";
        g.stroke();
      } else {
        g.fillStyle = r.zs[i] === S.z ? "#cf222e" : "#0969da";
        g.fill();
      }
    }
  }

  $("ctf-trend").addEventListener("click", (e) => {
    if (!trendX || !SER.res) return;
    const x = e.clientX - $("ctf-trend").getBoundingClientRect().left;
    let best = 0;
    trendX.xs.forEach((v, i) => {
      if (Math.abs(trendX.X(v) - x) < Math.abs(trendX.X(trendX.xs[best]) - x)) best = i;
    });
    setZ(SER.res.zs[best]);
  });

  $("ctf-copy").addEventListener("click", () => {
    const r = SER.res;
    if (!r) return;
    const head = ["z", ...(r.tilts ? ["tilt_deg"] : []), "df1_A", "df2_A", "angle_deg", "phase_deg", "fit_res_A", "cc"];
    const rows = r.fits.map((f, i) =>
      [r.zs[i] + 1, ...(r.tilts ? [r.tilts[i]] : []), f.df1.toFixed(1), f.df2.toFixed(1), f.angle.toFixed(2), f.phase.toFixed(2), f.fitRes.toFixed(2), f.score.toFixed(4)].join("\t"),
    );
    navigator.clipboard.writeText([head.join("\t"), ...rows].join("\n")).then(
      () => toast("已复制每层 CTF 结果"),
      (e) => toast(`复制失败：${e}`),
    );
  });

  $("ctf-stack").addEventListener("click", runStack);
  $("ctf-run").addEventListener("click", run);
  // The spectrum was redrawn: the overlay only stays while it is the same one.
  document.addEventListener("tmv-fft", () => {
    fillFromFile();
    drawOverlay();
    syncStackButton();
    drawTrend();
  });
  window.addEventListener("resize", () => {
    drawPlot();
    drawTrend();
  });
})();
