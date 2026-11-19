"use strict";
// What to average is marked with the viewer's one label brush (labels.js), not with tools of this
// workspace. The section first says how (open a tomogram, paint a layer per thing, give it a
// structure type, 找同类 / 沿 Z), then lists every tomogram of the wall with the layers of its label
// file (sta_label_layers: name, type, how many objects) as tick boxes. The user ticks what to average
// (say only the membranes of ten tomograms); 用勾选的图层 makes the marks of those layers
// (sta_layers_marks, sta/layers.rs: 小颗粒/球 → points, 空心球 → spheres, 膜 → surfaces with the
// outward side the user named, 纤维 → lines, 不规则 → examples) and hands them to draw.js, whose list
// and 找出全部同类 / 平均我标的 do the rest on the raw tomograms. A tomogram whose layer is ticked is
// ticked on the wall too. Ticks are kept per output folder while the app runs.
// Shares app.js globals: S, $, invoke, openPath, setTool.
(() => {
  const W = window.TifMrcViewPro.sta;
  if (!W?.draw) return;
  const D = W.draw;
  const TYPE = { particle: "小颗粒", ball: "球", shell: "空心球", membrane: "单层膜", membrane2: "双层膜", fibre: "纤维", free: "不规则", "": "未选类型" };
  const WHAT = {
    particle: (l) => `${l.objects} 个`, ball: (l) => `${l.objects} 个`, shell: (l) => `${l.objects} 个`, free: (l) => `${l.objects} 个`, "": (l) => `${l.objects} 块`,
    membrane: (l) => `${l.lines} 条膜线`, membrane2: (l) => `${l.lines} 条膜线`, fibre: (l) => `${l.lines} 条`,
  };
  const name = (p) => p?.split(/[\\/]/).pop() || "";
  const L = { layers: [], ticks: new Map(), gen: 0, busy: false };
  const key = (l) => `${l.image}\n${l.label}`;
  const ticksOf = () => { if (!L.ticks.has(W.out)) L.ticks.set(W.out, new Set()); return L.ticks.get(W.out); };

  const box = document.createElement("div");
  box.id = "sta-layers";
  box.innerHTML = `
    <h4>① 用标签画笔标出要平均的东西</h4>
    <ol class="dim sta-teach">
      <li>点下面一张断层图的「打开并标注」，它会在查看器里打开（有清晰版就开清晰版），并换成标签画笔。</li>
      <li>每种东西一个图层：涂几下，再点结构类型（小颗粒、空心球、单层膜、纤维…）让程序贴到真实的边上；膜要再点一下「膜内」，告诉程序哪边朝里。</li>
      <li>想要同一张图里所有同样的：用标签画笔的「找同类」，再「沿 Z」带到上下层。</li>
      <li>回到这里，勾选要平均的图层（可以跨好几张断层图，比如只勾膜），按「用勾选的图层」。</li>
    </ol>
    <div id="sta-layers-list" class="sta-layers"></div>
    <p id="sta-layers-note" class="dim"></p>
    <div class="al-row"><button id="sta-layers-use" class="primary" disabled>用勾选的图层</button><button id="sta-layers-reload" class="small" title="标签画笔改过之后，重新读一遍各断层图的图层">刷新图层</button></div>`;
  $("sta-draw-slot").before(box);
  $("sta-draw-undo").hidden = true;

  function itemOf(image) {
    return W.items.find((i) => i.path === image || i.look === image);
  }

  async function reload() {
    const gen = ++L.gen;
    const images = [];
    for (const it of W.items) {
      if (it.look) images.push(it.look);
      images.push(it.path);
    }
    if (!images.length) { L.layers = []; render(); return; }
    $("sta-layers-note").textContent = "正在读取各断层图的标签图层…";
    try {
      const layers = await invoke("sta_label_layers", { images });
      if (gen !== L.gen) return;
      L.layers = layers || [];
    } catch (_) {
      if (gen !== L.gen) return;
      L.layers = []; /* no backend (web demo) */
    }
    render();
  }

  function render() {
    const list = $("sta-layers-list");
    const ticks = ticksOf();
    // Ticks of layers that are gone are dropped.
    for (const k of [...ticks]) if (!L.layers.some((l) => key(l) === k)) ticks.delete(k);
    list.replaceChildren(...W.items.map((it) => {
      const row = document.createElement("div");
      row.className = "sta-layer-item";
      const head = document.createElement("div");
      head.className = "al-row";
      const title = document.createElement("b");
      title.textContent = it.name || name(it.path);
      const open = document.createElement("button");
      open.className = "small";
      open.textContent = "打开并标注";
      open.title = it.look ? "在查看器里打开清晰版，换成标签画笔" : "在查看器里打开，换成标签画笔";
      open.onclick = async () => {
        try { await openPath(it.look || it.path); } catch (e) { $("sta-layers-note").textContent = `${e}`; return; }
        if (typeof setTool === "function") setTool("label");
        $("sta-fold")?.getAttribute("aria-expanded") === "true" && $("sta-fold").click();
      };
      head.append(title, open);
      row.append(head);
      const mine = L.layers.filter((l) => l.image === it.path || l.image === it.look);
      if (!mine.length) {
        const p = document.createElement("div");
        p.className = "dim";
        p.textContent = "还没有标签图层";
        row.append(p);
      }
      for (const l of mine) {
        const lab = document.createElement("label");
        lab.className = "sta-layer";
        const cb = document.createElement("input");
        cb.type = "checkbox";
        cb.checked = ticks.has(key(l));
        cb.disabled = L.busy || W.busy;
        cb.onchange = () => { if (cb.checked) ticks.add(key(l)); else ticks.delete(key(l)); render(); };
        const dot = document.createElement("span");
        dot.className = "sta-layer-dot";
        if (l.color) dot.style.background = `rgb(${l.color.join(",")})`;
        const text = document.createElement("span");
        text.textContent = `${l.name} · ${TYPE[l.structure] ?? l.structure} · ${(WHAT[l.structure] || WHAT[""])(l)}`;
        lab.append(cb, dot, text);
        row.append(lab);
      }
      return row;
    }));
    const n = ticks.size;
    $("sta-layers-use").disabled = !n || L.busy || W.busy;
    if (!L.busy) {
      $("sta-layers-note").textContent = !W.items.length ? "" : !L.layers.length ? "这些断层图上还没有标签图层：先按上面的步骤用标签画笔标一下。" : n ? `勾选了 ${n} 个图层` : "勾选要平均的图层";
    }
  }

  $("sta-layers-reload").onclick = reload;
  $("sta-layers-use").onclick = async () => {
    const ticks = ticksOf();
    const ticked = L.layers.filter((l) => ticks.has(key(l))).map((l) => [l.image, l.label]);
    if (!ticked.length) return;
    L.busy = true;
    render();
    $("sta-layers-note").textContent = "正在读取勾选的图层…";
    try {
      const marks = await invoke("sta_layers_marks", { ticked });
      // A tomogram whose layer is used takes part in the averaging.
      for (const [image] of ticked) { const it = itemOf(image); if (it) W.cur().off.delete(it.path); }
      D.setMarks(marks);
      W.wall?.render();
      $("sta-layers-note").textContent = `从 ${ticked.length} 个图层得到 ${marks.length} 个标注，见下面的列表`;
    } catch (e) {
      $("sta-layers-note").textContent = `${e}`;
    } finally {
      L.busy = false;
      const note = $("sta-layers-note").textContent;
      render();
      $("sta-layers-note").textContent = note;
    }
  };

  // The tomograms change with the folder; labels change while the user paints (reread when the panel shows again).
  let lastItems = "";
  W.layers = {
    refresh() {
      const sig = `${W.out}|${W.items.map((i) => i.path).join("|")}`;
      if (sig !== lastItems) { lastItems = sig; reload(); } else render();
    },
    reload,
  };
  document.addEventListener("pro-labels-saved", () => { if (!W.panel.hidden) reload(); });
  new MutationObserver(() => { if (!W.panel.hidden) reload(); }).observe(W.panel, { attributes: true, attributeFilter: ["hidden"] });
})();
