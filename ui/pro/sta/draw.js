"use strict";
// "自己标注" in the subtomogram-averaging workspace: the user marks on the
// tomogram open in the viewer what to average, and one click averages it.
// Five tools, each a click or a few: 点 (every click is one particle), 线
// (a filament: points along it), 膜 (drag a scribble over a membrane: the
// program takes the whole membrane under it from the automatic membrane
// segmentation, see src-tauri/src/sta/paint.rs), 面 (a membrane drawn by
// hand: points on it, the fallback), 球 (a vesicle: the centre, then a point
// on its surface). The software completes the shape and the size (see
// src-tauri/src/sta/marks.rs); nothing is typed.
// 点 alone: click 2–3 particles (one is allowed, more is fine) and the program
// finds all the others like them (their directions are unknown, so no shape is
// drawn) and SHOWS them before averaging (found.js: 找出全部同类); points and
// shapes are averaged separately.
// A mark is {kind: "point"|"line"|"surface"|"sphere"|"membrane", tomogram,
// points: [[x, y, z]…], flip, strokes: [{points, cut}…]} (strokes: membranes
// only, each on one section; a cut stroke ends a sheet where two membranes
// are joined: Alt-drag on a selected membrane) in voxels of the file that was open when it was clicked (the tomogram or
// its clear companion: the backend converts). Marks are kept in the one
// <out>/marks.json of the tomograms' output folder, and are drawn on the
// open file converted (a mark made on the look file shows on the tomogram and
// the other way round). Touch rule: no long press; tap a mark in the list to
// select it, then use the delete button.
// A surface, sphere or membrane has a side that faces out (every particle starts along
// the normal, as in Dynamo): the 翻面 button flips it, and arrows on the image
// (sta_mark_preview: sampled points and axes in the open file's voxels) show
// where particles will be taken and which way they face.
// Membranes need the automatic segmentation of the open file: choosing 膜
// checks it (sta_membrane_status) and runs it once if the model is there
// (sta_membrane_segment). The membrane a mark takes is filled on each section
// (sta_mark_section: runs [y, x0, x1) of the voxels on that section).
// Painted membranes, marked spheres and drawn lines are EXAMPLES: 找出全部同类 finds
// all of that kind in every ticked tomogram and shows them for checking
// (shapes.js, sta::shapes); 点 does the same for particles (found.js). Hand-drawn
// surfaces alone keep the direct run 「平均我标的」.
// Backend: sta_run_drawn, sta_marks_load, sta_marks_save, sta_mark_preview,
// sta_membrane_status, sta_membrane_segment, sta_mark_section. Every call is
// guarded: under the web demo they are missing and nothing is saved.
// Shares app.js globals: S, ext, $, invoke, setTool, toScreen, draw, toast.
(() => {
  const W = window.TifMrcViewPro.sta;
  if (!W) return;
  const D = W.draw = { marks: [], size: "medium", sel: null, open: null, kind: null, loaded: "", preview: [] };
  const KINDS = {
    point: { name: "点", color: "#0969da", hint: "在 2–3 个颗粒的中心各点一下，程序会找出其余的同类颗粒给你看，你确认后才平均" },
    line: { name: "线", color: "#2da44e", hint: "沿着丝状物依次点几个点，点完按“完成这一条”" },
    membrane: { name: "膜", color: "#d6336c", title: "在膜上涂一笔，自动选中整片膜", hint: "在膜上拖着涂一笔，程序会选中整片膜；涂完的这一片保持选中，再涂就加一片（要另起一组，先在列表里点一下取消选中）。两片膜连在一起时，按住 Alt 在连接处涂一笔把它们切开" },
    surface: { name: "面", color: "#bf8700", title: "面（手画）：自动找膜用不了或找得不对时，自己在膜上点几个点", hint: "在膜上点几个点（可以换几层再点），点完按“完成这一片”" },
    sphere: { name: "球", color: "#8250df", hint: "先点球的中心，再点球面上的一点（可以在另一层）" },
  };
  const ICONS = {
    point: '<circle cx="12" cy="12" r="3.2" fill="currentColor"/><circle cx="12" cy="12" r="7.5"/>',
    line: '<path d="M4 18C9 18 9 6 14 6S18 12 20 6"/><circle cx="4" cy="18" r="1.6" fill="currentColor"/><circle cx="14" cy="6" r="1.6" fill="currentColor"/><circle cx="20" cy="6" r="1.6" fill="currentColor"/>',
    membrane: '<path d="M3 13c3-5 6-5 9-1s6 3 9-3"/><path d="M5 19c4-2 9-2 14 0" stroke-dasharray="2 3"/>',
    surface: '<path d="M3 9c4-4 6 2 9-1s6-2 9 1v8c-3-3-6-4-9-1S7 13 3 17z"/>',
    sphere: '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="1.6" fill="currentColor"/><path d="M12 12h8"/>',
  };
  // A 点 run needs one click at least; two or three examples are more exact.
  const GOOD_POINTS = 2;
  const SIZES = [["small", "小"], ["medium", "中"], ["large", "大"]];

  const box = document.createElement("div");
  box.id = "sta-draw";
  box.innerHTML = `
    <h4>自己标注</h4>
    <p class="dim">程序找不到、或者不是你想要的？自己在断层图上标出来：选一个工具，在图上点几下，其余的交给程序。</p>
    <div id="sta-draw-tools" class="al-row"></div>
    <p id="sta-draw-hint" class="dim"></p>
    <p id="sta-draw-membrane" class="dim" hidden></p>
    <p id="sta-draw-side" class="dim" hidden>箭头指向颗粒朝外的一侧；不对就点「翻面」</p>
    <div id="sta-draw-sizes" class="al-row" hidden><span class="dim" title="线和面看不出颗粒多大，请选最接近的一个">颗粒大小</span></div>
    <div id="sta-draw-list" class="sta-marks"></div>
    <div class="al-row">
      <button id="sta-draw-done" class="small" hidden>完成</button>
      <button id="sta-draw-undo" class="small" title="去掉最后点的一个点">撤销一个点</button>
      <button id="sta-draw-del" class="small" title="先在列表里点一下要删的标注">删除所选</button>
      <button id="sta-draw-flip" class="small" title="把朝外的一侧换到另一边" hidden>翻面</button>
    </div>
    <div class="al-row"><button id="sta-draw-run" class="primary">平均我标的</button></div>`;
  $("sta-draw-slot").append(box);
  const tools = $("sta-draw-tools");
  const btn = {};
  for (const [k, v] of Object.entries(KINDS)) {
    const b = document.createElement("button");
    b.className = "small";
    b.title = v.title || `${v.name}：${v.hint}`;
    b.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${ICONS[k]}</svg><span>${v.name}</span>`;
    b.onclick = () => setKind(D.kind === k ? null : k);
    tools.append(b);
    btn[k] = b;
  }
  const sizes = {};
  for (const [k, label] of SIZES) {
    const b = document.createElement("button");
    b.className = "small";
    b.textContent = label;
    // 小/中/大 stay as they are in every interface language (the word list turns a lone 中 into "Medium").
    b.setAttribute("data-no-i18n", "");
    b.onclick = () => { D.size = k; save(); render(); };
    $("sta-draw-sizes").append(b);
    sizes[k] = b;
  }

  const count = (m) => m.points.length;
  const taking = (m) => (m.strokes || []).filter((s) => !s.cut).length;
  const usable = (m) => (m.kind === "membrane" ? taking(m) > 0 : m.kind !== "point" && count(m) >= 2);
  // A painted membrane or a complete sphere is an example of what to find (shapes.js).
  // A complete line (a filament) is one too: all filaments like it are found.
  const isExample = (m) => (m.kind === "membrane" && taking(m) > 0) || ((m.kind === "sphere" || m.kind === "line") && count(m) >= 2);
  const hasSide = (m) => m && (m.kind === "surface" || m.kind === "sphere" || m.kind === "membrane");
  const label = (m, i) => {
    const n = D.marks.slice(0, i + 1).filter((x) => x.kind === m.kind).length;
    const name = `${KINDS[m.kind].name} ${n}`;
    if (m.kind === "membrane") {
      const cuts = (m.strokes || []).length - taking(m);
      return `${name} · ${taking(m)} 笔${cuts ? `，切 ${cuts} 处` : ""}`;
    }
    if (m.kind === "sphere") return `${name} · ${count(m) >= 2 ? "已画好" : "还差球面上的一点"}`;
    if (m.kind !== "point" && count(m) < 2) return `${name} · ${count(m)} 个点（至少 2 个）`;
    return `${name} · ${count(m)} 个点`;
  };

  const plain = () => D.marks.map(({ kind, tomogram, points, flip, strokes }) => ({ kind, tomogram, points, flip: !!flip, strokes: strokes || [] }));
  // The sampled points of the shapes, for the arrows; the latest answer wins.
  let timer = 0, asked = 0;
  function refresh(wait = 0) {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const mine = ++asked;
      const open = S.info?.path;
      if (!open || !D.marks.some(usable)) { D.preview = []; draw(); return; }
      try {
        const r = await invoke("sta_mark_preview", { marks: plain(), tomogram: open, size: D.size });
        if (mine === asked) { D.preview = Array.isArray(r) ? r : []; render(); }
      } catch (_) { if (mine === asked) { D.preview = []; render(); } /* no backend (demo) */ }
    }, wait);
  }
  D.refresh = refresh;
  async function save() {
    sections(true);
    if (!W.out) return;
    try { await invoke("sta_marks_save", { out: W.out, marks: plain(), size: D.size }); } catch (_) { /* no backend (demo) */ }
    refresh(300);
  }
  async function load(out) {
    if (!out || out === D.loaded) return;
    D.loaded = out;
    D.marks = [];
    D.sel = D.open = null;
    render();
    try {
      const r = await invoke("sta_marks_load", { out });
      D.marks = (r.marks || []).map((m) => ({ ...m, strokes: m.strokes || [] }));
      sections(true);
      D.size = r.size || "medium";
      D.sel = D.open = null;
      render();
      refresh();
    } catch (_) { /* no backend (demo) */ }
  }

  // The membrane each membrane mark takes, on the section shown: runs
  // [y, x0, x1) from sta_mark_section, kept per (file, mark, section) until
  // the marks change. Asked ~120 ms after the section stops changing; an
  // answer to an older state is dropped.
  const secs = new Map();
  let secGen = 0, secTimer = 0, secWant = "";
  function sections(clear) {
    if (clear) { secs.clear(); secGen++; secWant = ""; clearTimeout(secTimer); secTimer = 0; }
    const open = S.info?.path, z = S.z;
    if (!open || S.axis !== "xy") return;
    const need = [];
    D.marks.forEach((m, i) => { if (m.kind === "membrane" && m.tomogram === open && taking(m) && !secs.has(`${open}|${i}|${z}`)) need.push(i); });
    if (!need.length) return;
    const want = `${secGen}|${open}|${z}|${need.join()}`;
    if (want === secWant) return;
    secWant = want;
    clearTimeout(secTimer);
    secTimer = setTimeout(async () => {
      secTimer = 0;
      const gen = secGen;
      const marks = plain();
      await Promise.all(need.map(async (i) => {
        let runs = [];
        try { const r = await invoke("sta_mark_section", { marks, mark: i, tomogram: open, z }); runs = Array.isArray(r) ? r : []; } catch (_) { /* no backend (demo) */ }
        if (gen !== secGen) return;
        if (secs.size > 600) secs.clear();
        secs.set(`${open}|${i}|${z}`, runs);
        if (S.z === z && S.info?.path === open) draw();
      }));
      if (gen === secGen && secWant === want) secWant = "";
    }, 120);
  }

  // What painting membranes on the open file needs: its segmentation. `have`
  // says per file whether it is there (null: asking); once started, a
  // segmentation is not repeated for a file unless the tool is chosen again.
  const M = D.membrane = { have: {}, model: true, seg: null, line: "", tried: new Set() };
  const NO_MODEL = "还没有膜分割模型：请先在标签面板的「自动分割膜」里载入一次权重";
  async function checkMembrane() {
    const open = S.info?.path;
    if (D.kind !== "membrane" || !open || M.seg || M.have[open] === true) return render();
    let st;
    try { st = await invoke("sta_membrane_status", { tomogram: open }); } catch (_) { return render(); /* no backend (demo) */ }
    if (D.kind !== "membrane" || S.info?.path !== open || M.seg) return;
    M.have[open] = !!st?.ready;
    M.model = st?.model !== false;
    M.line = "";
    if (!M.have[open] && M.model && !M.tried.has(open)) return segment(open);
    if (!M.have[open] && !M.model) M.line = NO_MODEL;
    render();
  }
  async function segment(open) {
    M.tried.add(open);
    M.seg = open;
    M.line = "正在自动找膜…（只需一次）";
    render();
    let off = () => {};
    try {
      const un = await T.event.listen("sta-progress", (e) => {
        const p = e.payload || {};
        M.line = `正在自动找膜…（只需一次）${p.label ? ` ${p.label}` : ""}${p.total > 1 ? ` ${p.done}/${p.total}` : ""}`;
        render();
      });
      off = un;
    } catch (_) { /* no events in the demo */ }
    try {
      const r = await invoke("sta_membrane_segment", { tomogram: open });
      M.have[open] = r?.ready !== false;
      M.line = M.have[open] ? "" : "没有找到膜：这张断层图里可能没有膜";
    } catch (e) {
      M.line = `找膜失败：${e}`;
    }
    try { off(); } catch (_) { /* gone */ }
    M.seg = null;
    sections(true);
    refresh(0);
    render();
    checkMembrane();
  }
  const canPaint = () => D.kind === "membrane" && !M.seg && !W.busy && M.have[S.info?.path] === true;

  function setKind(k) {
    D.kind = k;
    D.open = null;
    if (k === "membrane") { M.tried.delete(S.info?.path); M.line = ""; checkMembrane(); }
    if (k) { if (S.tool !== "mark") setTool("mark"); } else if (S.tool === "mark") setTool("select");
    render();
  }

  function render() {
    for (const [k, b] of Object.entries(btn)) { b.classList.toggle("on", D.kind === k); b.disabled = W.busy; }
    // While marking, and once there are marks, the panel makes room: the image has to be visible to click on.
    W.panel.classList.toggle("marking", !!D.kind || D.marks.length > 0);
    const pointsN = D.marks.filter((m) => m.kind === "point").reduce((n, m) => n + count(m), 0);
    const shapes = D.marks.some(usable);
    const mixed = pointsN > 0 && D.marks.some((m) => m.kind !== "point");
    const few = pointsN > 0 && pointsN < GOOD_POINTS && !D.marks.some((m) => m.kind !== "point");
    // Membranes and vesicles are examples: all of that kind are found and shown first (shapes.js).
    const examples = D.marks.filter(isExample).length;
    const fewShapes = examples === 1;
    $("sta-draw-hint").textContent = [D.kind ? KINDS[D.kind].hint : mixed ? "点和线、面、球、膜要分开平均：这次只平均线、面、球、膜，点先不算" : "", few ? "再点 1–2 个会更准" : "", fewShapes ? "再标 1–2 个会更准" : ""].filter(Boolean).join("。");
    const ml = $("sta-draw-membrane");
    ml.textContent = D.kind === "membrane" ? M.line : "";
    ml.hidden = !ml.textContent;
    const needSize = D.marks.some((m) => m.kind === "line" || m.kind === "surface" || m.kind === "membrane");
    $("sta-draw-sizes").hidden = !needSize;
    for (const [k, b] of Object.entries(sizes)) b.classList.toggle("on", D.size === k);
    const open = D.open != null ? D.marks[D.open] : null;
    const done = $("sta-draw-done");
    done.hidden = !(open && (open.kind === "line" || open.kind === "surface" || open.kind === "point"));
    done.textContent = open?.kind === "surface" ? "完成这一片" : open?.kind === "line" ? "完成这一条" : "完成这一组";
    const list = $("sta-draw-list");
    list.replaceChildren(...D.marks.map((m, i) => {
      const row = document.createElement("div");
      row.className = `sta-mark${D.sel === i ? " on" : ""}`;
      row.textContent = label(m, i);
      if (m.kind === "membrane" && D.preview.find((p) => p.mark === i)?.voxels === 0 && !M.seg) {
        const w = document.createElement("span");
        w.className = "warn";
        w.textContent = "没有碰到膜";
        row.append(" ", w);
      }
      row.style.setProperty("--mark", KINDS[m.kind].color);
      row.title = m.tomogram;
      row.onclick = () => { D.sel = D.sel === i ? null : i; render(); draw(); };
      return row;
    }));
    $("sta-draw-del").disabled = D.sel == null || W.busy;
    const side = D.sel != null && hasSide(D.marks[D.sel]) ? D.marks[D.sel] : null;
    $("sta-draw-flip").hidden = !side;
    $("sta-draw-flip").classList.toggle("on", !!side?.flip);
    $("sta-draw-flip").disabled = W.busy;
    $("sta-draw-side").hidden = !side;
    $("sta-draw-undo").disabled = !D.marks.length || W.busy;
    const last = D.marks[D.open != null ? D.open : D.marks.length - 1];
    $("sta-draw-undo").title = last?.kind === "membrane" ? "去掉最后涂的一笔" : "去掉最后点的一个点";
    const pointsOnly = pointsN > 0 && D.marks.every((m) => m.kind === "point");
    const ok = shapes || pointsOnly;
    // Points alone find everything like them first; shapes are averaged directly.
    const finds = (pointsOnly && !shapes) || examples > 0;
    $("sta-draw-run").textContent = finds ? "找出全部同类" : "平均我标的";
    $("sta-draw-run").disabled = !ok || W.busy || !!M.seg;
    $("sta-draw-run").title = ok ? (pointsOnly && pointsN < GOOD_POINTS ? "再点 1–2 个会更准" : fewShapes ? "再标 1–2 个会更准" : "") : D.marks.length ? "线和面至少要点 2 个点，球要点中心和球面上的一点，膜要在膜上涂一笔" : "先在图上标一个东西";
    draw();
    W.wall?.render();
  }

  function add(p) {
    if (!D.kind || W.busy) return;
    if (!S.info || S.axis !== "xy" || S.info.sizeZ < 3) return W.status("请在断层图的 XY 截面上标注");
    // Pixel p covers [p, p + 1): the voxel's centre is its integer index.
    const v = [p[0] - 0.5, p[1] - 0.5, S.z].map((x) => Math.round(x * 100) / 100);
    if (v[0] < -0.5 || v[1] < -0.5 || v[0] > S.info.width - 0.5 || v[1] > S.info.height - 0.5) return;
    let m = D.open != null ? D.marks[D.open] : null;
    if (m && (m.kind !== D.kind || m.tomogram !== S.info.path)) m = null;
    if (!m) {
      m = { kind: D.kind, tomogram: S.info.path, points: [] };
      D.marks.push(m);
      D.open = D.marks.length - 1;
    }
    m.points.push(v);
    if (m.kind === "sphere" && m.points.length >= 2) D.open = null;
    D.sel = null;
    save();
    render();
  }

  $("sta-draw-done").onclick = () => { D.open = null; render(); };
  $("sta-draw-undo").onclick = () => {
    const i = D.open != null ? D.open : D.marks.length - 1;
    const m = D.marks[i];
    if (!m) return;
    if (m.kind === "membrane") m.strokes.pop(); else m.points.pop();
    if (m.kind === "membrane" ? !m.strokes.length : !m.points.length) {
      D.marks.splice(i, 1);
      D.preview = [];
      D.open = null;
    }
    D.sel = null;
    save();
    render();
  };
  $("sta-draw-del").onclick = () => {
    if (D.sel == null) return;
    D.marks.splice(D.sel, 1);
    D.preview = [];
    D.open = null;
    D.sel = null;
    save();
    render();
  };
  $("sta-draw-flip").onclick = () => {
    const m = D.sel != null ? D.marks[D.sel] : null;
    if (!hasSide(m)) return;
    m.flip = !m.flip;
    save();
    refresh(0);
    render();
  };
  $("sta-draw-run").onclick = () => {
    const marks = plain();
    const onlyPoints = marks.length > 0 && marks.every((m) => m.kind === "point");
    D.open = null;
    setKind(null);
    if (onlyPoints && W.found) return W.found.findPoints(marks);
    // Membranes and vesicles: find all of that kind, show them, average after the user confirms.
    if (W.shapes && marks.some(isExample)) return W.shapes.find(marks, D.size);
    W.go((inputs, out) => invoke("sta_run_drawn", { inputs, marks, out, size: D.size }), false);
  };

  // The viewer's tool: a tap (not a drag) adds a point; dragging still moves the image.
  let press = null;
  // The membrane tool: a drag is a freehand stroke on the section shown,
  // sampled about every 3 screen pixels, in voxels like clicks. Alt: a cut.
  let live = null;
  const voxel = (p) => [p[0] - 0.5, p[1] - 0.5, S.z].map((x) => Math.round(x * 100) / 100);
  function commitStroke(l) {
    if (!l.vox.length || !S.info) return;
    const stroke = { points: l.vox, cut: l.cut };
    let m = D.sel != null ? D.marks[D.sel] : null;
    if (m && (m.kind !== "membrane" || m.tomogram !== S.info.path)) m = null;
    if (l.cut) {
      if (!m) return W.status("先在列表里点选一片膜，再按住 Alt 涂一笔，把它从连接处切开");
      m.strokes.push(stroke);
    } else if (m) {
      m.strokes.push(stroke);
    } else {
      m = { kind: "membrane", tomogram: S.info.path, points: [], flip: false, strokes: [stroke] };
      D.marks.push(m);
      D.sel = D.marks.length - 1;
    }
    D.open = null;
    save();
    render();
  }
  ext.tools.mark = {
    down(e, p) {
      if (D.kind === "membrane") {
        if (!canPaint()) return;
        if (!S.info || S.axis !== "xy" || S.info.sizeZ < 3) return W.status("请在断层图的 XY 截面上标注");
        live = { x: e.clientX, y: e.clientY, vox: [voxel(p)], cut: !!e.altKey };
        draw();
        return;
      }
      press = { x: e.clientX, y: e.clientY, p, moved: false };
    },
    move(e, p) {
      if (live) {
        if (Math.hypot(e.clientX - live.x, e.clientY - live.y) >= 3) {
          live.x = e.clientX;
          live.y = e.clientY;
          live.vox.push(voxel(p));
          draw();
        }
        return;
      }
      if (press && Math.hypot(e.clientX - press.x, e.clientY - press.y) > 4) press.moved = true;
    },
    up() {
      const l = live;
      live = null;
      if (l) { commitStroke(l); draw(); return; }
      const t = press;
      press = null;
      if (t && !t.moved) add(t.p);
    },
    active(on) { if (!on && D.kind) { live = null; D.kind = null; D.open = null; render(); } },
  };

  // The points of a mark in voxels of the file open in the viewer: marks made
  // on the tomogram show on its look companion and the other way round
  // (raw = scale × look + offset, as the backend carries them over); null when
  // the mark belongs to a file that is neither.
  function onOpen(m) {
    const open = S.info?.path;
    if (m.tomogram === open) return m.points;
    for (const it of W.items) {
      if (!it.look || it.scale == null || !it.offset) continue;
      if (it.look === m.tomogram && it.path === open) return m.points.map((p) => p.map((v, k) => it.scale * v + it.offset[k]));
      if (it.path === m.tomogram && it.look === open) return m.points.map((p) => p.map((v, k) => (v - it.offset[k]) / it.scale));
    }
    return null;
  }
  D.onOpen = onOpen;

  // Marks over the image: faint on the slices near the clicked one.
  ext.overlays.push((ctx) => {
    // While the found membranes and vesicles are shown, the examples are among them.
    if (W.panel.hidden || !S.info || S.axis !== "xy" || W.shapes?.shown()) return;
    const sc = toScreen(1, 0)[0] - toScreen(0, 0)[0];
    const at = (p) => toScreen(p[0] + 0.5, p[1] + 0.5);
    const alpha = (dz) => (dz <= 1 ? 1 : dz <= 6 ? 0.35 : 0);
    ctx.save();
    ctx.lineJoin = "round";
    // The membranes the marks take, filled; the strokes only on their own section.
    sections(false);
    D.marks.forEach((mark, i) => {
      if (mark.kind !== "membrane" || mark.tomogram !== S.info.path) return;
      const col = KINDS.membrane.color;
      const runs = secs.get(`${S.info.path}|${i}|${S.z}`);
      if (runs?.length) {
        ctx.globalAlpha = D.sel === i ? 0.5 : 0.32;
        ctx.fillStyle = col;
        ctx.beginPath();
        for (const [y, x0, x1] of runs) {
          const [sx, sy] = toScreen(x0, y);
          ctx.rect(sx, sy, (x1 - x0) * sc, sc);
        }
        ctx.fill();
      }
      ctx.strokeStyle = col;
      ctx.lineWidth = D.sel === i ? 2 : 1.2;
      ctx.globalAlpha = 1;
      for (const st of mark.strokes || []) {
        if (!st.points.length || Math.abs(st.points[0][2] - S.z) >= 0.5) continue;
        ctx.setLineDash(st.cut ? [5, 4] : []);
        ctx.beginPath();
        st.points.forEach((p, k) => { const [x, y] = at(p); if (k) ctx.lineTo(x, y); else ctx.moveTo(x, y); });
        if (st.points.length === 1) { const [x, y] = at(st.points[0]); ctx.arc(x, y, 1.5, 0, 2 * Math.PI); }
        ctx.stroke();
      }
      ctx.setLineDash([]);
    });
    if (live) {
      ctx.globalAlpha = 1;
      ctx.strokeStyle = KINDS.membrane.color;
      ctx.lineWidth = 2;
      ctx.setLineDash(live.cut ? [5, 4] : []);
      ctx.beginPath();
      live.vox.forEach((p, k) => { const [x, y] = at(p); if (k) ctx.lineTo(x, y); else ctx.moveTo(x, y); });
      ctx.stroke();
      ctx.setLineDash([]);
    }
    D.marks.forEach((mark, i) => {
      if (mark.kind === "membrane") return;
      const pts = onOpen(mark);
      if (!pts) return;
      const m = { kind: mark.kind, points: pts };
      const col = KINDS[m.kind].color;
      const on = D.sel === i;
      const dot = (p, a) => {
        if (!a) return;
        const [x, y] = at(p);
        ctx.globalAlpha = a;
        ctx.beginPath();
        ctx.arc(x, y, on ? 5 : 3.5, 0, 2 * Math.PI);
        ctx.fillStyle = col;
        ctx.fill();
        ctx.lineWidth = 1.5;
        ctx.strokeStyle = "#fff";
        ctx.stroke();
      };
      ctx.lineWidth = on ? 3 : 1.8;
      ctx.strokeStyle = col;
      if (m.kind === "sphere") {
        const [c, s] = m.points;
        dot(c, alpha(Math.abs(c[2] - S.z)));
        if (!s) return;
        dot(s, alpha(Math.abs(s[2] - S.z)));
        const r = Math.hypot(s[0] - c[0], s[1] - c[1], s[2] - c[2]);
        const dz = S.z - c[2];
        if (Math.abs(dz) < r) {
          const [x, y] = at(c);
          ctx.globalAlpha = 1;
          ctx.beginPath();
          ctx.arc(x, y, Math.sqrt(r * r - dz * dz) * sc, 0, 2 * Math.PI);
          ctx.stroke();
        }
        return;
      }
      if (m.kind !== "point") {
        for (let k = 1; k < m.points.length; k++) {
          const a = alpha(Math.max(Math.abs(m.points[k][2] - S.z), Math.abs(m.points[k - 1][2] - S.z)));
          if (!a) continue;
          const [x0, y0] = at(m.points[k - 1]), [x1, y1] = at(m.points[k]);
          ctx.globalAlpha = a;
          ctx.setLineDash(m.kind === "surface" ? [5, 4] : []);
          ctx.beginPath();
          ctx.moveTo(x0, y0);
          ctx.lineTo(x1, y1);
          ctx.stroke();
        }
        ctx.setLineDash([]);
      }
      for (const p of m.points) dot(p, alpha(Math.abs(p[2] - S.z)));
    });
    // The sampled points as short arrows (a line with a dot at the tip) along
    // the axis seen from above: the way each particle faces.
    ctx.lineCap = "round";
    ctx.setLineDash([]);
    for (const pv of D.preview) {
      const mark = D.marks[pv.mark];
      if (!mark || mark.kind !== pv.kind) continue;
      ctx.strokeStyle = ctx.fillStyle = KINDS[mark.kind].color;
      ctx.lineWidth = D.sel === pv.mark ? 2 : 1.4;
      for (const s of pv.seeds) {
        const a = alpha(Math.abs(s.pos[2] - S.z));
        if (!a) continue;
        const [x, y] = at(s.pos);
        const flat = Math.hypot(s.axis[0], s.axis[1]);
        const tx = flat ? x + 16 * s.axis[0] : x, ty = flat ? y + 16 * s.axis[1] : y;
        ctx.globalAlpha = a;
        ctx.beginPath();
        ctx.moveTo(x, y);
        ctx.lineTo(tx, ty);
        ctx.stroke();
        ctx.beginPath();
        ctx.arc(tx, ty, 2.5, 0, 2 * Math.PI);
        ctx.fill();
      }
    }
    ctx.restore();
  });
  document.addEventListener("tmv-opened", () => { draw(); refresh(0); if (D.kind === "membrane") checkMembrane(); });

  document.addEventListener("sta-out", (e) => { load(e.detail); draw(); });
  document.addEventListener("sta-busy", () => render());
  // Closing the workspace puts the tool away.
  new MutationObserver(() => { if (W.panel.hidden && D.kind) setKind(null); }).observe(W.panel, { attributes: true, attributeFilter: ["hidden"] });
  render();
})();
