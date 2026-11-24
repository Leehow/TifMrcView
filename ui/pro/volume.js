"use strict";
// Pro 3D view: the current Z stack as a WebGL2 volume (ray marching), or
// the segmentation as smooth surfaces over a slice of it (surfaces.js).
// Shares app.js globals: S (viewer state), invoke, $.
(() => {
  const MAX_CHANNELS = 4;
  const FOVY = 0.7; // vertical field of view of the 3D camera
  // Same order as labels.js, so a label has one colour in 2D and 3D.
  const LABEL_COLORS = window.TifMrcViewPro.LABEL_COLORS;
  const VOXEL_BUDGET = 96 << 20; // all channels together

  const VERT = `#version 300 es
in vec2 pos;
out vec2 ndc;
void main() { ndc = pos; gl_Position = vec4(pos, 0.0, 1.0); }`;

  const FRAG = `#version 300 es
precision highp float;
precision highp sampler3D;
precision highp usampler3D;
in vec2 ndc;
out vec4 outColor;
uniform mat4 invViewProj;
uniform vec3 extent;      // half size of the box
uniform sampler3D vol[${MAX_CHANNELS}];
uniform vec3 color[${MAX_CHANNELS}];
uniform int nch;
uniform int mode;         // 0 MIP, 1 translucent, 2 surface (3, segmentation surfaces, is drawn by surfaces.js)
// Per channel: the levels set on channel 0's histogram, moved to the same
// number of standard deviations above each channel's own mean.
uniform float threshold[${MAX_CHANNELS}]; // surface level, or where MIP / translucent start
uniform float upper[${MAX_CHANNELS}];     // where MIP / translucent reach full brightness
uniform float opacity;
uniform float brightness;
uniform int steps;
uniform vec3 texel;       // 1 / voxel counts
uniform usampler3D labels; // segmentation labels, 0 = none
uniform int hasLabels;
uniform float labelOpacity;
uniform sampler2D lut;     // colour per label byte, alpha 0 = hidden

uint labelAt(vec3 t) {
  uint l = texture(labels, t).r;
  return l == 0u || texelFetch(lut, ivec2(int(l), 0), 0).a < 0.5 ? 0u : l;
}

// Shaded colour of the label at t, lit by the outline of its region.
vec3 labelColor(uint l, vec3 t, vec3 rd) {
  vec3 g = vec3(
    float(labelAt(t + vec3(texel.x, 0, 0)) == l) - float(labelAt(t - vec3(texel.x, 0, 0)) == l),
    float(labelAt(t - vec3(0, texel.y, 0)) == l) - float(labelAt(t + vec3(0, texel.y, 0)) == l),
    float(labelAt(t + vec3(0, 0, texel.z)) == l) - float(labelAt(t - vec3(0, 0, texel.z)) == l));
  vec3 n = length(g) > 1e-5 ? -normalize(g) : -rd;
  return texelFetch(lut, ivec2(int(l), 0), 0).rgb * (0.35 + 0.65 * abs(dot(n, -rd)));
}

uniform int invert;       // dark structures (cryo-EM density) shown bright
uniform vec2 remap[${MAX_CHANNELS}]; // stretch of each channel after smoothing: (start, scale)

float sampleRaw(int c, vec3 t) {
  // Indexing a sampler array needs a constant index in GLSL ES.
  if (c == 0) return texture(vol[0], t).r;
  if (c == 1) return texture(vol[1], t).r;
  if (c == 2) return texture(vol[2], t).r;
  return texture(vol[3], t).r;
}

float sampleCh(int c, vec3 t) {
  float v = clamp((sampleRaw(c, t) - remap[c].x) * remap[c].y, 0.0, 1.0);
  return invert == 1 ? 1.0 - v : v;
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
  vec3 surf = vec3(0.0);
  bool surfHit = false;
  vec3 lab = vec3(0.0);
  bool labHit = false;
  float mx[${MAX_CHANNELS}];
  for (int c = 0; c < ${MAX_CHANNELS}; c++) mx[c] = 0.0;
  for (int i = 0; i < 2048; i++) {
    if (i >= steps) break;
    vec3 tc = toTex(ro + rd * (tn + (float(i) + 0.5) * dt));
    if (hasLabels == 1 && !labHit) {
      uint l = labelAt(tc);
      if (l > 0u) {
        vec3 lc = labelColor(l, tc, rd);
        if (mode == 1) {
          // Labels take part in the translucent blend like dense voxels.
          float a1 = 1.0 - pow(1.0 - labelOpacity, dt * 200.0);
          acc += (1.0 - alpha) * a1 * lc;
          alpha += (1.0 - alpha) * a1;
          if (alpha > 0.98) break;
          continue;
        }
        lab = lc;
        labHit = true;
        if (mode == 2 && !surfHit) break;
      }
    }
    if (mode == 0) {
      for (int c = 0; c < ${MAX_CHANNELS}; c++) if (c < nch) mx[c] = max(mx[c], sampleCh(c, tc));
    } else if (mode == 1) {
      vec3 col = vec3(0.0);
      float a1 = 0.0;
      for (int c = 0; c < ${MAX_CHANNELS}; c++) {
        if (c >= nch) break;
        float v = clamp((sampleCh(c, tc) - threshold[c]) / max(upper[c] - threshold[c], 1e-3), 0.0, 1.0);
        col += color[c] * v;
        a1 = max(a1, smoothstep(0.0, 1.0, v));
      }
      a1 = 1.0 - pow(1.0 - clamp(a1 * opacity, 0.0, 1.0), dt * 200.0);
      acc += (1.0 - alpha) * a1 * col * brightness;
      alpha += (1.0 - alpha) * a1;
      if (alpha > 0.98) break;
    } else {
      for (int c = 0; c < ${MAX_CHANNELS}; c++) {
        if (c >= nch) break;
        if (sampleCh(c, tc) > threshold[c]) {
          vec3 g = vec3(
            sampleCh(c, tc + vec3(texel.x, 0, 0)) - sampleCh(c, tc - vec3(texel.x, 0, 0)),
            sampleCh(c, tc - vec3(0, texel.y, 0)) - sampleCh(c, tc + vec3(0, texel.y, 0)),
            sampleCh(c, tc + vec3(0, 0, texel.z)) - sampleCh(c, tc - vec3(0, 0, texel.z)));
          vec3 n = length(g) > 1e-5 ? -normalize(g) : -rd;
          float diff = abs(dot(n, -rd));
          surf = color[c] * (0.25 + 0.75 * diff) * brightness;
          surfHit = true;
          break;
        }
      }
      // A surface in front hides the labels behind it.
      if (surfHit) break;
    }
  }
  vec3 base;
  if (mode == 0) {
    base = vec3(0.0);
    for (int c = 0; c < ${MAX_CHANNELS}; c++) {
      if (c < nch) base += color[c] * clamp((mx[c] - threshold[c]) / max(upper[c] - threshold[c], 1e-3), 0.0, 1.0);
    }
    base *= brightness;
  } else if (mode == 1) {
    base = acc;
  } else {
    base = surf;
  }
  // In MIP mode the nearest label is laid over the picture.
  // (In surface mode a label reached first is a surface itself.)
  float k = mode == 2 ? 1.0 : labelOpacity;
  outColor = vec4(labHit && mode != 1 ? mix(base, lab, k) : base, 1.0);
}`;

  // --- GPU helpers: Gaussian smoothing and histograms of the volume textures ---
  // Both draw without vertex buffers; positions come from gl_VertexID.
  const BLUR_VERT = `#version 300 es
void main() { gl_Position = vec4(float((gl_VertexID & 1) * 4 - 1), float((gl_VertexID >> 1) * 4 - 1), 0.0, 1.0); }`;

  // One pass of a separable blur along `axis`, writing one Z layer.
  const BLUR_FRAG = `#version 300 es
precision highp float;
precision highp sampler3D;
uniform sampler3D src;
uniform ivec3 size;
uniform ivec3 axis;
uniform int layer;
uniform int radius;
uniform float weights[32];
out vec4 outColor;
void main() {
  ivec3 p = ivec3(ivec2(gl_FragCoord.xy), layer);
  float acc = 0.0;
  for (int i = -radius; i <= radius; i++) {
    acc += weights[i + radius] * texelFetch(src, clamp(p + axis * i, ivec3(0), size - 1), 0).r;
  }
  outColor = vec4(acc, 0.0, 0.0, 1.0);
}`;

  // Every voxel (or every stride-th) adds 1 to its bin in an n x 1 float target.
  const HIST_VERT = `#version 300 es
precision highp float;
precision highp sampler3D;
uniform sampler3D src;
uniform ivec3 size;
uniform int stride;
uniform float lo;
uniform float hi;
uniform int bins;
void main() {
  int id = gl_VertexID * stride;
  ivec3 p = ivec3(id % size.x, (id / size.x) % size.y, id / (size.x * size.y));
  float v = texelFetch(src, p, 0).r;
  float b = clamp(floor((v - lo) / (hi - lo) * float(bins)), 0.0, float(bins - 1));
  gl_Position = vec4((b + 0.5) / float(bins) * 2.0 - 1.0, 0.0, 0.0, 1.0);
  gl_PointSize = 1.0;
}`;
  const HIST_FRAG = `#version 300 es
precision highp float;
out vec4 outColor;
void main() { outColor = vec4(1.0); }`;

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
  const orthographic = (halfH, aspect, near, far) => {
    const nf = 1 / (near - far);
    return new Float32Array([1 / (halfH * aspect), 0, 0, 0, 0, 1 / halfH, 0, 0, 0, 0, 2 * nf, 0, 0, 0, (far + near) * nf, 1]);
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
      const gl = canvas.getContext("webgl2", { antialias: true, preserveDrawingBuffer: true });
      if (!gl) throw new Error("这台电脑的显卡不支持 WebGL2，无法显示 3D");
      this.gl = gl;
      const sh = (type, src) => {
        const s = gl.createShader(type);
        gl.shaderSource(s, src);
        gl.compileShader(s);
        if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
        return s;
      };
      const link = (vs, fs) => {
        const p = gl.createProgram();
        gl.attachShader(p, sh(gl.VERTEX_SHADER, vs));
        gl.attachShader(p, sh(gl.FRAGMENT_SHADER, fs));
        gl.linkProgram(p);
        if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
        return p;
      };
      const prog = link(VERT, FRAG);
      this.prog = prog;
      // Smoothing and histograms run on the GPU when it can render to and blend
      // float textures; otherwise volume.rs smooths and the histogram is counted here.
      this.gpu = !!(gl.getExtension("EXT_color_buffer_float") && gl.getExtension("EXT_float_blend"));
      if (this.gpu) {
        this.blurProg = link(BLUR_VERT, BLUR_FRAG);
        this.histProg = link(HIST_VERT, HIST_FRAG);
        this.emptyVao = gl.createVertexArray();
        this.fbo = gl.createFramebuffer();
      }
      gl.useProgram(prog);
      this.vao = gl.createVertexArray();
      gl.bindVertexArray(this.vao);
      const vbo = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
      const loc = gl.getAttribLocation(prog, "pos");
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
      this.u = {};
      for (const name of ["invViewProj", "extent", "nch", "mode", "threshold", "upper", "opacity", "brightness", "steps", "texel", "hasLabels", "labelOpacity", "invert", "remap"]) {
        this.u[name] = gl.getUniformLocation(prog, name);
      }
      gl.uniform1iv(gl.getUniformLocation(prog, "vol"), [0, 1, 2, 3]);
      gl.uniform1i(gl.getUniformLocation(prog, "labels"), MAX_CHANNELS);
      gl.uniform1i(gl.getUniformLocation(prog, "lut"), MAX_CHANNELS + 3);
      this.lut = gl.createTexture();
      const table = new Uint8Array(256 * 4);
      for (let b = 1; b < 256; b++) table.set([...LABEL_COLORS[(b - 1) % LABEL_COLORS.length], 255], b * 4);
      this.setColorTable(table);
      this.uColor = gl.getUniformLocation(prog, "color");
      this.textures = []; // what the ray marcher samples: the raw R8 volumes or their smoothed copies
      this.raw = [];
      this.smoothed = [];
      this.remap = new Float32Array(MAX_CHANNELS * 2).fill(1);
      this.labelTex = null;
      this.setLabels(null);
      // levels[mode] = [start, full] in 0..1 of the volume's value range (surface uses start only).
      // levels[3] is unused (segmentation surfaces); slice = depth of their XY slice in 0..1.
      this.settings = { mode: 0, levels: [[0, 1], [0.25, 1], [0.25, 1], [0, 1]], opacity: 0.2, brightness: 1.2, zScale: 1, showLabels: true, labelOpacity: 0.7, invert: false, smooth: 0, showSlice: true, slice: 0.5, ortho: false, fig: null };
      // fig (figure.js): null, or {bg: [r, g, b, a], ao, gloss, outline, width, line, slice} for
      // the segmentation surfaces drawn as a figure; ortho: parallel projection (true scale bars).
      this.scene = null; // surfaces.js SurfaceScene, made when surfaces arrive
      this.selected = -1; // picked object of the surfaces
      this.objectColor = () => null; // object index -> [r, g, b] in 0..1, or null when hidden
      this.resetView();
    }

    resetView() {
      this.yaw = 0.6;
      this.pitch = 0.35;
      this.dist = 2.6;
      this.target = [0, 0, 0]; // point the camera orbits, moved by panning
      this.draw();
    }

    setVolume(vol, colors) {
      const gl = this.gl;
      this.dropSmoothed();
      this.raw.forEach((t) => gl.deleteTexture(t));
      this.raw = [];
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      vol.chans.slice(0, MAX_CHANNELS).forEach((c, i) => {
        const tex = gl.createTexture();
        gl.activeTexture(gl.TEXTURE0 + i);
        gl.bindTexture(gl.TEXTURE_3D, tex);
        gl.texImage3D(gl.TEXTURE_3D, 0, gl.R8, vol.w, vol.h, vol.d, 0, gl.RED, gl.UNSIGNED_BYTE, c.data);
        gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
        for (const p of [gl.TEXTURE_WRAP_S, gl.TEXTURE_WRAP_T, gl.TEXTURE_WRAP_R]) gl.texParameteri(gl.TEXTURE_3D, p, gl.CLAMP_TO_EDGE);
        this.raw.push(tex);
      });
      this.textures = this.raw.slice();
      this.remap.fill(1);
      for (let i = 0; i < MAX_CHANNELS; i++) this.remap[i * 2] = 0;
      this.vol = vol;
      this.colors = colors.slice(0, MAX_CHANNELS);
      this.draw();
    }

    dropSmoothed() {
      this.smoothed.forEach((t) => this.gl.deleteTexture(t));
      this.smoothed = [];
    }

    float3D(filter) {
      const gl = this.gl, v = this.vol;
      const tex = gl.createTexture();
      gl.activeTexture(gl.TEXTURE0 + MAX_CHANNELS + 1);
      gl.bindTexture(gl.TEXTURE_3D, tex);
      gl.texStorage3D(gl.TEXTURE_3D, 1, gl.R16F, v.w, v.h, v.d);
      gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MIN_FILTER, filter);
      gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MAG_FILTER, filter);
      for (const p of [gl.TEXTURE_WRAP_S, gl.TEXTURE_WRAP_T, gl.TEXTURE_WRAP_R]) gl.texParameteri(gl.TEXTURE_3D, p, gl.CLAMP_TO_EDGE);
      gl.bindTexture(gl.TEXTURE_3D, null);
      return tex;
    }

    /// Gaussian smoothing on the GPU (sigma in voxels, 0 = off): three separable
    /// passes into half-float textures, then each channel is stretched so its
    /// 0.1–99.9 % range fills 0..1, like volume.rs does on the CPU.
    /// Returns per channel [a, b], the kept range in 0..1 of the raw values.
    smooth(sigma) {
      const gl = this.gl, v = this.vol;
      this.dropSmoothed();
      const ranges = [];
      if (!(sigma > 0) || !this.gpu) {
        this.textures = this.raw.slice();
        this.raw.forEach((_, c) => { this.remap[c * 2] = 0; this.remap[c * 2 + 1] = 1; ranges.push([0, 1]); });
        this.draw();
        return ranges;
      }
      const radius = Math.min(15, Math.ceil(sigma * 3));
      const w = new Float32Array(32);
      let sum = 0;
      for (let i = -radius; i <= radius; i++) sum += (w[i + radius] = Math.exp(-(i * i) / (2 * sigma * sigma)));
      w.forEach((x, i) => (w[i] = x / sum));
      const p = this.blurProg;
      gl.useProgram(p);
      gl.bindVertexArray(this.emptyVao);
      gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
      gl.viewport(0, 0, v.w, v.h);
      gl.disable(gl.BLEND);
      gl.uniform1i(gl.getUniformLocation(p, "src"), MAX_CHANNELS + 2);
      gl.uniform3i(gl.getUniformLocation(p, "size"), v.w, v.h, v.d);
      gl.uniform1i(gl.getUniformLocation(p, "radius"), radius);
      gl.uniform1fv(gl.getUniformLocation(p, "weights"), w);
      const uAxis = gl.getUniformLocation(p, "axis"), uLayer = gl.getUniformLocation(p, "layer");
      const pass = (src, dst, axis) => {
        gl.activeTexture(gl.TEXTURE0 + MAX_CHANNELS + 2);
        gl.bindTexture(gl.TEXTURE_3D, src);
        gl.uniform3i(uAxis, ...axis);
        for (let z = 0; z < v.d; z++) {
          gl.framebufferTextureLayer(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, dst, 0, z);
          gl.uniform1i(uLayer, z);
          gl.drawArrays(gl.TRIANGLES, 0, 3);
        }
      };
      this.raw.forEach((raw, c) => {
        const a = this.float3D(gl.LINEAR), b = this.float3D(gl.NEAREST);
        pass(raw, a, [1, 0, 0]);
        pass(a, b, [0, 1, 0]);
        if (v.d > 1) pass(b, a, [0, 0, 1]);
        else pass(b, a, [0, 0, 0]);
        gl.deleteTexture(b);
        this.smoothed.push(a);
        // Percentiles from a fine histogram of the blurred values.
        const counts = this.histogram(a, 0, 1, 4096);
        const total = counts.reduce((x, y) => x + y, 0);
        const pick = (frac) => {
          let seen = 0;
          for (let i = 0; i < counts.length; i++) {
            seen += counts[i];
            if (seen > total * frac) return i / 4096;
          }
          return 1;
        };
        const lo = pick(0.001);
        const hi = Math.max(pick(0.999) + 1 / 4096, lo + 1 / 255);
        this.remap[c * 2] = lo;
        this.remap[c * 2 + 1] = 1 / (hi - lo);
        ranges.push([lo, hi]);
      });
      this.textures = this.smoothed.slice();
      this.draw();
      return ranges;
    }

    /// Counts of channel c's shown values (after the stretch) in `bins` bins over 0..1.
    shownHistogram(c, bins) {
      const a = this.remap[c * 2], k = this.remap[c * 2 + 1];
      return this.histogram(this.textures[c], a, a + 1 / k, bins);
    }

    /// GPU histogram of a volume texture: `bins` counts of values in lo..hi.
    histogram(tex, lo, hi, bins) {
      const gl = this.gl, v = this.vol;
      const n = v.w * v.h * v.d;
      // Float sums stay exact up to 2^24, so very large volumes are sampled.
      const stride = Math.max(1, Math.ceil(n / (1 << 24)));
      const target = gl.createTexture();
      gl.activeTexture(gl.TEXTURE0 + MAX_CHANNELS + 1);
      gl.bindTexture(gl.TEXTURE_2D, target);
      gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA32F, bins, 1);
      gl.bindTexture(gl.TEXTURE_2D, null);
      const p = this.histProg;
      gl.useProgram(p);
      gl.bindVertexArray(this.emptyVao);
      gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, target, 0);
      gl.viewport(0, 0, bins, 1);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.ONE, gl.ONE);
      gl.activeTexture(gl.TEXTURE0 + MAX_CHANNELS + 2);
      gl.bindTexture(gl.TEXTURE_3D, tex);
      gl.uniform1i(gl.getUniformLocation(p, "src"), MAX_CHANNELS + 2);
      gl.uniform3i(gl.getUniformLocation(p, "size"), v.w, v.h, v.d);
      gl.uniform1i(gl.getUniformLocation(p, "stride"), stride);
      gl.uniform1f(gl.getUniformLocation(p, "lo"), lo);
      gl.uniform1f(gl.getUniformLocation(p, "hi"), hi);
      gl.uniform1i(gl.getUniformLocation(p, "bins"), bins);
      gl.drawArrays(gl.POINTS, 0, Math.ceil(n / stride));
      gl.disable(gl.BLEND);
      const px = new Float32Array(bins * 4);
      gl.readPixels(0, 0, bins, 1, gl.RGBA, gl.FLOAT, px);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, null, 0);
      gl.deleteTexture(target);
      const counts = new Float64Array(bins);
      for (let i = 0; i < bins; i++) counts[i] = px[i * 4] * stride;
      return counts;
    }

    /// 256 RGBA bytes: colour of each label byte, alpha 0 hides it.
    setColorTable(table) {
      const gl = this.gl;
      gl.activeTexture(gl.TEXTURE0 + MAX_CHANNELS + 3);
      gl.bindTexture(gl.TEXTURE_2D, this.lut);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, 256, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, table);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      this.lutTable = table;
      this.draw();
    }

    /// Label volume {w, h, d, data} matching the image volume, or null.
    setLabels(lab) {
      const gl = this.gl;
      if (this.labelTex) gl.deleteTexture(this.labelTex);
      const ok = lab && this.vol && lab.w === this.vol.w && lab.h === this.vol.h && lab.d === this.vol.d;
      this.labelTex = gl.createTexture();
      gl.activeTexture(gl.TEXTURE0 + MAX_CHANNELS);
      gl.bindTexture(gl.TEXTURE_3D, this.labelTex);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      // Integer textures cannot be filtered; an empty 1x1x1 one stands in when there are no labels.
      if (ok) gl.texImage3D(gl.TEXTURE_3D, 0, gl.R8UI, lab.w, lab.h, lab.d, 0, gl.RED_INTEGER, gl.UNSIGNED_BYTE, lab.data);
      else gl.texImage3D(gl.TEXTURE_3D, 0, gl.R8UI, 1, 1, 1, 0, gl.RED_INTEGER, gl.UNSIGNED_BYTE, new Uint8Array(1));
      gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_3D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      for (const p of [gl.TEXTURE_WRAP_S, gl.TEXTURE_WRAP_T, gl.TEXTURE_WRAP_R]) gl.texParameteri(gl.TEXTURE_3D, p, gl.CLAMP_TO_EDGE);
      this.hasLabels = !!ok;
      this.labDims = ok ? [lab.w, lab.h, lab.d] : null;
      this.labData = ok ? lab.data : null; // for picking points (pick)
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
      this.renderScene(fast);
      if (this.onRender) this.onRender();
    }

    renderScene(fast) {
      const gl = this.gl, c = this.canvas;
      const dpr = window.devicePixelRatio || 1;
      const scale = fast ? 0.5 : 1;
      const w = Math.max(1, Math.round(c.clientWidth * dpr * scale));
      const h = Math.max(1, Math.round(c.clientHeight * dpr * scale));
      if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      gl.bindVertexArray(this.vao);
      gl.disable(gl.BLEND);
      gl.viewport(0, 0, w, h);
      // Segmentation surfaces sit on white, like figures of segmented tomograms.
      const white = this.settings.mode === 3 ? 1 : 0;
      gl.clearColor(white, white, white, 1);
      gl.clear(gl.COLOR_BUFFER_BIT);
      if (!this.vol) return;
      // Centre the volume in the part not covered by the tools panel (app.js freeRect).
      // (and, in figure mode, not by the figure panel on the right).
      const x0 = Math.min(w - 1, Math.round(freeRect().x0 * dpr * scale));
      const vw = Math.max(1, w - x0 - Math.round(this.rightInset() * dpr * scale));
      gl.viewport(x0, 0, vw, h);
      this.lastViewport = [x0, 0, vw, h];
      const s = this.settings;
      const { vp, eye } = this.camera(vw / h);
      if (s.mode === 3) {
        this.last = { vp, eye, viewport: [x0, 0, vw, h], w, h };
        if (!this.scene) return;
        // Dragging stays smooth on any GPU: the occlusion rays wait for the still frame.
        this.quick = fast;
        if (s.fig) this.drawFigure(null, w, h, this.last.viewport, 1);
        else this.scene.draw(this.sceneOptions());
        return;
      }
      gl.useProgram(this.prog);
      // Unused samplers still need a valid texture bound.
      for (let i = 0; i < MAX_CHANNELS; i++) {
        gl.activeTexture(gl.TEXTURE0 + i);
        gl.bindTexture(gl.TEXTURE_3D, this.textures[Math.min(i, this.textures.length - 1)]);
      }
      gl.activeTexture(gl.TEXTURE0 + MAX_CHANNELS);
      gl.bindTexture(gl.TEXTURE_3D, this.labelTex);
      gl.activeTexture(gl.TEXTURE0 + MAX_CHANNELS + 3);
      gl.bindTexture(gl.TEXTURE_2D, this.lut);
      gl.uniform2fv(this.u.remap, this.remap);
      gl.uniformMatrix4fv(this.u.invViewProj, false, invert(vp));
      gl.uniform3fv(this.u.extent, this.extent());
      gl.uniform1i(this.u.nch, this.textures.length);
      gl.uniform1i(this.u.mode, s.mode);
      const [lo, hi] = s.levels[s.mode];
      const [thr, upp] = this.levelsPerChannel(lo, hi);
      gl.uniform1fv(this.u.threshold, thr);
      gl.uniform1fv(this.u.upper, upp);
      gl.uniform1f(this.u.opacity, s.opacity);
      gl.uniform1f(this.u.brightness, s.brightness);
      gl.uniform1i(this.u.hasLabels, this.hasLabels && s.showLabels ? 1 : 0);
      gl.uniform1f(this.u.labelOpacity, s.labelOpacity);
      gl.uniform1i(this.u.invert, s.invert ? 1 : 0);
      const v = this.vol;
      const full = Math.min(2048, Math.round(1.5 * Math.max(v.w, v.h, v.d)));
      gl.uniform1i(this.u.steps, fast ? Math.max(64, full >> 2) : full);
      gl.uniform3fv(this.u.texel, [1 / v.w, 1 / v.h, 1 / v.d]);
      const cols = new Float32Array(MAX_CHANNELS * 3);
      this.colors.forEach((rgb, i) => cols.set(rgb.map((x) => x / 255), i * 3));
      gl.uniform3fv(this.uColor, cols);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    }

    /// CSS pixels on the right covered by the figure panel.
    rightInset() {
      const f = window.TifMrcViewPro.figure;
      const r = this.settings.fig && f ? f.inset() : 0;
      return this.canvas.clientWidth - freeRect().x0 - r >= 200 ? r : 0;
    }

    /// Width / height of the part of the canvas the scene is drawn in.
    aspect() {
      const c = this.canvas;
      return Math.max(1, c.clientWidth - freeRect().x0 - this.rightInset()) / Math.max(1, c.clientHeight);
    }

    /// Looks from (yaw, pitch) with the whole box just filling the view.
    lookFrom(yaw, pitch) {
      this.yaw = yaw;
      this.pitch = pitch;
      this.target = [0, 0, 0];
      const z = [Math.cos(pitch) * Math.sin(yaw), Math.sin(pitch), Math.cos(pitch) * Math.cos(yaw)];
      const xl = Math.hypot(z[2], z[0]) || 1;
      const x = [z[2] / xl, 0, -z[0] / xl];
      const y = [z[1] * x[2] - z[2] * x[1], z[2] * x[0] - z[0] * x[2], z[0] * x[1] - z[1] * x[0]];
      const e = this.extent();
      let mx = 0, my = 0;
      for (let i = 0; i < 8; i++) {
        const c = [i & 1 ? e[0] : -e[0], i & 2 ? e[1] : -e[1], i & 4 ? e[2] : -e[2]];
        mx = Math.max(mx, Math.abs(c[0] * x[0] + c[1] * x[1] + c[2] * x[2]));
        my = Math.max(my, Math.abs(c[0] * y[0] + c[1] * y[1] + c[2] * y[2]));
      }
      const halfH = Math.max(my, mx / this.aspect()) * 1.08;
      this.dist = Math.max(0.3, Math.min(8, halfH / Math.tan(FOVY / 2)));
      this.draw();
    }

    /// View-projection for a viewport of this aspect, and the eye (a point,
    /// or with the parallel projection the direction towards the viewer).
    camera(aspect) {
      const s = this.settings;
      const eye = [
        this.dist * Math.cos(this.pitch) * Math.sin(this.yaw),
        this.dist * Math.sin(this.pitch),
        this.dist * Math.cos(this.pitch) * Math.cos(this.yaw),
      ];
      const t = this.target;
      const pan = new Float32Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -t[0], -t[1], -t[2], 1]);
      // The parallel projection frames what the perspective one shows at the target.
      const proj = s.ortho
        ? orthographic(this.dist * Math.tan(FOVY / 2), aspect, this.dist - 4, this.dist + 4)
        : perspective(FOVY, aspect, 0.05, 20);
      const vp = mul(proj, mul(lookAt(eye), pan));
      return { vp, eye: s.ortho ? eye : eye.map((v, i) => v + t[i]) };
    }

    /// The surfaces as a figure (settings.fig) into framebuffer `dst` (null =
    /// canvas) of w x h, the scene in `viewport`; outlines scaled by `scale`.
    drawFigure(dst, w, h, viewport, scale) {
      const gl = this.gl, s = this.settings;
      if (!this.figure) this.figure = new window.TifMrcViewPro.surfaces.FigureTarget(gl);
      this.figure.begin(w, h);
      gl.viewport(...viewport);
      this.scene.draw(this.sceneOptions());
      // With the parallel projection depth runs evenly from dist - 4 to dist + 4;
      // the box lies within its half diagonal of the target.
      const r = Math.hypot(...this.extent()) / 8;
      this.figure.compose(dst, w, h, { ...s.fig, width: (s.fig.width || 1.5) * scale * (window.devicePixelRatio || 1), cue: s.ortho ? s.fig.cue || 0 : 0, cueRange: [0.5 - r, 0.5 + r] });
    }

    /// The figure as it is on screen, rendered at w x h pixels (same view,
    /// same aspect as the screen's free part): RGBA rows from the top.
    snapshot(w, h) {
      const gl = this.gl, s = this.settings;
      if (!this.scene || !this.vol) return null;
      const fig = s.fig || { bg: [1, 1, 1, 1], ao: 0, outline: 0 };
      const keep = s.fig;
      s.fig = fig;
      this.exporting = true;
      const { vp, eye } = this.camera(w / h);
      const prev = this.last;
      this.last = { vp, eye, viewport: [0, 0, w, h], w, h };
      const fbo = gl.createFramebuffer(), rb = gl.createRenderbuffer();
      gl.bindRenderbuffer(gl.RENDERBUFFER, rb);
      gl.renderbufferStorage(gl.RENDERBUFFER, gl.RGBA8, w, h);
      gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
      gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.RENDERBUFFER, rb);
      // Outlines keep their width relative to the picture.
      const scale = h / Math.max(1, prev ? prev.h : h);
      this.drawFigure(fbo, w, h, [0, 0, w, h], scale);
      const px = new Uint8Array(w * h * 4);
      gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
      gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, px);
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      gl.deleteFramebuffer(fbo);
      gl.deleteRenderbuffer(rb);
      this.figure.free(); // do not keep a poster-sized target around
      s.fig = keep;
      this.exporting = false;
      this.last = prev;
      // GL rows run from the bottom.
      const out = new Uint8Array(px.length), row = w * 4;
      for (let y = 0; y < h; y++) out.set(px.subarray((h - 1 - y) * row, (h - y) * row), y * row);
      return out;
    }

    /// Length in the image's units of one pixel of a picture `hPx` high with
    /// the parallel projection, or 0 (perspective, or no pixel size).
    unitsPerPixel(hPx) {
      if (!this.settings.ortho || !this.vol || !S.px || !S.info) return 0;
      const ext = this.extent();
      const perWorld = (S.px.x * S.info.width) / (2 * ext[0]);
      return (2 * this.dist * Math.tan(FOVY / 2) * perWorld) / Math.max(1, hPx);
    }

    /// Largest picture this GPU can render.
    maxSize() {
      const gl = this.gl;
      return Math.min(gl.getParameter(gl.MAX_TEXTURE_SIZE), gl.getParameter(gl.MAX_RENDERBUFFER_SIZE), ...gl.getParameter(gl.MAX_VIEWPORT_DIMS));
    }

    /// Surfaces plus the XY slice of the first channel, its grey levels at
    /// mean ± 3 standard deviations; the camera of the last frame.
    sceneOptions() {
      const s = this.settings, ext = this.extent(), st = (this.stats || [])[0];
      const slice = s.showSlice && this.textures.length ? {
        tex: this.textures[0], unit: MAX_CHANNELS + 2,
        remap: [this.remap[0], this.remap[1]],
        window: st && st.sd > 1e-3 ? [st.m - 3 * st.sd, st.m + 3 * st.sd] : [0, 1],
        z: (s.slice - 0.5) * 2 * ext[2],
      } : null;
      const f = s.fig;
      const labels = f && f.ao > 0 && this.hasLabels && this.labDims
        ? { tex: this.labelTex, unit: MAX_CHANNELS, lut: this.lut, lutUnit: MAX_CHANNELS + 3, dims: this.labDims } : null;
      return {
        vp: this.last.vp, eye: this.last.eye, extent: ext, slice: f && f.slice === false ? null : slice,
        colorOf: this.objectColor, selected: this.exporting ? -1 : this.selected,
        ortho: s.ortho, ao: f ? f.ao : 0, quick: !!this.quick && !this.exporting, gloss: f ? f.gloss : 0.35, labels,
      };
    }

    /// Mesh parsed by surfaces.js, or null.
    setSurfaces(mesh) {
      if (!this.scene && mesh) this.scene = new window.TifMrcViewPro.surfaces.SurfaceScene(this.gl);
      if (this.scene) this.scene.setMesh(mesh);
      this.selected = -1;
      this.draw();
    }

    /// Object under CSS pixel (x, y) of the canvas, or -1 (surfaces of the last frame).
    pickAt(x, y) {
      if (!this.scene || !this.last || this.settings.mode !== 3) return -1;
      const dpr = this.last.w / Math.max(1, this.canvas.clientWidth);
      const id = this.scene.pick(this.sceneOptions(), x * dpr, y * dpr, this.last.w, this.last.h, this.last.viewport);
      return id;
    }

    /// Levels for each channel: channel 0 takes `lo`/`hi` as they are; the
    /// others the same distance from their mean in their own standard
    /// deviations, so a dim or sparse channel is not cut off by levels
    /// chosen on a bright one. `this.stats` holds {m, sd} per channel in
    /// 0..1 units before inverting.
    levelsPerChannel(lo, hi) {
      const thr = new Float32Array(MAX_CHANNELS).fill(lo), upp = new Float32Array(MAX_CHANNELS).fill(hi);
      const st = (this.stats || []).map((x) => ({ m: this.settings.invert ? 1 - x.m : x.m, sd: x.sd })), s0 = st[0];
      if (!s0 || !(s0.sd > 1e-3)) return [thr, upp];
      const c01 = (x) => Math.max(0, Math.min(1, x));
      for (let c = 1; c < Math.min(st.length, MAX_CHANNELS); c++) {
        if (!(st[c] && st[c].sd > 1e-3)) continue;
        thr[c] = c01(st[c].m + ((lo - s0.m) / s0.sd) * st[c].sd);
        upp[c] = Math.max(c01(st[c].m + ((hi - s0.m) / s0.sd) * st[c].sd), thr[c] + 0.002);
      }
      return [thr, upp];
    }

    /// The camera in CSS pixels of the canvas: view-projection and the part
    /// of the canvas the scene is drawn in (left edge x0, width w, height h).
    cssCamera() {
      const c = this.canvas, x0 = freeRect().x0;
      const w = Math.max(1, c.clientWidth - x0 - this.rightInset()), h = Math.max(1, c.clientHeight);
      return { vp: this.camera(w / h).vp, x0, w, h };
    }

    /// A point of the image [x, y, z] (x, y in pixels, slice i at z = i) in the scene's box.
    toWorld(q) {
      const e = this.extent(), i = S.info;
      const f = [q[0] / i.width, q[1] / i.height, (q[2] + 0.5) / i.sizeZ];
      return [(f[0] - 0.5) * 2 * e[0], (0.5 - f[1]) * 2 * e[1], (f[2] - 0.5) * 2 * e[2]];
    }

    /// CSS pixel of a scene point, or null behind the camera.
    project(p, cam) {
      const m = cam.vp;
      const r = [0, 1, 2, 3].map((i) => m[i] * p[0] + m[4 + i] * p[1] + m[8 + i] * p[2] + m[12 + i]);
      if (r[3] <= 1e-6) return null;
      return [cam.x0 + ((r[0] / r[3] + 1) / 2) * cam.w, ((1 - r[1] / r[3]) / 2) * cam.h];
    }

    /// The image point [x, y, z] shown under CSS pixel (cx, cy), or null:
    /// along the ray, the first voxel over the surface level (surface mode),
    /// the brightest one (maximum intensity), where the blend becomes half
    /// opaque (translucent), or the first label / the slice (segmentation surfaces).
    pick(cx, cy) {
      const v = this.vol, s = this.settings, i = S.info;
      if (!v || !i) return null;
      const cam = this.cssCamera();
      const nx = ((cx - cam.x0) / cam.w) * 2 - 1, ny = 1 - (cy / cam.h) * 2;
      if (Math.abs(nx) > 1 || Math.abs(ny) > 1) return null;
      const inv = invert(cam.vp);
      const un = (z) => {
        const r = [0, 1, 2, 3].map((k) => inv[k] * nx + inv[4 + k] * ny + inv[8 + k] * z + inv[12 + k]);
        return [r[0] / r[3], r[1] / r[3], r[2] / r[3]];
      };
      const ro = un(-1), far = un(1);
      const len = Math.hypot(far[0] - ro[0], far[1] - ro[1], far[2] - ro[2]);
      const rd = far.map((x, k) => (x - ro[k]) / len);
      const e = this.extent();
      let tn = 0, tf = Infinity;
      for (let k = 0; k < 3; k++) {
        const a = (-e[k] - ro[k]) / rd[k], b = (e[k] - ro[k]) / rd[k];
        tn = Math.max(tn, Math.min(a, b));
        tf = Math.min(tf, Math.max(a, b));
      }
      if (!(tf > tn)) return null;
      const toTex = (p) => [p[0] / e[0] * 0.5 + 0.5, 0.5 - p[1] / e[1] * 0.5, p[2] / e[2] * 0.5 + 0.5];
      const out = (t) => [t[0] * i.width, t[1] * i.height, t[2] * i.sizeZ - 0.5];
      const at = (tt) => toTex(ro.map((x, k) => x + rd[k] * tt));
      // Segmentation surfaces: the first shown label, else the XY slice under them.
      const labelAt = (t) => {
        if (!this.labData || !this.hasLabels) return 0;
        const [w, h, d] = this.labDims;
        const x = Math.min(w - 1, Math.floor(t[0] * w)), y = Math.min(h - 1, Math.floor(t[1] * h)), z = Math.min(d - 1, Math.floor(t[2] * d));
        const l = this.labData[(z * h + y) * w + x];
        return l && (!this.lutTable || this.lutTable[l * 4 + 3] > 127) ? l : 0;
      };
      const steps = Math.min(4096, Math.round(2 * Math.max(v.w, v.h, v.d)));
      const dt = (tf - tn) / steps;
      if (s.mode === 3) {
        for (let k = 0; k < steps; k++) {
          const t = at(tn + (k + 0.5) * dt);
          if (labelAt(t)) return out(t);
        }
        if (!s.showSlice) return null;
        const zw = (s.slice - 0.5) * 2 * e[2];
        const tt = (zw - ro[2]) / rd[2];
        if (!(tt >= tn && tt <= tf)) return null;
        return out(at(tt));
      }
      // Values as the ray marcher sees them; smoothing is stood in for by a box average.
      const r = s.smooth > 0 ? Math.round(s.smooth) : 0;
      const value = (c, t) => {
        const data = v.chans[c].data;
        const fx = t[0] * v.w - 0.5, fy = t[1] * v.h - 0.5, fz = t[2] * v.d - 0.5;
        let raw;
        if (r > 0) {
          const X = Math.round(fx), Y = Math.round(fy), Z = Math.round(fz);
          let sum = 0, n = 0;
          for (let z = Math.max(0, Z - r); z <= Math.min(v.d - 1, Z + r); z++)
            for (let y = Math.max(0, Y - r); y <= Math.min(v.h - 1, Y + r); y++)
              for (let x = Math.max(0, X - r); x <= Math.min(v.w - 1, X + r); x++) { sum += data[(z * v.h + y) * v.w + x]; n++; }
          raw = n ? sum / n : 0;
        } else {
          const x0 = Math.max(0, Math.min(v.w - 1, Math.floor(fx))), y0 = Math.max(0, Math.min(v.h - 1, Math.floor(fy))), z0 = Math.max(0, Math.min(v.d - 1, Math.floor(fz)));
          const x1 = Math.min(v.w - 1, x0 + 1), y1 = Math.min(v.h - 1, y0 + 1), z1 = Math.min(v.d - 1, z0 + 1);
          const ax = Math.max(0, Math.min(1, fx - x0)), ay = Math.max(0, Math.min(1, fy - y0)), az = Math.max(0, Math.min(1, fz - z0));
          const g = (x, y, z) => data[(z * v.h + y) * v.w + x];
          const lx = (y, z) => g(x0, y, z) * (1 - ax) + g(x1, y, z) * ax;
          const ly = (z) => lx(y0, z) * (1 - ay) + lx(y1, z) * ay;
          raw = ly(z0) * (1 - az) + ly(z1) * az;
        }
        const k = Math.max(0, Math.min(1, (raw / 255 - this.remap[c * 2]) * this.remap[c * 2 + 1]));
        return s.invert ? 1 - k : k;
      };
      const [lo, hi] = s.levels[s.mode];
      const [thr, upp] = this.levelsPerChannel(lo, hi);
      const nch = Math.min(v.chans.length, MAX_CHANNELS);
      const showLabels = this.hasLabels && s.showLabels && s.mode !== 1;
      let best = null, bestScore = 0, alpha = 0;
      for (let k = 0; k < steps; k++) {
        const t = at(tn + (k + 0.5) * dt);
        if (showLabels && labelAt(t)) return out(t);
        if (s.mode === 2) {
          for (let c = 0; c < nch; c++) if (value(c, t) > thr[c]) return out(t);
        } else if (s.mode === 0) {
          for (let c = 0; c < nch; c++) {
            const score = (value(c, t) - thr[c]) / Math.max(upp[c] - thr[c], 1e-3);
            if (score > bestScore) { bestScore = score; best = t; }
          }
        } else {
          let a1 = 0;
          for (let c = 0; c < nch; c++) {
            const x = Math.max(0, Math.min(1, (value(c, t) - thr[c]) / Math.max(upp[c] - thr[c], 1e-3)));
            a1 = Math.max(a1, x * x * (3 - 2 * x));
          }
          a1 = 1 - Math.pow(1 - Math.max(0, Math.min(1, a1 * s.opacity)), dt * 200);
          alpha += (1 - alpha) * a1;
          if (a1 > bestScore) { bestScore = a1; best = t; }
          if (alpha >= 0.5) return out(t);
        }
      }
      return best ? out(best) : null;
    }

    /// Moves the view by (dx, dy) CSS pixels, so the volume follows the pointer.
    panBy(dx, dy) {
      const z = [Math.cos(this.pitch) * Math.sin(this.yaw), Math.sin(this.pitch), Math.cos(this.pitch) * Math.cos(this.yaw)];
      const x = [z[2], 0, -z[0]];
      const xl = Math.hypot(...x) || 1;
      x.forEach((v, i) => (x[i] = v / xl));
      const y = [z[1] * x[2] - z[2] * x[1], z[2] * x[0] - z[0] * x[2], z[0] * x[1] - z[1] * x[0]];
      const k = (2 * this.dist * Math.tan(FOVY / 2)) / Math.max(1, this.canvas.clientHeight);
      this.target = this.target.map((v, i) => v - x[i] * dx * k + y[i] * dy * k);
    }

    dispose() {
      this.setSurfaces(null);
      this.dropSmoothed();
      this.raw.forEach((t) => this.gl.deleteTexture(t));
      this.raw = [];
      this.textures = [];
      this.vol = null;
      this.setLabels(null);
    }
  }

  // --- UI: quick toolbar button, overlay canvas and settings panel ---
  const btn = document.createElement("button");
  btn.id = "btn-3d";
  btn.title = "3D 视图：把 Z 堆栈显示成可以旋转的三维体（拖动旋转，右键拖动（触控板双指按下拖动或 Control+拖动）、中键或 Shift+拖动平移，滚轮或双指捏合缩放，双击复位）";
  btn.innerHTML = '<svg viewBox="0 0 24 24"><path d="M12 2l9 5v10l-9 5-9-5V7z"/><path d="M3 7l9 5 9-5M12 12v10"/></svg>';
  btn.disabled = true;
  // At the right end of the quick toolbar, after its own separator.
  const sep = document.createElement("span");
  sep.className = "sep";
  $("quickbar").append(sep, btn);

  const stage = $("stage");
  const canvas3d = document.createElement("canvas");
  canvas3d.id = "vol3d";
  canvas3d.hidden = true;
  stage.appendChild(canvas3d);
  // Axes with ticks and the measurements, drawn over the 3D picture.
  const over = document.createElement("canvas");
  over.id = "vol3d-over";
  over.hidden = true;
  stage.appendChild(over);

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
        <option value="3" id="vp-mode-seg" hidden>分割表面</option>
      </select>
    </label>
    <div id="vp-levels">
      <canvas id="vp-hist" title="密度直方图：拖动竖线调整显示的密度范围"></canvas>
      <div class="vp-row vp-nums">
        <span id="vp-lo-label">等值面</span>
        <input type="number" id="vp-lo" step="any" title="密度值">
        <input type="number" id="vp-hi" step="any" title="密度值">
      </div>
      <div class="vp-sigma" id="vp-sigma"></div>
    </div>
    <label id="vp-op-row">不透明度 <input type="range" id="vp-op" min="0.02" max="1" step="0.01" value="0.2"></label>
    <div class="vp-row vp-pre">
      <label class="vp-check" title="暗的结构显示为亮，冷冻电镜的断层和密度图一般要开"><input type="checkbox" id="vp-inv"> 反相</label>
      <label class="vp-inline" title="先做高斯平滑再显示，噪声大的断层用它才能看出连续的膜">平滑
        <select id="vp-smooth">
          <option value="0">无</option>
          <option value="1">轻</option>
          <option value="2">中</option>
          <option value="3">强</option>
        </select>
      </label>
    </div>
    <div id="vp-seg-rows" hidden>
      <label class="vp-check" title="在分割表面下面显示一张 XY 断层切片"><input type="checkbox" id="vp-slice-on" checked> 切片</label>
      <label>切片位置 <input type="range" id="vp-slice" min="0" max="1" step="0.001" value="0.5"></label>
      <div class="vp-obj" id="vp-obj" hidden></div>
      <div class="vp-row">
        <button id="vp-obj-all" hidden>全部显示</button>
        <button id="vp-export-obj" title="把显示中的分割表面导出成 OBJ 网格（附 .mtl 颜色），Blender、ChimeraX、Amira 等都能打开">导出网格</button>
        <button id="vp-export-png" title="把当前 3D 画面存成 PNG 图片">保存图片</button>
      </div>
    </div>
    <label id="vp-br-row">亮度 <input type="range" id="vp-br" min="0.2" max="4" step="0.05" value="1.2"></label>
    <label>Z 比例 <input type="range" id="vp-z" min="0.1" max="5" step="0.05" value="1"></label>
    <div id="vp-lab-rows" hidden>
      <label class="vp-check"><input type="checkbox" id="vp-lab" checked> 显示标签</label>
      <label>标签不透明度 <input type="range" id="vp-lab-op" min="0.05" max="1" step="0.01" value="0.7"></label>
    </div>
    <div class="vp-row vp-pre">
      <label class="vp-check" title="在体外框上标出长、宽、高的刻度（物理单位）"><input type="checkbox" id="vp-axes" checked> 标尺</label>
    </div>
    <div class="vp-hint" id="vp-measure-hint" hidden>测距：在结构上点起点，再点终点。点在表面或最亮的地方；Esc 取消。</div>
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
    $("vp-op-row").hidden = mode !== 1;
    $("vp-levels").hidden = mode === 3;
    $("vp-br-row").hidden = mode === 3;
    $("vp-seg-rows").hidden = mode !== 3;
    $("vp-lab-rows").hidden = mode === 3 || $("vp-mode-seg").hidden;
    $("vp-hi").hidden = mode === 2;
    $("vp-lo-label").textContent = mode === 2 ? "等值面" : "范围";
    syncLevels();
  }

  // --- density histogram with draggable levels (like ChimeraX's Volume Viewer) ---
  // Values are shown in the file's units: v in 0..1 maps to the first channel's min..max.
  const histCanvas = $("vp-hist");
  let hist = null; // {counts, min, max, mean, sd}

  /// `range` is the first channel's [a, b] kept by smoothing, in 0..1 of its raw values.
  function computeHist(vol, range = [0, 1]) {
    const c = vol.chans[0];
    const min = c.min + range[0] * (c.max - c.min), max = c.min + range[1] * (c.max - c.min);
    let counts;
    if (view.gpu) {
      counts = view.shownHistogram(0, 256);
    } else {
      counts = new Float64Array(256);
      for (const v of c.data) counts[v]++;
    }
    let n = 0, s1 = 0, s2 = 0;
    for (let b = 0; b < 256; b++) {
      const x = min + (b / 255) * (max - min);
      n += counts[b];
      s1 += counts[b] * x;
      s2 += counts[b] * x * x;
    }
    const mean = s1 / Math.max(1, n);
    const sd = Math.sqrt(Math.max(0, s2 / Math.max(1, n) - mean * mean));
    hist = { counts, min, max, mean, sd };
    // Mean and spread of every channel (0..1, before inverting), for per-channel levels.
    view.stats = vol.chans.slice(0, MAX_CHANNELS).map((ch, i) => {
      const k = i === 0 ? counts : view.gpu ? view.shownHistogram(i, 256) : (() => {
        const h = new Float64Array(256);
        for (const v of ch.data) h[v]++;
        return h;
      })();
      let n = 0, a = 0, b = 0;
      for (let j = 0; j < 256; j++) {
        const x = j / 255;
        n += k[j];
        a += k[j] * x;
        b += k[j] * x * x;
      }
      const m = a / Math.max(1, n);
      return { m, sd: Math.sqrt(Math.max(0, b / Math.max(1, n) - m * m)) };
    });
  }

  // With invert on, 0..1 runs from the brightest value down to the darkest.
  const inv = () => !!(view && view.settings.invert);
  const toValue = (v) => (inv() ? hist.max - v * (hist.max - hist.min) : hist.min + v * (hist.max - hist.min));
  const fromValue = (x) => {
    if (!(hist.max > hist.min)) return 0;
    const f = (x - hist.min) / (hist.max - hist.min);
    return inv() ? 1 - f : f;
  };
  const num = (x) => +x.toPrecision(4);
  // σ above the mean, without rounding noise such as -1.4e-16.
  const sigmas = (v) => {
    const k = (v - hist.mean) / hist.sd;
    return Math.abs(k) < 1e-6 ? 0 : num(k);
  };

  function drawHist() {
    const dpr = window.devicePixelRatio || 1;
    const w = histCanvas.clientWidth, h = histCanvas.clientHeight;
    if (!w) return;
    histCanvas.width = Math.round(w * dpr);
    histCanvas.height = Math.round(h * dpr);
    const g = histCanvas.getContext("2d");
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, h);
    if (!hist || !view) return;
    const logs = Array.from(hist.counts, (n) => Math.log1p(n));
    if (inv()) logs.reverse();
    const top = Math.max(...logs) || 1;
    g.fillStyle = "#8c959f";
    for (let b = 0; b < 256; b++) {
      const bh = (logs[b] / top) * (h - 4);
      g.fillRect((b / 256) * w, h - bh, Math.max(1, w / 256), bh);
    }
    const s = view.settings, [lo, hi] = s.levels[s.mode];
    const accent = getComputedStyle(document.documentElement).getPropertyValue("--accent").trim() || "#e71316";
    if (s.mode !== 2) {
      // Ramp from "starts to show" to "full".
      g.strokeStyle = accent;
      g.lineWidth = 1;
      g.beginPath();
      g.moveTo(0, h - 1);
      g.lineTo(lo * w, h - 1);
      g.lineTo(hi * w, 1);
      g.lineTo(w, 1);
      g.stroke();
    }
    g.lineWidth = 2;
    g.strokeStyle = accent;
    for (const v of s.mode === 2 ? [lo] : [lo, hi]) {
      g.beginPath();
      g.moveTo(v * w, 0);
      g.lineTo(v * w, h);
      g.stroke();
    }
  }

  function syncLevels() {
    if (!view || !hist) { drawHist(); return; }
    const s = view.settings, [lo, hi] = s.levels[s.mode];
    if (document.activeElement !== $("vp-lo")) $("vp-lo").value = num(toValue(lo));
    if (document.activeElement !== $("vp-hi")) $("vp-hi").value = num(toValue(hi));
    $("vp-sigma").textContent = hist.sd > 0 ? `${sigmas(toValue(lo))} σ · 平均 ${num(hist.mean)} · σ ${num(hist.sd)}` : "";
    drawHist();
  }

  /// Starting levels from the value spread, like ChimeraX's σ levels:
  /// MIP mean..mean+4σ, translucent mean+0.5σ..mean+3σ, surface mean+2σ
  /// (in the shown direction, so after invert they pick the dark side).
  let levelsFor = 0;
  function autoLevels() {
    if (!view || !hist || !(hist.sd > 0)) return;
    const span = hist.max - hist.min;
    let m = (hist.mean - hist.min) / span;
    const sd = hist.sd / span;
    if (inv()) m = 1 - m;
    const c = (x) => Math.max(0, Math.min(1, x));
    view.settings.levels = [
      [c(m), c(m + 4 * sd)],
      [c(m + 0.5 * sd), c(m + 3 * sd)],
      [c(m + 2 * sd), 1],
      [0, 1],
    ];
  }

  function setLevel(which, v) {
    const s = view.settings, lv = s.levels[s.mode].slice();
    v = Math.max(0, Math.min(1, v));
    if (which === 0) lv[0] = s.mode === 2 ? v : Math.min(v, lv[1] - 0.002);
    else lv[1] = Math.max(v, lv[0] + 0.002);
    s.levels[s.mode] = lv;
    syncLevels();
    view.draw(true);
    clearTimeout(view.settle);
    view.settle = setTimeout(() => view.draw(), 150);
  }

  let levelDrag = -1;
  histCanvas.addEventListener("pointerdown", (e) => {
    if (!view || !hist) return;
    const r = histCanvas.getBoundingClientRect();
    const v = (e.clientX - r.left) / r.width;
    const s = view.settings, [lo, hi] = s.levels[s.mode];
    levelDrag = s.mode === 2 || Math.abs(v - lo) <= Math.abs(v - hi) ? 0 : 1;
    histCanvas.setPointerCapture(e.pointerId);
    setLevel(levelDrag, v);
  });
  histCanvas.addEventListener("pointermove", (e) => {
    if (levelDrag < 0) return;
    const r = histCanvas.getBoundingClientRect();
    setLevel(levelDrag, (e.clientX - r.left) / r.width);
  });
  const endLevel = () => { levelDrag = -1; };
  histCanvas.addEventListener("pointerup", endLevel);
  histCanvas.addEventListener("pointercancel", endLevel);
  for (const [id, which] of [["vp-lo", 0], ["vp-hi", 1]]) {
    $(id).addEventListener("change", (e) => {
      const x = parseFloat(e.target.value);
      if (view && hist && Number.isFinite(x)) setLevel(which, fromValue(x));
    });
  }

  /// Cryo-ET tomograms (MRC slabs much thinner than wide) start inverted and
  /// lightly smoothed; other stacks start as they are. Per image.
  let defaultsFor = 0;
  function applyDefaults() {
    if (!view || defaultsFor === S.token || !S.info) return;
    defaultsFor = S.token;
    const i = S.info;
    const tomo = /mrc|rec|st\b/i.test(i.format) && i.sizeZ * 2 <= Math.max(i.width, i.height);
    view.settings.invert = tomo;
    view.settings.smooth = tomo ? 1 : 0;
    $("vp-inv").checked = view.settings.invert;
    $("vp-smooth").value = String(view.settings.smooth);
  }

  async function load() {
    applyDefaults();
    const chans = S.chans.map((c, i) => ({ c, i })).filter(({ c }) => c.visible).slice(0, MAX_CHANNELS);
    if (!chans.length) { status("没有显示中的通道"); return; }
    const token = S.token;
    const req = {
      token, t: S.t,
      channels: chans.map(({ c, i }) => ({ index: i, min: c.min ?? null, max: c.max ?? null })),
      maxVoxels: Math.floor(VOXEL_BUDGET / chans.length),
      // Without GPU float targets volume.rs smooths instead.
      smooth: view && !view.gpu ? view.settings.smooth : 0,
    };
    status("正在读取整个 Z 堆栈…");
    const started = performance.now();
    try {
      const vol = parseVolume(await invoke("volume_data", { req }));
      if (token !== S.token || !view) return;
      view.setVolume(vol, chans.map(({ c }) => c.color));
      computeHist(vol, view.smooth(view.settings.smooth)[0]);
      if (levelsFor !== token) {
        levelsFor = token;
        autoLevels();
      }
      syncLevels();
      shownToken = token;
      lastBudget = req.maxVoxels;
      await loadLabels();
      const secs = ((performance.now() - started) / 1000).toFixed(1);
      status(`${vol.w} × ${vol.h} × ${vol.d} 体素 · ${secs} 秒`);
      if (view && view.settings.mode === 3) await loadSurfaces();
    } catch (e) {
      status(String(e));
    }
  }

  let lastBudget = 0;

  /// Lays the segmentation labels of this image (labels.js) into the volume.
  async function loadLabels() {
    const labels = window.TifMrcViewPro.labels;
    const has = !!(labels && labels.token() === S.token && labels.any());
    $("vp-mode-seg").hidden = !has;
    if (!has && view && view.settings.mode === 3) setMode(0);
    syncRows();
    if (!view || !view.vol) return;
    if (!has) { view.setLabels(null); return; }
    view.setColorTable(labels.colorTable());
    const token = S.token;
    try {
      const buf = await invoke("labels_volume", { token, maxVoxels: lastBudget });
      if (token !== S.token || !view || !view.vol) return;
      const dv = new DataView(buf);
      const [w, h, d] = [0, 4, 8].map((o) => dv.getUint32(o, true));
      view.setLabels({ w, h, d, data: new Uint8Array(buf, 12, w * h * d) });
    } catch (e) {
      status(String(e));
    }
  }

  // --- segmentation surfaces (mode 3) ---
  let surfaces = null; // parsed mesh shown
  let surfToken = 0;
  let surfStale = true; // the labels changed since the surfaces were made
  const hiddenObjects = new Set();
  let dust = 0; // objects of fewer pixels are not drawn (figure.js)

  function setMode(m) {
    view.settings.mode = m;
    $("vp-mode").value = String(m);
    syncRows();
    view.draw();
  }

  /// Rebuilds the surfaces of the labels (labels_surfaces) and shows them.
  async function loadSurfaces() {
    if (!view || !view.vol || view.settings.mode !== 3) return;
    const token = S.token;
    status("正在生成分割表面…");
    const started = performance.now();
    try {
      const mesh = window.TifMrcViewPro.surfaces.parse(await invoke("labels_surfaces", { token }));
      if (token !== S.token || !view || !view.vol) return;
      surfaces = mesh;
      surfToken = token;
      surfStale = false;
      hiddenObjects.clear();
      view.setSurfaces(mesh);
      showObject(-1);
      const secs = ((performance.now() - started) / 1000).toFixed(1);
      status(`${mesh.objects.length.toLocaleString()} 个物体 · ${(mesh.tri.length / 3).toLocaleString()} 个三角形 · ${secs} 秒`);
    } catch (e) {
      status(String(e));
    }
  }

  function objectColor(i) {
    const labels = window.TifMrcViewPro.labels;
    const o = surfaces && surfaces.objects[i];
    if (!o || !labels || hiddenObjects.has(i) || o.pixels < dust || labels.hidden(o.label)) return null;
    return labels.colorOf(o.label).map((x) => x / 255);
  }

  /// Size of an object: pixels, and its volume when the voxel size is known.
  function objectSize(o) {
    let s = `约 ${o.pixels.toLocaleString()} 像素`;
    const pz = S.info && S.info.pixelSize && S.info.pixelSize[2];
    if (S.px && pz > 0) s += ` · ${+(o.pixels * S.px.x * S.px.y * pz).toPrecision(3)} ${S.px.unit}³`;
    return s;
  }

  // One-click classes for picked objects, coloured like published
  // segmentation figures (e.g. Houcek et al. 2026, Fig. 4). "囊泡" matches
  // the material membrane segmentation writes vesicles to.
  const CLASSES = [
    ["囊泡", [92, 190, 60]], ["其他囊泡", [140, 140, 140]], ["质膜", [86, 190, 233]], ["线粒体", [240, 220, 40]],
    ["内质网", [40, 60, 170]], ["微管", [240, 110, 20]], ["突触后致密区", [190, 140, 230]], ["其他膜", [255, 196, 0]],
  ];

  /// Shows what is known about the picked object (-1 = none) with its actions.
  function showObject(i) {
    const labels = window.TifMrcViewPro.labels;
    if (view) {
      view.selected = i;
      view.draw();
    }
    $("vp-obj-all").hidden = !hiddenObjects.size;
    const box = $("vp-obj");
    const o = i >= 0 && surfaces && surfaces.objects[i];
    box.hidden = !o;
    if (!o || !labels) return;
    box.replaceChildren();
    const title = document.createElement("div");
    title.className = "vp-obj-name";
    const sw = document.createElement("span");
    sw.className = "vp-swatch";
    sw.style.background = `rgb(${labels.colorOf(o.label).join(",")})`;
    title.append(sw, `${labels.nameOf(o.label)} · 物体 ${i + 1}`);
    const size = document.createElement("div");
    size.className = "dim";
    size.textContent = objectSize(o);
    const move = document.createElement("select");
    move.title = "把这个物体改成另一个标签（可以撤销）";
    move.append(new Option("改为标签…", ""));
    const used = labels.used();
    for (const k of used) if (k !== o.label) move.append(new Option(labels.nameOf(k), String(k)));
    move.append(new Option("新标签", String(Math.max(0, ...used) + 1)));
    move.addEventListener("change", () => {
      const k = +move.value;
      if (k) relabel(i, k);
    });
    const hide = document.createElement("button");
    hide.textContent = "隐藏";
    hide.title = "只在 3D 里隐藏这个物体，不改标签";
    hide.addEventListener("click", () => {
      hiddenObjects.add(i);
      showObject(-1);
    });
    const del = document.createElement("button");
    del.textContent = "删除";
    del.title = "把这个物体从标签里擦掉（可以撤销）";
    del.addEventListener("click", () => relabel(i, 0));
    const row = document.createElement("div");
    row.className = "vp-row";
    row.append(hide, del);
    const chips = document.createElement("div");
    chips.className = "vp-classes";
    chips.title = "把这个物体归到一类结构（可以撤销）";
    for (const [name, rgb] of CLASSES) {
      const b = document.createElement("button");
      b.className = "vp-class";
      b.textContent = name;
      b.style.setProperty("--c", `rgb(${rgb.join(",")})`);
      if (labels.nameOf(o.label) === name) b.classList.add("on");
      b.addEventListener("click", () => {
        if (labels.nameOf(o.label) === name) return;
        relabel(i, labels.materialLabel(name, rgb));
      });
      chips.append(b);
    }
    box.append(title, size, chips, move, row);
  }

  async function relabel(i, label) {
    const labels = window.TifMrcViewPro.labels;
    if (!labels || surfToken !== S.token) return;
    status("正在修改标签…");
    // labels.js reports the change through labelsChanged, which rebuilds the surfaces.
    await labels.relabelObject(i, label);
  }

  /// Saves the shown surfaces as OBJ; the dialog starts in `dir` when given
  /// (the figure flow's 5_figures). Returns the path saved, or null.
  async function exportMesh(dir) {
    const labels = window.TifMrcViewPro.labels;
    if (!surfaces || !labels || surfToken !== S.token) return null;
    const stem = S.info.path.split(/[\\/]/).pop().replace(/\.[^.]+$/, "");
    const path = await invoke("pick_save_path", { dir: dir || saveDir(), name: `${stem}_surfaces.obj`, filter: "OBJ", exts: ["obj"] }).catch(() => null);
    if (!path) return null;
    if (!dir) setSaveDir(dirname(path));
    const shown = [...new Set(surfaces.objects.map((o) => o.label))].filter((k) => !labels.hidden(k));
    const materials = shown.map((k) => ({ label: k, name: labels.nameOf(k), color: labels.colorOf(k) }));
    const pz = S.info.pixelSize && S.info.pixelSize[2];
    const known = !!(S.px && pz > 0);
    status("正在导出网格…");
    try {
      await invoke("labels_surfaces_export", {
        token: S.token, path, materials,
        skip: [...new Set([...hiddenObjects, ...surfaces.objects.map((o, i) => (o.pixels < dust ? i : -1)).filter((i) => i >= 0)])],
        scale: known ? [S.px.x, S.px.y, pz] : [1, 1, 1], unit: known ? S.px.unit : "pixel",
      });
      status(`已导出 ${path}`);
      return path;
    } catch (e) {
      status(String(e));
      return null;
    }
  }

  /// The 3D picture as PNG, without the part under the tools panel.
  async function exportPicture() {
    if (!view || !view.vol) return;
    const stem = S.info.path.split(/[\\/]/).pop().replace(/\.[^.]+$/, "");
    const path = await invoke("pick_save_path", { dir: saveDir(), name: `${stem}_3d.png` }).catch(() => null);
    if (!path) return;
    setSaveDir(dirname(path));
    view.render(false);
    const [x, , w, h] = view.lastViewport;
    const c = document.createElement("canvas");
    c.width = w;
    c.height = h;
    c.getContext("2d").drawImage(canvas3d, x, 0, w, h, 0, 0, w, h);
    const blob = await new Promise((res) => c.toBlob(res, "image/png"));
    try {
      await writeFile(path, new Uint8Array(await blob.arrayBuffer()));
      status(`已保存 ${path}`);
    } catch (e) {
      status(String(e));
    }
  }

  /// Opens the 3D view showing the segmentation as surfaces.
  function openSurfaces() {
    if (canvas3d.hidden) open();
    if (!view) return;
    if (view.settings.mode !== 3) setMode(3);
    if (view.vol && shownToken === S.token && (surfStale || surfToken !== S.token)) loadSurfaces();
  }

  function open() {
    if (!view) {
      try {
        view = new VolumeView(canvas3d);
        view.objectColor = objectColor;
      } catch (e) {
        alert(String(e.message || e));
        return;
      }
    }
    canvas3d.hidden = false;
    panel.hidden = false;
    over.hidden = false;
    // The 2D tools panel and the X / Y / Z sliders do nothing in 3D.
    document.body.classList.add("vol3d-on");
    view.onRender = drawOverlay;
    btn.classList.add("on");
    syncRows();
    view.draw();
    load();
  }

  function close() {
    panel.classList.remove("vp-figure");
    if (view) { view.settings.fig = null; view.settings.ortho = false; }
    dust = 0;
    if (window.TifMrcViewPro.figure) window.TifMrcViewPro.figure.closed3d();
    surfaces = null;
    hiddenObjects.clear();
    $("vp-obj").hidden = true;
    canvas3d.hidden = true;
    panel.hidden = true;
    over.hidden = true;
    document.body.classList.remove("vol3d-on");
    pending = null;
    hover = null;
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
  // Right button (a two-finger click-drag on a Mac trackpad, or Control +
  // drag), middle button or Shift + left drag pans.
  canvas3d.addEventListener("pointerdown", (e) => {
    if (e.button === 1 || e.button === 2) e.preventDefault();
    const pan = e.button === 1 || e.button === 2 || (e.button === 0 && (e.shiftKey || e.ctrlKey));
    drag = { x: e.clientX, y: e.clientY, x0: e.clientX, y0: e.clientY, yaw: view.yaw, pitch: view.pitch, pan };
    canvas3d.style.cursor = pan ? "move" : "";
    canvas3d.setPointerCapture(e.pointerId);
  });
  canvas3d.addEventListener("mousedown", (e) => { if (e.button === 1) e.preventDefault(); });
  // The right button pans, so no context menu over the 3D view.
  canvas3d.addEventListener("contextmenu", (e) => e.preventDefault());
  canvas3d.addEventListener("auxclick", (e) => e.preventDefault());
  canvas3d.addEventListener("pointermove", (e) => {
    if (!drag) {
      canvas3d.style.cursor = S.tool === "measure" ? "crosshair" : "";
      if (pending) hoverAt(e);
      return;
    }
    if (drag.pan) {
      view.panBy(e.clientX - drag.x, e.clientY - drag.y);
      drag.x = e.clientX;
      drag.y = e.clientY;
    } else {
      view.yaw = drag.yaw - (e.clientX - drag.x) * 0.01;
      view.pitch = Math.max(-1.55, Math.min(1.55, drag.pitch + (e.clientY - drag.y) * 0.01));
    }
    view.draw(true);
  });
  const endDrag = () => { if (drag) { drag = null; canvas3d.style.cursor = ""; view.draw(); } };
  // A click without dragging picks an object of the segmentation surfaces.
  canvas3d.addEventListener("pointerup", (e) => {
    const click = drag && !drag.pan && Math.hypot(e.clientX - drag.x0, e.clientY - drag.y0) < 4;
    if (click && S.tool === "measure") {
      const r = canvas3d.getBoundingClientRect();
      measureClick(e.clientX - r.left, e.clientY - r.top);
    } else if (click && view.settings.mode === 3 && surfaces) {
      const r = canvas3d.getBoundingClientRect();
      showObject(view.pickAt(e.clientX - r.left, e.clientY - r.top));
    }
    endDrag();
  });
  canvas3d.addEventListener("pointercancel", endDrag);
  canvas3d.addEventListener("wheel", (e) => {
    e.preventDefault();
    view.dist = Math.max(0.6, Math.min(8, view.dist * Math.exp(e.deltaY * 0.001)));
    view.draw(true);
    clearTimeout(view.settle);
    view.settle = setTimeout(() => view.draw(), 150);
  }, { passive: false });
  canvas3d.addEventListener("dblclick", () => view.resetView());
  const redraw = () => { if (view && !canvas3d.hidden) view.draw(); };
  new ResizeObserver(redraw).observe(canvas3d);
  // The volume is centred beside the tools panel, so follow it opening, closing and resizing.
  new ResizeObserver(redraw).observe($("tools"));
  new MutationObserver(redraw).observe($("tools"), { attributes: true, attributeFilter: ["hidden", "class"] });

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
  $("vp-mode").addEventListener("change", () => {
    if (view && view.settings.mode === 3 && (surfStale || surfToken !== S.token)) loadSurfaces();
  });
  $("vp-slice-on").addEventListener("change", (e) => {
    if (!view) return;
    view.settings.showSlice = e.target.checked;
    view.draw();
  });
  bind("vp-slice", "slice", parseFloat);
  $("vp-obj-all").addEventListener("click", () => {
    hiddenObjects.clear();
    showObject(-1);
  });
  $("vp-export-obj").addEventListener("click", () => exportMesh());
  $("vp-export-png").addEventListener("click", exportPicture);
  bind("vp-op", "opacity", parseFloat);
  bind("vp-br", "brightness", parseFloat);
  bind("vp-z", "zScale", parseFloat);
  bind("vp-lab-op", "labelOpacity", parseFloat);
  $("vp-inv").addEventListener("change", (e) => {
    if (!view) return;
    view.settings.invert = e.target.checked;
    autoLevels();
    syncLevels();
    view.draw();
  });
  $("vp-smooth").addEventListener("change", (e) => {
    if (!view) return;
    view.settings.smooth = +e.target.value;
    if (!view.gpu || !view.vol) { load(); return; }
    const started = performance.now();
    computeHist(view.vol, view.smooth(view.settings.smooth)[0]);
    syncLevels();
    status(`${view.vol.w} × ${view.vol.h} × ${view.vol.d} 体素 · 平滑 ${((performance.now() - started) / 1000).toFixed(2)} 秒`);
  });
  $("vp-lab").addEventListener("change", (e) => {
    if (!view) return;
    view.settings.showLabels = e.target.checked;
    view.draw();
  });
  $("vp-reset").addEventListener("click", () => view && view.resetView());
  $("vp-reload").addEventListener("click", load);

  // --- axes with physical ticks, and the ruler (the 2D measure tool) in 3D ---
  let pending = null; // first end of a measurement being made, [x, y, z]
  let hover = null; // where the second end would go

  /// Step of about n ticks over `len`: 1, 2 or 5 times a power of ten.
  function niceStep(len, n = 5) {
    const raw = len / n, p = 10 ** Math.floor(Math.log10(raw)), f = raw / p;
    return p * (f < 1.5 ? 1 : f < 3.5 ? 2 : f < 7.5 ? 5 : 10);
  }

  /// Length of the image along x, y and z with the unit shown on the axes.
  function axisLengths() {
    const i = S.info;
    if (!S.px || !UNIT_M[S.px.unit]) return { len: [i.width, i.height, i.sizeZ], unit: "px", zUnit: "层" };
    const m = UNIT_M[S.px.unit];
    const meters = [i.width * S.px.x * m, i.height * (S.axis === "xy" ? S.px.y : S.px.x) * m, i.sizeZ * zStep() * m];
    const unit = lengthUnit(Math.max(...meters));
    return { len: meters.map((x) => x / UNIT_M[unit]), unit, zUnit: unit };
  }

  function drawOverlay() {
    const dpr = window.devicePixelRatio || 1;
    const w = canvas3d.clientWidth, h = canvas3d.clientHeight;
    if (over.width !== Math.round(w * dpr) || over.height !== Math.round(h * dpr)) {
      over.width = Math.round(w * dpr);
      over.height = Math.round(h * dpr);
    }
    const g = over.getContext("2d");
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.clearRect(0, 0, over.width, over.height);
    $("vp-measure-hint").hidden = S.tool !== "measure";
    if (!view || !view.vol || !S.info || canvas3d.hidden) return;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const cam = view.cssCamera();
    const light = view.settings.mode === 3 && !view.settings.fig;
    if ($("vp-axes").checked && !view.settings.fig) drawAxes(g, cam, light);
    drawMeasures(g, cam, light);
  }

  function drawAxes(g, cam, light) {
    const e = view.extent();
    const P = (p) => view.project(p, cam);
    const corners = [];
    for (let k = 0; k < 8; k++) corners.push([k & 1 ? e[0] : -e[0], k & 2 ? e[1] : -e[1], k & 4 ? e[2] : -e[2]]);
    const pts = corners.map(P);
    if (pts.some((p) => !p)) return;
    const ink = light ? "rgba(40,44,52,.75)" : "rgba(255,255,255,.6)";
    const text = light ? "#1f2328" : "#fff";
    g.save();
    g.lineWidth = 1;
    g.strokeStyle = ink;
    g.beginPath();
    for (let a = 0; a < 8; a++) for (const bit of [1, 2, 4]) {
      if (a & bit) continue;
      g.moveTo(...pts[a]);
      g.lineTo(...pts[a | bit]);
    }
    g.stroke();
    const cx = pts.reduce((s, p) => s + p[0], 0) / 8, cy = pts.reduce((s, p) => s + p[1], 0) / 8;
    const { len, unit, zUnit } = axisLengths();
    const names = ["X", "Y", "Z"];
    g.font = "11px -apple-system, system-ui, sans-serif";
    g.fillStyle = text;
    for (let ax = 0; ax < 3; ax++) {
      // Of the four edges along this axis, the one lowest on screen (for Y, the leftmost).
      const others = [0, 1, 2].filter((k) => k !== ax);
      let best = null;
      for (const sa of [-1, 1]) for (const sb of [-1, 1]) {
        const at = (f) => { const p = [0, 0, 0]; p[ax] = f; p[others[0]] = sa * e[others[0]]; p[others[1]] = sb * e[others[1]]; return p; };
        const a = P(at(-e[ax])), b = P(at(e[ax]));
        if (!a || !b) continue;
        const mid = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
        const score = ax === 1 ? -mid[0] : mid[1];
        if (!best || score > best.score) best = { score, at, mid };
      }
      if (!best) continue;
      let ox = best.mid[0] - cx, oy = best.mid[1] - cy;
      const ol = Math.hypot(ox, oy) || 1;
      ox /= ol; oy /= ol;
      // About one tick per 70 screen pixels, at least two.
      const a0 = P(best.at(-e[ax])), a1 = P(best.at(e[ax]));
      const span = Math.hypot(a1[0] - a0[0], a1[1] - a0[1]);
      const step = niceStep(len[ax], Math.max(1.5, Math.min(8, span / 70)));
      // Fraction along the image axis -> scene coordinate (image y runs downwards).
      const world = (f) => (ax === 1 ? (0.5 - f) : (f - 0.5)) * 2 * e[ax];
      const digits = Math.max(0, -Math.floor(Math.log10(step) + 1e-9));
      // The axis name sits outside the middle of the edge; tick labels next to it give way.
      const tip = [best.mid[0] + ox * 34, best.mid[1] + oy * 30];
      g.textAlign = "center";
      g.textBaseline = "middle";
      for (let val = 0; val <= len[ax] + step * 1e-6; val += step) {
        const p = P(best.at(world(val / len[ax])));
        if (!p) continue;
        g.beginPath();
        g.moveTo(p[0], p[1]);
        g.lineTo(p[0] + ox * 5, p[1] + oy * 5);
        g.stroke();
        const lx = p[0] + ox * 16, ly = p[1] + oy * 14;
        if (Math.hypot(lx - tip[0], ly - tip[1]) > 30) g.fillText(val.toFixed(digits), lx, ly);
      }
      g.font = "600 12px -apple-system, system-ui, sans-serif";
      g.fillText(`${names[ax]} (${ax === 2 ? zUnit : unit})`, tip[0], tip[1]);
      g.font = "11px -apple-system, system-ui, sans-serif";
    }
    g.restore();
  }

  /// Every measurement of the image: drawn in 2D, XZ / YZ or here.
  function drawMeasures(g, cam, light) {
    if (!S.annShow) return;
    const items = S.ann.filter((a) => a.type === "line" && a.pts && a.pts.length >= 2);
    const ends = (a) => a.p3 || [[...a.pts[0], a.z ?? 0], [...a.pts[a.pts.length - 1], a.z ?? 0]];
    g.save();
    g.lineCap = "round";
    g.font = "600 12px -apple-system, system-ui, sans-serif";
    g.textAlign = "center";
    g.textBaseline = "bottom";
    const line = (p3, color, label) => {
      const [a, b] = p3.map((q) => view.project(view.toWorld(q), cam));
      if (!a || !b) return;
      g.strokeStyle = color;
      g.fillStyle = color;
      g.lineWidth = 2;
      g.beginPath();
      g.moveTo(...a);
      g.lineTo(...b);
      g.stroke();
      for (const p of [a, b]) { g.beginPath(); g.arc(p[0], p[1], 3.5, 0, Math.PI * 2); g.fill(); }
      if (!label) return;
      const mx = (a[0] + b[0]) / 2, my = (a[1] + b[1]) / 2 - 6;
      const tw = g.measureText(label).width;
      g.fillStyle = light ? "rgba(255,255,255,.85)" : "rgba(31,35,40,.85)";
      g.beginPath();
      g.roundRect(mx - tw / 2 - 6, my - 15, tw + 12, 19, 5);
      g.fill();
      g.fillStyle = light ? "#1f2328" : "#fff";
      g.fillText(label, mx, my);
    };
    for (const a of items) line(ends(a), a.color || "#ffb020", annLength(a).text.split("  (")[0]);
    if (pending) {
      const color = $("ann-color").value;
      if (hover) line([pending, hover], color, lineLength3(pending, hover).text.split("  (")[0]);
      else {
        const p = view.project(view.toWorld(pending), cam);
        if (p) { g.fillStyle = color; g.beginPath(); g.arc(p[0], p[1], 4, 0, Math.PI * 2); g.fill(); }
      }
    }
    g.restore();
  }

  function measureClick(x, y) {
    const q = view.pick(x, y);
    if (!q) {
      status("这里没有碰到结构：点在看得见的表面或亮处上");
      return;
    }
    if (!pending) {
      pending = q;
      hover = null;
      status("再点终点");
      drawOverlay();
      return;
    }
    const snap = snapshot();
    const a = setLineEnds({ type: "line", color: $("ann-color").value, width: +$("ann-width").value }, [pending, q]);
    pending = null;
    hover = null;
    pushHistory(snap);
    toggleAnn(true);
    S.ann.push(a);
    S.lastDrawn = a;
    status(`测距 ${annLength(a).text}`);
    annChanged();
    drawOverlay();
  }

  let hoverFrame = 0;
  function hoverAt(e) {
    const r = canvas3d.getBoundingClientRect();
    const x = e.clientX - r.left, y = e.clientY - r.top;
    if (hoverFrame) cancelAnimationFrame(hoverFrame);
    hoverFrame = requestAnimationFrame(() => {
      hoverFrame = 0;
      if (!pending || !view) return;
      hover = view.pick(x, y);
      drawOverlay();
    });
  }

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && pending && !canvas3d.hidden) {
      pending = null;
      hover = null;
      status("");
      drawOverlay();
    }
  });
  window.addEventListener("tmv-annotations", () => { if (view && !canvas3d.hidden) drawOverlay(); });
  $("vp-axes").addEventListener("change", () => drawOverlay());
  // Picking another tool shows or hides the hint, and drops a half-made measurement.
  $("toolbtns").addEventListener("click", () => setTimeout(() => {
    if (S.tool !== "measure") { pending = null; hover = null; }
    if (view && !canvas3d.hidden) drawOverlay();
  }));

  // Labels changed in 2D: refresh them in the open 3D view.
  let labelTimer = 0;
  function labelsChanged() {
    surfStale = true;
    if (canvas3d.hidden || !view || !view.vol) return;
    clearTimeout(labelTimer);
    labelTimer = setTimeout(async () => {
      await loadLabels();
      if (view && view.settings.mode === 3) loadSurfaces();
    }, 300);
  }

  // Label colours or visibility changed: only the colour table needs updating.
  function materialsChanged() {
    const labels = window.TifMrcViewPro.labels;
    if (!view || canvas3d.hidden || !labels || labels.token() !== S.token) return;
    view.setColorTable(labels.colorTable());
    if (surfaces) showObject(view.selected);
  }

  /// Figure mode (figure.js): the surfaces drawn with `fig` settings and the
  /// parallel projection, the expert 3D panel put away; null leaves it.
  function setFigure(fig) {
    if (!view) return;
    view.settings.fig = fig;
    view.settings.ortho = !!fig;
    panel.classList.toggle("vp-figure", !!fig);
    view.draw();
  }

  window.TifMrcViewPro.volume = {
    open, close, openSurfaces, parseVolume, labelsChanged, materialsChanged,
    setFigure, exportMesh,
    view: () => view,
    isOpen: () => !canvas3d.hidden,
    canvas: canvas3d,
    /** Surfaces of the shown image are loaded and up to date. */
    surfacesReady: () => !!(view && surfaces && surfToken === S.token && !surfStale && view.settings.mode === 3),
    /** Objects hidden in 3D only. */
    hiddenObjects: () => hiddenObjects.size,
    showAll: () => { hiddenObjects.clear(); showObject(-1); },
    mesh: () => surfaces,
    /** Hides objects of fewer than `n` pixels (0 = none). */
    setDust: (n) => { dust = n; if (view) { showObject(-1); } },
    status: (t) => status(t),
  };
})();
