"use strict";
// Picking by example, the default way into single-particle picking. The particles are marked
// with the viewer's one label brush (labels.js), not with a tool of this workspace: open a
// micrograph, paint two or three particles on a layer, click 小颗粒 so the paint snaps to them
// (找同类 may add more on that micrograph). The micrographs with label layers are listed here
// (sta_label_layers) with their layers as tick boxes; 按例子找颗粒 searches every micrograph for
// what looks like the painted ones (spa_run step "examples", spa/examples.rs) and the places
// found are shown on one micrograph at a time to check (spa_examples_view): green circles are
// kept, grey ones are not. A click on a circle flips it, a click elsewhere adds one, the slider
// moves how alike is alike enough (spa_examples_set), 再找一轮 searches again with the user's
// clicks as more examples and counter-examples. 用这些颗粒完成重构 makes the kept places the picks
// (step "examplesUse") and runs the rest ("auto" with picked: extraction, 2D classes, symmetry,
// initial model, refinement). Shares app.js globals: S, $, invoke, openPath, setTool.
(() => {
  const W = window.TifMrcViewPro.spa;
  const { fmt } = W;
  const E = { layers: [], ticks: new Set(), view: null, mic: 0, found: false, gen: 0, img: null };
  const key = (l) => `${l.image}\n${l.label}`;
  const TYPE = { particle: "小颗粒", ball: "球", shell: "空心球", membrane: "单层膜", membrane2: "双层膜", fibre: "纤维", free: "不规则", "": "未选类型" };

  const box = document.createElement("div");
  box.id = "spa-ex";
  box.innerHTML = `
    <h4>挑颗粒：涂几个例子</h4>
    <ol class="dim spa-teach">
      <li>按「打开一张显微图标注」（或在查看器里打开任意一张），换成标签画笔。</li>
      <li>在 2–3 个你要的颗粒上涂一下，点结构类型「小颗粒」，让涂的贴到颗粒上。不同朝向（顶视、侧视）都要的话各涂一两个。</li>
      <li>回到这里，勾选这个图层，按「按例子找颗粒」。</li>
    </ol>
    <div class="al-row"><button id="spa-ex-open" class="small">打开一张显微图标注</button><button id="spa-ex-reload" class="small" title="标签画笔改过之后，重新读一遍各显微图的图层">刷新图层</button></div>
    <div id="spa-ex-layers" class="spa-ex-layers"></div>
    <div class="al-row"><button id="spa-ex-find" class="primary" disabled>按例子找颗粒</button></div>
    <div id="spa-ex-review" hidden>
      <p id="spa-ex-sum" class="dim"></p>
      <div class="al-row spa-ex-nav">
        <button id="spa-ex-prev" class="small">◀</button><span id="spa-ex-name" class="dim"></span><button id="spa-ex-next" class="small">▶</button>
      </div>
      <div class="spa-ex-view"><canvas id="spa-ex-canvas" title="点圆圈：要 ⇄ 不要；点空白处：加一个颗粒"></canvas></div>
      <label class="al-wide spa-ex-cut">像例子的程度<input id="spa-ex-cutoff" type="range" min="0" max="1" step="0.005"><span id="spa-ex-cutval" class="dim"></span><button id="spa-ex-reset" class="small" title="回到程序按例子建议的松紧">建议值</button></label>
      <p class="dim">绿色是要的，灰色是不要的。点圆圈切换，点空白处加一个；点得多了按「再找一轮」，程序会把你加的当成例子、去掉的当成反例再找。</p>
      <div class="al-opts">
        <label>对称性<select id="spa-ex-sym"><option value="">自动判断</option><option>C1</option></select></label>
        <label>或填写<input id="spa-ex-sym-text" type="text" placeholder="如 C4、D7、O" size="5"></label>
      </div>
      <div class="al-row"><button id="spa-ex-again" class="small">再找一轮</button><button id="spa-ex-go" class="primary">用这些颗粒完成重构</button></div>
    </div>`;
  $("spa-ex-slot").append(box);

  const p = () => W.project;

  async function reload() {
    const gen = ++E.gen;
    const pr = p();
    if (!pr) { E.layers = []; render(); return; }
    try {
      const layers = await invoke("sta_label_layers", { images: pr.mics.map((m) => m.path) });
      if (gen !== E.gen) return;
      E.layers = layers || [];
    } catch (_) { if (gen !== E.gen) return; E.layers = []; /* no backend (web demo) */ }
    // A layer of particles starts ticked.
    for (const l of E.layers) if (l.structure === "particle" && !E.ticked) E.ticks.add(key(l));
    E.ticked = true;
    render();
    if (!E.view) loadView(E.mic, true);
  }

  function render() {
    const list = $("spa-ex-layers");
    for (const k of [...E.ticks]) if (!E.layers.some((l) => key(l) === k)) E.ticks.delete(k);
    if (!E.layers.length) {
      list.innerHTML = '<p class="dim">还没有显微图带标签图层。</p>';
    } else {
      list.replaceChildren(...E.layers.map((l) => {
        const lab = document.createElement("label");
        lab.className = "sta-layer";
        const cb = document.createElement("input");
        cb.type = "checkbox";
        cb.checked = E.ticks.has(key(l));
        cb.disabled = W.busy;
        cb.onchange = () => { if (cb.checked) E.ticks.add(key(l)); else E.ticks.delete(key(l)); render(); };
        const dot = document.createElement("span");
        dot.className = "sta-layer-dot";
        if (l.color) dot.style.background = `rgb(${l.color.join(",")})`;
        const t = document.createElement("span");
        t.textContent = `${W.name(l.image)} · ${l.name} · ${TYPE[l.structure] ?? l.structure} · ${l.objects} 个`;
        lab.append(cb, dot, t);
        return lab;
      }));
    }
    $("spa-ex-find").disabled = !E.ticks.size || W.busy || !p();
  }

  // The best-fitting micrograph by CTF, else the first.
  function bestMic() {
    const mics = p()?.mics || [];
    let best = 0, score = -Infinity;
    mics.forEach((m, i) => { if (!m.excluded && m.ctf && m.ctf.score > score) { score = m.ctf.score; best = i; } });
    return best;
  }

  $("spa-ex-open").onclick = async () => {
    const m = p()?.mics[bestMic()];
    if (!m) return W.status("请先打开一张显微图或电影");
    try { await openPath(m.path); } catch (e) { return W.status(`${e}`); }
    if (typeof setTool === "function") setTool("label");
    W.status("用标签画笔在 2–3 个颗粒上涂一下，点「小颗粒」，再回到这里勾选图层");
  };
  $("spa-ex-reload").onclick = reload;

  $("spa-ex-find").onclick = async () => {
    const ticked = E.layers.filter((l) => E.ticks.has(key(l))).map((l) => [l.image, l.label]);
    const r = await W.run("按例子找颗粒", "examples", { ticked });
    if (r == null) return;
    const mic = W.project.mics.findIndex((m) => m.path === ticked[0]?.[0]);
    loadView(mic >= 0 ? mic : 0, false);
  };
  $("spa-ex-again").onclick = async () => {
    if (await W.run("再找一轮", "examples", { again: true }) != null) loadView(E.mic, false);
  };
  $("spa-ex-go").onclick = async () => {
    const n = await W.run("保存挑选", "examplesUse", {});
    if (n == null) return;
    const symmetry = ($("spa-ex-sym-text").value.trim() || $("spa-ex-sym").value).toUpperCase();
    const r = await W.run(`重构（${n} 个颗粒，直径 ${fmt(W.project.diameterA, 0)} Å）`, "auto", { symmetry, picked: true });
    if (r) W.showResult?.(W.project);
  };

  async function loadView(mic, quiet) {
    const pr = p();
    if (!pr) return;
    const n = pr.mics.length;
    mic = ((mic % n) + n) % n;
    let v;
    try { v = await invoke("spa_examples_view", { dir: pr.dir, mic }); } catch (e) {
      if (!quiet) W.status(`${e}`);
      E.view = null;
      $("spa-ex-review").hidden = true;
      return;
    }
    E.mic = mic;
    E.view = v;
    const img = new Image();
    img.onload = () => { E.img = img; draw(); };
    img.src = `data:image/png;base64,${v.png}`;
    $("spa-ex-review").hidden = false;
    const scores = v.places.map((q) => q.score).filter((s) => s >= 0);
    const cut = $("spa-ex-cutoff");
    cut.min = Math.min(0, ...scores).toFixed(3);
    cut.max = Math.max(v.suggested * 1.6, ...scores, 0.1).toFixed(3);
    cut.value = v.cutoff;
    $("spa-ex-cutval").textContent = fmt(v.cutoff, 3);
    $("spa-ex-name").textContent = `${mic + 1} / ${n} · ${W.name(pr.mics[mic].path)} · 这张留下 ${v.perMic[mic] ?? 0} 个`;
    $("spa-ex-sum").textContent = `所有显微图一共留下 ${v.kept} 个颗粒（按例子估的直径约 ${fmt(2 * v.radius * pr.optics.pixelA, 0)} Å）`;
  }

  function draw() {
    const v = E.view, img = E.img, c = $("spa-ex-canvas");
    if (!v || !img) return;
    c.width = img.width;
    c.height = img.height;
    const ctx = c.getContext("2d");
    ctx.drawImage(img, 0, 0);
    const r = v.radius / v.scale;
    ctx.lineWidth = 1.5;
    for (const q of v.places) {
      ctx.strokeStyle = q.kept ? (q.user === 1 ? "#1f9bff" : "#2ecc71") : "rgba(200,200,200,0.45)";
      ctx.beginPath();
      ctx.arc(q.x / v.scale, q.y / v.scale, r, 0, 2 * Math.PI);
      ctx.stroke();
    }
    ctx.strokeStyle = "#ff9f1a";
    ctx.lineWidth = 2.5;
    for (const e of v.examples) {
      ctx.beginPath();
      ctx.arc(e.x / v.scale, e.y / v.scale, e.radius / v.scale, 0, 2 * Math.PI);
      ctx.stroke();
    }
  }

  $("spa-ex-canvas").onclick = async (ev) => {
    const v = E.view, c = ev.currentTarget;
    if (!v || W.busy) return;
    const b = c.getBoundingClientRect();
    const x = ((ev.clientX - b.left) / b.width) * c.width * v.scale;
    const y = ((ev.clientY - b.top) / b.height) * c.height * v.scale;
    try { await invoke("spa_examples_set", { dir: p().dir, mic: E.mic, at: [x, y] }); } catch (e) { return W.status(`${e}`); }
    loadView(E.mic, false);
  };
  let cutTimer = 0;
  $("spa-ex-cutoff").oninput = (e) => {
    const c = +e.target.value;
    $("spa-ex-cutval").textContent = fmt(c, 3);
    if (E.view) { E.view.places.forEach((q) => { q.kept = q.user === 1 || (q.user === 0 && q.score >= c && q.score > q.neg); }); draw(); }
    clearTimeout(cutTimer);
    cutTimer = setTimeout(async () => {
      try { await invoke("spa_examples_set", { dir: p().dir, cutoff: c }); } catch (_) { return; }
      loadView(E.mic, true);
    }, 250);
  };
  $("spa-ex-reset").onclick = async () => {
    if (!E.view) return;
    try { await invoke("spa_examples_set", { dir: p().dir, cutoff: E.view.suggested }); } catch (e) { return W.status(`${e}`); }
    loadView(E.mic, false);
  };
  $("spa-ex-prev").onclick = () => loadView(E.mic - 1, false);
  $("spa-ex-next").onclick = () => loadView(E.mic + 1, false);

  let lastDir = "";
  W.examples = { reload };
  W.hooks.set("examples", {
    show(pr) {
      if (!pr) { E.layers = []; E.view = null; $("spa-ex-review").hidden = true; render(); return; }
      if (pr.dir !== lastDir) { lastDir = pr.dir; E.view = null; E.ticked = false; E.ticks.clear(); $("spa-ex-review").hidden = true; reload(); }
      else render();
    },
  });
  document.addEventListener("pro-labels-saved", () => { if (!W.panel.hidden) reload(); });
  new MutationObserver(() => { if (!W.panel.hidden) reload(); }).observe(W.panel, { attributes: true, attributeFilter: ["hidden"] });
})();
