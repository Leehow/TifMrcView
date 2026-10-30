//! Diffraction indexing (Pro): finds the spots of a single-crystal electron
//! diffraction pattern (or of a lattice FFT), its centre, and the 2D
//! reciprocal lattice that indexes them; and, given a unit cell, ranks the
//! zone axes whose spot pattern looks like the measured one.
//!
//! - Spots: the image is log-compressed (diffraction spans many decades),
//!   a broad box background is subtracted, and local maxima standing a few
//!   robust standard deviations above it are kept, refined to a centroid.
//! - Centre: a pattern is centrosymmetric (Friedel's law), so the midpoints
//!   of spot pairs pile up at the centre. The densest midpoint wins.
//! - Lattice: pairs of short spot vectors are tried as bases, each scored
//!   by how many spots it indexes; the best one (and among nearly as good
//!   ones, the one with the fewest empty lattice points) is reduced and
//!   then refined by least squares together with the centre.
//! - Zone axes: for each [uvw] the reflections perpendicular to it, allowed
//!   by the lattice centring, give a 2D lattice; its two shortest vectors
//!   and their angle are compared with the measured ones.

use std::sync::Arc;

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::image::OpenImage;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindReq {
    pub token: u64,
    pub z: usize,
    pub t: usize,
    pub c: usize,
    /// [x, y, w, h]; None is the whole image.
    pub region: Option<[f64; 4]>,
    /// Smallest distance between two spots, pixels.
    pub min_dist: f64,
    /// Spots must stand this many robust standard deviations above the background.
    pub sigma: f64,
    /// A centre given by the user, image pixels; None finds it.
    pub center: Option<[f64; 2]>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Spot {
    pub x: f64,
    pub y: f64,
    /// Height above the background, in robust standard deviations.
    pub snr: f64,
    /// Lattice indices when the spot is on the lattice.
    pub hk: Option<[i32; 2]>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Lattice {
    /// Reciprocal basis vectors, image pixels, reduced: |a| ≤ |b|, angle 60–90°.
    pub a: [f64; 2],
    pub b: [f64; 2],
    /// Spots on the lattice, and the RMS distance of those from their lattice points.
    pub inliers: usize,
    pub rms: f64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Pattern {
    pub center: [f64; 2],
    pub spots: Vec<Spot>,
    pub lattice: Option<Lattice>,
    pub ms: f64,
}

/// Most spots kept, strongest first.
const MAX_SPOTS: usize = 400;

/// Box mean over a (2r+1)² window, by running sums along rows then columns.
fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let line = |src: &[f32], len: usize, get: &dyn Fn(usize) -> usize| -> Vec<f32> {
        let mut out = vec![0f32; len];
        let mut s = 0f64;
        let mut n = 0usize;
        for i in 0..len.min(r + 1) {
            s += src[get(i)] as f64;
            n += 1;
        }
        for (i, o) in out.iter_mut().enumerate() {
            *o = (s / n as f64) as f32;
            if i + r + 1 < len {
                s += src[get(i + r + 1)] as f64;
                n += 1;
            }
            if i >= r {
                s -= src[get(i - r)] as f64;
                n -= 1;
            }
        }
        out
    };
    let rows: Vec<f32> = (0..h).into_par_iter().flat_map_iter(|y| line(v, w, &|x| y * w + x)).collect();
    let cols: Vec<Vec<f32>> = (0..w).into_par_iter().map(|x| line(&rows, h, &|y| y * w + x)).collect();
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            *o = cols[x][y];
        }
    });
    out
}

/// Spots in a plane: (x, y, snr), strongest first.
fn find_spots(v: &[f32], w: usize, h: usize, min_dist: f64, sigma: f64) -> Vec<(f64, f64, f64)> {
    let lo = v.iter().copied().fold(f32::INFINITY, f32::min);
    let lg: Vec<f32> = v.par_iter().map(|&x| (x - lo).max(0.0).ln_1p()).collect();
    let r = (min_dist.max(2.0) / 2.0).round() as usize;
    // Background: a box several spot spacings wide, twice to round it.
    let bg = box_mean(&box_mean(&lg, w, h, 3 * r + 2), w, h, 3 * r + 2);
    let res: Vec<f32> = lg.par_iter().zip(&bg).map(|(a, b)| a - b).collect();
    // Robust noise: median absolute deviation of a sample.
    let mut sample: Vec<f32> = res.iter().step_by((res.len() / 200_000).max(1)).copied().collect();
    sample.sort_by(f32::total_cmp);
    let med = sample[sample.len() / 2];
    let mut dev: Vec<f32> = sample.iter().map(|x| (x - med).abs()).collect();
    dev.sort_by(f32::total_cmp);
    let noise = (dev[dev.len() / 2] as f64 * 1.4826).max(1e-6);
    let thr = med as f64 + sigma * noise;
    // Local maxima over a (2r+1)² window above the threshold.
    let peaks: Vec<(f64, f64, f64)> = (r..h.saturating_sub(r))
        .into_par_iter()
        .flat_map_iter(|y| {
            let res = &res;
            (r..w.saturating_sub(r)).filter_map(move |x| {
                let c = res[y * w + x];
                if (c as f64) < thr {
                    return None;
                }
                for yy in y - r..=y + r {
                    for xx in x - r..=x + r {
                        let o = res[yy * w + xx];
                        // Ties go to the first in raster order.
                        if o > c || (o == c && (yy, xx) < (y, x)) {
                            return None;
                        }
                    }
                }
                // Centroid of what stands above the background nearby.
                let q = r.clamp(1, 3);
                let (mut sx, mut sy, mut sw) = (0f64, 0f64, 0f64);
                for yy in y.saturating_sub(q)..=(y + q).min(h - 1) {
                    for xx in x.saturating_sub(q)..=(x + q).min(w - 1) {
                        let wt = (res[yy * w + xx] as f64 - med as f64).max(0.0);
                        sx += wt * xx as f64;
                        sy += wt * yy as f64;
                        sw += wt;
                    }
                }
                let (px, py) = if sw > 0.0 { (sx / sw, sy / sw) } else { (x as f64, y as f64) };
                Some((px, py, (c as f64 - med as f64) / noise))
            })
        })
        .collect();
    let mut peaks = peaks;
    peaks.sort_by(|a, b| b.2.total_cmp(&a.2));
    peaks.truncate(MAX_SPOTS);
    peaks
}

/// The point the spot pattern is most symmetric about: the densest
/// midpoint of spot pairs, weighted by strength, then averaged nearby.
fn friedel_center(spots: &[(f64, f64, f64)], tol: f64) -> Option<[f64; 2]> {
    if spots.len() < 3 {
        return None;
    }
    let mids: Vec<(f64, f64, f64)> = (0..spots.len())
        .flat_map(|i| (i..spots.len()).map(move |j| (i, j)))
        .map(|(i, j)| {
            let (a, b) = (spots[i], spots[j]);
            // A spot paired with itself votes too: the central beam.
            let w = (a.2 * b.2).sqrt() * if i == j { 0.5 } else { 1.0 };
            ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0, w)
        })
        .collect();
    let score = |c: (f64, f64)| -> f64 { mids.iter().filter(|m| (m.0 - c.0).hypot(m.1 - c.1) < tol).map(|m| m.2).sum() };
    // Candidates: midpoints of pairs that include one of the strongest spots.
    let top = spots.len().min(30);
    let best = (0..top)
        .flat_map(|i| (i..spots.len()).map(move |j| (i, j)))
        .map(|(i, j)| ((spots[i].0 + spots[j].0) / 2.0, (spots[i].1 + spots[j].1) / 2.0))
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|c| (score(c), c))
        .max_by(|a, b| a.0.total_cmp(&b.0))?;
    let near: Vec<&(f64, f64, f64)> = mids.iter().filter(|m| (m.0 - best.1 .0).hypot(m.1 - best.1 .1) < tol).collect();
    let sw: f64 = near.iter().map(|m| m.2).sum();
    Some([near.iter().map(|m| m.0 * m.2).sum::<f64>() / sw, near.iter().map(|m| m.1 * m.2).sum::<f64>() / sw])
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn norm(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}

/// Gauss (Lagrange) reduction: shortest basis of the same lattice, |a| ≤ |b|,
/// angle between 60° and 90°, right-handed in image coordinates.
fn reduce(mut a: [f64; 2], mut b: [f64; 2]) -> ([f64; 2], [f64; 2]) {
    for _ in 0..100 {
        if norm(b) < norm(a) {
            std::mem::swap(&mut a, &mut b);
        }
        let m = (dot(a, b) / dot(a, a)).round();
        if m == 0.0 {
            break;
        }
        b = [b[0] - m * a[0], b[1] - m * a[1]];
    }
    if norm(b) < norm(a) {
        std::mem::swap(&mut a, &mut b);
    }
    // Obtuse → acute (angle ≤ 90°).
    if dot(a, b) < 0.0 {
        b = [-b[0], -b[1]];
    }
    if cross(a, b) < 0.0 {
        a = [-a[0], -a[1]];
        b = [-b[0], -b[1]];
        // Keep the angle acute after flipping both (it is).
    }
    (a, b)
}

/// Indices of g in basis (a, b), and how far g is from that lattice point.
fn index(g: [f64; 2], a: [f64; 2], b: [f64; 2]) -> ([i32; 2], f64) {
    let det = cross(a, b);
    let h = cross(g, b) / det;
    let k = cross(a, g) / det;
    let (hi, ki) = (h.round(), k.round());
    let p = [hi * a[0] + ki * b[0], hi * a[1] + ki * b[1]];
    ([hi as i32, ki as i32], norm([g[0] - p[0], g[1] - p[1]]))
}

/// Best lattice for spot vectors g (relative to the centre) with weights.
fn fit_lattice(g: &[([f64; 2], f64)], min_len: f64) -> Option<([f64; 2], [f64; 2])> {
    let mut by_len: Vec<usize> = (0..g.len()).filter(|&i| norm(g[i].0) >= min_len).collect();
    by_len.sort_by(|&i, &j| norm(g[i].0).total_cmp(&norm(g[j].0)));
    by_len.truncate(24);
    if by_len.len() < 2 {
        return None;
    }
    let tol_of = |a: [f64; 2], b: [f64; 2]| (0.12 * norm(a).min(norm(b))).max(1.0);
    let score = |a: [f64; 2], b: [f64; 2]| -> f64 {
        let tol = tol_of(a, b);
        g.iter().filter(|(v, _)| index(*v, a, b).1 < tol).map(|(_, w)| w).sum()
    };
    let mut cands: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for (x, &i) in by_len.iter().enumerate() {
        for &j in &by_len[x + 1..] {
            let (u, v) = (g[i].0, g[j].0);
            if cross(u, v).abs() > 0.2 * norm(u) * norm(v) {
                cands.push(reduce(u, v));
            }
        }
    }
    let scored: Vec<(f64, f64, ([f64; 2], [f64; 2]))> = cands.into_par_iter().map(|(a, b)| (score(a, b), cross(a, b).abs(), (a, b))).collect();
    let best = scored.iter().map(|s| s.0).fold(0f64, f64::max);
    if best <= 0.0 {
        return None;
    }
    // Among bases that index nearly as much, the largest cell: a basis of
    // half the spacing would index every spot too, with empty points between.
    scored.iter().filter(|s| s.0 >= 0.95 * best).max_by(|a, b| a.1.total_cmp(&b.1)).map(|s| s.2)
}

/// Least squares for centre and basis together: p = c + h·a + k·b, over
/// the spots that index within tolerance.
fn refine(spots: &[(f64, f64, f64)], c: [f64; 2], a: [f64; 2], b: [f64; 2]) -> ([f64; 2], [f64; 2], [f64; 2]) {
    let tol = (0.12 * norm(a).min(norm(b))).max(1.0);
    let rows: Vec<([f64; 3], [f64; 2])> = spots
        .iter()
        .filter_map(|s| {
            let (hk, e) = index([s.0 - c[0], s.1 - c[1]], a, b);
            (e < tol).then_some(([1.0, hk[0] as f64, hk[1] as f64], [s.0, s.1]))
        })
        .collect();
    if rows.len() < 4 {
        return (c, a, b);
    }
    // Normal equations, 3 × 3, shared by x and y.
    let mut m = [[0f64; 3]; 3];
    let mut r = [[0f64; 3]; 2];
    for (x, p) in &rows {
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += x[i] * x[j];
            }
            r[0][i] += x[i] * p[0];
            r[1][i] += x[i] * p[1];
        }
    }
    let Some(inv) = invert3(m) else { return (c, a, b) };
    let solve = |rhs: [f64; 3]| -> [f64; 3] { std::array::from_fn(|i| (0..3).map(|j| inv[i][j] * rhs[j]).sum()) };
    let (sx, sy) = (solve(r[0]), solve(r[1]));
    ([sx[0], sy[0]], [sx[1], sy[1]], [sx[2], sy[2]])
}

fn invert3(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let c = |i: usize, j: usize| {
        let (r0, r1) = ((i + 1) % 3, (i + 2) % 3);
        let (c0, c1) = ((j + 1) % 3, (j + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    // Inverse = adjugate / det; adjugate[i][j] = cofactor[j][i].
    Some(std::array::from_fn(|i| std::array::from_fn(|j| c(j, i) / det)))
}

/// Spots, centre and lattice of a set of spots (image pixels).
fn analyse(spots: Vec<(f64, f64, f64)>, center: Option<[f64; 2]>, min_dist: f64) -> Result<(Vec<Spot>, [f64; 2], Option<Lattice>)> {
    if spots.is_empty() {
        bail!("没有找到斑点：把阈值调低一些，或检查最小间距");
    }
    let tol = (min_dist / 3.0).max(1.0);
    let mut c = match center {
        Some(c) => c,
        None => friedel_center(&spots, tol).unwrap_or([spots[0].0, spots[0].1]),
    };
    let g: Vec<([f64; 2], f64)> = spots.iter().map(|s| ([s.0 - c[0], s.1 - c[1]], s.2.sqrt())).collect();
    let mut lattice = None;
    if let Some((a0, b0)) = fit_lattice(&g, min_dist * 0.75) {
        let (c1, a1, b1) = refine(&spots, c, a0, b0);
        let (a, b) = reduce(a1, b1);
        if center.is_none() {
            c = c1;
        }
        let tol = (0.12 * norm(a).min(norm(b))).max(1.0);
        let errs: Vec<f64> = spots.iter().map(|s| index([s.0 - c[0], s.1 - c[1]], a, b).1).filter(|&e| e < tol).collect();
        let rms = (errs.iter().map(|e| e * e).sum::<f64>() / errs.len().max(1) as f64).sqrt();
        lattice = Some(Lattice { a, b, inliers: errs.len(), rms });
    }
    let out = spots
        .iter()
        .map(|s| {
            let hk = lattice.as_ref().and_then(|l| {
                let (hk, e) = index([s.0 - c[0], s.1 - c[1]], l.a, l.b);
                (e < (0.12 * norm(l.a).min(norm(l.b))).max(1.0)).then_some(hk)
            });
            Spot { x: s.0, y: s.1, snr: s.2, hk }
        })
        .collect();
    Ok((out, c, lattice))
}

pub fn find(img: &Arc<OpenImage>, req: &FindReq) -> Result<Pattern> {
    let start = std::time::Instant::now();
    let info = img.info();
    let (page, sample) = info.locate(req.c, req.z, req.t);
    let planes = img.page(page)?;
    let p = &planes[sample.min(planes.len() - 1)];
    let [x0, y0, w, h] = match req.region {
        Some(r) => {
            let x0 = r[0].max(0.0).floor() as usize;
            let y0 = r[1].max(0.0).floor() as usize;
            let x1 = ((r[0] + r[2]).ceil() as usize).min(p.w);
            let y1 = ((r[1] + r[3]).ceil() as usize).min(p.h);
            if x1 <= x0 + 16 || y1 <= y0 + 16 {
                bail!("区域太小");
            }
            [x0, y0, x1 - x0, y1 - y0]
        }
        None => [0, 0, p.w, p.h],
    };
    let v: Vec<f32> = (0..w * h).into_par_iter().map(|i| p.data.get_f64((y0 + i / w) * p.w + x0 + i % w) as f32).collect();
    let spots: Vec<(f64, f64, f64)> = find_spots(&v, w, h, req.min_dist, req.sigma).into_iter().map(|(x, y, s)| (x + x0 as f64, y + y0 as f64, s)).collect();
    let (spots, center, lattice) = analyse(spots, req.center, req.min_dist)?;
    Ok(Pattern { center, spots, lattice, ms: start.elapsed().as_secs_f64() * 1e3 })
}

// ---- Zone axes from a unit cell ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneReq {
    /// a, b, c (Å) and α, β, γ (degrees).
    pub cell: [f64; 6],
    /// "P", "I", "F", "C", "A", "B" or "R" (hexagonal axes, obverse).
    pub centering: String,
    /// Measured |a*|, |b*| (1/Å) and the angle between them (degrees).
    pub a_len: f64,
    pub b_len: f64,
    pub gamma: f64,
    /// Largest index of the zone axes tried.
    #[serde(default = "default_max_uvw")]
    pub max_uvw: i32,
}

fn default_max_uvw() -> i32 {
    3
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Zone {
    pub uvw: [i32; 3],
    /// The reflections along the measured a* and b*.
    pub g1: [i32; 3],
    pub g2: [i32; 3],
    /// Their lengths (1/Å) and angle (degrees).
    pub g1_len: f64,
    pub g2_len: f64,
    pub angle: f64,
    /// Relative length error (largest of the two) and angle error (degrees).
    pub len_err: f64,
    pub angle_err: f64,
}

fn allowed(c: &str, h: i32, k: i32, l: i32) -> bool {
    match c {
        "I" => (h + k + l) % 2 == 0,
        "F" => (h % 2 == 0) == (k % 2 == 0) && (k % 2 == 0) == (l % 2 == 0),
        "C" => (h + k) % 2 == 0,
        "A" => (k + l) % 2 == 0,
        "B" => (h + l) % 2 == 0,
        "R" => (-h + k + l).rem_euclid(3) == 0,
        _ => true,
    }
}

fn gcd(a: i32, b: i32) -> i32 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

/// Reciprocal metric tensor G* of a cell (Å, degrees), for |g|² = hᵀ G* h.
fn reciprocal_metric(cell: [f64; 6]) -> Result<[[f64; 3]; 3]> {
    let [a, b, c, al, be, ga] = cell;
    let (ca, cb, cg) = (al.to_radians().cos(), be.to_radians().cos(), ga.to_radians().cos());
    let g = [[a * a, a * b * cg, a * c * cb], [a * b * cg, b * b, b * c * ca], [a * c * cb, b * c * ca, c * c]];
    invert3(g).ok_or_else(|| anyhow::anyhow!("晶胞参数不对"))
}

pub fn zones(req: &ZoneReq) -> Result<Vec<Zone>> {
    if !(req.a_len > 0.0 && req.b_len > 0.0) || req.cell[..3].iter().any(|&v| !(v > 0.0)) {
        bail!("需要晶胞参数和测得的倒格矢");
    }
    let gs = reciprocal_metric(req.cell)?;
    let len = |h: [i32; 3]| -> f64 {
        let v = h.map(|x| x as f64);
        (0..3).map(|i| (0..3).map(|j| v[i] * gs[i][j] * v[j]).sum::<f64>()).sum::<f64>().max(0.0).sqrt()
    };
    let dotg = |h: [i32; 3], k: [i32; 3]| -> f64 {
        let (u, v) = (h.map(|x| x as f64), k.map(|x| x as f64));
        (0..3).map(|i| (0..3).map(|j| u[i] * gs[i][j] * v[j]).sum::<f64>()).sum()
    };
    // Reflections out to a bit past the longer measured vector.
    let gmax = req.a_len.max(req.b_len) * 2.2;
    let lim = 12;
    let mut refl: Vec<([i32; 3], f64)> = Vec::new();
    for h in -lim..=lim {
        for k in -lim..=lim {
            for l in -lim..=lim {
                if (h, k, l) == (0, 0, 0) || !allowed(&req.centering, h, k, l) {
                    continue;
                }
                let g = len([h, k, l]);
                if g <= gmax {
                    refl.push(([h, k, l], g));
                }
            }
        }
    }
    refl.sort_by(|a, b| a.1.total_cmp(&b.1));
    let m = req.max_uvw.clamp(1, 6);
    let mut out = Vec::new();
    for u in -m..=m {
        for v in -m..=m {
            for w in 0..=m {
                // One of ±[uvw], primitive.
                if (u, v, w) == (0, 0, 0) || gcd(gcd(u, v), w) != 1 || (w == 0 && (v < 0 || (v == 0 && u < 0))) {
                    continue;
                }
                let zone: Vec<&([i32; 3], f64)> = refl.iter().filter(|(h, _)| h[0] * u + h[1] * v + h[2] * w == 0).collect();
                let Some(&&(g1, l1)) = zone.first() else { continue };
                // Shortest one not parallel to g1.
                let Some(&&(g2, l2)) = zone.iter().find(|(h, l)| {
                    let c = dotg(g1, *h) / (l1 * l);
                    c.abs() < 0.999
                }) else {
                    continue;
                };
                let mut angle = (dotg(g1, g2) / (l1 * l2)).clamp(-1.0, 1.0).acos().to_degrees();
                let mut g2 = g2;
                if angle > 90.0 {
                    angle = 180.0 - angle;
                    g2 = g2.map(|x| -x);
                }
                // Measured |a*| ≤ |b*|; compare like with like.
                let len_err = ((l1 - req.a_len).abs() / req.a_len).max((l2 - req.b_len).abs() / req.b_len);
                let angle_err = (angle - req.gamma).abs();
                out.push(Zone { uvw: [u, v, w], g1, g2, g1_len: l1, g2_len: l2, angle, len_err, angle_err });
            }
        }
    }
    // A 1% length error weighs like 1° of angle.
    out.sort_by(|a, b| (a.len_err * 100.0 + a.angle_err).total_cmp(&(b.len_err * 100.0 + b.angle_err)));
    out.truncate(12);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic pattern: Gaussian spots on a lattice around a centre,
    /// fading outward, on a sloping background with noise.
    fn pattern(w: usize, h: usize, c: [f64; 2], a: [f64; 2], b: [f64; 2], missing: &[[i32; 2]]) -> Vec<f32> {
        let mut v = vec![0f32; w * h];
        for (i, o) in v.iter_mut().enumerate() {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            let r = (x - c[0]).hypot(y - c[1]);
            let mut z = 1e3 / (1.0 + r / 30.0) + 2.0 * x / w as f64;
            let hash = ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5555) >> 44;
            z += (hash % 1000) as f64 * 0.02;
            *o = z as f32;
        }
        for hh in -12..=12 {
            for kk in -12..=12 {
                if missing.contains(&[hh, kk]) {
                    continue;
                }
                let p = [c[0] + hh as f64 * a[0] + kk as f64 * b[0], c[1] + hh as f64 * a[1] + kk as f64 * b[1]];
                let r = (p[0] - c[0]).hypot(p[1] - c[1]);
                let amp = if (hh, kk) == (0, 0) { 5e4 } else { 2e3 * (-r / 150.0).exp() };
                let (x0, y0) = (p[0].round() as isize, p[1].round() as isize);
                for dy in -4..=4isize {
                    for dx in -4..=4isize {
                        let (x, y) = (x0 + dx, y0 + dy);
                        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
                            continue;
                        }
                        let d2 = (x as f64 - p[0]).powi(2) + (y as f64 - p[1]).powi(2);
                        v[y as usize * w + x as usize] += (amp * (-d2 / (2.0 * 1.5 * 1.5)).exp()) as f32;
                    }
                }
            }
        }
        v
    }

    fn run(v: &[f32], w: usize, h: usize, min_dist: f64) -> (Vec<Spot>, [f64; 2], Option<Lattice>) {
        let spots = find_spots(v, w, h, min_dist, 5.0);
        analyse(spots, None, min_dist).unwrap()
    }

    #[test]
    fn finds_centre_and_lattice() {
        let (w, h) = (512, 480);
        let c = [251.3, 238.7];
        let a = [31.2, 4.1];
        let b = [-9.5, 36.8];
        let v = pattern(w, h, c, a, b, &[]);
        let (spots, cc, lat) = run(&v, w, h, 12.0);
        assert!((cc[0] - c[0]).hypot(cc[1] - c[1]) < 0.2, "centre {cc:?}");
        let l = lat.expect("lattice");
        // Same lattice: each found vector is a short integer combination of the true ones.
        let (ra, rb) = reduce(a, b);
        for (got, want) in [(l.a, ra), (l.b, rb)] {
            let e = norm([got[0] - want[0], got[1] - want[1]]).min(norm([got[0] + want[0], got[1] + want[1]]));
            assert!(e < 0.1, "got {got:?} want {want:?} (basis {:?} {:?})", l.a, l.b);
        }
        assert!(l.rms < 0.3, "rms {}", l.rms);
        assert!(l.inliers >= 40 && l.inliers as f64 > 0.9 * spots.len() as f64, "{} of {}", l.inliers, spots.len());
    }

    #[test]
    fn prefers_the_real_cell_when_spots_are_missing() {
        // Every other row missing: a basis of half the spacing still indexes
        // everything but leaves empty points; the real one must win.
        let (w, h) = (512, 512);
        let c = [256.0, 256.0];
        let a = [24.0, 0.0];
        let b = [0.0, 30.0];
        let missing: Vec<[i32; 2]> = (-12..=12).flat_map(|hh| (-12..=12).map(move |kk| [hh, kk])).filter(|[hh, kk]| (hh + kk) % 2 != 0).collect();
        let v = pattern(w, h, c, a, b, &missing);
        let (_, _, lat) = run(&v, w, h, 12.0);
        let l = lat.unwrap();
        // The centred lattice: shortest vectors (24, ±30)... reduced, area 2·24·30.
        let area = cross(l.a, l.b).abs();
        assert!((area - 2.0 * 24.0 * 30.0).abs() < 20.0, "area {area} basis {:?} {:?}", l.a, l.b);
    }

    #[test]
    fn reduces_bases() {
        let (a, b) = reduce([10.0, 0.0], [23.0, 10.0]);
        assert!((norm(a) - 10.0).abs() < 1e-9 && (b[1].abs() - 10.0).abs() < 1e-9, "{a:?} {b:?}");
        assert!(dot(a, b) >= 0.0 && cross(a, b) > 0.0);
    }

    #[test]
    fn matches_silicon_110() {
        // Si: a = 5.431 Å, F-centred (ignoring the diamond glide). [110]
        // shows 111 (|g| = 0.3189 1/Å) and 002 (0.3683 1/Å) at 54.7°;
        // the shortest pair is 111 and 1-11 at 70.5°.
        let a = 5.431;
        let g111 = 3f64.sqrt() / a;
        let req = ZoneReq { cell: [a, a, a, 90.0, 90.0, 90.0], centering: "F".into(), a_len: g111, b_len: g111, gamma: 70.53, max_uvw: 3 };
        let z = zones(&req).unwrap();
        let first = &z[0];
        let mut uvw = first.uvw.map(|x| x.abs());
        uvw.sort();
        assert_eq!(uvw, [0, 1, 1], "{z:#?}");
        assert!(first.len_err < 1e-6 && first.angle_err < 0.1);
    }

    #[test]
    fn inverts() {
        let m = [[2.0, 1.0, 0.0], [1.0, 3.0, 1.0], [0.0, 1.0, 4.0]];
        let i = invert3(m).unwrap();
        for r in 0..3 {
            for c in 0..3 {
                let v: f64 = (0..3).map(|k| m[r][k] * i[k][c]).sum();
                assert!((v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-12);
            }
        }
    }
}

#[cfg(test)]
mod bench {
    /// TMV_DIFF_RAW=path.f32 TMV_DIFF_W=w TMV_DIFF_H=h cargo test --release --lib diffraction::bench -- --ignored --nocapture
    #[test]
    #[ignore]
    fn file() {
        let path = std::env::var("TMV_DIFF_RAW").unwrap();
        let w: usize = std::env::var("TMV_DIFF_W").unwrap().parse().unwrap();
        let h: usize = std::env::var("TMV_DIFF_H").unwrap().parse().unwrap();
        let min_dist: f64 = std::env::var("TMV_DIFF_DIST").ok().and_then(|v| v.parse().ok()).unwrap_or(12.0);
        let b = std::fs::read(path).unwrap();
        let v: Vec<f32> = b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let t = std::time::Instant::now();
        let spots = super::find_spots(&v, w, h, min_dist, 5.0);
        match super::analyse(spots, None, min_dist) {
            Ok((s, c, l)) => println!("{} spots, centre {c:?}, lattice {l:?}, {:?}", s.len(), t.elapsed()),
            Err(e) => println!("{e}"),
        }
    }
}
