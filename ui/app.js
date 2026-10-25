"use strict";

const T = window.__TAURI__;
const invoke = T.core.invoke;
const $ = (id) => document.getElementById(id);

const COLORS = [
  [255, 255, 255], [0, 255, 0], [255, 0, 255], [255, 0, 0],
  [0, 0, 255], [0, 255, 255], [255, 255, 0],
];

const S = {
  token: 0,
  info: null,
  z: 0,
  t: 0,
  chans: [],        // {visible, color, min, max, dmin, dmax, auto}
  sel: 0,           // channel whose contrast the toolbar edits
  gamma: 1,
  view: { cx: 0, cy: 0, scale: 1 },
  folder: { dir: null, files: [], index: -1 },
  frame: null,      // last rendered bitmap and the image region it covers
  inflight: false,
  dirty: false,
  playing: null,
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

function fmt(v) {
  if (v === undefined || v === null || Number.isNaN(v)) return "";
  if (Number.isInteger(v)) return String(v);
  const a = Math.abs(v);
  return a !== 0 && (a < 1e-3 || a >= 1e6) ? v.toExponential(3) : String(+v.toFixed(4));
}

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
    S.token = res.token;
    S.info = res.info;
    S.z = Math.floor(res.info.sizeZ / 2);
    S.t = 0;
    S.sel = 0;
    S.frame = null;
    S.chans = res.info.channels.map((c) => ({
      visible: true,
      color: c.color,
      name: c.name,
      min: null,
      max: null,
      auto: true,
    }));
    $("empty").hidden = true;
    $("st-pixel").textContent = "";
    T.window.getCurrentWindow().setTitle(`${res.info.name} — tifview`).catch(() => {});
    buildChannels();
    setupSliders();
    updateInfo();
    fit();
    await refreshFolder(path);
  } catch (e) {
    toast(`无法打开 ${basename(path)}：${e}`);
  } finally {
    $("loading").hidden = true;
  }
}

async function refreshFolder(path) {
  const dir = dirname(path);
  if (dir !== S.folder.dir) {
    const f = await invoke("list_folder", { path });
    S.folder = { dir, files: f.files, index: f.index ?? -1 };
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

// ---------- toolbar ----------

function buildChannels() {
  const box = $("channels");
  box.innerHTML = "";
  const info = S.info;
  if (info.isRgb) {
    $("contrast").hidden = true;
    return;
  }
  $("contrast").hidden = false;
  if (S.chans.length > 1) {
    S.chans.forEach((c, i) => {
      const el = document.createElement("div");
      el.className = "chan";
      el.title = "单击：选中并调节对比度　双击：显示/隐藏　点圆点：换颜色";
      const sw = document.createElement("span");
      sw.className = "sw";
      const nm = document.createElement("span");
      nm.textContent = c.name;
      el.append(sw, nm);
      sw.addEventListener("click", (e) => {
        e.stopPropagation();
        const k = COLORS.findIndex((x) => x.join() === c.color.join());
        c.color = COLORS[(k + 1) % COLORS.length];
        paintChannels();
        requestRender();
      });
      el.addEventListener("click", () => {
        S.sel = i;
        paintChannels();
        syncContrast();
      });
      el.addEventListener("dblclick", () => {
        c.visible = !c.visible;
        paintChannels();
        requestRender();
      });
      box.append(el);
    });
  }
  paintChannels();
  syncContrast();
}

function paintChannels() {
  [...$("channels").children].forEach((el, i) => {
    const c = S.chans[i];
    el.classList.toggle("off", !c.visible);
    el.classList.toggle("sel", i === S.sel);
    el.firstChild.style.background = `rgb(${c.color.join(",")})`;
  });
}

function syncContrast() {
  const c = S.chans[S.sel];
  if (!c) return;
  $("auto").checked = c.auto;
  if (document.activeElement !== $("cmin")) $("cmin").value = fmt(c.min);
  if (document.activeElement !== $("cmax")) $("cmax").value = fmt(c.max);
  $("cmin").title = `显示最小值（数据最小 ${fmt(c.dmin)}）`;
  $("cmax").title = `显示最大值（数据最大 ${fmt(c.dmax)}）`;
}

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
}

function setT(t) {
  t = Math.max(0, Math.min(S.info.sizeT - 1, t));
  S.t = t;
  $("tslider").value = t;
  updateZLabel();
  requestRender();
}

function updateInfo() {
  const i = S.info;
  const dims = [`${i.width}×${i.height}`];
  if (i.sizeC > 1) dims.push(`C${i.sizeC}`);
  if (i.sizeZ > 1) dims.push(`Z${i.sizeZ}`);
  if (i.sizeT > 1) dims.push(`T${i.sizeT}`);
  let px = "";
  if (i.pixelSize && i.pixelSize[0] > 0) px = ` · ${fmt(i.pixelSize[0])} ${i.unit}/px`;
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

function draw() {
  const dpr = window.devicePixelRatio || 1;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  if (!S.frame) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  const [x, y, w, h] = S.frame.region;
  const [sx, sy] = toScreen(x, y);
  ctx.imageSmoothingEnabled = S.view.scale * dpr < 1 || S.frame.stale;
  ctx.drawImage(frameCanvas, 0, 0, S.frame.w, S.frame.h, sx, sy, w * S.view.scale, h * S.view.scale);
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
  const channels = [];
  S.chans.forEach((c, i) => {
    if (!c.visible) return;
    channels.push({
      index: i,
      color: c.color,
      min: c.auto ? null : c.min,
      max: c.auto ? null : c.max,
    });
  });
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
  } catch (e) {
    if (String(e) !== "stale") toast(`渲染失败：${e}`);
  } finally {
    S.inflight = false;
    if (S.dirty) requestRender();
  }
}

// ---------- interaction ----------

let drag = null;
canvas.addEventListener("pointerdown", (e) => {
  if (!S.info) return;
  drag = { x: e.clientX, y: e.clientY, cx: S.view.cx, cy: S.view.cy };
  canvas.setPointerCapture(e.pointerId);
  canvas.classList.add("drag");
});
canvas.addEventListener("pointermove", (e) => {
  if (drag) {
    S.view.cx = drag.cx - (e.clientX - drag.x) / S.view.scale;
    S.view.cy = drag.cy - (e.clientY - drag.y) / S.view.scale;
    draw();
    requestRender();
  }
  showPixel(e);
});
canvas.addEventListener("pointerup", () => {
  drag = null;
  canvas.classList.remove("drag");
});
canvas.addEventListener("dblclick", fit);

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
  const r = canvas.getBoundingClientRect();
  const { cw, ch } = cssSize();
  const x = Math.floor(S.view.cx + (e.clientX - r.left - cw / 2) / S.view.scale);
  const y = Math.floor(S.view.cy + (e.clientY - r.top - ch / 2) / S.view.scale);
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
  if (e.target.tagName === "INPUT" && e.target.type === "number") return;
  const mod = e.ctrlKey || e.metaKey;
  if (mod && e.key.toLowerCase() === "o") {
    e.preventDefault();
    pickFile();
    return;
  }
  switch (e.key) {
    case "ArrowLeft":
      step(-1);
      break;
    case "ArrowRight":
      step(1);
      break;
    case "ArrowUp":
    case "PageUp":
    case ".":
      setZ(S.z + 1);
      break;
    case "ArrowDown":
    case "PageDown":
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
    case "f":
    case "F":
      fit();
      break;
    case "1":
      oneToOne();
      break;
    default:
      return;
  }
  e.preventDefault();
});

$("btn-open").addEventListener("click", pickFile);
$("btn-prev").addEventListener("click", () => step(-1));
$("btn-next").addEventListener("click", () => step(1));
$("btn-fit").addEventListener("click", fit);
$("btn-1to1").addEventListener("click", oneToOne);
$("btn-play").addEventListener("click", togglePlay);
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
for (const id of ["cmin", "cmax"]) {
  $(id).addEventListener("change", () => {
    const c = S.chans[S.sel];
    if (!c) return;
    const lo = parseFloat($("cmin").value);
    const hi = parseFloat($("cmax").value);
    if (Number.isFinite(lo)) c.min = lo;
    if (Number.isFinite(hi)) c.max = hi;
    c.auto = false;
    $("auto").checked = false;
    requestRender();
  });
}
$("gamma").addEventListener("input", (e) => {
  S.gamma = +e.target.value;
  requestRender();
});

new ResizeObserver(resizeCanvas).observe(canvas);

// ---------- folder thumbnails ----------

const thumbQueue = [];
let thumbActive = 0;
const THUMB_PARALLEL = Math.max(2, Math.min(8, (navigator.hardwareConcurrency || 4) - 1));

function buildStrip() {
  const strip = $("strip");
  strip.innerHTML = "";
  thumbQueue.length = 0;
  const observer = new IntersectionObserver(
    (entries) => {
      for (const en of entries) {
        if (en.isIntersecting && !en.target.dataset.req) {
          en.target.dataset.req = "1";
          thumbQueue.push(en.target);
          pumpThumbs();
        }
      }
    },
    { root: strip, rootMargin: "0px 400px" },
  );
  S.folder.files.forEach((f, i) => {
    const el = document.createElement("div");
    el.className = "thumb";
    el.title = basename(f);
    el.dataset.path = f;
    el.innerHTML = `<div class="box"></div><div class="nm"></div>`;
    el.querySelector(".nm").textContent = basename(f);
    el.addEventListener("click", () => openPath(f));
    strip.append(el);
    observer.observe(el);
  });
}

function pumpThumbs() {
  while (thumbActive < THUMB_PARALLEL && thumbQueue.length) {
    // Load the thumbnails closest to the current file first.
    const cur = S.folder.index;
    let best = 0;
    for (let i = 1; i < thumbQueue.length; i++) {
      const d = (el) => Math.abs(S.folder.files.indexOf(el.dataset.path) - cur);
      if (d(thumbQueue[i]) < d(thumbQueue[best])) best = i;
    }
    const el = thumbQueue.splice(best, 1)[0];
    if (!el.isConnected) continue;
    thumbActive++;
    invoke("thumbnail", { path: el.dataset.path, size: 192 })
      .then((buf) => {
        const f = parseFrame(buf);
        const c = document.createElement("canvas");
        c.width = f.w;
        c.height = f.h;
        c.getContext("2d").putImageData(f.image, 0, 0);
        el.querySelector(".box").replaceChildren(c);
      })
      .catch(() => {
        el.querySelector(".box").textContent = "✕";
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
  if (cur) cur.scrollIntoView({ block: "nearest", inline: "center", behavior: "smooth" });
}

// ---------- startup ----------

T.event.listen("open-pending", takePending);
T.webview
  .getCurrentWebview()
  .onDragDropEvent((e) => {
    if (e.payload.type === "drop" && e.payload.paths.length) openPath(e.payload.paths[0]);
  })
  .catch(() => {});
takePending();
