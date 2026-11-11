"use strict";
// "自己标注" in the subtomogram-averaging workspace: the user marks on the
// tomogram open in the viewer what to average, and one click averages it.
// Four tools, each a click or a few: 点 (every click is one particle), 线
// (a filament: points along it), 面 (a membrane: points on it), 球 (a
// vesicle: the centre, then a point on its surface). The software completes
// the shape and the size (see src-tauri/src/sta/marks.rs); nothing is typed.
// 点 alone: click at least MIN_POINTS particles and the program finds all the
// others like them (their directions are unknown, so no shape is drawn);
// points and shapes are averaged separately.
// A mark is {kind: "point"|"line"|"surface"|"sphere", tomogram, points: [[x, y, z]…]}
// in voxels of the file that was open when it was clicked (the tomogram or
// its clear companion: the backend converts). Marks are kept in the one
// <out>/marks.json of the tomograms' output folder, and are drawn on the
// open file converted (a mark made on the look file shows on the tomogram and
// the other way round). Touch rule: no long press; tap a mark in the list to
// select it, then use the delete button.
// Backend: sta_run_drawn, sta_marks_load, sta_marks_save. Every call is
// guarded: under the web demo they are missing and nothing is saved.
// Shares app.js globals: S, ext, $, invoke, setTool, toScreen, draw, toast.
(() => {
  const W = window.TifMrcViewPro.sta;
  if (!W) return;
  const D = W.draw = { marks: [], size: "medium", sel: null, open: null, kind: null, loaded: "" };
  const KINDS = {
    point: { name: "点", color: "#0969da", hint: "在几个颗粒的中心各点一下（至少 4 个，多点几个更准），程序会找出其余的同类颗粒" },
    line: { name: "线", color: "#2da44e", hint: "沿着丝状物依次点几个点，点完按“完成这一条”" },
    surface: { name: "面", color: "#bf8700", hint: "在膜上点几个点（可以换几层再点），点完按“完成这一片”" },
    sphere: { name: "球", color: "#8250df", hint: "先点球的中心，再点球面上的一点（可以在另一层）" },
  };
  const ICONS = {
    point: '<circle cx="12" cy="12" r="3.2" fill="currentColor"/><circle cx="12" cy="12" r="7.5"/>',
    line: '<path d="M4 18C9 18 9 6 14 6S18 12 20 6"/><circle cx="4" cy="18" r="1.6" fill="currentColor"/><circle cx="14" cy="6" r="1.6" fill="currentColor"/><circle cx="20" cy="6" r="1.6" fill="currentColor"/>',
    surface: '<path d="M3 9c4-4 6 2 9-1s6-2 9 1v8c-3-3-6-4-9-1S7 13 3 17z"/>',
    sphere: '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="1.6" fill="currentColor"/><path d="M12 12h8"/>',
  };
  // Fewest particles a 点 run needs (MIN_POINTS in src-tauri/src/sta/project.rs).
  const MIN_POINTS = 4;
  const SIZES = [["small", "小"], ["medium", "中"], ["large", "大"]];

  const box = document.createElement("div");
  box.id = "sta-draw";
  box.innerHTML = `
    <h4>自己标注</h4>
    <p class="dim">程序找不到、或者不是你想要的？自己在断层图上标出来：选一个工具，在图上点几下，其余的交给程序。</p>
    <div id="sta-draw-tools" class="al-row"></div>
    <p id="sta-draw-hint" class="dim"></p>
    <div id="sta-draw-sizes" class="al-row" hidden><span class="dim" title="线和面看不出颗粒多大，请选最接近的一个">颗粒大小</span></div>
    <div id="sta-draw-list" class="sta-marks"></div>
    <div class="al-row">
      <button id="sta-draw-done" class="small" hidden>完成</button>
      <button id="sta-draw-undo" class="small" title="去掉最后点的一个点">撤销一个点</button>
      <button id="sta-draw-del" class="small" title="先在列表里点一下要删的标注">删除所选</button>
    </div>
    <div class="al-row"><button id="sta-draw-run" class="primary">平均我标的</button></div>`;
  $("sta-draw-slot").append(box);
  const tools = $("sta-draw-tools");
  const btn = {};
  for (const [k, v] of Object.entries(KINDS)) {
    const b = document.createElement("button");
    b.className = "small";
    b.title = `${v.name}：${v.hint}`;
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
    b.onclick = () => { D.size = k; save(); render(); };
    $("sta-draw-sizes").append(b);
    sizes[k] = b;
  }

  const count = (m) => m.points.length;
  const usable = (m) => m.kind !== "point" && count(m) >= 2;
  const label = (m, i) => {
    const n = D.marks.slice(0, i + 1).filter((x) => x.kind === m.kind).length;
    const name = `${KINDS[m.kind].name} ${n}`;
    if (m.kind === "sphere") return `${name} · ${count(m) >= 2 ? "已画好" : "还差球面上的一点"}`;
    if (m.kind !== "point" && count(m) < 2) return `${name} · ${count(m)} 个点（至少 2 个）`;
    return `${name} · ${count(m)} 个点`;
  };

  async function save() {
    if (!W.out) return;
    try { await invoke("sta_marks_save", { out: W.out, marks: D.marks.map(({ kind, tomogram, points }) => ({ kind, tomogram, points })), size: D.size }); } catch (_) { /* no backend (demo) */ }
  }
  async function load(out) {
    if (!out || out === D.loaded) return;
    D.loaded = out;
    D.marks = [];
    D.sel = D.open = null;
    render();
    try {
      const r = await invoke("sta_marks_load", { out });
      D.marks = r.marks || [];
      D.size = r.size || "medium";
      D.sel = D.open = null;
      render();
    } catch (_) { /* no backend (demo) */ }
  }

  function setKind(k) {
    D.kind = k;
    D.open = null;
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
    $("sta-draw-hint").textContent = D.kind ? KINDS[D.kind].hint : mixed ? "点和线、面、球要分开平均：这次只平均线、面、球，点先不算" : "";
    const needSize = D.marks.some((m) => m.kind === "line" || m.kind === "surface");
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
      row.style.setProperty("--mark", KINDS[m.kind].color);
      row.title = m.tomogram;
      row.onclick = () => { D.sel = D.sel === i ? null : i; render(); draw(); };
      return row;
    }));
    $("sta-draw-del").disabled = D.sel == null || W.busy;
    $("sta-draw-undo").disabled = !D.marks.length || W.busy;
    const pointsOnly = pointsN > 0 && D.marks.every((m) => m.kind === "point");
    const ok = shapes || (pointsOnly && pointsN >= MIN_POINTS);
    $("sta-draw-run").disabled = !ok || W.busy;
    $("sta-draw-run").title = ok ? "" : pointsOnly ? `再点 ${MIN_POINTS - pointsN} 个颗粒（至少 ${MIN_POINTS} 个）` : D.marks.length ? "线和面至少要点 2 个点，球要点中心和球面上的一点" : "先在图上标一个东西";
    draw();
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
    m.points.pop();
    if (!m.points.length) {
      D.marks.splice(i, 1);
      D.open = null;
    }
    D.sel = null;
    save();
    render();
  };
  $("sta-draw-del").onclick = () => {
    if (D.sel == null) return;
    D.marks.splice(D.sel, 1);
    D.open = null;
    D.sel = null;
    save();
    render();
  };
  $("sta-draw-run").onclick = () => {
    const marks = D.marks.map(({ kind, tomogram, points }) => ({ kind, tomogram, points }));
    D.open = null;
    setKind(null);
    W.go((inputs, out) => invoke("sta_run_drawn", { inputs, marks, out, size: D.size }), false);
  };

  // The viewer's tool: a tap (not a drag) adds a point; dragging still moves the image.
  let press = null;
  ext.tools.mark = {
    down(e, p) { press = { x: e.clientX, y: e.clientY, p, moved: false }; },
    move(e) { if (press && Math.hypot(e.clientX - press.x, e.clientY - press.y) > 4) press.moved = true; },
    up() {
      const t = press;
      press = null;
      if (t && !t.moved) add(t.p);
    },
    active(on) { if (!on && D.kind) { D.kind = null; D.open = null; render(); } },
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
    if (W.panel.hidden || !S.info || S.axis !== "xy") return;
    const sc = toScreen(1, 0)[0] - toScreen(0, 0)[0];
    const at = (p) => toScreen(p[0] + 0.5, p[1] + 0.5);
    const alpha = (dz) => (dz <= 1 ? 1 : dz <= 6 ? 0.35 : 0);
    ctx.save();
    ctx.lineJoin = "round";
    D.marks.forEach((mark, i) => {
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
    ctx.restore();
  });
  document.addEventListener("tmv-opened", () => draw());

  document.addEventListener("sta-out", (e) => { load(e.detail); draw(); });
  document.addEventListener("sta-busy", () => render());
  // Closing the workspace puts the tool away.
  new MutationObserver(() => { if (W.panel.hidden && D.kind) setKind(null); }).observe(W.panel, { attributes: true, attributeFilter: ["hidden"] });
  render();
})();
