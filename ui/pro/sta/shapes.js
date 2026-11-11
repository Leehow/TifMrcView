"use strict";
// "Find them all, show them, then average" for membranes and vesicles
// (sta::shapes), the same loop found.js has for particles. The user gives 2–3
// EXAMPLES on the clean tomogram (paints membranes with 膜, marks vesicles with
// 球); the program FINDS every membrane / vesicle of that kind in all the
// ticked tomograms, works out which side each faces, and SHOWS them before
// anything is averaged: membranes filled, vesicles as circles, an arrow per
// sampling point (the side that faces out), a small ? where the program could
// not tell the side. The user only confirms: click one to flip it or drop it,
// flip all, or go back to the program's suggestion; then 「平均这些」.
// Examples are drawn in the accent colour, the ones found in a second colour,
// dropped ones grey and dashed with no arrows. The state (shapes.json) is kept
// in <out>/drawn, so the review shows again when the folder is reopened and no
// newer average exists. Lines and hand-drawn surfaces are not searched for:
// they are averaged as drawn, beside the rest.
// Backend: sta_shapes_find {inputs, marks, size, out} (→ {needSegment: [files]}
// or the review), sta_shapes_review {out, tomogram}, sta_shapes_section {out, id,
// tomogram, z} (→ [[y, x0, x1], …]), sta_shapes_set {out, tomogram, drop?, flip?,
// flipAll?, reset?}, sta_shapes_average {out}; sta_membrane_segment for files
// that have no segmentation (once). The review is of the file open in the
// viewer: voxels of that file. Every call is guarded; late answers are ignored.
// Shares app.js globals: S, ext, $, invoke, setTool, toScreen, draw, T.
(() => {
  const W = window.TifMrcViewPro.sta;
  if (!W) return;
  const MIN_SEEDS = 20; // as sta::shapes::MIN_SEEDS
  const FOUND_COLOR = "#0e8a7d", DROPPED_COLOR = "#8c959f";
  // `d` is the last review the backend gave (null: none to show), for the folder `out`.
  const R = { d: null, raw: null, out: "", gen: 0, sel: null, queue: Promise.resolve(), pending: 0, shown: "", dismissed: new Map() };
  // The membrane each item takes on a section: runs [y, x0, x1), per (folder, file, id, z).
  const secs = new Map();
  let secGen = 0, secTimer = 0, secWant = "";

  const box = document.createElement("div");
  box.id = "sta-shapes";
  box.className = "sta-found";
  box.hidden = true;
  box.innerHTML = `
    <h4 id="sta-shapes-head"></h4>
    <ul id="sta-shapes-per" class="sta-found-per"></ul>
    <p id="sta-shapes-lines" class="dim" hidden></p>
    <p id="sta-shapes-unsure" class="warn" hidden></p>
    <p class="dim sta-found-legend">图上：深色的是你给的例子，另一种颜色是程序找到的，灰色虚线的是你去掉的；箭头指向颗粒朝外的一侧，带 ? 的是程序拿不准朝向的。点一片膜或一个囊泡，可以给它翻面或去掉。</p>
    <div class="al-row"><button id="sta-shapes-flipall" class="small" title="把找到的全部换到另一侧">全部翻面</button><button id="sta-shapes-reset" class="small">恢复程序的建议</button></div>
    <div id="sta-shapes-sel" class="al-row sta-shapes-sel" hidden><span id="sta-shapes-selname" class="dim"></span><button id="sta-shapes-flip" class="small" title="把朝外的一侧换到另一边">翻面</button><button id="sta-shapes-drop" class="small"></button></div>
    <p id="sta-shapes-msg" class="dim"></p>
    <div class="al-row"><button id="sta-shapes-go" class="primary"></button><button id="sta-shapes-other" class="small" title="回到选例子">换一批例子</button></div>`;
  // Where the particle review sits: what was found is what the user looks at next.
  $("sta-cards-box").before(box);

  const pendingOf = (d) => !!d && !d.stale && (d.averageMs == null || d.foundMs > d.averageMs);
  const open = () => S.info?.path || W.items?.[0]?.path || "";
  const minutes = (s) => `约 ${Math.max(1, Math.round(s / 60))} 分钟`;
  const key = () => R.out;
  const item = (id) => R.d?.items?.find((i) => i.id === id) || null;

  // Fetches again (after a tab switch, a set, a find); the latest answer wins.
  async function load() {
    const gen = ++R.gen;
    const out = W.out, tomogram = open();
    if (!out || !tomogram) return apply(null);
    let d = null;
    try { d = await invoke("sta_shapes_review", { out, tomogram }); } catch (_) { /* no backend (demo), or nothing found there */ }
    if (gen !== R.gen || R.pending) return;
    R.out = out;
    apply(pendingOf(d) ? d : null);
  }
  W.reviewShapesLoad = load;

  // Puts the review `d` (or none) on the screen. Of two reviews (particles, shapes) the newer shows.
  function apply(d, quiet) {
    if (d && R.dismissed.get(key()) === d.foundMs) d = null;
    R.raw = d;
    if (d && (W.found?.pendingMs?.() || 0) > d.foundMs) d = null;
    R.d = d;
    if (R.sel != null && !item(R.sel)) R.sel = null;
    const id = d ? `${key()}|${d.foundMs}` : "";
    const fresh = !!d && id !== R.shown;
    R.shown = id;
    if (fresh) { secs.clear(); secGen++; }
    render();
    if (d && fresh) { try { box.scrollIntoView({ block: "nearest" }); } catch (_) { /* old webview */ } }
    if (d && fresh && S.tool !== "mark") activate();
    if (!d && S.tool === "shapes") setTool("select");
    if (!quiet) W.found?.recheck?.();
  }
  function activate() { if (S.tool !== "shapes") setTool("shapes"); }

  const plural = (d) => {
    const bits = [];
    if (d.membranes > 0) bits.push(`${d.membranes} 片膜`);
    if (d.balls > 0) bits.push(`${d.balls} 个囊泡`);
    return bits.join("、") || "0 片膜";
  };

  function render() {
    const d = R.d;
    const show = !!d && !(W.busy && W.runOut === W.out) && !W.panel.hidden;
    box.hidden = !show;
    W.reviewed.shapes = show ? d.perTomogram.map((t) => ({ path: t.path, n: (t.membranes || 0) + (t.balls || 0) + (t.lines || 0) })) : [];
    document.dispatchEvent(new CustomEvent("sta-review"));
    W.panel.classList.toggle("reviewing-shapes", show);
    if (!show) { draw(); return; }
    $("sta-shapes-head").textContent = `找到 ${plural(d)}`;
    const per = $("sta-shapes-per");
    per.hidden = d.perTomogram.length < 2;
    per.replaceChildren(...d.perTomogram.map((t) => {
      const li = document.createElement("li");
      li.textContent = `${t.name}：${[t.membranes ? `${t.membranes} 片膜` : "", t.balls ? `${t.balls} 个囊泡` : ""].filter(Boolean).join("、") || "0 个"}`;
      li.title = t.path;
      return li;
    }));
    $("sta-shapes-lines").hidden = !d.lines;
    $("sta-shapes-lines").textContent = d.lines ? `另有你画的 ${d.lines} 条线或面，按你画的算进去` : "";
    const un = $("sta-shapes-unsure");
    un.hidden = !(d.unsure > 0);
    un.textContent = d.unsure > 0 ? `有 ${d.unsure} 个程序拿不准朝向（带 ? 的），请看一眼箭头` : "";
    $("sta-shapes-flipall").disabled = W.busy;
    $("sta-shapes-reset").disabled = W.busy;
    const it = R.sel != null ? item(R.sel) : null;
    $("sta-shapes-sel").hidden = !it;
    if (it) {
      $("sta-shapes-selname").textContent = `已选：${it.kind === "ball" ? "囊泡" : "膜"}${it.example ? "（例子）" : ""}`;
      $("sta-shapes-flip").disabled = W.busy || it.state === "dropped";
      $("sta-shapes-flip").classList.toggle("on", !!it.flipped);
      $("sta-shapes-drop").textContent = it.state === "dropped" ? "要回来" : "不要这个";
      $("sta-shapes-drop").disabled = W.busy;
    }
    const small = d.seeds < MIN_SEEDS;
    const go = $("sta-shapes-go");
    go.textContent = `平均这些 · ${minutes(d.etaSeconds)}`;
    go.disabled = small || W.busy;
    go.title = small ? `采样点只有 ${d.seeds} 个，至少要 ${MIN_SEEDS} 个` : `将平均 ${plural(d)}，${minutes(d.etaSeconds)}`;
    const msg = $("sta-shapes-msg");
    if (!msg.dataset.err) msg.textContent = small ? `采样点只有 ${d.seeds} 个，至少要 ${MIN_SEEDS} 个：点灰色的膜把它要回来，或多标几个例子` : d.open ? "" : "这张断层图不在找出的断层图里，看不到标记";
    draw();
  }
  function say(t) { const m = $("sta-shapes-msg"); m.textContent = t; if (t) m.dataset.err = "1"; else delete m.dataset.err; }

  // Edits go to the backend one after another (each reads and writes shapes.json);
  // the answer to the last one is what is shown.
  function edit(change) {
    const out = R.out, tomogram = open();
    R.pending++;
    R.queue = R.queue.then(async () => {
      try {
        const d = await invoke("sta_shapes_set", { out, tomogram, ...change });
        if (R.pending === 1 && out === R.out) { R.gen++; say(""); R.raw = d; R.d = d; if (R.sel != null && !item(R.sel)) R.sel = null; render(); }
      } catch (e) {
        say(`${e}`);
      } finally {
        R.pending--;
      }
    });
    return R.queue;
  }
  $("sta-shapes-flipall").onclick = () => { activate(); edit({ flipAll: true }); };
  $("sta-shapes-reset").onclick = () => { activate(); edit({ reset: true }); };
  $("sta-shapes-flip").onclick = () => { if (R.sel != null) edit({ flip: R.sel }); };
  $("sta-shapes-drop").onclick = () => { if (R.sel != null) edit({ drop: R.sel }); };
  function dismiss() {
    if (R.d) R.dismissed.set(key(), R.d.foundMs);
    R.d = R.raw = null;
    R.sel = null;
    render();
    if (S.tool === "shapes") setTool("select");
  }
  $("sta-shapes-other").onclick = () => {
    dismiss();
    W.status("请再涂几片膜、或标几个球作例子，再按“找出全部同类”");
  };
  $("sta-shapes-go").onclick = async () => {
    const d = R.d;
    if (!d || W.busy || d.seeds < MIN_SEEDS) return;
    const { out } = R;
    await R.queue;
    await W.go(async (_inputs, o) => {
      const s = W.st(o);
      s.line = `将平均 ${plural(d)}，${minutes(d.etaSeconds)}`;
      if (o === W.out) $("sta-line").textContent = s.line;
      return invoke("sta_shapes_average", { out: out || o });
    }, false);
    load();
  };

  // Starts the search with the examples among `marks`. Files with no
  // segmentation are segmented first, one after another (once for a file), and
  // the search is asked again. Afterwards the review is shown for the user to check.
  async function find(marks, size) {
    if (W.busy) return;
    const before = R.d?.foundMs;
    R.d = R.raw = null;
    R.sel = null;
    await W.go(async (inputs, out) => {
      const s = W.st(out);
      const line = (t) => { s.line = t; if (out === W.out) $("sta-line").textContent = t; };
      let r = await invoke("sta_shapes_find", { inputs, marks, size: size || null, out });
      if (r?.needSegment?.length) {
        const files = r.needSegment;
        for (let i = 0; i < files.length; i++) {
          line(`正在自动找膜（${i + 1}/${files.length}）…只需一次`);
          await invoke("sta_membrane_segment", { tomogram: files[i] });
        }
        line("");
        r = await invoke("sta_shapes_find", { inputs, marks, size: size || null, out });
        if (r?.needSegment?.length) throw new Error(`没有找到膜：${r.needSegment.map((f) => f.split(/[\\/]/).pop()).join("、")} 里可能没有膜`);
      }
      return { drawn: true };
    }, false);
    R.dismissed.clear();
    secs.clear();
    secGen++;
    await load();
    if (R.d && R.d.foundMs !== before) {
      W.found?.dismiss?.();
      W.status(`找到 ${plural(R.d)}：看看图上的标记对不对，调好后按“平均这些”`);
    }
  }
  W.shapes = {
    find,
    shown: () => !box.hidden,
    pendingMs: () => R.d?.foundMs || 0,
    recheck: () => apply(R.raw, true),
    dismiss,
  };

  // ---- on the image ----
  const css = () => getComputedStyle(document.documentElement);
  const colorOf = (it) => (it.state === "dropped" ? DROPPED_COLOR : it.example ? css().getPropertyValue("--accent").trim() || "#0969da" : FOUND_COLOR);

  // The membranes' runs on the section shown: asked ~120 ms after the section stops changing, kept until a new search.
  function sections() {
    const d = R.d, tomogram = open(), z = S.z, out = R.out;
    if (!d || !d.open || !tomogram || S.axis !== "xy") return;
    const need = d.items.filter((i) => i.kind === "membrane" && !secs.has(`${out}|${tomogram}|${i.id}|${z}`)).map((i) => i.id);
    if (!need.length) return;
    const want = `${secGen}|${out}|${tomogram}|${z}|${need.join()}`;
    if (want === secWant) return;
    secWant = want;
    clearTimeout(secTimer);
    secTimer = setTimeout(async () => {
      secTimer = 0;
      const gen = secGen;
      await Promise.all(need.map(async (id) => {
        let runs = [];
        try { const r = await invoke("sta_shapes_section", { out, id, tomogram, z }); runs = Array.isArray(r) ? r : []; } catch (_) { /* no backend (demo) */ }
        if (gen !== secGen) return;
        if (secs.size > 4000) secs.clear();
        secs.set(`${out}|${tomogram}|${id}|${z}`, runs);
        if (S.z === z && open() === tomogram) draw();
      }));
      if (gen === secGen && secWant === want) secWant = "";
    }, 120);
  }
  const runsOf = (id) => secs.get(`${R.out}|${open()}|${id}|${S.z}`);
  // A ball's circle on the section shown (null: it does not reach it).
  const circle = (it) => {
    const dz = Math.abs(it.centre[2] - S.z);
    return dz < it.radius ? Math.sqrt(it.radius * it.radius - dz * dz) : null;
  };
  // Whether an item is seen on this section, and so carries its ? and can be clicked.
  const seenOn = (it) => (it.kind === "ball" ? circle(it) != null : !!runsOf(it.id)?.length);

  ext.overlays.push((ctx) => {
    if (box.hidden || !R.d || !S.info || S.axis !== "xy") return;
    sections();
    const sc = toScreen(1, 0)[0] - toScreen(0, 0)[0];
    const at = (p) => toScreen(p[0] + 0.5, p[1] + 0.5);
    const alpha = (dz) => (dz <= 1 ? 1 : dz <= 6 ? 0.35 : 0);
    ctx.save();
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    for (const it of R.d.items) {
      const col = colorOf(it), on = R.sel === it.id, dropped = it.state === "dropped";
      if (it.kind === "membrane") {
        const runs = runsOf(it.id);
        if (!runs?.length) continue;
        ctx.globalAlpha = dropped ? 0.2 : on ? 0.5 : 0.32;
        ctx.fillStyle = col;
        ctx.beginPath();
        for (const [y, x0, x1] of runs) { const [sx, sy] = toScreen(x0, y); ctx.rect(sx, sy, (x1 - x0) * sc, sc); }
        ctx.fill();
        if ((dropped || on) && runs.length < 1500) {
          ctx.globalAlpha = 1;
          ctx.strokeStyle = col;
          ctx.lineWidth = on ? 2.4 : 1;
          ctx.setLineDash(dropped ? [4, 3] : []);
          ctx.stroke();
        }
      } else {
        const r = circle(it);
        if (r == null) continue;
        const [x, y] = at(it.centre);
        ctx.globalAlpha = 1;
        ctx.strokeStyle = col;
        ctx.lineWidth = on ? 3.4 : dropped ? 1.2 : 2;
        ctx.setLineDash(dropped ? [4, 3] : []);
        ctx.beginPath();
        ctx.arc(x, y, Math.max(3, r * sc), 0, 2 * Math.PI);
        ctx.stroke();
      }
      ctx.setLineDash([]);
      if (dropped) continue;
      // The arrows: a line with a dot at the tip, along the axis seen from above.
      ctx.strokeStyle = ctx.fillStyle = col;
      ctx.lineWidth = on ? 2 : 1.4;
      for (const a of it.arrows) {
        const op = alpha(Math.abs(a.pos[2] - S.z));
        if (!op) continue;
        const [x, y] = at(a.pos);
        const flat = Math.hypot(a.axis[0], a.axis[1]);
        const tx = flat ? x + 16 * a.axis[0] : x, ty = flat ? y + 16 * a.axis[1] : y;
        ctx.globalAlpha = op;
        ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(tx, ty); ctx.stroke();
        ctx.beginPath(); ctx.arc(tx, ty, 2.5, 0, 2 * Math.PI); ctx.fill();
      }
      // A small ? where the program could not tell the side (until the user flips it).
      if (!it.example && !it.sideSure && !it.flipped && seenOn(it)) {
        const [x, y] = at(it.centre);
        ctx.globalAlpha = 1;
        ctx.font = "bold 13px sans-serif";
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.lineWidth = 3;
        ctx.strokeStyle = "#fff";
        ctx.strokeText("?", x, y);
        ctx.fillStyle = "#9a6700";
        ctx.fillText("?", x, y);
      }
    }
    ctx.restore();
  });

  // The item under a click (voxels of the open file) on the section shown, or null.
  function hit(x, y) {
    const near = Math.max(1.5, 4 / S.view.scale);
    let best = null, bestD = Infinity;
    for (const it of R.d?.items || []) {
      let dist = Infinity;
      if (it.kind === "ball") {
        const r = circle(it);
        if (r == null) continue;
        const dc = Math.hypot(it.centre[0] - x, it.centre[1] - y);
        // On the circle, or inside it: the nearer circle edge wins over a bigger one around it.
        if (dc <= r + near) dist = Math.abs(dc - r);
      } else {
        for (const [ry, x0, x1] of runsOf(it.id) || []) {
          if (Math.abs(ry - y) > near) continue;
          const dx = x < x0 ? x0 - x : x > x1 - 1 ? x - (x1 - 1) : 0;
          dist = Math.min(dist, Math.hypot(dx, ry - y));
        }
        if (dist > near) dist = Infinity;
      }
      if (dist < bestD) { best = it; bestD = dist; }
    }
    return best;
  }

  // The tool the viewer has while a review is shown: a tap selects an item (or nothing); a drag moves the image.
  let press = null;
  ext.tools.shapes = {
    down(e, p) { press = { x: e.clientX, y: e.clientY, p, cx: S.view.cx, cy: S.view.cy, moved: false }; },
    move(e) {
      if (!press) return;
      if (!press.moved && Math.hypot(e.clientX - press.x, e.clientY - press.y) <= 4) return;
      press.moved = true;
      S.view.cx = press.cx - (e.clientX - press.x) / S.view.scale;
      S.view.cy = press.cy - (e.clientY - press.y) / S.view.scale;
      draw();
    },
    up() {
      const t = press;
      press = null;
      if (!t || t.moved || !R.d) return;
      const h = hit(t.p[0] - 0.5, t.p[1] - 0.5);
      R.sel = h && R.sel !== h.id ? h.id : null;
      render();
    },
    active(on) { if (!on) { press = null; } },
  };

  document.addEventListener("tmv-opened", () => { if (!W.panel.hidden) load(); });
  document.addEventListener("sta-out", () => load());
  document.addEventListener("sta-busy", (e) => { if (e.detail) render(); else load(); });
  new MutationObserver(() => {
    if (W.panel.hidden) { if (S.tool === "shapes") setTool("select"); render(); } else load();
  }).observe(W.panel, { attributes: true, attributeFilter: ["hidden"] });
})();
