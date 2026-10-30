"use strict";
// Pro 4D-STEM: the stack of an open file is read as one diffraction pattern
// per probe position. stem4d.rs builds virtual bright-field / dark-field
// images, centre-of-mass (DPC) maps, iDPC and dDPC in one pass; this panel
// shows them, draws the detectors on the pattern in the main view, and a
// click on a map shows the pattern of that probe position.
// Shares app.js globals: S, invoke, $, toast, ext, requestRender, toScreen, setZ, setT, F, setFft, fftChannel, stem, saveDir.
(() => {
  const M = { busy: false, res: null, token: null, map: "adf", pick: null, scan: null, dets: null };

  const btn = document.createElement("button");
  btn.id = "btn-4d";
  btn.className = "toggle";
  btn.title = "4D-STEM：虚拟明场 / 暗场像、质心（DPC）、iDPC、dDPC";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><rect x="3" y="3" width="8" height="8" rx="1"/><rect x="13" y="3" width="8" height="8" rx="1"/><rect x="3" y="13" width="8" height="8" rx="1"/><circle cx="17" cy="17" r="3.2"/><circle cx="17" cy="17" r="0.8"/></svg>';
  btn.disabled = true;
  $("btn-diff") ? $("btn-diff").after(btn) : $("quickbar").append(btn);

  const panel = document.createElement("aside");
  panel.id = "stem-panel";
  panel.className = "pro-panel";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head">
      <b>4D-STEM</b>
      <button id="st-close" class="small" title="关闭">✕</button>
    </div>
    <div class="al-opts">
      <label>扫描宽（点）<input type="number" id="st-sx" min="1" step="1" title="每行的探针位置数" /></label>
      <label>扫描高（行）<input type="number" id="st-sy" min="1" step="1" /></label>
    </div>
    <div class="al-row">
      <button id="st-run" class="small primary">分析</button>
      <span id="st-status" class="dim"></span>
    </div>
    <div id="st-result" hidden>
      <div class="al-row">
        <select id="st-map" title="显示哪张图"></select>
        <button id="st-save" class="small" title="把这张图存成 32 位 TIFF">保存</button>
      </div>
      <canvas id="st-canvas" title="点一下看这个位置的衍射图"></canvas>
      <div id="st-pick" class="dim"></div>
      <details id="st-det" open>
        <summary>探测器</summary>
        <canvas id="st-dp" title="平均衍射图（对数）；点一下把中心放到这里"></canvas>
        <div class="al-opts st-det3">
          <label>中心 x <input type="number" id="st-cx" step="0.5" /></label>
          <label>中心 y <input type="number" id="st-cy" step="0.5" /></label>
          <label>明场半径 <input type="number" id="st-rbf" min="0" step="0.5" /></label>
          <label>暗场内径 <input type="number" id="st-r0" min="0" step="0.5" /></label>
          <label>暗场外径 <input type="number" id="st-r1" min="0" step="0.5" /></label>
          <label class="al-check"><input type="checkbox" id="st-show" checked /> 图上画出</label>
        </div>
        <div class="al-row"><button id="st-again" class="small">按这些探测器重算</button></div>
      </details>
      <details id="st-rot">
        <summary>扫描与探测器的夹角</summary>
        <div class="al-opts">
          <label>转角 ° <input type="number" id="st-angle" step="0.5" /></label>
          <label class="al-check"><input type="checkbox" id="st-flip" /> 镜像</label>
        </div>
        <div class="al-row">
          <button id="st-180" class="small" title="iDPC 原子暗、背景亮时用">+180°</button>
          <button id="st-rerot" class="small">按此角度重算</button>
        </div>
        <div id="st-rotnote" class="dim"></div>
      </details>
    </div>`;
  $("stage").appendChild(panel);

  const MAPS = [
    ["bf", "虚拟明场 BF"],
    ["adf", "虚拟环形暗场 ADF"],
    ["idpc", "iDPC（投影势）"],
    ["ddpc", "dDPC（电荷密度）"],
    ["dpc", "DPC 方向和大小（色轮）"],
    ["comX", "质心偏移 x"],
    ["comY", "质心偏移 y"],
  ];
  $("st-map").innerHTML = MAPS.map(([v, t]) => `<option value="${v}">${t}</option>`).join("");

  const open = () => {
    panel.hidden = false;
    btn.classList.add("on");
    if (M.token !== S.token) reset();
    if (F.on) setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
  };
  const close = () => {
    panel.hidden = true;
    btn.classList.remove("on");
    requestRender();
  };
  btn.addEventListener("click", () => (panel.hidden ? open() : close()));
  $("st-close").addEventListener("click", close);
  document.addEventListener("pro-panel", (e) => e.detail !== panel.id && !panel.hidden && close());
  $("btn-fft").addEventListener("click", () => F.on && !panel.hidden && close());

  /** Scan shape from the file: Z × T when both are there, else the squarest factorisation. */
  function guessScan() {
    const { sizeZ: z, sizeT: t } = S.info;
    if (z > 1 && t > 1) return [z, t];
    const n = z * t;
    let best = [n, 1];
    for (let a = 1; a * a <= n; a++) if (n % a === 0) best = [n / a, a];
    return best;
  }

  function reset() {
    M.res = null;
    M.token = S.token;
    M.pick = null;
    M.dets = null;
    $("st-result").hidden = true;
    $("st-status").textContent = "";
    if (S.info) {
      const [sx, sy] = guessScan();
      $("st-sx").value = sx;
      $("st-sy").value = sy;
    }
    requestRender();
  }

  function detsFromInputs() {
    const cx = +$("st-cx").value, cy = +$("st-cy").value;
    return [
      { kind: "annulus", cx, cy, r0: 0, r1: +$("st-rbf").value },
      { kind: "annulus", cx, cy, r0: +$("st-r0").value, r1: +$("st-r1").value },
    ];
  }

  async function run(opts = {}) {
    if (M.busy || !S.info) return;
    const sx = Math.round(+$("st-sx").value), sy = Math.round(+$("st-sy").value);
    const n = S.info.sizeZ * S.info.sizeT;
    if (!(sx > 0 && sy > 0)) return toast("先填扫描尺寸");
    if (sx * sy > n) return toast(`扫描 ${sx} × ${sy} = ${sx * sy} 点，但文件只有 ${n} 张衍射图`);
    const token = S.token;
    const req = { token, c: fftChannel(), scan: [sx, sy] };
    if (opts.dets) req.detectors = opts.dets;
    else if (M.res && M.token === token) req.detectors = M.res.detectors;
    if (opts.rot) {
      req.rotation = opts.rot[0];
      req.flip = opts.rot[1];
    }
    M.busy = true;
    $("st-run").disabled = true;
    $("st-status").textContent = `读取 ${sx * sy} 张衍射图…`;
    try {
      const r = await invoke("stem4d_analyze", { req });
      if (token !== S.token) return;
      M.res = r;
      M.token = token;
      M.scan = [sx, sy];
      $("st-status").textContent = `${(r.ms / 1000).toFixed(1)} 秒${r.gpu ? " · GPU" : ""}`;
      $("st-result").hidden = false;
      const [bf, adf] = r.detectors;
      $("st-cx").value = +bf.cx.toFixed(1);
      $("st-cy").value = +bf.cy.toFixed(1);
      $("st-rbf").value = +bf.r1.toFixed(1);
      if (adf && adf.kind === "annulus") {
        $("st-r0").value = +adf.r0.toFixed(1);
        $("st-r1").value = +adf.r1.toFixed(1);
      }
      $("st-angle").value = +r.rotation.toFixed(2);
      $("st-flip").checked = r.flip;
      $("st-rotnote").textContent = r.rotationFound
        ? `自动找到：转 ${r.rotation.toFixed(1)}°${r.flip ? "，镜像" : ""}（使质心场无旋）。如果 iDPC 里原子是暗的，点 +180°。`
        : "用的是你给的角度。";
      drawMap();
      drawDp();
      requestRender();
    } catch (e) {
      $("st-status").textContent = "";
      toast(`4D-STEM 分析失败：${e}`);
    } finally {
      M.busy = false;
      $("st-run").disabled = false;
    }
  }
  $("st-run").addEventListener("click", () => run({ dets: null }));
  $("st-again").addEventListener("click", () => run({ dets: detsFromInputs() }));
  $("st-rerot").addEventListener("click", () => run({ rot: [+$("st-angle").value, $("st-flip").checked] }));
  $("st-180").addEventListener("click", () => {
    $("st-angle").value = ((+$("st-angle").value + 180) % 360).toFixed(2);
    run({ rot: [+$("st-angle").value, $("st-flip").checked] });
  });
  $("st-show").addEventListener("change", requestRender);
  for (const id of ["st-cx", "st-cy", "st-rbf", "st-r0", "st-r1"]) $(id).addEventListener("input", () => (drawDp(), requestRender()));

  /** Values of the chosen map. */
  function mapValues(key) {
    const r = M.res;
    if (key === "bf") return r.images[0];
    if (key === "adf") return r.images[1];
    return r[key];
  }

  function stretch(v) {
    const s = Float32Array.from(v).sort();
    const lo = s[Math.floor(s.length * 0.005)], hi = s[Math.ceil(s.length * 0.995) - 1];
    return [lo, hi > lo ? hi : lo + 1];
  }

  /** HSV → RGB, h in turns. */
  function hsv(h, s, v) {
    const i = Math.floor(h * 6), f = h * 6 - i;
    const p = v * (1 - s), q = v * (1 - f * s), t = v * (1 - (1 - f) * s);
    return [[v, t, p], [q, v, p], [p, v, t], [p, q, v], [t, p, v], [v, p, q]][((i % 6) + 6) % 6];
  }

  function drawMap() {
    const r = M.res;
    if (!r) return;
    const [sx, sy] = r.scan;
    const c = $("st-canvas");
    c.width = sx;
    c.height = sy;
    const g = c.getContext("2d");
    const img = g.createImageData(sx, sy);
    const key = $("st-map").value;
    if (key === "dpc") {
      const mag = r.comX.map((x, i) => Math.hypot(x, r.comY[i]));
      const [, hi] = stretch(mag);
      for (let i = 0; i < sx * sy; i++) {
        const [R, G, B] = hsv(Math.atan2(r.comY[i], r.comX[i]) / (2 * Math.PI) + 0.5, 1, Math.min(1, mag[i] / hi));
        img.data.set([R * 255, G * 255, B * 255, 255], i * 4);
      }
    } else {
      const v = mapValues(key);
      const [lo, hi] = stretch(v);
      for (let i = 0; i < sx * sy; i++) {
        const q = Math.max(0, Math.min(255, ((v[i] - lo) / (hi - lo)) * 255));
        img.data.set([q, q, q, 255], i * 4);
      }
    }
    g.putImageData(img, 0, 0);
    if (M.pick) {
      g.strokeStyle = "#cf222e";
      g.lineWidth = Math.max(1, sx / 128);
      const [x, y] = M.pick;
      g.strokeRect(x - 1.5, y - 1.5, 4, 4);
    }
  }
  $("st-map").addEventListener("change", () => {
    M.map = $("st-map").value;
    drawMap();
  });

  function drawDp() {
    const r = M.res;
    if (!r) return;
    const [w, h] = r.det;
    const c = $("st-dp");
    c.width = w;
    c.height = h;
    const g = c.getContext("2d");
    const img = g.createImageData(w, h);
    const v = r.meanDp.map((a) => Math.log1p(Math.max(0, a)));
    const [lo, hi] = stretch(v);
    for (let i = 0; i < w * h; i++) {
      const q = Math.max(0, Math.min(255, ((v[i] - lo) / (hi - lo)) * 255));
      img.data.set([q, q, q, 255], i * 4);
    }
    g.putImageData(img, 0, 0);
    drawDets(g, (x, y) => [x, y], 1, Math.max(1, w / 160));
  }

  /** Detector outlines through `map` (pattern px → canvas px). */
  function drawDets(g, map, scale, lw) {
    const d = detsFromInputs();
    g.save();
    g.lineWidth = lw;
    g.setLineDash([]);
    const [cx, cy] = map(d[0].cx, d[0].cy);
    const ring = (r, color) => {
      if (!(r > 0)) return;
      g.strokeStyle = color;
      g.beginPath();
      g.arc(cx, cy, r * scale, 0, 2 * Math.PI);
      g.stroke();
    };
    ring(d[0].r1, "#2da44e");
    ring(d[1].r0, "#d4a72c");
    ring(d[1].r1, "#d4a72c");
    g.strokeStyle = "#cf222e";
    g.beginPath();
    g.moveTo(cx - 4 * lw, cy);
    g.lineTo(cx + 4 * lw, cy);
    g.moveTo(cx, cy - 4 * lw);
    g.lineTo(cx, cy + 4 * lw);
    g.stroke();
    g.restore();
  }

  $("st-dp").addEventListener("click", (e) => {
    if (!M.res) return;
    const c = $("st-dp"), b = c.getBoundingClientRect();
    $("st-cx").value = (((e.clientX - b.left) / b.width) * c.width).toFixed(1);
    $("st-cy").value = (((e.clientY - b.top) / b.height) * c.height).toFixed(1);
    drawDp();
    requestRender();
  });

  // A click on the map shows that probe position's pattern in the main view.
  $("st-canvas").addEventListener("click", (e) => {
    const r = M.res;
    if (!r || M.token !== S.token) return;
    const c = $("st-canvas"), b = c.getBoundingClientRect();
    const x = Math.floor(((e.clientX - b.left) / b.width) * r.scan[0]);
    const y = Math.floor(((e.clientY - b.top) / b.height) * r.scan[1]);
    if (x < 0 || y < 0 || x >= r.scan[0] || y >= r.scan[1]) return;
    M.pick = [x, y];
    const i = y * r.scan[0] + x;
    const z = i % S.info.sizeZ, t = Math.floor(i / S.info.sizeZ);
    if (S.info.sizeT > 1) setT(t);
    setZ(z);
    const v = mapValues($("st-map").value === "dpc" ? "idpc" : $("st-map").value);
    $("st-pick").textContent = `位置 (${x}, ${y}) · 第 ${i + 1} 张${v ? ` · 值 ${+v[i].toPrecision(4)}` : ""}`;
    drawMap();
  });

  $("st-save").addEventListener("click", async () => {
    const r = M.res;
    if (!r) return;
    const key = $("st-map").value;
    const name = `${stem(S.info.path)}_${key}.tif`;
    const path = await invoke("pick_save_path", { dir: saveDir(), name, filter: "TIFF", exts: ["tif", "tiff"] }).catch(() => null);
    if (!path) return;
    const [w, h] = r.scan;
    let bytes;
    if (key === "dpc") {
      // Colour wheel as 8-bit RGB.
      const c = $("st-canvas");
      const d = c.getContext("2d").getImageData(0, 0, w, h).data;
      const rgb = new Uint8Array(w * h * 3);
      for (let i = 0; i < w * h; i++) rgb.set([d[i * 4], d[i * 4 + 1], d[i * 4 + 2]], i * 3);
      bytes = tiff(w, h, rgb, 3, 8, 1);
    } else {
      bytes = tiff(w, h, new Uint8Array(Float32Array.from(mapValues(key)).buffer), 1, 32, 3);
    }
    try {
      await invoke("save_file", bytes, { headers: { "x-path": encodeURIComponent(path) } });
      toast("已保存");
    } catch (e) {
      toast(`保存失败：${e}`);
    }
  });

  /** A minimal baseline TIFF (little-endian, one strip). */
  function tiff(w, h, data, spp, bits, format) {
    const tags = [
      [256, 4, 1, w], [257, 4, 1, h], [258, 3, spp, bits], [259, 3, 1, 1],
      [262, 3, 1, spp === 3 ? 2 : 1], [273, 4, 1, 0], [277, 3, 1, spp], [278, 4, 1, h],
      [279, 4, 1, data.length], [339, 3, spp, format],
    ];
    const ifd = 8, ifdLen = 2 + tags.length * 12 + 4;
    let extra = ifd + ifdLen;
    const extras = [];
    // Values over 4 bytes (per-sample shorts for RGB) go after the IFD.
    for (const t of tags) {
      if (t[1] === 3 && t[2] * 2 > 4) {
        extras.push([t, extra]);
        extra += t[2] * 2;
      }
    }
    const dataOff = extra;
    const buf = new ArrayBuffer(dataOff + data.length);
    const v = new DataView(buf);
    v.setUint16(0, 0x4949, true);
    v.setUint16(2, 42, true);
    v.setUint32(4, ifd, true);
    v.setUint16(ifd, tags.length, true);
    tags.forEach((t, k) => {
      const o = ifd + 2 + k * 12;
      v.setUint16(o, t[0], true);
      v.setUint16(o + 2, t[1], true);
      v.setUint32(o + 4, t[2], true);
      const ex = extras.find((e) => e[0] === t);
      if (ex) {
        v.setUint32(o + 8, ex[1], true);
        for (let i = 0; i < t[2]; i++) v.setUint16(ex[1] + i * 2, t[3], true);
      } else if (t[0] === 273) v.setUint32(o + 8, dataOff, true);
      else if (t[1] === 3) v.setUint16(o + 8, t[3], true);
      else v.setUint32(o + 8, t[3], true);
    });
    v.setUint32(ifd + 2 + tags.length * 12, 0, true);
    new Uint8Array(buf, dataOff).set(data);
    return new Uint8Array(buf);
  }

  // Detectors on the pattern in the main view.
  ext.overlays.push((ctx) => {
    if (panel.hidden || !M.res || M.token !== S.token || !$("st-show").checked || S.axis !== "xy") return;
    const [w] = M.res.det;
    if (S.info.width !== w) return;
    const [x0] = toScreen(0, 0), [x1] = toScreen(1, 0);
    drawDets(ctx, (x, y) => toScreen(x, y), x1 - x0, 1.5);
  });

  setInterval(() => {
    const ok = !!(S.info && S.info.sizeZ * S.info.sizeT >= 4 && !S.info.isRgb);
    btn.disabled = !ok;
    if (!ok && !panel.hidden && !M.busy) close();
    if (!panel.hidden && S.token !== M.token && !M.busy) reset();
  }, 400);
})();
