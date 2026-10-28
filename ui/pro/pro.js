"use strict";
// TifMrcView Pro. Only the Pro build loads this file (see EDITION in app.js);
// the advanced tools (3D view, segmentation) hook in from here.
(() => {
  const css = document.createElement("link");
  css.rel = "stylesheet";
  css.href = "pro/pro.css";
  document.head.appendChild(css);
  const badge = document.createElement("span");
  badge.id = "pro-badge";
  badge.textContent = "Pro";
  badge.title = "TifMrcView Pro";
  document.getElementById("toolbar").prepend(badge);
})();
window.TifMrcViewPro = { version: 1 };
