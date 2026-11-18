//! Filled structures (小颗粒, 球, 不规则) and the ring of a 空心球: the
//! painted region is the object's first guess, and refining snaps its
//! outline to the image. One engine for all of them: rays from the paint's
//! centre unwrap a band around the paint's own outline (inside and outside
//! of it by a share of its size); along each ray the boundary answers as an
//! edge (inside differs from outside) or a ridge (a thin line); dynamic
//! programming finds the closed contour with the strongest boundary that
//! moves smoothly from ray to ray. The type adds only a shape prior: 球 and
//! 小颗粒 prefer a circle, 不规则 only smoothness, 空心球 a thin line (refined
//! further by the line engine). A contour that is weak, or far from the
//! paint's size, gives way to the paint with its outline smoothed: a result
//! never collapses to a fragment.

use super::line::band_line;
use super::*;
use std::f32::consts::{PI, SQRT_2};


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


/// Pixels inside a closed polygon (even-odd rule, pixel centres).
fn fill_polygon(w: usize, h: usize, pts: &[[f32; 2]]) -> Vec<bool> {
    let mut m = vec![false; w * h];
    if pts.len() < 3 {
        return m;
    }
    let (y0, y1) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    let (ya, yb) = (y0.floor().max(0.0) as usize, (y1.ceil().max(0.0) as usize).min(h));
    let mut xs = vec![];
    for y in ya..yb {
        let cy = y as f32 + 0.5;
        xs.clear();
        for k in 0..pts.len() {
            let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
            if (a[1] <= cy) != (b[1] <= cy) {
                xs.push(a[0] + (cy - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
            }
        }
        xs.sort_by(f32::total_cmp);
        for pair in xs.chunks(2) {
            if pair.len() < 2 {
                break;
            }
            let (xa, xb) = ((pair[0] - 0.5).ceil().max(0.0) as usize, ((pair[1] - 0.5).floor() + 1.0).max(0.0) as usize);
            for x in xa..xb.min(w) {
                m[y * w + x] = true;
            }
        }
    }
    m
}

/// The paint with its outline smoothed: what stays when no outline is clear.
fn smoothed(paint: &[bool], w: usize, h: usize, bl: &Blob) -> Vec<bool> {
    let pad = 4.0;
    let (x0, y0) = ((bl.bbox[0] - pad).max(0.0) as usize, (bl.bbox[1] - pad).max(0.0) as usize);
    let (x1, y1) = (((bl.bbox[2] + pad) as usize).min(w), ((bl.bbox[3] + pad) as usize).min(h));
    let (rw, rh) = (x1 - x0, y1 - y0);
    let local: Vec<f32> = (0..rw * rh).map(|j| if paint[(y0 + j / rw) * w + x0 + j % rw] { 1.0 } else { 0.0 }).collect();
    let sm = blur(&local, rw, rh, 1.5);
    let mut m = vec![false; w * h];
    for (j, v) in sm.iter().enumerate() {
        if *v >= 0.5 {
            m[(y0 + j / rw) * w + x0 + j % rw] = true;
        }
    }
    m
}

#[derive(Clone, Copy, PartialEq)]
enum Boundary {
    /// An edge or a line, whichever answers (filled objects).
    Any,
    /// A thin line only (the membrane of a shell).
    Line,
}

/// A snapped closed contour: its points, its centre (from the contour), and
/// its strength (median boundary response on it, in robust sd of the band).
struct Contour {
    pts: Vec<[f32; 2]>,
    strength: f32,
}

/// Snaps the outline of the painted region `paint` (one object, about
/// star-shaped around its centroid) to the image. `smooth`: how strongly the
/// contour is held to move smoothly from ray to ray.
fn snap(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool], bl: &Blob, kind: Boundary, smooth: f32) -> Result<Option<Contour>> {
    let (w, h, _) = dims;
    let c = bl.c;
    // The paint's own outline along each ray, smoothed around the circle.
    let n_rays = ((2.0 * PI * bl.r_max).ceil() as usize).clamp(48, 360);
    let dir = |k: usize| {
        let a = k as f32 / n_rays as f32 * 2.0 * PI;
        [a.cos(), a.sin()]
    };
    let painted = |x: f32, y: f32| x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h && paint[y as usize * w + x as usize];
    let raw: Vec<f32> = (0..n_rays)
        .map(|k| {
            let d = dir(k);
            let mut last = 0.0;
            let mut t = 0.0;
            while t <= bl.r_max + 2.0 {
                if painted(c[0] + t * d[0], c[1] + t * d[1]) {
                    last = t;
                }
                t += 0.5;
            }
            last
        })
        .collect();
    let k = gauss_kernel((n_rays as f32 / 48.0).max(1.0));
    let tripled: Vec<f32> = raw.iter().chain(&raw).chain(&raw).copied().collect();
    let rp: Vec<f32> = smooth1(&tripled, &k)[n_rays..2 * n_rays].to_vec();
    // The band: inside and outside of the paint's outline.
    let band = (0.3 * bl.r_eq).max(6.0);
    let half = band.ceil() as usize;
    let t_len = 2 * half + 1;
    let reach = bl.r_max + band + 4.0;
    let roi = Roi::read(vals, dims, z, [c[0] - reach, c[1] - reach, c[0] + reach, c[1] + reach])?;
    let mut prof = vec![0f32; n_rays * t_len];
    for k in 0..n_rays {
        let d = dir(k);
        for t in 0..t_len {
            let r = (rp[k] + t as f32 - half as f32).max(0.5);
            let v = roi.at(c[0] + r * d[0], c[1] + r * d[1]);
            prof[k * t_len + t] = if v.is_finite() { v } else { 0.0 };
        }
    }
    // Smooth a little across rays (the boundary is continuous), then along.
    let mut sm = prof.clone();
    let ka = gauss_kernel(1.0);
    for t in 0..t_len {
        let col: Vec<f32> = (0..3 * n_rays).map(|k| prof[(k % n_rays) * t_len + t]).collect();
        for (k, v) in smooth1(&col, &ka)[n_rays..2 * n_rays].iter().enumerate() {
            sm[k * t_len + t] = *v;
        }
    }
    // Polarity: is the inside darker or brighter than the outside?
    let (mut din, mut dout) = (0f32, 0f32);
    for k in 0..n_rays {
        din += sm[k * t_len + 1];
        dout += sm[k * t_len + t_len - 2];
    }
    let dark_inside = din < dout;
    // Responses: an edge (step from inside to outside) and a line (ridge),
    // each in robust sd of the band, signed for this object.
    let mut edge = vec![0f32; n_rays * t_len];
    let mut ridge = vec![0f32; n_rays * t_len];
    for k in 0..n_rays {
        let row = &sm[k * t_len..(k + 1) * t_len];
        for t in 2..t_len - 2 {
            let step = (row[t + 1] + row[t + 2]) - (row[t - 1] + row[t - 2]);
            edge[k * t_len + t] = if dark_inside { step } else { -step };
            // A dark line in the usual cryo-ET contrast; bright if the object is bright.
            let curv = row[t - 2] + row[t + 2] - 2.0 * row[t];
            ridge[k * t_len + t] = if dark_inside { curv } else { -curv };
        }
    }
    let norm = |v: &mut Vec<f32>| {
        let inner: Vec<f32> = (0..n_rays).flat_map(|k| v[k * t_len + 2..(k + 1) * t_len - 2].to_vec()).collect();
        let med = median(inner.clone());
        let sd = 1.4826 * median(inner.iter().map(|x| (x - med).abs()).collect()).max(1e-6);
        v.iter_mut().for_each(|x| *x = (*x - med) / sd);
    };
    norm(&mut edge);
    norm(&mut ridge);
    let score: Vec<f32> = (0..n_rays * t_len)
        .map(|i| {
            let t = i % t_len;
            if t < 2 || t + 2 >= t_len {
                return -10.0;
            }
            // Leaving the paint's outline costs a little: the paint is the guess.
            let away = (t as f32 - half as f32).abs() / half as f32;
            let v = match kind {
                Boundary::Any => edge[i].max(ridge[i]),
                Boundary::Line => ridge[i],
            };
            v - 0.25 * away
        })
        .collect();
    // Closed contour: the rays three times round, the middle round kept.
    let tripled: Vec<f32> = score.iter().chain(&score).chain(&score).copied().collect();
    let path = best_path(&tripled, 3 * n_rays, t_len, smooth);
    let path: Vec<usize> = path[n_rays..2 * n_rays].to_vec();
    let on: Vec<f32> = (0..n_rays).map(|k| score[k * t_len + path[k]]).collect();
    let strength = median(on);
    let pts = (0..n_rays)
        .map(|k| {
            let d = dir(k);
            let r = (rp[k] + path[k] as f32 - half as f32).max(0.5);
            [c[0] + r * d[0], c[1] + r * d[1]]
        })
        .collect();
    Ok(Some(Contour { pts, strength }))
}

/// Area of a closed polygon.
fn area(pts: &[[f32; 2]]) -> f32 {
    (0..pts.len()).map(|k| {
        let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
        a[0] * b[1] - b[0] * a[1]
    }).sum::<f32>().abs() / 2.0
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

/// One filled object from its paint: snap, accept or fall back.
/// `circle`: prefer a circle through the contour (球, 小颗粒).
fn filled(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool], circle: bool) -> Result<(Vec<bool>, Option<Mark>, bool)> {
    let (w, h, _) = dims;
    let Some(bl) = blob(paint, w) else { return Ok((paint.to_vec(), None, false)) };
    let smooth = if circle { 2.0 } else { 0.8 };
    let fallback = || (smoothed(paint, w, h, &bl), None, false);
    let Some(ct) = snap(vals, dims, z, paint, &bl, Boundary::Any, smooth)? else { return Ok(fallback()) };
    let a = area(&ct.pts);
    if std::env::var_os("TMV_STRUCT_DEBUG").is_some() {
        eprintln!("filled c=({:.0},{:.0}) n={} strength={:.2} area={:.0} circle={:?}", bl.c[0], bl.c[1], bl.n, ct.strength, a, fit_circle(&ct.pts).map(|(c, r, k)| (c[0] as i32, c[1] as i32, r as i32, (k * 100.0) as i32)));
    }
    // A small dab in the middle of an object may grow to it (by the band);
    // shrinking far below the paint means the contour lost the object.
    if ct.strength < 0.5 || a < 0.4 * bl.n as f32 {
        return Ok(fallback());
    }
    if circle {
        if let Some((c, r, kept)) = fit_circle(&ct.pts) {
            if kept >= 0.6 && (PI * r * r) >= 0.4 * bl.n as f32 && (PI * r * r) <= 2.0 * a {
                let mark = Mark { z, path: vec![[round1(c[0]), round1(c[1])]], thickness: round1(2.0 * r), inside: 0, closed: false };
                return Ok((disc(w, h, c, r), Some(mark), true));
            }
        }
    }
    let mask = fill_polygon(w, h, &ct.pts);
    let ac = area(&ct.pts);
    let (cx, cy) = (ct.pts.iter().map(|p| p[0]).sum::<f32>() / ct.pts.len() as f32, ct.pts.iter().map(|p| p[1]).sum::<f32>() / ct.pts.len() as f32);
    let mark = Mark { z, path: vec![[round1(cx), round1(cy)]], thickness: round1(2.0 * (ac / PI).sqrt()), inside: 0, closed: false };
    Ok((mask, Some(mark), true))
}

// ---------- 球 ----------

pub(super) fn ball(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool]) -> Result<Outcome> {
    let (mask, mark, ok) = filled(vals, dims, z, paint, true)?;
    let note = if ok { String::new() } else { "没找到清楚的边界，保留了你画的样子（边缘修圆）".into() };
    Ok(Outcome::Done(mask, mark.into_iter().collect(), note))
}

// ---------- 不规则 ----------

pub(super) fn free(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool]) -> Result<Outcome> {
    let (mask, _, ok) = filled(vals, dims, z, paint, false)?;
    let note = if ok { String::new() } else { "没找到清楚的边界，保留了你画的样子（边缘修圆）".into() };
    Ok(Outcome::Done(mask, vec![], note))
}

// ---------- 小颗粒 ----------

/// Like 球 for each particle. A dab is one particle unless it clearly spans
/// several (much longer than wide): then it is cut at the maxima of the
/// Laplacian of Gaussian at the dab's own width, a disc of that width each.
pub(super) fn particles(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool]) -> Result<Outcome> {
    let (w, h, _) = dims;
    let Some(bl) = blob(paint, w) else { return Ok(Outcome::Rough(String::new())) };
    let pad = 4.0;
    let (x0, y0) = ((bl.bbox[0] - pad).max(0.0) as usize, (bl.bbox[1] - pad).max(0.0) as usize);
    let (x1, y1) = (((bl.bbox[2] + pad) as usize).min(w), ((bl.bbox[3] + pad) as usize).min(h));
    let (rw, rh) = (x1 - x0, y1 - y0);
    let local: Vec<bool> = (0..rw * rh).map(|j| paint[(y0 + j / rw) * w + x0 + j % rw]).collect();
    let depth = crate::labels::edt(&local.iter().map(|&v| !v).collect::<Vec<_>>(), rw, rh);
    let r_size = depth.iter().copied().fold(0f32, f32::max).max(2.0);
    let mut pieces: Vec<Vec<bool>> = vec![];
    if bl.r_max > 2.2 * r_size {
        // Seeds: LoG maxima at the dab's width, kept apart by a particle.
        let sigma = (r_size / SQRT_2).max(1.0);
        let roi = Roi::read(vals, dims, z, [x0 as f32, y0 as f32, x1 as f32, y1 as f32])?;
        let g = blur(&roi.v, roi.w, roi.h, sigma);
        let mut cand: Vec<(f32, usize, usize)> = vec![];
        let dark = {
            let (mut a, mut b, mut na, mut nb) = (0f32, 0f32, 0, 0);
            for (j, &p) in local.iter().enumerate() {
                if j < roi.v.len() {
                    if p { a += roi.v[j]; na += 1 } else { b += roi.v[j]; nb += 1 }
                }
            }
            a / na.max(1) as f32 <= b / nb.max(1) as f32
        };
        for y in 1..roi.h.saturating_sub(1) {
            for x in 1..roi.w.saturating_sub(1) {
                let i = y * roi.w + x;
                if !local.get(i).copied().unwrap_or(false) {
                    continue;
                }
                let lap = sigma * sigma * (g[i - 1] + g[i + 1] + g[i - roi.w] + g[i + roi.w] - 4.0 * g[i]);
                cand.push((if dark { lap } else { -lap }, x, y));
            }
        }
        cand.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut seeds: Vec<(usize, usize)> = vec![];
        let top = cand.first().map_or(0.0, |c| c.0);
        for &(v, x, y) in &cand {
            if v < 0.4 * top {
                break;
            }
            if seeds.iter().all(|&(sx, sy)| ((sx as f32 - x as f32).powi(2) + (sy as f32 - y as f32).powi(2)).sqrt() >= 1.6 * r_size) {
                seeds.push((x, y));
            }
        }
        for (x, y) in seeds {
            let cx = (x0 + x) as f32 + 0.5;
            let cy = (y0 + y) as f32 + 0.5;
            pieces.push(disc(w, h, [cx, cy], r_size));
        }
    }
    if pieces.is_empty() {
        pieces.push(paint.to_vec());
    }
    let mut mask = vec![false; w * h];
    let mut marks = vec![];
    let mut kept = 0;
    for piece in &pieces {
        let (m, mark, ok) = filled(vals, dims, z, piece, true)?;
        mask.iter_mut().zip(m).for_each(|(a, b)| *a |= b);
        marks.extend(mark);
        kept += usize::from(!ok);
    }
    let note = if kept == pieces.len() { "没找到清楚的颗粒边界，保留了你画的样子（边缘修圆）".to_string() } else { String::new() };
    Ok(Outcome::Done(mask, marks, note))
}

// ---------- 空心球 ----------

/// The ring of a vesicle: the contour snapped as a thin line, then the line
/// engine along it (thickness, exact position). Its inside is the centre.
pub(super) fn shell(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool]) -> Result<Outcome> {
    let (w, h, _) = dims;
    let Some(bl) = blob(paint, w) else { return Ok(Outcome::Rough(String::new())) };
    // A painted ring: fill its hole so the paint is the whole disc.
    let filled_paint = {
        let mut m = paint.to_vec();
        let outside = crate::labels::components_where(w, h, |i| !paint[i]);
        for part in outside {
            let touches = part.iter().any(|&i| i % w == 0 || i / w == 0 || i % w == w - 1 || i / w == h - 1);
            if !touches {
                part.iter().for_each(|&i| m[i] = true);
            }
        }
        m
    };
    let bl2 = blob(&filled_paint, w).unwrap_or(bl);
    let Some(ct) = snap(vals, dims, z, &filled_paint, &bl2, Boundary::Line, 1.5)? else { return Ok(Outcome::Rough(String::new())) };
    let a = area(&ct.pts);
    if ct.strength < 1.0 || a < 0.4 * bl2.n as f32 || a > 2.5 * bl2.n as f32 {
        return Ok(Outcome::Rough("没找到清楚的空心球的膜，先保留你画的样子".into()));
    }
    let mut ring: Vec<[f32; 2]> = ct.pts.clone();
    ring.push(ring[0]);
    let keep: Vec<bool> = {
        // Unclear stretches keep only paint near the ring.
        let mut near = Labels::new(w, h, 1);
        near.stroke(0, &ring, 4.0, 1, 0);
        let nm = near.slice(0).map(|s| s.to_vec()).unwrap_or_default();
        paint.iter().zip(nm.iter().chain(std::iter::repeat(&0))).map(|(p, n)| *p && *n == 1).collect()
    };
    let mut out = band_line(vals, dims, z, &ring, 3.0, &keep, None, true)?;
    let c = [ct.pts.iter().map(|p| p[0]).sum::<f32>() / ct.pts.len() as f32, ct.pts.iter().map(|p| p[1]).sum::<f32>() / ct.pts.len() as f32];
    if let Outcome::Done(_, marks, _) = &mut out {
        for m in marks.iter_mut() {
            if m.path.len() >= 3 {
                let k = m.path.len() / 2;
                let (q0, q1) = (m.path[k - 1], m.path[k + 1]);
                let (tx, ty) = (q1[0] - q0[0], q1[1] - q0[1]);
                let (vx, vy) = (c[0] - m.path[k][0], c[1] - m.path[k][1]);
                m.inside = if tx * vy - ty * vx > 0.0 { 1 } else { -1 };
                m.closed = true;
            }
        }
    }
    Ok(out)
}
