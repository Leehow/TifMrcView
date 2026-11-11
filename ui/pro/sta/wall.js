"use strict";
// The thumbnail wall of the "choose what to average" step: one tile per
// tomogram of the project, so one sees which are there, which can be used and
// which is which, instead of a list of names. It replaces the checklist.
// A tile shows the section with the most in it (sta_thumbnail; made one after
// another, the ticked and visible ones first), the name, a tick box (ticked =
// "take part in the averaging"; the same `off` set and `W.items` index.js has
// always sent to the backend), a ring when its file is open in the viewer, and
// a note on what is wrong with it. Clicking the picture opens the tomogram's
// clean version in the viewer; clicking the tick box only ticks. Statuses
// (sta_gallery): ready, noLook (usable, only the tomogram itself to look at),
// notBuilt (a tilt series with no tomogram yet: the click opens it in the
// Cryo-ET workspace), unusable (greyed, with the reason).
// The tiles come from sta_gallery (asked again whenever index.js refreshes);
// tomograms added from elsewhere (W.items that no tile covers) are appended as
// plain tiles. Tomograms the gallery lists but sta_siblings does not start
// unticked; ticking one adds it (W.addItem). Without a backend (the web demo)
// the tiles are the plain list and have no pictures.
// Small badges: 已标 N (the user's marks on that tomogram, draw.js) and 找到 N
// (what the review shown found there, found.js / shapes.js via W.reviewed).
// While a review box (found.js, shapes.js) is showing the wall folds to one line, 「8 个断层图，已选 3 个 ▸」,
// that unfolds on click, so the review is not pushed down the panel.
// Default ticks: every "ready" tomogram of the project starts ticked (the ones sta_siblings did not list are
// added through W.addItem); "noLook" ones only when no tomogram has a clean version (else they start unticked).
// This is decided once per tomogram and folder, so the user's own ticks are never undone.
// Shares app.js globals: S, $, invoke, openPath.
(() => {
  const W = window.TifMrcViewPro.sta;
  if (!W) return;
  const G = W.wall = { tiles: [], gen: 0 };
  const name = (p) => p?.split(/[\\/]/).pop() || "";
  const norm = (p) => (p || "").replace(/\\/g, "/");
  // The same file: the same path, or (paths come from different listings) the same name.
  const same = (a, b) => !!a && !!b && (norm(a) === norm(b) || name(a) === name(b));

  const box = document.createElement("div");
  box.id = "sta-wall-box";
  box.innerHTML = '<button id="sta-wall-fold" type="button" class="sta-wall-fold" hidden aria-expanded="false"></button><div id="sta-wall" class="sta-wall"></div><p id="sta-wall-count" class="dim"></p>';
  $("sta-data").replaceWith(box);
  const wall = $("sta-wall");
  const foldBtn = $("sta-wall-fold");
  // The wall is folded while a review shows, until the user unfolds it (once per review).
  let reviewing = false, unfolded = false;
  foldBtn.onclick = () => { unfolded = !unfolded; render(); };

  // ---- the tiles ----
  const keyOf = (t) => t.raw || t.stack || t.name;
  const shown = (t) => t.look || t.raw || null;
  const usable = (t) => !!t.raw && (t.status === "ready" || t.status === "noLook");
  const itemOf = (t) => (t.raw ? W.items.find((i) => same(i.path, t.raw)) : null);
  const ticked = (t) => { const it = usable(t) && itemOf(t); return !!it && !W.cur().off.has(it.path); };
  const isOpen = (t) => { const p = S.info?.path; return p ? norm(p) === norm(t.raw) || norm(p) === norm(t.look) : !!t.open; };
  function tiles() {
    const out = G.tiles.map((t) => ({ ...t }));
    for (const it of W.items) {
      if (out.some((t) => same(t.raw, it.path) || same(t.look, it.path))) continue;
      out.push({ name: it.name, raw: it.path, look: it.look || null, stack: null, status: it.look ? "ready" : "noLook", reason: null, open: false });
    }
    return out;
  }

  // What the gallery says about the project of the open file.
  async function load() {
    const gen = ++G.gen;
    const path = S.info?.path && S.info.sizeZ >= 3 ? S.info.path : W.items[0]?.path;
    let list = [];
    if (path) { try { const r = await invoke("sta_gallery", { path }); if (Array.isArray(r)) list = r; } catch (_) { /* no backend (web demo): the plain list */ } }
    if (gen !== G.gen) return;
    G.tiles = list;
    G.loaded = gen;
    render();
    defaults(list, gen);
  }
  G.load = load;

  // The ticks a tomogram starts with (see the head of this file).
  const decided = new Set();
  async function defaults(list, gen) {
    if (!list.length || !W.items.length) return;
    let out = "";
    try { out = await invoke("sta_out", { first: W.items[0].path }); } catch (_) { return; }
    if (gen !== G.gen || !out || out !== W.out) return;
    const clean = list.some((t) => t.raw && t.status === "ready");
    let changed = false;
    for (const t of list) {
      const k = `${out}|${t.raw}`;
      if (!t.raw || !usable(t) || decided.has(k)) continue;
      decided.add(k);
      const it = itemOf(t);
      if (t.status === "ready" || !clean) {
        if (!it) { try { await W.addItem(t.raw); changed = true; } catch (_) { decided.delete(k); } }
      } else if (it && !W.cur().off.has(it.path)) {
        W.cur().off.add(it.path);
        changed = true;
      }
      if (gen !== G.gen) return;
    }
    if (changed) render();
  }

  // The folder is known a moment after the tiles may be (index.js asks for it after the list): try again then.
  document.addEventListener("sta-out", () => { if (G.loaded === G.gen) defaults(G.tiles, G.gen); });

  // ---- thumbnails: one after another, the ticked and visible ones first ----
  // file -> {s: "busy" | "ok" | "fail", url, section, stale}
  const thumbs = new Map();
  const seen = new Set();
  function nextFile() {
    let best = null, bestP = 99;
    for (const t of tiles()) {
      const f = shown(t);
      const have = f && thumbs.get(f);
      if (!f || (have && !have.stale)) continue;
      const p = (seen.has(keyOf(t)) ? 0 : 2) + (ticked(t) ? 0 : 1);
      if (p < bestP) { best = f; bestP = p; }
    }
    return best;
  }
  let pumping = false;
  async function pump() {
    if (pumping) return;
    pumping = true;
    try {
      for (let f; !W.panel.hidden && (f = nextFile());) {
        const old = thumbs.get(f);
        thumbs.set(f, { ...old, s: old?.url ? "ok" : "busy", stale: false });
        let r = null;
        try { r = await invoke("sta_thumbnail", { file: f }); } catch (_) { /* no backend, or the file cannot be read */ }
        thumbs.set(f, r?.dataUrl ? { s: "ok", url: r.dataUrl, section: r.section } : old?.url ? { ...old, stale: false } : { s: "fail" });
        render();
      }
    } finally { pumping = false; }
  }
  // Opening the workspace again: what was made is shown at once and checked again (the file may have changed).
  G.reset = () => { for (const v of thumbs.values()) v.stale = true; };

  const io = typeof IntersectionObserver === "function" ? new IntersectionObserver((es) => {
    for (const e of es) { const k = e.target.dataset.key; if (e.isIntersecting) seen.add(k); else seen.delete(k); }
    pump();
  }, { root: wall }) : null;

  // ---- one tile ----
  const els = new Map();
  function makeTile(k) {
    const el = document.createElement("div");
    el.className = "sta-tile";
    el.dataset.key = k;
    el.innerHTML = '<div class="pic" role="button" tabindex="0"><img alt="" hidden><span class="ph"></span><span class="marks"></span></div>' +
      '<label class="tick"><input type="checkbox"></label><div class="nm"><span class="a"></span><span class="b"></span></div><div class="note"></div>';
    const pic = el.querySelector(".pic"), cb = el.querySelector("input");
    pic.onclick = () => onPicture(el._t);
    pic.onkeydown = (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onPicture(el._t); } };
    cb.onchange = async () => {
      const t = el._t, it = itemOf(t);
      if (it) {
        if (cb.checked) W.cur().off.delete(it.path); else W.cur().off.add(it.path);
      } else if (cb.checked) {
        // A tomogram the gallery lists but the old list did not: it joins when ticked.
        try { await W.addItem(t.raw); } catch (_) { cb.checked = false; }
      }
      render();
    };
    io?.observe(el);
    return el;
  }

  async function onPicture(t) {
    if (W.busy) return;
    if (t.raw) {
      const f = shown(t);
      if (isOpen(t) && S.info?.path === f) return;
      try { await openPath(f); } catch (e) { W.status(`${e}`); }
      return;
    }
    // A tilt series that was not reconstructed: the Cryo-ET workspace is where that is done.
    const cryo = window.TifMrcViewPro.cryoet;
    if (!t.stack || !cryo?.load || cryo.busy) return;
    try {
      await cryo.load(t.stack);
      if (cryo.panel.hidden) $("btn-cryoet").click();
    } catch (e) { W.status(`无法打开这个倾转序列：${e}`); }
  }

  function paint(el, t) {
    el._t = t;
    const usableNow = usable(t);
    el.classList.toggle("open", isOpen(t));
    el.classList.toggle("off", !usableNow);
    el.classList.toggle("ticked", ticked(t));
    el.querySelector(".pic").classList.toggle("clickable", !!t.raw || (!!t.stack && !!window.TifMrcViewPro.cryoet?.load));
    // The name: the middle is cut off, the end (often the telling part) stays.
    const nm = t.name || "";
    const cut = nm.length > 16 ? nm.length - 8 : nm.length;
    el.querySelector(".a").textContent = nm.slice(0, cut);
    el.querySelector(".b").textContent = nm.slice(cut);
    el.querySelector(".nm").title = nm;
    const cryo = window.TifMrcViewPro.cryoet;
    const note = el.querySelector(".note");
    let text = "", why = "";
    if (t.status === "noLook") { text = "没有清晰版"; why = "只有断层图本身，没有清晰（去噪）的版本；仍然可以平均，但图上看得不如清晰版清楚"; }
    else if (t.status === "notBuilt") { text = "还没重建"; why = t.stack && cryo?.load ? "还没有重建成断层图。点一下，在 Cryo-ET 里打开这个倾转序列去重建" : "先在 Cryo-ET 里重建"; }
    else if (t.status === "unusable") { text = t.reason || "不能用"; why = text; }
    note.textContent = text;
    note.title = why;
    const pic = el.querySelector(".pic");
    pic.title = t.raw ? `点一下在查看器里打开${t.look ? "清晰版" : ""}；图是内容最多的一层` : why;
    // The picture.
    const f = shown(t), th = f ? thumbs.get(f) : null;
    const img = el.querySelector("img"), ph = el.querySelector(".ph");
    if (th?.url) { if (img.getAttribute("src") !== th.url) img.src = th.url; img.hidden = false; } else img.hidden = true;
    ph.hidden = !!th?.url;
    ph.className = `ph${!f ? " none" : th?.s === "fail" ? " fail" : " wait"}`;
    ph.textContent = !f ? (t.status === "notBuilt" ? "⛶" : "") : th?.s === "fail" ? "无法预览" : "";
    // The tick box.
    const cb = el.querySelector("input");
    cb.checked = ticked(t);
    cb.disabled = !usableNow || W.busy;
    el.querySelector(".tick").title = usableNow ? (cb.checked ? "参与平均" : "不参与平均；打勾后参与") : "还不能参与平均";
    // Badges.
    const bits = [];
    const marked = (W.draw?.marks || []).filter((m) => same(m.tomogram, t.raw) || same(m.tomogram, t.look)).length;
    if (marked) bits.push(["已标 " + marked, "mk"]);
    const rev = [...(W.reviewed?.found || []), ...(W.reviewed?.shapes || [])].filter((r) => same(r.path, t.raw)).reduce((n, r) => n + r.n, 0);
    if (rev) bits.push(["找到 " + rev, "fd"]);
    const row = el.querySelector(".marks");
    row.replaceChildren(...bits.map(([x, c]) => { const b = document.createElement("span"); b.className = c; b.textContent = x; return b; }));
  }

  // Puts `list` in `parent` in that order, touching the DOM only when it differs (focus and hover stay).
  function sync(parent, list) {
    const now = [...parent.children];
    if (now.length === list.length && now.every((c, i) => c === list[i])) return;
    parent.replaceChildren(...list);
  }

  let shownOpen = "";
  function render() {
    const list = tiles();
    const keep = new Set();
    const nodes = list.map((t) => {
      const k = keyOf(t);
      keep.add(k);
      if (!els.has(k)) els.set(k, makeTile(k));
      const el = els.get(k);
      paint(el, t);
      return el;
    });
    for (const [k, el] of els) if (!keep.has(k)) { io?.unobserve(el); els.delete(k); seen.delete(k); }
    sync(wall, [...nodes, plus]);
    // The line under the wall.
    const total = list.length, on = list.filter(ticked).length, no = list.filter((t) => !usable(t)).length;
    $("sta-wall-count").textContent = total ? `共 ${total} 个断层图，已选 ${on} 个${no ? `，${no} 个还不能用` : ""}` : "先打开一张断层图，或在 Cryo-ET 工作区里完成“子断层平均”用途的重构";
    // Folded while a review shows.
    const rev = (W.reviewed?.found?.length || 0) + (W.reviewed?.shapes?.length || 0) > 0 && total > 0;
    if (rev && !reviewing) unfolded = false;
    reviewing = rev;
    const folded = rev && !unfolded;
    foldBtn.hidden = !rev;
    foldBtn.textContent = `${total} 个断层图，已选 ${on} 个 ${unfolded ? "▾" : "▸"}`;
    foldBtn.title = unfolded ? "收起断层图" : "展开，看看是哪几个断层图";
    foldBtn.setAttribute("aria-expanded", String(!folded));
    wall.hidden = folded;
    $("sta-wall-count").hidden = folded;
    fit();
    // The open one in sight (once per change).
    const o = list.find(isOpen);
    const ok = o ? keyOf(o) : "";
    if (ok && ok !== shownOpen) {
      const el = els.get(ok);
      if (el && el.offsetHeight) {
        if (el.offsetTop < wall.scrollTop) wall.scrollTop = el.offsetTop;
        else if (el.offsetTop + el.offsetHeight > wall.scrollTop + wall.clientHeight) wall.scrollTop = el.offsetTop + el.offsetHeight - wall.clientHeight;
      }
    }
    shownOpen = ok;
    pump();
  }
  G.render = render;

  // The wall shows about two and a half rows and scrolls by itself beyond that.
  function fit() {
    const first = wall.querySelector(".sta-tile:not(.plus)");
    if (!first || !first.offsetHeight) return;
    const gap = parseFloat(getComputedStyle(wall).rowGap) || 0;
    wall.style.maxHeight = `${Math.round(2.5 * (first.offsetHeight + gap))}px`;
  }
  if (typeof ResizeObserver === "function") new ResizeObserver(fit).observe(wall);

  // The last tile: a tomogram from another folder.
  const plus = document.createElement("button");
  plus.id = "sta-add";
  plus.type = "button";
  plus.className = "sta-tile plus";
  plus.title = "再加入别的文件夹里的断层图一起平均";
  plus.innerHTML = "<b>＋</b><span>添加断层图…</span>";
  plus.onclick = () => W.addTomogram();

  document.addEventListener("sta-review", () => { if (!W.panel.hidden) render(); });
  wall.append(plus);
  render();
})();
