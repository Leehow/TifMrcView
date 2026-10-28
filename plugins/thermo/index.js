"use strict";

// Plugin: Thermo Fisher (FEI) electron-microscopy files. The work happens in
// thermo.wasm (built from src/), a sandboxed WebAssembly format plugin the
// app runs when such a file is opened: it reads TIA .ser, Velox .emd and
// Falcon .eer files, and adds the settings stored in Thermo Fisher TIFF and
// MRC files the app opens itself. This script only puts the plugin in the
// plugin list, where it can be switched on and off; it asks for no
// permissions.

TifMrcView.register({ id: "thermo", activate() {} });
