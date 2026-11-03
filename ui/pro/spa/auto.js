"use strict";
// One-click mode, the panel's default view (the steps by hand are under
// 高级设置). Motion correction and CTF use the optics read from the open
// folder; pixel size and voltage are asked only when no file had them.
// The kinds of particle found on a few denoised micrographs are cards.
// The user clicks theirs; picking, 2D classification, the point group,
// the initial model and the refinement then run on their own.
(() => {
  const W = window.TifMrcViewPro.spa;
  const { fmt } = W;
  $("spa-auto").innerHTML = `
    <p class="dim">对当前打开的文件所在文件夹自动做运动校正和 CTF，在几张去噪后的显微图上找出不同种类的颗粒。点出你的样品后，挑选、二维分类、初始模型和三维精修会接着做完。</p>
    <p id="spa-optics-line" class="dim"></p>
    <div id="spa-ask" class="al-opts" hidden>
      <p class="dim al-wide">当前文件夹里没有找到配置文件中的这些参数，请填写：</p>
      <label id="spa-ask-pixel-row">像素大小 Å<input id="spa-ask-pixel" type="number" min="0.05" step="0.001" placeholder="例如 1.06"></label>
      <label id="spa-ask-kv-row">电压 kV<input id="spa-ask-kv" type="number" min="20" max="400" step="10" placeholder="例如 300"></label>
    </div>
    <div class="al-row"><button id="spa-a-start" class="primary">一键开始</button></div>
    <div id="spa-a-kinds" class="ws-kinds"></div>
    <p id="spa-a-hint" class="dim"></p>
    <div id="spa-a-finish" hidden>
      <div class="al-opts">
        <label>对称性<select id="spa-a-sym"><option value="">自动判断</option><option>C1</option></select></label>
        <label>或填写<input id="spa-a-sym-text" type="text" placeholder="如 C4、D7、O" size="5"></label>
      </div>
      <div class="al-row"><button id="spa-a-go" class="primary" disabled>用选中的颗粒完成重构</button></div>
    </div>
    <p id="spa-a-result" class="dim"></p>
    <div class="al-row" id="spa-a-open" hidden><button id="spa-a-map" class="small">打开三维图</button><button id="spa-a-more" class="small">在高级设置里查看</button></div>`;

  const chosen = new Set();

  function opticsLine(p) {
    const f = p.opticsFound || {};
    const bits = [];
    if (f.pixel) bits.push(`像素 ${fmt(p.optics.pixelA, 3)} Å`);
    if (f.kv) bits.push(`${fmt(p.optics.kv, 0)} kV`);
    if (f.cs) bits.push(`Cs ${fmt(p.optics.csMm, 2)} mm`);
    if (f.amplitude) bits.push(`振幅衬度 ${fmt(p.optics.amplitude, 2)}`);
    if (p.doseTotal > 0) bits.push(`剂量 ${fmt(p.doseTotal, 2)} e⁻/Å²`);
    if (f.file) bits.push(f.file);
    return bits.join(" · ");
  }

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

  W.hooks.set("auto", {
    show(p) {
      if (!p) {
        W.kindMark = "";
        $("spa-optics-line").textContent = "";
        $("spa-ask").hidden = true;
        $("spa-a-kinds").replaceChildren();
        $("spa-a-hint").textContent = "";
        $("spa-a-finish").hidden = true;
        $("spa-a-open").hidden = true;
        $("spa-a-result").textContent = "";
        return;
      }
      const f = p.opticsFound || { pixel: true, kv: true };
      $("spa-ask-pixel-row").hidden = !!f.pixel;
      $("spa-ask-kv-row").hidden = !!f.kv;
      $("spa-ask").hidden = !!(f.pixel && f.kv);
      $("spa-optics-line").textContent = opticsLine(p);
      showResult(p);
      // Rebuilding the cards on every refresh would drop a selection that
      // has not been saved yet. Rebuild only when the found set changes.
      const mark = `${p.dir}:${p.discovery?.micrographs ?? ""}:${(p.discovery?.chosen || []).join(",")}`;
      if (mark !== W.kindMark) {
        W.kindMark = mark;
        if (p.discovery) showKinds();
        else {
          $("spa-a-kinds").replaceChildren();
          $("spa-a-hint").textContent = "";
          $("spa-a-finish").hidden = true;
        }
      }
    },
  });

  $("spa-a-start").onclick = async () => {
    if (!W.project) return W.status("请先打开一张显微图或电影");
    const f = W.project.opticsFound || { pixel: true, kv: true };
    if (!f.pixel) {
      const px = +$("spa-ask-pixel").value;
      if (!(px > 0)) return W.status("当前文件夹里没有找到像素大小，请填写（Å）");
      $("spa-pixel").value = px;
    }
    const pending = {};
    if (!f.kv) {
      const kv = +$("spa-ask-kv").value;
      if (!(kv > 0)) return W.status("当前文件夹里没有找到电压，请填写（kV）");
      $("spa-kv").value = kv;
      pending.kv = kv;
      pending.kvFromUser = true;
    }
    W.pendingOptics = pending;
    let p = W.project;
    if (p.movies && p.mics.some((m) => m.path === m.movie)) {
      if (await $("spa-run-motion").onclick() == null) return;
      p = W.project;
    } else if (pending.kvFromUser || !f.pixel) {
      try { await W.settings(); } catch (e) { return W.status(`${e}`); }
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
  $("spa-a-more").onclick = () => { $("spa-advanced").open = true; W.select(5); };
})();
