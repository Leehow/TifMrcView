"use strict";
// The subtomogram-averaging workspace (sta::auto): the tomograms of a cryo-ET
// dataset go in, "find candidates" makes the class-average "cards" and then
// STOPS: the program never chooses what to average. The user chooses on the
// clear "look" tomogram, which is opened for that: click a card, or mark
// particles / lines / surfaces / spheres / membranes on the image (draw.js).
// Only after that choice everything runs by itself (mapping to the raw
// tomogram, search, alignment, average, FSC). Card tags are hints only.
// Results live in <first tomogram>_sta/ beside the tomogram, so reopening the
// workspace shows the cards (and the average, if there is one) without a rerun.
// The tomograms taken are the raw ones of the open file's dataset/folder
// (sta_siblings; the open file may be the clear "look" companion, which is
// only for looking: averages are always of the raw tomogram). They are shown
// as a wall of thumbnails (wall.js, sta_gallery / sta_thumbnail), each with a
// tick box. Everything shown below the wall (status, progress, live preview, cards, result, ticks)
// belongs to the output folder, so a tab of another folder shows its own.
// "找出全部" (found.js) replaces the direct run of a card: the particles like it are found and shown first.
// The last tile of the cards, 「＋ 用现成的密度图」, takes a map the user already has (an average from
// earlier work, a map from a data bank; MRC or EM): sta_pick_map asks for the file, sta_own_map makes
// the card of it (kept under the output folder, a second map replaces the first, sta_own_saved gives
// it back when the workspace is reopened). It is selected like a card and "找出全部" then runs the
// route "own" (found.js). `selected` is a card's number, or "own".
// Backend: sta_run, sta_cards, sta_result, sta_out, sta_siblings, sta_cancel (marks: draw.js); events
// sta-progress {label, done, total}, sta-live {png}, sta-note {kind, …}.
// Every call is guarded: under the web demo the commands are missing and the
// panel only says so. Shares app.js globals: S, T, F, invoke, $, setFft, openPath.
(() => {
  // `reviewed`: what the review shown found per tomogram ([{path, n}]), for the badges of the wall.
  const W = window.TifMrcViewPro.sta = { busy: false, runOut: "", items: [], out: "", gen: 0, folded: false, pixelNeed: [], pixelLine: "这些图的文件头里没有写像素大小，请先在断层图下面填像素大小", reviewed: { found: [], shapes: [] } };
  // "2026-11-11 14:05", the local time.
  const stamp = (ms) => {
    const d = new Date(ms), z = (n) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${z(d.getMonth() + 1)}-${z(d.getDate())} ${z(d.getHours())}:${z(d.getMinutes())}`;
  };
  const fmt = (v, d = 1) => (v == null || !isFinite(v) ? "—" : (+v).toFixed(d));
  const name = (p) => p?.split(/[\\/]/).pop() || "";

  // What belongs to one output folder; kept while the app runs.
  const states = new Map();
  const st = (out) => {
    if (!states.has(out)) states.set(out, { status: "", line: "", live: null, cards: [], trials: {}, selected: null, result: null, off: new Set(), added: [], own: null, kinds: new Map(), kindsAsk: new Set(), full: null });
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
    <p class="dim">选好断层图，按“找候选”：程序做出候选的“卡片”，然后停下来，由你在清晰的断层图上决定平均什么；你选定之后，其余的（找出所有同样的颗粒给你看、对齐、平均）都由程序完成。</p>
    <div id="sta-data"></div>
    <div class="al-row">
      <button id="sta-run" class="primary" title="自动做出候选卡片；不会替你选，也不会平均">找候选</button>
    </div>
    <p id="sta-line" class="dim"></p>
    <p id="sta-lookhint" class="dim" hidden>这张断层图没有清晰版，可在原图上选择</p>
    <figure id="sta-live" class="sta-live" hidden><img alt="正在得到的平均"><figcaption class="dim">正在得到的平均（正中的三个截面）</figcaption></figure>
    <div id="sta-cards-box" hidden>
      <h4>① 点一张卡片，或用现成的密度图</h4>
      <p id="sta-cards-hint" class="dim"></p>
      <div id="sta-cards" class="sta-cards"></div>
      <div class="al-row"><button id="sta-again" class="primary" disabled>找出全部</button></div>
    </div>
    <h4 id="sta-mark-head">② 或者自己标</h4>
    <div id="sta-draw-slot"></div>
    <div id="sta-result" hidden>
      <h4>结果</h4>
      <div id="sta-which" class="al-row sta-which" hidden></div>
      <div id="sta-kept-row" class="al-row" hidden><span id="sta-kept" class="dim"></span><button id="sta-kept-back" class="small">回到完整结果</button></div>
      <p id="sta-old" class="sta-old" hidden></p>
      <p id="sta-summary"></p>
      <p id="sta-used" class="dim"></p>
      <img id="sta-slices" class="ws-img" alt="平均结构的三个中心截面（XY、XZ、YZ）">
      <div id="sta-fsc"></div>
      <div class="al-row"><button id="sta-open" class="small primary">在查看器中打开</button><button id="sta-folder" class="small">打开结果文件夹</button></div>
      <div class="al-row sta-more"><button id="sta-placed" class="small" title="在清晰的断层图上，标出这些颗粒在哪里、朝哪个方向">在图上看这些颗粒</button><button id="sta-kinds-go" class="small" title="看这些颗粒是不是有几种不同的样子">看看有没有不同种类</button></div>
      <div id="sta-placed-box" class="sta-placed" hidden>
        <p class="dim">图上：小圆点是颗粒的位置，短线指向颗粒的“上”。</p>
        <div id="sta-placed-per" class="al-row sta-which"></div>
        <div id="sta-placed-legend" class="sta-placed-legend"></div>
        <p id="sta-placed-msg" class="dim"></p>
        <div class="al-row"><button id="sta-placed-off" class="small">收起</button></div>
      </div>
      <div id="sta-kinds-box" class="sta-kinds" hidden>
        <p id="sta-kinds-head"></p>
        <div id="sta-kinds-list" class="sta-cards"></div>
        <div class="al-row"><button id="sta-kinds-keep" class="primary" hidden></button></div>
      </div>
    </div>
    <div class="ws-foot"><span id="sta-status" role="status" class="dim"></span><button id="sta-cancel" class="small" hidden>取消</button></div>`;
  $("stage").append(panel);
  W.panel = panel;
  // Status of the folder `out` (shown only while it is the current one).
  const setStatus = (out, t) => { st(out).status = t; if (out === W.out) $("sta-status").textContent = statusText(st(out)); };
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
  // The wall of tomograms (wall.js) shows W.items and the ticks.
  const renderData = () => W.wall?.render();

  const NOTES = {
    noDataset: () => "没找到这张断层图的倾转信息，按 ±60° 处理",
    lookMissing: () => "清晰版断层图不见了，只用原断层图",
    olderVersion: () => "这张断层图是旧版本做的，平均只用断层图本身",
    cardsStale: () => "断层图变了，重新做卡片",
    cardsReused: () => "沿用上次的卡片",
    cardsMade: (n) => `做出了 ${n.count} 张卡片`,
    marked: (n) => markedText(n.clicked, n.members, n.sizeA),
    using: (n) => `平均第 ${n.card + 1} 张卡片`,
  };
  // The template made from clicked particles: how many were clicked, how many particles like them it is
  // the average of (when that is another number), and the size measured on them.
  function markedText(clicked, members, sizeA) {
    const size = sizeA != null ? `，大小约 ${fmt(sizeA, 0)} Å` : "";
    if (clicked == null) return `用你点的颗粒做了模板${size}`;
    if (members > clicked) return `你点了 ${clicked} 个例子，模板是 ${members} 个和它们相像的颗粒的平均${size}`;
    return `用你点的 ${clicked} 个颗粒做了模板${size}`;
  }
  // Shapes: what the user drew and what the program found, kind by kind.
  const SHAPES = [["lines", "linesDrawn", "条纤维"], ["surfaces", null, "片面"], ["spheres", "spheresDrawn", "个球"], ["membranes", "membranesDrawn", "片膜"]];
  function shapesText(u) {
    const parts = [];
    for (const [k, dk, word] of SHAPES) {
      const n = u[k] || 0;
      if (n <= 0) continue;
      const mine = dk && u[dk] != null ? Math.min(n, u[dk]) : n, found = n - mine;
      if (!found) parts.push(`你画的 ${n} ${word}`);
      else if (!mine) parts.push(`程序找到的 ${n} ${word}`);
      else parts.push(`你画的 ${mine} ${word}和程序找到的 ${found} ${word[0]}`);
    }
    return parts.length ? `用了${parts.join("，")}` : "用了你画的标注";
  }
  // The line under the result: what the average was made from.
  function usedText(u) {
    if (!u) return "";
    const size = u.sizeA != null ? fmt(u.sizeA, 0) : null;
    let t = "";
    if (u.kind === "own") t = `用了你给的密度图${u.name ? ` ${u.name}` : ""}${size ? `，颗粒约 ${size} Å` : ""}`;
    else if (u.kind === "points") t = markedText(u.clicked, u.members, u.sizeA);
    else if (u.kind === "circled") t = `用你圈的 ${u.examples} 个例子找出同类，平均了 ${u.averaged} 个`;
    else if (u.kind === "shapes") t = `${shapesText(u)}${size ? `，颗粒约 ${size} Å` : ""}`;
    else if (u.card != null) t = `用了卡片 ${u.card + 1}${size ? `（约 ${size} Å）` : ""}`;
    // The review kept more places than the average could use: the ones whose cube reaches past the tomogram.
    if (u.kept != null && u.averaged != null && u.averaged < u.kept) t += `${t ? "。" : ""}你留下的 ${u.kept} 个里有 ${u.kept - u.averaged} 个太靠近图的边缘，切不出完整的方块，没有用上`;
    if (u.tiltAssumed) t += `${t ? "。" : ""}没有找到倾转角范围，按 ±60° 处理`;
    return t;
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
    const bits = [`约 ${fmt(c.measuredA ?? c.diameterA, 0)} Å`];
    if (t?.ok) bits.push(`${t.particles} 个颗粒`);
    else if (t) bits.push(t.noGroup ? "没有成群的颗粒" : "试跑失败");
    cap.textContent = bits.join(" · ");
    fig.append(img, cap);
    // Hints only: they say what the card looks like, never what to average.
    const badges = [];
    if (c.likely) badges.push(["像颗粒", "good"]);
    if (c.runsOn) badges.push(["线状/层状", "runs"]);
    else if (c.good) badges.push(["试平均较好", "good"]);
    if (c.chosen) badges.push(["上次平均了这张", "pick"]);
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
  // The last tile: the map the user brought (selected like a card), or the invitation to bring one.
  function ownEl() {
    const s = cur(), o = s.own;
    if (!o) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "sta-own-add";
      b.title = "以前解出来的平均图，或数据库里下载的图；MRC 或 EM 格式";
      b.innerHTML = "<b>＋</b><span>用现成的密度图</span>";
      b.disabled = W.busy;
      b.onclick = () => pickMap();
      return b;
    }
    const fig = document.createElement("figure");
    fig.className = `own${s.selected === "own" ? " on" : ""}`;
    const img = new Image();
    if (o.png) img.src = `data:image/png;base64,${o.png}`;
    img.alt = "你给的密度图";
    const nm = document.createElement("figcaption");
    nm.className = "own-name";
    nm.textContent = o.name || "密度图";
    nm.title = o.name || "";
    const cap = document.createElement("figcaption");
    cap.textContent = `约 ${fmt(o.card?.diameterA, 0)} Å`;
    const swap = document.createElement("button");
    swap.type = "button";
    swap.className = "small own-swap";
    swap.textContent = "换一张";
    swap.title = "换成另一张现成的密度图";
    swap.disabled = W.busy;
    swap.onclick = (e) => { e.stopPropagation(); pickMap(); };
    cap.append(swap);
    fig.append(img, nm, cap);
    const notes = [];
    if (o.assumed) notes.push("这张图没写体素大小，按断层图的体素算");
    if (o.changed) notes.push("这个文件之后改过，用的还是当时的版本");
    for (const n of notes) {
      const p = document.createElement("p");
      p.className = "dim own-note";
      p.textContent = n;
      fig.append(p);
    }
    fig.onclick = () => {
      if (W.busy) return;
      cur().selected = "own";
      renderCards();
    };
    return fig;
  }
  // Asks for the file and makes the card of it; the tile becomes the map's card, selected.
  async function pickMap() {
    if (W.busy) return;
    const out = W.out;
    let map;
    try { map = await invoke("sta_pick_map"); } catch (e) { return status(`${e}`); }
    if (!map) return;
    const inputs = W.items.filter((i) => !cur().off.has(i.path)).map((i) => i.path);
    if (!inputs.length) return status("请先打开一张断层图并勾选要平均的断层图");
    status("读取密度图…");
    try {
      const r = await invoke("sta_own_map", { map, inputs, out });
      const s = st(out);
      s.own = r;
      s.selected = "own";
      setStatus(out, "");
    } catch (e) {
      setStatus(out, `失败：${e}`);
    }
    if (out === W.out) renderCards();
  }
  function renderCards() {
    const s = cur();
    const cards = s.cards.map((c) => ({ ...c, trial: c.trial || s.trials[c.id] || null }));
    // Always there once the folder is known: the tile of a map of one's own is a way in without cards.
    $("sta-cards-box").hidden = !W.out;
    $("sta-cards").replaceChildren(...cards.map(cardEl), ownEl());
    const again = $("sta-again");
    again.disabled = s.selected == null || W.busy || W.pixelNeed.length > 0;
    again.title = W.pixelNeed.length ? W.pixelLine : s.selected == null ? "先点一张卡片，或选一张现成的密度图" : "";
    $("sta-cards-hint").textContent = cards.length
      ? "点你要平均的那一类作例子，再按“找出全部”：程序会把同类的都找出来给你看，你确认后才平均。卡片上的小标签只是提示（标“线状/层状”的会延伸到框外，一般不是颗粒），选哪一张由你定。也可以用你已有的密度图作例子。"
      : "还没有候选卡片。可以先按“找候选”，也可以直接用你已有的密度图作例子，再按“找出全部”。";
  }
  // The line the run ends with while it waits for the user's choice.
  const PROMPT = "请选择要平均什么：点一张卡片，或用标注工具在图上点、涂、画出来";
  const waiting = (s) => !s.cards.length ? "" : !s.result ? PROMPT : "";
  const statusText = (s) => s.status || waiting(s);

  function fscSvg(r) {
    const f = r.fsc || [], q = r.freq || [];
    if (f.length < 2 || q.length !== f.length) return "";
    const w = 360, h = 140, l = 30, b = 22, fmax = q[q.length - 1];
    const x = (v) => l + (v / fmax) * (w - l - 8), y = (v) => 8 + (1 - Math.max(-0.1, Math.min(1, v))) / 1.1 * (h - b - 8);
    const pts = f.map((v, i) => `${x(q[i]).toFixed(1)},${y(v).toFixed(1)}`).join(" ");
    const ticks = [100, 50, 30, 20, 15, 10, 7, 5].filter((a) => 1 / a <= fmax).map((a) => `<text x="${x(1 / a).toFixed(1)}" y="${h - 6}" text-anchor="middle">${a}</text>`).join("");
    return `<svg viewBox="0 0 ${w} ${h}" class="sta-fsc" role="img" aria-label="两半图的 FSC 曲线"><line x1="${l}" y1="${y(0.143)}" x2="${w - 8}" y2="${y(0.143)}" stroke="var(--dim)" stroke-dasharray="4 3"/><text x="${l - 3}" y="${y(0.143) + 3}" text-anchor="end">0.143</text><text x="${l - 3}" y="${y(1) + 3}" text-anchor="end">1</text><line x1="${l}" y1="${y(0)}" x2="${w - 8}" y2="${y(0)}" stroke="var(--line)"/><polyline fill="none" stroke="var(--accent)" stroke-width="1.8" points="${pts}"/>${ticks}<text x="${w - 8}" y="${h - 6}" text-anchor="end">Å</text></svg>`;
  }
  const WHICH = { auto: "卡片", points: "点", drawn: "画的", own: "自带模板", circled: "圈例子" };
  function renderResult() {
    const r = cur().result;
    W.result = r;
    $("sta-result").hidden = !r;
    if (!r) { W.found?.placed?.sync(); return renderKinds(); }
    const res = r.resolutionA != null ? `分辨率 ${fmt(r.resolutionA)} Å · ${r.particles} 个颗粒` : `${r.particles} 个颗粒 · 两半图不够一致，没有给出分辨率`;
    const pt = r.pattern ? `。在膜${r.pattern.insideA < 0 ? "外" : "内"}侧 ${Math.round(Math.abs(r.pattern.insideA))} Å 处找到重复排列（行距 ${Math.round(r.pattern.rowsA)} Å${r.pattern.fold > 1 ? `，${r.pattern.fold} 重对称，颗粒中心已放在对称轴上${r.pattern.symmetric ? "，并按这个对称性做了平均" : ""}` : ""}），已按它对齐` : "";
    $("sta-summary").textContent = res + pt;
    $("sta-used").textContent = usedText(r.used);
    // A result read from the folder, not made in this session: say when it is from, until a new run ends.
    const old = $("sta-old");
    old.hidden = !r.old;
    old.textContent = r.old ? `上次的结果${r.modifiedMs ? `（${stamp(r.modifiedMs)}）` : ""}` : "";
    $("sta-slices").src = r.png ? `data:image/png;base64,${r.png}` : "";
    $("sta-fsc").innerHTML = fscSvg(r);
    // Several results in the folder: small tabs to switch.
    const tabs = $("sta-which");
    const all = r.available || [];
    tabs.hidden = all.length < 2 || !!r.kept;
    tabs.replaceChildren(...all.map((w) => {
      const b = document.createElement("button");
      b.className = `small${w === r.which ? " on" : ""}`;
      b.textContent = WHICH[w] || w;
      b.title = { auto: "平均所选卡片的结果", points: "用你点的颗粒做的结果", drawn: "用你画的线、面、球做的结果", own: "用你给的现成密度图做的结果", circled: "用你圈的例子找出同类做的结果" }[w] || "";
      b.onclick = async () => {
        if (w === r.which) return;
        const out = W.out;
        try {
          const nr = await invoke("sta_result", { out, which: w });
          if (nr) { st(out).result = { ...nr, old: true }; st(out).full = null; if (out === W.out) renderResult(); }
        } catch (e) { status(`${e}`); }
      };
      return b;
    }));
    // The average of some kinds only: what it is made from, and the way back.
    $("sta-kept-row").hidden = !r.kept;
    if (r.kept) {
      const k = r.kept;
      $("sta-kept").textContent = `只用了选中的 ${k.n ?? r.particles} 个颗粒，按原来的位置和朝向，从断层图平均而成（采样和原来的平均相同）${k.tilt ? "；第一次的平均用了倾转图，可能更清晰一些" : ""}`;
    }
    $("sta-placed").hidden = !!r.kept;
    W.found?.placed?.sync();
    renderKinds();
    // The kinds found before (the panel survives a reopen).
    if (!r.kept && !cur().kinds.has(r.which) && !cur().kindsAsk.has(r.which)) {
      const out = W.out, which = r.which;
      cur().kindsAsk.add(which);
      invoke("sta_kinds_saved", { out, which }).then((d) => {
        if (!d) return;
        st(out).kinds.set(which, { data: d, off: new Set() });
        if (out === W.out) renderKinds();
      }).catch(() => { /* no backend, or none saved */ });
    }
  }

  // "看看有没有不同种类": the kinds of the shown average as cards to tick, then only the ticked ones are averaged.
  function renderKinds() {
    const s = cur(), r = s.result;
    const k = r && !r.kept ? s.kinds.get(r.which) : null;
    $("sta-kinds-go").hidden = !r || !!r.kept;
    $("sta-kinds-go").disabled = W.busy || W.pixelNeed.length > 0;
    $("sta-kinds-box").hidden = !k;
    if (!k) return;
    const list = k.data.kinds || [];
    const keep = $("sta-kinds-keep");
    if (!list.length) {
      $("sta-kinds-head").textContent = "没有发现明显不同的种类";
      $("sta-kinds-list").replaceChildren();
      keep.hidden = true;
      return;
    }
    $("sta-kinds-head").textContent = `这些颗粒分成了 ${list.length} 种。不要的点一下去掉，再按下面的按钮`;
    const toggle = (id) => { if (W.busy) return; if (k.off.has(id)) k.off.delete(id); else k.off.add(id); renderKinds(); };
    $("sta-kinds-list").replaceChildren(...list.map((x) => {
      const fig = document.createElement("figure");
      const on = !k.off.has(x.id);
      fig.className = on ? "on" : "off";
      fig.tabIndex = 0;
      fig.setAttribute("role", "checkbox");
      fig.setAttribute("aria-checked", String(on));
      const img = new Image();
      if (x.png) img.src = `data:image/png;base64,${x.png}`;
      img.alt = `第 ${x.id} 种`;
      const cap = document.createElement("figcaption");
      cap.textContent = `${on ? "☑" : "☐"} ${x.count} 个 · ${Math.round(100 * (x.share || 0))}%`;
      fig.append(img, cap);
      fig.onclick = () => toggle(x.id);
      fig.onkeydown = (e) => { if (e.key === " " || e.key === "Enter") { e.preventDefault(); toggle(x.id); } };
      return fig;
    }));
    const ticked = list.filter((x) => !k.off.has(x.id));
    const n = ticked.reduce((a, x) => a + x.count, 0);
    keep.hidden = false;
    keep.textContent = `只平均选中的 ${n} 个`;
    keep.disabled = W.busy || W.pixelNeed.length > 0 || ticked.length === 0 || ticked.length === list.length;
    keep.title = W.pixelNeed.length ? W.pixelLine : ticked.length === list.length ? "先点掉不要的种类" : ticked.length === 0 ? "至少留一种" : "";
  }
  async function findKinds() {
    const r = cur().result;
    if (!r || r.kept || W.busy) return;
    const which = r.which;
    let got = null;
    await go(async (inputs, out) => { got = await invoke("sta_kinds", { inputs, out, which }); return { drawn: true }; }, false, { keepResult: true });
    if (!got) return;
    const s = cur();
    s.kinds.set(which, { data: got, off: new Set() });
    s.kindsAsk.add(which);
    status("");
    renderKinds();
  }
  async function keepKinds() {
    const full = cur().result, k = full && !full.kept ? cur().kinds.get(full.which) : null;
    if (!k || W.busy) return;
    const keep = (k.data.kinds || []).filter((x) => !k.off.has(x.id)).map((x) => x.id);
    if (!keep.length) return;
    const which = full.which;
    let got = null;
    await go(async (inputs, out) => { got = await invoke("sta_kinds_keep", { inputs, out, which, keep }); return { drawn: true }; }, false, { keepResult: true });
    if (!got?.result) return;
    const s = cur();
    s.full = full;
    s.result = { ...got.result, which, available: full.available, base: full.base, kept: { ids: keep, n: got.result.particles, tilt: !!got.fromTiltImages } };
    status("");
    paint();
  }
  $("sta-kinds-go").onclick = findKinds;
  $("sta-kinds-keep").onclick = keepKinds;
  $("sta-kept-back").onclick = () => { const s = cur(); if (s.full) { s.result = s.full; s.full = null; renderResult(); } };
  $("sta-placed").onclick = () => W.found?.placed?.toggle();

  // Everything of the current output folder onto the screen.
  function paint() {
    const s = cur();
    $("sta-status").textContent = statusText(s);
    $("sta-line").textContent = s.line;
    $("sta-lookhint").hidden = !s.noLook;
    const run = $("sta-run");
    run.textContent = s.cards.length ? "重新找候选" : "找候选";
    run.classList.toggle("primary", !s.cards.length);
    const live = $("sta-live");
    live.hidden = !(here() && s.live);
    if (s.live) live.querySelector("img").src = s.live;
    $("sta-cancel").hidden = !here();
    $("sta-run").disabled = W.busy || W.pixelNeed.length > 0;
    $("sta-run").title = W.pixelNeed.length ? W.pixelLine : W.busy && !here() ? "另一个断层图的平均正在运行" : "";
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
    W.wall?.load();
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
      const [cards, result, own] = await Promise.all([invoke("sta_cards", { out }), invoke("sta_result", { out }), invoke("sta_own_saved", { out }).catch(() => null)]);
      if (gen !== W.gen || (W.busy && W.runOut === out)) return;
      const s = st(out);
      s.cards = cards.cards || [];
      s.own = own || null;
      if (s.selected != null && !(s.selected === "own" ? s.own : s.cards.some((c) => c.id === s.selected))) s.selected = null;
      // The result this session made stays "new"; one read from the folder is the last time's.
      const same = s.result && !s.result.old && result && s.result.modifiedMs === result.modifiedMs && s.result.which === result.which;
      s.result = result && !same ? { ...result, old: true } : result ? { ...result, old: false } : result;
      if (out === W.out) { paint(); }
    } catch (_) {
      // Nothing there yet: the panel stays as it is.
    }
  }

  // Takes a tomogram into the list and ticks it (with its clean companion, when it has one).
  async function addItem(p) {
    let one = { path: p, name: name(p) };
    try {
      const o = (await invoke("sta_siblings", { path: p })).find((s) => s.open);
      if (o) one = { path: o.raw, name: o.name || name(o.raw), look: o.look || null, scale: o.lookScale, offset: o.lookOffset };
    } catch (_) { /* no backend */ }
    const s = cur();
    const have = W.items.find((i) => i.path === one.path);
    if (!have) { s.added.push(one); W.items.push(one); }
    s.off.delete(one.path);
    return have || one;
  }
  async function addTomogram() {
    let p;
    try { p = await invoke("pick_file"); } catch (e) { return status(`${e}`); }
    if (!p) return;
    await addItem(p);
    renderData();
  }
  W.addItem = addItem;
  W.addTomogram = addTomogram;

  // One run of any kind: `task(inputs, out)` calls the backend and gives its
  // result. The drawn run (draw.js) goes through here too.
  // `opts.keepResult`: the result shown stays while it runs and after it (kinds).
  async function go(task, fresh, opts = {}) {
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
    if (!opts.keepResult) {
      s.result = null;
      s.full = null;
      s.kinds.clear();
      s.kindsAsk.clear();
    }
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
      if (!opts.keepResult) {
        let res = r.result;
        try { res = (await invoke("sta_result", { out })) || res; } catch (_) { /* demo */ }
        s.result = res && { ...res, old: false };
      }
      // The run found the cards and stopped: the user chooses what to average.
      if (r.waiting) setStatus(out, s.cards.length ? PROMPT : "没有找到候选卡片：这批断层图里可能没有成群的颗粒，可以用下面的标注工具自己标");
      else setStatus(out, `完成 · ${((performance.now() - t0) / 1000).toFixed(0)} s`);
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
  // Where the user chooses: the clear "look" companion of the open raw tomogram
  // (opened once per run start, not when it is already shown).
  async function showLook() {
    const it = W.items.find((i) => i.open);
    if (!it) return;
    st(W.out).noLook = !it.look;
    if (it.look && S.info?.path !== it.look) { try { await openPath(it.look); } catch (_) { /* stays on the raw one */ } }
  }
  const run = async (pick) => {
    if (W.busy) return;
    if (pick == null) await showLook();
    return go((inputs, out) => invoke("sta_run", { inputs, pick: pick ?? null, out }), pick == null);
  };
  W.go = go;
  W.st = st;
  W.cur = cur;
  W.status = status;
  W.renderData = renderData;
  $("sta-run").onclick = () => run(null);
  $("sta-again").onclick = () => {
    const n = cur().selected;
    if (n == null) return;
    if (n === "own") return W.found?.findOwn();
    W.found ? W.found.find(n) : run(n);
  };
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
    W.wall?.reset();
    refresh();
  };
  $("sta-close").onclick = close;
  // Inside another panel (the Cryo-ET panel's "子断层平均" part): `host(el)` moves the workspace into
  // `el` once, after which it has no button, head, fold or close of its own and takes the size it is
  // given; the host calls `show()` when the part comes into view and `hide()` when it leaves.
  W.hosted = false;
  W.host = (el) => {
    W.hosted = true;
    button.remove();
    panel.classList.remove("folded");
    panel.classList.add("hosted");
    el.append(panel);
  };
  W.show = () => {
    if (!W.hosted) return button.onclick();
    if (!panel.hidden) return;
    panel.hidden = false;
    W.wall?.reset();
    refresh();
  };
  W.hide = () => { panel.hidden = true; button.classList.remove("on"); };
  document.addEventListener("sta-pixel", () => { if (!panel.hidden) paint(); });
  document.addEventListener("pro-panel", (e) => { if (!W.hosted && e.detail !== panel.id && !panel.hidden) close(); });
  document.addEventListener("tmv-opened", () => { if (!panel.hidden) refresh(); });
  $("btn-fft").addEventListener("click", () => { if (F.on && !W.hosted) close(); });
})();
