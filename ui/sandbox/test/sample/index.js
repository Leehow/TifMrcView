"use strict";

// A hostile plugin can bypass the friendly wrapper; tests exercise the same
// port directly to prove that permission checks live in the host.
const originalPostMessage = MessagePort.prototype.postMessage;
MessagePort.prototype.postMessage = function (...args) {
  window.testPort = this;
  return originalPostMessage.apply(this, args);
};

// Test fixture only. Never installed or included in plugin_scan by the app.
TifMrcView.register({
  id: "sandbox-sample",
  async activate(app) {
    window.testApp = app; // browser tests can exercise this frame's public API
    const state = await app.state();
    const report = (name, value) => app.image.addMetadata(state.path, [{ name, items: [["value", String(value)]] }]);
    await app.ui.addToolbarButton({ label: "Sandbox test", title: "Sandbox callback", onClick: () => report("toolbar-click", true) });
    await app.ui.addToolSection({ title: "Sandbox panel", items: [
      { type: "text", text: "Sandbox text" },
      { type: "button", label: "Panel action", onClick: () => report("panel-click", true) },
      { type: "number", label: "Count", value: 2, min: 0, max: 10, onChange: (value) => report("number-change", value) },
      { type: "select", label: "Mode", options: [["a", "A"], ["b", "B"]], onChange: (value) => report("select-change", value) },
      { type: "checkbox", label: "Enabled", value: true, onChange: (value) => report("checkbox-change", value) },
    ] });
    await app.ui.showTools();
    await report("metadata-write", true);
    const probes = {
      "parent.document": () => window.parent.document.body,
      "top.location.read": () => top.location.href,
      "top.location.write": () => { top.location = "https://example.com/escape"; },
      fetch: () => fetch("https://example.com"),
      indexedDB: () => new Promise((resolve, reject) => {
        const request = indexedDB.open("sandbox-probe");
        request.onsuccess = resolve; request.onerror = () => reject(request.error);
      }),
      localStorage: () => localStorage.setItem("sandbox-probe", "bad"),
      cookie: () => { document.cookie = "sandbox-probe=bad"; return document.cookie; },
    };
    for (const [name, probe] of Object.entries(probes)) {
      let blocked = false;
      try { await probe(); } catch (_) { blocked = true; }
      await report("probe:" + name, blocked);
    }
    await report("probe:__TAURI__", window.__TAURI__ === undefined);
    await report("probe:__TAURI_INTERNALS__", window.__TAURI_INTERNALS__ === undefined);
    try { await app.settings.set("undeclared", true); await report("probe:permission", false); }
    catch (e) { await report("probe:permission", /Undeclared permission/.test(e.message)); }
    await app.on("slice", ({ z, t }) => report("slice-event", `${z},${t}`));
    await app.on("open", (info) => report("open-event", info.path));
    const png = await app.renderRegion([0, 0, 2, 2], { size: [2, 2] });
    const path = await app.dialog.save({ name: "sandbox.png", exts: ["png"] });
    if (path) await app.fs.write(path, png);
  },
});
