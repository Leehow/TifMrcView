"use strict";
// Pro 3D view: smooth surfaces of the segmentation (labels_surfaces, see
// surfaces.rs) drawn lit over a slice of the volume, the way Amira shows a
// segmented tomogram. volume.js owns the camera and the volume textures and
// calls draw() and pick(); each object is drawn on its own, so objects can be
// hidden, highlighted and picked one by one.
(() => {
  // Box coordinates as in volume.js: pixel p of a w x h x d stack maps to
  // t = p / dims in 0..1, and the box spans -extent..extent with Y flipped.
  const MESH_VERT = `#version 300 es
in vec3 pos;
in vec3 nrm;
uniform mat4 viewProj;
uniform vec3 dims;
uniform vec3 extent;
out vec3 n;
out vec3 world;
out vec3 tc;   // position in the label map, 0..1
out vec3 npx;  // normal in label-map pixels
void main() {
  vec3 t = pos / dims;
  tc = t;
  npx = nrm;
  world = vec3((t.x - 0.5) * 2.0 * extent.x, (0.5 - t.y) * 2.0 * extent.y, (t.z - 0.5) * 2.0 * extent.z);
  vec3 s = 2.0 * extent / dims;
  n = vec3(nrm.x / s.x, -nrm.y / s.y, nrm.z / s.z);
  gl_Position = viewProj * vec4(world, 1.0);
}`;

  // Lit surfaces. For figures (ao > 0) the light a point gets is dimmed by
  // the segmentation around it: rays in 14 directions over its side of the
  // surface are marched through the label volume (the reduced labels the
  // volume view already holds), so creases, the inside of a vesicle cluster
  // and membranes close to each other darken like ambient occlusion.
  const MESH_FRAG = `#version 300 es
precision highp float;
precision highp usampler3D;
in vec3 n;
in vec3 world;
in vec3 tc;
in vec3 npx;
uniform vec3 color;
uniform vec3 eye;
uniform float glow;      // > 0 for the picked object
uniform int pickPass;
uniform vec3 pickColor;
uniform int ortho;       // eye is a direction, not a point
uniform float ao;        // 0 = off .. 1
uniform int quick;       // while the view is dragged: no occlusion rays, an even dimming instead
uniform float gloss;     // highlight strength
uniform usampler3D labels;
uniform sampler2D lut;
uniform vec3 labDims;    // voxels of the label volume
uniform vec3 dims;       // pixels of the label map
out vec4 outColor;

bool solid(vec3 p) {
  if (any(lessThan(p, vec3(0.0))) || any(greaterThan(p, vec3(1.0)))) return false;
  uint l = texture(labels, p).r;
  return l != 0u && texelFetch(lut, ivec2(int(l), 0), 0).a > 0.5;
}

float occlusion() {
  // Directions in voxels of the label volume.
  vec3 nt = normalize(npx * dims / labDims);
  const vec3 D[14] = vec3[14](
    vec3(1, 0, 0), vec3(-1, 0, 0), vec3(0, 1, 0), vec3(0, -1, 0), vec3(0, 0, 1), vec3(0, 0, -1),
    vec3(0.577, 0.577, 0.577), vec3(-0.577, 0.577, 0.577), vec3(0.577, -0.577, 0.577), vec3(0.577, 0.577, -0.577),
    vec3(-0.577, -0.577, 0.577), vec3(-0.577, 0.577, -0.577), vec3(0.577, -0.577, -0.577), vec3(-0.577, -0.577, -0.577));
  float r0 = max(labDims.x, max(labDims.y, labDims.z)) / 48.0; // reach of the rays
  float hit = 0.0, sum = 0.0;
  for (int i = 0; i < 14; i++) {
    float c = dot(D[i], nt);
    if (c < 0.15) continue;
    sum += c;
    for (int k = 1; k <= 6; k++) {
      float r = 1.5 + r0 * float(k * k) / 36.0;
      if (solid(tc + D[i] * r / labDims)) { hit += c * (1.0 - 0.5 * float(k - 1) / 5.0); break; }
    }
  }
  return sum > 0.0 ? hit / sum : 0.0;
}

void main() {
  if (pickPass == 1) { outColor = vec4(pickColor, 1.0); return; }
  vec3 v = ortho == 1 ? normalize(eye) : normalize(eye - world);
  vec3 nn = normalize(n);
  if (dot(nn, v) < 0.0) nn = -nn; // lit from both sides
  // Key light above and left of the camera, plus a weak fill from the view.
  vec3 up = abs(v.y) > 0.99 ? vec3(0, 0, 1) : vec3(0, 1, 0);
  vec3 side = normalize(cross(up, v));
  vec3 l = normalize(v + 0.6 * normalize(cross(v, side)) - 0.4 * side);
  float diff = max(dot(nn, l), 0.0);
  float fill = max(dot(nn, v), 0.0);
  float spec = pow(max(dot(nn, normalize(l + v)), 0.0), 48.0);
  float a = ao > 0.0 ? 1.0 - ao * (quick == 1 ? 0.2 : occlusion()) : 1.0;
  // Figures get more ambient light, so shaded and occluded parts keep their colour.
  vec3 c = ao > 0.0
    ? color * (0.34 * a + (0.48 * diff + 0.26 * fill) * mix(1.0, a, 0.45)) + vec3(gloss * spec * a)
    : color * (0.18 + 0.62 * diff + 0.25 * fill) + vec3(gloss * spec);
  outColor = vec4(mix(c, vec3(1.0), glow), 1.0);
}`;

  // An XY slice of the volume at depth z (box coordinates), two triangles from gl_VertexID.
  const SLICE_VERT = `#version 300 es
uniform mat4 viewProj;
uniform vec3 extent;
uniform float z;
out vec3 tc;
void main() {
  int i = gl_VertexID;
  vec2 c = vec2(i == 1 || i == 2 || i == 4 ? 1.0 : -1.0, i == 2 || i == 4 || i == 5 ? 1.0 : -1.0);
  vec3 p = vec3(c * extent.xy, z);
  tc = vec3(p.xy / extent.xy * 0.5 + 0.5, p.z / extent.z * 0.5 + 0.5);
  tc.y = 1.0 - tc.y;
  gl_Position = viewProj * vec4(p, 1.0);
}`;

  const SLICE_FRAG = `#version 300 es
precision highp float;
precision highp sampler3D;
in vec3 tc;
uniform sampler3D vol;
uniform vec2 remap;  // stretch of the volume texture: (start, scale)
uniform vec2 window; // grey levels: (black, white) after the stretch
uniform int pickPass;
out vec4 outColor;
void main() {
  if (pickPass == 1) { outColor = vec4(0.0); return; }
  float v = clamp((texture(vol, tc).r - remap.x) * remap.y, 0.0, 1.0);
  float g = clamp((v - window.x) / max(window.y - window.x, 1e-3), 0.0, 1.0);
  outColor = vec4(vec3(g), 1.0);
}`;

  /// Parses the reply of `labels_surfaces` (see Surfaces::encode).
  function parse(buf) {
    const dv = new DataView(buf);
    const [w, h, d, nObj, nVert, nTri] = [0, 4, 8, 12, 16, 20].map((o) => dv.getUint32(o, true));
    const objects = [];
    let off = 24;
    for (let i = 0; i < nObj; i++, off += 32) {
      const u = (k) => dv.getUint32(off + k * 4, true);
      objects.push({ label: u(0), firstTri: u(1), tris: u(2), seed: [u(3), u(4), u(5)], pixels: u(6) });
    }
    const pos = new Float32Array(buf, off, nVert * 3);
    off += nVert * 12;
    const nrm = new Int8Array(buf, off, nVert * 4);
    off += nVert * 4;
    const tri = new Uint32Array(buf, off, nTri * 3);
    return { dims: [w, h, d], objects, pos, nrm, tri };
  }

  class SurfaceScene {
    constructor(gl) {
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
      this.mesh = link(MESH_VERT, MESH_FRAG);
      this.slice = link(SLICE_VERT, SLICE_FRAG);
      this.um = {};
      for (const k of ["viewProj", "dims", "extent", "color", "eye", "glow", "pickPass", "pickColor", "ortho", "ao", "quick", "gloss", "labels", "lut", "labDims"]) this.um[k] = gl.getUniformLocation(this.mesh, k);
      this.us = {};
      for (const k of ["viewProj", "extent", "z", "vol", "remap", "window", "pickPass"]) this.us[k] = gl.getUniformLocation(this.slice, k);
      this.vao = gl.createVertexArray();
      this.emptyVao = gl.createVertexArray();
      this.buffers = [];
      this.data = null;
      this.pickFbo = null;
    }

    /// Mesh from `parse`, or null.
    setMesh(m) {
      const gl = this.gl;
      this.buffers.forEach((b) => gl.deleteBuffer(b));
      this.buffers = [];
      this.data = m;
      if (!m) return;
      gl.bindVertexArray(this.vao);
      const attr = (name, data, size, type, norm, stride) => {
        const b = gl.createBuffer();
        gl.bindBuffer(gl.ARRAY_BUFFER, b);
        gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
        const loc = gl.getAttribLocation(this.mesh, name);
        gl.enableVertexAttribArray(loc);
        gl.vertexAttribPointer(loc, size, type, norm, stride, 0);
        this.buffers.push(b);
      };
      attr("pos", m.pos, 3, gl.FLOAT, false, 12);
      attr("nrm", m.nrm, 3, gl.BYTE, true, 4);
      const ib = gl.createBuffer();
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ib);
      gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, m.tri, gl.STATIC_DRAW);
      this.buffers.push(ib);
      gl.bindVertexArray(null);
    }

    /// Draws the slice and the shown objects into the bound framebuffer.
    /// o: {vp, eye, extent, slice: {tex, unit, remap, window, z} | null,
    ///     colorOf(object) -> [r, g, b] in 0..1 or null when hidden, selected, pick}
    draw(o) {
      const gl = this.gl;
      gl.enable(gl.DEPTH_TEST);
      gl.depthFunc(gl.LEQUAL);
      gl.clear(gl.DEPTH_BUFFER_BIT);
      const pick = o.pick ? 1 : 0;
      if (o.slice) {
        gl.useProgram(this.slice);
        gl.bindVertexArray(this.emptyVao);
        gl.activeTexture(gl.TEXTURE0 + o.slice.unit);
        gl.bindTexture(gl.TEXTURE_3D, o.slice.tex);
        gl.uniform1i(this.us.vol, o.slice.unit);
        gl.uniformMatrix4fv(this.us.viewProj, false, o.vp);
        gl.uniform3fv(this.us.extent, o.extent);
        gl.uniform1f(this.us.z, o.slice.z);
        gl.uniform2fv(this.us.remap, o.slice.remap);
        gl.uniform2fv(this.us.window, o.slice.window);
        gl.uniform1i(this.us.pickPass, pick);
        gl.drawArrays(gl.TRIANGLES, 0, 6);
      }
      const m = this.data;
      if (m && m.objects.length) {
        gl.useProgram(this.mesh);
        gl.bindVertexArray(this.vao);
        gl.uniformMatrix4fv(this.um.viewProj, false, o.vp);
        gl.uniform3fv(this.um.dims, m.dims);
        gl.uniform3fv(this.um.extent, o.extent);
        gl.uniform3fv(this.um.eye, o.eye);
        gl.uniform1i(this.um.pickPass, pick);
        gl.uniform1i(this.um.ortho, o.ortho ? 1 : 0);
        gl.uniform1f(this.um.gloss, o.gloss ?? 0.35);
        // Occlusion needs the label volume (o.labels: {tex, unit, lut, lutUnit, dims}).
        const lab = !pick && o.ao > 0 && o.labels;
        gl.uniform1f(this.um.ao, lab ? o.ao : 0);
        gl.uniform1i(this.um.quick, o.quick ? 1 : 0);
        if (lab) {
          gl.activeTexture(gl.TEXTURE0 + lab.unit);
          gl.bindTexture(gl.TEXTURE_3D, lab.tex);
          gl.activeTexture(gl.TEXTURE0 + lab.lutUnit);
          gl.bindTexture(gl.TEXTURE_2D, lab.lut);
          gl.uniform1i(this.um.labels, lab.unit);
          gl.uniform1i(this.um.lut, lab.lutUnit);
          gl.uniform3fv(this.um.labDims, lab.dims);
        } else {
          // Samplers of different types may not share a unit, even unused.
          gl.uniform1i(this.um.labels, 13);
          gl.uniform1i(this.um.lut, 14);
        }
        m.objects.forEach((ob, i) => {
          const c = o.colorOf(i);
          if (!c || !ob.tris) return;
          if (pick) {
            const id = i + 1;
            gl.uniform3f(this.um.pickColor, (id & 255) / 255, ((id >> 8) & 255) / 255, ((id >> 16) & 255) / 255);
          } else {
            gl.uniform3fv(this.um.color, c);
            gl.uniform1f(this.um.glow, i === o.selected ? 0.25 : 0);
          }
          gl.drawElements(gl.TRIANGLES, ob.tris * 3, gl.UNSIGNED_INT, ob.firstTri * 12);
        });
        gl.bindVertexArray(null);
      }
      gl.disable(gl.DEPTH_TEST);
    }

    /// Index of the object under device pixel (x, y) of a w x h canvas
    /// (y from the top), or -1. `o` as for draw; `viewport` [x, y, w, h].
    pick(o, x, y, w, h, viewport) {
      const gl = this.gl;
      if (!this.pickFbo || this.pickSize[0] !== w || this.pickSize[1] !== h) {
        if (this.pickFbo) {
          gl.deleteFramebuffer(this.pickFbo);
          gl.deleteRenderbuffer(this.pickColor);
          gl.deleteRenderbuffer(this.pickDepth);
        }
        this.pickFbo = gl.createFramebuffer();
        this.pickColor = gl.createRenderbuffer();
        this.pickDepth = gl.createRenderbuffer();
        gl.bindRenderbuffer(gl.RENDERBUFFER, this.pickColor);
        gl.renderbufferStorage(gl.RENDERBUFFER, gl.RGBA8, w, h);
        gl.bindRenderbuffer(gl.RENDERBUFFER, this.pickDepth);
        gl.renderbufferStorage(gl.RENDERBUFFER, gl.DEPTH_COMPONENT24, w, h);
        gl.bindFramebuffer(gl.FRAMEBUFFER, this.pickFbo);
        gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.RENDERBUFFER, this.pickColor);
        gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.DEPTH_ATTACHMENT, gl.RENDERBUFFER, this.pickDepth);
        this.pickSize = [w, h];
      }
      gl.bindFramebuffer(gl.FRAMEBUFFER, this.pickFbo);
      gl.viewport(...viewport);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      this.draw({ ...o, pick: true });
      const px = new Uint8Array(4);
      gl.readPixels(Math.round(x), Math.round(h - 1 - y), 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      return px[0] + (px[1] << 8) + (px[2] << 16) - 1;
    }

    dispose() {
      this.setMesh(null);
    }
  }

  // --- figures: the scene drawn into an offscreen target, then laid on the
  // background with outlines where the depth jumps (object edges), the way
  // ChimeraX draws silhouettes. Used for the figure styles and for export
  // at any size.
  const FULL_VERT = `#version 300 es
out vec2 uv;
void main() {
  vec2 p = vec2(float((gl_VertexID & 1) * 4 - 1), float((gl_VertexID >> 1) * 4 - 1));
  uv = p * 0.5 + 0.5;
  gl_Position = vec4(p, 0.0, 1.0);
}`;

  const COMPOSE_FRAG = `#version 300 es
precision highp float;
in vec2 uv;
uniform sampler2D col;
uniform sampler2D dep;
uniform vec2 px;          // 1 / size
uniform vec4 bg;          // background, alpha 0 = transparent
uniform float outline;    // 0 = none .. 1
uniform float width;      // outline width in pixels
uniform vec3 lineColor;
uniform float cue;        // depth cue: how far the back fades into the background
uniform vec2 cueRange;    // depth of the front and the back of the scene
out vec4 outColor;
void main() {
  float d = texture(dep, uv).r;
  vec4 c = texture(col, uv);
  c.rgb = mix(c.rgb, bg.rgb, cue * smoothstep(cueRange.x, cueRange.y, d));
  // Largest depth step to a neighbour within the outline width; the
  // background counts as far away, so objects get a silhouette.
  float e = 0.0;
  if (outline > 0.0) {
    for (int i = 0; i < 8; i++) {
      float a = float(i) * 0.785398;
      vec2 o = vec2(cos(a), sin(a)) * width * px;
      float q = texture(dep, uv + o).r;
      e = max(e, abs(q - d));
    }
    e = smoothstep(0.002, 0.02, e) * outline;
  }
  if (d >= 1.0) {
    // Background pixel next to an object: the outer half of the line.
    outColor = e > 0.0 ? vec4(mix(bg.rgb, lineColor, e), max(bg.a, e)) : bg;
    return;
  }
  outColor = vec4(mix(c.rgb, lineColor, e), 1.0);
}`;

  class FigureTarget {
    constructor(gl) {
      this.gl = gl;
      const sh = (type, src) => {
        const x = gl.createShader(type);
        gl.shaderSource(x, src);
        gl.compileShader(x);
        if (!gl.getShaderParameter(x, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(x));
        return x;
      };
      const p = gl.createProgram();
      gl.attachShader(p, sh(gl.VERTEX_SHADER, FULL_VERT));
      gl.attachShader(p, sh(gl.FRAGMENT_SHADER, COMPOSE_FRAG));
      gl.linkProgram(p);
      if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
      this.prog = p;
      this.u = {};
      for (const k of ["col", "dep", "px", "bg", "outline", "width", "lineColor", "cue", "cueRange"]) this.u[k] = gl.getUniformLocation(p, k);
      this.vao = gl.createVertexArray();
      this.size = [0, 0];
    }

    /// Binds an offscreen w x h target (colour + depth texture), cleared.
    begin(w, h) {
      const gl = this.gl;
      if (this.size[0] !== w || this.size[1] !== h) {
        this.free();
        this.fbo = gl.createFramebuffer();
        this.col = gl.createTexture();
        gl.bindTexture(gl.TEXTURE_2D, this.col);
        gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA8, w, h);
        this.dep = gl.createTexture();
        gl.bindTexture(gl.TEXTURE_2D, this.dep);
        gl.texStorage2D(gl.TEXTURE_2D, 1, gl.DEPTH_COMPONENT24, w, h);
        for (const t of [this.col, this.dep]) {
          gl.bindTexture(gl.TEXTURE_2D, t);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        }
        gl.bindTexture(gl.TEXTURE_2D, null);
        gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
        gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, this.col, 0);
        gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.DEPTH_ATTACHMENT, gl.TEXTURE_2D, this.dep, 0);
        this.size = [w, h];
      }
      gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
      gl.viewport(0, 0, w, h);
      gl.clearColor(0, 0, 0, 0);
      gl.clearDepth(1);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    }

    /// Lays the target onto framebuffer `dst` (null = canvas), w x h.
    /// style: {bg: [r, g, b, a] 0..1, outline 0..1, width px, line: [r, g, b], cue 0..1, cueRange: [near, far] depth}
    compose(dst, w, h, style) {
      const gl = this.gl;
      gl.bindFramebuffer(gl.FRAMEBUFFER, dst);
      gl.viewport(0, 0, w, h);
      gl.disable(gl.DEPTH_TEST);
      gl.disable(gl.BLEND);
      gl.useProgram(this.prog);
      gl.bindVertexArray(this.vao);
      gl.activeTexture(gl.TEXTURE0 + 13);
      gl.bindTexture(gl.TEXTURE_2D, this.col);
      gl.activeTexture(gl.TEXTURE0 + 14);
      gl.bindTexture(gl.TEXTURE_2D, this.dep);
      gl.uniform1i(this.u.col, 13);
      gl.uniform1i(this.u.dep, 14);
      gl.uniform2f(this.u.px, 1 / this.size[0], 1 / this.size[1]);
      gl.uniform4fv(this.u.bg, style.bg);
      gl.uniform1f(this.u.outline, style.outline || 0);
      gl.uniform1f(this.u.width, style.width || 1.5);
      gl.uniform3fv(this.u.lineColor, style.line || [0.08, 0.08, 0.1]);
      gl.uniform1f(this.u.cue, style.cue || 0);
      gl.uniform2fv(this.u.cueRange, style.cueRange || [0, 1]);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      gl.bindTexture(gl.TEXTURE_2D, null);
      gl.activeTexture(gl.TEXTURE0 + 13);
      gl.bindTexture(gl.TEXTURE_2D, null);
      gl.bindVertexArray(null);
    }

    free() {
      const gl = this.gl;
      if (this.fbo) gl.deleteFramebuffer(this.fbo);
      if (this.col) gl.deleteTexture(this.col);
      if (this.dep) gl.deleteTexture(this.dep);
      this.fbo = this.col = this.dep = null;
      this.size = [0, 0];
    }
  }

  window.TifMrcViewPro.surfaces = { SurfaceScene, FigureTarget, parse };
})();
