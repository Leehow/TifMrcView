//! The smart selection tool: a click, a loop drawn around an object or a
//! line drawn along it selects that object, and "find similar" proposes
//! the others like the ones already selected.
//!
//! Two kinds of proposal answer every request and the better one wins:
//! the blob under the pointer from segpick.rs (instant, right for spots
//! and particles that SAM blurs together), and SAM's masks. SAM sees the
//! part of the image that is on screen, not the whole slice shrunk to its
//! input size, so a small object is as large for the network as it is for
//! the user; the encoded parts are kept, and moving the pointer only runs
//! the mask decoder.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{bail, Result};

use super::{Denoise, LastFound, LastMask, NativePredictor, PickReq, Picked, Sam, SimilarReq};
use crate::segpick::{self, Found};

/// (token, z, c, t, lo, hi, slab, sigma)
pub type SliceKey = (u64, usize, usize, usize, u32, u32, usize, u32);

/// One slice as the tool sees it.
pub struct SliceData {
    pub key: SliceKey,
    /// 8-bit over the display range, what SAM encodes.
    gray: Vec<u8>,
    /// Raw values, lightly smoothed, for the blob proposals.
    smooth: Vec<f32>,
    noise: f32,
    med: f32,
    sd: f32,
}

/// Encoded parts kept per slice (about 20 MB of GPU memory each).
const MAX_TILES: usize = 12;

/// The part of a `w`×`h` image SAM encodes for an object around (cx, cy)
/// seen at `side` pixels: a square snapped in size and place, so nearby
/// requests share one encoding; the whole image when that is not much more.
fn tile_rect(cx: f32, cy: f32, side: f32, w: usize, h: usize) -> [usize; 4] {
    let mut s = 64.0f32;
    while s < side {
        s *= std::f32::consts::SQRT_2;
    }
    let s = s.round();
    if s >= 0.8 * w.max(h) as f32 {
        return [0, 0, w, h];
    }
    let (sw, sh) = ((s as usize).min(w), (s as usize).min(h));
    let step = (s / 8.0).max(1.0);
    let snap = |c: f32, len: usize, full: usize| (((c - len as f32 / 2.0) / step).round() * step).clamp(0.0, (full - len) as f32) as usize;
    let (x0, y0) = (snap(cx, sw, w), snap(cy, sh, h));
    [x0, y0, x0 + sw, y0 + sh]
}

/// Pixels of the image where low-resolution logits (a square grid over
/// `rect`) are positive, sampled bilinearly; with `seed` only the
/// 4-connected part that holds it, with `clip` only inside that box.
/// The flag says the mask runs into a side of `rect` that is not a side of
/// the image: the object goes on outside what was encoded.
fn low_to_pixels(low: &[f32], rect: [usize; 4], seed: Option<(usize, usize)>, clip: Option<[usize; 4]>, w: usize, h: usize) -> (Vec<usize>, bool) {
    let n = (low.len() as f64).sqrt().round() as usize;
    let (tw, th) = (rect[2] - rect[0], rect[3] - rect[1]);
    if n < 2 || tw == 0 || th == 0 {
        return (vec![], false);
    }
    let (mut u0, mut v0, mut u1, mut v1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, _) in low.iter().enumerate().filter(|(_, l)| **l > 0.0) {
        (u0, v0, u1, v1) = (u0.min(i % n), v0.min(i / n), u1.max(i % n), v1.max(i / n));
    }
    if u0 == usize::MAX {
        return (vec![], false);
    }
    // The box of positive cells, one cell wider, in image pixels.
    let mut b = [
        rect[0] + u0.saturating_sub(1) * tw / n,
        rect[1] + v0.saturating_sub(1) * th / n,
        (rect[0] + ((u1 + 2) * tw).div_ceil(n)).min(rect[2]),
        (rect[1] + ((v1 + 2) * th).div_ceil(n)).min(rect[3]),
    ];
    if let Some(c) = clip {
        b = [b[0].max(c[0]), b[1].max(c[1]), b[2].min(c[2]), b[3].min(c[3])];
    }
    if b[0] >= b[2] || b[1] >= b[3] {
        return (vec![], false);
    }
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    let logit = |x: usize, y: usize| -> f32 {
        let u = ((x - rect[0]) as f32 + 0.5) * n as f32 / tw as f32 - 0.5;
        let v = ((y - rect[1]) as f32 + 0.5) * n as f32 / th as f32 - 0.5;
        let (u, v) = (u.clamp(0.0, (n - 1) as f32), v.clamp(0.0, (n - 1) as f32));
        let (a, c) = (u.floor() as usize, v.floor() as usize);
        let (a1, c1) = ((a + 1).min(n - 1), (c + 1).min(n - 1));
        let (fu, fv) = (u - a as f32, v - c as f32);
        (low[c * n + a] * (1.0 - fu) + low[c * n + a1] * fu) * (1.0 - fv) + (low[c1 * n + a] * (1.0 - fu) + low[c1 * n + a1] * fu) * fv
    };
    let mut mask = vec![false; bw * bh];
    for y in 0..bh {
        for x in 0..bw {
            mask[y * bw + x] = logit(b[0] + x, b[1] + y) > 0.0;
        }
    }
    let local: Vec<usize> = match seed {
        None => (0..bw * bh).filter(|&i| mask[i]).collect(),
        Some((sx, sy)) => {
            if sx < b[0] || sy < b[1] || sx >= b[2] || sy >= b[3] || !mask[(sy - b[1]) * bw + sx - b[0]] {
                return (vec![], false);
            }
            let start = (sy - b[1]) * bw + sx - b[0];
            let mut out = vec![];
            let mut stack = vec![start];
            mask[start] = false;
            while let Some(i) = stack.pop() {
                out.push(i);
                let (x, y) = (i % bw, i / bw);
                for j in [(x > 0).then(|| i - 1), (x + 1 < bw).then(|| i + 1), (y > 0).then(|| i - bw), (y + 1 < bh).then(|| i + bw)].into_iter().flatten() {
                    if mask[j] {
                        mask[j] = false;
                        stack.push(j);
                    }
                }
            }
            out
        }
    };
    let mut cut = false;
    let pix = local
        .into_iter()
        .map(|i| {
            let (x, y) = (b[0] + i % bw, b[1] + i / bw);
            cut |= (x == rect[0] && x > 0) || (y == rect[1] && y > 0) || (x + 1 == rect[2] && x + 1 < w) || (y + 1 == rect[3] && y + 1 < h);
            y * w + x
        })
        .collect();
    (pix, cut)
}

/// A window of `v` around (cx, cy), `side` image pixels wide, reduced by
/// whole factors to at most `most` pixels a side: (values, width, height,
/// factor, x0, y0).
fn window(v: &[f32], w: usize, h: usize, cx: f32, cy: f32, side: f32, most: usize) -> (Vec<f32>, usize, usize, usize, usize, usize) {
    let half = (side / 2.0).max(8.0);
    let (x0, y0) = ((cx - half).floor().max(0.0) as usize, (cy - half).floor().max(0.0) as usize);
    let (x1, y1) = (((cx + half).ceil() as usize).min(w), ((cy + half).ceil() as usize).min(h));
    let f = ((x1 - x0).max(y1 - y0)).div_ceil(most).max(1);
    let (ww, wh) = ((x1 - x0) / f, (y1 - y0) / f);
    let mut out = vec![0f32; ww * wh];
    for y in 0..wh {
        for x in 0..ww {
            let mut sum = 0.0;
            for yy in 0..f {
                let row = (y0 + y * f + yy) * w + x0 + x * f;
                sum += v[row..row + f].iter().sum::<f32>();
            }
            out[y * ww + x] = sum / (f * f) as f32;
        }
    }
    (out, ww, wh, f, x0, y0)
}

/// Pixels of a reduced window back in the image.
fn unwindow(pix: &[usize], ww: usize, f: usize, x0: usize, y0: usize, w: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(pix.len() * f * f);
    for &i in pix {
        let (x, y) = (x0 + (i % ww) * f, y0 + (i / ww) * f);
        for yy in 0..f {
            for xx in 0..f {
                out.push((y + yy) * w + x + xx);
            }
        }
    }
    out
}

/// Pixels inside a closed outline (even-odd), in image pixels.
fn polygon_pixels(pts: &[[f32; 2]], w: usize, h: usize) -> Vec<usize> {
    let (y0, y1) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    let mut out = vec![];
    for y in (y0.floor().max(0.0) as usize)..=(y1.ceil().max(0.0) as usize).min(h.saturating_sub(1)) {
        let yc = y as f32 + 0.5;
        let mut xs: Vec<f32> = vec![];
        for k in 0..pts.len() {
            let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
            if (a[1] <= yc) != (b[1] <= yc) {
                xs.push(a[0] + (yc - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
            }
        }
        xs.sort_by(f32::total_cmp);
        for pair in xs.chunks_exact(2) {
            let (xa, xb) = ((pair[0] - 0.5).ceil().max(0.0) as usize, ((pair[1] - 0.5).floor().max(-1.0) as isize).min(w as isize - 1));
            for x in xa as isize..=xb {
                out.push(y * w + x as usize);
            }
        }
    }
    out
}

/// What the user drew.
enum Stroke {
    Click([f32; 2]),
    /// A closed outline: its pixels, bounding box x0, y0, x1, y1.
    Loop(Vec<usize>, [f32; 4]),
    /// A line along the object: up to five points on it.
    Line(Vec<[f32; 2]>, [f32; 4]),
}

fn stroke(path: &[[f32; 2]], w: usize, h: usize) -> Stroke {
    let len: f32 = path.windows(2).map(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1])).sum();
    let (first, last) = (path[0], path[path.len() - 1]);
    if path.len() < 2 || len < 3.0 {
        return Stroke::Click(first);
    }
    let b = path.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
    let closed = (first[0] - last[0]).hypot(first[1] - last[1]) <= 0.25 * len && b[2] - b[0] >= 3.0 && b[3] - b[1] >= 3.0;
    if closed {
        let pix = polygon_pixels(path, w, h);
        if pix.len() >= 6 {
            return Stroke::Loop(pix, b);
        }
        return Stroke::Click([(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]);
    }
    // Five points evenly along the line.
    let mut pts = vec![];
    for k in 0..5 {
        let want = len * (k as f32 + 0.5) / 5.0;
        let mut run = 0.0;
        for p in path.windows(2) {
            let seg = (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]);
            if run + seg >= want && seg > 0.0 {
                let f = (want - run) / seg;
                pts.push([p[0][0] + f * (p[1][0] - p[0][0]), p[0][1] + f * (p[1][1] - p[0][1])]);
                break;
            }
            run += seg;
        }
    }
    Stroke::Line(pts, b)
}

/// One way to read the request.
struct Cand {
    pix: Vec<usize>,
    quality: f32,
}

/// Intersection over union of two pixel lists.
fn iou(a: &[usize], b: &HashSet<usize>) -> f32 {
    let both = a.iter().filter(|i| b.contains(i)).count();
    both as f32 / (a.len() + b.len() - both).max(1) as f32
}

/// How sure a blob's outline is, on SAM's 0..1 scale: an outline ten
/// noise sigmas high is as good as a mask SAM gives 0.8.
fn blob_quality(r: &segpick::Region) -> f32 {
    (1.0 - (-r.edge / 6.0).exp()) * if r.alone { 1.0 } else { 0.8 }
}

impl Sam {
    fn slice_data(&self, key: SliceKey, (w, h, d): (usize, usize, usize), values: &impl Fn(usize) -> Result<Vec<f32>>, (lo, hi): (f32, f32), dn: Denoise) -> Result<Arc<SliceData>> {
        if let Some(s) = self.inner.lock().unwrap().slice.clone().filter(|s| s.key == key) {
            return Ok(s);
        }
        let z = key.1;
        let vals = if dn.slab == 0 && dn.sigma <= 0.0 {
            values(z)?
        } else {
            let slices = (z.saturating_sub(dn.slab)..=(z + dn.slab).min(d.max(1) - 1)).map(values).collect::<Result<Vec<_>>>()?;
            super::denoise(&slices, w, h, dn.sigma)
        };
        if vals.len() != w * h {
            bail!("图像大小不对");
        }
        let gray = super::to_gray(&vals, lo, hi);
        let smooth = if dn.sigma >= 1.0 { vals } else { super::denoise(&[vals], w, h, 1.0) };
        let (med, sd) = segpick::median_sd(&smooth);
        let noise = segpick::noise_sigma(&smooth, w, h);
        let data = Arc::new(SliceData { key, gray, smooth, noise, med, sd });
        let mut inner = self.inner.lock().unwrap();
        inner.slice = Some(data.clone());
        inner.tiles.retain(|t| t.0 == key);
        Ok(data)
    }

    /// The model, loaded on first use; None when it is not downloaded.
    fn model(&self) -> Result<Option<Arc<NativePredictor>>> {
        let mut inner = self.inner.lock().unwrap();
        let have = inner.predictor.is_some() || super::sam31_path().is_some() || super::model_path().is_some_and(|p| p.metadata().is_ok_and(|m| m.len() == super::MODEL_SIZE));
        if !have {
            return Ok(None);
        }
        Self::predictor(&mut inner).map(Some)
    }

    /// Loads the model ahead of the first click.
    pub fn warm(&self) -> Result<()> {
        self.model().map(|_| ())
    }

    /// The model if it is in memory, and whether it runs on a GPU.
    fn loaded(&self) -> Option<(Arc<NativePredictor>, bool)> {
        let inner = self.inner.lock().unwrap();
        inner.predictor.clone().map(|p| (p, inner.backend.as_deref() != Some("CPU")))
    }

    fn has_tile(&self, key: SliceKey, rect: [usize; 4]) -> bool {
        self.inner.lock().unwrap().tiles.iter().any(|t| t.0 == key && t.1 == rect)
    }

    /// SAM's encoding of one part of the slice, kept for the next request.
    fn tile(&self, p: &NativePredictor, sd: &SliceData, rect: [usize; 4], w: usize) -> Result<Arc<tmv_sam::Encoded>> {
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(i) = inner.tiles.iter().position(|t| t.0 == sd.key && t.1 == rect) {
                let t = inner.tiles.remove(i);
                let enc = t.2.clone();
                inner.tiles.push(t);
                return Ok(enc);
            }
        }
        let (tw, th) = (rect[2] - rect[0], rect[3] - rect[1]);
        let mut crop = Vec::with_capacity(tw * th);
        for y in rect[1]..rect[3] {
            crop.extend_from_slice(&sd.gray[y * w + rect[0]..y * w + rect[2]]);
        }
        let t = std::time::Instant::now();
        let enc = Arc::new(p.encode_gray(&crop, tw, th)?);
        eprintln!("SAM: encoded {rect:?} of slice {} in {:.1?}", sd.key.1, t.elapsed());
        let mut inner = self.inner.lock().unwrap();
        inner.tiles.retain(|t| t.0 == sd.key);
        if inner.tiles.len() >= MAX_TILES {
            inner.tiles.remove(0);
        }
        inner.tiles.push((sd.key, rect, enc.clone()));
        Ok(enc)
    }

    /// The object a click or a drawn stroke means on slice `req.z`.
    pub fn pick(&self, req: &PickReq, (w, h, d): (usize, usize, usize), values: impl Fn(usize) -> Result<Vec<f32>>) -> Result<Picked> {
        if req.path.is_empty() || w < 8 || h < 8 {
            bail!("请在物体上点一下，或者绕着它画一圈");
        }
        let dn = req.denoise;
        let key = (req.token, req.z, req.c, req.t, req.lo.to_bits(), req.hi.to_bits(), dn.slab, dn.sigma.to_bits());
        let sd = self.slice_data(key, (w, h, d), &values, (req.lo, req.hi), dn)?;
        let path: Vec<[f32; 2]> = req.path.iter().map(|p| [p[0].clamp(0.0, w as f32 - 1.0), p[1].clamp(0.0, h as f32 - 1.0)]).collect();
        let view = [req.view[0].max(0.0), req.view[1].max(0.0), req.view[2].min(w as f32), req.view[3].min(h as f32)];
        let view_side = (view[2] - view[0]).max(view[3] - view[1]).max(32.0);
        let view_centre = ((view[0] + view[2]) / 2.0, (view[1] + view[3]) / 2.0);
        let drawn = stroke(&path, w, h);
        // The part SAM looks at: what is on screen for a click, the drawn
        // shape with room around it otherwise.
        let rect = match &drawn {
            Stroke::Click(_) => tile_rect(view_centre.0, view_centre.1, view_side, w, h),
            Stroke::Loop(_, b) | Stroke::Line(_, b) => {
                let side = (b[2] - b[0]).max(b[3] - b[1]);
                let all = tile_rect(view_centre.0, view_centre.1, view_side, w, h);
                let fits = b[0] >= all[0] as f32 && b[1] >= all[1] as f32 && b[2] <= all[2] as f32 && b[3] <= all[3] as f32;
                if matches!(drawn, Stroke::Line(..)) && fits && side * 2.5 >= view_side * 0.5 {
                    all
                } else {
                    tile_rect((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0, side * 2.5, w, h)
                }
            }
        };
        // A preview does not wait for the network: not for loading it, and
        // on a machine without a GPU not for encoding either.
        let mut waiting = false;
        let model = if req.preview {
            match self.loaded() {
                Some((p, gpu)) if gpu || self.has_tile(key, rect) => Some(p),
                Some(_) => {
                    waiting = true;
                    None
                }
                None => {
                    waiting = super::sam31_path().is_some() || super::model_path().is_some_and(|p| p.exists());
                    None
                }
            }
        } else {
            self.model()?
        };
        let px = |p: [f32; 2]| ((p[0] as usize).min(w - 1), (p[1] as usize).min(h - 1));
        let in_tile = |p: [f32; 2], r: [usize; 4]| tmv_sam::Point { x: p[0] - r[0] as f32, y: p[1] - r[1] as f32, positive: true };
        let mut cands: Vec<Cand> = vec![];
        match &drawn {
            Stroke::Click(c) => {
                let seed = px(*c);
                let (win, ww, wh, f, x0, y0) = window(&sd.smooth, w, h, c[0], c[1], view_side / 2.0, 512);
                if seed.0 >= x0 && seed.1 >= y0 {
                    let noise = if f > 1 { segpick::noise_sigma(&win, ww, wh) } else { sd.noise };
                    let regions = segpick::flood(&win, ww, wh, ((seed.0 - x0) / f, (seed.1 - y0) / f), noise);
                    let top = regions.iter().map(|r| r.edge).fold(0f32, f32::max);
                    for r in regions.iter().filter(|r| r.edge >= 3.0 && r.edge >= 0.5 * top) {
                        cands.push(Cand { pix: unwindow(&r.pix, ww, f, x0, y0, w), quality: blob_quality(r) });
                    }
                }
                if let Some(p) = &model {
                    let mut rect = rect;
                    loop {
                        let enc = self.tile(p, &sd, rect, w)?;
                        let outs = p.predict_multi(&enc, &[in_tile(*c, rect)], None)?;
                        let outs = if outs.len() == 4 { &outs[1..] } else { &outs[..] };
                        let tile_area = (rect[2] - rect[0]) * (rect[3] - rect[1]);
                        let mut any_whole = false;
                        let mut got = vec![];
                        for (low, q) in outs {
                            let (pix, cut) = low_to_pixels(low, rect, Some(seed), None, w, h);
                            if pix.is_empty() || pix.len() * 10 > tile_area * 6 {
                                continue;
                            }
                            any_whole |= !cut;
                            got.push(Cand { pix, quality: q.clamp(0.0, 1.0) * if cut { 0.5 } else { 1.0 } });
                        }
                        // Every mask runs out of what is on screen: the
                        // object is larger, so look at the whole slice.
                        if !any_whole && !got.is_empty() && rect != [0, 0, w, h] && !req.preview {
                            rect = [0, 0, w, h];
                            continue;
                        }
                        cands.extend(got);
                        break;
                    }
                }
            }
            Stroke::Loop(poly, b) => {
                let inside: HashSet<usize> = poly.iter().copied().collect();
                let centre = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
                let seed = px(centre);
                let seed = if inside.contains(&(seed.1 * w + seed.0)) { seed } else { (poly[poly.len() / 2] % w, poly[poly.len() / 2] / w) };
                let side = (b[2] - b[0]).max(b[3] - b[1]);
                // The drawn shape itself, when nothing in the image agrees with it.
                cands.push(Cand { pix: poly.clone(), quality: 0.45 });
                let (win, ww, wh, f, x0, y0) = window(&sd.smooth, w, h, centre[0], centre[1], side * 3.0, 512);
                if seed.0 >= x0 && seed.1 >= y0 {
                    let noise = if f > 1 { segpick::noise_sigma(&win, ww, wh) } else { sd.noise };
                    for r in segpick::flood(&win, ww, wh, ((seed.0 - x0) / f, (seed.1 - y0) / f), noise) {
                        let pix = unwindow(&r.pix, ww, f, x0, y0, w);
                        let q = iou(&pix, &inside);
                        cands.push(Cand { pix, quality: q });
                    }
                }
                if let Some(p) = &model {
                    let enc = self.tile(p, &sd, rect, w)?;
                    let bbox = [b[0] - rect[0] as f32, b[1] - rect[1] as f32, b[2] - rect[0] as f32, b[3] - rect[1] as f32];
                    let grow = side * 0.15;
                    let clip = [(b[0] - grow).max(0.0) as usize, (b[1] - grow).max(0.0) as usize, ((b[2] + grow).ceil() as usize).min(w), ((b[3] + grow).ceil() as usize).min(h)];
                    for (low, _) in p.predict_multi(&enc, &[in_tile([seed.0 as f32 + 0.5, seed.1 as f32 + 0.5], rect)], Some(bbox))? {
                        let (pix, _) = low_to_pixels(&low, rect, None, Some(clip), w, h);
                        if !pix.is_empty() {
                            let q = iou(&pix, &inside);
                            cands.push(Cand { pix, quality: q });
                        }
                    }
                }
            }
            Stroke::Line(pts, _) => {
                let on = |pix: &[usize]| {
                    let set: HashSet<usize> = pix.iter().copied().collect();
                    pts.iter().filter(|p| set.contains(&(px(**p).1 * w + px(**p).0))).count() as f32 / pts.len() as f32
                };
                let mid = pts[pts.len() / 2];
                let seed = px(mid);
                let (win, ww, wh, f, x0, y0) = window(&sd.smooth, w, h, mid[0], mid[1], view_side / 2.0, 512);
                if seed.0 >= x0 && seed.1 >= y0 {
                    let noise = if f > 1 { segpick::noise_sigma(&win, ww, wh) } else { sd.noise };
                    for r in segpick::flood(&win, ww, wh, ((seed.0 - x0) / f, (seed.1 - y0) / f), noise) {
                        let pix = unwindow(&r.pix, ww, f, x0, y0, w);
                        let q = blob_quality(&r) * on(&pix);
                        cands.push(Cand { pix, quality: q });
                    }
                }
                if let Some(p) = &model {
                    let enc = self.tile(p, &sd, rect, w)?;
                    let prompts: Vec<tmv_sam::Point> = pts.iter().map(|q| in_tile(*q, rect)).collect();
                    for (low, q) in p.predict_multi(&enc, &prompts, None)? {
                        // The parts of the mask the line runs through.
                        let mut pix: Vec<usize> = vec![];
                        let mut seen: HashSet<usize> = HashSet::new();
                        for pt in pts {
                            let s = px(*pt);
                            if seen.contains(&(s.1 * w + s.0)) {
                                continue;
                            }
                            let (part, _) = low_to_pixels(&low, rect, Some(s), None, w, h);
                            seen.extend(part.iter().copied());
                            pix.extend(part);
                        }
                        if !pix.is_empty() {
                            let quality = q.clamp(0.0, 1.0) * on(&pix);
                            cands.push(Cand { pix, quality });
                        }
                    }
                }
            }
        }
        cands.retain(|c| c.quality > 0.02 && c.pix.len() >= 4);
        if cands.is_empty() {
            bail!("这里没有认出物体，试试绕着它画一圈");
        }
        // Smallest first; of two nearly equal ones the surer stays.
        cands.sort_by_key(|c| c.pix.len());
        let mut kept: Vec<Cand> = vec![];
        for c in cands {
            let same = kept.iter().position(|k| {
                c.pix.len() as f32 <= k.pix.len() as f32 * 1.3 && {
                    let set: HashSet<usize> = k.pix.iter().copied().collect();
                    iou(&c.pix, &set) >= 0.7
                }
            });
            match same {
                Some(i) if kept[i].quality >= c.quality => {}
                Some(i) => kept[i] = c,
                None => kept.push(c),
            }
        }
        // A few readings to step through, not every level of a flood.
        let top = kept.iter().map(|c| c.quality).fold(0f32, f32::max);
        kept.retain(|c| c.quality >= 0.35 * top);
        while kept.len() > 4 {
            let worst = (0..kept.len()).min_by(|&a, &b| kept[a].quality.total_cmp(&kept[b].quality)).unwrap();
            kept.remove(worst);
        }
        let best = (0..kept.len()).max_by(|&a, &b| kept[a].quality.total_cmp(&kept[b].quality)).unwrap();
        let choice = req.choice.map_or(best, |c| c.min(kept.len() - 1));
        let c = &kept[choice];
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for &i in &c.pix {
            (x0, y0, x1, y1) = (x0.min(i % w), y0.min(i / w), x1.max(i % w), y1.max(i / w));
        }
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut mask = vec![0u8; bw * bh];
        for &i in &c.pix {
            mask[(i / w - y0) * bw + i % w - x0] = 1;
        }
        let first = px(path[0]);
        let seed = if first.0 >= x0 && first.1 >= y0 && first.0 <= x1 && first.1 <= y1 && mask[(first.1 - y0) * bw + first.0 - x0] == 1 { first } else { (c.pix[0] % w, c.pix[0] / w) };
        if !req.preview {
            let mut full = vec![false; w * h];
            for &i in &c.pix {
                full[i] = true;
            }
            *self.last.lock().unwrap() = Some(LastMask { token: req.token, z: req.z, mask: full, iou: c.quality.max(0.5), low_res: vec![] });
        }
        Ok(Picked { rect: [x0, y0, bw, bh], mask, choices: kept.len(), choice, quality: c.quality, seed: [seed.0, seed.1], waiting })
    }

    /// The objects on slice `req.z` that look like the `examples` (pixel
    /// lists), best first; `taken` marks pixels that already have a label.
    /// Spots and particles are found by their outline alone; anything else
    /// by SAM: places whose image features resemble the examples' are
    /// clicked for it, part by part of the slice.
    pub fn similar(
        &self,
        req: &SimilarReq,
        (w, h, d): (usize, usize, usize),
        values: impl Fn(usize) -> Result<Vec<f32>>,
        examples: &[Vec<usize>],
        taken: &[bool],
        progress: impl Fn(usize, usize),
    ) -> Result<Vec<Found>> {
        use std::sync::atomic::Ordering::SeqCst;
        let examples: Vec<&Vec<usize>> = examples.iter().filter(|e| e.len() >= 4).collect();
        if examples.is_empty() {
            bail!("先选出一个要找的物体：在它上面点一下，或者绕着它画一圈");
        }
        self.cancel.store(false, SeqCst);
        let dn = req.denoise;
        let key = (req.token, req.z, req.c, req.t, req.lo.to_bits(), req.hi.to_bits(), dn.slab, dn.sigma.to_bits());
        let sd = self.slice_data(key, (w, h, d), &values, (req.lo, req.hi), dn)?;
        let stats = (sd.med, sd.sd, sd.noise);
        let mean_area = examples.iter().map(|e| e.len()).sum::<usize>() as f32 / examples.len() as f32;
        let diameter = 2.0 * (mean_area / std::f32::consts::PI).sqrt();
        progress(0, 1);

        // Blobs, on the slice reduced so an example is about 20 pixels wide.
        let f = ((diameter / 20.0).floor() as usize).max(1);
        let (small, sw, sh, _, _, _) = if f > 1 { window(&sd.smooth, w, h, w as f32 / 2.0, h as f32 / 2.0, w.max(h) as f32, w.max(h).div_ceil(f)) } else { (vec![], w, h, 1, 0, 0) };
        // `window` picks its own factor; it has to be the one meant here.
        let f = if f > 1 { w.max(h).div_ceil(w.max(h).div_ceil(f)).max(1) } else { 1 };
        let (sw, sh) = if f > 1 { (w / f, h / f) } else { (sw, sh) };
        let v: &[f32] = if f > 1 { &small } else { &sd.smooth };
        let reduce = |pix: &[usize]| -> Vec<usize> {
            if f == 1 {
                return pix.to_vec();
            }
            let mut set: Vec<usize> = pix.iter().filter(|&&i| (i % w) / f < sw && (i / w) / f < sh).map(|&i| ((i / w) / f) * sw + (i % w) / f).collect();
            set.sort_unstable();
            set.dedup();
            set
        };
        let ex_small: Vec<Vec<usize>> = examples.iter().map(|e| reduce(e)).collect();
        let taken_small: Vec<bool> = if f == 1 {
            taken.to_vec()
        } else {
            (0..sw * sh).map(|i| taken[((i / sw) * f + f / 2) * w + (i % sw) * f + f / 2]).collect()
        };
        let noise = if f > 1 { segpick::noise_sigma(v, sw, sh) } else { sd.noise };
        // An example is a blob when the flood from its middle outlines it.
        let blobby = ex_small.iter().all(|e| {
            let (cx, cy) = (e.iter().map(|i| i % sw).sum::<usize>() / e.len(), e.iter().map(|i| i / sw).sum::<usize>() / e.len());
            let half = ((e.len() as f32).sqrt() * 3.0).ceil().max(12.0) as usize;
            let (x0, y0, x1, y1) = (cx.saturating_sub(half), cy.saturating_sub(half), (cx + half + 1).min(sw), (cy + half + 1).min(sh));
            let (ww, wh) = (x1 - x0, y1 - y0);
            let win: Vec<f32> = (0..wh).flat_map(|r| v[(y0 + r) * sw + x0..(y0 + r) * sw + x1].iter().copied()).collect();
            let set: HashSet<usize> = e.iter().copied().collect();
            segpick::flood(&win, ww, wh, (cx - x0, cy - y0), noise).iter().any(|r| {
                let pix: Vec<usize> = r.pix.iter().map(|&i| (y0 + i / ww) * sw + x0 + i % ww).collect();
                iou(&pix, &set) >= 0.5
            })
        });
        let model = if blobby { None } else { self.model()? };
        let mut found: Vec<Found> = vec![];
        if blobby || model.is_none() {
            for b in segpick::find_blobs(v, sw, sh, &ex_small, f as f32, &taken_small, 0.002) {
                if f == 1 {
                    found.push(b);
                    continue;
                }
                let [x0, y0, bw, bh] = b.rect;
                let mut mask = vec![false; bw * f * bh * f];
                for y in 0..bh * f {
                    for x in 0..bw * f {
                        mask[y * bw * f + x] = b.mask[(y / f) * bw + x / f];
                    }
                }
                found.push(Found { rect: [x0 * f, y0 * f, bw * f, bh * f], mask, score: b.score, look: b.look });
            }
        }
        if let Some(p) = &model {
            let looks: Vec<segpick::Look> = examples.iter().map(|e| segpick::look(&sd.smooth, w, h, e, 1.0, stats)).collect();
            // Parts of the slice in which an example is about 43 of SAM's
            // 1024 pixels wide, sixteen at most, overlapping by three
            // examples so each object is whole in one of them.
            let mut side = (24.0 * diameter).max(256.0);
            let lap = |side: f32| (3.0 * diameter).min(side / 3.0);
            let count = |len: usize, side: f32| if len as f32 <= side { 1 } else { ((len as f32 - lap(side)) / (side - lap(side))).ceil() as usize };
            while count(w, side) * count(h, side) > 16 {
                side *= 1.2;
            }
            let rects: Vec<[usize; 4]> = if side >= 0.8 * w.max(h) as f32 {
                vec![[0, 0, w, h]]
            } else {
                let (nx, ny) = (count(w, side), count(h, side));
                let (tw, th) = ((side as usize).min(w), (side as usize).min(h));
                let at = |i: usize, n: usize, full: usize, len: usize| if n <= 1 { 0 } else { i * (full - len) / (n - 1) };
                (0..ny).flat_map(|j| (0..nx).map(move |i| (i, j))).map(|(i, j)| [at(i, nx, w, tw), at(j, ny, h, th), at(i, nx, w, tw) + tw, at(j, ny, h, th) + th]).collect()
            };
            // What the examples look like to SAM: their mean embedding, from
            // the first part that holds each one whole.
            let mut protos: Vec<Vec<f32>> = vec![];
            let mut embeds: Vec<Option<(Vec<f32>, usize, usize)>> = vec![None; rects.len()];
            let cell = |x: usize, y: usize, r: [usize; 4], n: usize| (((y - r[1]) * n / (r[3] - r[1])).min(n - 1)) * n + ((x - r[0]) * n / (r[2] - r[0])).min(n - 1);
            for e in &examples {
                let Some(k) = rects.iter().position(|r| e.iter().all(|&i| i % w >= r[0] && i % w < r[2] && i / w >= r[1] && i / w < r[3])) else { continue };
                if embeds[k].is_none() {
                    embeds[k] = Some(self.tile(p, &sd, rects[k], w)?.embedding());
                }
                let (emb, c, n) = embeds[k].as_ref().unwrap();
                let mut proto = vec![0f32; *c];
                for &i in e.iter() {
                    let g = cell(i % w, i / w, rects[k], *n);
                    for (ch, o) in proto.iter_mut().enumerate() {
                        *o += emb[ch * n * n + g];
                    }
                }
                let norm = proto.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
                proto.iter_mut().for_each(|x| *x /= norm);
                protos.push(proto);
            }
            if protos.is_empty() {
                bail!("示例太大了，这一层放不下完整的一个");
            }
            for (k, &rect) in rects.iter().enumerate() {
                if self.cancel.load(SeqCst) {
                    break;
                }
                progress(k, rects.len());
                let enc = self.tile(p, &sd, rect, w)?;
                let (emb, c, n) = match embeds[k].take() {
                    Some(e) => e,
                    None => enc.embedding(),
                };
                // Likeness of every cell to the nearest example.
                let mut sim = vec![0f32; n * n];
                for (g, s) in sim.iter_mut().enumerate() {
                    let norm = (0..c).map(|ch| emb[ch * n * n + g].powi(2)).sum::<f32>().sqrt().max(1e-6);
                    *s = protos.iter().map(|pr| (0..c).map(|ch| emb[ch * n * n + g] * pr[ch]).sum::<f32>() / norm).fold(f32::MIN, f32::max);
                }
                // The local maxima are clicked, most alike first.
                let mut seeds: Vec<(usize, f32)> = vec![];
                for g in 0..n * n {
                    let (gx, gy) = (g % n, g / n);
                    if sim[g] < 0.45 {
                        continue;
                    }
                    let top = (gy.saturating_sub(1)..=(gy + 1).min(n - 1)).all(|y| (gx.saturating_sub(1)..=(gx + 1).min(n - 1)).all(|x| sim[y * n + x] < sim[g] || (y * n + x) >= g));
                    if top {
                        seeds.push((g, sim[g]));
                    }
                }
                seeds.sort_by(|a, b| b.1.total_cmp(&a.1));
                seeds.truncate(96);
                let (tw, th) = (rect[2] - rect[0], rect[3] - rect[1]);
                let clicks: Vec<(usize, usize, f32)> = seeds
                    .iter()
                    .map(|&(g, s)| (rect[0] + ((g % n) * 2 + 1) * tw / (2 * n), rect[1] + ((g / n) * 2 + 1) * th / (2 * n), s))
                    .filter(|&(x, y, _)| !taken[y * w + x])
                    .collect();
                let points: Vec<tmv_sam::Point> = clicks.iter().map(|&(x, y, _)| tmv_sam::Point { x: (x - rect[0]) as f32 + 0.5, y: (y - rect[1]) as f32 + 0.5, positive: true }).collect();
                for (&(x, y, s), outs) in clicks.iter().zip(p.predict_multi_many(&enc, &points)?) {
                    let outs = if outs.len() == 4 { &outs[1..] } else { &outs[..] };
                    // Of part, object and whole, the one nearest the examples' size.
                    let size = |low: &[f32]| low.iter().filter(|l| **l > 0.0).count() as f32 * (tw * th) as f32 / low.len() as f32;
                    let Some((low, q)) = outs.iter().filter(|o| o.1 >= 0.4).min_by(|a, b| (size(&a.0) / mean_area).ln().abs().total_cmp(&(size(&b.0) / mean_area).ln().abs())) else { continue };
                    let (pix, cut) = low_to_pixels(low, rect, Some((x, y)), None, w, h);
                    if pix.len() < 4 || cut || pix.iter().filter(|&&i| taken[i]).count() * 5 > pix.len() {
                        continue;
                    }
                    let look = segpick::look(&sd.smooth, w, h, &pix, 1.0, stats);
                    let score = segpick::alike(&look, &looks) * ((s - 0.3) / 0.4).clamp(0.3, 1.0) * (q / 0.7).clamp(0.0, 1.0);
                    if score >= 0.002 {
                        found.push(Found::from_pixels(&pix, w, (0, 0), score, look));
                    }
                }
            }
        }
        progress(1, 1);
        let found = segpick::dedupe(found);
        *self.found.lock().unwrap() = Some(LastFound { token: req.token, z: req.z, found: found.clone() });
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_snap_and_stay_inside() {
        assert_eq!(tile_rect(600.0, 600.0, 1100.0, 1200, 1200), [0, 0, 1200, 1200]);
        let a = tile_rect(600.0, 500.0, 300.0, 1200, 1200);
        let b = tile_rect(610.0, 505.0, 310.0, 1200, 1200);
        assert_eq!(a, b);
        assert_eq!(a[2] - a[0], 362);
        assert!(a[0] <= 450 && a[2] >= 750);
        let c = tile_rect(5.0, 1195.0, 200.0, 1200, 1200);
        assert_eq!((c[0], c[3]), (0, 1200));
    }

    #[test]
    fn strokes_are_read_as_click_loop_or_line() {
        assert!(matches!(stroke(&[[5.0, 5.0]], 100, 100), Stroke::Click(_)));
        assert!(matches!(stroke(&[[5.0, 5.0], [6.0, 5.5]], 100, 100), Stroke::Click(_)));
        let ring: Vec<[f32; 2]> = (0..40).map(|k| (k as f32 / 40.0 * std::f32::consts::TAU).sin_cos()).map(|(s, c)| [50.0 + 20.0 * c, 40.0 + 20.0 * s]).collect();
        match stroke(&ring, 100, 100) {
            Stroke::Loop(pix, b) => {
                assert!((pix.len() as f32 - 1256.0).abs() < 80.0, "{}", pix.len());
                assert!(b[0] < 31.0 && b[2] > 69.0);
            }
            _ => panic!("a ring is a loop"),
        }
        match stroke(&[[10.0, 10.0], [50.0, 12.0], [90.0, 10.0]], 100, 100) {
            Stroke::Line(pts, _) => {
                assert_eq!(pts.len(), 5);
                assert!((pts[2][0] - 50.0).abs() < 1.0);
            }
            _ => panic!("a line is a line"),
        }
    }

    #[test]
    fn low_resolution_masks_come_back_at_image_size() {
        // A disc of radius 8 cells on a 64² grid over the tile (100..356)².
        let n = 64;
        let low: Vec<f32> = (0..n * n).map(|i| 8.0 - ((i % n) as f32 + 0.5 - 32.0).hypot((i / n) as f32 + 0.5 - 32.0)).collect();
        let (pix, cut) = low_to_pixels(&low, [100, 100, 356, 356], Some((228, 228)), None, 1000, 1000);
        assert!(!cut);
        let want = std::f32::consts::PI * 32.0 * 32.0;
        assert!((pix.len() as f32 - want).abs() < want * 0.05, "{} vs {want}", pix.len());
        assert!(pix.iter().all(|&i| ((i % 1000) as f32 + 0.5 - 228.0).hypot((i / 1000) as f32 + 0.5 - 228.0) < 34.0));
        // A seed outside the mask selects nothing.
        assert!(low_to_pixels(&low, [100, 100, 356, 356], Some((110, 110)), None, 1000, 1000).0.is_empty());
        // A mask over the whole tile is cut off by it.
        assert!(low_to_pixels(&vec![1.0; n * n], [100, 100, 356, 356], Some((228, 228)), None, 1000, 1000).1);
        assert!(!low_to_pixels(&vec![1.0; n * n], [0, 0, 256, 256], Some((128, 128)), None, 256, 256).1);
    }
}
