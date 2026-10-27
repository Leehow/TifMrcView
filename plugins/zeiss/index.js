"use strict";

// Plugin: every Zeiss format. The work happens in zeiss.wasm (built from
// src/), a sandboxed WebAssembly plugin the app runs when a file opens:
// CZI from ZEN and LSM from ZEN black / AIM are indexed and handed to the app
// to decode, and the CZ_SEM tag of SmartSEM TIFFs becomes acquisition
// settings and a calibrated pixel size. This script only puts the plugin in
// the plugin list, where it can be switched on and off; it asks for no
// permissions.

TifMrcView.register({ id: "zeiss", activate() {} });
