"use strict";

// Plugin: Zeiss SmartSEM TIFFs (Zeiss SEM / FIB-SEM such as Crossbeam,
// Gemini, Sigma). The work happens in smartsem.wasm (built from src/lib.rs),
// a sandboxed WebAssembly metadata plugin the app runs when a .tif opens: it
// reads the CZ_SEM tag and adds every acquisition setting, the calibrated
// pixel size and the format name to the image info. This script only puts
// the plugin in the plugin list, where it can be switched on and off.

TifMrcView.register({
  id: "smartsem",
  name: "Zeiss SmartSEM",
  version: "2.0.0",
  description: "读取蔡司 SmartSEM 保存的 TIF：显示电压、工作距离、放大倍数、探测器、样品台等全部采集参数，并按校准的像素尺寸显示比例尺",
  activate() {},
});
