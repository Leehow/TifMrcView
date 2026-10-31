"use strict";
// Step ①: beam-induced motion correction (motion::run for one movie,
// motion::series for every movie of an mdoc). The mdoc batch writes the
// tilt series sorted by angle, its even/odd halves and its dataset, which
// becomes the workspace's dataset.
(() => {
  const W = window.TifMrcViewPro.cryoet;
  const host = W.sections[0];
  const M = { movie: null, mdoc: null, folder: null, gain: null, result: null };
  host.innerHTML = `
    <div class="al-head"><b>束流诱导运动校正</b></div>
    <div class="al-opts">
      <label class="al-wide">输入<select id="mo-source"><option value="mdoc">mdoc 倾转批次（推荐）</option><option value="file">单个电影 / EER 文件</option><option value="current">当前打开的电影</option></select></label>
      <div class="al-wide al-row"><button id="mo-mdoc" class="small">选择 mdoc…</button><span id="mo-mdoc-name" class="dim"></span></div>
      <div class="al-wide al-row"><button id="mo-folder" class="small">电影所在文件夹…</button><span id="mo-folder-name" class="dim">默认在 mdoc 同级、上级的 eer / frames 文件夹里找</span></div>
      <div class="al-wide al-row" hidden><button id="mo-movie" class="small">选择电影…</button><span id="mo-movie-name" class="dim"></span></div>
      <label>每帧合并 fraction 数<input id="mo-group" type="number" min="0" max="256" value="0" title="0：EER 自动分成约 40 帧；普通电影不合并"></label>
      <label>EER 渲染<select id="mo-up"><option value="1">1×（4K）</option><option value="2">2×（8K 超分辨率）</option></select></label>
      <label>局部网格 P×P<input id="mo-patches" type="number" min="2" max="10" value="5"></label>
      <label class="al-check"><input id="mo-local" type="checkbox" checked>拟合局部形变</label>
      <div class="al-wide al-row"><button id="mo-gain" class="small">增益参考（可选）…</button><button id="mo-clear-gain" class="small">清除</button><span id="mo-gain-name" class="dim">无</span></div>
      <label>增益旋转<select id="mo-rotate"><option value="0">0°</option><option value="1">90°</option><option value="2">180°</option><option value="3">270°</option></select></label>
      <div><label class="al-check"><input id="mo-flip-x" type="checkbox">左右翻转</label><label class="al-check"><input id="mo-flip-y" type="checkbox">上下翻转</label></div>
      <label>剂量<select id="mo-dose-mode"><option value="auto">mdoc 的 ExposureDose</option><option value="total">每个电影的总剂量</option><option value="frame">每帧剂量</option><option value="none">不做剂量加权</option></select></label>
      <label>剂量 e⁻/Å²<input id="mo-dose" type="number" min="0" step="0.1" placeholder="自动" disabled></label>
      <label>原始像素 Å<input id="mo-pixel" type="number" min="0" step="0.01" placeholder="图像 / mdoc"></label>
      <label>电压 kV<select id="mo-kv"><option value="auto" selected>mdoc（默认 300）</option><option>300</option><option>200</option></select></label>
      <label class="al-check al-wide"><input id="mo-halves" type="checkbox" checked>写出奇偶帧两半（供 Noise2Noise）</label>
    </div>
    <p class="dim">全局对齐后再按 P×P 图块测局部运动，拟合随时间平滑变化的形变场（x、y 二次 × 时间三次），全局平移在傅里叶空间精确施加。剂量按采集先后累计。</p>
    <div class="al-row"><button id="mo-run" class="small primary">校正并保存…</button><button id="mo-open" class="small" disabled>打开结果</button></div>
    <p id="mo-summary" class="dim"></p>
    <canvas id="mo-trajectory" width="700" height="200" hidden title="全局位移（像素）：蓝 dx，红 dy"></canvas>
    <canvas id="mo-vectors" width="700" height="220" hidden title="各图块首末帧之间的局部位移"></canvas>
    <p id="mo-vectors-label" class="dim"></p>
    <div id="mo-table"></div>`;

  const pick = (kind) => invoke("motion_pick", { dir: saveDir(), kind });
  function source() {
    const s = $("mo-source").value;
    $("mo-mdoc").parentElement.hidden = $("mo-folder").parentElement.hidden = s !== "mdoc";
    $("mo-movie").parentElement.hidden = s !== "file";
    return s;
  }
  $("mo-source").onchange = source;
  $("mo-mdoc").onclick = async () => { const p = await pick("mdoc"); if (p) { M.mdoc = p; $("mo-mdoc-name").textContent = W.name(p); } };
  $("mo-movie").onclick = async () => { const p = await pick("movie"); if (p) { M.movie = p; $("mo-movie-name").textContent = W.name(p); } };
  $("mo-folder").onclick = async () => { const p = await invoke("pick_folder", { dir: saveDir() }); if (p) { M.folder = p; $("mo-folder-name").textContent = p; } };
  $("mo-gain").onclick = async () => { const p = await pick("gain"); if (p) { M.gain = p; $("mo-gain-name").textContent = W.name(p); } };
  $("mo-clear-gain").onclick = () => { M.gain = null; $("mo-gain-name").textContent = "无"; };
  $("mo-dose-mode").onchange = () => { $("mo-dose").disabled = !["total", "frame"].includes($("mo-dose-mode").value); };

  function params() {
    const mode = $("mo-dose-mode").value, dose = +$("mo-dose").value;
    if (["total", "frame"].includes(mode) && !(dose > 0)) throw Error("请输入剂量");
    return {
      group: +$("mo-group").value,
      upsample: +$("mo-up").value,
      frameAxis: "auto",
      workingSize: 1024,
      local: $("mo-local").checked,
      patches: +$("mo-patches").value,
      doseWeighted: mode !== "none",
      unweighted: true,
      evenOdd: $("mo-halves").checked,
      pixelA: +$("mo-pixel").value || null,
      voltageKv: +$("mo-kv").value || 300,
      voltageAuto: $("mo-kv").value === "auto",
      dosePerFrame: mode === "frame" ? dose : null,
      totalDose: mode === "total" ? dose : null,
      preExposure: 0,
      gainPath: M.gain,
      gainRotate: +$("mo-rotate").value,
      gainFlipX: $("mo-flip-x").checked,
      gainFlipY: $("mo-flip-y").checked,
      backend: "auto",
      benchmark: false,
      diagnostics: false,
    };
  }

  $("mo-run").onclick = async () => {
    const s = source();
    let p;
    try {
      p = params();
      if (s === "mdoc" && !M.mdoc) throw Error("请先选择 mdoc");
      if (s === "file" && !M.movie) throw Error("请先选择电影");
      if (s === "current" && !S.info) throw Error("请先打开电影");
    } catch (e) { return W.status(e.message); }
    const stem = W.name(s === "mdoc" ? M.mdoc : s === "file" ? M.movie : S.info.name).replace(/\.[^.]+$/, "");
    const output = await invoke("pick_save_path", { dir: saveDir(), name: `${stem}${s === "mdoc" ? "" : "_motion"}.mrc`, filter: "MRC", exts: ["mrc"] });
    if (!output) return;
    setSaveDir(dirname(output));
    const token = S.token;
    const un = await T.event.listen("motion-progress", (e) => W.status(`${e.payload.stage} ${e.payload.done} / ${e.payload.total}`));
    const r = await W.run("运动校正", () => s === "mdoc"
      ? invoke("motion_series", { req: { mdoc: M.mdoc, folder: M.folder, output, params: p } })
      : invoke("motion_run", { req: { token: s === "current" ? token : null, path: s === "file" ? M.movie : null, mdoc: M.mdoc, output, c: s === "current" ? fftChannel() : 0, z: s === "current" ? S.z : 0, t: s === "current" ? S.t : 0, params: p } }), "motion_cancel");
    un();
    if (!r) return;
    M.result = r;
    $("mo-open").disabled = !r.path;
    if (r.records) {
      showSeries(r);
      if (r.path) {
        await W.load(r.path);
        W.status(`已写出 ${r.angles.length} 个倾角的倾转序列${r.angles.length < 3 ? "；倾转对齐至少需要 3 个" : "，可以继续 ② CTF / ③ 倾转对齐"}`);
      } else {
        W.status("没有找到可处理的电影");
      }
    } else {
      showMovie(r);
      W.status("运动校正完成");
    }
  };
  $("mo-open").onclick = () => M.result?.path && openPath(M.result.path);

  function showMovie(r) {
    $("mo-summary").textContent = `${r.rawFractions} fractions → ${r.frames} 帧 · 解码 ${(r.decodeMs / 1000).toFixed(1)} s · 对齐 ${(r.alignmentMs / 1000).toFixed(1)} s · 共 ${(r.totalMs / 1000).toFixed(1)} s · ${r.backend} · 漂移 ${r.totalDrift.toFixed(1)} px · 局部 ${r.meanLocalMotion.toFixed(2)} px` + (r.warnings.length ? ` · ${r.warnings.join("；")}` : "");
    $("mo-table").replaceChildren();
    trajectory(r.global);
    vectors(r);
  }

  function showSeries(r) {
    const done = r.records.filter((v) => v.result);
    $("mo-summary").textContent = `${done.length} 个倾角完成，${r.records.length - done.length} 个跳过` + (r.warnings.length ? ` · ${r.warnings.join("；")}` : "");
    const table = document.createElement("table");
    table.innerHTML = "<thead><tr><th>ZValue</th><th>倾角</th><th>前曝光</th><th>电影 / 状态</th></tr></thead>";
    const body = document.createElement("tbody");
    for (const v of r.records) {
      const row = document.createElement("tr");
      if (v.skipped) row.className = "missing";
      for (const x of [v.z, v.angle.toFixed(2), v.preExposure.toFixed(2), `${W.name(v.movie)} · ${v.skipped || "完成"}`]) {
        const td = document.createElement("td");
        td.textContent = x;
        row.append(td);
      }
      if (v.result) row.onclick = () => { trajectory(v.result.global); vectors(v.result); };
      body.append(row);
    }
    table.append(body);
    $("mo-table").replaceChildren(table);
    if (done.length) { trajectory(done[0].result.global); vectors(done[0].result); }
  }

  function trajectory(shifts) {
    const cv = $("mo-trajectory"), ctx = cv.getContext("2d"), w = cv.width, h = cv.height;
    cv.hidden = false;
    ctx.clearRect(0, 0, w, h);
    if (!shifts.length) return;
    const lo = Math.min(...shifts.flat()), hi = Math.max(...shifts.flat());
    for (const [k, color] of [[0, "#0969da"], [1, "#cf222e"]]) {
      ctx.strokeStyle = color;
      ctx.beginPath();
      shifts.forEach((s, i) => {
        const x = 35 + i / Math.max(1, shifts.length - 1) * (w - 45), y = 15 + (hi - s[k]) / Math.max(1, hi - lo) * (h - 40);
        i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
      });
      ctx.stroke();
    }
    ctx.fillStyle = "#888";
    ctx.font = "16px sans-serif";
    ctx.fillText(`${hi.toFixed(1)} px`, 3, 15);
    ctx.fillText(`${lo.toFixed(1)} px`, 3, h - 12);
  }

  function vectors(r) {
    const cv = $("mo-vectors"), ctx = cv.getContext("2d");
    cv.hidden = false;
    ctx.clearRect(0, 0, cv.width, cv.height);
    const patches = r.local.patches;
    const scale = 25 / Math.max(0.1, ...patches.map((p) => Math.hypot(...p.vector)));
    ctx.strokeStyle = "#cf222e";
    ctx.fillStyle = "#0969da";
    for (const p of patches) {
      const x = 15 + p.centre[0] / r.width * (cv.width - 30), y = 15 + p.centre[1] / r.height * (cv.height - 30);
      const dx = p.vector[0] * scale, dy = p.vector[1] * scale, a = Math.atan2(dy, dx);
      ctx.beginPath();
      ctx.arc(x, y, 3, 0, Math.PI * 2);
      ctx.fill();
      ctx.beginPath();
      ctx.moveTo(x, y);
      ctx.lineTo(x + dx, y + dy);
      ctx.lineTo(x + dx - 6 * Math.cos(a - 0.4), y + dy - 6 * Math.sin(a - 0.4));
      ctx.moveTo(x + dx, y + dy);
      ctx.lineTo(x + dx - 6 * Math.cos(a + 0.4), y + dy - 6 * Math.sin(a + 0.4));
      ctx.stroke();
    }
    $("mo-vectors-label").textContent = patches.length ? `局部位移（首末帧之差），箭头放大 ${scale.toFixed(1)}×` : "局部校正已关闭";
  }

  source();
})();
