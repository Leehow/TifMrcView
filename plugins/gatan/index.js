"use strict";

// Plugin: Gatan DigitalMicrograph / GMS files (.dm3 and .dm4). The work
// happens in gatan.wasm (built from src/), a sandboxed WebAssembly format
// plugin the app runs when such a file is opened: it walks the tag tree,
// hands the pixel array to the app to read, and turns the calibrations and
// image tags into pixel size and acquisition settings. This script only
// puts the plugin in the plugin list, where it can be switched on and off;
// it asks for no permissions.

TifMrcView.register({ id: "gatan", activate() {} });
