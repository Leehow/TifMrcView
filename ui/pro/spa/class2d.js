"use strict";
// Step ④: extraction into one particle stack and reference-free 2D
// classification. The sharp classes (even/odd FRC) are chosen by default;
// a click on an average adds or removes it.
(() => {
  const W = window.TifMrcViewPro.spa;
  const { fmt } = W;
  let classes = null;
  W.sections[3].innerHTML = `
    <div class="al-head"><b>提取与二维分类</b></div>
    <div class="al-opts">
      <label>输出盒子（像素，上限）<select id="spa-box"><option>64</option><option selected>128</option><option>256</option></select></label>
      <label class="al-check"><input id="spa-invert" type="checkbox" checked>反转衬度（蛋白变亮）</label>
    </div>
    <div class="al-row"><button id="spa-run-extract" class="small primary">提取颗粒</button><span id="spa-stack" class="dim"></span></div>
    <div class="al-opts">
      <label>类别数<input id="spa-k" type="number" min="1" max="200" value="20"></label>
      <label>迭代<input id="spa-it2" type="number" min="1" max="100" value="20"></label>
      <label>面内角步长 °<input id="spa-psi" type="number" min="1" max="45" value="6"></label>
      <label>最终分辨率 Å<input id="spa-res2" type="number" min="2" value="8"></label>
    </div>
    <div class="al-row"><button id="spa-run-2d" class="small primary">二维分类</button><button id="spa-keep" class="small" disabled>保留选中的类</button></div>
    <p class="dim">分类完成后自动选中清晰的类（奇偶两半颗粒的类平均之间的 FRC 高的类，高亮框）；点类平均可增减。后面的步骤只用选中类的颗粒，不选则全部使用。</p>
    <div id="spa-gallery" class="ws-gallery"></div>
    <p id="spa-2d-summary" class="dim"></p>`;

  // The class averages as a gallery in `gallery` (a click adds or removes a
  // class; `changed` is told), the counts in `summary`. Returns the state
  // ({counts, chosen: Set, …}) or null. Shared with the wizard (auto.js).
  W.drawClasses = async (gallery, summary, changed = () => {}) => {
    const p = W.project;
    if (!p?.class2d) { gallery.replaceChildren(); summary.textContent = ""; return null; }
    let r;
    try { r = await invoke("spa_classes", { dir: p.dir }); } catch (e) { W.status(`${e}`); return null; }
    const st = { ...r, chosen: new Set(r.selected) };
    const used = () => (st.chosen.size ? [...st.chosen].reduce((a, i) => a + r.counts[i], 0) : r.counts.reduce((a, b) => a + b, 0));
    const sum = () => { summary.textContent = `${r.counts.length} 类 · ${r.counts.reduce((a, b) => a + b, 0)} 个颗粒 · 选中 ${st.chosen.size} 类，共 ${used()} 个颗粒`; };
    gallery.replaceChildren();
    const img = new Image();
    img.src = `data:image/png;base64,${r.png}`;
    img.onload = () => {
      r.counts.forEach((n, i) => {
        const c = document.createElement("canvas");
        c.width = c.height = r.cell;
        c.getContext("2d").drawImage(img, (i % r.cols) * r.cell, Math.floor(i / r.cols) * r.cell, r.cell, r.cell, 0, 0, r.cell, r.cell);
        const cell = document.createElement("figure");
        cell.className = st.chosen.has(i) ? "on" : "";
        const res = r.res && r.res[i] < 999 ? `${fmt(r.res[i], 0)} Å` : "";
        cell.title = `类 ${i + 1}：${n} 个颗粒 · 相关 ${fmt(r.scores[i], 2)}${res ? ` · 分辨率 ${res}` : ""}`;
        const cap = document.createElement("figcaption");
        cap.textContent = res ? `${n} · ${res}` : `${i + 1} · ${n}`;
        cell.append(c, cap);
        cell.onclick = () => {
          st.chosen.has(i) ? st.chosen.delete(i) : st.chosen.add(i);
          cell.className = st.chosen.has(i) ? "on" : "";
          sum();
          changed(st);
        };
        gallery.append(cell);
      });
    };
    sum();
    return st;
  };

  async function showClasses() {
    classes = await W.drawClasses($("spa-gallery"), $("spa-2d-summary"), () => { $("spa-keep").disabled = false; });
  }

  W.register(3, {
    enter: showClasses,
    show(p) {
      $("spa-stack").textContent = p.stack ? `${p.stack.count} 个颗粒 · 盒子 ${p.stack.boxPx} px · ${fmt(p.stack.pixelA, 2)} Å/px · 已写出 particles.star` : "";
    },
  });

  $("spa-run-extract").onclick = () => W.run("提取颗粒", "extract", { maxBox: +$("spa-box").value, invert: $("spa-invert").checked });
  $("spa-run-2d").onclick = async () => {
    const r = await W.run("二维分类", "class2d", { classes: +$("spa-k").value, iterations: +$("spa-it2").value, psiStep: +$("spa-psi").value, resStart: 30, resEnd: +$("spa-res2").value, maxShiftA: W.project ? W.project.diameterA * 0.2 : 30, boxPx: 64, seed: 1 });
    if (r) showClasses();
  };
  $("spa-keep").onclick = async () => {
    const n = await W.run("保存选择", "select", [...classes.chosen].sort((a, b) => a - b));
    if (n != null) { W.status(`后续步骤使用 ${n} 个颗粒`); $("spa-keep").disabled = true; showClasses(); }
  };
})();
