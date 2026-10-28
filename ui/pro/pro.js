"use strict";
// TifMrcView Pro. Only the Pro build loads this file (see EDITION in app.js);
// the advanced tools hook in from here.
window.TifMrcViewPro = { version: 1 };
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
  // 3D view of Z stacks.
  const vol = document.createElement("script");
  vol.src = "pro/volume.js";
  document.body.appendChild(vol);
})();
