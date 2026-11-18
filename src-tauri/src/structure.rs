//! Refines a rough painted stroke into the structure the user named
//! (labels panel: paint, then pick 小颗粒 / 球 / 空心球 / 膜 / ...).
//!
//! The paint is the only prior: scale, polarity (dark or bright line) and
//! thickness are read from the stroke and the image under it, never typed.
//! A refiner that is not sure says so and the rough paint stays.
//!
//! Line-like structures use one engine (`band`): the image is straightened
//! along the stroke's centre line, a ridge filter across the band at the
//! scale that answers best gives a score per (along, across) position, and
//! dynamic programming finds the smooth path of the strongest ridge. A hand
//! paints beside a thin line as often as on it, so the paint is a hint, not
//! a boundary: the search reaches a stroke width beyond the paint on each
//! side, with a cost that grows with the distance from it. A second pass
//! matches each column against the median profile of the stroke itself (a
//! template learnt from this very stroke). Each stretch of the stroke is
//! judged on its own: where the line is clear it is refined, where it is not
//! the rough paint of that stretch stays.

use crate::labels::{Labels, Mark, Values};
use anyhow::Result;
use serde::Deserialize;

/// A stroke as the labels panel drew it.
#[derive(Clone, Debug, Deserialize)]
pub struct Stroke {
    pub z: usize,
    /// brush | lasso | wand
    pub tool: String,
    #[serde(default)]
    pub pts: Vec<[f32; 2]>,
    #[serde(default)]
    pub radius: f32,
    #[serde(default)]
    pub seed: [f32; 2],
}

pub enum Outcome {
    /// The refined shape (pixels of the slice), one description per refined
    /// stretch, and a note when part of the paint was kept as it was.
    Done(Vec<bool>, Vec<Mark>, String),
    /// Keep the rough paint; the reason is shown to the user.
    Rough(String),
}

/// Pixels of this stroke that still carry `from` on its slice.
pub fn stroke_pixels(l: &Labels, s: &Stroke, from: u16) -> Vec<bool> {
    let (w, h) = (l.w, l.h);
    let n = w * h;
    let cur = l.slice(s.z);
    let at = |i: usize| cur.map_or(0, |c| c[i]);
    let mut m = vec![false; n];
    match s.tool.as_str() {
        // All of the layer's paint on the slice (the caller removes what is
        // refined already).
        "region" => {
            for (i, v) in m.iter_mut().enumerate() {
                *v = at(i) == from;
            }
        }
        "wand" => {
            let (x, y) = (s.seed[0].floor() as i64, s.seed[1].floor() as i64);
            if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                return m;
            }
            let i0 = y as usize * w + x as usize;
            if at(i0) != from {
                return m;
            }
            let mut stack = vec![i0];
            m[i0] = true;
            while let Some(i) = stack.pop() {
                let (px, py) = (i % w, i / w);
                let near = [(px > 0).then(|| i - 1), (px + 1 < w).then(|| i + 1), (py > 0).then(|| i - w), (py + 1 < h).then(|| i + w)];
                for j in near.into_iter().flatten() {
                    if !m[j] && at(j) == from {
                        m[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        tool => {
            let mut tmp = Labels::new(w, h, 1);
            if tool == "lasso" {
                tmp.polygon(0, &s.pts, 1, 0);
            } else {
                tmp.stroke(0, &s.pts, s.radius, 1, 0);
            }
            if let Some(r) = tmp.slice(0) {
                for i in 0..n {
                    m[i] = r[i] == 1 && at(i) == from;
                }
            }
        }
    }
    m
}

/// Refines every object of the rough paint `paint` on slice `z` on its own:
/// pieces within `gap` of each other are one object. Returns the pixels to
/// write (refined objects, and the paint of those that stayed rough), the
/// refined objects' descriptions and a note, or `None` when nothing refined.
pub fn refine_all(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool], gap: f32, structure: &str) -> Result<(Option<(Vec<bool>, Vec<Mark>)>, String)> {
    let (w, h, _) = dims;
    let near = crate::labels::edt(paint, w, h);
    let parts = crate::labels::components_where(w, h, |i| near[i] <= gap / 2.0);
    let mut add = vec![false; w * h];
    let mut marks = vec![];
    let mut notes: Vec<String> = vec![];
    let mut rough = 0;
    let mut done = 0;
    for part in parts {
        let px: Vec<usize> = part.into_iter().filter(|&i| paint[i]).collect();
        if px.len() < 4 {
            px.iter().for_each(|&i| add[i] = true);
            continue;
        }
        let mut one = vec![false; w * h];
        px.iter().for_each(|&i| one[i] = true);
        match refine(vals, dims, z, &one, gap, structure)? {
            Outcome::Done(a, m, note) => {
                done += 1;
                for (o, v) in add.iter_mut().zip(a) {
                    *o |= v;
                }
                marks.extend(m);
                if !note.is_empty() {
                    notes.push(note);
                }
            }
            Outcome::Rough(note) => {
                px.iter().for_each(|&i| add[i] = true);
                rough += 1;
                if !note.is_empty() {
                    notes.push(note);
                }
            }
        }
    }
    notes.dedup();
    let note = notes.first().cloned().unwrap_or_default();
    if done == 0 {
        return Ok((None, note));
    }
    let note = if rough > 0 && note.is_empty() { format!("有 {rough} 处没找到清楚的结构，保留了你画的样子") } else { note };
    Ok((Some((add, marks)), note))
}

/// Refines the rough paint `paint` (pixels of slice `z`: one object, maybe
/// painted in several strokes) as `structure`. `gap`: pieces of paint this
/// close belong together (about a brush radius).
pub fn refine(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool], gap: f32, structure: &str) -> Result<Outcome> {
    match structure {
        // A fibre is an open line like a membrane: same band engine.
        "membrane" | "fibre" => membrane(vals, dims, z, paint, gap),
        "membrane2" => membrane2(vals, dims, z, paint, gap),
        "particle" => particles(vals, dims, z, paint),
        "ball" => round(vals, dims, z, paint, gap, false),
        "shell" => round(vals, dims, z, paint, gap, true),
        "free" => free(vals, dims, z, paint),
        "" => Ok(Outcome::Rough(String::new())),
        _ => Ok(Outcome::Rough("这个类型的精细分割还没做好，先保留你画的样子".into())),
    }
}

// ---------- small helpers ----------

fn gauss_kernel(sigma: f32) -> Vec<f32> {
    if sigma < 0.3 {
        return vec![1.0];
    }
    let r = (3.0 * sigma).ceil() as i32;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= s);
    k
}

/// 1D convolution with clamped ends.
fn smooth1(v: &[f32], k: &[f32]) -> Vec<f32> {
    let r = (k.len() / 2) as i64;
    let n = v.len() as i64;
    (0..n)
        .map(|i| k.iter().enumerate().map(|(j, kv)| kv * v[(i + j as i64 - r).clamp(0, n - 1) as usize]).sum())
        .collect()
}

/// Separable Gaussian blur of a `w`×`h` image.
fn blur(v: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let k = gauss_kernel(sigma);
    if k.len() == 1 {
        return v.to_vec();
    }
    let mut a = vec![0f32; w * h];
    for y in 0..h {
        let row = smooth1(&v[y * w..(y + 1) * w], &k);
        a[y * w..(y + 1) * w].copy_from_slice(&row);
    }
    let mut out = vec![0f32; w * h];
    let mut col = vec![0f32; h];
    for x in 0..w {
        for y in 0..h {
            col[y] = a[y * w + x];
        }
        for (y, v) in smooth1(&col, &k).into_iter().enumerate() {
            out[y * w + x] = v;
        }
    }
    out
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mid = v.len() / 2;
    v.select_nth_unstable_by(mid, f32::total_cmp);
    v[mid]
}

/// A polyline resampled at `step` pixels along its length.
fn resample(pts: &[[f32; 2]], step: f32) -> Vec<[f32; 2]> {
    let mut out = vec![pts[0]];
    let mut carry = 0f32;
    for win in pts.windows(2) {
        let (a, b) = (win[0], win[1]);
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if len <= 0.0 {
            continue;
        }
        let mut t = step - carry;
        while t <= len {
            let f = t / len;
            out.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]);
            t += step;
        }
        carry = len - (t - step);
    }
    out
}

/// A window of the slice (averaged over a few neighbouring slices for a
/// stack), blurred lightly and in robust units (median 0, sd 1).
struct Roi {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Roi {
    fn read(vals: &dyn Values, (w, h, d): (usize, usize, usize), z: usize, b: [f32; 4]) -> Result<Roi> {
        let x0 = (b[0].floor().max(0.0) as usize).min(w - 1);
        let y0 = (b[1].floor().max(0.0) as usize).min(h - 1);
        let x1 = (b[2].ceil().max(0.0) as usize).clamp(x0 + 1, w);
        let y1 = (b[3].ceil().max(0.0) as usize).clamp(y0 + 1, h);
        let (rw, rh) = (x1 - x0, y1 - y0);
        let slab = if d > 1 { 2 } else { 0 };
        let zs = z.saturating_sub(slab)..(z + slab + 1).min(d);
        let mut v = vec![0f32; rw * rh];
        let n = zs.len() as f32;
        for zz in zs {
            let s = vals.slice(zz)?;
            for y in 0..rh {
                for x in 0..rw {
                    v[y * rw + x] += s[(y0 + y) * w + x0 + x] / n;
                }
            }
        }
        let mut v = blur(&v, rw, rh, 1.0);
        let (med, sd) = crate::segpick::median_sd(&v);
        v.iter_mut().for_each(|p| *p = (*p - med) / sd);
        Ok(Roi { x0, y0, w: rw, h: rh, v })
    }

    /// Bilinear value at image position (x, y); NaN outside.
    fn at(&self, x: f32, y: f32) -> f32 {
        let (x, y) = (x - self.x0 as f32 - 0.5, y - self.y0 as f32 - 0.5);
        if x < 0.0 || y < 0.0 || x > (self.w - 1) as f32 || y > (self.h - 1) as f32 {
            return f32::NAN;
        }
        let (ix, iy) = ((x as usize).min(self.w.saturating_sub(2)), (y as usize).min(self.h.saturating_sub(2)));
        let (fx, fy) = (x - ix as f32, y - iy as f32);
        let g = |xx: usize, yy: usize| self.v[yy.min(self.h - 1) * self.w + xx.min(self.w - 1)];
        let a = g(ix, iy) * (1.0 - fx) + g(ix + 1, iy) * fx;
        let b = g(ix, iy + 1) * (1.0 - fx) + g(ix + 1, iy + 1) * fx;
        a * (1.0 - fy) + b * fy
    }
}

/// The best path through `r` (S rows of T columns): one column per row,
/// moving at most one column between rows, maximising the summed score
/// minus `lambda` per step sideways. Columns 0 and T-1 are not used.
fn best_path(r: &[f32], s_len: usize, t_len: usize, lambda: f32) -> Vec<usize> {
    let mut acc = vec![f32::NEG_INFINITY; s_len * t_len];
    let mut from = vec![0u16; s_len * t_len];
    for t in 1..t_len - 1 {
        acc[t] = r[t];
    }
    for s in 1..s_len {
        for t in 1..t_len - 1 {
            let mut best = (f32::NEG_INFINITY, t);
            for p in [t - 1, t, t + 1] {
                if p == 0 || p >= t_len - 1 {
                    continue;
                }
                let v = acc[(s - 1) * t_len + p] - if p != t { lambda } else { 0.0 };
                if v > best.0 {
                    best = (v, p);
                }
            }
            acc[s * t_len + t] = best.0 + r[s * t_len + t];
            from[s * t_len + t] = best.1 as u16;
        }
    }
    let last = s_len - 1;
    let mut t = (1..t_len - 1).max_by(|&a, &b| acc[last * t_len + a].total_cmp(&acc[last * t_len + b])).unwrap_or(t_len / 2);
    let mut path = vec![0usize; s_len];
    for s in (0..s_len).rev() {
        path[s] = t;
        if s > 0 {
            t = from[s * t_len + t] as usize;
        }
    }
    path
}

// ---------- 单层膜 ----------

/// Thins `m` (`w`×`h`, false on the border) to one-pixel lines (Zhang & Suen).
fn thin(m: &mut [bool], w: usize, h: usize) {
    let mut del = vec![];
    loop {
        let mut changed = false;
        for step in 0..2 {
            del.clear();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let i = y * w + x;
                    if !m[i] {
                        continue;
                    }
                    // P2..P9 clockwise from north.
                    let p = [m[i - w], m[i - w + 1], m[i + 1], m[i + w + 1], m[i + w], m[i + w - 1], m[i - 1], m[i - w - 1]];
                    let b = p.iter().filter(|&&v| v).count();
                    let a = (0..8).filter(|&k| !p[k] && p[(k + 1) % 8]).count();
                    let ok = if step == 0 { !(p[0] && p[2] && p[4]) && !(p[2] && p[4] && p[6]) } else { !(p[0] && p[2] && p[6]) && !(p[0] && p[4] && p[6]) };
                    if (2..=6).contains(&b) && a == 1 && ok {
                        del.push(i);
                    }
                }
            }
            for &i in &del {
                m[i] = false;
            }
            changed |= !del.is_empty();
        }
        if !changed {
            break;
        }
    }
}

/// The centre line of a painted object, however it was painted (one stroke,
/// several overlapping ones, a stroke that doubles back, a lasso or a wand
/// region): pieces within `gap` are joined, the joined paint is thinned to a
/// skeleton, and the longest path through the skeleton is the line. Also
/// returns the paint's half width along that line.
fn centre_line(paint: &[bool], w: usize, h: usize, gap: f32) -> Option<(Vec<[f32; 2]>, f32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, _) in paint.iter().enumerate().filter(|(_, &v)| v) {
        let (x, y) = (i % w, i / w);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    if x0 > x1 {
        return None;
    }
    let pad = gap.ceil() as usize + 2;
    let (ox, oy) = (x0 as i64 - pad as i64, y0 as i64 - pad as i64);
    let (lw, lh) = (x1 - x0 + 1 + 2 * pad, y1 - y0 + 1 + 2 * pad);
    let local: Vec<bool> = (0..lw * lh)
        .map(|j| {
            let (x, y) = ((j % lw) as i64 + ox, (j / lw) as i64 + oy);
            x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && paint[y as usize * w + x as usize]
        })
        .collect();
    let inside = crate::labels::edt(&local.iter().map(|&v| !v).collect::<Vec<_>>(), lw, lh);
    let near = crate::labels::edt(&local, lw, lh);
    let mut sk: Vec<bool> = near.iter().map(|&d| d <= gap).collect();
    for x in 0..lw {
        sk[x] = false;
        sk[(lh - 1) * lw + x] = false;
    }
    for y in 0..lh {
        sk[y * lw] = false;
        sk[y * lw + lw - 1] = false;
    }
    thin(&mut sk, lw, lh);
    // Longest path: farthest pixel from any pixel, then farthest from that,
    // in every piece of the skeleton; the longest wins.
    let nbrs = |i: usize| {
        let (x, y) = ((i % lw) as i64, (i / lw) as i64);
        [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].into_iter().filter_map(move |(dx, dy)| {
            let (a, b) = (x + dx, y + dy);
            (a >= 0 && b >= 0 && (a as usize) < lw && (b as usize) < lh).then(|| (b as usize * lw + a as usize, if dx != 0 && dy != 0 { 1.414f32 } else { 1.0 }))
        })
    };
    // Dijkstra-lite: skeletons are thin, a few passes of relaxation suffice.
    let far = |start: usize| -> (usize, Vec<f32>, Vec<usize>) {
        let mut dist = vec![f32::INFINITY; lw * lh];
        let mut from = vec![usize::MAX; lw * lh];
        let mut queue = std::collections::VecDeque::from([start]);
        dist[start] = 0.0;
        while let Some(i) = queue.pop_front() {
            for (j, c) in nbrs(i) {
                if sk[j] && dist[i] + c < dist[j] {
                    dist[j] = dist[i] + c;
                    from[j] = i;
                    queue.push_back(j);
                }
            }
        }
        let end = (0..lw * lh).filter(|&j| dist[j].is_finite()).max_by(|&a, &b| dist[a].total_cmp(&dist[b])).unwrap_or(start);
        (end, dist, from)
    };
    let mut seen = vec![false; lw * lh];
    let mut best: Option<(f32, Vec<usize>)> = None;
    for i in 0..lw * lh {
        if !sk[i] || seen[i] {
            continue;
        }
        let (a, d0, _) = far(i);
        d0.iter().enumerate().filter(|(_, d)| d.is_finite()).for_each(|(j, _)| seen[j] = true);
        let (b, d1, from) = far(a);
        if best.as_ref().map_or(true, |x| d1[b] > x.0) {
            let mut path = vec![b];
            while *path.last().unwrap() != a {
                let p = from[*path.last().unwrap()];
                if p == usize::MAX {
                    break;
                }
                path.push(p);
            }
            best = Some((d1[b], path));
        }
    }
    let (_, path) = best?;
    let hw = median(path.iter().filter(|&&j| local[j]).map(|&j| inside[j]).collect());
    let pts = path.iter().map(|&j| [(j % lw) as f32 + ox as f32 + 0.5, (j / lw) as f32 + oy as f32 + 0.5]).collect();
    Some((pts, if hw > 0.0 { hw } else { gap.max(1.5) }))
}

fn membrane(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool], gap: f32) -> Result<Outcome> {
    let (w, h, _) = dims;
    let Some((centre, hw)) = centre_line(paint, w, h, gap) else {
        return Ok(Outcome::Rough(String::new()));
    };
    band_line(vals, dims, z, &centre, hw, paint, None, false)
}

/// The line engine: finds the strongest ridge line along `centre` (paint
/// half width `hw`), refines it and returns it. `avoid`: pixels the line must
/// not run through (a line found already). `closed`: the centre is a loop.
#[allow(clippy::too_many_arguments)]
fn band_line(vals: &dyn Values, dims: (usize, usize, usize), z: usize, centre: &[[f32; 2]], hw: f32, paint: &[bool], avoid: Option<&[bool]>, closed: bool) -> Result<Outcome> {
    let (w, h, _) = dims;
    let slice_z = z;
    if centre.len() < 2 {
        return Ok(Outcome::Rough("这一笔太短，看不出膜的走向，先保留你画的样子".into()));
    }
    let width = (2.0 * hw).max(3.0);
    let line = resample(&centre, 1.0);
    if line.len() < 8 {
        return Ok(Outcome::Rough("这一笔太短，看不出膜的走向，先保留你画的样子".into()));
    }
    // Smooth the hand's wobble so the normals do not cross, then resample.
    let k = gauss_kernel((width / 2.0).max(2.0));
    let xs = smooth1(&line.iter().map(|p| p[0]).collect::<Vec<_>>(), &k);
    let ys = smooth1(&line.iter().map(|p| p[1]).collect::<Vec<_>>(), &k);
    let c = resample(&xs.iter().zip(&ys).map(|(&x, &y)| [x, y]).collect::<Vec<_>>(), 1.0);
    let s_len = c.len();
    if s_len < 8 {
        return Ok(Outcome::Rough("这一笔太短，看不出膜的走向，先保留你画的样子".into()));
    }
    let normals: Vec<[f32; 2]> = (0..s_len)
        .map(|i| {
            let (a, b) = (c[i.saturating_sub(2)], c[(i + 2).min(s_len - 1)]);
            let (tx, ty) = (b[0] - a[0], b[1] - a[1]);
            let n = (tx * tx + ty * ty).sqrt().max(1e-6);
            [-ty / n, tx / n]
        })
        .collect();

    // The band reaches one paint width (at least 4 px) beyond the paint.
    let ph = width / 2.0;
    let reach = ph.max(4.0);
    let half = (ph + reach + 2.0).ceil() as usize;
    let t_len = 2 * half + 1;
    let pad = half as f32 + 8.0;
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in &c {
        b = [b[0].min(p[0] - pad), b[1].min(p[1] - pad), b[2].max(p[0] + pad), b[3].max(p[1] + pad)];
    }
    let roi = Roi::read(vals, dims, z, b)?;
    // Cost of leaving the paint: 0 inside it, 1 at the far edge of the band.
    let away: Vec<f32> = (0..t_len).map(|t| (((t as f32 - half as f32).abs() - ph).max(0.0) / reach).min(1.0)).collect();

    // Straightened band: row s along the line, column t across it.
    let mut band = vec![0f32; s_len * t_len];
    for s in 0..s_len {
        for t in 0..t_len {
            let off = t as f32 - half as f32;
            let v = roi.at(c[s][0] + off * normals[s][0], c[s][1] + off * normals[s][1]);
            band[s * t_len + t] = if v.is_finite() { v } else { 0.0 };
        }
    }
    // Along the membrane the signal is constant and the noise is not.
    let along = gauss_kernel(4.0);
    let mut sm = vec![0f32; s_len * t_len];
    let mut col = vec![0f32; s_len];
    for t in 0..t_len {
        for s in 0..s_len {
            col[s] = band[s * t_len + t];
        }
        for (s, v) in smooth1(&col, &along).into_iter().enumerate() {
            sm[s * t_len + t] = v;
        }
    }

    // Ridge across the band at the scale and sign that answer best.
    let ridge = |sigma: f32, sign: f32| -> Vec<f32> {
        let k = gauss_kernel(sigma);
        let mut r = vec![0f32; s_len * t_len];
        for s in 0..s_len {
            let row = smooth1(&sm[s * t_len..(s + 1) * t_len], &k);
            for t in 1..t_len - 1 {
                r[s * t_len + t] = sign * sigma * sigma * (row[t - 1] - 2.0 * row[t] + row[t + 1]);
            }
        }
        r
    };
    let row_max_median = |r: &[f32]| median((0..s_len).map(|s| r[s * t_len + 1..(s + 1) * t_len - 1].iter().copied().fold(f32::MIN, f32::max)).collect());
    // The scale may follow the line's own width, not the paint's.
    let mut best: Option<(f32, f32, f32, Vec<f32>)> = None;
    for &sigma in &[1.0f32, 1.4, 2.0, 2.8, 4.0, 5.6] {
        if sigma > (width / 4.0).max(1.4) {
            break;
        }
        for sign in [1.0f32, -1.0] {
            let r = ridge(sigma, sign);
            let m = row_max_median(&r);
            if best.as_ref().map_or(true, |b| m > b.0) {
                best = Some((m, sigma, sign, r));
            }
        }
    }
    let Some((m1, sigma, sign, mut r1)) = best else { return Ok(Outcome::Rough("这一笔太细了".into())) };
    // Positions on a line found already are out of bounds.
    let blocked: Vec<bool> = (0..s_len * t_len)
        .map(|i| {
            let Some(av) = avoid else { return false };
            let (s, t) = (i / t_len, i % t_len);
            let off = t as f32 - half as f32;
            let (x, y) = (c[s][0] + off * normals[s][0], c[s][1] + off * normals[s][1]);
            x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h && av[y as usize * w + x as usize]
        })
        .collect();
    let block = |r: &mut [f32], m: f32| r.iter_mut().zip(&blocked).filter(|(_, &b)| b).for_each(|(v, _)| *v = -4.0 * m.abs().max(1e-3));
    block(&mut r1, m1);
    let with_cost = |r: &[f32], m: f32| -> Vec<f32> { (0..s_len * t_len).map(|i| r[i] - 0.5 * m.max(0.0) * away[i % t_len]).collect() };
    let path1 = best_path(&with_cost(&r1, m1), s_len, t_len, 0.5 * m1.max(0.0));

    // Confidence, stretch by stretch: the ridge on the path against the
    // spread of the band, averaged over a short piece of the line.
    let inner: Vec<f32> = (0..s_len).flat_map(|s| r1[s * t_len + 1..(s + 1) * t_len - 1].to_vec()).collect();
    let med_all = median(inner.clone());
    let sd = 1.4826 * median(inner.iter().map(|v| (v - med_all).abs()).collect()).max(1e-6);
    let z: Vec<f32> = (0..s_len).map(|s| (r1[s * t_len + path1[s]] - med_all) / sd).collect();
    let z = smooth1(&z, &gauss_kernel(6.0));
    let min_run = (width.max(12.0)) as usize;
    let runs = |ok: &dyn Fn(usize) -> bool| -> Vec<(usize, usize)> {
        let mut out = vec![];
        let mut s = 0;
        while s < s_len {
            if !ok(s) {
                s += 1;
                continue;
            }
            let a = s;
            while s < s_len && ok(s) {
                s += 1;
            }
            if s - a >= min_run {
                out.push((a, s - 1));
            }
        }
        out
    };
    // Hysteresis: a stretch is seeded where the line is clearly there (2 sd)
    // and followed while it stays above 1 sd, so a faint piece between two
    // clear ones does not cut the line.
    let mut grown = vec![false; s_len];
    for s in 0..s_len {
        if z[s] >= 2.0 && !grown[s] {
            let mut a = s;
            while a > 0 && z[a - 1] >= 1.0 {
                a -= 1;
            }
            let mut b = s;
            while b + 1 < s_len && z[b + 1] >= 1.0 {
                b += 1;
            }
            grown[a..=b].iter_mut().for_each(|g| *g = true);
        }
    }
    let found = runs(&|s| grown[s]);
    if found.is_empty() {
        return Ok(Outcome::Rough("在你涂的地方附近没找到清楚的膜，先保留你画的样子".into()));
    }
    let on_line = |s: usize| found.iter().any(|&(a, b)| s >= a && s <= b);

    // Second pass: the stroke's own median profile as a template.
    let hw = half as i64;
    let profile_at = |path: &[usize]| -> Vec<f32> {
        (-hw..=hw)
            .map(|u| {
                median(
                    (0..s_len)
                        .filter(|&s| on_line(s))
                        .filter_map(|s| {
                            let t = path[s] as i64 + u;
                            (t >= 0 && t < t_len as i64).then(|| band[s * t_len + t as usize])
                        })
                        .collect(),
                )
            })
            .collect()
    };
    let tpl = profile_at(&path1);
    let kw = ((2.5 * sigma).round() as i64).clamp(2, hw);
    let tp: Vec<f32> = (-kw..=kw).map(|u| tpl[(u + hw) as usize]).collect();
    let tm = tp.iter().sum::<f32>() / tp.len() as f32;
    let tn = tp.iter().map(|v| (v - tm).powi(2)).sum::<f32>().sqrt().max(1e-6);
    let mut r2 = vec![0f32; s_len * t_len];
    for s in 0..s_len {
        for t in 1..t_len - 1 {
            let win: Vec<f32> = (-kw..=kw).map(|u| sm[s * t_len + (t as i64 + u).clamp(0, t_len as i64 - 1) as usize]).collect();
            let wm = win.iter().sum::<f32>() / win.len() as f32;
            let wn = win.iter().map(|v| (v - wm).powi(2)).sum::<f32>().sqrt().max(1e-6);
            r2[s * t_len + t] = win.iter().zip(&tp).map(|(a, b)| (a - wm) * (b - tm)).sum::<f32>() / (wn * tn);
        }
    }
    let m2 = row_max_median(&r2);
    block(&mut r2, m2);
    let path = best_path(&with_cost(&r2, m2), s_len, t_len, 0.3 * m2.max(0.0));

    // Trim the ends of each stretch where the template stops matching
    // (paint overshooting the line's end).
    let score: Vec<f32> = (0..s_len).map(|s| r2[s * t_len + path[s]]).collect();
    let cut = 0.3 * median((0..s_len).filter(|&s| on_line(s)).map(|s| score[s]).collect());
    let keep_runs: Vec<(usize, usize)> = found
        .iter()
        .filter_map(|&(a, b)| {
            let a2 = (a..=b).find(|&s| score[s] >= cut)?;
            let b2 = (a..=b).rev().find(|&s| score[s] >= cut)?;
            (b2 + 1 >= a2 + min_run).then_some((a2, b2))
        })
        .collect();
    if keep_runs.is_empty() {
        return Ok(Outcome::Rough("在你涂的地方附近没找到清楚的膜，先保留你画的样子".into()));
    }

    // Thickness: full width at half depth of the aligned profile, less our blur.
    let prof = profile_at(&path);
    let depth: Vec<f32> = {
        let outer: Vec<f32> = prof.iter().enumerate().filter(|(i, _)| (*i as i64 - hw).abs() >= hw - 1).map(|(_, v)| *v).collect();
        let base = outer.iter().sum::<f32>() / outer.len().max(1) as f32;
        prof.iter().map(|v| sign * (v - base)).map(|v| -v).collect()
    };
    // `depth` is positive at the line for both polarities (dark: below base).
    let peak = depth[hw as usize];
    let mut fwhm = 1.0f32;
    if peak > 0.0 {
        let halfv = peak / 2.0;
        let mut reach = [hw as f32; 2];
        for (side, dir) in [-1i64, 1].into_iter().enumerate() {
            let mut i = hw;
            while i + dir >= 0 && i + dir <= 2 * hw {
                let (a, b) = (depth[i as usize], depth[(i + dir) as usize]);
                if b < halfv {
                    reach[side] = (i - hw).abs() as f32 + (a - halfv) / (a - b).max(1e-6);
                    break;
                }
                i += dir;
            }
        }
        fwhm = reach[0] + reach[1];
    }
    let blur_w = 2.355f32 * 1.0;
    let thickness = (fwhm * fwhm - blur_w * blur_w).max(1.0).sqrt().clamp(1.0, width);

    // Back to image coordinates, one line per clear stretch.
    let at = |s: usize| -> [f32; 2] {
        let off = path[s] as f32 - half as f32;
        [c[s][0] + off * normals[s][0], c[s][1] + off * normals[s][1]]
    };
    let mut tmp = Labels::new(w, h, 1);
    let r1 = |v: f32| (v * 10.0).round() / 10.0;
    let mut marks = vec![];
    for &(a, b) in &keep_runs {
        let pts: Vec<[f32; 2]> = (a..=b).map(at).collect();
        tmp.stroke(0, &pts, (thickness / 2.0).max(0.75), 1, 0);
        marks.push(Mark {
            z: slice_z,
            path: pts.iter().step_by(3).chain(pts.last()).map(|p| [r1(p[0]), r1(p[1])]).collect(),
            thickness: r1(thickness),
            inside: 0,
            closed,
        });
    }
    let Some(lines) = tmp.slice(0) else { return Ok(Outcome::Rough("没找到膜".into())) };
    let mut mask: Vec<bool> = lines.iter().map(|&v| v == 1).collect();

    // The paint of the stretches without a clear line stays as painted.
    let refined = |s: usize| keep_runs.iter().any(|&(a, b)| s >= a && s <= b);
    let mut kept = 0usize;
    if (0..s_len).any(|s| !refined(s)) {
        {
            let pm = paint;
            let (x0, y0) = (b[0].max(0.0) as usize, b[1].max(0.0) as usize);
            let (x1, y1) = ((b[2].ceil().max(0.0) as usize).min(w), (b[3].ceil().max(0.0) as usize).min(h));
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = y * w + x;
                    if !pm[i] {
                        continue;
                    }
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let near = (0..s_len).min_by(|&p, &q| {
                        let d = |s: usize| (c[s][0] - px).powi(2) + (c[s][1] - py).powi(2);
                        d(p).total_cmp(&d(q))
                    });
                    if near.is_some_and(|s| !refined(s)) {
                        mask[i] = true;
                        kept += 1;
                    }
                }
            }
        }
    }
    let note = if kept > 0 { "有一段附近没找到清楚的膜，那一段保留了你画的样子".to_string() } else { String::new() };
    Ok(Outcome::Done(mask, marks, note))
}

// ---------- shared by the round and free types ----------

/// The paint's pixels: centroid, equivalent radius (of a disc of its area),
/// farthest pixel from the centroid, and bounding box.
struct Blob {
    c: [f32; 2],
    r_eq: f32,
    r_max: f32,
    bbox: [f32; 4],
    n: usize,
}

fn blob(paint: &[bool], w: usize) -> Option<Blob> {
    let (mut sx, mut sy, mut n) = (0f64, 0f64, 0usize);
    let mut bbox = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for (i, _) in paint.iter().enumerate().filter(|(_, &v)| v) {
        let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
        sx += x as f64;
        sy += y as f64;
        n += 1;
        bbox = [bbox[0].min(x), bbox[1].min(y), bbox[2].max(x), bbox[3].max(y)];
    }
    if n < 4 {
        return None;
    }
    let c = [(sx / n as f64) as f32, (sy / n as f64) as f32];
    let r_max = paint.iter().enumerate().filter(|(_, &v)| v).map(|(i, _)| (((i % w) as f32 + 0.5 - c[0]).powi(2) + ((i / w) as f32 + 0.5 - c[1]).powi(2)).sqrt()).fold(0f32, f32::max);
    Some(Blob { c, r_eq: (n as f32 / std::f32::consts::PI).sqrt(), r_max, bbox, n })
}

/// A filled disc as pixels of the slice.
fn disc(w: usize, h: usize, c: [f32; 2], r: f32) -> Vec<bool> {
    let mut m = vec![false; w * h];
    let (x0, y0) = ((c[0] - r).floor().max(0.0) as usize, (c[1] - r).floor().max(0.0) as usize);
    let (x1, y1) = (((c[0] + r).ceil().max(0.0) as usize).min(w), ((c[1] + r).ceil().max(0.0) as usize).min(h));
    for y in y0..y1 {
        for x in x0..x1 {
            if (x as f32 + 0.5 - c[0]).powi(2) + (y as f32 + 0.5 - c[1]).powi(2) <= r * r {
                m[y * w + x] = true;
            }
        }
    }
    m
}

/// A circle as a closed polyline (first point repeated at the end).
fn circle_pts(c: [f32; 2], r: f32) -> Vec<[f32; 2]> {
    let n = ((2.0 * std::f32::consts::PI * r).ceil() as usize).max(16);
    (0..=n).map(|k| {
        let a = k as f32 / n as f32 * 2.0 * std::f32::consts::PI;
        [c[0] + r * a.cos(), c[1] + r * a.sin()]
    }).collect()
}

/// Least-squares circle through `pts` (Kåsa), refitted twice without the
/// points farther than 2.5 robust sd from it. Returns centre, radius and the
/// share of points kept.
fn fit_circle(pts: &[[f32; 2]]) -> Option<([f32; 2], f32, f32)> {
    let mut use_: Vec<bool> = vec![true; pts.len()];
    let mut out = None;
    for _ in 0..3 {
        let sel: Vec<[f64; 2]> = pts.iter().zip(&use_).filter(|(_, &u)| u).map(|(p, _)| [p[0] as f64, p[1] as f64]).collect();
        if sel.len() < 5 {
            return out;
        }
        // Solve [x y 1]·[a b c] = x²+y² in least squares.
        let (mut m, mut v) = ([[0f64; 3]; 3], [0f64; 3]);
        for p in &sel {
            let row = [p[0], p[1], 1.0];
            let rhs = p[0] * p[0] + p[1] * p[1];
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] += row[i] * row[j];
                }
                v[i] += row[i] * rhs;
            }
        }
        let det = |m: &[[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        let d = det(&m);
        if d.abs() < 1e-9 {
            return out;
        }
        let solve = |k: usize| {
            let mut mk = m;
            for i in 0..3 {
                mk[i][k] = v[i];
            }
            det(&mk) / d
        };
        let (a, b, c) = (solve(0), solve(1), solve(2));
        let (cx, cy) = (a / 2.0, b / 2.0);
        let r = (c + cx * cx + cy * cy).max(0.0).sqrt();
        let res: Vec<f32> = pts.iter().map(|p| (((p[0] as f64 - cx).powi(2) + (p[1] as f64 - cy).powi(2)).sqrt() - r).abs() as f32).collect();
        let sd = 1.4826 * median(res.iter().zip(&use_).filter(|(_, &u)| u).map(|(v, _)| *v).collect()) + 0.5;
        use_ = res.iter().map(|&e| e <= 2.5 * sd).collect();
        let kept = use_.iter().filter(|&&u| u).count() as f32 / pts.len() as f32;
        out = Some(([cx as f32, cy as f32], r as f32, kept));
    }
    out
}

// ---------- 小颗粒 ----------

/// Small particles in the paint: maxima of the scale-normalised Laplacian of
/// Gaussian inside the paint (dark or bright, whichever answers more
/// strongly), each outlined by the sharpest flood region around it (a disc
/// of the detected scale when the flood finds nothing that fits).
fn particles(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool]) -> Result<Outcome> {
    let (w, h, _) = dims;
    let Some(bl) = blob(paint, w) else { return Ok(Outcome::Rough(String::new())) };
    let reach = bl.r_max.max(4.0);
    let pad = reach + 8.0;
    let roi = Roi::read(vals, dims, z, [bl.bbox[0] - pad, bl.bbox[1] - pad, bl.bbox[2] + pad, bl.bbox[3] + pad])?;
    let (rw, rh) = (roi.w, roi.h);
    let inside = |x: usize, y: usize| {
        let (ix, iy) = (roi.x0 + x, roi.y0 + y);
        ix < w && iy < h && paint[iy * w + ix]
    };
    // Scale-normalised LoG at radii up to the paint's size; a particle is a
    // maximum over position and scale (noise specks peak at the finest scale
    // only, inside a particle the particle's own scale wins).
    let sigmas: Vec<f32> = [1.0f32, 1.4, 2.0, 2.8, 4.0, 5.6, 8.0, 11.0].into_iter().filter(|s| s * std::f32::consts::SQRT_2 <= bl.r_max.max(3.0)).collect();
    let laps: Vec<Vec<f32>> = sigmas
        .iter()
        .map(|&sigma| {
            let g = blur(&roi.v, rw, rh, sigma);
            let mut lap = vec![0f32; rw * rh];
            for y in 1..rh - 1 {
                for x in 1..rw - 1 {
                    let i = y * rw + x;
                    lap[i] = sigma * sigma * (g[i - 1] + g[i + 1] + g[i - rw] + g[i + rw] - 4.0 * g[i]);
                }
            }
            lap
        })
        .collect();
    let mut best: Vec<(f32, usize, usize, f32, f32)> = vec![]; // (response, x, y, sigma, sign)
    let mut all = vec![];
    for (k, lap) in laps.iter().enumerate() {
        all.extend(lap.iter().step_by(7).map(|v| v.abs()));
        for sign in [1.0f32, -1.0] {
            for y in 2..rh - 2 {
                for x in 2..rw - 2 {
                    let i = y * rw + x;
                    let v = sign * lap[i];
                    if v <= 0.0 || !inside(x, y) {
                        continue;
                    }
                    let near = [i - 1, i + 1, i - rw, i + rw, i - rw - 1, i - rw + 1, i + rw - 1, i + rw + 1];
                    let space = near.iter().all(|&j| sign * lap[j] < v);
                    let scale = (k == 0 || sign * laps[k - 1][i] < v) && (k + 1 == laps.len() || sign * laps[k + 1][i] < v);
                    if space && scale && k > 0 {
                        best.push((v, x, y, sigmas[k], sign));
                    }
                }
            }
        }
    }
    if best.is_empty() {
        return Ok(Outcome::Rough("在你涂的地方没找到颗粒，先保留你画的样子".into()));
    }
    // Significance against the spread of the filter responses in the window.
    let (_, sd) = crate::segpick::median_sd(&all);
    best.sort_by(|a, b| b.0.total_cmp(&a.0));
    // One polarity: the one of the strongest particle.
    let sign = best[0].4;
    let mut picked: Vec<(f32, usize, usize, f32)> = vec![];
    // Particles answer about as strongly as the strongest one; noise
    // specks answer far less (and only at the smallest scales).
    let floor = (3.0 * sd).max(0.6 * best[0].0);
    for &(v, x, y, sigma, sg) in &best {
        if sg != sign || v < floor {
            continue;
        }
        let r = sigma * std::f32::consts::SQRT_2;
        if picked.iter().any(|&(_, px, py, ps)| ((px as f32 - x as f32).powi(2) + (py as f32 - y as f32).powi(2)).sqrt() < (r + ps * std::f32::consts::SQRT_2) * 0.8) {
            continue;
        }
        picked.push((v, x, y, sigma));
    }
    if picked.is_empty() {
        return Ok(Outcome::Rough("在你涂的地方没找到清楚的颗粒，先保留你画的样子".into()));
    }
    let noise = crate::segpick::noise_sigma(&roi.v, rw, rh);
    let mut mask = vec![false; w * h];
    let mut marks = vec![];
    for &(_, x, y, sigma) in &picked {
        let r = sigma * std::f32::consts::SQRT_2;
        let want = std::f32::consts::PI * r * r;
        let regions = crate::segpick::flood(&roi.v, rw, rh, (x, y), noise);
        let fit = regions.iter().filter(|g| (g.bright == (sign < 0.0))).filter(|g| {
            let a = g.pix.len() as f32;
            a >= 0.4 * want && a <= 2.5 * want
        }).max_by(|a, b| a.edge.total_cmp(&b.edge));
        let (cx, cy) = (roi.x0 as f32 + x as f32 + 0.5, roi.y0 as f32 + y as f32 + 0.5);
        let mut diameter = 2.0 * r;
        match fit {
            Some(g) => {
                for &j in &g.pix {
                    mask[(roi.y0 + j / rw) * w + roi.x0 + j % rw] = true;
                }
                diameter = 2.0 * (g.pix.len() as f32 / std::f32::consts::PI).sqrt();
            }
            None => {
                for (o, v) in mask.iter_mut().zip(disc(w, h, [cx, cy], r)) {
                    *o |= v;
                }
            }
        }
        marks.push(Mark { z, path: vec![[(cx * 10.0).round() / 10.0, (cy * 10.0).round() / 10.0]], thickness: (diameter * 10.0).round() / 10.0, inside: 0, closed: false });
    }
    let _ = bl.n;
    let note = if picked.len() > 1 { format!("找到 {} 个颗粒", picked.len()) } else { String::new() };
    Ok(Outcome::Done(mask, marks, note))
}

// ---------- 球 / 空心球 ----------

/// A ball (solid: its outline is an edge) or a shell (hollow: its outline is
/// a thin line). Rays from the paint's centre find the outline on each side;
/// a robust circle fit through those points gives the round object. A ball
/// is the disc; a shell is refined further with the line engine along the
/// fitted circle, so the drawn ring follows the real membrane.
fn round(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool], gap: f32, shell: bool) -> Result<Outcome> {
    let (w, h, _) = dims;
    let Some(bl) = blob(paint, w) else { return Ok(Outcome::Rough(String::new())) };
    let r_out = bl.r_max + gap.max(4.0);
    let pad = r_out + 6.0;
    let roi = Roi::read(vals, dims, z, [bl.c[0] - pad, bl.c[1] - pad, bl.c[0] + pad, bl.c[1] + pad])?;
    let g = blur(&roi.v, roi.w, roi.h, 1.0);
    let at = |x: f32, y: f32| {
        let (lx, ly) = (x - roi.x0 as f32 - 0.5, y - roi.y0 as f32 - 0.5);
        if lx < 0.0 || ly < 0.0 || lx >= (roi.w - 1) as f32 || ly >= (roi.h - 1) as f32 {
            return f32::NAN;
        }
        let (ix, iy) = (lx as usize, ly as usize);
        let (fx, fy) = (lx - ix as f32, ly - iy as f32);
        let q = |a: usize, b: usize| g[b * roi.w + a];
        (q(ix, iy) * (1.0 - fx) + q(ix + 1, iy) * fx) * (1.0 - fy) + (q(ix, iy + 1) * (1.0 - fx) + q(ix + 1, iy + 1) * fx) * fy
    };
    let r_min = (0.25 * bl.r_eq).max(2.0);
    let n_rays = 64;
    // Profiles along each ray, smoothed a little along the ray.
    let steps = r_out.ceil() as usize + 1;
    let k = gauss_kernel(1.0);
    let profiles: Vec<Vec<f32>> = (0..n_rays)
        .map(|k_| {
            let a = k_ as f32 / n_rays as f32 * 2.0 * std::f32::consts::PI;
            let p: Vec<f32> = (0..steps).map(|t| at(bl.c[0] + t as f32 * a.cos(), bl.c[1] + t as f32 * a.sin())).map(|v| if v.is_finite() { v } else { 0.0 }).collect();
            smooth1(&p, &k)
        })
        .collect();
    // Per ray, the best outline candidate for each polarity; the polarity
    // that answers more strongly over all rays wins.
    let score = |p: &[f32], t: usize, sign: f32| -> f32 {
        if shell {
            // A line: darker (or brighter) than both sides.
            if t < 2 || t + 2 >= p.len() {
                return f32::MIN;
            }
            sign * (p[t - 2] + p[t + 2] - 2.0 * p[t])
        } else {
            // An edge: inside differs from outside.
            if t < 2 || t + 2 >= p.len() {
                return f32::MIN;
            }
            sign * ((p[t - 2] + p[t - 1]) - (p[t + 1] + p[t + 2]))
        }
    };
    // The outline is looked for where the paint is (a painted ring limits
    // it to the ring; a filled disc allows the whole disc), plus the gap.
    let near = crate::labels::edt(paint, w, h);
    let allowed = |a: f32, t: usize| {
        let (x, y) = (bl.c[0] + t as f32 * a.cos(), bl.c[1] + t as f32 * a.sin());
        x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h && near[y as usize * w + x as usize] <= gap.max(4.0)
    };
    let mut best: Option<(f32, Vec<[f32; 2]>)> = None;
    for sign in [1.0f32, -1.0] {
        let mut total = 0f32;
        let mut pts = vec![];
        for (k_, p) in profiles.iter().enumerate() {
            let a = k_ as f32 / n_rays as f32 * 2.0 * std::f32::consts::PI;
            let (t, v) = (r_min.ceil() as usize..p.len()).filter(|&t| allowed(a, t)).map(|t| (t, score(p, t, sign))).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((0, f32::MIN));
            if v > 0.0 {
                total += v;
                pts.push([bl.c[0] + t as f32 * a.cos(), bl.c[1] + t as f32 * a.sin()]);
            }
        }
        if best.as_ref().map_or(true, |b| total > b.0) {
            best = Some((total, pts));
        }
    }
    let Some((_, pts)) = best else { return Ok(Outcome::Rough(String::new())) };
    let what = if shell { "空心球" } else { "球" };
    let Some((c, r, kept)) = fit_circle(&pts) else {
        return Ok(Outcome::Rough(format!("没找到清楚的{what}边界，先保留你画的样子")));
    };
    // The fit must be carried by most rays and stay near the paint.
    let off = ((c[0] - bl.c[0]).powi(2) + (c[1] - bl.c[1]).powi(2)).sqrt();
    if kept < 0.5 || r < r_min || r > r_out + 2.0 || off > r.max(4.0) {
        return Ok(Outcome::Rough(format!("没找到清楚的{what}边界，先保留你画的样子")));
    }
    let r1 = |v: f32| (v * 10.0).round() / 10.0;
    if !shell {
        let mark = Mark { z, path: vec![[r1(c[0]), r1(c[1])]], thickness: r1(2.0 * r), inside: 0, closed: false };
        return Ok(Outcome::Done(disc(w, h, c, r), vec![mark], String::new()));
    }
    // Shell: the line engine along the circle, the whole paint as the region
    // whose unclear stretches stay rough. The paint half width is the ring's
    // uncertainty: a quarter of the radius, at least the brush gap.
    let ring = circle_pts(c, r);
    // The fitted circle is close already: search only a little around it.
    let hw = (0.12 * r).clamp(3.0, 8.0);
    let ring_paint = {
        let mut m = disc(w, h, c, r + hw);
        let hole = disc(w, h, c, (r - hw).max(0.0));
        m.iter_mut().zip(hole).for_each(|(a, b)| *a &= !b);
        m
    };
    // The paint inside the ring is replaced too (a filled disc painted for a vesicle).
    // Unclear stretches keep only what was painted there; the paint inside
    // the ring (a filled disc painted for a vesicle) is replaced.
    let keep: Vec<bool> = ring_paint.iter().zip(paint).map(|(a, b)| *a && *b).collect();
    let mut out = band_line(vals, dims, z, &ring, hw, &keep, None, true)?;
    if let Outcome::Done(mask, marks, _) = &mut out {
        // A stretch that left the circle followed something else.
        marks.retain(|m| {
            let dev = median(m.path.iter().map(|p| (((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt() - r).abs()).collect());
            dev <= (0.12 * r).max(3.0)
        });
        if marks.is_empty() {
            return Ok(Outcome::Rough("没找到清楚的空心球的膜，先保留你画的样子".into()));
        }
        let mut lines = Labels::new(w, h, 1);
        for m in marks.iter() {
            lines.stroke(0, &m.path, (m.thickness / 2.0).max(0.75), 1, 0);
        }
        if let Some(l) = lines.slice(0) {
            mask.iter_mut().zip(l).for_each(|(a, &b)| *a = b == 1);
        }
        for m in marks.iter_mut() {
            // Inside of a ring is its centre.
            if m.path.len() >= 3 {
                let k = m.path.len() / 2;
                let (q0, q1) = (m.path[k - 1], m.path[k + 1]);
                let (tx, ty) = (q1[0] - q0[0], q1[1] - q0[1]);
                let (vx, vy) = (c[0] - m.path[k][0], c[1] - m.path[k][1]);
                m.inside = if tx * vy - ty * vx > 0.0 { 1 } else { -1 };
            }
        }
    }
    Ok(out)
}

// ---------- 双层膜 ----------

/// Two parallel lines: the strongest line first, then the strongest other
/// line in the same paint that keeps clear of the first. When the second is
/// not clear the single line is kept, with a note.
fn membrane2(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool], gap: f32) -> Result<Outcome> {
    let (w, h, _) = dims;
    let Some((centre, hw)) = centre_line(paint, w, h, gap) else { return Ok(Outcome::Rough(String::new())) };
    let first = band_line(vals, dims, z, &centre, hw, paint, None, false)?;
    let Outcome::Done(m1, marks1, note1) = first else { return Ok(first) };
    // Keep the second line off the first: its pixels widened by its thickness.
    let t1 = marks1.iter().map(|m| m.thickness).fold(1f32, f32::max);
    let near = crate::labels::edt(&m1, w, h);
    let avoid: Vec<bool> = near.iter().map(|&d| d <= t1 + 1.0).collect();
    let second = band_line(vals, dims, z, &centre, hw, paint, Some(&avoid), false)?;
    if let Outcome::Done(m2, marks2, _) = second {
        // The two must run side by side: mean distance between them about
        // a few line widths, not crossing.
        let d1 = crate::labels::edt(&m1, w, h);
        let ds: Vec<f32> = m2.iter().enumerate().filter(|(_, &v)| v).map(|(i, _)| d1[i]).collect();
        let md = median(ds);
        if md > t1 && md < 2.0 * hw + 4.0 {
            let mask: Vec<bool> = m1.iter().zip(&m2).map(|(a, b)| *a || *b).collect();
            let mut marks = marks1;
            marks.extend(marks2);
            return Ok(Outcome::Done(mask, marks, note1));
        }
    }
    Ok(Outcome::Done(m1, marks1, "只找到一层清楚的膜，先按单层画".into()))
}

// ---------- 不规则 ----------

/// An object of any shape: the flood region around the paint whose outline
/// is sharpest and which overlaps the paint best. The paint stays when no
/// region matches it well.
fn free(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool]) -> Result<Outcome> {
    let (w, h, _) = dims;
    let Some(bl) = blob(paint, w) else { return Ok(Outcome::Rough(String::new())) };
    let pad = bl.r_max + 12.0;
    let roi = Roi::read(vals, dims, z, [bl.bbox[0] - pad, bl.bbox[1] - pad, bl.bbox[2] + pad, bl.bbox[3] + pad])?;
    let (rw, rh) = (roi.w, roi.h);
    let noise = crate::segpick::noise_sigma(&roi.v, rw, rh);
    let local: Vec<bool> = (0..rw * rh).map(|j| {
        let (x, y) = (roi.x0 + j % rw, roi.y0 + j / rw);
        x < w && y < h && paint[y * w + x]
    }).collect();
    // Seeds: a few points inside the paint (its centre and around it).
    let seeds: Vec<(usize, usize)> = local.iter().enumerate().filter(|(_, &v)| v).map(|(j, _)| (j % rw, j / rw)).step_by((bl.n / 24).max(1)).collect();
    let mut best: Option<(f32, Vec<usize>)> = None;
    for &sd in &seeds {
        for g in crate::segpick::flood(&roi.v, rw, rh, sd, noise) {
            let inter = g.pix.iter().filter(|&&j| local[j]).count() as f32;
            let iou = inter / (g.pix.len() as f32 + bl.n as f32 - inter);
            let score = iou * g.edge.min(8.0);
            if iou >= 0.4 && best.as_ref().map_or(true, |b| score > b.0) {
                best = Some((score, g.pix));
            }
        }
    }
    let Some((_, pix)) = best else { return Ok(Outcome::Rough("没找到和你涂的范围吻合的清楚边界，先保留你画的样子".into())) };
    let mut mask = vec![false; w * h];
    for &j in &pix {
        mask[(roi.y0 + j / rw) * w + roi.x0 + j % rw] = true;
    }
    Ok(Outcome::Done(mask, vec![], String::new()))
}
