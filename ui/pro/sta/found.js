"use strict";
// "Find them all, show them, then average" for particles (sta::review): the
// user gives examples (a card, or 2–3 clicked particles) and the program
// FINDS everything like them and SHOWS it before anything is averaged:
// circles on the tomogram, "找到 N 个" with the count per tomogram, a
// strict ↔ loose slider, a click on a circle drops it, a click where there is
// none adds the nearest candidate. Only then 「平均这 N 个」, which says how
// many and about how long. The state (found.json, with the user's edits) is
// kept beside the cards (the cards' route) or in marked/ (clicked particles),
// so closing the app loses nothing; the review shows again when the folder is
// reopened and there is no newer average.
// Membranes and vesicles (膜 球) have their own review in shapes.js; lines and surfaces keep their direct run.
// Three routes (`which`): "card" (a card), "marked" (clicked particles) and "own" (a map the user brought, index.js).
// When the user has marked shapes (shapes.js) the review says so (`onShapes`): a tick box 「只要在你标的…上的」
// keeps only the places on them; unticking brings the others back (their state is "off" while ticked and not drawn).
// 「再找一次」 judges the places the user did not touch again by what the user dropped and kept
// (sta_review_again; the answer is `again: {out, in, ran}`, 「撤销」 takes it back).
// Backend: sta_find {inputs, pick|marks|which:"own", out}, sta_review {out, which, tomogram},
// sta_review_set {out, which, tomogram, keep?, toggle?, addAt?, reset?, onShapes?, undoAgain?},
// sta_review_again {out, which, tomogram}, sta_average_found {out, which}.
// The review is of the file open in the viewer (the tomogram or its look
// companion): places come back in that file's voxels. Every call is guarded.
// At the end: 「在图上看这些颗粒」, the particles of a finished average drawn on the clean tomogram with the same overlay (no editing).
// Shares app.js globals: S, ext, $, invoke, setTool, toScreen, draw, openPath.
(() => {
  const W = window.TifMrcViewPro.sta;
  if (!W) return;
  const MIN_KEPT = 20; // as sta::review::MIN_KEPT
  // `d` is the last review the backend gave (null: none to show), for the folder `out`.
  const R = { d: null, out: "", which: "", gen: 0, add: false, queue: Promise.resolve(), pending: 0, shown: "", dismissed: new Map(), raw: null };

  const box = document.createElement("div");
  box.id = "sta-found";
  box.className = "sta-found";
  box.hidden = true;
  box.innerHTML = `
    <h4 id="sta-found-head"></h4>
    <ul id="sta-found-per" class="sta-found-per"></ul>
    <p id="sta-found-unsure" class="warn" hidden>程序分不清哪些是，请拖动滑块自己定</p>
    <div class="sta-found-slider"><span class="dim">严格</span><input id="sta-found-keep" type="range" min="1" max="1" step="1" aria-label="保留多少个，从最像的开始"><span class="dim">宽松</span></div>
    <canvas id="sta-found-curve" height="44" aria-label="每个候选和例子的相似度，从高到低"></canvas>
    <p class="dim sta-found-legend">图上：实线圈会被平均，灰色虚线圈是没选上的，打叉的是去掉的，绿圈是加上的。点一个圈去掉或加上它。</p>
    <label id="sta-found-shapes-row" class="sta-found-shapes" hidden><input id="sta-found-shapes" type="checkbox"><span id="sta-found-shapes-text"></span></label>
    <div class="al-row"><button id="sta-found-add" class="small" aria-pressed="false" title="图上有漏掉的？打开后，点在它的中心，程序把最近的一个候选加进来">补一个</button><button id="sta-found-reset" class="small">恢复程序的建议</button></div>
    <div class="al-row sta-found-again"><button id="sta-found-again" class="small">再找一次</button><span id="sta-found-again-text" class="dim"></span><button id="sta-found-undo" class="link" data-no-i18n hidden>撤销</button></div>
    <p id="sta-found-msg" class="dim"></p>
    <div class="al-row"><button id="sta-found-go" class="primary"></button><button id="sta-found-other" class="small" title="回到选例子">换一个例子</button></div>`;
  // Above the cards: what was found is what the user looks at next.
  $("sta-cards-box").before(box);
  const curve = $("sta-found-curve");

  const pendingOf = (d) => !!d && !d.stale && (d.averageMs == null || d.foundMs > d.averageMs);
  const open = () => S.info?.path || W.items?.[0]?.path || "";
  const KIND = { membrane: "膜", ball: "囊泡", line: "纤维" };
  // 「膜、囊泡或纤维」
  const orOf = (a) => (a.length < 2 ? a.join("") : `${a.slice(0, -1).join("、")}或${a[a.length - 1]}`);
  const minutes = (s) => { const m = Math.max(1, Math.round(s / 60)); return `约 ${m} 分钟`; };

  // The review to show for the output folder `out`: of the two routes the newer one that no average followed.
  async function fetchBoth(out, tomogram) {
    const got = [];
    for (const which of ["card", "marked", "own"]) {
      try {
        const d = await invoke("sta_review", { out, which, tomogram });
        if (pendingOf(d)) got.push({ which, d });
      } catch (_) { /* no backend (demo), or nothing found there */ }
    }
    got.sort((a, b) => b.d.foundMs - a.d.foundMs);
    return got[0] || null;
  }
  // Fetches again (after a tab switch, a set, a find); the latest answer wins.
  async function load() {
    const gen = ++R.gen;
    const out = W.out, tomogram = open();
    if (!out || !tomogram) return apply(null);
    const got = await fetchBoth(out, tomogram);
    if (gen !== R.gen || R.pending) return;
    if (got) { R.which = got.which; R.out = out; }
    apply(got?.d || null);
  }
  W.reviewLoad = load;

  const key = () => `${R.out}|${R.which}`;
  // Puts the review `d` (or none) on the screen; the viewer is the place the user checks it on.
  // Of two reviews (particles here, membranes and vesicles in shapes.js) the newer one shows.
  function apply(d, quiet) {
    if (d && R.dismissed.get(key()) === d.foundMs) d = null;
    R.raw = d;
    if (d && (W.shapes?.pendingMs?.() || 0) > d.foundMs) d = null;
    R.d = d;
    const id = d ? `${key()}|${d.foundMs}` : "";
    const fresh = !!d && id !== R.shown;
    R.shown = id;
    render();
    if (d && fresh) { try { box.scrollIntoView({ block: "nearest" }); } catch (_) { /* old webview */ } }
    if (d && fresh && S.tool !== "mark") activate();
    if (!d && S.tool === "found") setTool("select");
    if (!quiet) W.shapes?.recheck?.();
  }
  function activate() { if (S.tool !== "found") setTool("found"); }

  function render() {
    const d = R.d;
    const show = !!d && !(W.busy && W.runOut === W.out) && !W.panel.hidden;
    box.hidden = !show;
    if (show && P.on) placedOff();
    W.reviewed.found = show ? d.perTomogram.map((t) => ({ path: t.path, n: t.kept })) : [];
    document.dispatchEvent(new CustomEvent("sta-review"));
    W.panel.classList.toggle("reviewing", show);
    if (!show) { R.add = false; draw(); return; }
    $("sta-found-head").textContent = `找到 ${d.total} 个`;
    const per = $("sta-found-per");
    per.hidden = d.perTomogram.length < 2;
    per.replaceChildren(...d.perTomogram.map((t) => { const li = document.createElement("li"); li.textContent = `${t.name}：${t.kept} 个`; li.title = t.path; return li; }));
    $("sta-found-unsure").hidden = !d.unsure;
    const slider = $("sta-found-keep");
    slider.max = String(Math.max(1, d.total));
    if (document.activeElement !== slider) slider.value = String(d.keep);
    slider.disabled = W.busy;
    $("sta-found-add").classList.toggle("on", R.add);
    $("sta-found-add").setAttribute("aria-pressed", String(R.add));
    $("sta-found-add").disabled = W.busy;
    $("sta-found-reset").disabled = W.busy;
    // Only on the shapes the user marked.
    const sh = d.onShapes || {};
    $("sta-found-shapes-row").hidden = !sh.available;
    $("sta-found-shapes").checked = !!sh.on;
    $("sta-found-shapes").disabled = W.busy;
    $("sta-found-shapes-text").textContent = `只要在你标的${orOf((sh.kinds || []).map((k) => KIND[k]).filter(Boolean)) || "形状"}上的（${sh.near ?? 0} 个）`;
    // Find again: once something was dropped by hand.
    const ag = d.again || {};
    const dropped = d.removed > 0;
    $("sta-found-again").disabled = W.busy || !dropped || W.pixelNeed.length > 0;
    $("sta-found-again").title = dropped ? "" : "先在图上点掉几个不要的";
    $("sta-found-again-text").textContent = ag.ran ? (ag.out || ag.in ? `又去掉 ${ag.out} 个像你去掉的，补回 ${ag.in} 个` : "重新判断过了，没有要改的") : "按你去掉和补上的，把其余的重新判断一遍";
    $("sta-found-undo").hidden = !ag.ran;
    $("sta-found-undo").disabled = W.busy;
    const small = d.kept < MIN_KEPT;
    const go = $("sta-found-go");
    go.textContent = `平均这 ${d.kept} 个 · ${minutes(d.etaSeconds)}`;
    go.disabled = small || W.busy || W.pixelNeed.length > 0;
    go.title = W.pixelNeed.length ? W.pixelLine : small ? `保留的只有 ${d.kept} 个，至少要 ${MIN_KEPT} 个` : `将平均 ${d.kept} 个，${minutes(d.etaSeconds)}`;
    $("sta-found-msg").textContent = small ? `保留的只有 ${d.kept} 个，至少要 ${MIN_KEPT} 个：把滑块往“宽松”拖，或点图上灰色的圈加几个` : d.open ? "" : "这张断层图不在找出的断层图里，看不到圈";
    sparkline();
    draw();
  }

  // The scores, best first, with the places kept and the program's suggestion marked.
  function sparkline() {
    const d = R.d;
    const dpr = window.devicePixelRatio || 1;
    const w = curve.clientWidth || 300, h = 44;
    curve.width = Math.round(w * dpr);
    curve.height = Math.round(h * dpr);
    const g = curve.getContext("2d");
    if (!g) return;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, h);
    const cs = getComputedStyle(document.documentElement);
    const accent = cs.getPropertyValue("--accent").trim() || "#0969da", dim = cs.getPropertyValue("--dim").trim() || "#656d76";
    const sc = d.scores || [];
    if (sc.length < 2) return;
    const lo = Math.min(...sc), hi = Math.max(...sc), span = hi - lo || 1;
    const x = (f) => 3 + f * (w - 6), y = (v) => 4 + (1 - (v - lo) / span) * (h - 8);
    g.lineWidth = 1.4;
    g.strokeStyle = dim;
    g.beginPath();
    sc.forEach((v, i) => { const px = x(i / (sc.length - 1)), py = y(v); if (i) g.lineTo(px, py); else g.moveTo(px, py); });
    g.stroke();
    const at = (n) => x(Math.min(1, n / Math.max(1, d.total)));
    g.globalAlpha = 0.12;
    g.fillStyle = accent;
    g.fillRect(3, 0, at(d.keep) - 3, h);
    g.globalAlpha = 1;
    g.strokeStyle = dim;
    g.setLineDash([3, 3]);
    g.beginPath(); g.moveTo(at(d.sure), 0); g.lineTo(at(d.sure), h); g.stroke();
    g.setLineDash([]);
    g.strokeStyle = accent;
    g.lineWidth = 2;
    g.beginPath(); g.moveTo(at(d.keep), 0); g.lineTo(at(d.keep), h); g.stroke();
  }

  // Edits go to the backend one after another (each reads and writes found.json);
  // the answer to the last one is what is shown.
  function edit(change) {
    const out = R.out, which = R.which, tomogram = open();
    R.pending++;
    R.queue = R.queue.then(async () => {
      try {
        const d = await invoke("sta_review_set", { out, which, tomogram, ...change });
        if (R.pending === 1 && out === R.out) { R.gen++; R.d = d; $("sta-found-msg").textContent = ""; render(); }
      } catch (e) {
        $("sta-found-msg").textContent = `${e}`;
      } finally {
        R.pending--;
      }
    });
    return R.queue;
  }

  // The slider: the circles follow at once, the file is written ~200 ms after it stops.
  let keepTimer = 0;
  $("sta-found-keep").oninput = (e) => {
    const d = R.d;
    if (!d) return;
    const keep = +e.target.value;
    for (const p of d.places) {
      if (p.i < keep && p.state === "below") p.state = "kept";
      else if (p.i >= keep && p.state === "kept") p.state = "below";
    }
    d.kept = Math.max(0, d.kept + keep - d.keep);
    d.keep = keep;
    activate();
    render();
    clearTimeout(keepTimer);
    keepTimer = setTimeout(() => edit({ keep }), 200);
  };
  $("sta-found-reset").onclick = () => { activate(); edit({ reset: true }); };
  $("sta-found-shapes").onchange = (e) => { activate(); edit({ onShapes: e.target.checked }); };
  $("sta-found-undo").onclick = () => { activate(); edit({ undoAgain: true }); };
  $("sta-found-again").onclick = async () => {
    const d = R.d;
    if (!d || W.busy || !(d.removed > 0)) return;
    const { out, which } = R;
    clearTimeout(keepTimer);
    await R.queue;
    const tomogram = open();
    await W.go(async () => { await invoke("sta_review_again", { out, which, tomogram }); return { drawn: true }; }, false);
    load();
  };
  $("sta-found-add").onclick = () => { R.add = !R.add; if (R.add) activate(); render(); };
  $("sta-found-other").onclick = () => {
    if (R.d) R.dismissed.set(key(), R.d.foundMs);
    R.d = null;
    R.add = false;
    render();
    if (S.tool === "found") setTool("select");
    W.status("请选择要平均什么：点一张卡片，或在图上点几个颗粒作例子");
  };
  $("sta-found-go").onclick = async () => {
    const d = R.d;
    if (!d || W.busy || d.kept < MIN_KEPT) return;
    const { out, which } = R;
    clearTimeout(keepTimer);
    await R.queue;
    await W.go(async (_inputs, o) => {
      const s = W.st(o);
      s.line = `将平均 ${d.kept} 个，${minutes(d.etaSeconds)}`;
      if (o === W.out) $("sta-line").textContent = s.line;
      return invoke("sta_average_found", { out: out || o, which });
    }, false);
    load();
  };

  // Starts the search: `task` is the sta_find call. Afterwards the review is shown for the user to check.
  async function find(task) {
    if (W.busy) return;
    const before = R.d?.foundMs;
    R.d = null;
    await W.go(async (inputs, out) => { await task(inputs, out); return { drawn: true }; }, false);
    R.dismissed.clear();
    await load();
    if (R.d && R.d.foundMs !== before) W.shapes?.dismiss?.();
    if (R.d && R.d.foundMs !== before) W.status(`找到 ${R.d.total} 个：看看图上的圈对不对，调好后按“平均这 ${R.d.kept} 个”`);
  }
  W.found = {
    // A card of the folder is the example.
    find: (pick) => find((inputs, out) => invoke("sta_find", { inputs, pick, marks: null, out })),
    // The map the user brought (index.js) is the example.
    findOwn: () => find((inputs, out) => invoke("sta_find", { inputs, pick: null, marks: null, out, which: "own" })),
    // Clicked particles (point marks) are the examples.
    findPoints: (marks) => find((inputs, out) => invoke("sta_find", { inputs, pick: null, marks, out })),
    // For shapes.js: the time of the review shown (0: none), showing it again after the other one changed, putting it away.
    pendingMs: () => R.d?.foundMs || 0,
    recheck: () => apply(R.raw, true),
    dismiss: () => {
      if (R.d) R.dismissed.set(key(), R.d.foundMs);
      R.d = R.raw = null;
      R.add = false;
      render();
      if (S.tool === "found") setTool("select");
    },
  };

  // ---- on the image ----
  const css = () => getComputedStyle(document.documentElement);
  // The places drawn on the slice shown: a particle is a ball, so its circle
  // shrinks away from its centre section and is gone beyond its radius;
  // near the centre it is solid, further off faint (as the marks in draw.js fade with distance).
  function seen() {
    const d = R.d;
    if (!d || !d.open || !d.radius || !S.info || S.axis !== "xy") return [];
    const out = [];
    for (const p of d.places) {
      const dz = Math.abs(p.pos[2] - S.z);
      if (dz >= d.radius || p.state === "off") continue;
      out.push({ p, r: Math.sqrt(d.radius * d.radius - dz * dz), a: dz <= 0.25 * d.radius ? 1 : 0.4 });
    }
    return out;
  }
  ext.overlays.push((ctx) => {
    if (box.hidden || !R.d) return;
    const sc = toScreen(1, 0)[0] - toScreen(0, 0)[0];
    const accent = css().getPropertyValue("--accent").trim() || "#0969da";
    ctx.save();
    ctx.lineCap = "round";
    for (const { p, r, a } of seen()) {
      if (p.state === "off") continue;
      const [x, y] = toScreen(p.pos[0] + 0.5, p.pos[1] + 0.5);
      const rr = Math.max(3, r * sc);
      ctx.globalAlpha = a;
      ctx.setLineDash([]);
      if (p.state === "kept") {
        ctx.strokeStyle = accent;
        ctx.lineWidth = 2;
      } else if (p.state === "added" || p.state === "taughtIn") {
        ctx.strokeStyle = "#1a7f37";
        ctx.lineWidth = 2.4;
      } else if (p.state === "removed" || p.state === "taughtOut") {
        ctx.strokeStyle = "#cf222e";
        ctx.lineWidth = 1.4;
      } else {
        ctx.strokeStyle = "#8c959f";
        ctx.lineWidth = 1;
        ctx.setLineDash([4, 3]);
      }
      ctx.beginPath();
      ctx.arc(x, y, rr, 0, 2 * Math.PI);
      ctx.stroke();
      if (p.state === "removed" || p.state === "taughtOut") {
        const k = rr * 0.6;
        ctx.setLineDash([]);
        ctx.beginPath();
        ctx.moveTo(x - k, y - k); ctx.lineTo(x + k, y + k);
        ctx.moveTo(x + k, y - k); ctx.lineTo(x - k, y + k);
        ctx.stroke();
      }
    }
    ctx.restore();
  });

  // The tool the viewer has while a review is shown: a tap on a circle drops or restores it,
  // a tap elsewhere adds the nearest candidate when 补一个 is on; a drag moves the image.
  let press = null;
  ext.tools.found = {
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
      const [x, y] = [t.p[0] - 0.5, t.p[1] - 0.5];
      // The nearest circle the tap is inside of.
      let hit = null, best = Infinity;
      for (const { p, r } of seen()) {
        const dist = Math.hypot(p.pos[0] - x, p.pos[1] - y);
        const reach = Math.max(r, 3 / S.view.scale);
        if (dist <= reach && dist < best) { hit = p; best = dist; }
      }
      if (hit) {
        // Shown at once, confirmed by the answer.
        hit.state = { kept: "removed", removed: "kept", below: "added", added: "below", taughtOut: "added", taughtIn: "below" }[hit.state];
        render();
        edit({ toggle: hit.i });
      } else if (R.add) {
        edit({ addAt: [Math.round(x * 100) / 100, Math.round(y * 100) / 100, S.z] });
      }
    },
    active(on) { if (!on) { press = null; R.add = false; if (R.d) render(); } },
  };


  // ---- where the averaged particles lie (「在图上看这些颗粒」) ----
  // After an average: the clean (look) version of a tomogram of the result is opened, as for a review, and
  // every particle of particles.csv near the shown section is drawn with the overlay above: a dot at its
  // centre and a short line for the direction of its Z axis ("up"), coloured by kind when kinds were looked for.
  // Backend: sta_placed {inputs, out, which, tomogram} -> {places: [{i, pos, axis, kind, score}], radius, open, look,
  // perTomogram: [{name, path, look, n}], kinds: [{id, count}], particles}; places are in the voxels of the shown file.
  // No editing in this view.
  const P = { d: null, on: false, out: "", which: "", gen: 0 };
  const pbox = $("sta-placed-box");
  const norm = (p) => (p || "").replace(/\\/g, "/");
  const sameFile = (a, b) => !!a && !!b && norm(a) === norm(b);
  const KIND_COLOURS = ["#0969da", "#cf222e", "#1a7f37", "#bf8700", "#8250df", "#bf3989", "#0a7e8c", "#6e7781"];
  const kindColour = (k) => (k == null ? css().getPropertyValue("--accent").trim() || KIND_COLOURS[0] : KIND_COLOURS[(k - 1) % KIND_COLOURS.length]);
  const pInputs = () => W.items.filter((i) => !W.cur().off.has(i.path)).map((i) => i.path);

  function placedOff() {
    P.on = false;
    P.d = null;
    P.gen++;
    pbox.hidden = true;
    $("sta-placed").classList.remove("on");
    draw();
  }
  function placedRender() {
    const d = P.d;
    pbox.hidden = !P.on;
    $("sta-placed").classList.toggle("on", P.on);
    if (!P.on || !d) return draw();
    const per = $("sta-placed-per");
    per.hidden = d.perTomogram.length < 2;
    per.replaceChildren(...d.perTomogram.map((t) => {
      const b = document.createElement("button");
      const shown = t.look || t.path;
      b.className = `small${sameFile(S.info?.path, shown) || sameFile(S.info?.path, t.path) ? " on" : ""}`;
      b.textContent = `${t.name} · ${t.n}`;
      b.title = t.path;
      b.onclick = async () => { if (!W.busy) { try { await openPath(shown); } catch (e) { W.status(`${e}`); } } };
      return b;
    }));
    const legend = $("sta-placed-legend");
    legend.replaceChildren(...(d.kinds || []).map((k) => {
      const el = document.createElement("span");
      const sw = document.createElement("i");
      sw.style.background = kindColour(k.id);
      el.append(sw, `第 ${k.id} 种 · ${k.count} 个`);
      return el;
    }));
    $("sta-placed-msg").textContent = d.open ? `这张断层图上有 ${d.places.length} 个颗粒（共 ${d.particles} 个）` : "这张断层图不在这个结果里，看不到颗粒";
    draw();
  }
  // The places on the file now open; the clean version of the first tomogram of the result is opened when this one is not in it.
  async function placedFetch(want) {
    const gen = ++P.gen;
    const d = await invoke("sta_placed", { inputs: pInputs(), out: P.out, which: P.which, tomogram: want || open() });
    if (gen !== P.gen || !P.on) return null;
    P.d = d;
    return d;
  }
  async function placedShow() {
    const r = W.result;
    if (!r || r.kept || W.busy) return;
    P.out = W.out;
    P.which = r.which;
    P.on = true;
    try {
      let d = await placedFetch();
      if (!d) return;
      const t = d.perTomogram.find((x) => sameFile(x.path, open()) || sameFile(x.look, open())) || d.perTomogram[0];
      if (!t) { placedOff(); return W.status("这个结果里没有颗粒表"); }
      const want = t.look || t.path;
      if (!sameFile(S.info?.path, want)) {
        await openPath(want);
        d = await placedFetch(want);
        if (!d) return;
      }
      placedRender();
      if (!d.open) W.status("这张断层图不在这个结果里");
    } catch (e) {
      placedOff();
      W.status(`${e}`);
    }
  }
  W.found.placed = {
    toggle: () => (P.on ? placedOff() : placedShow()),
    // index.js tells when the result shown changed: the marks go when it is no longer the one they belong to.
    sync: () => { if (P.on && (!W.result || W.result.kept || W.result.which !== P.which || W.out !== P.out || W.panel.hidden)) placedOff(); },
  };
  $("sta-placed-off").onclick = placedOff;
  // Another tomogram opened (from the wall or the buttons): its particles.
  document.addEventListener("tmv-opened", async () => {
    if (!P.on) return;
    try { if (await placedFetch()) placedRender(); } catch (_) { /* stays as it was */ }
  });

  ext.overlays.push((ctx) => {
    const d = P.d;
    if (!P.on || !d || !d.open || !S.info || S.axis !== "xy") return;
    const sc = toScreen(1, 0)[0] - toScreen(0, 0)[0];
    const R = d.radius || 10;
    ctx.save();
    ctx.lineCap = "round";
    ctx.lineWidth = 2;
    for (const p of d.places) {
      const dz = Math.abs(p.pos[2] - S.z);
      if (dz >= R) continue;
      const colour = kindColour(p.kind);
      const [x, y] = toScreen(p.pos[0] + 0.5, p.pos[1] + 0.5);
      const [ex, ey] = toScreen(p.pos[0] + 0.5 + p.axis[0] * R * 0.9, p.pos[1] + 0.5 + p.axis[1] * R * 0.9);
      ctx.globalAlpha = dz <= 0.25 * R ? 1 : 0.4;
      ctx.strokeStyle = colour;
      ctx.beginPath();
      ctx.moveTo(x, y);
      ctx.lineTo(ex, ey);
      ctx.stroke();
      ctx.fillStyle = colour;
      ctx.beginPath();
      ctx.arc(x, y, Math.max(2.5, Math.min(5, 0.3 * R * sc)), 0, 2 * Math.PI);
      ctx.fill();
    }
    ctx.restore();
  });

  document.addEventListener("tmv-opened", () => { if (!W.panel.hidden) load(); });
  document.addEventListener("sta-out", () => load());
  document.addEventListener("sta-pixel", () => render());
  document.addEventListener("sta-busy", (e) => { if (e.detail) render(); else load(); });
  new MutationObserver(() => {
    if (W.panel.hidden) { if (S.tool === "found") setTool("select"); render(); } else load();
  }).observe(W.panel, { attributes: true, attributeFilter: ["hidden"] });
})();
