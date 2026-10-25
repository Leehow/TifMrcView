"use strict";

// Plugin: Zeiss SmartSEM TIFFs (Zeiss SEM / FIB-SEM such as Crossbeam,
// Gemini, Sigma). Reads the CZ_SEM tag through the native half
// (src-tauri/src/plugins/smartsem.rs), shows every acquisition setting in the
// image-info panel and uses the calibrated pixel size for the scale bar.

TifMrcView.register({
  id: "smartsem",
  name: "Zeiss SmartSEM",
  version: "1.0.0",
  description: "读取蔡司 SmartSEM 保存的 TIF：显示电压、工作距离、放大倍数、探测器、样品台等全部采集参数，并按校准的像素尺寸显示比例尺",

  activate(app) {
    if (!app.hasNative()) throw new Error("这个版本没有编译 SmartSEM 支持");
    app.on("open", async (info) => {
      if (!/\.tiff?$/i.test(info.path)) return;
      let r;
      try {
        r = await app.native("read", { path: info.path, width: info.width });
      } catch (e) {
        console.warn("smartsem", e);
        return;
      }
      if (!r) return;
      app.image.setFormat(info.path, "Zeiss SmartSEM TIFF");
      if (r.pixelNm > 0) app.image.setPixelSize(info.path, { x: r.pixelNm, y: r.pixelNm, unit: "nm" });
      app.image.addMetadata(info.path, r.groups);
    });
  },
});
