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

mod line;
mod region;

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
    let pieces = done + rough;
    // One line for the whole click: how many pieces, what was found, what stayed.
    let note = if pieces > 1 {
        let unit = match structure {
            "particle" => "个颗粒",
            "ball" => "个球",
            "shell" => "段囊泡膜",
            "membrane" | "membrane2" => "段膜",
            "fibre" => "段纤维",
            _ => "个对象",
        };
        let found = if structure == "free" { done } else { marks.len() };
        let mut t = format!("精修了 {pieces} 块：找到 {found} {unit}");
        if rough > 0 {
            t += &format!("，{rough} 块保留原样");
        }
        t
    } else {
        notes.first().cloned().unwrap_or_default()
    };
    if done == 0 {
        return Ok((None, note));
    }
    Ok((Some((add, marks)), note))
}

/// Refines the rough paint `paint` (pixels of slice `z`: one object, maybe
/// painted in several strokes) as `structure`. `gap`: pieces of paint this
/// close belong together (about a brush radius).
pub fn refine(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool], gap: f32, structure: &str) -> Result<Outcome> {
    match structure {
        // A fibre is an open line like a membrane: same band engine.
        "membrane" | "fibre" => line::membrane(vals, dims, z, paint, gap),
        "membrane2" => line::membrane2(vals, dims, z, paint, gap),
        "particle" => region::particles(vals, dims, z, paint),
        "ball" => region::ball(vals, dims, z, paint),
        "shell" => region::shell(vals, dims, z, paint),
        "free" => region::free(vals, dims, z, paint),
        "" => Ok(Outcome::Rough(String::new())),
        _ => Ok(Outcome::Rough("这个类型的精细分割还没做好，先保留你画的样子".into())),
    }
}

// ---------- small helpers ----------

pub(super) fn gauss_kernel(sigma: f32) -> Vec<f32> {
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
pub(super) fn smooth1(v: &[f32], k: &[f32]) -> Vec<f32> {
    let r = (k.len() / 2) as i64;
    let n = v.len() as i64;
    (0..n)
        .map(|i| k.iter().enumerate().map(|(j, kv)| kv * v[(i + j as i64 - r).clamp(0, n - 1) as usize]).sum())
        .collect()
}

/// Separable Gaussian blur of a `w`×`h` image.
pub(super) fn blur(v: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
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

pub(super) fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mid = v.len() / 2;
    v.select_nth_unstable_by(mid, f32::total_cmp);
    v[mid]
}

/// A polyline resampled at `step` pixels along its length.
pub(super) fn resample(pts: &[[f32; 2]], step: f32) -> Vec<[f32; 2]> {
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
pub(super) struct Roi {
    pub(super) x0: usize,
    pub(super) y0: usize,
    pub(super) w: usize,
    pub(super) h: usize,
    pub(super) v: Vec<f32>,
}

impl Roi {
    pub(super) fn read(vals: &dyn Values, (w, h, d): (usize, usize, usize), z: usize, b: [f32; 4]) -> Result<Roi> {
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
    pub(super) fn at(&self, x: f32, y: f32) -> f32 {
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
pub(super) fn best_path(r: &[f32], s_len: usize, t_len: usize, lambda: f32) -> Vec<usize> {
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

