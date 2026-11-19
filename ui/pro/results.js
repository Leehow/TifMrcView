"use strict";
// 「处理结果」 at the top of the right sidebar (Pro): the tomograms made by
// 三维重构 and the maps made by 单颗粒 for the folder the sidebar shows, local
// or over SSH, each with what was done to it (精对齐, N2N 降噪, 去缺失楔, …).
// A click opens the result in the viewer, as a file of the list would.
// Nothing is indexed here: the cryo-ET part is the same `tomo_session` the
// Cryo-ET panel's cards read (taken from that panel when it has it), the
// single-particle part the project as saved (`spa_peek`, which makes nothing).
// Shares app.js globals: S, invoke, $, openHere, openPath, openMenu,
// stripFolded, setStripFold.
(() => {
  const R = window.TmvCryoRules;
  const GROUP = { id: "results", name: "处理结果", em: true };
  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  const fmtA = (v) => (v >= 10 ? v.toFixed(0) : v.toFixed(1));
  const lastName = (p) => String(p || "").split(/[\\/]/).filter(Boolean).pop() || p;

  // Results by sidebar folder (S.folder.dir): {cryo, spa}; null while read.
  const known = new Map();
  let shown = null, section = null, gen = 0;

  /** The folder the sidebar shows, as the processing commands take it; null where results are not looked for. */
  function current() {
    const f = S.folder;
    if (!f?.dir) return null;
    if (f.remote && !f.processing) return null;
    // A whole disk or a home folder: reading every file under it to look for tilt series is too much.
    const parts = String(f.remote ? f.remotePath : f.dir).split(/[\\/]/).filter(Boolean);
    if (parts.length < 2 || /^[A-Za-z]:$/.test(parts[0]) && parts.length < 3) return null;
    if (parts.length === 2 && ["Users", "home"].includes(parts[0])) return null;
    return f.dir;
  }

  /** The results of `dir`, else of the nearest folder above it that has some (inside a data folder's eer/, say). */
  function resultsFor(dir) {
    const own = known.get(dir);
    if (own && count(own)) return { dir, r: own };
    let best = null;
    for (const [d, r] of known) {
      if (!r || !count(r) || d === dir || !dir.startsWith(d.replace(/[\\/]+$/, "")) || !/[\\/]/.test(dir[d.replace(/[\\/]+$/, "").length] || "")) continue;
      if (!best || d.length > best.dir.length) best = { dir: d, r };
    }
    return best;
  }

  const cryoRows = (r) => (r?.cryo?.series || []).filter((s) => s.result || s.quick);
  function spaRows(p) {
    if (!p) return [];
    const rows = [];
    if (p.refine?.sharpened || p.refine?.map) rows.push({ path: p.refine.sharpened || p.refine.map, name: "三维精修图", line: `${p.refine.particles} 个颗粒 · 分辨率 ${fmtA(p.refine.resMasked || p.refine.res)} Å` });
    if (p.initial?.path) rows.push({ path: p.initial.path, name: "初始模型", line: `${fmtA(p.initial.pixelA)} Å/像素 · ${p.initial.boxPx} 像素盒` });
    if (p.class2d?.path) rows.push({ path: p.class2d.path, name: "二维分类平均", line: `${p.class2d.counts.length} 类 · 选了 ${p.class2d.selected.length} 类` });
    return rows;
  }
  /** What the single-particle project went through, in the words of its steps. */
  function spaDone(p) {
    const done = [];
    const n = p.mics.length;
    if (p.movies && p.mics.some((m) => m.movie && m.path !== m.movie)) done.push("运动校正");
    const ctf = p.mics.filter((m) => m.ctf).length;
    if (ctf) done.push(ctf === n ? "CTF" : `CTF ${ctf}/${n}`);
    const picks = p.mics.reduce((a, m) => a + (m.picks || 0), 0);
    if (picks) done.push(`挑选 ${picks} 个颗粒`);
    if (p.class2d) done.push("二维分类");
    if (p.initial) done.push("初始模型");
    if (p.refine) done.push(`三维精修 ${fmtA(p.refine.resMasked || p.refine.res)} Å`);
    return done;
  }
  const count = (r) => cryoRows(r).length + spaRows(r.spa).length;

  // ---------- small pictures, a few at a time (over SSH each is a server call) ----------

  const pics = new Map(), queue = [];
  let active = 0;
  function pump() {
    while (active < 2 && queue.length) {
      const job = queue.shift();
      if (!job.box.isConnected) continue;
      active++;
      job.run().finally(() => { active--; pump(); });
    }
  }
  const seen = new IntersectionObserver((entries) => {
    for (const en of entries) if (en.isIntersecting) { seen.unobserve(en.target); en.target._load?.(); }
  }, { root: $("sidebar"), rootMargin: "300px 0px" });
  function picture(box, path) {
    const put = (url) => { const img = el("img"); img.alt = ""; img.src = url; box.replaceChildren(img); };
    if (pics.has(path)) return put(pics.get(path));
    box._load = () => {
      queue.push({ box, run: () => invoke("tomo_view", { path, z: null, max: 128 }).then((buf) => {
        const d = R?.decodePicture(buf);
        if (!d) throw new Error("空图像");
        const c = el("canvas");
        c.width = d.w; c.height = d.h;
        c.getContext("2d").putImageData(new ImageData(d.rgba, d.w, d.h), 0, 0);
        const url = c.toDataURL();
        pics.set(path, url);
        if (pics.size > 300) pics.delete(pics.keys().next().value);
        put(url);
      }).catch(() => { box.textContent = "无图"; }) });
      pump();
    };
    seen.observe(box);
  }

  // ---------- the section ----------

  function row(path, name, lines, chips, tip) {
    const b = el("button", "pr-row");
    b.type = "button";
    b.dataset.path = path;
    b.title = `${tip || name}\n点按在主窗口打开`;
    const box = el("span", "pr-pic", "…");
    const text = el("span", "pr-text");
    text.append(el("b", "pr-nm", name));
    for (const l of lines.filter(Boolean)) text.append(el("span", "pr-line", l));
    if (chips.length) {
      const c = el("span", "pr-chips");
      for (const t of chips) c.append(el("span", "pr-chip", t));
      text.append(c);
    }
    b.append(box, text);
    picture(box, path);
    b.onclick = () => { mark(path); openHere(path); };
    // A local result can go to a new tab, as files of the list can.
    b.oncontextmenu = (e) => {
      if (!window.TifMrcViewPro?.documents || window.TmvRemote?.displayPath(path) !== path) return;
      e.preventDefault();
      openMenu(b, [{ value: "tab", label: "在新标签页打开" }], () => openPath(path));
      const m = $("dropmenu");
      m.style.left = `${Math.max(8, Math.min(e.clientX, window.innerWidth - m.offsetWidth - 8))}px`;
      m.style.top = `${Math.max(8, Math.min(e.clientY, window.innerHeight - m.offsetHeight - 8))}px`;
    };
    return b;
  }
  let picked = null;
  function mark(path) {
    picked = path;
    section?.querySelectorAll(".pr-row").forEach((r) => r.classList.toggle("cur", r.dataset.path === path || r.dataset.path === S.info?.path));
  }

  function render() {
    const strip = $("strip");
    const dir = current();
    const found = dir && resultsFor(dir);
    section?.remove();
    section = null;
    if (!found) return;
    const { r } = found;
    const cryo = cryoRows(r), spa = spaRows(r.spa);
    section = el("section", "strip-group pr-results");
    section.dataset.group = GROUP.id;
    section.classList.toggle("folded", stripFolded(GROUP));
    const head = el("button", "strip-head");
    head.innerHTML = `<svg viewBox="0 0 24 24"><path d="M9 6l6 6-6 6"/></svg><span class="gn"></span><span class="gc"></span>`;
    head.querySelector(".gn").textContent = found.dir === dir ? GROUP.name : `${GROUP.name} · ${lastName(window.TmvRemote?.displayPath(found.dir) || found.dir)}`;
    head.querySelector(".gc").textContent = String(cryo.length + spa.length);
    head.title = "这个文件夹里三维重构和单颗粒处理做出的结果；点一项在主窗口打开";
    head.onclick = () => setStripFold(GROUP, !section.classList.contains("folded"));
    const body = el("div", "strip-body pr-body");
    if (cryo.length) {
      body.append(el("div", "pr-kind", `冷冻电镜断层 · ${cryo.length} 个`));
      for (const s of R ? R.sortSeries(cryo) : cryo) {
        const path = s.result || s.quick;
        const px = s.pixelA > 0 && s.bin ? s.pixelA * s.bin : 0;
        const size = s.bin ? (px ? `bin ${s.bin} · ${fmtA(px)} Å · 分辨率约 ${fmtA(2 * px)} Å` : `bin ${s.bin}`) : "";
        const chips = s.result ? (s.done?.length ? s.done : ["快速重构"]) : ["快速预览"];
        const left = R?.leftOutText(s) || "";
        const r0 = row(path, s.name, [size, left], chips, `${s.name}\n${s.result ? "最终断层图" : "快速断层图（还没精做）"}`);
        if (R?.needsAttention(s)) r0.classList.add("attn");
        body.append(r0);
      }
    }
    if (spa.length) {
      const p = r.spa;
      body.append(el("div", "pr-kind", `单颗粒 · ${lastName(window.TmvRemote?.displayPath(p.source) || p.source)}`));
      const done = spaDone(p);
      // What the project went through is said once, on its furthest result.
      spa.forEach((x, i) => body.append(row(x.path, x.name, [x.line], i ? [] : done, `${x.name}\n${window.TmvRemote?.displayPath(x.path) || x.path}`)));
    }
    section.append(head, body);
    strip.prepend(section);
    mark(picked);
  }

  // ---------- reading ----------

  async function read(dir, cryo = undefined) {
    const g = ++gen;
    const [c, p] = await Promise.all([
      cryo !== undefined ? cryo : invoke("tomo_session", { folder: dir }).catch(() => null),
      invoke("spa_peek", { source: dir }).catch(() => null),
    ]);
    known.set(dir, { cryo: c, spa: p });
    if (known.size > 40) known.delete(known.keys().next().value);
    if (g === gen && current() === dir) render();
  }

  // The sidebar list was built (a folder opened, or another one of the server browsed).
  document.addEventListener("tmv-strip", () => {
    const dir = current();
    render();
    if (dir && dir !== shown) { shown = dir; if (!known.has(dir)) read(dir); }
  });
  // The Cryo-ET panel read the same folder: its cards and this list are one reading.
  document.addEventListener("cryoet-session", (e) => {
    const { folder, session } = e.detail || {};
    if (!folder) return;
    known.set(folder, { cryo: session, spa: known.get(folder)?.spa ?? null });
    if (current() === folder) render();
  });
  // Something was made: read the folder again (a run reports `tomo-session` as it goes).
  let timer = 0;
  const again = () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      const dir = current();
      if (!dir) return;
      // The Cryo-ET panel reads its folder again itself and hands it over (above).
      const panel = window.TifMrcViewPro?.cryoet?.reconFolder?.() === dir && known.get(dir)?.cryo;
      read(dir, panel ? known.get(dir).cryo : undefined);
    }, 1500);
  };
  document.addEventListener("tmv-folder-changed", again);
  document.addEventListener("cryoet-idle", again);
  T.event.listen("tomo-session", again).catch(() => {});
  // The list may have been built before this file was loaded.
  if (S.folder?.dir) document.dispatchEvent(new CustomEvent("tmv-strip"));
})();
