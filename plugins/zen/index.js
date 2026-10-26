"use strict";

// Plugin: Zeiss ZEN CZI files (LSM / Airyscan confocals, Axio Imager,
// Celldiscoverer…). The work happens in zen.wasm (built from src/lib.rs), a
// sandboxed WebAssembly format plugin the app runs when a .czi file is
// opened: it indexes the file, hands the tiles to the app to decode, and
// turns the embedded XML into pixel size, channels and acquisition settings.
// This script only puts the plugin in the plugin list, where it can be
// switched on and off.

TifMrcView.register({
  id: "zen",
  name: "Zeiss ZEN",
  version: "2.0.0",
  description: "打开蔡司 ZEN 保存的 CZI，并显示物镜、激光、探测器、针孔、扫描和 Airyscan 处理等采集参数",
  activate() {},
});
