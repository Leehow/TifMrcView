"use strict";
// Pro spectrum quantification in the spectrum panel: EDS by Cliff–Lorimer
// with theoretical or given k-factors, EELS core-loss edges by power-law
// background subtraction (spectro.rs). Draws the fitted backgrounds and
// line / edge labels on the plot.
// Shares app.js globals: S, P, invoke, $, pref, toast, ext, specInfo, specX, specRegion, drawSpec, fmt.
(() => {
  const Q = { open: false, busy: false, kind: null, res: null, key: null, token: null };

  const btn = document.createElement("button");
  btn.id = "spec-quant";
  btn.className = "small toggle";
  btn.title = "定量：EDS 元素含量（Cliff–Lorimer）、EELS 电离边扣背景积分";
  btn.textContent = "定量";
  $("spec-close").before(btn);

  // The plot and the quantification side by side.
  const body = document.createElement("div");
  body.className = "sq-body";
  $("spec-plot").before(body);
  body.append($("spec-plot"));
  const side = document.createElement("div");
  side.id = "sq-side";
  side.hidden = true;
  side.innerHTML = `
    <div class="al-row sq-top">
      <select id="sq-kind" title="谱的类型">
        <option value="EDS">EDS 能谱</option>
        <option value="EELS">EELS 能量损失谱</option>
      </select>
      <button id="sq-run" class="small primary">分析</button>
      <span id="sq-status" class="dim"></span>
    </div>
    <div id="sq-eds" class="al-opts">
      <label>加速电压 kV <input type="number" id="sq-kv" min="1" step="1" /></label>
      <label>分辨率 eV（Mn Kα）<input type="number" id="sq-fwhm" min="40" step="1" title="探测器在 5.9 keV 的半高宽，硅漂移探测器一般 125–135" /></label>
      <label class="al-wide">元素 <input type="text" id="sq-els" placeholder="留空自动识别，如 Fe Cr Ni" spellcheck="false" /></label>
      <label class="al-wide">k 因子 <input type="text" id="sq-k" placeholder="留空用理论值，如 Fe=1 Cr=0.92 Ni=1.07" spellcheck="false" title="相对任一元素的 Cliff–Lorimer k 因子，有标样实测值时填上更准" /></label>
    </div>
    <div id="sq-eels" class="al-opts" hidden>
      <label>积分窗口 eV <input type="number" id="sq-win" min="5" step="5" /></label>
      <label>边前窗口 eV <input type="number" id="sq-pre" min="0" step="5" title="拟合幂律背景的宽度；0 为自动（起始能量的五分之一，20–100 eV）" /></label>
      <label class="al-wide">电离边 <input type="text" id="sq-edges" placeholder="留空自动识别，如 C K, O K, Fe L3" spellcheck="false" /></label>
      <label class="al-wide">截面 σ <input type="text" id="sq-sigma" placeholder="可选，如 C K=1.2e-21, O K=4.5e-22" spellcheck="false" title="每个边在所用窗口和收集角下的部分电离截面；都填了才算原子比" /></label>
    </div>
    <div id="sq-sum" class="dim"></div>
    <div id="sq-table" class="pro-table"></div>
    <div class="al-row">
      <label class="al-check"><input type="checkbox" id="sq-show" checked /> 在谱上标出</label>
      <button id="sq-copy" class="small" title="复制结果（制表符分隔，可贴到 Excel）" disabled>复制</button>
    </div>
    <div id="sq-note" class="dim"></div>`;
  body.append(side);

  const num = (id, d) => {
    const v = parseFloat($(id).value);
    return Number.isFinite(v) ? v : d;
  };
  for (const [id, d] of [["sq-kv", 200], ["sq-fwhm", 130], ["sq-win", 50], ["sq-pre", 0]]) {
    $(id).value = pref.get("tmv.sq." + id, d);
    $(id).addEventListener("change", () => pref.set("tmv.sq." + id, $(id).value));
  }

  /** eV per axis unit, or null when the axis is not energy. */
  function evScale(sp) {
    const u = (sp.unit || "").trim();
    if (u === "keV") return 1e3;
    if (u === "eV" || u === "") return 1;
    if (u === "MeV") return 1e6;
    return null;
  }

  function guessKind(sp) {
    if (sp.kind === "EDS" || sp.kind === "EELS") return sp.kind;
    const f = evScale(sp) || 1;
    const lo = sp.start * f, hi = (sp.start + sp.step * ((P.data && P.data.length) || 1)) * f;
    // EELS starts at or below the zero loss; EDS reaches several keV.
    return lo < -1 || (sp.unit !== "keV" && hi < 3000) ? "EELS" : "EDS";
  }

  function showKind() {
    const k = $("sq-kind").value;
    $("sq-eds").hidden = k !== "EDS";
    $("sq-eels").hidden = k !== "EELS";
  }

  function setOpen(on) {
    Q.open = on;
    btn.classList.toggle("on", on);
    side.hidden = !on;
    $("spec-panel").classList.toggle("sq-open", on);
    if (on) {
      const sp = specInfo();
      // Guess the kind once per file.
      if (sp && Q.token !== S.token) {
        Q.token = S.token;
        Q.kind = guessKind(sp);
        $("sq-kind").value = Q.kind;
      }
      showKind();
    }
    drawSpec();
  }

  btn.addEventListener("click", () => setOpen(!Q.open));
  $("sq-kind").addEventListener("change", () => {
    showKind();
    clear();
  });
  $("sq-show").addEventListener("change", drawSpec);

  function clear() {
    Q.res = null;
    Q.key = null;
    $("sq-table").innerHTML = "";
    $("sq-sum").textContent = "";
    $("sq-note").textContent = "";
    $("sq-copy").disabled = true;
    drawSpec();
  }

  /** "Fe=1, Cr 0.9" → [["Fe", 1], ["Cr", 0.9]]. */
  function pairs(text) {
    const out = [];
    for (const part of text.split(/[,;，；\n]+/)) {
      const m = part.trim().match(/^(.+?)\s*[=:：]?\s*([-+]?[\d.]+(?:e[-+]?\d+)?)$/i);
      if (m) out.push([m[1].trim(), parseFloat(m[2])]);
    }
    return out;
  }

  function list(text, sep) {
    return text.split(sep).map((s) => s.trim()).filter(Boolean);
  }

  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const sig = (v, n = 3) => (Number.isFinite(v) ? String(+v.toPrecision(n)) : "–");

  async function run() {
    const sp = specInfo();
    if (!sp || !P.data || Q.busy) return;
    const f = evScale(sp);
    if (f == null) return toast(`谱的横轴单位是「${sp.unit}」，不是能量，没法定量`);
    // Means of many pixels are scaled back to summed counts so that the
    // counting errors mean something.
    const reg = specRegion();
    const n = reg ? (sp.axis === "x" ? reg[3] : reg[2] * reg[3]) : 1;
    const values = P.data.map((v) => v * Math.max(1, n));
    const kind = $("sq-kind").value;
    Q.kind = kind;
    Q.busy = true;
    $("sq-run").disabled = true;
    $("sq-status").textContent = "分析中…";
    const key = P.key;
    try {
      let res;
      if (kind === "EDS") {
        res = await invoke("eds_quant", {
          req: {
            values, start: sp.start * f, step: sp.step * f,
            e0Kv: num("sq-kv", 200), fwhmMn: num("sq-fwhm", 130),
            elements: list($("sq-els").value, /[\s,，;；]+/),
            k: pairs($("sq-k").value),
          },
        });
      } else {
        res = await invoke("eels_quant", {
          req: {
            values, start: sp.start * f, step: sp.step * f,
            window: num("sq-win", 50), pre: num("sq-pre", 0),
            edges: list($("sq-edges").value, /[,，;；]+/),
            sigma: pairs($("sq-sigma").value),
          },
        });
      }
      Q.res = { kind, f, n, ...res };
      Q.key = key;
      $("sq-status").textContent = "";
      show();
    } catch (e) {
      $("sq-status").textContent = "";
      toast(`定量失败：${e}`);
    } finally {
      Q.busy = false;
      $("sq-run").disabled = false;
    }
  }
  $("sq-run").addEventListener("click", run);

  function show() {
    const r = Q.res;
    const t = $("sq-table");
    if (r.kind === "EDS") {
      const rows = r.quant;
      $("sq-sum").textContent = rows.length
        ? `${r.auto ? "自动识别 " : ""}${rows.length} 个元素，${r.peaks.length} 个峰`
        : `没找到可定量的元素（${r.peaks.length} 个峰）`;
      t.innerHTML = rows.length
        ? `<table><thead><tr><th>元素</th><th>线系</th><th>wt%</th><th>at%</th><th>±%</th><th>净计数</th><th>k</th></tr></thead><tbody>${rows
            .map(
              (q) =>
                `<tr${q.overlaps.length ? ' class="edge" title="和 ' + esc(q.overlaps.join("、")) + ' 的谱线重叠，含量可能偏高"' : ""}><td>${esc(q.element)}${q.overlaps.length ? " ⚠" : ""}</td><td>${esc(q.family)}</td><td>${sig(q.wt)}</td><td>${sig(q.at)}</td><td>${sig(q.wt * (q.relErr ?? NaN), 2)}</td><td>${sig(q.net, 4)}</td><td>${sig(q.k)}${q.kGiven ? "" : "*"}</td></tr>`,
            )
            .join("")}</tbody></table>`
        : "";
      const given = rows.length && rows.every((q) => q.kGiven);
      $("sq-note").textContent = given
        ? "用你给的 k 因子（薄样品近似）。"
        : "* 理论 k 因子（相对 Fe K；Bethe 电离截面 × 荧光产额，薄样品近似，没做吸收和探测器效率校正）。K 线之间误差约 10–20%，L、M 线可差到 2 倍；有标样实测的 k 因子时填上。SEM 块状样品需要 ZAF 校正，这里的结果只作参考。";
    } else {
      const rows = r.edges;
      const tl = r.tLambda != null ? `，t/λ = ${sig(r.tLambda)}` : "";
      $("sq-sum").textContent = rows.length ? `${r.auto ? "自动识别 " : ""}${rows.length} 个电离边${tl}` : `没找到电离边${tl}`;
      t.innerHTML = rows.length
        ? `<table><thead><tr><th>边</th><th>起始 eV</th><th>信号</th><th>±</th><th>跳跃比</th><th>r</th><th>at%</th></tr></thead><tbody>${rows
            .map(
              (m) =>
                `<tr${m.alt.length ? ' title="也可能是 ' + esc(m.alt.join("、")) + '"' : ""}><td>${esc(m.label)}${m.alt.length ? " ?" : ""}</td><td>${sig(m.onset, 4)}</td><td>${sig(m.signal, 4)}</td><td>${sig(m.err, 2)}</td><td>${sig(m.jump)}</td><td>${sig(m.r)}</td><td>${m.at != null ? sig(m.at) : "–"}</td></tr>`,
            )
            .join("")}</tbody></table>`
        : "";
      $("sq-note").textContent =
        rows.length && rows.every((m) => m.at != null)
          ? "原子比 = 信号 / 截面，截面由你给出。"
          : "信号是幂律背景以上、窗口内的净计数；填上各边的部分截面 σ 才能算原子比。带 ? 的边还可能是别的元素，鼠标停留看候选。";
    }
    $("sq-copy").disabled = false;
    drawSpec();
  }

  $("sq-copy").addEventListener("click", () => {
    const r = Q.res;
    if (!r) return;
    const lines =
      r.kind === "EDS"
        ? [["元素", "线系", "能量 eV", "wt%", "at%", "相对误差", "净计数", "k", "k 来源", "重叠"].join("\t")].concat(
            r.quant.map((q) => [q.element, q.family, q.energy, q.wt, q.at, q.relErr, q.net, q.k, q.kGiven ? "给定" : "理论", q.overlaps.join(" ")].join("\t")),
          )
        : [["边", "起始 eV", "信号", "误差", "跳跃比", "幂律 r", "at%", "其他候选"].join("\t")]
            .concat(r.edges.map((m) => [m.label, m.onset, m.signal, m.err, m.jump, m.r, m.at ?? "", m.alt.join(" ")].join("\t")))
            .concat(r.tLambda != null ? [`t/λ\t${r.tLambda}`] : []);
    navigator.clipboard.writeText(lines.join("\n")).then(() => toast("已复制"));
  });

  // A different spectrum makes the result stale.
  ext.specOverlays.push((g, m) => {
    const r = Q.res;
    if (!r || !Q.open || !$("sq-show").checked) return;
    if (Q.key !== P.key) {
      if (P.key) clearSoon();
      return;
    }
    const sp = specInfo();
    // Results are in eV and summed counts; the plot is in axis units and means.
    const ch = (ev) => (ev / r.f - sp.start) / sp.step;
    const y = (v) => m.py(m.tf(v / Math.max(1, r.n)));
    g.save();
    g.font = "11px -apple-system, 'Segoe UI', sans-serif";
    g.textAlign = "center";
    g.textBaseline = "bottom";
    if (r.kind === "EDS") {
      g.strokeStyle = "#8c959f";
      g.setLineDash([4, 3]);
      g.lineWidth = 1;
      g.beginPath();
      const step = Math.max(1, Math.floor((m.i1 - m.i0) / (m.W - m.l - m.r) / 2));
      for (let i = Math.max(0, Math.floor(m.i0)); i <= Math.min(r.background.length - 1, Math.ceil(m.i1)); i += step) {
        const fn = i === Math.max(0, Math.floor(m.i0)) ? g.moveTo : g.lineTo;
        fn.call(g, m.px(i), y(r.background[i]));
      }
      g.stroke();
      g.setLineDash([]);
      g.fillStyle = "#cf222e";
      // Strongest peaks first; a label that would cover another moves up a
      // row, and is left out after three rows.
      const placed = [];
      const peaks = r.peaks.filter((p) => p.label).sort((a, b) => b.net - a.net).slice(0, 40);
      for (const p of peaks) {
        const i = ch(p.energy);
        if (i < m.i0 || i > m.i1) continue;
        const w = g.measureText(p.label).width + 6;
        const x = Math.min(Math.max(m.px(i), m.l + w / 2), m.W - m.r - w / 2);
        const v = P.data[Math.round(i)] ?? 0;
        let yb = Math.max(m.t + 12, m.py(m.tf(v)) - 3);
        let ok = false;
        for (let row = 0; row < 3 && !ok; row++) {
          ok = !placed.some((q) => Math.abs(q.x - x) < (q.w + w) / 2 && Math.abs(q.y - yb) < 12);
          if (!ok) yb = Math.max(m.t + 12, yb - 13);
        }
        if (!ok) continue;
        placed.push({ x, y: yb, w });
        g.fillText(p.label, x, yb);
      }
    } else {
      for (const e of r.edges) {
        g.strokeStyle = "#bc4c00";
        g.lineWidth = 1.25;
        g.beginPath();
        e.background.forEach(([ev, v], k) => (k ? g.lineTo : g.moveTo).call(g, m.px(ch(ev)), y(v)));
        g.stroke();
        // Fit range and integration window.
        g.fillStyle = "rgba(188, 76, 0, 0.08)";
        const [fa, fb] = e.fit.map((v) => m.px(ch(v)));
        g.fillRect(fa, m.t, fb - fa, m.H - m.t - m.b);
        g.fillStyle = "rgba(26, 127, 55, 0.10)";
        const end = e.background[e.background.length - 1][0];
        const [wa, wb] = [m.px(ch(e.onset)), m.px(ch(end))];
        g.fillRect(wa, m.t, wb - wa, m.H - m.t - m.b);
        g.strokeStyle = "#1a7f37";
        g.beginPath();
        g.moveTo(wa, m.t);
        g.lineTo(wa, m.H - m.b);
        g.stroke();
        g.fillStyle = "#1a7f37";
        g.textAlign = "left";
        g.fillText(e.label + (e.alt.length ? " ?" : ""), wa + 3, m.t + 12);
      }
    }
    g.restore();
  });

  let clearTimer = 0;
  function clearSoon() {
    // Not while drawing: clear() redraws.
    clearTimeout(clearTimer);
    clearTimer = setTimeout(() => {
      if (Q.key !== P.key) {
        clear();
        $("sq-status").textContent = "谱变了，重新分析";
        if (Q.open && Q.token !== S.token) setOpen(true);
      }
    }, 0);
  }

})();
