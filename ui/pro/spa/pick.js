"use strict";
// Step ③: template-free picking (difference of Gaussians), previewed on
// one micrograph at a time; micrographs can be left out here.
(() => {
  const W = window.TifMrcViewPro.spa;
  W.mic = 0;
  W.sections[2].innerHTML = `
    <div class="al-head"><b>颗粒挑选（无模板）</b></div>
    <p class="dim">高斯差分斑点检测，按颗粒直径调好尺度；稳健 z 分数做阈值，大尺度过滤跳过冰污染和碳膜边缘。先在一张图上预览再挑选全部。</p>
    <div class="al-opts">
      <label>阈值 z<input id="spa-thr" type="number" min="0.5" max="20" step="0.1" value="3"></label>
      <label>最小间距（直径）<input id="spa-dist" type="number" min="0.3" max="3" step="0.05" value="0.9"></label>
      <label>污染过滤 z（0 关）<input id="spa-contam" type="number" min="0" max="20" step="0.5" value="3"></label>
      <label>每张最多<input id="spa-max" type="number" min="0" value="0" title="0：不限"></label>
      <label class="al-check"><input id="spa-dark" type="checkbox" checked>颗粒比背景暗（冷冻电镜；负染取消）</label>
    </div>
    <div class="al-row"><button id="spa-prev" class="small">◀</button><span id="spa-mic-name" class="dim"></span><button id="spa-next" class="small">▶</button><button id="spa-exclude" class="small" title="不用这张显微图">排除</button><button id="spa-preview" class="small">预览</button><button id="spa-run-pick" class="small primary">挑选全部</button></div>
    <img id="spa-pick-img" class="ws-img" alt="" hidden>
    <p id="spa-pick-summary" class="dim"></p>`;

  const params = () => ({
    diameterA: +$("spa-diam").value, dark: $("spa-dark").checked, threshold: +$("spa-thr").value,
    distance: +$("spa-dist").value, contamination: +$("spa-contam").value, marginA: 0, maxPerMic: +$("spa-max").value,
  });

  async function showMic(pp = null) {
    const p = W.project;
    if (!p || !p.mics.length) return;
    W.mic = Math.max(0, Math.min(W.mic, p.mics.length - 1));
    const m = p.mics[W.mic];
    $("spa-mic-name").textContent = `${W.mic + 1} / ${p.mics.length} · ${W.name(m.path)}${m.excluded ? "（已排除）" : ""}`;
    $("spa-exclude").textContent = m.excluded ? "恢复" : "排除";
    if (p.movies && m.path === m.movie) { $("spa-pick-img").hidden = true; return W.status("请先做运动校正"); }
    try {
      const r = await invoke("spa_preview", { dir: p.dir, mic: W.mic, params: pp });
      $("spa-pick-img").src = `data:image/png;base64,${r.png}`;
      $("spa-pick-img").hidden = false;
      W.status(`${pp ? "预览" : "已保存的挑选"}：${r.picks} 个颗粒`);
    } catch (e) { W.status(`${e}`); }
  }

  W.register(2, {
    enter: () => showMic(),
    show(p) {
      const picks = p.mics.reduce((a, m) => a + (m.picks || 0), 0);
      $("spa-pick-summary").textContent = picks ? `已挑选 ${picks} 个颗粒（${p.mics.filter((m) => m.picks).length} 张图）` : "";
    },
  });

  $("spa-prev").onclick = () => { W.mic--; showMic(); };
  $("spa-next").onclick = () => { W.mic++; showMic(); };
  $("spa-preview").onclick = async () => { await W.settings(); showMic(params()); };
  $("spa-exclude").onclick = async () => {
    const p = W.project;
    if (!p) return;
    const ex = p.mics.map((m, i) => (i === W.mic ? !m.excluded : m.excluded) ? i : -1).filter((i) => i >= 0);
    await W.settings({ excluded: ex });
    showMic();
  };
  $("spa-run-pick").onclick = async () => { if (await W.run("挑选颗粒", "pick", params()) != null) showMic(); };
})();
