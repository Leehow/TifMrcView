"use strict";
// Step ③: tilt-series alignment (tomo::align). Patch tracking and the
// projection model run in the backend; the result is kept in the dataset.
// Views can be excluded and the model solved again from the same contours.
(() => {
  const W = window.TifMrcViewPro.cryoet;
  const host = W.sections[2];
  host.innerHTML = `
    <div class="al-head"><b>倾转对齐</b></div>
    <p class="dim">在每张倾转图上跟踪图块（patch tracking），再用投影模型求解轴角、平移，以及分组的面内旋转、倾角修正和放大率。残差为加权平均，单位为原始像素。</p>
    <div class="al-opts">
      <div id="ta-manual" class="al-wide al-row" hidden>
        <label>起始角 °<input id="ta-start" type="number" value="-60" step="0.1"></label>
        <label>步长 °<input id="ta-step" type="number" value="3" step="0.1"></label>
      </div>
      <label>倾转轴角 °<input id="ta-axis" type="number" step="0.1" placeholder="自动" title="从图像 X 轴逆时针量；留空则用 mdoc 的值（两种换算都试），没有 mdoc 时整圈粗扫"></label>
      <label>计算 binning<select id="ta-bin"><option value="0" selected>自动（短边约 1024）</option><option>1</option><option>2</option><option>4</option><option>8</option></select></label>
      <label class="al-check al-wide"><input id="ta-sweep" type="checkbox" checked>自动扫描轴角和图块配置</label>
      <label class="al-check al-wide"><input id="ta-groups" type="checkbox" checked>分组求解面内旋转、倾角修正、放大率</label>
    </div>
    <div class="al-row"><button id="ta-run" class="small primary">对齐</button></div>
    <div id="ta-result" hidden>
      <div id="ta-summary"></div>
      <canvas id="ta-plot" width="580" height="200" title="每张倾转图的加权残差（原始像素）；红点为可疑视图"></canvas>
      <div id="ta-table"></div>
      <div class="al-row"><button id="ta-resolve" class="small" title="用已有轮廓、去掉勾选的视图重新求解">重新求解</button><button id="ta-export" class="small" title="IMOD 的 .xf / .tlt 和原始层号 .views">导出 .xf / .tlt…</button></div>
      <p class="dim">红色为可疑视图（残差或跟踪明显差于中位数），只作提示；勾选「排除」后重新求解。</p>
    </div>`;

  const A = { dirty: false };

  function show(d) {
    const a = d?.alignment;
    $("ta-manual").hidden = !!d?.angles.length;
    if (!a) {
      $("ta-result").hidden = true;
      $("ta-axis").placeholder = d?.axisHint != null ? `mdoc：${d.axisHint.toFixed(1)}` : "自动";
      return;
    }
    $("ta-result").hidden = false;
    $("ta-axis").value = a.axis.toFixed(2);
    $("ta-summary").textContent = `加权残差 ${a.residual.toFixed(2)} px（${a.quality}）· 轴 ${a.axis.toFixed(2)}° · 图块 ${a.patch.nx}×${a.patch.ny} / ${a.patch.size} px · bin ${a.bin} · ${(a.ms / 1000).toFixed(1)} s`;
    const table = document.createElement("table");
    table.innerHTML = "<thead><tr><th>排除</th><th>层</th><th>倾角 °</th><th>修正后 °</th><th>旋转 °</th><th>放大率</th><th>残差 px</th><th>轮廓</th></tr></thead>";
    const body = document.createElement("tbody");
    for (const v of a.views) {
      const row = document.createElement("tr");
      if (v.flagged) row.className = "flagged";
      const td = document.createElement("td");
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.checked = v.excluded;
      cb.onclick = (e) => e.stopPropagation();
      cb.onchange = () => { v.excluded = cb.checked; A.dirty = true; W.status("排除已更改，请重新求解"); };
      td.append(cb);
      row.append(td);
      const refined = v.tilt || v.angle;
      for (const x of [v.fileIndex + 1, v.angle.toFixed(1), refined.toFixed(2), v.rotation.toFixed(2), v.mag.toFixed(3), v.residual.toFixed(2), v.contours]) {
        const c = document.createElement("td");
        c.textContent = x;
        row.append(c);
      }
      // Jump to that view when the stack is the open image.
      row.onclick = () => { if (S.info?.path === d.stack) setZ(v.fileIndex); };
      body.append(row);
    }
    table.append(body);
    $("ta-table").replaceChildren(table);
    plot(a);
    A.dirty = false;
  }

  function plot(a) {
    const cv = $("ta-plot"), ctx = cv.getContext("2d");
    ctx.clearRect(0, 0, cv.width, cv.height);
    const angles = a.views.map((v) => v.angle);
    const min = Math.min(...angles), max = Math.max(...angles);
    const top = Math.max(1, ...a.views.map((v) => v.residual));
    const x = (t) => 30 + (t - min) / Math.max(1, max - min) * (cv.width - 50);
    const y = (r) => cv.height - 25 - r / top * (cv.height - 50);
    ctx.fillStyle = "#888";
    ctx.font = "16px sans-serif";
    ctx.fillText(`${min.toFixed(0)}°`, 10, cv.height - 5);
    ctx.fillText(`${max.toFixed(0)}°`, cv.width - 45, cv.height - 5);
    ctx.fillText(`${top.toFixed(1)} px`, 4, 18);
    for (const v of a.views) {
      ctx.beginPath();
      ctx.fillStyle = v.excluded ? "#bbb" : v.flagged ? "#e71316" : "#4488bb";
      ctx.arc(x(v.angle), y(v.residual), 4, 0, Math.PI * 2);
      ctx.fill();
    }
  }

  async function align(previous) {
    const d = W.dataset;
    if (!d) return W.status("请先载入倾转序列（上方「使用当前叠层」）");
    const manual = !d.angles.length;
    const axis = $("ta-axis").value === "" ? null : +$("ta-axis").value;
    const n = d.alignment?.views.length || 0;
    const req = {
      token: 0,
      angles: [],
      start: manual ? +$("ta-start").value : null,
      step: manual ? +$("ta-step").value : null,
      axis: previous ? previous.axis : axis,
      bin: previous ? previous.bin : +$("ta-bin").value,
      sweep: !previous && $("ta-sweep").checked,
      patch: null,
      excluded: previous ? previous.views.filter((v) => v.excluded).map((v) => v.fileIndex) : [],
      groups: $("ta-groups").checked ? null : { rotation: 0, tilt: 0, mag: 0 },
      previous,
    };
    const next = await W.run("倾转对齐", () => invoke("tomo_align", { stack: d.stack, req }));
    if (!next) return;
    W.setDataset(next);
    W.status(`对齐完成 · 加权残差 ${next.alignment.residual.toFixed(2)} px` + (n && previous ? "（已重新求解）" : ""));
  }

  $("ta-run").onclick = () => align(null);
  $("ta-resolve").onclick = () => W.dataset?.alignment && align(W.dataset.alignment);
  $("ta-export").onclick = async () => {
    const d = W.dataset;
    if (!d?.alignment) return;
    if (A.dirty) return W.status("请先重新求解");
    if (S.info?.path !== d.stack) return W.status("导出需要先打开这个倾转序列");
    const stem = W.name(d.stack).replace(/\.[^.]+$/, "");
    const path = await invoke("pick_save_path", { dir: saveDir(), name: `${stem}_ali.xf`, filter: "IMOD .xf", exts: ["xf"] });
    if (!path) return;
    setSaveDir(dirname(path));
    const token = S.token;
    await W.run("导出", () => invoke("tomo_save_aligned", { req: { token, c: 0, t: 0, alignment: d.alignment, bin: d.alignment.bin, path, transformsOnly: true } }));
    W.status(`已导出 .xf / .tlt / .views：${dirname(path)}`);
  };

  W.register(2, { dataset: show });
})();
