"use strict";
// Cryo-ET pipeline. The alignment belongs to a file token; excluded views
// retain their contours for re-solving. All residuals are original pixels.
(() => {
  const A = { token: null, res: null, busy: false, c: 0, t: 0 };
  const W=window.TifMrcViewPro.cryoet;
  const panel=document.createElement("div"); panel.id="cryo-align";
  panel.innerHTML = `
    <div class="al-head"><b>倾转对齐</b></div>
    <div class="al-opts">
      <label class="al-wide">倾转角来源<select id="tm-source"><option value="auto">自动 .mdoc / .tlt / .rawtlt</option><option value="manual">起始角 / 步长</option></select></label>
      <label>起始角 °<input id="tm-start" type="number" value="-60" step="0.1"></label>
      <label>步长 °<input id="tm-step" type="number" value="3" step="0.1"></label>
      <label>倾转轴角 °（从 X 逆时针）<input id="tm-axis" type="number" value="90" step="0.2"></label>
      <label>计算 binning<select id="tm-bin"><option value="0" selected>自动（短边约 1024）</option><option>1</option><option>2</option><option>4</option><option>8</option></select></label>
      <label class="al-check al-wide"><input id="tm-sweep" type="checkbox" checked>自动扫描倾转轴角和 patch 配置</label>
    </div>
    <div class="al-row"><button id="tm-run" class="small primary">对齐</button><button id="tm-cancel" class="small" hidden>取消</button></div>
    <div id="tm-status" class="dim" role="status"></div>
    <div id="tm-result" hidden>
      <div id="tm-summary"></div><canvas id="tm-plot" width="580" height="220" title="加权平均残差 / 倾角"></canvas>
      <div id="tm-table"></div>
      <div class="al-row"><button id="tm-resolve" class="small">重新求解</button><button id="tm-save" class="small">保存对齐叠层…</button><button id="tm-export" class="small">导出 .xf/.tlt…</button></div>
      <p class="dim">标记视图仅提示；勾选排除后重新求解。导出按倾角排序，.views 为原始层号（从 0 开始）。</p>
      <div class="al-opts" id="tm-recon-fields">
        <label>重构方法<select id="tm-method"><option value="wbp">WBP</option><option value="sirt">SIRT</option></select></label>
        <label>SIRT 迭代次数<input id="tm-iters" type="number" min="1" max="1000" value="15"></label>
        <label>厚度 Z（输出 px）<input id="tm-z" type="number" min="1" max="4096" value="128"></label>
        <label>输出 binning<select id="tm-outbin"><option>1</option><option>2</option><option selected>4</option><option>8</option></select></label>
        <label>WBP 滤波<select id="tm-filter"><option value="ramp">Ramp</option><option value="cosine">Cosine</option><option value="hamming">Hamming</option><option value="sirtLike">SIRT-like</option></select></label>
        <label>Z 偏移（输出 px）<input id="tm-offset" type="number" value="0"></label>
        <label class="al-check al-wide"><input id="tm-positive" type="checkbox">SIRT 非负约束</label>
      </div>
      <button id="tm-recon" class="small primary">重构并打开…</button>
    </div>`;
  W.sections[2].append(panel);
  const status=W.status;
  const recon=W.sections[3];
  recon.innerHTML='<div class="al-head"><b>三维重构</b></div><p id="cryo-recon-geometry" class="dim">先在③完成倾转对齐，再重构。</p>';
  recon.append($("tm-recon-fields"),$("tm-recon"));
  W.register(2,{enter:()=>{if(A.token!==S.token&&!A.busy)reset();}});
  async function reset() {
    A.token = S.token; A.res = null; $("cryo-recon-geometry").textContent="先在③完成倾转对齐，再重构。"; $("tm-result").hidden = true;
    const token = S.token;
    try {
      const m = await invoke("tomo_geometry", { token });
      if (token !== S.token) return;
      $("tm-axis").value = m.axis ?? 90;
      status(m.angles.length ? `已读取 ${m.angles.length} 个倾角` : "未找到倾角文件，请输入起始角 / 步长");
      if (!m.angles.length) $("tm-source").value = "manual";
    } catch (e) { if (token === S.token) status(String(e)); }
  }
  async function job(fn) {
    if (A.busy) return;
    A.busy = true; const token = S.token;
    $("tm-cancel").hidden = false;
    for (const id of ["tm-run", "tm-resolve", "tm-save", "tm-export", "tm-recon"]) $(id).disabled = true;
    let un;
    try {
      un = await T.event.listen("tomo-progress", (e) => { if (token === S.token) status(`计算中 ${e.payload.done} / ${e.payload.total}`); });
      await fn(token);
    } catch (e) { if (token === S.token) status(`失败：${e}`); }
    finally {
      un?.(); A.busy = false; $("tm-cancel").hidden = true;
      for (const id of ["tm-run", "tm-resolve", "tm-save", "tm-export", "tm-recon"]) $(id).disabled = false;
    }
  }
  $("tm-cancel").onclick = () => invoke("tomo_cancel");
  async function estimate(previous) {
    return job(async (token) => {
      const manual = $("tm-source").value === "manual";
      const excluded = previous ? previous.views.filter(v => v.excluded).map(v => v.fileIndex) : [];
      const c = previous ? A.c : fftChannel(), t = previous ? A.t : S.t;
      const req = { token, c, t, angles: [], start: manual ? +$("tm-start").value : null, step: manual ? +$("tm-step").value : null,
        axis: +$("tm-axis").value, bin: previous?.bin ?? +$("tm-bin").value, sweep: $("tm-sweep").checked, patch: null, excluded, magGroup: 0, previous };
      const res = await invoke("tomo_align", { req });
      if (token !== S.token) return;
      A.res = res; A.token = token; A.c = c; A.t = t;
      $("tm-axis").value = res.axis.toFixed(2); show(); status(`完成 · ${(res.ms / 1000).toFixed(1)} s`);
    });
  }
  function show() {
    const r = A.res; $("tm-result").hidden = false;
    $("tm-summary").textContent = `加权平均残差 ${r.residual.toFixed(3)} px · ${r.quality} · 轴 ${r.axis.toFixed(2)}° · ${r.patch.nx}×${r.patch.ny}/${r.patch.size}px（bin ${r.bin}）`;
    $("cryo-recon-geometry").textContent=$("tm-summary").textContent;
    $("tm-outbin").value=r.bin;
    const table = document.createElement("table");
    table.innerHTML = "<thead><tr><th>排除</th><th>原层</th><th>倾角 °</th><th>残差 px</th><th>CC</th><th>轮廓</th></tr></thead>";
    const body = document.createElement("tbody");
    for (const v of r.views) {
      const row = document.createElement("tr"); row.className = v.flagged ? "flagged" : "";
      const td = document.createElement("td"), cb = document.createElement("input"); cb.type = "checkbox"; cb.checked = v.excluded;
      cb.onclick = e => e.stopPropagation(); cb.onchange = () => { v.excluded = cb.checked; A.dirty = true; status("排除已更改，请重新求解"); };
      td.append(cb); row.append(td);
      for (const x of [v.fileIndex + 1, v.angle.toFixed(1), v.residual.toFixed(3), v.cc.toFixed(3), v.contours]) { const td = document.createElement("td"); td.textContent = x; row.append(td); }
      row.onclick = () => setZ(v.fileIndex); body.append(row);
    }
    table.append(body); $("tm-table").replaceChildren(table); A.dirty = false;
    const cv = $("tm-plot"), ctx = cv.getContext("2d"); ctx.clearRect(0, 0, cv.width, cv.height);
    const min = Math.min(...r.views.map(v => v.angle)), max = Math.max(...r.views.map(v => v.angle));
    const top = Math.max(1, ...r.views.map(v => v.residual));
    ctx.fillStyle = "#888"; ctx.font = "20px sans-serif"; ctx.fillText(`${min.toFixed(0)}°`, 8, 215); ctx.fillText(`${max.toFixed(0)}°`, 510, 215); ctx.fillText(`${top.toFixed(1)} px`, 8, 22);
    for (const v of r.views) { ctx.beginPath(); ctx.fillStyle = v.flagged ? "#e71316" : "#4488bb"; ctx.arc(25 + (v.angle - min) / Math.max(1, max - min) * 530, 190 - v.residual / top * 150, 4, 0, Math.PI * 2); ctx.fill(); }
  }
  $("tm-run").onclick = () => estimate(null);
  $("tm-resolve").onclick = () => A.res && estimate(A.res);
  async function pathFor(suffix) {
    const stem = S.info.name.replace(/\.[^.]+$/, "");
    const path = await invoke("pick_save_path", { dir: saveDir(), name: `${stem}_${suffix}.mrc`, filter: "MRC", exts: ["mrc"] });
    if (path) setSaveDir(dirname(path)); return path;
  }
  async function save(transformsOnly) {
    if (!A.res || A.token !== S.token || A.busy) return;
    if (A.dirty) return status("请先重新求解");
    const path = await pathFor(transformsOnly ? "geometry" : "aligned"); if (!path || A.token !== S.token) return;
    await job(async () => {
      await invoke("tomo_save_aligned", { req: { token: A.token, c: A.c, t: A.t, alignment: A.res, bin: +$("tm-outbin").value, path, transformsOnly } }); status(transformsOnly ? `已导出 .xf / .tlt / .views：${dirname(path)}` : `已保存：${path}`);
    });
  }
  $("tm-save").onclick = () => save(false); $("tm-export").onclick = () => save(true);
  $("tm-recon").onclick = async () => {
    if (!A.res || A.token !== S.token || A.busy) return;
    if (A.dirty) return status("请先重新求解");
    const path = await pathFor($("tm-method").value); if (!path || A.token !== S.token) return;
    await job(async (token) => {
      const req = { token, c: A.c, t: A.t, alignment: A.res, bin: +$("tm-outbin").value, paths: [path], additionalTokens: [],
        params: { method: $("tm-method").value, thickness: +$("tm-z").value, iterations: +$("tm-iters").value, lambda: 1, nonNegative: $("tm-positive").checked, zOffset: +$("tm-offset").value, filter: $("tm-filter").value, backend: "auto" } };
      const r = await invoke("tomo_reconstruct", { req });
      if (token !== S.token) return;
      status(`完成 · ${r.backend.join(", ")} · ${(r.ms / 1000).toFixed(1)} s`); await openPath(r.paths[0]); W.select(4);
    });
  };
  setInterval(()=>{
    if(!A.busy&&A.token!==S.token&&!W.panel.hidden&&W.active()===2)reset();
    $("tm-recon").disabled=A.busy||!A.res||A.token!==S.token;
  },500);
})();
