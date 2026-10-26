"use strict";

// Plugin: Zeiss ZEN black / AIM LSM files (LSM 510 / 710 / 780 / 880 / 900 /
// 980 confocals). The work happens in zenblack.wasm (built from src/lib.rs),
// a sandboxed WebAssembly format plugin the app runs when a .lsm file is
// opened: it indexes the TIFF underneath, hands the strips to the app to
// decode, and turns the private CZ_LSMINFO structures into pixel size,
// channels and acquisition settings. This script only puts the plugin in the
// plugin list, where it can be switched on and off.

TifMrcView.register({
  id: "zenblack",
  name: "Zeiss ZEN black",
  version: "1.0.0",
  description: "打开蔡司 ZEN black / AIM 保存的 LSM，并显示物镜、激光、检测器、针孔和各轨道的采集参数",
  activate() {},
});
