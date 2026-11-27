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
// When only membrane layers are ticked, 平均膜本身 averages the membrane itself
// (sta_membrane_average, sta/bilayer.rs): the membranes are cut into small patches, the ones whose
// profile across the membrane does not look like the others are left out, and the rest give the
// profile (thickness, leaflets, denser side) and the average patch. The check list shows one row per
// membrane (kept / left out and why, agreement, a warning when its inside and outside look swapped)
// with 翻面 and 去掉; 按修改重新平均 runs again with those edits. Every patch is drawn on the image
// (green kept, red unlike, orange off the membrane, blue hidden by the missing wedge, grey taken out).
// Shares app.js globals: S, $, invoke, openPath, setTool, ext, toScreen, draw, T.
(() => {
  const W = window.TifMrcViewPro.sta;
  if (!W?.draw) return;
  const D = W.draw;
  const TYPE = { particle: "小颗粒", ball: "球", shell: "空心球", membrane: "单层膜", membrane2: "双层膜", fibre: "纤维", free: "不规则", "": "未选类型" };
  const FAMILY = { particle: "颗粒/例子", ball: "颗粒/例子", free: "颗粒/例子", "": "颗粒/例子", shell: "囊泡", membrane: "膜", membrane2: "膜", fibre: "纤维" };
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
    <p id="sta-layers-warn" class="warn" hidden></p>
    <p id="sta-layers-note" class="dim"></p>
    <div class="al-row"><button id="sta-layers-use" class="primary" disabled>用勾选的图层</button><button id="sta-layers-membrane" hidden title="平均膜本身：把勾选的膜切成小片，去掉不像膜的，平均出膜的横截面（膜厚、双层、哪侧更密）">平均膜本身</button><button id="sta-layers-reload" class="small" title="标签画笔改过之后，重新读一遍各断层图的图层">刷新图层</button></div>
    <div id="sta-membrane" class="sta-membrane" hidden>
      <h4 id="sta-membrane-head"></h4>
      <p id="sta-membrane-sum" class="dim"></p>
      <div id="sta-membrane-chart"></div>
      <p class="dim sta-found-legend">图上每个点是一小片膜：<span style="color:#1a7f37">●</span> 留下 <span style="color:#cf222e">●</span> 不像膜 <span style="color:#bc4c00">●</span> 偏离膜 <span style="color:#0969da">●</span> 缺失楔方向看不见 <span style="color:#8c959f">●</span> 你去掉的</p>
      <ul id="sta-membrane-rows" class="sta-membrane-rows"></ul>
      <p id="sta-membrane-msg" class="dim"></p>
      <div class="al-row"><button id="sta-membrane-again" class="primary" hidden>按修改重新平均</button><button id="sta-membrane-open" class="small">打开平均膜片</button></div>
    </div>`;
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
    const makeRow = (it, mine) => {
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
      };
      head.append(title, open);
      row.append(head);
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
    };
    const withLayers = [], withoutLayers = [];
    for (const it of W.items) {
      const mine = L.layers.filter((l) => l.image === it.path || l.image === it.look);
      if (mine.length) withLayers.push([it, mine]); else withoutLayers.push([it, mine]);
    }
    const nodes = withLayers.map(([it, mine]) => makeRow(it, mine));
    if (withoutLayers.length) {
      // Avoid duplicating a long wall of unlabelled tomograms when some already have labels.
      if (withLayers.length && withoutLayers.length > 2) {
        const det = document.createElement("details");
        det.className = "sta-layers-empty";
        const sum = document.createElement("summary");
        sum.textContent = `还有 ${withoutLayers.length} 张断层图尚无标签（展开标注）`;
        det.append(sum, ...withoutLayers.map(([it, mine]) => makeRow(it, mine)));
        nodes.push(det);
      } else {
        nodes.push(...withoutLayers.map(([it, mine]) => makeRow(it, mine)));
      }
    }
    list.replaceChildren(...nodes);
    // Warn immediately when incompatible structure families are ticked together.
    const tickedLayers = L.layers.filter((l) => ticks.has(key(l)));
    const families = [...new Set(tickedLayers.map((l) => FAMILY[l.structure] || "颗粒/例子"))];
    const hasParticles = families.includes("颗粒/例子");
    const hasShapes = families.some((f) => f !== "颗粒/例子");
    const warn = $("sta-layers-warn");
    if (hasParticles && hasShapes) {
      warn.textContent = `同时勾选了「${families.join("」和「")}」：颗粒和膜/囊泡/纤维需分开平均，建议每次只勾同一类结构。`;
      warn.hidden = false;
    } else {
      warn.textContent = "";
      warn.hidden = true;
    }
    const n = ticks.size;
    $("sta-layers-use").disabled = !n || L.busy || W.busy;
    const onlyMembranes = tickedLayers.length > 0 && tickedLayers.every((l) => FAMILY[l.structure] === "膜");
    $("sta-layers-membrane").hidden = !onlyMembranes;
    $("sta-layers-membrane").disabled = !onlyMembranes || L.busy || W.busy;
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

  // ---- 平均膜本身 ----
  const COLORS = ["#1a7f37", "#cf222e", "#bc4c00", "#0969da", "#8c959f", "#8c959f"];
  const M = { r: null, ticked: null, out: "", flip: new Set(), drop: new Set() };
  const fmt = (v, d = 0) => (v == null ? "?" : Number(v).toFixed(d));

  async function averageMembranes(ticked, edits) {
    if (L.busy || W.busy) return;
    L.busy = true;
    W.busy = true;
    render();
    document.dispatchEvent(new CustomEvent("sta-busy", { detail: true }));
    const t0 = performance.now();
    const msg = $("sta-membrane-msg");
    $("sta-membrane").hidden = false;
    msg.textContent = "开始…";
    let unlisten = null;
    try {
      try {
        unlisten = await T.event.listen("sta-progress", (e) => {
          const p = e.payload, sec = (performance.now() - t0) / 1000;
          msg.textContent = `${p.label}${p.total > 1 ? ` ${p.done}/${p.total}` : ""} · ${sec.toFixed(0)} s`;
        });
      } catch (_) { /* no events in the demo */ }
      let out = W.out;
      try { out = await invoke("sta_out", { first: W.items[0].path }); } catch (_) { /* demo */ }
      const r = await invoke("sta_membrane_average", { ticked, out, edits });
      M.r = r;
      M.ticked = ticked;
      M.out = out;
      M.flip = new Set(r.membranes.map((m, i) => (m.flipped ? i : -1)).filter((i) => i >= 0));
      M.drop = new Set(r.membranes.map((m, i) => (m.dropped ? i : -1)).filter((i) => i >= 0));
      msg.textContent = `完成 · ${((performance.now() - t0) / 1000).toFixed(0)} s`;
      for (const [image] of ticked) { const it = itemOf(image); if (it) W.cur().off.delete(it.path); }
    } catch (e) {
      msg.textContent = `${e}`.includes("已取消") ? "已取消" : `失败：${e}`;
    } finally {
      if (unlisten) try { unlisten(); } catch (_) { /* gone */ }
      L.busy = false;
      W.busy = false;
      document.dispatchEvent(new CustomEvent("sta-busy", { detail: false }));
      const note = msg.textContent;
      render();
      showMembranes();
      msg.textContent = note;
      if (typeof draw === "function") draw();
    }
  }

  // The profile across the membrane as a small chart: inside left, outside right, dense down.
  function chart(offsets, profile, title) {
    const w = 300, h = 120, pad = 18;
    const xs = offsets, ys = profile;
    const x0 = Math.min(...xs), x1 = Math.max(...xs), y0 = Math.min(...ys), y1 = Math.max(...ys);
    const X = (x) => pad + ((x - x0) / (x1 - x0 || 1)) * (w - 2 * pad);
    const Y = (y) => h - pad - ((y - y0) / (y1 - y0 || 1)) * (h - 2 * pad);
    const pts = xs.map((x, i) => `${X(x).toFixed(1)},${Y(ys[i]).toFixed(1)}`).join(" ");
    return `<svg viewBox="0 0 ${w} ${h}" class="sta-membrane-svg" role="img" aria-label="${title}">
      <line x1="${X(0)}" y1="${pad / 2}" x2="${X(0)}" y2="${h - pad}" class="axis"/>
      <polyline points="${pts}" class="curve"/>
      <text x="${pad}" y="${h - 4}" class="lab">内侧 ${x0.toFixed(0)} Å</text>
      <text x="${w - pad}" y="${h - 4}" class="lab" text-anchor="end">外侧 ${x1.toFixed(0)} Å</text>
      <text x="${w / 2}" y="11" class="lab" text-anchor="middle">${title}</text>
    </svg>`;
  }

  function showMembranes() {
    const r = M.r;
    if (!r) return;
    $("sta-membrane").hidden = false;
    const f = r.fine;
    const best = f || r;
    const side = { inside: "内侧", outside: "外侧", even: "两侧差不多" }[best.denserSide] || best.denserSide;
    $("sta-membrane-head").textContent = `膜本身：留下 ${r.kept} / ${r.patches} 片`;
    $("sta-membrane-sum").textContent =
      `膜厚约 ${fmt(best.thicknessA)} Å · ${best.leafletsA ? `双层间距 ${fmt(best.leafletsA)} Å` : "双层没有分开"} · 更密的一侧：${side}` +
      (f ? ` · 从倾斜图像平均（${fmt(f.voxelA, 1)} Å 体素${f.resolutionA ? `，约 ${fmt(f.resolutionA)} Å` : ""}）` : ` · 断层图上（${fmt(r.voxelA, 1)} Å 体素）`);
    $("sta-membrane-chart").innerHTML = chart(best.offsetsA, best.profile, "横穿膜的密度（越低越密）");
    const rows = r.membranes.map((m, i) => {
      const li = document.createElement("li");
      const dropped = M.drop.has(i), flipped = M.flip.has(i);
      const text = document.createElement("span");
      const out = [m.unlike && `不像膜 ${m.unlike}`, m.off && `偏离 ${m.off}`, m.hidden && `看不见 ${m.hidden}`].filter(Boolean).join("，");
      text.textContent = `膜 ${i + 1} · ${m.layer} · 第 ${m.z[0] + 1}${m.z[1] > m.z[0] ? `–${m.z[1] + 1}` : ""} 层 · 留 ${m.kept}/${m.patches}${out ? `（${out}）` : ""} · 吻合 ${fmt(m.score, 2)}`;
      if (dropped) text.className = "dim";
      li.append(text);
      if (m.sideDoubt && !flipped) {
        const w = document.createElement("span");
        w.className = "warn";
        w.textContent = " 内外可能标反了";
        li.append(w);
      }
      const flip = document.createElement("button");
      flip.className = "small";
      flip.textContent = flipped ? "已翻面" : "翻面";
      flip.title = "把这张膜的内外对调";
      flip.onclick = () => { if (M.flip.has(i)) M.flip.delete(i); else M.flip.add(i); showMembranes(); };
      const drop = document.createElement("button");
      drop.className = "small";
      drop.textContent = dropped ? "放回来" : "去掉";
      drop.title = dropped ? "这张膜重新参与平均" : "这张膜不参与平均";
      drop.onclick = () => { if (M.drop.has(i)) M.drop.delete(i); else M.drop.add(i); showMembranes(); };
      li.append(flip, drop);
      return li;
    });
    $("sta-membrane-rows").replaceChildren(...rows);
    const changed = r.membranes.some((m, i) => m.flipped !== M.flip.has(i) || m.dropped !== M.drop.has(i));
    $("sta-membrane-again").hidden = !changed;
    $("sta-membrane-again").disabled = L.busy || W.busy;
    $("sta-membrane-open").hidden = !(f?.map || r.map);
  }

  $("sta-layers-membrane").onclick = () => {
    const ticks = ticksOf();
    const ticked = L.layers.filter((l) => ticks.has(key(l))).map((l) => [l.image, l.label]);
    if (ticked.length) averageMembranes(ticked, { flip: [], drop: [] });
  };
  $("sta-membrane-again").onclick = () => {
    if (M.ticked) averageMembranes(M.ticked, { flip: [...M.flip], drop: [...M.drop] });
  };
  $("sta-membrane-open").onclick = () => {
    const p = M.r?.fine?.map || M.r?.map;
    if (p) openPath(p).catch((e) => { $("sta-membrane-msg").textContent = `${e}`; });
  };

  // Every patch on the image it was labelled on, near the section shown.
  ext.overlays.push((ctx) => {
    const r = M.r;
    const path = S.info?.path;
    if (!r || !path || $("sta-membrane").hidden) return;
    const sc = toScreen(1, 0)[0] - toScreen(0, 0)[0];
    const rad = Math.max(2, Math.min(5, sc * 0.6));
    ctx.save();
    for (const [m, x, y, z, code] of r.patchList || []) {
      const mem = r.membranes[m];
      if (!mem || mem.image !== path || Math.abs(z - S.z) > 1) continue;
      const c = M.drop.has(m) ? 5 : code;
      const [sx, sy] = toScreen(x, y);
      ctx.globalAlpha = Math.abs(z - S.z) <= 0.5 ? 0.95 : 0.45;
      ctx.fillStyle = COLORS[c] || COLORS[4];
      ctx.beginPath();
      ctx.arc(sx, sy, rad, 0, 2 * Math.PI);
      ctx.fill();
    }
    ctx.restore();
  });

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
