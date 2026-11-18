//! Line-like structures: the band engine (straightened band along the
//! paint's centre line, ridge filter, dynamic programming) for 单层膜, 纤维
//! and 双层膜, and for the ring of a 空心球.

use super::*;


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

pub(super) fn membrane(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool], gap: f32) -> Result<Outcome> {
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
pub(super) fn band_line(vals: &dyn Values, dims: (usize, usize, usize), z: usize, centre: &[[f32; 2]], hw: f32, paint: &[bool], avoid: Option<&[bool]>, closed: bool) -> Result<Outcome> {
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

// ---------- 双层膜 ----------

/// Two parallel lines: the strongest line first, then the strongest other
/// line in the same paint that keeps clear of the first. When the second is
/// not clear the single line is kept, with a note.
pub(super) fn membrane2(vals: &dyn Values, dims: (usize, usize, usize), z: usize, paint: &[bool], gap: f32) -> Result<Outcome> {
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

