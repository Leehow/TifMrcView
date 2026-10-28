"use strict";
// Pro 3D view: the current Z stack as a WebGL2 volume (ray marching).
// Shares app.js globals: S (viewer state), invoke, $.
(() => {
  const MAX_CHANNELS = 4;
  const VOXEL_BUDGET = 96 << 20; // all channels together

  const VERT = `#version 300 es
in vec2 pos;
out vec2 ndc;
void main() { ndc = pos; gl_Position = vec4(pos, 0.0, 1.0); }`;

  const FRAG = `#version 300 es
precision highp float;
precision highp sampler3D;
in vec2 ndc;
out vec4 outColor;
uniform mat4 invViewProj;
uniform vec3 extent;      // half size of the box
uniform sampler3D vol[${MAX_CHANNELS}];
uniform vec3 color[${MAX_CHANNELS}];
uniform int nch;
uniform int mode;         // 0 MIP, 1 translucent, 2 surface
uniform float threshold;
uniform float opacity;
uniform float brightness;
uniform int steps;
uniform vec3 texel;       // 1 / voxel counts

float sampleCh(int c, vec3 t) {
  // Indexing a sampler array needs a constant index in GLSL ES.
  if (c == 0) return texture(vol[0], t).r;
  if (c == 1) return texture(vol[1], t).r;
  if (c == 2) return texture(vol[2], t).r;
  return texture(vol[3], t).r;
}

vec3 toTex(vec3 p) {
  vec3 t = p / extent * 0.5 + 0.5;
  return vec3(t.x, 1.0 - t.y, t.z);
}

void main() {
  vec4 a = invViewProj * vec4(ndc, -1.0, 1.0);
  vec4 b = invViewProj * vec4(ndc, 1.0, 1.0);
  vec3 ro = a.xyz / a.w;
  vec3 rd = normalize(b.xyz / b.w - ro);
  vec3 inv = 1.0 / rd;
  vec3 t0 = (-extent - ro) * inv, t1 = (extent - ro) * inv;
  vec3 tmin = min(t0, t1), tmax = max(t0, t1);
  float tn = max(max(tmin.x, tmin.y), max(tmin.z, 0.0));
  float tf = min(min(tmax.x, tmax.y), tmax.z);
  if (tf <= tn) { outColor = vec4(0.0); return; }
  float dt = (tf - tn) / float(steps);
  vec3 acc = vec3(0.0);
  float alpha = 0.0;
  float mx[${MAX_CHANNELS}];
  for (int c = 0; c < ${MAX_CHANNELS}; c++) mx[c] = 0.0;
  for (int i = 0; i < 2048; i++) {
    if (i >= steps) break;
    vec3 tc = toTex(ro + rd * (tn + (float(i) + 0.5) * dt));
    if (mode == 0) {
      for (int c = 0; c < ${MAX_CHANNELS}; c++) if (c < nch) mx[c] = max(mx[c], sampleCh(c, tc));
    } else if (mode == 1) {
      vec3 col = vec3(0.0);
      float a1 = 0.0;
      for (int c = 0; c < ${MAX_CHANNELS}; c++) {
        if (c >= nch) break;
        float v = sampleCh(c, tc);
        float w = smoothstep(threshold, 1.0, v);
        col += color[c] * v;
        a1 = max(a1, w);
      }
      a1 = 1.0 - pow(1.0 - clamp(a1 * opacity, 0.0, 1.0), dt * 200.0);
      acc += (1.0 - alpha) * a1 * col * brightness;
      alpha += (1.0 - alpha) * a1;
      if (alpha > 0.98) break;
    } else {
      for (int c = 0; c < ${MAX_CHANNELS}; c++) {
        if (c >= nch) break;
        if (sampleCh(c, tc) > threshold) {
          vec3 g = vec3(
            sampleCh(c, tc + vec3(texel.x, 0, 0)) - sampleCh(c, tc - vec3(texel.x, 0, 0)),
            sampleCh(c, tc - vec3(0, texel.y, 0)) - sampleCh(c, tc + vec3(0, texel.y, 0)),
            sampleCh(c, tc + vec3(0, 0, texel.z)) - sampleCh(c, tc - vec3(0, 0, texel.z)));
          vec3 n = length(g) > 1e-5 ? -normalize(g) : -rd;
          float diff = abs(dot(n, -rd));
          outColor = vec4(color[c] * (0.25 + 0.75 * diff) * brightness, 1.0);
          return;
        }
      }
    }
  }
  if (mode == 0) {
    vec3 col = vec3(0.0);
    for (int c = 0; c < ${MAX_CHANNELS}; c++) if (c < nch) col += color[c] * mx[c];
    outColor = vec4(col * brightness, 1.0);
  } else {
    outColor = vec4(acc, 1.0);
  }
}`;

  // --- 4x4 matrices, column-major like WebGL ---
  const mul = (a, b) => {
    const o = new Float32Array(16);
    for (let i = 0; i < 4; i++) for (let j = 0; j < 4; j++) {
      let s = 0;
      for (let k = 0; k < 4; k++) s += a[k * 4 + j] * b[i * 4 + k];
      o[i * 4 + j] = s;
    }
    return o;
  };
  const perspective = (fovy, aspect, near, far) => {
    const f = 1 / Math.tan(fovy / 2), nf = 1 / (near - far);
    return new Float32Array([f / aspect, 0, 0, 0, 0, f, 0, 0, 0, 0, (far + near) * nf, -1, 0, 0, 2 * far * near * nf, 0]);
  };
  const lookAt = (eye) => {
    const len = (v) => Math.hypot(...v);
    const norm = (v) => v.map((x) => x / len(v));
    const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    const z = norm(eye);
    const x = norm(cross([0, 1, 0], z));
    const y = cross(z, x);
    return new Float32Array([x[0], y[0], z[0], 0, x[1], y[1], z[1], 0, x[2], y[2], z[2], 0, -dot(x, eye), -dot(y, eye), -dot(z, eye), 1]);
  };
  const invert = (m) => {
    const inv = new Float32Array(16);
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
    const det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    return inv.map((v) => v / det);
  };

  /// Parses the reply of the `volume_data` command (see volume.rs).
  function parseVolume(buf) {
    const dv = new DataView(buf);
    const [w, h, d, n] = [0, 4, 8, 12].map((o) => dv.getUint32(o, true));
    const voxel = [16, 20, 24].map((o) => dv.getFloat32(o, true));
    const chans = [];
    let off = 28;
    for (let c = 0; c < n; c++) {
      const min = dv.getFloat32(off, true), max = dv.getFloat32(off + 4, true);
      off += 8;
      chans.push({ min, max, data: new Uint8Array(buf, off, w * h * d) });
      off += w * h * d;
    }
    return { w, h, d, voxel, chans };
  }

  class VolumeView {
    constructor(canvas) {
      this.canvas = canvas;
      const gl = canvas.getContext("webgl2", { antialias: false, preserveDrawingBuffer: true });
      if (!gl) throw new Error("这台电脑的显卡不支持 WebGL2，无法显示 3D");
      this.gl = gl;
      const sh = (type, src) => {
        const s = gl.createShader(type);
        gl.shaderSource(s, src);
        gl.compileShader(s);
        if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
        return s;
      };
      const prog = gl.createProgram();
      gl.attachShader(prog, sh(gl.VERTEX_SHADER, VERT));
      gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, FRAG));
      gl.linkProgram(prog);
      if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog));
      this.prog = prog;
      gl.useProgram(prog);
      const vbo = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
      const loc = gl.getAttribLocation(prog, "pos");
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
      this.u = {};
      for (const name of ["invViewProj", "extent", "nch", "mode", "threshold", "opacity", "brightness", "steps", "texel"]) {
        this.u[name] = gl.getUniformLocation(prog, name);
      }
      gl.uniform1iv(gl.getUniformLocation(prog, "vol"), [0, 1, 2, 3]);
      this.uColor = gl.getUniformLocation(prog, "color");
      this.textures = [];
      this.settings = { mode: 0, threshold: 0.25, opacity: 0.2, brightness: 1.2, zScale: 1 };
      this.resetView();
    }

    resetView() {
      this.yaw = 0.6;
      this.pitch = 0.35;
      this.dist = 2.6;
      this.draw();
    }

    setVolume(vol, colors) {
      const gl = this.gl;
      this.textures.forEach((t) => gl.deleteTexture(t));
      this.textures = [];
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      vol.chans.slice(0, MAX_CHANNELS).forEach((c, i) => {
        const tex = gl.createTexture();
        gl.activeTexture(gl.TEXTURE0 + i);
        gl.bindTexture(gl.TEXTURE_3D, tex);
        gl.texImage3D(gl.TEXTURE_3D, 0, gl.R8, vol.w, vol.h, vol.d, 0, gl.RED, gl.UNSIGNED_BYTE, c.data);
        gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
        for (const p of [gl.TEXTURE_WRAP_S, gl.TEXTURE_WRAP_T, gl.TEXTURE_WRAP_R]) gl.texParameteri(gl.TEXTURE_3D, p, gl.CLAMP_TO_EDGE);
        this.textures.push(tex);
      });
      // Unused samplers still need a valid texture bound.
      for (let i = this.textures.length; i < MAX_CHANNELS; i++) {
        gl.activeTexture(gl.TEXTURE0 + i);
        gl.bindTexture(gl.TEXTURE_3D, this.textures[0]);
      }
      this.vol = vol;
      this.colors = colors.slice(0, MAX_CHANNELS);
      this.draw();
    }

    extent() {
      const v = this.vol;
      const size = [v.w * v.voxel[0], v.h * v.voxel[1], v.d * v.voxel[2] * this.settings.zScale];
      const m = Math.max(...size);
      return size.map((s) => (0.5 * s) / m);
    }

    draw(fast = false) {
      if (this.frame) return;
      this.frame = requestAnimationFrame(() => {
        this.frame = 0;
        this.render(fast);
      });
    }

    render(fast) {
      const gl = this.gl, c = this.canvas;
      const dpr = window.devicePixelRatio || 1;
      const scale = fast ? 0.5 : 1;
      const w = Math.max(1, Math.round(c.clientWidth * dpr * scale));
      const h = Math.max(1, Math.round(c.clientHeight * dpr * scale));
      if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
      gl.viewport(0, 0, w, h);
      gl.clearColor(0, 0, 0, 1);
      gl.clear(gl.COLOR_BUFFER_BIT);
      if (!this.vol) return;
      const s = this.settings;
      const eye = [
        this.dist * Math.cos(this.pitch) * Math.sin(this.yaw),
        this.dist * Math.sin(this.pitch),
        this.dist * Math.cos(this.pitch) * Math.cos(this.yaw),
      ];
      const vp = mul(perspective(0.7, w / h, 0.05, 20), lookAt(eye));
      gl.useProgram(this.prog);
      gl.uniformMatrix4fv(this.u.invViewProj, false, invert(vp));
      gl.uniform3fv(this.u.extent, this.extent());
      gl.uniform1i(this.u.nch, this.textures.length);
      gl.uniform1i(this.u.mode, s.mode);
      gl.uniform1f(this.u.threshold, s.threshold);
      gl.uniform1f(this.u.opacity, s.opacity);
      gl.uniform1f(this.u.brightness, s.brightness);
      const v = this.vol;
      const full = Math.min(2048, Math.round(1.5 * Math.max(v.w, v.h, v.d)));
      gl.uniform1i(this.u.steps, fast ? Math.max(64, full >> 2) : full);
      gl.uniform3fv(this.u.texel, [1 / v.w, 1 / v.h, 1 / v.d]);
      const cols = new Float32Array(MAX_CHANNELS * 3);
      this.colors.forEach((rgb, i) => cols.set(rgb.map((x) => x / 255), i * 3));
      gl.uniform3fv(this.uColor, cols);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    }

    dispose() {
      this.textures.forEach((t) => this.gl.deleteTexture(t));
      this.textures = [];
      this.vol = null;
    }
  }

  // --- UI: quick toolbar button, overlay canvas and settings panel ---
  const btn = document.createElement("button");
  btn.id = "btn-3d";
  btn.title = "3D 视图：把 Z 堆栈显示成可以旋转的三维体（拖动旋转，滚轮缩放，双击复位）";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><path d="M12 2l9 5v10l-9 5-9-5V7z"/><path d="M3 7l9 5 9-5M12 12v10"/></svg>';
  btn.disabled = true;
  // Right after the tool buttons, so it stays visible in narrow windows.
  $("toolbtns").after(btn);

  const stage = $("stage");
  const canvas3d = document.createElement("canvas");
  canvas3d.id = "vol3d";
  canvas3d.hidden = true;
  stage.appendChild(canvas3d);

  const panel = document.createElement("div");
  panel.id = "vol-panel";
  panel.hidden = true;
  panel.innerHTML = `
    <div class="vp-title">3D</div>
    <label>显示方式
      <select id="vp-mode">
        <option value="0">最大强度投影</option>
        <option value="1">半透明</option>
        <option value="2">表面</option>
      </select>
    </label>
    <label id="vp-th-row">阈值 <input type="range" id="vp-th" min="0" max="1" step="0.01" value="0.25"></label>
    <label id="vp-op-row">不透明度 <input type="range" id="vp-op" min="0.02" max="1" step="0.01" value="0.2"></label>
    <label>亮度 <input type="range" id="vp-br" min="0.2" max="4" step="0.05" value="1.2"></label>
    <label>Z 比例 <input type="range" id="vp-z" min="0.1" max="5" step="0.05" value="1"></label>
    <div class="vp-row">
      <button id="vp-reset">复位视角</button>
      <button id="vp-reload" title="按 2D 视图当前的通道和对比度重新读取">刷新</button>
    </div>
    <div class="vp-status" id="vp-status"></div>`;
  stage.appendChild(panel);

  let view = null;
  let shownToken = 0;
  const status = (text) => { $("vp-status").textContent = text; };

  function syncRows() {
    const mode = view ? view.settings.mode : 0;
    $("vp-th-row").hidden = mode === 0;
    $("vp-op-row").hidden = mode !== 1;
  }

  async function load() {
    const chans = S.chans.map((c, i) => ({ c, i })).filter(({ c }) => c.visible).slice(0, MAX_CHANNELS);
    if (!chans.length) { status("没有显示中的通道"); return; }
    const token = S.token;
    const req = {
      token, t: S.t,
      channels: chans.map(({ c, i }) => ({ index: i, min: c.min ?? null, max: c.max ?? null })),
      maxVoxels: Math.floor(VOXEL_BUDGET / chans.length),
    };
    status("正在读取整个 Z 堆栈…");
    const started = performance.now();
    try {
      const vol = parseVolume(await invoke("volume_data", { req }));
      if (token !== S.token || !view) return;
      view.setVolume(vol, chans.map(({ c }) => c.color));
      shownToken = token;
      const secs = ((performance.now() - started) / 1000).toFixed(1);
      status(`${vol.w} × ${vol.h} × ${vol.d} 体素 · ${secs} 秒`);
    } catch (e) {
      status(String(e));
    }
  }

  function open() {
    if (!view) {
      try {
        view = new VolumeView(canvas3d);
      } catch (e) {
        alert(String(e.message || e));
        return;
      }
    }
    canvas3d.hidden = false;
    panel.hidden = false;
    btn.classList.add("on");
    syncRows();
    view.draw();
    load();
  }

  function close() {
    canvas3d.hidden = true;
    panel.hidden = true;
    btn.classList.remove("on");
    if (view) view.dispose();
    shownToken = 0;
  }

  btn.addEventListener("click", () => (canvas3d.hidden ? open() : close()));

  // Enable the button for stacks; leave 3D when another image opens.
  setInterval(() => {
    const stack = !!(S.info && S.info.sizeZ > 1);
    btn.disabled = !stack;
    if (!canvas3d.hidden && shownToken && S.token !== shownToken) close();
  }, 400);

  // Drag to rotate, wheel to zoom, double-click to reset.
  let drag = null;
  canvas3d.addEventListener("pointerdown", (e) => {
    drag = { x: e.clientX, y: e.clientY, yaw: view.yaw, pitch: view.pitch };
    canvas3d.setPointerCapture(e.pointerId);
  });
  canvas3d.addEventListener("pointermove", (e) => {
    if (!drag) return;
    view.yaw = drag.yaw - (e.clientX - drag.x) * 0.01;
    view.pitch = Math.max(-1.55, Math.min(1.55, drag.pitch + (e.clientY - drag.y) * 0.01));
    view.draw(true);
  });
  const endDrag = () => { if (drag) { drag = null; view.draw(); } };
  canvas3d.addEventListener("pointerup", endDrag);
  canvas3d.addEventListener("pointercancel", endDrag);
  canvas3d.addEventListener("wheel", (e) => {
    e.preventDefault();
    view.dist = Math.max(0.6, Math.min(8, view.dist * Math.exp(e.deltaY * 0.001)));
    view.draw(true);
    clearTimeout(view.settle);
    view.settle = setTimeout(() => view.draw(), 150);
  }, { passive: false });
  canvas3d.addEventListener("dblclick", () => view.resetView());
  new ResizeObserver(() => { if (view && !canvas3d.hidden) view.draw(); }).observe(canvas3d);

  const bind = (id, key, parse) => {
    $(id).addEventListener("input", (e) => {
      if (!view) return;
      view.settings[key] = parse(e.target.value);
      syncRows();
      view.draw(true);
      clearTimeout(view.settle);
      view.settle = setTimeout(() => view.draw(), 150);
    });
  };
  bind("vp-mode", "mode", (v) => +v);
  bind("vp-th", "threshold", parseFloat);
  bind("vp-op", "opacity", parseFloat);
  bind("vp-br", "brightness", parseFloat);
  bind("vp-z", "zScale", parseFloat);
  $("vp-reset").addEventListener("click", () => view && view.resetView());
  $("vp-reload").addEventListener("click", load);

  window.TifMrcViewPro.volume = { open, close, parseVolume };
})();
