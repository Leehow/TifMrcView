"use strict";
// Pro denoising: Gaussian, median, anisotropic diffusion and non-local
// means (denoise.rs, NLM on the GPU), native NAD 3D, and a self-supervised network
// (Noise2Void / Noise2Noise, denoise_learn.rs) trained on the image itself.
// Previews the middle of the
// view next to the original, then saves the slice or the whole stack as a
// float32 MRC; the open image is never changed.
// Shares app.js globals: S, F, T, invoke, $, pref, setFft, visibleRegion, fftChannel, toScreen, ext, draw, requestRender, toast, saveDir, setSaveDir, dirname, openPath.
(() => {
  const N = { generation: 0, cache: new Map(), busy: false, saving: false, res: null, key: "", want: "", timer: 0, img: null, imgKey: "", saved: null };

  // Strength slider per method: [label, min, max, step, default, hint].
  const METHODS = {
    nlm: ["h（× 噪声 σ）", 0.2, 3, 0.05, 0.8, "非局部均值：按邻域块的相似度加权平均，保留纹理和膜；越大越平滑"],
    diffusion: ["边缘阈值 K（× 噪声 σ）", 0.5, 8, 0.1, 2, "各向异性扩散：沿边缘平滑、不跨边缘；越大越平滑"],
    nad3d: ["K（× 体数据标准差 SD）", 0.02, 5, 0.02, 0.1, "内置三维 NAD：三维边缘增强非线性各向异性扩散（NAD / EED），联合处理 XYZ；K 越大越平滑；无需安装 IMOD"],
    gaussian: ["σ px", 0.3, 6, 0.1, 1, "高斯模糊：最快，细节和边缘也一起变模糊"],
    median: ["半径 px", 1, 7, 1, 1, "中值：去掉坏点和椒盐噪声"],
    learned: ["混合比例（1 = 全用网络结果）", 0, 1, 0.05, 1, "自监督学习：在这张图本身上训练一个小网络，不需要干净图像；先选训练方式，点「训练」"],
  };

  const btn = document.createElement("button");
  btn.id = "btn-denoise";
  btn.className = "toggle";
  btn.title = "降噪：NLM、二维 Perona–Malik、内置 NAD 3D、Gaussian、Median、Noise2Void / Noise2Noise，预览对比后另存为 32 位浮点 MRC";
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
          <option value="nlm">非局部均值（Non-Local Means / NLM）</option>
          <option value="diffusion">二维各向异性扩散（Perona–Malik）</option>
          <option value="nad3d">三维非线性各向异性扩散（NAD 3D / EED）</option>
          <option value="gaussian">高斯滤波（Gaussian）</option>
          <option value="median">中值滤波（Median）</option>
          <option value="learned">自监督降噪（Noise2Void / Noise2Noise）</option>
        </select>
      </label>
      <label class="al-wide"><span id="dn-slabel">强度</span>
        <span class="dn-slider"><input type="range" id="dn-strength" /><output id="dn-sval"></output></span>
      </label>
      <label class="dn-nlm">块半径 px <select id="dn-patch" title="比较相似度用的小块大小；噪声越大可以越大"><option>1</option><option>2</option><option>3</option></select></label>
      <label class="dn-nlm">搜索半径 px <select id="dn-search" title="在多大范围里找相似的块；越大越平滑也越慢"><option>5</option><option>7</option><option>10</option><option>15</option></select></label>
      <label class="al-wide dn-learn">训练方式
        <select id="dn-mode">
          <option value="n2v">Noise2Void：任意图像，只需一份数据</option>
          <option value="frames">Noise2Noise：电影帧（奇数帧对偶数帧）</option>
          <option value="neighbours">Noise2Noise：相邻层（断层、连续切片）</option>
        </select>
      </label>
      <label class="dn-learn" title="训练和降噪前先把 k×k 像素平均成一个，结果再放大回原尺寸。计数模式的电影帧每像素电子太少，建议 2–4">合并像素 <select id="dn-bin"><option value="1">不合并</option><option value="2">2×2</option><option value="3">3×3</option><option value="4">4×4</option></select></label>
      <label class="dn-learn">训练步数 <select id="dn-steps" title="越多越好也越慢；用时取决于步数而不是图像大小，GPU 上 1000 步通常一两分钟（Mac 稍慢）"><option>500</option><option>1000</option><option>2000</option><option>4000</option></select></label>
      <span class="dn-learn dn-trainrow"><button id="dn-train" class="small primary">训练</button></span>
      <span class="al-wide dn-learn dim" id="dn-model"></span>
      <div class="al-wide dn-learn dim" id="dn-model-record" hidden></div>
      <label class="dn-diff">迭代次数 <input type="number" id="dn-iter" min="1" max="200" step="1" /></label>
      <span class="al-wide dn-nad dim">内置 NAD 3D / EED · 自动加速 · 首次预览处理整个体，完成后可切换 Z 逐层对比；保存使用相同结果。大体数据建议先分箱。</span>
      <label class="dn-z" title="先和上下各几层求平均再降噪（断层图、连续切片）；会降低 Z 方向分辨率">Z 邻层 ±<select id="dn-slab"><option>0</option><option>1</option><option>2</option><option>3</option></select></label>
    </div>
    <label class="al-check"><input type="checkbox" id="dn-preview" checked /> 预览（视野中间最多 2048 px）</label>
    <label class="dn-split">左原图 · 右降噪 <input type="range" id="dn-split" min="0" max="100" step="1" title="也可在图像上直接拖动分割线" /></label>
    <div class="al-row"><span id="dn-status" class="dim"></span></div>
    <div class="al-row"><button id="dn-cancel-preview" class="small" hidden>取消 NAD 3D 计算</button></div>
    <div class="al-row">
      <button id="dn-save-one" class="small" title="当前层降噪后存成 32 位浮点 MRC">保存当前层…</button>
      <button id="dn-save-all" class="small primary" title="每一层降噪后存成 32 位浮点 MRC 叠层">保存全部层…</button>
    </div>
    <div class="al-row" id="dn-saved" hidden><span id="dn-saved-name" class="dim"></span><button id="dn-open" class="small">打开</button></div>`;
  $("stage").appendChild(panel);

  const P = { method: "nlm", strength: {}, patch: 1, search: 7, iterations: 10, slab: 0, split: 50, preview: true, ...pref.get("denoise-params", {}) };
  // Backend selection is automatic; discard a legacy user override.
  delete P.cpu;

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
    for (const el of panel.querySelectorAll(".dn-diff")) el.hidden = m !== "diffusion" && m !== "nad3d";
    for (const el of panel.querySelectorAll(".dn-nad")) el.hidden = m !== "nad3d";
    for (const el of panel.querySelectorAll(".dn-z")) el.hidden = m === "nad3d" || !(S.info && S.info.sizeZ > 1);
    for (const el of panel.querySelectorAll(".dn-learn")) el.hidden = m !== "learned";
    syncMode();
  }

  const MODE_HINT = {
    n2v: "把每块里少数像素换成邻近像素，让网络从周围猜出它们；适合任何单张图或叠层。要用没滤过波的原始数据，噪声在相邻像素间相关时效果会变差",
    frames: "用奇数帧的平均去预测偶数帧的平均（反过来也一样），两半信号相同、噪声独立；结果是一张图。电影帧要先做漂移校正",
    neighbours: "用每一层去预测相邻的一层；适合层厚比细节小的断层和连续切片，不适合倾转系列",
  };

  function syncMode() {
    const mode = $("dn-mode").value;
    $("dn-mode").title = MODE_HINT[mode];
    const stack = !!(S.info && S.info.sizeZ > 1);
    for (const o of $("dn-mode").options) o.disabled = o.value !== "n2v" && !stack;
    if (!stack && mode !== "n2v") $("dn-mode").value = "n2v";
  }

  $("dn-method").value = P.method;
  $("dn-patch").value = P.patch;
  $("dn-search").value = P.search;
  $("dn-iter").value = P.iterations;
  $("dn-slab").value = P.slab;
  $("dn-split").value = P.split;
  $("dn-preview").checked = P.preview !== false;
  $("dn-mode").value = P.mode || "n2v";
  $("dn-steps").value = P.steps || 1000;
  $("dn-bin").value = P.bin || 1;
  syncMethod();

  const modelContext = () => ({ token: S.token, c: fftChannel(), t: S.t });
  const modelRecords = window.TifMrcViewPro.models.attach("denoise", "dn-model-record", modelContext);

  async function restoreModel() {
    if (!S.info || S.axis !== "xy" || N.training || N.saving) return;
    const req = modelContext(), key = JSON.stringify(req);
    if (key === N.restoreKey) return;
    N.restoreKey = key;
    N.restoring = true;
    N.restoreError = null;
    clearPreviews();
    const result = await modelRecords.restore(req);
    if (result.stale || key !== N.restoreKey) return;
    N.restoring = false;
    N.restoreError = result.error;
    if (result.saved) {
      const saved = result.saved, effective = saved.record.parameters.effective;
      for (const [id, v] of [["dn-mode", effective.mode], ["dn-bin", effective.bin], ["dn-slab", effective.slab], ["dn-steps", effective.config.steps]]) window.TifMrcViewPro.models.setValue(id, v);
      const name = { n2v: "Noise2Void / N2V", frames: "Noise2Noise / N2N（奇偶帧）", neighbours: "Noise2Noise / N2N（相邻层）" }[effective.mode];
      N.model = `${name} · ${effective.config.steps} 步`;
      N.modelToken = req.token;
      N.frames = effective.mode === "frames";
      $("dn-model").textContent = `已自动恢复：${N.model}`;
      store();
      clearPreviews();
      requestRender();
    }
  }

  function store() {
    P.method = $("dn-method").value;
    P.strength[P.method] = +$("dn-strength").value;
    P.patch = +$("dn-patch").value;
    P.search = +$("dn-search").value;
    P.iterations = Math.max(1, Math.min(200, +$("dn-iter").value || 10));
    P.slab = +$("dn-slab").value;
    P.split = +$("dn-split").value;
    P.preview = $("dn-preview").checked;
    P.mode = $("dn-mode").value;
    P.steps = +$("dn-steps").value;
    P.bin = +$("dn-bin").value;
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
      slab: method !== "nad3d" && S.info && S.info.sizeZ > 1 ? P.slab : 0,
      device: "auto",
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
  for (const id of ["dn-patch", "dn-search", "dn-iter", "dn-slab", "dn-preview", "dn-steps", "dn-bin"]) $(id).addEventListener("change", store);
  $("dn-mode").addEventListener("change", () => {
    syncMode();
    // Frame halves of counting-mode movies are too sparse at full resolution.
    $("dn-bin").value = $("dn-mode").value === "frames" ? 2 : 1;
    store();
  });

  // ---- Self-supervised training ----

  async function train() {
    if (N.restoring) { status("正在读取已保存的模型…"); return; }
    N.restoreError = null;
    if (N.training) {
      invoke("denoise_cancel");
      return;
    }
    if (!S.info) return;
    const req = { token: S.token, c: fftChannel(), t: S.t, mode: $("dn-mode").value, steps: +$("dn-steps").value, bin: +$("dn-bin").value, slab: S.info.sizeZ > 1 ? P.slab : 0 };
    const un = await T.event.listen("denoise-train", (ev) => {
      const { step, steps, loss } = ev.payload;
      $("dn-model").textContent = `训练中 ${step}/${steps} · 损失 ${loss.toPrecision(3)}` + (N.live ? " · 预览随训练实时更新" : "");
    });
    // Every few seconds the network so far: show its result as it improves.
    const unLive = await T.event.listen("denoise-live", () => {
      N.live = true;
      N.modelToken = req.token;
      clearPreviews();
    });
    const prevToken = N.modelToken;
    N.live = false;
    N.training = true;
    $("dn-train").textContent = "取消";
    $("dn-model").textContent = "准备训练数据…";
    try {
      const r = await invoke("denoise_train", { req });
      if (req.token === S.token && req.c === fftChannel() && req.t === S.t) modelRecords.show(r.persistence);
      const name = { n2v: "Noise2Void", frames: "Noise2Noise 奇偶帧", neighbours: "Noise2Noise 相邻层" }[r.mode];
      N.model = `${name} · ${r.images} ${r.mode === "n2v" ? "张图" : "对"} · ${req.steps} 步${req.bin > 1 ? ` · 合并 ${req.bin}×${req.bin}` : ""} · 损失 ${r.loss.toPrecision(3)} · ${r.seconds.toFixed(0)} s · ${r.backend}`;
      N.frames = r.mode === "frames";
      N.modelToken = req.token;
      $("dn-model").textContent = `已训练：${N.model}` + (r.warning ? `\n⚠ ${r.warning}` : "");
      // Show the new model's result.
      clearPreviews();
    } catch (e) {
      $("dn-model").textContent = String(e).includes("已取消") ? "已取消训练" : "训练失败：" + e;
      // The backend kept the earlier network.
      N.modelToken = prevToken;
    } finally {
      un();
      unLive();
      N.training = false;
      N.live = false;
      clearPreviews();
      $("dn-train").textContent = "训练";
    }
  }
  $("dn-train").addEventListener("click", train);
  $("dn-split").addEventListener("input", () => {
    store();
    draw();
  });

  function open() {
    panel.hidden = false;
    btn.classList.add("on");
    // The right-hand panels share one spot: one at a time.
    if (F.on) setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
    syncMethod();
    syncMode();
    $("dn-save-all").hidden = !(S.info && S.info.sizeZ > 1);
  }

  function close() {
    panel.hidden = true;
    btn.classList.remove("on");
    clearPreviews();
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

  function previewKey(req) {
    return JSON.stringify([N.generation, req]);
  }

  function clearPreviews() {
    N.generation++;
    N.cache.clear();
    N.res = null;
    N.key = "";
    N.imgKey = "";
  }

  function showPreview(res, key) {
    N.res = res;
    N.key = key;
    N.imgKey = "";
    const time = res.ms < 1000 ? `${res.ms.toFixed(0)} ms` : `${(res.ms / 1000).toFixed(1)} s`;
    status(`噪声 σ ≈ ${fmtNum(res.sigma)} · ${res.rect[2]}×${res.rect[3]} px · ${time} · ${res.backend}`);
    requestRender();
  }

  async function runPreview() {
    if (N.restoring || (N.restoreError && params().method === "learned") || (N.training && !N.live) || N.saving || panel.hidden || !$("dn-preview").checked || S.scrubbing) return;
    const req = previewReq();
    if (!req) return;
    const key = previewKey(req);
    const cached = N.cache.get(key);
    if (cached) {
      N.cache.delete(key);
      N.cache.set(key, cached);
      if (N.key !== key) showPreview(cached, key);
      return;
    }
    if (N.busy || key === N.key) return;
    const generation = N.generation;
    N.busy = true;
    $("dn-cancel-preview").hidden = req.params.method !== "nad3d";
    status(req.params.method === "nad3d" ? "内置 NAD 3D：正在处理整个体，完成后可逐层对比…" : `第 ${S.z + 1} 层预览计算中…`);
    try {
      const res = { ...parse(await invoke("denoise_preview", { req })), z: req.z, t: req.t, c: req.c, token: req.token };
      if (req.token !== S.token || generation !== N.generation) return;
      N.cache.set(key, res);
      // At most eight layers and 64 MiB; revisiting them needs no inference.
      let bytes = [...N.cache.values()].reduce((n, r) => n + r.values.byteLength, 0);
      while (N.cache.size > 1 && (N.cache.size > 8 || bytes > 64 * 1024 * 1024)) {
        const oldest = N.cache.keys().next().value;
        bytes -= N.cache.get(oldest).values.byteLength;
        N.cache.delete(oldest);
      }
      const current = previewReq();
      if (current && previewKey(current) === key) showPreview(res, key);
    } catch (e) {
      const current = previewReq();
      if (generation === N.generation && current && previewKey(current) === key) {
        N.key = key;
        status("预览失败：" + e);
      }
    } finally {
      N.busy = false;
      $("dn-cancel-preview").hidden = true;
      const current = previewReq();
      if (current && previewKey(current) !== N.key) setTimeout(runPreview, 0);
    }
  }

  function fmtNum(v) {
    return Math.abs(v) >= 100 || v === 0 ? v.toFixed(1) : v.toPrecision(3);
  }

  /** 0.5 and 99.5 percentiles of the preview's values. */
  function ownRange(r) {
    if (!r.range) {
      const step = Math.max(1, Math.floor(r.values.length / 200000));
      const v = [];
      for (let i = 0; i < r.values.length; i += step) if (Number.isFinite(r.values[i])) v.push(r.values[i]);
      v.sort((a, b) => a - b);
      r.range = v.length ? [v[Math.floor(v.length * 0.005)], v[Math.min(v.length - 1, Math.floor(v.length * 0.995))]] : [0, 1];
    }
    return r.range;
  }

  /** The preview as an image, with the channel's current contrast. */
  function previewCanvas() {
    const r = N.res;
    const ch = S.chans[r.c] || {};
    // A movie's result is a mean of many frames, far from one frame's range.
    const [lo, hi] = N.frames && S.info.sizeZ > 1 && $("dn-method").value === "learned" ? ownRange(r) : [ch.min ?? 0, ch.max ?? 1];
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

  function comparisonGeometry() {
    if (panel.hidden || !$("dn-preview").checked || S.axis !== "xy" || S.xyz) return null;
    const req = previewReq();
    const key = req && previewKey(req);
    const r = key && N.cache.get(key);
    if (!r || r.token !== S.token || r.z !== S.z || r.t !== S.t) return null;
    const [x, y, w, h] = r.rect;
    const [sx, sy] = toScreen(x, y);
    const [ex, ey] = toScreen(x + w, y + h);
    if (!(ex > sx && ey > sy)) return null;
    const split = sx + ((ex - sx) * +$("dn-split").value) / 100;
    return { r, sx, sy, ex, ey, split, context: JSON.stringify([key, sx, sy, ex, ey]) };
  }

  function overlay(g) {
    const geometry = comparisonGeometry();
    if (N.splitDrag && geometry?.context !== N.splitDrag.context) endSplitDrag();
    if (!geometry) return;
    const { r, sx, sy, ex, ey, split } = geometry;
    if (N.res !== r) { N.res = r; N.imgKey = ""; }
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

  // ---- Drag the comparison divider ----

  const splitView = $("view");

  function endSplitDrag() {
    const drag = N.splitDrag;
    N.splitDrag = null;
    if (drag) {
      if (splitView.hasPointerCapture(drag.id)) splitView.releasePointerCapture(drag.id);
      store();
    }
    if (N.splitHover) {
      N.splitHover = false;
      splitView.style.cursor = "";
    }
  }

  function splitPointer(e) {
    const geometry = comparisonGeometry();
    if (N.splitDrag && geometry?.context !== N.splitDrag.context) endSplitDrag();
    const drag = N.splitDrag;
    const onView = e.target === splitView;
    if (!drag && !onView) { endSplitDrag(); return; }
    const rect = splitView.getBoundingClientRect();
    const x = e.clientX - rect.left, y = e.clientY - rect.top;
    const consume = () => { e.preventDefault(); e.stopImmediatePropagation(); };
    if (drag) {
      if (e.pointerId !== drag.id) { if (onView) consume(); return; }
      if (e.type === "pointermove" || e.type === "pointerup") {
        $("dn-split").value = Math.round(Math.max(0, Math.min(100, (x - drag.offset - geometry.sx) * 100 / (geometry.ex - geometry.sx))));
        draw();
      }
      if (e.type !== "pointermove") endSplitDrag();
      consume();
      return;
    }
    const near = geometry && !S.op && !S.textEdit && !e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey &&
      y >= geometry.sy && y <= geometry.ey && Math.abs(x - geometry.split) <= (e.pointerType === "touch" ? 14 : 8);
    if (e.type === "pointermove") {
      if (near && !e.buttons) {
        N.splitHover = true;
        splitView.style.cursor = "ew-resize";
        consume();
      } else endSplitDrag();
    } else if (e.type === "pointerdown" && near && e.button === 0 && e.isPrimary !== false) {
      N.splitDrag = { id: e.pointerId, context: geometry.context, offset: x - geometry.split };
      N.splitHover = true;
      splitView.style.cursor = "ew-resize";
      splitView.setPointerCapture(e.pointerId);
      consume();
    } else if (e.type === "dblclick" && near && e.button === 0) consume();
  }

  // Capture on the window before canvas touch/annotation handlers start a
  // gesture. Pointer capture keeps the divider responsive beyond the image.
  for (const type of ["pointerdown", "pointermove", "pointerup", "pointercancel", "lostpointercapture", "dblclick"]) window.addEventListener(type, splitPointer, true);
  window.addEventListener("blur", endSplitDrag);
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && N.splitDrag) {
      endSplitDrag();
      e.preventDefault();
      e.stopImmediatePropagation();
    }
  }, true);

  // ---- Save ----

  async function save(all) {
    if (N.restoring || (N.restoreError && params().method === "learned")) { status("模型尚未成功恢复，请检查训练记录"); return; }
    if (N.saving) {
      invoke("denoise_cancel");
      return;
    }
    if (!S.info) return;
    if (N.training) return status("训练完成后再保存");
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

  // Another image: the save line belongs to the old one, the training mode
  // must suit the new one, and the model is marked as trained elsewhere.
  function imageChanged() {
    const first = N.token === undefined;
    N.token = S.token;
    clearPreviews();
    if (first || !S.info) return;
    N.saved = null;
    $("dn-saved").hidden = true;
    const before = $("dn-mode").value;
    syncMode();
    if ($("dn-mode").value !== before) $("dn-bin").value = 1;
    if (N.model) $("dn-model").textContent = (N.modelToken === S.token ? "已训练：" + N.model : `已训练（在另一张图上）：${N.model}\n数据类型不同时效果可能较差，最好在这张图上重新训练`);
  }

  // Enable in the XY view (not for RGB pictures); recompute the preview a
  // moment after the view, slice or settings stop changing.
  setInterval(() => {
    if (S.token !== N.token) imageChanged();
    restoreModel();
    const ok = !!(S.info && S.axis === "xy" && !S.info.isRgb);
    btn.disabled = !ok;
    if (!ok && !panel.hidden && !N.saving) close();
    if (panel.hidden || !$("dn-preview").checked) return;
    const req = previewReq();
    const want = req ? previewKey(req) : "";
    if (want !== N.want) {
      N.want = want;
      clearTimeout(N.timer);
      N.timer = setTimeout(runPreview, 250);
    } else if (want !== N.key && !N.busy) {
      runPreview();
    }
  }, 200);
  $("dn-preview").addEventListener("change", requestRender);
  $("dn-cancel-preview").addEventListener("click", () => {
    const req = previewReq();
    if (req) N.key = previewKey(req);
    status("已取消 NAD 3D 计算");
    invoke("denoise_cancel");
  });
})();
