"use strict";
// The subtomogram-averaging workspace (sta::auto): the tomograms of a cryo-ET
// dataset go in, one click runs the whole pipeline, and the intermediate
// results show as they come: class-average "cards" (the program tries each as
// a template and takes the one that averages best), then the average with its
// FSC. The user may click another card and average again from it; nothing is
// asked otherwise. Results live in <first tomogram>_sta/ beside the tomogram,
// so reopening the workspace shows them without a rerun.
// The tomograms listed are the raw ones of the open file's dataset/folder
// (sta_siblings; the open file may be the clear "look" companion, which is
// only for looking: averages are always of the raw tomogram). Everything shown
// below the list (status, progress, live preview, cards, result, ticks)
// belongs to the output folder, so a tab of another folder shows its own.
// Backend: sta_run, sta_cards, sta_result, sta_out, sta_siblings, sta_cancel (marks: draw.js); events
// sta-progress {label, done, total}, sta-live {png}, sta-note {kind, …}.
// Every call is guarded: under the web demo the commands are missing and the
// panel only says so. Shares app.js globals: S, T, F, invoke, $, setFft, openPath.
(() => {
  const W = window.TifMrcViewPro.sta = { busy: false, runOut: "", items: [], out: "", gen: 0, folded: false };
  const fmt = (v, d = 1) => (v == null || !isFinite(v) ? "—" : (+v).toFixed(d));
  const name = (p) => p?.split(/[\\/]/).pop() || "";

  // What belongs to one output folder; kept while the app runs.
  const states = new Map();
  const st = (out) => {
    if (!states.has(out)) states.set(out, { status: "", line: "", live: null, cards: [], trials: {}, selected: null, result: null, off: new Set(), added: [] });
    return states.get(out);
  };
  const cur = () => st(W.out);
  const here = () => W.busy && W.runOut === W.out;

  const button = document.createElement("button");
  button.id = "btn-sta";
  button.className = "toggle";
  button.title = "子断层平均：一键从断层图得到颗粒的平均结构";
  button.innerHTML = '<svg viewBox="0 0 24 24"><circle cx="5" cy="5" r="2"/><circle cx="19" cy="5" r="2"/><circle cx="5" cy="19" r="2"/><circle cx="19" cy="19" r="2"/><circle cx="12" cy="12" r="3.5"/><path d="M7 7l2.5 2.5M17 7l-2.5 2.5M7 17l2.5-2.5M17 17l-2.5-2.5"/></svg><span>子断层平均</span>';
  $("quickbar").append(button);

  const panel = document.createElement("section");
  panel.id = "sta-workspace";
  panel.className = "pro-workspace";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head"><b>子断层平均 · Subtomogram averaging</b><span class="sta-head-btns"><button id="sta-fold" class="small" title="收起面板，让出图像" aria-expanded="true"><svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 15l6-6 6 6"/></svg></button><button id="sta-close" class="small" title="关闭">✕</button></span></div>
    <p class="dim">选好断层图，按下面的按钮：程序自己找出颗粒、做出候选的“卡片”、挑最合适的一张，再把所有同样的颗粒对齐平均。</p>
    <div id="sta-data" class="sta-data"></div>
    <div class="al-row"><button id="sta-add" class="small" title="再加入别的文件夹里的断层图一起平均">添加断层图…</button></div>
    <div class="al-row">
      <button id="sta-run" class="primary">一键平均</button>
    </div>
    <div id="sta-draw-slot"></div>
    <p id="sta-line" class="dim"></p>
    <figure id="sta-live" class="sta-live" hidden><img alt="正在得到的平均"><figcaption class="dim">正在得到的平均（正中的三个截面）</figcaption></figure>
    <div id="sta-cards-box" hidden>
      <h4>候选卡片</h4>
      <p id="sta-cards-hint" class="dim"></p>
      <div id="sta-cards" class="sta-cards"></div>
      <div class="al-row"><button id="sta-again" class="small" hidden>用这个重新平均</button></div>
    </div>
    <div id="sta-result" hidden>
      <h4>结果</h4>
      <div id="sta-which" class="al-row sta-which" hidden></div>
      <p id="sta-summary"></p>
      <p id="sta-used" class="dim"></p>
      <img id="sta-slices" class="ws-img" alt="平均结构的三个中心截面（XY、XZ、YZ）">
      <div id="sta-fsc"></div>
      <div class="al-row"><button id="sta-open" class="small primary">在查看器中打开</button><button id="sta-folder" class="small">打开结果文件夹</button></div>
    </div>
    <div class="ws-foot"><span id="sta-status" role="status" class="dim"></span><button id="sta-cancel" class="small" hidden>取消</button></div>`;
  $("stage").append(panel);
  W.panel = panel;
  // Status of the folder `out` (shown only while it is the current one).
  const setStatus = (out, t) => { st(out).status = t; if (out === W.out) $("sta-status").textContent = t; };
  const status = (t) => setStatus(W.out, t);

  // The tomograms to average: those of the open file's dataset/folder (the
  // raw ones; the open file may be their look companion), else the old guess
  // from the Cryo-ET workspace's dataset.
  function collect() {
    const items = [];
    const add = (p) => { if (p && !items.some((i) => i.path === p)) items.push({ path: p, name: name(p) }); };
    if (S.info?.path && S.info.sizeZ >= 3) add(S.info.path);
    const all = window.TifMrcViewPro.cryoet?.dataset?.tomograms?.filter((t) => t.kind === "full") || [];
    for (const t of all.some((x) => x.look) ? all.filter((x) => x.look) : all) add(t.path);
    return items;
  }
  async function listed() {
    const open = S.info?.path;
    if (open && S.info.sizeZ >= 3) {
      try {
        const l = await invoke("sta_siblings", { path: open });
        if (l?.length) return l.map((s) => ({ path: s.raw, name: s.name || name(s.raw), look: s.look || null, scale: s.lookScale, offset: s.lookOffset, open: !!s.open }));
      } catch (_) { /* no backend (web demo) */ }
    }
    return collect();
  }
  function renderData() {
    const box = $("sta-data");
    box.replaceChildren();
    if (!W.items.length) {
      box.innerHTML = '<span class="dim">先打开一张断层图，或在 Cryo-ET 工作区里完成“子断层平均”用途的重构</span>';
      return;
    }
    const off = cur().off;
    for (const it of W.items) {
      const label = document.createElement("label");
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.checked = !off.has(it.path);
      cb.onchange = () => { cb.checked ? off.delete(it.path) : off.add(it.path); };
      label.append(cb, ` ${it.name}`);
      if (it.open) {
        const tag = document.createElement("span");
        tag.className = "dim";
        tag.textContent = ` · 当前${it.look && S.info?.path === it.look ? "（正在看它的清晰版）" : ""}`;
        label.append(tag);
      }
      label.title = it.look ? `${it.path}\n清晰版（只用来看和标）：${it.look}` : it.path;
      box.append(label);
    }
  }

  const NOTES = {
    noDataset: () => "没找到这张断层图的倾转信息，按 ±60° 处理",
    lookMissing: () => "清晰版断层图不见了，只用原断层图",
    olderVersion: () => "这张断层图是旧版本做的，平均只用断层图本身",
    cardsStale: () => "断层图变了，重新做卡片",
    cardsReused: () => "沿用上次的卡片",
    cardsMade: (n) => `做出了 ${n.count} 张卡片，正在逐张试跑`,
    trialsReused: () => "沿用上次的试跑结果",
    trialsMade: () => "试跑完成，正在选择",
    compareFailed: () => "卡片无法互相比较，各算一种",
    marked: (n) => `用你点的 ${n.points} 个颗粒做了模板，大小 ${n.sizeA} Å`,
    using: (n) => `使用第 ${n.card + 1} 张卡片${n.other?.length ? `；还有 ${n.other.length} 种也不错` : ""}`,
  };
  // The line under the result: what the average was made from.
  function usedText(u) {
    if (!u) return "";
    const size = u.sizeA != null ? fmt(u.sizeA, 0) : null;
    if (u.kind === "points") return `用你点的 ${u.points} 个颗粒做了模板，大小 ${size ?? "—"} Å`;
    if (u.kind === "shapes") {
      const parts = [[u.lines, "条线"], [u.surfaces, "片面"], [u.spheres, "个球"]].filter(([n]) => n > 0).map(([n, w]) => `${n} ${w}`);
      return `用了你画的标注（${parts.join(" / ") || "—"}）${size ? `，颗粒约 ${size} Å` : ""}`;
    }
    if (u.card == null) return "";
    return `用了卡片 ${u.card + 1}${size ? `（约 ${size} Å）` : ""}`;
  }
  W.usedText = usedText;

  function cardEl(c) {
    const fig = document.createElement("figure");
    fig.className = (cur().selected === c.id ? "on" : "") + (c.chosen ? " chosen" : "");
    const img = new Image();
    if (c.png) img.src = `data:image/png;base64,${c.png}`;
    img.alt = `第 ${c.id + 1} 张`;
    const cap = document.createElement("figcaption");
    const t = c.trial;
    const bits = [`约 ${fmt(c.diameterA, 0)} Å`];
    if (t?.ok) bits.push(`${t.particles} 个颗粒`);
    else if (t) bits.push(t.noGroup ? "没有成群的颗粒" : "试跑失败");
    cap.textContent = bits.join(" · ");
    fig.append(img, cap);
    const badges = [];
    if (c.chosen) badges.push(["选中", "pick"]);
    if (c.runsOn) badges.push(["线状/层状", "runs"]);
    else if (c.good && !c.chosen) badges.push(["也不错", "good"]);
    if (badges.length) {
      const row = document.createElement("div");
      for (const [text, kind] of badges) {
        const b = document.createElement("span");
        b.className = `sta-badge ${kind}`;
        b.textContent = text;
        row.append(b);
      }
      fig.append(row);
    }
    fig.onclick = () => {
      if (W.busy) return;
      cur().selected = c.id;
      renderCards();
    };
    return fig;
  }
  function renderCards() {
    const s = cur();
    const cards = s.cards.map((c) => ({ ...c, trial: c.trial || s.trials[c.id] || null }));
    $("sta-cards-box").hidden = !cards.length;
    $("sta-cards").replaceChildren(...cards.map(cardEl));
    const chosen = s.cards.find((c) => c.chosen)?.id;
    const other = s.selected != null && s.selected !== chosen;
    $("sta-again").hidden = !other || W.busy;
    $("sta-cards-hint").textContent = chosen == null ? "程序会逐张试跑，挑出平均得最好的一张。" : "程序已经选了标“选中”的一张。想换一张，点它，再点“用这个重新平均”。标“线状/层状”的会延伸到框外，一般不是颗粒。";
  }

  function fscSvg(r) {
    const f = r.fsc || [], q = r.freq || [];
    if (f.length < 2 || q.length !== f.length) return "";
    const w = 360, h = 140, l = 30, b = 22, fmax = q[q.length - 1];
    const x = (v) => l + (v / fmax) * (w - l - 8), y = (v) => 8 + (1 - Math.max(-0.1, Math.min(1, v))) / 1.1 * (h - b - 8);
    const pts = f.map((v, i) => `${x(q[i]).toFixed(1)},${y(v).toFixed(1)}`).join(" ");
    const ticks = [100, 50, 30, 20, 15, 10, 7, 5].filter((a) => 1 / a <= fmax).map((a) => `<text x="${x(1 / a).toFixed(1)}" y="${h - 6}" text-anchor="middle">${a}</text>`).join("");
    return `<svg viewBox="0 0 ${w} ${h}" class="sta-fsc" role="img" aria-label="两半图的 FSC 曲线"><line x1="${l}" y1="${y(0.143)}" x2="${w - 8}" y2="${y(0.143)}" stroke="var(--dim)" stroke-dasharray="4 3"/><text x="${l - 3}" y="${y(0.143) + 3}" text-anchor="end">0.143</text><text x="${l - 3}" y="${y(1) + 3}" text-anchor="end">1</text><line x1="${l}" y1="${y(0)}" x2="${w - 8}" y2="${y(0)}" stroke="var(--line)"/><polyline fill="none" stroke="var(--accent)" stroke-width="1.8" points="${pts}"/>${ticks}<text x="${w - 8}" y="${h - 6}" text-anchor="end">Å</text></svg>`;
  }
  const WHICH = { auto: "自动", points: "点", drawn: "画的" };
  function renderResult() {
    const r = cur().result;
    W.result = r;
    $("sta-result").hidden = !r;
    if (!r) return;
    const res = r.resolutionA != null ? `分辨率 ${fmt(r.resolutionA)} Å · ${r.particles} 个颗粒` : `${r.particles} 个颗粒 · 两半图不够一致，没有给出分辨率`;
    $("sta-summary").textContent = res;
    $("sta-used").textContent = usedText(r.used);
    $("sta-slices").src = r.png ? `data:image/png;base64,${r.png}` : "";
    $("sta-fsc").innerHTML = fscSvg(r);
    // Several results in the folder: small tabs to switch.
    const tabs = $("sta-which");
    const all = r.available || [];
    tabs.hidden = all.length < 2;
    tabs.replaceChildren(...all.map((w) => {
      const b = document.createElement("button");
      b.className = `small${w === r.which ? " on" : ""}`;
      b.textContent = WHICH[w] || w;
      b.title = { auto: "程序自己找颗粒的结果", points: "用你点的颗粒做的结果", drawn: "用你画的线、面、球做的结果" }[w] || "";
      b.onclick = async () => {
        if (w === r.which) return;
        const out = W.out;
        try {
          const nr = await invoke("sta_result", { out, which: w });
          if (nr) { st(out).result = nr; if (out === W.out) renderResult(); }
        } catch (e) { status(`${e}`); }
      };
      return b;
    }));
  }

  // Everything of the current output folder onto the screen.
  function paint() {
    const s = cur();
    $("sta-status").textContent = s.status;
    $("sta-line").textContent = s.line;
    const live = $("sta-live");
    live.hidden = !(here() && s.live);
    if (s.live) live.querySelector("img").src = s.live;
    $("sta-cancel").hidden = !here();
    $("sta-run").disabled = W.busy;
    $("sta-run").title = W.busy && !here() ? "另一个断层图的平均正在运行" : "";
    renderData();
    renderCards();
    renderResult();
  }

  // Shows what the output folder of the open file holds (also when nothing runs).
  async function refresh() {
    const gen = ++W.gen;
    const base = await listed();
    if (gen !== W.gen) return;
    W.items = base;
    const first = base[0]?.path;
    if (!first) return paint();
    let out;
    try {
      out = await invoke("sta_out", { first });
    } catch (_) {
      // No backend (web demo): the list only.
      return renderData();
    }
    if (gen !== W.gen) return;
    W.out = out;
    // Tomograms added from elsewhere stay with the folder.
    W.items = [...base, ...cur().added.filter((a) => !base.some((b) => b.path === a.path))];
    paint();
    document.dispatchEvent(new CustomEvent("sta-out", { detail: out }));
    if (W.busy && W.runOut === out) return;
    try {
      const [cards, result] = await Promise.all([invoke("sta_cards", { out }), invoke("sta_result", { out })]);
      if (gen !== W.gen || (W.busy && W.runOut === out)) return;
      const s = st(out);
      s.cards = cards.cards || [];
      if (s.selected != null && !s.cards.some((c) => c.id === s.selected)) s.selected = null;
      s.result = result;
      if (out === W.out) { renderCards(); renderResult(); }
    } catch (_) {
      // Nothing there yet: the panel stays as it is.
    }
  }

  async function addTomogram() {
    let p;
    try { p = await invoke("pick_file"); } catch (e) { return status(`${e}`); }
    if (!p) return;
    let one = { path: p, name: name(p) };
    try {
      const o = (await invoke("sta_siblings", { path: p })).find((s) => s.open);
      if (o) one = { path: o.raw, name: o.name || name(o.raw), look: o.look || null, scale: o.lookScale, offset: o.lookOffset };
    } catch (_) { /* no backend */ }
    const s = cur();
    if (!W.items.some((i) => i.path === one.path)) { s.added.push(one); W.items.push(one); }
    s.off.delete(one.path);
    renderData();
  }
  $("sta-add").onclick = addTomogram;

  // One run of any kind: `task(inputs, out)` calls the backend and gives its
  // result. The drawn run (draw.js) goes through here too.
  async function go(task, fresh) {
    if (W.busy) return;
    const inputs = W.items.filter((i) => !cur().off.has(i.path)).map((i) => i.path);
    if (!inputs.length) return status("请先打开一张断层图并勾选要平均的断层图");
    W.busy = true;
    let out = W.out;
    try { out = await invoke("sta_out", { first: W.items[0].path }); } catch (_) { /* demo */ }
    W.runOut = out;
    W.out = out;
    const s = st(out);
    s.trials = {};
    s.live = null;
    s.line = "";
    s.result = null;
    if (fresh) s.cards = [];
    paint();
    $("sta-cancel").onclick = () => invoke("sta_cancel").catch(() => {});
    document.dispatchEvent(new CustomEvent("sta-busy", { detail: true }));
    const t0 = performance.now();
    const unlisten = [];
    const show = () => { if (out === W.out) { renderCards(); } };
    try {
      const on = async (ev, fn) => { try { unlisten.push(await T.event.listen(ev, (e) => fn(e.payload))); } catch (_) { /* no events in the demo */ } };
      await on("sta-progress", (p) => {
        const sec = (performance.now() - t0) / 1000;
        setStatus(out, `${p.label}${p.total > 1 ? ` ${p.done}/${p.total}` : ""} · ${sec < 120 ? sec.toFixed(0) + " s" : (sec / 60).toFixed(1) + " min"}`);
      });
      await on("sta-live", (p) => {
        s.live = `data:image/png;base64,${p.png}`;
        if (out === W.out) { $("sta-live").hidden = false; $("sta-live").querySelector("img").src = s.live; }
      });
      await on("sta-note", async (n) => {
        if (n.kind === "trial") {
          s.trials[n.trial.card] = n.trial;
          show();
          return;
        }
        if (NOTES[n.kind]) { s.line = NOTES[n.kind](n); if (out === W.out) $("sta-line").textContent = s.line; }
        if (n.kind === "cardsMade" || n.kind === "cardsReused") {
          try { s.cards = (await invoke("sta_cards", { out })).cards; show(); } catch (_) { /* shown at the end */ }
        }
      });
      setStatus(out, "开始…");
      const r = await task(inputs, out);
      if (!r.drawn) s.cards = r.cards || [];
      s.selected = null;
      // The newest result of the folder (this one), with what else is there.
      let res = r.result;
      try { res = (await invoke("sta_result", { out })) || res; } catch (_) { /* demo */ }
      s.result = res;
      setStatus(out, r.needsChoice ? "没有一张卡片能平均出结果：这批断层图里可能没有成群的颗粒" : `完成 · ${((performance.now() - t0) / 1000).toFixed(0)} s`);
    } catch (e) {
      setStatus(out, `${e}`.includes("已取消") ? "已取消" : `失败：${e}`);
    } finally {
      unlisten.forEach((u) => { try { u(); } catch (_) { /* gone */ } });
      W.busy = false;
      s.live = null;
      s.line = "";
      paint();
      document.dispatchEvent(new CustomEvent("sta-busy", { detail: false }));
    }
  }
  const run = (pick) => go((inputs, out) => invoke("sta_run", { inputs, pick: pick ?? null, out }), pick == null);
  W.go = go;
  W.status = status;
  W.renderData = renderData;
  $("sta-run").onclick = () => run(null);
  $("sta-again").onclick = () => run(cur().selected);
  $("sta-open").onclick = () => W.result?.map && openPath(W.result.map, true);
  $("sta-folder").onclick = () => (W.result?.out || W.out) && invoke("open_folder", { path: W.result?.out || W.out }).catch((e) => status(`${e}`));

  // Folding the panel to its head frees the image; the state lasts while the app runs.
  function fold(on) {
    W.folded = on;
    panel.classList.toggle("folded", on);
    const b = $("sta-fold");
    b.title = on ? "展开面板" : "收起面板，让出图像";
    b.setAttribute("aria-expanded", String(!on));
  }
  $("sta-fold").onclick = () => fold(!W.folded);

  function close() {
    panel.hidden = true;
    button.classList.remove("on");
  }
  button.onclick = () => {
    if (!panel.hidden) return close();
    if (F.on) setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
    panel.hidden = false;
    button.classList.add("on");
    fold(W.folded);
    refresh();
  };
  $("sta-close").onclick = close;
  document.addEventListener("pro-panel", (e) => { if (e.detail !== panel.id && !panel.hidden) close(); });
  document.addEventListener("tmv-opened", () => { if (!panel.hidden) refresh(); });
  $("btn-fft").addEventListener("click", () => { if (F.on) close(); });
})();
