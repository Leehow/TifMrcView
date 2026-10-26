"use strict";

// Plugin: Olympus / Evident FluoView confocal files (OIB from the FV1000 /
// FV1200, OIR from the FV3000 / FV4000). The work happens in fluoview.wasm
// (built from src/), a sandboxed WebAssembly format plugin the app runs when
// an .oib or .oir file is opened: it indexes the file, hands the pixel
// strips to the app to read, and turns the settings into pixel size,
// channels and acquisition settings. This script only puts the plugin in
// the plugin list, where it can be switched on and off.

TifMrcView.register({
  id: "fluoview",
  name: "Olympus FluoView",
  version: "1.0.0",
  description: "打开奥林巴斯 / Evident FluoView 共聚焦的 OIB 和 OIR 文件，并显示物镜、扫描、激光、探测器和针孔等采集参数",
  activate() {},
});
