"use strict";

// Plugin: TVIPS camera files. The work happens in tvips.wasm (built from
// src/), a sandboxed WebAssembly format plugin the app runs when such a file
// is opened: it reads .tvips movie streams recorded by TVIPS cameras (EMMENU,
// EM-TOOLS) and adds the microscope and camera settings EM-MENU stores in
// its TIFFs. This script only puts the plugin in the plugin list, where it
// can be switched on and off; it asks for no permissions.

TifMrcView.register({ id: "tvips", activate() {} });
