//! Label maps for the Pro segmentation tools: one 16-bit label per pixel of
//! a Z stack (0 = unlabelled), painted by hand or filled by the classic tools
//! (magic wand, threshold, watershed). Saved next to the image as a 16-bit
//! TIFF stack that Fiji, napari and Python read directly.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::image::{OpenImage, Px};
use crate::with_plane;

/// Previous contents of the slices one edit changed (None = empty slice).
type Edit = Vec<(usize, Option<Vec<u16>>)>;

/// Undo history is trimmed to roughly this many bytes.
const HISTORY_BYTES: usize = 1 << 30;

/// A 3D magic wand or connected-region pass stops past this many voxels.
const MAX_REGION: usize = 1 << 28;

/// Pixel values of one channel, slice by slice.
pub trait Values: Sync {
    fn slice(&self, z: usize) -> Result<Vec<f32>>;
}

/// One channel at one time point of an open image.
pub struct ChannelValues {
    pub img: Arc<OpenImage>,
    pub c: usize,
    pub t: usize,
}

impl Values for ChannelValues {
    fn slice(&self, z: usize) -> Result<Vec<f32>> {
        let info = self.img.info();
        let (w, h) = (info.width, info.height);
        let (page, sample) = info.locate(self.c, z, self.t);
        let planes = self.img.page(page)?;
        let plane = &planes[sample.min(planes.len() - 1)];
        let mut out = vec![0f32; w * h];
        with_plane!(&plane.data, v => {
            for y in 0..h.min(plane.h) {
                let n = w.min(plane.w);
                for (o, p) in out[y * w..y * w + n].iter_mut().zip(&v[y * plane.w..y * plane.w + n]) {
                    *o = p.to_f32();
                }
            }
        });
        Ok(out)
    }
}

/// Bit per pixel of one slice.
struct Bits(Vec<u64>);

impl Bits {
    fn new(n: usize) -> Bits {
        Bits(vec![0; n.div_ceil(64)])
    }
    #[inline]
    fn get(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
    #[inline]
    fn set(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
}

/// Whether a pixel holding `cur` may be changed to `label`. `guard` 0 lets
/// everything through; otherwise painting only fills empty pixels (or ones
/// already holding the label) and erasing only clears label `guard`.
/// Labels in `locked` are never changed by the tools.
#[inline]
fn allowed(cur: u16, label: u16, guard: u16, locked: &[u16]) -> bool {
    if cur != 0 && cur != label && locked.contains(&cur) {
        return false;
    }
    guard == 0 || if label != 0 { cur == 0 || cur == label } else { cur == guard }
}

/// Name, colour and flags of one label (like a material in Amira). Labels
/// without an entry use the default name and palette colour.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Material {
    pub label: u16,
    pub name: String,
    pub color: Option<[u8; 3]>,
    pub hidden: bool,
    pub locked: bool,
}

/// Euclidean distance from every pixel to the nearest `true` one
/// (Felzenszwalb & Huttenlocher); a large value when there is none.
pub(crate) fn edt(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e20;
    let mut g: Vec<f32> = mask.iter().map(|&b| if b { 0.0 } else { INF }).collect();
    let n = w.max(h);
    let (mut f, mut d, mut v, mut z) = (vec![0f32; n], vec![0f32; n], vec![0usize; n], vec![0f32; n + 1]);
    // Squared distances along one line of `len` samples.
    let mut pass = |f: &mut [f32], d: &mut [f32], len: usize| {
        let mut k = 0;
        v[0] = 0;
        z[0] = -INF;
        z[1] = INF;
        let cross = |f: &[f32], q: usize, p: usize| ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32));
        for q in 1..len {
            let mut s = cross(f, q, v[k]);
            // z[0] is -INF, so this stops at k = 0.
            while s <= z[k] {
                k -= 1;
                s = cross(f, q, v[k]);
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = INF;
        }
        k = 0;
        for (q, out) in d.iter_mut().enumerate().take(len) {
            while z[k + 1] < q as f32 {
                k += 1;
            }
            let p = v[k];
            *out = (q as f32 - p as f32).powi(2) + f[p];
        }
    };
    for x in 0..w {
        for y in 0..h {
            f[y] = g[y * w + x];
        }
        pass(&mut f, &mut d, h);
        for y in 0..h {
            g[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&g[y * w..(y + 1) * w]);
        pass(&mut f, &mut d, w);
        g[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
    g.iter().map(|&x| if x >= INF / 2.0 { (w + h) as f32 } else { x.sqrt() }).collect()
}

/// Key of the materials line in the label file's ImageJ description.
const MATERIALS_KEY: &str = "tmv_materials=";

/// TIFF text tags must be ASCII, so other characters become \u escapes.
fn ascii_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for u in c.encode_utf16(&mut [0; 2]) {
                out += &format!("\\u{u:04x}");
            }
        }
    }
    out
}

pub struct Labels {
    pub w: usize,
    pub h: usize,
    pub d: usize,
    slices: Vec<Option<Vec<u16>>>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    pub materials: Vec<Material>,
    /// Changed since the last save.
    pub dirty: bool,
}

impl Labels {
    pub fn new(w: usize, h: usize, d: usize) -> Labels {
        Labels { w, h, d, slices: vec![None; d], undo: vec![], redo: vec![], materials: vec![], dirty: false }
    }

    pub fn is_empty(&self) -> bool {
        self.slices.iter().all(Option::is_none)
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> u16 {
        self.slices[z].as_ref().map_or(0, |s| s[y * self.w + x])
    }

    /// Slice `z` as little-endian u16.
    pub fn slice_bytes(&self, z: usize) -> Vec<u8> {
        match &self.slices[z.min(self.d - 1)] {
            Some(s) => s.iter().flat_map(|v| v.to_le_bytes()).collect(),
            None => vec![0; self.w * self.h * 2],
        }
    }

    fn locked(&self) -> Vec<u16> {
        self.materials.iter().filter(|m| m.locked).map(|m| m.label).collect()
    }

    /// Replaces the material list (entries for label 0 are dropped).
    pub fn set_materials(&mut self, mut m: Vec<Material>) {
        m.retain(|m| m.label != 0);
        m.sort_by_key(|m| m.label);
        m.dedup_by_key(|m| m.label);
        if m != self.materials {
            self.materials = m;
            self.dirty = true;
        }
    }

    /// The label of the material called `name`, added (with `color`) as a
    /// new label when there is none.
    pub fn material_label(&mut self, name: &str, color: [u8; 3]) -> u16 {
        if let Some(m) = self.materials.iter().find(|m| m.name == name) {
            return m.label;
        }
        let label = self.max_label().max(self.materials.iter().map(|m| m.label).max().unwrap_or(0)) + 1;
        let mut m = self.materials.clone();
        m.push(Material { label, name: name.into(), color: Some(color), ..Default::default() });
        self.set_materials(m);
        label
    }

    pub fn max_label(&self) -> u16 {
        self.slices.iter().flatten().map(|s| s.iter().copied().max().unwrap_or(0)).max().unwrap_or(0)
    }

    /// Voxel count of every label, indexed by label (index 0 is unused).
    pub fn counts(&self) -> Vec<u64> {
        let mut n = vec![0u64; self.max_label() as usize + 1];
        for s in self.slices.iter().flatten() {
            for &v in s {
                n[v as usize] += 1;
            }
        }
        n[0] = 0;
        n
    }

    /// The most common label of each slice (0 for an empty slice).
    pub fn slice_labels(&self) -> Vec<u16> {
        let mut n = vec![0u32; self.max_label() as usize + 1];
        self.slices
            .iter()
            .map(|s| {
                let Some(s) = s else { return 0 };
                n.iter_mut().for_each(|c| *c = 0);
                for &v in s {
                    n[v as usize] += 1;
                }
                (1..n.len()).max_by_key(|&k| n[k]).filter(|&k| n[k] > 0).unwrap_or(0) as u16
            })
            .collect()
    }

    /// Mutable slice `z`, remembering its old contents in `edit` first.
    fn touch<'a>(slices: &'a mut [Option<Vec<u16>>], edit: &mut Edit, z: usize, n: usize) -> &'a mut Vec<u16> {
        if !edit.iter().any(|(k, _)| *k == z) {
            edit.push((z, slices[z].clone()));
        }
        slices[z].get_or_insert_with(|| vec![0; n])
    }

    /// Records `edit` for undo and returns the slices that really changed.
    fn commit(&mut self, mut edit: Edit) -> Vec<usize> {
        for (z, _) in &edit {
            if self.slices[*z].as_ref().is_some_and(|s| s.iter().all(|&v| v == 0)) {
                self.slices[*z] = None;
            }
        }
        edit.retain(|(z, old)| *old != self.slices[*z]);
        if edit.is_empty() {
            return vec![];
        }
        let mut zs: Vec<usize> = edit.iter().map(|e| e.0).collect();
        zs.sort_unstable();
        self.undo.push(edit);
        self.redo.clear();
        self.dirty = true;
        let size = |e: &Edit| e.iter().map(|(_, s)| s.as_ref().map_or(0, |s| s.len() * 2)).sum::<usize>();
        let mut total: usize = self.undo.iter().map(size).sum();
        while total > HISTORY_BYTES && self.undo.len() > 1 {
            total -= size(&self.undo.remove(0));
        }
        zs
    }

    fn swap(&mut self, edit: Edit) -> (Edit, Vec<usize>) {
        let mut back = Vec::with_capacity(edit.len());
        let mut zs = vec![];
        for (z, old) in edit {
            back.push((z, std::mem::replace(&mut self.slices[z], old)));
            zs.push(z);
        }
        self.dirty = true;
        (back, zs)
    }

    pub fn undo(&mut self) -> Vec<usize> {
        let Some(e) = self.undo.pop() else { return vec![] };
        let (back, zs) = self.swap(e);
        self.redo.push(back);
        zs
    }

    pub fn redo(&mut self) -> Vec<usize> {
        let Some(e) = self.redo.pop() else { return vec![] };
        let (back, zs) = self.swap(e);
        self.undo.push(back);
        zs
    }

    /// Paints a round brush of `radius` pixels along `pts` (image
    /// coordinates; pixel x covers x..x+1). Label 0 erases.
    pub fn stroke(&mut self, z: usize, pts: &[[f32; 2]], radius: f32, label: u16, guard: u16) -> Vec<usize> {
        if z >= self.d || pts.is_empty() {
            return vec![];
        }
        let (w, h) = (self.w, self.h);
        let locked = self.locked();
        let mut edit = vec![];
        let s = Self::touch(&mut self.slices, &mut edit, z, w * h);
        let r = radius.max(0.5);
        let mut stamp = |cx: f32, cy: f32| {
            let x0 = (cx - r).floor().max(0.0) as usize;
            let y0 = (cy - r).floor().max(0.0) as usize;
            let x1 = ((cx + r).ceil() as isize).clamp(0, w as isize) as usize;
            let y1 = ((cy + r).ceil() as isize).clamp(0, h as isize) as usize;
            for y in y0..y1 {
                let dy = y as f32 + 0.5 - cy;
                for x in x0..x1 {
                    let dx = x as f32 + 0.5 - cx;
                    let v = &mut s[y * w + x];
                    if dx * dx + dy * dy <= r * r && allowed(*v, label, guard, &locked) {
                        *v = label;
                    }
                }
            }
        };
        let step = (r / 2.0).max(0.5);
        stamp(pts[0][0], pts[0][1]);
        for p in pts.windows(2) {
            let (a, b) = (p[0], p[1]);
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let n = (len / step).ceil().max(1.0) as usize;
            for i in 1..=n {
                let k = i as f32 / n as f32;
                stamp(a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k);
            }
        }
        self.commit(edit)
    }

    /// Flood fill from (x, y, z) over connected pixels whose value is within
    /// `tol` of the clicked one; `three_d` also grows through Z.
    #[allow(clippy::too_many_arguments)]
    pub fn wand(&mut self, vol: &dyn Values, x: usize, y: usize, z: usize, tol: f32, label: u16, guard: u16, three_d: bool) -> Result<Vec<usize>> {
        let (w, h, d) = (self.w, self.h, self.d);
        if x >= w || y >= h || z >= d {
            return Ok(vec![]);
        }
        let n = w * h;
        let seed = vol.slice(z)?[y * w + x];
        let locked = self.locked();
        let mut seen: HashMap<usize, Bits> = HashMap::new();
        // Pixels waiting in each slice, worked through one slice at a time.
        let mut todo: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        todo.insert(z, vec![y * w + x]);
        let mut edit = vec![];
        let mut filled = 0usize;
        while let Some((&cz, _)) = todo.iter().next() {
            let mut stack = todo.remove(&cz).unwrap();
            let vals = vol.slice(cz)?;
            let bits = seen.entry(cz).or_insert_with(|| Bits::new(n));
            let s = Self::touch(&mut self.slices, &mut edit, cz, n);
            let ok = |i: usize, s: &[u16]| (vals[i] - seed).abs() <= tol && allowed(s[i], label, guard, &locked);
            while let Some(i) = stack.pop() {
                if bits.get(i) || !ok(i, s) {
                    continue;
                }
                bits.set(i);
                s[i] = label;
                filled += 1;
                if filled > MAX_REGION {
                    bail!("区域太大，请调小容差");
                }
                let (px, py) = (i % w, i / w);
                if px > 0 { stack.push(i - 1) }
                if px + 1 < w { stack.push(i + 1) }
                if py > 0 { stack.push(i - w) }
                if py + 1 < h { stack.push(i + w) }
                // Pixels of the next slices are checked when that slice is worked on.
                if three_d {
                    if cz > 0 {
                        todo.entry(cz - 1).or_default().push(i);
                    }
                    if cz + 1 < d {
                        todo.entry(cz + 1).or_default().push(i);
                    }
                }
            }
        }
        Ok(self.commit(edit))
    }

    /// Labels every pixel of slices `zs` whose value lies in lo..=hi. With
    /// `split`, each connected region (through Z too) of at least `min_size`
    /// pixels gets its own new label, counting up from the highest label in
    /// use; otherwise they all get `label`. Returns the changed slices.
    #[allow(clippy::too_many_arguments)]
    pub fn threshold(&mut self, vol: &dyn Values, zs: Range<usize>, lo: f32, hi: f32, label: u16, guard: u16, split: bool, min_size: usize) -> Result<Vec<usize>> {
        let zs = zs.start.min(self.d)..zs.end.min(self.d);
        let n = self.w * self.h;
        let locked = self.locked();
        let mut edit = vec![];
        if !split {
            for z in zs {
                let vals = vol.slice(z)?;
                let s = Self::touch(&mut self.slices, &mut edit, z, n);
                for (v, &p) in s.iter_mut().zip(&vals) {
                    if p >= lo && p <= hi && allowed(*v, label, guard, &locked) {
                        *v = label;
                    }
                }
            }
            return Ok(self.commit(edit));
        }
        // Mask of the pixels in range that may be labelled.
        let mut mask = Vec::with_capacity(zs.len());
        for z in zs.clone() {
            let vals = vol.slice(z)?;
            let mut b = Bits::new(n);
            for (i, &p) in vals.iter().enumerate() {
                let cur = self.get(i % self.w, i / self.w, z);
                if p >= lo && p <= hi && (guard == 0 || cur == 0) && !locked.contains(&cur) {
                    b.set(i);
                }
            }
            mask.push(b);
        }
        let mut next = self.max_label() as usize + 1;
        let (w, h, z0, nz) = (self.w, self.h, zs.start, zs.len());
        let mut seen: Vec<Bits> = (0..nz).map(|_| Bits::new(n)).collect();
        let mut region: Vec<(usize, usize)> = vec![];
        let mut stack: Vec<(usize, usize)> = vec![];
        for sz in 0..nz {
            for start in 0..n {
                if !mask[sz].get(start) || seen[sz].get(start) {
                    continue;
                }
                region.clear();
                stack.push((sz, start));
                seen[sz].set(start);
                while let Some((cz, i)) = stack.pop() {
                    region.push((cz, i));
                    let (px, py) = (i % w, i / w);
                    let mut visit = |z: usize, j: usize| {
                        if mask[z].get(j) && !seen[z].get(j) {
                            seen[z].set(j);
                            stack.push((z, j));
                        }
                    };
                    if px > 0 { visit(cz, i - 1) }
                    if px + 1 < w { visit(cz, i + 1) }
                    if py > 0 { visit(cz, i - w) }
                    if py + 1 < h { visit(cz, i + w) }
                    if cz > 0 { visit(cz - 1, i) }
                    if cz + 1 < nz { visit(cz + 1, i) }
                }
                if region.len() > MAX_REGION {
                    bail!("区域太大，请收窄阈值范围");
                }
                if region.len() < min_size.max(1) {
                    continue;
                }
                if next > u16::MAX as usize {
                    bail!("区域超过 65535 个，请加大最小体积或收窄阈值范围");
                }
                for &(cz, i) in &region {
                    Self::touch(&mut self.slices, &mut edit, z0 + cz, n)[i] = next as u16;
                }
                next += 1;
            }
        }
        Ok(self.commit(edit))
    }

    /// Marker watershed on slices `zs`: the labels already there are seeds,
    /// and every unlabelled pixel joins the seed that reaches it first when
    /// flooding the image gradient from low to high.
    pub fn watershed(&mut self, vol: &dyn Values, zs: Range<usize>) -> Result<Vec<usize>> {
        let zs = zs.start.min(self.d)..zs.end.min(self.d);
        let (w, h, z0, nz) = (self.w, self.h, zs.start, zs.len());
        let n = w * h;
        // Gradient magnitude of each slice (Sobel), and its largest value.
        let mut grad: Vec<Vec<f32>> = Vec::with_capacity(nz);
        let mut gmax = 0f32;
        for z in zs.clone() {
            let v = vol.slice(z)?;
            let mut g = vec![0f32; n];
            for y in 0..h {
                let (ym, yp) = (y.saturating_sub(1), (y + 1).min(h - 1));
                for x in 0..w {
                    let (xm, xp) = (x.saturating_sub(1), (x + 1).min(w - 1));
                    let p = |xx: usize, yy: usize| v[yy * w + xx];
                    let gx = p(xp, ym) + 2.0 * p(xp, y) + p(xp, yp) - p(xm, ym) - 2.0 * p(xm, y) - p(xm, yp);
                    let gy = p(xm, yp) + 2.0 * p(x, yp) + p(xp, yp) - p(xm, ym) - 2.0 * p(x, ym) - p(xp, ym);
                    let m = (gx * gx + gy * gy).sqrt();
                    if m.is_finite() {
                        g[y * w + x] = m;
                        gmax = gmax.max(m);
                    }
                }
            }
            grad.push(g);
        }
        const LEVELS: usize = 1024;
        let level = |g: f32| if gmax > 0.0 { ((g / gmax) * (LEVELS - 1) as f32) as usize } else { 0 };
        // Bucket queue of (slice in range, pixel, label).
        let mut queue: Vec<Vec<(u32, u32, u16)>> = vec![vec![]; LEVELS];
        let mut edit = vec![];
        let mut cur: Vec<Vec<u16>> = Vec::with_capacity(nz);
        for z in zs.clone() {
            cur.push(self.slices[z].clone().unwrap_or_else(|| vec![0; n]));
        }
        let mut seeds = 0usize;
        let neighbours = |cz: usize, i: usize, out: &mut Vec<(usize, usize)>| {
            out.clear();
            let (px, py) = (i % w, i / w);
            if px > 0 { out.push((cz, i - 1)) }
            if px + 1 < w { out.push((cz, i + 1)) }
            if py > 0 { out.push((cz, i - w)) }
            if py + 1 < h { out.push((cz, i + w)) }
            if cz > 0 { out.push((cz - 1, i)) }
            if cz + 1 < nz { out.push((cz + 1, i)) }
        };
        let mut nb = Vec::with_capacity(6);
        for cz in 0..nz {
            for i in 0..n {
                let l = cur[cz][i];
                if l == 0 {
                    continue;
                }
                seeds += 1;
                neighbours(cz, i, &mut nb);
                for &(qz, j) in &nb {
                    if cur[qz][j] == 0 {
                        queue[level(grad[qz][j])].push((qz as u32, j as u32, l));
                    }
                }
            }
        }
        if seeds == 0 {
            bail!("先用画笔在每个物体和背景里各画一笔不同的标签，作为分水岭的种子");
        }
        let mut lvl = 0;
        while lvl < LEVELS {
            let Some((qz, j, l)) = queue[lvl].pop() else {
                lvl += 1;
                continue;
            };
            let (qz, j) = (qz as usize, j as usize);
            if cur[qz][j] != 0 {
                continue;
            }
            cur[qz][j] = l;
            neighbours(qz, j, &mut nb);
            for &(rz, k) in &nb {
                if cur[rz][k] == 0 {
                    // Never flood back below the current level.
                    let lk = level(grad[rz][k]).max(lvl);
                    queue[lk].push((rz as u32, k as u32, l));
                }
            }
        }
        for (cz, s) in cur.into_iter().enumerate() {
            *Self::touch(&mut self.slices, &mut edit, z0 + cz, n) = s;
        }
        Ok(self.commit(edit))
    }

    /// Clears label `k` everywhere.
    /// Fills the closed polygon `pts` (image coordinates) on slice `z`; a
    /// pixel is inside when its centre is (even-odd rule). Label 0 erases.
    pub fn polygon(&mut self, z: usize, pts: &[[f32; 2]], label: u16, guard: u16) -> Vec<usize> {
        if z >= self.d || pts.len() < 3 {
            return vec![];
        }
        let (w, h) = (self.w, self.h);
        let locked = self.locked();
        let mut edit = vec![];
        let s = Self::touch(&mut self.slices, &mut edit, z, w * h);
        let mut xs: Vec<f32> = vec![];
        for y in 0..h {
            let yc = y as f32 + 0.5;
            xs.clear();
            for (i, a) in pts.iter().enumerate() {
                let b = pts[(i + 1) % pts.len()];
                if (a[1] <= yc) != (b[1] <= yc) {
                    xs.push(a[0] + (yc - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
                }
            }
            xs.sort_by(f32::total_cmp);
            for p in xs.chunks_exact(2) {
                // Pixels whose centre x + 0.5 lies in p[0]..p[1].
                let x0 = (p[0] - 0.5).ceil().max(0.0) as usize;
                let x1 = ((p[1] - 0.5).ceil().max(0.0) as usize).min(w);
                for v in &mut s[y * w + x0.min(x1)..y * w + x1] {
                    if allowed(*v, label, guard, &locked) {
                        *v = label;
                    }
                }
            }
        }
        self.commit(edit)
    }

    /// Fills the slices between drawn slices of `label` by blending the
    /// signed distance maps of the nearest drawn slice above and below
    /// (shape-based interpolation, like Amira's Interpolate).
    pub fn interpolate(&mut self, label: u16, guard: u16) -> Vec<usize> {
        use rayon::prelude::*;
        let (w, h) = (self.w, self.h);
        let keys: Vec<usize> = (0..self.d)
            .filter(|&z| self.slices[z].as_ref().is_some_and(|s| s.contains(&label)))
            .collect();
        let sdf = |z: usize| {
            let s = self.slices[z].as_ref().unwrap();
            let inside: Vec<bool> = s.iter().map(|&v| v == label).collect();
            let outside: Vec<bool> = inside.iter().map(|&b| !b).collect();
            let (a, b) = (edt(&inside, w, h), edt(&outside, w, h));
            a.iter().zip(&b).map(|(a, b)| a - b).collect::<Vec<f32>>()
        };
        let maps: Vec<Vec<f32>> = keys.par_iter().map(|&z| sdf(z)).collect();
        // New pixels of each in-between slice.
        let fills: Vec<(usize, Vec<usize>)> = keys
            .windows(2)
            .zip(maps.windows(2))
            .filter(|(k, _)| k[1] - k[0] > 1)
            .flat_map(|(k, m)| (k[0] + 1..k[1]).map(move |z| (z, k[0], k[1], &m[0], &m[1])))
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|(z, a, b, ma, mb)| {
                let t = (z - a) as f32 / (b - a) as f32;
                let px = ma.iter().zip(mb).enumerate().filter(|(_, (x, y))| (1.0 - t) * **x + t * **y < 0.0).map(|(i, _)| i).collect();
                (z, px)
            })
            .collect();
        let locked = self.locked();
        let mut edit = vec![];
        for (z, px) in fills {
            let s = Self::touch(&mut self.slices, &mut edit, z, w * h);
            for i in px {
                if allowed(s[i], label, guard, &locked) {
                    s[i] = label;
                }
            }
        }
        self.commit(edit)
    }

    /// Paints `label` (0 erases) wherever `mask` is set on slice `z`.
    /// With `trim`, pixels of `label` outside the mask on that slice are
    /// cleared too (a rough brush outline refined by SAM).
    pub fn apply_mask(&mut self, z: usize, mask: &[bool], label: u16, guard: u16, trim: bool) -> Vec<usize> {
        self.apply_masks(&[(z, mask)], label, guard, trim)
    }

    /// `apply_mask` on several slices as one undo step.
    pub fn apply_masks(&mut self, masks: &[(usize, &[bool])], label: u16, guard: u16, trim: bool) -> Vec<usize> {
        let locked = self.locked();
        let mut edit = vec![];
        for &(z, mask) in masks {
            if z >= self.d || mask.len() != self.w * self.h {
                continue;
            }
            let s = Self::touch(&mut self.slices, &mut edit, z, mask.len());
            for (v, &m) in s.iter_mut().zip(mask) {
                if m && allowed(*v, label, guard, &locked) {
                    *v = label;
                } else if !m && trim && label != 0 && *v == label {
                    *v = 0;
                }
            }
        }
        self.commit(edit)
    }

    /// Writes per-pixel labels on several slices as one undo step (a
    /// classifier result); 0 in `classes` leaves the pixel as it is.
    pub fn apply_classes(&mut self, slices: &[(usize, Vec<u16>)], guard: u16) -> Vec<usize> {
        let locked = self.locked();
        let n = self.w * self.h;
        let mut edit = vec![];
        for (z, classes) in slices {
            if *z >= self.d || classes.len() != n {
                continue;
            }
            let s = Self::touch(&mut self.slices, &mut edit, *z, n);
            for (v, &k) in s.iter_mut().zip(classes) {
                if k != 0 && allowed(*v, k, guard, &locked) {
                    *v = k;
                }
            }
        }
        self.commit(edit)
    }

    /// Slice `z`, or None when it is empty.
    pub fn slice(&self, z: usize) -> Option<&[u16]> {
        self.slices.get(z).and_then(|s| s.as_deref())
    }

    /// Slices that hold any label.
    pub fn used_slices(&self) -> Vec<usize> {
        (0..self.d).filter(|&z| self.slices[z].is_some()).collect()
    }

    /// The pixels of the label at (x, y, z) connected to it, by slice:
    /// 4-connected within a slice and, with `three_d`, through Z as well.
    fn region(&self, x: usize, y: usize, z: usize, three_d: bool) -> (u16, BTreeMap<usize, Vec<usize>>) {
        let (w, h, d) = (self.w, self.h, self.d);
        let mut out: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        if x >= w || y >= h || z >= d {
            return (0, out);
        }
        let k = self.get(x, y, z);
        if k == 0 {
            return (0, out);
        }
        let n = w * h;
        let mut seen: HashMap<usize, Bits> = HashMap::new();
        let mut stack = vec![(z, y * w + x)];
        while let Some((cz, i)) = stack.pop() {
            let Some(s) = self.slices[cz].as_ref() else { continue };
            let bits = seen.entry(cz).or_insert_with(|| Bits::new(n));
            if bits.get(i) || s[i] != k {
                continue;
            }
            bits.set(i);
            out.entry(cz).or_default().push(i);
            let (px, py) = (i % w, i / w);
            if px > 0 { stack.push((cz, i - 1)) }
            if px + 1 < w { stack.push((cz, i + 1)) }
            if py > 0 { stack.push((cz, i - w)) }
            if py + 1 < h { stack.push((cz, i + w)) }
            if three_d {
                if cz > 0 { stack.push((cz - 1, i)) }
                if cz + 1 < d { stack.push((cz + 1, i)) }
            }
        }
        (k, out)
    }

    /// Clears the region at (x, y, z) (see `region`), unless its label is locked.
    pub fn delete_region(&mut self, x: usize, y: usize, z: usize, three_d: bool) -> Vec<usize> {
        let (k, region) = self.region(x, y, z, three_d);
        if k == 0 || self.locked().contains(&k) {
            return vec![];
        }
        let n = self.w * self.h;
        let mut edit = vec![];
        for (cz, pix) in region {
            let s = Self::touch(&mut self.slices, &mut edit, cz, n);
            for i in pix {
                s[i] = 0;
            }
        }
        self.commit(edit)
    }

    /// Moves the region at (x, y, z) by (dx, dy) pixels within its slices.
    /// Pixels moved off the image are lost; it covers other labels except
    /// locked ones.
    pub fn move_region(&mut self, x: usize, y: usize, z: usize, dx: i64, dy: i64, three_d: bool) -> Vec<usize> {
        let (k, region) = self.region(x, y, z, three_d);
        let locked = self.locked();
        if k == 0 || locked.contains(&k) || (dx == 0 && dy == 0) {
            return vec![];
        }
        let (w, h) = (self.w as i64, self.h as i64);
        let mut edit = vec![];
        for (cz, pix) in region {
            let s = Self::touch(&mut self.slices, &mut edit, cz, (w * h) as usize);
            for &i in &pix {
                s[i] = 0;
            }
            for &i in &pix {
                let (nx, ny) = ((i as i64 % w) + dx, (i as i64 / w) + dy);
                if nx < 0 || ny < 0 || nx >= w || ny >= h {
                    continue;
                }
                let j = (ny * w + nx) as usize;
                if allowed(s[j], k, 0, &locked) {
                    s[j] = k;
                }
            }
        }
        self.commit(edit)
    }

    pub fn delete_label(&mut self, k: u16) -> Vec<usize> {
        let n = self.w * self.h;
        let mut edit = vec![];
        for z in 0..self.d {
            if !self.slices[z].as_ref().is_some_and(|s| s.contains(&k)) {
                continue;
            }
            for v in Self::touch(&mut self.slices, &mut edit, z, n).iter_mut() {
                if *v == k {
                    *v = 0;
                }
            }
        }
        self.commit(edit)
    }

    /// The labels box-reduced by `f` in XY and `fz` in Z for the 3D view, one
    /// byte per voxel (labels above 255 wrap around, skipping 0). A block
    /// takes a label when any pixel in it has one, so thin structures stay.
    pub fn reduced(&self, f: usize, fz: usize) -> (usize, usize, usize, Vec<u8>) {
        let (ow, oh, od) = (self.w.div_ceil(f), self.h.div_ceil(f), self.d.div_ceil(fz));
        let mut out = vec![0u8; ow * oh * od];
        for z in 0..self.d {
            let Some(s) = &self.slices[z] else { continue };
            let base = (z / fz) * ow * oh;
            for y in 0..self.h {
                for x in 0..self.w {
                    let v = s[y * self.w + x];
                    if v != 0 {
                        out[base + (y / f) * ow + x / f] = ((v - 1) % 255 + 1) as u8;
                    }
                }
            }
        }
        (ow, oh, od, out)
    }

    /// Writes all slices as a 16-bit TIFF stack (deflate), or removes the
    /// file when there are no labels. Returns whether a file was written.
    pub fn save(&mut self, path: &Path) -> Result<bool> {
        if self.is_empty() {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
            self.dirty = false;
            return Ok(false);
        }
        use tiff::encoder::{colortype::Gray16, compression::DeflateLevel, Compression, TiffEncoder};
        use tiff::tags::Tag;
        let tmp = path.with_extension("tif.tmp");
        {
            let file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            let mut enc = TiffEncoder::new(file)?.with_compression(Compression::Deflate(DeflateLevel::Fast));
            let zero = vec![0u16; self.w * self.h];
            // ImageJ reads the pages as a Z stack.
            // Names and colours ride along on a line of their own, which ImageJ ignores.
            let mut desc = format!("ImageJ=1.11a\nimages={0}\nslices={0}\nhyperstack=false\n", self.d);
            if !self.materials.is_empty() {
                desc += &format!("{MATERIALS_KEY}{}\n", ascii_json(&serde_json::to_string(&self.materials)?));
            }
            for z in 0..self.d {
                let mut img = enc.new_image::<Gray16>(self.w as u32, self.h as u32)?;
                if z == 0 {
                    img.encoder().write_tag(Tag::ImageDescription, desc.as_str())?;
                    img.encoder().write_tag(Tag::Software, "TifMrcView Pro labels")?;
                }
                img.write_data(self.slices[z].as_deref().unwrap_or(&zero))?;
            }
        }
        std::fs::rename(&tmp, path)?;
        self.dirty = false;
        Ok(true)
    }

    /// Reads a label stack written by `save` (8- or 16-bit, same size).
    pub fn load(path: &Path, w: usize, h: usize, d: usize) -> Result<Labels> {
        use tiff::decoder::{Decoder, DecodingResult};
        let file = std::io::BufReader::new(std::fs::File::open(path)?);
        let mut dec = Decoder::new(file).context("标签文件不是 TIFF")?;
        let mut out = Labels::new(w, h, d);
        if let Ok(desc) = dec.get_tag_ascii_string(tiff::tags::Tag::ImageDescription) {
            if let Some(json) = desc.lines().find_map(|l| l.strip_prefix(MATERIALS_KEY)) {
                out.materials = serde_json::from_str(json).unwrap_or_default();
            }
        }
        for z in 0..d {
            let (pw, ph) = dec.dimensions()?;
            if (pw as usize, ph as usize) != (w, h) {
                bail!("标签文件大小 {pw}×{ph} 和图像 {w}×{h} 不一致");
            }
            let s: Vec<u16> = match dec.read_image()? {
                DecodingResult::U16(v) => v,
                DecodingResult::U8(v) => v.into_iter().map(u16::from).collect(),
                _ => bail!("标签文件必须是 8 位或 16 位整数"),
            };
            if s.len() != w * h {
                bail!("标签文件必须是单通道");
            }
            if s.iter().any(|&v| v != 0) {
                out.slices[z] = Some(s);
            }
            if z + 1 < d {
                if !dec.more_images() {
                    bail!("标签文件只有 {} 层，图像有 {d} 层", z + 1);
                }
                dec.next_image()?;
            }
        }
        Ok(out)
    }
}

/// `cell.mrc` → `cell.mrc.labels.tif`; other images of a multi-image file
/// get their own (`project.lif.series3.labels.tif`), like the annotations.
pub fn labels_path(image: &str, series: usize) -> String {
    if series > 0 {
        format!("{image}.series{}.labels.tif", series + 1)
    } else {
        format!("{image}.labels.tif")
    }
}

/// Whether `path` is a label file of some image (kept out of folder lists).
pub fn is_labels_file(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().ends_with(".labels.tif"))
}

/// Label maps of the open images, by image token.
pub struct Entry {
    pub labels: Labels,
    pub path: String,
    /// Pixel classifier training strokes (same size as `labels`, not saved).
    pub scribbles: Option<Labels>,
    #[cfg(feature = "pro")]
    pub classifier: Option<Arc<crate::classify::Classifier>>,
    #[cfg(feature = "pro")]
    pub unet: Option<Arc<crate::unet::Trained>>,
}

impl Entry {
    pub fn new(labels: Labels, path: String) -> Entry {
        Entry {
            labels,
            path,
            scribbles: None,
            #[cfg(feature = "pro")]
            classifier: None,
            #[cfg(feature = "pro")]
            unet: None,
        }
    }

    /// The scribble layer, made on first use.
    pub fn scribbles(&mut self) -> &mut Labels {
        let (w, h, d) = (self.labels.w, self.labels.h, self.labels.d);
        self.scribbles.get_or_insert_with(|| Labels::new(w, h, d))
    }
}

pub type Store = Mutex<HashMap<u64, Arc<Mutex<Entry>>>>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A stack whose values come straight from a closure.
    struct Fake<F: Fn(usize, usize, usize) -> f32 + Sync>(usize, usize, F);
    impl<F: Fn(usize, usize, usize) -> f32 + Sync> Values for Fake<F> {
        fn slice(&self, z: usize) -> Result<Vec<f32>> {
            Ok((0..self.0 * self.1).map(|i| (self.2)(i % self.0, i / self.0, z)).collect())
        }
    }

    #[test]
    fn classes_respect_locks_and_undo() {
        let mut l = Labels::new(4, 2, 2);
        l.stroke(0, &[[0.5, 0.5]], 0.5, 5, 0);
        l.set_materials(vec![Material { label: 5, locked: true, ..Default::default() }]);
        let classes = vec![1, 2, 2, 0, 1, 1, 2, 2];
        assert_eq!(l.apply_classes(&[(0, classes.clone())], 0), vec![0]);
        // Locked label 5 stays, 0 leaves the pixel alone.
        assert_eq!((0..4).map(|x| l.get(x, 0, 0)).collect::<Vec<_>>(), vec![5, 2, 2, 0]);
        assert_eq!(l.get(3, 1, 0), 2);
        assert_eq!(l.undo(), vec![0]);
        assert_eq!(l.get(1, 0, 0), 0);
    }

    #[test]
    fn brush_paints_and_undoes() {
        let mut l = Labels::new(20, 20, 3);
        let zs = l.stroke(1, &[[5.0, 5.0], [15.0, 5.0]], 2.0, 3, 0);
        assert_eq!(zs, vec![1]);
        assert_eq!(l.get(10, 5, 1), 3);
        assert_eq!(l.get(10, 12, 1), 0);
        assert_eq!(l.get(10, 5, 0), 0);
        assert_eq!(l.max_label(), 3);
        l.undo();
        assert!(l.is_empty());
        l.redo();
        assert_eq!(l.get(10, 5, 1), 3);
    }

    #[test]
    fn guard_keeps_other_labels() {
        let mut l = Labels::new(10, 10, 1);
        l.stroke(0, &[[2.5, 2.5]], 1.0, 1, 0);
        l.stroke(0, &[[2.5, 2.5]], 1.0, 2, 2);
        assert_eq!(l.get(2, 2, 0), 1);
        // Erasing with a guard only clears that label.
        l.stroke(0, &[[2.5, 2.5]], 1.0, 0, 2);
        assert_eq!(l.get(2, 2, 0), 1);
        l.stroke(0, &[[2.5, 2.5]], 1.0, 0, 1);
        assert_eq!(l.get(2, 2, 0), 0);
    }

    #[test]
    fn regions_move_and_go() {
        let mut l = Labels::new(10, 10, 3);
        for z in 0..2 {
            l.stroke(z, &[[2.5, 2.5]], 1.0, 1, 0);
        }
        l.stroke(0, &[[7.5, 7.5]], 1.0, 1, 0);
        // Only the touched blob moves; the other one of the same label stays.
        assert_eq!(l.move_region(2, 2, 0, 3, 0, false), vec![0]);
        assert_eq!((l.get(2, 2, 0), l.get(5, 2, 0), l.get(7, 7, 0), l.get(2, 2, 1)), (0, 1, 1, 1));
        // Through Z: the blob on slice 1 is joined to nothing at (5, 2) any more.
        assert_eq!(l.delete_region(5, 2, 0, true), vec![0]);
        assert_eq!((l.get(5, 2, 0), l.get(2, 2, 1)), (0, 1));
        l.stroke(0, &[[2.5, 2.5]], 1.0, 1, 0);
        assert_eq!(l.delete_region(2, 2, 1, true), vec![0, 1]);
        assert_eq!(l.get(7, 7, 0), 1);
        l.undo();
        assert_eq!((l.get(2, 2, 0), l.get(2, 2, 1)), (1, 1));
    }

    #[test]
    fn trimmed_mask_clears_the_rest_of_the_label() {
        let mut l = Labels::new(4, 1, 1);
        l.stroke(0, &[[0.5, 0.5], [3.5, 0.5]], 0.6, 2, 0);
        l.apply_mask(0, &[false, true, true, false], 2, 0, true);
        assert_eq!((0..4).map(|x| l.get(x, 0, 0)).collect::<Vec<_>>(), vec![0, 2, 2, 0]);
    }

    #[test]
    fn locked_labels_stay() {
        let mut l = Labels::new(10, 10, 1);
        l.stroke(0, &[[2.5, 2.5]], 1.0, 1, 0);
        l.set_materials(vec![Material { label: 1, locked: true, ..Default::default() }]);
        l.stroke(0, &[[2.5, 2.5]], 3.0, 2, 0);
        assert_eq!(l.get(2, 2, 0), 1);
        assert_eq!(l.get(4, 2, 0), 2);
        l.stroke(0, &[[2.5, 2.5]], 3.0, 0, 0);
        assert_eq!(l.get(2, 2, 0), 1);
        assert_eq!(l.get(4, 2, 0), 0);
    }

    #[test]
    fn distance_transform() {
        let mut m = vec![false; 7 * 5];
        m[2 * 7 + 3] = true;
        let d = edt(&m, 7, 5);
        assert_eq!(d[2 * 7 + 3], 0.0);
        assert_eq!(d[2 * 7 + 6], 3.0);
        assert!((d[0] - 13f32.sqrt()).abs() < 1e-5);
        assert_eq!(edt(&[false; 4], 2, 2), vec![4.0; 4]);
    }

    #[test]
    fn polygon_fills_inside() {
        let mut l = Labels::new(10, 10, 1);
        l.polygon(0, &[[2.0, 2.0], [8.0, 2.0], [8.0, 6.0], [2.0, 6.0]], 3, 0);
        assert_eq!(l.counts()[3], 24);
        assert_eq!(l.get(2, 2, 0), 3);
        assert_eq!(l.get(7, 5, 0), 3);
        assert_eq!(l.get(8, 2, 0), 0);
        assert_eq!(l.get(2, 6, 0), 0);
        l.polygon(0, &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], 0, 0);
        assert!(l.is_empty());
    }

    #[test]
    fn interpolation_morphs_between_slices() {
        let mut l = Labels::new(30, 30, 5);
        // Small square on slice 0, big square on slice 4.
        l.polygon(0, &[[10.0, 10.0], [14.0, 10.0], [14.0, 14.0], [10.0, 14.0]], 1, 0);
        l.polygon(4, &[[6.0, 6.0], [22.0, 6.0], [22.0, 22.0], [6.0, 22.0]], 1, 0);
        l.stroke(2, &[[28.5, 28.5]], 0.5, 2, 0);
        let zs = l.interpolate(1, 0);
        assert_eq!(zs, vec![1, 2, 3]);
        let n = |z: usize| (0..900).filter(|&i| l.get(i % 30, i / 30, z) == 1).count();
        assert!(n(0) < n(1) && n(1) < n(2) && n(2) < n(3) && n(3) < n(4), "{:?}", (0..5).map(n).collect::<Vec<_>>());
        assert_eq!(l.get(28, 28, 2), 2, "other labels stay");
        assert!(l.interpolate(1, 0).is_empty(), "nothing left to fill");
    }

    #[test]
    fn wand_stays_inside_region() {
        // A bright 4x4 square in each slice, dark elsewhere.
        let vol = Fake(10, 10, |x, y, _| if (3..7).contains(&x) && (3..7).contains(&y) { 100.0 } else { 0.0 });
        let mut l = Labels::new(10, 10, 4);
        l.wand(&vol, 4, 4, 1, 10.0, 5, 0, false).unwrap();
        assert_eq!(l.counts()[5], 16);
        l.undo();
        let zs = l.wand(&vol, 4, 4, 1, 10.0, 5, 0, true).unwrap();
        assert_eq!(zs, vec![0, 1, 2, 3]);
        assert_eq!(l.counts()[5], 64);
    }

    #[test]
    fn threshold_splits_regions() {
        // Two separate bright columns through all slices plus one lone pixel.
        let vol = Fake(12, 6, |x, y, z| {
            if x < 3 || x > 8 || (x == 6 && y == 3 && z == 0) { 50.0 } else { 0.0 }
        });
        let mut l = Labels::new(12, 6, 3);
        l.threshold(&vol, 0..3, 10.0, 100.0, 1, 0, true, 5).unwrap();
        let c = l.counts();
        assert_eq!(c.len(), 3, "two regions, the lone pixel is too small");
        assert_eq!((c[1], c[2]), (54, 54));
        assert_eq!(l.get(6, 3, 0), 0);
        l.undo();
        l.threshold(&vol, 0..1, 10.0, 100.0, 4, 0, false, 0).unwrap();
        assert_eq!(l.counts()[4], 37);
    }

    #[test]
    fn watershed_splits_at_the_edge() {
        // Left half dark, right half bright: one seed on each side.
        let vol = Fake(16, 8, |x, _, _| if x < 8 { 0.0 } else { 100.0 });
        let mut l = Labels::new(16, 8, 2);
        l.stroke(0, &[[2.5, 4.5]], 0.5, 1, 0);
        l.stroke(0, &[[13.5, 4.5]], 0.5, 2, 0);
        l.watershed(&vol, 0..2).unwrap();
        for z in 0..2 {
            for y in 0..8 {
                assert_eq!(l.get(3, y, z), 1);
                assert_eq!(l.get(12, y, z), 2);
            }
        }
        assert_eq!(l.counts()[1] + l.counts()[2], 16 * 8 * 2);
        let mut none = Labels::new(4, 4, 1);
        assert!(none.watershed(&vol, 0..1).is_err());
    }

    #[test]
    fn delete_and_reduce() {
        let mut l = Labels::new(8, 8, 4);
        l.stroke(2, &[[6.5, 6.5]], 0.5, 300, 0);
        l.stroke(0, &[[0.5, 0.5]], 0.5, 1, 0);
        let (w, h, d, v) = l.reduced(2, 2);
        assert_eq!((w, h, d), (4, 4, 2));
        assert_eq!(v[0], 1);
        assert_eq!(v[16 + 3 * 4 + 3], 45, "300 wraps to 45");
        assert_eq!(l.slice_labels(), vec![1, 0, 300, 0]);
        l.delete_label(300);
        assert_eq!(l.counts().len(), 2);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.mrc.labels.tif");
        let mut l = Labels::new(9, 7, 5);
        l.stroke(3, &[[4.0, 3.0]], 2.0, 1234, 0);
        l.set_materials(vec![Material { label: 1234, name: "膜 \"a\"\nb 🦠".into(), color: Some([1, 2, 3]), hidden: true, locked: false }]);
        assert!(l.save(&path).unwrap());
        assert!(!l.dirty);
        let back = Labels::load(&path, 9, 7, 5).unwrap();
        assert_eq!(back.counts(), l.counts());
        assert_eq!(back.materials, l.materials);
        assert!(back.slices[0].is_none());
        assert!(Labels::load(&path, 9, 7, 6).is_err());
        assert!(Labels::load(&path, 8, 7, 5).is_err());
        l.delete_label(1234);
        assert!(!l.save(&path).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn label_file_names() {
        assert_eq!(labels_path("/d/a.mrc", 0), "/d/a.mrc.labels.tif");
        assert_eq!(labels_path("/d/a.lif", 2), "/d/a.lif.series3.labels.tif");
        assert!(is_labels_file(Path::new("/d/a.mrc.labels.tif")));
        assert!(!is_labels_file(Path::new("/d/labels.tif")));
    }
}
