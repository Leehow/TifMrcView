"use strict";
// One-click mode: motion correction and CTF with the usual settings, then
// the kinds of particle found on a few denoised micrographs (spa::discover)
// as cards. The user clicks theirs; picking, 2D classification (sharp
// classes kept), the point group, the initial model and the refinement
// then run on their own. The steps by hand stay available for changes.
(() => {
  const W = window.TifMrcViewPro.spa;
  const { fmt } = W;
  const host = W.sections[6];
  host.innerHTML = `
    <div class="al-head"><b>一键重构</b></div>
    <p class="dim">选好显微图或电影所在的文件夹，点“开始”。程序自动做运动校正和 CTF，在几张去噪后的显微图上找出不同种类的颗粒，你只需点出哪一种是你的样品，剩下的全部自动完成。</p>
    <div class="al-opts">
      <label>像素 Å<input id="spa-a-pixel" type="number" min="0.1" step="0.001" title="从文件头读取；读不到时请填写"></label>
      <label>电压 kV<select id="spa-a-kv"><option>300</option><option>200</option><option>120</option></select></label>
    </div>
    <div class="al-row"><button id="spa-a-start" class="small primary">① 开始：预处理并寻找颗粒</button></div>
    <div id="spa-a-kinds" class="ws-kinds"></div>
    <p id="spa-a-hint" class="dim"></p>
    <div id="spa-a-finish" hidden>
      <div class="al-opts">
        <label>对称性<select id="spa-a-sym"><option value="">自动判断</option><option>C1</option></select></label>
        <label>或填写<input id="spa-a-sym-text" type="text" placeholder="如 C4、D7、O" size="5"></label>
      </div>
      <div class="al-row"><button id="spa-a-go" class="small primary" disabled>② 用选中的颗粒完成重构</button></div>
    </div>
    <p id="spa-a-result" class="dim"></p>
    <div class="al-row" id="spa-a-open" hidden><button id="spa-a-map" class="small">打开三维图</button><button id="spa-a-more" class="small">查看各步骤</button></div>`;

  const chosen = new Set();

  async function showKinds() {
    const p = W.project;
    const box = $("spa-a-kinds");
    box.replaceChildren();
    $("spa-a-finish").hidden = !p?.discovery;
    if (!p?.discovery) { $("spa-a-hint").textContent = ""; return; }
    let kinds;
    try { kinds = await invoke("spa_kinds", { dir: p.dir }); } catch (e) { W.status(`${e}`); return; }
    chosen.clear();
    for (const i of p.discovery.chosen || []) chosen.add(i);
    kinds.forEach((k, i) => {
      const card = document.createElement("figure");
      card.className = chosen.has(i) ? "on" : "";
      const img = new Image();
      img.src = `data:image/png;base64,${k.png}`;
      img.alt = `第 ${i + 1} 种`;
      const cap = document.createElement("figcaption");
      const shape = k.elongation > 1.6 ? " · 细长" : "";
      cap.textContent = `第 ${i + 1} 种 · 约 ${fmt(k.diameterA, 0)} Å · 占 ${fmt(k.share * 100, 0)}%${shape}`;
      card.append(img, cap);
      card.onclick = () => {
        chosen.has(i) ? chosen.delete(i) : chosen.add(i);
        card.className = chosen.has(i) ? "on" : "";
        $("spa-a-go").disabled = !chosen.size;
      };
      box.append(card);
    });
    $("spa-a-go").disabled = !chosen.size;
    $("spa-a-hint").textContent = `在 ${p.discovery.micrographs} 张显微图上找到 ${kinds.length} 种${p.discovery.outlined ? "（SAM 描边）" : ""}。点选你的颗粒（同一种颗粒的不同朝向可以多选），冰渣、碳膜边缘、团聚的不要选。`;
  }

  function showResult(p) {
    const r = p?.refine;
    $("spa-a-open").hidden = !r;
    if (!r) { $("spa-a-result").textContent = ""; return; }
    const sym = p.symmetry ? ` · 对称性 ${r.params.symmetry}${p.symmetry.symmetry === r.params.symmetry ? "（自动判断）" : ""}` : ` · 对称性 ${r.params.symmetry}`;
    $("spa-a-result").textContent = `完成：分辨率 ${fmt(r.res)} Å，掩膜后 ${fmt(r.resMasked)} Å · ${r.particles} 个颗粒${sym}`;
  }

  W.register(6, {
    enter: showKinds,
    show(p) {
      $("spa-a-pixel").value = p.optics.pixelA;
      $("spa-a-kv").value = String(Math.round(p.optics.kv || 300));
      showResult(p);
    },
  });

  $("spa-a-start").onclick = async () => {
    if (!W.project) {
      const dir = await invoke("pick_folder", { dir: S.info ? saveDir() : "" });
      if (!dir) return;
      await W.load(dir);
      if (!W.project) return;
    }
    const px = +$("spa-a-pixel").value;
    if (!(px > 0)) return W.status("请填写像素大小（Å）");
    $("spa-pixel").value = px;
    $("spa-kv").value = $("spa-a-kv").value;
    let p = W.project;
    if (p.movies && p.mics.some((m) => m.path === m.movie)) {
      if (await $("spa-run-motion").onclick() == null) return;
      p = W.project;
    }
    if (!p.mics.some((m) => m.ctf)) {
      if (await $("spa-run-ctf").onclick() == null) return;
    }
    if (await W.run("寻找颗粒", "discover", { sam: true }) != null) showKinds();
  };

  $("spa-a-go").onclick = async () => {
    const d = await W.run("保存选择", "choose", [...chosen].sort((a, b) => a - b));
    if (d == null) return;
    const symmetry = ($("spa-a-sym-text").value.trim() || $("spa-a-sym").value).toUpperCase();
    const r = await W.run(`重构（颗粒直径 ${fmt(d, 0)} Å）`, "auto", { symmetry, sam: true });
    if (r) showResult(W.project);
  };
  $("spa-a-map").onclick = () => W.project?.refine && openPath(W.project.refine.sharpened, true);
  $("spa-a-more").onclick = () => W.select(5);
})();
