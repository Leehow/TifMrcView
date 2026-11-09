"use strict";
// Versioned editing state shared by the layer UI, renderer and saved project.
(() => {
  const copy = (value) => JSON.parse(JSON.stringify(value));
  const finite = (value, fallback) => Number.isFinite(value) ? value : fallback;
  const clamp = (value, lo, hi) => Math.max(lo, Math.min(hi, value));
  const modes = new Set(["normal", "add", "screen"]);
  let serial = 0;

  function layer(source, settings = {}) {
    const info = source.info;
    const out = {
      id: `layer-${++serial}`, token: source.token, info,
      path: info.path, series: info.seriesIndex || 0, name: info.name,
      visible: true, opacity: 1, mode: "screen", color: null,
      x: 0, y: 0, width: info.width, height: info.height,
      z: Math.floor(info.sizeZ / 2), t: 0, gamma: 1,
      channels: info.channels.map((c, index) => ({ index, color: c.color, min: null, max: null })),
      ...copy(settings),
    };
    // Runtime identity/metadata always belong to the source opened this time.
    Object.assign(out, { token: source.token, info, path: info.path, series: info.seriesIndex || 0 });
    out.id = `layer-${++serial}`;
    out.z = clamp(Math.round(finite(out.z, 0)), 0, info.sizeZ - 1);
    out.t = clamp(Math.round(finite(out.t, 0)), 0, info.sizeT - 1);
    return out;
  }

  function create(name = "合成图") {
    return { type: "merge", name, width: 0, height: 0, background: "#000000", layers: [],
      selected: null, dirty: true, path: null, revision: 0, view: null };
  }

  // Changing sidebar/Z-bar layout must not move an image that is being
  // converted in place. Canvas coordinates use the centre of the viewport.
  function reframe(view, before, after) {
    return { ...view,
      cx: view.cx + (after.width - before.width) / (2 * view.scale),
      cy: view.cy + (after.height - before.height) / (2 * view.scale) };
  }

  function fromImage(source, state, name) {
    const doc = create(name);
    const region = state.crop || [0, 0, source.info.width, source.info.height];
    const channels = state.chans.flatMap((c, index) => c.visible
      ? [{ index, color: c.color, min: c.auto ? null : c.min, max: c.auto ? null : c.max }] : []);
    const base = layer(source, { z: state.z, t: state.t, gamma: state.gamma,
      ...(channels.length ? { channels } : {}), visible: channels.length > 0,
      x: -region[0], y: -region[1] });
    add(doc, base);
    doc.width = region[2]; doc.height = region[3];
    doc.view = { ...state.view, cx: state.view.cx - region[0], cy: state.view.cy - region[1] };
    return doc;
  }

  function add(doc, item) {
    if (!doc.layers.length) { doc.width = item.info.width; doc.height = item.info.height; item.mode = "normal"; }
    doc.layers.push(item);
    doc.selected = item.id;
    doc.dirty = true;
    doc.revision++;
    return item;
  }

  function project(doc) {
    return {
      format: "tifmrcview-merge", version: 1, name: doc.name,
      width: doc.width, height: doc.height, background: doc.background,
      selected: doc.layers.findIndex((l) => l.id === doc.selected), view: doc.view ? copy(doc.view) : null,
      layers: doc.layers.map((l) => {
        const { token, info, id, ...settings } = l;
        return copy(settings);
      }),
    };
  }

  function validate(data) {
    if (data?.format !== "tifmrcview-merge" || data.version !== 1) throw new Error("不支持的合成工程格式或版本");
    if (!Number.isInteger(data.width) || !Number.isInteger(data.height) || data.width < 1 || data.height < 1 || data.width > 1000000 || data.height > 1000000) throw new Error("工程画布尺寸无效");
    if (!Array.isArray(data.layers) || data.layers.length > 256) throw new Error("工程图层列表无效");
    if (!/^#[0-9a-f]{6}$/i.test(data.background)) throw new Error("工程背景颜色无效");
    for (const l of data.layers) {
      if (typeof l.path !== "string" || !l.path || typeof l.name !== "string") throw new Error("工程缺少图层来源");
      if (!modes.has(l.mode) || typeof l.visible !== "boolean" || !Number.isFinite(l.opacity) || l.opacity < 0 || l.opacity > 1) throw new Error("工程图层显示设置无效");
      for (const key of ["x", "y", "width", "height", "gamma"]) if (!Number.isFinite(l[key])) throw new Error("工程图层几何或亮度设置无效");
      if (l.width <= 0 || l.height <= 0 || l.gamma <= 0) throw new Error("工程图层大小或 gamma 无效");
      if (l.color !== null && !/^#[0-9a-f]{6}$/i.test(l.color)) throw new Error("工程图层颜色无效");
      for (const key of ["z", "t", "series"]) if (!Number.isSafeInteger(l[key]) || l[key] < 0) throw new Error("工程图层 Z/T 设置无效");
      if (!Array.isArray(l.channels) || !l.channels.length || l.channels.length > 256) throw new Error("工程通道设置无效");
      for (const c of l.channels) {
        if (!Number.isInteger(c.index) || c.index < 0 || !Array.isArray(c.color) || c.color.length !== 3 || c.color.some((v) => !Number.isInteger(v) || v < 0 || v > 255)) throw new Error("工程通道颜色无效");
        for (const key of ["min", "max"]) if (c[key] !== null && !Number.isFinite(c[key])) throw new Error("工程通道显示范围无效");
      }
    }
    if (data.view && (![data.view.cx, data.view.cy, data.view.scale].every(Number.isFinite) || data.view.scale <= 0)) throw new Error("工程视图无效");
    return copy(data);
  }

  // A 2D layer remains at Z=0, independently of any other stack's depth.
  function setZ(item, z) { item.z = clamp(Math.round(z), 0, item.info.sizeZ - 1); }

  window.TmvMerge = { copy, clamp, layer, create, add, fromImage, reframe, project, validate, setZ,
    operation: (mode) => ({ normal: "source-over", add: "lighter", screen: "screen" })[mode] || "source-over" };
})();
