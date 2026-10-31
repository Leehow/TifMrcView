"use strict";
// Step ②: CTF of every tilt (tomo::ctf): fitted on a strip along the tilt
// axis, where the defocus gradient of the tilted specimen stays small, plus
// the direction of that gradient. Used by step ④ for phase flipping.
(() => {
  const W = window.TifMrcViewPro.cryoet;
  const host = W.sections[1];
  host.innerHTML = `
    <div class="al-head"><b>倾转序列 CTF</b></div>
    <p class="dim">每张倾转图只在沿倾转轴的窄条上拟合（条宽让离焦变化不超过约 1500 Å），得到轴上的离焦；再在最大倾角两侧各拟合一次，判断离焦梯度的方向。对齐后再做更准（用对齐得到的轴角）。</p>
    <div class="al-opts">
      <label>电压 kV<input id="ct-kv" type="number" min="60" max="400" value="300"></label>
      <label>Cs mm<input id="ct-cs" type="number" min="0" max="10" step="0.01" value="2.7"></label>
      <label>振幅衬度<input id="ct-amp" type="number" min="0" max="0.5" step="0.01" value="0.07"></label>
      <label>像素 Å<input id="ct-pixel" type="number" min="0" step="0.001" placeholder="叠层 / mdoc"></label>
      <label>离焦范围 µm<span class="ct-range"><input id="ct-dfmin" type="number" min="0" step="0.1" value="0.5">–<input id="ct-dfmax" type="number" min="0" step="0.1" value="8"></span></label>
      <label>拟合分辨率 Å<span class="ct-range"><input id="ct-lo" type="number" min="5" value="30">–<input id="ct-hi" type="number" min="1" placeholder="自动"></span></label>
      <label>谱尺寸<select id="ct-n"><option>256</option><option selected>512</option><option>1024</option></select></label>
    </div>
    <div class="al-row"><button id="ct-run" class="small primary">拟合全部倾角</button></div>
    <p id="ct-summary" class="dim"></p>
    <canvas id="ct-plot" width="580" height="200" hidden title="每张倾转图的离焦（µm，蓝）与拟合到的分辨率（Å，橙）"></canvas>`;

  function show(d) {
    if (!d) return;
    $("ct-kv").value = d.optics.kv;
    $("ct-cs").value = d.optics.csMm;
    $("ct-amp").value = d.optics.amplitude;
    if (d.optics.pixelA) $("ct-pixel").value = d.optics.pixelA;
    const c = d.ctf;
    $("ct-plot").hidden = !c;
    if (!c) {
      $("ct-summary").textContent = "";
      return;
    }
    const df = c.tilts.map((t) => (t.df1 + t.df2) / 2e4);
    const sorted = df.slice().sort((a, b) => a - b);
    const gradient = { 1: "离焦随 +x 增大", "-1": "离焦随 +x 减小", 0: "未能判定（相位翻转只用轴上离焦）" }[c.gradient];
    $("ct-summary").textContent = `离焦中位 ${sorted[sorted.length >> 1].toFixed(2)} µm（${sorted[0].toFixed(2)}–${sorted[sorted.length - 1].toFixed(2)}）· 梯度方向：${gradient}`;
    plot(d, df);
  }

  function plot(d, df) {
    const cv = $("ct-plot"), ctx = cv.getContext("2d");
    ctx.clearRect(0, 0, cv.width, cv.height);
    const angles = d.ctf.tilts.map((t) => d.angles[t.z]);
    const min = Math.min(...angles), max = Math.max(...angles);
    const x = (a) => 30 + (a - min) / Math.max(1, max - min) * (cv.width - 60);
    const top = Math.max(...df) * 1.1, res = Math.max(...d.ctf.tilts.map((t) => t.fitRes));
    ctx.font = "16px sans-serif";
    ctx.fillStyle = "#4488bb";
    ctx.fillText(`${top.toFixed(1)} µm`, 4, 18);
    ctx.fillStyle = "#d9822b";
    ctx.fillText(`${res.toFixed(0)} Å`, cv.width - 55, 18);
    d.ctf.tilts.forEach((t, i) => {
      ctx.fillStyle = "#4488bb";
      ctx.beginPath();
      ctx.arc(x(angles[i]), cv.height - 20 - df[i] / top * (cv.height - 45), 4, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = "#d9822b";
      ctx.fillRect(x(angles[i]) - 2, cv.height - 20 - t.fitRes / res * (cv.height - 45) - 2, 4, 4);
    });
  }

  $("ct-run").onclick = async () => {
    const d = W.dataset;
    if (!d) return W.status("请先载入倾转序列");
    const pixel = +$("ct-pixel").value || d.optics.pixelA;
    if (!pixel) return W.status("请输入像素大小（Å）");
    const params = {
      voltageKv: +$("ct-kv").value,
      csMm: +$("ct-cs").value,
      ampContrast: +$("ct-amp").value,
      pixelA: pixel,
      resLowA: +$("ct-lo").value,
      resHighA: +$("ct-hi").value || Math.max(5, pixel * 2.5),
      dfMinA: +$("ct-dfmin").value * 1e4,
      dfMaxA: +$("ct-dfmax").value * 1e4,
      maxAstigA: 2000,
      phase: false,
    };
    if (d.optics.pixelA !== pixel) {
      d.optics.pixelA = pixel;
      await invoke("tomo_dataset_save", { dataset: d });
    }
    const r = await W.step("CTF 拟合", "tomo_ctf", { n: +$("ct-n").value, params });
    if (r) W.status(`CTF 完成：${r.tilts.length} 个倾角`);
  };

  W.register(1, { dataset: show });
})();
