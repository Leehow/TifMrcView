"use strict";

const T = window.__TAURI__;
const invoke = T.core.invoke;
const $ = (id) => document.getElementById(id);

const COLORS = [
  [255, 255, 255], [0, 255, 0], [255, 0, 255], [255, 0, 0],
  [0, 0, 255], [0, 255, 255], [255, 255, 0],
];
const UNIT_M = { "Å": 1e-10, nm: 1e-9, "µm": 1e-6, mm: 1e-3 };

const S = {
  token: 0,
  info: null,
  z: 0,
  t: 0,
  chans: [],        // {visible, color, name, min, max, dmin, dmax, auto}
  sel: 0,           // channel whose contrast the tools panel edits
  gamma: 1,
  view: { cx: 0, cy: 0, scale: 1 },
  folder: { dir: null, files: [], sizes: [], index: -1 },
  frame: null,      // last rendered bitmap and the image region it covers
  inflight: false,
  dirty: false,
  playing: null,
  tool: "pan",
  ann: [],          // {type: "line"|"path", pts: [[x, y], ...], color, width}
  annShow: true,
  draft: null,      // annotation or selection being drawn
  selRect: null,    // [x, y, w, h] in image pixels
  px: null,         // {x, y, unit} physical pixel size
  pxSource: "",
  scaleBar: false,
  hist: null,
};

const canvas = $("view");
const ctx = canvas.getContext("2d");
const frameCanvas = document.createElement("canvas");
const frameCtx = frameCanvas.getContext("2d");

// ---------- helpers ----------

function toast(msg, ms = 3500) {
  const el = $("toast");
  el.textContent = msg;
  el.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => (el.hidden = true), ms);
}

function basename(p) {
  return p.split(/[\\/]/).pop();
}

function dirname(p) {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i >= 0 ? p.slice(0, i) : "";
}

function stem(p) {
  return basename(p).replace(/\.ome\.tiff?$/i, "").replace(/\.[^.]+$/, "");
}

function fmt(v) {
  if (v === undefined || v === null || Number.isNaN(v)) return "";
  if (Number.isInteger(v)) return String(v);
  const a = Math.abs(v);
  return a !== 0 && (a < 1e-3 || a >= 1e6) ? v.toExponential(3) : String(+v.toFixed(4));
}

/** Notifies plugins (see plugin-host.js). */
function pluginEmit(event, data) {
  if (window.TifMrcView) window.TifMrcView.emit(event, data);
}

function fmtBytes(n) {
  if (n >= 1 << 30) return (n / (1 << 30)).toFixed(1) + " GB";
  if (n >= 1 << 20) return (n / (1 << 20)).toFixed(1) + " MB";
  return Math.max(1, Math.round(n / 1024)) + " KB";
}

function fmtLength(m) {
  const useA = S.px && S.px.unit === "Å";
  let u = "mm";
  if (useA && m < 1e-8) u = "Å";
  else if (m < 1e-6) u = "nm";
  else if (m < 1e-3) u = "µm";
  const v = m / UNIT_M[u];
  return `${+v.toPrecision(v >= 100 ? 4 : 3)} ${u}`;
}

const pref = {
  get(k, d) {
    try {
      const v = localStorage.getItem(k);
      return v === null ? d : JSON.parse(v);
    } catch (_) {
      return d;
    }
  },
  set(k, v) {
    try {
      localStorage.setItem(k, JSON.stringify(v));
    } catch (_) {}
  },
};

/** Parses the binary reply of render_view / thumbnail. */
function parseFrame(buf) {
  const dv = new DataView(buf);
  const w = dv.getUint32(0, true);
  const h = dv.getUint32(4, true);
  const n = dv.getUint32(8, true);
  const ranges = [];
  for (let i = 0; i < n; i++) {
    const o = 12 + i * 16;
    ranges.push({
      min: dv.getFloat32(o, true),
      max: dv.getFloat32(o + 4, true),
      dmin: dv.getFloat32(o + 8, true),
      dmax: dv.getFloat32(o + 12, true),
    });
  }
  const pixels = new Uint8ClampedArray(buf, 12 + n * 16, w * h * 4);
  return { w, h, ranges, image: new ImageData(pixels, w, h) };
}

// ---------- opening files ----------

async function openPath(path) {
  if (!path) return;
  $("loading").textContent = "加载中…";
  $("loading").hidden = false;
  try {
    const res = await invoke("open_image", { path });
    stopPlay();
    const sameSize = S.info && S.info.width === res.info.width && S.info.height === res.info.height;
    S.token = res.token;
    S.info = res.info;
    S.z = Math.floor(res.info.sizeZ / 2);
    S.t = 0;
    S.sel = Math.min(S.sel, res.info.channels.length - 1);
    S.frame = null;
    S.hist = null;
    S.ann = [];
    S.selRect = null;
    S.chans = res.info.channels.map((c) => ({
      visible: true,
      color: c.color,
      name: c.name,
      min: null,
      max: null,
      auto: true,
    }));
    const ps = res.info.pixelSize;
    if (ps && ps[0] > 0 && UNIT_M[normUnit(res.info.unit)]) {
      S.px = { x: ps[0], y: ps[1] > 0 ? ps[1] : ps[0], unit: normUnit(res.info.unit) };
      S.pxSource = "来自文件";
    } else if (!(S.px && S.pxSource === "手动")) {
      S.px = null;
      S.pxSource = "";
    }
    $("empty").hidden = true;
    $("st-pixel").textContent = "";
    T.window.getCurrentWindow().setTitle(`${res.info.name} — TifMrcView`).catch(() => {});
    buildChannels();
    setupSliders();
    syncScaleUi();
    updateInfo();
    updateSaveButtons();
    // Keep the zoom when flipping through images of the same size.
    if (sameSize) {
      draw();
      requestRender();
    } else {
      fit();
    }
    await refreshFolder(path);
    pluginEmit("open", S.info);
  } catch (e) {
    toast(`无法打开 ${basename(path)}：${e}`);
  } finally {
    $("loading").hidden = true;
  }
}

function normUnit(u) {
  if (!u) return "";
  if (u === "um" || u === "micron" || u === "µm" || u === "μm") return "µm";
  if (u === "A" || u === "Å" || u === "angstrom") return "Å";
  return u;
}

async function refreshFolder(path) {
  const dir = dirname(path);
  if (dir !== S.folder.dir) {
    const f = await invoke("list_folder", { path });
    S.folder = { dir, files: f.files, sizes: f.sizes, index: f.index ?? -1 };
    buildStrip();
  } else {
    S.folder.index = S.folder.files.indexOf(path);
    if (S.folder.index < 0) {
      S.folder.dir = null;
      return refreshFolder(path);
    }
  }
  markStrip();
}

function step(delta) {
  const { files, index } = S.folder;
  const next = index + delta;
  if (next >= 0 && next < files.length) openPath(files[next]);
}

async function pickFile() {
  const p = await invoke("pick_file");
  if (p) openPath(p);
}

async function takePending() {
  const p = await invoke("take_pending_open");
  if (p) openPath(p);
}

// ---------- channels ----------

function buildChannels() {
  const box = $("channels");
  box.innerHTML = "";
  if (S.chans.length <= 1) {
    paintChannels();
    syncContrast();
    return;
  }
  S.chans.forEach((c, i) => {
    const el = document.createElement("label");
    el.className = "chan";
    el.title = "勾选：显示/隐藏　点名字：调这个通道的亮度对比度　点圆点：换颜色";
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = c.visible;
    const sw = document.createElement("span");
    sw.className = "sw";
    const nm = document.createElement("span");
    nm.className = "nm";
    nm.textContent = c.name;
    el.append(cb, sw, nm);
    cb.addEventListener("change", () => {
      c.visible = cb.checked;
      paintChannels();
      requestRender();
    });
    sw.addEventListener("click", (e) => {
      e.preventDefault();
      const k = COLORS.findIndex((x) => x.join() === c.color.join());
      c.color = COLORS[(k + 1) % COLORS.length];
      paintChannels();
      requestRender();
    });
    nm.addEventListener("click", (e) => {
      e.preventDefault();
      S.sel = i;
      paintChannels();
      syncContrast();
      loadHist();
    });
    box.append(el);
  });
  paintChannels();
  syncContrast();
}

function paintChannels() {
  [...$("channels").children].forEach((el, i) => {
    const c = S.chans[i];
    el.classList.toggle("off", !c.visible);
    el.classList.toggle("sel", i === S.sel);
    el.querySelector("input").checked = c.visible;
    el.querySelector(".sw").style.background = `rgb(${c.color.join(",")})`;
  });
  const c = S.chans[S.sel];
  $("tool-chan").textContent = S.chans.length > 1 && c ? `· ${c.name}` : "";
}

// ---------- contrast ----------

function dataRange(c) {
  if (S.hist && S.hist.c === S.sel && S.hist.max > S.hist.min) return [S.hist.min, S.hist.max];
  if (c.dmax > c.dmin) return [c.dmin, c.dmax];
  return [0, 1];
}

function syncContrast() {
  const c = S.chans[S.sel];
  if (!c) return;
  $("auto").checked = c.auto;
  if (document.activeElement !== $("cmin")) $("cmin").value = fmt(c.min);
  if (document.activeElement !== $("cmax")) $("cmax").value = fmt(c.max);
  $("cmin").title = `显示最小值（数据最小 ${fmt(c.dmin)}）`;
  $("cmax").title = `显示最大值（数据最大 ${fmt(c.dmax)}）`;
  // Brightness moves the window centre, contrast scales its width (ImageJ style).
  if (c.min !== null && c.max !== null) {
    const [d0, d1] = dataRange(c);
    const range = d1 - d0 || 1;
    const center = (c.min + c.max) / 2;
    const width = Math.max(c.max - c.min, range * 1e-6);
    if (document.activeElement !== $("bright")) $("bright").value = ((d1 - center) / range) * 100;
    if (document.activeElement !== $("contr")) $("contr").value = 50 - 12.5 * Math.log2(width / range);
  }
  drawHist();
}

function setWindow(lo, hi) {
  const c = S.chans[S.sel];
  if (!c) return;
  c.min = lo;
  c.max = hi;
  c.auto = false;
  $("auto").checked = false;
  syncContrast();
  requestRender();
}

function onBrightContrast() {
  const c = S.chans[S.sel];
  if (!c || c.min === null) return;
  const [d0, d1] = dataRange(c);
  const range = d1 - d0 || 1;
  const center = d1 - (+$("bright").value / 100) * range;
  const width = range * Math.pow(2, (50 - +$("contr").value) / 12.5);
  setWindow(center - width / 2, center + width / 2);
}

async function loadHist() {
  if (!S.info || $("tools").hidden || S.info.isRgb) {
    drawHist();
    return;
  }
  const key = `${S.token}:${S.z}:${S.t}:${S.sel}`;
  if (S.hist && S.hist.key === key) return;
  try {
    const h = await invoke("histogram", { token: S.token, z: S.z, t: S.t, c: S.sel });
    S.hist = { ...h, key, c: S.sel };
  } catch (_) {
    S.hist = null;
  }
  drawHist();
}

function drawHist() {
  const hc = $("hist");
  const g = hc.getContext("2d");
  g.clearRect(0, 0, hc.width, hc.height);
  const h = S.hist;
  const c = S.chans[S.sel];
  if (!h || !c) return;
  const max = Math.log1p(Math.max(...h.bins));
  const color = c.color.join(",");
  g.fillStyle = `rgba(${color === "255,255,255" ? "180,190,205" : color},0.85)`;
  h.bins.forEach((v, i) => {
    const y = (Math.log1p(v) / max) * hc.height;
    g.fillRect(i, hc.height - y, 1, y);
  });
  if (c.min !== null && h.max > h.min) {
    const xs = [c.min, c.max].map((v) => ((v - h.min) / (h.max - h.min)) * hc.width);
    g.strokeStyle = "#ffffffcc";
    g.beginPath();
    g.moveTo(xs[0], hc.height);
    g.lineTo(xs[1], 0);
    g.stroke();
    g.fillStyle = "#4fa3ff";
    xs.forEach((x) => g.fillRect(x - 1, 0, 2, hc.height));
  }
}

{
  const hc = $("hist");
  let start = null;
  const val = (e) => {
    const r = hc.getBoundingClientRect();
    const f = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    return S.hist.min + f * (S.hist.max - S.hist.min);
  };
  hc.addEventListener("pointerdown", (e) => {
    if (!S.hist) return;
    start = val(e);
    hc.setPointerCapture(e.pointerId);
  });
  hc.addEventListener("pointermove", (e) => {
    if (start === null) return;
    const v = val(e);
    if (Math.abs(v - start) > 0) setWindow(Math.min(start, v), Math.max(start, v));
  });
  hc.addEventListener("pointerup", () => (start = null));
}

// ---------- sliders / info ----------

function setupSliders() {
  const { sizeZ, sizeT } = S.info;
  $("zbar").hidden = sizeZ <= 1 && sizeT <= 1;
  $("zslider").max = sizeZ - 1;
  $("zslider").value = S.z;
  $("zslider").disabled = sizeZ <= 1;
  $("btn-play").disabled = sizeZ <= 1;
  $("tgroup").hidden = sizeT <= 1;
  $("tslider").max = sizeT - 1;
  $("tslider").value = S.t;
  updateZLabel();
}

function updateZLabel() {
  $("zlabel").textContent = `${S.z + 1} / ${S.info.sizeZ}`;
  $("tlabel").textContent = `${S.t + 1} / ${S.info.sizeT}`;
}

function setZ(z) {
  if (!S.info) return;
  z = Math.max(0, Math.min(S.info.sizeZ - 1, z));
  if (z === S.z) return;
  S.z = z;
  $("zslider").value = z;
  updateZLabel();
  requestRender();
  pluginEmit("slice", { z: S.z, t: S.t });
}

function setT(t) {
  t = Math.max(0, Math.min(S.info.sizeT - 1, t));
  S.t = t;
  $("tslider").value = t;
  updateZLabel();
  requestRender();
  pluginEmit("slice", { z: S.z, t: S.t });
}

function updateInfo() {
  const i = S.info;
  if (!i) return;
  const dims = [`${i.width}×${i.height}`];
  if (i.sizeC > 1) dims.push(`C${i.sizeC}`);
  if (i.sizeZ > 1) dims.push(`Z${i.sizeZ}`);
  if (i.sizeT > 1) dims.push(`T${i.sizeT}`);
  const px = S.px ? ` · ${fmt(S.px.x)} ${S.px.unit}/px` : "";
  $("st-info").textContent = `${i.format} · ${dims.join(" × ")} · ${i.dtype}${px}`;
  $("file-name").textContent = i.name;
  $("file-name").title = i.path;
  const { files, index } = S.folder;
  $("file-pos").textContent = files.length ? `${index + 1} / ${files.length}` : "–";
  $("btn-prev").disabled = index <= 0;
  $("btn-next").disabled = index < 0 || index >= files.length - 1;
}

function togglePlay() {
  if (S.playing) return stopPlay();
  if (!S.info || S.info.sizeZ <= 1) return;
  $("btn-play").textContent = "❚❚";
  S.playing = setInterval(() => {
    if (S.inflight) return; // never queue frames faster than we can draw
    setZ(S.z + 1 >= S.info.sizeZ ? 0 : S.z + 1);
  }, 1000 / 20);
}

function stopPlay() {
  clearInterval(S.playing);
  S.playing = null;
  $("btn-play").textContent = "▶";
}

// ---------- scale bar / pixel size ----------

function syncScaleUi() {
  if (S.px) {
    $("px-size").value = fmt(S.px.x);
    $("px-unit").value = S.px.unit;
  } else {
    $("px-size").value = "";
  }
  $("px-src").textContent = S.px ? `像素尺寸${S.pxSource}` : "文件里没有像素尺寸，可以手动填写";
  $("sb-show").checked = S.scaleBar;
  $("sb-show").disabled = !S.px;
}

function onPxInput() {
  const v = parseFloat($("px-size").value);
  if (Number.isFinite(v) && v > 0) {
    S.px = { x: v, y: v, unit: $("px-unit").value };
    S.pxSource = "手动";
  } else {
    S.px = null;
  }
  syncScaleUi();
  updateInfo();
  draw();
}

/** Picks a 1/2/5 × 10^n length close to `target` (metres). */
function niceLength(target) {
  const p = Math.pow(10, Math.floor(Math.log10(target)));
  const f = target / p;
  return (f >= 5 ? 5 : f >= 2 ? 2 : 1) * p;
}

/** Draws a scale bar; `k` is context pixels per image pixel. */
function drawScaleBar(g, outW, outH, k, onScreen) {
  if (!S.scaleBar || !S.px) return;
  const pxM = S.px.x * UNIT_M[S.px.unit];
  const target = onScreen ? 120 : outW * 0.15;
  const len = niceLength((target / k) * pxM);
  const barPx = (len / pxM) * k;
  const thick = onScreen ? 4 : Math.max(3, Math.round(outH * 0.008));
  const font = onScreen ? 13 : Math.max(12, Math.round(outH * 0.025));
  const margin = onScreen ? 18 : Math.max(10, Math.round(outW * 0.025));
  const x1 = outW - margin;
  const x0 = x1 - barPx;
  const y = outH - margin;
  g.save();
  g.fillStyle = "#fff";
  g.shadowColor = "#000";
  g.shadowBlur = 3;
  g.fillRect(x0, y - thick, barPx, thick);
  g.font = `600 ${font}px -apple-system, "Segoe UI", sans-serif`;
  g.textAlign = "center";
  g.textBaseline = "bottom";
  g.fillText(fmtLength(len), (x0 + x1) / 2, y - thick - font * 0.3);
  g.restore();
}

// ---------- view / rendering ----------

function resizeCanvas() {
  const dpr = window.devicePixelRatio || 1;
  const r = canvas.getBoundingClientRect();
  canvas.width = Math.max(1, Math.round(r.width * dpr));
  canvas.height = Math.max(1, Math.round(r.height * dpr));
  draw();
  requestRender();
}

function cssSize() {
  const r = canvas.getBoundingClientRect();
  return { cw: r.width, ch: r.height };
}

function fit() {
  if (!S.info) return;
  const { cw, ch } = cssSize();
  S.view.scale = Math.min(cw / S.info.width, ch / S.info.height);
  S.view.cx = S.info.width / 2;
  S.view.cy = S.info.height / 2;
  draw();
  requestRender();
}

function oneToOne() {
  if (!S.info) return;
  S.view.scale = 1 / (window.devicePixelRatio || 1);
  draw();
  requestRender();
}

/** Visible part of the image, in image pixels. */
function visibleRegion() {
  const { cw, ch } = cssSize();
  const { cx, cy, scale } = S.view;
  const x0 = Math.max(0, Math.floor(cx - cw / 2 / scale));
  const y0 = Math.max(0, Math.floor(cy - ch / 2 / scale));
  const x1 = Math.min(S.info.width, Math.ceil(cx + cw / 2 / scale));
  const y1 = Math.min(S.info.height, Math.ceil(cy + ch / 2 / scale));
  return x1 > x0 && y1 > y0 ? [x0, y0, x1 - x0, y1 - y0] : null;
}

function toScreen(x, y) {
  const { cw, ch } = cssSize();
  return [(x - S.view.cx) * S.view.scale + cw / 2, (y - S.view.cy) * S.view.scale + ch / 2];
}

function toImage(e) {
  const r = canvas.getBoundingClientRect();
  const { cw, ch } = cssSize();
  return [
    S.view.cx + (e.clientX - r.left - cw / 2) / S.view.scale,
    S.view.cy + (e.clientY - r.top - ch / 2) / S.view.scale,
  ];
}

function lineLength(a, b) {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const px = Math.hypot(dx, dy);
  if (!S.px) return { px, text: `${fmt(+px.toFixed(1))} px` };
  const m = Math.hypot(dx * S.px.x, dy * S.px.y) * UNIT_M[S.px.unit];
  return { px, text: `${fmtLength(m)}  (${fmt(+px.toFixed(1))} px)` };
}

/**
 * Draws annotations. `map` turns image coordinates into context
 * coordinates and `unit` is the size of one screen pixel in the context.
 */
function drawAnnotations(g, map, unit, items) {
  for (const a of items) {
    if (a.pts.length < 2) continue;
    g.save();
    g.strokeStyle = a.color;
    g.lineWidth = a.width * unit;
    g.lineCap = "round";
    g.lineJoin = "round";
    g.beginPath();
    a.pts.forEach((p, i) => {
      const [x, y] = map(p[0], p[1]);
      i ? g.lineTo(x, y) : g.moveTo(x, y);
    });
    g.stroke();
    if (a.type === "line") {
      // End ticks and the length label.
      const [x0, y0] = map(...a.pts[0]);
      const [x1, y1] = map(...a.pts[1]);
      const ang = Math.atan2(y1 - y0, x1 - x0) + Math.PI / 2;
      const tk = 5 * unit;
      for (const [x, y] of [[x0, y0], [x1, y1]]) {
        g.beginPath();
        g.moveTo(x - Math.cos(ang) * tk, y - Math.sin(ang) * tk);
        g.lineTo(x + Math.cos(ang) * tk, y + Math.sin(ang) * tk);
        g.stroke();
      }
      const label = lineLength(a.pts[0], a.pts[1]).text.split("  (")[0];
      g.font = `600 ${13 * unit}px -apple-system, "Segoe UI", sans-serif`;
      g.textAlign = "center";
      g.textBaseline = "bottom";
      const w = g.measureText(label).width;
      const mx = (x0 + x1) / 2;
      const my = (y0 + y1) / 2 - 6 * unit;
      g.fillStyle = "#000a";
      g.fillRect(mx - w / 2 - 4 * unit, my - 16 * unit, w + 8 * unit, 18 * unit);
      g.fillStyle = a.color;
      g.fillText(label, mx, my);
    }
    g.restore();
  }
}

function draw() {
  const dpr = window.devicePixelRatio || 1;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  if (!S.frame || !S.info) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  const [x, y, w, h] = S.frame.region;
  const [sx, sy] = toScreen(x, y);
  ctx.imageSmoothingEnabled = S.view.scale * dpr < 1;
  ctx.drawImage(frameCanvas, 0, 0, S.frame.w, S.frame.h, sx, sy, w * S.view.scale, h * S.view.scale);

  const items = S.annShow ? S.ann : [];
  const extra = S.draft && S.draft.type !== "select" ? [S.draft] : [];
  drawAnnotations(ctx, toScreen, 1, items.concat(extra));

  const rect = S.draft && S.draft.type === "select" ? draftRect() : S.selRect;
  if (rect) {
    const [rx, ry] = toScreen(rect[0], rect[1]);
    ctx.save();
    ctx.setLineDash([6, 4]);
    ctx.strokeStyle = "#fff";
    ctx.lineWidth = 1.5;
    ctx.strokeRect(rx, ry, rect[2] * S.view.scale, rect[3] * S.view.scale);
    ctx.restore();
  }

  const { cw, ch } = cssSize();
  drawScaleBar(ctx, cw, ch, S.view.scale, true);
  $("st-zoom").textContent = `${Math.round(S.view.scale * dpr * 100)}%`;
}

function requestRender() {
  if (!S.info) return;
  if (S.inflight) {
    S.dirty = true;
    return;
  }
  renderNow();
}

function channelReqs() {
  const out = [];
  S.chans.forEach((c, i) => {
    if (!c.visible) return;
    out.push({ index: i, color: c.color, min: c.auto ? null : c.min, max: c.auto ? null : c.max });
  });
  return out;
}

async function renderNow() {
  const region = visibleRegion();
  if (!region) {
    S.frame = null;
    draw();
    return;
  }
  const dpr = window.devicePixelRatio || 1;
  const scale = S.view.scale * dpr;
  // Never ask for more pixels than the source has; the GPU upscales.
  const outW = Math.max(1, Math.round(region[2] * Math.min(scale, 1)));
  const outH = Math.max(1, Math.round(region[3] * Math.min(scale, 1)));
  const channels = channelReqs();
  const token = S.token;
  const req = { token, z: S.z, t: S.t, channels, region, outW, outH, gamma: S.gamma, invert: false };
  S.inflight = true;
  S.dirty = false;
  try {
    const buf = await invoke("render_view", { req });
    if (token !== S.token) return;
    const f = parseFrame(buf);
    frameCanvas.width = f.w;
    frameCanvas.height = f.h;
    frameCtx.putImageData(f.image, 0, 0);
    S.frame = { region, w: f.w, h: f.h };
    channels.forEach((c, k) => {
      const ch = S.chans[c.index];
      const r = f.ranges[k];
      ch.min = r.min;
      ch.max = r.max;
      ch.dmin = r.dmin;
      ch.dmax = r.dmax;
    });
    syncContrast();
    draw();
    loadHist();
    pluginEmit("render");
  } catch (e) {
    if (String(e) !== "stale") toast(`渲染失败：${e}`);
  } finally {
    S.inflight = false;
    if (S.dirty) requestRender();
  }
}

// ---------- saving ----------

/**
 * Renders `region` of slice z/t (default: the shown one) with the current
 * display settings and visible overlays into a canvas. `size` forces the
 * output size; otherwise it is full resolution, capped at 16384 px.
 */
async function renderRegion(region, { z = S.z, t = S.t, size = null } = {}) {
  const [x, y, w, h] = region.map(Math.round);
  const k = size ? size[0] / w : Math.min(1, 16384 / Math.max(w, h));
  const outW = size ? size[0] : Math.max(1, Math.round(w * k));
  const outH = size ? size[1] : Math.max(1, Math.round(h * k));
  const req = {
    token: S.token, z, t, channels: channelReqs(),
    region: [x, y, w, h], outW, outH, gamma: S.gamma, invert: false,
  };
  const f = parseFrame(await invoke("render_view", { req }));
  const c = document.createElement("canvas");
  c.width = f.w;
  c.height = f.h;
  const g = c.getContext("2d");
  g.putImageData(f.image, 0, 0);
  // Overlays keep the size they have on screen relative to the image.
  const unit = Math.max(0.5, k / S.view.scale);
  if (S.annShow) drawAnnotations(g, (ix, iy) => [(ix - x) * k, (iy - y) * k], unit, S.ann);
  drawScaleBar(g, f.w, f.h, k, false);
  return c;
}

/** Renders `region` at full resolution with visible overlays into PNG bytes. */
async function exportPng(region) {
  const c = await renderRegion(region);
  const blob = await new Promise((res) => c.toBlob(res, "image/png"));
  return new Uint8Array(await blob.arrayBuffer());
}

async function writeFile(path, bytes) {
  await invoke("save_file", bytes, { headers: { "x-path": encodeURIComponent(path) } });
}

function exportName() {
  const z = S.info.sizeZ > 1 ? `_z${S.z + 1}` : "";
  return `${stem(S.info.path)}${z}${S.selRect ? "_crop" : ""}`;
}

async function saveAs(useSelection) {
  if (!S.info) return;
  const region = useSelection && S.selRect ? S.selRect : [0, 0, S.info.width, S.info.height];
  const path = await invoke("pick_save_path", { dir: dirname(S.info.path), name: exportName() + ".png" });
  if (!path) return;
  await doSave(region, path);
}

async function quickSave() {
  if (!S.info) return;
  const region = S.selRect || [0, 0, S.info.width, S.info.height];
  const path = await invoke("free_path", { dir: dirname(S.info.path), stem: exportName() });
  await doSave(region, path);
}

async function doSave(region, path) {
  $("loading").textContent = "保存中…";
  $("loading").hidden = false;
  try {
    await writeFile(path, await exportPng(region));
    toast(`已保存 ${path}`);
    if (dirname(path) === S.folder.dir) {
      S.folder.dir = null; // refresh the list so the new file shows up
      refreshFolder(S.info.path);
    }
  } catch (e) {
    toast(`保存失败：${e}`, 6000);
  } finally {
    $("loading").hidden = true;
  }
}

function updateSaveButtons() {
  $("btn-save-sel").disabled = !S.selRect;
  $("btn-save-all").disabled = !S.info;
}

// ---------- tools / interaction ----------

function setTool(t) {
  S.tool = t;
  for (const b of $("toolbtns").children) b.classList.toggle("on", b.dataset.tool === t);
  canvas.classList.toggle("cross", t !== "pan");
  if (t !== "pan" && $("tools").hidden) toggleTools(true);
}

function toggleTools(force) {
  const show = force ?? $("tools").hidden;
  $("tools").hidden = !show;
  $("btn-tools").classList.toggle("on", show);
  pref.set("tools", show);
  if (show) loadHist();
}

function toggleSidebar(force) {
  const sb = $("sidebar");
  const show = force ?? sb.classList.contains("collapsed");
  sb.classList.toggle("collapsed", !show);
  $("btn-sidebar").classList.toggle("on", show);
  pref.set("sidebar", show);
  if (show) markStrip();
}

function draftRect(d = S.draft) {
  const [a, b] = d.pts;
  const x0 = Math.max(0, Math.min(a[0], b[0]));
  const y0 = Math.max(0, Math.min(a[1], b[1]));
  const x1 = Math.min(S.info.width, Math.max(a[0], b[0]));
  const y1 = Math.min(S.info.height, Math.max(a[1], b[1]));
  return [x0, y0, Math.max(0, x1 - x0), Math.max(0, y1 - y0)];
}

let drag = null;
canvas.addEventListener("pointerdown", (e) => {
  if (!S.info) return;
  canvas.setPointerCapture(e.pointerId);
  const panning = S.tool === "pan" || e.button === 1 || e.button === 2;
  if (panning) {
    drag = { x: e.clientX, y: e.clientY, cx: S.view.cx, cy: S.view.cy };
    canvas.classList.add("drag");
    return;
  }
  const p = toImage(e);
  const color = $("ann-color").value;
  const width = +$("ann-width").value;
  if (S.tool === "measure") S.draft = { type: "line", pts: [p, p], color, width };
  else if (S.tool === "pen") S.draft = { type: "path", pts: [p], color, width };
  else if (S.tool === "select") S.draft = { type: "select", pts: [p, p] };
  if (S.tool !== "select") S.annShow = $("ann-show").checked = true;
});
canvas.addEventListener("contextmenu", (e) => e.preventDefault());
canvas.addEventListener("pointermove", (e) => {
  if (drag) {
    S.view.cx = drag.cx - (e.clientX - drag.x) / S.view.scale;
    S.view.cy = drag.cy - (e.clientY - drag.y) / S.view.scale;
    draw();
    requestRender();
  } else if (S.draft) {
    const p = toImage(e);
    if (S.draft.type === "path") {
      const last = S.draft.pts[S.draft.pts.length - 1];
      if (Math.hypot(p[0] - last[0], p[1] - last[1]) * S.view.scale > 2) S.draft.pts.push(p);
    } else {
      S.draft.pts[1] = p;
    }
    if (S.draft.type === "line") $("measure-out").textContent = lineLength(...S.draft.pts).text;
    if (S.draft.type === "select") {
      const r = draftRect();
      $("measure-out").textContent = `选区 ${Math.round(r[2])} × ${Math.round(r[3])} px`;
    }
    draw();
  }
  showPixel(e);
});
function endPointer() {
  if (drag) {
    drag = null;
    canvas.classList.remove("drag");
    return;
  }
  const d = S.draft;
  S.draft = null;
  if (!d) return;
  if (d.type === "select") {
    const r = draftRect(d);
    S.selRect = r[2] * S.view.scale > 3 && r[3] * S.view.scale > 3 ? r : null;
    updateSaveButtons();
  } else {
    const [a, b] = [d.pts[0], d.pts[d.pts.length - 1]];
    const long = d.type === "path" ? d.pts.length > 1 : Math.hypot(b[0] - a[0], b[1] - a[1]) * S.view.scale > 3;
    if (long) S.ann.push(d);
  }
  draw();
}
// Some WebKit builds drop pointerup on the captured element; listen on the
// window too. endPointer is idempotent.
canvas.addEventListener("pointerup", endPointer);
window.addEventListener("pointerup", endPointer);
canvas.addEventListener("lostpointercapture", endPointer);
canvas.addEventListener("dblclick", () => {
  if (S.tool === "pan") fit();
});

canvas.addEventListener(
  "wheel",
  (e) => {
    if (!S.info) return;
    e.preventDefault();
    if (e.shiftKey && !e.ctrlKey) {
      setZ(S.z + Math.sign(e.deltaY || e.deltaX));
      return;
    }
    const r = canvas.getBoundingClientRect();
    const mx = e.clientX - r.left;
    const my = e.clientY - r.top;
    const { cw, ch } = cssSize();
    const ix = S.view.cx + (mx - cw / 2) / S.view.scale;
    const iy = S.view.cy + (my - ch / 2) / S.view.scale;
    const k = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0025));
    S.view.scale = Math.min(64, Math.max(0.005, S.view.scale * k));
    S.view.cx = ix - (mx - cw / 2) / S.view.scale;
    S.view.cy = iy - (my - ch / 2) / S.view.scale;
    draw();
    requestRender();
  },
  { passive: false },
);

let pixelBusy = false;
async function showPixel(e) {
  if (!S.info || pixelBusy) return;
  const [fx, fy] = toImage(e);
  const x = Math.floor(fx);
  const y = Math.floor(fy);
  if (x < 0 || y < 0 || x >= S.info.width || y >= S.info.height) {
    $("st-pixel").textContent = "";
    return;
  }
  pixelBusy = true;
  try {
    const v = await invoke("pixel_values", { token: S.token, z: S.z, t: S.t, x, y });
    $("st-pixel").textContent = `x ${x}  y ${y}` + (v.length ? `  值 ${v.map(fmt).join(" / ")}` : "");
  } catch (_) {
    // stale token while switching files
  } finally {
    pixelBusy = false;
  }
}

window.addEventListener("keydown", (e) => {
  const tag = e.target.tagName;
  if ((tag === "INPUT" && e.target.type !== "checkbox" && e.target.type !== "range") || tag === "SELECT") return;
  const mod = e.ctrlKey || e.metaKey;
  const key = e.key.toLowerCase();
  if (mod) {
    if (key === "o") pickFile();
    else if (key === "s") quickSave();
    else if (key === "z") undo();
    else return;
    e.preventDefault();
    return;
  }
  switch (e.key) {
    case "ArrowUp":
      step(-1);
      break;
    case "ArrowDown":
      step(1);
      break;
    case "ArrowRight":
    case "PageDown":
    case ".":
      setZ(S.z + 1);
      break;
    case "ArrowLeft":
    case "PageUp":
    case ",":
      setZ(S.z - 1);
      break;
    case "Home":
      setZ(0);
      break;
    case "End":
      if (S.info) setZ(S.info.sizeZ - 1);
      break;
    case " ":
      togglePlay();
      break;
    case "Escape":
      S.draft = null;
      S.selRect = null;
      updateSaveButtons();
      setTool("pan");
      draw();
      break;
    case "1":
      oneToOne();
      break;
    default:
      switch (key) {
        case "f": fit(); break;
        case "t": toggleTools(); break;
        case "b": toggleSidebar(); break;
        case "v": setTool("pan"); break;
        case "m": setTool("measure"); break;
        case "p": setTool("pen"); break;
        case "r": setTool("select"); break;
        case "h": toggleAnn(); break;
        default: return;
      }
  }
  e.preventDefault();
});

function undo() {
  if (S.ann.length) {
    S.ann.pop();
    draw();
  }
}

function toggleAnn() {
  S.annShow = !S.annShow;
  $("ann-show").checked = S.annShow;
  draw();
}

$("btn-open").addEventListener("click", pickFile);
$("btn-prev").addEventListener("click", () => step(-1));
$("btn-next").addEventListener("click", () => step(1));
$("btn-fit").addEventListener("click", fit);
$("btn-1to1").addEventListener("click", oneToOne);
$("btn-play").addEventListener("click", togglePlay);
$("btn-tools").addEventListener("click", () => toggleTools());
$("btn-sidebar").addEventListener("click", () => toggleSidebar());
$("btn-default").addEventListener("click", async () => {
  try {
    toast(await invoke("set_default_app"), 6000);
  } catch (e) {
    toast(`设置失败：${e}`, 6000);
  }
});
$("zslider").addEventListener("input", (e) => setZ(+e.target.value));
$("tslider").addEventListener("input", (e) => setT(+e.target.value));
$("auto").addEventListener("change", (e) => {
  const c = S.chans[S.sel];
  if (!c) return;
  c.auto = e.target.checked;
  requestRender();
});
$("btn-reset").addEventListener("click", () => {
  S.chans.forEach((c) => (c.auto = true));
  S.gamma = 1;
  $("gamma").value = 1;
  $("gamma-v").textContent = "1.00";
  requestRender();
});
for (const id of ["cmin", "cmax"]) {
  $(id).addEventListener("change", () => {
    const c = S.chans[S.sel];
    if (!c) return;
    const lo = parseFloat($("cmin").value);
    const hi = parseFloat($("cmax").value);
    setWindow(Number.isFinite(lo) ? lo : c.min, Number.isFinite(hi) ? hi : c.max);
  });
}
$("bright").addEventListener("input", onBrightContrast);
$("contr").addEventListener("input", onBrightContrast);
$("gamma").addEventListener("input", (e) => {
  S.gamma = +e.target.value;
  $("gamma-v").textContent = S.gamma.toFixed(2);
  requestRender();
});
for (const b of $("toolbtns").children) b.addEventListener("click", () => setTool(b.dataset.tool));
$("ann-show").addEventListener("change", (e) => {
  S.annShow = e.target.checked;
  draw();
});
$("btn-undo").addEventListener("click", undo);
$("btn-clear").addEventListener("click", () => {
  S.ann = [];
  S.selRect = null;
  updateSaveButtons();
  $("measure-out").textContent = "";
  draw();
});
$("sb-show").addEventListener("change", (e) => {
  S.scaleBar = e.target.checked;
  pref.set("scalebar", S.scaleBar);
  draw();
});
$("px-size").addEventListener("change", onPxInput);
$("px-unit").addEventListener("change", onPxInput);
$("btn-save-all").addEventListener("click", () => saveAs(false));
$("btn-save-sel").addEventListener("click", () => saveAs(true));

new ResizeObserver(resizeCanvas).observe(canvas);

// ---------- folder thumbnails ----------

const thumbQueue = [];
let thumbActive = 0;
const THUMB_PARALLEL = Math.max(2, Math.min(8, (navigator.hardwareConcurrency || 4) - 1));
let thumbObserver = null;

function buildStrip() {
  const strip = $("strip");
  strip.innerHTML = "";
  thumbQueue.length = 0;
  if (thumbObserver) thumbObserver.disconnect();
  thumbObserver = new IntersectionObserver(
    (entries) => {
      for (const en of entries) {
        if (en.isIntersecting && !en.target.dataset.req) {
          en.target.dataset.req = "1";
          thumbQueue.push(en.target);
          pumpThumbs();
        }
      }
    },
    { root: $("sidebar"), rootMargin: "600px 0px" },
  );
  S.folder.files.forEach((f, i) => {
    const el = document.createElement("div");
    el.className = "thumb";
    el.title = f;
    el.dataset.path = f;
    el.innerHTML = `<div class="box"></div><div class="nm"></div><div class="meta"></div>`;
    el.querySelector(".nm").textContent = basename(f);
    const ext = (f.match(/\.([^.\\/]+)$/) || [, ""])[1].toUpperCase();
    el.querySelector(".meta").textContent = `${ext} · ${fmtBytes(S.folder.sizes[i] || 0)}`;
    el.addEventListener("click", () => openPath(f));
    strip.append(el);
    thumbObserver.observe(el);
  });
}

function pumpThumbs() {
  while (thumbActive < THUMB_PARALLEL && thumbQueue.length) {
    // Load the thumbnails closest to the current file first.
    const cur = S.folder.index;
    const dist = (el) => Math.abs(S.folder.files.indexOf(el.dataset.path) - cur);
    let best = 0;
    for (let i = 1; i < thumbQueue.length; i++) {
      if (dist(thumbQueue[i]) < dist(thumbQueue[best])) best = i;
    }
    const el = thumbQueue.splice(best, 1)[0];
    if (!el.isConnected) continue;
    thumbActive++;
    invoke("thumbnail", { path: el.dataset.path, size: 256 })
      .then((buf) => {
        const f = parseFrame(buf);
        const c = document.createElement("canvas");
        c.width = f.w;
        c.height = f.h;
        c.getContext("2d").putImageData(f.image, 0, 0);
        el.querySelector(".box").replaceChildren(c);
      })
      .catch(() => {
        el.querySelector(".box").textContent = "无法预览";
      })
      .finally(() => {
        thumbActive--;
        pumpThumbs();
      });
  }
}

function markStrip() {
  updateInfo();
  const strip = $("strip");
  [...strip.children].forEach((el, i) => el.classList.toggle("cur", i === S.folder.index));
  const cur = strip.children[S.folder.index];
  if (cur && !$("sidebar").classList.contains("collapsed")) {
    cur.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  }
}

// ---------- startup ----------

S.scaleBar = pref.get("scalebar", false);
toggleSidebar(pref.get("sidebar", true));
toggleTools(pref.get("tools", false));
syncScaleUi();
T.event.listen("open-pending", takePending);
T.webview
  .getCurrentWebview()
  .onDragDropEvent((e) => {
    if (e.payload.type === "drop" && e.payload.paths.length) openPath(e.payload.paths[0]);
  })
  .catch(() => {});
takePending();
