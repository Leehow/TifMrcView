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
  // Segmentation labels, the 3D view of Z stacks, CTF fitting in the FFT panel, drift correction, the Cryo-ET workspace, the single-particle workspace, diffraction indexing, denoising, missing-wedge correction, spectrum quantification, 4D-STEM and particle statistics (in this order).
  for (const src of ["pro/labels.js", "pro/volume.js", "pro/ctf.js", "pro/align.js", "pro/cryoet/index.js", "pro/cryoet/motion.js", "pro/cryoet/ctf.js", "pro/cryoet/align.js", "pro/cryoet/recon.js", "pro/cryoet/post.js", "pro/spa/index.js", "pro/spa/mics.js", "pro/spa/ctf.js", "pro/spa/pick.js", "pro/spa/class2d.js", "pro/spa/initial.js", "pro/spa/refine.js", "pro/spa/auto.js", "pro/diffraction.js", "pro/denoise.js", "pro/wedge.js", "pro/spectro.js", "pro/stem4d.js", "pro/particles.js"]) {
    const s = document.createElement("script");
    s.src = src;
    s.async = false;
    document.body.appendChild(s);
  }
})();
// No GPU found (no or very old graphics driver, a VM without a GPU): Pro
// still works on the CPU, only slower. Say so once, with the driver pages;
// there is nothing to choose or set up in the app.
setTimeout(async () => {
  if (localStorage.getItem("tmv-gpu-hint") || !window.__TAURI__) return;
  let info;
  try {
    info = await window.__TAURI__.core.invoke("gpu_info");
  } catch {
    return;
  }
  if (info.backend !== "CPU") return;
  localStorage.setItem("tmv-gpu-hint", "1");
  const bar = document.createElement("div");
  bar.className = "pro-gpu-hint";
  bar.append("没有找到可用的显卡加速，Pro 的计算会在 CPU 上进行，速度会慢很多。安装或更新显卡驱动后重新打开即可自动使用显卡：");
  for (const [name, url] of [["NVIDIA", "https://www.nvidia.com/drivers"], ["AMD", "https://www.amd.com/support/download/drivers.html"], ["Intel", "https://www.intel.com/content/www/us/en/download-center/home.html"]]) {
    const a = document.createElement("a");
    a.href = url;
    a.textContent = name;
    a.addEventListener("click", (e) => {
      e.preventDefault();
      window.__TAURI__.core.invoke("open_link", { url }).catch(() => {});
    });
    bar.append(" ", a);
  }
  const close = document.createElement("button");
  close.textContent = "×";
  close.title = "关闭";
  close.addEventListener("click", () => bar.remove());
  bar.append(close);
  document.body.appendChild(bar);
}, 8000);
