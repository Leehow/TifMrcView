"use strict";
// Step ⑤: Noise2Noise denoising (on the even/odd half tomograms when there
// are some, else on neighbouring sections) and missing-wedge correction
// (IsoNet method, crate::wedge) of the dataset's tomograms.
(() => {
  const W = window.TifMrcViewPro.cryoet;
  const host = W.sections[4];
  host.innerHTML = `
    <div class="al-head"><b>去噪 / 去伪影</b></div>
    <label class="al-wide">输入断层<select id="pp-source"></select></label>
    <p id="pp-n2n-note" class="dim"></p>
    <div class="al-opts">
      <label>Noise2Noise 训练步数<select id="pp-steps"><option>500</option><option selected>1000</option><option>2000</option><option>4000</option></select></label>
    </div>
    <div class="al-row"><button id="pp-n2n" class="small primary">Noise2Noise 去噪并打开</button></div>
    <p class="dim">缺失楔补偿（IsoNet 方法）：在断层里切小立方体，旋转后让网络学会补回倾转角之外缺失的频率，迭代多轮。倾角范围取对齐里实际使用的倾角。</p>
    <p id="pp-wedge-note" class="dim"></p>
    <div class="al-opts">
      <label>迭代轮数<input id="pp-iters" type="number" min="1" max="30" value="10"></label>
      <label>每轮步数<input id="pp-wsteps" type="number" min="10" max="2000" value="200"></label>
      <label>立方体数<input id="pp-cubes" type="number" min="8" max="1000" value="120"></label>
      <label>立方体边长<select id="pp-cube"><option>32</option><option title="仅奇偶两半模式">48</option><option selected>64</option></select></label>
      <label id="pp-equi-row" hidden title="网络自己的结果旋转后再切楔，必须还原成另一半的旋转结果（ICECREAM 方法）。留出倾转角验证里补楔更准，训练慢约 4 倍。"><input id="pp-equi" type="checkbox" checked>等变约束（更准，慢约 4 倍）</label>
      <label>去噪强度<input id="pp-noise" type="number" min="0" max="1" step="0.05" value="0" title="最后几轮给输入加噪，0 为不去噪"></label>
    </div>
    <div class="al-row"><button id="pp-wedge" class="small primary">缺失楔补偿并打开</button></div>`;

  function show(d) {
    const sel = $("pp-source");
    const keep = sel.value;
    sel.replaceChildren();
    const usable = (d?.tomograms ?? []).filter((t) => t.kind !== "even" && t.kind !== "odd");
    for (const t of usable.slice().reverse()) {
      const o = document.createElement("option");
      o.value = t.path;
      o.textContent = W.name(t.path);
      sel.append(o);
    }
    if (usable.some((t) => t.path === keep)) sel.value = keep;
    note();
    $("pp-n2n").disabled = $("pp-wedge").disabled = !usable.length;
  }

  function note() {
    const t = W.dataset?.tomograms.find((t) => t.path === $("pp-source").value);
    $("pp-n2n-note").textContent = !t
      ? "先在④完成重构。"
      : t.halves
        ? "Noise2Noise：用这次重构的奇偶两半断层互为训练目标（信号相同、噪声独立），两半都去噪后取平均。"
        : "这个断层没有奇偶两半（运动校正时写出奇偶帧并在④勾选重构两半即可），改用相邻切片互为训练目标。";
    $("pp-wedge-note").textContent = t?.halves
      ? "有奇偶两半：缺失楔补偿同时去噪，一个网络把一半旋转后补回另一半（DeepDeWedge 方法），不需要另做 Noise2Noise；「去噪强度」不起作用。立方体 32 快约 10 倍。"
      : "";
    $("pp-equi-row").hidden = !t?.halves;
    if (t?.halves && !$("pp-cube").dataset.touched) $("pp-cube").value = "32";
  }
  $("pp-source").onchange = note;
  $("pp-cube").onchange = () => ($("pp-cube").dataset.touched = "1");

  $("pp-n2n").onclick = async () => {
    const r = await W.step("Noise2Noise", "tomo_denoise", { source: $("pp-source").value, steps: +$("pp-steps").value });
    if (!r) return;
    W.status(`去噪完成 · ${(r.ms / 1000).toFixed(1)} s`);
    await openPath(r.paths[0]);
  };

  $("pp-wedge").onclick = async () => {
    const cfg = {
      iterations: +$("pp-iters").value,
      steps: +$("pp-wsteps").value,
      count: +$("pp-cubes").value,
      cube: +$("pp-cube").value,
      noise: +$("pp-noise").value,
      equivariant: $("pp-equi").checked && !$("pp-equi-row").hidden ? 2 : 0,
    };
    const r = await W.step("缺失楔补偿", "tomo_wedge", { source: $("pp-source").value, cfg });
    if (!r) return;
    W.status(`缺失楔补偿完成 · ${(r.ms / 1000).toFixed(1)} s`);
    await openPath(r.paths[0]);
  };

  W.register(4, { dataset: show, enter: () => show(W.dataset) });
})();
