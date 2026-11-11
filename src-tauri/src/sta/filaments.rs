//! "Draw two or three filaments, get all of that kind": the lines the user
//! drew on a section are examples. They tell how thick the filament is and
//! whether it is darker or brighter than what surrounds it (`learn`); every
//! thread of that thickness is then traced through the volume in 3D
//! (`ridge`, `trace`), straight or bent, in the section drawn on or
//! leaving it. The result is for the user to check before anything is
//! averaged: each filament has a score and can be dropped.
//!
//! How a thread is told from a sheet or a blob: the second derivatives of
//! the smoothed density at a point (its Hessian) have three principal
//! values. Along a thread one is near zero and the two across it are large
//! and of one sign; a membrane has one large value and a blob three (the
//! line filter of Sato et al., Med. Image Anal. 2, 1998, and Frangi et al.,
//! MICCAI 1998; written here from the papers). Smoothing by the thread's
//! own half-width gives the strongest answer, which is how the thickness is
//! read from the examples (Lindeberg's scale selection, IJCV 30, 1998).
//!
//! Positions are in voxels (x, y, z) of the volume given; volumes are in
//! z, y, x order. The density should be scaled to unit robust spread so
//! that strengths of different tomograms compare.

use std::collections::HashMap;

use anyhow::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::marks::{self, Mark};
use super::membranes::density;
use super::shape;

type P = [f64; 3];

/// Half-widths tried when reading the examples, voxels.
const SCALES: [f64; 7] = [1.0, 1.4, 2.0, 2.8, 4.0, 5.6, 8.0];
/// The volume is binned until the half-width is about this many voxels.
const FINE: f64 = 1.2;
/// A drawn line counts as on a filament when it is this many times stronger
/// there than beside it.
const CLEAR: f32 = 2.0;
/// Threads down to this share of the examples' strength are traced.
pub const LOOSE: f32 = 0.35;
/// From this share on a filament is taken without asking.
pub const SURE: f32 = 0.6;
/// A drawn line belongs to the traced filament that lies under at least
/// this share of it.
const UNDER: f64 = 0.5;

/// The kind of filament the examples show.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kind {
    /// Half-width (the σ of its Gaussian cross-section), voxels.
    pub sigma: f64,
    /// Darker than its surroundings.
    pub dark: bool,
}

/// One filament.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Filament {
    /// Its middle line, points about a voxel of the binned volume apart.
    pub line: Vec<P>,
    /// Length along the line, voxels.
    pub length: f64,
    /// Strength as a share of the examples' strength.
    pub score: f32,
    /// One the user drew on.
    pub example: bool,
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: P) -> f64 {
    dot(a, a).sqrt()
}
fn length(line: &[P]) -> f64 {
    line.windows(2).map(|w| norm(sub(w[1], w[0]))).sum()
}
fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let k = v.len() / 2;
    *v.select_nth_unstable_by(k, f32::total_cmp).1
}

/// How far from a drawn point the filament may be, voxels: the user draws
/// on what the section shows, a little off the middle.
fn snap(sigma: f64) -> f64 {
    1.5 * sigma + 2.0
}

/// Gaussian blur with edge values repeated.
fn blur(v: &[f32], n: [usize; 3], sigma: f64) -> Vec<f32> {
    let r = (3.0 * sigma).ceil() as isize;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-0.5 * (i as f64 / sigma).powi(2)).exp() as f32).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|x| *x /= sum);
    let (nx, ny, nz) = (n[0], n[1], n[2]);
    let slice = nx * ny;
    // Along x.
    let mut a = vec![0f32; v.len()];
    a.par_chunks_mut(nx).zip(v.par_chunks(nx)).for_each(|(o, i)| {
        for x in 0..nx {
            o[x] = k.iter().enumerate().map(|(j, w)| w * i[(x as isize + j as isize - r).clamp(0, nx as isize - 1) as usize]).sum();
        }
    });
    // Along y.
    let mut b = vec![0f32; v.len()];
    b.par_chunks_mut(slice).zip(a.par_chunks(slice)).for_each(|(o, i)| {
        for y in 0..ny {
            let row = &mut o[y * nx..(y + 1) * nx];
            for (j, w) in k.iter().enumerate() {
                let yy = (y as isize + j as isize - r).clamp(0, ny as isize - 1) as usize;
                row.iter_mut().zip(&i[yy * nx..(yy + 1) * nx]).for_each(|(o, i)| *o += w * i);
            }
        }
    });
    // Along z.
    a.par_chunks_mut(slice).enumerate().for_each(|(z, o)| {
        o.fill(0.0);
        for (j, w) in k.iter().enumerate() {
            let zz = (z as isize + j as isize - r).clamp(0, nz as isize - 1) as usize;
            o.iter_mut().zip(&b[zz * slice..(zz + 1) * slice]).for_each(|(o, i)| *o += w * i);
        }
    });
    a
}

/// The three principal values of a symmetric 3×3 matrix (xx, yy, zz, xy,
/// xz, yz), smallest magnitude first.
fn principal(h: [f64; 6]) -> [f64; 3] {
    let [xx, yy, zz, xy, xz, yz] = h;
    let off = xy * xy + xz * xz + yz * yz;
    let mut e = if off < 1e-30 {
        [xx, yy, zz]
    } else {
        let q = (xx + yy + zz) / 3.0;
        let p = (((xx - q).powi(2) + (yy - q).powi(2) + (zz - q).powi(2) + 2.0 * off) / 6.0).sqrt();
        let (a, b, c) = ((xx - q) / p, (yy - q) / p, (zz - q) / p);
        let (d, e, f) = (xy / p, xz / p, yz / p);
        let det = a * (b * c - f * f) - d * (d * c - f * e) + e * (d * f - b * e);
        let phi = (det / 2.0).clamp(-1.0, 1.0).acos() / 3.0;
        let e1 = q + 2.0 * p * phi.cos();
        let e3 = q + 2.0 * p * (phi + 2.0 * std::f64::consts::FRAC_PI_3).cos();
        [e1, 3.0 * q - e1 - e3, e3]
    };
    e.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
    e
}

/// How thread-like the smoothed density `s` is at each voxel, for dark
/// threads (`sign` 1) or bright ones (−1), times `sigma`² so that answers at
/// different smoothings compare. Zero on the outermost voxels.
fn threadness(s: &[f32], n: [usize; 3], sigma: f64, sign: f64) -> Vec<f32> {
    let (nx, ny, nz) = (n[0], n[1], n[2]);
    let slice = nx * ny;
    let mut out = vec![0f32; s.len()];
    if nx < 3 || ny < 3 || nz < 3 {
        return out;
    }
    let scale = sigma * sigma;
    out.par_chunks_mut(slice).enumerate().for_each(|(z, o)| {
        if z == 0 || z + 1 == nz {
            return;
        }
        for y in 1..ny - 1 {
            for x in 1..nx - 1 {
                let i = (z * ny + y) * nx + x;
                let g = |dx: isize, dy: isize, dz: isize| s[(i as isize + dx + dy * nx as isize + dz * slice as isize) as usize] as f64;
                let c = 2.0 * g(0, 0, 0);
                let h = [
                    g(1, 0, 0) + g(-1, 0, 0) - c,
                    g(0, 1, 0) + g(0, -1, 0) - c,
                    g(0, 0, 1) + g(0, 0, -1) - c,
                    (g(1, 1, 0) - g(1, -1, 0) - g(-1, 1, 0) + g(-1, -1, 0)) / 4.0,
                    (g(1, 0, 1) - g(1, 0, -1) - g(-1, 0, 1) + g(-1, 0, -1)) / 4.0,
                    (g(0, 1, 1) - g(0, 1, -1) - g(0, -1, 1) + g(0, -1, -1)) / 4.0,
                ];
                let e = principal(h);
                let (across, most) = (sign * e[1], sign * e[2]);
                if across <= 0.0 || most <= 0.0 {
                    continue;
                }
                let both = (across * most).sqrt();
                // Not a sheet (one value much smaller across) and not a blob
                // (a large value along as well).
                let sheet = across / most;
                let blob = e[0].abs() / both;
                o[y * nx + x] = (scale * both * (1.0 - (-2.0 * sheet * sheet).exp()) * (-2.0 * blob * blob).exp()) as f32;
            }
        }
    });
    out
}

/// The largest value of `v` within `reach` of `p` (0 outside the volume).
fn peak_near(v: &[f32], n: [usize; 3], p: P, reach: f64) -> f32 {
    let r = reach.ceil() as isize;
    let c = [p[0].round() as isize, p[1].round() as isize, p[2].round() as isize];
    let mut best = 0f32;
    for dz in -r..=r {
        for dy in -r..=r {
            for dx in -r..=r {
                let q = [c[0] + dx, c[1] + dy, c[2] + dz];
                if (0..3).any(|a| q[a] < 0 || q[a] >= n[a] as isize) {
                    continue;
                }
                if norm(sub([q[0] as f64, q[1] as f64, q[2] as f64], p)) > reach {
                    continue;
                }
                best = best.max(v[(q[2] as usize * n[1] + q[1] as usize) * n[0] + q[0] as usize]);
            }
        }
    }
    best
}

/// Points a voxel apart along the smooth curve through the clicks of a
/// drawn line, each with the direction of the curve there.
fn dense(line: &[P]) -> Vec<shape::Seed> {
    shape::line(line, 1.0)
}

/// A unit vector across the curve, in the plane of the section where that
/// can be told.
fn beside(axis: P) -> P {
    let d = [axis[1], -axis[0], 0.0];
    let len = norm(d);
    if len > 0.2 {
        [d[0] / len, d[1] / len, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    }
}

/// What the drawn `lines` (clicks, voxels of `vol`) show: how thick the
/// filament under them is and whether it is dark. None when no smoothing
/// makes the lines stand out from what lies beside them.
pub fn learn(vol: &[f32], n: [usize; 3], lines: &[Vec<P>]) -> Option<Kind> {
    let lines: Vec<Vec<shape::Seed>> = lines.iter().map(|l| dense(l)).filter(|l| l.len() >= 4).collect();
    if lines.is_empty() {
        return None;
    }
    let mut best: Option<(f32, Kind)> = None;
    for sigma in SCALES {
        let reach = snap(sigma);
        // Beside the line, clear of the filament itself.
        let off = 3.0 * sigma + reach + 2.0;
        let margin = (off + reach + 3.0 * sigma + 2.0).ceil() as isize;
        let mut on = [vec![], vec![]];
        let mut by = [vec![], vec![]];
        for line in &lines {
            // The part of the volume around this line.
            let mut lo = [isize::MAX; 3];
            let mut hi = [isize::MIN; 3];
            for s in line {
                for a in 0..3 {
                    lo[a] = lo[a].min(s.pos[a].floor() as isize - margin);
                    hi[a] = hi[a].max(s.pos[a].ceil() as isize + margin);
                }
            }
            let lo: [usize; 3] = std::array::from_fn(|a| lo[a].clamp(0, n[a] as isize - 1) as usize);
            let hi: [usize; 3] = std::array::from_fn(|a| hi[a].clamp(0, n[a] as isize - 1) as usize);
            let m: [usize; 3] = std::array::from_fn(|a| hi[a] - lo[a] + 1);
            let mut part = Vec::with_capacity(m[0] * m[1] * m[2]);
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    let row = (z * n[1] + y) * n[0];
                    part.extend_from_slice(&vol[row + lo[0]..=row + hi[0]]);
                }
            }
            let smooth = blur(&part, m, sigma);
            for (k, sign) in [1.0, -1.0].into_iter().enumerate() {
                let t = threadness(&smooth, m, sigma, sign);
                for s in line {
                    let p: P = std::array::from_fn(|a| s.pos[a] - lo[a] as f64);
                    on[k].push(peak_near(&t, m, p, reach));
                    let d = beside(s.axis);
                    for side in [-off, off] {
                        let q: P = std::array::from_fn(|a| p[a] + side * d[a]);
                        if (0..3).all(|a| q[a] >= 0.0 && q[a] <= (m[a] - 1) as f64) {
                            by[k].push(peak_near(&t, m, q, reach));
                        }
                    }
                }
            }
        }
        for (k, dark) in [true, false].into_iter().enumerate() {
            let on = median(std::mem::take(&mut on[k]));
            let by = median(std::mem::take(&mut by[k]));
            if on > CLEAR * by && best.map_or(true, |(b, _)| on - by > b) {
                best = Some((on - by, Kind { sigma, dark }));
            }
        }
    }
    best.map(|(_, k)| k)
}

/// How thread-like every place of a volume is, on a binned grid.
pub struct Ridge {
    /// Size of the binned grid.
    pub n: [usize; 3],
    /// Voxels of the volume per voxel of the grid.
    pub bin: usize,
    /// Half-width looked for, voxels of the volume.
    pub sigma: f64,
    pub v: Vec<f32>,
}

impl Ridge {
    /// A position of the volume on the grid.
    fn to_grid(&self, p: P) -> P {
        p.map(|x| (x + 0.5) / self.bin as f64 - 0.5)
    }
    fn to_volume(&self, p: P) -> P {
        p.map(|x| (x + 0.5) * self.bin as f64 - 0.5)
    }
    /// The strength under each point of the drawn lines.
    fn under(&self, lines: &[Vec<P>]) -> Vec<f32> {
        let reach = snap(self.sigma) / self.bin as f64;
        lines.iter().flat_map(|l| dense(l)).map(|s| peak_near(&self.v, self.n, self.to_grid(s.pos), reach)).collect()
    }
    /// How strong the filaments under the drawn lines are (their median).
    pub fn strength(&self, lines: &[Vec<P>]) -> f32 {
        median(self.under(lines))
    }
}

/// The volume averaged over cubes of `b` voxels.
fn binned(vol: &[f32], n: [usize; 3], b: usize) -> (Vec<f32>, [usize; 3]) {
    if b <= 1 {
        return (vol.to_vec(), n);
    }
    let m = n.map(|x| x / b);
    let mut out = vec![0f32; m[0] * m[1] * m[2]];
    let k = 1.0 / (b * b * b) as f32;
    out.par_chunks_mut((m[0] * m[1]).max(1)).enumerate().for_each(|(z, o)| {
        for dz in 0..b {
            for y in 0..m[1] {
                for dy in 0..b {
                    let row = ((z * b + dz) * n[1] + y * b + dy) * n[0];
                    for x in 0..m[0] {
                        o[y * m[0] + x] += vol[row + x * b..row + x * b + b].iter().sum::<f32>() * k;
                    }
                }
            }
        }
    });
    (out, m)
}

/// How thread-like every place of the volume is for filaments of `kind`.
pub fn ridge(vol: &[f32], n: [usize; 3], kind: Kind) -> Ridge {
    let bin = ((kind.sigma / FINE).floor() as usize).clamp(1, 8);
    let (small, m) = binned(vol, n, bin);
    let s = kind.sigma / bin as f64;
    let smooth = blur(&small, m, s);
    let v = threadness(&smooth, m, s, if kind.dark { 1.0 } else { -1.0 });
    Ridge { n: m, bin, sigma: kind.sigma, v }
}

/// The 26 neighbours of a voxel of the grid `n` that lie inside it.
fn around(i: u32, n: [usize; 3], mut each: impl FnMut(u32)) {
    let i = i as usize;
    let c = [(i % n[0]) as isize, (i / n[0] % n[1]) as isize, (i / (n[0] * n[1])) as isize];
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let q = [c[0] + dx, c[1] + dy, c[2] + dz];
                if (dx, dy, dz) != (0, 0, 0) && (0..3).all(|a| q[a] >= 0 && q[a] < n[a] as isize) {
                    each(((q[2] as usize * n[1] + q[1] as usize) * n[0] + q[0] as usize) as u32);
                }
            }
        }
    }
}

fn place(i: u32, n: [usize; 3]) -> P {
    let i = i as usize;
    [(i % n[0]) as f64, (i / n[0] % n[1]) as f64, (i / (n[0] * n[1])) as f64]
}

/// The voxels of `set` in connected groups (`set` maps a voxel to its
/// place in the list it came from; only the keys matter).
fn groups(set: &HashMap<u32, u32>, n: [usize; 3]) -> Vec<Vec<u32>> {
    let mut seen: HashMap<u32, ()> = HashMap::with_capacity(set.len());
    let mut out = vec![];
    let mut keys: Vec<u32> = set.keys().copied().collect();
    keys.sort_unstable();
    for start in keys {
        if seen.contains_key(&start) {
            continue;
        }
        seen.insert(start, ());
        let mut group = vec![start];
        let mut at = 0;
        while at < group.len() {
            around(group[at], n, |j| {
                if set.contains_key(&j) && seen.insert(j, ()).is_none() {
                    group.push(j);
                }
            });
            at += 1;
        }
        out.push(group);
    }
    out
}

/// The voxel of the group farthest from `start` going through the group,
/// and the way back from it to `start`.
fn farthest(start: u32, index: &HashMap<u32, u32>, n: [usize; 3]) -> Vec<u32> {
    let mut from: Vec<u32> = vec![u32::MAX; index.len()];
    let mut queue = vec![start];
    from[index[&start] as usize] = start;
    let mut at = 0;
    while at < queue.len() {
        let i = queue[at];
        around(i, n, |j| {
            if let Some(&k) = index.get(&j) {
                if from[k as usize] == u32::MAX {
                    from[k as usize] = i;
                    queue.push(j);
                }
            }
        });
        at += 1;
    }
    let mut path = vec![*queue.last().unwrap()];
    while *path.last().unwrap() != start {
        path.push(from[index[path.last().unwrap()] as usize]);
    }
    path
}

/// A running mean over `half` points each side; the ends stay.
fn smooth(line: &[P], half: usize) -> Vec<P> {
    let k = line.len();
    (0..k)
        .map(|i| {
            let h = half.min(i).min(k - 1 - i);
            let mut m = [0.0; 3];
            for p in &line[i - h..=i + h] {
                for a in 0..3 {
                    m[a] += p[a] / (2 * h + 1) as f64;
                }
            }
            m
        })
        .collect()
}

/// Points `step` apart along a polyline.
fn every(line: &[P], step: f64) -> Vec<P> {
    let mut out = vec![];
    let (mut next, mut gone) = (0.0, 0.0);
    for w in line.windows(2) {
        let d = sub(w[1], w[0]);
        let len = norm(d);
        if len < 1e-9 {
            continue;
        }
        while next <= gone + len {
            let t = (next - gone) / len;
            out.push([w[0][0] + t * d[0], w[0][1] + t * d[1], w[0][2] + t * d[2]]);
            next += step;
        }
        gone += len;
    }
    out
}

/// The path of voxels made a smooth middle line: each point is moved to the
/// middle of the strength across the line there.
fn middle(path: &[u32], ridge: &Ridge, floor: f32) -> Vec<P> {
    let n = ridge.n;
    let mut line = smooth(&path.iter().map(|&i| place(i, n)).collect::<Vec<_>>(), 2);
    let reach = 1.5 * ridge.sigma / ridge.bin as f64 + 1.0;
    let r = reach.ceil() as isize;
    for _ in 0..2 {
        let k = line.len();
        let moved: Vec<P> = (0..k)
            .map(|i| {
                let p = line[i];
                let t = sub(line[(i + 2).min(k - 1)], line[i.saturating_sub(2)]);
                let len = norm(t);
                if len < 1e-9 {
                    return p;
                }
                let t = t.map(|x| x / len);
                let c = [p[0].round() as isize, p[1].round() as isize, p[2].round() as isize];
                let (mut sum, mut m) = (0.0, [0.0; 3]);
                for dz in -r..=r {
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let q = [c[0] + dx, c[1] + dy, c[2] + dz];
                            if (0..3).any(|a| q[a] < 0 || q[a] >= n[a] as isize) {
                                continue;
                            }
                            let d = sub([q[0] as f64, q[1] as f64, q[2] as f64], p);
                            if norm(d) > reach || dot(d, t).abs() > 0.75 {
                                continue;
                            }
                            let w = (ridge.v[(q[2] as usize * n[1] + q[1] as usize) * n[0] + q[0] as usize] - floor).max(0.0) as f64;
                            sum += w;
                            for a in 0..3 {
                                m[a] += w * q[a] as f64;
                            }
                        }
                    }
                }
                if sum > 0.0 {
                    m.map(|x| x / sum)
                } else {
                    p
                }
            })
            .collect();
        line = smooth(&moved, 2);
    }
    every(&line, 1.0)
}

/// The strength at a position of the grid (nearest voxel).
fn value(ridge: &Ridge, p: P) -> f32 {
    let q = p.map(|x| x.round() as isize);
    if (0..3).any(|a| q[a] < 0 || q[a] >= ridge.n[a] as isize) {
        return 0.0;
    }
    ridge.v[(q[2] as usize * ridge.n[1] + q[1] as usize) * ridge.n[0] + q[0] as usize]
}

/// Every filament of the volume: threads at least `LOOSE` × `strong` strong
/// and `least` voxels (of the volume) long, each with its strength as a
/// share of `strong` (the examples' strength, `Ridge::strength`). `drawn`
/// are the lines drawn on this volume: the filament under each is marked
/// as an example, and a line with no thread under it is kept as drawn.
pub fn trace(ridge: &Ridge, strong: f32, least: f64, drawn: &[Vec<P>]) -> Vec<Filament> {
    let n = ridge.n;
    let floor = LOOSE * strong;
    let least_grid = least / ridge.bin as f64;
    let mut out: Vec<Filament> = vec![];
    if strong > 0.0 {
        let all: HashMap<u32, u32> = ridge.v.iter().enumerate().filter(|(_, &v)| v > floor).map(|(i, _)| (i as u32, 0)).collect();
        // A path is taken out of its group with this much around it; what
        // is left (a branch, a filament that crossed) is looked at again.
        let clear = 2.0 * ridge.sigma / ridge.bin as f64 + 1.0;
        let c = clear.ceil() as isize;
        let mut stack = groups(&all, n);
        drop(all);
        while let Some(group) = stack.pop() {
            if (group.len() as f64) < least_grid {
                continue;
            }
            let index: HashMap<u32, u32> = group.iter().enumerate().map(|(k, &i)| (i, k as u32)).collect();
            let end = farthest(group[0], &index, n)[0];
            let path = farthest(end, &index, n);
            let line = middle(&path, ridge, floor);
            let long = length(&line);
            if long < least_grid {
                continue;
            }
            let score = median(line.iter().map(|&p| value(ridge, p)).collect()) / strong;
            out.push(Filament { line: line.iter().map(|&p| ridge.to_volume(p)).collect(), length: long * ridge.bin as f64, score, example: false });
            let mut rest = index;
            for &i in &path {
                let p = place(i, n);
                for dz in -c..=c {
                    for dy in -c..=c {
                        for dx in -c..=c {
                            if ((dx * dx + dy * dy + dz * dz) as f64).sqrt() > clear {
                                continue;
                            }
                            let q = [p[0] as isize + dx, p[1] as isize + dy, p[2] as isize + dz];
                            if (0..3).all(|a| q[a] >= 0 && q[a] < n[a] as isize) {
                                rest.remove(&(((q[2] as usize * n[1] + q[1] as usize) * n[0] + q[0] as usize) as u32));
                            }
                        }
                    }
                }
            }
            stack.extend(groups(&rest, n));
        }
    }
    // Which filament each drawn line is on.
    let reach = snap(ridge.sigma);
    for line in drawn {
        let points: Vec<P> = dense(line).iter().map(|s| s.pos).collect();
        if points.is_empty() {
            continue;
        }
        let share = |f: &Filament| points.iter().filter(|&&p| f.line.iter().any(|&q| norm(sub(p, q)) <= reach)).count() as f64 / points.len() as f64;
        let best = out.iter().enumerate().map(|(k, f)| (share(f), k)).max_by(|a, b| a.0.total_cmp(&b.0));
        match best {
            Some((s, k)) if s >= UNDER => out[k].example = true,
            _ => out.push(Filament { length: length(&points), line: points, score: 1.0, example: true }),
        }
    }
    out.sort_by(|a, b| b.example.cmp(&a.example).then(b.score.total_cmp(&a.score)));
    out
}

/// The shortest filament worth keeping, voxels, when the examples are
/// `lines` of a filament `sigma` wide: half the shortest example, and at
/// least a few widths.
pub fn least(lines: &[Vec<P>], sigma: f64) -> f64 {
    let shortest = lines.iter().map(|l| length(l)).fold(f64::INFINITY, f64::min);
    let few = 10.0 * sigma;
    if shortest.is_finite() {
        (0.5 * shortest).max(few)
    } else {
        few
    }
}

/// One filament of a file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    /// The file it was found in (as the marks name it).
    pub tomogram: String,
    #[serde(flatten)]
    pub filament: Filament,
}

/// What `find` gives.
pub struct Search {
    /// None when the drawn lines do not stand out from what lies beside
    /// them: then only the lines as drawn come back.
    pub kind: Option<Kind>,
    pub filaments: Vec<Found>,
}

/// The filaments to average from: those under the lines of `marks` (kind
/// line) and all of their kind in `files` (the files to search; the marks'
/// own files are searched whether listed or not). Positions are in voxels
/// of the file each was found in.
pub fn find(marks: &[Mark], files: &[String]) -> Result<Search> {
    // The drawn lines, file by file.
    let mut drawn: Vec<(String, Vec<Vec<P>>)> = vec![];
    for m in marks.iter().filter(|m| m.kind == marks::Kind::Line && m.points.len() >= 2) {
        match drawn.iter_mut().find(|d| d.0 == m.tomogram) {
            Some(d) => d.1.push(m.points.clone()),
            None => drawn.push((m.tomogram.clone(), vec![m.points.clone()])),
        }
    }
    let as_drawn = |drawn: &[(String, Vec<Vec<P>>)]| {
        drawn
            .iter()
            .flat_map(|(file, lines)| {
                lines.iter().map(move |l| {
                    let line: Vec<P> = dense(l).iter().map(|s| s.pos).collect();
                    Found { tomogram: file.clone(), filament: Filament { length: length(&line), line, score: 1.0, example: true } }
                })
            })
            .collect::<Vec<_>>()
    };
    // What each file's lines say; the kind is what most of them say.
    let mut volumes: Vec<(Vec<f32>, [usize; 3])> = vec![];
    let mut kinds: Vec<Kind> = vec![];
    for (file, lines) in &drawn {
        super::check_cancel()?;
        let (v, n) = density(file)?;
        kinds.extend(learn(&v, n, lines));
        volumes.push((v, n));
    }
    if kinds.is_empty() {
        return Ok(Search { kind: None, filaments: as_drawn(&drawn) });
    }
    kinds.sort_by(|a, b| a.sigma.total_cmp(&b.sigma));
    let kind = Kind { sigma: kinds[kinds.len() / 2].sigma, dark: 2 * kinds.iter().filter(|k| k.dark).count() >= kinds.len() };
    // How strong the examples are, over all the files drawn on.
    let ridges: Vec<Ridge> = volumes.drain(..).map(|(v, n)| ridge(&v, n, kind)).collect();
    let strong = median(ridges.iter().zip(&drawn).flat_map(|(r, d)| r.under(&d.1)).collect());
    let all: Vec<Vec<P>> = drawn.iter().flat_map(|d| d.1.iter().cloned()).collect();
    let least = least(&all, kind.sigma);
    let mut out = vec![];
    for (r, (file, lines)) in ridges.iter().zip(&drawn) {
        super::check_cancel()?;
        out.extend(trace(r, strong, least, lines).into_iter().map(|filament| Found { tomogram: file.clone(), filament }));
    }
    drop(ridges);
    for file in files.iter().filter(|f| !drawn.iter().any(|d| &d.0 == *f)) {
        super::check_cancel()?;
        let (v, n) = density(file)?;
        let r = ridge(&v, n, kind);
        drop(v);
        out.extend(trace(&r, strong, least, &[]).into_iter().map(|filament| Found { tomogram: file.clone(), filament }));
    }
    Ok(Search { kind: Some(kind), filaments: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic noise of unit spread.
    fn noise(seed: &mut u64) -> f32 {
        let mut sum = 0.0;
        for _ in 0..12 {
            *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            sum += (*seed >> 40) as f32 / (1u64 << 24) as f32;
        }
        sum - 6.0
    }

    const N: [usize; 3] = [128, 128, 48];

    /// The three curves of the test volume: an arc in one section, a wave
    /// that climbs through the sections, and a straight line that falls.
    fn curves() -> Vec<Vec<P>> {
        let arc = (14..=114).map(|x| [x as f64, -52.0 + (10000.0 - (x as f64 - 64.0).powi(2)).sqrt(), 24.0]).collect();
        let wave = (14..=114).map(|x| [x as f64, 80.0 + 8.0 * (x as f64 / 12.0).sin(), 12.0 + 0.2 * (x as f64 - 14.0)]).collect();
        let fall = (0..=90).map(|k| [20.0 + k as f64, 112.0 - k as f64 / 9.0, 36.0 - 26.0 * k as f64 / 90.0]).collect();
        vec![arc, wave, fall]
    }

    fn off(p: P, curve: &[P]) -> f64 {
        curve.iter().map(|&q| norm(sub(p, q))).fold(f64::INFINITY, f64::min)
    }

    /// Noise with the three curves as threads of half-width 2, a sheet at
    /// y = 12 and a ball, all `depth` deep.
    fn volume(depth: f32) -> Vec<f32> {
        let curves = curves();
        let mut seed = 7u64;
        let mut v = vec![0f32; N[0] * N[1] * N[2]];
        for z in 0..N[2] {
            for y in 0..N[1] {
                for x in 0..N[0] {
                    let p = [x as f64, y as f64, z as f64];
                    let mut d = curves.iter().map(|c| off(p, c)).fold(f64::INFINITY, f64::min);
                    d = d.min((y as f64 - 12.0).abs());
                    let ball = norm(sub(p, [100.0, 62.0, 24.0])) / 1.5;
                    d = d.min(ball);
                    v[(z * N[1] + y) * N[0] + x] = noise(&mut seed) + depth * (-0.5 * (d / 2.0).powi(2)).exp() as f32;
                }
            }
        }
        v
    }

    /// The clicks of a user who drew along part of the arc, a little off.
    fn drawn() -> Vec<Vec<P>> {
        let arc = &curves()[0];
        vec![(20..=80).step_by(12).enumerate().map(|(k, i)| [arc[i][0], arc[i][1] + if k % 2 == 0 { 1.0 } else { -1.0 }, 24.0]).collect()]
    }

    #[test]
    fn all_threads_like_the_drawn_one_are_found() {
        let vol = volume(-3.0);
        let lines = drawn();
        let kind = learn(&vol, N, &lines).expect("the drawn line is on a thread");
        assert!(kind.dark);
        assert!((1.4..=2.8).contains(&kind.sigma), "half-width {}", kind.sigma);
        let ridge = ridge(&vol, N, kind);
        let strong = ridge.strength(&lines);
        let found = trace(&ridge, strong, least(&lines, kind.sigma), &lines);
        let curves = curves();
        assert_eq!(found.len(), 3, "{:?}", found.iter().map(|f| (f.length, f.score, f.line[0])).collect::<Vec<_>>());
        assert_eq!(found.iter().filter(|f| f.example).count(), 1);
        for (k, curve) in curves.iter().enumerate() {
            // The filament on this curve: all of it lies on the curve and it
            // covers most of the curve, in whichever section the curve is.
            let f = found.iter().find(|f| off(f.line[f.line.len() / 2], curve) < 2.0).unwrap_or_else(|| panic!("curve {k} not found"));
            let worst = f.line.iter().map(|&p| off(p, curve)).fold(0.0, f64::max);
            assert!(worst < 2.5, "curve {k}: {worst} off");
            assert!(f.length > 0.8 * length(curve), "curve {k}: {} of {}", f.length, length(curve));
            assert_eq!(f.example, k == 0);
            assert!(f.score >= SURE, "curve {k}: score {}", f.score);
        }
    }

    #[test]
    fn bright_threads_are_read_as_bright() {
        let vol = volume(3.0);
        let kind = learn(&vol, N, &drawn()).unwrap();
        assert!(!kind.dark);
    }

    #[test]
    fn a_line_on_nothing_teaches_nothing() {
        let vol = volume(0.0);
        assert_eq!(learn(&vol, N, &drawn()), None);
    }

    #[test]
    fn principal_values_of_a_known_matrix() {
        // Diagonal 1, −5, 3 turned about z by 30°.
        let (s, c) = 30f64.to_radians().sin_cos();
        let (a, b) = (1.0, -5.0);
        let h = [a * c * c + b * s * s, a * s * s + b * c * c, 3.0, (a - b) * s * c, 0.0, 0.0];
        let e = principal(h);
        assert!((e[0] - 1.0).abs() < 1e-9 && (e[1] - 3.0).abs() < 1e-9 && (e[2] + 5.0).abs() < 1e-9, "{e:?}");
    }

    #[test]
    fn lines_drawn_in_one_file_find_the_threads_of_another() {
        let dir = tempfile::tempdir().unwrap();
        let vol = volume(-3.0);
        let (a, b) = (dir.path().join("a.mrc"), dir.path().join("b.mrc"));
        for p in [&a, &b] {
            crate::tomo::output::write_mrc(p.to_str().unwrap(), &vol, N, 10.0, false).unwrap();
        }
        let (a, b) = (a.to_str().unwrap().to_string(), b.to_str().unwrap().to_string());
        let marks = vec![Mark { kind: marks::Kind::Line, tomogram: a.clone(), points: drawn().remove(0), flip: false, strokes: vec![] }];
        let found = find(&marks, &[a.clone(), b.clone()]).unwrap();
        assert!(found.kind.is_some());
        for (file, examples) in [(&a, 1), (&b, 0)] {
            let of: Vec<&Found> = found.filaments.iter().filter(|f| &f.tomogram == file).collect();
            assert_eq!(of.len(), 3);
            assert_eq!(of.iter().filter(|f| f.filament.example).count(), examples);
        }
    }
}
