//! Label maps for the Pro segmentation tools: one 16-bit label per pixel of
//! a Z stack (0 = unlabelled), painted by hand or filled by the classic tools
//! (magic wand, threshold, watershed). Saved next to the image as a 16-bit
//! TIFF stack that Fiji, napari and Python read directly.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};

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
#[inline]
fn allowed(cur: u16, label: u16, guard: u16) -> bool {
    guard == 0 || if label != 0 { cur == 0 || cur == label } else { cur == guard }
}

pub struct Labels {
    pub w: usize,
    pub h: usize,
    pub d: usize,
    slices: Vec<Option<Vec<u16>>>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// Changed since the last save.
    pub dirty: bool,
}

impl Labels {
    pub fn new(w: usize, h: usize, d: usize) -> Labels {
        Labels { w, h, d, slices: vec![None; d], undo: vec![], redo: vec![], dirty: false }
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
                    if dx * dx + dy * dy <= r * r && allowed(*v, label, guard) {
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
            let ok = |i: usize, s: &[u16]| (vals[i] - seed).abs() <= tol && allowed(s[i], label, guard);
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
        let mut edit = vec![];
        if !split {
            for z in zs {
                let vals = vol.slice(z)?;
                let s = Self::touch(&mut self.slices, &mut edit, z, n);
                for (v, &p) in s.iter_mut().zip(&vals) {
                    if p >= lo && p <= hi && allowed(*v, label, guard) {
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
                if p >= lo && p <= hi && (guard == 0 || self.get(i % self.w, i / self.w, z) == 0) {
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
            let desc = format!("ImageJ=1.11a\nimages={0}\nslices={0}\nhyperstack=false\n", self.d);
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
        l.delete_label(300);
        assert_eq!(l.counts().len(), 2);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.mrc.labels.tif");
        let mut l = Labels::new(9, 7, 5);
        l.stroke(3, &[[4.0, 3.0]], 2.0, 1234, 0);
        assert!(l.save(&path).unwrap());
        assert!(!l.dirty);
        let back = Labels::load(&path, 9, 7, 5).unwrap();
        assert_eq!(back.counts(), l.counts());
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
