"use strict";
// TifMrcView Pro. Only the Pro build loads this file (see EDITION in app.js);
// the advanced tools hook in from here.
window.TifMrcViewPro = {
  version: 1,
  // Label colours, by (label - 1) % 16 (labels above 255 wrap like in 3D).
  LABEL_COLORS: [
    [230, 25, 75], [60, 180, 75], [255, 225, 25], [0, 130, 200], [245, 130, 48], [145, 30, 180],
    [70, 240, 240], [240, 50, 230], [210, 245, 60], [250, 190, 212], [0, 128, 128], [220, 190, 255],
    [170, 110, 40], [255, 250, 200], [128, 0, 0], [170, 255, 195],
  ],
};
(() => {
  const css = document.createElement("link");
  css.rel = "stylesheet";
  css.href = "pro/pro.css";
  document.head.appendChild(css);
  // Segmentation labels, the 3D view of Z stacks, CTF fitting in the FFT panel, drift correction, diffraction indexing, denoising, missing-wedge correction, spectrum quantification, 4D-STEM and particle statistics (in this order).
  for (const src of ["pro/labels.js", "pro/volume.js", "pro/ctf.js", "pro/align.js", "pro/diffraction.js", "pro/denoise.js", "pro/wedge.js", "pro/spectro.js", "pro/stem4d.js", "pro/particles.js"]) {
    const s = document.createElement("script");
    s.src = src;
    s.async = false;
    document.body.appendChild(s);
  }
})();
