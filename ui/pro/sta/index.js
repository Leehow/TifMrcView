"use strict";
// The subtomogram-averaging workspace (sta::auto): the tomograms of a cryo-ET
// dataset go in, one click runs the whole pipeline, and the intermediate
// results show as they come: class-average "cards" (the program tries each as
// a template and takes the one that averages best), then the average with its
// FSC. The user may click another card and average again from it; nothing is
// asked otherwise. Results live in <first tomogram>_sta/ beside the tomogram,
// so reopening the workspace shows them without a rerun.
// Backend: sta_run, sta_cards, sta_result, sta_out, sta_cancel (marks: draw.js); events
// sta-progress {label, done, total}, sta-live {png}, sta-note {kind, …}.
// Every call is guarded: under the web demo the commands are missing and the
// panel only says so. Shares app.js globals: S, T, F, invoke, $, setFft, openPath.
(() => {
  const W = window.TifMrcViewPro.sta = { busy: false, items: [], off: new Set(), out: "", cards: [], selected: null, trials: {}, gen: 0 };
  const fmt = (v, d = 1) => (v == null || !isFinite(v) ? "—" : (+v).toFixed(d));
  const name = (p) => p?.split(/[\\/]/).pop() || "";

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
    <div class="al-head"><b>子断层平均 · Subtomogram averaging</b><button id="sta-close" class="small" title="关闭">✕</button></div>
    <p class="dim">选好断层图，按下面的按钮：程序自己找出颗粒、做出候选的“卡片”、挑最合适的一张，再把所有同样的颗粒对齐平均。</p>
    <div id="sta-data" class="sta-data"></div>
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
      <p id="sta-summary"></p>
      <img id="sta-slices" class="ws-img" alt="平均结构的三个中心截面（XY、XZ、YZ）">
      <div id="sta-fsc"></div>
      <div class="al-row"><button id="sta-open" class="small primary">在查看器中打开</button><button id="sta-folder" class="small">打开结果文件夹</button></div>
    </div>
    <div class="ws-foot"><span id="sta-status" role="status" class="dim"></span><button id="sta-cancel" class="small" hidden>取消</button></div>`;
  $("stage").append(panel);
  W.panel = panel;
  const status = (t) => { $("sta-status").textContent = t; };

  // The tomograms: the one open in the viewer, then the other full tomograms
  // of the cryo-ET dataset (those made for averaging, with a clear companion, when there are any).
  function collect() {
    const items = [];
    const add = (p) => { if (p && !items.some((i) => i.path === p)) items.push({ path: p, name: name(p) }); };
    if (S.info?.path && S.info.sizeZ >= 3) add(S.info.path);
    const all = window.TifMrcViewPro.cryoet?.dataset?.tomograms?.filter((t) => t.kind === "full") || [];
    for (const t of all.some((x) => x.look) ? all.filter((x) => x.look) : all) add(t.path);
    return items;
  }
  function renderData() {
    W.items = collect();
    const box = $("sta-data");
    box.replaceChildren();
    if (!W.items.length) {
      box.innerHTML = '<span class="dim">先打开一张断层图，或在 Cryo-ET 工作区里完成“子断层平均”用途的重构</span>';
      return;
    }
    for (const it of W.items) {
      const label = document.createElement("label");
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.checked = !W.off.has(it.path);
      cb.onchange = () => { cb.checked ? W.off.delete(it.path) : W.off.add(it.path); };
      label.append(cb, ` ${it.name}`);
      label.title = it.path;
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

  function cardEl(c) {
    const fig = document.createElement("figure");
    fig.className = (W.selected === c.id ? "on" : "") + (c.chosen ? " chosen" : "");
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
      W.selected = c.id;
      renderCards();
    };
    return fig;
  }
  function renderCards() {
    const cards = W.cards.map((c) => ({ ...c, trial: c.trial || W.trials[c.id] || null }));
    $("sta-cards-box").hidden = !cards.length;
    $("sta-cards").replaceChildren(...cards.map(cardEl));
    const chosen = W.cards.find((c) => c.chosen)?.id;
    const other = W.selected != null && W.selected !== chosen;
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
  function renderResult(r) {
    $("sta-result").hidden = !r;
    if (!r) return;
    W.result = r;
    const res = r.resolutionA != null ? `分辨率 ${fmt(r.resolutionA)} Å · ${r.particles} 个颗粒` : `${r.particles} 个颗粒 · 两半图不够一致，没有给出分辨率`;
    $("sta-summary").textContent = res;
    $("sta-slices").src = r.png ? `data:image/png;base64,${r.png}` : "";
    $("sta-fsc").innerHTML = fscSvg(r);
  }

  // Shows what the output folder holds (also when nothing runs).
  async function refresh() {
    const gen = ++W.gen;
    renderData();
    const first = W.items[0]?.path;
    if (!first || W.busy) return;
    try {
      const out = await invoke("sta_out", { first });
      W.out = out;
      document.dispatchEvent(new CustomEvent("sta-out", { detail: out }));
      const [cards, result] = await Promise.all([invoke("sta_cards", { out }), invoke("sta_result", { out })]);
      if (gen !== W.gen || W.busy) return;
      W.out = out;
      W.cards = cards.cards || [];
      W.selected = null;
      W.trials = {};
      renderCards();
      renderResult(result);
    } catch (_) {
      // No backend (web demo) or nothing there yet: the panel stays as it is.
    }
  }

  // One run of any kind: `task(inputs, out)` calls the backend and gives its
  // result. The drawn run (draw.js) goes through here too.
  async function go(task, fresh) {
    if (W.busy) return;
    const inputs = W.items.filter((i) => !W.off.has(i.path)).map((i) => i.path);
    if (!inputs.length) return status("请先打开一张断层图并勾选要平均的断层图");
    W.busy = true;
    W.trials = {};
    if (fresh) { W.cards = []; }
    renderResult(null);
    renderCards();
    $("sta-run").disabled = true;
    $("sta-cancel").hidden = false;
    $("sta-cancel").onclick = () => invoke("sta_cancel").catch(() => {});
    document.dispatchEvent(new CustomEvent("sta-busy", { detail: true }));
    const t0 = performance.now();
    const unlisten = [];
    try {
      const on = async (ev, fn) => { try { unlisten.push(await T.event.listen(ev, (e) => fn(e.payload))); } catch (_) { /* no events in the demo */ } };
      await on("sta-progress", (p) => {
        const s = (performance.now() - t0) / 1000;
        status(`${p.label}${p.total > 1 ? ` ${p.done}/${p.total}` : ""} · ${s < 120 ? s.toFixed(0) + " s" : (s / 60).toFixed(1) + " min"}`);
      });
      await on("sta-live", (p) => { $("sta-live").hidden = false; $("sta-live").querySelector("img").src = `data:image/png;base64,${p.png}`; });
      await on("sta-note", async (n) => {
        if (n.kind === "trial") {
          W.trials[n.trial.card] = n.trial;
          renderCards();
          return;
        }
        if (NOTES[n.kind]) $("sta-line").textContent = NOTES[n.kind](n);
        if (n.kind === "cardsMade" || n.kind === "cardsReused") {
          try { W.cards = (await invoke("sta_cards", { out: W.out })).cards; renderCards(); } catch (_) { /* shown at the end */ }
        }
      });
      status("开始…");
      W.out = await invoke("sta_out", { first: W.items[0].path });
      const r = await task(inputs, W.out);
      if (!r.drawn) W.cards = r.cards || [];
      W.selected = null;
      renderResult(r.result);
      status(r.needsChoice ? "没有一张卡片能平均出结果：这批断层图里可能没有成群的颗粒" : `完成 · ${((performance.now() - t0) / 1000).toFixed(0)} s`);
    } catch (e) {
      status(`${e}`.includes("已取消") ? "已取消" : `失败：${e}`);
    } finally {
      unlisten.forEach((u) => { try { u(); } catch (_) { /* gone */ } });
      W.busy = false;
      $("sta-run").disabled = false;
      $("sta-cancel").hidden = true;
      $("sta-live").hidden = true;
      $("sta-line").textContent = "";
      renderCards();
      document.dispatchEvent(new CustomEvent("sta-busy", { detail: false }));
    }
  }
  const run = (pick) => go((inputs, out) => invoke("sta_run", { inputs, pick: pick ?? null, out }), pick == null);
  W.go = go;
  W.status = status;
  W.renderData = renderData;
  $("sta-run").onclick = () => run(null);
  $("sta-again").onclick = () => run(W.selected);
  $("sta-open").onclick = () => W.result?.map && openPath(W.result.map, true);
  $("sta-folder").onclick = () => (W.result?.out || W.out) && invoke("open_folder", { path: W.result?.out || W.out }).catch((e) => status(`${e}`));

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
    refresh();
  };
  $("sta-close").onclick = close;
  document.addEventListener("pro-panel", (e) => { if (e.detail !== panel.id && !panel.hidden) close(); });
  document.addEventListener("tmv-opened", () => { if (!panel.hidden && !W.busy) refresh(); });
  $("btn-fft").addEventListener("click", () => { if (F.on) close(); });
})();
