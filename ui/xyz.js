"use strict";
// IMOD-style XYZ window. With a Z stack the axis menu gains XYZ: four panes
// on one screen (XZ upper left, XY lower left, YZ lower right, and the
// intersecting slice overview at the upper right). X, Y and Z each have their own
// slider and marker line. A mark drawn on a slice is drawn again where the
// other two planes cut it, and an IMOD contour is drawn where it lies in
// the plane.
(() => {
  const COL = { x: "#cf222e", y: "#1a7f37", z: "#0969da" };
  const DRAW = new Set(["measure", "pen", "segment", "arrow", "rect", "ellipse", "text"]);
  const cache = { xy: null, xz: null, yz: null };
  let drag = null;
  let gen = 0;
  const overview = { xy: null, xz: null, yz: null };

  function info() {
    return S.info;
  }
  function voxel() {
    if (!S.px || !(S.px.x > 0)) return { x: 1, y: 1, z: 1 };
    const x = S.px.x;
    return { x: 1, y: (S.px.y > 0 ? S.px.y : x) / x, z: (S.pz > 0 ? S.pz : x) / x };
  }
  function layout() {
    const { x0, x1, y0, y1 } = freeRect();
    const gap = 8;
    const left = x0 + 6;
    const top = y0 + 6;
    const w = Math.max(40, x1 - left - 6);
    const h = Math.max(40, y1 - top - 6);
    const cw = (w - gap) / 2;
    const ch = (h - gap) / 2;
    return {
      xz: { x: left, y: top, w: cw, h: ch },
      cross: { x: left + cw + gap, y: top, w: cw, h: ch },
      xy: { x: left, y: top + ch + gap, w: cw, h: ch },
      yz: { x: left + cw + gap, y: top + ch + gap, w: cw, h: ch },
    };
  }
  function dims(kind) {
    const i = info();
    if (kind === "xy") return [i.width, i.height];
    if (kind === "xz") return [i.width, i.sizeZ];
    return [i.height, i.sizeZ];
  }
  function axes(kind) {
    if (kind === "xy") return ["x", "y"];
    if (kind === "xz") return ["x", "z"];
    return ["y", "z"];
  }
  // Screen pixels per voxel at zoom 1, so every pane fits.
  function baseStep() {
    const L = layout();
    const v = voxel();
    const fit = (pane, nu, nv, su, sv) => {
      const aw = Math.max(1, pane.w - 12);
      const ah = Math.max(1, pane.h - 32);
      return Math.min(aw / (nu * su), ah / (nv * sv));
    };
    const i = info();
    return Math.min(
      fit(L.xy, i.width, i.height, v.x, v.y),
      fit(L.xz, i.width, i.sizeZ, v.x, v.z),
      fit(L.yz, i.height, i.sizeZ, v.y, v.z),
    );
  }
  function geom(kind) {
    const pane = layout()[kind];
    const [nu, nv] = dims(kind);
    const [au, av] = axes(kind);
    const v = voxel();
    const step = baseStep() * S.xyz.zoom;
    const su = v[au] * step;
    const sv = v[av] * step;
    const panU = S.xyz.pan[au];
    const panV = S.xyz.pan[av];
    const dw = nu * su;
    const dh = nv * sv;
    const ox = pane.x + (pane.w - dw) / 2 + panU * su;
    const oy = pane.y + 20 + (pane.h - 20 - dh) / 2 + panV * sv;
    return {
      pane, su, sv, ox, oy, nu, nv,
      map: (u, vv) => [ox + u * su, oy + vv * sv],
      unmap: (sx, sy) => [(sx - ox) / su, (sy - oy) / sv],
    };
  }
  function local(e) {
    const r = canvas.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  }
  function paneAt(sx, sy) {
    const L = layout();
    for (const k of ["xy", "xz", "yz", "cross"]) {
      const p = L[k];
      if (sx >= p.x && sx < p.x + p.w && sy >= p.y && sy < p.y + p.h) return k;
    }
    return null;
  }
  function lineAt(sx, sy) {
    if (!S.xyz) return null;
    const L = layout();
    const xy = geom("xy");
    const xz = geom("xz");
    const yz = geom("yz");
    const hits = [];
    const consider = (axis, coord, pane, horizontal) => {
      const p = L[pane];
      const inside = sx >= p.x && sx <= p.x + p.w && sy >= p.y && sy <= p.y + p.h;
      if (!inside) return;
      const d = horizontal ? Math.abs(sy - coord) : Math.abs(sx - coord);
      if (d < 7) hits.push({ axis, d });
    };
    consider("x", xy.map(S.xyz.x, 0)[0], "xy", false);
    consider("y", xy.map(0, S.xyz.y)[1], "xy", true);
    consider("x", xz.map(S.xyz.x, 0)[0], "xz", false);
    consider("z", xz.map(0, S.xyz.z)[1], "xz", true);
    consider("y", yz.map(S.xyz.y, 0)[0], "yz", false);
    consider("z", yz.map(0, S.xyz.z)[1], "yz", true);
    hits.sort((a, b) => a.d - b.d);
    return hits.length ? hits[0].axis : null;
  }

  function clamp(v, lo, hi) {
    return Math.max(lo, Math.min(hi, v));
  }
  function syncBars() {
    const i = info();
    const p = S.xyz;
    $("xyz-x").max = Math.max(0, i.width - 1);
    $("xyz-y").max = Math.max(0, i.height - 1);
    $("xyz-x").value = p.x;
    $("xyz-y").value = p.y;
    $("xyz-xl").textContent = `${p.x + 1} / ${i.width}`;
    $("xyz-yl").textContent = `${p.y + 1} / ${i.height}`;
    $("zslider").max = Math.max(0, i.sizeZ - 1);
    $("zslider").value = p.z;
    $("zlabel").textContent = `${p.z + 1} / ${i.sizeZ}`;
  }
  function setPoint(part) {
    const i = info();
    const p = S.xyz;
    const planes = [];
    if (part.x != null) {
      const x = clamp(Math.round(part.x), 0, i.width - 1);
      if (x !== p.x) { p.x = x; planes.push("yz"); }
    }
    if (part.y != null) {
      const y = clamp(Math.round(part.y), 0, i.height - 1);
      if (y !== p.y) { p.y = y; planes.push("xz"); }
    }
    if (part.z != null) {
      const z = clamp(Math.round(part.z), 0, i.sizeZ - 1);
      if (z !== p.z) {
        p.z = z;
        S.z = z;
        planes.push("xy");
        pluginEmit("slice", { z: S.z, t: S.t });
      }
    }
    syncBars();
    paint();
    if (!planes.length) return;
    if (drag) {
      clearTimeout(xyz._t);
      xyz._planes = planes;
      xyz._t = setTimeout(() => reload(xyz._planes), 70);
    } else reload(planes);
  }
  function setFromPointer(axis, sx, sy) {
    const order = axis === "x" ? ["xy", "xz"] : axis === "y" ? ["xy", "yz"] : ["xz", "yz"];
    const pane = order.find((k) => paneAt(sx, sy) === k) || order[0];
    const [u, v] = geom(pane).unmap(sx, sy);
    const [au, av] = axes(pane);
    const part = {};
    if (au === axis) part[axis] = u;
    if (av === axis) part[axis] = v;
    setPoint(part);
  }
  function visible(g, pane) {
    const [nu, nv] = [g.nu, g.nv];
    const u0 = clamp(Math.floor(g.unmap(pane.x, pane.y)[0]), 0, nu - 1);
    const v0 = clamp(Math.floor(g.unmap(pane.x, pane.y)[1]), 0, nv - 1);
    const u1 = clamp(Math.ceil(g.unmap(pane.x + pane.w, pane.y)[0]), u0 + 1, nu);
    const v1 = clamp(Math.ceil(g.unmap(pane.x, pane.y + pane.h)[1]), v0 + 1, nv);
    return [u0, v0, Math.max(1, u1 - u0), Math.max(1, v1 - v0)];
  }

  async function reload(which) {
    if (!S.xyz || !S.info) return;
    const mine = ++gen;
    // A newer request invalidates earlier responses, so request every stale plane.
    const planes = ["xy", "xz", "yz"];
    await Promise.all([
      ...planes.map((kind) => fetchPlane(kind, mine)),
      ...["xy", "xz", "yz"].map((kind) => fetchOverview(kind, mine)),
    ]);
    if (mine === gen) paint();
  }
  async function fetchPlane(kind, mine) {
    const g = geom(kind);
    const region = visible(g, g.pane);
    const dpr = window.devicePixelRatio || 1;
    const outW = Math.max(1, Math.min(2048, Math.round(region[2] * g.su * dpr)));
    const outH = Math.max(1, Math.min(2048, Math.round(region[3] * g.sv * dpr)));
    const index = kind === "xy" ? S.xyz.z : kind === "xz" ? S.xyz.y : S.xyz.x;
    const channels = channelReqs().map((c) => ({ ...c, min: S.chans[c.index].min, max: S.chans[c.index].max }));
    const key = JSON.stringify([S.token, kind, index, S.t, region, outW, outH, channels, S.gamma]);
    if (cache[kind] && cache[kind].key === key) return;
    const req = { token: S.token, z: index, t: S.t, channels, region, outW, outH, gamma: S.gamma, invert: false, plane: kind === "xy" ? "" : kind };
    try {
      const f = parseFrame(await invoke("render_view", { req }));
      if (mine !== gen) return;
      const c = document.createElement("canvas");
      c.width = f.w;
      c.height = f.h;
      c.getContext("2d").putImageData(f.image, 0, 0);
      cache[kind] = { key, canvas: c, region };
    } catch (e) {
      if (mine === gen) toast(`三视图失败：${e}`);
    }
  }

  async function fetchOverview(kind, mine) {
    const [w, h] = dims(kind);
    const index = kind === "xy" ? S.xyz.z : kind === "xz" ? S.xyz.y : S.xyz.x;
    const channels = channelReqs().map((c) => ({ ...c, min: S.chans[c.index].min, max: S.chans[c.index].max }));
    const scale = Math.min(1, 512 / Math.max(w, h));
    const outW = Math.max(1, Math.round(w * scale));
    const outH = Math.max(1, Math.round(h * scale));
    const key = JSON.stringify([S.token, kind, index, S.t, channels, S.gamma, w, h]);
    if (overview[kind]?.key === key) return;
    const req = { token: S.token, z: index, t: S.t, channels, region: [0, 0, w, h], outW, outH, gamma: S.gamma, invert: false, plane: kind === "xy" ? "" : kind };
    try {
      const f = parseFrame(await invoke("render_view", { req }));
      if (mine !== gen || !S.xyz) return;
      const c = document.createElement("canvas");
      c.width = f.w; c.height = f.h;
      c.getContext("2d").putImageData(f.image, 0, 0);
      overview[kind] = { key, canvas: c, index };
    } catch (e) {
      if (mine === gen) toast(`三维切片失败：${e}`);
    }
  }

  // Orthographic projection; split at the intersection so nearer patches
  // occlude farther patches rather than drawing one entire plane on top.
  function drawOverview(pane) {
    const i = info(), v = voxel(), p = S.xyz;
    const size = [i.width * v.x, i.height * v.y, i.sizeZ * v.z];
    const basis = [[0.866, 0.35], [-0.866, 0.35], [0, 0.8]];
    const bw = 0.866 * (size[0] + size[1]);
    const bh = 0.35 * (size[0] + size[1]) + 0.8 * size[2];
    const scale = Math.min(Math.max(1, pane.w - 40) / bw, Math.max(1, pane.h - 55) / bh);
    const project = (q) => [pane.x + pane.w / 2 + scale * q.reduce((a, n, j) => a + (n - size[j] / 2) * basis[j][0], 0),
      pane.y + pane.h / 2 + scale * q.reduce((a, n, j) => a + (n - size[j] / 2) * basis[j][1], 0)];
    const point = [(p.x + 0.5) * v.x, (p.y + 0.5) * v.y, (p.z + 0.5) * v.z];
    const specs = [{ kind: "xy", u: 0, v: 1, color: COL.z },
      { kind: "xz", u: 0, v: 2, color: COL.y },
      { kind: "yz", u: 1, v: 2, color: COL.x }];
    const patches = [];
    for (const spec of specs) {
      const cutsU = [0, point[spec.u], size[spec.u]];
      const cutsV = [0, point[spec.v], size[spec.v]];
      for (let a = 0; a < 2; a++) for (let b = 0; b < 2; b++) {
        const q = [...point]; q[spec.u] = cutsU[a]; q[spec.v] = cutsV[b];
        const u1 = cutsU[a + 1], v1 = cutsV[b + 1];
        const center = [...q]; center[spec.u] = (q[spec.u] + u1) / 2; center[spec.v] = (q[spec.v] + v1) / 2;
        // Camera is above and in front of the volume.
        const depth = center[0] + center[1] - center[2];
        patches.push({ spec, q, u1, v1, depth });
      }
    }
    patches.sort((a, b) => a.depth - b.depth);
    ctx.save(); clip(ctx, pane);
    ctx.fillStyle = "#111"; ctx.fillRect(pane.x, pane.y, pane.w, pane.h);
    for (const { spec, q, u1, v1 } of patches) {
      const candidate = overview[spec.kind];
      const current = spec.kind === "xy" ? p.z : spec.kind === "xz" ? p.y : p.x;
      const im = candidate?.index === current ? candidate : null;
      const qu = [...q], qv = [...q]; qu[spec.u] = u1; qv[spec.v] = v1;
      const [x, y] = project(q), [xu, yu] = project(qu), [xv, yv] = project(qv);
      ctx.save();
      ctx.transform(xu - x, yu - y, xv - x, yv - y, x, y);
      if (im) {
        const sw = im.canvas.width, sh = im.canvas.height;
        ctx.drawImage(im.canvas, q[spec.u] / size[spec.u] * sw, q[spec.v] / size[spec.v] * sh,
          (u1 - q[spec.u]) / size[spec.u] * sw, (v1 - q[spec.v]) / size[spec.v] * sh, 0, 0, 1, 1);
      } else { ctx.fillStyle = "#333"; ctx.fillRect(0, 0, 1, 1); }
      ctx.restore();
      ctx.strokeStyle = spec.color; ctx.lineWidth = 1;
      ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(xu, yu);
      ctx.lineTo(xu + xv - x, yu + yv - y); ctx.lineTo(xv, yv); ctx.closePath(); ctx.stroke();
    }
    const [cx, cy] = project(point);
    ctx.fillStyle = "#fff"; ctx.beginPath(); ctx.arc(cx, cy, 3, 0, 2 * Math.PI); ctx.fill();
    ctx.fillStyle = "#fff"; ctx.font = "12px sans-serif"; ctx.fillText("XYZ", pane.x + 8, pane.y + 16);
    ctx.restore();
    ctx.strokeStyle = COL.z; ctx.lineWidth = 2;
    ctx.strokeRect(pane.x + 1, pane.y + 1, pane.w - 2, pane.h - 2);
  }

  function clip(ctx, pane) {
    ctx.beginPath();
    ctx.rect(pane.x, pane.y, pane.w, pane.h);
    ctx.clip();
  }
  function strokeLine(ctx, color, x0, y0, x1, y1) {
    ctx.save();
    ctx.strokeStyle = color;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(x0, y0);
    ctx.lineTo(x1, y1);
    ctx.stroke();
    ctx.restore();
  }
  function handle(ctx, color, x, y) {
    ctx.save();
    ctx.fillStyle = color;
    ctx.fillRect(x - 4, y - 4, 8, 8);
    ctx.restore();
  }

  // Where a 2D mark on slices z0..z1 meets the plane x=X or y=Y.
  function zOf(a) {
    const z0 = a.z ?? 0;
    const z1 = a.z1 ?? z0;
    return [Math.min(z0, z1), Math.max(z0, z1)];
  }
  function spanOf(a, axis, value) {
    if (!a.pts || a.pts.length < 2) return [];
    if (a.type === "rect") return rectSpan(a, axis, value);
    if (a.type === "ellipse") return ellipseSpan(a, axis, value);
    const spans = [];
    const pts = a.pts;
    for (let i = 1; i < pts.length; i++) {
      const hit = cross(pts[i - 1], pts[i], axis, value);
      if (!hit) continue;
      if (hit.span) spans.push(hit.span);
      else spans.push([hit.u, hit.u]);
    }
    return spans;
  }
  function cross(p, q, axis, value) {
    const ui = axis === "y" ? 0 : 1;
    const vi = axis === "y" ? 1 : 0;
    const dv = q[vi] - p[vi];
    if (Math.abs(dv) < 1e-6) {
      if (Math.abs(p[vi] - value) > 0.51) return null;
      return { span: [Math.min(p[ui], q[ui]), Math.max(p[ui], q[ui])] };
    }
    const t = (value - p[vi]) / dv;
    if (t < -1e-4 || t > 1 + 1e-4) return null;
    return { u: p[ui] + t * (q[ui] - p[ui]) };
  }
  function rectSpan(a, axis, value) {
    const x0 = Math.min(a.pts[0][0], a.pts[1][0]);
    const x1 = Math.max(a.pts[0][0], a.pts[1][0]);
    const y0 = Math.min(a.pts[0][1], a.pts[1][1]);
    const y1 = Math.max(a.pts[0][1], a.pts[1][1]);
    if (axis === "y") return value >= y0 && value <= y1 ? [[x0, x1]] : [];
    return value >= x0 && value <= x1 ? [[y0, y1]] : [];
  }
  function ellipseSpan(a, axis, value) {
    const cx = (a.pts[0][0] + a.pts[1][0]) / 2;
    const cy = (a.pts[0][1] + a.pts[1][1]) / 2;
    const rx = Math.abs(a.pts[1][0] - a.pts[0][0]) / 2;
    const ry = Math.abs(a.pts[1][1] - a.pts[0][1]) / 2;
    if (rx < 1e-6 || ry < 1e-6) return [];
    if (axis === "y") {
      const n = (value - cy) / ry;
      if (n < -1 || n > 1) return [];
      const d = rx * Math.sqrt(Math.max(0, 1 - n * n));
      return [[cx - d, cx + d]];
    }
    const n = (value - cx) / rx;
    if (n < -1 || n > 1) return [];
    const d = ry * Math.sqrt(Math.max(0, 1 - n * n));
    return [[cy - d, cy + d]];
  }
  function drawCut(ctx, a, kind, map) {
    const axis = kind === "xz" ? "y" : "x";
    const value = kind === "xz" ? S.xyz.y : S.xyz.x;
    const [z0, z1] = zOf(a);
    ctx.save();
    ctx.strokeStyle = a.color || COL.z;
    ctx.fillStyle = ctx.strokeStyle;
    ctx.lineWidth = a.width || 2;
    ctx.lineCap = "round";
    if (a.type === "text" && a.pts[0]) {
      const ok = axis === "y" ? Math.abs(a.pts[0][1] - value) < 0.51 : Math.abs(a.pts[0][0] - value) < 0.51;
      if (ok) {
        const u = axis === "y" ? a.pts[0][0] : a.pts[0][1];
        const [x, y] = map(u, (z0 + z1) / 2);
        ctx.font = "12px sans-serif";
        ctx.fillText(a.text || "", x + 4, y);
      }
      ctx.restore();
      return;
    }
    for (const [u0, u1] of spanOf(a, axis, value)) {
      const [x0, y0] = map(u0, z0);
      const [x1, y1] = map(u1, z1);
      ctx.beginPath();
      if (Math.abs(u1 - u0) < 0.5 && z0 === z1) {
        ctx.arc(x0, y0, 3, 0, Math.PI * 2);
        ctx.fill();
      } else if (Math.abs(u1 - u0) < 0.5) {
        ctx.moveTo(x0, y0);
        ctx.lineTo(x0, y1);
        ctx.stroke();
      } else {
        ctx.strokeRect(Math.min(x0, x1), Math.min(y0, y1), Math.abs(x1 - x0), Math.abs(y1 - y0));
      }
    }
    ctx.restore();
  }
  function marks() {
    const list = S.annShow ? S.ann.filter((a) => a && a.type !== "crop") : [];
    if (S.op && S.op.kind === "draw" && S.op.item) list.push(S.op.item);
    return list;
  }
  function drawModelOn(ctx, kind, map) {
    const m = S.model;
    if (!m || !S.modelShow) return;
    const axis = kind === "xy" ? "z" : kind === "xz" ? "y" : "x";
    const value = S.xyz[axis];
    const comp = { x: 0, y: 1, z: 2 }[axis];
    const u = kind === "yz" ? 1 : 0;
    const v = kind === "xy" ? 1 : 2;
    ctx.save();
    ctx.lineWidth = 1.5;
    for (const o of m.objects) {
      if (!o.visible) continue;
      ctx.strokeStyle = `rgb(${o.color.join(",")})`;
      for (const c of o.contours) {
        const p = c.pts;
        const n = p.length / 3;
        ctx.beginPath();
        let pen = false;
        for (let i = 0; i < n; i++) {
          if (Math.abs(p[i * 3 + comp] - value) > 0.51) { pen = false; continue; }
          const [sx, sy] = map(p[i * 3 + u], p[i * 3 + v]);
          if (pen) ctx.lineTo(sx, sy);
          else ctx.moveTo(sx, sy);
          pen = true;
        }
        ctx.stroke();
      }
    }
    ctx.restore();
  }

  function paint() {
    if (!S.xyz || !S.info) return;
    const dpr = window.devicePixelRatio || 1;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const { cw, ch } = cssSize();
    ctx.fillStyle = "#d8dbe0";
    ctx.fillRect(0, 0, cw, ch);
    const L = layout();
    S.view.scale = geom("xy").su || 1;
    for (const kind of ["xz", "xy", "yz"]) {
      const g = geom(kind);
      const pane = g.pane;
      ctx.save();
      clip(ctx, pane);
      ctx.fillStyle = "#111";
      ctx.fillRect(pane.x, pane.y, pane.w, pane.h);
      const im = cache[kind];
      if (im) {
        const [sx, sy] = g.map(im.region[0], im.region[1]);
        ctx.imageSmoothingEnabled = g.su < 1;
        ctx.drawImage(im.canvas, sx, sy, im.region[2] * g.su, im.region[3] * g.sv);
      }
      if (kind === "xy") {
        const items = marks().filter((a) => onSlice(a, S.xyz.z));
        drawAnnotations(ctx, g.map, 1, items);
        drawModel(ctx, (x, y) => g.map(x + 0.5, y + 0.5), g.su, 1, S.xyz.z);
      } else {
        for (const a of marks()) drawCut(ctx, a, kind, g.map);
        drawModelOn(ctx, kind, g.map);
      }
      ctx.restore();
      ctx.save();
      ctx.strokeStyle = kind === "xy" ? COL.z : kind === "xz" ? COL.y : COL.x;
      ctx.lineWidth = 2;
      ctx.strokeRect(pane.x + 1, pane.y + 1, pane.w - 2, pane.h - 2);
      ctx.font = "12px sans-serif";
      const label = kind.toUpperCase();
      const tw = ctx.measureText(label).width;
      ctx.fillStyle = "rgba(0,0,0,.55)";
      ctx.fillRect(pane.x + 4, pane.y + 4, tw + 10, 16);
      ctx.fillStyle = "#fff";
      ctx.fillText(label, pane.x + 8, pane.y + 16);
      ctx.restore();
    }
    const xy = geom("xy");
    const xz = geom("xz");
    const yz = geom("yz");
    const [xv] = xy.map(S.xyz.x, 0);
    const [, yh] = xy.map(0, S.xyz.y);
    const [xv2] = xz.map(S.xyz.x, 0);
    const [, zh] = xz.map(0, S.xyz.z);
    const [yv] = yz.map(S.xyz.y, 0);
    const [, zh2] = yz.map(0, S.xyz.z);
    strokeLine(ctx, COL.x, xv, L.xy.y, xv, L.xy.y + L.xy.h);
    strokeLine(ctx, COL.y, L.xy.x, yh, L.xy.x + L.xy.w, yh);
    handle(ctx, COL.z, xv, yh);
    strokeLine(ctx, COL.x, xv2, L.xz.y, xv2, L.xz.y + L.xz.h);
    strokeLine(ctx, COL.z, L.xz.x, zh, L.xz.x + L.xz.w, zh);
    handle(ctx, COL.y, xv2, zh);
    strokeLine(ctx, COL.y, yv, L.yz.y, yv, L.yz.y + L.yz.h);
    strokeLine(ctx, COL.z, L.yz.x, zh2, L.yz.x + L.yz.w, zh2);
    handle(ctx, COL.x, yv, zh2);

    drawOverview(L.cross);
  }

  function zoomAt(k, sx, sy, pane) {
    const g = geom(pane);
    const [u, v] = g.unmap(sx, sy);
    S.xyz.zoom = clamp(S.xyz.zoom * k, 0.05, 64);
    const g2 = geom(pane);
    const [u2, v2] = g2.unmap(sx, sy);
    const [au, av] = axes(pane);
    S.xyz.pan[au] += u2 - u;
    S.xyz.pan[av] += v2 - v;
    paint();
    reload();
  }

  const xyz = {
    toggle() {
      if (S.xyz) this.close();
      else this.open();
    },
    open() {
      if (!S.info) return;
      const depth = (S.base ? S.base.info : S.info).sizeZ;
      if (depth < 2) return;
      if (S.axis !== "xy") {
        setAxis("xy").then(() => { if (!S.xyz) this.open(); });
        return;
      }
      stopPlay();
      S.xyz = {
        x: Math.floor(S.info.width / 2),
        y: Math.floor(S.info.height / 2),
        z: S.z,
        zoom: 1,
        pan: { x: 0, y: 0, z: 0 },
      };
      $("xyz-axes").hidden = false;
      $("axis-label").textContent = "XYZ";
      syncBars();
      cache.xy = cache.xz = cache.yz = null;
      overview.xy = overview.xz = overview.yz = null;
      reload();
      paint();
    },
    close(silent) {
      if (!S.xyz) return;
      S.xyz = null;
      drag = null;
      gen++;
      $("xyz-axes").hidden = true;
      if ($("axis-label") && S.axis) $("axis-label").textContent = S.axis.toUpperCase();
      if (!silent) {
        draw();
        requestRender();
      }
    },
    layout() {
      paint();
      reload();
    },
    moved() {
      syncBars();
      paint();
      reload(["xy"]);
    },
    fit() {
      if (!S.xyz) return;
      S.xyz.zoom = 1;
      S.xyz.pan = { x: 0, y: 0, z: 0 };
      paint();
      reload();
    },
    zoom(k) {
      if (!S.xyz) return;
      const L = layout();
      zoomAt(k, L.xy.x + L.xy.w / 2, L.xy.y + L.xy.h / 2, "xy");
    },
    reload() { reload(); },
    paint,
    toVolume(e) {
      const [sx, sy] = local(e);
      return geom("xy").unmap(sx, sy);
    },
    pointer(phase, e) {
      if (!S.xyz || !e) return false;
      const [sx, sy] = local(e);
      if (phase === "down") {
        if (e.button === 2) return false;
        const pane = paneAt(sx, sy);
        if (!pane) return false;
        const axis = lineAt(sx, sy);
        if (axis) {
          drag = { kind: "line", axis };
          canvas.setPointerCapture?.(e.pointerId);
          return true;
        }
        if (pane === "cross") return true;
        if (pane === "xy" && e.button === 0 && (DRAW.has(S.tool) || (S.tool === "select" && annAt(geom("xy").unmap(sx, sy), HIT / (geom("xy").su || 1))))) return false;
        drag = { kind: "nav", pane, cx: e.clientX, cy: e.clientY, moved: false, pan: { ...S.xyz.pan } };
        canvas.setPointerCapture?.(e.pointerId);
        return true;
      }
      if (phase === "move") {
        if (!drag) return false;
        if (drag.kind === "line") {
          setFromPointer(drag.axis, sx, sy);
          return true;
        }
        if (Math.hypot(e.clientX - drag.cx, e.clientY - drag.cy) > 4) drag.moved = true;
        if (drag.moved) {
          const dx = e.clientX - drag.cx;
          const dy = e.clientY - drag.cy;
          const step = baseStep() * S.xyz.zoom;
          const v = voxel();
          const p = { ...drag.pan };
          const shift = (axis, pixels, phys) => { p[axis] = drag.pan[axis] + pixels / (phys * step); };
          if (drag.pane === "xy" || drag.pane === "xz") shift("x", dx, v.x);
          if (drag.pane === "xy" || drag.pane === "yz") shift("y", dy, v.y);
          if (drag.pane === "xz" || drag.pane === "yz" || drag.pane === "cross") shift("z", dy, v.z);
          S.xyz.pan = p;
          paint();
          clearTimeout(xyz._t);
          xyz._t = setTimeout(() => reload(), 80);
        }
        return true;
      }
      if (!drag) return false;
      const d = drag;
      drag = null;
      if (d.kind === "nav" && !d.moved) {
        const g = geom(d.pane);
        const [u, v] = g.unmap(sx, sy);
        if (d.pane === "xy") setPoint({ x: u, y: v });
        else if (d.pane === "xz") setPoint({ x: u, z: v });
        else if (d.pane === "yz") setPoint({ y: u, z: v });
      } else reload();
      return true;
    },
    wheel(e) {
      const [sx, sy] = local(e);
      const pane = paneAt(sx, sy) || "xy";
      if ((IS_MAC ? e.metaKey : e.ctrlKey) || (e.shiftKey && !e.ctrlKey)) {
        const dir = Math.sign(e.deltaY || e.deltaX) || 1;
        if (pane === "xz") setPoint({ y: S.xyz.y + dir });
        else if (pane === "yz") setPoint({ x: S.xyz.x + dir });
        else setPoint({ z: S.xyz.z + dir });
        return true;
      }
      if (pane === "cross") return true;
      zoomAt(Math.exp(-e.deltaY * 0.0025), sx, sy, pane);
      return true;
    },
  };

  $("xyz-x").addEventListener("input", () => S.xyz && setPoint({ x: +$("xyz-x").value }));
  $("xyz-y").addEventListener("input", () => S.xyz && setPoint({ y: +$("xyz-y").value }));
  new ResizeObserver(() => { if (S.xyz) { paint(); reload(); } }).observe(canvas);
  window.xyz = xyz;
})();
