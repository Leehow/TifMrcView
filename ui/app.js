"use strict";

const T = window.__TAURI__;
// Fingers are blunter than a mouse: on touch screens annotations are easier
// to hit and their grab points bigger.
const TOUCH = matchMedia("(pointer: coarse)").matches;
const HIT = TOUCH ? 14 : 6;
const GRIP = TOUCH ? 9 : 5;
const invoke = T.core.invoke;
const $ = (id) => document.getElementById(id);

// Standard or Pro build (same code; the Pro build has its own product name).
// Pro loads its extra tools from pro/pro.js.
const EDITION = { name: "TifMrcView", pro: false };
T.app.getName().then((name) => {
  EDITION.name = name;
  EDITION.pro = name !== "TifMrcView";
  document.title = name;
  if (!EDITION.pro) return;
  document.documentElement.dataset.edition = "pro";
  const s = document.createElement("script");
  s.src = "pro/pro.js";
  document.body.appendChild(s);
}).catch(() => {});

const COLORS = [
  [255, 255, 255], [0, 255, 0], [255, 0, 255], [255, 0, 0],
  [0, 0, 255], [0, 255, 255], [255, 255, 0],
];
const UNIT_M = { "Å": 1e-10, nm: 1e-9, "µm": 1e-6, mm: 1e-3 };
// Reciprocal (diffraction) units, per metre; lengths in them are |g|, d = 1/|g|.
Object.assign(UNIT_M, { "1/µm": 1e6, "1/nm": 1e9, "1/Å": 1e10 });

const isRecip = (u) => u.startsWith("1/");

const S = {
  token: 0,
  info: null,
  z: 0,
  t: 0,
  chans: [],        // {visible, color, name, min, max, dmin, dmax, auto}
  sel: 0,           // channel whose contrast the tools panel edits
  gamma: 1,
  view: { cx: 0, cy: 0, scale: 1 },
  folder: { dir: null, files: [], sizes: [], stacks: {}, index: -1 },
  frame: null,      // last rendered bitmap and the image region it covers
  inflight: false,
  dirty: false,
  playing: null,
  tool: "select",
  axis: "xy",
  base: null,
  ann: [],          // {type: "line"|"path"|"segment"|"arrow"|"rect"|"ellipse"|"text", pts: [[x, y], ...], color, width, z, text?}
  annShow: true,
  history: [],      // snapshots of annotations and crop, for undo
  future: [],       // undone snapshots, for redo
  lastDrawn: null,  // the annotation drawn last, which Esc removes
  annPending: null, // annotations waiting to be written to disk
  annTimer: 0,
  op: null,         // pointer drag in progress
  picked: null,     // annotation chosen with the select tool
  cropBox: null,    // crop rectangle being drawn, {type: "crop", pts}
  crop: null,       // applied crop [x, y, w, h] in image pixels
  cropStack: [],    // earlier crops, for undo
  px: null,         // {x, y, unit} physical pixel size
  pz: null,         // distance between Z slices, in px.unit
  pluginMeta: [],   // metadata groups plugins added for the open file
  pxSource: "",
  scaleBar: false,
  hist: null,
  model: null,      // IMOD model loaded for this image
  modelShow: true,
};

// Tools and overlays added from outside this file (the Pro label tools):
// tools[name] = {down(e, p), move(e, p), up(), undo(), redo(), active(on)},
// where p is the pointer in image pixels; overlays are f(ctx) drawn over the
// image in screen coordinates, under the annotations.
const ext = { tools: {}, overlays: [] };

const canvas = $("view");
const ctx = canvas.getContext("2d");
const frameCanvas = document.createElement("canvas");
const frameCtx = frameCanvas.getContext("2d");
// The last picture of the previous image, faded out over the new one.
const fadeCanvas = document.createElement("canvas");
const FADE_MS = 160;
// A small picture of the whole image, drawn under the sharp view so that
// panning or zooming out never shows blank space while the view renders.
const ovCanvas = document.createElement("canvas");
const OV_SIZE = 1024;

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

function lengthUnit(m) {
  if (S.px && isRecip(S.px.unit)) return S.px.unit;
  if (S.px && S.px.unit === "Å" && m < 1e-8) return "Å";
  return m < 1e-6 ? "nm" : m < 1e-3 ? "µm" : "mm";
}

/** Formats a length in metres; `u` forces the unit. */
function fmtLength(m, u = lengthUnit(m)) {
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

/** Whether an open error says a (format) plugin is missing or switched off. */
function needsPlugin(e) {
  return /「.+」插件/.test(String(e));
}

/**
 * Opens `path` (image `series` of a file that holds several); `force`
 * re-reads the file shown now (e.g. after a plugin was switched).
 */
async function openPath(path, force = false, series = 0) {
  if (!path) return;
  // Clicking the image already shown changes nothing.
  if (!force && S.info && S.info.path === path && S.info.seriesIndex === series && S.axis === "xy") return;
  finishText(true);
  await flushAnn();
  $("loading").textContent = "加载中…";
  // Only show the hint when loading takes a while, so quick flips do not blink.
  const slow = setTimeout(() => ($("loading").hidden = false), 300);
  try {
    const res = await invoke("open_image", { path, series });
    stopPlay();
    sideDone();
    holdFrame();
    const sameSize = S.info && S.info.width === res.info.width && S.info.height === res.info.height;
    S.token = res.token;
    S.info = res.info;
    S.pluginMeta = [];
    S.axis = "xy";
    S.base = null;
    S.z = Math.floor(res.info.sizeZ / 2);
    S.t = 0;
    S.sel = Math.min(S.sel, res.info.channels.length - 1);
    S.frame = null;
    S.hist = null;
    S.ann = [];
    S.history = [];
    S.future = [];
    S.lastDrawn = null;
    S.picked = null;
    S.op = null;
    S.cropBox = null;
    S.crop = null;
    S.cropStack = [];
    showSize(null);
    $("measure-out").textContent = "";
    setModel(null);
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
    S.pz = S.px && ps && ps[2] > 0 && S.pxSource !== "手动" ? ps[2] : null;
    $("empty").hidden = true;
    $("st-pixel").textContent = "";
    T.window.getCurrentWindow().setTitle(`${res.info.name} — ${EDITION.name}`).catch(() => {});
    buildChannels();
    buildSeries();
    setupSliders();
    syncScaleUi();
    updateInfo();
    updateSaveButtons();
    $("btn-info").disabled = false;
    $("meta-list").dataset.file = "";
    renderMeta();
    specOpened();
    // Keep the zoom when flipping through images of the same size.
    if (sameSize) {
      clampView();
      draw();
      requestRender();
    } else {
      fit();
    }
    autoLoadModel(path);
    loadAnn(path, res.info.seriesIndex);
    await refreshFolder(path);
    pluginEmit("open", S.info);
    watchLoad();
  } catch (e) {
    toast(`无法打开 ${basename(path)}：${e}`, needsPlugin(e) ? 8000 : 3500);
    // The plugin window has the way to the plugin market.
    if (needsPlugin(e) && typeof togglePluginDialog === "function") togglePluginDialog(true);
  } finally {
    clearTimeout(slow);
    $("loading").hidden = true;
  }
}

/** Keeps the current picture on screen until the next image has a frame. */
function holdFrame() {
  if (!S.frame) return;
  fadeCanvas.width = canvas.width;
  fadeCanvas.height = canvas.height;
  fadeCanvas.getContext("2d").drawImage(canvas, 0, 0);
  S.fade = { alpha: 1, start: 0 };
  // Never leave the old picture up if the new one fails to render.
  if (!S.sideWait) setTimeout(startFade, 1500);
}

/** Fades the held picture out once the new image has drawn. */
function startFade() {
  const f = S.fade;
  if (!f || f.start) return;
  f.start = performance.now();
  const tick = (now) => {
    if (S.fade !== f) return;
    f.alpha = Math.max(0, 1 - (now - f.start) / FADE_MS);
    if (f.alpha === 0) S.fade = null;
    draw();
    if (S.fade === f) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

function drawFade() {
  if (!S.fade) return;
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.globalAlpha = S.fade.alpha;
  ctx.drawImage(fadeCanvas, 0, 0);
  ctx.restore();
}

/** Switches between the XY view and the XZ / YZ side views of a Z stack. */
async function setAxis(axis) {
  finishText(true);
  if (!S.info || axis === S.axis) return;
  const prev = S.axis;
  if (prev === "xy") {
    flushAnn();
    S.base = { token: S.token, info: S.info, z: S.z, ann: S.ann, history: S.history, future: S.future, model: S.model, crop: S.crop, cropStack: S.cropStack };
  }
  let token, info, z;
  if (axis === "xy") {
    ({ token, info, z } = S.base);
  } else {
    try {
      ({ token, info } = await invoke("open_reslice", { token: S.base.token, axis }));
    } catch (e) {
      toast(`无法切换视图：${e}`);
      if (prev === "xy") S.base = null;
      return;
    }
    z = Math.floor(info.sizeZ / 2);
  }
  stopPlay();
  // A side view needs every slice in memory; show progress until it draws.
  if (axis === "xy") sideDone();
  else S.sideWait = true;
  holdFrame();
  S.axis = axis;
  S.token = token;
  S.info = info;
  S.z = z;
  S.frame = null;
  S.hist = null;
  S.picked = null;
  S.op = null;
  S.cropBox = null;
  S.lastDrawn = null;
  showSize(null);
  if (axis === "xy") {
    Object.assign(S, { ann: S.base.ann, history: S.base.history, future: S.base.future, model: S.base.model, crop: S.base.crop, cropStack: S.base.cropStack });
    S.base = null;
  } else {
    Object.assign(S, { ann: [], history: [], future: [], model: null, crop: null, cropStack: [] });
  }
  const ps = info.pixelSize;
  if (S.pxSource !== "手动" && ps && ps[0] > 0 && UNIT_M[normUnit(info.unit)]) {
    S.px = { x: ps[0], y: ps[1] > 0 ? ps[1] : ps[0], unit: normUnit(info.unit) };
  }
  applyZSpacing();
  const view = axis === "xy" ? "" : ` (${axis.toUpperCase()})`;
  T.window.getCurrentWindow().setTitle(`${info.name}${view} — ${EDITION.name}`).catch(() => {});
  setupSliders();
  syncScaleUi();
  updateInfo();
  specOpened();
  fit();
  watchLoad();
}

/**
 * Shows how much of a Z stack is in memory (side views are instant once it
 * all is), and the progress of a side view that is still being built.
 */
function watchLoad() {
  if (S.loadTimer) return;
  const tick = async () => {
    const token = S.axis === "xy" ? S.token : S.base && S.base.token;
    let done = true;
    if (token != null && S.info) {
      try {
        const [have, total, fits] = await invoke("load_progress", { token });
        const pct = Math.floor((have / Math.max(1, total)) * 100);
        const loading = total > 1 && fits && have < total;
        $("st-cache").textContent = loading ? `已载入内存 ${pct}%` : "";
        if (S.sideWait) {
          // Slices read so far for the side view being built.
          const [d, n] = await invoke("load_progress", { token: S.token });
          if (S.sideWait) {
            $("loading").textContent = `正在生成 ${S.axis.toUpperCase()} 视图… ${d} / ${n} 层`;
            $("loading").style.setProperty("--p", `${(d / Math.max(1, n)) * 100}%`);
            $("loading").classList.add("progress");
            $("loading").hidden = false;
          }
        }
        done = !loading && !S.sideWait;
      } catch (e) {
        // The image was closed or replaced; the next open restarts this.
      }
    }
    S.loadTimer = done ? null : setTimeout(tick, 250);
  };
  S.loadTimer = setTimeout(tick, 0);
}

function sideDone() {
  if (!S.sideWait) return;
  S.sideWait = false;
  $("loading").hidden = true;
  $("loading").classList.remove("progress");
}

function normUnit(u) {
  if (!u) return "";
  if (u === "um" || u === "micron" || u === "µm" || u === "μm") return "µm";
  if (u === "A" || u === "Å" || u === "angstrom") return "Å";
  const r = /^(?:1\/(.+)|(.+?)(?:\^-1|⁻¹|-1))$/.exec(u.trim());
  if (r) {
    const base = normUnit(r[1] || r[2]);
    if (base === "Å" || base === "nm" || base === "µm") return `1/${base}`;
  }
  return u;
}

/** Where the open image is in the folder list: a stack's entry is its
 * project file, or its folder when the project file is further up. */
function folderIndex(path) {
  const { files } = S.folder;
  if (!(S.info && S.info.folder)) return files.indexOf(path);
  const i = files.indexOf(S.info.path);
  return i >= 0 ? i : files.indexOf(S.info.folder);
}

async function refreshFolder(path) {
  // A stack made of a folder's slices lists that folder; the stack itself
  // is in the list when a project file describes it.
  const stackDir = S.info && S.info.folder;
  const dir = stackDir || dirname(path);
  if (dir !== S.folder.dir) {
    const f = await invoke("list_folder", { path: stackDir || path });
    const stacks = f.stacks || [];
    const files = [...stacks.map((s) => s.path), ...f.files];
    S.folder = {
      dir,
      files,
      sizes: [...stacks.map(() => 0), ...f.sizes],
      stacks: Object.fromEntries(stacks.map((s) => [s.path, s])),
      index: -1,
    };
    $("folder-name").textContent = basename(dir) || dir;
    $("folder-name").title = dir;
    $("side-head").hidden = false;
    S.folder.index = folderIndex(path);
    updateSaveDir();
    buildStrip();
  } else {
    S.folder.index = folderIndex(path);
    if (S.folder.index < 0 && !stackDir) {
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

/** Opens a folder of slices (FIB-SEM, serial sections…) as one Z stack. */
async function pickStackFolder() {
  const dir = await invoke("pick_folder", { dir: S.info ? dirname(S.info.folder || S.info.path) : "" });
  if (dir) openPath(dir);
}

async function takePending() {
  const p = await invoke("take_pending_open");
  if (p) openPath(p);
}

// ---------- channels ----------

/** The picker for files that hold several images (LIF projects, ND2 positions…). */
function buildSeries() {
  const sel = $("series");
  const list = (S.info && S.info.series) || [];
  sel.hidden = list.length < 2;
  sel.innerHTML = "";
  list.forEach((s, i) => {
    const o = document.createElement("option");
    o.value = i;
    o.textContent = `${i + 1}/${list.length}  ${s.name}`;
    o.title = s.detail ? `${s.name}\n${s.detail}` : s.name;
    sel.appendChild(o);
  });
  if (list.length > 1) sel.value = S.info.seriesIndex;
}

$("series").addEventListener("change", (e) => {
  if (S.info) openPath(S.info.path, false, +e.target.value);
  e.target.blur();
});

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
  if (!S.info || $("tools").hidden) {
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
  g.fillStyle = `rgba(${color === "255,255,255" ? "140,149,159" : color},0.85)`;
  h.bins.forEach((v, i) => {
    const y = (Math.log1p(v) / max) * hc.height;
    g.fillRect(i, hc.height - y, 1, y);
  });
  if (c.min !== null && h.max > h.min) {
    const xs = [c.min, c.max].map((v) => ((v - h.min) / (h.max - h.min)) * hc.width);
    g.strokeStyle = "#1f232899";
    g.beginPath();
    g.moveTo(xs[0], hc.height);
    g.lineTo(xs[1], 0);
    g.stroke();
    g.fillStyle = "#0969da";
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
  const stack = (S.base ? S.base.info : S.info).sizeZ > 1;
  $("btn-axis").hidden = $("axis-sep").hidden = !stack;
  $("axis-label").textContent = S.axis.toUpperCase();
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
  $("zname").textContent = { xy: "Z", xz: "Y", yz: "X" }[S.axis];
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
  // While Z changes quickly, draw at half resolution, then sharpen once it stops.
  S.scrubbing = true;
  clearTimeout(S.scrubTimer);
  S.scrubTimer = setTimeout(() => {
    S.scrubbing = false;
    requestRender();
  }, 250);
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
  const { files, index } = S.folder;
  $("file-pos").textContent = i.folder && index < 0 ? `堆叠 · ${files.length}` : files.length ? `${index + 1} / ${files.length}` : "–";
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
  const stackInfo = S.base ? S.base.info : S.info;
  $("pz-row").hidden = !(S.px && stackInfo && stackInfo.sizeZ > 1);
  $("pz-size").value = S.pz ? fmt(S.pz) : "";
  $("pz-unit").textContent = S.px ? S.px.unit : "";
  $("sb-show").checked = S.scaleBar;
  $("sb-show").disabled = !S.px;
  $("btn-scalebar").classList.toggle("on", S.scaleBar && !!S.px);
}

/** In a side view (XZ / YZ) the vertical axis is Z: its pixels are one slice apart. */
function applyZSpacing() {
  if (!S.px) return;
  if (S.axis !== "xy") {
    if (S.pz) S.px = { ...S.px, y: S.pz };
  } else if (S.pxSource === "手动") {
    S.px = { ...S.px, y: S.px.x };
  }
}

function onPzInput() {
  const v = parseFloat($("pz-size").value);
  S.pz = Number.isFinite(v) && v > 0 ? v : null;
  if (S.axis !== "xy" && S.px) S.px = { ...S.px, y: S.pz || S.px.x };
  syncScaleUi();
  updateInfo();
  renderMeta();
  draw();
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
  renderMeta();
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
  let ox = 0, oy = 0;
  if (onScreen) {
    // Keep the bar on the image when it does not fill the window.
    const [bx, by, bw, bh] = bounds();
    const [x0, y0] = toScreen(bx, by);
    const [x1, y1] = toScreen(bx + bw, by + bh);
    ox = Math.max(0, x0);
    oy = Math.max(0, y0);
    outW = Math.min(outW, x1) - ox;
    outH = Math.min(outH, y1) - oy;
    if (outW < 60 || outH < 40) return;
  }
  const pxM = S.px.x * UNIT_M[S.px.unit];
  const target = onScreen ? 120 : outW * 0.15;
  const len = niceLength((target / k) * pxM);
  const barPx = (len / pxM) * k;
  const thick = onScreen ? 4 : Math.max(3, Math.round(outH * 0.008));
  const font = onScreen ? 13 : Math.max(12, Math.round(outH * 0.025));
  const margin = onScreen ? 18 : Math.max(10, Math.round(outW * 0.025));
  const x1 = ox + outW - margin;
  const x0 = x1 - barPx;
  const y = oy + outH - margin;
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
  if (S.info) clampView();
  draw();
  requestRender();
}

function cssSize() {
  const r = canvas.getBoundingClientRect();
  return { cw: r.width, ch: r.height };
}

/** Canvas area available beside the floating tools panel, in CSS pixels. */
function freeRect() {
  const r = canvas.getBoundingClientRect();
  const panel = $("tools");
  let x0 = 0;
  if (!panel.hidden) {
    const inset = Math.max(0, panel.getBoundingClientRect().right - r.left + 10);
    if (r.width - inset >= 200) x0 = inset;
  }
  return { x0, x1: r.width, y0: 0, y1: r.height };
}

function clampView() {
  if (!S.info) return;
  const { cw, ch } = cssSize();
  const { x0, x1, y0, y1 } = freeRect();
  const [x, y, w, h] = bounds();
  const dpr = window.devicePixelRatio || 1;
  const fitScale = Math.min((x1 - x0) / w, (y1 - y0) / h);
  const minScale = Math.min(fitScale, 1 / dpr) / 2;
  S.view.scale = Math.min(64, Math.max(minScale, S.view.scale));
  const scale = S.view.scale;
  const clampAxis = (c, start, size, canvasSize, lo, hi) => {
    const overlap = Math.min(size * scale, (hi - lo) / 2);
    // Screen right >= lo + overlap, and screen left <= hi - overlap.
    const min = start + (canvasSize / 2 - hi + overlap) / scale;
    const max = start + size + (canvasSize / 2 - lo - overlap) / scale;
    return Math.min(max, Math.max(min, c));
  };
  S.view.cx = clampAxis(S.view.cx, x, w, cw, x0, x1);
  S.view.cy = clampAxis(S.view.cy, y, h, ch, y0, y1);
}

function fit() {
  if (!S.info) return;
  const { cw, ch } = cssSize();
  const { x0, x1, y0, y1 } = freeRect();
  const [x, y, w, h] = bounds();
  S.view.scale = Math.min((x1 - x0) / w, (y1 - y0) / h);
  S.view.cx = x + w / 2 - ((x0 + x1) / 2 - cw / 2) / S.view.scale;
  S.view.cy = y + h / 2 - ((y0 + y1) / 2 - ch / 2) / S.view.scale;
  draw();
  requestRender();
}

function zoomBy(k) {
  if (!S.info) return;
  S.view.scale *= k;
  clampView();
  draw();
  requestRender();
}

function oneToOne() {
  if (!S.info) return;
  S.view.scale = 1 / (window.devicePixelRatio || 1);
  clampView();
  draw();
  requestRender();
}

/** Visible part of the image, in image pixels. */
function visibleRegion() {
  const { cw, ch } = cssSize();
  const { cx, cy, scale } = S.view;
  const [bx, by, bw, bh] = bounds();
  const x0 = Math.max(bx, Math.floor(cx - cw / 2 / scale));
  const y0 = Math.max(by, Math.floor(cy - ch / 2 / scale));
  const x1 = Math.min(bx + bw, Math.ceil(cx + cw / 2 / scale));
  const y1 = Math.min(by + bh, Math.ceil(cy + ch / 2 / scale));
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
  if (isRecip(S.px.unit) && m > 0) {
    // From the direct beam to a spot: the lattice spacing is d = 1/|g|.
    const d = 1 / m;
    return { px, text: `${fmtLength(m)} · d = ${fmtLength(d, d < 1e-9 ? "Å" : "nm")}  (${fmt(+px.toFixed(1))} px)` };
  }
  return { px, text: `${fmtLength(m)}  (${fmt(+px.toFixed(1))} px)` };
}

/**
 * Draws annotations. `map` turns image coordinates into context
 * coordinates and `unit` is the size of one screen pixel in the context.
 */
function drawAnnotations(g, map, unit, items) {
  for (const a of items) {
    if (a.type === "text") {
      if (!S.textEdit || S.textEdit.item !== a) drawText(g, a, map, unit);
      continue;
    }
    if (a.pts.length < 2) continue;
    g.save();
    g.strokeStyle = a.color;
    g.lineWidth = a.width * unit;
    g.lineCap = "round";
    g.lineJoin = "round";
    g.beginPath();
    if (a.type === "rect" || a.type === "ellipse") {
      const [x0, y0] = map(...a.pts[0]);
      const [x1, y1] = map(...a.pts[1]);
      if (a.type === "rect") g.rect(Math.min(x0, x1), Math.min(y0, y1), Math.abs(x1 - x0), Math.abs(y1 - y0));
      else g.ellipse((x0 + x1) / 2, (y0 + y1) / 2, Math.abs(x1 - x0) / 2, Math.abs(y1 - y0) / 2, 0, 0, Math.PI * 2);
    } else if (a.type === "arrow") {
      drawArrow(g, map(...a.pts[0]), map(...a.pts[1]), a.width * unit);
    } else {
      a.pts.forEach((p, i) => {
        const [x, y] = map(p[0], p[1]);
        i ? g.lineTo(x, y) : g.moveTo(x, y);
      });
    }
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
      drawAnnLabel(g, label, (x0 + x1) / 2, (y0 + y1) / 2 - 6 * unit, unit, a.color);
    } else if (a.type === "rect" || a.type === "ellipse") {
      // Width and height above the shape, like the length of a measurement.
      const [x0, y0] = map(...a.pts[0]);
      const [x1, y1] = map(...a.pts[1]);
      const [, , w, h] = rectOf(a);
      if (w >= 1 && h >= 1) {
        // Width over the top edge, height beside the right edge.
        const l = shapeLabels(a.type, w, h);
        const top = Math.min(y0, y1) - 6 * unit;
        if (l.d) drawAnnLabel(g, l.d, (x0 + x1) / 2, top, unit, a.color);
        else {
          drawAnnLabel(g, l.w, (x0 + x1) / 2, top, unit, a.color);
          drawAnnLabel(g, l.h, Math.max(x0, x1) + 10 * unit, (y0 + y1) / 2 + 9 * unit, unit, a.color, "left");
        }
      }
    }
    g.restore();
  }
}

/** A line from `a` to `b` with a filled head at `b`; leaves the shaft as the current path. */
function drawArrow(g, [x0, y0], [x1, y1], lw) {
  const len = Math.hypot(x1 - x0, y1 - y0);
  const head = Math.min(len, 6 + lw * 3.5);
  const ang = Math.atan2(y1 - y0, x1 - x0);
  // The shaft stops inside the head so its round cap does not poke out of the tip.
  const k = len ? (len - head * 0.7) / len : 0;
  g.save();
  g.fillStyle = g.strokeStyle;
  g.beginPath();
  g.moveTo(x1, y1);
  g.lineTo(x1 - head * Math.cos(ang - 0.42), y1 - head * Math.sin(ang - 0.42));
  g.lineTo(x1 - head * Math.cos(ang + 0.42), y1 - head * Math.sin(ang + 0.42));
  g.closePath();
  g.fill();
  g.restore();
  g.beginPath();
  g.moveTo(x0, y0);
  g.lineTo(x0 + (x1 - x0) * k, y0 + (y1 - y0) * k);
}

/** Font size of a text annotation in screen pixels; it follows the line width. */
function textSize(a) {
  return 10 + a.width * 3;
}

function textFont(a, unit) {
  return `600 ${textSize(a) * unit}px -apple-system, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif`;
}

/** Text with a dark outline, so it reads on bright and dark images alike. */
function drawText(g, a, map, unit) {
  const [x, y] = map(...a.pts[0]);
  g.save();
  g.font = textFont(a, unit);
  g.textBaseline = "top";
  g.lineJoin = "round";
  g.strokeStyle = "rgba(0, 0, 0, 0.6)";
  g.lineWidth = 3 * unit;
  g.strokeText(a.text, x, y);
  g.fillStyle = a.color;
  g.fillText(a.text, x, y);
  g.restore();
}

function drawAnnLabel(g, label, mx, my, unit, color, align = "center") {
  g.font = `600 ${13 * unit}px -apple-system, "Segoe UI", sans-serif`;
  g.textAlign = align;
  g.textBaseline = "bottom";
  const w = g.measureText(label).width;
  const left = align === "left" ? mx : mx - w / 2;
  g.fillStyle = "#000a";
  g.fillRect(left - 4 * unit, my - 16 * unit, w + 8 * unit, 18 * unit);
  g.fillStyle = color;
  g.fillText(label, mx, my);
}

/** {w, h} labels, or {d} ("⌀ 120 nm") for a circle. Pixels when the pixel size is unknown. */
function shapeLabels(type, w, h) {
  let lw = `${Math.round(w)} px`;
  let lh = `${Math.round(h)} px`;
  if (S.px) {
    // Both sides in the unit of the longer one.
    const m = UNIT_M[S.px.unit];
    const [mw, mh] = [w * S.px.x * m, h * S.px.y * m];
    const u = lengthUnit(Math.max(mw, mh));
    [lw, lh] = [fmtLength(mw, u), fmtLength(mh, u)];
  }
  if (type === "ellipse" && lw === lh) return { d: `⌀ ${lw}` };
  return { w: lw, h: lh };
}

// ---------- IMOD models ----------

function setModel(m) {
  S.model = m;
  const box = $("mod-objects");
  box.innerHTML = "";
  if (!m) {
    $("mod-info").textContent = "同名的 .mod / .fid 会自动加载";
    return;
  }
  const npts = m.objects.reduce((a, o) => a + o.contours.reduce((b, c) => b + c.pts.length / 3, 0), 0);
  let info = `${basename(m.path)} · ${m.objects.length} 个对象 · ${npts} 个点`;
  const [mx, my] = m.size;
  if (S.info && mx > 0 && my > 0 && (Math.abs(mx - S.info.width) > 1 || Math.abs(my - S.info.height) > 1)) {
    info += `（模型建在 ${mx}×${my} 的图上，和当前图像大小不同）`;
  }
  $("mod-info").textContent = info;
  m.objects.forEach((o, i) => {
    o.visible = !o.hidden;
    const el = document.createElement("label");
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = o.visible;
    cb.addEventListener("change", () => {
      o.visible = cb.checked;
      draw();
    });
    const sw = document.createElement("span");
    sw.className = "sw";
    sw.style.background = `rgb(${o.color.join(",")})`;
    const nm = document.createElement("span");
    nm.textContent = `${o.name || `对象 ${i + 1}`}（${o.contours.length}）`;
    el.append(cb, sw, nm);
    box.append(el);
  });
}

async function loadModelFile(path) {
  try {
    setModel(await invoke("load_model", { path }));
    S.modelShow = $("mod-show").checked = true;
    draw();
  } catch (e) {
    toast(`无法读取模型：${e}`);
  }
}

async function autoLoadModel(imagePath) {
  try {
    const found = await invoke("find_models", { path: imagePath });
    if (found.length && S.info && S.info.path === imagePath) {
      await loadModelFile(found[0]);
      toast(`已加载 IMOD 模型 ${basename(found[0])}`);
    }
  } catch (_) {}
}

/** Draws the model objects that cross the current Z slice. */
function drawModel(g, map, k, unit, z = S.z) {
  const m = S.model;
  if (!m || !S.modelShow) return;
  g.save();
  g.lineJoin = "round";
  for (const o of m.objects) {
    if (!o.visible) continue;
    const color = `rgb(${o.color.join(",")})`;
    g.strokeStyle = color;
    g.fillStyle = color;
    g.lineWidth = Math.max(1, o.lineWidth) * unit;
    for (const c of o.contours) {
      const p = c.pts;
      const n = p.length / 3;
      if (!o.scattered) {
        // Planar contours: connect consecutive points on this slice.
        g.beginPath();
        let on = 0;
        let prev = false;
        for (let i = 0; i < n; i++) {
          const here = Math.abs(p[i * 3 + 2] - z) < 0.5;
          if (here) {
            const [x, y] = map(p[i * 3], p[i * 3 + 1]);
            prev ? g.lineTo(x, y) : g.moveTo(x, y);
            on++;
          }
          prev = here;
        }
        if (!o.open && on === n && n > 2) g.closePath();
        if (on === 1) {
          const i = [...Array(n).keys()].find((j) => Math.abs(p[j * 3 + 2] - z) < 0.5);
          const [x, y] = map(p[i * 3], p[i * 3 + 1]);
          g.moveTo(x + 2 * unit, y);
          g.arc(x, y, 2 * unit, 0, Math.PI * 2);
        }
        g.stroke();
      }
      // Points as spheres: full size on their slice, smaller nearby.
      if (o.scattered || o.pointSize > 0) {
        for (let i = 0; i < n; i++) {
          const r = (c.sizes && c.sizes[i]) || o.pointSize || 3;
          const dz = Math.abs(p[i * 3 + 2] - z);
          if (dz >= Math.max(r, 0.5)) continue;
          const rr = Math.sqrt(Math.max(0, r * r - dz * dz)) || 1;
          const [x, y] = map(p[i * 3], p[i * 3 + 1]);
          g.beginPath();
          g.arc(x, y, Math.max(2 * unit, rr * k), 0, Math.PI * 2);
          g.stroke();
        }
      }
    }
  }
  g.restore();
}

function draw() {
  updateZMarks();
  const dpr = window.devicePixelRatio || 1;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  if (!S.frame || !S.info) return drawFade();
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  const [x, y, w, h] = S.frame.region;
  const [sx, sy] = toScreen(x, y);
  if (S.ov && S.ov.token === S.token) {
    const [ox, oy, ow, oh] = S.ov.region;
    const [qx, qy] = toScreen(ox, oy);
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(ovCanvas, 0, 0, S.ov.w, S.ov.h, qx, qy, ow * S.view.scale, oh * S.view.scale);
  }
  ctx.imageSmoothingEnabled = S.view.scale * dpr < 1;
  ctx.drawImage(frameCanvas, 0, 0, S.frame.w, S.frame.h, sx, sy, w * S.view.scale, h * S.view.scale);

  // Overlays stay inside the shown part of the image.
  const [bx, by] = toScreen(bounds()[0], bounds()[1]);
  ctx.save();
  ctx.beginPath();
  ctx.rect(bx, by, bounds()[2] * S.view.scale, bounds()[3] * S.view.scale);
  ctx.clip();

  // Model coordinates are pixel indices; +0.5 puts them on pixel centres.
  drawModel(ctx, (x, y) => toScreen(x + 0.5, y + 0.5), S.view.scale, 1);
  for (const f of ext.overlays) f(ctx);

  const items = S.annShow ? S.ann.filter((a) => onSlice(a)) : [];
  const drawing = S.op && S.op.kind === "draw" && S.op.item.type !== "crop" ? [S.op.item] : [];
  drawAnnotations(ctx, toScreen, 1, items.concat(drawing));
  if (S.picked && S.annShow) drawHandles(S.picked);
  drawSpecPick();
  ctx.restore();
  if (S.cropBox) drawCropBox(S.cropBox);

  // Size of what is being drawn, next to it.
  const live = S.op && S.op.kind !== "pan" ? S.op.item : null;
  if (live && live.pts.length > 1) {
    const [x1, y1] = toScreen(...live.pts[live.pts.length - 1]);
    drawTag(ctx, sizeText(live), x1 + 12, y1 + 18);
  }

  const { cw, ch } = cssSize();
  drawScaleBar(ctx, cw, ch, S.view.scale, true);
  drawFade();
  placeTextEdit();
  const pct = `${Math.round(S.view.scale * dpr * 100)}%`;
  $("st-zoom").textContent = pct;
  $("btn-zoom").textContent = pct;
}

/** Dashed outline and grab points of the picked annotation. */
function drawHandles(a) {
  ctx.save();
  if (!TWO_POINT.has(a.type)) {
    const [x0, y0, x1, y1] = annBox(a);
    const [sx, sy] = toScreen(x0, y0);
    ctx.setLineDash([4, 3]);
    ctx.strokeStyle = "#0969da";
    ctx.lineWidth = 1;
    ctx.strokeRect(sx - 3, sy - 3, (x1 - x0) * S.view.scale + 6, (y1 - y0) * S.view.scale + 6);
    ctx.setLineDash([]);
  } else {
    // A glow along a picked line, so the pick shows at a glance.
    ctx.strokeStyle = "rgba(9, 105, 218, 0.35)";
    ctx.lineWidth = GRIP * 2;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(...toScreen(...a.pts[0]));
    ctx.lineTo(...toScreen(...a.pts[a.pts.length - 1]));
    ctx.stroke();
  }
  for (const h of handles(a)) {
    const [x, y] = toScreen(...h.at);
    ctx.fillStyle = "#fff";
    ctx.strokeStyle = "#0969da";
    ctx.lineWidth = TOUCH ? 2.5 : 1.5;
    ctx.beginPath();
    if (h.dir) ctx.rect(x - GRIP + 1, y - GRIP + 1, 2 * GRIP - 2, 2 * GRIP - 2);
    else ctx.arc(x, y, GRIP, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
  }
  ctx.restore();
}

/** The crop rectangle: dims everything outside it. */
function drawCropBox(b) {
  const { cw, ch } = cssSize();
  const [x0, y0, x1, y1] = bbox(b.pts);
  const [sx, sy] = toScreen(x0, y0);
  const w = (x1 - x0) * S.view.scale;
  const h = (y1 - y0) * S.view.scale;
  ctx.save();
  ctx.fillStyle = "rgba(31, 35, 40, 0.5)";
  ctx.beginPath();
  ctx.rect(0, 0, cw, ch);
  ctx.rect(sx, sy, w, h);
  ctx.fill("evenodd");
  ctx.strokeStyle = "#fff";
  ctx.lineWidth = 1.5;
  ctx.strokeRect(sx, sy, w, h);
  // Rule-of-thirds guides, as in photo croppers.
  ctx.strokeStyle = "rgba(255, 255, 255, 0.45)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  for (const f of [1 / 3, 2 / 3]) {
    ctx.moveTo(sx + w * f, sy);
    ctx.lineTo(sx + w * f, sy + h);
    ctx.moveTo(sx, sy + h * f);
    ctx.lineTo(sx + w, sy + h * f);
  }
  ctx.stroke();
  ctx.restore();
  if (!S.op) drawHandles(b);
  if (!S.op) drawTag(ctx, `${boxText(x1 - x0, y1 - y0)}　双击裁剪`, sx + 4, sy + h + 20);
}

/** A small label with a dark background at screen position x, y (baseline). */
function drawTag(g, text, x, y) {
  g.save();
  g.font = `600 12px -apple-system, "Segoe UI", sans-serif`;
  const w = g.measureText(text).width;
  const { cw, ch } = cssSize();
  x = Math.max(4, Math.min(cw - w - 12, x));
  y = Math.max(18, Math.min(ch - 6, y));
  g.fillStyle = "rgba(31, 35, 40, 0.85)";
  g.beginPath();
  g.roundRect(x - 6, y - 15, w + 12, 21, 5);
  g.fill();
  g.fillStyle = "#fff";
  g.fillText(text, x, y);
  g.restore();
}

function requestRender() {
  if (!S.info) return;
  if (S.inflight) {
    S.dirty = true;
    return;
  }
  renderNow();
}

/** Re-renders the whole-image underlay when the slice or display changed. */
async function refreshOverview() {
  const region = bounds();
  // Same contrast as the sharp view, so the two blend without a seam.
  const channels = channelReqs().map((c) => ({ ...c, min: S.chans[c.index].min, max: S.chans[c.index].max }));
  const key = JSON.stringify([S.token, S.z, S.t, region, channels, S.gamma]);
  if (S.ovKey === key || S.ovBusy) return;
  const k = Math.min(1, OV_SIZE / Math.max(region[2], region[3]));
  const outW = Math.max(1, Math.round(region[2] * k));
  const outH = Math.max(1, Math.round(region[3] * k));
  const token = S.token;
  S.ovBusy = true;
  try {
    const req = { token, z: S.z, t: S.t, channels, region, outW, outH, gamma: S.gamma, invert: false };
    const f = parseFrame(await invoke("render_view", { req }));
    if (token !== S.token) return;
    ovCanvas.width = f.w;
    ovCanvas.height = f.h;
    ovCanvas.getContext("2d").putImageData(f.image, 0, 0);
    S.ov = { token, region, w: f.w, h: f.h };
    draw();
  } catch (e) {
    // The underlay is only a nicety; the sharp view reports real errors.
  } finally {
    S.ovKey = key;
    S.ovBusy = false;
  }
  // Catch up if the view moved on while this was rendering.
  if (!S.scrubbing) refreshOverview();
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
  const k = Math.min(scale, 1) * (S.scrubbing ? 0.5 : 1);
  const outW = Math.max(1, Math.round(region[2] * k));
  const outH = Math.max(1, Math.round(region[3] * k));
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
    sideDone();
    startFade();
    loadHist();
    if (!S.scrubbing) refreshOverview();
    pluginEmit("render");
    scheduleFft();
    scheduleSpec();
  } catch (e) {
    if (String(e) !== "stale") toast(`渲染失败：${e}`);
    if (token === S.token) {
      sideDone();
      startFade();
    }
  } finally {
    S.inflight = false;
    if (S.dirty) requestRender();
  }
}

// ---------- FFT / power spectrum ----------

/** Power spectrum of the visible region, for lattice spacings and Thon rings. */
const F = { on: false, timer: 0, busy: false, again: false, key: "", data: null, hover: null };

function scheduleFft() {
  if (!F.on || !S.info) return;
  clearTimeout(F.timer);
  F.timer = setTimeout(computeFft, 150);
}

/** The channel the spectrum is computed from: the selected one if shown. */
function fftChannel() {
  if (S.chans[S.sel] && S.chans[S.sel].visible) return S.sel;
  const i = S.chans.findIndex((c) => c.visible);
  return i < 0 ? 0 : i;
}

async function computeFft() {
  const region = visibleRegion();
  if (!F.on || !S.info || !region || $("fft-panel").hidden) return;
  if (F.busy) {
    F.again = true;
    return;
  }
  const n = +$("fft-n").value;
  const c = fftChannel();
  const key = JSON.stringify([S.token, S.z, S.t, c, region, n]);
  if (key === F.key) return;
  const token = S.token;
  F.busy = true;
  try {
    const buf = await invoke("power_spectrum", { token, z: S.z, t: S.t, c, region, n });
    if (token !== S.token) return;
    const dv = new DataView(buf);
    const size = dv.getUint32(0, true);
    const all = new Float32Array(buf.slice(8));
    F.data = { n: size, tiles: dv.getUint32(4, true), log: all.subarray(0, size * size), radial: all.subarray(size * size) };
    F.key = key;
    drawFft();
  } catch (e) {
    if (String(e) !== "stale") {
      F.data = null;
      F.key = key;
      drawFft(String(e));
    }
  } finally {
    F.busy = false;
    if (F.again) {
      F.again = false;
      computeFft();
    }
  }
}

/** Display range of the spectrum: a percentile window that ignores the centre. */
function fftRange(log, n) {
  const sample = [];
  const step = Math.max(1, Math.floor((n * n) / 40000));
  for (let i = 0; i < n * n; i += step) {
    const x = (i % n) - n / 2;
    const y = Math.floor(i / n) - n / 2;
    if (x * x + y * y > 4) sample.push(log[i]);
  }
  sample.sort((a, b) => a - b);
  const at = (q) => sample[Math.min(sample.length - 1, Math.floor(q * sample.length))];
  return [at(0.02), at(0.998)];
}

function drawFft(error) {
  const img = $("fft-img");
  const g = img.getContext("2d");
  const d = F.data;
  if (!d) {
    g.clearRect(0, 0, img.width, img.height);
    $("fft-prof").getContext("2d").clearRect(0, 0, 9999, 9999);
    $("fft-info").textContent = error || "";
    return;
  }
  const { n, log } = d;
  img.width = img.height = n;
  const [lo, hi] = fftRange(log, n);
  const k = 255 / Math.max(1e-6, hi - lo);
  const im = g.createImageData(n, n);
  for (let i = 0; i < n * n; i++) {
    const v = Math.max(0, Math.min(255, (log[i] - lo) * k));
    im.data[i * 4] = im.data[i * 4 + 1] = im.data[i * 4 + 2] = v;
    im.data[i * 4 + 3] = 255;
  }
  g.putImageData(im, 0, 0);
  $("fft-info").textContent = `${n}×${n}，平均 ${d.tiles} 块`;
  drawFftProfile();
  showFftHover();
}

/** Spatial frequency (1/m per FFT pixel along x and y), or null without a length calibration. */
function fftFreqStep() {
  if (!S.px || isRecip(S.px.unit) || !F.data) return null;
  const m = UNIT_M[S.px.unit];
  return [1 / (F.data.n * S.px.x * m), 1 / (F.data.n * S.px.y * m)];
}

/** "f = … 1/nm · d = … nm" for an offset of (du, dv) FFT pixels from the centre. */
function fftReadout(du, dv) {
  const r = Math.hypot(du, dv);
  if (r < 0.5) return "";
  const step = fftFreqStep();
  if (!step) return `r = ${fmt(+r.toFixed(1))} · 周期 ${fmt(+(F.data.n / r).toPrecision(3))} px`;
  const f = Math.hypot(du * step[0], dv * step[1]);
  const per = 1 / f;
  return `${fmtLength(f, "1/nm")} · d = ${fmtLength(per, per < 1e-9 ? "Å" : per < 1e-6 ? "nm" : "µm")}`;
}

function drawFftProfile() {
  const c = $("fft-prof");
  const dpr = window.devicePixelRatio || 1;
  const r = c.getBoundingClientRect();
  c.width = Math.max(1, Math.round(r.width * dpr));
  c.height = Math.max(1, Math.round(r.height * dpr));
  const g = c.getContext("2d");
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  const W = r.width, H = r.height;
  g.clearRect(0, 0, W, H);
  if (!F.data) return;
  const p = F.data.radial;
  // Skip the centre, which only says how bright the image is.
  const vals = Array.from(p.subarray(2));
  const lo = Math.min(...vals), hi = Math.max(...vals);
  const x = (i) => (i / (p.length - 1)) * W;
  const y = (v) => H - 4 - ((v - lo) / Math.max(1e-6, hi - lo)) * (H - 8);
  g.strokeStyle = "#0969da";
  g.lineWidth = 1.25;
  g.beginPath();
  for (let i = 2; i < p.length; i++) (i === 2 ? g.moveTo : g.lineTo).call(g, x(i), y(p[i]));
  g.stroke();
  if (F.hover) {
    const rr = Math.hypot(F.hover[0], F.hover[1]);
    g.strokeStyle = "#cf222e";
    g.beginPath();
    g.moveTo(x(rr), 0);
    g.lineTo(x(rr), H);
    g.stroke();
  }
}

function showFftHover() {
  $("fft-read").textContent = F.hover && F.data ? fftReadout(F.hover[0], F.hover[1]) : "移动鼠标查看频率和间距";
}

function setFft(on) {
  F.on = on;
  $("fft-panel").hidden = !on;
  $("btn-fft").classList.toggle("on", on);
  pref.set("fft-on", on);
  if (on) {
    F.key = "";
    scheduleFft();
  }
}

$("btn-fft").addEventListener("click", () => setFft(!F.on));
$("fft-close").addEventListener("click", () => setFft(false));
$("fft-n").addEventListener("change", () => {
  pref.set("fft-n", $("fft-n").value);
  scheduleFft();
});
$("fft-img").addEventListener("mousemove", (e) => {
  if (!F.data) return;
  const r = e.currentTarget.getBoundingClientRect();
  const n = F.data.n;
  F.hover = [((e.clientX - r.left) / r.width) * n - n / 2, ((e.clientY - r.top) / r.height) * n - n / 2];
  showFftHover();
  drawFftProfile();
});
$("fft-prof").addEventListener("mousemove", (e) => {
  if (!F.data) return;
  const r = e.currentTarget.getBoundingClientRect();
  F.hover = [((e.clientX - r.left) / r.width) * (F.data.radial.length - 1), 0];
  showFftHover();
  drawFftProfile();
});
for (const id of ["fft-img", "fft-prof"]) {
  $(id).addEventListener("mouseleave", () => {
    F.hover = null;
    showFftHover();
    drawFftProfile();
  });
}

// ---------- spectra (EELS, EDS…) ----------

/**
 * Plot of the spectra in the file. With `axis` "x" each image row is a
 * spectrum; with "z" each pixel's spectrum runs through the slices. The
 * plot shows the picked row / pixel, or the mean of the visible area.
 */
const P = { on: false, pick: null, mode: "view", data: null, key: "", busy: false, again: false, timer: 0, zoom: null, hover: null, drag: null, log: false };

function specInfo() {
  return S.info && S.info.spectrum;
}

function specOpened() {
  const sp = specInfo();
  $("btn-spec").hidden = !sp;
  P.data = null;
  P.key = "";
  P.zoom = null;
  P.pick = null;
  // A single spectrum has only one row to show.
  P.mode = sp && sp.axis === "x" && S.info.height === 1 ? "pick" : "view";
  if (sp && sp.axis === "x" && S.info.height === 1) P.pick = [0, 0];
  const title = sp ? `${sp.kind ? sp.kind + " " : ""}谱` : "";
  $("spec-title").textContent = title;
  $("spec-view").hidden = !!(sp && sp.axis === "x" && S.info.height === 1);
  // Rows of spectra have no picture to Fourier transform.
  if (F.on) $("fft-panel").hidden = !!(sp && sp.axis === "x");
  setSpec(!!sp);
}

function setSpec(on) {
  P.on = on && !!specInfo();
  $("spec-panel").hidden = !P.on;
  $("btn-spec").classList.toggle("on", P.on);
  if (P.on) scheduleSpec();
}

function scheduleSpec() {
  if (!P.on) return;
  clearTimeout(P.timer);
  P.timer = setTimeout(computeSpec, 120);
}

/** The pixels whose mean is plotted, in image pixels. */
function specRegion() {
  const sp = specInfo();
  if (P.mode === "pick" && P.pick) {
    const [x, y] = P.pick;
    return sp.axis === "x" ? [0, y, S.info.width, 1] : [x, y, 1, 1];
  }
  const r = visibleRegion();
  return r && (sp.axis === "x" ? [0, r[1], S.info.width, r[3]] : r);
}

function specSource() {
  const sp = specInfo();
  if (P.mode === "pick" && P.pick) {
    if (sp.axis === "x") return S.info.height > 1 ? `第 ${P.pick[1] + 1} 行` : "";
    return `像素 (${P.pick[0]}, ${P.pick[1]})`;
  }
  return "当前视图平均";
}

async function computeSpec() {
  const sp = specInfo();
  const region = sp && specRegion();
  if (!P.on || !region) return;
  if (P.busy) {
    P.again = true;
    return;
  }
  const c = fftChannel();
  // A spectrum image's spectrum does not depend on the slice shown.
  const key = JSON.stringify([S.token, sp.axis === "z" ? 0 : S.z, S.t, c, region, sp.axis]);
  $("spec-src").textContent = specSource();
  $("spec-view").classList.toggle("on", P.mode === "view");
  if (key === P.key) return drawSpec();
  const token = S.token;
  P.busy = true;
  try {
    const v = await invoke("spectrum_values", { token, axis: sp.axis, c, z: S.z, t: S.t, region });
    if (token !== S.token) return;
    P.data = v;
    P.key = key;
    drawSpec();
  } catch (e) {
    if (String(e) !== "stale") toast(`读取谱失败：${e}`);
  } finally {
    P.busy = false;
    if (P.again) {
      P.again = false;
      computeSpec();
    }
  }
}

function specClick(pt) {
  const sp = specInfo();
  if (!P.on || !sp) return;
  const x = Math.floor(pt[0]);
  const y = Math.floor(pt[1]);
  if (x < 0 || y < 0 || x >= S.info.width || y >= S.info.height) return;
  P.pick = [x, y];
  P.mode = "pick";
  computeSpec();
  draw();
}

/** Marks the picked row or pixel on the image. */
function drawSpecPick() {
  const sp = specInfo();
  if (!P.on || !sp || P.mode !== "pick" || !P.pick || S.axis !== "xy") return;
  const [x, y] = P.pick;
  ctx.save();
  ctx.strokeStyle = "#cf222e";
  ctx.lineWidth = 1.5;
  if (sp.axis === "x") {
    if (S.info.height === 1) return ctx.restore();
    const [x0, y0] = toScreen(0, y);
    const [x1, y1] = toScreen(S.info.width, y + 1);
    ctx.strokeRect(x0, y0, x1 - x0, Math.max(2, y1 - y0));
  } else {
    const [x0, y0] = toScreen(x, y);
    const [x1, y1] = toScreen(x + 1, y + 1);
    const cx = (x0 + x1) / 2, cy = (y0 + y1) / 2;
    const r = Math.max(6, (x1 - x0) / 2 + 3);
    ctx.strokeRect(x0, y0, x1 - x0, y1 - y0);
    ctx.beginPath();
    ctx.moveTo(cx - r - 6, cy); ctx.lineTo(cx - r, cy);
    ctx.moveTo(cx + r, cy); ctx.lineTo(cx + r + 6, cy);
    ctx.moveTo(cx, cy - r - 6); ctx.lineTo(cx, cy - r);
    ctx.moveTo(cx, cy + r); ctx.lineTo(cx, cy + r + 6);
    ctx.stroke();
  }
  ctx.restore();
}

/** Axis value of channel i. */
function specX(i) {
  const sp = specInfo();
  return sp.start + i * sp.step;
}

/** About `n` round tick values between lo and hi. */
function ticks(lo, hi, n) {
  const span = hi - lo;
  if (!(span > 0)) return [lo];
  const step0 = span / n;
  const p = Math.pow(10, Math.floor(Math.log10(step0)));
  const f = step0 / p;
  const step = (f >= 5 ? 10 : f >= 2 ? 5 : f >= 1 ? 2 : 1) * p;
  const out = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9; v += step) out.push(+v.toPrecision(12));
  return out;
}

const SPEC_PAD = { l: 52, r: 12, t: 8, b: 22 };

/** Channel range shown: [i0, i1]. */
function specRange() {
  const n = P.data ? P.data.length : 1;
  return P.zoom || [0, n - 1];
}

function drawSpec() {
  const c = $("spec-plot");
  const dpr = window.devicePixelRatio || 1;
  const rect = c.getBoundingClientRect();
  c.width = Math.max(1, Math.round(rect.width * dpr));
  c.height = Math.max(1, Math.round(rect.height * dpr));
  const g = c.getContext("2d");
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  const W = rect.width, H = rect.height;
  g.clearRect(0, 0, W, H);
  const sp = specInfo();
  if (!P.data || !sp) return;
  const v = P.data;
  const [i0, i1] = specRange();
  const tf = (y) => (P.log ? Math.log10(Math.max(y, 0) + 1) : y);
  let lo = Infinity, hi = -Infinity;
  for (let i = Math.max(0, Math.floor(i0)); i <= Math.min(v.length - 1, Math.ceil(i1)); i++) {
    const y = tf(v[i]);
    if (y < lo) lo = y;
    if (y > hi) hi = y;
  }
  if (!(hi > lo)) hi = lo + 1;
  const pad = (hi - lo) * 0.05;
  lo -= P.log ? 0 : pad;
  hi += pad;
  const { l, r, t, b } = SPEC_PAD;
  const px = (i) => l + ((i - i0) / Math.max(1e-9, i1 - i0)) * (W - l - r);
  const py = (y) => t + (1 - (y - lo) / (hi - lo)) * (H - t - b);
  P.map = { px, i0, i1, l, r, W };
  // Axes and ticks.
  g.font = "11px -apple-system, 'Segoe UI', sans-serif";
  g.fillStyle = "#59636e";
  g.strokeStyle = "#d1d9e0";
  g.lineWidth = 1;
  const xa = specX(i0), xb = specX(i1);
  g.textAlign = "center";
  g.textBaseline = "top";
  for (const tv of ticks(Math.min(xa, xb), Math.max(xa, xb), Math.max(2, Math.floor((W - l - r) / 80)))) {
    const x = px((tv - sp.start) / sp.step);
    g.beginPath();
    g.moveTo(x, t);
    g.lineTo(x, H - b);
    g.stroke();
    g.fillText(fmt(tv), x, H - b + 4);
  }
  g.textAlign = "right";
  g.textBaseline = "middle";
  for (const tv of ticks(lo, hi, 4)) {
    const y = py(tv);
    g.beginPath();
    g.moveTo(l, y);
    g.lineTo(W - r, y);
    g.stroke();
    g.fillText(P.log ? `1e${fmt(tv)}` : fmt(+tv.toPrecision(4)), l - 6, y);
  }
  if (sp.unit) {
    g.textAlign = "right";
    g.textBaseline = "top";
    g.fillText(sp.unit, l - 6, H - b + 4);
  }
  // The spectrum.
  g.save();
  g.beginPath();
  g.rect(l, t, W - l - r, H - t - b);
  g.clip();
  g.strokeStyle = "#0969da";
  g.lineWidth = 1.25;
  g.beginPath();
  const a = Math.max(0, Math.floor(i0) - 1), z = Math.min(v.length - 1, Math.ceil(i1) + 1);
  // Draw at most a few points per screen pixel: min and max of each column.
  const perPx = (z - a) / Math.max(1, W - l - r);
  if (perPx > 2) {
    for (let x = l; x <= W - r; x++) {
      const ia = Math.max(a, Math.floor(i0 + ((x - l) / (W - l - r)) * (i1 - i0)));
      const ib = Math.min(z, Math.floor(i0 + ((x + 1 - l) / (W - l - r)) * (i1 - i0)));
      let mn = Infinity, mx = -Infinity;
      for (let i = ia; i <= Math.max(ia, ib); i++) {
        const y = tf(v[i]);
        if (y < mn) mn = y;
        if (y > mx) mx = y;
      }
      g.moveTo(x, py(mx));
      g.lineTo(x, py(mn) + 0.01);
    }
  } else {
    for (let i = a; i <= z; i++) (i === a ? g.moveTo : g.lineTo).call(g, px(i), py(tf(v[i])));
  }
  g.stroke();
  if (P.drag) {
    g.fillStyle = "rgba(9, 105, 218, 0.12)";
    const [d0, d1] = [Math.min(...P.drag), Math.max(...P.drag)];
    g.fillRect(d0, t, d1 - d0, H - t - b);
  }
  if (sp.axis === "z") {
    // The slice shown in the image.
    g.strokeStyle = "#1a7f37";
    g.setLineDash([4, 3]);
    g.beginPath();
    g.moveTo(px(S.z), t);
    g.lineTo(px(S.z), H - b);
    g.stroke();
    g.setLineDash([]);
  }
  if (P.hover != null) {
    g.strokeStyle = "#cf222e";
    g.beginPath();
    g.moveTo(px(P.hover), t);
    g.lineTo(px(P.hover), H - b);
    g.stroke();
  }
  g.restore();
  showSpecHover();
}

function showSpecHover() {
  const sp = specInfo();
  const i = P.hover;
  if (!P.data || !sp || i == null || i < 0 || i >= P.data.length) {
    $("spec-read").textContent = P.zoom ? "拖动选择范围放大，双击还原" : "拖动选择范围放大";
    return;
  }
  const val = P.data[i];
  $("spec-read").textContent = `${fmt(+specX(i).toPrecision(6))}${sp.unit ? " " + sp.unit : ""} · ${fmt(+val.toPrecision(6))}（通道 ${i}）`;
}

/** Channel under the mouse. */
function specIndex(e) {
  const m = P.map;
  if (!m) return null;
  const x = e.clientX - $("spec-plot").getBoundingClientRect().left;
  const i = m.i0 + ((x - m.l) / Math.max(1, m.W - m.l - m.r)) * (m.i1 - m.i0);
  return Math.round(Math.max(0, Math.min(P.data.length - 1, i)));
}

$("btn-spec").addEventListener("click", () => setSpec(!P.on));
$("spec-close").addEventListener("click", () => setSpec(false));
$("spec-view").addEventListener("click", () => {
  P.mode = P.mode === "view" ? "pick" : "view";
  if (P.mode === "pick" && !P.pick) P.pick = [Math.floor(S.info.width / 2), Math.floor(S.info.height / 2)];
  computeSpec();
  draw();
});
$("spec-log").addEventListener("click", () => {
  P.log = !P.log;
  $("spec-log").classList.toggle("on", P.log);
  drawSpec();
});
$("spec-plot").addEventListener("pointerdown", (e) => {
  if (!P.data) return;
  const x = e.clientX - e.currentTarget.getBoundingClientRect().left;
  P.drag = [x, x];
  e.currentTarget.setPointerCapture(e.pointerId);
});
$("spec-plot").addEventListener("pointermove", (e) => {
  if (!P.data) return;
  P.hover = specIndex(e);
  if (P.drag) P.drag[1] = e.clientX - e.currentTarget.getBoundingClientRect().left;
  drawSpec();
});
$("spec-plot").addEventListener("pointerup", (e) => {
  if (!P.drag) return;
  const [a, b] = P.drag;
  P.drag = null;
  if (Math.abs(b - a) > 6) {
    const m = P.map;
    const at = (x) => m.i0 + ((x - m.l) / Math.max(1, m.W - m.l - m.r)) * (m.i1 - m.i0);
    const i0 = Math.max(0, Math.min(at(a), at(b)));
    const i1 = Math.min(P.data.length - 1, Math.max(at(a), at(b)));
    if (i1 - i0 >= 2) P.zoom = [i0, i1];
  } else if (specInfo().axis === "z") {
    // A click shows the image at that energy.
    const i = specIndex(e);
    if (i != null) setZ(i);
  }
  drawSpec();
});
$("spec-plot").addEventListener("dblclick", () => {
  P.zoom = null;
  drawSpec();
});
$("spec-plot").addEventListener("mouseleave", () => {
  P.hover = null;
  drawSpec();
});
window.addEventListener("resize", () => P.on && drawSpec());

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
  drawModel(g, (ix, iy) => [(ix + 0.5 - x) * k, (iy + 0.5 - y) * k], k, unit, z);
  if (S.annShow) drawAnnotations(g, (ix, iy) => [(ix - x) * k, (iy - y) * k], unit, S.ann.filter((a) => onSlice(a, z)));
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
  const z = S.info.sizeZ > 1 ? `_${{ xy: "z", xz: "y", yz: "x" }[S.axis]}${S.z + 1}` : "";
  return `${stem(S.info.path)}${z}${S.crop ? "_crop" : ""}`;
}

/** Folder that saved images go to: the one chosen by the user, else the image's own. */
function saveDir() {
  return pref.get("save-dir", null) || dirname(S.info.path);
}

function setSaveDir(dir) {
  pref.set("save-dir", dir || null);
  updateSaveDir();
}

/** Shows the save folder in the tools panel and the sidebar button for it. */
function updateSaveDir() {
  const dir = pref.get("save-dir", null);
  $("save-dir").textContent = dir || "图片所在文件夹";
  $("save-dir").title = dir || "";
  $("save-dir-reset").disabled = !dir;
  $("btn-reveal-save").hidden = !(dir && S.info && dir !== dirname(S.info.path));
}

async function pickSaveDir() {
  const dir = await invoke("pick_folder", { dir: S.info ? saveDir() : pref.get("save-dir", "") || "" });
  if (dir) setSaveDir(dir);
}

// The first save asks where to put the image; later saves go to that folder.
async function saveAs() {
  if (!S.info) return;
  const region = bounds();
  const first = !pref.get("save-dir", null);
  const path = await invoke("pick_save_path", { dir: saveDir(), name: exportName() + ".png" });
  if (!path) return;
  if (first) setSaveDir(dirname(path));
  await doSave(region, path);
}

async function quickSave() {
  if (!S.info) return;
  const region = bounds();
  const path = await invoke("free_path", { dir: saveDir(), stem: exportName() });
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
  $("btn-save").disabled = !S.info;
}

// ---------- tools / interaction ----------

const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
let wheelZ = 0;

const TOOL_KEYS = {
  v: "pan", s: "select", m: "measure", p: "pen", l: "segment", a: "arrow", r: "rect", e: "ellipse", x: "text", c: "crop",
};
const DRAW_TYPES = {
  measure: "line", pen: "path", segment: "segment", arrow: "arrow", rect: "rect", ellipse: "ellipse", crop: "crop",
};
// Annotations made of two end points rather than a box.
const TWO_POINT = new Set(["line", "segment", "arrow"]);

// The pen draws free lines, straight lines, arrows, rectangles or ellipses;
// the toolbar button shows the current shape and its menu picks another.
const PEN_SHAPES = ["pen", "segment", "arrow", "rect", "ellipse"];
const PEN_ICONS = {
  pen: '<path d="M4 20l4-1L19 8l-3-3L5 16z"/><path d="M14 7l3 3"/>',
  segment: '<path d="M5 19L19 5"/>',
  arrow: '<path d="M5 19L19 5"/><path d="M10 5h9v9"/>',
  rect: '<rect x="4" y="6" width="16" height="12" rx="1"/>',
  ellipse: '<ellipse cx="12" cy="12" rx="8.5" ry="7"/>',
};
const PEN_NAMES = { pen: "画笔", segment: "直线", arrow: "箭头", rect: "画框", ellipse: "画圆" };
const PEN_KEYS = { pen: "P", segment: "L", arrow: "A", rect: "R", ellipse: "E" };
const PEN_TITLES = {
  pen: "画笔 (P)",
  segment: "直线 (L)，按住 Shift 对齐 45°",
  arrow: "箭头 (A)，按住 Shift 对齐 45°",
  rect: "画框 (R)，按住 Shift 画正方形",
  ellipse: "画圆 (E)，按住 Shift 画正圆",
};

function setTool(t) {
  if (S.textEdit) finishText(true);
  closeMenu();
  S.tool = t;
  const pen = PEN_SHAPES.includes(t);
  if (pen) {
    S.penShape = t;
    pref.set("pen", t);
    const b = document.querySelector('#toolbtns [data-tool="pen"]');
    b.querySelector("svg").innerHTML = PEN_ICONS[t];
    b.title = PEN_TITLES[t];
    for (const o of document.querySelectorAll("#pen-shapes button")) o.classList.toggle("on", o.dataset.shape === t);
  }
  for (const b of document.querySelectorAll("#toolbtns [data-tool]")) b.classList.toggle("on", b.dataset.tool === t || (pen && b.dataset.tool === "pen"));
  $("pen-split").classList.toggle("on", pen);
  canvas.classList.toggle("cross", t !== "pan" && t !== "select" && t !== "text");
  canvas.classList.toggle("text", t === "text");
  if (t !== "select") S.picked = null;
  if (t !== "crop") S.cropBox = null;
  for (const [name, x] of Object.entries(ext.tools)) if (x.active) x.active(name === t);
  draw();
}

// ---------- drop-down menus ----------

let menuAnchor = null;

/**
 * Opens the drop-down menu under `anchor`. Items are {value, label, icon?,
 * hint?, key?, on?}; `pick` gets the chosen value.
 */
function openMenu(anchor, items, pick) {
  const m = $("dropmenu");
  m.innerHTML = "";
  for (const it of items) {
    const b = document.createElement("button");
    b.setAttribute("role", "menuitemradio");
    b.setAttribute("aria-checked", it.on ? "true" : "false");
    b.classList.toggle("on", !!it.on);
    if (it.icon) b.insertAdjacentHTML("beforeend", `<svg viewBox="0 0 24 24">${it.icon}</svg>`);
    const text = document.createElement("span");
    text.className = "mi-label";
    text.textContent = it.label;
    b.append(text);
    if (it.hint) {
      const h = document.createElement("span");
      h.className = "mi-hint";
      h.textContent = it.hint;
      b.append(h);
    }
    if (it.key) {
      const k = document.createElement("kbd");
      k.textContent = it.key;
      b.append(k);
    }
    b.addEventListener("click", () => {
      closeMenu();
      pick(it.value);
    });
    m.append(b);
  }
  menuAnchor = anchor;
  anchor.classList.add("open");
  m.hidden = false;
  const r = anchor.getBoundingClientRect();
  m.style.top = `${r.bottom + 6}px`;
  m.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - m.offsetWidth - 8))}px`;
}

/** Closes the menu; true if one was open (for `anchor`, when given, so a second click toggles it shut). */
function closeMenu(anchor) {
  const was = menuAnchor;
  if (!was) return false;
  menuAnchor = null;
  was.classList.remove("open");
  $("dropmenu").hidden = true;
  return !anchor || was === anchor;
}

document.addEventListener(
  "pointerdown",
  (e) => {
    if (menuAnchor && !$("dropmenu").contains(e.target) && !menuAnchor.contains(e.target)) closeMenu();
  },
  true,
);
window.addEventListener("blur", () => closeMenu());
window.addEventListener("resize", () => closeMenu());
$("quickbar").addEventListener("scroll", () => closeMenu());
$("dropmenu").addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    closeMenu();
    e.stopPropagation();
  }
});

// ---------- text annotations ----------

/** Opens the in-place editor for text annotation `item` (`isNew`: not yet in S.ann). */
function editText(item, isNew) {
  const el = $("text-edit");
  S.textEdit = { item, isNew, snap: snapshot() };
  S.picked = null;
  showSize(null);
  el.value = item.text;
  el.hidden = false;
  placeTextEdit();
  draw();
  // After the press that opened it has moved focus, or it would blur at once.
  setTimeout(() => {
    if (S.textEdit && S.textEdit.item !== item) return;
    el.focus();
    el.select();
  });
}

/** Keeps the editor over its text while the view moves. */
function placeTextEdit() {
  const t = S.textEdit;
  if (!t) return;
  const el = $("text-edit");
  const [x, y] = toScreen(...t.item.pts[0]);
  const size = textSize(t.item);
  el.style.font = textFont(t.item, 1);
  el.style.color = t.item.color;
  // The padding and border are taken off so the typed text sits where it will be drawn.
  el.style.left = `${x - 5}px`;
  el.style.top = `${y - 3}px`;
  el.style.height = `${Math.round(size * 1.2) + 6}px`;
  ctx.save();
  ctx.font = textFont(t.item, 1);
  const w = ctx.measureText(el.value || "M").width;
  ctx.restore();
  el.style.width = `${Math.ceil(w) + 24}px`;
}

/** Ends text editing; `ok` keeps what was typed. Empty text removes the annotation. */
function finishText(ok) {
  const t = S.textEdit;
  if (!t) return;
  S.textEdit = null;
  const el = $("text-edit");
  const v = el.value.trim();
  el.hidden = true;
  el.blur();
  if (!ok || v === t.item.text || (t.isNew && !v)) return draw();
  pushHistory(t.snap);
  if (!v) S.ann = S.ann.filter((a) => a !== t.item);
  else {
    t.item.text = v;
    if (t.isNew) {
      S.ann.push(t.item);
      S.lastDrawn = t.item;
    }
  }
  annChanged();
}

$("text-edit").addEventListener("input", placeTextEdit);
$("text-edit").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.isComposing) finishText(true);
  else if (e.key === "Escape") finishText(false);
});
$("text-edit").addEventListener("blur", () => finishText(true));

/** Shows the side panel: "contrast" (only brightness/contrast), "info"
 * (only file and acquisition metadata), "all", or null. */
function showPanel(mode) {
  const panel = $("tools");
  const cur = panel.hidden
    ? null
    : panel.classList.contains("only-contrast")
      ? "contrast"
      : panel.classList.contains("only-info")
        ? "info"
        : "all";
  if (mode === cur) mode = null;
  panel.hidden = !mode;
  panel.classList.toggle("only-contrast", mode === "contrast");
  panel.classList.toggle("only-info", mode === "info");
  $("btn-tools").classList.toggle("on", mode === "all");
  $("btn-contrast").classList.toggle("on", mode === "contrast");
  $("btn-info").classList.toggle("on", mode === "info");
  pref.set("tools", mode);
  if (S.info) {
    clampView();
    draw();
    requestRender();
  }
  if (mode === "info") renderMeta();
  else if (mode) loadHist();
}

// ---------- image info / acquisition metadata ----------

/** File facts plus the groups the reader found (e.g. Zeiss SEM settings). */
function metaGroups() {
  const i = S.info;
  if (!i) return [];
  const dims = [`${i.width} × ${i.height}`];
  if (i.sizeC > 1) dims.push(`C ${i.sizeC}`);
  if (i.sizeZ > 1) dims.push(`Z ${i.sizeZ}`);
  if (i.sizeT > 1) dims.push(`T ${i.sizeT}`);
  const items = [
    ["格式", i.format],
    ["尺寸", dims.join(" · ")],
    ["数据类型", i.dtype],
  ];
  if (S.px) items.push(["像素尺寸", `${fmt(S.px.x)} ${S.px.unit}`]);
  const pz = S.axis === "xy" ? S.pz : null;
  if (S.px && pz) items.push(["Z 间距", `${fmt(pz)} ${S.px.unit}`]);
  items.push(["路径", i.path]);
  const groups = [...(i.metadata || []), ...S.pluginMeta];
  const common = commonParams(groups);
  return [{ name: "文件信息", items }, ...(common ? [common] : []), ...groups];
}

/**
 * The settings people look for first, in one fixed order whatever wrote
 * the file: [label, names the readers use for it]. Readers should use the
 * first name where they can (see docs/format-plugins.md).
 */
const COMMON_PARAMS = [
  ["显微镜", ["显微镜", "系统类型", "仪器"]],
  ["加速电压", ["加速电压", "EHT", "高压"]],
  ["模式", ["操作模式", "模式", "TEM 子模式", "成像模式", "照明模式"]],
  ["放大倍数", ["放大倍数", "标称放大倍数", "实际放大倍数", "总放大倍数", "Mag"]],
  ["相机长度", ["相机长度"]],
  ["视场", ["视野", "视场", "水平视场 (HFW)"]],
  ["工作距离", ["工作距离", "WD"]],
  ["探测器 / 相机", ["探测器", "相机", "相机型号", "检测器", "Signal A", "Signal"]],
  ["曝光时间", ["曝光时间", "曝光时间 (ms)", "积分时间", "帧时间", "扫描帧时间"]],
  ["驻留时间", ["驻留时间", "像素驻留 (µs)"]],
  ["剂量", ["剂量", "总剂量", "剂量率"]],
  ["散焦", ["散焦", "设定散焦"]],
  ["球差 Cs", ["球差 Cs"]],
  ["束斑", ["束斑", "聚光镜束斑", "探针尺寸", "Spot Size"]],
  ["束流", ["探针电流", "束流", "发射电流", "I Probe", "Beam Current"]],
  ["会聚半角", ["会聚半角"]],
  ["倾转", ["α 倾转", "样品台 α", "样品台倾转", "α / β 倾转"]],
  ["像素合并", ["像素合并"]],
  ["采集时间", ["采集时间", "日期时间", "采集日期", "时间", "日期", "Date"]],
];

/** "常用参数": the COMMON_PARAMS the reader found, or null when it found few. */
function commonParams(groups) {
  const items = [];
  for (const [label, names] of COMMON_PARAMS) {
    let hit = null;
    for (const n of names) {
      for (const g of groups) {
        const it = g.items.find(([k, v]) => k === n && String(v).trim() !== "");
        if (it) {
          hit = it[1];
          // Readers that split the date from the time of day.
          const date = n === "采集时间" && g.items.find(([k]) => k === "采集日期");
          if (date && !hit.includes(date[1])) hit = `${date[1]} ${hit}`;
          break;
        }
      }
      if (hit != null) break;
    }
    if (hit != null) items.push([label, hit]);
  }
  if (S.px && items.length) items.splice(Math.min(items.length, 4), 0, ["像素尺寸", `${fmt(S.px.x)} ${S.px.unit}`]);
  return items.length >= 2 ? { name: "常用参数", items } : null;
}

function renderMeta() {
  const list = $("meta-list");
  if ($("tools").hidden || !$("tools").classList.contains("only-info")) return;
  const q = $("meta-filter").value.trim().toLowerCase();
  // Keep what the user expanded while the same file stays open.
  const same = S.info && list.dataset.file === S.info.path;
  const prev = new Map(same ? [...list.querySelectorAll("details")].map((d) => [d.dataset.name, d.open]) : []);
  list.textContent = "";
  let shown = 0;
  metaGroups().forEach((g, gi) => {
    const items = q ? g.items.filter(([k, v]) => `${k} ${v}`.toLowerCase().includes(q)) : g.items;
    if (!items.length) return;
    shown += items.length;
    const d = document.createElement("details");
    d.dataset.name = g.name;
    // Expand the summaries by default and everything that matches a search.
    d.open = q ? items.length <= 60 : prev.has(g.name) ? prev.get(g.name) : gi < 2;
    const sum = document.createElement("summary");
    const name = document.createElement("span");
    name.textContent = g.name;
    const n = document.createElement("span");
    n.className = "dim";
    n.textContent = String(items.length);
    sum.append(name, n);
    const table = document.createElement("table");
    for (const [k, v] of items) {
      const tr = table.insertRow();
      tr.insertCell().textContent = k;
      tr.insertCell().textContent = v;
      tr.title = `${k} = ${v}`;
    }
    d.append(sum, table);
    list.append(d);
  });
  list.dataset.file = S.info ? S.info.path : "";
  if (!shown) {
    const p = document.createElement("div");
    p.className = "dim none";
    p.textContent = q ? "没有匹配的参数" : "未打开图像";
    list.append(p);
  }
}

function copyMeta() {
  const text = metaGroups()
    .map((g) => `[${g.name}]\n` + g.items.map(([k, v]) => `${k} = ${v}`).join("\n"))
    .join("\n\n");
  navigator.clipboard.writeText(text).then(
    () => toast("已复制全部参数"),
    (e) => toast(`复制失败：${e}`),
  );
}

function toggleSidebar(force) {
  const sb = $("sidebar");
  const show = force ?? sb.classList.contains("collapsed");
  sb.classList.toggle("collapsed", !show);
  $("sidebar-resizer").hidden = !show;
  $("btn-sidebar").classList.toggle("on", show);
  pref.set("sidebar", show);
  if (show) markStrip();
}

/** The part of the image that is shown: the crop, or all of it. */
function bounds() {
  return S.crop || [0, 0, S.info.width, S.info.height];
}

function bbox(pts) {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const [x, y] of pts) {
    x0 = Math.min(x0, x); y0 = Math.min(y0, y);
    x1 = Math.max(x1, x); y1 = Math.max(y1, y);
  }
  return [x0, y0, x1, y1];
}

function rectOf(item) {
  const [x0, y0, x1, y1] = bbox(item.pts);
  return [x0, y0, x1 - x0, y1 - y0];
}

/** Annotations belong to the Z slice they were drawn on. */
function onSlice(a, z = S.z) {
  return !S.info || S.info.sizeZ <= 1 || a.z === undefined || (z >= a.z && z <= (a.z1 ?? a.z));
}

function distToSeg(p, a, b) {
  const dx = b[0] - a[0], dy = b[1] - a[1];
  const l2 = dx * dx + dy * dy;
  const t = l2 ? Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2)) : 0;
  return Math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy);
}

/** Whether point `p` (image pixels) touches the outline of `a`, within `tol`. */
function hits(a, p, tol) {
  const [x0, y0, x1, y1] = annBox(a);
  if (p[0] < x0 - tol || p[0] > x1 + tol || p[1] < y0 - tol || p[1] > y1 + tol) return false;
  if (a.type === "text") return true;
  if (a.type === "rect") {
    return !(p[0] > x0 + tol && p[0] < x1 - tol && p[1] > y0 + tol && p[1] < y1 - tol);
  }
  if (a.type === "ellipse") {
    const rx = (x1 - x0) / 2, ry = (y1 - y0) / 2;
    if (rx < tol || ry < tol) return true;
    const d = Math.hypot((p[0] - x0 - rx) / rx, (p[1] - y0 - ry) / ry);
    return Math.abs(d - 1) * Math.min(rx, ry) < tol;
  }
  for (let i = 1; i < a.pts.length; i++) {
    if (distToSeg(p, a.pts[i - 1], a.pts[i]) < tol) return true;
  }
  return false;
}

/** Image-space box of an annotation; text takes its size on screen. */
function annBox(a) {
  if (a.type !== "text") return bbox(a.pts);
  ctx.save();
  ctx.font = textFont(a, 1);
  const w = ctx.measureText(a.text).width;
  ctx.restore();
  const [x, y] = a.pts[0];
  return [x, y, x + w / S.view.scale, y + (textSize(a) * 1.2) / S.view.scale];
}

/** Drag handles: the two ends of a line, none for text, otherwise the 8 points of the box. */
function handles(a) {
  if (TWO_POINT.has(a.type)) return a.pts.map((p, i) => ({ at: p, i }));
  if (a.type === "text") return [];
  const [x0, y0, x1, y1] = bbox(a.pts);
  const xm = (x0 + x1) / 2, ym = (y0 + y1) / 2;
  return [
    [x0, y0, "nw"], [xm, y0, "n"], [x1, y0, "ne"], [x1, ym, "e"],
    [x1, y1, "se"], [xm, y1, "s"], [x0, y1, "sw"], [x0, ym, "w"],
  ].map(([x, y, dir]) => ({ at: [x, y], dir }));
}

const HANDLE_CURSOR = { n: "ns", s: "ns", e: "ew", w: "ew", nw: "nwse", se: "nwse", ne: "nesw", sw: "nesw" };

function handleAt(a, p, tol) {
  return handles(a).find((h) => Math.hypot(h.at[0] - p[0], h.at[1] - p[1]) < tol * 1.6) || null;
}

function inBox(a, p, tol) {
  // A line's box is invisible and mostly empty: only the line itself counts.
  if (TWO_POINT.has(a.type)) return hits(a, p, tol);
  const [x0, y0, x1, y1] = annBox(a);
  return p[0] >= x0 - tol && p[0] <= x1 + tol && p[1] >= y0 - tol && p[1] <= y1 + tol;
}

/** Topmost annotation on this slice under `p`. */
function annAt(p, tol) {
  for (let i = S.ann.length - 1; i >= 0; i--) {
    const a = S.ann[i];
    if (onSlice(a) && hits(a, p, tol)) return a;
  }
  return null;
}

function clampToImage(p) {
  const [bx, by, bw, bh] = bounds();
  return [Math.max(bx, Math.min(bx + bw, p[0])), Math.max(by, Math.min(by + bh, p[1]))];
}

/** "w × h" of a box in physical units when the pixel size is known. */
function boxText(w, h) {
  const pxT = `${Math.round(w)} × ${Math.round(h)} px`;
  if (!S.px) return pxT;
  const m = UNIT_M[S.px.unit];
  const [mw, mh] = [w * S.px.x * m, h * S.px.y * m];
  const u = lengthUnit(Math.max(mw, mh));
  return `${fmtLength(mw, u)} × ${fmtLength(mh, u)}  (${pxT})`;
}

function sizeText(a) {
  if (a.type === "text") return "文字";
  if (TWO_POINT.has(a.type)) return lineLength(a.pts[0], a.pts[a.pts.length - 1]).text;
  const [, , w, h] = rectOf(a);
  return (a.type === "crop" ? "裁剪 " : "") + boxText(w, h);
}

function showSize(a) {
  $("size-out").hidden = !a;
  $("size-text").textContent = a ? sizeText(a) : "";
  const zb = $("ann-z");
  const isAnn = a && a.type !== "crop" && a.z !== undefined && S.info && S.info.sizeZ > 1;
  zb.hidden = !isAnn;
  if (a !== S.zEdit) {
    S.zEdit = null;
    $("ann-z-edit").hidden = true;
  }
  if (isAnn) {
    zb.textContent = zRangeText(a);
    zb.hidden = !!S.zEdit;
  }
  if (a && a.type === "line") $("measure-out").textContent = sizeText(a);
  $("ann-del").hidden = !a || a !== S.picked;
  if (!a || a !== S.picked) $("ann-menu").hidden = true;
}

// Undo history: every change to annotations or the crop first saves a
// snapshot of both.
function snapshot() {
  return { ann: JSON.stringify(S.ann), crop: S.crop, cropStack: S.cropStack.slice() };
}

function pushHistory(snap) {
  styled = null;
  S.future = [];
  S.history.push(snap);
  if (S.history.length > 200) S.history.shift();
}

// A tool extension guards its own gestures; S.op "ext" can outlive a
// pointerup the webview never delivered, and must not block undo then.
const midGesture = () => S.op && !(S.op.kind === "ext" && ext.tools[S.tool]);

function undo() {
  if (midGesture()) return;
  if (ext.tools[S.tool] && ext.tools[S.tool].undo) return ext.tools[S.tool].undo();
  const h = S.history.pop();
  if (!h) return;
  S.future.push(snapshot());
  restore(h);
}

function redo() {
  if (midGesture()) return;
  if (ext.tools[S.tool] && ext.tools[S.tool].redo) return ext.tools[S.tool].redo();
  const h = S.future.pop();
  if (!h) return;
  S.history.push(snapshot());
  restore(h);
}

function restore(h) {
  const annChange = h.ann !== JSON.stringify(S.ann);
  S.ann = JSON.parse(h.ann);
  S.picked = null;
  S.lastDrawn = null;
  S.cropBox = null;
  showSize(null);
  if (h.crop !== S.crop) {
    S.crop = h.crop;
    S.cropStack = h.cropStack;
    S.frame = null;
    updateSaveButtons();
    fit();
  }
  if (annChange) annChanged();
  else draw();
}

function deletePicked() {
  $("ann-menu").hidden = true;
  if (!S.picked) return false;
  pushHistory(snapshot());
  S.ann = S.ann.filter((a) => a !== S.picked);
  S.picked = null;
  showSize(null);
  annChanged();
  return true;
}

function toggleAnn(force) {
  S.annShow = force ?? !S.annShow;
  $("ann-show").checked = S.annShow;
  $("btn-annshow").classList.toggle("on", S.annShow);
  draw();
}

// ---------- annotation file ----------

/** Redraws and saves the annotations a moment after the last change. */
function zRangeText(a) {
  const z1 = a.z1 ?? a.z;
  if (a.z === 0 && z1 === S.info.sizeZ - 1) return "全部 Z";
  return a.z === z1 ? `Z ${a.z + 1}` : `Z ${a.z + 1}–${z1 + 1}`;
}

/** Sets the Z range of the picked annotation (0-based, inclusive). */
function setAnnZ(a, z0, z1) {
  const n = S.info.sizeZ;
  z0 = Math.max(0, Math.min(n - 1, Math.round(z0)));
  z1 = Math.max(0, Math.min(n - 1, Math.round(z1)));
  if (z1 < z0) [z0, z1] = [z1, z0];
  if (a.z === z0 && (a.z1 ?? a.z) === z1) return;
  pushHistory(snapshot());
  a.z = z0;
  if (z1 === z0) delete a.z1;
  else a.z1 = z1;
  annChanged();
  if (!onSlice(a)) setZ(z0);
  showSize(a);
  if (S.zEdit === a) {
    $("ann-z0").value = z0 + 1;
    $("ann-z1").value = z1 + 1;
  }
}

$("ann-z").addEventListener("click", () => {
  const a = S.picked;
  if (!a) return;
  S.zEdit = a;
  $("ann-z0").max = $("ann-z1").max = S.info.sizeZ;
  $("ann-z0").value = a.z + 1;
  $("ann-z1").value = (a.z1 ?? a.z) + 1;
  $("ann-z").hidden = true;
  $("ann-z-edit").hidden = false;
  $("ann-z0").focus();
});
for (const id of ["ann-z0", "ann-z1"])
  $(id).addEventListener("change", () => S.zEdit && setAnnZ(S.zEdit, $("ann-z0").value - 1, $("ann-z1").value - 1));
$("ann-z-one").addEventListener("click", () => S.zEdit && setAnnZ(S.zEdit, S.z, S.z));
$("ann-z-all").addEventListener("click", () => S.zEdit && setAnnZ(S.zEdit, 0, S.info.sizeZ - 1));
$("ann-del").addEventListener("click", () => deletePicked());
$("ann-menu-del").addEventListener("click", () => deletePicked());
for (const el of [$("ann-z0"), $("ann-z1")])
  el.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === "Escape") {
      el.blur();
      S.zEdit = null;
      $("ann-z-edit").hidden = true;
      showSize(S.picked);
    }
  });

/** Marks on the Z slider where annotations are shown. */
let zMarksKey = "";
function updateZMarks() {
  const n = S.info ? S.info.sizeZ : 0;
  const items = n > 1 ? S.ann.filter((a) => a.z !== undefined) : [];
  const key = n + "|" + items.map((a) => `${a.z},${a.z1 ?? a.z},${a.color}`).join(";");
  if (key === zMarksKey) return;
  zMarksKey = key;
  const box = $("zmarks");
  box.innerHTML = "";
  for (const a of items) {
    const i = document.createElement("i");
    const f0 = a.z / (n - 1);
    const f1 = (a.z1 ?? a.z) / (n - 1);
    i.style.left = `${f0 * 100}%`;
    i.style.width = `calc(${(f1 - f0) * 100}% + 6px)`;
    i.style.background = a.color;
    box.append(i);
  }
}

function annChanged() {
  draw();
  // Side views share the file name; their marks are not saved.
  if (!S.info || S.axis !== "xy") return;
  S.annPending = { path: S.info.path, series: S.info.seriesIndex, size: [S.info.width, S.info.height] };
  clearTimeout(S.annTimer);
  S.annTimer = setTimeout(flushAnn, 2000);
}

async function flushAnn() {
  clearTimeout(S.annTimer);
  const p = S.annPending;
  if (!p) return;
  S.annPending = null;
  // z1 is the last Z of a range; it is absent for a single slice.
  const items = S.ann.map(({ type, z, z1, color, width, pts, text }) => ({
    type, z, z1, color, width, text, pts: pts.map(([x, y]) => [+x.toFixed(2), +y.toFixed(2)]),
  }));
  const json = items.length
    ? JSON.stringify({ app: "TifMrcView", version: 1, image: basename(p.path), size: p.size, items })
    : "";
  try {
    await invoke("write_annotations", { image: p.path, series: p.series, json });
    if (S.info && S.info.path === p.path && S.info.seriesIndex === p.series) {
      $("ann-file").textContent = items.length ? `已自动保存到 ${annName(p.path, p.series)}` : "";
    }
  } catch (e) {
    toast(`标注保存失败：${e}`, 6000);
  }
}

/** The annotations file of an image (see annot_path in lib.rs). */
function annName(path, series) {
  return `${basename(path)}${series > 0 ? `.series${series + 1}` : ""}.annot.json`;
}

async function loadAnn(path, series) {
  const token = S.token;
  let text = null;
  try {
    text = await invoke("read_annotations", { image: path, series });
  } catch (_) {}
  if (token !== S.token) return;
  S.ann = [];
  if (text) {
    try {
      const items = JSON.parse(text).items || [];
      const ok = new Set(["line", "path", "segment", "arrow", "rect", "ellipse", "text"]);
      S.ann = items.filter(
        (a) => ok.has(a.type) && Array.isArray(a.pts) && a.pts.length && (a.type !== "text" || typeof a.text === "string"),
      );
    } catch (_) {
      toast(`${annName(path, series)} 读取失败`);
    }
  }
  $("ann-file").textContent = text
    ? `从 ${annName(path, series)} 读入 ${S.ann.length} 个标注`
    : "标注会在画完几秒后自动存到图片旁边的 .annot.json 文件";
  draw();
}

// ---------- crop ----------

function applyCrop() {
  const b = S.cropBox;
  if (!b) return;
  const [x, y, w, h] = rectOf(b).map(Math.round);
  S.cropBox = null;
  showSize(null);
  if (w < 2 || h < 2) return draw();
  pushHistory(snapshot());
  S.cropStack.push(S.crop);
  S.crop = [x, y, w, h];
  S.frame = null;
  updateSaveButtons();
  fit();
  toast(`已裁剪到 ${w} × ${h} px，按 Esc 或退格撤回`);
}

function undoCrop() {
  if (!S.crop) return false;
  pushHistory(snapshot());
  S.crop = S.cropStack.pop() ?? null;
  S.frame = null;
  updateSaveButtons();
  fit();
  return true;
}

// ---------- pointer ----------

function setCursor(p) {
  let c = "";
  if (S.tool === "select" || S.tool === "crop") {
    const tol = HIT / S.view.scale;
    const target = S.tool === "crop" ? S.cropBox : S.picked;
    const h = target && handleAt(target, p, tol);
    // Ends of a line get a crosshair, so they read differently from moving the whole line.
    if (h) c = h.dir ? `${HANDLE_CURSOR[h.dir]}-resize` : "crosshair";
    else if (target && inBox(target, p, tol)) c = "move";
    else if (S.tool === "select") {
      // Pressing an unpicked annotation acts on it straight away, so show what will happen.
      const a = annAt(p, tol);
      const ha = a && handleAt(a, p, tol);
      if (ha) c = ha.dir ? `${HANDLE_CURSOR[ha.dir]}-resize` : "crosshair";
      else if (a) c = "move";
    }
  } else if (S.tool === "measure") {
    const tol = HIT / S.view.scale;
    if (S.ann.some((a) => a.type === "line" && onSlice(a) && handleAt(a, p, tol))) c = "crosshair";
  } else if (S.tool === "text") {
    const a = annAt(p, HIT / S.view.scale);
    c = a && a.type === "text" ? "pointer" : "text";
  }
  canvas.style.cursor = c;
}

// ---------- touch: two fingers pinch to zoom and drag to pan ----------
// Capture listeners on the canvas run before the tool handlers below, so a
// second finger can take over from whatever the first one started.
const touches = new Map();
let pinch = null;
// Holding a finger still on an annotation picks it and offers 删除.
let press = null;

function cancelPress() {
  if (press) clearTimeout(press.timer);
  press = null;
}

function longPress() {
  const { a, x, y } = press;
  press = null;
  // Whatever the finger started (a stroke, a drag) gives way to the menu.
  if (S.op && S.op.kind === "draw" && S.op.item === S.cropBox) S.cropBox = null;
  S.op = null;
  canvas.classList.remove("drag");
  if (S.tool !== "select") setTool("select");
  S.picked = a;
  showSize(a);
  draw();
  const r = $("stage").getBoundingClientRect();
  const m = $("ann-menu");
  m.style.left = `${x - r.left}px`;
  m.style.top = `${Math.max(56, y - r.top - 24)}px`;
  m.hidden = false;
  if (navigator.vibrate) navigator.vibrate(15);
}

function pinchCentre() {
  const [a, b] = [...touches.values()];
  const r = canvas.getBoundingClientRect();
  return { mx: (a.x + b.x) / 2 - r.left, my: (a.y + b.y) / 2 - r.top, d: Math.hypot(a.x - b.x, a.y - b.y) || 1 };
}

canvas.addEventListener(
  "pointerdown",
  (e) => {
    if (e.pointerType !== "touch") return;
    touches.set(e.pointerId, { x: e.clientX, y: e.clientY });
    $("ann-menu").hidden = true;
    cancelPress();
    if (touches.size === 1 && S.info && S.annShow) {
      const a = annAt(toImage(e), HIT / S.view.scale);
      if (a) press = { id: e.pointerId, x: e.clientX, y: e.clientY, a, timer: setTimeout(longPress, 500) };
    }
    if (touches.size !== 2 || !S.info) return;
    e.stopImmediatePropagation();
    // Drop the stroke or drag the first finger began.
    if (S.op && S.op.kind === "draw" && S.op.item === S.cropBox) S.cropBox = null;
    S.op = null;
    canvas.classList.remove("drag");
    const c = pinchCentre();
    const { cw, ch } = cssSize();
    pinch = { ...c, scale: S.view.scale, ix: S.view.cx + (c.mx - cw / 2) / S.view.scale, iy: S.view.cy + (c.my - ch / 2) / S.view.scale };
    draw();
  },
  true,
);

canvas.addEventListener(
  "pointermove",
  (e) => {
    if (!touches.has(e.pointerId)) return;
    touches.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (press && Math.hypot(e.clientX - press.x, e.clientY - press.y) > 10) cancelPress();
    if (!pinch) return;
    e.stopImmediatePropagation();
    if (touches.size < 2) return;
    const c = pinchCentre();
    const { cw, ch } = cssSize();
    S.view.scale = (pinch.scale * c.d) / pinch.d;
    clampView();
    S.view.cx = pinch.ix - (c.mx - cw / 2) / S.view.scale;
    S.view.cy = pinch.iy - (c.my - ch / 2) / S.view.scale;
    clampView();
    draw();
    requestRender();
  },
  true,
);

for (const type of ["pointerup", "pointercancel"]) {
  canvas.addEventListener(
    type,
    (e) => {
      if (press && press.id === e.pointerId) cancelPress();
      if (!touches.delete(e.pointerId) || !pinch) return;
      e.stopImmediatePropagation();
      // The finger left on the glass does nothing until it is lifted too.
      if (touches.size === 0) pinch = null;
    },
    true,
  );
}

canvas.addEventListener("pointerdown", (e) => {
  if (!S.info) return;
  // A click beside the text being typed finishes it.
  if (S.textEdit) {
    finishText(true);
    if (e.button === 0) return;
  }
  // Right-click with any other tool goes back to the select tool.
  if (e.button === 2 && S.tool !== "select") {
    S.op = null;
    setTool("select");
    showSize(null);
    return;
  }
  canvas.setPointerCapture(e.pointerId);
  if (S.tool === "pan" || e.button === 1 || e.button === 2) {
    S.op = { kind: "pan", x: e.clientX, y: e.clientY, cx: S.view.cx, cy: S.view.cy };
    canvas.classList.add("drag");
    return;
  }
  if (ext.tools[S.tool]) {
    S.op = { kind: "ext" };
    ext.tools[S.tool].down(e, toImage(e));
    return;
  }
  const p = toImage(e);
  const tol = HIT / S.view.scale;
  const snap = snapshot();

  // Handles and the body of the picked item (or of the crop box) come first.
  const target = S.tool === "crop" ? S.cropBox : S.tool === "select" ? S.picked : null;
  if (target) {
    const h = handleAt(target, p, tol);
    if (h) {
      S.op = h.dir
        ? { kind: "scale", item: target, dir: h.dir, orig: target.pts.map((q) => q.slice()), box: bbox(target.pts), snap }
        : { kind: "point", item: target, i: h.i, snap };
      return;
    }
    if (inBox(target, p, tol)) {
      S.op = { kind: "move", item: target, start: p, orig: target.pts.map((q) => q.slice()), snap };
      return;
    }
  }
  if (S.tool === "select") {
    const a = (S.picked = annAt(p, tol));
    showSize(a);
    if (a) {
      $("ann-color").value = a.color;
      $("ann-width").value = a.width;
      // Pressing on a grab point acts on it straight away.
      const h = handleAt(a, p, tol);
      S.op = !h
        ? { kind: "move", item: a, start: p, orig: a.pts.map((q) => q.slice()), snap }
        : h.dir
          ? { kind: "scale", item: a, dir: h.dir, orig: a.pts.map((q) => q.slice()), box: bbox(a.pts), snap }
          : { kind: "point", item: a, i: h.i, snap };
    } else {
      // Dragging empty space moves the image, as with the move tool.
      S.op = { kind: "pan", x: e.clientX, y: e.clientY, cx: S.view.cx, cy: S.view.cy };
      canvas.classList.add("drag");
    }
    draw();
    return;
  }
  if (S.tool === "measure") {
    // Grab the end of an existing measurement to measure again.
    for (let i = S.ann.length - 1; i >= 0; i--) {
      const a = S.ann[i];
      const h = a.type === "line" && onSlice(a) && handleAt(a, p, tol);
      if (h) {
        S.op = { kind: "point", item: a, i: h.i, snap };
        return;
      }
    }
  }
  if (S.tool === "text") {
    const a = annAt(p, tol);
    if (a && a.type === "text") editText(a, false);
    else {
      toggleAnn(true);
      editText({ type: "text", pts: [p], text: "", color: $("ann-color").value, width: +$("ann-width").value, z: S.z }, true);
    }
    e.preventDefault();
    return;
  }
  const type = DRAW_TYPES[S.tool];
  if (!type) return;
  const q = type === "crop" ? clampToImage(p) : p;
  const item = { type, pts: type === "path" ? [q] : [q, q] };
  if (type === "crop") {
    S.cropBox = item;
  } else {
    Object.assign(item, { color: $("ann-color").value, width: +$("ann-width").value, z: S.z });
    toggleAnn(true);
  }
  S.picked = null;
  S.op = { kind: "draw", item, snap };
});

canvas.addEventListener("contextmenu", (e) => e.preventDefault());

canvas.addEventListener("pointermove", (e) => {
  const op = S.op;
  showPixel(e);
  if (!op) {
    if (S.info) setCursor(toImage(e));
    return;
  }
  if (op.kind === "pan") {
    S.view.cx = op.cx - (e.clientX - op.x) / S.view.scale;
    S.view.cy = op.cy - (e.clientY - op.y) / S.view.scale;
    clampView();
    draw();
    requestRender();
    return;
  }
  if (op.kind === "ext") {
    if (ext.tools[S.tool]) ext.tools[S.tool].move(e, toImage(e));
    return;
  }
  let p = toImage(e);
  const it = op.item;
  if (it.type === "crop") p = clampToImage(p);
  if (op.kind === "draw") {
    if (it.type === "path") {
      const last = it.pts[it.pts.length - 1];
      if (Math.hypot(p[0] - last[0], p[1] - last[1]) * S.view.scale > 2) it.pts.push(p);
    } else {
      if (e.shiftKey && (it.type === "segment" || it.type === "arrow")) {
        // Snap to multiples of 45°.
        const a = it.pts[0];
        const len = Math.hypot(p[0] - a[0], p[1] - a[1]);
        const ang = Math.round(Math.atan2(p[1] - a[1], p[0] - a[0]) / (Math.PI / 4)) * (Math.PI / 4);
        p = [a[0] + len * Math.cos(ang), a[1] + len * Math.sin(ang)];
      } else if (e.shiftKey && it.type !== "line") {
        // Square / circle: use the longer side.
        const a = it.pts[0];
        const d = Math.max(Math.abs(p[0] - a[0]), Math.abs(p[1] - a[1]));
        p = [a[0] + Math.sign(p[0] - a[0] || 1) * d, a[1] + Math.sign(p[1] - a[1] || 1) * d];
      }
      it.pts[1] = p;
    }
  } else if (op.kind === "move") {
    let dx = p[0] - op.start[0], dy = p[1] - op.start[1];
    if (it.type === "crop") {
      // Keep the whole box inside the image.
      const [x0, y0, x1, y1] = bbox(op.orig);
      const [bx, by, bw, bh] = bounds();
      dx = Math.max(bx - x0, Math.min(bx + bw - x1, dx));
      dy = Math.max(by - y0, Math.min(by + bh - y1, dy));
    }
    it.pts = op.orig.map(([x, y]) => [x + dx, y + dy]);
  } else if (op.kind === "point") {
    it.pts[op.i] = p;
  } else if (op.kind === "scale") {
    let [x0, y0, x1, y1] = op.box;
    const [nx0, ny0, nx1, ny1] = [
      op.dir.includes("w") ? p[0] : x0, op.dir.includes("n") ? p[1] : y0,
      op.dir.includes("e") ? p[0] : x1, op.dir.includes("s") ? p[1] : y1,
    ];
    const sx = x1 > x0 ? (nx1 - nx0) / (x1 - x0) : 1;
    const sy = y1 > y0 ? (ny1 - ny0) / (y1 - y0) : 1;
    it.pts = op.orig.map(([x, y]) => [nx0 + (x - x0) * sx, ny0 + (y - y0) * sy]);
  }
  op.changed = true;
  showSize(it);
  draw();
});

function endPointer(e) {
  const op = S.op;
  S.op = null;
  if (!op) return;
  if (op.kind === "pan") {
    canvas.classList.remove("drag");
    // A click without dragging picks the spectrum shown in the plot.
    if (e && e.type === "pointerup" && Math.hypot(e.clientX - op.x, e.clientY - op.y) < 4) specClick(toImage(e));
    return;
  }
  if (op.kind === "ext") {
    if (ext.tools[S.tool]) ext.tools[S.tool].up();
    return;
  }
  const it = op.item;
  if (it.type === "crop") {
    const [, , w, h] = rectOf(it);
    if (w * S.view.scale < 4 || h * S.view.scale < 4) {
      S.cropBox = null;
      showSize(null);
    } else {
      // Normalise so the handles keep their meaning.
      const [x0, y0, x1, y1] = bbox(it.pts);
      it.pts = [[x0, y0], [x1, y1]];
      showSize(null);
    }
    draw();
    return;
  }
  if (op.kind === "draw") {
    const [x0, y0, x1, y1] = bbox(it.pts);
    const big = Math.max(x1 - x0, y1 - y0) * S.view.scale > 3;
    if (big || (it.type === "path" && it.pts.length > 1)) {
      pushHistory(op.snap);
      S.ann.push(it);
      S.lastDrawn = it;
      showSize(it);
      annChanged();
    } else {
      showSize(null);
    }
  } else if (op.changed) {
    pushHistory(op.snap);
    S.lastDrawn = null;
    annChanged();
  }
  draw();
}
// Some WebKit builds drop pointerup on the captured element; listen on the
// window too. endPointer is idempotent.
canvas.addEventListener("pointerup", endPointer);
window.addEventListener("pointerup", endPointer);
canvas.addEventListener("lostpointercapture", endPointer);
canvas.addEventListener("dblclick", (e) => {
  if (!S.info) return;
  const hit = S.tool === "select" && annAt(toImage(e), HIT / S.view.scale);
  if (S.cropBox && inBox(S.cropBox, toImage(e), 0)) applyCrop();
  else if (hit && hit.type === "text") editText(hit, false);
  else if (S.tool === "pan" || (S.tool === "select" && !annAt(toImage(e), HIT / S.view.scale))) fit();
});

canvas.addEventListener(
  "wheel",
  (e) => {
    if (!S.info) return;
    e.preventDefault();
    // Ctrl (⌘ on Mac) or Shift + wheel steps through Z. On a Mac, ctrlKey is
    // also set by trackpad pinches, which keep zooming.
    if ((IS_MAC ? e.metaKey : e.ctrlKey) || (e.shiftKey && !e.ctrlKey)) {
      const d = e.deltaY || e.deltaX;
      // One slice per mouse-wheel notch; trackpads send many small deltas.
      if (Math.abs(d) >= 50) {
        setZ(S.z + Math.sign(d));
      } else {
        wheelZ += d;
        if (Math.abs(wheelZ) >= 30) {
          setZ(S.z + Math.sign(wheelZ));
          wheelZ = 0;
        }
      }
      return;
    }
    const r = canvas.getBoundingClientRect();
    const mx = e.clientX - r.left;
    const my = e.clientY - r.top;
    const { cw, ch } = cssSize();
    const [x, y, w, h] = bounds();
    const ix = Math.min(x + w, Math.max(x, S.view.cx + (mx - cw / 2) / S.view.scale));
    const iy = Math.min(y + h, Math.max(y, S.view.cy + (my - ch / 2) / S.view.scale));
    const k = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0025));
    S.view.scale *= k;
    // Use the constrained scale when keeping the anchor in place.
    clampView();
    S.view.cx = ix - (mx - cw / 2) / S.view.scale;
    S.view.cy = iy - (my - ch / 2) / S.view.scale;
    clampView();
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
  if (e.key === "Escape" && closeMenu()) return e.preventDefault();
  const mod = e.ctrlKey || e.metaKey;
  const key = e.key.toLowerCase();
  if (mod) {
    if (key === "o") (e.shiftKey ? pickStackFolder : pickFile)();
    else if (key === "s") quickSave();
    else if (key === "z") (e.shiftKey ? redo : undo)();
    else if (key === "y") redo();
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
    case "Enter":
      if (!S.cropBox) return;
      applyCrop();
      break;
    case "Delete":
    case "Backspace":
      if (deletePicked()) break;
      if (S.cropBox) {
        S.cropBox = null;
        showSize(null);
        draw();
        break;
      }
      if (!undoCrop()) return;
      break;
    case "Escape":
      if (S.op) {
        if (S.op.snap && S.op.kind !== "draw" && S.op.item.type !== "crop") S.ann = JSON.parse(S.op.snap.ann);
        if (S.op.item && S.op.item.type === "crop") S.cropBox = null;
        S.op = null;
        canvas.classList.remove("drag");
        S.picked = null;
      } else if (S.cropBox) {
        S.cropBox = null;
      } else if (S.picked) {
        S.picked = null;
      } else if (S.lastDrawn && S.ann.includes(S.lastDrawn)) {
        // Take back what was just drawn.
        pushHistory(snapshot());
        S.ann = S.ann.filter((a) => a !== S.lastDrawn);
        S.lastDrawn = null;
        annChanged();
      } else if (!undoCrop()) {
        setTool("select");
      }
      showSize(null);
      draw();
      break;
    case "+":
    case "=":
      zoomBy(1.25);
      break;
    case "-":
      zoomBy(0.8);
      break;
    case "1":
      oneToOne();
      break;
    default:
      if (TOOL_KEYS[key]) {
        setTool(TOOL_KEYS[key]);
        break;
      }
      switch (key) {
        case "f": fit(); break;
        case "t": showPanel("all"); break;
        case "b": toggleSidebar(); break;
        case "h": toggleAnn(); break;
        case "i": S.modelShow = $("mod-show").checked = !S.modelShow; draw(); break;
        default: return;
      }
  }
  e.preventDefault();
});

$("btn-open").addEventListener("click", pickFile);
$("btn-open-folder").addEventListener("click", pickStackFolder);
$("btn-prev").addEventListener("click", () => step(-1));
$("btn-reveal").title = IS_MAC ? "在访达中显示当前图像" : /Win/.test(navigator.platform) ? "在资源管理器中显示当前图像" : "打开图像所在的文件夹";
$("btn-reveal").addEventListener("click", () => S.info && invoke("reveal_in_folder", { path: S.info.path }).catch((e) => toast(`${e}`)));
$("btn-reveal-save").addEventListener("click", () => S.info && invoke("open_folder", { path: saveDir() }).catch((e) => toast(`${e}`)));
$("save-dir-pick").addEventListener("click", pickSaveDir);
$("save-dir-reset").addEventListener("click", () => setSaveDir(null));
updateSaveDir();
$("btn-next").addEventListener("click", () => step(1));
$("btn-zoom").addEventListener("click", fit);
$("btn-zoomin").addEventListener("click", () => zoomBy(1.25));
$("btn-zoomout").addEventListener("click", () => zoomBy(0.8));
$("btn-play").addEventListener("click", togglePlay);
$("btn-tools").addEventListener("click", () => showPanel("all"));
$("btn-contrast").addEventListener("click", () => showPanel("contrast"));
$("btn-info").addEventListener("click", () => showPanel("info"));
$("meta-filter").addEventListener("input", renderMeta);
$("meta-copy").addEventListener("click", copyMeta);
$("btn-sidebar").addEventListener("click", () => toggleSidebar());
$("btn-scalebar").addEventListener("click", () => {
  if (!S.px) {
    showPanel("all");
    $("sec-scale").scrollIntoView({ block: "nearest" });
    $("px-size").focus();
    toast("文件里没有像素尺寸，先在这里填写");
    return;
  }
  S.scaleBar = !S.scaleBar;
  pref.set("scalebar-on", S.scaleBar);
  syncScaleUi();
  draw();
});
$("btn-save").addEventListener("click", saveAs);
$("btn-undo").addEventListener("click", undo);
$("btn-redo").addEventListener("click", redo);
$("btn-annshow").addEventListener("click", () => toggleAnn());
const AXES = { xy: "正常看每一层", xz: "从侧面看，拉条切换 Y", yz: "从侧面看，拉条切换 X" };
$("btn-axis").addEventListener("click", () => {
  if (!S.info || closeMenu($("btn-axis"))) return;
  const items = Object.entries(AXES).map(([v, hint]) => ({ value: v, label: v.toUpperCase(), hint, on: S.axis === v }));
  openMenu($("btn-axis"), items, (v) => v !== S.axis && setAxis(v));
});
// Arrows to scroll the tool buttons when the window is too narrow for them.
{
  const bar = $("quickbar");
  const update = () => {
    const over = bar.scrollWidth > bar.clientWidth + 1;
    $("scroll-left").hidden = !over || bar.scrollLeft <= 0;
    $("scroll-right").hidden = !over || bar.scrollLeft + bar.clientWidth >= bar.scrollWidth - 1;
  };
  const by = (d) => bar.scrollBy({ left: d * bar.clientWidth * 0.7, behavior: "smooth" });
  $("scroll-left").addEventListener("click", () => by(-1));
  $("scroll-right").addEventListener("click", () => by(1));
  bar.addEventListener("scroll", update);
  new ResizeObserver(update).observe(bar);
}
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
for (const b of document.querySelectorAll("#toolbtns [data-tool]"))
  b.addEventListener("click", () => {
    const t = b.dataset.tool;
    if (t !== "pen") return setTool(t);
    // A second click on the active pen opens its shape menu.
    if (PEN_SHAPES.includes(S.tool)) return $("pen-menu").click();
    setTool(S.penShape);
  });
$("pen-menu").addEventListener("click", () => {
  if (closeMenu($("pen-split"))) return;
  const items = PEN_SHAPES.map((v) => ({ value: v, icon: PEN_ICONS[v], label: PEN_NAMES[v], key: PEN_KEYS[v], on: S.tool === v }));
  openMenu($("pen-split"), items, setTool);
});
for (const o of document.querySelectorAll("#pen-shapes button")) o.addEventListener("click", () => setTool(o.dataset.shape));
// Both side panels can be resized by dragging their inner edge.
function setPanelWidth(name, w) {
  const [min, max] = name === "sidebar" ? [120, 600] : [220, 640];
  w = Math.round(Math.max(min, Math.min(max, w, window.innerWidth - 240)));
  document.documentElement.style.setProperty(`--${name}-w`, `${w}px`);
  return w;
}
for (const name of ["sidebar", "tools"]) {
  const saved = pref.get(`${name}-w`);
  if (saved) setPanelWidth(name, saved);
  const handle = $(`${name}-resizer`);
  handle.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    handle.setPointerCapture(e.pointerId);
    handle.classList.add("active");
    const start = e.clientX;
    const w0 = $(name).getBoundingClientRect().width;
    const dir = name === "sidebar" ? -1 : 1;
    let w = w0;
    const move = (ev) => {
      w = setPanelWidth(name, w0 + dir * (ev.clientX - start));
      if (name === "tools" && S.info) clampView();
      draw();
      if (name === "tools" && S.info) requestRender();
    };
    const up = () => {
      handle.classList.remove("active");
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("lostpointercapture", up);
      pref.set(`${name}-w`, w);
      requestRender();
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("lostpointercapture", up);
  });
}

S.penShape = PEN_SHAPES.includes(pref.get("pen")) ? pref.get("pen") : "pen";
setTool(S.penShape);
setTool("select");
$("ann-show").addEventListener("change", (e) => toggleAnn(e.target.checked));
$("btn-clear").addEventListener("click", () => {
  if (!S.ann.some((a) => onSlice(a))) return;
  pushHistory(snapshot());
  S.ann = S.ann.filter((a) => !onSlice(a));
  S.picked = null;
  showSize(null);
  $("measure-out").textContent = "";
  annChanged();
});
// Restyle the picked annotation when the colour or width changes.
// One undo step per restyled annotation.
let styled = null;
function beforeRestyle() {
  if (styled === S.picked) return;
  pushHistory(snapshot());
  styled = S.picked;
}
$("ann-color").addEventListener("input", (e) => {
  if (!S.picked) return;
  beforeRestyle();
  S.picked.color = e.target.value;
  annChanged();
});
$("ann-width").addEventListener("change", (e) => {
  if (!S.picked) return;
  beforeRestyle();
  S.picked.width = +e.target.value;
  annChanged();
});
$("sb-show").addEventListener("change", (e) => {
  S.scaleBar = e.target.checked;
  pref.set("scalebar-on", S.scaleBar);
  syncScaleUi();
  draw();
});
$("px-size").addEventListener("change", onPxInput);
$("pz-size").addEventListener("change", onPzInput);
$("px-unit").addEventListener("change", onPxInput);
$("mod-show").addEventListener("change", (e) => {
  S.modelShow = e.target.checked;
  draw();
});
$("btn-mod-load").addEventListener("click", async () => {
  const p = await invoke("pick_model", { dir: S.info ? dirname(S.info.path) : "" });
  if (p) loadModelFile(p);
});
window.addEventListener("beforeunload", flushAnn);
document.addEventListener("visibilitychange", () => {
  if (document.hidden) flushAnn();
});

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
    const stack = S.folder.stacks[f];
    if (stack) {
      el.classList.add("stack");
      el.title = `${stack.name}：${stack.slices} 张切片组成的 Z 堆叠\n${f}`;
      el.querySelector(".nm").textContent = stack.name;
      el.querySelector(".meta").textContent = `堆叠 · ${stack.slices} 层`;
    } else {
      el.querySelector(".nm").textContent = basename(f);
      const ext = (f.match(/\.([^.\\/]+)$/) || [, ""])[1].toUpperCase();
      el.querySelector(".meta").textContent = `${ext} · ${fmtBytes(S.folder.sizes[i] || 0)}`;
    }
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

S.scaleBar = pref.get("scalebar-on", true);
$("fft-n").value = String(pref.get("fft-n", "512"));
if (pref.get("fft-on", false)) setFft(true);
toggleSidebar(pref.get("sidebar", true));
{
  const t = pref.get("tools", null);
  showPanel(t === true ? "all" : t || null);
}
updateSaveButtons();
syncScaleUi();
T.event.listen("open-pending", takePending);
T.webview
  .getCurrentWebview()
  .onDragDropEvent((e) => {
    const { type, paths } = e.payload;
    $("drop-hint").hidden = !(type === "enter" || type === "over");
    if (type === "drop" && paths.length) openPath(paths[0]);
  })
  .catch(() => {});
takePending();
