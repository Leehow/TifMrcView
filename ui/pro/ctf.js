"use strict";
// Pro CTF fitting, inside the FFT panel: fits defocus, astigmatism and
// phase shift to the panel's power spectrum (ctf.rs) and draws the fitted
// CTF zeros over the spectrum plus the 1D fit below it.
// Shares app.js globals: S, F, invoke, $, pref, visibleRegion, fftChannel, metaGroups, UNIT_M, isRecip.
(() => {
  const C = { busy: false, fit: null, key: "" };
  const panel = $("fft-panel");

  const box = document.createElement("section");
  box.id = "ctf-box";
  box.innerHTML = `
    <div class="ctf-head">
      <b>CTF 拟合</b>
      <button id="ctf-run" class="small primary" title="对当前 FFT 区域拟合离焦、像散（和相位板相移）">拟合</button>
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
      <label class="ctf-check"><input type="checkbox" id="ctf-phase" /> 相位板（拟合相移）</label>
      <label>计算 <select id="ctf-backend"><option value="auto">自动（有 GPU 用 GPU）</option><option value="cpu">只用 CPU</option></select></label>
    </div>
    <div id="ctf-out" class="dim">按“拟合”在当前看到的区域上拟合 CTF</div>
    <canvas id="ctf-plot" title="蓝：等相位平均的谱；红：拟合的 CTF²；绿：逐环相关系数（0–1）"></canvas>`;
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

  async function run() {
    fillFromFile();
    const region = visibleRegion();
    if (!S.info || !region || C.busy) return;
    const px = +$("ctf-px").value;
    if (!(px > 0)) {
      $("ctf-out").textContent = "需要像素尺寸：在参数里填像素 Å，或先给图像设定像素尺寸";
      $("ctf-opts").hidden = false;
      return;
    }
    const n = +$("fft-n").value;
    const c = fftChannel();
    const params = {
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

  /** CTF zeros on the spectrum: solid inside the fit resolution, faint beyond. */
  function drawOverlay() {
    const f = C.fit;
    const fresh = f && F.data && C.key && F.key === C.key && F.data.n === f.n;
    over.width = over.height = F.data ? F.data.n : 1;
    const g = over.getContext("2d");
    g.clearRect(0, 0, over.width, over.height);
    if (!fresh) return;
    const n = f.n;
    const rFit = (n * +$("ctf-px").value) / f.fitRes;
    g.lineWidth = Math.max(1, n / 400);
    for (const ring of f.zeros) {
      const r = Math.hypot(ring[0][0], ring[0][1]);
      g.strokeStyle = r <= rFit ? "rgba(46, 204, 64, 0.9)" : "rgba(46, 204, 64, 0.3)";
      g.beginPath();
      ring.forEach(([u, v], i) => (i ? g.lineTo : g.moveTo).call(g, n / 2 + u, n / 2 + v));
      g.closePath();
      g.stroke();
    }
    // Fitting band.
    g.setLineDash([n / 100, n / 100]);
    g.strokeStyle = "rgba(255, 210, 0, 0.6)";
    for (const r of f.band) {
      g.beginPath();
      g.arc(n / 2, n / 2, r, 0, 2 * Math.PI);
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

  $("ctf-run").addEventListener("click", run);
  // The spectrum was redrawn: the overlay only stays while it is the same one.
  document.addEventListener("tmv-fft", () => {
    fillFromFile();
    drawOverlay();
  });
  window.addEventListener("resize", drawPlot);
})();
