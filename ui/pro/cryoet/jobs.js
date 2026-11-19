"use strict";
// The Cryo-ET workspace is one panel with two jobs, steps 1 and 3 of the step guide under its
// title (step 2, painting what to average, sits at the top of the second):
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

  // The two jobs are steps 1 and 3 of the guide on top (guide.js, shared with the single-particle
  // panel); step 2, marking what to average with the label brush, is done on the tomograms and its
  // list of layers is part of 子断层平均 (sta/layers.js).
  const part = (sel) => () => { go("sta"); requestAnimationFrame(() => $(sel)?.scrollIntoView({ behavior: "smooth", block: "start" })); };
  const steps = [
    { title: "三维重构", go: () => go("recon"),
      help: "选放着倾转序列（电影或 .st / .mrc）的项目文件夹，勾选序列，再勾想要的处理（都不勾就是几分钟一个的快速重构，适合先看看样品），按「重构勾选序列」。卡片上的「打开」可以检查倾转图、对齐和断层图，挑掉不好的图后再重构。" },
  ];
  if (JOBS.some(([id]) => id === "sta")) steps.push(
    { title: "画标签（mask）", go: part("#sta-layers"),
      help: "在断层图上标出要平均的东西：在「子断层平均」里点一张断层图的「打开并标注」，查看器换成标签画笔。每种东西一个图层，涂几下，点结构类型（小颗粒、空心球、单层膜、纤维…）让程序贴到真实的边上；想要整张图里所有同类，用「找同类」再「沿 Z」。" },
    { title: "子断层平均", go: part("#sta-layers-use"),
      help: "勾选要平均的图层（可以跨好几张断层图，比如只勾膜），按「用勾选的图层」：程序在原始断层图里找全同类、对齐、平均，并用两半数据的 FSC 给出分辨率。" },
  );
  const guide = window.TifMrcViewPro.guide(panel, "cryoet", steps);
  panel.querySelector(".al-head").after(guide.el);
  W.guide = guide;
  // Done: a tomogram of the folder; a painted layer on one; an average.
  function states() {
    const series = W.series?.() || [];
    const done = [
      series.some((s) => s.result || s.quick) || (sta?.items?.length > 0),
      (sta?.layersMarked || 0) > 0,
      !!sta?.result,
    ].slice(0, steps.length);
    const now = done.findIndex((d) => !d);
    guide.set(done.map((d, i) => (d ? "done" : i === now ? "now" : i > 0 && !done[i - 1] ? "locked" : "open")));
  }
  for (const e of ["pro-guide", "cryoet-idle", "cryoet-open", "sta-out", "sta-busy"]) document.addEventListener(e, states);

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
    // The guide shows the part that is open (a click on a step has set it already).
    if (id === "recon") guide.show(0);
    else if (guide.at() === 0) guide.show(steps.length > 2 && (sta?.layersMarked || 0) > 0 ? 2 : 1);
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
  states();
  // The hosted part is refreshed whenever the panel shows it again.
  document.addEventListener("cryoet-open", () => { if (job === "sta" && hosted) sta.show(); });
  document.addEventListener("cryoet-close", () => { if (job === "sta" && hosted) sta.hide(); });

  // A floating panel: moved by its title, resized by its corners (pro/float.js).
  window.TifMrcViewPro.float(panel, "cryo-box-win");
})();
