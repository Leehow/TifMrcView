"use strict";
// The Cryo-ET workspace is one panel with three jobs, as tabs under its title:
//   快速查看 — movies become tilt series to look at and choose from (the first pass);
//   三维重构 — the tomograms of the chosen series, for a purpose;
//   子断层平均 — the subtomogram-averaging part (sta/), hosted in this panel.
// The first two are the folder flow (session.js) with what the other job
// needs hidden (pro.css, by the panel's data-job); the third is the STA
// workspace moved in here (sta.host). The job last used is kept.
// The panel floats over the stage: it is moved by its title and resized by
// any of its four corners (both kept; a double click on the title puts it back).
// Shares app.js globals: $, pref.
(() => {
  const W = window.TifMrcViewPro.cryoet;
  const sta = window.TifMrcViewPro.sta;
  const panel = W.panel;
  const JOBS = [
    ["quick", "快速查看", "把电影快速变成倾转序列，看哪些序列、哪几张图能用"],
    ["recon", "三维重构", "把选好的序列重构成断层图"],
    ["sta", "子断层平均", "从断层图里找出同一种颗粒，平均出它的结构"],
  ].filter(([id]) => id !== "sta" || sta?.host);

  const nav = document.createElement("nav");
  nav.className = "ws-jobs";
  nav.setAttribute("role", "tablist");
  nav.setAttribute("aria-label", "Cryo-ET 的三个功能");
  for (const [id, name, title] of JOBS) {
    const b = document.createElement("button");
    b.dataset.job = id;
    b.textContent = name;
    b.title = title;
    b.setAttribute("role", "tab");
    b.onclick = () => go(id);
    nav.append(b);
  }
  panel.querySelector(".al-head").after(nav);

  const staBox = document.createElement("div");
  staBox.id = "cryo-sta";
  $("cryo-foot").before(staBox);
  const hosted = JOBS.some(([id]) => id === "sta");
  if (hosted) {
    sta.host(staBox);
    sta.hide();
    // While the user marks or checks on the image, the panel gets out of its way.
    const part = staBox.firstElementChild;
    const narrow = () => panel.classList.toggle("narrow", panel.dataset.job === "sta" && ["marking", "reviewing", "reviewing-shapes"].some((c) => part.classList.contains(c)));
    new MutationObserver(narrow).observe(part, { attributes: true, attributeFilter: ["class"] });
    new MutationObserver(narrow).observe(panel, { attributes: true, attributeFilter: ["data-job"] });
  }

  let job = pref.get("cryo-job", "quick");
  if (!JOBS.some(([id]) => id === job)) job = "quick";

  function go(id) {
    const was = panel.dataset.job;
    job = id;
    pref.set("cryo-job", id);
    panel.dataset.job = id;
    for (const b of nav.children) {
      b.classList.toggle("on", b.dataset.job === id);
      b.setAttribute("aria-selected", b.dataset.job === id);
    }
    if (id === "sta") {
      if (!panel.hidden) sta.show();
    } else if (was === "sta") {
      sta.hide();
    }
    document.dispatchEvent(new CustomEvent("cryoet-job", { detail: id }));
  }
  W.job = () => panel.dataset.job;
  W.go = go;
  go(job);
  // The hosted part is refreshed whenever the panel shows it again.
  document.addEventListener("cryoet-open", () => { if (job === "sta" && hosted) sta.show(); });
  document.addEventListener("cryoet-close", () => { if (job === "sta" && hosted) sta.hide(); });

  // ---------- a floating panel: moved by its title, resized by its corners ----------

  const stage = $("stage");
  const head = panel.querySelector(".al-head");
  // Left, top, width, height inside the stage; none until the user moves or resizes it.
  let box = pref.get("cryo-box", null);
  const MIN_W = 340, MIN_H = 240;
  const corners = ["nw", "ne", "sw", "se"].map((c) => {
    const g = document.createElement("div");
    g.className = `ws-corner ws-${c}`;
    g.dataset.corner = c;
    g.title = "拖动改变大小";
    stage.append(g);
    return g;
  });

  const rect = () => {
    const s = stage.getBoundingClientRect(), r = panel.getBoundingClientRect();
    return { x: r.left - s.left, y: r.top - s.top, w: r.width, h: r.height };
  };
  // Keeps the panel inside the stage, whatever the window has become.
  function fit(b) {
    const s = stage.getBoundingClientRect();
    b.w = Math.max(MIN_W, Math.min(b.w, s.width));
    b.h = Math.max(MIN_H, Math.min(b.h, s.height));
    b.x = Math.max(0, Math.min(b.x, s.width - b.w));
    b.y = Math.max(0, Math.min(b.y, s.height - b.h));
    return b;
  }
  function place() {
    if (box && !panel.hidden) {
      fit(box);
      Object.assign(panel.style, { left: `${box.x}px`, top: `${box.y}px`, right: "auto", bottom: "auto", width: `${box.w}px`, height: `${box.h}px` });
    } else if (!box) {
      for (const k of ["left", "top", "right", "bottom", "width", "height"]) panel.style[k] = "";
    }
    const show = !panel.hidden && !panel.classList.contains("narrow");
    const r = show ? rect() : null;
    for (const g of corners) {
      g.hidden = !show;
      if (!show) continue;
      const c = g.dataset.corner;
      g.style.left = `${(c[1] === "w" ? r.x : r.x + r.w) - 8}px`;
      g.style.top = `${(c[0] === "n" ? r.y : r.y + r.h) - 8}px`;
    }
  }

  function drag(e, move) {
    e.preventDefault();
    const from = { ...(box = box || rect()) }, x0 = e.clientX, y0 = e.clientY;
    const target = e.currentTarget;
    target.setPointerCapture(e.pointerId);
    target.onpointermove = (m) => {
      move(from, m.clientX - x0, m.clientY - y0);
      place();
    };
    target.onpointerup = target.onpointercancel = () => {
      target.onpointermove = target.onpointerup = target.onpointercancel = null;
      pref.set("cryo-box", box);
    };
  }
  head.addEventListener("pointerdown", (e) => {
    if (e.button || e.target.closest("button")) return;
    drag(e, (f, dx, dy) => { box.x = f.x + dx; box.y = f.y + dy; });
  });
  // A double click on the title: where and as large as it first was.
  head.addEventListener("dblclick", (e) => {
    if (e.target.closest("button")) return;
    box = null;
    pref.set("cryo-box", null);
    place();
  });
  for (const g of corners) {
    g.onpointerdown = (e) => drag(e, (f, dx, dy) => {
      const c = g.dataset.corner, s = stage.getBoundingClientRect();
      // The opposite corner stays where it is.
      if (c[1] === "w") { box.x = Math.max(0, Math.min(f.x + dx, f.x + f.w - MIN_W)); box.w = f.x + f.w - box.x; }
      else box.w = Math.max(MIN_W, Math.min(f.w + dx, s.width - f.x));
      if (c[0] === "n") { box.y = Math.max(0, Math.min(f.y + dy, f.y + f.h - MIN_H)); box.h = f.y + f.h - box.y; }
      else box.h = Math.max(MIN_H, Math.min(f.h + dy, s.height - f.y));
    });
  }
  new MutationObserver(place).observe(panel, { attributes: true, attributeFilter: ["hidden", "class"] });
  new ResizeObserver(place).observe(stage);
  place();
})();
