"use strict";
// Pro "figure" flow: from a finished tomogram (or any Z stack) to the kind
// of 3D picture papers show. One panel, top to bottom:
//   ① the user names the structures and marks them (labels.js smart tool
//     and "find similar": the program proposes, the user decides),
//   ② the labels become 3D surfaces (volume.js / surfaces.js); a click on
//     an object removes it or moves it to another structure,
//   ③ a look is chosen from three rendered thumbnails,
//   ④ a view is chosen from three rendered thumbnails (or dragged freely),
//   ⑤ export: high-resolution PNG with a true scale bar, turntable MP4,
//     OBJ mesh.
// Shares app.js globals: S, T, invoke, $, toast, writeFile, saveDir,
// setSaveDir, dirname, freeRect.
(() => {
  const P = window.TifMrcViewPro;

  // Structures offered by name, coloured as in published segmentation
  // figures (the same colours volume.js and membrane/mod.rs use).
  const PRESETS = [
    ["膜", [255, 196, 0]], ["质膜", [86, 190, 233]], ["囊泡", [92, 190, 60]], ["线粒体", [240, 220, 40]],
    ["内质网", [40, 60, 170]], ["微管", [240, 110, 20]], ["肌动蛋白", [200, 40, 40]], ["核糖体", [235, 100, 165]],
  ];
  // Colours for structures the user names, in turn.
  const EXTRA = [[150, 110, 220], [0, 150, 136], [140, 140, 140], [170, 110, 40], [70, 240, 240], [128, 0, 0]];

  // Looks. bg alpha 0 would be transparent (set at export only).
  const STYLES = [
    { key: "white", name: "白底", tip: "白色背景，带一张断层切片，最常见的论文图样式", fig: { bg: [1, 1, 1, 1], ao: 0.85, gloss: 0.2, outline: 0, cue: 0.25, slice: true } },
    { key: "black", name: "黑底", tip: "黑色背景，表面更亮，适合幻灯片和海报", fig: { bg: [0, 0, 0, 1], ao: 0.85, gloss: 0.45, outline: 0, cue: 0.45, slice: true } },
    { key: "line", name: "描边", tip: "只有结构，带黑色轮廓线，适合拼进组图或做示意图", fig: { bg: [1, 1, 1, 1], ao: 0.55, gloss: 0.1, outline: 0.9, width: 1.2, line: [0.1, 0.1, 0.12], slice: false } },
  ];
  // Views: [yaw, pitch]. Yaw 0, pitch 0 looks along the beam, like the 2D view.
  const VIEWS = [
    { key: "front", name: "正面", tip: "沿电子束方向看，和二维切片同一个方向", at: [0, 0] },
    { key: "oblique", name: "斜视", tip: "斜着看，立体感最强", at: [0.55, 0.42] },
    { key: "side", name: "侧面", tip: "从侧面看，能看出样品的厚度", at: [0.25, 1.15] },
  ];

  const F = {
    open: false,
    marking: 0,       // label being marked, 0 = none
    made: new Set(),  // labels created here that have no pixels yet
    style: "white",
    view: "",         // "" once the user has rotated by hand
    three: false,     // the 3D figure is showing
    busy: "",         // what is running (opening 3D, export)
    stop: false,      // movie cancelled
    mesh: null,       // surfaces the thumbnails were made from
    bar: true,
    clear: false,     // transparent background in the PNG
  };

  const button = document.createElement("button");
  button.id = "btn-figure";
  button.className = "toggle";
  button.title = "一键出图：标记结构，做成论文里那种三维图，导出高清图片、转圈视频和网格";
  button.innerHTML = '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 16l5-5 4 4 3-3 6 6"/><circle cx="16" cy="9" r="1.5"/></svg><span>出图</span>';
  button.disabled = true;
  $("quickbar").append(button);

  const panel = document.createElement("section");
  panel.id = "fig-panel";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="fig-head"><b>一键出图</b><button id="fig-close" class="small" title="关闭">✕</button></div>
    <div class="fig-step" id="fig-s1">
      <div class="fig-title"><span class="fig-n">1</span>图里要有哪些结构</div>
      <div id="fig-list" role="list"></div>
      <div id="fig-add" class="fig-chips"></div>
      <div id="fig-other" class="fig-row" hidden>
        <input id="fig-other-name" type="text" maxlength="24" placeholder="结构的名字" />
        <button class="small primary" id="fig-other-ok">加上</button>
      </div>
      <p class="dim" id="fig-hint"></p>
    </div>
    <div class="fig-step" id="fig-s2">
      <div class="fig-title"><span class="fig-n">2</span>做成三维图</div>
      <div class="fig-row"><button class="primary" id="fig-make">生成三维图</button><button class="small" id="fig-back" hidden title="回到二维切片接着标记">继续标记</button></div>
      <p class="dim" id="fig-check" hidden>拖动旋转，滚轮缩放。点图里的一个物体，可以把它删掉、隐藏，或者改成另一种结构。</p>
      <label class="fig-slider" id="fig-dust-row" hidden title="分割里常有零碎的小块。往右拖，把小于这个大小的碎片从图里拿掉（标签不会被删）">不画小碎片 <input type="range" id="fig-dust" min="0" max="1" step="0.01" value="0" /><span id="fig-dust-n" class="dim"></span></label>
      <div id="fig-obj"></div>
      <div class="fig-row"><button class="small" id="fig-show-all" hidden>显示隐藏的物体</button></div>
    </div>
    <div class="fig-step" id="fig-s3">
      <div class="fig-title"><span class="fig-n">3</span>挑一种样式</div>
      <div class="fig-thumbs" id="fig-styles"></div>
      <label class="fig-check" id="fig-slice-row"><input type="checkbox" id="fig-slice-on" checked /> 带一张断层切片</label>
      <input type="range" id="fig-slice" min="0" max="1" step="0.001" value="0.5" title="切片的位置" />
    </div>
    <div class="fig-step" id="fig-s4">
      <div class="fig-title"><span class="fig-n">4</span>挑一个角度</div>
      <div class="fig-thumbs" id="fig-views"></div>
    </div>
    <div class="fig-step" id="fig-s5">
      <div class="fig-title"><span class="fig-n">5</span>导出</div>
      <div class="fig-row">
        <label class="fig-check" title="在图片左下角画比例尺（需要文件里有像素大小）"><input type="checkbox" id="fig-bar" checked /> 比例尺</label>
        <label class="fig-check" title="图片背景透明，方便放到任何底色上（视频不支持）"><input type="checkbox" id="fig-clear" /> 透明背景</label>
      </div>
      <div class="fig-row">
        <button class="primary" id="fig-png" title="按现在的画面存一张高分辨率 PNG（长边约 3600 像素）">保存图片</button>
        <button id="fig-mp4" title="结构转一整圈的 MP4 视频（8 秒），放在幻灯片或补充材料里">转圈视频</button>
        <button id="fig-obj-export" title="导出 OBJ 网格（带颜色），可以在 Blender、ChimeraX 里继续做">网格</button>
      </div>
    </div>
    <div class="fig-foot"><span id="fig-status" role="status" class="dim"></span><button id="fig-cancel" class="small" hidden>取消</button></div>`;
  $("stage").append(panel);

  // Scale bar over the 3D view, as it will be in the picture.
  const barBox = document.createElement("div");
  barBox.id = "fig-scalebar";
  barBox.hidden = true;
  barBox.innerHTML = "<span></span><i></i>";
  $("stage").append(barBox);

  const status = (t) => { $("fig-status").textContent = t || ""; };
  const labels = () => P.labels;
  const volume = () => P.volume;
  const usable = () => !!(S.info && S.info.sizeZ > 1);
  const labelsHere = () => !!(labels() && labels().token() === S.token);

  // ---------- ① structures ----------

  /// Labels shown as structures: the ones in use and the ones just added.
  function structures() {
    if (!labelsHere()) return [];
    const ks = new Set(labels().used());
    for (const k of F.made) ks.add(k);
    return [...ks].sort((a, b) => a - b);
  }

  /// Objects of each label in the 3D surfaces, when they are current.
  function objectCounts() {
    const m = F.three && volume().surfacesReady() ? volume().mesh() : null;
    if (!m) return null;
    const n = new Map();
    for (const o of m.objects) n.set(o.label, (n.get(o.label) || 0) + 1);
    return n;
  }

  function renderList() {
    const list = $("fig-list");
    list.replaceChildren();
    const ks = structures();
    const counts = objectCounts();
    for (const k of ks) {
      const row = document.createElement("div");
      row.className = "fig-struct" + (k === F.marking ? " on" : "");
      row.setAttribute("role", "listitem");
      const sw = document.createElement("span");
      sw.className = "fig-sw";
      sw.style.background = `rgb(${labels().colorOf(k).join(",")})`;
      const name = document.createElement("span");
      name.className = "fig-name";
      name.textContent = labels().nameOf(k);
      const n = document.createElement("span");
      n.className = "dim fig-count";
      const marked = labels().count(k) > 0;
      n.textContent = !marked ? "还没标记" : counts ? `${counts.get(k) || 0} 个` : "已标记";
      const eye = document.createElement("button");
      eye.className = "small fig-eye" + (labels().hidden(k) ? " off" : "");
      eye.title = "图里显示 / 不显示这种结构";
      eye.textContent = labels().hidden(k) ? "不显示" : "显示";
      eye.addEventListener("click", () => {
        labels().setMaterial(k, { hidden: !labels().hidden(k) });
        renderList();
        thumbsSoon();
      });
      const mark = document.createElement("button");
      mark.className = "small" + (k === F.marking ? " primary" : "");
      mark.textContent = k === F.marking ? "标记中" : marked ? "接着标" : "标记";
      mark.title = "在二维切片上标记这种结构";
      mark.addEventListener("click", () => startMarking(k));
      row.append(sw, name, n, eye, mark);
      list.append(row);
    }
    const names = new Set(ks.map((k) => labels().nameOf(k)));
    const add = $("fig-add");
    add.replaceChildren();
    const lead = document.createElement("span");
    lead.className = "dim";
    lead.textContent = ks.length ? "再加一种：" : "加一种结构：";
    add.append(lead);
    for (const [name, rgb] of PRESETS) {
      if (names.has(name)) continue;
      const b = document.createElement("button");
      b.className = "fig-chip";
      b.textContent = name;
      b.style.setProperty("--c", `rgb(${rgb.join(",")})`);
      b.addEventListener("click", () => addStructure(name, rgb));
      add.append(b);
    }
    const other = document.createElement("button");
    other.className = "fig-chip fig-chip-other";
    other.textContent = "其他…";
    other.addEventListener("click", () => {
      $("fig-other").hidden = false;
      $("fig-other-name").focus();
    });
    add.append(other);
    $("fig-hint").textContent = !labelsHere()
      ? "这张图还不能标记（标签要在 XY 视图里做）。"
      : F.marking
        ? `正在标记「${labels().nameOf(F.marking)}」：在图上点一下这种结构，或者拖一个圈把它圈出来。标好两三个，按旁边出现的「找同类」，程序会列出相似的候选，由你勾选要哪些。翻到别的层接着标，标得越全，图越完整。`
        : ks.length
          ? "点「标记」在切片上圈出这种结构。标完所有结构，再做第 2 步。"
          : "先选图里要画出来的结构，比如膜、囊泡、微管。每种结构由你自己在切片上标记，程序只帮你找相似的。";
  }

  function addStructure(name, rgb) {
    if (!labelsHere()) return toast("这张图还不能标记");
    const k = labels().materialLabel(name, rgb);
    F.made.add(k);
    startMarking(k);
  }

  $("fig-other-ok").addEventListener("click", () => {
    const name = $("fig-other-name").value.trim();
    if (!name) return;
    const known = PRESETS.find(([n]) => n === name);
    addStructure(name, known ? known[1] : EXTRA[F.made.size % EXTRA.length]);
    $("fig-other-name").value = "";
    $("fig-other").hidden = true;
  });
  $("fig-other-name").addEventListener("keydown", (e) => {
    if (e.key === "Enter") $("fig-other-ok").click();
    e.stopPropagation();
  });

  /// Back to the 2D slices with the smart tool on label k.
  function startMarking(k) {
    if (!labelsHere()) return toast("这张图还不能标记");
    if (F.busy) return;
    if (F.three) leave3d();
    F.marking = k;
    labels().mark(k);
    renderList();
    sync();
  }

  // ---------- ② the 3D figure ----------

  const styleOf = (key) => STYLES.find((s) => s.key === key) || STYLES[0];

  /// Settings of the chosen look, with the user's slice choice.
  function figSettings() {
    return { ...styleOf(F.style).fig, slice: $("fig-slice-on").checked };
  }

  function applyFigure() {
    if (!F.three) return;
    const v = volume().view();
    if (!v) return;
    v.settings.showSlice = true;
    v.settings.slice = parseFloat($("fig-slice").value);
    volume().setFigure(figSettings());
  }

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));

  async function make3d() {
    if (F.busy) return;
    if (!labelsHere() || !labels().any()) return toast("先在第 1 步标记至少一种结构");
    F.busy = "open";
    F.marking = 0;
    status("正在生成三维表面…");
    sync();
    try {
      await labels().flush();
      const token = S.token;
      volume().openSurfaces();
      // The volume and the surfaces load in the background (volume.js).
      for (let i = 0; i < 1500 && !volume().surfacesReady(); i++) {
        await wait(200);
        if (token !== S.token || !F.open || !volume().isOpen()) return;
      }
      if (!volume().surfacesReady()) throw new Error("三维表面没有生成，请再试一次");
      F.three = true;
      $("fig-obj").append($("vp-obj"));
      $("fig-slice-on").checked = styleOf(F.style).fig.slice;
      applyFigure();
      pickView(F.view || "oblique");
      status("");
    } catch (e) {
      status(String(e.message || e));
    } finally {
      F.busy = "";
      renderList();
      sync();
      thumbsSoon();
    }
  }

  /// Leaves the 3D figure (the labels stay).
  function leave3d() {
    if (!F.three && !volume().isOpen()) return;
    F.three = false;
    F.mesh = null;
    const box = $("vp-obj");
    if (box && box.parentElement === $("fig-obj")) {
      box.hidden = true;
      $("vp-seg-rows").insertBefore(box, $("vp-seg-rows").children[2] || null);
    }
    if (volume().isOpen()) volume().close();
    barBox.hidden = true;
  }

  /// The dust slider: 0 = draw everything, 1 = only the largest objects
  /// (sizes on a log scale between the smallest and a tenth of the largest).
  function applyDust() {
    const m = volume().mesh();
    const t = parseFloat($("fig-dust").value);
    if (!m || !m.objects.length || !(t > 0)) {
      volume().setDust(0);
      $("fig-dust-n").textContent = "";
      return;
    }
    let lo = Infinity, hi = 0;
    for (const o of m.objects) { lo = Math.min(lo, o.pixels); hi = Math.max(hi, o.pixels); }
    lo = Math.max(1, lo);
    const top = Math.max(lo + 1, hi / 10);
    const n = Math.round(lo * Math.pow(top / lo, t)) + 1;
    volume().setDust(n);
    const gone = m.objects.filter((o) => o.pixels < n).length;
    $("fig-dust-n").textContent = gone ? `拿掉 ${gone} 个` : "";
  }

  function pickStyle(key) {
    F.style = key;
    $("fig-slice-on").checked = styleOf(key).fig.slice;
    applyFigure();
    sync();
    thumbsSoon();
  }

  function pickView(key) {
    const v = volume().view();
    const d = VIEWS.find((x) => x.key === key);
    if (!v || !d) return;
    F.view = key;
    v.lookFrom(d.at[0], d.at[1]);
    sync();
  }

  // ---------- ③ ④ thumbnails rendered from the real scene ----------

  function toCanvas(px, w, h) {
    const c = document.createElement("canvas");
    c.width = w;
    c.height = h;
    c.getContext("2d").putImageData(new ImageData(new Uint8ClampedArray(px.buffer, px.byteOffset, px.length), w, h), 0, 0);
    return c;
  }

  let thumbTimer = 0;
  function thumbsSoon() {
    clearTimeout(thumbTimer);
    thumbTimer = setTimeout(thumbs, 250);
  }

  function thumbs() {
    const v = volume().view();
    if (!F.three || !v || !volume().surfacesReady() || F.busy) return;
    F.mesh = volume().mesh();
    const h = 150, w = Math.round(Math.max(0.8, Math.min(2, v.aspect())) * h);
    const keep = { fig: v.settings.fig, yaw: v.yaw, pitch: v.pitch, dist: v.dist, target: v.target.slice() };
    const shot = () => {
      const px = v.snapshot(w, h);
      return px ? toCanvas(px, w, h) : null;
    };
    const fill = (host, items, current, make, pick) => {
      host.replaceChildren();
      for (const it of items) {
        const b = document.createElement("button");
        b.className = "fig-thumb" + (it.key === current ? " on" : "");
        b.title = it.tip;
        const c = make(it);
        if (c) b.append(c);
        const cap = document.createElement("span");
        cap.textContent = it.name;
        b.append(cap);
        b.addEventListener("click", () => pick(it.key));
        host.append(b);
      }
    };
    fill($("fig-styles"), STYLES, F.style, (st) => {
      v.settings.fig = { ...st.fig, slice: st.fig.slice && $("fig-slice-on").checked };
      return shot();
    }, pickStyle);
    v.settings.fig = keep.fig;
    // The views are framed for the thumbnail's own shape.
    fill($("fig-views"), VIEWS, F.view, (d) => {
      v.lookFrom(d.at[0], d.at[1]);
      return shot();
    }, (key) => { pickView(key); thumbsSoon(); });
    Object.assign(v, { yaw: keep.yaw, pitch: keep.pitch, dist: keep.dist, target: keep.target });
    v.settings.fig = keep.fig;
    v.draw();
  }

  // ---------- scale bar ----------

  const TO_NM = { "Å": 0.1, A: 0.1, nm: 1, "µm": 1000, "μm": 1000, um: 1000, mm: 1e6 };

  /// A round bar of about `frac` of a picture w x h: {px, text}, or null
  /// (perspective, or no pixel size).
  function scaleBar(w, h, frac = 0.18) {
    const v = volume().view();
    const upp = v ? v.unitsPerPixel(h) : 0;
    if (!(upp > 0)) return null;
    const k = TO_NM[S.px.unit];
    const per = k ? upp * k : upp; // nm (or the file's own unit) per pixel
    const target = per * w * frac;
    const e = Math.pow(10, Math.floor(Math.log10(target)));
    const len = [5, 2, 1].map((m) => m * e).find((x) => x <= target) || e;
    const text = !k ? `${+len.toPrecision(3)} ${S.px.unit}` : len >= 1000 ? `${+(len / 1000).toPrecision(3)} µm` : `${+len.toPrecision(3)} nm`;
    return { px: len / per, text };
  }

  function syncBar() {
    const v = volume().view();
    const show = F.three && F.bar && v && volume().isOpen();
    const b = show ? scaleBar(v.aspect() * v.canvas.clientHeight, v.canvas.clientHeight) : null;
    barBox.hidden = !b;
    if (!b) return;
    barBox.style.left = `${freeRect().x0 + 24}px`;
    barBox.classList.toggle("dark", F.style === "black");
    barBox.firstChild.textContent = b.text;
    barBox.lastChild.style.width = `${b.px}px`;
  }

  // ---------- ⑤ export ----------

  const stem = () => S.info.path.split(/[\\/]/).pop().replace(/\.[^.]+$/, "");

  /// Asks where to save `name`: in a cryo-ET project the dialog starts in
  /// its 5_figures folder, so the picture shows up in the side panel's
  /// 出图 group; elsewhere in the usual save folder.
  async function askPath(name, filter) {
    const own = await invoke("figure_dir", { path: S.info.path }).catch(() => null);
    const path = await invoke("pick_save_path", { dir: own || saveDir(), name, ...filter }).catch(() => null);
    if (path && !own) setSaveDir(dirname(path));
    return path;
  }
  const saved = () => document.dispatchEvent(new CustomEvent("tmv-folder-changed"));

  /// The figure at w x h as a canvas, rendered at twice the size when the
  /// GPU allows and scaled down (smooth edges), with the scale bar drawn in.
  function picture(w, h, opts = {}) {
    const v = volume().view();
    const ss = Math.max(w, h) * 2 <= Math.min(v.maxSize(), 8192) ? 2 : 1;
    const keep = v.settings.fig;
    const fig = { ...figSettings() };
    if (opts.clear) fig.bg = [fig.bg[0], fig.bg[1], fig.bg[2], 0];
    v.settings.fig = fig;
    const px = v.snapshot(w * ss, h * ss);
    v.settings.fig = keep;
    if (!px) return null;
    let c = toCanvas(px, w * ss, h * ss);
    if (ss > 1) {
      const d = document.createElement("canvas");
      d.width = w;
      d.height = h;
      const g = d.getContext("2d");
      g.imageSmoothingQuality = "high";
      g.drawImage(c, 0, 0, w, h);
      c = d;
    }
    const b = F.bar ? scaleBar(w, h) : null;
    if (b) {
      const g = c.getContext("2d");
      const m = Math.round(h * 0.04), t = Math.max(3, Math.round(h * 0.008)), font = Math.max(12, Math.round(h * 0.032));
      g.fillStyle = F.style === "black" && !opts.clear ? "#fff" : "#000";
      g.fillRect(m, h - m - t, b.px, t);
      g.font = `600 ${font}px -apple-system, "Segoe UI", "Helvetica Neue", Arial, sans-serif`;
      g.textAlign = "center";
      g.textBaseline = "bottom";
      g.fillText(b.text, m + b.px / 2, h - m - t - Math.round(font * 0.25));
    }
    return c;
  }

  /// Size with this long side and the shape of the 3D view (even numbers).
  function sizeFor(long) {
    const v = volume().view();
    const a = v.aspect();
    const cap = Math.min(long, v.maxSize());
    let w = a >= 1 ? cap : cap * a, h = a >= 1 ? cap / a : cap;
    return [Math.max(2, Math.round(w / 2) * 2), Math.max(2, Math.round(h / 2) * 2)];
  }

  async function savePng() {
    if (!F.three || F.busy) return;
    const path = await askPath(`${stem()}_figure.png`);
    if (!path) return;
    F.busy = "png";
    status("正在渲染高分辨率图片…");
    sync();
    await wait(30); // let the status show
    try {
      const [w, h] = sizeFor(3600);
      const c = picture(w, h, { clear: F.clear });
      if (!c) throw new Error("三维图还没有准备好");
      const blob = await new Promise((res) => c.toBlob(res, "image/png"));
      await writeFile(path, new Uint8Array(await blob.arrayBuffer()));
      status(`已保存 ${w} × ${h} 像素：${path}`);
      saved();
    } catch (e) {
      status(`没有保存：${e.message || e}`);
    } finally {
      F.busy = "";
      sync();
      volume().view().draw();
    }
  }

  const movieCall = (method, args, bytes) => invoke("figure_movie", bytes ?? new Uint8Array(0), {
    headers: { "x-method": method, "x-args": encodeURIComponent(JSON.stringify(args ?? null)) },
  });

  async function saveMovie() {
    if (!F.three || F.busy) return;
    const path = await askPath(`${stem()}_figure.mp4`, { filter: "MP4", exts: ["mp4"] });
    if (!path) return;
    const v = volume().view();
    const FPS = 30, N = 240;
    // H.264 here takes at most 3840 x 2160.
    let [w, h] = sizeFor(1920);
    if (h > 2160) { w = Math.round((w * 2160) / h / 2) * 2; h = 2160; }
    F.busy = "mp4";
    F.stop = false;
    sync();
    const yaw0 = v.yaw;
    let begun = false;
    try {
      await movieCall("begin", { path, width: w, height: h, fps: FPS, quality: 0.8 });
      begun = true;
      for (let i = 0; i < N; i++) {
        if (F.stop) throw new Error("已取消");
        v.yaw = yaw0 + (2 * Math.PI * i) / N;
        const c = picture(w, h);
        if (!c) throw new Error("三维图还没有准备好");
        const px = c.getContext("2d").getImageData(0, 0, w, h).data;
        await movieCall("frame", null, new Uint8Array(px.buffer, px.byteOffset, px.length));
        status(`正在做转圈视频 ${Math.round((100 * (i + 1)) / N)}%`);
      }
      await movieCall("finish");
      begun = false;
      status(`已保存 ${path}`);
      saved();
    } catch (e) {
      if (begun) await movieCall("cancel").catch(() => {});
      status(F.stop ? "已取消" : `没有保存：${e.message || e}`);
    } finally {
      v.yaw = yaw0;
      F.busy = "";
      sync();
      v.draw();
    }
  }

  // ---------- panel state ----------

  function sync() {
    const has = labelsHere() && labels().any();
    $("fig-make").disabled = !has || !!F.busy;
    $("fig-make").textContent = F.busy === "open" ? "正在生成…" : F.three ? "重新生成" : "生成三维图";
    $("fig-make").hidden = F.three;
    $("fig-back").hidden = !F.three;
    $("fig-check").hidden = !F.three;
    $("fig-dust-row").hidden = !F.three;
    $("fig-show-all").hidden = !(F.three && volume().hiddenObjects() > 0);
    for (const id of ["fig-s3", "fig-s4", "fig-s5"]) $(id).classList.toggle("off", !F.three);
    for (const id of ["fig-png", "fig-mp4", "fig-obj-export"]) $(id).disabled = !F.three || !!F.busy;
    $("fig-slice").hidden = !$("fig-slice-on").checked;
    $("fig-cancel").hidden = F.busy !== "mp4";
    for (const b of $("fig-styles").children) b.classList.toggle("on", b.lastChild.textContent === styleOf(F.style).name);
    for (const b of $("fig-views").children) b.classList.toggle("on", !!F.view && b.lastChild.textContent === (VIEWS.find((x) => x.key === F.view) || {}).name);
    syncBar();
  }

  function open() {
    if (!usable()) return;
    F.open = true;
    panel.hidden = false;
    button.classList.add("on");
    renderList();
    sync();
  }

  function close() {
    if (F.busy === "mp4") F.stop = true;
    F.open = false;
    F.marking = 0;
    panel.hidden = true;
    button.classList.remove("on");
    if (F.three) leave3d();
    barBox.hidden = true;
  }

  button.addEventListener("click", () => (F.open ? close() : open()));
  $("fig-close").addEventListener("click", close);
  $("fig-make").addEventListener("click", make3d);
  $("fig-back").addEventListener("click", () => {
    const ks = structures();
    if (ks.length) startMarking(ks.includes(F.marking) ? F.marking : ks[0]);
  });
  $("fig-show-all").addEventListener("click", () => { volume().showAll(); sync(); thumbsSoon(); });
  $("fig-slice-on").addEventListener("change", () => { applyFigure(); sync(); thumbsSoon(); });
  $("fig-slice").addEventListener("input", applyFigure);
  $("fig-dust").addEventListener("input", applyDust);
  $("fig-dust").addEventListener("change", thumbsSoon);
  $("fig-slice").addEventListener("change", thumbsSoon);
  $("fig-bar").addEventListener("change", (e) => { F.bar = e.target.checked; syncBar(); });
  $("fig-clear").addEventListener("change", (e) => { F.clear = e.target.checked; });
  $("fig-png").addEventListener("click", savePng);
  $("fig-mp4").addEventListener("click", saveMovie);
  // The mesh goes to 5_figures too, like the picture and the movie.
  $("fig-obj-export").addEventListener("click", async () => {
    if (!F.three || F.busy) return;
    const own = await invoke("figure_dir", { path: S.info.path }).catch(() => null);
    const path = await volume().exportMesh(own);
    if (path) {
      status(`已导出 ${path}`);
      saved();
    }
  });
  $("fig-cancel").addEventListener("click", () => { F.stop = true; });

  // A view dragged by hand is no longer one of the three.
  volume().canvas.addEventListener("pointerup", () => {
    if (!F.three) return;
    const v = volume().view();
    const d = VIEWS.find((x) => x.key === F.view);
    if (d && v && (Math.abs(v.yaw - d.at[0]) > 1e-3 || Math.abs(v.pitch - d.at[1]) > 1e-3)) F.view = "";
    sync();
    thumbsSoon();
  });

  // Follow the image, the labels and the 3D view.
  let seenToken = 0, seenKey = "", redirected = "";
  setInterval(() => {
    button.disabled = !usable();
    // A label map picked in the side panel (分割标签): show the image it
    // belongs to, with this panel, so marking can go on.
    const lm = S.info && /\.labels\.tif$/i.test(S.info.path) ? S.info.path : "";
    if (lm && lm !== redirected) {
      redirected = lm;
      // The label map's own tab is not wanted once its image is shown.
      const D = P.documents;
      const lmTab = D && D.current();
      openPath(lm.replace(/(\.series\d+)?\.labels\.tif$/i, "")).then(() => {
        if (lmTab && lmTab.path === lm && D.tabs.includes(lmTab) && D.current() !== lmTab) D.close(lmTab);
        if (usable()) open();
      }).catch(() => {});
      return;
    }
    if (!lm) redirected = "";
    if (!F.open) return;
    if (S.token !== seenToken) {
      seenToken = S.token;
      F.made.clear();
      F.marking = 0;
      F.three = false;
      F.mesh = null;
      if (!usable()) return close();
      renderList();
    }
    // Leaving the label tool ends marking.
    if (F.marking && S.tool !== "label") F.marking = 0;
    const key = labelsHere()
      ? structures().map((k) => `${k}:${labels().count(k) > 0}:${labels().hidden(k)}:${labels().nameOf(k)}:${labels().colorOf(k)}`).join("|") + `#${F.marking}#${F.three && volume().surfacesReady()}`
      : "";
    if (key !== seenKey) {
      seenKey = key;
      renderList();
    }
    // Labels changed under the 3D figure: volume.js rebuilt the surfaces.
    if (F.three && volume().surfacesReady() && volume().mesh() !== F.mesh && !F.busy) {
      applyDust();
      renderList();
      thumbs();
    }
    sync();
  }, 400);

  window.TifMrcViewPro.figure = {
    open,
    close,
    /** CSS pixels of the 3D view the panel covers on the right. */
    inset: () => (panel.hidden ? 0 : panel.offsetWidth + 20),
    /** volume.js closed the 3D view. */
    closed3d: () => {
      const box = $("vp-obj");
      if (box && box.parentElement === $("fig-obj")) {
        box.hidden = true;
        $("vp-seg-rows").insertBefore(box, $("vp-seg-rows").children[2] || null);
      }
      F.three = false;
      F.mesh = null;
      barBox.hidden = true;
      if (F.open) sync();
    },
  };
})();
