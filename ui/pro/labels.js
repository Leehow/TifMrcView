"use strict";
// Pro segmentation labels: one label per pixel of the stack, painted with a
// brush or filled by the magic wand, threshold and watershed tools. The label
// map lives in Rust (labels.rs) and is saved next to the image as
// <image>.labels.tif; this file draws the current slice over the image.
// Shares app.js globals: S, ext, invoke, $, setTool, showPanel, toScreen, toast, draw.
(() => {
  const COLORS = window.TifMrcViewPro.LABEL_COLORS;
  const colorOf = (k) => COLORS[((k - 1) % 255) % COLORS.length];

  const L = {
    token: 0,
    opening: 0,
    error: "",
    path: "",
    counts: [],
    active: 1,
    sub: "brush",     // brush | eraser | wand
    radius: 6,        // brush radius in image pixels
    guard: false,     // only paint empty pixels / only erase the active label
    tol: 0.1,         // wand tolerance, part of the display range
    threeD: false,
    opacity: 0.45,
    show: true,
    z: -1,            // slice held in `slice`
    slice: null,      // Uint16Array of the shown slice
    fetching: false,
    hover: null,      // pointer in image pixels
    stroke: null,
    busy: false,
    saveTimer: 0,
  };
  const canvas = document.createElement("canvas");
  const cctx = canvas.getContext("2d");
  let pixels = null;  // ImageData of `canvas`

  const any = () => L.counts.some((n, k) => k > 0 && n > 0);
  const maxUsed = () => {
    for (let k = L.counts.length - 1; k > 0; k--) if (L.counts[k] > 0) return k;
    return 0;
  };
  const channel = () => Math.min(S.sel, S.chans.length - 1);

  // ---------- panel ----------

  const sec = document.createElement("section");
  sec.id = "sec-labels";
  sec.innerHTML = `
    <h3>标签分割 <span class="dim" id="lab-busy"></span></h3>
    <div class="row" id="lab-tools">
      <button class="small" data-sub="brush" title="画笔：按住拖动涂上当前标签（[ ] 调大小）"><svg viewBox="0 0 24 24"><path d="M4 20c2.5 0 4-1.5 4-4a3 3 0 0 0-3-3"/><path d="M8 13l9-9 3 3-9 9"/></svg><span>画笔</span></button>
      <button class="small" data-sub="eraser" title="橡皮：擦掉标签（[ ] 调大小）"><svg viewBox="0 0 24 24"><path d="M8 20h12"/><path d="M4 15l9-9 6 6-7 7H8z"/></svg><span>橡皮</span></button>
      <button class="small" data-sub="wand" title="魔棒：点一下，把和点击处亮度相近的连通区域涂上当前标签"><svg viewBox="0 0 24 24"><path d="M4 20L15 9"/><path d="M17 3v3M17 12v3M11 7h3M20 7h3M19 5l1.5-1.5M19 9l1.5 1.5"/></svg><span>魔棒</span></button>
    </div>
    <label class="slider" id="lab-size-row">大小 <input type="range" id="lab-size" min="1" max="100" step="1" /><span id="lab-size-v"></span></label>
    <label class="slider" id="lab-tol-row">容差 <input type="range" id="lab-tol" min="0" max="0.5" step="0.005" /><span id="lab-tol-v"></span></label>
    <div class="row">
      <label id="lab-3d-row" title="魔棒沿 Z 方向一起长到相邻的层"><input type="checkbox" id="lab-3d" /> 三维</label>
      <label title="画笔和魔棒只涂没有标签的地方；橡皮只擦当前标签"><input type="checkbox" id="lab-guard" /> 不覆盖其他标签</label>
    </div>
    <div id="lab-list"></div>
    <div class="row">
      <button class="small" id="lab-new" title="换一个新的标签号继续画">＋ 新标签</button>
      <label><input type="checkbox" id="lab-show" checked /> 显示</label>
    </div>
    <label class="slider">不透明度 <input type="range" id="lab-op" min="0.05" max="1" step="0.05" /><span id="lab-op-v"></span></label>
    <details id="lab-th">
      <summary>阈值</summary>
      <div class="row">
        <input type="number" id="lab-lo" step="any" title="最小值" /> <span>–</span> <input type="number" id="lab-hi" step="any" title="最大值" />
        <button class="small" id="lab-range" title="填入当前通道的显示范围">用显示范围</button>
      </div>
      <div class="row">
        <label title="每个连通区域各自一个新标签，方便数颗粒"><input type="checkbox" id="lab-split" /> 分成独立区域</label>
        <label title="比这个小的区域忽略（像素）">最小 <input type="number" id="lab-min" min="1" value="20" style="width: 64px" /></label>
      </div>
      <div class="row">
        <button class="small" id="lab-th-z">本层</button>
        <button class="small" id="lab-th-all">全部层</button>
      </div>
    </details>
    <details id="lab-ws">
      <summary>分水岭</summary>
      <div class="dim">先用画笔在每个物体和背景里各画一笔不同的标签当种子，再按下面的按钮，剩下没有标签的像素会沿图像边缘分给最近的种子。</div>
      <div class="row">
        <button class="small" id="lab-ws-z">本层</button>
        <button class="small" id="lab-ws-all">全部层</button>
      </div>
    </details>
    <div id="lab-file" class="dim"></div>`;
  $("sec-ann").after(sec);

  // Quick toolbar button, after the crop tool.
  const btn = document.createElement("button");
  btn.dataset.tool = "label";
  btn.title = "标签分割 (G)：画笔、橡皮、魔棒、阈值、分水岭，结果自动存成图片旁边的 .labels.tif";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><path d="M3 12c0-5 4-9 9-9s9 4 9 9-4 9-9 9"/><path d="M12 21c-2 0-3-1.5-3-3 0-2 3-3 3-6 2 2 4 4 4 6s-2 3-4 3z"/></svg>';
  document.querySelector('#toolbtns [data-tool="crop"]').after(btn);
  btn.addEventListener("click", () => setTool("label"));
  TOOL_KEYS.g = "label";

  function syncPanel() {
    for (const b of sec.querySelectorAll("#lab-tools button")) b.classList.toggle("on", b.dataset.sub === L.sub);
    $("lab-size-row").hidden = L.sub === "wand";
    $("lab-tol-row").hidden = L.sub !== "wand";
    $("lab-3d-row").hidden = L.sub !== "wand";
    $("lab-size").value = L.radius;
    $("lab-size-v").textContent = `${L.radius * 2}`;
    $("lab-tol").value = L.tol;
    $("lab-tol-v").textContent = `${Math.round(L.tol * 100)}%`;
    $("lab-op").value = L.opacity;
    $("lab-op-v").textContent = `${Math.round(L.opacity * 100)}%`;
    $("lab-3d").checked = L.threeD;
    $("lab-guard").checked = L.guard;
    $("lab-show").checked = L.show;
    $("lab-busy").textContent = L.busy ? "处理中…" : "";
    for (const b of sec.querySelectorAll("#lab-th button, #lab-ws button")) b.disabled = L.busy || !L.token;
  }

  function renderList() {
    const list = $("lab-list");
    list.textContent = "";
    const ks = [];
    for (let k = 1; k < L.counts.length; k++) if (L.counts[k] > 0) ks.push(k);
    if (!ks.includes(L.active)) ks.push(L.active);
    ks.sort((a, b) => a - b);
    // Thousands of regions after a split: list the active one and the first few.
    const shown = ks.length > 200 ? ks.filter((k, i) => i < 200 || k === L.active) : ks;
    for (const k of shown) {
      const row = document.createElement("div");
      row.className = "lab-row" + (k === L.active ? " on" : "");
      const n = L.counts[k] || 0;
      row.innerHTML = `<span class="sw"></span><span class="name"></span><span class="n dim"></span><button class="del" title="删除这个标签">✕</button>`;
      row.querySelector(".sw").style.background = `rgb(${colorOf(k).join(",")})`;
      row.querySelector(".name").textContent = `标签 ${k}`;
      row.querySelector(".n").textContent = n ? sizeText(n) : "";
      row.querySelector(".del").hidden = !n;
      row.addEventListener("click", (e) => {
        if (e.target.classList.contains("del")) return edit({ kind: "delete", label: k });
        L.active = k;
        renderList();
      });
      list.appendChild(row);
    }
    if (shown.length < ks.length) {
      const more = document.createElement("div");
      more.className = "dim";
      more.textContent = `共 ${ks.length} 个标签，只列出前 200 个`;
      list.appendChild(more);
    }
  }

  /** Pixel count, plus the volume when the pixel size is known. */
  function sizeText(n) {
    let s = `${n.toLocaleString()} 像素`;
    if (S.px && S.info && S.info.sizeZ > 1 && S.info.pixelSize && S.info.pixelSize[2] > 0) {
      const v = n * S.px.x * S.px.y * S.info.pixelSize[2];
      s += ` · ${fmt(+v.toPrecision(3))} ${S.px.unit}³`;
    }
    return s;
  }

  function fileText() {
    if (L.error) return L.error;
    const name = L.path ? L.path.split(/[\\/]/).pop() : "";
    return any() ? `自动保存到 ${name}` : "标签会在画完几秒后自动存到图片旁边的 .labels.tif（16 位 TIFF，Fiji / napari 可直接打开）";
  }

  // ---------- label map of the open image ----------

  async function ensure() {
    if (!S.info || S.axis !== "xy" || L.token === S.token || L.opening === S.token) return;
    const token = S.token;
    L.opening = token;
    await flush();
    try {
      const st = await invoke("labels_open", { token });
      if (token !== S.token) return;
      Object.assign(L, { token, path: st.path, counts: st.counts, error: "", z: -1, slice: null, active: Math.max(1, maxUsedOf(st.counts)) });
    } catch (e) {
      if (token !== S.token) return;
      Object.assign(L, { token, path: "", counts: [], error: `标签文件读取失败：${e}`, z: -1, slice: null });
    } finally {
      if (L.opening === token) L.opening = 0;
    }
    $("lab-file").textContent = fileText();
    renderList();
    syncPanel();
    if (window.TifMrcViewPro.volume) window.TifMrcViewPro.volume.labelsChanged();
    draw();
  }
  const maxUsedOf = (counts) => {
    for (let k = counts.length - 1; k > 0; k--) if (counts[k] > 0) return k;
    return 0;
  };

  async function fetchSlice() {
    if (L.fetching || L.error || !L.token) return;
    const token = L.token, z = S.z;
    L.fetching = true;
    try {
      const buf = await invoke("labels_slice", { token, z });
      if (token !== L.token) return;
      L.slice = new Uint16Array(buf);
      L.z = z;
      repaint(0, 0, S.info.width, S.info.height);
    } catch (_) {
      // A new image is opening; the next draw asks again.
    } finally {
      L.fetching = false;
    }
    draw();
  }

  /** Colours the label slice into the overlay canvas, for one rectangle. */
  function repaint(x0, y0, x1, y1) {
    const w = S.info.width, h = S.info.height;
    if (canvas.width !== w || canvas.height !== h || !pixels) {
      canvas.width = w;
      canvas.height = h;
      pixels = cctx.createImageData(w, h);
      x0 = 0; y0 = 0; x1 = w; y1 = h;
    }
    x0 = Math.max(0, x0 | 0); y0 = Math.max(0, y0 | 0);
    x1 = Math.min(w, Math.ceil(x1)); y1 = Math.min(h, Math.ceil(y1));
    if (x1 <= x0 || y1 <= y0) return;
    const px = pixels.data, s = L.slice;
    for (let y = y0; y < y1; y++) {
      for (let x = x0; x < x1; x++) {
        const i = y * w + x, k = s[i], o = i * 4;
        if (k) {
          const c = colorOf(k);
          px[o] = c[0]; px[o + 1] = c[1]; px[o + 2] = c[2]; px[o + 3] = 255;
        } else {
          px[o + 3] = 0;
        }
      }
    }
    cctx.putImageData(pixels, 0, 0, x0, y0, x1 - x0, y1 - y0);
  }

  function overlay(g) {
    if (!S.info) return;
    if (S.axis === "xy" && L.token !== S.token) ensure();
    const ready = S.axis === "xy" && L.token === S.token && !L.error;
    if (ready && L.z !== S.z && (any() || S.tool === "label")) fetchSlice();
    if (ready && L.show && L.slice && L.z === S.z && (any() || L.stroke)) {
      const [sx, sy] = toScreen(0, 0);
      g.save();
      g.globalAlpha = L.opacity;
      g.imageSmoothingEnabled = false;
      g.drawImage(canvas, sx, sy, S.info.width * S.view.scale, S.info.height * S.view.scale);
      g.restore();
    }
    // Brush outline under the pointer.
    if (S.tool === "label" && L.sub !== "wand" && L.hover) {
      const [cx, cy] = toScreen(L.hover[0], L.hover[1]);
      g.save();
      g.beginPath();
      g.arc(cx, cy, Math.max(1, L.radius * S.view.scale), 0, Math.PI * 2);
      g.lineWidth = 1.5;
      g.strokeStyle = "rgba(0, 0, 0, 0.6)";
      g.stroke();
      g.lineWidth = 1;
      g.strokeStyle = L.sub === "eraser" ? "#fff" : `rgb(${colorOf(L.active).join(",")})`;
      g.stroke();
      g.restore();
    }
  }
  ext.overlays.push(overlay);

  // ---------- editing ----------

  async function edit(op) {
    if (!L.token || L.error) return;
    const token = L.token;
    L.busy = true;
    syncPanel();
    try {
      const st = await invoke("labels_edit", { token, op });
      if (token !== L.token) return;
      L.counts = st.counts;
      if (st.zs.includes(S.z) || L.z !== S.z) L.z = -1;
      if (st.zs.length) scheduleSave();
      renderList();
      if (window.TifMrcViewPro.volume) window.TifMrcViewPro.volume.labelsChanged();
    } catch (e) {
      toast(String(e), 6000);
    } finally {
      L.busy = false;
      syncPanel();
      $("lab-file").textContent = fileText();
      draw();
    }
  }

  function scheduleSave() {
    clearTimeout(L.saveTimer);
    L.saveTimer = setTimeout(flush, 1500);
  }

  async function flush() {
    if (!L.saveTimer || !L.token) return;
    clearTimeout(L.saveTimer);
    L.saveTimer = 0;
    try {
      await invoke("labels_save", { token: L.token });
    } catch (e) {
      if (e !== "stale") toast(`标签保存失败：${e}`, 6000);
    }
  }

  /** Paints the brush into the local slice right away (labels.rs repeats it). */
  function stampLocal(cx, cy) {
    const w = S.info.width, h = S.info.height, r = Math.max(0.5, L.radius), s = L.slice;
    const label = L.sub === "eraser" ? 0 : L.active;
    const guard = L.guard ? L.active : 0;
    const x0 = Math.max(0, Math.floor(cx - r)), y0 = Math.max(0, Math.floor(cy - r));
    const x1 = Math.min(w, Math.ceil(cx + r)), y1 = Math.min(h, Math.ceil(cy + r));
    for (let y = y0; y < y1; y++) {
      const dy = y + 0.5 - cy;
      for (let x = x0; x < x1; x++) {
        const dx = x + 0.5 - cx, i = y * w + x, cur = s[i];
        if (dx * dx + dy * dy > r * r) continue;
        if (guard && (label ? cur !== 0 && cur !== label : cur !== guard)) continue;
        s[i] = label;
      }
    }
    repaint(x0, y0, x1, y1);
  }

  function strokeTo(p) {
    const pts = L.stroke.pts;
    const a = pts[pts.length - 1];
    const len = Math.hypot(p[0] - a[0], p[1] - a[1]);
    const step = Math.max(0.5, L.radius / 2);
    const n = Math.max(1, Math.ceil(len / step));
    for (let i = 1; i <= n; i++) stampLocal(a[0] + ((p[0] - a[0]) * i) / n, a[1] + ((p[1] - a[1]) * i) / n);
    pts.push(p);
  }

  function ready() {
    if (S.axis !== "xy") {
      toast("标签只能在 XY 视图里画");
      return false;
    }
    if (L.error) {
      toast(L.error, 6000);
      return false;
    }
    return L.token === S.token && !L.busy;
  }

  ext.tools.label = {
    down(e, p) {
      if (!ready()) return;
      if (L.sub === "wand") {
        const c = S.chans[channel()];
        const range = Math.abs((c.max ?? 1) - (c.min ?? 0)) || 1;
        const [x, y] = [Math.floor(p[0]), Math.floor(p[1])];
        if (x < 0 || y < 0 || x >= S.info.width || y >= S.info.height) return;
        edit({ kind: "wand", x, y, z: S.z, c: channel(), t: S.t, tol: L.tol * range, label: L.active, guard: L.guard ? L.active : 0, threeD: L.threeD });
        return;
      }
      if (!L.slice || L.z !== S.z) return;
      L.stroke = { z: S.z, pts: [p] };
      stampLocal(p[0], p[1]);
      draw();
    },
    move(e, p) {
      L.hover = p;
      if (L.stroke) strokeTo(p);
      draw();
    },
    up() {
      const st = L.stroke;
      L.stroke = null;
      if (!st) return;
      const label = L.sub === "eraser" ? 0 : L.active;
      const guard = L.guard ? L.active : 0;
      const pts = st.pts.map(([x, y]) => [+x.toFixed(2), +y.toFixed(2)]);
      edit({ kind: "stroke", z: st.z, pts, radius: L.radius, label, guard });
    },
    undo() { if (!L.busy && !L.stroke) edit({ kind: "undo" }); },
    redo() { if (!L.busy && !L.stroke) edit({ kind: "redo" }); },
    active(on) {
      btn.classList.toggle("on", on);
      if (!on) {
        L.hover = null;
        return;
      }
      if ($("tools").hidden || $("tools").classList.contains("only-contrast") || $("tools").classList.contains("only-info")) showPanel("all");
      sec.scrollIntoView({ block: "nearest" });
      draw();
    },
  };

  // Brush outline follows the pointer even when no button is down.
  $("view").addEventListener("pointermove", (e) => {
    if (S.tool !== "label" || S.op) return;
    const r = $("view").getBoundingClientRect();
    L.hover = [
      S.view.cx + (e.clientX - r.left - r.width / 2) / S.view.scale,
      S.view.cy + (e.clientY - r.top - r.height / 2) / S.view.scale,
    ];
    draw();
  });
  $("view").addEventListener("pointerleave", () => {
    if (!L.hover) return;
    L.hover = null;
    draw();
  });

  // [ and ] change the brush size.
  window.addEventListener("keydown", (e) => {
    if (S.tool !== "label" || e.ctrlKey || e.metaKey) return;
    const tag = e.target.tagName;
    if (tag === "INPUT" && e.target.type !== "checkbox" && e.target.type !== "range") return;
    if (e.key !== "[" && e.key !== "]") return;
    L.radius = Math.max(1, Math.min(100, Math.round(L.radius * (e.key === "]" ? 1.25 : 0.8)) + (e.key === "]" ? 1 : 0)));
    syncPanel();
    draw();
    e.preventDefault();
  });

  // ---------- panel controls ----------

  for (const b of sec.querySelectorAll("#lab-tools button")) {
    b.addEventListener("click", () => {
      L.sub = b.dataset.sub;
      syncPanel();
      if (S.tool !== "label") setTool("label");
      draw();
    });
  }
  $("lab-size").addEventListener("input", (e) => { L.radius = +e.target.value; syncPanel(); draw(); });
  $("lab-tol").addEventListener("input", (e) => { L.tol = +e.target.value; syncPanel(); });
  $("lab-op").addEventListener("input", (e) => { L.opacity = +e.target.value; syncPanel(); draw(); });
  $("lab-3d").addEventListener("change", (e) => { L.threeD = e.target.checked; });
  $("lab-guard").addEventListener("change", (e) => { L.guard = e.target.checked; });
  $("lab-show").addEventListener("change", (e) => { L.show = e.target.checked; draw(); });
  $("lab-new").addEventListener("click", () => {
    L.active = Math.max(maxUsed(), L.active) + 1;
    renderList();
    if (S.tool !== "label") setTool("label");
  });

  function fillRange() {
    const c = S.chans[channel()];
    if (!c) return;
    $("lab-lo").value = fmt(c.min);
    $("lab-hi").value = fmt(c.max);
  }
  $("lab-range").addEventListener("click", fillRange);
  $("lab-th").addEventListener("toggle", () => {
    if ($("lab-th").open && $("lab-lo").value === "") fillRange();
  });

  function threshold(all) {
    if (!ready()) return;
    const lo = parseFloat($("lab-lo").value), hi = parseFloat($("lab-hi").value);
    if (!Number.isFinite(lo) || !Number.isFinite(hi) || lo > hi) return toast("请填写阈值范围（最小值不大于最大值）");
    const [z0, z1] = all ? [0, S.info.sizeZ] : [S.z, S.z + 1];
    const minSize = Math.max(1, parseInt($("lab-min").value, 10) || 1);
    edit({ kind: "threshold", z0, z1, c: channel(), t: S.t, lo, hi, label: L.active, guard: L.guard ? L.active : 0, split: $("lab-split").checked, minSize });
  }
  $("lab-th-z").addEventListener("click", () => threshold(false));
  $("lab-th-all").addEventListener("click", () => threshold(true));

  function watershed(all) {
    if (!ready()) return;
    if (L.counts.filter((n, k) => k > 0 && n > 0).length < 2) {
      return toast("至少要两种标签当种子：每个物体一种，背景一种");
    }
    const [z0, z1] = all ? [0, S.info.sizeZ] : [S.z, S.z + 1];
    edit({ kind: "watershed", z0, z1, c: channel(), t: S.t });
  }
  $("lab-ws-z").addEventListener("click", () => watershed(false));
  $("lab-ws-all").addEventListener("click", () => watershed(true));

  // Save before the window goes away.
  window.addEventListener("beforeunload", () => { flush(); });

  syncPanel();
  renderList();
  $("lab-file").textContent = fileText();

  window.TifMrcViewPro.labels = {
    token: () => L.token,
    any,
    flush,
  };
})();
