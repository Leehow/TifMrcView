"use strict";

// Plugin: Imaris .ims files (Oxford Instruments / Bitplane Imaris; also
// written by Andor Dragonfly, Fusion and ImarisFileConverter). The work
// happens in imaris.wasm (built from src/lib.rs), a sandboxed WebAssembly
// format plugin the app runs when an .ims file is opened: it reads the HDF5
// structure, hands the compressed chunks to the app to decode, and turns the
// stored attributes into pixel size, channels and acquisition settings.
// This script only puts the plugin in the plugin list, where it can be
// switched on and off.

TifMrcView.register({
  id: "imaris",
  name: "Imaris",
  version: "1.0.0",
  description: "打开 Imaris（.ims）文件，并显示像素尺寸、通道颜色与波长、采集时间等参数",
  activate() {},
});
