"use strict";

// Plugin: Leica LAS X / LAS AF files (.lif projects and .lof exports). The
// work happens in lasx.wasm (built from src/lib.rs), a sandboxed WebAssembly
// format plugin the app runs when a .lif or .lof file is opened: it indexes
// the memory blocks, hands the planes to the app to read, and turns the
// embedded XML into pixel size, channels and acquisition settings. This
// script only puts the plugin in the plugin list, where it can be switched
// on and off.

TifMrcView.register({
  id: "lasx",
  name: "Leica LAS X",
  version: "1.0.0",
  description: "打开徕卡 LAS X / LAS AF 保存的 LIF 项目和 LOF 文件，并显示物镜、扫描、探测器和激光等采集参数",
  activate() {},
});
