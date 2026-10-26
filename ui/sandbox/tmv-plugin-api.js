"use strict";

// This file runs only inside the opaque-origin frame. It has no Tauri imports.
(() => {
  let definition, port, nextId = 0, nextCallback = 0;
  const pending = new Map(), callbacks = new Map(), events = new Map();
  const call = (method, ...args) => new Promise((resolve, reject) => {
    if (!port) return reject(new Error("Plugin bridge is not connected"));
    if (pending.size >= 16) return reject(new Error("Too many pending calls"));
    const id = ++nextId;
    pending.set(id, { resolve, reject });
    try { port.postMessage({ id, method, args }); }
    catch (e) { pending.delete(id); reject(e); }
  });
  const callback = (fn) => {
    if (typeof fn !== "function") throw new Error("Expected callback");
    const id = String(++nextCallback);
    callbacks.set(id, fn);
    return id;
  };
  const run = (fn, value) => {
    // Host events are notifications, never RPCs that wait on plugin code.
    Promise.resolve().then(() => fn(value)).catch((e) => console.error("Plugin callback", e));
  };
  function makeApp(appVersion) {
    return {
      apiVersion: 2, appVersion,
      state: () => call("state"),
      async on(event, fn) {
        if (typeof fn !== "function") throw new Error("Expected callback");
        if (!events.has(event)) events.set(event, new Set());
        const set = events.get(event);
        set.add(fn);
        try { await call("on", event); }
        catch (e) { set.delete(fn); throw e; }
        return async () => {
          set.delete(fn);
          if (!set.size) await call("off", event);
        };
      },
      setZ: (z) => call("setZ", z),
      setT: (t) => call("setT", t),
      renderRegion: (region, opts) => call("renderRegion", region, opts),
      ui: {
        toast: (msg, ms) => call("ui.toast", msg, ms),
        showTools: () => call("ui.showTools"),
        async addToolbarButton({ label, title, onClick }) {
          const id = callback(onClick);
          try { await call("ui.addToolbarButton", { label, title, callback: id }); }
          catch (e) { callbacks.delete(id); throw e; }
        },
        async addToolSection({ title, items }) {
          const ids = [];
          try {
            const description = items.map((item) => {
              const { onClick, onChange, ...data } = item;
              if (item.type !== "text") {
                data.callback = callback(item.type === "button" ? onClick : onChange);
                ids.push(data.callback);
              }
              return data;
            });
            await call("ui.addToolSection", { title, items: description });
          } catch (e) { ids.forEach((id) => callbacks.delete(id)); throw e; }
        },
      },
      image: {
        addMetadata: (path, groups) => call("image.addMetadata", path, groups),
        setPixelSize: (path, size) => call("image.setPixelSize", path, size),
        setFormat: (path, name) => call("image.setFormat", path, name),
      },
      settings: {
        get: (key, fallback) => call("settings.get", key, fallback),
        set: (key, value) => call("settings.set", key, value),
      },
      dialog: { save: (options) => call("dialog.save", options) },
      fs: { write: (path, bytes) => call("fs.write", path, bytes) },
    };
  }
  window.TifMrcView = Object.freeze({
    apiVersion: 2,
    register(def) {
      if (definition) throw new Error("Plugin already registered");
      if (!def || def.id !== document.documentElement.dataset.plugin || typeof def.activate !== "function") throw new Error("Plugin needs a matching id and activate()");
      definition = def;
    },
  });
  function connect(event) {
    if (event.source !== window.parent || event.data?.type !== "tmv-plugin-connect" || !event.ports[0]) return;
    window.removeEventListener("message", connect);
    port = event.ports[0];
    port.onmessage = ({ data }) => {
      if (!data) return;
      if (data.event === "ui.callback") {
        const fn = callbacks.get(data.data?.callback);
        if (fn) run(fn, data.data.value);
      } else if (data.event) {
        for (const fn of events.get(data.event) || []) run(fn, data.data);
      } else {
        const request = pending.get(data.id);
        if (!request) return;
        pending.delete(data.id);
        if (Object.hasOwn(data, "error")) request.reject(new Error(data.error));
        else request.resolve(data.result);
      }
    };
    Promise.resolve().then(async () => {
      // Some WebView versions inject native startup scripts into subframes.
      // Fail closed BEFORE loading third-party code if that happened. CSP alone
      // cannot remove a native-injected IPC object (which can be nonconfigurable).
      if (window.__TAURI__ !== undefined || window.__TAURI_INTERNALS__ !== undefined) throw new Error("Native APIs are exposed in this frame; refusing to load plugin");
      await new Promise((resolve, reject) => {
        const script = document.createElement("script");
        script.src = document.documentElement.dataset.main;
        script.onload = resolve;
        script.onerror = () => reject(new Error("Cannot load plugin main script"));
        document.head.append(script);
      });
      if (!definition || definition.id !== event.data.id) throw new Error("Plugin did not register its package id");
      port.postMessage({ event: "$loaded" });
      return definition.activate(makeApp(event.data.appVersion));
    }).then(
      () => port.postMessage({ event: "$activated" }),
      (e) => port.postMessage({ event: "$error", data: String(e) }),
    );
  }
  window.addEventListener("message", connect);
  // Connect before fetching any third-party code, so startup failures can be
  // reported and the native-API guard runs before the plugin's main script.
  window.addEventListener("load", () => window.parent.postMessage({ type: "tmv-plugin-ready" }, "*"), { once: true });
})();
