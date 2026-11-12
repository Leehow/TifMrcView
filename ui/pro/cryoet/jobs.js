"use strict";
// The Cryo-ET workspace is one panel with three jobs, as tabs under its title:
//   快速查看 — movies become tilt series to look at and choose from (the first pass);
//   三维重构 — the tomograms of the chosen series, for a purpose;
//   子断层平均 — the subtomogram-averaging part (sta/), hosted in this panel.
// The first two are the folder flow (session.js) with what the other job
// needs hidden (pro.css, by the panel's data-job); the third is the STA
// workspace moved in here (sta.host). The job last used is kept.
// The panel is resized by its left and bottom edges (kept too; a double
// click gives the usual size back); the other workspace panels follow.
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

  // ---------- size ----------

  const stage = $("stage");
  const size = pref.get("ws-size", {});
  const apply = () => {
    if (size.w) stage.style.setProperty("--ws-w", `${size.w}px`);
    else stage.style.removeProperty("--ws-w");
    if (size.h) stage.style.setProperty("--ws-h", `${size.h}px`);
    else stage.style.removeProperty("--ws-h");
  };
  apply();
  for (const side of ["w", "h"]) {
    const grip = document.createElement("div");
    grip.className = `ws-grip ws-grip-${side}`;
    grip.title = side === "w" ? "拖动改变工作区宽度（双击恢复）" : "拖动改变工作区高度（双击恢复）";
    stage.append(grip);
    grip.onpointerdown = (e) => {
      e.preventDefault();
      grip.setPointerCapture(e.pointerId);
      const box = stage.getBoundingClientRect();
      grip.onpointermove = (m) => {
        if (side === "w") size.w = Math.round(Math.min(box.width - 20, Math.max(320, box.right - 10 - m.clientX)));
        else size.h = Math.round(Math.min(box.height - 20, Math.max(220, m.clientY - box.top - 10)));
        apply();
      };
      grip.onpointerup = () => {
        grip.onpointermove = grip.onpointerup = null;
        // Pulled to the bottom again: as tall as the stage, whatever its size becomes.
        if (side === "h" && size.h >= box.height - 24) delete size.h;
        apply();
        pref.set("ws-size", size);
      };
    };
    grip.ondblclick = () => {
      delete size[side];
      apply();
      pref.set("ws-size", size);
    };
  }
})();
