"use strict";
// Step ⑤: the ab initio model (stochastic-gradient-like reconstruction from
// random orientations), with a hand flip.
(() => {
  const W = window.TifMrcViewPro.spa;
  W.sections[4].innerHTML = `
    <div class="al-head"><b>从头建立初始模型</b></div>
    <p class="dim">颗粒先取随机取向，再一批一批地全局搜索取向并插入一个逐渐遗忘旧批次的重构，分辨率逐步提高（随机梯度式从头建模）。投影无法分辨手性：若结构已知，可翻转手性。</p>
    <div class="al-opts">
      <label>对称性<input id="spa-sym0" type="text" value="C1" title="C1、Cn、Dn、T、O、I"></label>
      <label>迭代<input id="spa-it0" type="number" min="10" max="1000" value="80"></label>
      <label>每批颗粒<input id="spa-batch" type="number" min="50" max="5000" value="300"></label>
      <label>最终分辨率 Å<input id="spa-res0" type="number" min="4" value="12"></label>
    </div>
    <div class="al-row"><button id="spa-run-init" class="small primary">建立初始模型</button><button id="spa-open-init" class="small" disabled>打开</button><button id="spa-flip" class="small" disabled>翻转手性</button></div>
    <p id="spa-init-summary" class="dim"></p>`;

  W.register(4, {
    show(p) {
      $("spa-open-init").disabled = $("spa-flip").disabled = !p.initial;
      $("spa-init-summary").textContent = p.initial ? `初始模型：${W.name(p.initial.path)} · ${p.initial.boxPx}³ · ${W.fmt(p.initial.pixelA, 2)} Å/px` : "";
    },
  });

  $("spa-run-init").onclick = () => {
    // The refinement uses the same symmetry unless it was changed there.
    if ($("spa-sym").value === "C1") $("spa-sym").value = $("spa-sym0").value || "C1";
    W.run("初始模型", "initial", { symmetry: $("spa-sym0").value || "C1", iterations: +$("spa-it0").value, batch: +$("spa-batch").value, resStart: 35, resEnd: +$("spa-res0").value, boxPx: 64, seed: 7 });
  };
  $("spa-open-init").onclick = () => W.project?.initial && openPath(W.project.initial.path, true);
  $("spa-flip").onclick = () => W.run("翻转手性", "flip", null);
})();
