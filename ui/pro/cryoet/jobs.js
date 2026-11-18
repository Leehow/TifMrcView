"use strict";
// The Cryo-ET workspace is one panel with two jobs, as tabs under its title:
//   三维重构 — the folder flow (session.js): movies become tilt series and
//     tomograms of the ticked series, with the steps the user ticked (none:
//     the fast tomogram, which is also the look at what the series are like);
//   子断层平均 — the subtomogram-averaging part (sta/), hosted in this panel.
// What the other job needs is hidden by pro.css (the panel's data-job); the second is the STA
// workspace moved in here (sta.host). The job last used is kept.
// The panel floats over the stage: it is moved by its title and resized by
// any of its four corners (both kept; a double click on the title puts it back).
// Shares app.js globals: $, pref.
(() => {
  const W = window.TifMrcViewPro.cryoet;
  const sta = window.TifMrcViewPro.sta;
  const panel = W.panel;
  const JOBS = [
    ["recon", "三维重构", "把选好的序列重构成断层图"],
    ["sta", "子断层平均", "从断层图里找出同一种颗粒，平均出它的结构"],
  ].filter(([id]) => id !== "sta" || sta?.host);

  const nav = document.createElement("nav");
  nav.className = "ws-jobs";
  nav.setAttribute("role", "tablist");
  nav.setAttribute("aria-label", "Cryo-ET 的功能");
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

  // An older version's 快速查看 is now part of 三维重构.
  let job = pref.get("cryo-job", "recon");
  if (!JOBS.some(([id]) => id === job)) job = "recon";

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

  // A floating panel: moved by its title, resized by its corners (pro/float.js).
  window.TifMrcViewPro.float(panel, "cryo-box-win");
})();
