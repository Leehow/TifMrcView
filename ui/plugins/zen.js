"use strict";

// Built-in plugin: Zeiss ZEN CZI files (LSM / Airyscan confocals, Axio
// Imager, Celldiscoverer…). The core reader decodes the pixels; this plugin
// reads the embedded XML through the native half (src-tauri/src/plugins/zen.rs)
// and shows the acquisition settings in the image-info panel.

TifMrcView.register({
  id: "zen",
  name: "Zeiss ZEN",
  version: "1.0.0",
  description: "读取蔡司 ZEN 保存的 CZI：显示物镜、激光、探测器、针孔、扫描和 Airyscan 处理等采集参数",

  activate(app) {
    if (!app.hasNative()) throw new Error("这个版本没有编译 ZEN 支持");
    app.on("open", async (info) => {
      if (!/\.czi$/i.test(info.path)) return;
      let r;
      try {
        r = await app.native("read", { path: info.path });
      } catch (e) {
        console.warn("zen", e);
        return;
      }
      if (!r) return;
      app.image.setFormat(info.path, r.version ? `Zeiss ZEN CZI (ZEN ${r.version})` : "Zeiss ZEN CZI");
      app.image.addMetadata(info.path, r.groups);
    });
  },
});
