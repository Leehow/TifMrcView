"use strict";
// Pro segmentation labels: one label per pixel of the stack, painted with a
// brush or filled by the magic wand, threshold and watershed tools. The label
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
    sub: "brush",     // select | brush | eraser | lasso | wand | sam | classify
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
    sel: null,        // picked region {z, x, y} (a pixel in it) for the select tool
    selDrag: null,    // {a: pointer at press, dx, dy} while it is dragged
    sam: { points: [], box: null, drag: null, z: -1, mask: null, iou: 0, busy: false, status: null, progress: null, along: null },
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

  // ---------- panel ----------

  const sec = document.createElement("section");
  sec.id = "sec-labels";
  sec.innerHTML = `
    <h3>标签分割 <span class="dim" id="lab-busy"></span></h3>
    <div class="row" id="lab-tools">
      <button class="small" data-sub="select" title="选择：点一块分割区域选中它，拖动挪位置，Delete 删除（勾上“整个物体”就连同其他层一起）"><svg viewBox="0 0 24 24"><path d="M5 3l14 8-6 1.5L10 19z"/></svg><span>选择</span></button>
      <button class="small" data-sub="brush" title="画笔：按住拖动涂上当前标签（[ ] 调大小）"><svg viewBox="0 0 24 24"><path d="M4 20c2.5 0 4-1.5 4-4a3 3 0 0 0-3-3"/><path d="M8 13l9-9 3 3-9 9"/></svg><span>画笔</span></button>
      <button class="small" data-sub="eraser" title="橡皮：擦掉标签（[ ] 调大小）"><svg viewBox="0 0 24 24"><path d="M8 20h12"/><path d="M4 15l9-9 6 6-7 7H8z"/></svg><span>橡皮</span></button>
      <button class="small" data-sub="lasso" title="套索：按住画一圈，松开后把圈里涂上当前标签（按住 Alt 画圈是擦除）"><svg viewBox="0 0 24 24"><path d="M12 4c5 0 9 2.5 9 6s-4 6-9 6c-1.5 0-3-.2-4.2-.6"/><path d="M12 4C7 4 3 6.5 3 10c0 1.6.9 3 2.3 4.1"/><circle cx="6" cy="17" r="2"/><path d="M6 19c0 1.5-1 2.5-2 3"/></svg><span>套索</span></button>
      <button class="small" data-sub="sam" title="SAM 点选：点一下物体自动分割（Alt+点击标背景，Shift+拖动拉框），回车写进当前标签"><svg viewBox="0 0 24 24"><path d="M12 3l2.2 5.3L20 9l-4.4 3.8L17 18.5 12 15.6 7 18.5l1.4-5.7L4 9l5.8-.7z"/></svg><span>SAM</span></button>
      <button class="small" data-sub="classify" title="像素分类：用画笔给每种结构（包括背景）各画几笔不同的标签，训练后自动分出整层或整个栈（按住 Alt 擦掉标注）"><svg viewBox="0 0 24 24"><path d="M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4z"/><path d="M14 17h6M17 14v6"/></svg><span>分类</span></button>
      <button class="small" data-sub="wand" title="魔棒：点一下，把和点击处亮度相近的连通区域涂上当前标签"><svg viewBox="0 0 24 24"><path d="M4 20L15 9"/><path d="M17 3v3M17 12v3M11 7h3M20 7h3M19 5l1.5-1.5M19 9l1.5 1.5"/></svg><span>魔棒</span></button>
    </div>
    <label class="slider" id="lab-size-row">大小 <input type="range" id="lab-size" min="1" max="100" step="1" /><span id="lab-size-v"></span></label>
    <label class="slider" id="lab-tol-row">容差 <input type="range" id="lab-tol" min="0" max="0.5" step="0.005" /><span id="lab-tol-v"></span></label>
    <div class="row">
      <label id="lab-3d-row" title="魔棒沿 Z 方向一起长到相邻的层"><input type="checkbox" id="lab-3d" /> <span id="lab-3d-text">三维</span></label>
      <button class="small" id="lab-sel-del" title="删除选中的区域（Delete）">删除选中</button>
      <label title="画笔和魔棒只涂没有标签的地方；橡皮只擦当前标签"><input type="checkbox" id="lab-guard" /> 不覆盖其他标签</label>
    </div>
    <div id="lab-sam" hidden>
      <div class="dim" id="lab-sam-text"></div>
      <div class="dim" id="lab-sam-backend"></div>
      <div class="row">
        <button class="small" id="lab-sam-get">下载模型</button>
        <button class="small" id="lab-sam-apply" title="把预览的分割写进当前标签（回车）">应用</button>
        <button class="small" id="lab-sam-trim" title="写入预览的分割，并清掉这一层里当前标签在分割以外的部分（先用画笔粗涂、再用 SAM 精修时用）">清除外围</button>
        <button class="small" id="lab-sam-z" title="从这一层开始，逐层向上和向下自动分割同一个物体，直到找不到它为止（Esc 停止）">沿 Z 传播</button>
        <button class="small" id="lab-sam-clear" title="清除这次的点和预览（Esc）">清除</button>
      </div>
      <label id="lab-sam-dn-row" title="先把相邻几层平均再模糊一点，然后交给 SAM；噪声大的冷冻电镜断层会分得更完整"><input type="checkbox" id="lab-sam-dn" /> 去噪后再分割</label>
    </div>
    <div id="lab-cls" hidden>
      <div class="dim" id="lab-cls-text"></div>
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
    <div id="lab-list"></div>
    <div class="row">
      <button class="small" id="lab-interp" title="隔几层画一个轮廓，再按这里，把当前标签画过的层之间的层按形状自动补齐">隔层插值</button>
    </div>
    <div class="row">
      <button class="small" id="lab-new" title="换一个新的标签号继续画">＋ 新标签</button>
      <label><input type="checkbox" id="lab-show" checked /> 显示</label>
    </div>
    <label class="slider">不透明度 <input type="range" id="lab-op" min="0.05" max="1" step="0.05" /><span id="lab-op-v"></span></label>
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
      <div class="dim">一键找出冷冻电镜断层里的所有生物膜，写进标签「膜」。像素大小从文件读取；第一次使用会下载模型（约 100 MB）。可以一步撤回。</div>
      <div class="row">
        <button class="small primary" id="lab-mem-run" title="分割整个断层里的膜（Esc 停止）">自动分割膜</button>
      </div>
      <div class="dim" id="lab-mem-text"></div>
    </details>
    <details id="lab-unet">
      <summary>深度学习 U-Net</summary>
      <div class="dim" id="lab-unet-text"></div>
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
    $("lab-size-row").hidden = L.sub === "wand" || L.sub === "lasso" || L.sub === "sam" || L.sub === "select";
    syncSam();
    syncCls();
    syncUnet();
    $("lab-tol-row").hidden = L.sub !== "wand";
    $("lab-3d-row").hidden = L.sub !== "wand" && L.sub !== "select";
    $("lab-3d-text").textContent = L.sub === "select" ? "整个物体（所有层）" : "三维";
    $("lab-3d-row").title = L.sub === "select" ? "挪动和删除时连同相邻层里连着的部分一起" : "魔棒沿 Z 方向一起长到相邻的层";
    $("lab-sel-del").hidden = L.sub !== "select";
    $("lab-sel-del").disabled = !L.sel || L.busy;
    $("lab-size").value = L.radius;
    $("lab-size-v").textContent = `${L.radius * 2}`;
    $("lab-tol").value = L.tol;
    $("lab-tol-v").textContent = `${Math.round(L.tol * 100)}%`;
    $("lab-op").value = L.opacity;
    $("lab-op-v").textContent = `${Math.round(L.opacity * 100)}%`;
    $("lab-3d").checked = L.threeD;
    $("lab-guard").checked = L.guard;
    $("lab-show").checked = L.show;
    $("lab-busy").textContent = L.busy ? "处理中…" : "";
    for (const b of sec.querySelectorAll("#lab-th button, #lab-ws button, #lab-interp")) b.disabled = L.busy || !L.token;
  }

  function renderList() {
    const list = $("lab-list");
    list.textContent = "";
    const ks = [];
    for (let k = 1; k < L.counts.length; k++) if (L.counts[k] > 0) ks.push(k);
    if (!ks.includes(L.active)) ks.push(L.active);
    ks.sort((a, b) => a - b);
    // Thousands of regions after a split: list the active one and the first few.
    const shown = ks.length > 200 ? ks.filter((k, i) => i < 200 || k === L.active) : ks;
    for (const k of shown) {
      const row = document.createElement("div");
      row.className = "lab-row" + (k === L.active ? " on" : "");
      const n = L.counts[k] || 0;
      row.className += (hidden(k) ? " off" : "") + (locked(k) ? " locked" : "");
      row.innerHTML = `<button class="sw" title="改颜色"></button><span class="name" title="双击改名"></span><span class="n dim"></span>`
        + `<button class="eye" title="显示 / 隐藏（2D 和 3D）">${hidden(k) ? EYE_OFF : EYE}</button>`
        + `<button class="lock" title="锁定：其他工具不会改动这个标签">${locked(k) ? LOCK : UNLOCK}</button>`
        + `<button class="del" title="删除这个标签">✕</button>`;
      const hex = "#" + colorOf(k).map((x) => x.toString(16).padStart(2, "0")).join("");
      row.querySelector(".sw").style.background = hex;
      row.querySelector(".name").textContent = nameOf(k);
      row.querySelector(".name").addEventListener("dblclick", () => rename(k, row.querySelector(".name")));
      row.querySelector(".n").textContent = n ? sizeText(n) : "";
      row.querySelector(".n").title = n ? sizeText(n) : "";
      row.querySelector(".del").hidden = !n;
      row.addEventListener("click", (e) => {
        const b = e.target.closest("button, label, input");
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
  }

  const EYE = '<svg viewBox="0 0 24 24"><path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>';
  const EYE_OFF = '<svg viewBox="0 0 24 24"><path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><path d="M4 4l16 16"/></svg>';
  const LOCK = '<svg viewBox="0 0 24 24"><rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/></svg>';
  const UNLOCK = '<svg viewBox="0 0 24 24"><rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 7.5-2"/></svg>';

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

  /** Changes one label's material and stores it with the label file. */
  function setMaterial(k, patch) {
    const m = { label: k, name: "", color: null, hidden: false, locked: false, ...mat(k), ...patch };
    L.materials = { ...L.materials, [k]: m };
    const list = Object.values(L.materials).filter((m) => m.name || m.color || m.hidden || m.locked);
    if (L.slice) repaint(0, 0, S.info.width, S.info.height);
    renderList();
    renderZMarks();
    if (window.TifMrcViewPro.volume) window.TifMrcViewPro.volume.materialsChanged();
    draw();
    edit({ kind: "materials", materials: list });
  }

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
    await flush();
    try {
      const st = await invoke("labels_open", { token });
      if (token !== S.token) return;
      // Noisy tomograms segment better denoised (same rule as the 3D view's invert).
      const i = S.info;
      const tomo = /mrc|rec|st\b/i.test(i.format) && i.sizeZ * 2 <= Math.max(i.width, i.height);
      Object.assign(L.sam, { points: [], box: null, mask: null, z: -1, denoise: tomo });
      $("lab-sam-dn").checked = tomo;
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
    if (ready && L.z !== S.z && (any() || S.tool === "label")) fetchSlice();
    if (ready && L.show && L.slice && L.z === S.z && (any() || L.stroke)) {
      const [sx, sy] = toScreen(0, 0);
      g.save();
      g.globalAlpha = L.opacity;
      g.imageSmoothingEnabled = false;
      g.drawImage(canvas, sx, sy, S.info.width * S.view.scale, S.info.height * S.view.scale);
      g.restore();
    }
    drawCls(g);
    drawUnet(g);
    drawSam(g);
    drawSelection(g);
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
  async function editWith(cmd, args) {
    if (!L.token || L.error) return;
    const token = L.token;
    const op = args.op || { kind: cmd };
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
      if (st.zs.length && op.kind !== "undo" && op.kind !== "redo" && op.kind !== "materials") ext.pushUndo("label");
      renderList();
      if (window.TifMrcViewPro.volume) window.TifMrcViewPro.volume.labelsChanged();
      return st;
    } catch (e) {
      toast(String(e), 6000);
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

  // ---------- select tool: pick, drag and delete a region ----------

  /** Pixels of the 4-connected region of one label containing (x, y) on the shown slice. */
  function regionAt(x, y) {
    const w = S.info.width, h = S.info.height, s = L.slice;
    if (!s || x < 0 || y < 0 || x >= w || y >= h) return null;
    const k = s[y * w + x];
    if (!k) return null;
    const seen = new Uint8Array(w * h), out = [], stack = [y * w + x];
    seen[y * w + x] = 1;
    while (stack.length) {
      const i = stack.pop();
      out.push(i);
      const px = i % w;
      for (const j of [px > 0 ? i - 1 : -1, px + 1 < w ? i + 1 : -1, i - w, i + w]) {
        if (j >= 0 && j < w * h && !seen[j] && s[j] === k) { seen[j] = 1; stack.push(j); }
      }
    }
    return { k, pix: out };
  }
  let selCache = { key: "", r: null };
  function selRegion() {
    if (!L.sel || L.sel.z !== S.z || L.z !== S.z || !L.slice) return null;
    const key = `${L.sel.x},${L.sel.y},${L.sel.z}`;
    if (selCache.key !== key || selCache.slice !== L.slice) selCache = { key, slice: L.slice, r: regionAt(L.sel.x, L.sel.y) };
    return selCache.r;
  }

  function selectDown(p) {
    const x = Math.floor(p[0]), y = Math.floor(p[1]);
    const r = regionAt(x, y);
    if (!r || locked(r.k)) {
      L.sel = null;
      if (r) toast("这个标签锁定了");
    } else {
      L.sel = { z: S.z, x, y };
      L.selDrag = { a: p, dx: 0, dy: 0 };
    }
    syncPanel();
    draw();
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

  /** Dashed outline of the picked region, moved with the drag. */
  function drawSelection(g) {
    if (S.tool !== "label" || L.sub !== "select") return;
    const r = selRegion();
    if (!r) return;
    const w = S.info.width, dx = L.selDrag ? L.selDrag.dx : 0, dy = L.selDrag ? L.selDrag.dy : 0;
    const inside = new Set(r.pix);
    const sc = S.view.scale;
    g.save();
    g.beginPath();
    for (const i of r.pix) {
      const x = i % w, y = (i / w) | 0;
      const [sx, sy] = toScreen(x + dx, y + dy);
      if (!inside.has(i - w) || y === 0) { g.moveTo(sx, sy); g.lineTo(sx + sc, sy); }
      if (!inside.has(i + w)) { g.moveTo(sx, sy + sc); g.lineTo(sx + sc, sy + sc); }
      if (x === 0 || !inside.has(i - 1)) { g.moveTo(sx, sy); g.lineTo(sx, sy + sc); }
      if (x === w - 1 || !inside.has(i + 1)) { g.moveTo(sx + sc, sy); g.lineTo(sx + sc, sy + sc); }
    }
    g.lineWidth = 2;
    g.strokeStyle = "rgba(0, 0, 0, 0.7)";
    g.stroke();
    g.setLineDash([4, 3]);
    g.lineWidth = 1.2;
    g.strokeStyle = "#fff";
    g.stroke();
    if (dx || dy) {
      g.globalAlpha = L.opacity;
      g.fillStyle = `rgb(${colorOf(r.k).join(",")})`;
      for (const i of r.pix) {
        const [sx, sy] = toScreen((i % w) + dx, ((i / w) | 0) + dy);
        g.fillRect(sx, sy, sc, sc);
      }
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
      if (L.sub === "sam") return samDown(e, p);
      if (L.sub === "classify") return clsDown(e, p);
      if (!L.slice || L.z !== S.z) return;
      if (L.sub === "select") return selectDown(p);
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
      if (L.sam.drag) {
        L.sam.drag.b = p;
      }
      if (L.lasso) {
        const a = L.lasso.pts[L.lasso.pts.length - 1];
        if (Math.hypot(p[0] - a[0], p[1] - a[1]) * S.view.scale >= 2) L.lasso.pts.push(p);
      }
      draw();
    },
    up() {
      if (L.sam.drag) return samUp();
      if (L.cls.stroke) return clsUp();
      if (L.selDrag) return selectUp();
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
      if (L.busy || L.stroke || L.lasso || L.sam.drag || L.selDrag || L.cls.stroke) return false;
      edit({ kind: "undo" });
    },
    redo() {
      if (L.busy || L.stroke || L.lasso || L.sam.drag || L.selDrag || L.cls.stroke) return false;
      edit({ kind: "redo" });
    },
    // SAM clicks not written yet come off one at a time before the label steps.
    undoLocal() {
      if (L.sub !== "sam" || L.sam.busy || !(L.sam.points.length || L.sam.box)) return false;
      if (L.sam.points.length) L.sam.points.pop();
      else L.sam.box = null;
      if (L.sam.points.length || L.sam.box) samRun();
      else samClear();
      return true;
    },
    // Right-click undoes, as the label tool has no other use for it.
    context() {
      if (L.stroke || L.lasso || L.sam.drag || L.selDrag || L.cls.stroke) return;
      undo();
    },
    active(on) {
      btn.classList.toggle("on", on);
      if (!on) {
        L.hover = null;
        return;
      }
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
    draw();
  });
  $("view").addEventListener("pointerleave", () => {
    if (!L.hover) return;
    L.hover = null;
    draw();
  });

  // Enter applies the SAM preview, Esc drops it.
  window.addEventListener("keydown", (e) => {
    if (S.tool !== "label" || L.sub !== "sam" || e.target.tagName === "INPUT" && e.target.type === "text") return;
    if (e.key === "Enter" && L.sam.mask) { samApply(false); e.preventDefault(); }
    if (e.key === "Escape" && L.sam.along) { invoke("sam_cancel"); e.preventDefault(); e.stopPropagation(); return; }
    if (e.key === "Escape" && (L.sam.points.length || L.sam.box)) { samClear(); e.preventDefault(); e.stopPropagation(); }
  }, true);

  // Delete / Backspace remove the region picked with the select tool.
  window.addEventListener("keydown", (e) => {
    if (S.tool !== "label" || L.sub !== "select" || !L.sel) return;
    if (e.target.tagName === "INPUT" && e.target.type === "text" || e.target.tagName === "TEXTAREA") return;
    if (e.key !== "Delete" && e.key !== "Backspace") return;
    selectDelete();
    e.preventDefault();
    e.stopPropagation();
  }, true);

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

  // ---------- SAM click segmentation (labels.rs + sam.rs) ----------

  const samCanvas = document.createElement("canvas");
  const samCtx = samCanvas.getContext("2d");

  async function samStatus() {
    try {
      L.sam.status = await invoke("sam_status");
    } catch (e) {
      L.sam.status = { available: false };
    }
    syncSam();
  }

  function syncSam() {
    const box = $("lab-sam");
    box.hidden = L.sub !== "sam";
    if (box.hidden) return;
    const st = L.sam.status;
    if (!st) { samStatus(); return; }
    const ready = st.downloaded;
    $("lab-sam-get").hidden = ready || !st.available;
    $("lab-sam-get").disabled = !!L.sam.progress;
    $("lab-sam-apply").hidden = !ready;
    $("lab-sam-trim").hidden = !ready;
    $("lab-sam-trim").disabled = !L.sam.mask || L.sam.busy;
    $("lab-sam-clear").hidden = !ready;
    $("lab-sam-dn-row").hidden = !ready;
    $("lab-sam-apply").disabled = !L.sam.mask || L.sam.busy;
    $("lab-sam-z").hidden = !ready || !(S.info.sizeZ > 1);
    $("lab-sam-z").disabled = !L.sam.mask || L.sam.busy;
    let text;
    if (!st.available) text = "SAM 分割只在 TifMrcView Pro 里有";
    else if (L.sam.progress) text = `正在下载模型 ${Math.round((L.sam.progress.done / L.sam.progress.total) * 100)}%`;
    else if (!ready) text = `第一次使用要先下载 SAM 2.1 模型（约 ${Math.round(st.size / 1e6)} MB，只下载一次）`;
    else if (L.sam.along) text = L.sam.along.total ? `正在沿 Z 传播 ${L.sam.along.done}/${L.sam.along.total} 层（Esc 停止）` : "正在沿 Z 传播…（Esc 停止）";
    else if (L.sam.busy) text = st.loaded ? "SAM 计算中…" : "正在加载模型和计算图像特征…";
    else if (L.sam.mask) text = `预测质量 ${Math.round(L.sam.iou * 100)}%：回车写进当前标签，Alt+点击去掉多余部分`;
    else text = "点一下要分割的物体；Alt+点击标背景，Shift+拖动拉框";
    $("lab-sam-text").textContent = text;
    $("lab-sam-backend").textContent = st.backend && ready ? `后端：${st.backend}` : "";
  }

  function samClear() {
    Object.assign(L.sam, { points: [], box: null, drag: null, mask: null, iou: 0 });
    syncSam();
    draw();
  }

  function samDown(e, p) {
    const st = L.sam.status;
    if (!st || !st.downloaded) return toast("请先在标签面板里下载 SAM 模型");
    if (L.sam.busy) return;
    if (L.sam.z !== S.z) { samClear(); L.sam.z = S.z; }
    if (e.shiftKey) {
      L.sam.drag = { a: p, b: p };
      return;
    }
    L.sam.points.push({ x: p[0], y: p[1], positive: !e.altKey });
    samRun();
  }

  function samUp() {
    const d = L.sam.drag;
    L.sam.drag = null;
    if (Math.abs(d.b[0] - d.a[0]) < 2 || Math.abs(d.b[1] - d.a[1]) < 2) return draw();
    L.sam.box = [Math.min(d.a[0], d.b[0]), Math.min(d.a[1], d.b[1]), Math.max(d.a[0], d.b[0]), Math.max(d.a[1], d.b[1])];
    samRun();
  }

  async function samRun() {
    const c = S.chans[channel()];
    const req = {
      token: S.token, z: S.z, c: channel(), t: S.t,
      lo: c.min ?? 0, hi: c.max ?? 1,
      points: L.sam.points, bbox: L.sam.box, denoise: samDenoise(),
    };
    L.sam.busy = true;
    syncSam();
    draw();
    try {
      const buf = await invoke("sam_predict", { req });
      if (req.token !== S.token || req.z !== S.z) return;
      const dv = new DataView(buf);
      const w = dv.getUint32(0, true), h = dv.getUint32(4, true);
      L.sam.iou = dv.getFloat32(8, true);
      L.sam.mask = new Uint8Array(buf, 12, w * h);
      samPaint(w, h);
    } catch (e) {
      toast(String(e), 6000);
      L.sam.points.pop();
    } finally {
      L.sam.busy = false;
      if (!L.sam.status.loaded) samStatus();
      syncSam();
      draw();
    }
  }

  function samPaint(w, h) {
    samCanvas.width = w;
    samCanvas.height = h;
    const img = samCtx.createImageData(w, h);
    const [r, g, b] = colorOf(L.active);
    const m = L.sam.mask;
    for (let i = 0; i < m.length; i++) {
      if (!m[i]) continue;
      const x = i % w, y = (i / w) | 0;
      // Edge pixels opaque, inside translucent.
      const edge = x === 0 || y === 0 || x === w - 1 || y === h - 1 || !m[i - 1] || !m[i + 1] || !m[i - w] || !m[i + w];
      const o = i * 4;
      img.data[o] = edge ? 255 : r; img.data[o + 1] = edge ? 255 : g; img.data[o + 2] = edge ? 255 : b;
      img.data[o + 3] = edge ? 255 : 150;
    }
    samCtx.putImageData(img, 0, 0);
  }

  function drawSam(g) {
    if (S.tool !== "label" || L.sub !== "sam" || L.sam.z !== S.z && !L.sam.drag) return;
    const [sx, sy] = toScreen(0, 0);
    if (L.sam.mask && samCanvas.width === S.info.width) {
      g.save();
      g.imageSmoothingEnabled = false;
      g.drawImage(samCanvas, sx, sy, S.info.width * S.view.scale, S.info.height * S.view.scale);
      g.restore();
    }
    const box = L.sam.drag ? [L.sam.drag.a[0], L.sam.drag.a[1], L.sam.drag.b[0], L.sam.drag.b[1]] : L.sam.box;
    if (box) {
      const [ax, ay] = toScreen(box[0], box[1]), [bx, by] = toScreen(box[2], box[3]);
      g.save();
      g.setLineDash([5, 4]);
      g.strokeStyle = "#fff";
      g.lineWidth = 1.5;
      g.strokeRect(ax, ay, bx - ax, by - ay);
      g.restore();
    }
    for (const pt of L.sam.points) {
      const [cx, cy] = toScreen(pt.x, pt.y);
      g.save();
      g.beginPath();
      g.arc(cx, cy, 5, 0, Math.PI * 2);
      g.fillStyle = pt.positive ? "#2da44e" : "#cf222e";
      g.fill();
      g.lineWidth = 1.5;
      g.strokeStyle = "#fff";
      g.stroke();
      g.restore();
    }
  }

  async function samApply(trim = false) {
    if (!L.sam.mask || L.sam.busy) return;
    // A mask over most of the slice is nearly always background picked up
    // by a stray point; it is written only on a second Enter.
    let n = 0;
    for (const v of L.sam.mask) n += v ? 1 : 0;
    if (n > 0.6 * L.sam.mask.length && L.sam.bigOk !== L.sam.mask) {
      L.sam.bigOk = L.sam.mask;
      toast("这个分割几乎盖满了整层，多半选到了背景；确定要写入就再按一次回车", 6000);
      return;
    }
    const z = L.sam.z;
    const st = await editWith("sam_apply", { token: L.token, z, label: L.active, guard: L.guard ? L.active : 0, trim });
    if (st) samClear();
  }

  const samDenoise = () => (L.sam.denoise ? { slab: 2, sigma: 1.5 } : { slab: 0, sigma: 0 });

  async function samAlong() {
    if (!L.sam.mask || L.sam.busy) return;
    const c = S.chans[channel()];
    const req = { token: S.token, z: L.sam.z, c: channel(), t: S.t, lo: c.min ?? 0, hi: c.max ?? 1, up: true, down: true, max: 100000, denoise: samDenoise() };
    L.sam.busy = true;
    L.sam.along = { done: 0, total: 0 };
    syncSam();
    const un = await T.event.listen("sam-propagate", (ev) => { L.sam.along = ev.payload; syncSam(); });
    try {
      const st = await editWith("sam_propagate", { req, label: L.active, guard: L.guard ? L.active : 0 });
      if (st) {
        samClear();
        toast(`沿 Z 分割了 ${st.zs.filter((z) => z !== req.z).length} 层`, 3000);
      }
    } finally {
      un();
      L.sam.busy = false;
      L.sam.along = null;
      syncSam();
    }
  }

  async function samDownload() {
    L.sam.progress = { done: 0, total: L.sam.status.size };
    syncSam();
    const un = await T.event.listen("sam-download", (ev) => { L.sam.progress = ev.payload; syncSam(); });
    try {
      await invoke("sam_download");
    } catch (e) {
      toast(String(e), 8000);
    } finally {
      un();
      L.sam.progress = null;
      await samStatus();
    }
  }

  $("lab-sam-get").addEventListener("click", samDownload);
  $("lab-sam-apply").addEventListener("click", () => samApply(false));
  $("lab-sam-trim").addEventListener("click", () => samApply(true));
  $("lab-sel-del").addEventListener("click", selectDelete);
  $("lab-sam-z").addEventListener("click", samAlong);
  $("lab-sam-dn").addEventListener("change", (e) => {
    L.sam.denoise = e.target.checked;
    // Redo the current prompt on the new image.
    if (L.sam.z === S.z && (L.sam.points.length || L.sam.box) && !L.sam.busy) samRun();
  });
  $("lab-sam-clear").addEventListener("click", samClear);


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
    $("lab-cls-train").disabled = C.busy || L.busy;
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
    C.busy = true;
    syncCls();
    try {
      const buf = await invoke("classify_predict", { req: clsReq(), z });
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
    const C = L.cls;
    if (!ready() || C.busy) return;
    C.busy = true;
    syncCls();
    try {
      C.info = await invoke("classify_train", { req: clsReq() });
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
    } catch (_) {}
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
    $("lab-unet-train").disabled = U.busy || L.busy || !L.token;
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
    if (!U.info || U.busy) return;
    const z = S.z;
    U.busy = true;
    syncUnet();
    try {
      const buf = await invoke("unet_predict", { req: clsReq(), z });
      const dv = new DataView(buf), w = dv.getUint32(0, true), h = dv.getUint32(4, true);
      U.pred = { labels: new Uint16Array(buf.slice(8, 8 + w * h * 2)), conf: new Uint8Array(buf, 8 + w * h * 2, w * h) };
      U.predZ = z;
      unetPaint();
    } catch (e) {
      toast(String(e), 6000);
      U.predZ = z;
    } finally {
      U.busy = false;
      syncUnet();
      draw();
    }
  }

  function drawUnet(g) {
    const U = L.unet;
    if (S.tool !== "label" || !unetOpen() || !U.info || !U.preview || S.axis !== "xy" || L.token !== S.token) return;
    if (U.predZ !== S.z) {
      if (!U.busy) unetPredict();
      return;
    }
    if (!U.pred) return;
    const [sx, sy] = toScreen(0, 0);
    g.save();
    g.imageSmoothingEnabled = false;
    g.globalAlpha = L.opacity;
    g.drawImage(unetCanvas, sx, sy, S.info.width * S.view.scale, S.info.height * S.view.scale);
    g.restore();
  }

  async function unetTrain() {
    const U = L.unet;
    if (!ready() || U.busy) return;
    await flush();
    U.busy = true;
    U.train = { step: 0, steps: +$("lab-unet-steps").value || 400, loss: 0 };
    syncUnet();
    const un = await T.event.listen("unet-train", (ev) => { if (U.train) { U.train = ev.payload; syncUnet(); } });
    try {
      U.info = await invoke("unet_train", { req: clsReq(), steps: U.train.steps });
      Object.assign(U, { pred: null, predZ: -1 });
    } catch (e) {
      toast(String(e), 6000);
    } finally {
      un();
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

  const M = { busy: false, text: "" };
  function syncMem() {
    $("lab-mem-run").disabled = M.busy || L.busy || !L.token;
    $("lab-mem-run").textContent = M.busy ? "分割中…" : "自动分割膜";
    $("lab-mem-text").textContent = M.text;
  }
  async function memRun() {
    if (!ready() || M.busy) return;
    if (!(S.info && S.info.sizeZ >= 8)) return toast("膜分割需要三维断层");
    M.busy = true;
    M.text = "";
    syncMem();
    const unDl = await T.event.listen("membrane-download", (ev) => {
      const p = ev.payload;
      M.text = `下载模型 ${Math.round((100 * p.done) / Math.max(1, p.total))}%`;
      syncMem();
    });
    const STAGES = { read: "读取断层", net: "网络推理" };
    const un = await T.event.listen("membrane-progress", (ev) => {
      const p = ev.payload;
      M.text = `${STAGES[p.stage] || p.stage} ${Math.round((100 * p.done) / Math.max(1, p.total))}%`;
      syncMem();
    });
    try {
      const st = await invoke("membrane_status");
      if (!st.downloaded) await invoke("membrane_download");
      const r = await editWith("membrane_segment", { token: L.token, c: channel(), t: S.t || 0, guard: L.guard ? L.active : 0, op: { kind: "membrane" } });
      M.text = r ? `找到 ${r.components} 片膜 · ${r.backend} · ${r.seconds.toFixed(0)} s` : "";
      if (r) {
        L.active = r.label;
        renderList();
      }
    } catch (e) {
      M.text = "";
      toast(String(e), 8000);
    } finally {
      un();
      unDl();
      M.busy = false;
      syncMem();
      draw();
    }
  }
  $("lab-mem-run").addEventListener("click", memRun);
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
      syncPanel();
      if (S.tool !== "label") setTool("label");
      draw();
    });
  }
  $("lab-size").addEventListener("input", (e) => { L.radius = +e.target.value; syncPanel(); draw(); });
  $("lab-tol").addEventListener("input", (e) => { L.tol = +e.target.value; syncPanel(); });
  $("lab-op").addEventListener("input", (e) => { L.opacity = +e.target.value; syncPanel(); draw(); });
  $("lab-3d").addEventListener("change", (e) => { L.threeD = e.target.checked; });
  $("lab-guard").addEventListener("change", (e) => { L.guard = e.target.checked; });
  $("lab-show").addEventListener("change", (e) => { L.show = e.target.checked; draw(); });
  $("lab-interp").addEventListener("click", async () => {
    if (!ready()) return;
    if (!(L.counts[L.active] > 0)) return toast("先用画笔或套索隔几层画出当前标签的轮廓");
    const st = await edit({ kind: "interpolate", label: L.active, guard: L.guard ? L.active : 0 });
    if (st) toast(st.zs.length ? `补齐了 ${st.zs.length} 层` : "没有要补的层：当前标签画过的层之间没有空层");
  });
  $("lab-new").addEventListener("click", () => {
    L.active = Math.max(maxUsed(), L.active) + 1;
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
  };
})();
