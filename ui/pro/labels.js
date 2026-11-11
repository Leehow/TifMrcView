"use strict";
// Pro segmentation labels: one label per pixel of the stack, made with the
// smart selection tool (click or draw around an object, then "find similar"),
// a brush, the magic wand, threshold and watershed tools. The labels are
// listed as rows of the Layers panel, above the image's layers. The label
// map lives in Rust (labels.rs) and is saved next to the image as
// <image>.labels.tif; this file draws the current slice over the image.
// Shares app.js globals: S, ext, invoke, $, setTool, showPanel, toScreen, toast, draw.
(() => {
  const COLORS = window.TifMrcViewPro.LABEL_COLORS;
  // Name, colour and flags per label, like Amira's materials (labels.rs Material).
  const mat = (k) => L.materials[k] || {};
  const colorOf = (k) => mat(k).color || COLORS[((k - 1) % 255) % COLORS.length];
  const nameOf = (k) => mat(k).name || `标签 ${k}`;
  const hidden = (k) => !!mat(k).hidden;
  const locked = (k) => !!mat(k).locked;

  const L = {
    token: 0,
    opening: 0,
    error: "",
    path: "",
    counts: [],
    slices: [],       // main label of each slice, for the Z slider marks
    materials: {},    // label -> {name, color, hidden, locked}
    active: 1,
    sub: "smart",     // smart | brush | eraser | lasso | wand | classify
    radius: 6,        // brush radius in image pixels
    guard: false,     // only paint empty pixels / only erase the active label
    tol: 0.1,         // wand tolerance, part of the display range
    threeD: false,
    opacity: 0.45,
    show: true,
    z: -1,            // slice held in `slice`
    slice: null,      // Uint16Array of the shown slice
    fetching: false,
    hover: null,      // pointer in image pixels
    stroke: null,
    lasso: null,      // {z, pts, erase} while a lasso is drawn
    sel: null,        // selected object {z, x, y} (a pixel in it)
    selDrag: null,    // {a: pointer at press, dx, dy} while it is dragged
    sam: { busy: false, status: null, progress: null, along: null, denoise: false, warmed: false },
    // Smart tool: the preview under the pointer, the stroke being drawn and
    // the last result (so it can be swapped for a smaller or larger reading).
    pick: { hover: null, timer: 0, running: false, again: false, path: null, last: null },
    cand: null,       // proposals of "find similar": {z, label, items, thr, busy, done, total}
    objects: [],      // objects of each label on the shown slice
    // Pixel classifier: scribbles of the shown slice, the trained forest's
    // summary and the prediction of the shown slice.
    cls: { z: -1, scribbles: null, stroke: null, info: null, preview: true, pred: null, predZ: -1, minConf: 0, busy: false, fetching: false, along: null },
    // U-Net trained on the labelled slices (unet.rs).
    unet: { info: null, preview: true, pred: null, predZ: -1, minConf: 0.5, busy: false, train: null, along: null },
    busy: false,
    saveTimer: 0,
  };
  const canvas = document.createElement("canvas");
  const cctx = canvas.getContext("2d");
  let pixels = null;  // ImageData of `canvas`

  const any = () => L.counts.some((n, k) => k > 0 && n > 0);
  const maxUsed = () => {
    for (let k = L.counts.length - 1; k > 0; k--) if (L.counts[k] > 0) return k;
    return 0;
  };
  const channel = () => Math.min(S.sel, S.chans.length - 1);

  const EYE = '<svg viewBox="0 0 24 24"><path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>';
  const EYE_OFF = '<svg viewBox="0 0 24 24"><path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><path d="M4 4l16 16"/></svg>';
  const LOCK = '<svg viewBox="0 0 24 24"><rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/></svg>';
  const UNLOCK = '<svg viewBox="0 0 24 24"><rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 7.5-2"/></svg>';
  const GRIP = '<svg viewBox="0 0 24 24"><circle cx="9" cy="6" r="1.4"/><circle cx="15" cy="6" r="1.4"/><circle cx="9" cy="12" r="1.4"/><circle cx="15" cy="12" r="1.4"/><circle cx="9" cy="18" r="1.4"/><circle cx="15" cy="18" r="1.4"/></svg>';

  // ---------- panel ----------

  const sec = document.createElement("section");
  sec.id = "sec-labels";
  sec.innerHTML = `
    <h3>标签分割 <span class="dim" id="lab-busy"></span></h3>
    <div class="row" id="lab-tools">
      <button class="small" data-sub="smart" title="智能选择：在物体上点一下就选出它；绕着它画一圈、或者沿着它画一笔，选得更准。按住 Alt 是减去。选中后可以「找同类」"><svg viewBox="0 0 24 24"><path d="M4 3l7 17 2.2-6.8L20 11z"/><path d="M17 3v3M15.5 4.5h3M20 16v2M19 17h2"/></svg></button>
      <button class="small" data-sub="brush" title="画笔：按住拖动涂上当前标签（[ ] 调大小）"><svg viewBox="0 0 24 24"><path d="M4 20c2.5 0 4-1.5 4-4a3 3 0 0 0-3-3"/><path d="M8 13l9-9 3 3-9 9"/></svg></button>
      <button class="small" data-sub="eraser" title="橡皮：擦掉标签（[ ] 调大小）"><svg viewBox="0 0 24 24"><path d="M8 20h12"/><path d="M4 15l9-9 6 6-7 7H8z"/></svg></button>
      <button class="small" data-sub="lasso" title="套索：按住画一圈，松开后把圈里涂上当前标签（按住 Alt 画圈是擦除）"><svg viewBox="0 0 24 24"><path d="M12 4c5 0 9 2.5 9 6s-4 6-9 6c-1.5 0-3-.2-4.2-.6"/><path d="M12 4C7 4 3 6.5 3 10c0 1.6.9 3 2.3 4.1"/><circle cx="6" cy="17" r="2"/><path d="M6 19c0 1.5-1 2.5-2 3"/></svg></button>
      <button class="small" data-sub="wand" title="魔棒：点一下，把和点击处亮度相近的连通区域涂上当前标签"><svg viewBox="0 0 24 24"><path d="M4 20L15 9"/><path d="M17 3v3M17 12v3M11 7h3M20 7h3M19 5l1.5-1.5M19 9l1.5 1.5"/></svg></button>
      <button class="small" data-sub="classify" title="像素分类：用画笔给每种结构（包括背景）各画几笔不同的标签，训练后自动分出整层或整个栈（按住 Alt 擦掉标注）"><svg viewBox="0 0 24 24"><path d="M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4z"/><path d="M14 17h6M17 14v6"/></svg></button>
    </div>
    <div class="dim" id="lab-hint">在物体上点一下，或者绕着它画一圈。选中后点「找同类」，一次找出所有一样的。</div>
    <label class="slider" id="lab-size-row">大小 <input type="range" id="lab-size" min="1" max="100" step="1" /><span id="lab-size-v"></span></label>
    <label class="slider" id="lab-tol-row">容差 <input type="range" id="lab-tol" min="0" max="0.5" step="0.005" /><span id="lab-tol-v"></span></label>
    <div class="row">
      <label id="lab-3d-row" title="魔棒沿 Z 方向一起长到相邻的层"><input type="checkbox" id="lab-3d" /> <span id="lab-3d-text">三维</span></label>
      <label title="画笔和魔棒只涂没有标签的地方；橡皮只擦当前标签"><input type="checkbox" id="lab-guard" /> 不覆盖其他标签</label>
    </div>
    <div id="lab-sam" hidden>
      <div class="dim" id="lab-sam-text"></div>
      <div class="row">
        <button class="small" id="lab-sam-get">下载模型</button>
      </div>
    </div>
    <div id="lab-cls" hidden>
      <div class="dim" id="lab-cls-text"></div>
      <div class="dim" id="lab-cls-model-record" hidden></div>
      <div class="row">
        <button class="small" id="lab-cls-train" title="用所有层上的标注训练分类器">训练</button>
        <label title="在当前层上显示分类结果"><input type="checkbox" id="lab-cls-preview" /> 预览</label>
        <button class="small" id="lab-cls-clear" title="清除所有标注和训练好的分类器">清除标注</button>
      </div>
      <label class="slider" title="把握不到这个程度的像素不写入标签">最低把握 <input type="range" id="lab-cls-conf" min="0" max="0.95" step="0.05" /><span id="lab-cls-conf-v"></span></label>
      <div class="row">
        <button class="small" id="lab-cls-z" title="把当前层的分类结果写进标签">应用到本层</button>
        <button class="small" id="lab-cls-all" title="对每一层分类并写进标签，一步就能撤回（Esc 停止）">应用到所有层</button>
      </div>
    </div>
    <div class="row">
      <button class="small" id="lab-3d-surf" title="把所有标签做成光滑的三维表面，叠在断层切片上显示；点一个物体可以隐藏、删除或换标签">3D 表面</button>
      <button class="small" id="lab-interp" title="隔几层画一个轮廓，再按这里，把当前标签画过的层之间的层按形状自动补齐">隔层插值</button>
    </div>
    <details id="lab-th">
      <summary>阈值</summary>
      <div class="row">
        <input type="number" id="lab-lo" step="any" title="最小值" /> <span>–</span> <input type="number" id="lab-hi" step="any" title="最大值" />
        <button class="small" id="lab-range" title="填入当前通道的显示范围">用显示范围</button>
      </div>
      <div class="row">
        <label title="每个连通区域各自一个新标签，方便数颗粒"><input type="checkbox" id="lab-split" /> 分成独立区域</label>
        <label title="比这个小的区域忽略（像素）">最小 <input type="number" id="lab-min" min="1" value="20" style="width: 64px" /></label>
      </div>
      <div class="row">
        <button class="small" id="lab-th-z">本层</button>
        <button class="small" id="lab-th-all">全部层</button>
      </div>
    </details>
    <details id="lab-ws">
      <summary>分水岭</summary>
      <div class="dim">先用画笔在每个物体和背景里各画一笔不同的标签当种子，再按下面的按钮，剩下没有标签的像素会沿图像边缘分给最近的种子。</div>
      <div class="row">
        <button class="small" id="lab-ws-z">本层</button>
        <button class="small" id="lab-ws-all">全部层</button>
      </div>
    </details>
    <details id="lab-mem">
      <summary>膜分割（断层）</summary>
      <div class="dim">一键找出冷冻电镜断层里的所有生物膜，写进标签「膜」；其中呈球壳状的单独写进「囊泡」，并测出直径；微管写进「微管」。用 MemBrain-seg 官方模型：第一次使用时按提示下载并选择模型文件，以后自动载入。像素大小从文件读取，可以一步撤回。</div>
      <div class="row">
        <button class="small primary" id="lab-mem-run" title="分割整个断层里的膜（Esc 停止）">自动分割膜</button>
      </div>
      <div class="dim" id="lab-mem-text"></div>
      <canvas id="lab-ves-hist" hidden title="囊泡直径（膜中线）分布"></canvas>
      <div class="row">
        <button class="small" id="lab-ves-csv" hidden title="把每个囊泡的中心坐标（像素）和直径（nm）存成 CSV">导出囊泡表</button>
      </div>
    </details>
    <details id="lab-unet">
      <summary>深度学习 U-Net</summary>
      <div class="dim" id="lab-unet-text"></div>
      <div class="dim" id="lab-unet-model-record" hidden></div>
      <div class="row">
        <label title="训练步数，越多越准也越慢">步数 <input type="number" id="lab-unet-steps" min="50" max="20000" step="50" value="400" style="width: 72px" /></label>
        <button class="small" id="lab-unet-train" title="用已经画好标签的层训练 U-Net（Esc 停止）">训练</button>
        <label title="在当前层上显示 U-Net 的结果"><input type="checkbox" id="lab-unet-preview" checked /> 预览</label>
      </div>
      <label class="slider" title="把握不到这个程度的像素不写入标签">最低把握 <input type="range" id="lab-unet-conf" min="0" max="0.95" step="0.05" value="0.5" /><span id="lab-unet-conf-v"></span></label>
      <div class="row">
        <button class="small" id="lab-unet-z" title="把当前层的结果写进标签">应用到本层</button>
        <button class="small" id="lab-unet-all" title="对所有还没有标签的层分割并写进标签，一步就能撤回（Esc 停止）">应用到其余层</button>
      </div>
    </details>
    <div id="lab-file" class="dim"></div>`;
  $("sec-ann").after(sec);

  // The labels are rows of the Layers panel (layers.js), above the image's
  // layers: select, show, lock, rename, reorder and merge them there.
  const layersBox = document.createElement("div");
  layersBox.id = "label-layers";
  layersBox.hidden = true;
  layersBox.innerHTML = `
    <div class="ll-head"><b>标签</b><span class="ll-space"></span>
      <input type="range" id="lab-op" min="0.05" max="1" step="0.05" title="标签的不透明度" aria-label="标签的不透明度" />
      <button class="ll-btn" id="lab-show" title="显示 / 隐藏所有标签"></button>
      <button class="ll-btn" id="lab-new" title="新标签：之后选出来的东西放进一个新的标签">＋</button>
    </div>
    <div id="lab-list" role="list" aria-label="标签"></div>`;
  if ($("merge-list")) $("merge-list").before(layersBox);
  else sec.querySelector("#lab-th").before(layersBox);

  function syncLayersBox() {
    const D = window.TifMrcViewPro.documents;
    const merge = D && D.current() && D.current().type === "merge";
    layersBox.hidden = !S.info || merge || S.axis !== "xy" || L.token !== S.token || !!L.error || !(any() || S.tool === "label");
  }
  document.addEventListener("tmv-document", () => setTimeout(syncLayersBox));

  const modelContext = () => ({ token: S.token, c: channel(), t: S.t });
  const clsRecords = window.TifMrcViewPro.models.attach("classifier", "lab-cls-model-record", modelContext);
  const unetRecords = window.TifMrcViewPro.models.attach("unet", "lab-unet-model-record", modelContext);

  async function restoreModels() {
    if (L.token !== S.token || S.axis !== "xy" || L.busy || L.cls.busy || L.unet.busy) return;
    const req = modelContext(), key = JSON.stringify(req);
    if (key === L.restoreModelsKey) return;
    L.restoreModelsKey = key;
    L.restoringModels = true;
    Object.assign(L.cls, { info: null, pred: null, predZ: -1 });
    Object.assign(L.unet, { info: null, pred: null, predZ: -1 });
    syncCls(); syncUnet();
    const [classifier, unet] = await Promise.all([clsRecords.restore(req), unetRecords.restore(req)]);
    if (key !== L.restoreModelsKey || key !== JSON.stringify(modelContext())) return;
    L.restoringModels = false;
    if (classifier.saved) L.cls.info = classifier.saved.summary;
    if (unet.saved) {
      L.unet.info = unet.saved.summary;
      window.TifMrcViewPro.models.setValue("lab-unet-steps", unet.saved.record.parameters.effective.config.steps);
    }
    syncCls(); syncUnet(); draw();
  }

  // Quick toolbar button, after the crop tool.
  const btn = document.createElement("button");
  btn.dataset.tool = "label";
  btn.title = "标签分割 (G)：画笔、橡皮、魔棒、阈值、分水岭，结果自动存成图片旁边的 .labels.tif";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><path d="M3 12c0-5 4-9 9-9s9 4 9 9-4 9-9 9"/><path d="M12 21c-2 0-3-1.5-3-3 0-2 3-3 3-6 2 2 4 4 4 6s-2 3-4 3z"/></svg>';
  document.querySelector('#toolbtns [data-tool="crop"]').after(btn);
  btn.addEventListener("click", () => setTool("label"));
  TOOL_KEYS.g = "label";

  function syncPanel() {
    for (const b of sec.querySelectorAll("#lab-tools button")) b.classList.toggle("on", b.dataset.sub === L.sub);
    $("lab-size-row").hidden = L.sub === "wand" || L.sub === "lasso" || L.sub === "smart";
    $("lab-hint").hidden = L.sub !== "smart";
    syncSam();
    syncCls();
    syncUnet();
    syncMem();
    $("lab-tol-row").hidden = L.sub !== "wand";
    $("lab-3d-row").hidden = L.sub !== "wand" && !(L.sub === "smart" && S.info && S.info.sizeZ > 1);
    $("lab-3d-text").textContent = L.sub === "smart" ? "整个物体（所有层）" : "三维";
    $("lab-3d-row").title = L.sub === "smart" ? "挪动、删除和换标签时连同相邻层里连着的部分一起" : "魔棒沿 Z 方向一起长到相邻的层";
    $("lab-size").value = L.radius;
    $("lab-size-v").textContent = `${L.radius * 2}`;
    $("lab-tol").value = L.tol;
    $("lab-tol-v").textContent = `${Math.round(L.tol * 100)}%`;
    $("lab-op").value = L.opacity;
    $("lab-3d").checked = L.threeD;
    $("lab-guard").checked = L.guard;
    $("lab-show").innerHTML = L.show ? EYE : EYE_OFF;
    $("lab-show").classList.toggle("off", !L.show);
    $("lab-busy").textContent = L.busy ? "处理中…" : "";
    for (const b of sec.querySelectorAll("#lab-th button, #lab-ws button, #lab-interp")) b.disabled = L.busy || !L.token;
    $("lab-3d-surf").disabled = L.busy || !any() || !(S.info && S.info.sizeZ > 1);
  }

  // Place in the list: where the user dragged it, otherwise by label number.
  const orderOf = (k) => mat(k).order ?? 1e6 + k;
  /** Labels in the list, top first: the ones in use and the active one. */
  function listed() {
    const ks = [];
    for (let k = 1; k < L.counts.length; k++) if (L.counts[k] > 0) ks.push(k);
    if (!ks.includes(L.active)) ks.push(L.active);
    return ks.sort((a, b) => orderOf(a) - orderOf(b) || a - b);
  }

  function renderList() {
    syncLayersBox();
    const list = $("lab-list");
    list.textContent = "";
    const ks = listed();
    // Thousands of regions after a split: list the active one and the first few.
    const shown = ks.length > 200 ? ks.filter((k, i) => i < 200 || k === L.active) : ks;
    for (const k of shown) {
      const row = document.createElement("div");
      row.className = "lab-row" + (k === L.active ? " on" : "") + (hidden(k) ? " off" : "") + (locked(k) ? " locked" : "");
      row.dataset.label = k;
      row.setAttribute("role", "listitem");
      const n = L.counts[k] || 0;
      row.innerHTML = `<span class="grip" title="拖动排序；拖到另一个标签上，把两个合并">${GRIP}</span>`
        + `<button class="eye" title="显示 / 隐藏（2D 和 3D）">${hidden(k) ? EYE_OFF : EYE}</button>`
        + `<button class="sw" title="改颜色"></button><span class="name" title="双击改名"></span><span class="n dim"></span>`
        + `<button class="lock" title="锁定：其他工具不会改动这个标签">${locked(k) ? LOCK : UNLOCK}</button>`
        + `<button class="del" title="删除这个标签">✕</button>`;
      const hex = "#" + colorOf(k).map((x) => x.toString(16).padStart(2, "0")).join("");
      row.querySelector(".sw").style.background = hex;
      row.querySelector(".name").textContent = nameOf(k);
      row.querySelector(".name").addEventListener("dblclick", () => rename(k, row.querySelector(".name")));
      row.querySelector(".del").hidden = !n;
      row.querySelector(".grip").addEventListener("pointerdown", (e) => gripDown(e, k, row));
      row.addEventListener("click", (e) => {
        const b = e.target.closest("button, label, input, .grip");
        if (b && b.classList.contains("sw")) return pickColor(k, b, hex);
        if (b && b.classList.contains("del")) return edit({ kind: "delete", label: k });
        if (b && b.classList.contains("eye")) return setMaterial(k, { hidden: !hidden(k) });
        if (b && b.classList.contains("lock")) return setMaterial(k, { locked: !locked(k) });
        if (b) return;
        // Only move the highlight, so a double-click still reaches the name.
        L.active = k;
        for (const r of list.querySelectorAll(".lab-row")) r.classList.toggle("on", r === row);
      });
      list.appendChild(row);
    }
    if (shown.length < ks.length) {
      const more = document.createElement("div");
      more.className = "dim";
      more.textContent = `共 ${ks.length} 个标签，只列出前 200 个`;
      list.appendChild(more);
    }
    syncCounts();
  }

  /** The number beside each label: its objects on the shown slice. */
  function syncCounts() {
    for (const row of $("lab-list").querySelectorAll(".lab-row")) {
      const k = +row.dataset.label, n = L.counts[k] || 0, el = row.querySelector(".n");
      const objs = L.z === S.z ? L.objects[k] || 0 : 0;
      el.textContent = objs ? `${objs}` : "";
      el.title = n ? (objs ? `这一层 ${objs} 个对象 · ${sizeText(n)}` : sizeText(n)) : "";
    }
  }

  /** Dragging a row by its grip: between two rows reorders, onto a row merges. */
  function gripDown(e, k, row) {
    if (e.button !== 0) return;
    e.preventDefault();
    const grip = e.currentTarget, list = $("lab-list");
    grip.setPointerCapture(e.pointerId);
    let target = null;
    const clear = () => { for (const r of list.children) r.classList.remove("drop-before", "drop-after", "drop-into", "dragging"); };
    const move = (ev) => {
      clear();
      row.classList.add("dragging");
      target = null;
      const el = document.elementFromPoint(ev.clientX, ev.clientY);
      const over = el && el.closest("#lab-list .lab-row");
      if (!over || over === row) return;
      const r = over.getBoundingClientRect(), f = (ev.clientY - r.top) / r.height;
      target = { k: +over.dataset.label, where: f < 0.28 ? "before" : f > 0.72 ? "after" : "into" };
      over.classList.add(`drop-${target.where}`);
    };
    const up = () => {
      grip.removeEventListener("pointermove", move);
      grip.removeEventListener("pointerup", up);
      grip.removeEventListener("pointercancel", up);
      clear();
      if (!target) return;
      if (target.where === "into") return mergeInto(k, target.k);
      const ks = listed().filter((x) => x !== k);
      ks.splice(ks.indexOf(target.k) + (target.where === "after" ? 1 : 0), 0, k);
      setMaterials(Object.fromEntries(ks.map((x, i) => [x, { order: i }])));
    };
    grip.addEventListener("pointermove", move);
    grip.addEventListener("pointerup", up);
    grip.addEventListener("pointercancel", up);
  }

  async function mergeInto(from, to) {
    if (!(L.counts[from] > 0)) return toast("这个标签还是空的，没有可合并的");
    if (locked(from) || locked(to)) return toast("这个标签锁定了");
    const text = `「${nameOf(from)}」并入了「${nameOf(to)}」`;
    const st = await edit({ kind: "merge", from, to });
    if (!st) return;
    if (L.active === from) L.active = to;
    renderList();
    toast(text, 4000);
  }

  // The colour picker: one input shared by all rows (see #lab-color in pro.css).
  const colorInput = document.createElement("input");
  colorInput.type = "color";
  colorInput.id = "lab-color";
  colorInput.tabIndex = -1;
  sec.appendChild(colorInput);
  let colorFor = 0, colorTimer = 0;
  function pickColor(k, swatch, hex) {
    colorFor = k;
    const r = swatch.getBoundingClientRect();
    colorInput.style.left = `${r.left}px`;
    colorInput.style.top = `${r.top}px`;
    colorInput.value = hex;
    colorInput.click();
  }
  function takeColor(now) {
    clearTimeout(colorTimer);
    const k = colorFor, v = colorInput.value;
    const apply = () => {
      const rgb = [1, 3, 5].map((i) => parseInt(v.slice(i, i + 2), 16));
      if (k && rgb.join() !== colorOf(k).join()) setMaterial(k, { color: rgb });
    };
    if (now) apply();
    else colorTimer = setTimeout(apply, 250);
  }
  // Live while dragging in the picker, at most every 250 ms; `change` when it closes.
  colorInput.addEventListener("input", () => takeColor(false));
  colorInput.addEventListener("change", () => takeColor(true));

  /** Changes labels' materials ({label: patch}) and stores them with the label file. */
  function setMaterials(patches) {
    const next = { ...L.materials };
    for (const [k, patch] of Object.entries(patches)) next[k] = { label: +k, name: "", color: null, hidden: false, locked: false, order: null, ...mat(k), ...patch };
    L.materials = next;
    const list = Object.values(next).filter((m) => m.name || m.color || m.hidden || m.locked || m.order != null);
    if (L.slice) repaint(0, 0, S.info.width, S.info.height);
    renderList();
    renderZMarks();
    if (window.TifMrcViewPro.volume) window.TifMrcViewPro.volume.materialsChanged();
    draw();
    edit({ kind: "materials", materials: list });
  }
  const setMaterial = (k, patch) => setMaterials({ [k]: patch });

  function rename(k, el) {
    const input = document.createElement("input");
    input.className = "name-edit";
    input.value = nameOf(k);
    el.replaceWith(input);
    input.focus();
    input.select();
    let done = false;
    const finish = (save) => {
      if (done) return;
      done = true;
      const v = input.value.trim();
      if (save && v !== nameOf(k)) setMaterial(k, { name: v === `标签 ${k}` ? "" : v });
      else renderList();
    };
    input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") finish(true);
      if (e.key === "Escape") finish(false);
    });
    input.addEventListener("blur", () => finish(true));
  }

  /** 256 RGBA entries for the 3D view: reduced label byte -> colour, alpha 0 = hidden. */
  function colorTable() {
    const t = new Uint8Array(256 * 4);
    for (let b = 1; b < 256; b++) {
      t.set(colorOf(b), b * 4);
      t[b * 4 + 3] = hidden(b) ? 0 : 255;
    }
    return t;
  }

  /** Pixel count, plus the volume when the pixel size is known. */
  function sizeText(n) {
    let s = `${n.toLocaleString()} 像素`;
    if (S.px && S.info && S.info.sizeZ > 1 && S.info.pixelSize && S.info.pixelSize[2] > 0) {
      const v = n * S.px.x * S.px.y * S.info.pixelSize[2];
      s += ` · ${fmt(+v.toPrecision(3))} ${S.px.unit}³`;
    }
    return s;
  }

  function fileText() {
    if (L.error) return L.error;
    const name = L.path ? L.path.split(/[\\/]/).pop() : "";
    return any() ? `自动保存到 ${name}` : "标签会在画完几秒后自动存到图片旁边的 .labels.tif（16 位 TIFF，Fiji / napari 可直接打开）";
  }

  // ---------- label map of the open image ----------

  async function ensure() {
    if (!S.info || S.axis !== "xy" || L.token === S.token || L.opening === S.token) return;
    const token = S.token;
    L.opening = token;
    syncMem();
    await flush();
    try {
      const st = await invoke("labels_open", { token });
      if (token !== S.token) return;
      // Noisy tomograms segment better denoised (same rule as the 3D view's invert).
      const i = S.info;
      const tomo = /mrc|rec|st\b/i.test(i.format) && i.sizeZ * 2 <= Math.max(i.width, i.height);
      L.sam.denoise = tomo;
      Object.assign(L.pick, { hover: null, path: null, last: null });
      Object.assign(L, { cand: null, sel: null, objects: [] });
      syncCand();
      Object.assign(L.cls, { z: -1, scribbles: null, stroke: null, info: null, pred: null, predZ: -1 });
      Object.assign(L.unet, { info: null, pred: null, predZ: -1 });
      Object.assign(L, { token, path: st.path, counts: st.counts, slices: st.slices, materials: matMap(st.materials), error: "", z: -1, slice: null, active: Math.max(1, maxUsedOf(st.counts)) });
    } catch (e) {
      if (token !== S.token) return;
      Object.assign(L, { token, path: "", counts: [], slices: [], materials: {}, error: `标签文件读取失败：${e}`, z: -1, slice: null });
    } finally {
      if (L.opening === token) L.opening = 0;
    }
    $("lab-file").textContent = fileText();
    renderList();
    renderZMarks();
    syncPanel();
    if (window.TifMrcViewPro.volume) window.TifMrcViewPro.volume.labelsChanged();
    draw();
  }
  const matMap = (list) => Object.fromEntries((list || []).map((m) => [m.label, m]));
  const maxUsedOf = (counts) => {
    for (let k = counts.length - 1; k > 0; k--) if (counts[k] > 0) return k;
    return 0;
  };

  async function fetchSlice() {
    if (L.fetching || L.error || !L.token) return;
    const token = L.token, z = S.z;
    L.fetching = true;
    try {
      const buf = await invoke("labels_slice", { token, z });
      if (token !== L.token) return;
      L.slice = new Uint16Array(buf);
      L.z = z;
      repaint(0, 0, S.info.width, S.info.height);
      invoke("labels_objects", { token, z }).then((o) => {
        if (token !== L.token || L.z !== z) return;
        L.objects = o;
        syncCounts();
      }).catch(() => {});
    } catch (_) {
      // A new image is opening; the next draw asks again.
    } finally {
      L.fetching = false;
    }
    draw();
  }

  /** Colours the label slice into the overlay canvas, for one rectangle. */
  function repaint(x0, y0, x1, y1) {
    const w = S.info.width, h = S.info.height;
    if (canvas.width !== w || canvas.height !== h || !pixels) {
      canvas.width = w;
      canvas.height = h;
      pixels = cctx.createImageData(w, h);
      x0 = 0; y0 = 0; x1 = w; y1 = h;
    }
    x0 = Math.max(0, x0 | 0); y0 = Math.max(0, y0 | 0);
    x1 = Math.min(w, Math.ceil(x1)); y1 = Math.min(h, Math.ceil(y1));
    if (x1 <= x0 || y1 <= y0) return;
    const px = pixels.data, s = L.slice;
    for (let y = y0; y < y1; y++) {
      for (let x = x0; x < x1; x++) {
        const i = y * w + x, k = s[i], o = i * 4;
        if (k && !hidden(k)) {
          const c = colorOf(k);
          px[o] = c[0]; px[o + 1] = c[1]; px[o + 2] = c[2]; px[o + 3] = 255;
        } else {
          px[o + 3] = 0;
        }
      }
    }
    cctx.putImageData(pixels, 0, 0, x0, y0, x1 - x0, y1 - y0);
  }

  function overlay(g) {
    if (!S.info) return;
    renderZMarks();
    if (S.axis === "xy" && L.token !== S.token) ensure();
    const ready = S.axis === "xy" && L.token === S.token && !L.error;
    if (ready) restoreModels();
    if (ready && L.z !== S.z && (any() || S.tool === "label")) fetchSlice();
    if (ready && L.show && L.slice && L.z === S.z && (any() || L.stroke)) {
      const [sx, sy] = toScreen(0, 0);
      g.save();
      g.globalAlpha = L.opacity;
      g.imageSmoothingEnabled = false;
      g.drawImage(canvas, sx, sy, S.info.width * S.view.scale, S.info.height * S.view.scale);
      g.restore();
    }
    drawLive(g);
    drawCls(g);
    drawUnet(g);
    drawPick(g);
    drawSelection(g);
    placeChips();
    if (L.lasso && L.lasso.z === S.z && L.lasso.pts.length > 1) {
      g.save();
      g.beginPath();
      L.lasso.pts.forEach(([x, y], i) => {
        const [sx, sy] = toScreen(x, y);
        if (i) g.lineTo(sx, sy);
        else g.moveTo(sx, sy);
      });
      g.closePath();
      if (!L.lasso.erase) {
        g.globalAlpha = L.opacity * 0.6;
        g.fillStyle = `rgb(${colorOf(L.active).join(",")})`;
        g.fill();
        g.globalAlpha = 1;
      }
      g.setLineDash([5, 4]);
      g.lineWidth = 1.5;
      g.strokeStyle = L.lasso.erase ? "#fff" : `rgb(${colorOf(L.active).join(",")})`;
      g.stroke();
      g.restore();
    }
    // Brush outline under the pointer.
    if (S.tool === "label" && (L.sub === "brush" || L.sub === "eraser" || L.sub === "classify") && L.hover) {
      const [cx, cy] = toScreen(L.hover[0], L.hover[1]);
      g.save();
      g.beginPath();
      g.arc(cx, cy, Math.max(1, L.radius * S.view.scale), 0, Math.PI * 2);
      g.lineWidth = 1.5;
      g.strokeStyle = "rgba(0, 0, 0, 0.6)";
      g.stroke();
      g.lineWidth = 1;
      g.strokeStyle = L.sub === "eraser" ? "#fff" : `rgb(${colorOf(L.active).join(",")})`;
      g.stroke();
      g.restore();
    }
  }
  ext.overlays.push(overlay);

  // ---------- Z slider: which slices hold labels ----------

  // A band under the slider, in each slice's main label colour; the
  // annotation marks stay above it.
  const zband = document.createElement("div");
  zband.id = "zlabels";
  $("zwrap").appendChild(zband);
  let zbandKey = "";

  function renderZMarks() {
    const n = S.info ? S.info.sizeZ : 0;
    const show = n > 1 && S.axis === "xy" && L.token === S.token && L.slices.length === n;
    const key = show ? L.slices.join(",") + JSON.stringify(L.materials) : "";
    if (key === zbandKey) return;
    zbandKey = key;
    zband.textContent = "";
    if (!show) return;
    for (let z = 0; z < n; ) {
      const k = L.slices[z];
      let e = z + 1;
      while (e < n && L.slices[e] === k) e++;
      if (k && !hidden(k)) {
        const i = document.createElement("i");
        i.style.left = `${(z / (n - 1)) * 100}%`;
        i.style.width = `calc(${((e - 1 - z) / (n - 1)) * 100}% + 6px)`;
        i.style.background = `rgb(${colorOf(k).join(",")})`;
        i.title = `${nameOf(k)} · ` + (e - z > 1 ? `Z ${z + 1}–${e}` : `Z ${z + 1}`);
        zband.appendChild(i);
      }
      z = e;
    }
  }

  // ---------- editing ----------

  const edit = (op) => editWith("labels_edit", { token: L.token, op });

  /** Runs a label-changing command and takes over the new label state. */
  async function editWith(cmd, args, replaces = false) {
    if (!L.token || L.error) return;
    const token = L.token;
    const op = args.op || { kind: cmd };
    L.pick.last = null;
    L.pick.hover = null;
    L.lastError = "";
    L.busy = true;
    syncPanel();
    try {
      const st = await invoke(cmd, args);
      if (token !== L.token) return;
      L.counts = st.counts;
      L.slices = st.slices;
      L.materials = matMap(st.materials);
      renderZMarks();
      if (st.zs.includes(S.z) || L.z !== S.z) L.z = -1;
      if (st.zs.length || op.kind === "materials") scheduleSave();
      // A step labels.rs can undo goes into the app's shared history.
      // (Not one that replaces the step before it: sam_apply with `replace`.)
      if (st.zs.length && op.kind !== "undo" && op.kind !== "redo" && op.kind !== "materials" && !replaces) ext.pushUndo("label");
      renderList();
      if (window.TifMrcViewPro.volume) window.TifMrcViewPro.volume.labelsChanged();
      return st;
    } catch (e) {
      L.lastError = String(e);
      toast(L.lastError, 6000);
    } finally {
      L.busy = false;
      syncPanel();
      $("lab-file").textContent = fileText();
      draw();
    }
  }

  function scheduleSave() {
    clearTimeout(L.saveTimer);
    L.saveTimer = setTimeout(flush, 1500);
  }

  async function flush() {
    if (!L.saveTimer || !L.token) return;
    clearTimeout(L.saveTimer);
    L.saveTimer = 0;
    try {
      await invoke("labels_save", { token: L.token });
    } catch (e) {
      if (e !== "stale") toast(`标签保存失败：${e}`, 6000);
    }
  }

  /** Paints the brush into the local slice right away (labels.rs repeats it). */
  function stampLocal(cx, cy) {
    const w = S.info.width, h = S.info.height, r = Math.max(0.5, L.radius), s = L.slice;
    const label = L.sub === "eraser" ? 0 : L.active;
    const guard = L.guard ? L.active : 0;
    const x0 = Math.max(0, Math.floor(cx - r)), y0 = Math.max(0, Math.floor(cy - r));
    const x1 = Math.min(w, Math.ceil(cx + r)), y1 = Math.min(h, Math.ceil(cy + r));
    for (let y = y0; y < y1; y++) {
      const dy = y + 0.5 - cy;
      for (let x = x0; x < x1; x++) {
        const dx = x + 0.5 - cx, i = y * w + x, cur = s[i];
        if (dx * dx + dy * dy > r * r) continue;
        if (cur && cur !== label && locked(cur)) continue;
        if (guard && (label ? cur !== 0 && cur !== label : cur !== guard)) continue;
        s[i] = label;
      }
    }
    repaint(x0, y0, x1, y1);
  }

  function strokeTo(p) {
    const pts = L.stroke.pts;
    const a = pts[pts.length - 1];
    const len = Math.hypot(p[0] - a[0], p[1] - a[1]);
    const step = Math.max(0.5, L.radius / 2);
    const n = Math.max(1, Math.ceil(len / step));
    for (let i = 1; i <= n; i++) stampLocal(a[0] + ((p[0] - a[0]) * i) / n, a[1] + ((p[1] - a[1]) * i) / n);
    pts.push(p);
  }

  function ready() {
    if (S.axis !== "xy") {
      toast("标签只能在 XY 视图里画");
      return false;
    }
    if (L.error) {
      toast(L.error, 6000);
      return false;
    }
    return L.token === S.token && !L.busy;
  }

  // ---------- the selected object: drag, delete, outline ----------

  /** The 4-connected region of one label containing (x, y) on the shown
   *  slice: its pixels, their set, bounding box and outline segments. */
  function regionAt(x, y) {
    const w = S.info.width, h = S.info.height, s = L.slice;
    if (!s || x < 0 || y < 0 || x >= w || y >= h) return null;
    const k = s[y * w + x];
    if (!k) return null;
    const seen = new Uint8Array(w * h), out = [], stack = [y * w + x];
    seen[y * w + x] = 1;
    const box = [x, y, x, y], segs = [];
    while (stack.length) {
      const i = stack.pop();
      out.push(i);
      const px = i % w, py = (i / w) | 0;
      if (px < box[0]) box[0] = px;
      if (py < box[1]) box[1] = py;
      if (px > box[2]) box[2] = px;
      if (py > box[3]) box[3] = py;
      // Left, right, up, down: a neighbour of the region or a side of its outline.
      const near = [px > 0 ? i - 1 : -1, px + 1 < w ? i + 1 : -1, py > 0 ? i - w : -1, py + 1 < h ? i + w : -1];
      for (let n = 0; n < 4; n++) {
        const j = near[n];
        if (j >= 0 && s[j] === k) {
          if (!seen[j]) { seen[j] = 1; stack.push(j); }
        } else if (n === 0) segs.push(px, py, px, py + 1);
        else if (n === 1) segs.push(px + 1, py, px + 1, py + 1);
        else if (n === 2) segs.push(px, py, px + 1, py);
        else segs.push(px, py + 1, px + 1, py + 1);
      }
    }
    return { k, pix: out, set: new Set(out), box, segs };
  }
  let selCache = { key: "", r: null };
  function selRegion() {
    if (!L.sel || L.sel.z !== S.z || L.z !== S.z || !L.slice) return null;
    const key = `${L.sel.x},${L.sel.y},${L.sel.z}`;
    if (selCache.key !== key || selCache.slice !== L.slice) selCache = { key, slice: L.slice, r: regionAt(L.sel.x, L.sel.y) };
    return selCache.r;
  }

  async function selectUp() {
    const d = L.selDrag;
    L.selDrag = null;
    if (!d || !L.sel || (!d.dx && !d.dy)) return draw();
    const { x, y, z } = L.sel;
    const st = await edit({ kind: "moveRegion", x, y, z, dx: d.dx, dy: d.dy, threeD: L.threeD });
    if (st) L.sel = { z, x: Math.min(S.info.width - 1, Math.max(0, x + d.dx)), y: Math.min(S.info.height - 1, Math.max(0, y + d.dy)) };
    syncPanel();
  }

  async function selectDelete() {
    if (!L.sel || L.busy) return;
    const { x, y, z } = L.sel;
    L.sel = null;
    await edit({ kind: "deleteRegion", x, y, z, threeD: L.threeD });
    syncPanel();
  }

  /** Dashed outline of the selected object, moved with the drag. */
  function drawSelection(g) {
    if (S.tool !== "label" || L.sub !== "smart" || L.cand) return;
    const r = selRegion();
    if (!r) return;
    const w = S.info.width, dx = L.selDrag ? L.selDrag.dx : 0, dy = L.selDrag ? L.selDrag.dy : 0;
    const sc = S.view.scale, [ox, oy] = toScreen(dx, dy), q = r.segs;
    g.save();
    g.beginPath();
    for (let i = 0; i < q.length; i += 4) {
      g.moveTo(ox + q[i] * sc, oy + q[i + 1] * sc);
      g.lineTo(ox + q[i + 2] * sc, oy + q[i + 3] * sc);
    }
    g.lineWidth = 2.5;
    g.strokeStyle = "rgba(0, 0, 0, 0.7)";
    g.stroke();
    g.lineWidth = 1.3;
    g.strokeStyle = "#fff";
    g.stroke();
    if (dx || dy) {
      g.globalAlpha = L.opacity;
      g.fillStyle = `rgb(${colorOf(r.k).join(",")})`;
      for (const i of r.pix) g.fillRect(ox + (i % w) * sc, oy + ((i / w) | 0) * sc, sc, sc);
    }
    g.restore();
  }

  ext.tools.label = {
    down(e, p) {
      if (!ready()) return;
      if (L.sub === "wand") {
        const c = S.chans[channel()];
        const range = Math.abs((c.max ?? 1) - (c.min ?? 0)) || 1;
        const [x, y] = [Math.floor(p[0]), Math.floor(p[1])];
        if (x < 0 || y < 0 || x >= S.info.width || y >= S.info.height) return;
        edit({ kind: "wand", x, y, z: S.z, c: channel(), t: S.t, tol: L.tol * range, label: L.active, guard: L.guard ? L.active : 0, threeD: L.threeD });
        return;
      }
      if (L.sub === "lasso") {
        L.lasso = { z: S.z, pts: [p], erase: e.altKey };
        draw();
        return;
      }
      if (L.sub === "classify") return clsDown(e, p);
      if (!L.slice || L.z !== S.z) return;
      if (L.sub === "smart") return smartDown(e, p);
      L.stroke = { z: S.z, pts: [p] };
      stampLocal(p[0], p[1]);
      draw();
    },
    move(e, p) {
      L.hover = p;
      if (L.selDrag) {
        L.selDrag.dx = Math.round(p[0] - L.selDrag.a[0]);
        L.selDrag.dy = Math.round(p[1] - L.selDrag.a[1]);
      }
      if (L.stroke) strokeTo(p);
      if (L.cls.stroke) clsStrokeTo(p);
      if (L.pick.path) {
        const a = L.pick.path.pts[L.pick.path.pts.length - 1];
        if (Math.hypot(p[0] - a[0], p[1] - a[1]) * S.view.scale >= 2) L.pick.path.pts.push(p);
      }
      if (L.lasso) {
        const a = L.lasso.pts[L.lasso.pts.length - 1];
        if (Math.hypot(p[0] - a[0], p[1] - a[1]) * S.view.scale >= 2) L.lasso.pts.push(p);
      }
      draw();
    },
    up() {
      if (L.cls.stroke) return clsUp();
      if (L.selDrag) return selectUp();
      if (L.pick.path) return smartUp();
      const la = L.lasso;
      L.lasso = null;
      if (la) {
        if (la.pts.length < 3) return draw();
        const pts = la.pts.map(([x, y]) => [+x.toFixed(2), +y.toFixed(2)]);
        const label = la.erase ? 0 : L.active;
        edit({ kind: "polygon", z: la.z, pts, label, guard: L.guard ? L.active : 0 });
        return;
      }
      const st = L.stroke;
      L.stroke = null;
      if (!st) return;
      const label = L.sub === "eraser" ? 0 : L.active;
      const guard = L.guard ? L.active : 0;
      const pts = st.pts.map(([x, y]) => [+x.toFixed(2), +y.toFixed(2)]);
      edit({ kind: "stroke", z: st.z, pts, radius: L.radius, label, guard });
    },
    // Steps recorded with ext.pushUndo; false while busy, so the app keeps the step.
    undo() {
      if (L.busy || L.sam.busy || L.stroke || L.lasso || L.pick.path || L.selDrag || L.cls.stroke) return false;
      edit({ kind: "undo" });
    },
    redo() {
      if (L.busy || L.sam.busy || L.stroke || L.lasso || L.pick.path || L.selDrag || L.cls.stroke) return false;
      edit({ kind: "redo" });
    },
    // Right-click undoes, as the label tool has no other use for it.
    context() {
      if (L.stroke || L.lasso || L.pick.path || L.selDrag || L.cls.stroke || L.cand) return;
      undo();
    },
    active(on) {
      btn.classList.toggle("on", on);
      if (!on) {
        L.hover = null;
        hoverDrop();
        candCancel();
        syncLayersBox();
        placeChips();
        return;
      }
      // The labels are listed in the Layers panel.
      samStatus();
      renderList();
      const P = window.TifMrcViewPro, merge = P.documents && P.documents.current() && P.documents.current().type === "merge";
      if (P.layers && S.info && !merge && (document.body.dataset.sidebarPane !== "layers" || $("sidebar").classList.contains("collapsed"))) P.layers.setPanel("layers");
      if ($("tools").hidden || $("tools").classList.contains("only-contrast") || $("tools").classList.contains("only-info")) showPanel("all");
      sec.scrollIntoView({ block: "nearest" });
      draw();
    },
  };

  // Brush outline follows the pointer even when no button is down.
  $("view").addEventListener("pointermove", (e) => {
    if (S.tool !== "label" || S.op) return;
    const r = $("view").getBoundingClientRect();
    L.hover = [
      S.view.cx + (e.clientX - r.left - r.width / 2) / S.view.scale,
      S.view.cy + (e.clientY - r.top - r.height / 2) / S.view.scale,
    ];
    hoverSoon();
    draw();
  });
  $("view").addEventListener("pointerleave", () => {
    if (!L.hover) return;
    L.hover = null;
    hoverDrop();
    draw();
  });

  // [ and ] change the brush size.
  window.addEventListener("keydown", (e) => {
    if (S.tool !== "label" || e.ctrlKey || e.metaKey) return;
    const tag = e.target.tagName;
    if (tag === "INPUT" && e.target.type !== "checkbox" && e.target.type !== "range") return;
    if (e.key !== "[" && e.key !== "]") return;
    L.radius = Math.max(1, Math.min(100, Math.round(L.radius * (e.key === "]" ? 1.25 : 0.8)) + (e.key === "]" ? 1 : 0)));
    syncPanel();
    draw();
    e.preventDefault();
  });

  // ---------- smart selection (sam_pick.rs, segpick.rs) ----------
  //
  // One tool for "this object": a click, a loop drawn around it or a line
  // drawn along it. Rust answers with the blob under the pointer or SAM's
  // mask, whichever is surer, and the result goes straight into the active
  // label; it stays selected, with its actions on a small bar beside it.

  const samDenoise = () => (L.sam.denoise ? { slab: 2, sigma: 1.5 } : { slab: 0, sigma: 0 });

  async function samStatus() {
    try {
      L.sam.status = await invoke("sam_status");
    } catch (_) {
      L.sam.status = { available: false };
    }
    syncSam();
    // Load the model before the first click needs it.
    const st = L.sam.status;
    if (st.available && st.downloaded && !st.loaded && !L.sam.warmed && S.tool === "label") {
      L.sam.warmed = true;
      invoke("sam_warm").catch(() => {}).finally(samStatus);
    }
  }

  function syncSam() {
    const box = $("lab-sam");
    const st = L.sam.status;
    if (!st) { box.hidden = true; samStatus(); return; }
    const need = !!st.available && !st.downloaded;
    box.hidden = L.sub !== "smart" || !(need || L.sam.progress || L.sam.along);
    $("lab-sam-get").hidden = !need || !!L.sam.progress;
    let text = "";
    if (L.sam.progress) text = `正在下载模型 ${Math.round((L.sam.progress.done / L.sam.progress.total) * 100)}%`;
    else if (L.sam.along) text = L.sam.along.total ? `正在沿 Z 传播 ${L.sam.along.done}/${L.sam.along.total} 层（Esc 停止）` : "正在沿 Z 传播…（Esc 停止）";
    else if (need) text = "斑点和颗粒现在就能点选。下载 SAM 模型后，形状复杂的物体也能一点就选出来。";
    $("lab-sam-text").textContent = text;
  }

  async function samDownload() {
    L.sam.progress = { done: 0, total: L.sam.status.size };
    syncSam();
    const un = await T.event.listen("sam-download", (ev) => { L.sam.progress = ev.payload; syncSam(); });
    try {
      await invoke("sam_download");
    } catch (e) {
      toast(String(e), 6000);
    } finally {
      un();
      L.sam.progress = null;
      await samStatus();
    }
  }
  $("lab-sam-get").addEventListener("click", samDownload);

  /** The part of the image on screen: x0, y0, x1, y1 in image pixels. */
  function viewRect() {
    const r = $("view").getBoundingClientRect(), sc = S.view.scale;
    return [S.view.cx - r.width / 2 / sc, S.view.cy - r.height / 2 / sc, S.view.cx + r.width / 2 / sc, S.view.cy + r.height / 2 / sc];
  }

  function pickReq(path, preview, choice = null, view = viewRect()) {
    const c = S.chans[channel()];
    // A long stroke says no more with every point.
    const step = Math.ceil(path.length / 400);
    const pts = path.filter((_, i) => i % step === 0 || i === path.length - 1).map(([x, y]) => [+x.toFixed(2), +y.toFixed(2)]);
    return { token: S.token, z: S.z, c: channel(), t: S.t, lo: c.min ?? 0, hi: c.max ?? 1, denoise: samDenoise(), path: pts, view, choice, preview };
  }

  function parsePick(buf) {
    const dv = new DataView(buf);
    const u = (i) => dv.getUint32(i * 4, true);
    return { x: u(0), y: u(1), w: u(2), h: u(3), choices: u(4), choice: u(5), quality: dv.getFloat32(24, true), seed: [u(7), u(8)], waiting: !!u(9), mask: new Uint8Array(buf, 40, u(2) * u(3)) };
  }

  /** A mask as a small canvas: a solid outline and a faint fill. */
  function maskCanvas(w, h, m, rgb, fill) {
    const cv = document.createElement("canvas");
    cv.width = w;
    cv.height = h;
    const cx = cv.getContext("2d"), img = cx.createImageData(w, h), d = img.data;
    for (let i = 0; i < w * h; i++) {
      if (!m[i]) continue;
      const x = i % w, y = (i / w) | 0, o = i * 4;
      const edge = x === 0 || y === 0 || x === w - 1 || y === h - 1 || !m[i - 1] || !m[i + 1] || !m[i - w] || !m[i + w];
      d[o] = rgb[0]; d[o + 1] = rgb[1]; d[o + 2] = rgb[2]; d[o + 3] = edge ? 255 : fill;
    }
    cx.putImageData(img, 0, 0);
    return cv;
  }
  const inMask = (m, x, y) => x >= m.x && y >= m.y && x < m.x + m.w && y < m.y + m.h && !!m.mask[(y - m.y) * m.w + x - m.x];

  // ----- preview under the pointer -----

  function hoverDrop() {
    clearTimeout(L.pick.timer);
    if (!L.pick.hover) return;
    L.pick.hover = null;
    draw();
  }

  /** Asks for the object under the pointer once it has rested a moment. */
  function hoverSoon() {
    clearTimeout(L.pick.timer);
    const idle = S.tool === "label" && L.sub === "smart" && S.axis === "xy" && L.token === S.token && !L.error
      && !L.cand && !L.pick.path && !L.selDrag && !L.busy && !L.sam.busy && L.hover && L.slice && L.z === S.z;
    if (!idle) return hoverDrop();
    const x = Math.floor(L.hover[0]), y = Math.floor(L.hover[1]), w = S.info.width;
    if (x < 0 || y < 0 || x >= w || y >= S.info.height) return hoverDrop();
    // Over a labelled object a click selects it; nothing to preview.
    const k = L.slice[y * w + x];
    if (k && !hidden(k)) return hoverDrop();
    const hv = L.pick.hover;
    if (hv && hv.z === S.z && inMask(hv, x, y)) return;
    L.pick.timer = setTimeout(hoverRun, 60);
  }

  async function hoverRun() {
    if (L.pick.running) { L.pick.again = true; return; }
    if (!L.hover || S.tool !== "label" || L.sub !== "smart") return;
    L.pick.running = true;
    const z = S.z, token = S.token;
    let waiting = false;
    try {
      const p = parsePick(await invoke("sam_pick", { req: pickReq([L.hover], true) }));
      waiting = p.waiting;
      if (token === S.token && z === S.z && S.tool === "label" && L.sub === "smart" && !L.sam.busy && !L.cand) {
        L.pick.hover = { ...p, z, canvas: maskCanvas(p.w, p.h, p.mask, colorOf(L.active), 70) };
      }
    } catch (_) {
      // Nothing recognisable here.
      L.pick.hover = null;
    } finally {
      L.pick.running = false;
    }
    draw();
    if (L.pick.again) { L.pick.again = false; hoverSoon(); }
    // The model was still loading: look again when it may be ready.
    else if (waiting) L.pick.timer = setTimeout(() => { if (L.pick.hover) L.pick.hover.x = -1e9; hoverSoon(); }, 1500);
  }

  // ----- click and draw -----

  function smartDown(e, p) {
    hoverDrop();
    if (L.cand) return candClick(p);
    if (L.sam.busy) return;
    chipsMenu(false);
    const x = Math.floor(p[0]), y = Math.floor(p[1]);
    const r = selRegion();
    // Pressing on the selected object moves it; anything else draws.
    if (r && !e.altKey && r.set.has(y * S.info.width + x)) {
      L.selDrag = { a: p, dx: 0, dy: 0 };
      return;
    }
    L.pick.path = { pts: [p], minus: e.altKey, z: S.z };
  }

  function smartUp() {
    const pa = L.pick.path;
    L.pick.path = null;
    if (!pa || pa.z !== S.z) return draw();
    let len = 0;
    for (let i = 1; i < pa.pts.length; i++) len += Math.hypot(pa.pts[i][0] - pa.pts[i - 1][0], pa.pts[i][1] - pa.pts[i - 1][1]);
    if (len * S.view.scale >= 6) return smartPick(pa.pts, pa.minus);
    // A click: on a labelled object it selects that, elsewhere it segments.
    const x = Math.floor(pa.pts[0][0]), y = Math.floor(pa.pts[0][1]), w = S.info.width;
    if (x < 0 || y < 0 || x >= w || y >= S.info.height) { L.sel = null; return draw(); }
    const k = L.slice[y * w + x];
    if (k && !hidden(k) && !pa.minus) {
      if (locked(k)) {
        L.sel = null;
        toast("这个标签锁定了");
      } else {
        L.sel = { z: S.z, x, y };
        L.active = k;
        renderList();
      }
      return draw();
    }
    smartPick([pa.pts[0]], pa.minus);
  }

  /** Segments what `path` means and writes it into the active label
   *  (`minus`: takes it away from it). With `choice`, the last result is
   *  replaced by another reading of the same stroke. */
  async function smartPick(path, minus, choice = null, view = viewRect()) {
    if (L.busy || L.sam.busy || !L.token) return;
    const req = pickReq(path, false, choice, view);
    if (locked(L.active)) return toast("这个标签锁定了");
    L.sam.busy = true;
    L.sel = minus ? L.sel : null;
    $("view").classList.add("lab-wait");
    try {
      const p = parsePick(await invoke("sam_pick", { req }));
      const args = { token: L.token, z: req.z, label: minus ? 0 : L.active, guard: minus || L.guard ? L.active : 0, trim: false, replace: choice !== null };
      const st = await editWith("sam_apply", args, choice !== null);
      if (!st) return;
      if (!minus) L.sel = { z: req.z, x: p.seed[0], y: p.seed[1] };
      L.pick.last = { path, minus, view, choices: p.choices, choice: p.choice, sel: L.sel };
    } catch (e) {
      toast(String(e), 5000);
    } finally {
      L.sam.busy = false;
      $("view").classList.remove("lab-wait");
      if (L.sam.status && !L.sam.status.loaded) samStatus();
      syncPanel();
      draw();
    }
  }

  function smartStep(d) {
    const last = L.pick.last;
    if (!last || L.busy || L.sam.busy) return;
    const choice = Math.max(0, Math.min(last.choices - 1, last.choice + d));
    if (choice !== last.choice) smartPick(last.path, last.minus, choice, last.view);
  }

  // ----- the bar beside the selected object -----

  const chips = document.createElement("div");
  chips.id = "lab-chips";
  chips.hidden = true;
  chips.innerHTML = `
    <button data-act="less" title="分小一点：换成更小的那种分法">－</button>
    <button data-act="more" title="分大一点：换成更大的那种分法">＋</button>
    <button data-act="similar" class="primary" title="在这一层找出所有和它一样的物体，挑好后一起加入这个标签">找同类</button>
    <button data-act="z" title="从这一层开始，逐层向上和向下自动分割同一个物体，直到找不到它为止（Esc 停止）">沿 Z</button>
    <button data-act="move" title="把这个对象换到另一个标签">移到…</button>
    <button data-act="del" title="删除这个对象（Delete）">删除</button>
    <div id="lab-chips-menu" hidden></div>`;
  $("stage").append(chips);
  for (const type of ["pointerdown", "mousedown", "dblclick", "contextmenu", "wheel"]) chips.addEventListener(type, (e) => e.stopPropagation());

  function chipsMenu(open) {
    const menu = $("lab-chips-menu");
    const r = open && selRegion();
    menu.hidden = !r;
    if (!r) return;
    menu.textContent = "";
    const add = (text, color, to) => {
      const b = document.createElement("button");
      b.innerHTML = '<i></i><span></span>';
      b.querySelector("i").style.background = color;
      b.querySelector("span").textContent = text;
      b.addEventListener("click", () => moveTo(to));
      menu.append(b);
    };
    for (const k of listed()) if (k !== r.k && !locked(k)) add(nameOf(k), `rgb(${colorOf(k).join(",")})`, k);
    add("＋ 新标签", "transparent", 0);
  }

  async function moveTo(to) {
    chipsMenu(false);
    if (!L.sel || L.busy) return;
    const { x, y, z } = L.sel;
    if (!to) to = Math.max(maxUsed(), L.active, ...Object.keys(L.materials).map(Number)) + 1;
    const st = await edit({ kind: "relabelRegion", x, y, z, to, threeD: L.threeD });
    if (st) {
      L.active = to;
      renderList();
    }
  }

  chips.addEventListener("click", (e) => {
    const act = e.target.closest("button[data-act]")?.dataset.act;
    if (!act) return;
    if (act !== "move") chipsMenu(false);
    if (act === "less") smartStep(-1);
    if (act === "more") smartStep(1);
    if (act === "similar") findSimilar();
    if (act === "z") alongZ();
    if (act === "move") chipsMenu($("lab-chips-menu").hidden);
    if (act === "del") selectDelete();
  });

  /** Shows the bar above the selected object (below it at the top edge). */
  function placeChips() {
    const on = S.tool === "label" && L.sub === "smart" && S.axis === "xy" && L.sel && L.sel.z === S.z && !L.cand && !L.selDrag && !L.sam.along && !L.pick.path;
    // While the slice is fetched again after an edit the bar stays put.
    if (on && L.z !== S.z) return;
    const r = on && selRegion();
    if (!r) {
      if (!chips.hidden) { chips.hidden = true; chipsMenu(false); }
      return;
    }
    const last = L.pick.last && L.pick.last.sel === L.sel && L.pick.last.choices > 1 ? L.pick.last : null;
    const st = L.sam.status || {};
    const q = (a) => chips.querySelector(`[data-act="${a}"]`);
    q("less").hidden = q("more").hidden = !last;
    if (last) {
      q("less").disabled = last.choice <= 0 || L.sam.busy;
      q("more").disabled = last.choice >= last.choices - 1 || L.sam.busy;
    }
    q("z").hidden = !(S.info.sizeZ > 1 && st.downloaded && st.tracking !== false);
    for (const b of chips.querySelectorAll("button[data-act]")) if (b.dataset.act !== "less" && b.dataset.act !== "more") b.disabled = L.busy || L.sam.busy;
    chips.hidden = false;
    const [x0, y0] = toScreen(r.box[0], r.box[1]), [x1, y1] = toScreen(r.box[2] + 1, r.box[3] + 1);
    const stage = $("stage").getBoundingClientRect(), w = chips.offsetWidth, h = chips.offsetHeight;
    let top = y0 - h - 8;
    if (top < 4) top = Math.min(y1 + 8, stage.height - h - 4);
    chips.style.left = `${Math.max(4, Math.min(stage.width - w - 4, (x0 + x1) / 2 - w / 2))}px`;
    chips.style.top = `${Math.max(4, top)}px`;
  }

  async function alongZ() {
    const r = selRegion();
    if (!r || L.busy || L.sam.busy) return;
    const { x, y, z } = L.sel, c = S.chans[channel()];
    const req = { token: S.token, z, c: channel(), t: S.t, lo: c.min ?? 0, hi: c.max ?? 1, up: true, down: true, max: 100000, denoise: samDenoise() };
    L.sam.busy = true;
    L.sam.along = { done: 0, total: 0 };
    syncSam();
    draw();
    const un = await T.event.listen("sam-propagate", (ev) => { L.sam.along = ev.payload; syncSam(); });
    try {
      await invoke("sam_use_region", { token: L.token, x, y, z });
      const st = await editWith("sam_propagate", { req, label: r.k, guard: L.guard ? r.k : 0 });
      if (st) toast(`沿 Z 分割了 ${st.zs.filter((s) => s !== z).length} 层`, 3000);
    } catch (e) {
      toast(String(e), 6000);
    } finally {
      un();
      L.sam.busy = false;
      L.sam.along = null;
      syncPanel();
      draw();
    }
  }

  // ----- find similar: proposals to accept together -----

  const candBar = document.createElement("div");
  candBar.id = "lab-cand";
  candBar.hidden = true;
  candBar.innerHTML = `
    <span id="lab-cand-text"></span>
    <label title="往右放宽，找出更多；往左收紧，只留最像的">少 <input type="range" id="lab-cand-thr" min="0" max="1" step="0.01" /> 多</label>
    <button class="small" id="lab-cand-cancel" title="一个也不加入（Esc）">取消</button>
    <button class="small primary" id="lab-cand-ok" title="把圈出来的都加入这个标签（回车）；不想要的先点一下去掉">全部加入</button>`;
  $("stage").append(candBar);
  for (const type of ["pointerdown", "mousedown", "dblclick", "contextmenu", "wheel"]) candBar.addEventListener(type, (e) => e.stopPropagation());

  // The slider runs from only the most alike (score 0.8) to nearly all (0.02).
  const thrOf = (v) => Math.exp(Math.log(0.8) + (Math.log(0.02) - Math.log(0.8)) * v);
  const CAND_START = 0.32;
  const candOn = (c) => c.items.filter((it) => it.score >= c.thr && !it.off);

  function syncCand() {
    const c = L.cand;
    candBar.hidden = !c;
    if (!c) return;
    const n = candOn(c).length;
    $("lab-cand-text").textContent = c.busy ? (c.total > 1 ? `正在找同类 ${c.done}/${c.total}…` : "正在找同类…") : `找到 ${n} 个`;
    $("lab-cand-thr").disabled = c.busy;
    $("lab-cand-ok").disabled = c.busy || !n;
  }

  async function findSimilar() {
    const r = selRegion();
    if (!r || L.busy || L.sam.busy || L.cand) return;
    const c = S.chans[channel()];
    const req = { token: S.token, z: S.z, c: channel(), t: S.t, lo: c.min ?? 0, hi: c.max ?? 1, denoise: samDenoise() };
    const cand = { z: S.z, label: r.k, items: [], thr: thrOf(CAND_START), busy: true, done: 0, total: 0 };
    L.cand = cand;
    L.sam.busy = true;
    $("lab-cand-thr").value = CAND_START;
    syncCand();
    draw();
    const un = await T.event.listen("sam-similar", (ev) => { if (L.cand === cand && cand.busy) { Object.assign(cand, ev.payload); syncCand(); } });
    try {
      const buf = await invoke("sam_similar", { req, label: r.k, seed: [L.sel.x, L.sel.y] });
      const dv = new DataView(buf), n = dv.getUint32(0, true), rgb = colorOf(r.k);
      let o = 4;
      for (let i = 0; i < n; i++) {
        const [x, y, w, h] = [0, 4, 8, 12].map((d) => dv.getUint32(o + d, true));
        const it = { i, x, y, w, h, score: dv.getFloat32(o + 16, true), mask: new Uint8Array(buf, o + 20, w * h), off: false, cv: null };
        it.canvas = () => (it.cv ||= maskCanvas(w, h, it.mask, rgb, 90));
        cand.items.push(it);
        o += 20 + w * h;
      }
      cand.busy = false;
      if (L.cand === cand && !n) {
        L.cand = null;
        toast("这一层没有找到和它相似的物体", 4000);
      }
    } catch (e) {
      if (L.cand === cand) {
        L.cand = null;
        toast(String(e), 6000);
      }
    } finally {
      un();
      L.sam.busy = false;
      syncCand();
      syncPanel();
      draw();
    }
  }

  function candCancel() {
    if (!L.cand) return;
    if (L.cand.busy) invoke("sam_cancel");
    L.cand = null;
    syncCand();
    draw();
  }

  async function candAccept() {
    const c = L.cand;
    if (!c || c.busy) return;
    const keep = candOn(c).map((it) => it.i);
    L.cand = null;
    syncCand();
    if (!keep.length) return draw();
    const st = await editWith("sam_similar_apply", { token: L.token, z: c.z, label: c.label, guard: L.guard ? c.label : 0, keep });
    if (st) toast(`加入了 ${keep.length} 个`, 3000);
  }

  /** A click on a proposal takes it out, another puts it back. */
  function candClick(p) {
    const c = L.cand, x = Math.floor(p[0]), y = Math.floor(p[1]);
    if (!c || c.busy) return;
    for (let i = c.items.length - 1; i >= 0; i--) {
      const it = c.items[i];
      if (it.score >= c.thr && inMask(it, x, y)) {
        it.off = !it.off;
        break;
      }
    }
    syncCand();
    draw();
  }

  $("lab-cand-thr").addEventListener("input", (e) => {
    if (!L.cand) return;
    L.cand.thr = thrOf(+e.target.value);
    syncCand();
    draw();
  });
  $("lab-cand-cancel").addEventListener("click", candCancel);
  $("lab-cand-ok").addEventListener("click", candAccept);

  function drawPick(g) {
    if (S.tool !== "label" || L.sub !== "smart" || S.axis !== "xy") return;
    if (L.cand && L.cand.z !== S.z) candCancel();
    const sc = S.view.scale;
    const blit = (cv, x, y, alpha) => {
      const [sx, sy] = toScreen(x, y);
      g.globalAlpha = alpha;
      g.drawImage(cv, sx, sy, cv.width * sc, cv.height * sc);
    };
    g.save();
    g.imageSmoothingEnabled = false;
    const c = L.cand;
    if (c) {
      for (const it of c.items) if (it.score >= c.thr) blit(it.canvas(), it.x, it.y, it.off ? 0.22 : 1);
    }
    const hv = L.pick.hover;
    if (!c && hv && hv.z === S.z && !L.pick.path && !L.selDrag && !L.sam.busy) blit(hv.canvas, hv.x, hv.y, 1);
    g.globalAlpha = 1;
    const pa = L.pick.path;
    if (pa && pa.z === S.z && pa.pts.length > 1) {
      g.beginPath();
      pa.pts.forEach(([x, y], i) => {
        const [sx, sy] = toScreen(x, y);
        if (i) g.lineTo(sx, sy);
        else g.moveTo(sx, sy);
      });
      g.lineJoin = "round";
      g.lineWidth = 3.5;
      g.strokeStyle = "rgba(0, 0, 0, 0.55)";
      g.stroke();
      g.lineWidth = 2;
      g.strokeStyle = pa.minus ? "#fff" : `rgb(${colorOf(L.active).join(",")})`;
      g.stroke();
    }
    g.restore();
  }

  // Esc backs out one step, Enter accepts the proposals, Delete removes
  // the selected object.
  window.addEventListener("keydown", (e) => {
    if (S.tool !== "label" || L.sub !== "smart") return;
    const t = e.target;
    if (t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.tagName === "INPUT" && t.type !== "range" && t.type !== "checkbox") return;
    const stop = () => { e.preventDefault(); e.stopPropagation(); };
    if (e.key === "Escape") {
      if (L.sam.along) { invoke("sam_cancel"); return stop(); }
      if (L.cand) { candCancel(); return stop(); }
      if (L.pick.path) { L.pick.path = null; draw(); return stop(); }
      if (!$("lab-chips-menu").hidden) { chipsMenu(false); return stop(); }
      if (L.sel) { L.sel = null; draw(); return stop(); }
      return;
    }
    if (e.key === "Enter" && L.cand && !L.cand.busy) { candAccept(); return stop(); }
    if ((e.key === "Delete" || e.key === "Backspace") && L.sel && !L.cand) { selectDelete(); return stop(); }
  }, true);


  // ---------- pixel classifier (classify.rs) ----------

  const clsCanvas = document.createElement("canvas");
  const clsCtx = clsCanvas.getContext("2d");
  const predCanvas = document.createElement("canvas");
  const predCtx = predCanvas.getContext("2d");

  const clsReq = () => {
    const c = S.chans[channel()];
    return { token: L.token, c: channel(), t: S.t, lo: c.min ?? 0, hi: c.max ?? 1 };
  };

  function syncCls() {
    const box = $("lab-cls");
    box.hidden = L.sub !== "classify";
    if (box.hidden) return;
    const C = L.cls, info = C.info;
    let text;
    if (C.along) text = `正在分类 ${C.along.done}/${C.along.total || "?"} 层（Esc 停止）`;
    else if (C.busy) text = "计算中…";
    else if (info) text = `已用 ${info.labels.length} 种标签、${info.samples} 个像素训练（自检准确率 ${Math.round(info.accuracy * 100)}%，${info.backend}，${info.seconds.toFixed(1)} 秒）。补几笔再训练可以改进。`;
    else text = "用画笔给每种结构和背景各画几笔不同的标签（换标签点下面的列表或“新标签”），然后按“训练”。";
    $("lab-cls-text").textContent = text;
    $("lab-cls-preview").checked = C.preview;
    $("lab-cls-conf").value = C.minConf;
    $("lab-cls-conf-v").textContent = `${Math.round(C.minConf * 100)}%`;
    $("lab-cls-train").disabled = C.busy || L.busy || !!L.restoringModels;
    for (const id of ["lab-cls-z", "lab-cls-all"]) $(id).disabled = !info || C.busy || L.busy;
    $("lab-cls-all").hidden = !(S.info && S.info.sizeZ > 1);
  }

  /** Colours a u16 label slice into a canvas (alpha from `alpha`, or opaque). */
  function paintLabels(cv, cx, src, alpha) {
    const w = S.info.width, h = S.info.height;
    if (cv.width !== w || cv.height !== h) { cv.width = w; cv.height = h; }
    const img = cx.createImageData(w, h), px = img.data;
    for (let i = 0; i < w * h; i++) {
      const k = src[i];
      if (!k) continue;
      const c = colorOf(k), o = i * 4;
      px[o] = c[0]; px[o + 1] = c[1]; px[o + 2] = c[2]; px[o + 3] = alpha ? alpha[i] : 255;
    }
    cx.putImageData(img, 0, 0);
  }

  async function clsFetch() {
    const C = L.cls;
    if (C.fetching || !L.token) return;
    const token = L.token, z = S.z;
    C.fetching = true;
    try {
      C.scribbles = new Uint16Array(await invoke("classify_scribbles", { token, z }));
      C.z = z;
      paintLabels(clsCanvas, clsCtx, C.scribbles);
      if (C.info && C.preview && C.predZ !== z) clsPredict();
    } catch (_) {
      // Image changed; the next draw asks again.
    } finally {
      C.fetching = false;
    }
    if (token === L.token) draw();
  }

  async function clsPredict() {
    const C = L.cls;
    if (!C.info || C.busy) return;
    const z = S.z;
    const context = JSON.stringify(modelContext());
    C.busy = true;
    syncCls();
    try {
      const buf = await invoke("classify_predict", { req: clsReq(), z });
      if (context !== JSON.stringify(modelContext())) return;
      const dv = new DataView(buf), w = dv.getUint32(0, true), h = dv.getUint32(4, true);
      const labels = new Uint16Array(buf.slice(8, 8 + w * h * 2));
      const conf = new Uint8Array(buf, 8 + w * h * 2, w * h);
      const min = Math.round(C.minConf * 255);
      const alpha = new Uint8Array(w * h);
      for (let i = 0; i < w * h; i++) alpha[i] = conf[i] >= min ? 255 : 0;
      paintLabels(predCanvas, predCtx, labels, alpha);
      C.pred = { labels, conf };
      C.predZ = z;
    } catch (e) {
      toast(String(e), 6000);
    } finally {
      C.busy = false;
      syncCls();
      draw();
    }
  }

  function drawCls(g) {
    if (S.tool !== "label" || L.sub !== "classify" || S.axis !== "xy" || L.token !== S.token) return;
    const C = L.cls;
    if (C.z !== S.z && !C.stroke) clsFetch();
    const [sx, sy] = toScreen(0, 0);
    const sw = S.info.width * S.view.scale, sh = S.info.height * S.view.scale;
    g.save();
    g.imageSmoothingEnabled = false;
    if (C.preview && C.pred && C.predZ === S.z) {
      g.globalAlpha = L.opacity;
      g.drawImage(predCanvas, sx, sy, sw, sh);
    }
    if (C.scribbles && C.z === S.z) {
      g.globalAlpha = 0.9;
      g.drawImage(clsCanvas, sx, sy, sw, sh);
    }
    g.restore();
  }

  /** Paints into the local scribble copy right away (classify.rs repeats it). */
  function clsStamp(cx, cy, label) {
    const w = S.info.width, h = S.info.height, r = Math.max(0.5, L.radius), s = L.cls.scribbles;
    const x0 = Math.max(0, Math.floor(cx - r)), y0 = Math.max(0, Math.floor(cy - r));
    const x1 = Math.min(w, Math.ceil(cx + r)), y1 = Math.min(h, Math.ceil(cy + r));
    for (let y = y0; y < y1; y++) {
      const dy = y + 0.5 - cy;
      for (let x = x0; x < x1; x++) {
        const dx = x + 0.5 - cx;
        if (dx * dx + dy * dy <= r * r) s[y * w + x] = label;
      }
    }
  }

  function clsDown(e, p) {
    const C = L.cls;
    if (!C.scribbles || C.z !== S.z) return;
    C.stroke = { z: S.z, pts: [p], label: e.altKey ? 0 : L.active };
    clsStamp(p[0], p[1], C.stroke.label);
    paintLabels(clsCanvas, clsCtx, C.scribbles);
    draw();
  }

  function clsStrokeTo(p) {
    const st = L.cls.stroke, a = st.pts[st.pts.length - 1];
    const n = Math.max(1, Math.ceil(Math.hypot(p[0] - a[0], p[1] - a[1]) / Math.max(0.5, L.radius / 2)));
    for (let i = 1; i <= n; i++) clsStamp(a[0] + ((p[0] - a[0]) * i) / n, a[1] + ((p[1] - a[1]) * i) / n, st.label);
    st.pts.push(p);
    paintLabels(clsCanvas, clsCtx, L.cls.scribbles);
  }

  async function clsUp() {
    const C = L.cls, st = C.stroke;
    C.stroke = null;
    const pts = st.pts.map(([x, y]) => [+x.toFixed(2), +y.toFixed(2)]);
    try {
      const buf = await invoke("classify_scribble", { token: L.token, z: st.z, pts, radius: L.radius, label: st.label });
      if (st.z === C.z) {
        C.scribbles = new Uint16Array(buf);
        paintLabels(clsCanvas, clsCtx, C.scribbles);
      }
    } catch (e) {
      toast(String(e), 6000);
    }
    draw();
  }

  async function clsTrain() {
    if (L.restoringModels) return;
    const C = L.cls;
    if (!ready() || C.busy) return;
    C.busy = true;
    syncCls();
    try {
      const context = JSON.stringify(modelContext());
      const info = await invoke("classify_train", { req: clsReq() });
      if (context !== JSON.stringify(modelContext())) return;
      C.info = info;
      clsRecords.show(info.persistence);
      C.predZ = -1;
    } catch (e) {
      toast(String(e), 6000);
    } finally {
      C.busy = false;
      syncCls();
    }
    if (C.info && C.preview) clsPredict();
  }

  async function clsApply(all) {
    const C = L.cls;
    if (!ready() || !C.info || C.busy) return;
    const [z0, z1] = all ? [0, S.info.sizeZ] : [S.z, S.z + 1];
    C.busy = true;
    if (all) C.along = { done: 0, total: z1 - z0 };
    syncCls();
    const un = await T.event.listen("classify-apply", (ev) => { if (C.along) { C.along = ev.payload; syncCls(); } });
    try {
      const st = await editWith("classify_apply", { req: clsReq(), z0, z1, guard: L.guard ? L.active : 0, minConf: C.minConf });
      if (st && all) toast(`分类写入了 ${st.zs.length} 层`, 3000);
    } finally {
      un();
      C.busy = false;
      C.along = null;
      syncCls();
    }
  }

  $("lab-cls-train").addEventListener("click", clsTrain);
  $("lab-cls-preview").addEventListener("change", (e) => {
    L.cls.preview = e.target.checked;
    if (L.cls.preview && L.cls.predZ !== S.z) clsPredict();
    draw();
  });
  $("lab-cls-conf").addEventListener("input", (e) => {
    L.cls.minConf = +e.target.value;
    syncCls();
    const P = L.cls.pred;
    if (P && L.cls.predZ === S.z) {
      const min = Math.round(L.cls.minConf * 255), alpha = new Uint8Array(P.conf.length);
      for (let i = 0; i < alpha.length; i++) alpha[i] = P.conf[i] >= min ? 255 : 0;
      paintLabels(predCanvas, predCtx, P.labels, alpha);
      draw();
    }
  });
  $("lab-cls-clear").addEventListener("click", async () => {
    try {
      await invoke("classify_clear", { token: L.token });
    } catch (e) { toast(String(e), 6000); return; }
    clsRecords.clear();
    Object.assign(L.cls, { z: -1, scribbles: null, info: null, pred: null, predZ: -1 });
    syncCls();
    draw();
  });
  $("lab-cls-z").addEventListener("click", () => clsApply(false));
  $("lab-cls-all").addEventListener("click", () => clsApply(true));
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && L.cls.along) { invoke("classify_cancel"); e.preventDefault(); e.stopPropagation(); }
  }, true);

  // ---------- U-Net (unet.rs) ----------

  const unetCanvas = document.createElement("canvas");
  const unetCtx = unetCanvas.getContext("2d");
  const unetOpen = () => $("lab-unet").open;

  function syncUnet() {
    const U = L.unet, info = U.info;
    let text;
    if (U.train) text = `正在训练 ${U.train.step}/${U.train.steps} 步，损失 ${U.train.loss.toFixed(3)}（Esc 停止）`;
    else if (U.along) text = `正在分割 ${U.along.done}/${U.along.total || "?"} 层（Esc 停止）`;
    else if (U.busy) text = "计算中…";
    else if (info) text = `已用 ${info.slices} 层、${info.labels.length} 种标签训练（准确率 ${Math.round(info.accuracy * 100)}%，${info.backend}，${info.seconds.toFixed(0)} 秒）。多标几层再训练可以改进。`;
    else text = "先在几层上把物体和背景都画上标签（比如用画笔、SAM 或分类），再按“训练”；训练好的网络会分割其余的层。";
    $("lab-unet-text").textContent = text;
    $("lab-unet-preview").checked = U.preview;
    $("lab-unet-conf").value = U.minConf;
    $("lab-unet-conf-v").textContent = `${Math.round(U.minConf * 100)}%`;
    $("lab-unet-train").disabled = U.busy || L.busy || !L.token || !!L.restoringModels;
    for (const id of ["lab-unet-z", "lab-unet-all"]) $(id).disabled = !info || U.busy || L.busy;
    $("lab-unet-all").hidden = !(S.info && S.info.sizeZ > 1);
  }

  function unetPaint() {
    const P = L.unet.pred, min = Math.round(L.unet.minConf * 255), alpha = new Uint8Array(P.conf.length);
    for (let i = 0; i < alpha.length; i++) alpha[i] = P.conf[i] >= min ? 255 : 0;
    paintLabels(unetCanvas, unetCtx, P.labels, alpha);
  }

  async function unetPredict() {
    const U = L.unet;
    // While training, the network so far (unet-live) segments the slice.
    if (!(U.info || U.live) || U.predicting || (U.busy && !U.live)) return;
    const z = S.z;
    const context = JSON.stringify(modelContext());
    const own = !U.busy;
    U.stale = false;
    U.predicting = true;
    U.busy = true;
    syncUnet();
    try {
      const buf = await invoke("unet_predict", { req: clsReq(), z });
      if (context !== JSON.stringify(modelContext())) return;
      const dv = new DataView(buf), w = dv.getUint32(0, true), h = dv.getUint32(4, true);
      U.pred = { labels: new Uint16Array(buf.slice(8, 8 + w * h * 2)), conf: new Uint8Array(buf, 8 + w * h * 2, w * h) };
      U.predZ = z;
      unetPaint();
    } catch (e) {
      if (!U.live) toast(String(e), 6000);
      U.predZ = z;
    } finally {
      U.predicting = false;
      if (own) U.busy = false;
      syncUnet();
      draw();
    }
  }

  function drawUnet(g) {
    const U = L.unet;
    if (S.tool !== "label" || !unetOpen() || !(U.info || U.live) || !U.preview || S.axis !== "xy" || L.token !== S.token) return;
    if (U.predZ !== S.z || U.stale) unetPredict();
    if (U.predZ !== S.z) return;
    if (!U.pred) return;
    const [sx, sy] = toScreen(0, 0);
    g.save();
    g.imageSmoothingEnabled = false;
    g.globalAlpha = L.opacity;
    g.drawImage(unetCanvas, sx, sy, S.info.width * S.view.scale, S.info.height * S.view.scale);
    g.restore();
  }

  async function unetTrain() {
    if (L.restoringModels) return;
    const U = L.unet;
    if (!ready() || U.busy) return;
    await flush();
    U.busy = true;
    U.train = { step: 0, steps: +$("lab-unet-steps").value || 400, loss: 0 };
    syncUnet();
    const un = await T.event.listen("unet-train", (ev) => { if (U.train) { U.train = ev.payload; syncUnet(); } });
    // Every few seconds the network so far: its segmentation of this slice
    // shows while it trains.
    const unLive = await T.event.listen("unet-live", () => {
      U.live = true;
      U.stale = true;
      draw();
    });
    try {
      const context = JSON.stringify(modelContext());
      const info = await invoke("unet_train", { req: clsReq(), steps: U.train.steps });
      if (context !== JSON.stringify(modelContext())) return;
      U.info = info;
      unetRecords.show(info.persistence);
      Object.assign(U, { pred: null, predZ: -1 });
    } catch (e) {
      toast(String(e), 6000);
      Object.assign(U, { pred: null, predZ: -1 });
    } finally {
      un();
      unLive();
      U.live = false;
      U.busy = false;
      U.train = null;
      syncUnet();
      draw();
    }
  }

  async function unetApply(all) {
    const U = L.unet;
    if (!ready() || !U.info || U.busy) return;
    const [z0, z1] = all ? [0, S.info.sizeZ] : [S.z, S.z + 1];
    U.busy = true;
    if (all) U.along = { done: 0, total: 0 };
    syncUnet();
    const un = await T.event.listen("unet-apply", (ev) => { if (U.along) { U.along = ev.payload; syncUnet(); } });
    try {
      const st = await editWith("unet_apply", { req: clsReq(), z0, z1, guard: L.guard ? L.active : 0, minConf: U.minConf, onlyEmpty: all });
      if (st && all) toast(`U-Net 写入了 ${st.zs.length} 层`, 3000);
      U.predZ = -1;
    } finally {
      un();
      U.busy = false;
      U.along = null;
      syncUnet();
      draw();
    }
  }

  // ---------- membranes (membrane/) ----------

  const M = { busy: false, text: "", run: null, vesicles: [], token: 0 };

  // What the running job has found so far (live.rs), drawn over the image
  // so the result builds up in the viewer, window by window.
  const LV = { on: false, token: 0, z: -1, fetching: false, again: false, last: 0, data: null };
  const liveCanvas = document.createElement("canvas");
  const liveCtx = liveCanvas.getContext("2d");
  function liveStart() {
    Object.assign(LV, { on: true, token: L.token, z: -1, data: null, last: 0 });
  }
  function liveStop() {
    Object.assign(LV, { on: false, data: null, z: -1 });
  }
  /** Fetches the slice under Z, at most about four times a second. */
  async function liveFetch() {
    if (!LV.on || S.axis !== "xy" || LV.token !== S.token) return;
    if (LV.fetching) { LV.again = true; return; }
    const wait = 250 - (performance.now() - LV.last);
    if (wait > 0 && LV.z === S.z) {
      if (!LV.timer) LV.timer = setTimeout(() => { LV.timer = 0; liveFetch(); }, wait);
      return;
    }
    LV.fetching = true;
    LV.last = performance.now();
    const z = S.z;
    try {
      const buf = await invoke("live_slice", { token: LV.token, z });
      if (!LV.on) return;
      LV.z = z;
      if (!buf || buf.byteLength < 32) {
        LV.data = null;
        return;
      }
      const dv = new DataView(buf);
      const u = (i) => dv.getUint32(i * 4, true);
      const [gw, gh, nc, busy] = [u(0), u(1), u(2), u(3)];
      const pal = new Uint8Array(buf, 32, nc * 4);
      const cells = new Uint8Array(buf, 32 + nc * 4, gw * gh);
      if (liveCanvas.width !== gw || liveCanvas.height !== gh) {
        liveCanvas.width = gw;
        liveCanvas.height = gh;
      }
      const im = liveCtx.createImageData(gw, gh), px = im.data;
      for (let i = 0; i < cells.length; i++) {
        const k = cells[i];
        if (!k || k >= nc) continue;
        const o = i * 4;
        px[o] = pal[k * 4]; px[o + 1] = pal[k * 4 + 1]; px[o + 2] = pal[k * 4 + 2]; px[o + 3] = pal[k * 4 + 3];
      }
      liveCtx.putImageData(im, 0, 0);
      LV.data = { busy: busy ? [u(4), u(5), u(6), u(7)] : null };
      draw();
    } catch (_) {
      // The job ended meanwhile.
    } finally {
      LV.fetching = false;
      if (LV.again || (LV.on && LV.z !== S.z)) { LV.again = false; liveFetch(); }
    }
  }
  function drawLive(g) {
    if (!LV.on || S.axis !== "xy" || LV.token !== S.token || !S.info) return;
    if (LV.z !== S.z) liveFetch();
    if (!LV.data || LV.z !== S.z) return;
    const [sx, sy] = toScreen(0, 0);
    const k = S.view.scale;
    g.save();
    g.globalAlpha = L.opacity;
    g.imageSmoothingEnabled = false;
    g.drawImage(liveCanvas, sx, sy, S.info.width * k, S.info.height * k);
    g.restore();
    // The window the network is reading now.
    const b = LV.data.busy;
    if (b) {
      const [x0, y0] = toScreen(b[0], b[1]), [x1, y1] = toScreen(b[2], b[3]);
      g.save();
      g.setLineDash([6, 4]);
      g.lineWidth = 1.5;
      g.strokeStyle = "rgba(0, 0, 0, 0.6)";
      g.strokeRect(x0, y0, x1 - x0, y1 - y0);
      g.lineDashOffset = 5;
      g.strokeStyle = "rgb(255, 196, 0)";
      g.strokeRect(x0, y0, x1 - x0, y1 - y0);
      g.restore();
    }
  }
  function syncMem() {
    const current = M.run?.token === S.token && L.token === S.token;
    $("lab-mem-run").disabled = M.busy || L.busy || !L.token;
    $("lab-mem-run").textContent = M.busy && current ? "分割中…" : "自动分割膜";
    $("lab-mem-text").textContent = current ? M.text : "";
    const ves = M.token === L.token ? M.vesicles : [];
    $("lab-ves-csv").hidden = !ves.length;
    $("lab-ves-hist").hidden = ves.length < 2;
    if (ves.length >= 2) drawVesicles(ves);
  }

  /** Diameter histogram of the vesicles, 5 nm bins. */
  function drawVesicles(ves) {
    const c = $("lab-ves-hist");
    const dpr = window.devicePixelRatio || 1;
    const w = c.clientWidth || 240, h = 90;
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
    const g = c.getContext("2d");
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, h);
    const d = ves.map((v) => v.diameter);
    const lo = Math.floor(Math.min(...d) / 5) * 5, hi = Math.ceil((Math.max(...d) + 0.01) / 5) * 5;
    const bins = new Array(Math.max(1, (hi - lo) / 5)).fill(0);
    for (const x of d) bins[Math.min(bins.length - 1, Math.floor((x - lo) / 5))]++;
    const top = Math.max(...bins), axis = 14, bw = w / bins.length;
    const css = getComputedStyle(document.documentElement);
    g.fillStyle = "rgb(92, 190, 60)"; // the colour of 囊泡 (membrane/mod.rs)
    bins.forEach((n, i) => {
      const bh = (n / top) * (h - axis - 4);
      g.fillRect(i * bw + 1, h - axis - bh, Math.max(1, bw - 2), bh);
    });
    g.fillStyle = css.getPropertyValue("--dim").trim() || "#888";
    g.font = "10px sans-serif";
    g.textBaseline = "bottom";
    g.textAlign = "left";
    g.fillText(`${lo}`, 0, h);
    g.textAlign = "right";
    g.fillText(`${hi} nm`, w, h);
  }

  async function exportVesicles() {
    const ves = M.token === L.token ? M.vesicles : [];
    if (!ves.length) return;
    const stem = S.info.path.split(/[\\/]/).pop().replace(/\.[^.]+$/, "");
    const path = await invoke("pick_save_path", { dir: saveDir(), name: `${stem}_vesicles.csv`, filter: "CSV", exts: ["csv"] }).catch(() => null);
    if (!path) return;
    setSaveDir(dirname(path));
    const rows = ["x_px,y_px,z_px,diameter_nm,voxels", ...ves.map((v) => `${v.center.map((c) => c.toFixed(1)).join(",")},${v.diameter.toFixed(2)},${v.voxels}`)];
    try {
      await writeFile(path, new TextEncoder().encode(rows.join("\n") + "\n"));
      toast(`已保存 ${path}`);
    } catch (e) {
      toast(`保存失败：${e}`, 6000);
    }
  }
  async function memRun() {
    if (!ready() || M.busy) return;
    if (!(S.info && S.info.sizeZ >= 8)) return toast("膜分割需要三维断层");
    const run = { token: L.token };
    M.run = run;
    const current = () => M.run === run && run.token === S.token && run.token === L.token;
    M.busy = true;
    M.text = "";
    syncMem();
    const STAGES = { read: "读取断层", net: "网络推理", tubes: "找微管" };
    let un = () => {};
    try {
      un = await T.event.listen("membrane-progress", (ev) => {
        if (!M.busy || !current()) return;
        const p = ev.payload;
        M.text = `${STAGES[p.stage] || p.stage} ${Math.round((100 * p.done) / Math.max(1, p.total))}%`;
        syncMem();
        if (p.stage !== "read") liveFetch();
      });
      if (!current() || !(await window.TifMrcViewPro.membrane.ensure()) || !current()) return;
      liveStart();
      const r = await editWith("membrane_segment", { token: run.token, c: channel(), t: S.t || 0, guard: L.guard ? L.active : 0, op: { kind: "membrane" } });
      if (!current()) return;
      // A failure stays on the panel: a toast alone is gone by the time a
      // long segmentation is looked at again.
      M.text = r ? `找到 ${r.components} 片膜 · ${r.backend} · ${r.seconds.toFixed(0)} s` : L.lastError ? `分割没有完成：${L.lastError}` : "";
      if (r) {
        M.vesicles = r.vesicles || [];
        M.token = run.token;
        const d = M.vesicles.map((v) => v.diameter);
        if (d.length) {
          const mean = d.reduce((a, b) => a + b, 0) / d.length;
          const sd = Math.sqrt(d.reduce((a, b) => a + (b - mean) ** 2, 0) / d.length);
          M.text += `，其中 ${d.length} 个囊泡，直径 ${mean.toFixed(1)} ± ${sd.toFixed(1)} nm`;
        }
        const mt = r.microtubules || [];
        if (mt.length) M.text += `；${mt.length} 根微管，共 ${(mt.reduce((a, b) => a + b, 0) / 1000).toFixed(2)} µm`;
      }
      if (r) {
        L.active = r.label;
        renderList();
      }
    } catch (e) {
      if (!current()) return;
      M.text = `分割没有完成：${e}`;
      toast(String(e), 8000);
    } finally {
      un();
      liveStop();
      M.busy = false;
      syncMem();
      draw();
    }
  }
  $("lab-mem-run").addEventListener("click", memRun);
  $("lab-ves-csv").addEventListener("click", exportVesicles);
  $("lab-mem").addEventListener("toggle", syncMem);
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && M.busy) { invoke("membrane_cancel"); e.preventDefault(); e.stopPropagation(); }
  }, true);

  $("lab-unet").addEventListener("toggle", () => { syncUnet(); draw(); });
  $("lab-unet-train").addEventListener("click", unetTrain);
  $("lab-unet-preview").addEventListener("change", (e) => { L.unet.preview = e.target.checked; draw(); });
  $("lab-unet-conf").addEventListener("input", (e) => {
    L.unet.minConf = +e.target.value;
    syncUnet();
    if (L.unet.pred && L.unet.predZ === S.z) { unetPaint(); draw(); }
  });
  $("lab-unet-z").addEventListener("click", () => unetApply(false));
  $("lab-unet-all").addEventListener("click", () => unetApply(true));
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && (L.unet.train || L.unet.along)) { invoke("unet_cancel"); e.preventDefault(); e.stopPropagation(); }
  }, true);

  // ---------- panel controls ----------

  for (const b of sec.querySelectorAll("#lab-tools button")) {
    b.addEventListener("click", () => {
      L.sub = b.dataset.sub;
      if (L.sub !== "smart") { candCancel(); hoverDrop(); }
      syncPanel();
      if (S.tool !== "label") setTool("label");
      draw();
    });
  }
  $("lab-size").addEventListener("input", (e) => { L.radius = +e.target.value; syncPanel(); draw(); });
  $("lab-tol").addEventListener("input", (e) => { L.tol = +e.target.value; syncPanel(); });
  $("lab-op").addEventListener("input", (e) => { L.opacity = +e.target.value; draw(); });
  $("lab-3d").addEventListener("change", (e) => { L.threeD = e.target.checked; });
  $("lab-guard").addEventListener("change", (e) => { L.guard = e.target.checked; });
  $("lab-show").addEventListener("click", () => { L.show = !L.show; syncPanel(); draw(); });
  $("lab-interp").addEventListener("click", async () => {
    if (!ready()) return;
    if (!(L.counts[L.active] > 0)) return toast("先用画笔或套索隔几层画出当前标签的轮廓");
    const st = await edit({ kind: "interpolate", label: L.active, guard: L.guard ? L.active : 0 });
    if (st) toast(st.zs.length ? `补齐了 ${st.zs.length} 层` : "没有要补的层：当前标签画过的层之间没有空层");
  });
  $("lab-new").addEventListener("click", () => {
    L.active = Math.max(maxUsed(), L.active, ...Object.keys(L.materials).map(Number)) + 1;
    renderList();
    if (S.tool !== "label") setTool("label");
  });

  function fillRange() {
    const c = S.chans[channel()];
    if (!c) return;
    $("lab-lo").value = fmt(c.min);
    $("lab-hi").value = fmt(c.max);
  }
  $("lab-range").addEventListener("click", fillRange);
  $("lab-th").addEventListener("toggle", () => {
    if ($("lab-th").open && $("lab-lo").value === "") fillRange();
  });

  function threshold(all) {
    if (!ready()) return;
    const lo = parseFloat($("lab-lo").value), hi = parseFloat($("lab-hi").value);
    if (!Number.isFinite(lo) || !Number.isFinite(hi) || lo > hi) return toast("请填写阈值范围（最小值不大于最大值）");
    const [z0, z1] = all ? [0, S.info.sizeZ] : [S.z, S.z + 1];
    const minSize = Math.max(1, parseInt($("lab-min").value, 10) || 1);
    edit({ kind: "threshold", z0, z1, c: channel(), t: S.t, lo, hi, label: L.active, guard: L.guard ? L.active : 0, split: $("lab-split").checked, minSize });
  }
  $("lab-th-z").addEventListener("click", () => threshold(false));
  $("lab-th-all").addEventListener("click", () => threshold(true));

  function watershed(all) {
    if (!ready()) return;
    if (L.counts.filter((n, k) => k > 0 && n > 0).length < 2) {
      return toast("至少要两种标签当种子：每个物体一种，背景一种");
    }
    const [z0, z1] = all ? [0, S.info.sizeZ] : [S.z, S.z + 1];
    edit({ kind: "watershed", z0, z1, c: channel(), t: S.t });
  }
  $("lab-ws-z").addEventListener("click", () => watershed(false));
  $("lab-ws-all").addEventListener("click", () => watershed(true));

  $("lab-3d-surf").addEventListener("click", () => {
    if (window.TifMrcViewPro.volume) window.TifMrcViewPro.volume.openSurfaces();
  });

  // Save before the window goes away.
  window.addEventListener("beforeunload", () => { flush(); });

  syncPanel();
  renderList();
  $("lab-file").textContent = fileText();

  window.TifMrcViewPro.labels = {
    token: () => L.token,
    any,
    flush,
    colorTable,
    colorOf,
    nameOf,
    hidden,
    /** Gives object `object` of the 3D surfaces label `label` (0 deletes it), undoable. */
    relabelObject: (object, label) => editWith("labels_object_relabel", { token: L.token, object, label }),
    /** The label of the material called `name`, made with `color` when there is none. */
    materialLabel: (name, color) => {
      const old = Object.values(L.materials).find((m) => m.name === name);
      if (old) return old.label;
      const k = Math.max(0, maxUsedOf(L.counts), ...Object.keys(L.materials).map(Number)) + 1;
      setMaterial(k, { name, color });
      return k;
    },
    /** Labels in use, ascending. */
    used: () => L.counts.map((n, k) => (k > 0 && n > 0 ? k : 0)).filter((k) => k),
  };
})();
