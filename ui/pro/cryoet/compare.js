"use strict";
// The 对照 pane: while the main window shows a tilt series of the Cryo-ET
// folder flow, a small pane in the corner of the stage shows its tomogram,
// and the other way round. A click on the pane's picture swaps the two (the
// counterpart opens in, or goes to, its own tab, so going back and forth
// keeps each one's place); the pane folds to its title bar.
// The pictures are the same previews the series review uses (tomo_view after
// tomo_preview_file), so no second viewer is built.
// session.js tells it the series it knows: learn(series) and open(series, side).
(() => {
  const R = window.TmvCryoRules;
  const W = window.TifMrcViewPro.cryoet;
  const api = window.TifMrcViewPro.cryoCompare = {};
  const LABEL = { tilt: "倾转序列", tomo: "断层图" };

  const pane = document.createElement("aside");
  pane.id = "cryo-compare";
  pane.hidden = true;
  pane.setAttribute("aria-label", "对照小窗");
  pane.innerHTML = `
    <div class="cc-head">
      <b class="cc-title"></b><span class="cc-name dim"></span>
      <button class="small cc-swap" title="和主窗口交换（也可以直接点图）"><svg viewBox="0 0 24 24"><path d="M7 4L3 8l4 4"/><path d="M3 8h14"/><path d="M17 20l4-4-4-4"/><path d="M21 16H7"/></svg></button>
      <button class="small cc-fold" title="折叠 / 展开"><svg viewBox="0 0 24 24"><path d="M6 15l6-6 6 6"/></svg></button>
    </div>
    <div class="cc-body">
      <div class="cc-pic" title="点击：和主窗口交换"><canvas></canvas><span class="cc-state dim">正在读取预览…</span></div>
      <div class="cc-ctl"><input type="range" class="cc-slider" min="0" max="0" value="0" aria-label="对照小窗的层"><span class="cc-lab dim"></span></div>
    </div>`;
  $("stage").append(pane);
  const $$ = (s) => pane.querySelector(s);
  const cv = $$("canvas"), state = $$(".cc-state"), slider = $$(".cc-slider"), lab = $$(".cc-lab");

  // Every series path the flow made, to the series it belongs to.
  const known = new Map();
  let expect = null;          // {pair, side} of an open this pane (or the review) asked for
  let cur = null;             // {pair, side} shown in the main window
  let gen = 0, z = 0, depth = 0, painted = -1;
  const frames = new Map(), lastZ = new Map();
  const prepared = new Map();

  const pairOf = (s) => {
    const t = R.tomogramOf(s);
    const tilt = s.aligned || s.stack || null;
    return tilt && t ? { name: s.name, tilt, tomo: t.path, tiltLabel: s.aligned ? "对齐后的倾转序列" : "倾转序列", tomoLabel: t.label, stack: s.stack } : null;
  };
  api.learn = (series) => {
    for (const s of series || []) {
      const p = pairOf(s);
      if (!p) continue;
      known.set(p.tilt, { pair: p, side: "tilt" });
      if (s.stack && s.stack !== p.tilt) known.set(s.stack, { pair: p, side: "tilt" });
      known.set(p.tomo, { pair: p, side: "tomo" });
    }
    sync();
  };
  /** Opens one side of series `s` in the main window, as 在主窗口打开 does. */
  api.open = (s, side) => {
    const p = pairOf(s);
    const path = side === "tilt" ? s.aligned : R.tomogramOf(s)?.path;
    if (p) { api.learn([s]); expect = { pair: p, side }; }
    return openHere(path);
  };

  const fold = (on) => {
    pane.classList.toggle("folded", on);
    $$(".cc-fold").setAttribute("aria-expanded", String(!on));
    pref.set("cryo-compare-folded", on);
  };
  fold(!!pref.get("cryo-compare-folded", false));
  $$(".cc-fold").onclick = () => {
    fold(!pane.classList.contains("folded"));
    // A canvas that was hidden may have lost its pixels: draw it again.
    if (!pane.classList.contains("folded") && cur && depth) { painted = -1; paint(); }
  };

  // Which series the main window shows now.
  function sync() {
    const path = S.info?.path;
    const docs = window.TifMrcViewPro.documents;
    const merge = docs?.current?.()?.type === "merge";
    let hit = path ? known.get(path) || known.get(window.TmvRemote?.displayPath(path)) : null;
    if (!hit && expect && path) { hit = expect; known.set(path, expect); }
    if (path) expect = null;
    if (!hit || merge || !S.info) { cur = null; pane.hidden = true; return; }
    if (cur && cur.pair.tilt === hit.pair.tilt && cur.side === hit.side && !pane.hidden) return;
    cur = hit;
    pane.hidden = false;
    show();
    side();
  }
  const other = () => (cur.side === "tilt" ? "tomo" : "tilt");
  const otherPath = () => cur.pair[other()];

  async function show() {
    const g = ++gen, path = otherPath();
    frames.clear(); painted = -1; depth = 0;
    $$(".cc-title").textContent = other() === "tilt" ? cur.pair.tiltLabel : cur.pair.tomoLabel;
    $$(".cc-name").textContent = cur.pair.name;
    $$(".cc-name").title = W.name(path);
    slider.disabled = true;
    state.hidden = false;
    state.textContent = "正在读取预览…";
    lab.textContent = "";
    try {
      if (!prepared.has(path)) prepared.set(path, invoke("tomo_preview_file", { path }).catch(() => {}));
      await prepared.get(path);
      if (g !== gen) return;
      const d = await get(null, g);
      if (g !== gen) return;
      depth = R.depthFromTitle(d.title) || 1;
      slider.max = depth - 1;
      slider.disabled = depth < 2;
      // The tilt series opens on its middle view (about 0°), the tomogram on its middle section.
      z = Math.min(depth - 1, lastZ.has(path) ? lastZ.get(path) : Math.floor(depth / 2));
      slider.value = z;
      paint();
    } catch (e) {
      if (g === gen) state.textContent = `无法读取预览：${e}`;
    }
  }
  function get(k, g) {
    const key = k == null ? "mid" : k;
    if (!frames.has(key)) {
      const p = invoke("tomo_view", { path: otherPath(), z: k, max: 512 }).then((buf) => {
        const d = R.decodePicture(buf);
        if (!d) throw new Error("空图像");
        return d;
      });
      p.catch(() => { if (g === gen) frames.delete(key); });
      frames.set(key, p);
    }
    return frames.get(key);
  }
  async function paint() {
    const g = gen, k = z;
    lab.textContent = `${k + 1} / ${depth}`;
    if (painted === k) return;
    try {
      const d = await get(k, g);
      if (g !== gen || k !== z) return;
      cv.width = d.w; cv.height = d.h;
      cv.getContext("2d").putImageData(new ImageData(d.rgba, d.w, d.h), 0, 0);
      painted = k;
      state.hidden = true;
      // The next ones are read ahead so dragging stays smooth.
      for (const n of [k + 1, k - 1, k + 2]) if (n >= 0 && n < depth) get(n, g).catch(() => {});
    } catch (e) {
      if (g === gen && k === z) { state.hidden = false; state.textContent = `第 ${k + 1} 层读取失败：${e}`; }
    }
  }
  const go = (k) => {
    z = Math.max(0, Math.min(depth - 1, k));
    slider.value = z;
    lastZ.set(otherPath(), z);
    paint();
  };
  slider.oninput = () => go(+slider.value);
  $$(".cc-pic").addEventListener("wheel", (e) => {
    if (!depth) return;
    e.preventDefault();
    go(z + (e.deltaY > 0 ? 1 : -1));
  }, { passive: false });

  // Swap: the counterpart goes to the main window at the section the pane shows,
  // the image that was there comes into the pane where it was.
  async function swap() {
    if (!cur) return;
    const from = { pair: cur.pair, side: cur.side }, to = other(), path = cur.pair[to];
    const at = depth ? z : null, mainPath = S.info?.path, mainZ = S.z;
    lastZ.set(cur.pair[from.side], mainZ);
    expect = { pair: from.pair, side: to };
    const docs = window.TifMrcViewPro.documents;
    try {
      const remote = !!window.TmvRemote && window.TmvRemote.displayPath(path) !== path;
      if (remote || !docs) await openHere(path);
      else await docs.open(path, false, 0, { replace: false });
    } catch (e) { toast(String(e)); return; }
    if (S.info?.path !== mainPath && at != null && S.info?.sizeZ > at && S.axis === "xy") { setZ(at); requestRender(); }
  }
  $$(".cc-pic").onclick = swap;
  $$(".cc-swap").onclick = swap;

  // The Cryo-ET panel floats where the user put it: the pane takes the side it leaves free.
  function side() {
    const panel = $("cryoet-workspace");
    if (!panel || panel.hidden || pane.hidden) return pane.classList.remove("left");
    const s = $("stage").getBoundingClientRect(), r = panel.getBoundingClientRect();
    pane.classList.toggle("left", r.left + r.width / 2 >= s.left + s.width / 2);
  }
  if ($("cryoet-workspace")) new MutationObserver(side).observe($("cryoet-workspace"), { attributes: true, attributeFilter: ["hidden", "style", "class"] });

  document.addEventListener("tmv-opened", () => setTimeout(sync, 0));
  document.addEventListener("tmv-document", () => setTimeout(sync, 0));
})();
