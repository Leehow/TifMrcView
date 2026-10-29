// A stand-in for the Tauri backend so the real viewer UI (ui/index.html,
// app.js and style.css, copied in by scripts/build-site-demo.mjs) runs in a
// web page. It serves a few small sample images from data/samples.json and
// mirrors the parts of the Rust side the demo needs: render_view, histogram,
// pixel values, thumbnails, the folder list and the XZ / YZ side views.
// Anything else (saving, plugins, updates…) answers with a friendly error.
"use strict";

(function () {
  const BASE = new URL("data/", document.currentScript.src).href;
  const DIR = "/20261015-ibp";
  const HIST_BINS = 4096;
  const params = new URLSearchParams(location.search);

  // Settings live in memory: the demo starts the same every time and never
  // touches the website's own storage.
  const mem = new Map([
    ["sidebar", "true"],
    ["tools", JSON.stringify("contrast")],
    ["scalebar-on", "true"],
  ]);
  const lang = params.get("lang");
  if (lang) mem.set("plugin.i18n.lang", JSON.stringify(lang));
  const store = {
    getItem: (k) => (mem.has(k) ? mem.get(k) : null),
    setItem: (k, v) => mem.set(k, String(v)),
    removeItem: (k) => mem.delete(k),
    clear: () => mem.clear(),
    key: (i) => [...mem.keys()][i] ?? null,
    get length() {
      return mem.size;
    },
  };
  try {
    Object.defineProperty(window, "localStorage", { value: store, configurable: true });
  } catch (_) {}

  // Inside the website's iframe, keep the app from scrolling the page: its
  // thumbnail strip scrolls the current file into view, which would also
  // scroll every page around it.
  if (window.top !== window) {
    Element.prototype.scrollIntoView = function () {
      let p = this.parentElement;
      while (p && !(p.scrollHeight > p.clientHeight && /auto|scroll/.test(getComputedStyle(p).overflowY))) p = p.parentElement;
      if (!p) return;
      const r = this.getBoundingClientRect(), pr = p.getBoundingClientRect();
      if (r.top < pr.top) p.scrollTop -= pr.top - r.top;
      else if (r.bottom > pr.bottom) p.scrollTop += Math.min(r.bottom - pr.bottom, r.top - pr.top);
    };
    const focus = HTMLElement.prototype.focus;
    HTMLElement.prototype.focus = function (o) {
      focus.call(this, { ...o, preventScroll: true });
    };
  }

  const manifest = fetch(BASE + "samples.json").then((r) => r.json());
  const pathOf = (f) => `${DIR}/${f.name}`;
  async function fileOf(path) {
    const m = await manifest;
    const f = m.files.find((f) => pathOf(f) === path);
    if (!f) throw "网页示例里没有这个文件";
    return f;
  }

  // ---------- pixels ----------

  /** Decoded planes of a sample, [channel][z] -> Float32Array. */
  const decoded = new Map();
  function planesOf(f) {
    if (!decoded.has(f.name)) decoded.set(f.name, Promise.all(f.channels.map((c) => decodeAtlas(f, c))));
    return decoded.get(f.name);
  }

  async function decodeAtlas(f, ch) {
    const blob = await (await fetch(BASE + ch.atlas)).blob();
    const bmp = await createImageBitmap(blob, { colorSpaceConversion: "none", premultiplyAlpha: "none" });
    const cv = document.createElement("canvas");
    cv.width = bmp.width;
    cv.height = bmp.height;
    const g = cv.getContext("2d", { willReadFrequently: true });
    g.drawImage(bmp, 0, 0);
    const rgba = g.getImageData(0, 0, bmp.width, bmp.height).data;
    // The atlas holds sqrt((v - lo) / (hi - lo)) in 8 bits; undo the curve.
    // Each byte stands for an interval reaching halfway to the next level
    // that occurs (lossy WebP skips some), and values are spread over it so
    // histograms come out smooth instead of as combs.
    const seen = new Uint8Array(256);
    for (let i = 0; i < rgba.length; i += 4) seen[rgba[i]] = 1;
    const levels = [];
    for (let i = 0; i < 256; i++) if (seen[i]) levels.push(i);
    const from = new Float32Array(256), width = new Float32Array(256);
    levels.forEach((l, k) => {
      const a = k ? (levels[k - 1] + l) / 2 : l;
      const b = k < levels.length - 1 ? (l + levels[k + 1]) / 2 : l;
      from[l] = a;
      width[l] = b - a;
    });
    const span = ch.hi - ch.lo;
    let seed = 1;
    const rnd = () => (seed = (Math.imul(seed, 1103515245) + 12345) >>> 0) / 4294967296;
    const val = (q) => Math.round(ch.lo + span * ((from[q] + width[q] * rnd()) / 255) ** 2);
    const { width: w, height: h, sizeZ, cols } = f;
    const out = [];
    for (let z = 0; z < sizeZ; z++) {
      const p = new Float32Array(w * h);
      const ox = (z % cols) * w, oy = Math.floor(z / cols) * h;
      for (let y = 0; y < h; y++) {
        let s = ((oy + y) * bmp.width + ox) * 4;
        for (let x = 0; x < w; x++, s += 4) p[y * w + x] = val(rgba[s]);
      }
      out.push(p);
    }
    return out;
  }

  // ---------- open images ----------

  let nextToken = 0;
  /** token -> {info, plane(c, z), stats} */
  const open = new Map();

  function makeInfo(f, path) {
    return {
      path,
      name: f.name,
      format: f.format,
      width: f.width,
      height: f.height,
      sizeC: f.channels.length,
      sizeZ: f.sizeZ,
      sizeT: 1,
      dtype: "u16",
      isRgb: false,
      channels: f.channels.map((c) => ({ name: c.name, color: c.color })),
      pixelSize: f.pixelSize,
      unit: f.unit,
      metadata: f.metadata,
      series: [],
      seriesIndex: 0,
      folder: null,
      spectrum: null,
    };
  }

  function register(info, plane) {
    const token = ++nextToken;
    open.set(token, { info, plane, stats: new Map() });
    // Keep the open image and its side view; drop older ones.
    for (const k of open.keys()) if (k < token - 4) open.delete(k);
    return { token, info };
  }

  function current(token) {
    const img = open.get(token);
    if (!img) throw "stale";
    return img;
  }

  async function openImage(path) {
    const f = await fileOf(path);
    const planes = await planesOf(f);
    return register(makeInfo(f, path), (c, z) => planes[c][Math.min(z, planes[c].length - 1)]);
  }

  function openReslice(token, axis) {
    const base = current(token);
    const b = base.info;
    const xz = axis === "xz";
    const w = xz ? b.width : b.height;
    const depth = xz ? b.height : b.width;
    const ps = b.pixelSize;
    const info = {
      ...b,
      width: w,
      height: b.sizeZ,
      sizeZ: depth,
      spectrum: null,
      pixelSize: ps && (xz ? [ps[0], ps[2], ps[1]] : [ps[1], ps[2], ps[0]]),
    };
    const cache = new Map();
    return register(info, (c, i) => {
      const key = c * depth + i;
      if (!cache.has(key)) {
        const p = new Float32Array(w * b.sizeZ);
        for (let z = 0; z < b.sizeZ; z++) {
          const src = base.plane(c, z);
          for (let k = 0; k < w; k++) p[z * w + k] = xz ? src[i * b.width + k] : src[k * b.width + i];
        }
        cache.set(key, p);
      }
      return cache.get(key);
    });
  }

  // ---------- statistics (image.rs Stats) ----------

  function stats(img, c, z) {
    const key = c * 1e6 + z;
    if (img.stats.has(key)) return img.stats.get(key);
    const v = img.plane(c, z);
    let lo = Infinity, hi = -Infinity;
    for (let i = 0; i < v.length; i++) {
      if (v[i] < lo) lo = v[i];
      if (v[i] > hi) hi = v[i];
    }
    if (!isFinite(lo)) [lo, hi] = [0, 1];
    const hist = new Uint32Array(HIST_BINS);
    const scale = (HIST_BINS - 1) / Math.max(hi - lo, 1e-38);
    for (let i = 0; i < v.length; i++) hist[Math.floor((v[i] - lo) * scale)]++;
    const s = { min: lo, max: hi, hist, total: v.length };
    img.stats.set(key, s);
    return s;
  }

  function percentile(s, frac) {
    if (!s.total || s.max <= s.min) return s.min;
    const target = Math.floor(s.total * frac);
    let acc = 0;
    for (let i = 0; i < HIST_BINS; i++) {
      acc += s.hist[i];
      if (acc > target) return s.min + (s.max - s.min) * (i / (HIST_BINS - 1));
    }
    return s.max;
  }

  function autoRange(s) {
    const lo = percentile(s, 0.001);
    let hi = percentile(s, 0.999);
    if (hi <= lo) hi = Math.max(s.max, lo + 1);
    return [Math.floor(lo), Math.ceil(hi)];
  }

  // ---------- rendering (render.rs) ----------

  function render(img, req) {
    const info = img.info;
    const iw = info.width, ih = info.height;
    const outW = Math.min(Math.max(req.outW, 1), 4096);
    const outH = Math.min(Math.max(req.outH, 1), 4096);
    const [sx, sy] = req.region;
    const sw = Math.max(req.region[2], 1e-6), sh = Math.max(req.region[3], 1e-6);
    const n = req.channels.length;
    const buf = new ArrayBuffer(12 + n * 16 + outW * outH * 4);
    const dv = new DataView(buf);
    dv.setUint32(0, outW, true);
    dv.setUint32(4, outH, true);
    dv.setUint32(8, n, true);
    const chans = req.channels.map((c, k) => {
      const s = stats(img, c.index, req.z);
      const [alo, ahi] = autoRange(s);
      const lo = c.min ?? alo, hi = c.max ?? ahi;
      [lo, hi, s.min, s.max].forEach((v, j) => dv.setFloat32(12 + k * 16 + j * 4, v, true));
      return { v: img.plane(c.index, req.z), lo, inv: 1 / Math.max(hi - lo, 1e-38), color: c.color };
    });
    const span = (o, cnt, s0, sl, lim) => {
      let a = Math.max(0, Math.floor(s0 + (o * sl) / cnt));
      const b0 = Math.floor(s0 + ((o + 1) * sl) / cnt);
      a = Math.min(a, lim - 1);
      const b = Math.min(Math.max(b0, a + 1), lim);
      return [a, b, Math.max(1, Math.ceil((b - a) / 6))];
    };
    const cols = [];
    for (let o = 0; o < outW; o++) cols.push(span(o, outW, sx, sw, iw));
    const px = new Uint8ClampedArray(buf, 12 + n * 16);
    const acc = new Float32Array(outW * 3);
    const gamma = req.gamma ?? 1;
    for (let oy = 0; oy < outH; oy++) {
      const [y0, y1, ys] = span(oy, outH, sy, sh, ih);
      acc.fill(0);
      for (const ch of chans) {
        for (let x = 0; x < outW; x++) {
          const [x0, x1, xs] = cols[x];
          let sum = 0, cnt = 0;
          for (let y = y0; y < y1; y += ys) {
            const row = y * iw;
            for (let xx = x0; xx < x1; xx += xs) {
              sum += ch.v[row + xx];
              cnt++;
            }
          }
          let f = Math.min(1, Math.max(0, (sum / Math.max(cnt, 1) - ch.lo) * ch.inv));
          if (gamma !== 1) f = f ** gamma;
          acc[x * 3] += f * ch.color[0];
          acc[x * 3 + 1] += f * ch.color[1];
          acc[x * 3 + 2] += f * ch.color[2];
        }
      }
      let o = oy * outW * 4;
      for (let x = 0; x < outW; x++, o += 4) {
        for (let k = 0; k < 3; k++) {
          const v = Math.min(255, acc[x * 3 + k]) | 0;
          px[o + k] = req.invert ? 255 - v : v;
        }
        px[o + 3] = 255;
      }
    }
    return buf;
  }

  async function thumbnail(path, size) {
    const { token, info } = await openImage(path);
    const img = current(token);
    open.delete(token);
    const k = Math.min(1, size / Math.max(info.width, info.height));
    return render(img, {
      z: Math.floor(info.sizeZ / 2),
      channels: info.channels.map((c, i) => ({ index: i, color: c.color, min: null, max: null })),
      region: [0, 0, info.width, info.height],
      outW: Math.max(1, Math.round(info.width * k)),
      outH: Math.max(1, Math.round(info.height * k)),
      gamma: 1,
      invert: false,
    });
  }

  // ---------- commands ----------

  // A measurement on the Z stack to start with, like the screenshot on the page.
  const annotations = new Map();
  manifest.then((m) => {
    const f = m.files.find((f) => f.sizeZ > 1);
    if (!f) return;
    const items = [{ type: "line", z: 0, z1: f.sizeZ - 1, color: "#ffb020", width: 2, pts: [[106, 186], [225, 181]] }];
    annotations.set(pathOf(f), JSON.stringify({ app: "TifMrcView", version: 1, items }));
  });
  const notHere = () => Promise.reject("网页示例里没有这个功能，下载 TifMrcView 后即可使用");

  const commands = {
    async take_pending_open() {
      const m = await manifest;
      const f = m.files.find((f) => f.sizeZ > 1) || m.files[0];
      return pathOf(f);
    },
    open_image: ({ path }) => openImage(path),
    open_reslice: ({ token, axis }) => openReslice(token, axis),
    load_progress: ({ token }) => {
      const n = current(token).info.sizeZ;
      return [n, n, true];
    },
    async list_folder({ path }) {
      const m = await manifest;
      const files = m.files.map(pathOf);
      return { files, sizes: m.files.map((f) => f.bytes), index: files.indexOf(path), stacks: [] };
    },
    thumbnail: ({ path, size }) => thumbnail(path, size),
    render_view: ({ req }) => render(current(req.token), req),
    histogram({ token, z, c }) {
      const s = stats(current(token), c, z);
      const bins = new Array(256).fill(0);
      for (let i = 0; i < HIST_BINS; i++) bins[Math.floor((i * 256) / HIST_BINS)] += s.hist[i];
      return { bins, min: s.min, max: s.max };
    },
    pixel_values({ token, z, x, y }) {
      const img = current(token);
      if (x >= img.info.width || y >= img.info.height) return [];
      return img.info.channels.map((_, c) => img.plane(c, z)[y * img.info.width + x]);
    },
    read_annotations: ({ image }) => annotations.get(image) ?? null,
    write_annotations({ image, json }) {
      if (json) annotations.set(image, json);
      else annotations.delete(image);
    },
    find_models: () => [],
    plugin_scan: () => ({ native: [], dir: "", packages: [], user: [] }),
    plugin_market_list: () => [],
    plugin_set_disabled: () => {},
    take_install_request: () => null,
    check_update: () => null,
  };

  async function invoke(cmd, args = {}) {
    const fn = commands[cmd];
    if (!fn) return notHere();
    return fn(args);
  }

  const noop = () => {};
  const win = {
    label: "main",
    setTitle: () => Promise.resolve(),
    listen: () => Promise.resolve(noop),
    onDragDropEvent: () => Promise.resolve(noop),
  };
  window.__TAURI__ = {
    core: { invoke },
    app: { getName: () => Promise.resolve("TifMrcView"), getVersion: () => Promise.resolve("demo") },
    event: { listen: () => Promise.resolve(noop) },
    window: { getCurrentWindow: () => win },
    webview: { getCurrentWebview: () => win },
    webviewWindow: { getCurrentWebviewWindow: () => win },
  };
})();
