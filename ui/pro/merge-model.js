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
      scaleBar: null,
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

  function fromImage(source, state, name, scaleBar = null) {
    const doc = create(name);
    const region = state.crop || [0, 0, source.info.width, source.info.height];
    const channels = state.chans.flatMap((c, index) => c.visible
      ? [{ index, color: c.color, min: c.auto ? null : c.min, max: c.auto ? null : c.max }] : []);
    const base = layer(source, { z: state.z, t: state.t, gamma: state.gamma,
      ...(channels.length ? { channels } : {}), visible: channels.length > 0,
      x: -region[0], y: -region[1], scaleBar });
    add(doc, base);
    doc.width = region[2]; doc.height = region[3];
    doc.view = { ...state.view, cx: state.view.cx - region[0], cy: state.view.cy - region[1] };
    return doc;
  }

  function add(doc, item) {
    if (!doc.layers.length) { doc.width = item.info.width; doc.height = item.info.height; item.mode = "normal"; }
    else {
      // Match one canvas dimension without cropping or stretching the source.
      const scale = Math.min(doc.width / item.info.width, doc.height / item.info.height);
      item.width = item.info.width * scale; item.height = item.info.height * scale;
      item.x = (doc.width - item.width) / 2; item.y = (doc.height - item.height) / 2;
    }
    doc.layers.push(item);
    doc.selected = item.id;
    doc.dirty = true;
    doc.revision++;
    return item;
  }

  const handles = [["nw", 0, 0], ["n", 0.5, 0], ["ne", 1, 0], ["e", 1, 0.5],
    ["se", 1, 1], ["s", 0.5, 1], ["sw", 0, 1], ["w", 0, 0.5]];

  function resize(rect, handle, dx, dy, proportional = true) {
    const horizontal = /[ew]/.test(handle), vertical = /[ns]/.test(handle);
    let width = Math.max(1, rect.width + (handle.includes("w") ? -dx : horizontal ? dx : 0));
    let height = Math.max(1, rect.height + (handle.includes("n") ? -dy : vertical ? dy : 0));
    if (proportional) {
      const scale = horizontal && vertical
        ? (width * rect.width + height * rect.height) / (rect.width ** 2 + rect.height ** 2)
        : horizontal ? width / rect.width : height / rect.height;
      const k = Math.max(1 / rect.width, 1 / rect.height, scale);
      width = rect.width * k; height = rect.height * k;
    }
    return { width, height,
      x: rect.x + (handle.includes("w") ? rect.width - width : horizontal ? 0 : (rect.width - width) / 2),
      y: rect.y + (handle.includes("n") ? rect.height - height : vertical ? 0 : (rect.height - height) / 2) };
  }

  function scaleBarGeometry(item) {
    const bar = item?.scaleBar;
    if (!bar?.visible || !item.visible || item.opacity <= 0) return null;
    const sx = item.width / item.info.width, sy = item.height / item.info.height;
    const [x, y, width, height] = bar.region;
    return { x: item.x + (x + width - bar.margin - bar.length) * sx,
      y: item.y + (y + height - bar.margin - bar.thickness) * sy,
      width: bar.length * sx, height: bar.thickness * sy, font: bar.font * sy,
      label: bar.label, physicalLength: bar.length * bar.pixelSize.x, unit: bar.pixelSize.unit };
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
      const bar = l.scaleBar;
      if (bar != null) {
        const units = ["Å", "nm", "µm", "mm", "1/µm", "1/nm", "1/Å"];
        if (typeof bar.visible !== "boolean" || !bar.pixelSize || !units.includes(bar.pixelSize.unit)
          || ![bar.pixelSize.x, bar.pixelSize.y, bar.length, bar.font, bar.thickness].every((v) => Number.isFinite(v) && v > 0)
          || !Number.isFinite(bar.margin) || bar.margin < 0 || typeof bar.label !== "string" || bar.label.length > 80
          || !Array.isArray(bar.region) || bar.region.length !== 4 || !bar.region.every(Number.isFinite)
          || bar.region[0] < 0 || bar.region[1] < 0 || bar.region[2] <= 0 || bar.region[3] <= 0) throw new Error("工程比例尺标定无效");
      }
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

  window.TmvMerge = { copy, clamp, layer, create, add, fromImage, reframe, project, validate, setZ, handles, resize, scaleBarGeometry,
    operation: (mode) => ({ normal: "source-over", add: "lighter", screen: "screen" })[mode] || "source-over" };
})();
