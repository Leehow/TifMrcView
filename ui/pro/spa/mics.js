"use strict";
// Step ①: the micrographs, the particle diameter and pixel size, and beam-
// induced motion correction when the inputs are movies (the cryo-ET
// package's motion correction, dose-weighted sums in tifmrcview-spa/motion).
(() => {
  const W = window.TifMrcViewPro.spa;
  W.sections[0].innerHTML = `
    <div class="al-head"><b>显微图与运动校正</b></div>
    <p id="spa-mics" class="dim">选择一个文件夹：里面的 MRC / TIFF 显微图，或 EER / TIFF / MRC 电影（多帧）。工程保存在它的 tifmrcview-spa 子文件夹里，可以随时关掉再接着做。</p>
    <div class="al-opts">
      <label>颗粒直径 Å<input id="spa-diam" type="number" min="20" max="2000" value="150"></label>
      <label>像素 Å<input id="spa-pixel" type="number" min="0.1" step="0.001"></label>
    </div>
    <div id="spa-motion" hidden>
      <p class="dim">输入是电影：先做束流诱导运动校正（Cryo-ET 工作区同一套算法：全局 + 局部形变，剂量加权），和显微图写到 tifmrcview-spa/motion。</p>
      <div class="al-opts">
        <label>每帧剂量 e⁻/Å²<input id="spa-dose" type="number" min="0" step="0.01" placeholder="不加权"></label>
        <label>每帧合并 fraction 数<input id="spa-group" type="number" min="0" max="256" value="0" title="0：EER 自动分成约 40 帧"></label>
        <label class="al-check"><input id="spa-local" type="checkbox" checked>拟合局部形变</label>
      </div>
      <div class="al-row"><button id="spa-run-motion" class="small primary">运动校正全部电影</button></div>
    </div>
    <div class="al-row"><button id="spa-save" class="small">保存设置</button></div>`;

  W.register(0, {
    show(p) {
      $("spa-mics").textContent = `${p.mics.length} 张${p.movies ? "电影" : "显微图"}（${p.mics[0]?.width}×${p.mics[0]?.height}）· 工程：${p.dir}`;
      $("spa-diam").value = p.diameterA;
      $("spa-pixel").value = p.optics.pixelA;
      $("spa-motion").hidden = !p.movies;
    },
  });

  $("spa-save").onclick = async () => { try { await W.settings(); W.status("已保存"); } catch (e) { W.status(`${e}`); } };

  $("spa-run-motion").onclick = () => {
    const dose = +$("spa-dose").value;
    W.run("运动校正", "motion", {
      group: +$("spa-group").value, upsample: 1, frameAxis: "auto", workingSize: 1024, local: $("spa-local").checked, patches: 5,
      doseWeighted: dose > 0, unweighted: false, evenOdd: false, pixelA: +$("spa-pixel").value || null, voltageKv: +$("spa-kv").value || 300,
      voltageAuto: false, dosePerFrame: dose > 0 ? dose : null, totalDose: null, preExposure: 0, gainPath: null, gainRotate: 0,
      gainFlipX: false, gainFlipY: false, backend: "auto", benchmark: false, diagnostics: false,
    });
  };
})();
