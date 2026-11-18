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

/// Refines `stroke` as `structure`.
pub fn refine(vals: &dyn Values, dims: (usize, usize, usize), stroke: &Stroke, structure: &str) -> Result<Outcome> {
    match structure {
        "membrane" => membrane(vals, dims, stroke),
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

fn membrane(vals: &dyn Values, dims: (usize, usize, usize), st: &Stroke) -> Result<Outcome> {
    if st.tool != "brush" || st.pts.len() < 2 {
        return Ok(Outcome::Rough("膜请用画笔沿着膜涂一笔，这一笔先保留你画的样子".into()));
    }
    let (w, h, _) = dims;
    let width = (2.0 * st.radius).max(3.0);
    let line = resample(&st.pts, 1.0);
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
    let roi = Roi::read(vals, dims, st.z, b)?;
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
    let Some((m1, sigma, sign, r1)) = best else { return Ok(Outcome::Rough("这一笔太细了".into())) };
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
    let found = runs(&|s| z[s] >= 2.0);
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
    let path = best_path(&with_cost(&r2, m2), s_len, t_len, 0.3 * m2.max(0.0));

    // Within the clear stretches, drop the ends where the template stops matching.
    let score: Vec<f32> = (0..s_len).map(|s| r2[s * t_len + path[s]]).collect();
    let cut = 0.3 * median((0..s_len).filter(|&s| on_line(s)).map(|s| score[s]).collect());
    let keep_runs = runs(&|s| on_line(s) && score[s] >= cut);
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
            z: st.z,
            path: pts.iter().step_by(3).chain(pts.last()).map(|p| [r1(p[0]), r1(p[1])]).collect(),
            thickness: r1(thickness),
            inside: 0,
            closed: false,
        });
    }
    let Some(lines) = tmp.slice(0) else { return Ok(Outcome::Rough("没找到膜".into())) };
    let mut mask: Vec<bool> = lines.iter().map(|&v| v == 1).collect();

    // The paint of the stretches without a clear line stays as painted.
    let refined = |s: usize| keep_runs.iter().any(|&(a, b)| s >= a && s <= b);
    let mut kept = 0usize;
    if (0..s_len).any(|s| !refined(s)) {
        let mut paint = Labels::new(w, h, 1);
        paint.stroke(0, &st.pts, st.radius, 1, 0);
        if let Some(pm) = paint.slice(0) {
            let (x0, y0) = (b[0].max(0.0) as usize, b[1].max(0.0) as usize);
            let (x1, y1) = ((b[2].ceil().max(0.0) as usize).min(w), (b[3].ceil().max(0.0) as usize).min(h));
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = y * w + x;
                    if pm[i] != 1 {
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
