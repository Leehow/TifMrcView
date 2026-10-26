"use strict";

// Plugin: Nikon NIS-Elements ND2 files (widefield, spinning disk, A1/AX
// confocals, colour cameras…). The work happens in nis.wasm (built from
// src/lib.rs), a sandboxed WebAssembly format plugin the app runs when an
// .nd2 file is opened: it reads the chunk map, hands the frames to the app
// to decode, offers each stage position as its own image, and turns the
// metadata into pixel size, channels and acquisition settings. This script
// only puts the plugin in the plugin list, where it can be switched on and off.

TifMrcView.register({
  id: "nis",
  name: "Nikon NIS-Elements",
  version: "1.0.0",
  description: "打开尼康 NIS-Elements 保存的 ND2，支持多位置、Z 栈、时间序列和多通道，并显示物镜、相机、激光和滤镜等采集参数",
  activate() {},
});
