//! "Paint the membrane": the user scribbles over a membrane and the places
//! to cut cubes at come from the membrane segmentation (`crate::membrane`)
//! instead of from a surface drawn by hand (`shape::surface`).
//!
//! `touched` takes the part of the segmentation the scribble lies on (the
//! voxels joined to it; a second kind of stroke cuts a sheet where two
//! membranes touch). `seeds` turns those voxels into evenly spread points on
//! the sheet's middle, each with the sheet's normal there: the normal is the
//! direction in which the voxels around a point are spread least (principal
//! axes of their positions), and all normals are turned to one side by
//! following the sheet from point to point. Which side that is cannot be
//! known from a membrane alone: normals face away from the middle of what
//! was taken (outward on a vesicle), and the workspace's flip reverses them.
//!
//! Positions are voxels (x, y, z) of the segmented volume, dims `n` with X
//! fastest; a voxel index is (z·n[1] + y)·n[0] + x.

use std::collections::HashMap;

use super::shape::Seed;

type P = [f64; 3];

/// A stroke takes the membrane voxels this near (voxels): scribbles are not
/// drawn exactly on the membrane.
pub const REACH: f64 = 3.0;
/// A point whose surroundings are thicker than this share of their width is
/// not a sheet (a junction, a clump) and gives no seed.
const FLAT: f64 = 0.45;

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn at(i: u32, n: [usize; 3]) -> P {
    let i = i as usize;
    [(i % n[0]) as f64, (i / n[0] % n[1]) as f64, (i / (n[0] * n[1])) as f64]
}

/// Voxels of the grid within `reach` of a stroke's points (and of the
/// straight pieces between consecutive points on one section).
fn near(stroke: &[P], n: [usize; 3], reach: f64) -> Vec<u32> {
    let mut dense: Vec<P> = vec![];
    for (k, p) in stroke.iter().enumerate() {
        dense.push(*p);
        if let Some(q) = stroke.get(k + 1).filter(|q| (q[2] - p[2]).abs() < 0.5) {
            let steps = sub(*q, *p).iter().fold(0.0f64, |m, d| m.max(d.abs())).ceil() as usize;
            dense.extend((1..steps).map(|s| std::array::from_fn(|c| p[c] + (q[c] - p[c]) * s as f64 / steps as f64)));
        }
    }
    let r = reach.ceil() as i64;
    let mut out = vec![];
    for p in dense {
        let c: [i64; 3] = std::array::from_fn(|k| p[k].round() as i64);
        for dz in -r..=r {
            for dy in -r..=r {
                for dx in -r..=r {
                    let q = [c[0] + dx, c[1] + dy, c[2] + dz];
                    if (0..3).any(|k| q[k] < 0 || q[k] >= n[k] as i64) || ((dx * dx + dy * dy + dz * dz) as f64) > reach * reach {
                        continue;
                    }
                    out.push(((q[2] as usize * n[1] + q[1] as usize) * n[0] + q[0] as usize) as u32);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The membrane under the scribble: the voxels of `mask` joined (through
/// faces, edges or corners) to a voxel within `REACH` of the strokes
/// `take`. Voxels within `REACH` of the strokes `cut` count as empty, so a
/// stroke across a sheet ends it there. Sorted voxel indices; empty when
/// the scribble touches no membrane.
pub fn touched(mask: &[bool], n: [usize; 3], take: &[Vec<P>], cut: &[Vec<P>]) -> Vec<u32> {
    assert_eq!(mask.len(), n[0] * n[1] * n[2]);
    let mut gone = vec![false; mask.len()];
    for s in cut {
        for i in near(s, n, REACH) {
            gone[i as usize] = true;
        }
    }
    let mut seen = vec![false; mask.len()];
    let mut queue: Vec<u32> = vec![];
    for s in take {
        for i in near(s, n, REACH) {
            let j = i as usize;
            if mask[j] && !gone[j] && !seen[j] {
                seen[j] = true;
                queue.push(i);
            }
        }
    }
    let mut head = 0;
    while head < queue.len() {
        let i = queue[head] as usize;
        head += 1;
        let (x, y, z) = ((i % n[0]) as i64, (i / n[0] % n[1]) as i64, (i / (n[0] * n[1])) as i64);
        for dz in -1..=1i64 {
            for dy in -1..=1i64 {
                for dx in -1..=1i64 {
                    let (a, b, c) = (x + dx, y + dy, z + dz);
                    if a < 0 || b < 0 || c < 0 || a >= n[0] as i64 || b >= n[1] as i64 || c >= n[2] as i64 {
                        continue;
                    }
                    let j = (c as usize * n[1] + b as usize) * n[0] + a as usize;
                    if mask[j] && !gone[j] && !seen[j] {
                        seen[j] = true;
                        queue.push(j as u32);
                    }
                }
            }
        }
    }
    queue.sort_unstable();
    queue
}

/// Eigenvalues (ascending) and unit eigenvectors of a symmetric 3×3 matrix,
/// by Jacobi rotations.
fn axes(mut a: [[f64; 3]; 3]) -> ([f64; 3], [P; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..32 {
        let (mut p, mut q, mut big) = (0, 1, 0.0f64);
        for (i, j) in [(0, 1), (0, 2), (1, 2)] {
            if a[i][j].abs() > big {
                (p, q, big) = (i, j, a[i][j].abs());
            }
        }
        if big < 1e-12 {
            break;
        }
        let theta = 0.5 * (2.0 * a[p][q]).atan2(a[q][q] - a[p][p]);
        let (s, c) = theta.sin_cos();
        // a ← Jᵀ a J, v ← v J, with J the rotation in the (p, q) plane.
        for k in 0..3 {
            let (x, y) = (a[k][p], a[k][q]);
            a[k][p] = c * x - s * y;
            a[k][q] = s * x + c * y;
        }
        for k in 0..3 {
            let (x, y) = (a[p][k], a[q][k]);
            a[p][k] = c * x - s * y;
            a[q][k] = s * x + c * y;
        }
        for row in &mut v {
            let (x, y) = (row[p], row[q]);
            row[p] = c * x - s * y;
            row[q] = s * x + c * y;
        }
    }
    let mut order = [0, 1, 2];
    order.sort_by(|&i, &j| a[i][i].total_cmp(&a[j][j]));
    (order.map(|i| a[i][i]), order.map(|i| [v[0][i], v[1][i], v[2][i]]))
}

/// Points in cubic cells of side `side`, for "who is near" questions.
struct Cells {
    side: f64,
    map: HashMap<[i32; 3], Vec<u32>>,
}

impl Cells {
    fn key(&self, p: P) -> [i32; 3] {
        std::array::from_fn(|k| (p[k] / self.side).floor() as i32)
    }
    fn new(points: impl Iterator<Item = P>, side: f64) -> Cells {
        let mut c = Cells { side, map: HashMap::new() };
        for (i, p) in points.enumerate() {
            let k = c.key(p);
            c.map.entry(k).or_default().push(i as u32);
        }
        c
    }
    /// Indices of the points in the cells within `radius` of `p` (a
    /// superset of the points within `radius`).
    fn around(&self, p: P, radius: f64, mut each: impl FnMut(u32)) {
        let lo = self.key([p[0] - radius, p[1] - radius, p[2] - radius]);
        let hi = self.key([p[0] + radius, p[1] + radius, p[2] + radius]);
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    if let Some(v) = self.map.get(&[x, y, z]) {
                        v.iter().for_each(|&i| each(i));
                    }
                }
            }
        }
    }
}

/// Seeds on the sheet made of `voxels` (as `touched` returns them), about
/// `spacing` voxels apart, each on the middle of the sheet with the normal
/// there. Normals agree from seed to seed along the sheet and face away
/// from the middle of the voxels: outward on a vesicle or a bulge. Places
/// that are not sheet-like give no seed.
pub fn seeds(voxels: &[u32], n: [usize; 3], spacing: f64) -> Vec<Seed> {
    if voxels.is_empty() {
        return vec![];
    }
    let spacing = spacing.max(2.0);
    let pos: Vec<P> = voxels.iter().map(|&i| at(i, n)).collect();
    // The neighbourhood a normal is taken from: wide enough to span the
    // sheet's thickness several times, narrow enough to follow its bends.
    let around = spacing.clamp(5.0, 12.0);
    let cells = Cells::new(pos.iter().copied(), around);
    // One candidate per spacing-sized cell: the middle of its voxels.
    let coarse = Cells::new(pos.iter().copied(), spacing);
    let mut filled: Vec<usize> = coarse.map.values().map(Vec::len).collect();
    filled.sort_unstable();
    let enough = (filled[filled.len() / 2] / 4).max(1);
    let mut keys: Vec<&[i32; 3]> = coarse.map.keys().collect();
    keys.sort_unstable();
    let mut out: Vec<Seed> = vec![];
    for key in keys {
        let members = &coarse.map[key];
        if members.len() < enough {
            continue;
        }
        let mut c = [0.0; 3];
        for &i in members {
            (0..3).for_each(|k| c[k] += pos[i as usize][k]);
        }
        let c = c.map(|v| v / members.len() as f64);
        // Principal axes of the membrane voxels around the candidate.
        let (mut m, mut count) = ([0.0; 3], 0usize);
        let mut local: Vec<P> = vec![];
        cells.around(c, around, |i| {
            let p = pos[i as usize];
            if dot(sub(p, c), sub(p, c)) <= around * around {
                local.push(p);
                (0..3).for_each(|k| m[k] += p[k]);
                count += 1;
            }
        });
        if count < 6 {
            continue;
        }
        let m = m.map(|v| v / count as f64);
        let mut cov = [[0.0; 3]; 3];
        for p in &local {
            let d = sub(*p, m);
            for a in 0..3 {
                for b in 0..3 {
                    cov[a][b] += d[a] * d[b];
                }
            }
        }
        let (values, vectors) = axes(cov);
        if values[1] <= 0.0 || (values[0] / values[1]).sqrt() > FLAT {
            continue;
        }
        let normal = vectors[0];
        // Onto the sheet's middle, along the normal.
        let lift = dot(sub(m, c), normal);
        out.push(Seed { pos: std::array::from_fn(|k| c[k] + lift * normal[k]), axis: normal });
    }
    one_side(&mut out, 1.9 * spacing);
    out
}

/// Turns the seeds' normals to one side: each connected run of seeds
/// (neighbours within `link`) is walked from seed to seed, a normal being
/// reversed when it opposes its neighbour's; the run then faces away from
/// its own middle.
fn one_side(seeds: &mut [Seed], link: f64) {
    let cells = Cells::new(seeds.iter().map(|s| s.pos), link);
    let mut done = vec![false; seeds.len()];
    for start in 0..seeds.len() {
        if done[start] {
            continue;
        }
        done[start] = true;
        let mut run = vec![start];
        let mut head = 0;
        while head < run.len() {
            let i = run[head];
            head += 1;
            let (p, a) = (seeds[i].pos, seeds[i].axis);
            let mut next: Vec<(f64, usize)> = vec![];
            cells.around(p, link, |j| {
                let j = j as usize;
                let d = sub(seeds[j].pos, p);
                if !done[j] && dot(d, d) <= link * link {
                    next.push((dot(d, d), j));
                }
            });
            // Nearest first: a normal is best judged against a close neighbour.
            next.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            for (_, j) in next {
                if done[j] {
                    continue;
                }
                done[j] = true;
                if dot(seeds[j].axis, a) < 0.0 {
                    seeds[j].axis = seeds[j].axis.map(|v| -v);
                }
                run.push(j);
            }
        }
        let mut c = [0.0; 3];
        for &i in &run {
            (0..3).for_each(|k| c[k] += seeds[i].pos[k]);
        }
        let c = c.map(|v| v / run.len() as f64);
        let out: f64 = run.iter().map(|&i| dot(seeds[i].axis, sub(seeds[i].pos, c))).sum();
        if out < 0.0 {
            for &i in &run {
                seeds[i].axis = seeds[i].axis.map(|v| -v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(n: usize, centre: P, radius: f64, thick: f64) -> Vec<bool> {
        let mut m = vec![false; n * n * n];
        for (i, v) in m.iter_mut().enumerate() {
            let p = at(i as u32, [n; 3]);
            let d = dot(sub(p, centre), sub(p, centre)).sqrt();
            *v = (d - radius).abs() <= thick / 2.0;
        }
        m
    }

    #[test]
    fn a_scribble_takes_the_vesicle_it_lies_on_and_not_the_one_beside() {
        let n = 64;
        let mut mask = shell(n, [20.0, 32.0, 32.0], 12.0, 3.0);
        let other = shell(n, [48.0, 32.0, 32.0], 9.0, 3.0);
        let own = mask.iter().filter(|&&b| b).count();
        mask.iter_mut().zip(&other).for_each(|(a, b)| *a |= *b);
        // A short stroke near the first shell's left edge, two voxels off it.
        let stroke = vec![vec![[6.0, 30.0, 32.0], [6.0, 34.0, 32.0]]];
        let got = touched(&mask, [n; 3], &stroke, &[]);
        assert_eq!(got.len(), own);
        assert!(got.iter().all(|&i| at(i, [n; 3])[0] < 36.0));
        assert!(touched(&mask, [n; 3], &[vec![[34.0, 2.0, 2.0]]], &[]).is_empty());
    }

    #[test]
    fn a_cut_ends_the_sheet() {
        let n = [60, 20, 12];
        // A flat sheet along X at y = 10, three voxels thick.
        let mask: Vec<bool> = (0..n[0] * n[1] * n[2]).map(|i| (at(i as u32, n)[1] - 10.0).abs() <= 1.0).collect();
        let take = vec![vec![[5.0, 10.0, 6.0]]];
        let all = touched(&mask, n, &take, &[]);
        assert_eq!(all.len(), 60 * 3 * 12);
        // A stroke across the sheet on every section at x = 30.
        let cut: Vec<Vec<P>> = (0..12).map(|z| vec![[30.0, 6.0, z as f64], [30.0, 14.0, z as f64]]).collect();
        let part = touched(&mask, n, &take, &cut);
        assert!(!part.is_empty() && part.iter().all(|&i| at(i, n)[0] < 30.0));
    }

    #[test]
    fn seeds_on_a_vesicle_sit_on_its_middle_and_face_outward() {
        let n = 80;
        let (centre, radius) = ([40.0, 39.0, 41.0], 26.0);
        let mask = shell(n, centre, radius, 4.0);
        let voxels = touched(&mask, [n; 3], &[vec![[14.0, 39.0, 41.0]]], &[]);
        let s = seeds(&voxels, [n; 3], 8.0);
        // About one seed per spacing² of surface.
        let expect = 4.0 * std::f64::consts::PI * radius * radius / 64.0;
        assert!((s.len() as f64) > 0.5 * expect && (s.len() as f64) < 2.5 * expect, "{} seeds, about {expect:.0} expected", s.len());
        for seed in &s {
            let d = sub(seed.pos, centre);
            let r = dot(d, d).sqrt();
            assert!((r - radius).abs() < 1.5, "seed at radius {r:.2}");
            let cos = dot(seed.axis, d) / r;
            assert!(cos > 0.97, "normal {:.3} from outward", cos);
        }
    }

    #[test]
    fn normals_of_an_open_bent_sheet_agree_along_it() {
        // Half a cylinder shell about the Z axis (y above the middle).
        let n = [80, 48, 30];
        let c = [40.0, 4.0];
        let mask: Vec<bool> = (0..n[0] * n[1] * n[2])
            .map(|i| {
                let p = at(i as u32, n);
                let r = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
                (r - 30.0).abs() <= 1.5 && p[1] > c[1] + 3.0
            })
            .collect();
        let voxels = touched(&mask, n, &[vec![[40.0, 34.0, 15.0]]], &[]);
        let s = seeds(&voxels, n, 7.0);
        assert!(s.len() > 30);
        for seed in &s {
            let d = [seed.pos[0] - c[0], seed.pos[1] - c[1]];
            let r = (d[0] * d[0] + d[1] * d[1]).sqrt();
            let cos = (seed.axis[0] * d[0] + seed.axis[1] * d[1]) / r;
            assert!(cos > 0.9, "normal {cos:.3} from outward at {:?}", seed.pos);
            assert!(seed.axis[2].abs() < 0.3);
        }
    }

    #[test]
    fn a_clump_gives_no_seeds() {
        let n = 40;
        let mask: Vec<bool> = (0..n * n * n).map(|i| at(i as u32, [n; 3]).iter().all(|&v| (v - 20.0).abs() <= 8.0)).collect();
        let voxels = touched(&mask, [n; 3], &[vec![[20.0, 20.0, 20.0]]], &[]);
        assert!(!voxels.is_empty());
        let s = seeds(&voxels, [n; 3], 6.0);
        // Only the faces' rims could pass for sheets; the inside must not.
        assert!(s.iter().all(|x| x.pos.iter().any(|&v| (v - 20.0).abs() > 4.0)), "{} seeds", s.len());
    }
}
