//! Microtubules in a tomogram (Pro), found automatically the way Amira's
//! cylinder tracing is used in segmentation figures, but without a user
//! picking seeds: a microtubule is a dense tube about 25 nm across that runs
//! straight for hundreds of nanometres.
//!
//! The tomogram is brought to 2 nm voxels, density made positive (dark in
//! cryo-ET), and smoothed at the scale of a 12.5 nm rod. At every voxel the
//! Hessian's eigenvalues tell the local shape: a rod has two strongly
//! negative ones across it and one near zero along it, a membrane only one,
//! a ribosome-sized blob three. Voxels whose rod score is high (in robust
//! standard deviations of the tomogram's scores) and that are ridges across
//! their own axis are linked into lines (ridge voxels within 6 nm of each
//! other, so the parallel ridges a hollow tube leaves become one line), and
//! lines whose ends meet in line are joined (noise breaks a ridge). Where a
//! microtubule merges with its neighbours or breaks up depends on the
//! threshold and on the rest of the tomogram, so lines are made at several
//! thresholds and pooled, strictest first. A line counts as a microtubule
//! when it is long and straight enough and its centre is steadily less
//! dense than a ring 10 nm out (the hollow lumen), where the voxel size lets
//! the lumen show.

use rayon::prelude::*;
use serde::Serialize;

#[path = "filaments/cylinder.rs"]
mod cylinder;

/// Working voxel size, nm.
const WORK_NM: f32 = 2.0;
/// Microtubule outer radius, nm.
pub const RADIUS_NM: f32 = 12.5;
/// Rod score thresholds tried, in robust standard deviations of the score.
const K_LEVELS: [f32; 5] = [8.0, 6.0, 5.0, 4.0, 3.0];
/// Shortest microtubule kept, nm.
const MIN_LENGTH_NM: f32 = 100.0;
/// Least end-to-end length per voxel length of a line (straightness).
const MIN_STRAIGHT: f32 = 0.6;
/// Lines are joined across gaps up to this, nm, when within this angle.
const JOIN_NM: f32 = 30.0;
const JOIN_COS: f32 = 0.9;
/// The lumen must be less dense than the wall with at least this t
/// statistic (wedge-stretched solid particles look like short rods along
/// the beam but are not hollow).
const LUMEN_T: f32 = 3.0;
/// The lumen test needs voxels at least this fine, Å.
const LUMEN_MAX_A: f32 = 16.0;

#[derive(Clone, Debug, Serialize)]
pub struct Filament {
    /// Centre-line points, voxels of the tomogram (x, y, z), ordered.
    pub points: Vec<[f32; 3]>,
    pub length: f32,
}

/// Eigenvalues of a symmetric 3×3 matrix (xx, yy, zz, xy, xz, yz), sorted
/// by absolute value, and the eigenvector of the smallest.
fn eigen(h: [f32; 6]) -> ([f32; 3], [f32; 3]) {
    let [a, b, c, d, e, f] = h.map(|x| x as f64);
    let p1 = d * d + e * e + f * f;
    let q = (a + b + c) / 3.0;
    let p2 = (a - q).powi(2) + (b - q).powi(2) + (c - q).powi(2) + 2.0 * p1;
    let p = (p2 / 6.0).sqrt();
    let mut l = if p < 1e-12 {
        [a, b, c]
    } else {
        let bm = [(a - q) / p, (b - q) / p, (c - q) / p, d / p, e / p, f / p];
        let det = bm[0] * (bm[1] * bm[2] - bm[5] * bm[5]) - bm[3] * (bm[3] * bm[2] - bm[5] * bm[4]) + bm[4] * (bm[3] * bm[5] - bm[1] * bm[4]);
        let r = (det / 2.0).clamp(-1.0, 1.0);
        let phi = r.acos() / 3.0;
        let l1 = q + 2.0 * p * phi.cos();
        let l3 = q + 2.0 * p * (phi + 2.0 * std::f64::consts::PI / 3.0).cos();
        [l1, 3.0 * q - l1 - l3, l3]
    };
    l.sort_by(|x, y| x.abs().total_cmp(&y.abs()));
    // Eigenvector of l[0]: the largest cross product of rows of (H - λI).
    let m = [[a - l[0], d, e], [d, b - l[0], f], [e, f, c - l[0]]];
    let cross = |u: [f64; 3], v: [f64; 3]| [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let cands = [cross(m[0], m[1]), cross(m[0], m[2]), cross(m[1], m[2])];
    let best = cands.iter().max_by(|u, v| (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).total_cmp(&(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]))).unwrap();
    let n = (best[0] * best[0] + best[1] * best[1] + best[2] * best[2]).sqrt().max(1e-30);
    (l.map(|x| x as f32), [best[0] / n, best[1] / n, best[2] / n].map(|x| x as f32))
}

/// Rod score and axis of every voxel of `g` (smoothed, dims `m`).
fn rods(g: &[f32], m: [usize; 3]) -> (Vec<f32>, Vec<[f32; 3]>) {
    let (w, h, d) = (m[0], m[1], m[2]);
    let mut score = vec![0f32; g.len()];
    let mut axis = vec![[0f32; 3]; g.len()];
    score.par_chunks_mut(w * h).zip(axis.par_chunks_mut(w * h)).enumerate().for_each(|(z, (sp, ap))| {
        if z == 0 || z + 1 >= d {
            return;
        }
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                let i = (z * h + y) * w + x;
                let at = |dx: isize, dy: isize, dz: isize| g[(i as isize + dx + dy * w as isize + dz * (w * h) as isize) as usize];
                let c = g[i];
                let hxx = at(1, 0, 0) - 2.0 * c + at(-1, 0, 0);
                let hyy = at(0, 1, 0) - 2.0 * c + at(0, -1, 0);
                let hzz = at(0, 0, 1) - 2.0 * c + at(0, 0, -1);
                let hxy = (at(1, 1, 0) - at(1, -1, 0) - at(-1, 1, 0) + at(-1, -1, 0)) / 4.0;
                let hxz = (at(1, 0, 1) - at(1, 0, -1) - at(-1, 0, 1) + at(-1, 0, -1)) / 4.0;
                let hyz = (at(0, 1, 1) - at(0, 1, -1) - at(0, -1, 1) + at(0, -1, -1)) / 4.0;
                let (l, v) = eigen([hxx, hyy, hzz, hxy, hxz, hyz]);
                let j = y * w + x;
                // A bright rod: both cross-section curvatures negative and
                // alike, the one along it small.
                if l[1] < 0.0 && l[2] < 0.0 {
                    let round = l[1] / l[2];
                    let along = (l[0].abs() / l[1].abs()).min(1.0);
                    sp[j] = -l[1] * round * (1.0 - along) * (1.0 - along);
                    ap[j] = v;
                }
            }
        }
    });
    (score, axis)
}

/// Median and robust standard deviation (1.4826 × MAD) of the non-zero values.
fn robust(v: &[f32]) -> (f32, f32) {
    let mut s: Vec<f32> = v.iter().copied().filter(|&x| x > 0.0).step_by(7).collect();
    if s.is_empty() {
        return (0.0, 0.0);
    }
    s.sort_by(f32::total_cmp);
    let med = s[s.len() / 2];
    let mut dev: Vec<f32> = s.iter().map(|x| (x - med).abs()).collect();
    dev.sort_by(f32::total_cmp);
    (med, 1.4826 * dev[dev.len() / 2])
}

/// Two unit vectors across `a`.
fn across(a: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let t = if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let cross = |u: [f32; 3], v: [f32; 3]| [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let u = cross(a, t);
    let l = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
    let u = u.map(|x| x / l);
    (u, cross(a, u))
}

/// Trilinear sample of `v` (dims `m`) at p, 0 outside.
fn sample(v: &[f32], m: [usize; 3], p: [f32; 3]) -> f32 {
    if (0..3).any(|a| p[a] < 0.0 || p[a] > (m[a] - 1) as f32) {
        return 0.0;
    }
    let i = p.map(|x| x.floor() as usize);
    let t = [p[0] - i[0] as f32, p[1] - i[1] as f32, p[2] - i[2] as f32];
    let mut s = 0.0;
    for k in 0..8 {
        let o = [k & 1, (k >> 1) & 1, (k >> 2) & 1];
        let q = [(i[0] + o[0]).min(m[0] - 1), (i[1] + o[1]).min(m[1] - 1), (i[2] + o[2]).min(m[2] - 1)];
        let wgt = (0..3).map(|a| if o[a] == 1 { t[a] } else { 1.0 - t[a] }).product::<f32>();
        s += wgt * v[(q[2] * m[1] + q[1]) * m[0] + q[0]];
    }
    s
}

/// Finds the microtubules of a tomogram (`values`, dims `n`, X fastest,
/// `pixel_a` Å per voxel).
pub fn find(values: &[f32], n: [usize; 3], pixel_a: f32) -> Vec<Filament> {
    let f = WORK_NM * 10.0 / pixel_a;
    let m: [usize; 3] = std::array::from_fn(|a| ((n[a] as f32 / f).round() as usize).max(4));
    let (mean, std) = crate::wedge::stats(values);
    // Dense is dark in cryo-ET: flip so microtubules are bright.
    let inv: Vec<f32> = values.par_iter().map(|&x| if x.is_finite() { (mean - x) / std.max(1e-12) } else { 0.0 }).collect();
    let work = crate::membrane::resample(&inv, n, m);
    drop(inv);
    let sigma = RADIUS_NM / std::f32::consts::SQRT_2 / WORK_NM;
    let g = crate::wedge::smooth(&crate::wedge::Vol { n: m, v: work.clone() }, sigma).v;
    let (score, axis) = rods(&g, m);
    drop(g);
    let (med, sd) = robust(&score);
    let lumen = pixel_a <= LUMEN_MAX_A;
    let light = crate::wedge::smooth(&crate::wedge::Vol { n: m, v: work }, 0.6).v;
    // Lines come out at several thresholds: too low and a microtubule merges
    // with its neighbours, too high and it breaks up, and where that happens
    // depends on the rest of the tomogram. Strictest first; a line is kept
    // unless most of it lies on one kept already.
    let mut kept: Vec<Vec<[f32; 3]>> = vec![];
    let mut template_lines = Vec::new();
    for k in K_LEVELS {
        for l in lines_at(&score, &axis, m, med + k * sd.max(1e-12)) {
            if lumen && polyline_len(&l) >= MIN_LENGTH_NM / WORK_NM {
                template_lines.push(l.clone());
            }
            if !accept(&l, &light, m, lumen) {
                continue;
            }
            let r2 = (RADIUS_NM / WORK_NM).powi(2);
            let on = l.iter().filter(|p| kept.iter().any(|q| q.iter().any(|c| (0..3).map(|a| (p[a] - c[a]).powi(2)).sum::<f32>() <= r2))).count();
            if on * 2 < l.len() {
                kept.push(l);
            }
        }
    }
    // A ridge may follow a wall rather than the hollow centre. A finite
    // cylinder supplies both a centre and an orientation for tracing it.
    // Keep the existing accepted lines, and require the same geometric and
    // lumen checks of additional traces. Coarse data cannot resolve the lumen.
    if lumen {
        for line in cylinder::trace(&light, m, &template_lines) {
            let r2 = (RADIUS_NM / WORK_NM).powi(2);
            let on = line.iter().filter(|p| kept.iter().any(|q| q.iter().any(|c| (0..3).map(|a| (p[a] - c[a]).powi(2)).sum::<f32>() <= r2))).count();
            if on * 2 < line.len() {
                kept.push(line);
            }
        }
    }
    kept.into_iter()
        .map(|fl| {
            // Back to the tomogram's voxels.
            let to = |p: [f32; 3]| std::array::from_fn(|a| (p[a] + 0.5) * n[a] as f32 / m[a] as f32 - 0.5);
            Filament { points: fl.iter().map(|&p| to(p)).collect(), length: polyline_len(&fl) * WORK_NM }
        })
        .collect()
}

/// Centre lines of the ridges above `thr` (see the module notes).
fn lines_at(score: &[f32], axis: &[[f32; 3]], m: [usize; 3], thr: f32) -> Vec<Vec<[f32; 3]>> {
    let (w, h) = (m[0], m[1]);
    // Ridge voxels: above the threshold and a maximum across the axis.
    let ridge: Vec<bool> = (0..score.len())
        .into_par_iter()
        .map(|i| {
            if score[i] < thr {
                return false;
            }
            let p = [(i % w) as f32, ((i / w) % h) as f32, (i / (w * h)) as f32];
            let (u, v) = across(axis[i]);
            [u, v, u.map(|x| -x), v.map(|x| -x)].iter().all(|d| sample(score, m, [p[0] + d[0], p[1] + d[1], p[2] + d[2]]) <= score[i])
        })
        .collect();
    // Lines: ridge voxels within LINK voxels of each other.
    const LINK: i64 = 3;
    let mut seen = vec![false; ridge.len()];
    let mut lines = vec![];
    for s in 0..ridge.len() {
        if !ridge[s] || seen[s] {
            continue;
        }
        seen[s] = true;
        let mut stack = vec![s];
        let mut pts: Vec<[f32; 3]> = vec![];
        while let Some(i) = stack.pop() {
            let p = [i % w, (i / w) % h, i / (w * h)];
            pts.push(p.map(|x| x as f32));
            for dz in -LINK..=LINK {
                for dy in -LINK..=LINK {
                    for dx in -LINK..=LINK {
                        let q = [p[0] as i64 + dx, p[1] as i64 + dy, p[2] as i64 + dz];
                        if (0..3).any(|a| q[a] < 0 || q[a] >= m[a] as i64) {
                            continue;
                        }
                        let j = (q[2] as usize * h + q[1] as usize) * w + q[0] as usize;
                        if ridge[j] && !seen[j] {
                            seen[j] = true;
                            stack.push(j);
                        }
                    }
                }
            }
        }
        if pts.len() >= 3 {
            lines.push(centre(&pts));
        }
    }
    join(lines)
}


/// Unit direction from a to b.
fn dir(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
    d.map(|x| x / l)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Joins lines end to end where two ends are close, the lines point the same
/// way and the gap runs along them: candidate pairs of ends (found through a
/// grid) are linked closest first, each end at most once, and the linked
/// lines are walked into chains.
fn join(lines: Vec<Vec<[f32; 3]>>) -> Vec<Vec<[f32; 3]>> {
    let gap = JOIN_NM / WORK_NM;
    // End e of line i: 0 = first point, 1 = last; direction out of the line.
    let end = |l: &[[f32; 3]], e: usize| if e == 1 { l[l.len() - 1] } else { l[0] };
    let out_dir = |l: &[[f32; 3]], e: usize| {
        let k = l.len().min(6) - 1;
        if e == 1 { dir(l[l.len() - 1 - k], l[l.len() - 1]) } else { dir(l[k], l[0]) }
    };
    let key = |p: [f32; 3]| p.map(|x| (x / gap).floor() as i64);
    let mut grid: std::collections::HashMap<[i64; 3], Vec<(usize, usize)>> = std::collections::HashMap::new();
    for (i, l) in lines.iter().enumerate() {
        for e in 0..2 {
            grid.entry(key(end(l, e))).or_default().push((i, e));
        }
    }
    let mut pairs: Vec<(f32, (usize, usize), (usize, usize))> = vec![];
    for (i, l) in lines.iter().enumerate() {
        for e in 0..2 {
            let a = end(l, e);
            let ka = key(a);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(v) = grid.get(&[ka[0] + dx, ka[1] + dy, ka[2] + dz]) else { continue };
                        for &(j, f) in v {
                            if j <= i {
                                continue;
                            }
                            let b = end(&lines[j], f);
                            let d = (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt();
                            if d > gap {
                                continue;
                            }
                            let (oi, oj) = (out_dir(l, e), out_dir(&lines[j], f));
                            let g = dir(a, b);
                            if dot(oi, oj) > -JOIN_COS || (d > 1.5 && (dot(oi, g) < JOIN_COS || dot(oj, g) > -JOIN_COS)) {
                                continue;
                            }
                            pairs.push((d, (i, e), (j, f)));
                        }
                    }
                }
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    // link[i][e] = the end it is joined to.
    let mut link: Vec<[Option<(usize, usize)>; 2]> = vec![[None, None]; lines.len()];
    let mut root: Vec<usize> = (0..lines.len()).collect();
    fn find(r: &mut [usize], mut i: usize) -> usize {
        while r[i] != i {
            r[i] = r[r[i]];
            i = r[i];
        }
        i
    }
    for (_, (i, e), (j, f)) in pairs {
        if link[i][e].is_some() || link[j][f].is_some() {
            continue;
        }
        let (ri, rj) = (find(&mut root, i), find(&mut root, j));
        if ri == rj {
            continue; // would close a loop
        }
        root[ri] = rj;
        link[i][e] = Some((j, f));
        link[j][f] = Some((i, e));
    }
    let mut used = vec![false; lines.len()];
    let mut out = vec![];
    for s in 0..lines.len() {
        if used[s] || (link[s][0].is_some() && link[s][1].is_some()) {
            continue;
        }
        // Start at a free end and walk the chain.
        let (mut i, mut from) = (s, if link[s][0].is_none() { 0 } else { 1 });
        let mut chain: Vec<[f32; 3]> = vec![];
        loop {
            used[i] = true;
            if from == 0 {
                chain.extend(lines[i].iter().copied());
            } else {
                chain.extend(lines[i].iter().rev().copied());
            }
            let Some((j, f)) = link[i][1 - from] else { break };
            i = j;
            from = f;
        }
        out.push(chain);
    }
    out
}

fn polyline_len(p: &[[f32; 3]]) -> f32 {
    p.windows(2).map(|w| (0..3).map(|a| (w[1][a] - w[0][a]).powi(2)).sum::<f32>().sqrt()).sum()
}

/// Orders a line's voxels along its main direction into a centre line, one
/// point per voxel step (the mean of the voxels there). Working voxels.
fn centre(pts: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let c = [0, 1, 2].map(|a| pts.iter().map(|p| p[a]).sum::<f32>() / pts.len() as f32);
    let mut cov = [[0f32; 3]; 3];
    for p in pts {
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += (p[i] - c[i]) * (p[j] - c[j]);
            }
        }
    }
    let mut a = [0.6f32, 0.5, 0.62];
    for _ in 0..60 {
        let b = [0, 1, 2].map(|i| cov[i][0] * a[0] + cov[i][1] * a[1] + cov[i][2] * a[2]);
        let l = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt().max(1e-12);
        a = b.map(|x| x / l);
    }
    // Bin the points along the axis, one bin per voxel; average each bin.
    let t: Vec<f32> = pts.iter().map(|p| (0..3).map(|k| (p[k] - c[k]) * a[k]).sum()).collect();
    let (t0, t1) = t.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    let bins = ((t1 - t0).ceil() as usize + 1).max(1);
    let mut acc = vec![([0f32; 3], 0usize); bins];
    for (p, &tt) in pts.iter().zip(&t) {
        let b = ((tt - t0) as usize).min(bins - 1);
        for k in 0..3 {
            acc[b].0[k] += p[k];
        }
        acc[b].1 += 1;
    }
    let raw: Vec<[f32; 3]> = acc.iter().filter(|x| x.1 > 0).map(|(s, k)| s.map(|v| v / *k as f32)).collect();
    // A running mean over 5 points takes out the zigzag between the ridges.
    (0..raw.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(2), (i + 3).min(raw.len()));
            [0, 1, 2].map(|k| raw[a..b].iter().map(|p| p[k]).sum::<f32>() / (b - a) as f32)
        })
        .collect()
}

/// Whether a centre line is a microtubule: long, straight enough, and
/// (when `lumen`) hollow.
fn accept(centre: &[[f32; 3]], light: &[f32], m: [usize; 3], lumen: bool) -> bool {
    let len = polyline_len(centre);
    let span = (0..3).map(|a| (centre[centre.len() - 1][a] - centre[0][a]).powi(2)).sum::<f32>().sqrt();
    if len < MIN_LENGTH_NM / WORK_NM || span < MIN_STRAIGHT * len {
        return false;
    }
    if lumen {
        // Lumen less dense than the wall 10 nm out, steadily along the line:
        // the per-point difference must be positive with a t statistic of at
        // least LUMEN_T (independent of the tomogram's contrast).
        let r = 10.0 / WORK_NM;
        let mut diffs = Vec::with_capacity(centre.len());
        for (k, p) in centre.iter().enumerate() {
            let next = if k + 1 < centre.len() { centre[k + 1] } else { centre[k] };
            let prev = if k > 0 { centre[k - 1] } else { centre[k] };
            let mut d = [next[0] - prev[0], next[1] - prev[1], next[2] - prev[2]];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            d = if l > 1e-6 { d.map(|x| x / l) } else { dir(centre[0], centre[centre.len() - 1]) };
            let (u, v) = across(d);
            let mut wall = 0.0;
            for j in 0..8 {
                let ang = j as f32 * std::f32::consts::FRAC_PI_4;
                let q = [0, 1, 2].map(|i| p[i] + r * (ang.cos() * u[i] + ang.sin() * v[i]));
                wall += sample(light, m, q) / 8.0;
            }
            diffs.push(wall - sample(light, m, *p));
        }
        // Points one radius apart are roughly independent.
        let step = (RADIUS_NM / WORK_NM).round().max(1.0) as usize;
        let d: Vec<f32> = diffs.iter().copied().step_by(step).collect();
        let mean = d.iter().sum::<f32>() / d.len() as f32;
        let var = d.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / (d.len() as f32 - 1.0).max(1.0);
        let t = mean / (var.sqrt() / (d.len() as f32).sqrt()).max(1e-12);
        if t < LUMEN_T {
            return false;
        }
    }
    true
}

/// Marks the voxels within the microtubule radius of the centre lines
/// (dims `n`, `pixel_a` Å per voxel).
pub fn mask(fils: &[Filament], n: [usize; 3], pixel_a: f32) -> Vec<bool> {
    mask_r(fils, n, RADIUS_NM * 10.0 / pixel_a)
}

/// Voxels within `r` voxels of the centre lines.
fn mask_r(fils: &[Filament], n: [usize; 3], r: f32) -> Vec<bool> {
    let mut out = vec![false; n[0] * n[1] * n[2]];
    for f in fils {
        // Points every half voxel along the line.
        let mut pts = vec![];
        for w in f.points.windows(2) {
            let d = (0..3).map(|a| (w[1][a] - w[0][a]).powi(2)).sum::<f32>().sqrt();
            let k = (d * 2.0).ceil().max(1.0) as usize;
            for s in 0..k {
                let t = s as f32 / k as f32;
                pts.push([0, 1, 2].map(|a| w[0][a] + t * (w[1][a] - w[0][a])));
            }
        }
        if let Some(l) = f.points.last() {
            pts.push(*l);
        }
        for p in pts {
            let lo = p.map(|x| (x - r).floor().max(0.0) as usize);
            let hi: [usize; 3] = std::array::from_fn(|a| ((p[a] + r).ceil() as usize).min(n[a] - 1));
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        let d2 = (x as f32 - p[0]).powi(2) + (y as f32 - p[1]).powi(2) + (z as f32 - p[2]).powi(2);
                        if d2 <= r * r {
                            out[(z * n[1] + y) * n[0] + x] = true;
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 nm voxels: noise, two hollow tubes, a row of solid 25 nm balls
    /// (ribosomes) and a membrane sheet; density dark like cryo-ET.
    fn scene() -> (Vec<f32>, [usize; 3]) {
        let n = [240, 240, 120];
        let mut rng = 7u64;
        let mut noise = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % 2000) as f32 / 1000.0 - 1.0
        };
        let mut v = vec![0f32; n[0] * n[1] * n[2]];
        for z in 0..n[2] {
            for y in 0..n[1] {
                for x in 0..n[0] {
                    let (xf, yf, zf) = (x as f32, y as f32, z as f32);
                    let mut dens = 0.0;
                    // Tube 1 along X at y=60, z=60; tube 2 tilted in XY.
                    let r1 = ((yf - 60.0).powi(2) + (zf - 60.0).powi(2)).sqrt();
                    let r2 = {
                        let (dx, dy) = (xf - 40.0, yf - 200.0);
                        let t = (dx * 0.8 - dy * 0.6).clamp(0.0, 180.0);
                        let q = [dx - 0.8 * t, dy + 0.6 * t, zf - 50.0];
                        (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt()
                    };
                    for r in [r1, r2] {
                        if (7.5..12.5).contains(&r) {
                            dens += 1.0;
                        }
                    }
                    // A row of ribosomes along x at y=150.
                    for k in 0..6 {
                        let c = [60.0 + k as f32 * 32.0, 150.0, 85.0];
                        if ((xf - c[0]).powi(2) + (yf - c[1]).powi(2) + (zf - c[2]).powi(2)).sqrt() < 12.5 {
                            dens += 1.0;
                        }
                    }
                    // Membrane: plane z = 95, 4 nm thick.
                    if (zf - 95.0).abs() < 2.0 {
                        dens += 1.0;
                    }
                    v[(z * n[1] + y) * n[0] + x] = -dens + 0.8 * noise();
                }
            }
        }
        (v, n)
    }

    #[test]
    fn finds_tubes_not_ribosomes_or_membranes() {
        let (v, n) = scene();
        let f = find(&v, n, 10.0);
        assert_eq!(f.len(), 2, "{:?}", f.iter().map(|f| (f.length, f.points[0], *f.points.last().unwrap())).collect::<Vec<_>>());
        for fl in &f {
            assert!(fl.length > 150.0, "{}", fl.length);
        }
        let m = mask(&f, n, 10.0);
        assert!(m[(60 * n[1] + 60) * n[0] + 120]);
        assert!(!m[(85 * n[1] + 150) * n[0] + 60]);
    }

    #[test]
    fn finds_tubes_after_axis_permutations() {
        let (values, n) = scene();
        for order in [[2, 0, 1], [1, 2, 0]] {
            let rotated_n = order.map(|a| n[a]);
            let mut rotated = vec![0.0; values.len()];
            for z in 0..n[2] {
                for y in 0..n[1] {
                    for x in 0..n[0] {
                        let p = [x, y, z];
                        let q = order.map(|a| p[a]);
                        rotated[(q[2] * rotated_n[1] + q[1]) * rotated_n[0] + q[0]] = values[(z * n[1] + y) * n[0] + x];
                    }
                }
            }
            let f = find(&rotated, rotated_n, 10.0);
            assert_eq!(f.len(), 2, "axis order {order:?}: {f:?}");
            let mask = mask(&f, rotated_n, 10.0);
            let at = |p: [usize; 3]| {
                let q = order.map(|a| p[a]);
                mask[(q[2] * rotated_n[1] + q[1]) * rotated_n[0] + q[0]]
            };
            assert!(at([120, 60, 60]), "missed rotated tube: {order:?}");
            assert!(!at([60, 150, 85]), "accepted a rotated ribosome: {order:?}");
        }
    }

    /// On a tomogram: FIL_MRC=<tomogram .mrc> cargo test --release -- --ignored filaments_on_file
    #[test]
    #[ignore]
    fn filaments_on_file() {
        let path = std::env::var("FIL_MRC").unwrap();
        let (v, n, px) = crate::tomo::pipeline::load(&path).unwrap();
        let t = std::time::Instant::now();
        let f = find(&v, n, px as f32);
        eprintln!("{n:?} at {px} Å: {} microtubules in {:.1} s", f.len(), t.elapsed().as_secs_f32());
        for fl in &f {
            eprintln!("  {:.0} nm from {:?} to {:?}", fl.length, fl.points[0].map(|x| x.round()), fl.points.last().unwrap().map(|x| x.round()));
        }
        // Against an annotation mask: share of our tube voxels within 2 radii
        // of annotated microtubule voxels, and share of annotated voxels
        // within 2 radii of our centre lines.
        if let Ok(gt) = std::env::var("FIL_GT") {
            let (g, gn, _) = crate::tomo::pipeline::load(&gt).unwrap();
            assert_eq!(gn, n);
            let g: Vec<bool> = g.iter().map(|&x| x > 0.5).collect();
            let r = RADIUS_NM * 10.0 / px as f32;
            let ours = mask_r(&f, n, r);
            let near_ours = mask_r(&f, n, 2.0 * r);
            let recall = g.iter().zip(&near_ours).filter(|(a, b)| **a && **b).count() as f64 / g.iter().filter(|&&a| a).count().max(1) as f64;
            // Annotated voxels grown by 2 radii, by marking around each.
            let mut grown = g.clone();
            let k = (r as isize).max(1);
            for z in 0..n[2] as isize {
                for y in 0..n[1] as isize {
                    for x in 0..n[0] as isize {
                        if !g[((z as usize) * n[1] + y as usize) * n[0] + x as usize] {
                            continue;
                        }
                        for dz in [-k, 0, k] {
                            for dy in [-k, 0, k] {
                                for dx in [-k, 0, k] {
                                    let q = [x + dx, y + dy, z + dz];
                                    if (0..3).all(|a| q[a] >= 0 && q[a] < n[a] as isize) {
                                        grown[((q[2] as usize) * n[1] + q[1] as usize) * n[0] + q[0] as usize] = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let precision = ours.iter().zip(&grown).filter(|(a, b)| **a && **b).count() as f64 / ours.iter().filter(|&&a| a).count().max(1) as f64;
            eprintln!("precision {precision:.3}, recall {recall:.3} (annotated voxels {})", g.iter().filter(|&&a| a).count());
        }
        if let Ok(out) = std::env::var("FIL_OUT") {
            let m = mask(&f, n, px as f32);
            let v: Vec<f32> = m.iter().map(|&b| b as u8 as f32).collect();
            crate::tomo::output::write_mrc(&out, &v, n, px, false).unwrap();
        }
    }

    #[test]
    fn eigen_of_a_diagonal() {
        let (l, v) = eigen([-3.0, 0.1, -2.0, 0.0, 0.0, 0.0]);
        assert!((l[0] - 0.1).abs() < 1e-5 && (l[1] + 2.0).abs() < 1e-5 && (l[2] + 3.0).abs() < 1e-5);
        assert!((v[1].abs() - 1.0).abs() < 1e-4);
    }
}
