"use strict";
// The single-particle workspace (spa::project): a folder of micrographs or
// movies carried through motion correction, CTF, picking, extraction, 2D
// classification, an ab initio model and gold-standard 3D refinement. The
// project lives in <folder>/tifmrcview-spa; every step works on it by path.
// Shares app.js globals: S, T, F, invoke, $, setFft, openPath, saveDir.
(() => {
  const STEPS = ["① 显微图", "② CTF", "③ 挑选", "④ 二维分类", "⑤ 初始模型", "⑥ 三维精修"];
  const P = { project: null, busy: false, current: 0, mic: 0, classes: null };

  const button = document.createElement("button");
  button.id = "btn-spa";
  button.className = "toggle";
  button.title = "单颗粒工作区：运动校正、CTF、挑选、二维分类、初始模型、三维精修";
  button.innerHTML = '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3.2"/><circle cx="5" cy="6" r="2"/><circle cx="19" cy="7" r="2"/><circle cx="6" cy="18" r="2"/><circle cx="18" cy="18" r="2"/></svg><span>单颗粒</span>';
  $("quickbar").append(button);

  const panel = document.createElement("section");
  panel.id = "spa-workspace";
  panel.className = "pro-workspace";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head"><b>单颗粒重构 · Single particle</b><button id="spa-close" class="small" title="关闭">✕</button></div>
    <div class="ws-dataset">
      <span id="spa-name" class="dim">还没有工程：选择显微图或电影所在的文件夹</span>
      <button id="spa-folder" class="small">选择文件夹…</button>
      <button id="spa-here" class="small" title="用当前打开文件所在的文件夹">当前文件夹</button>
    </div>
    <nav class="ws-steps" id="spa-steps" aria-label="单颗粒步骤"></nav>
    <div class="ws-body" id="spa-body"></div>
    <div class="ws-foot"><span id="spa-status" role="status" class="dim"></span><button id="spa-cancel" class="small" hidden>取消</button></div>`;
  $("stage").append(panel);
  const sections = STEPS.map((name, i) => {
    const b = document.createElement("button");
    b.textContent = name;
    b.onclick = () => select(i);
    $("spa-steps").append(b);
    const s = document.createElement("section");
    s.hidden = true;
    $("spa-body").append(s);
    return s;
  });
  const status = (t) => { $("spa-status").textContent = t; };
  const name = (p) => p?.split(/[\\/]/).pop() || "";
  const fmt = (v, d = 1) => (v == null || !isFinite(v) ? "—" : (+v).toFixed(d));

  sections[0].innerHTML = `
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

  sections[1].innerHTML = `
    <div class="al-head"><b>每张显微图的 CTF</b></div>
    <div class="al-opts">
      <label>电压 kV<input id="spa-kv" type="number" min="60" max="400" value="300"></label>
      <label>Cs mm<input id="spa-cs" type="number" min="0" max="10" step="0.01" value="2.7"></label>
      <label>振幅衬度<input id="spa-amp" type="number" min="0" max="0.5" step="0.01" value="0.1"></label>
      <label>谱尺寸<select id="spa-n"><option>256</option><option selected>512</option><option>1024</option></select></label>
      <label>离焦范围 µm<span class="ct-range"><input id="spa-dfmin" type="number" min="0" step="0.1" value="0.3">–<input id="spa-dfmax" type="number" min="0" step="0.1" value="5"></span></label>
      <label>拟合分辨率 Å<span class="ct-range"><input id="spa-lo" type="number" min="5" value="30">–<input id="spa-hi" type="number" min="1" placeholder="自动"></span></label>
    </div>
    <div class="al-row"><button id="spa-run-ctf" class="small primary">拟合全部显微图</button></div>
    <div id="spa-ctf-table" class="ws-table"></div>`;

  sections[2].innerHTML = `
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

  sections[3].innerHTML = `
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
    <p class="dim">点类平均选择要保留的类（蓝框）；后面的步骤只用选中类的颗粒。不选则全部使用。</p>
    <div id="spa-gallery" class="ws-gallery"></div>
    <p id="spa-2d-summary" class="dim"></p>`;

  sections[4].innerHTML = `
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

  sections[5].innerHTML = `
    <div class="al-head"><b>金标准三维精修</b></div>
    <p class="dim">颗粒分成互不相见的两半，各自对齐到自己的图（先全局，后在上次取向附近细化），两半图之间的 FSC 决定下一轮的分辨率和维纳滤波；最后自动掩膜、算 B 因子并锐化。</p>
    <div class="al-opts">
      <label>对称性<input id="spa-sym" type="text" value="C1"></label>
      <label>起始低通 Å<input id="spa-resi" type="number" min="5" value="30"></label>
      <label>最多迭代<input id="spa-it" type="number" min="2" max="50" value="12"></label>
      <label>全局搜索步长 °<input id="spa-step" type="number" min="2" max="20" step="0.5" value="7.5"></label>
      <div class="al-wide al-row"><button id="spa-ref" class="small">参考图（可选）…</button><span id="spa-ref-name" class="dim">用初始模型</span><button id="spa-ref-clear" class="small">清除</button></div>
    </div>
    <div class="al-row"><button id="spa-run-refine" class="small primary">三维精修</button><button id="spa-open-map" class="small" disabled>打开锐化图</button><button id="spa-open-raw" class="small" disabled>打开未锐化图</button></div>
    <p id="spa-ref-summary" class="dim"></p>
    <canvas id="spa-fsc" width="600" height="220" hidden title="FSC：灰 = 未掩膜，蓝 = 掩膜；虚线 0.143"></canvas>
    <p id="spa-files" class="dim"></p>`;

  function select(i) {
    P.current = i;
    sections.forEach((s, j) => {
      s.hidden = i !== j;
      const b = $("spa-steps").children[j];
      b.classList.toggle("on", i === j);
      b.setAttribute("aria-current", i === j ? "step" : "false");
    });
    if (i === 2) showMic();
    if (i === 3) showClasses();
  }

  function marks() {
    const p = P.project;
    const done = [
      !!p && (!p.movies || p.mics.every((m) => m.path !== m.movie)),
      !!p?.mics.some((m) => m.ctf),
      !!p?.mics.some((m) => m.picks),
      !!p?.class2d,
      !!p?.initial,
      !!p?.refine,
    ];
    [...$("spa-steps").children].forEach((b, i) => { b.textContent = STEPS[i] + (done[i] ? " ✓" : ""); });
  }

  function show(p) {
    P.project = p;
    marks();
    if (!p) return;
    const ctf = p.mics.filter((m) => m.ctf).length;
    $("spa-name").textContent = `${name(p.source)} · ${p.mics.length} 张${p.movies ? "电影" : "显微图"}`;
    $("spa-mics").textContent = `${p.mics.length} 张${p.movies ? "电影" : "显微图"}（${p.mics[0]?.width}×${p.mics[0]?.height}）· 工程：${p.dir}`;
    $("spa-diam").value = p.diameterA;
    $("spa-pixel").value = p.optics.pixelA;
    $("spa-motion").hidden = !p.movies;
    $("spa-kv").value = p.optics.kv;
    $("spa-cs").value = p.optics.csMm;
    $("spa-amp").value = p.optics.amplitude;
    // CTF table.
    const t = document.createElement("table");
    t.innerHTML = "<thead><tr><th>显微图</th><th>离焦 µm</th><th>像散 Å</th><th>拟合到 Å</th><th>颗粒</th></tr></thead>";
    const body = document.createElement("tbody");
    p.mics.forEach((m, i) => {
      const row = document.createElement("tr");
      if (m.excluded) row.className = "missing";
      const c = m.ctf;
      for (const x of [name(m.path), c ? fmt((c.df1 + c.df2) / 2e4, 2) : "—", c ? fmt(c.df1 - c.df2, 0) : "—", c ? fmt(c.fitRes) : "—", m.picks || "—"]) {
        const td = document.createElement("td");
        td.textContent = x;
        row.append(td);
      }
      row.onclick = () => { P.mic = i; select(2); };
      body.append(row);
    });
    t.append(body);
    $("spa-ctf-table").replaceChildren(ctf || p.mics.length <= 200 ? t : "");
    const picks = p.mics.reduce((a, m) => a + (m.picks || 0), 0);
    $("spa-pick-summary").textContent = picks ? `已挑选 ${picks} 个颗粒（${p.mics.filter((m) => m.picks).length} 张图）` : "";
    $("spa-stack").textContent = p.stack ? `${p.stack.count} 个颗粒 · 盒子 ${p.stack.boxPx} px · ${fmt(p.stack.pixelA, 2)} Å/px · 已写出 particles.star` : "";
    $("spa-open-init").disabled = $("spa-flip").disabled = !p.initial;
    $("spa-init-summary").textContent = p.initial ? `初始模型：${name(p.initial.path)} · ${p.initial.boxPx}³ · ${fmt(p.initial.pixelA, 2)} Å/px` : "";
    const r = p.refine;
    $("spa-open-map").disabled = $("spa-open-raw").disabled = !r;
    if (r) {
      $("spa-ref-summary").textContent = `分辨率 ${fmt(r.res)} Å（FSC 0.143）· 掩膜后 ${fmt(r.resMasked)} Å · B 因子 ${fmt(r.bfactor, 0)} Å² · ${r.particles} 个颗粒 · ${r.history.length} 轮 · ${r.backend}`;
      $("spa-files").textContent = `写出：${[r.sharpened, r.map, r.half1, r.half2, r.mask, r.star].map(name).join("、")}`;
      fsc(r);
    } else {
      $("spa-ref-summary").textContent = $("spa-files").textContent = "";
      $("spa-fsc").hidden = true;
    }
  }

  async function load(source) {
    try {
      show(await invoke("spa_open", { source }));
      status("已打开工程");
    } catch (e) { status(`无法打开：${e}`); }
  }
  $("spa-folder").onclick = async () => {
    const dir = await invoke("pick_folder", { dir: S.info ? saveDir() : "" });
    if (dir) load(dir);
  };
  $("spa-here").onclick = () => {
    if (!S.info) return status("请先打开一张显微图");
    load(S.info.path);
  };

  async function settings(extra = {}) {
    if (!P.project) return null;
    const p = await invoke("spa_settings", { req: { dir: P.project.dir, diameterA: +$("spa-diam").value, pixelA: +$("spa-pixel").value, ...extra } });
    show(p);
    return p;
  }
  $("spa-save").onclick = async () => { try { await settings(); status("已保存"); } catch (e) { status(`${e}`); } };

  // A long step with progress, a cancel button, and the project returned.
  async function run(label, step, params) {
    if (!P.project) { status("请先选择文件夹"); return null; }
    if (P.busy) return null;
    try { await settings(); } catch (e) { status(`${e}`); return null; }
    P.busy = true;
    $("spa-cancel").hidden = false;
    $("spa-cancel").onclick = () => invoke("spa_cancel");
    const buttons = [...panel.querySelectorAll(".ws-body button, .ws-dataset button")];
    const was = buttons.map((b) => b.disabled);
    buttons.forEach((b) => { b.disabled = true; });
    const t0 = performance.now();
    const un = await T.event.listen("spa-progress", (e) => {
      const p = e.payload;
      const s = (performance.now() - t0) / 1000;
      status(`${p.stage} ${p.done} / ${p.total} · ${s < 120 ? s.toFixed(0) + " s" : (s / 60).toFixed(1) + " min"}`);
    });
    status(`${label}…`);
    try {
      const r = await invoke("spa_run", { req: { dir: P.project.dir, step, params } });
      show(r.project);
      status(`${label}完成 · ${((performance.now() - t0) / 1000).toFixed(0)} s`);
      return r.result ?? true;
    } catch (e) {
      status(`${label}失败：${e}`);
      return null;
    } finally {
      un();
      P.busy = false;
      $("spa-cancel").hidden = true;
      buttons.forEach((b, i) => { b.disabled = was[i]; });
      if (P.project) show(P.project);
    }
  }

  $("spa-run-motion").onclick = () => {
    const dose = +$("spa-dose").value;
    run("运动校正", "motion", {
      group: +$("spa-group").value, upsample: 1, frameAxis: "auto", workingSize: 1024, local: $("spa-local").checked, patches: 5,
      doseWeighted: dose > 0, unweighted: false, evenOdd: false, pixelA: +$("spa-pixel").value || null, voltageKv: +$("spa-kv").value || 300,
      voltageAuto: false, dosePerFrame: dose > 0 ? dose : null, totalDose: null, preExposure: 0, gainPath: null, gainRotate: 0,
      gainFlipX: false, gainFlipY: false, backend: "auto", benchmark: false, diagnostics: false,
    });
  };

  $("spa-run-ctf").onclick = () => {
    const px = +$("spa-pixel").value;
    if (!(px > 0)) return status("请先填写像素大小");
    run("CTF 拟合", "ctf", {
      n: +$("spa-n").value,
      params: {
        voltageKv: +$("spa-kv").value, csMm: +$("spa-cs").value, ampContrast: +$("spa-amp").value, pixelA: px,
        resLowA: +$("spa-lo").value, resHighA: +$("spa-hi").value || Math.max(4, px * 2.5),
        dfMinA: +$("spa-dfmin").value * 1e4, dfMaxA: +$("spa-dfmax").value * 1e4, maxAstigA: 2000, phase: false,
      },
    });
  };

  const pickParams = () => ({
    diameterA: +$("spa-diam").value, dark: $("spa-dark").checked, threshold: +$("spa-thr").value,
    distance: +$("spa-dist").value, contamination: +$("spa-contam").value, marginA: 0, maxPerMic: +$("spa-max").value,
  });
  async function showMic(params = null) {
    const p = P.project;
    if (!p || !p.mics.length) return;
    P.mic = Math.max(0, Math.min(P.mic, p.mics.length - 1));
    const m = p.mics[P.mic];
    $("spa-mic-name").textContent = `${P.mic + 1} / ${p.mics.length} · ${name(m.path)}${m.excluded ? "（已排除）" : ""}`;
    $("spa-exclude").textContent = m.excluded ? "恢复" : "排除";
    if (p.movies && m.path === m.movie) { $("spa-pick-img").hidden = true; return status("请先做运动校正"); }
    try {
      const r = await invoke("spa_preview", { dir: p.dir, mic: P.mic, params });
      $("spa-pick-img").src = `data:image/png;base64,${r.png}`;
      $("spa-pick-img").hidden = false;
      status(`${params ? "预览" : "已保存的挑选"}：${r.picks} 个颗粒`);
    } catch (e) { status(`${e}`); }
  }
  $("spa-prev").onclick = () => { P.mic--; showMic(); };
  $("spa-next").onclick = () => { P.mic++; showMic(); };
  $("spa-preview").onclick = async () => { await settings(); showMic(pickParams()); };
  $("spa-exclude").onclick = async () => {
    const p = P.project;
    if (!p) return;
    const ex = p.mics.map((m, i) => (i === P.mic ? !m.excluded : m.excluded) ? i : -1).filter((i) => i >= 0);
    await settings({ excluded: ex });
    showMic();
  };
  $("spa-run-pick").onclick = async () => { if (await run("挑选颗粒", "pick", pickParams()) != null) showMic(); };

  $("spa-run-extract").onclick = () => run("提取颗粒", "extract", { maxBox: +$("spa-box").value, invert: $("spa-invert").checked });
  $("spa-run-2d").onclick = async () => {
    const r = await run("二维分类", "class2d", { classes: +$("spa-k").value, iterations: +$("spa-it2").value, psiStep: +$("spa-psi").value, resStart: 30, resEnd: +$("spa-res2").value, maxShiftA: P.project ? P.project.diameterA * 0.2 : 30, boxPx: 64, seed: 1 });
    if (r) showClasses();
  };

  async function showClasses() {
    const p = P.project;
    if (!p?.class2d) { $("spa-gallery").replaceChildren(); $("spa-2d-summary").textContent = ""; return; }
    try {
      const r = await invoke("spa_classes", { dir: p.dir });
      P.classes = { ...r, chosen: new Set(r.selected) };
      const g = $("spa-gallery");
      g.replaceChildren();
      const img = new Image();
      img.src = `data:image/png;base64,${r.png}`;
      img.onload = () => {
        r.counts.forEach((n, i) => {
          const c = document.createElement("canvas");
          c.width = c.height = r.cell;
          c.getContext("2d").drawImage(img, (i % r.cols) * r.cell, Math.floor(i / r.cols) * r.cell, r.cell, r.cell, 0, 0, r.cell, r.cell);
          const cell = document.createElement("figure");
          cell.className = P.classes.chosen.has(i) ? "on" : "";
          cell.title = `类 ${i + 1}：${n} 个颗粒 · 相关 ${fmt(r.scores[i], 2)}`;
          const cap = document.createElement("figcaption");
          cap.textContent = `${i + 1} · ${n}`;
          cell.append(c, cap);
          cell.onclick = () => {
            P.classes.chosen.has(i) ? P.classes.chosen.delete(i) : P.classes.chosen.add(i);
            cell.className = P.classes.chosen.has(i) ? "on" : "";
            $("spa-keep").disabled = false;
          };
          g.append(cell);
        });
      };
      const total = r.counts.reduce((a, b) => a + b, 0);
      const kept = r.selected.length ? r.selected.reduce((a, i) => a + r.counts[i], 0) : total;
      $("spa-2d-summary").textContent = `${r.counts.length} 类 · ${total} 个颗粒 · 后续使用 ${kept} 个`;
    } catch (e) { status(`${e}`); }
  }
  $("spa-keep").onclick = async () => {
    const n = await run("保存选择", "select", [...P.classes.chosen].sort((a, b) => a - b));
    if (n != null) { status(`后续步骤使用 ${n} 个颗粒`); showClasses(); }
  };

  $("spa-run-init").onclick = () => run("初始模型", "initial", { symmetry: $("spa-sym0").value || "C1", iterations: +$("spa-it0").value, batch: +$("spa-batch").value, resStart: 35, resEnd: +$("spa-res0").value, boxPx: 64, seed: 7 });
  $("spa-open-init").onclick = () => P.project?.initial && openPath(P.project.initial.path, true);
  $("spa-flip").onclick = () => run("翻转手性", "flip", null);

  let reference = null;
  $("spa-ref").onclick = async () => {
    const p = await invoke("motion_pick", { dir: P.project ? P.project.dir : saveDir(), kind: "gain" });
    if (p) { reference = p; $("spa-ref-name").textContent = name(p); }
  };
  $("spa-ref-clear").onclick = () => { reference = null; $("spa-ref-name").textContent = "用初始模型"; };
  $("spa-run-refine").onclick = () => {
    const sym = $("spa-sym").value || "C1";
    run("三维精修", "refine", { reference, params: { symmetry: sym, resInit: +$("spa-resi").value, maxIterations: +$("spa-it").value, globalIterations: 2, globalStep: +$("spa-step").value, seed: 11 } });
  };
  $("spa-open-map").onclick = () => P.project?.refine && openPath(P.project.refine.sharpened, true);
  $("spa-open-raw").onclick = () => P.project?.refine && openPath(P.project.refine.map, true);

  function fsc(r) {
    const cv = $("spa-fsc"), ctx = cv.getContext("2d"), w = cv.width, h = cv.height;
    cv.hidden = false;
    ctx.clearRect(0, 0, w, h);
    const n = r.fsc.length, x = (i) => 40 + i / Math.max(1, n - 1) * (w - 55), y = (v) => 15 + (1 - Math.max(-0.1, Math.min(1, v))) / 1.1 * (h - 45);
    ctx.strokeStyle = "#ccc";
    ctx.setLineDash([4, 4]);
    ctx.beginPath(); ctx.moveTo(x(0), y(0.143)); ctx.lineTo(x(n - 1), y(0.143)); ctx.stroke();
    ctx.setLineDash([]);
    for (const [curve, color] of [[r.fsc, "#999"], [r.fscMasked, "#0969da"]]) {
      ctx.strokeStyle = color;
      ctx.lineWidth = 2;
      ctx.beginPath();
      curve.forEach((v, i) => (i ? ctx.lineTo(x(i), y(v)) : ctx.moveTo(x(i), y(v))));
      ctx.stroke();
    }
    ctx.fillStyle = "#888";
    ctx.font = "13px sans-serif";
    ctx.fillText("1", 25, y(1) + 4);
    ctx.fillText("0.143", 2, y(0.143) + 4);
    for (const s of [4, 2, 4 / 3, 1]) {
      const i = (n - 1) / s * 1;
      const res = r.boxPx * r.pixelA / i;
      ctx.fillText(`${res.toFixed(1)} Å`, x(i) - 18, h - 8);
    }
  }

  function close() {
    panel.hidden = true;
    button.classList.remove("on");
  }
  button.onclick = () => {
    if (!panel.hidden) return close();
    if (F.on) setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
    panel.hidden = false;
    button.classList.add("on");
    select(P.current);
  };
  $("spa-close").onclick = close;
  document.addEventListener("pro-panel", (e) => { if (e.detail !== panel.id && !panel.hidden) close(); });
  $("btn-fft").addEventListener("click", () => { if (F.on) close(); });
  select(0);
})();
