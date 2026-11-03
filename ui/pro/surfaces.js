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
void main() {
  vec3 t = pos / dims;
  world = vec3((t.x - 0.5) * 2.0 * extent.x, (0.5 - t.y) * 2.0 * extent.y, (t.z - 0.5) * 2.0 * extent.z);
  vec3 s = 2.0 * extent / dims;
  n = vec3(nrm.x / s.x, -nrm.y / s.y, nrm.z / s.z);
  gl_Position = viewProj * vec4(world, 1.0);
}`;

  const MESH_FRAG = `#version 300 es
precision highp float;
in vec3 n;
in vec3 world;
uniform vec3 color;
uniform vec3 eye;
uniform float glow;      // > 0 for the picked object
uniform int pickPass;
uniform vec3 pickColor;
out vec4 outColor;
void main() {
  if (pickPass == 1) { outColor = vec4(pickColor, 1.0); return; }
  vec3 v = normalize(eye - world);
  vec3 nn = normalize(n);
  if (dot(nn, v) < 0.0) nn = -nn; // lit from both sides
  // Key light above and left of the camera, plus a weak fill from the view.
  vec3 up = abs(v.y) > 0.99 ? vec3(0, 0, 1) : vec3(0, 1, 0);
  vec3 side = normalize(cross(up, v));
  vec3 l = normalize(v + 0.6 * normalize(cross(v, side)) - 0.4 * side);
  float diff = max(dot(nn, l), 0.0);
  float fill = max(dot(nn, v), 0.0);
  float spec = pow(max(dot(nn, normalize(l + v)), 0.0), 48.0);
  vec3 c = color * (0.18 + 0.62 * diff + 0.25 * fill) + vec3(0.35 * spec);
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
      for (const k of ["viewProj", "dims", "extent", "color", "eye", "glow", "pickPass", "pickColor"]) this.um[k] = gl.getUniformLocation(this.mesh, k);
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

  window.TifMrcViewPro.surfaces = { SurfaceScene, parse };
})();
