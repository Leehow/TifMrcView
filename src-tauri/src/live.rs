//! What a long Pro job has found so far, for the viewer to draw while the
//! job runs (membranes appearing window by window, say). One job at a time
//! publishes a class map on its own coarse grid; the UI asks for the slice
//! under the current Z (`slice`) whenever the job reports progress.
//!
//! Jobs that make a new volume rather than labels (the cryo-ET pipeline)
//! show one picture instead: the middle section as it is built
//! (`Picture`, read with `picture`).

use std::sync::Mutex;

/// A class map over the open image (class 0 = nothing to draw).
pub struct Map {
    /// Image it belongs to.
    pub token: u64,
    /// The image's size (x, y, z).
    pub full: [usize; 3],
    /// The map's own grid (x, y, z), X fastest.
    pub grid: [usize; 3],
    pub cells: Vec<u8>,
    /// Colour (r, g, b, a) of each class.
    pub palette: Vec<[u8; 4]>,
    /// Region being worked on now, on the grid: x0, y0, z0, x1, y1, z1
    /// (exclusive ends).
    pub busy: Option<[usize; 6]>,
}

pub static MAP: Mutex<Option<Map>> = Mutex::new(None);

/// Held by tests that use `MAP` (one map for the whole process).
#[cfg(test)]
pub static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Starts a map for `token` (replacing any other).
pub fn start(token: u64, full: [usize; 3], grid: [usize; 3], palette: Vec<[u8; 4]>) {
    let len = grid[0] * grid[1] * grid[2];
    *MAP.lock().unwrap() = Some(Map { token, full, grid, cells: vec![0; len], palette, busy: None });
}

/// Drops the map (the job ended).
pub fn stop() {
    *MAP.lock().unwrap() = None;
}

/// Changes the map, when there is one.
pub fn with(f: impl FnOnce(&mut Map)) {
    if let Some(m) = MAP.lock().unwrap().as_mut() {
        f(m);
    }
}

impl Map {
    /// Grid cell holding voxel `i` of an axis of `n` voxels on a grid of `m`
    /// (nearest, as `membrane::segment` maps them).
    fn near(i: usize, n: usize, m: usize) -> usize {
        (((i as f32 + 0.5) * m as f32 / n as f32) as usize).min(m - 1)
    }

    /// Marks `class` wherever `mask` (on the image's own grid) is set.
    pub fn paint_full(&mut self, mask: &[bool], class: u8) {
        let [n0, n1, n2] = self.full;
        if mask.len() != n0 * n1 * n2 {
            return;
        }
        let [m0, m1, _] = self.grid;
        let near: [Vec<usize>; 3] = std::array::from_fn(|a| (0..self.full[a]).map(|i| Self::near(i, self.full[a], self.grid[a])).collect());
        for z in 0..n2 {
            for y in 0..n1 {
                let row = &mask[(z * n1 + y) * n0..][..n0];
                let base = (near[2][z] * m1 + near[1][y]) * m0;
                for (x, &b) in row.iter().enumerate() {
                    if b {
                        self.cells[base + near[0][x]] = class;
                    }
                }
            }
        }
    }
}

/// The map's slice under Z `z` of the image `token`, laid out as
/// u32 gw, u32 gh, u32 classes, u32 busy (1 when the busy region crosses
/// this slice), u32 × 4 busy rectangle (x0, y0, x1, y1 on the image's grid),
/// then the palette (classes × RGBA) and gw × gh class bytes.
pub fn slice(token: u64, z: usize) -> Option<Vec<u8>> {
    let guard = MAP.lock().unwrap();
    let m = guard.as_ref().filter(|m| m.token == token)?;
    let [g0, g1, g2] = m.grid;
    let gz = Map::near(z.min(m.full[2].saturating_sub(1)), m.full[2], g2);
    let mut out = Vec::with_capacity(32 + m.palette.len() * 4 + g0 * g1);
    let mut rect = [0u32; 4];
    let mut busy = 0u32;
    if let Some([x0, y0, z0, x1, y1, z1]) = m.busy {
        if (z0..z1).contains(&gz) {
            busy = 1;
            // Grid edges to image pixels.
            let px = |i: usize, a: usize| ((i as f64 * m.full[a] as f64 / m.grid[a] as f64).round() as usize).min(m.full[a]) as u32;
            rect = [px(x0, 0), px(y0, 1), px(x1, 0), px(y1, 1)];
        }
    }
    for v in [g0 as u32, g1 as u32, m.palette.len() as u32, busy].into_iter().chain(rect) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for c in &m.palette {
        out.extend_from_slice(c);
    }
    out.extend_from_slice(&m.cells[gz * g0 * g1..][..g0 * g1]);
    Some(out)
}

/// A grey picture (the middle section of a volume being made), rows filled
/// in as they come (NaN = not yet), with an optional overlay mask.
pub struct Picture {
    pub title: String,
    pub w: usize,
    pub h: usize,
    pub values: Vec<f32>,
    /// Drawn in `MASK_COLOR` where set (empty = none).
    pub mask: Vec<bool>,
}

pub static PICTURE: Mutex<Option<Picture>> = Mutex::new(None);

/// Colour of the picture's mask (the membranes' colour).
pub const MASK_COLOR: [u8; 3] = [255, 196, 0];

/// Starts an empty picture.
pub fn begin(title: &str, w: usize, h: usize) {
    *PICTURE.lock().unwrap() = Some(Picture { title: title.into(), w, h, values: vec![f32::NAN; w * h], mask: vec![] });
}

/// Fills rows from `y0` of the picture (`rows` is whole rows).
pub fn rows(y0: usize, rows: &[f32]) {
    if let Some(p) = PICTURE.lock().unwrap().as_mut() {
        let start = (y0 * p.w).min(p.values.len());
        let n = rows.len().min(p.values.len() - start);
        p.values[start..start + n].copy_from_slice(&rows[..n]);
    }
}

/// Shows a whole picture, with a mask when `mask` is not empty.
pub fn show(title: &str, w: usize, h: usize, values: &[f32], mask: &[bool]) {
    if values.len() != w * h || (!mask.is_empty() && mask.len() != w * h) {
        return;
    }
    *PICTURE.lock().unwrap() = Some(Picture { title: title.into(), w, h, values: values.to_vec(), mask: mask.to_vec() });
}

/// Shows section `z` of a volume (dims `n`, X fastest), with the same
/// section of `mask` when not empty.
pub fn show_section(title: &str, v: &[f32], n: [usize; 3], z: usize, mask: &[bool]) {
    let plane = n[0] * n[1];
    if z >= n[2] || v.len() < (z + 1) * plane {
        return;
    }
    let m = if mask.len() >= (z + 1) * plane { &mask[z * plane..(z + 1) * plane] } else { &[][..] };
    show(title, n[0], n[1], &v[z * plane..(z + 1) * plane], m);
}

/// The picture at most `MAX` pixels a side, as u32 w, u32 h, u32 title
/// length, the UTF-8 title padded to 4 bytes, then w × h RGBA (alpha 0
/// where not made yet). Grey levels span the 1st to 99th percentile.
pub fn picture() -> Option<Vec<u8>> {
    const MAX: usize = 512;
    let guard = PICTURE.lock().unwrap();
    let p = guard.as_ref()?;
    let k = p.w.max(p.h).div_ceil(MAX).max(1);
    let (w, h) = (p.w.div_ceil(k), p.h.div_ceil(k));
    // Box means of k × k blocks (finite values only).
    let mut grey = vec![f32::NAN; w * h];
    let mut mask = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut sum, mut cnt, mut hit) = (0f64, 0usize, 0usize);
            for yy in y * k..((y + 1) * k).min(p.h) {
                for xx in x * k..((x + 1) * k).min(p.w) {
                    let i = yy * p.w + xx;
                    let v = p.values[i];
                    if v.is_finite() {
                        sum += v as f64;
                        cnt += 1;
                    }
                    if p.mask.get(i).copied().unwrap_or(false) {
                        hit += 1;
                    }
                }
            }
            if cnt > 0 {
                grey[y * w + x] = (sum / cnt as f64) as f32;
            }
            mask[y * w + x] = hit * 2 >= k * k && hit > 0;
        }
    }
    let mut sorted: Vec<f32> = grey.iter().copied().filter(|v| v.is_finite()).collect();
    sorted.sort_by(f32::total_cmp);
    let (lo, hi) = match sorted.len() {
        0 => (0.0, 1.0),
        n => (sorted[n / 100], sorted[(n * 99 / 100).min(n - 1)]),
    };
    let span = if hi > lo { hi - lo } else { 1.0 };
    let title = p.title.as_bytes();
    let mut out = Vec::with_capacity(12 + title.len() + 3 + w * h * 4);
    for v in [w as u32, h as u32, title.len() as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(title);
    out.resize(out.len().div_ceil(4) * 4, 0);
    for (g, &m) in grey.iter().zip(&mask) {
        if !g.is_finite() {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let v = (((g - lo) / span).clamp(0.0, 1.0) * 255.0).round() as u8;
        if m {
            let mix = |c: u8| ((v as u16 + c as u16) / 2) as u8;
            out.extend_from_slice(&[mix(MASK_COLOR[0]), mix(MASK_COLOR[1]), mix(MASK_COLOR[2]), 255]);
        } else {
            out.extend_from_slice(&[v, v, v, 255]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_follow_the_grid() {
        let _one = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        start(7, [8, 6, 4], [4, 3, 2], vec![[0; 4], [255, 0, 0, 255]]);
        with(|m| {
            let mut mask = vec![false; 8 * 6 * 4];
            mask[(3 * 6 + 5) * 8 + 7] = true; // x 7, y 5, z 3 → cell (3, 2, 1)
            m.paint_full(&mask, 1);
            m.busy = Some([0, 0, 1, 2, 3, 2]);
        });
        assert!(slice(8, 0).is_none());
        let s = slice(7, 3).unwrap();
        let u = |i: usize| u32::from_le_bytes(s[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!((u(0), u(1), u(2), u(3)), (4, 3, 2, 1));
        assert_eq!((u(4), u(5), u(6), u(7)), (0, 0, 4, 6));
        let cells = &s[32 + 8..];
        assert_eq!(cells.len(), 12);
        assert_eq!(cells.iter().position(|&c| c == 1), Some(2 * 4 + 3));
        let s0 = slice(7, 0).unwrap();
        assert_eq!(u32::from_le_bytes(s0[12..16].try_into().unwrap()), 0);
        assert!(s0[40..].iter().all(|&c| c == 0));
        stop();
        assert!(slice(7, 3).is_none());
    }

    #[test]
    fn pictures_fill_in_and_shrink() {
        begin("重构", 1024, 4);
        let head = picture().unwrap();
        let u = |b: &[u8], i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!((u(&head, 0), u(&head, 1)), (512, 2));
        let title = "重构".len();
        let px = 12 + title.div_ceil(4) * 4;
        assert!(head[px..].chunks(4).all(|c| c[3] == 0));
        rows(0, &(0..2048).map(|i| (i % 1024) as f32).collect::<Vec<_>>());
        let b = picture().unwrap();
        // First output row made, second not yet.
        assert!(b[px..px + 512 * 4].chunks(4).all(|c| c[3] == 255));
        assert!(b[px + 512 * 4..].chunks(4).all(|c| c[3] == 0));
        assert!(b[px] < b[px + 511 * 4]);
        let v = vec![1.0; 16];
        let mut m = vec![false; 16];
        m[0] = true;
        show("膜", 4, 4, &v, &m);
        let b = picture().unwrap();
        let px = 12 + "膜".len().div_ceil(4) * 4;
        assert_ne!(b[px..px + 4], b[px + 4..px + 8]);
        *PICTURE.lock().unwrap() = None;
    }
}
