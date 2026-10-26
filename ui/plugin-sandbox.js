"use strict";

// API 2 is deliberately separate from makeApi's trusted, synchronous API 1.
// Every callable method, including event subscriptions, has one permission here.
const SANDBOX_PERMISSIONS = Object.freeze({
  state: "state", on: "state", off: "state", setZ: "state", setT: "state",
  "ui.toast": "ui", "ui.addToolbarButton": "ui", "ui.addToolSection": "ui", "ui.showTools": "ui",
  "image.addMetadata": "metadata", "image.setPixelSize": "metadata", "image.setFormat": "metadata",
  "settings.get": "settings", "settings.set": "settings",
  renderRegion: "export", "dialog.save": "export", "fs.write": "export",
});
const SANDBOX_MAX_BYTES = 16 * 1024 * 1024;
const SANDBOX_JSON_BYTES = 1024 * 1024;

// Count structured data before dispatch, including binary buffers. Reject cycles,
// unusual prototypes and excessive nesting rather than coercing hostile values.
function sandboxPayload(value, limit = SANDBOX_MAX_BYTES) {
  let bytes = 0;
  const seen = new Set();
  function visit(v, depth) {
    if (depth > 32) throw new Error("Payload nesting limit exceeded");
    if (v == null || typeof v === "boolean" || typeof v === "number") bytes += 8;
    else if (typeof v === "string") bytes += v.length * 2;
    else if (v instanceof ArrayBuffer || ArrayBuffer.isView(v)) bytes += v.byteLength;
    else if (typeof v === "object") {
      if (seen.has(v)) throw new Error("Cyclic payload");
      if (!Array.isArray(v) && Object.getPrototypeOf(v) !== Object.prototype) throw new Error("Invalid payload");
      seen.add(v);
      for (const [k, item] of Object.entries(v)) { bytes += k.length * 2; visit(item, depth + 1); }
      seen.delete(v);
    } else if (v !== undefined) throw new Error("Invalid payload");
    if (bytes > limit) throw new Error("Payload size limit exceeded");
  }
  visit(value, 0);
}

function sandboxText(value, max = 2048) {
  if (typeof value !== "string" || value.length > max) throw new Error("Invalid text");
  return value;
}

function sandboxNumber(value) {
  if (typeof value !== "number" || !Number.isFinite(value)) throw new Error("Invalid number");
  return value;
}

function sandboxPanel(api, desc, send) {
  sandboxText(desc.title);
  if (!Array.isArray(desc.items) || desc.items.length > 100) throw new Error("Invalid panel items");
  // Build offscreen first: an invalid item must not leave a partial panel behind.
  const fragment = document.createDocumentFragment();
  for (const item of desc.items) {
    const row = document.createElement("div");
    row.className = "row";
    if (item.type === "text") {
      const text = document.createElement("span");
      text.className = "dim";
      text.textContent = sandboxText(item.text);
      row.append(text);
    } else {
      const label = sandboxText(item.label);
      const callback = sandboxText(item.callback, 128);
      const el = document.createElement(item.type === "button" ? "button" : item.type === "select" ? "select" : "input");
      if (item.type === "button") {
        el.className = "small";
        el.textContent = label;
        el.addEventListener("click", () => send("ui.callback", { callback }));
        row.append(el);
      } else {
        if (item.type === "select") {
          if (!Array.isArray(item.options) || item.options.length > 100) throw new Error("Invalid select options");
          for (const option of item.options) {
            if (!Array.isArray(option) || option.length !== 2) throw new Error("Invalid option");
            el.append(new Option(sandboxText(option[1]), sandboxText(option[0])));
          }
          if (item.value !== undefined) el.value = sandboxText(item.value);
        } else if (item.type === "number") {
          el.type = "number";
          for (const key of ["value", "min", "max", "step"]) {
            if (item[key] !== undefined) el[key] = sandboxNumber(item[key]);
          }
        } else if (item.type === "checkbox") {
          el.type = "checkbox";
          el.checked = !!item.value;
        } else throw new Error("Unknown panel item");
        const wrap = document.createElement("label");
        wrap.append(document.createTextNode(label + " "), el);
        row.append(wrap);
        el.addEventListener("change", () => {
          const value = item.type === "checkbox" ? el.checked : item.type === "number" ? el.valueAsNumber : el.value;
          send("ui.callback", { callback, value });
        });
      }
    }
    fragment.append(row);
  }
  api.ui.addToolSection(desc.title).append(fragment);
}

async function activateSandbox(rec) {
  if (rec.active) return;
  rec.active = true;
  rec.error = null;
  const api = makeApi(rec);
  const permissions = new Set(Array.isArray(rec.def.permissions) ? rec.def.permissions : []);
  const subscriptions = new Map();
  const frame = document.createElement("iframe");
  frame.hidden = true;
  frame.dataset.pluginSandbox = rec.def.id;
  frame.setAttribute("sandbox", "allow-scripts");
  frame.setAttribute("referrerpolicy", "no-referrer");
  // A protocol-served document does not inherit the parent's script policy.
  // Its CSP permits only the runtime and this package's main, without inline
  // scripts/eval. Parent frame-src permits ONLY this reserved route, including
  // subsequent navigation, so self-navigation cannot turn into network/IPC access.
  frame.src = userPluginUrl(`__sandbox/${rec.def.id}`);
  const channel = new MessageChannel();
  let disposed = false, selectedPath = null, picking = false, uiCount = 0, inFlight = 0;
  let timer, resolveReady, rejectReady;
  const ready = new Promise((resolve, reject) => { resolveReady = resolve; rejectReady = reject; });
  const send = (event, data) => {
    if (disposed) return;
    try {
      sandboxPayload(data);
      // Never wait for plugin callbacks; they run in the frame's event queue.
      channel.port1.postMessage({ event, data });
    } catch (e) { console.warn(`plugin ${rec.def.id} event`, e); }
  };
  const session = {
    dispose() {
      disposed = true;
      clearTimeout(timer);
      window.removeEventListener("message", handshake);
      channel.port1.close();
      channel.port2.close();
      frame.remove();
      selectedPath = null;
      rejectReady(new Error("Sandbox stopped"));
    },
  };
  rec.sandboxSession = session;

  async function dispatch(method, args) {
    if (typeof method !== "string" || !Object.hasOwn(SANDBOX_PERMISSIONS, method)) throw new Error("Unknown method");
    if (!permissions.has(SANDBOX_PERMISSIONS[method])) throw new Error("Undeclared permission: " + SANDBOX_PERMISSIONS[method]);
    if (!Array.isArray(args)) throw new Error("args must be an array");
    const [a, b] = args;
    switch (method) {
      case "state": return api.state();
      case "on": {
        if (!["open", "slice", "render"].includes(a)) throw new Error("Unknown event");
        if (!subscriptions.has(a)) {
          subscriptions.set(a, api.on(a, (data) => send(a, data)));
          if (a === "open" && S.info) send("open", S.info);
        }
        return;
      }
      case "off":
        if (subscriptions.has(a)) { subscriptions.get(a)(); subscriptions.delete(a); }
        return;
      case "setZ": case "setT":
        if (!Number.isInteger(a)) throw new Error("Slice must be an integer");
        if (S.info) api[method](a);
        return;
      case "ui.toast":
        api.ui.toast(sandboxText(a), b === undefined ? undefined : Math.max(0, Math.min(10000, sandboxNumber(b))));
        return;
      case "ui.showTools": return api.ui.showTools();
      case "ui.addToolbarButton":
        if (uiCount >= 32) throw new Error("UI element limit exceeded");
        api.ui.addToolbarButton({
          label: sandboxText(a.label), title: a.title === undefined ? "" : sandboxText(a.title),
          onClick: (() => { const callback = sandboxText(a.callback, 128); return () => send("ui.callback", { callback }); })(),
        });
        uiCount++;
        return;
      case "ui.addToolSection":
        if (uiCount >= 32) throw new Error("UI element limit exceeded");
        sandboxPanel(api, a, send);
        uiCount++;
        return;
      case "image.addMetadata":
        sandboxText(a);
        if (!Array.isArray(b) || b.length > 100) throw new Error("Invalid metadata");
        for (const group of b) {
          sandboxText(group.name);
          if (!Array.isArray(group.items) || group.items.length > 1000) throw new Error("Invalid metadata items");
          for (const pair of group.items) {
            if (!Array.isArray(pair) || pair.length !== 2) throw new Error("Invalid metadata item");
            pair.forEach((text) => sandboxText(text));
          }
        }
        return api.image.addMetadata(a, b);
      case "image.setPixelSize":
        sandboxText(a); sandboxNumber(b.x);
        if (b.y !== undefined) sandboxNumber(b.y);
        sandboxText(b.unit, 16);
        return api.image.setPixelSize(a, b);
      case "image.setFormat": return api.image.setFormat(sandboxText(a), sandboxText(b));
      case "settings.get": case "settings.set":
        sandboxText(a, 128);
        return method === "settings.get" ? api.settings.get(a, b) : api.settings.set(a, b);
      case "dialog.save": {
        if (picking) throw new Error("Save dialog already open");
        const opts = { name: sandboxText(a.name) };
        if (a.filter !== undefined) opts.filter = sandboxText(a.filter);
        if (a.exts !== undefined) {
          if (!Array.isArray(a.exts) || a.exts.length > 20) throw new Error("Invalid extensions");
          opts.exts = a.exts.map((ext) => sandboxText(ext, 32));
        }
        // Plugins cannot suggest arbitrary directories. Cancellation revokes the
        // previous selection; each successful selection authorizes one write.
        selectedPath = null;
        picking = true;
        try {
          const path = await api.dialog.save(opts);
          if (disposed) return null;
          selectedPath = path || null;
          return selectedPath;
        } finally { picking = false; }
      }
      case "fs.write": {
        if (typeof a !== "string" || !selectedPath || a !== selectedPath) throw new Error("Path was not selected in the save dialog");
        if (!(b instanceof Uint8Array)) throw new Error("Expected PNG bytes (Uint8Array)");
        selectedPath = null; // consume before awaiting, including on failure
        return api.fs.write(a, b);
      }
      case "renderRegion": {
        if (!S.info) throw new Error("No image open");
        if (!Array.isArray(a) || a.length !== 4) throw new Error("Invalid region");
        a.forEach(sandboxNumber);
        if (a[2] <= 0 || a[3] <= 0) throw new Error("Invalid region size");
        const opts = b || {};
        for (const key of ["z", "t"]) {
          if (opts[key] !== undefined && (!Number.isInteger(opts[key]) || opts[key] < 0 || opts[key] >= S.info[key === "z" ? "sizeZ" : "sizeT"])) throw new Error("Invalid slice");
        }
        const size = opts.size || [Math.min(4096, Math.max(1, Math.round(a[2]))), Math.min(4096, Math.max(1, Math.round(a[3])))];
        if (!Array.isArray(size) || size.length !== 2 || size.some((n) => !Number.isInteger(n) || n <= 0 || n > 4096) || size[0] * size[1] > 4 * 1024 * 1024) throw new Error("Render size limit exceeded");
        const canvas = await api.renderRegion(a, { z: opts.z, t: opts.t, size });
        const blob = await new Promise((resolve) => canvas.toBlob(resolve, "image/png"));
        if (!blob || blob.size > SANDBOX_MAX_BYTES) throw new Error("PNG size limit exceeded");
        return new Uint8Array(await blob.arrayBuffer());
      }
    }
  }

  channel.port1.onmessage = async ({ data: message }) => {
    if (disposed) return;
    // Bound transport/script startup, but let activate() await a real user
    // dialog without killing the frame when the user takes more than 10 s.
    if (message && message.event === "$loaded") {
      clearTimeout(timer);
      return;
    }
    if (message && message.event === "$activated") {
      clearTimeout(timer);
      resolveReady();
      return;
    }
    if (message && message.event === "$error") {
      rejectReady(new Error(typeof message.data === "string" ? message.data.slice(0, 2048) : "Plugin activation failed"));
      return;
    }
    const id = message && message.id;
    if (!Number.isSafeInteger(id) || id < 1) return;
    let reply;
    let counted = false;
    try {
      sandboxPayload(message, message.method === "fs.write" ? SANDBOX_MAX_BYTES : SANDBOX_JSON_BYTES);
      if (inFlight >= 16) throw new Error("Too many pending calls");
      inFlight++; counted = true;
      const result = await dispatch(message.method, message.args);
      sandboxPayload(result);
      reply = { id, result };
    } catch (e) { reply = { id, error: String(e?.message || e).slice(0, 2048) }; }
    finally { if (counted) inFlight--; }
    if (!disposed) channel.port1.postMessage(reply);
  };
  function handshake(event) {
    if (event.source !== frame.contentWindow || !event.data || event.data.type !== "tmv-plugin-ready") return;
    window.removeEventListener("message", handshake);
    frame.contentWindow.postMessage({ type: "tmv-plugin-connect", id: rec.def.id, appVersion: pluginHost.appVersion }, "*", [channel.port2]);
  }
  window.addEventListener("message", handshake);
  timer = setTimeout(() => rejectReady(new Error("Sandbox activation timed out")), 10000);
  document.body.append(frame);
  try {
    await ready;
  } catch (e) {
    if (rec.sandboxSession === session) {
      deactivatePlugin(rec);
      rec.error = String(e);
      console.error(`plugin ${rec.def.id}`, e);
    }
  }
  renderPluginList();
}
