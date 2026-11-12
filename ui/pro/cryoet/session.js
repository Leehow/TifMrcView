"use strict";
// The folder flow of the Cryo-ET workspace: a project folder with many tilt
// series. 「开始」 (first pass) corrects, judges, aligns and makes a quick
// tomogram of every series; the wall shows one tile per series, a click opens
// its review (图像 / 对齐 / 断层图); 「精做」 makes the final tomograms of the
// ticked series. The program only suggests; the user's ticks are kept.
// Pure wording and rules are in rules.js. Shares app.js globals: S, T, invoke,
// $, dirname, openHere, saveDir.
(() => {
  const W = window.TifMrcViewPro.cryoet;
  const R = window.TmvCryoRules;
  const host = $("cryo-session");

  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };

  host.innerHTML = `
    <div class="al-row cs-folder"><span class="dim">项目文件夹</span><b class="cs-path" title=""></b><button class="small cs-pick" title="换一个放着倾转序列的文件夹">选文件夹…</button></div>
    <div class="al-row cs-controls">
      <button class="primary cs-act cs-start" title="对文件夹里的每个序列：校正电影、自动判断每张倾转图、对齐、做一个快速断层图。某个序列出问题不会挡住其他序列">开始</button>
      <label>用途<select class="cs-purpose">
        <option value="display" selected>展示：尽可能清晰，去噪并补缺失楔（约 9 Å 体素，较慢）</option>
        <option value="sta">子断层平均：保留高频细节，不做去噪滤波，并导出 RELION 5（约 4.5 Å 体素）</option>
      </select></label>
      <button class="cs-act cs-finish" title="只对勾选的序列做最终断层图">精做</button>
    </div>
    <p class="dim cs-help">先按「开始」，程序处理好每个序列并给出建议；在下面检查、取消不想要的序列或倾转图，再选用途按「精做」。程序只给建议，你勾选的不会被改动。</p>
    <div class="cs-wall"></div>
    <div class="cs-review" hidden></div>`;
  const q = (s) => host.querySelector(s);
  const wall = q(".cs-wall"), review = q(".cs-review");

  // ---------- pictures: small sections of project files (tomo_view) ----------

  const cache = new Map(), rev = new Map();
  const queue = [];
  let active = 0;
  const LIMIT = 4;
  function pump() {
    while (active < LIMIT && queue.length) {
      const job = queue.shift();
      active++;
      job().finally(() => { active--; pump(); });
    }
  }
  /** A decoded section {w,h,title,rgba}; cached per (path, z, max), a few requests at a time. */
  function pic(path, z, max) {
    const key = `${path}|${z}|${max}|${rev.get(path) || 0}`;
    if (!cache.has(key)) {
      const p = new Promise((resolve, reject) => {
        queue.push(() => invoke("tomo_view", { path, z, max })
          .then((buf) => {
            const d = R.decodePicture(buf);
            d ? resolve(d) : reject(new Error("空图像"));
          })
          .catch(reject));
        pump();
      });
      p.catch(() => cache.delete(key));
      cache.set(key, p);
      if (cache.size > 400) cache.delete(cache.keys().next().value);
    }
    return cache.get(key);
  }
  function draw(canvas, d) {
    canvas.width = d.w;
    canvas.height = d.h;
    canvas.getContext("2d").putImageData(new ImageData(d.rgba, d.w, d.h), 0, 0);
  }
  /** Draws a section into `box` once it is on screen (queued, so a long list is loaded a few at a time). */
  const seen = new IntersectionObserver((entries) => {
    for (const en of entries) {
      if (!en.isIntersecting) continue;
      seen.unobserve(en.target);
      en.target._load?.();
    }
  }, { root: $("cryoet-workspace"), rootMargin: "200px" });
  function lazyPic(box, path, z, max, onFail = "无图") {
    const c = el("canvas");
    box.prepend(c);
    box._load = () => pic(path, z, max).then((d) => draw(c, d)).catch(() => { c.remove(); box.prepend(el("span", "cs-none dim", onFail)); });
    seen.observe(box);
  }

  // ---------- the folder and its session ----------

  let folder = null, chose = false, session = { root: "", series: [] }, scanning = 0, rescanAgain = false, loadError = "";
  const visible = () => !W.panel.hidden;

  function defaultFolder() {
    return S.folder?.dir || (S.info ? dirname(S.info.path) : null);
  }
  function showFolder() {
    const p = q(".cs-path");
    p.textContent = folder ? (folder.split(/[\\/]/).filter(Boolean).pop() || folder) : "还没有选";
    p.title = folder || "";
    host.querySelectorAll(".cs-act").forEach((b) => { b.disabled = W.busy || !folder; });
  }

  // Takes a session; pictures of a series that got further are loaded again.
  function adopt(s) {
    for (const n of s.series) {
      const old = session.series.find((o) => o.name === n.name);
      if (old && R.signature(old) !== R.signature(n)) for (const p of [n.small, n.aligned, n.quick, n.result]) if (p) rev.set(p, (rev.get(p) || 0) + 1);
    }
    session = s;
  }

  async function scan() {
    if (!folder) { session = { root: "", series: [] }; loadError = ""; return render(); }
    if (scanning) { rescanAgain = true; return; }
    scanning = 1;
    try {
      do {
        rescanAgain = false;
        try {
          adopt(await invoke("tomo_session", { folder }));
          loadError = "";
        } catch (e) {
          session = { root: "", series: [] };
          loadError = String(e);
        }
      } while (rescanAgain);
    } finally {
      scanning = 0;
    }
    render();
    syncReview();
  }

  function summary() {
    const n = session.series.length;
    if (!n) return;
    const attn = session.series.filter(R.needsAttention).length;
    W.status(`${n} 个序列${attn ? ` · ${attn} 个需要看一下` : ""}`);
  }

  q(".cs-pick").onclick = async () => {
    let dir = null;
    try { dir = await invoke("pick_folder", { dir: folder || (S.info ? saveDir() : null) }); } catch (e) { return W.status(`无法选择文件夹：${e}`); }
    if (!dir) return;
    folder = dir;
    chose = true;
    closeReview();
    showFolder();
    await scan();
    if (!W.busy) summary();
  };

  function follow() {
    if (!chose) {
      const f = defaultFolder();
      if (f !== folder) { folder = f; closeReview(); }
    }
    showFolder();
  }
  document.addEventListener("cryoet-open", async () => { follow(); await scan(); if (!W.busy) summary(); });
  document.addEventListener("cryoet-close", () => stopMedia());
  document.addEventListener("cryoet-idle", () => { showFolder(); scan(); });
  let folderTimer = 0;
  document.addEventListener("tmv-folder-changed", () => {
    clearTimeout(folderTimer);
    folderTimer = setTimeout(() => { if (visible() && !W.busy) { follow(); scan(); } }, 700);
  });

  // ---------- the wall ----------

  const tiles = new Map();
  function render() {
    showFolder();
    const list = R.sortSeries(session.series);
    if (!list.length) {
      tiles.clear();
      const msg = loadError ? `无法读取这个文件夹：${loadError}`
        : folder ? "这个文件夹里没有倾转序列。序列是 mdoc 加它旁边的电影（EER / TIFF），或做好的叠层（带 .tlt）。"
          : "先打开一个文件夹，或点「选文件夹…」。";
      wall.replaceChildren(el("p", loadError ? "cs-err" : "dim cs-empty", msg));
      return;
    }
    wall.querySelector(".cs-empty, p.cs-err")?.remove();
    const names = new Set(list.map((s) => s.name));
    for (const [n, t] of tiles) if (!names.has(n)) { t.el.remove(); tiles.delete(n); }
    list.forEach((s, i) => {
      let t = tiles.get(s.name);
      const sig = JSON.stringify(s);
      if (!t) {
        t = { el: el("div", "cs-tile"), sig: "" };
        tiles.set(s.name, t);
      }
      if (t.sig !== sig) {
        t.sig = sig;
        fillTile(t.el, s);
      }
      if (wall.children[i] !== t.el) wall.insertBefore(t.el, wall.children[i] || null);
    });
  }

  function fillTile(tile, s) {
    const chip = R.stageChip(s), verdict = R.verdictLine(s);
    tile.className = `cs-tile${R.needsAttention(s) ? " attn" : ""}${s.chosen ? "" : " off"}`;
    tile.tabIndex = 0;
    tile.setAttribute("role", "button");
    tile.title = `打开 ${s.name} 的检查`;
    tile.replaceChildren();
    const box = el("div", "cs-pic");
    const src = s.quick ? [s.quick, null] : s.small ? [s.small, null] : null;
    if (src) lazyPic(box, src[0], src[1], 192, "无图");
    else box.append(el("span", "cs-none dim", s.stage === "movies" ? "还没处理" : "无图"));
    const take = el("label", "cs-take");
    take.title = s.take === null ? "精做时包含这个序列（现在按程序的建议）" : "精做时包含这个序列（你改过）";
    const cb = el("input");
    cb.type = "checkbox";
    cb.checked = s.chosen;
    cb.disabled = s.stage === "movies";
    cb.onchange = () => setTake(s, cb.checked);
    take.append(cb);
    take.onclick = (e) => e.stopPropagation();
    box.append(take);
    const nm = el("div", "cs-nm", s.name);
    const meta = el("div", "cs-meta");
    meta.append(el("span", `cs-chip ${chip.tone}`, chip.text));
    const out = R.leftOutText(s);
    if (out) meta.append(el("span", "dim", out));
    tile.append(box, nm, meta);
    if (verdict) tile.append(el("div", `cs-verdict ${verdict.tone}`, verdict.text));
    const res = R.residualText(s);
    if (res) tile.append(el("div", "dim", res));
    if (s.error) {
      const e = el("div", "cs-err", s.error);
      e.title = s.error;
      tile.append(e);
    }
    if (s.note) {
      const n = el("div", "dim", s.note);
      n.title = s.note;
      tile.append(n);
    }
    if (s.take !== null && s.take !== undefined) {
      const b = el("button", "small cs-reset", "按程序的");
      b.title = "你改过这个序列的勾选；点这里回到程序的建议";
      b.onclick = (e) => { e.stopPropagation(); setTake(s, null); };
      tile.append(b);
    }
    const open = () => openReview(s.name);
    tile.onclick = open;
    tile.onkeydown = (e) => { if (e.target === tile && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); open(); } };
  }

  async function setTake(s, take) {
    try {
      await invoke("tomo_take", { stack: s.stack, take });
    } catch (e) {
      W.status(`无法保存勾选：${e}`);
    }
    await scan();
  }

  // ---------- running ----------

  async function runPass(label, purpose, only) {
    const un = await T.event.listen("tomo-session", () => scan());
    const r = await W.run(label, () => invoke("tomo_session_run", { req: { folder, purpose, only } }));
    un();
    if (r) {
      adopt(r);
      render();
      syncReview();
      const attn = r.series.filter(R.needsAttention).length;
      W.status(purpose ? `${label}完成` : `${label}完成：${r.series.length} 个序列${attn ? `，${attn} 个建议看一下` : ""}`);
    }
  }
  q(".cs-start").onclick = () => {
    if (!folder) return W.status("先选项目文件夹");
    return runPass("处理", null, []);
  };
  q(".cs-finish").onclick = async () => {
    if (!folder) return W.status("先选项目文件夹");
    const only = R.chosenNames(session.series);
    if (!only.length) return W.status("先勾选要精做的序列");
    const purpose = q(".cs-purpose").value;
    return runPass("精做", purpose, only);
  };

  // ---------- the review of one series ----------

  const rv = { name: null, tab: "images", dataset: null, sig: "", timer: 0, gen: 0 };
  const find = (name) => session.series.find((s) => s.name === name);
  const bodySig = (s) => [s.stage, !!s.small, !!s.aligned, !!s.quick, !!s.result, s.views].join("|");

  function stopMedia() {
    clearInterval(rv.timer);
    rv.timer = 0;
    rv.gen++;
  }
  function closeReview() {
    stopMedia();
    rv.name = null;
    rv.dataset = null;
    review.hidden = true;
    review.replaceChildren();
    wall.hidden = false;
    host.classList.remove("reviewing");
  }

  async function openReview(name) {
    const s = find(name);
    if (!s) return;
    rv.name = name;
    rv.tab = "images";
    rv.dataset = null;
    rv.loadError = "";
    rv.sig = bodySig(s);
    wall.hidden = true;
    review.hidden = false;
    host.classList.add("reviewing");
    $("cryoet-workspace").scrollTop = 0;
    buildReview(s);
    // The existing 高级设置 steps work on the series opened here.
    if (s.stage !== "movies" || s.small) {
      try {
        rv.dataset = await W.load(s.stack);
      } catch (e) {
        rv.dataset = null;
        rv.loadError = String(e);
      }
    }
    if (rv.name === name) showTab();
  }

  function buildReview(s) {
    review.replaceChildren();
    const head = el("div", "al-head");
    const back = el("button", "small", "← 返回");
    back.onclick = closeReview;
    head.append(back, el("b", null, s.name));
    const chip = R.stageChip(s);
    head.append(el("span", `cs-chip ${chip.tone}`, chip.text));
    const info = el("div", "cs-rv-info");
    review.append(head, info);
    const tabs = el("div", "cs-tabs");
    for (const [id, label] of [["images", "图像"], ["align", "对齐"], ["tomo", "断层图"]]) {
      const b = el("button", "cs-tab", label);
      b.dataset.tab = id;
      b.onclick = () => { rv.tab = id; showTab(); };
      tabs.append(b);
    }
    review.append(tabs, el("div", "cs-tabbody"));
    infoLine(s);
  }

  function infoLine(s) {
    const info = review.querySelector(".cs-rv-info");
    if (!info) return;
    info.replaceChildren();
    const v = R.verdictLine(s);
    if (v) info.append(el("span", `cs-verdict ${v.tone}`, v.text));
    for (const t of [R.leftOutText(s), R.residualText(s)]) if (t) info.append(el("span", "dim", t));
    if (s.error) info.append(el("span", "cs-err", s.error));
    if (s.note) info.append(el("span", "dim", s.note));
  }

  // After a rescan: keep the review open, redraw the tab only when the series got further.
  function syncReview() {
    if (!rv.name) return;
    const s = find(rv.name);
    if (!s) return closeReview();
    infoLine(s);
    const sig = bodySig(s);
    if (sig !== rv.sig) {
      rv.sig = sig;
      if (s.stage !== "movies" || s.small) W.load(s.stack).then((d) => { rv.dataset = d; showTab(); }).catch(() => showTab());
      else showTab();
    }
  }

  function showTab() {
    const s = find(rv.name);
    if (!s) return;
    stopMedia();
    rv.sig = bodySig(s);
    review.querySelectorAll(".cs-tab").forEach((b) => b.classList.toggle("on", b.dataset.tab === rv.tab));
    const body = review.querySelector(".cs-tabbody");
    body.replaceChildren();
    ({ images: imagesTab, align: alignTab, tomo: tomoTab })[rv.tab](body, s);
  }

  // ----- 图像 -----

  function imagesTab(body, s) {
    if (!s.small || !rv.dataset) {
      body.append(el("p", "dim", s.stage === "movies" && !s.small
        ? "这个序列还没有处理（电影还没校正成倾转序列）。按上面的「开始」处理。"
        : !s.small ? "还没有生成倾转图的预览。" : rv.loadError ? `无法读取这个序列：${rv.loadError}` : "载入中…"));
      return;
    }
    const d = rv.dataset;
    const checks = d.checks || [];
    const sum = el("p", "cs-summary");
    const hint = el("p", "dim", "点一张图改变是否使用；程序建议去掉的标红，你的选择不会被程序改动。");
    const grid = el("div", "cs-views");
    body.append(sum, hint, grid);
    const cells = checks.map((c) => {
      const cell = el("div", "cs-view");
      const box = el("div", "cs-vpic");
      lazyPic(box, s.small, c.z, 160, "无图");
      box.title = "点击改变是否使用";
      box.onclick = () => toggle(c);
      const zoom = el("button", "small cs-zoom", "⤢");
      zoom.title = "放大看";
      zoom.onclick = (e) => { e.stopPropagation(); enlarge(s, c); };
      box.append(zoom);
      const lab = el("div", "cs-vlab");
      lab.append(el("b", null, `${c.angle.toFixed(1)}°`), el("span", "cs-vst"));
      const why = el("div", "cs-vwhy dim");
      const give = el("button", "small cs-vgive", "交还程序");
      give.title = "撤销你的选择，按程序的建议";
      give.onclick = () => setPick(c, null);
      cell.append(box, lab, why, give);
      grid.append(cell);
      return { c, cell, why, give, st: lab.querySelector(".cs-vst") };
    });
    function paint() {
      sum.textContent = R.summaryLine(checks, d.picks);
      for (const x of cells) {
        const st = R.viewState(x.c, d.picks);
        x.cell.className = `cs-view ${st.state}`;
        x.st.textContent = { used: "使用", suggested: "建议去掉", dropped: "已去掉", kept: "已保留" }[st.state];
        x.why.textContent = x.c.bad && st.state !== "dropped" ? x.c.bad : "";
        x.why.title = R.stateLabel(x.c, st);
        x.cell.title = R.stateLabel(x.c, st);
        x.give.hidden = st.pick === null;
      }
    }
    rv.paint = paint;
    paint();
  }

  let pickChain = Promise.resolve();
  function setPick(c, keep) {
    const s = find(rv.name);
    if (!s) return;
    pickChain = pickChain.then(async () => {
      try {
        const d = await invoke("tomo_pick", { stack: s.stack, z: c.z, keep });
        rv.dataset = d;
        W.setDataset(d);
        rv.paint?.();
        document.querySelector(".cs-lightbox") && lightboxPaint();
      } catch (e) {
        W.status(`无法保存选择：${e}`);
      }
      scan();
    });
    return pickChain;
  }
  const toggle = (c) => setPick(c, R.toggleKeep(c, rv.dataset.picks));

  // A view bigger (tomo_view max 768), with the same choice buttons.
  let lightboxPaint = () => {};
  function enlarge(s, c) {
    document.querySelector(".cs-lightbox")?.remove();
    const back = el("div", "cs-lightbox");
    const card = el("div", "cs-lb-card");
    const cv = el("canvas");
    const cap = el("div", "cs-lb-cap");
    const act = el("button", "small");
    const close = el("button", "small", "关闭");
    const bar = el("div", "al-row");
    bar.append(act, close);
    card.append(cv, cap, bar);
    back.append(card);
    const done = () => { back.remove(); document.removeEventListener("keydown", onKey, true); lightboxPaint = () => {}; };
    const onKey = (e) => { if (e.key === "Escape") { e.stopPropagation(); done(); } };
    document.addEventListener("keydown", onKey, true);
    back.onclick = (e) => { if (e.target === back) done(); };
    close.onclick = done;
    act.onclick = () => toggle(c);
    lightboxPaint = () => {
      const st = R.viewState(c, rv.dataset.picks);
      cap.textContent = `${c.angle.toFixed(1)}° · ${R.stateLabel(c, st)}`;
      act.textContent = st.used ? "去掉这张" : "用这张";
    };
    lightboxPaint();
    document.body.append(back);
    pic(s.small, c.z, 768).then((d) => draw(cv, d)).catch(() => { cap.textContent += " · 无法载入大图"; });
  }

  // ----- 对齐 -----

  function alignTab(body, s) {
    if (!s.aligned) {
      body.append(el("p", "dim", s.stage === "movies" || s.stage === "stack" ? "这个序列还没有对齐。" : "没有对齐结果。"));
      if (s.error) body.append(el("p", "cs-err", s.error));
      return;
    }
    const al = rv.dataset?.alignment || null;
    const framesMeta = R.alignedViews(al);
    const n = framesMeta.length || Math.max(1, s.views - s.leftOut);
    const head = el("div", "al-row");
    const text = el("span", "cs-aligntext", R.residualText(s) || (al ? `残差 ${al.residual.toFixed(2)} px${al.quality ? ` · ${al.quality}` : ""}` : "对齐后的倾转序列"));
    const open = el("button", "small", "在主窗口打开");
    open.onclick = () => openHere(s.aligned);
    head.append(text, open);
    const cv = el("canvas", "cs-film");
    const ctl = el("div", "al-row cs-filmctl");
    const play = el("button", "small", "暂停");
    const slider = el("input");
    slider.type = "range";
    slider.min = 0;
    slider.max = n - 1;
    slider.value = 0;
    slider.className = "cs-slider";
    const lab = el("span", "dim cs-framelab", "载入中…");
    ctl.append(play, slider, lab);
    body.append(head, cv, ctl);
    const bars = barChart(al);
    if (bars) body.append(bars.node);

    const frames = new Array(n).fill(null);
    let i = 0, playing = true, loaded = 0;
    const gen = rv.gen;
    const show = (k) => {
      i = k;
      slider.value = k;
      if (frames[k]) draw(cv, frames[k]);
      const m = framesMeta[k];
      lab.textContent = `${k + 1} / ${n}${m ? ` · ${m.angle.toFixed(1)}°` : ""}${loaded < n ? ` · 载入 ${loaded}/${n}` : ""}`;
      bars?.mark(m);
    };
    for (let k = 0; k < n; k++) {
      pic(s.aligned, k, 512).then((d) => {
        if (gen !== rv.gen) return;
        frames[k] = d;
        loaded++;
        show(i);
      }).catch(() => { if (gen === rv.gen) { loaded++; lab.textContent = "有些帧无法载入"; } });
    }
    rv.timer = setInterval(() => {
      if (!playing || loaded < n) return;
      show((i + 1) % n);
    }, 125);
    play.onclick = () => {
      playing = !playing;
      play.textContent = playing ? "暂停" : "播放";
    };
    slider.oninput = () => { playing = false; play.textContent = "播放"; show(+slider.value); };
  }

  // Per-view residual by angle; views left out of the alignment are grey.
  function barChart(al) {
    const views = [...(al?.views || [])].sort((a, b) => a.angle - b.angle);
    if (!views.length) return null;
    const top = R.residualScale(views.filter((v) => !v.excluded));
    const box = el("div", "cs-bars");
    const NS = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(NS, "svg");
    svg.setAttribute("viewBox", `0 0 ${views.length} 100`);
    svg.setAttribute("preserveAspectRatio", "none");
    const rects = new Map();
    views.forEach((v, k) => {
      const r = document.createElementNS(NS, "rect");
      const h = Math.max(1, Math.min(100, ((v.residual || 0) / top) * 100));
      r.setAttribute("x", k + 0.1);
      r.setAttribute("width", 0.8);
      r.setAttribute("y", 100 - h);
      r.setAttribute("height", h);
      r.setAttribute("class", v.excluded ? "ex" : "");
      const t = document.createElementNS(NS, "title");
      t.textContent = `${v.angle.toFixed(1)}° · 残差 ${(v.residual || 0).toFixed(2)} px${v.excluded ? "（未参与对齐）" : ""}`;
      r.append(t);
      svg.append(r);
      rects.set(v, r);
    });
    const axis = el("div", "cs-axis dim");
    axis.append(el("span", null, `${views[0].angle.toFixed(0)}°`), el("span", null, `每张的残差（最高 ${top.toFixed(2)} px，灰色 = 没参与对齐）`), el("span", null, `${views[views.length - 1].angle.toFixed(0)}°`));
    box.append(svg, axis);
    let cur = null;
    return {
      node: box,
      mark(v) {
        cur?.classList.remove("cur");
        cur = v ? rects.get(v) || null : null;
        cur?.classList.add("cur");
      },
    };
  }

  // ----- 断层图 -----

  function tomoTab(body, s) {
    const t = R.tomogramOf(s);
    if (!t) {
      body.append(el("p", "dim", "还没有断层图。对齐之后会自动做一个快速断层图。"));
      if (s.error) body.append(el("p", "cs-err", s.error));
      return;
    }
    const head = el("div", "al-row");
    const text = el("span", "cs-aligntext", t.label);
    const open = el("button", "small", "在主窗口打开");
    open.onclick = () => openHere(t.path);
    head.append(text, open);
    const cv = el("canvas", "cs-film");
    const ctl = el("div", "al-row cs-filmctl");
    const slider = el("input");
    slider.type = "range";
    slider.className = "cs-slider";
    slider.disabled = true;
    const lab = el("span", "dim cs-framelab", "载入中…");
    ctl.append(el("span", "dim", "层"), slider, lab);
    body.append(head, cv, ctl);
    // The depth comes from the picture's title, else from the dataset's record of this tomogram.
    let depth = 0, want = 0;
    const gen = rv.gen;
    const fromDataset = () => rv.dataset?.tomograms?.find((x) => x.path === t.path)?.thickness || 0;
    async function load(z) {
      const mine = ++want;
      try {
        const d = await pic(t.path, z, 512);
        if (mine !== want || gen !== rv.gen) return;
        draw(cv, d);
        depth = depth || R.depthFromTitle(d.title) || fromDataset();
        if (depth && slider.disabled) {
          slider.max = depth - 1;
          slider.value = z ?? Math.floor(depth / 2);
          slider.disabled = false;
        }
        lab.textContent = depth ? `${(z ?? Math.floor(depth / 2)) + 1} / ${depth}` : "中间一层";
      } catch (e) {
        if (mine === want) lab.textContent = "无法载入";
      }
    }
    slider.oninput = () => load(+slider.value);
    load(null);
  }

  // The panel's first open scans too (cryoet-open); also when scripts load with it already open.
  follow();
  if (visible()) scan();
})();
