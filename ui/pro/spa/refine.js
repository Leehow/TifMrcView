"use strict";
// Step ⑥: gold-standard 3D refinement with the FSC plot (unmasked and
// masked, corrected for the mask by phase randomisation).
(() => {
  const W = window.TifMrcViewPro.spa;
  const { fmt, name } = W;
  W.sections[5].innerHTML = `
    <div class="al-head"><b>金标准三维精修</b></div>
    <p class="dim">颗粒分成互不相见的两半，各自对齐到自己的图（先全局，后在上次取向附近细化），两半图之间的 FSC 决定下一轮的分辨率和维纳滤波；参考图做溶剂平整，最后自动掩膜、算 B 因子并锐化。</p>
    <div class="al-opts">
      <label>对称性<input id="spa-sym" type="text" value="C1"></label>
      <label>起始低通 Å<input id="spa-resi" type="number" min="5" value="30"></label>
      <label>最多迭代<input id="spa-it" type="number" min="2" max="50" value="12"></label>
      <label>全局搜索步长 °<input id="spa-step" type="number" min="2" max="20" step="0.5" value="7.5"></label>
      <div class="al-wide al-row"><button id="spa-ref" class="small">参考图（可选）…</button><span id="spa-ref-name" class="dim">用初始模型</span><button id="spa-ref-clear" class="small">清除</button></div>
    </div>
    <div class="al-row"><button id="spa-run-refine" class="small primary">三维精修</button><button id="spa-open-map" class="small" disabled>打开锐化图</button><button id="spa-open-raw" class="small" disabled>打开未锐化图</button></div>
    <p id="spa-ref-summary" class="dim"></p>
    <canvas id="spa-fsc" width="600" height="220" hidden title="FSC：灰 = 未掩膜，蓝 = 掩膜；虚线 0.143"></canvas>
    <p id="spa-files" class="dim"></p>`;

  function fsc(r) {
    const cv = $("spa-fsc"), ctx = cv.getContext("2d"), w = cv.width, h = cv.height;
    cv.hidden = false;
    ctx.clearRect(0, 0, w, h);
    const n = r.fsc.length, x = (i) => 40 + i / Math.max(1, n - 1) * (w - 55), y = (v) => 15 + (1 - Math.max(-0.1, Math.min(1, v))) / 1.1 * (h - 45);
    ctx.strokeStyle = "#ccc";
    ctx.setLineDash([4, 4]);
    ctx.beginPath(); ctx.moveTo(x(0), y(0.143)); ctx.lineTo(x(n - 1), y(0.143)); ctx.stroke();
    ctx.setLineDash([]);
    for (const [curve, color] of [[r.fsc, "#999"], [r.fscMasked, "#0969da"]]) {
      ctx.strokeStyle = color;
      ctx.lineWidth = 2;
      ctx.beginPath();
      curve.forEach((v, i) => (i ? ctx.lineTo(x(i), y(v)) : ctx.moveTo(x(i), y(v))));
      ctx.stroke();
    }
    ctx.fillStyle = "#888";
    ctx.font = "13px sans-serif";
    ctx.fillText("1", 25, y(1) + 4);
    ctx.fillText("0.143", 2, y(0.143) + 4);
    for (const s of [4, 2, 4 / 3, 1]) {
      const i = (n - 1) / s;
      ctx.fillText(`${(r.boxPx * r.pixelA / i).toFixed(1)} Å`, x(i) - 18, h - 8);
    }
  }

  W.register(5, {
    show(p) {
      const r = p.refine;
      $("spa-open-map").disabled = $("spa-open-raw").disabled = !r;
      if (r) {
        $("spa-ref-summary").textContent = `分辨率 ${fmt(r.res)} Å（FSC 0.143）· 掩膜后 ${fmt(r.resMasked)} Å · B 因子 ${fmt(r.bfactor, 0)} Å² · ${r.particles} 个颗粒 · ${r.history.length} 轮 · ${r.backend}`;
        $("spa-files").textContent = `写出：${[r.sharpened, r.map, r.half1, r.half2, r.mask, r.star].map(name).join("、")}`;
        fsc(r);
      } else {
        $("spa-ref-summary").textContent = $("spa-files").textContent = "";
        $("spa-fsc").hidden = true;
      }
    },
  });

  let reference = null;
  $("spa-ref").onclick = async () => {
    const p = await invoke("motion_pick", { dir: W.project ? W.project.dir : saveDir(), kind: "gain" });
    if (p) { reference = p; $("spa-ref-name").textContent = name(p); }
  };
  $("spa-ref-clear").onclick = () => { reference = null; $("spa-ref-name").textContent = "用初始模型"; };
  $("spa-run-refine").onclick = () => {
    W.run("三维精修", "refine", { reference, params: { symmetry: $("spa-sym").value || "C1", resInit: +$("spa-resi").value, maxIterations: +$("spa-it").value, globalIterations: 2, globalStep: +$("spa-step").value, seed: 11 } });
  };
  $("spa-open-map").onclick = () => W.project?.refine && openPath(W.project.refine.sharpened, true);
  $("spa-open-raw").onclick = () => W.project?.refine && openPath(W.project.refine.map, true);
})();
