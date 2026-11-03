//! Smooth surfaces of a label map for the Pro 3D view, the way Amira shows a
//! segmentation: every connected object of a label becomes its own closed
//! triangle mesh, so objects can be picked, hidden or given another label
//! one by one, and the meshes can be exported.
//!
//! The labels are reduced to a voxel budget (a cell takes the label most of
//! its pixels carry and counts as inside when any pixel is labelled, so thin
//! membranes stay), split into 6-connected objects, and each object is meshed
//! with surface nets: one vertex in every cell of the dual grid that the
//! object's boundary passes through, at the mean of the crossed edges'
//! midpoints, and one quad for every crossed grid edge. Taubin smoothing then
//! takes the voxel steps out without shrinking the objects.

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use rayon::prelude::*;
use serde::Deserialize;

use crate::labels::Labels;

/// Voxel budget of the reduced label grid.
const MAX_VOXELS: usize = 48 << 20;
/// More triangles than this and the grid is reduced further.
const MAX_TRIS: usize = 6 << 20;
/// Taubin smoothing: rounds of one shrinking and one inflating step.
const SMOOTH_ROUNDS: usize = 6;
const LAMBDA: f32 = 0.5;
const MU: f32 = -0.53;

pub struct Object {
    pub label: u16,
    /// Cells of the reduced grid.
    pub cells: u64,
    /// A pixel of the object in the label map.
    pub seed: [u32; 3],
    pub first_vert: u32,
    pub verts: u32,
    pub first_tri: u32,
    pub tris: u32,
}

pub struct Surfaces {
    /// Size of the label map.
    pub dims: [usize; 3],
    /// Reduction in XY and in Z.
    pub f: usize,
    pub fz: usize,
    /// Size of the reduced grid and the object of each cell (id + 1, 0 = none).
    grid: [usize; 3],
    obj: Vec<u32>,
    pub objects: Vec<Object>,
    /// Vertices in pixel units of the label map (pixel centres at +0.5).
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[i8; 4]>,
    pub tri: Vec<[u32; 3]>,
}

/// The label of every cell of the label map reduced by `f` in XY and `fz` in
/// Z: the label most of its pixels carry (Boyer–Moore vote), 0 only when it
/// has none.
fn reduce(l: &Labels, f: usize, fz: usize) -> (usize, usize, usize, Vec<u16>) {
    let (gw, gh, gd) = (l.w.div_ceil(f), l.h.div_ceil(f), l.d.div_ceil(fz));
    let mut out = vec![0u16; gw * gh * gd];
    out.par_chunks_mut(gw * gh).enumerate().for_each(|(gz, layer)| {
        let mut count = vec![0u32; gw * gh];
        for z in gz * fz..((gz + 1) * fz).min(l.d) {
            let Some(s) = l.slice(z) else { continue };
            for y in 0..l.h {
                let row = (y / f) * gw;
                for x in 0..l.w {
                    let v = s[y * l.w + x];
                    if v == 0 {
                        continue;
                    }
                    let i = row + x / f;
                    if layer[i] == v {
                        count[i] += 1;
                    } else if count[i] == 0 {
                        layer[i] = v;
                        count[i] = 1;
                    } else {
                        count[i] -= 1;
                    }
                }
            }
        }
    });
    (gw, gh, gd, out)
}

/// One object found by `components`: label, cell count, first cell, box.
struct Found {
    label: u16,
    cells: u64,
    first: usize,
    lo: [usize; 3],
    hi: [usize; 3],
}

/// 6-connected objects of equal label; returns the object of every cell
/// (id + 1, 0 = none).
fn components(dims: [usize; 3], label: &[u16]) -> (Vec<u32>, Vec<Found>) {
    let [w, h, d] = dims;
    let mut obj = vec![0u32; label.len()];
    let mut found = vec![];
    let mut stack = vec![];
    for start in 0..label.len() {
        let k = label[start];
        if k == 0 || obj[start] != 0 {
            continue;
        }
        let id = found.len() as u32 + 1;
        let mut f = Found { label: k, cells: 0, first: start, lo: [usize::MAX; 3], hi: [0; 3] };
        obj[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let p = [i % w, (i / w) % h, i / (w * h)];
            f.cells += 1;
            for a in 0..3 {
                f.lo[a] = f.lo[a].min(p[a]);
                f.hi[a] = f.hi[a].max(p[a]);
            }
            let step = [1, w, w * h];
            for a in 0..3 {
                let size = [w, h, d][a];
                for n in [p[a].checked_sub(1), (p[a] + 1 < size).then_some(p[a] + 1)].into_iter().flatten() {
                    let j = i + n * step[a] - p[a] * step[a];
                    if obj[j] == 0 && label[j] == k {
                        obj[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
        found.push(f);
    }
    (obj, found)
}

/// Surface nets of one object: vertices in cell coordinates of the reduced
/// grid (cell centres at integers) and outward-facing triangles.
fn mesh_object(dims: [usize; 3], obj: &[u32], id: u32, lo: [usize; 3], hi: [usize; 3]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let [w, h, _] = dims;
    // Samples: the box with one cell of outside around it.
    let s = [hi[0] - lo[0] + 3, hi[1] - lo[1] + 3, hi[2] - lo[2] + 3];
    let inside = |p: [usize; 3]| -> bool {
        if (0..3).any(|a| p[a] == 0 || p[a] == s[a] - 1) {
            return false;
        }
        let (x, y, z) = (lo[0] + p[0] - 1, lo[1] + p[1] - 1, lo[2] + p[2] - 1);
        obj[(z * h + y) * w + x] == id
    };
    // Dual cells between samples i..i+1 along each axis.
    let c = [s[0] - 1, s[1] - 1, s[2] - 1];
    let mut vert = vec![u32::MAX; c[0] * c[1] * c[2]];
    let mut pos = vec![];
    for k in 0..c[2] {
        for j in 0..c[1] {
            for i in 0..c[0] {
                let mut mask = 0u8;
                for b in 0..8 {
                    if inside([i + (b & 1), j + ((b >> 1) & 1), k + ((b >> 2) & 1)]) {
                        mask |= 1 << b;
                    }
                }
                if mask == 0 || mask == 255 {
                    continue;
                }
                let (mut sum, mut n) = ([0f32; 3], 0f32);
                for a in 0..8usize {
                    for bit in [1usize, 2, 4] {
                        if a & bit != 0 || ((mask >> a) & 1) == ((mask >> (a | bit)) & 1) {
                            continue;
                        }
                        let corner = [(a & 1) as f32, ((a >> 1) & 1) as f32, ((a >> 2) & 1) as f32];
                        let axis = bit.trailing_zeros() as usize;
                        for (t, v) in sum.iter_mut().enumerate() {
                            *v += corner[t] + if t == axis { 0.5 } else { 0.0 };
                        }
                        n += 1.0;
                    }
                }
                vert[(k * c[1] + j) * c[0] + i] = pos.len() as u32;
                // Sample p sits at cell lo + p - 1.
                pos.push([
                    lo[0] as f32 - 1.0 + i as f32 + sum[0] / n,
                    lo[1] as f32 - 1.0 + j as f32 + sum[1] / n,
                    lo[2] as f32 - 1.0 + k as f32 + sum[2] / n,
                ]);
            }
        }
    }
    let mut tri = vec![];
    // For an edge along `a`, the plane axes (u, v) with u × v = a.
    let planes = [(1usize, 2usize), (2, 0), (0, 1)];
    for k in 0..s[2] {
        for j in 0..s[1] {
            for i in 0..s[0] {
                let p = [i, j, k];
                let here = inside(p);
                for (a, &(u, v)) in planes.iter().enumerate() {
                    let mut q = p;
                    q[a] += 1;
                    if q[a] == s[a] || here == inside(q) {
                        continue;
                    }
                    // The four dual cells around the edge, counter-clockwise seen from +a.
                    // (Border samples are outside, so a crossed edge is never on the border.)
                    let at = |du: usize, dv: usize| {
                        let mut d = p;
                        d[u] -= du;
                        d[v] -= dv;
                        vert[(d[2] * c[1] + d[1]) * c[0] + d[0]]
                    };
                    let mut quad = [at(1, 1), at(0, 1), at(0, 0), at(1, 0)];
                    if !here {
                        quad.reverse();
                    }
                    tri.push([quad[0], quad[1], quad[2]]);
                    tri.push([quad[0], quad[2], quad[3]]);
                }
            }
        }
    }
    taubin(&mut pos, &tri);
    (pos, tri)
}

/// Taubin λ|μ smoothing over the mesh's edges.
fn taubin(pos: &mut [[f32; 3]], tri: &[[u32; 3]]) {
    let n = pos.len();
    let mut edges: Vec<(u32, u32)> = Vec::with_capacity(tri.len() * 6);
    for t in tri {
        for e in 0..3 {
            let (a, b) = (t[e], t[(e + 1) % 3]);
            edges.push((a, b));
            edges.push((b, a));
        }
    }
    edges.sort_unstable();
    edges.dedup();
    let mut start = vec![0usize; n + 1];
    for &(a, _) in &edges {
        start[a as usize + 1] += 1;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut next = pos.to_vec();
    for round in 0..SMOOTH_ROUNDS * 2 {
        let k = if round % 2 == 0 { LAMBDA } else { MU };
        for i in 0..n {
            let nb = &edges[start[i]..start[i + 1]];
            if nb.is_empty() {
                continue;
            }
            let mut m = [0f32; 3];
            for &(_, b) in nb {
                for t in 0..3 {
                    m[t] += pos[b as usize][t];
                }
            }
            for t in 0..3 {
                next[i][t] = pos[i][t] + k * (m[t] / nb.len() as f32 - pos[i][t]);
            }
        }
        pos.copy_from_slice(&next);
    }
}

/// Area-weighted vertex normals as signed bytes.
fn normals(pos: &[[f32; 3]], tri: &[[u32; 3]]) -> Vec<[i8; 4]> {
    let mut acc = vec![[0f32; 3]; pos.len()];
    for t in tri {
        let [a, b, c] = t.map(|i| pos[i as usize]);
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        for &i in t {
            for k in 0..3 {
                acc[i as usize][k] += n[k];
            }
        }
    }
    acc.iter()
        .map(|n| {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
            [(n[0] / l * 127.0) as i8, (n[1] / l * 127.0) as i8, (n[2] / l * 127.0) as i8, 0]
        })
        .collect()
}

impl Surfaces {
    /// Meshes every object of the label map.
    pub fn build(l: &Labels) -> Surfaces {
        let mut budget = MAX_VOXELS;
        loop {
            let s = Self::build_at(l, budget);
            if s.tri.len() <= MAX_TRIS || s.f >= l.w.max(l.h) {
                return s;
            }
            budget /= 4;
        }
    }

    fn build_at(l: &Labels, budget: usize) -> Surfaces {
        let (f, fz) = crate::volume::factors(l.w, l.h, l.d, budget);
        let (gw, gh, gd, label) = reduce(l, f, fz);
        let grid = [gw, gh, gd];
        let (obj, found) = components(grid, &label);
        // Largest first, so the parallel meshing ends together.
        let mut order: Vec<usize> = (0..found.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(found[i].cells));
        let mut meshes: Vec<(usize, Vec<[f32; 3]>, Vec<[u32; 3]>)> = order
            .par_iter()
            .map(|&i| {
                let (p, t) = mesh_object(grid, &obj, i as u32 + 1, found[i].lo, found[i].hi);
                (i, p, t)
            })
            .collect();
        meshes.sort_by_key(|m| m.0);
        let (mut pos, mut tri, mut objects) = (vec![], vec![], vec![]);
        let scale = [f as f32, f as f32, fz as f32];
        for (i, p, t) in meshes {
            let fo = &found[i];
            let base = pos.len() as u32;
            objects.push(Object {
                label: fo.label,
                cells: fo.cells,
                seed: seed(l, fo, grid, f, fz),
                first_vert: base,
                verts: p.len() as u32,
                first_tri: tri.len() as u32,
                tris: t.len() as u32,
            });
            pos.extend(p.iter().map(|v| [(v[0] + 0.5) * scale[0], (v[1] + 0.5) * scale[1], (v[2] + 0.5) * scale[2]]));
            tri.extend(t.iter().map(|t| t.map(|i| i + base)));
        }
        let nrm = normals(&pos, &tri);
        Surfaces { dims: [l.w, l.h, l.d], f, fz, grid, obj, objects, pos, nrm, tri }
    }

    /// Reply for the 3D view: u32 width, height, depth of the label map,
    /// objects, vertices, triangles; per object u32 label, first triangle,
    /// triangles, seed x, y, z, pixels (approximate), 0; then f32 xyz per
    /// vertex, i8 xyz0 normal per vertex and u32 triples per triangle.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(28 + self.objects.len() * 32 + self.pos.len() * 16 + self.tri.len() * 12);
        let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
        for v in [self.dims[0], self.dims[1], self.dims[2], self.objects.len(), self.pos.len(), self.tri.len()] {
            put(v as u32);
        }
        let px = (self.f * self.f * self.fz) as u64;
        for o in &self.objects {
            for v in [o.label as u32, o.first_tri, o.tris, o.seed[0], o.seed[1], o.seed[2], (o.cells * px).min(u32::MAX as u64) as u32, 0] {
                put(v);
            }
        }
        for p in &self.pos {
            for v in p {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        for n in &self.nrm {
            out.extend(n.iter().map(|&b| b as u8));
        }
        for t in &self.tri {
            for v in t {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    /// Whether pixel (x, y, z) of the label map lies in a cell of object `id`.
    pub fn in_object(&self, id: usize, x: usize, y: usize, z: usize) -> bool {
        let [gw, gh, _] = self.grid;
        self.obj[((z / self.fz) * gh + y / self.f) * gw + x / self.f] == id as u32 + 1
    }

    /// Z range of the label map that object `id` covers.
    pub fn z_range(&self, id: usize) -> std::ops::Range<usize> {
        let o = &self.objects[id];
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for p in &self.pos[o.first_vert as usize..(o.first_vert + o.verts) as usize] {
            lo = lo.min(p[2]);
            hi = hi.max(p[2]);
        }
        let z0 = ((lo / self.fz as f32).floor().max(0.0) as usize).saturating_sub(1) * self.fz;
        let z1 = (((hi / self.fz as f32).ceil() as usize + 1) * self.fz).min(self.dims[2]);
        z0..z1.max(z0)
    }

    /// Writes the shown objects as a Wavefront OBJ file with a material file
    /// next to it (same name, .mtl). `scale` turns pixels into `unit`.
    pub fn write_obj(&self, path: &Path, mats: &[ExportMaterial], skip: &[u32], scale: [f32; 3], unit: &str) -> Result<()> {
        let mtl = path.with_extension("mtl");
        let mut m = std::io::BufWriter::new(std::fs::File::create(&mtl)?);
        for e in mats {
            let [r, g, b] = e.color.map(|c| c as f32 / 255.0);
            writeln!(m, "newmtl label{}\nKd {r:.4} {g:.4} {b:.4}\nKa 0 0 0\nKs 0.3 0.3 0.3\nNs 40\n", e.label)?;
        }
        m.flush()?;
        let mut w = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(w, "# TifMrcView segmentation surfaces, units: {unit}")?;
        writeln!(w, "mtllib {}", mtl.file_name().unwrap_or_default().to_string_lossy())?;
        let name = |label: u16| {
            let n = mats.iter().find(|e| e.label == label).map(|e| e.name.as_str()).unwrap_or("");
            let n: String = n.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect();
            if n.is_empty() { format!("label{label}") } else { n }
        };
        let mut written = 0u32;
        for (id, o) in self.objects.iter().enumerate() {
            if skip.contains(&(id as u32)) || !mats.iter().any(|e| e.label == o.label) {
                continue;
            }
            writeln!(w, "o {}_{}\nusemtl label{}", name(o.label), id + 1, o.label)?;
            let vs = o.first_vert as usize..(o.first_vert + o.verts) as usize;
            for p in &self.pos[vs.clone()] {
                writeln!(w, "v {:.3} {:.3} {:.3}", p[0] * scale[0], p[1] * scale[1], p[2] * scale[2])?;
            }
            for n in &self.nrm[vs] {
                // Normals follow the scaling of the positions.
                let v = [n[0] as f32 / scale[0], n[1] as f32 / scale[1], n[2] as f32 / scale[2]];
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
                writeln!(w, "vn {:.4} {:.4} {:.4}", v[0] / l, v[1] / l, v[2] / l)?;
            }
            let shift = written as i64 - o.first_vert as i64 + 1;
            for t in &self.tri[o.first_tri as usize..(o.first_tri + o.tris) as usize] {
                let [a, b, c] = t.map(|i| i as i64 + shift);
                writeln!(w, "f {a}//{a} {b}//{b} {c}//{c}")?;
            }
            written += o.verts;
        }
        w.flush()?;
        Ok(())
    }
}

/// A material to export: label, name and colour.
#[derive(Deserialize)]
pub struct ExportMaterial {
    pub label: u16,
    pub name: String,
    pub color: [u8; 3],
}

/// A pixel of the object's first cell that carries its label.
fn seed(l: &Labels, fo: &Found, grid: [usize; 3], f: usize, fz: usize) -> [u32; 3] {
    let [gw, gh, _] = grid;
    let (cx, cy, cz) = (fo.first % gw, (fo.first / gw) % gh, fo.first / (gw * gh));
    for z in cz * fz..((cz + 1) * fz).min(l.d) {
        let Some(s) = l.slice(z) else { continue };
        for y in cy * f..((cy + 1) * f).min(l.h) {
            for x in cx * f..((cx + 1) * f).min(l.w) {
                if s[y * l.w + x] == fo.label {
                    return [x as u32, y as u32, z as u32];
                }
            }
        }
    }
    [(cx * f) as u32, (cy * f) as u32, (cz * fz) as u32]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A label map with a ball of label 1 and a slab of label 2.
    fn scene(n: usize, r: f32) -> Labels {
        let mut l = Labels::new(n, n, n);
        let c = n as f32 / 2.0;
        let mut masks = vec![];
        for z in 0..n {
            let m: Vec<bool> = (0..n * n)
                .map(|i| {
                    let (x, y) = ((i % n) as f32 + 0.5, (i / n) as f32 + 0.5);
                    ((x - c).powi(2) + (y - c).powi(2) + (z as f32 + 0.5 - c).powi(2)).sqrt() <= r
                })
                .collect();
            masks.push((z, m));
        }
        let refs: Vec<(usize, &[bool])> = masks.iter().map(|(z, m)| (*z, m.as_slice())).collect();
        l.apply_masks(&refs, 1, 0, false);
        let slab: Vec<bool> = (0..n * n).map(|i| i % n < 4).collect();
        l.apply_masks(&[(2, &slab), (3, &slab)], 2, 0, false);
        l
    }

    #[test]
    fn ball_is_closed_round_and_faces_out() {
        let l = scene(48, 14.0);
        let s = Surfaces::build(&l);
        assert_eq!((s.f, s.fz), (1, 1));
        assert_eq!(s.objects.len(), 2);
        let ball = &s.objects[s.objects.iter().position(|o| o.label == 1).unwrap()];
        // Every edge is shared by exactly two triangles.
        let tris = &s.tri[ball.first_tri as usize..(ball.first_tri + ball.tris) as usize];
        let mut edges = std::collections::HashMap::new();
        for t in tris {
            for e in 0..3 {
                let (a, b) = (t[e].min(t[(e + 1) % 3]), t[e].max(t[(e + 1) % 3]));
                *edges.entry((a, b)).or_insert(0) += 1;
            }
        }
        assert!(edges.values().all(|&n| n == 2));
        // Enclosed volume (divergence theorem) close to the ball's, and positive: faces point out.
        let c = 24.0f64;
        let vol: f64 = tris
            .iter()
            .map(|t| {
                let [a, b, d] = t.map(|i| s.pos[i as usize].map(|v| v as f64 - c));
                (a[0] * (b[1] * d[2] - b[2] * d[1]) - a[1] * (b[0] * d[2] - b[2] * d[0]) + a[2] * (b[0] * d[1] - b[1] * d[0])) / 6.0
            })
            .sum();
        let want = 4.0 / 3.0 * std::f64::consts::PI * 14f64.powi(3);
        assert!((vol / want - 1.0).abs() < 0.06, "volume {vol} vs {want}");
        // Smoothed vertices lie close to the sphere.
        for p in &s.pos[ball.first_vert as usize..(ball.first_vert + ball.verts) as usize] {
            let r = p.iter().map(|&v| (v as f64 - c).powi(2)).sum::<f64>().sqrt();
            assert!((r - 14.0).abs() < 1.2, "radius {r}");
        }
        assert!(s.in_object(s.objects.iter().position(|o| o.label == 1).unwrap(), 24, 24, 24));
    }

    /// Timing on a tomogram-sized map: 400 vesicle shells and three wavy
    /// membranes in 960 x 960 x 300 (cargo test --release -- --ignored).
    #[test]
    #[ignore]
    fn tomogram_sized_timing() {
        let (w, h, d) = (960usize, 960usize, 300usize);
        let mut l = Labels::new(w, h, d);
        let mut rng = 12345u64;
        let mut next = || {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (rng >> 33) as f32 / (1u64 << 31) as f32
        };
        let ves: Vec<[f32; 3]> = (0..400).map(|_| [next() * w as f32, next() * h as f32, 30.0 + next() * 240.0]).collect();
        let mut masks1 = vec![];
        let mut masks2 = vec![];
        for z in 0..d {
            let mut m1 = vec![false; w * h];
            let mut m2 = vec![false; w * h];
            for v in &ves {
                let dz = z as f32 - v[2];
                if dz.abs() > 26.0 {
                    continue;
                }
                for y in (v[1] - 26.0).max(0.0) as usize..((v[1] + 26.0) as usize).min(h) {
                    for x in (v[0] - 26.0).max(0.0) as usize..((v[0] + 26.0) as usize).min(w) {
                        let r = ((x as f32 - v[0]).powi(2) + (y as f32 - v[1]).powi(2) + dz * dz).sqrt();
                        if (21.0..25.0).contains(&r) {
                            m1[y * w + x] = true;
                        }
                    }
                }
            }
            for y in 0..h {
                for x in 0..w {
                    for k in 0..3 {
                        let c = 200.0 + k as f32 * 280.0 + 30.0 * (x as f32 / 90.0 + z as f32 / 70.0).sin();
                        if (y as f32 - c).abs() < 2.5 {
                            m2[y * w + x] = true;
                        }
                    }
                }
            }
            masks1.push(m1);
            masks2.push(m2);
        }
        let r1: Vec<(usize, &[bool])> = masks1.iter().enumerate().map(|(z, m)| (z, m.as_slice())).collect();
        let r2: Vec<(usize, &[bool])> = masks2.iter().enumerate().map(|(z, m)| (z, m.as_slice())).collect();
        l.apply_masks(&r1, 1, 0, false);
        l.apply_masks(&r2, 2, 0, false);
        let t = std::time::Instant::now();
        let s = Surfaces::build(&l);
        if let Ok(out) = std::env::var("SURF_OUT") {
            std::fs::write(out, s.encode()).unwrap();
        }
        eprintln!(
            "f {} fz {}: {} objects, {} vertices, {} triangles, {:.2} s, {} MB reply",
            s.f, s.fz, s.objects.len(), s.pos.len(), s.tri.len(), t.elapsed().as_secs_f32(), s.encode().len() >> 20
        );
    }

    #[test]
    fn picked_object_takes_a_new_label() {
        let mut l = scene(32, 8.0);
        let s = Surfaces::build(&l);
        let ball = s.objects.iter().position(|o| o.label == 1).unwrap();
        let before = l.counts();
        let zs = l.relabel_where(s.z_range(ball), 1, 3, |x, y, z| s.in_object(ball, x, y, z));
        assert!(!zs.is_empty());
        let after = l.counts();
        assert_eq!(after[1], 0);
        assert_eq!(after[3], before[1]);
        assert_eq!(after[2], before[2]);
        l.undo();
        assert_eq!(l.counts()[1], before[1]);
    }

    #[test]
    fn encode_layout() {
        let s = Surfaces::build(&scene(32, 8.0));
        let b = s.encode();
        assert_eq!(b.len(), 24 + s.objects.len() * 32 + s.pos.len() * 16 + s.tri.len() * 12);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize, s.objects.len());
    }
}
