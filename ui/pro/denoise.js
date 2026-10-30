"use strict";
// Pro denoising: Gaussian, median, anisotropic diffusion and non-local
// means (denoise.rs, NLM on the GPU). Previews the middle of the
// view next to the original, then saves the slice or the whole stack as a
// float32 MRC; the open image is never changed.
// Shares app.js globals: S, F, T, invoke, $, pref, setFft, visibleRegion, fftChannel, toScreen, ext, requestRender, toast, saveDir, setSaveDir, dirname, openPath.
(() => {
  const N = { busy: false, saving: false, res: null, key: "", want: "", timer: 0, img: null, imgKey: "", saved: null };

  // Strength slider per method: [label, min, max, step, default, hint].
  const METHODS = {
    nlm: ["h（× 噪声 σ）", 0.2, 3, 0.05, 0.8, "非局部均值：按邻域块的相似度加权平均，保留纹理和膜；越大越平滑"],
    diffusion: ["边缘阈值 K（× 噪声 σ）", 0.5, 8, 0.1, 2, "各向异性扩散：沿边缘平滑、不跨边缘；越大越平滑"],
    gaussian: ["σ px", 0.3, 6, 0.1, 1, "高斯模糊：最快，细节和边缘也一起变模糊"],
    median: ["半径 px", 1, 7, 1, 1, "中值：去掉坏点和椒盐噪声"],
  };

  const btn = document.createElement("button");
  btn.id = "btn-denoise";
  btn.className = "toggle";
  btn.title = "降噪：非局部均值、各向异性扩散、高斯、中值，预览对比后另存为 32 位浮点 MRC";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><path d="M3 12c1.5-3 2.5 3 4 0s2.5-5 4 0"/><path d="M13 12c2-1.6 6-1.6 8 0"/><path d="M12 4v16" stroke-dasharray="2 2"/></svg>';
  btn.disabled = true;
  $("btn-3d") ? $("btn-3d").before(btn) : $("quickbar").append(btn);

  const panel = document.createElement("aside");
  panel.id = "denoise-panel";
  panel.className = "pro-panel";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head">
      <b>降噪</b>
      <button id="dn-close" class="small" title="关闭">✕</button>
    </div>
    <div class="al-opts">
      <label class="al-wide">方法
        <select id="dn-method">
          <option value="nlm">非局部均值 NLM（推荐）</option>
          <option value="diffusion">各向异性扩散</option>
          <option value="gaussian">高斯</option>
          <option value="median">中值</option>
        </select>
      </label>
      <label class="al-wide"><span id="dn-slabel">强度</span>
        <span class="dn-slider"><input type="range" id="dn-strength" /><output id="dn-sval"></output></span>
      </label>
      <label class="dn-nlm">块半径 px <select id="dn-patch" title="比较相似度用的小块大小；噪声越大可以越大"><option>1</option><option>2</option><option>3</option></select></label>
      <label class="dn-nlm">搜索半径 px <select id="dn-search" title="在多大范围里找相似的块；越大越平滑也越慢"><option>5</option><option>7</option><option>10</option><option>15</option></select></label>
      <label class="dn-diff">迭代次数 <input type="number" id="dn-iter" min="1" max="200" step="1" /></label>
      <label class="dn-z" title="先和上下各几层求平均再降噪（断层图、连续切片）；会降低 Z 方向分辨率">Z 邻层 ±<select id="dn-slab"><option>0</option><option>1</option><option>2</option><option>3</option></select></label>
      <label class="al-check al-wide"><input type="checkbox" id="dn-cpu" /> 只用 CPU</label>
    </div>
    <label class="al-check"><input type="checkbox" id="dn-preview" checked /> 预览（视野中间最多 2048 px）</label>
    <label class="dn-split">左原图 · 右降噪 <input type="range" id="dn-split" min="0" max="100" step="1" /></label>
    <div class="al-row"><span id="dn-status" class="dim"></span></div>
    <div class="al-row">
      <button id="dn-save-one" class="small" title="当前层降噪后存成 32 位浮点 MRC">保存当前层…</button>
      <button id="dn-save-all" class="small primary" title="每一层降噪后存成 32 位浮点 MRC 叠层">保存全部层…</button>
    </div>
    <div class="al-row" id="dn-saved" hidden><span id="dn-saved-name" class="dim"></span><button id="dn-open" class="small">打开</button></div>`;
  $("stage").appendChild(panel);

  const P = { method: "nlm", strength: {}, patch: 1, search: 7, iterations: 10, slab: 0, cpu: false, split: 50, preview: true, ...pref.get("denoise-params", {}) };

  function strengthOf(m) {
    const v = P.strength[m];
    return Number.isFinite(v) ? v : METHODS[m][4];
  }

  function syncMethod() {
    const m = $("dn-method").value;
    const [label, min, max, step, , hint] = METHODS[m];
    const s = $("dn-strength");
    s.min = min;
    s.max = max;
    s.step = step;
    s.value = strengthOf(m);
    $("dn-slabel").textContent = `强度 · ${label}`;
    $("dn-method").title = hint;
    $("dn-sval").textContent = (+s.value).toFixed(step < 1 ? 2 : 0);
    for (const el of panel.querySelectorAll(".dn-nlm")) el.hidden = m !== "nlm";
    for (const el of panel.querySelectorAll(".dn-diff")) el.hidden = m !== "diffusion";
  }

  $("dn-method").value = P.method;
  $("dn-patch").value = P.patch;
  $("dn-search").value = P.search;
  $("dn-iter").value = P.iterations;
  $("dn-slab").value = P.slab;
  $("dn-cpu").checked = !!P.cpu;
  $("dn-split").value = P.split;
  $("dn-preview").checked = P.preview !== false;
  syncMethod();

  function store() {
    P.method = $("dn-method").value;
    P.strength[P.method] = +$("dn-strength").value;
    P.patch = +$("dn-patch").value;
    P.search = +$("dn-search").value;
    P.iterations = Math.max(1, Math.min(200, +$("dn-iter").value || 10));
    P.slab = +$("dn-slab").value;
    P.cpu = $("dn-cpu").checked;
    P.split = +$("dn-split").value;
    P.preview = $("dn-preview").checked;
    pref.set("denoise-params", P);
  }

  function params() {
    const method = $("dn-method").value;
    return {
      method,
      strength: +$("dn-strength").value,
      iterations: P.iterations,
      patch: P.patch,
      search: P.search,
      slab: S.info && S.info.sizeZ > 1 ? P.slab : 0,
      device: P.cpu ? "cpu" : "auto",
    };
  }

  $("dn-method").addEventListener("change", () => {
    syncMethod();
    store();
  });
  $("dn-strength").addEventListener("input", () => {
    const step = +$("dn-strength").step;
    $("dn-sval").textContent = (+$("dn-strength").value).toFixed(step < 1 ? 2 : 0);
    store();
  });
  for (const id of ["dn-patch", "dn-search", "dn-iter", "dn-slab", "dn-cpu", "dn-preview"]) $(id).addEventListener("change", store);
  $("dn-split").addEventListener("input", () => {
    store();
    requestRender();
  });

  function open() {
    panel.hidden = false;
    btn.classList.add("on");
    // The right-hand panels share one spot: one at a time.
    if (F.on) setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
    for (const el of panel.querySelectorAll(".dn-z")) el.hidden = !(S.info && S.info.sizeZ > 1);
    $("dn-save-all").hidden = !(S.info && S.info.sizeZ > 1);
  }

  function close() {
    panel.hidden = true;
    btn.classList.remove("on");
    N.res = null;
    N.key = "";
    requestRender();
  }

  btn.addEventListener("click", () => (panel.hidden ? open() : close()));
  $("dn-close").addEventListener("click", close);
  document.addEventListener("pro-panel", (e) => e.detail !== panel.id && !panel.hidden && close());
  $("btn-fft").addEventListener("click", () => F.on && !panel.hidden && close());

  function status(t) {
    $("dn-status").textContent = t;
  }

  // ---- Preview ----

  function previewReq() {
    const region = visibleRegion();
    if (!region || !S.info) return null;
    return { token: S.token, z: S.z, c: fftChannel(), t: S.t, region: region.map((v) => Math.round(v)), params: params() };
  }

  function parse(buf) {
    const dv = new DataView(buf);
    const rect = [0, 1, 2, 3].map((i) => dv.getUint32(i * 4, true));
    const ms = dv.getFloat32(16, true);
    const sigma = dv.getFloat32(20, true);
    const name = new TextDecoder().decode(new Uint8Array(buf, 24, 16)).replace(/\0+$/, "");
    const values = new Float32Array(buf.slice(40, 40 + rect[2] * rect[3] * 4));
    return { rect, ms, sigma, backend: name, values };
  }

  async function runPreview() {
    if (N.busy || panel.hidden || !$("dn-preview").checked || S.scrubbing) return;
    const req = previewReq();
    if (!req) return;
    const key = JSON.stringify(req);
    if (key === N.key) return;
    N.busy = true;
    status("预览计算中…");
    try {
      const res = parse(await invoke("denoise_preview", { req }));
      if (req.token !== S.token) return;
      N.res = { ...res, token: req.token, z: req.z, t: req.t, c: req.c };
      N.key = key;
      N.imgKey = "";
      const time = res.ms < 1000 ? `${res.ms.toFixed(0)} ms` : `${(res.ms / 1000).toFixed(1)} s`;
      status(`噪声 σ ≈ ${fmtNum(res.sigma)} · ${res.rect[2]}×${res.rect[3]} px · ${time} · ${res.backend}`);
      requestRender();
    } catch (e) {
      N.key = key;
      status("预览失败：" + e);
    } finally {
      N.busy = false;
    }
  }

  function fmtNum(v) {
    return Math.abs(v) >= 100 || v === 0 ? v.toFixed(1) : v.toPrecision(3);
  }

  /** The preview as an image, with the channel's current contrast. */
  function previewCanvas() {
    const r = N.res;
    const ch = S.chans[r.c] || {};
    const lo = ch.min ?? 0;
    const hi = ch.max ?? 1;
    const color = ch.color || [255, 255, 255];
    const key = JSON.stringify([lo, hi, S.gamma, color, r.ms, r.rect]);
    if (N.img && N.imgKey === key) return N.img;
    const [, , w, h] = r.rect;
    const cv = N.img || document.createElement("canvas");
    cv.width = w;
    cv.height = h;
    const g = cv.getContext("2d");
    const im = g.createImageData(w, h);
    const inv = 1 / Math.max(hi - lo, 1e-30);
    const gamma = S.gamma || 1;
    for (let i = 0; i < w * h; i++) {
      let f = Math.min(1, Math.max(0, (r.values[i] - lo) * inv));
      if (gamma !== 1) f = Math.pow(f, gamma);
      const o = i * 4;
      im.data[o] = f * color[0];
      im.data[o + 1] = f * color[1];
      im.data[o + 2] = f * color[2];
      im.data[o + 3] = 255;
    }
    g.putImageData(im, 0, 0);
    N.img = cv;
    N.imgKey = key;
    return cv;
  }

  function overlay(g) {
    const r = N.res;
    if (!r || panel.hidden || !$("dn-preview").checked || r.token !== S.token || S.axis !== "xy" || r.z !== S.z || r.t !== S.t) return;
    const [x, y, w, h] = r.rect;
    const [sx, sy] = toScreen(x, y);
    const [ex, ey] = toScreen(x + w, y + h);
    const split = sx + ((ex - sx) * +$("dn-split").value) / 100;
    g.save();
    g.beginPath();
    g.rect(split, sy, ex - split, ey - sy);
    g.clip();
    g.imageSmoothingEnabled = S.view.scale * (window.devicePixelRatio || 1) < 1;
    g.drawImage(previewCanvas(), sx, sy, ex - sx, ey - sy);
    g.restore();
    g.save();
    g.strokeStyle = "rgba(255,255,255,0.9)";
    g.lineWidth = 1.5;
    g.beginPath();
    g.moveTo(split, sy);
    g.lineTo(split, ey);
    g.stroke();
    g.setLineDash([5, 4]);
    g.strokeStyle = "rgba(231,19,22,0.8)";
    g.strokeRect(sx, sy, ex - sx, ey - sy);
    g.restore();
  }
  ext.overlays.push(overlay);

  // ---- Save ----

  async function save(all) {
    if (N.saving) {
      invoke("denoise_cancel");
      return;
    }
    if (!S.info) return;
    const stem = S.info.name.replace(/\.[^.]+$/, "");
    const p = params();
    const path = await invoke("pick_save_path", { dir: saveDir(), name: `${stem}_denoised_${p.method}${all ? "" : `_z${S.z + 1}`}.mrc`, filter: "MRC", exts: ["mrc"] });
    if (!path) return;
    setSaveDir(dirname(path));
    const req = { token: S.token, zs: all ? [] : [S.z], c: fftChannel(), t: S.t, path, params: p };
    const un = await T.event.listen("denoise-progress", (ev) => {
      const { done, total } = ev.payload;
      status(`保存中 ${done}/${total}`);
    });
    N.saving = true;
    const b = $(all ? "dn-save-all" : "dn-save-one");
    const label = b.textContent;
    b.textContent = "取消";
    $(all ? "dn-save-one" : "dn-save-all").disabled = true;
    const t0 = performance.now();
    try {
      const backend = await invoke("denoise_save", { req });
      N.saved = path;
      status(`已保存 · ${((performance.now() - t0) / 1000).toFixed(1)} s · ${backend}`);
      $("dn-saved-name").textContent = path.split(/[\\/]/).pop();
      $("dn-saved").hidden = false;
    } catch (e) {
      status(String(e).includes("已取消") ? "已取消" : "保存失败：" + e);
    } finally {
      un();
      N.saving = false;
      b.textContent = label;
      $("dn-save-one").disabled = $("dn-save-all").disabled = false;
    }
  }

  $("dn-save-one").addEventListener("click", () => save(false));
  $("dn-save-all").addEventListener("click", () => save(true));
  $("dn-open").addEventListener("click", () => N.saved && openPath(N.saved));

  // Enable in the XY view (not for RGB pictures); recompute the preview a
  // moment after the view, slice or settings stop changing.
  setInterval(() => {
    const ok = !!(S.info && S.axis === "xy" && !S.info.isRgb);
    btn.disabled = !ok;
    if (!ok && !panel.hidden && !N.saving) close();
    if (panel.hidden || !$("dn-preview").checked) return;
    const req = previewReq();
    const want = req ? JSON.stringify(req) : "";
    if (want !== N.want) {
      N.want = want;
      clearTimeout(N.timer);
      N.timer = setTimeout(runPreview, 250);
    } else if (want !== N.key && !N.busy) {
      runPreview();
    }
  }, 200);
  $("dn-preview").addEventListener("change", requestRender);
})();
