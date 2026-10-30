"use strict";
// Pro diffraction indexing: finds the spots of a single-crystal diffraction
// pattern, its centre and 2D lattice (diffraction.rs), labels each spot with
// its (hk) and d on the image, and, given a unit cell, ranks the zone axes
// that would give this pattern.
// Shares app.js globals: S, invoke, $, pref, visibleRegion, fftChannel, toast, ext, requestRender, isRecip, UNIT_M.
(() => {
  const D = { busy: false, res: null, token: null, z: null, zones: null, pick: -1 };

  const btn = document.createElement("button");
  btn.id = "btn-diff";
  btn.className = "toggle";
  btn.title = "衍射标定：找斑点和中心，拟合二维倒易晶格，标出 (hk) 和 d 值，按晶胞匹配带轴";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="2.2"/><circle cx="5" cy="9" r="1.3"/><circle cx="19" cy="15" r="1.3"/><circle cx="9" cy="5" r="1.3"/><circle cx="15" cy="19" r="1.3"/><circle cx="16" cy="6" r="1.3"/><circle cx="8" cy="18" r="1.3"/></svg>';
  btn.disabled = true;
  $("btn-3d") ? $("btn-3d").before(btn) : $("quickbar").append(btn);

  const panel = document.createElement("aside");
  panel.id = "diff-panel";
  panel.className = "pro-panel";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="al-head">
      <b>衍射标定</b>
      <button id="df-close" class="small" title="关闭">✕</button>
    </div>
    <div class="al-opts">
      <label>斑点最小间距 px <input type="number" id="df-dist" min="3" step="1" title="两个斑点至少隔这么远；比斑点直径大、比斑点间距小" /></label>
      <label>阈值 σ <input type="number" id="df-sigma" min="1" step="0.5" title="斑点要比背景高出这么多倍噪声；看不到弱斑点就调低" /></label>
      <label class="al-check al-wide"><input type="checkbox" id="df-view" /> 只用当前看到的区域</label>
    </div>
    <div class="al-row">
      <button id="df-run" class="small primary">找斑点并标定</button>
      <span id="df-status" class="dim"></span>
    </div>
    <div id="df-result" hidden>
      <div id="df-sum"></div>
      <label class="al-check"><input type="checkbox" id="df-show" checked /> 在图上标出斑点和指数</label>
      <div id="df-table" class="pro-table"></div>
      <div class="al-row">
        <button id="df-copy" class="small" title="复制每个斑点的位置、指数和 d 值（制表符分隔，可贴到 Excel）">复制</button>
      </div>
      <details id="df-zone">
        <summary>按晶胞匹配带轴</summary>
        <div class="al-opts df-cell">
          <label>a Å <input type="number" id="df-a" step="0.001" min="0" /></label>
          <label>b Å <input type="number" id="df-b" step="0.001" min="0" /></label>
          <label>c Å <input type="number" id="df-c" step="0.001" min="0" /></label>
          <label>α° <input type="number" id="df-al" step="0.1" /></label>
          <label>β° <input type="number" id="df-be" step="0.1" /></label>
          <label>γ° <input type="number" id="df-ga" step="0.1" /></label>
          <label>点阵类型
            <select id="df-cen" title="带心点阵会消光一部分衍射">
              <option value="P">P 简单</option><option value="I">I 体心</option><option value="F">F 面心</option>
              <option value="C">C 底心</option><option value="A">A 底心</option><option value="B">B 底心</option>
              <option value="R">R 菱方（六方轴）</option>
            </select>
          </label>
          <label>带轴指数最大 <select id="df-uvw"><option>2</option><option>3</option><option>4</option><option>5</option></select></label>
        </div>
        <div class="al-row">
          <button id="df-match" class="small">匹配带轴</button>
          <span id="df-zstatus" class="dim"></span>
        </div>
        <div id="df-zones" class="pro-table"></div>
      </details>
    </div>`;
  $("stage").appendChild(panel);

  const P = { dist: 12, sigma: 5, view: false, cell: [5.431, 5.431, 5.431, 90, 90, 90], cen: "F", uvw: 3, ...pref.get("diff-params", {}) };
  const cellIds = ["df-a", "df-b", "df-c", "df-al", "df-be", "df-ga"];
  $("df-dist").value = P.dist;
  $("df-sigma").value = P.sigma;
  $("df-view").checked = !!P.view;
  cellIds.forEach((id, i) => ($(id).value = P.cell[i]));
  $("df-cen").value = P.cen;
  $("df-uvw").value = P.uvw;
  const saveParams = () =>
    pref.set("diff-params", {
      dist: +$("df-dist").value,
      sigma: +$("df-sigma").value,
      view: $("df-view").checked,
      cell: cellIds.map((id) => +$(id).value),
      cen: $("df-cen").value,
      uvw: +$("df-uvw").value,
    });
  for (const id of ["df-dist", "df-sigma", "df-view", "df-cen", "df-uvw", ...cellIds]) $(id).addEventListener("change", saveParams);

  const open = () => {
    panel.hidden = false;
    btn.classList.add("on");
    if (D.token !== S.token) reset();
    // The right-hand panels share one spot: one at a time.
    if (F.on) setFft(false);
    document.dispatchEvent(new CustomEvent("pro-panel", { detail: panel.id }));
  };
  const close = () => {
    panel.hidden = true;
    btn.classList.remove("on");
    requestRender();
  };
  function reset() {
    D.res = null;
    D.zones = null;
    D.token = S.token;
    D.pick = -1;
    $("df-result").hidden = true;
    $("df-status").textContent = "";
    $("df-zones").replaceChildren();
    $("df-zstatus").textContent = "";
    requestRender();
  }
  btn.addEventListener("click", () => (panel.hidden ? open() : close()));
  $("df-close").addEventListener("click", close);
  document.addEventListener("pro-panel", (e) => e.detail !== panel.id && !panel.hidden && close());
  $("btn-fft").addEventListener("click", () => F.on && !panel.hidden && close());

  // ---- Calibration: image pixels → reciprocal space ----

  /** Pixel vector → 1/Å, or null when the pixel size is not reciprocal. */
  function recip(v) {
    if (!S.px || !isRecip(S.px.unit)) return null;
    const k = UNIT_M[S.px.unit] / 1e10;
    return [v[0] * S.px.x * k, v[1] * S.px.y * k];
  }
  const len = (v) => Math.hypot(v[0], v[1]);
  const angle = (a, b) => (Math.acos(Math.max(-1, Math.min(1, (a[0] * b[0] + a[1] * b[1]) / (len(a) * len(b))))) * 180) / Math.PI;
  /** d spacing text for a pixel vector: Å below 10 Å, nm above. */
  function dText(v) {
    const g = recip(v);
    if (!g || len(g) === 0) return "";
    const d = 1 / len(g);
    return d < 10 ? `${d.toFixed(3)} Å` : `${(d / 10).toFixed(3)} nm`;
  }

  async function run() {
    if (D.busy || !S.info || S.axis !== "xy") return;
    const token = S.token;
    const req = {
      token,
      z: S.z,
      t: S.t,
      c: fftChannel(),
      region: $("df-view").checked ? visibleRegion() : null,
      minDist: Math.max(3, +$("df-dist").value || 12),
      sigma: Math.max(1, +$("df-sigma").value || 5),
      center: null,
    };
    D.busy = true;
    $("df-run").disabled = true;
    $("df-status").textContent = "计算中…";
    try {
      const res = await invoke("diffraction_find", { req });
      if (token !== S.token) return;
      D.res = res;
      D.token = token;
      D.z = req.z;
      D.zones = null;
      D.pick = -1;
      $("df-zones").replaceChildren();
      $("df-zstatus").textContent = "";
      $("df-status").textContent = `${res.spots.length} 个斑点 · ${res.ms.toFixed(0)} ms`;
      show();
    } catch (e) {
      $("df-status").textContent = "失败：" + e;
    } finally {
      D.busy = false;
      $("df-run").disabled = false;
      requestRender();
    }
  }

  function show() {
    const r = D.res;
    $("df-result").hidden = !r;
    if (!r) return;
    const l = r.lattice;
    const sum = $("df-sum");
    sum.replaceChildren();
    const line = (t, cls) => {
      const d = document.createElement("div");
      d.textContent = t;
      if (cls) d.className = cls;
      sum.appendChild(d);
    };
    line(`中心 (${r.center[0].toFixed(1)}, ${r.center[1].toFixed(1)}) px`);
    if (!l) {
      line("没找到能标定这些斑点的晶格：试试调整斑点间距或阈值", "dim");
    } else {
      const ga = recip(l.a), gb = recip(l.b);
      const ang = ga ? angle(ga, gb) : angle(l.a, l.b);
      const fmt = (v, g) => `${len(v).toFixed(2)} px` + (g ? ` · ${(len(g) * 10).toFixed(4)} 1/nm · d ${dText(v)}` : "");
      line(`a* ${fmt(l.a, ga)}`);
      line(`b* ${fmt(l.b, gb)}`);
      line(`夹角 γ* ${ang.toFixed(2)}° · ${l.inliers}/${r.spots.length} 个斑点在晶格上 · 残差 ${l.rms.toFixed(2)} px`);
      if (!ga) line("像素大小不是倒易单位（1/nm），只能给出像素长度；在左侧工具栏的比例尺里把像素大小设成 1/nm 后可得到 d 值", "dim");
    }
    const t = document.createElement("table");
    t.innerHTML = "<thead><tr><th>#</th><th>h k</th><th>|g| px</th><th>d</th><th>强度 σ</th></tr></thead>";
    const tb = document.createElement("tbody");
    r.spots.forEach((s, i) => {
      const tr = document.createElement("tr");
      tr.dataset.i = i;
      if (!s.hk) tr.className = "low";
      const g = [s.x - r.center[0], s.y - r.center[1]];
      for (const v of [i + 1, s.hk ? `${s.hk[0]} ${s.hk[1]}` : "–", len(g).toFixed(1), dText(g), s.snr.toFixed(0)]) {
        const td = document.createElement("td");
        td.textContent = v;
        tr.appendChild(td);
      }
      tb.appendChild(tr);
    });
    t.appendChild(tb);
    t.addEventListener("click", (e) => {
      const tr = e.target.closest("tr[data-i]");
      if (!tr) return;
      D.pick = +tr.dataset.i === D.pick ? -1 : +tr.dataset.i;
      for (const x of tb.children) x.classList.toggle("on", +x.dataset.i === D.pick);
      requestRender();
    });
    $("df-table").replaceChildren(t);
    $("df-match").disabled = !l;
  }

  async function match() {
    const l = D.res && D.res.lattice;
    if (!l) return;
    let ga = recip(l.a), gb = recip(l.b);
    if (!ga) {
      $("df-zstatus").textContent = "需要先在左侧工具栏的比例尺里把像素大小设成倒易单位（1/nm）";
      return;
    }
    if (len(gb) < len(ga)) [ga, gb] = [gb, ga];
    const req = {
      cell: cellIds.map((id) => +$(id).value),
      centering: $("df-cen").value,
      aLen: len(ga),
      bLen: len(gb),
      gamma: angle(ga, gb),
      maxUvw: +$("df-uvw").value,
    };
    try {
      D.zones = await invoke("diffraction_zones", { req });
    } catch (e) {
      $("df-zstatus").textContent = "失败：" + e;
      return;
    }
    $("df-zstatus").textContent = D.zones.length ? "按长度和夹角误差排序" : "没有可比的带轴";
    const t = document.createElement("table");
    t.innerHTML = "<thead><tr><th>[uvw]</th><th>g₁</th><th>g₂</th><th>夹角</th><th>长度差</th><th>角度差</th></tr></thead>";
    const tb = document.createElement("tbody");
    const hkl = (h) => `(${h.join(" ")})`;
    // Symmetry-equivalent axes give the same lengths and angle: one row each, with a count.
    const groups = [];
    for (const z of D.zones) {
      const g = groups.find((g) => Math.abs(g[0].g1Len - z.g1Len) < 1e-6 && Math.abs(g[0].g2Len - z.g2Len) < 1e-6 && Math.abs(g[0].angle - z.angle) < 1e-3);
      g ? g.push(z) : groups.push([z]);
    }
    for (const g of groups) {
      const z = g[0];
      const tr = document.createElement("tr");
      if (z.lenErr > 0.05 || z.angleErr > 3) tr.className = "low";
      for (const v of [`[${z.uvw.join(" ")}]` + (g.length > 1 ? ` ×${g.length}` : ""), hkl(z.g1), hkl(z.g2), `${z.angle.toFixed(1)}°`, `${(z.lenErr * 100).toFixed(1)}%`, `${z.angleErr.toFixed(1)}°`]) {
        const td = document.createElement("td");
        td.textContent = v;
        tr.appendChild(td);
      }
      tb.appendChild(tr);
    }
    t.appendChild(tb);
    $("df-zones").replaceChildren(t);
  }

  $("df-run").addEventListener("click", run);
  $("df-match").addEventListener("click", match);
  $("df-show").addEventListener("change", requestRender);
  $("df-copy").addEventListener("click", () => {
    const r = D.res;
    if (!r) return;
    const rows = r.spots.map((s, i) => {
      const g = [s.x - r.center[0], s.y - r.center[1]];
      const gr = recip(g);
      return [i + 1, s.x.toFixed(2), s.y.toFixed(2), s.hk ? s.hk[0] : "", s.hk ? s.hk[1] : "", len(g).toFixed(2), gr ? (len(gr) * 10).toFixed(5) : "", gr && len(gr) ? (1 / len(gr)).toFixed(4) : "", s.snr.toFixed(1)].join("\t");
    });
    navigator.clipboard.writeText(["#\tx_px\ty_px\th\tk\tg_px\tg_1/nm\td_A\tsnr", ...rows].join("\n")).then(
      () => toast("已复制斑点列表"),
      (e) => toast(`复制失败：${e}`),
    );
  });

  // ---- Overlay: spots, indices, basis vectors and centre ----

  function overlay(g) {
    const r = D.res;
    if (!r || panel.hidden || !$("df-show").checked || D.token !== S.token || S.axis !== "xy" || S.z !== D.z) return;
    const at = (x, y) => toScreen(x + 0.5, y + 0.5);
    const l = r.lattice;
    const rad = Math.max(4, Math.min(14, (+$("df-dist").value || 12) * 0.4 * S.view.scale));
    const labels = l && Math.min(len(l.a), len(l.b)) * S.view.scale > 34;
    g.save();
    g.lineWidth = 1.5;
    g.lineJoin = "round";
    g.font = "11px system-ui, sans-serif";
    g.textAlign = "center";
    g.textBaseline = "top";
    r.spots.forEach((s, i) => {
      const [x, y] = at(s.x, s.y);
      g.strokeStyle = i === D.pick ? "#cf222e" : s.hk ? "#1a7f37" : "#8c959f";
      g.beginPath();
      g.arc(x, y, i === D.pick ? rad + 2 : rad, 0, 2 * Math.PI);
      g.stroke();
      if (s.hk && (labels || i === D.pick) && (s.hk[0] || s.hk[1])) {
        const t = `${s.hk[0]} ${s.hk[1]}`;
        g.lineWidth = 3;
        g.strokeStyle = "rgba(255,255,255,0.85)";
        g.strokeText(t, x, y + rad + 2);
        g.fillStyle = "#0b4f1f";
        g.fillText(t, x, y + rad + 2);
        g.lineWidth = 1.5;
      }
    });
    const [cx, cy] = at(r.center[0], r.center[1]);
    if (l) {
      for (const [v, name, col] of [[l.a, "a*", "#0969da"], [l.b, "b*", "#bf3989"]]) {
        const [x, y] = at(r.center[0] + v[0], r.center[1] + v[1]);
        const ang = Math.atan2(y - cy, x - cx);
        g.strokeStyle = g.fillStyle = col;
        g.lineWidth = 2;
        g.beginPath();
        g.moveTo(cx, cy);
        g.lineTo(x, y);
        g.lineTo(x - 8 * Math.cos(ang - 0.4), y - 8 * Math.sin(ang - 0.4));
        g.moveTo(x, y);
        g.lineTo(x - 8 * Math.cos(ang + 0.4), y - 8 * Math.sin(ang + 0.4));
        g.stroke();
        g.textBaseline = "middle";
        g.fillText(name, x + 12 * Math.cos(ang), y + 12 * Math.sin(ang));
      }
    }
    g.strokeStyle = "#cf222e";
    g.lineWidth = 1.5;
    g.beginPath();
    g.moveTo(cx - 9, cy);
    g.lineTo(cx + 9, cy);
    g.moveTo(cx, cy - 9);
    g.lineTo(cx, cy + 9);
    g.stroke();
    g.restore();
  }
  ext.overlays.push(overlay);

  // Enable in the XY view; forget results when another image opens.
  setInterval(() => {
    const ok = !!(S.info && S.axis === "xy");
    btn.disabled = !ok;
    if (!ok && !panel.hidden && !D.busy) close();
    if (!panel.hidden && S.token !== D.token && !D.busy && S.axis === "xy") reset();
  }, 400);
})();
