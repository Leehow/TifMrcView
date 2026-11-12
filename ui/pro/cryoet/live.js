"use strict";
// The docked 「正在处理」 pane: while a pass runs, the picture of what is being
// processed (live_picture) sits at the left edge of the stage, beside the
// image the user is looking at, whether or not the Cryo-ET panel is open.
// W.run drives it: start(), progress(text), refresh(), stop().
(() => {
  const R = window.TmvCryoRules;
  const dock = window.TifMrcViewPro.liveDock = {};
  const pane = document.createElement("aside");
  pane.id = "live-dock";
  pane.hidden = true;
  pane.innerHTML = `
    <div class="ld-head"><b>正在处理</b><span class="ld-text dim"></span><button class="small ld-fold" title="折叠 / 展开">−</button></div>
    <figure class="ld-pic"><canvas></canvas><figcaption class="dim"></figcaption></figure>`;
  $("stage").append(pane);
  const cv = pane.querySelector("canvas"), cap = pane.querySelector("figcaption"), text = pane.querySelector(".ld-text");
  const fold = pane.querySelector(".ld-fold");
  fold.onclick = () => {
    pane.classList.toggle("folded");
    fold.textContent = pane.classList.contains("folded") ? "+" : "−";
  };

  let on = false, busy = false, last = 0, timer = 0, shown = false;

  // The Cryo-ET panel floats where the user put it: the pane takes the side
  // of the stage the panel leaves free.
  const side = () => {
    const panel = $("cryoet-workspace");
    if (!panel || panel.hidden || pane.hidden) return pane.classList.remove("right");
    const s = $("stage").getBoundingClientRect(), r = panel.getBoundingClientRect();
    pane.classList.toggle("right", r.left + r.width / 2 < s.left + s.width / 2);
  };
  let watching = false;

  dock.start = async () => {
    on = true;
    shown = false;
    pane.hidden = false;
    pane.querySelector(".ld-pic").hidden = true;
    if (!watching && $("cryoet-workspace")) {
      watching = true;
      new MutationObserver(side).observe($("cryoet-workspace"), { attributes: true, attributeFilter: ["hidden", "style", "class"] });
    }
    side();
    text.textContent = "";
    await invoke("live_clear").catch(() => {});
  };
  dock.progress = (t) => { text.textContent = t; text.title = t; };
  dock.stop = () => {
    on = false;
    clearTimeout(timer);
    timer = 0;
    pane.hidden = true;
  };
  dock.active = () => on;

  // At most one request in flight and one every 400 ms.
  dock.refresh = async () => {
    if (!on || busy || timer) return;
    const wait = 400 - (performance.now() - last);
    if (wait > 0) {
      timer = setTimeout(() => { timer = 0; dock.refresh(); }, wait);
      return;
    }
    busy = true;
    last = performance.now();
    try {
      const pic = R.decodePicture(await invoke("live_picture"));
      if (!on || !pic) return;
      cv.width = pic.w;
      cv.height = pic.h;
      cv.getContext("2d").putImageData(new ImageData(pic.rgba, pic.w, pic.h), 0, 0);
      cap.textContent = pic.title;
      pane.querySelector(".ld-pic").hidden = false;
      shown = true;
    } catch (_) {
      // Nothing to show yet.
    } finally {
      busy = false;
    }
  };
  dock.shown = () => shown;
})();
