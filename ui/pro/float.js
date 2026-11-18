"use strict";
// Floating workspaces (the Cryo-ET panel, the single-particle panel): moved by their title and
// resized by any of their four corners, both kept (`key` in the preferences); a double click on the
// title puts the panel back where and as large as it first was. Once moved, a panel is placed in the
// window (fixed), not in the image stage, so it can go over the toolbar and the side panel; it is
// kept inside the window. Shares app.js globals: $, pref.
(() => {
  window.TifMrcViewPro.float = (panel, key) => {
    const stage = $("stage");
    const head = panel.querySelector(".al-head");
    // Left, top, width, height in the window; none until the user moves or resizes it. Once moved, the
    // panel is placed in the window, not the stage, so it can go over the toolbar and the side panel.
    let box = pref.get(key, null);
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
      const r = panel.getBoundingClientRect();
      return { x: r.left, y: r.top, w: r.width, h: r.height };
    };
    // Keeps the panel inside the window, whatever the window has become.
    function fit(b) {
      const W = window.innerWidth, H = window.innerHeight;
      b.w = Math.max(MIN_W, Math.min(b.w, W));
      b.h = Math.max(MIN_H, Math.min(b.h, H));
      b.x = Math.max(0, Math.min(b.x, W - b.w));
      b.y = Math.max(0, Math.min(b.y, H - b.h));
      return b;
    }
    function place() {
      if (box && !panel.hidden) {
        fit(box);
        Object.assign(panel.style, { position: "fixed", zIndex: "60", left: `${box.x}px`, top: `${box.y}px`, right: "auto", bottom: "auto", width: `${box.w}px`, height: `${box.h}px` });
      } else if (!box) {
        for (const k of ["position", "zIndex", "left", "top", "right", "bottom", "width", "height"]) panel.style[k] = "";
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
        pref.set(key, box);
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
      pref.set(key, null);
      place();
    });
    for (const g of corners) {
      g.onpointerdown = (e) => drag(e, (f, dx, dy) => {
        const c = g.dataset.corner;
        // The opposite corner stays where it is.
        if (c[1] === "w") { box.x = Math.max(0, Math.min(f.x + dx, f.x + f.w - MIN_W)); box.w = f.x + f.w - box.x; }
        else box.w = Math.max(MIN_W, Math.min(f.w + dx, window.innerWidth - f.x));
        if (c[0] === "n") { box.y = Math.max(0, Math.min(f.y + dy, f.y + f.h - MIN_H)); box.h = f.y + f.h - box.y; }
        else box.h = Math.max(MIN_H, Math.min(f.h + dy, window.innerHeight - f.y));
      });
    }
    new MutationObserver(place).observe(panel, { attributes: true, attributeFilter: ["hidden", "class"] });
    new ResizeObserver(place).observe(stage);
    window.addEventListener("resize", place);
    place();
  };
  const spa = document.getElementById("spa-workspace");
  if (spa) window.TifMrcViewPro.float(spa, "spa-box-win");
})();
