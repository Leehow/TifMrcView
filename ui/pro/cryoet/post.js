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
    <p class="dim">缺失楔补偿：补回倾转角之外缺失的信息，减轻沿电子束方向的拉长。参数自动选择。</p>
    <p id="pp-wedge-note" class="dim"></p>
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
      ? "有奇偶两半：补楔同时去噪，不需要另做 Noise2Noise。"
      : "";
  }
  $("pp-source").onchange = note;

  $("pp-n2n").onclick = async () => {
    const r = await W.step("Noise2Noise", "tomo_denoise", { source: $("pp-source").value, steps: +$("pp-steps").value });
    if (!r) return;
    W.status(`去噪完成 · ${(r.ms / 1000).toFixed(1)} s`);
    await openPath(r.paths[0]);
  };

  $("pp-wedge").onclick = async () => {
    const r = await W.step("缺失楔补偿", "tomo_wedge", { source: $("pp-source").value });
    if (!r) return;
    W.status(`缺失楔补偿完成 · ${(r.ms / 1000).toFixed(1)} s`);
    await openPath(r.paths[0]);
  };

  W.register(4, { dataset: show, enter: () => show(W.dataset) });
})();
