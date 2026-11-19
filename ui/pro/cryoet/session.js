"use strict";
// The folder flow of the Cryo-ET workspace: a project folder with many tilt
// series. 「开始」 (first pass) corrects, judges, aligns and makes a quick
// tomogram of every series; the wall shows one tile per series, a click opens
// its review (图像 / 对齐 / 断层图); 「开始重构」 makes the final tomograms of the
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
      <div class="cs-purpose-box" role="group" aria-label="重构时要做的处理">
        <label class="cs-use"><input type="checkbox" data-step="fineAlign"><span><b>精对齐</b><span>多试几种对齐方式，自动去掉对不齐的倾角（会标出来，可以改回）</span></span></label>
        <label class="cs-use"><input type="checkbox" data-step="denoise"><span><b>N2N 降噪</b><span>去掉噪点，结构更干净（有电影数据时效果最好）</span></span></label>
        <label class="cs-use"><input type="checkbox" data-step="wedge"><span><b>去缺失楔</b><span>减轻上下方向的拉长和模糊；比较慢（台式机 GPU 每个序列约 20 分钟，更慢的电脑会更久），但一定做完</span></span></label>
        <label class="cs-use"><input type="checkbox" data-step="flatten"><span><b>自动展平</b><span>把斜着的薄片转平，翻层时整层都在样品里</span></span></label>
      </div>
      <label class="cs-bin" title="每个体素合并多少个原始像素（bin）。bin 越小细节越多，但越慢、文件越大">体素大小 <select class="cs-bin-pick" aria-label="体素大小（bin）"></select></label>
      <p class="dim cs-steps-line" role="status"></p>
      <button class="primary cs-act cs-finish" title="只对勾选的序列做最终断层图；还没处理过的序列会先处理">重构勾选序列</button>
      <span class="dim cs-selection-count" role="status"></span>
    </div>
    <p class="dim cs-help cs-only-recon">在下面勾选序列，再选想要的处理（都不勾就是快速重构，几分钟一个，适合先看看样品），点「重构勾选序列」。没处理过的电影会先自动校正。点卡片上的「打开」检查倾转图、对齐和断层图，挑掉不好的图后可以再重构。</p>
    <p class="dim cs-discovery" role="status"></p>
    <div class="al-row cs-selection"><input class="cs-search" type="search" placeholder="按序列名称搜索…" aria-label="搜索序列"><button class="small cs-select-all">全选</button><button class="small cs-select-none">全不选</button></div>
    <div class="cs-wall"></div>
    <div class="cs-review" hidden></div>`;
  const q = (s) => host.querySelector(s);
  const wall = q(".cs-wall"), review = q(".cs-review");

  // ---------- pictures: small sections of project files (tomo_view) ----------

  const cache = new Map(), wallCache = new Map(), rev = new Map();
  const queue = [];
  let active = 0;
  const LIMIT = 4;
  function pump() {
    while (active < LIMIT && queue.length) {
      const job = queue.shift(); active++;
      job.run().finally(() => { active--; pump(); });
    }
  }
  /** Interactive frames precede background thumbnails; only small pictures travel. */
  function pic(path, z, max, group = null, priority = max >= 512) {
    const key = `${path}|${z}|${max}|${rev.get(path) || 0}`;
    const store=group==null?wallCache:cache;
    if (!store.has(key)) {
      let rejectJob;
      const p = new Promise((resolve, reject) => {
        rejectJob = reject;
        const job = { group, key, store, run: () => invoke("tomo_view", { path, z, max })
          .then(buf => { const d = R.decodePicture(buf); d ? resolve(d) : reject(new Error("空图像")); }).catch(reject) };
        // The wall's thumbnails go before a review's preloading of sections.
        if (priority) queue.unshift(job);
        else if (group == null) { const at = queue.findIndex((j) => j.group != null); at < 0 ? queue.push(job) : queue.splice(at, 0, job); }
        else queue.push(job);
        pump();
      });
      p.catch(() => { if (store.get(key) === p) store.delete(key); });
      store.set(key, p);
      p.cancelQueued = () => { if (store.get(key) === p) store.delete(key); rejectJob(new Error('数据集已切换')); };
      if(group==null&&store.size>400)store.delete(store.keys().next().value);
    }
    if (priority) {
      const at = queue.findIndex(job => job.key === key);
      if (at > 0) queue.unshift(queue.splice(at, 1)[0]);
    }
    return store.get(key);
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
    // Until the picture arrives the tile says so (a server may take seconds).
    const wait = el("span", "cs-none dim cs-wait", "正在加载…");
    box.prepend(c, wait);
    // Kept as an image, not a canvas: WebKit may drop a hidden canvas's
    // pixels (the wall is hidden during a review, the app goes to the
    // background) and nothing would draw it again.
    box._load = () => pic(path, z, max).then((d) => {
      draw(c, d);
      const img = el("img");
      img.alt = "";
      img.src = d.url ||= c.toDataURL();
      c.replaceWith(img);
      wait.remove();
    }).catch(() => { c.remove(); wait.remove(); box.prepend(el("span", "cs-none dim", onFail)); });
    seen.observe(box);
  }

  // ---------- the folder and its session ----------

  let folder = null, chose = false, session = { root: "", series: [] }, scanning = 0, rescanAgain = false, loadError = "";
  const selected=new Set();
  const selectedNames=()=>session.series.filter(s=>selected.has(s.name)).map(s=>s.name);
  const matches=s=>s.name.toLocaleLowerCase().includes(q('.cs-search').value.trim().toLocaleLowerCase());
  function selectionChanged(){
    for(const [name,t]of tiles){const on=selected.has(name);t.el.classList.toggle('selected',on);const cb=t.el.querySelector('.cs-take input');if(cb){cb.checked=on;cb.disabled=W.busy;}}
    const shown=session.series.filter(matches).length;
    q('.cs-selection-count').textContent=`已勾选 ${selected.size} 个序列${q('.cs-search').value?` · 筛选显示 ${shown} 个`:''}`;
    const filtered=!!q('.cs-search').value.trim();q('.cs-select-all').textContent=filtered?'全选筛选结果':'全选';q('.cs-select-none').textContent=filtered?'全不选筛选结果':'全不选';
    q('.cs-select-all').disabled=q('.cs-select-none').disabled=W.busy;
    host.querySelectorAll('.cs-act').forEach(b=>b.disabled=W.busy||!folder||!selected.size);
    refreshSteps();
  }
  let refreshSteps=()=>{};
  function toggleSelection(name,on=!selected.has(name)){if(W.busy)return;on?selected.add(name):selected.delete(name);selectionChanged();}
  q('.cs-search').oninput=()=>{for(const [name,t]of tiles)t.el.hidden=!matches(find(name));selectionChanged();};
  q('.cs-select-all').onclick=()=>{if(W.busy)return;for(const s of session.series.filter(matches))selected.add(s.name);selectionChanged();};
  q('.cs-select-none').onclick=()=>{if(W.busy)return;for(const s of session.series.filter(matches))selected.delete(s.name);selectionChanged();};
  function changedFolder(){selected.clear();q('.cs-search').value='';closeReview();stopMedia();rv.name=null;rv.dataset=null;for(const job of queue.splice(0))job.store.get(job.key)?.cancelQueued?.();wallCache.clear();rev.clear();}
  const visible = () => !W.panel.hidden;
  // The project folder of 三维重构: the 子断层平均 part lists the tomograms made there (sta/index.js).
  W.reconFolder = () => folder;

  function defaultFolder() {
    return S.folder?.dir || (S.info ? dirname(S.info.path) : null);
  }
  function showFolder() {
    const p = q(".cs-path");
    p.textContent = folder ? (folder.split(/[\\/]/).filter(Boolean).pop() || folder) : "还没有选";
    p.title = folder || "";
    host.querySelectorAll(".cs-act").forEach((b) => { b.disabled = W.busy || !folder || !selected.size; });
    host.querySelectorAll(".cs-purpose-box input").forEach((b) => { b.disabled = W.busy; });
  }

  // Takes a session; pictures of a series that got further are loaded again.
  function adopt(s) {
    for (const n of s.series) {
      const old = session.series.find((o) => o.name === n.name);
      if (old && bodySig(old) !== bodySig(n)) for (const p of [n.small, n.aligned, n.quick, n.result]) if (p) rev.set(p, (rev.get(p) || 0) + 1);
    }
    session = s;
    // The sidebar's 「处理结果」 lists the same reading (pro/results.js).
    document.dispatchEvent(new CustomEvent("cryoet-session", { detail: { folder, session: s } }));
    for(const name of selected)if(!s.series.some(s=>s.name===name))selected.delete(name);
  }

  async function scan() {
    if (!folder) { session = { root: "", series: [] }; loadError = ""; return render(); }
    if (scanning) { rescanAgain = true; return; }
    scanning = 1;
    // The first reading of a folder (on a server it can take a while) says so.
    const first = !session.series.length;
    if (first) {
      if (!W.busy) W.status("正在读取文件夹里的倾转序列…");
      wall.replaceChildren(el("p", "dim cs-empty", "正在读取文件夹里的倾转序列…"));
      tiles.clear();
    }
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
    if (first && !W.busy) { W.status(""); summary(); }
  }

  function summary() {
    const n = session.series.length;
    if (!n) return;
    const attn = session.series.filter(R.needsAttention).length;
    W.status(`${n} 个序列${attn ? ` · ${attn} 个需要看一下` : ""}`);
  }

  q(".cs-pick").onclick = async () => {
    let dir = null;
    try { dir = await invoke("pick_folder", { dir: folder || (S.info ? saveDir() : "") }); } catch (e) { return W.status(`无法选择文件夹：${e}`); }
    if (!dir) return;
    folder = dir;
    chose = true;
    changedFolder();
    showFolder();
    await scan();
    if (!W.busy) summary();
  };

  function follow() {
    if (!chose || S.folder?.remote) {
      const f = defaultFolder();
      if (f !== folder) { folder = f; changedFolder(); }
    }
    showFolder();
  }
  document.addEventListener("cryoet-open", async () => { follow();if(rv.name&&!review.hidden)rv.open=true;await scan(); if (!W.busy) summary(); });
  document.addEventListener("cryoet-close", () => {rv.open=false;rv.closeLightbox?.();});
  document.addEventListener("cryoet-idle", () => { showFolder(); scan(); });
  let folderTimer = 0;
  document.addEventListener("tmv-remote-mounted", () => { if (visible() && !W.busy) { follow(); scan(); } });
  document.addEventListener("tmv-folder-changed", () => {
    clearTimeout(folderTimer);
    folderTimer = setTimeout(() => { if (visible() && !W.busy) { follow(); scan(); } }, 700);
  });

  // ---------- the wall ----------

  const tiles = new Map();
  function render() {
    showFolder();
    q('.cs-discovery').textContent=[...(session.warnings||[]),(session.gains||[]).length===1?`增益参考：${W.name(session.gains[0])}`:(session.gains||[]).length>1?'发现多个增益参考，开始前需明确选择或缩小范围':''].filter(Boolean).join('；');
    const list = R.sortSeries(session.series);
    selectionChanged();
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
      const sig = JSON.stringify([s, W.busy, lastRun()?.only?.includes(s.name) ?? false]);
      if (!t) {
        t = { el: el("div", "cs-tile"), sig: "" };
        tiles.set(s.name, t);
      }
      if (t.sig !== sig) {
        t.sig = sig;
        fillTile(t.el, s);
      }
      t.el.hidden=!matches(s);
      if (wall.children[i] !== t.el) wall.insertBefore(t.el, wall.children[i] || null);
    });
    selectionChanged();
  }

  // The movie-matching note in a few words (the whole note is the line's tooltip).
  const shortNote = (t) => { const m = t.match(/采集记录共 (\d+) 个电影，匹配 (\d+) 个/); return m ? `电影匹配 ${m[2]}/${m[1]}，其余跳过` : t; };
  function fillTile(tile, s) {
    const chip = R.stageChip(s), verdict = R.verdictLine(s);
    tile.className = `cs-tile${R.needsAttention(s) ? " attn" : ""}${selected.has(s.name)?' selected':''}`;
    tile.tabIndex = 0;
    tile.setAttribute("role", "group");tile.setAttribute('aria-label',s.name);
    tile.title = `点击勾选 ${s.name}`;
    tile.replaceChildren();
    const box = el("div", "cs-pic");
    const src = s.result ? [s.result, null] : s.quick ? [s.quick, null] : s.small ? [s.small, null] : null;
    if (src) lazyPic(box, src[0], src[1], 192, "无图");
    else box.append(el("span", "cs-none dim", s.stage === "movies" ? "还没处理" : "无图"));
    const take = el("label", "cs-take");
    take.title = "加入本次处理范围";
    const cb = el("input");
    cb.type = "checkbox";
    cb.checked = selected.has(s.name);cb.setAttribute('aria-label',`处理 ${s.name}`);
    cb.disabled = W.busy;
    cb.onchange = () => toggleSelection(s.name, cb.checked);
    take.append(cb);
    take.onclick = (e) => e.stopPropagation();
    box.append(take);
    // Card text, top to bottom: name; state chips; one line of numbers; what was done; a note or
    // an error (one line each, the whole text on hover); the buttons at the bottom of every card.
    const nm = el("div", "cs-nm", s.name);
    nm.title = s.name;
    const meta = el("div", "cs-meta");
    meta.append(el("span", `cs-chip ${chip.tone}`, chip.text));
    if (verdict) {
      const v = el("span", `cs-chip cs-verdict ${verdict.tone}`, verdict.tone === "ok" ? "可用" : verdict.tone === "warn" ? "看一下" : "不建议用");
      v.title = verdict.text;
      meta.append(v);
    }
    tile.append(box, nm, meta);
    const facts = [];
    if (s.bin) facts.push(s.pixelA > 0 ? `bin ${s.bin} · ${fmtA(s.pixelA * s.bin)} Å · 分辨率 ${fmtA(2 * s.pixelA * s.bin)} Å` : `bin ${s.bin}`);
    const nums = [R.leftOutText(s), s.residual != null ? `残差 ${s.residual.toFixed(1)} px` : ""].filter(Boolean).join(" · ");
    for (const t of facts) tile.append(el("div", "cs-line", t));
    if (nums) { const l = el("div", "cs-line dim", nums); l.title = R.residualText(s); tile.append(l); }
    if (s.result) tile.append(el("div", "cs-line dim cs-done", s.done?.length ? `已做：${s.done.join(" · ")}` : "已做：快速重构"));
    if (verdict && verdict.tone !== "ok") { const l = el("div", `cs-line cs-verdict ${verdict.tone}`, verdict.text); l.title = verdict.text; tile.append(l); }
    if (s.error) { const e = el("div", "cs-line cs-err", s.error); e.title = s.error; tile.append(e); }
    if (s.note) { const n = el("div", "cs-line dim", shortNote(s.note)); n.title = s.note; tile.append(n); }
    const acts = el("div", "cs-card-acts");
    acts.onclick = (e) => e.stopPropagation();
    if(s.aligned||s.small||s.quick||s.result){const open=el('button','small cs-open','打开');open.title=`打开 ${s.name} 的结果`;open.onclick=e=>{e.stopPropagation();openReview(s.name);};acts.append(open);}
    tile.append(acts);
    cardActions(acts, s);
    tile.onclick=e=>{if(!e.target.closest('button,label,input'))toggleSelection(s.name);};
    tile.onkeydown=e=>{if(e.target===tile&&(e.key==='Enter'||e.key===' ')){e.preventDefault();toggleSelection(s.name);}};
  }

  // ---------- running ----------

  // ---------- continue and reset, on each series' card ----------

  // The run started last in a folder, until it finishes. A series of it that was not finished gets
  // 「继续」 on its card: the same processing again, and the steps that were done are not done again
  // (every step skips what is already there), so a run that was stopped, or cut off by closing the
  // app, goes on from where it stopped. A series that is part-way without such a run continues with
  // the processing ticked now. 「重置」 removes what was made of that series (asked once more on the
  // card first), so its next run starts from its movies again.
  const lastKey = () => (folder ? `cryo-last-run:${folder}` : null);
  const lastRun = () => { const k = lastKey(); if (!k) return null; try { return pref.get(k, null); } catch { return null; } };
  // 「继续」 only for a series left part-way: one of the last run that it did not finish, one whose
  // movies were only partly corrected, or one whose last run stopped with an error. A series with
  // its quick or final tomogram is not part-way.
  const unfinished = (s) => {
    if (s.stage === "done") return undefined;
    const last = lastRun();
    if (last?.only?.includes(s.name)) return last.purpose ?? null;
    const partway = (s.stage === "movies" && s.corrected > 0) || (s.error && s.stage !== "quick");
    return partway ? { custom: { ...steps } } : undefined;
  };
  const made = (s) => s.stage !== "movies" || s.corrected > 0 || !!s.small || !!s.error;
  // A modal question; answers true when the user confirms.
  function ask(title, text, ok) {
    return new Promise((resolve) => {
      const d = el("dialog", "cs-ask");
      d.append(el("h3", null, title), el("p", null, text));
      const row = el("div", "cs-ask-row"), no = el("button", null, "取消"), go = el("button", "danger", ok);
      row.append(no, go);
      d.append(row);
      const done = (v) => { d.close(); d.remove(); resolve(v); };
      no.onclick = () => done(false);
      go.onclick = () => done(true);
      d.oncancel = (e) => { e.preventDefault(); done(false); };
      document.body.append(d);
      d.showModal();
      no.focus();
    });
  }
  function cardActions(row, s) {
    const purpose = unfinished(s);
    if (purpose !== undefined) {
      const b = el("button", "small cs-resume", "继续");
      b.title = "接着做这个序列没做完的处理，做完的步骤不再重做";
      b.disabled = W.busy;
      b.onclick = () => {
        if (W.busy) return;
        selected.clear();
        selected.add(s.name);
        selectionChanged();
        runSelected(purpose);
      };
      row.append(b);
    }
    if (made(s)) {
      const b = el("button", "small cs-reset", "清理");
      b.title = "删掉这个序列做出的中间结果，下次从头处理";
      b.disabled = W.busy;
      b.onclick = async () => {
        if (W.busy) return;
        if (!await ask(`清理 ${s.name}？`, "会删掉这个序列做出的校正叠层、断层图和清晰版，下次从头处理。原始数据（电影、mdoc）不会动。删掉的文件不能恢复。", "清理")) return;
        if (W.busy) return;
        try {
          await invoke("tomo_session_reset", { folder, name: s.name });
          rev.clear();
          await scan();
          document.dispatchEvent(new CustomEvent("tmv-folder-changed"));
          W.status(`已清理 ${s.name}：下次从头处理`);
        } catch (e) { W.status(`无法清理 ${s.name}：${e}`); }
      };
      row.append(b);
    }
  }

  async function runPass(label, purpose, only, call = () => invoke("tomo_session_run", { req: { folder, purpose, only } })) {
    const un = await T.event.listen("tomo-session", async () => {
      // The image list reads the folder again when a series has new files.
      const files = () => session.series.map(bodySig).join("\n");
      const before = files();
      await scan();
      if (files() !== before) document.dispatchEvent(new CustomEvent("tmv-folder-changed"));
    });
    const key = lastKey();
    if (key) pref.set(key, { purpose, only, label });
    let r;
    try { const running = W.run(label, call); render(); r = await running; } finally { un(); }
    // Finished: nothing to continue. Stopped or failed: the run is kept, to be continued (the cards' 继续).
    if (r && key) pref.set(key, null);
    render();
    if (r) {
      adopt(r);
      render();
      syncReview();
      const attn = r.series.filter(R.needsAttention).length;
      W.status(purpose ? `${label}完成，断层图在右侧栏「④ 清晰版」里，可以接着标结构、出三维图${noteOf(r, only)}` : `${label}完成：${r.series.length} 个序列${attn ? `，${attn} 个建议看一下` : ""}`);
    }
  }
  // The steps of the one pipeline the user ticked (kept; first use: none =
  // the fast tomogram). Order and settings stay the program's choice.
  // Subtomogram averaging is not asked here: its part of the panel makes
  // what it needs itself.
  const STEP_NAMES = { fineAlign: "精对齐", denoise: "N2N 降噪", wedge: "去缺失楔", flatten: "自动展平" };
  let steps = { fineAlign: false, denoise: false, wedge: false, flatten: false, bin: null };
  try { Object.assign(steps, pref.get("cryo-steps", {}) || {}); } catch {}
  const boxes = [...host.querySelectorAll(".cs-purpose-box input[data-step]")];
  const binPick = q(".cs-bin-pick");
  // The binning: the program's (as the engine chooses it, `auto::output_bin`)
  // or one the user picked, each with the voxel size it gives for the series
  // ticked (else all of them), from their pixel size.
  const BINS = [1, 2, 4, 8, 16], MAX_PIXELS = 4.2e6;
  const autoBin = (s, voxel) => BINS.filter((b) => !s.size || s.size[0] * s.size[1] / (b * b) <= MAX_PIXELS || b === 16)
    .reduce((a, b) => Math.abs(Math.log(s.pixelA * b / voxel)) < Math.abs(Math.log(s.pixelA * a / voxel)) ? b : a);
  function fmtA(v) { return v >= 10 ? v.toFixed(0) : v.toFixed(1); }
  const span = (list) => {
    if (!list.length) return "";
    const lo = Math.min(...list), hi = Math.max(...list);
    return hi / lo < 1.05 ? `约 ${fmtA(lo)} Å` : `约 ${fmtA(lo)}–${fmtA(hi)} Å`;
  };
  // Voxel size, image size after binning, and the finest detail it can hold (Nyquist: two voxels).
  const dims = (list, binOf) => {
    const d = [...new Set(list.filter((s) => s.size).map((s) => `${Math.floor(s.size[0] / binOf(s))}×${Math.floor(s.size[1] / binOf(s))}`))];
    return d.length === 1 ? d[0] : d.length ? `${d[0]} 等` : "";
  };
  const nyq = (list, binOf) => span(list.map((s) => 2 * s.pixelA * binOf(s))).replace("约 ", "");
  const detail = (list, binOf) => [span(list.map((s) => s.pixelA * binOf(s))), dims(list, binOf), `分辨率最高约 ${nyq(list, binOf)}`].filter(Boolean).join(" · ");
  const known = () => {
    const pool = session.series.filter((s) => s.pixelA > 0);
    const ticked = pool.filter((s) => selected.has(s.name));
    return ticked.length ? ticked : pool;
  };
  const showSteps = () => {
    for (const b of boxes) b.checked = !!steps[b.dataset.step];
    const on = Object.keys(STEP_NAMES).filter((k) => steps[k]);
    const fine = steps.fineAlign || steps.denoise || steps.wedge;
    const voxel = fine ? 9 : 18;
    const list = known();
    const autoText = list.length ? span(list.map((s) => s.pixelA * autoBin(s, voxel))) : `约 ${voxel} Å`;
    const autoDetail = list.length ? detail(list, (s) => autoBin(s, voxel)) : autoText;
    const opts = [["", `自动（${autoDetail}）`]];
    for (const b of BINS) {
      const big = list.some((s) => s.size && s.size[0] * s.size[1] / (b * b) > MAX_PIXELS);
      opts.push([String(b), `bin ${b}${list.length ? ` · ${detail(list, () => b)}` : ""}${big ? "（很慢，文件很大）" : ""}`]);
    }
    const want = steps.bin ? String(steps.bin) : "";
    binPick.replaceChildren(...opts.map(([v, t]) => { const o = document.createElement("option"); o.value = v; o.textContent = t; return o; }));
    binPick.value = want;
    binPick.disabled = !!W.busy;
    const size = steps.bin ? (list.length ? span(list.map((s) => s.pixelA * steps.bin)) : `bin ${steps.bin}`) : autoText;
    const res = list.length ? `，分辨率最高约 ${nyq(list, (s) => steps.bin || autoBin(s, voxel))}` : "";
    q(".cs-steps-line").textContent = !on.length
      ? `都不勾：快速重构，${size} 体素${res}，每个序列几分钟`
      : `将运行：${on.map((k) => STEP_NAMES[k]).join(" · ")}（${size} 体素${res}${steps.wedge ? "，去缺失楔每个序列约 20 分钟起，进度里会显示预计剩余时间" : ""}）`;
  };
  for (const b of boxes) b.onchange = () => { steps = { ...steps, [b.dataset.step]: b.checked }; pref.set("cryo-steps", steps); showSteps(); };
  binPick.onchange = () => { steps = { ...steps, bin: binPick.value ? Number(binPick.value) : null }; pref.set("cryo-steps", steps); showSteps(); };
  refreshSteps = showSteps;
  showSteps();
  // What a run left out or said, from the series it made.
  const noteOf = (r, only) => {
    const notes = [...new Set(r.series.filter((s) => only.includes(s.name) && s.note).map((s) => s.note))];
    return notes.length ? `。${notes.join("；")}` : "";
  };
  q(".cs-finish").onclick = async () => {
    return runSelected({ custom: { ...steps } });
  };
  async function runSelected(purpose){
    if(W.busy)return;
    if(!folder)return W.status('先选项目文件夹');
    const only=selectedNames();if(!only.length)return W.status('先勾选要处理的序列');
    // The processing scope is explicit. Merely selecting a card never changes
    // scientific picks; a requested run can include an earlier excluded series.
    try{for(const s of session.series)if(selected.has(s.name)&&s.stage!=='movies'&&!s.chosen)await invoke('tomo_take',{stack:s.stack,take:true});}
    catch(e){return W.status(`无法准备勾选的序列：${e}`);}
    if(only.includes(rv.name)){closeReview();stopMedia();rv.name=null;rv.dataset=null;}
    return runPass(purpose?'重构':'处理',purpose,only);
  }

  // A run that kept going on the server while the app was closed: show it
  // here as if it had just been started (folder, ticks, progress, cancel).
  document.addEventListener("tmv-remote-running", (e) => {
    const { record } = e.detail;
    if (record.command !== "tomo_session_run" || W.busy) return;
    e.detail.claimed = true;
    const req = record.args?.req || {};
    // Open the panel first: opening follows the open folder, which the job may not be.
    if ($("cryoet-workspace")?.hidden) $("btn-cryoet")?.click();
    if (W.job?.() !== "recon") W.go?.("recon");
    const f = window.TmvRemote.localPath(record, req.folder);
    if (f && f !== folder) { folder = f; changedFolder(); }
    chose = true;
    selected.clear();
    for (const n of req.only || []) selected.add(n);
    scan();
    runPass(req.purpose ? "重构" : "处理", req.purpose || null, req.only || [], () => window.TmvRemote.attach(record));
  });

  // ---------- the review of one series ----------

  const rv = { name: null, open:false, tab: "images", dataset: null, loading:false, sig: "", timer: 0, gen: 0, tabs:new Map(), prepared:new Map() };
  const find = (name) => session.series.find((s) => s.name === name);
  const bodySig = (s) => [s.stage,s.corrected,s.small,s.aligned,s.quick,s.result,s.views,(s.done||[]).join(",")].join("|");
  let prepareTail=Promise.resolve();
  function preparePreview(path){
    if(!rv.prepared.has(path)){
      const gen=rv.gen;
      const p=prepareTail.then(()=>{if(gen!==rv.gen)throw Error('数据集已切换');return invoke('tomo_preview_file',{path});});rv.prepared.set(path,p);prepareTail=p.catch(()=>{});
      p.catch(()=>{if(rv.prepared.get(path)===p)rv.prepared.delete(path);});
    }
    return rv.prepared.get(path);
  }

  function stopMedia() {
    clearInterval(rv.timer);
    rv.timer = 0;
    rv.closeLightbox?.();
    const old=rv.gen++;
    for (let k=queue.length-1;k>=0;k--) if(queue[k].group===old){const job=queue.splice(k,1)[0];job.store.get(job.key)?.cancelQueued?.();}
    cache.clear();rv.tabs.clear();rv.prepared.clear();rv.paint=null;
  }
  function closeReview() {
    rv.open=false;rv.closeLightbox?.();
    review.hidden = true;
    wall.hidden = false;
    host.classList.remove("reviewing");
  }

  async function openReview(name) {
    const s = find(name);
    if (!s) return;
    const reuse=rv.name===name&&rv.sig===bodySig(s)&&rv.dataset;
    if(!reuse){stopMedia();rv.dataset=null;rv.loadError='';rv.loading=true;}
    rv.name = name;
    rv.open=true;rv.tab=s.aligned?'align':(s.quick||s.result)?'tomo':'images';
    rv.sig = bodySig(s);
    wall.hidden = true;
    review.hidden = false;
    host.classList.add("reviewing");
    $("cryoet-workspace").scrollTop = 0;
    if(!reuse)buildReview(s);showTab();
    if(reuse)return;
    const gen=rv.gen;
    // The existing 高级设置 steps work on the series opened here.
    if (s.stage !== "movies" || s.small) {
      try {
        const d=await W.load(s.stack);if(gen!==rv.gen||rv.name!==name)return;rv.dataset=d;
      } catch (e) {
        if(gen!==rv.gen||rv.name!==name)return;rv.dataset = null;
        rv.loadError = String(e);
      }
    }
    if(gen===rv.gen&&rv.name===name){rv.loading=false;showTab();}
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
    review.append(tabs, el("div", "cs-tabpanels"));
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
    if (!rv.name||!rv.open) return;
    const s = find(rv.name);
    if (!s) return closeReview();
    infoLine(s);
    const sig = bodySig(s);
    if (sig !== rv.sig) {
      openReview(s.name);
    }
  }

  function showTab() {
    const s = find(rv.name);
    if (!s) return;
    rv.sig = bodySig(s);
    review.querySelectorAll(".cs-tab").forEach((b) => b.classList.toggle("on", b.dataset.tab === rv.tab));
    const panels=review.querySelector('.cs-tabpanels');
    if(rv.loading){if(!panels.querySelector('.cs-review-loading')){const loading=el('div','cs-review-loading','正在读取序列信息…');loading.setAttribute('role','status');loading.append(el('progress'));panels.append(loading);}return;}
    panels.querySelector('.cs-review-loading')?.remove();
    let body=rv.tabs.get(rv.tab);
    if(!body){body=el('div','cs-tabbody');rv.tabs.set(rv.tab,body);panels.append(body);({images:imagesTab,align:alignTab,tomo:tomoTab})[rv.tab](body,s);}
    for(const [tab,node]of rv.tabs)node.hidden=tab!==rv.tab;
  }

  // ----- 图像 -----

  function imagesTab(body, s) {
    if (!s.small || !rv.dataset) {
      body.append(el("p", "dim", !s.small ? "还没有生成倾转图的预览。" : rv.loadError ? `无法读取这个序列：${rv.loadError}` : "正在读取序列信息…"));
      return;
    }
    const d=rv.dataset,checks=d.checks||[],gen=rv.gen;
    const sum=el('p','cs-summary'),hint=el('p','dim','点击图像改变是否使用；点右上角放大查看高清预览。');
    const batch=el('div','cs-thumbs-loading'),status=el('span','cs-thumbs-status','正在准备低分辨率预览…'),progress=el('progress'),retryAll=el('button','small','重试失败项');
    status.setAttribute('role','status');progress.max=checks.length||1;progress.value=0;retryAll.hidden=true;batch.append(status,progress,retryAll);
    const grid=el('div','cs-views');body.append(sum,hint,batch,grid);
    const cells=checks.map(c=>{
      const cell=el('div','cs-view'),box=el('div','cs-vpic'),cv=el('canvas'),state=el('div','cs-thumb-state'),message=el('span',null,'等待批量加载…'),retry=el('button','small','重试');
      cv.hidden=true;retry.hidden=true;state.setAttribute('role','status');state.append(message,retry);box.append(cv,state);
      box.title='点击改变是否使用';box.onclick=()=>toggle(c);
      const zoom=el('button','small cs-zoom','⤢');zoom.title='放大看';zoom.setAttribute('aria-label',`放大 ${c.angle.toFixed(1)}°`);zoom.onclick=e=>{e.stopPropagation();enlarge(s,c);};box.append(zoom);
      retry.onclick=e=>{e.stopPropagation();load(item);};
      const lab=el('div','cs-vlab');lab.append(el('b',null,`${c.angle.toFixed(1)}°`),el('span','cs-vst'));
      const why=el('div','cs-vwhy dim'),give=el('button','small cs-vgive','恢复建议');give.title='清除这张图的手动选择，恢复程序建议';give.onclick=()=>setPick(c,null);
      cell.append(box,lab,why,give);grid.append(cell);
      const item={c,cell,box,cv,state,message,retry,why,give,st:lab.querySelector('.cs-vst'),loaded:false,failed:false,pending:false};return item;
    });
    function count(){if(gen!==rv.gen)return;const ready=cells.filter(x=>x.loaded).length,failed=cells.filter(x=>x.failed).length;progress.value=ready;status.textContent=`${ready===checks.length?'已加载全部':'正在批量加载'} ${ready}/${checks.length} 张缩略图${failed?` · ${failed} 张失败，可重试`:''}`;retryAll.hidden=!failed;}
    function load(x){
      if(gen!==rv.gen||x.loaded||x.pending)return;x.pending=true;x.failed=false;x.retry.hidden=true;x.message.textContent='正在读取低分辨率预览…';x.box.setAttribute('aria-busy','true');count();
      pic(s.small,x.c.z,128,gen,false).then(data=>{if(gen!==rv.gen)return;draw(x.cv,data);x.cv.hidden=false;x.state.hidden=true;x.loaded=true;})
        .catch(e=>{if(gen!==rv.gen)return;x.failed=true;x.message.textContent=`加载失败：${e}`;x.retry.hidden=false;})
        .finally(()=>{x.pending=false;if(gen===rv.gen){x.box.setAttribute('aria-busy','false');count();}});
    }
    retryAll.onclick=()=>cells.filter(x=>x.failed).forEach(load);
    function paint(){
      sum.textContent=R.summaryLine(checks,rv.dataset.picks);
      for(const x of cells){const st=R.viewState(x.c,rv.dataset.picks);x.cell.className=`cs-view ${st.state}`;x.st.textContent={used:'使用',suggested:'建议去掉',dropped:'已去掉',kept:'已保留'}[st.state];x.why.textContent=x.c.bad&&st.state!=='dropped'?x.c.bad:'';x.why.title=R.stateLabel(x.c,st);x.cell.title=R.stateLabel(x.c,st);x.give.hidden=st.pick===null;}
    }
    rv.paint=paint;paint();
    preparePreview(s.small).catch(e=>{if(gen===rv.gen)W.status(`预览缓存不可用，继续读取小图：${e}`);})
      .then(()=>{if(gen===rv.gen)cells.forEach(load);});
  }

  let pickChain = Promise.resolve();
  function setPick(c, keep) {
    const s = find(rv.name);
    if (!s) return;
    const gen=rv.gen;
    pickChain = pickChain.then(async () => {
      try {
        const d = await invoke("tomo_pick", { stack: s.stack, z: c.z, keep });
        if(gen!==rv.gen||rv.name!==s.name)return;
        rv.dataset = d;
        W.setDataset(d);
        rv.paint?.();
        document.querySelector(".cs-lightbox") && lightboxPaint();
      } catch (e) {
        if(gen===rv.gen)W.status(`无法保存选择：${e}`);
      }
      scan();
    });
    return pickChain;
  }
  const toggle = (c) => setPick(c, R.toggleKeep(c, rv.dataset.picks));

  // A view bigger (tomo_view max 768), with the same choice buttons.
  let lightboxPaint = () => {};
  function enlarge(s, c) {
    rv.closeLightbox?.();
    const back = el("div", "cs-lightbox");
    back.setAttribute('role','dialog');back.setAttribute('aria-modal','true');back.setAttribute('aria-label',`${c.angle.toFixed(1)}° 图像预览`);
    const card = el("div", "cs-lb-card");
    const cv = el("canvas");
    cv.hidden=true;
    const placeholder=el('div','cs-lb-placeholder','正在加载预览…'),loading=el('div','cs-lb-loading'),message=el('span',null,'正在加载高清预览…'),progress=el('progress'),retry=el('button','small','重试');
    loading.setAttribute('role','status');retry.hidden=true;loading.append(message,progress,retry);
    const cap = el("div", "cs-lb-cap");
    const act = el("button", "small");
    const close = el("button", "small", "关闭");
    const bar = el("div", "al-row");
    bar.append(act, close);
    card.append(placeholder,cv,loading,cap,bar);
    back.append(card);
    const done = () => { back.remove(); document.removeEventListener("keydown", onKey, true); lightboxPaint = () => {}; if(rv.closeLightbox===done)rv.closeLightbox=null; };
    rv.closeLightbox=done;
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
    const gen=rv.gen,live=()=>gen===rv.gen&&back.isConnected;
    pic(s.small,c.z,128,gen,true).then(d=>{if(live()&&!loading.hidden){draw(cv,d);cv.hidden=false;placeholder.hidden=true;}}).catch(()=>{});
    function high(){
      loading.hidden=false;retry.hidden=true;progress.hidden=false;message.textContent='正在加载高清预览…';
      pic(s.small,c.z,768,gen,true).then(d=>{if(!live())return;draw(cv,d);cv.hidden=false;placeholder.hidden=true;loading.hidden=true;})
        .catch(e=>{if(!live())return;message.textContent=`高清预览加载失败：${e}`;progress.hidden=true;retry.hidden=false;});
    }
    retry.onclick=high;high();
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
    const box = el('div','cs-preview-box'), state = el('div','cs-preview-state','正在读取对齐预览…');
    state.setAttribute('role','status'); box.append(cv,state);
    const ctl=el('div','al-row cs-filmctl'),play=el('button','small','播放'),retry=el('button','small','重试');
    const slider=el('input');slider.type='range';slider.min=0;slider.max=n-1;slider.value=0;slider.className='cs-slider';slider.setAttribute('aria-label','对齐预览帧');
    const lab=el('span','dim cs-framelab');lab.setAttribute('role','status');retry.hidden=true;play.disabled=true;
    ctl.append(play,slider,retry,lab);body.append(head,ctl,box);
    const bars=barChart(al);if(bars)body.append(bars.node);
    const frames=new Array(n).fill(null),pending=new Set(),failed=new Map();
    let i=0,playing=!S.folder?.remote,loaded=0,buffering=null,painted=null;const gen=rv.gen;
    function update(){
      if(gen!==rv.gen)return;
      play.textContent=playing?'暂停':'播放';play.disabled=!frames[i];slider.value=i;
      const m=framesMeta[i];lab.textContent=`第 ${i+1}/${n} 帧${m?` · ${m.angle.toFixed(1)}°`:''} · ${loaded===n?'已全部载入':'正在预加载'} ${loaded}/${n}${buffering!=null?` · 正在缓冲第 ${buffering+1} 帧`:''}${failed.size?` · ${failed.size} 帧失败，可重试`:''}`;
      retry.hidden=!failed.size;
      state.hidden=!!frames[i];box.setAttribute('aria-busy',String(!frames[i]));
      if(!frames[i])state.textContent=failed.has(i)?`第 ${i+1} 帧加载失败：${failed.get(i)}；请点“重试”`:`正在${S.folder?.remote?'从服务器':''}读取第 ${i+1} 帧…`;
      if(frames[i]&&painted!==frames[i]){draw(cv,frames[i]);painted=frames[i];}bars?.mark(m);
    }
    function get(k,priority=false){
      if(k<0||k>=n||frames[k]||failed.has(k))return;
      if(pending.has(k)){if(priority)pic(s.aligned,k,512,gen,true);return;}
      pending.add(k);
      pic(s.aligned,k,512,gen,priority).then(d=>{
        if(gen!==rv.gen)return;frames[k]=d;loaded++;update();
      }).catch(e=>{if(gen!==rv.gen)return;failed.set(k,String(e));if(k===i||k===buffering){playing=false;buffering=null;}update();})
        .finally(()=>pending.delete(k));
    }
    function show(k){i=k;buffering=null;get(k,true);get((k+1)%n);get((k+2)%n);update();}
    play.onclick=()=>{playing=!playing;buffering=null;update();};
    retry.onclick=()=>{const keys=[...failed.keys()];failed.clear();for(const k of keys)get(k,k===i);update();};
    slider.oninput=()=>{playing=false;show(+slider.value);};
    rv.timer=setInterval(()=>{
      if(!playing||!rv.open||rv.tab!=='align'||!visible()||gen!==rv.gen)return;const next=(i+1)%n;
      if(!frames[next]){buffering=next;get(next,true);if(failed.has(next)){playing=false;buffering=null;}update();return;}
      show(next);
    },125);
    state.textContent='正在准备全部对齐预览…';
    preparePreview(s.aligned)
      .catch(e=>{if(gen===rv.gen)W.status(`预览缓存不可用，继续读取小图：${e}`);})
      .then(()=>{if(gen!==rv.gen)return;show(0);for(let k=0;k<n;k++)get(k);});
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
    const box=el('div','cs-preview-box'),state=el('div','cs-preview-state','正在读取断层切片预览…');state.setAttribute('role','status');box.append(cv,state);
    const retry=el('button','small','重试');retry.hidden=true;
    const ctl = el("div", "al-row cs-filmctl");
    const slider = el("input");
    slider.type = "range";
    slider.className = "cs-slider";
    slider.disabled = true;
    const lab = el("span", "dim cs-framelab", "载入中…");
    ctl.append(el("span", "dim", "层"), slider, lab);
    ctl.append(retry);body.append(head, ctl, box);
    // The depth comes from the picture's title, else from the dataset's record of this tomogram.
    let depth = 0, i = 0, painted = null;
    const gen = rv.gen;
    const fromDataset = () => rv.dataset?.tomograms?.find((x) => x.path === t.path)?.thickness || 0;
    const frames=new Map(),pending=new Set(),failed=new Map();
    function update(){
      if(gen!==rv.gen)return;
      const d=frames.get(i);retry.hidden=!failed.size;
      state.hidden=!!d;box.setAttribute('aria-busy',String(!d));
      if(!d)state.textContent=failed.has(i)?`第 ${i+1} 层加载失败：${failed.get(i)}；请点“重试”`:`正在${S.folder?.remote?'从服务器':''}读取第 ${i+1} 层预览…`;
      if(d&&painted!==d){draw(cv,d);painted=d;}
      lab.textContent=`${i+1} / ${depth} · ${frames.size===depth?'已全部载入':'正在预加载'} ${frames.size}/${depth}${failed.size?` · ${failed.size} 层失败，可重试`:''}`;
    }
    function get(z,priority=false){
      if(gen!==rv.gen||frames.has(z)||failed.has(z))return;
      if(pending.has(z)){if(priority)pic(t.path,z,512,gen,true);return;}
      pending.add(z);
      pic(t.path,z,512,gen,priority).then(d=>{if(gen===rv.gen){frames.set(z,d);update();}})
        .catch(e=>{if(gen===rv.gen){failed.set(z,String(e));update();}}).finally(()=>pending.delete(z));
    }
    async function start(){
      retry.hidden=true;state.hidden=false;box.setAttribute('aria-busy','true');
      try {
        state.textContent=S.folder?.remote?'正在服务器上把整个断层图缩小成预览，再一次下载过来（大的断层图要几十秒）…':'正在准备全部断层图预览…';
        try{await preparePreview(t.path);}catch(e){if(gen===rv.gen)W.status(`预览缓存不可用，继续读取小图：${e}`);}
        if(gen!==rv.gen)return;
        const d=await pic(t.path,null,512,gen,true);
        if(gen!==rv.gen)return;
        depth=R.depthFromTitle(d.title)||fromDataset()||1;i=Math.floor(depth/2);
        frames.set(i,d);slider.max=depth-1;slider.value=i;slider.disabled=false;update();
        for(let z=0;z<depth;z++)get(z);
      } catch (e) {
        if(gen===rv.gen){state.hidden=false;state.textContent=`预览加载失败：${e}；请点“重试”`;retry.hidden=false;box.setAttribute('aria-busy','false');lab.textContent='预览未载入';}
      }
    }
    retry.onclick=()=>{if(!depth)return start();const keys=[...failed.keys()];failed.clear();for(const z of keys)get(z,z===i);update();};
    slider.oninput=()=>{i=+slider.value;get(i,true);update();};
    start();
  }

  // The panel's first open scans too (cryoet-open); also when scripts load with it already open.
  follow();
  if (visible()) scan();
})();
