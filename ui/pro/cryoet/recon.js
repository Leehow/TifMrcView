"use strict";
// Step ④: reconstruction (tomo::pipeline::reconstruct) of the full tilt
// series and, when motion correction made them, its even/odd halves, with
// the dataset's alignment and optional phase flipping from step ②.
(() => {
  const W = window.TifMrcViewPro.cryoet;
  const host = W.sections[3];
  host.innerHTML = `
    <div class="al-head"><b>三维重构</b></div>
    <p id="rc-geometry" class="dim">先在③完成倾转对齐。</p>
    <div class="al-opts">
      <label>方法<select id="rc-method"><option value="wbp">WBP（加权反投影）</option><option value="sirt">SIRT（迭代）</option></select></label>
      <label>SIRT 迭代次数<input id="rc-iters" type="number" min="1" max="1000" value="15"></label>
      <label>WBP 滤波<select id="rc-filter"><option value="ramp">Ramp</option><option value="cosine">Cosine</option><option value="hamming">Hamming</option></select></label>
      <label>输出 binning<select id="rc-bin"><option>1</option><option>2</option><option selected>4</option><option>8</option></select></label>
      <label>厚度（输出像素）<input id="rc-z" type="number" min="1" max="8192" value="256"></label>
      <label>Z 偏移（输出像素）<input id="rc-offset" type="number" value="0"></label>
      <label class="al-check al-wide"><input id="rc-flip" type="checkbox">相位翻转（需要② CTF）</label>
      <label class="al-check al-wide"><input id="rc-halves" type="checkbox">同时重构奇偶两半（供⑤ Noise2Noise）</label>
      <label class="al-check al-wide"><input id="rc-positive" type="checkbox">SIRT 非负约束</label>
    </div>
    <div class="al-row"><button id="rc-run" class="small primary">重构并打开</button></div>
    <div id="rc-list"></div>`;

  function show(d) {
    const a = d?.alignment;
    $("rc-geometry").textContent = a
      ? `对齐：轴 ${a.axis.toFixed(2)}° · 残差 ${a.residual.toFixed(2)} px · ${a.views.filter((v) => !v.excluded).length} 个倾角参与重构`
      : "先在③完成倾转对齐。";
    $("rc-run").disabled = !a;
    $("rc-flip").disabled = !d?.ctf;
    if (!d?.ctf) $("rc-flip").checked = false;
    const halves = !!(d?.even && d?.odd);
    $("rc-halves").disabled = !halves;
    $("rc-halves").checked = halves;
    list(d);
  }

  // The dataset's tomograms; a click opens one.
  function list(d) {
    const box = $("rc-list");
    box.replaceChildren();
    for (const t of d?.tomograms ?? []) {
      const row = document.createElement("div");
      row.className = "al-row";
      const kinds = { full: "断层", even: "偶数帧半", odd: "奇数帧半", denoised: "去噪", wedge: "缺失楔补偿" };
      row.innerHTML = `<span class="dim"></span><button class="small">打开</button>`;
      row.firstChild.textContent = `${kinds[t.kind] || t.kind} · ${t.method.toUpperCase()} · bin ${t.bin} · Z ${t.thickness}${t.phaseFlipped ? " · 相位翻转" : ""} · ${W.name(t.path)}`;
      row.lastChild.onclick = () => openPath(t.path);
      box.append(row);
    }
  }

  $("rc-run").onclick = async () => {
    const step = {
      bin: +$("rc-bin").value,
      halves: $("rc-halves").checked,
      phaseFlip: $("rc-flip").checked,
      params: {
        method: $("rc-method").value,
        thickness: +$("rc-z").value,
        iterations: +$("rc-iters").value,
        lambda: 1,
        nonNegative: $("rc-positive").checked,
        zOffset: +$("rc-offset").value,
        filter: $("rc-filter").value,
        backend: "auto",
      },
    };
    const r = await W.step("重构", "tomo_reconstruct", step);
    if (!r) return;
    W.status(`重构完成 · ${r.backend} · ${(r.ms / 1000).toFixed(1)} s`);
    await openPath(r.paths[0]);
  };

  W.register(3, { dataset: show, enter: () => show(W.dataset) });
})();
