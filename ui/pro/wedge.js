"use strict";
// Pro missing-wedge correction for tomograms (wedge.rs, after IsoNet): a 3D
// network trained on the open stack itself fills in the Fourier wedge a
// tilt series cannot measure, optionally denoising too. Previews a slice in
// the XY, XZ or YZ view next to the original, then saves the corrected
// volume as a float32 MRC; the open image is never changed.
// Shares app.js globals: S, T, invoke, $, pref, visibleRegion, fftChannel, toScreen, ext, requestRender, saveDir, setSaveDir, dirname, openPath.
(() => {
  const W = { busy: false, training: false, saving: false, res: null, key: "", want: "", timer: 0, img: null, imgKey: "", saved: null, model: null };
  const AXES = ["X", "Y", "Z"];
  // [beam, tilt axis] choices.
  const GEOMS = [[2, 1], [2, 0], [1, 0], [1, 2], [0, 1], [0, 2]];

  const btn = document.createElement("button");
  btn.id = "btn-wedge";
  btn.className = "toggle";
  btn.title = "缺失楔补偿（IsoNet 方法）：在断层图本身上训练 3D 网络，补回倾转范围外缺失的信息，减轻沿电子束方向的拉长伪影";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><path d="M12 12 4 6v12z" fill="currentColor" fill-opacity=".25"/><path d="M12 12l8-6v12z" fill="currentColor" fill-opacity=".25"/><path d="M12 12 6 3h12zM12 12l-6 9h12z"/></svg>';
  btn.disabled = true;
  $("btn-denoise") ? $("btn-denoise").after(btn) : $("quickbar").append(btn);

  const panel = document.createElement("aside");
  panel.id = "wedge-panel";
  panel.className = "pro-panel";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head">
      <b>缺失楔补偿</b>
      <button id="wg-close" class="small" title="关闭">✕</button>
    </div>
    <div class="al-opts">
      <label class="al-wide">方向（电子束 · 倾转轴）
        <select id="wg-geom">${GEOMS.map(([b, t], i) => `<option value="${i}">电子束 ${AXES[b]} · 倾转轴 ${AXES[t]}</option>`).join("")}</select>
      </label>
      <label>倾转范围 °
        <span class="wg-range"><input type="number" id="wg-min" min="-89" max="-10" step="1" /> 到 <input type="number" id="wg-max" min="10" max="89" step="1" /></span>
      </label>
      <label title="先把体数据按 n×n×n 平均缩小再处理；断层图常用 bin 4 左右，太大的体会很慢">分箱 <select id="wg-bin"><option>1</option><option>2</option><option>3</option><option>4</option></select></label>
      <span class="al-wide"><button id="wg-detect" class="small" title="从功率谱里找缺失楔：方向和倾转范围">自动检测方向和范围</button></span>
      <span class="al-wide dim" id="wg-detected"></span>
      <label title="训练用的立方块边长">子块 px <select id="wg-cube"><option>32</option><option>64</option></select></label>
      <label title="从结构最丰富的地方切多少块">子块数 <select id="wg-count"><option>60</option><option>120</option><option>240</option></select></label>
      <label title="每一轮用上一轮的结果补楔再训练；越多越好也越慢">迭代轮数 <select id="wg-iter"><option>5</option><option>10</option><option>15</option><option>20</option></select></label>
      <label title="RTX 5080 上约 1 秒一步，Mac 上慢约 4 倍">每轮步数 <select id="wg-steps"><option>100</option><option>200</option><option>400</option></select></label>
      <label title="先对体数据做高斯低通，噪声大的断层图（尤其 bin 1）否则会把噪声纹理填进缺失楔">预滤波
        <select id="wg-smooth"><option value="0">不滤</option><option value="0.7">弱</option><option value="1">中</option><option value="1.5">强</option></select>
      </label>
      <label title="后半程在输入上加缺楔形状的噪声，网络同时学会去噪（IsoNet 的做法）">同时去噪
        <select id="wg-noise"><option value="0">不去噪</option><option value="0.05">弱</option><option value="0.1">中</option><option value="0.2">强</option></select>
      </label>
      <span class="al-wide"><button id="wg-train" class="small primary">训练</button></span>
      <span class="al-wide dim" id="wg-model"></span>
    </div>
    <label class="al-check"><input type="checkbox" id="wg-preview" checked /> 预览（XY、XZ、YZ 视图都可以，XZ 最明显）</label>
    <label class="dn-split">左原图 · 右补偿后 <input type="range" id="wg-split" min="0" max="100" step="1" /></label>
    <div class="al-row"><span id="wg-status" class="dim"></span></div>
    <div class="al-row">
      <button id="wg-save" class="small primary" title="整个体补偿后存成 32 位浮点 MRC（分箱后的大小）">保存补偿后的体…</button>
    </div>
    <div class="al-row" id="wg-saved" hidden><span id="wg-saved-name" class="dim"></span><button id="wg-open" class="small">打开</button></div>`;
  $("stage").appendChild(panel);

  const P = { geom: 0, min: -60, max: 60, bin: 0, cube: 64, count: 120, iterations: 10, steps: 100, noise: 0, smooth: 1, split: 50, preview: true, ...pref.get("wedge-params", {}) };
  $("wg-geom").value = P.geom;
  $("wg-min").value = P.min;
  $("wg-max").value = P.max;
  $("wg-cube").value = P.cube;
  $("wg-count").value = P.count;
  $("wg-iter").value = P.iterations;
  $("wg-steps").value = P.steps;
  $("wg-noise").value = P.noise;
  $("wg-smooth").value = P.smooth;
  $("wg-split").value = P.split;
  $("wg-preview").checked = P.preview !== false;

  function store() {
    P.geom = +$("wg-geom").value;
    P.min = Math.max(-89, Math.min(-10, +$("wg-min").value || -60));
    P.max = Math.max(10, Math.min(89, +$("wg-max").value || 60));
    P.cube = +$("wg-cube").value;
    P.count = +$("wg-count").value;
    P.iterations = +$("wg-iter").value;
    P.steps = +$("wg-steps").value;
    P.noise = +$("wg-noise").value;
    P.smooth = +$("wg-smooth").value;
    P.split = +$("wg-split").value;
    P.preview = $("wg-preview").checked;
    pref.set("wedge-params", P);
  }
  for (const id of ["wg-geom", "wg-min", "wg-max", "wg-cube", "wg-count", "wg-iter", "wg-steps", "wg-noise", "wg-smooth", "wg-preview"]) $(id).addEventListener("change", store);
  $("wg-bin").addEventListener("change", () => (W.binPath = baseInfo() && baseInfo().path));
  $("wg-split").addEventListener("input", () => {
    store();
    requestRender();
  });
  $("wg-preview").addEventListener("change", requestRender);

  /** The stack itself, also while an XZ / YZ side view is shown. */
  function baseInfo() {
    return S.axis === "xy" ? S.info : S.base && S.base.info;
  }
  function baseToken() {
    return S.axis === "xy" ? S.token : S.base && S.base.token;
  }

  /** Bin that keeps the volume around 60 M voxels at most (training
   *  time hardly depends on it, applying and memory do). */
  function autoBin(info) {
    const v = info.width * info.height * info.sizeZ;
    return v > 1.9e9 ? 4 : v > 500e6 ? 3 : v > 60e6 ? 2 : 1;
  }

  function geometry() {
    const [beam, tilt] = GEOMS[+$("wg-geom").value];
    return { beam, tilt, maxTilt: P.max, minTilt: P.min };
  }

  function baseReq() {
    const info = baseInfo();
    return { token: baseToken(), c: fftChannel(), t: S.t, bin: +$("wg-bin").value || autoBin(info) };
  }

  function status(t) {
    $("wg-status").textContent = t;
  }

  async function detect() {
    if (!baseInfo() || W.training) return;
    $("wg-detected").textContent = "读取体数据、计算功率谱…";
    $("wg-detect").disabled = true;
    try {
      const d = await invoke("wedge_detect", { req: baseReq() });
      const g = d.geometry;
      const i = GEOMS.findIndex(([b, t]) => b === g.beam && t === g.tilt);
      if (i >= 0) $("wg-geom").value = i;
      $("wg-min").value = -g.maxTilt;
      $("wg-max").value = g.maxTilt;
      store();
      const sure = d.contrast >= 0.5;
      $("wg-detected").textContent = `电子束 ${AXES[g.beam]} · 倾转轴 ${AXES[g.tilt]} · 约 ±${g.maxTilt}° · 楔内弱 ${d.contrast.toFixed(2)}` + (sure ? "" : "\n⚠ 缺失楔不明显，请按采集参数手动设置");
    } catch (e) {
      $("wg-detected").textContent = "检测失败：" + e;
    } finally {
      $("wg-detect").disabled = false;
    }
  }
  $("wg-detect").addEventListener("click", detect);

  async function train() {
    if (W.training) {
      invoke("wedge_cancel");
      return;
    }
    if (!baseInfo()) return;
    store();
    const req = { ...baseReq(), geometry: geometry(), config: { cube: P.cube, count: P.count, iterations: P.iterations, steps: P.steps, noise: P.noise, smooth: P.smooth } };
    const un = await T.event.listen("wedge-train", (ev) => {
      const p = ev.payload;
      $("wg-model").textContent = `训练中 第 ${p.iteration}/${p.iterations} 轮 · ${p.step}/${p.steps} 步 · 损失 ${p.loss.toPrecision(3)}`;
    });
    W.training = true;
    $("wg-train").textContent = "取消";
    $("wg-model").textContent = "读取体数据、切子块…";
    try {
      const r = await invoke("wedge_train", { req });
      const g = req.geometry;
      W.model = { token: req.token };
      const t = r.seconds < 120 ? `${r.seconds.toFixed(0)} s` : `${(r.seconds / 60).toFixed(1)} min`;
      $("wg-model").textContent = `已训练：${r.dims.join("×")}（bin ${req.bin}）· 电子束 ${AXES[g.beam]} · ${g.minTilt}°…${g.maxTilt}° · 子块 ${r.cube} · 损失 ${r.loss.toPrecision(3)} · ${t} · ${r.backend}`;
      W.key = "";
    } catch (e) {
      $("wg-model").textContent = String(e).includes("已取消") ? "已取消训练" : "训练失败：" + e;
    } finally {
      un();
      W.training = false;
      $("wg-train").textContent = "训练";
    }
  }
  $("wg-train").addEventListener("click", train);

  function open() {
    panel.hidden = false;
    btn.classList.add("on");
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
    const info = baseInfo();
    if (info && W.binPath !== info.path) {
      $("wg-bin").value = autoBin(info);
      W.binPath = info.path;
    }
  }

  function close() {
    panel.hidden = true;
    btn.classList.remove("on");
    W.res = null;
    W.key = "";
    requestRender();
  }

  btn.addEventListener("click", () => (panel.hidden ? open() : close()));
  $("wg-close").addEventListener("click", close);
  document.addEventListener("pro-panel", (e) => e.detail !== panel.id && !panel.hidden && close());

  // ---- Preview ----

  function previewReq() {
    const region = visibleRegion();
    if (!region || !W.model || W.model.token !== baseToken()) return null;
    return { token: W.model.token, axis: S.axis, index: S.z, region: region.map((v) => Math.max(0, Math.round(v))) };
  }

  function parse(buf) {
    const dv = new DataView(buf);
    const u = [0, 1, 2, 3, 4, 5].map((i) => dv.getUint32(i * 4, true));
    const ms = dv.getFloat32(24, true);
    const name = new TextDecoder().decode(new Uint8Array(buf, 28, 16)).replace(/\0+$/, "");
    const [w, h] = [u[4], u[5]];
    return { rect: u.slice(0, 4), w, h, ms, backend: name, values: new Float32Array(buf.slice(44, 44 + w * h * 4)) };
  }

  async function runPreview() {
    if (W.busy || W.training || W.saving || panel.hidden || !$("wg-preview").checked || S.scrubbing) return;
    const req = previewReq();
    if (!req) return;
    const key = JSON.stringify(req);
    if (key === W.key) return;
    W.busy = true;
    status("预览计算中…");
    try {
      const res = parse(await invoke("wedge_preview", { req }));
      W.res = { ...res, axis: req.axis, index: req.index, token: req.token };
      W.key = key;
      W.imgKey = "";
      const time = res.ms < 1000 ? `${res.ms.toFixed(0)} ms` : `${(res.ms / 1000).toFixed(1)} s`;
      status(`${res.w}×${res.h} px · ${time} · ${res.backend}`);
      requestRender();
    } catch (e) {
      W.key = key;
      status("预览失败：" + e);
    } finally {
      W.busy = false;
    }
  }

  /** The preview as an image, with the channel's current contrast. */
  function previewCanvas() {
    const r = W.res;
    const ch = S.chans[fftChannel()] || {};
    const lo = ch.min ?? 0;
    const hi = ch.max ?? 1;
    const color = ch.color || [255, 255, 255];
    const key = JSON.stringify([lo, hi, S.gamma, color, r.ms, r.rect]);
    if (W.img && W.imgKey === key) return W.img;
    const cv = W.img || document.createElement("canvas");
    cv.width = r.w;
    cv.height = r.h;
    const g = cv.getContext("2d");
    const im = g.createImageData(r.w, r.h);
    const inv = 1 / Math.max(hi - lo, 1e-30);
    const gamma = S.gamma || 1;
    for (let i = 0; i < r.w * r.h; i++) {
      let f = Math.min(1, Math.max(0, (r.values[i] - lo) * inv));
      if (gamma !== 1) f = Math.pow(f, gamma);
      const o = i * 4;
      im.data[o] = f * color[0];
      im.data[o + 1] = f * color[1];
      im.data[o + 2] = f * color[2];
      im.data[o + 3] = 255;
    }
    g.putImageData(im, 0, 0);
    W.img = cv;
    W.imgKey = key;
    return cv;
  }

  function overlay(g) {
    const r = W.res;
    if (!r || panel.hidden || !$("wg-preview").checked || r.token !== baseToken() || r.axis !== S.axis || r.index !== S.z) return;
    const [x, y, w, h] = r.rect;
    const [sx, sy] = toScreen(x, y);
    const [ex, ey] = toScreen(x + w, y + h);
    const split = sx + ((ex - sx) * +$("wg-split").value) / 100;
    g.save();
    g.beginPath();
    g.rect(split, sy, ex - split, ey - sy);
    g.clip();
    // As the original is drawn: smooth only when a preview pixel is under a screen pixel.
    g.imageSmoothingEnabled = S.view.scale * (window.devicePixelRatio || 1) * (w / r.w) < 1;
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

  async function save() {
    if (W.saving) {
      invoke("wedge_cancel");
      return;
    }
    if (!W.model) {
      status("请先训练");
      return;
    }
    const info = baseInfo();
    const stem = info.name.replace(/\.[^.]+$/, "");
    const path = await invoke("pick_save_path", { dir: saveDir(), name: `${stem}_wedge.mrc`, filter: "MRC", exts: ["mrc"] });
    if (!path) return;
    setSaveDir(dirname(path));
    const un = await T.event.listen("wedge-progress", (ev) => {
      const { done, total } = ev.payload;
      status(`保存中 ${done}/${total} 块`);
    });
    W.saving = true;
    $("wg-save").textContent = "取消";
    const t0 = performance.now();
    try {
      const backend = await invoke("wedge_save", { path });
      W.saved = path;
      status(`已保存 · ${((performance.now() - t0) / 1000).toFixed(1)} s · ${backend}`);
      $("wg-saved-name").textContent = path.split(/[\\/]/).pop();
      $("wg-saved").hidden = false;
    } catch (e) {
      status(String(e).includes("已取消") ? "已取消" : "保存失败：" + e);
    } finally {
      un();
      W.saving = false;
      $("wg-save").textContent = "保存补偿后的体…";
    }
  }
  $("wg-save").addEventListener("click", save);
  $("wg-open").addEventListener("click", () => W.saved && openPath(W.saved));

  // Enabled for Z stacks of at least 16 slices (also in the side views);
  // recompute the preview a moment after the view or slice stops changing.
  setInterval(() => {
    const info = baseInfo();
    const ok = !!(info && info.sizeZ >= 16 && !info.isRgb);
    btn.disabled = !ok;
    if (!ok && !panel.hidden && !W.saving && !W.training) close();
    if (panel.hidden || !$("wg-preview").checked) return;
    const req = previewReq();
    const want = req ? JSON.stringify(req) : "";
    if (want !== W.want) {
      W.want = want;
      clearTimeout(W.timer);
      W.timer = setTimeout(runPreview, 300);
    } else if (want !== W.key && !W.busy) {
      runPreview();
    }
  }, 200);
})();
