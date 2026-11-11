//! Places where a membrane leaves its own run: pits, buds, funnels (a
//! membrane taking something in, or budding off). They come in every size
//! and stage, so no one template finds them; what they share is geometry:
//! the membrane turns off to one side of where it would run on, and comes
//! back around a mouth.
//!
//! `pits` finds them on a membrane given as seeds (`sheet::seeds`). For
//! every seed the membrane's run nearby is fitted as a plane to the seeds
//! within `wide`, leaving out those that lie off it (so the pit does not
//! pull the plane after it); seeds further than `off` from that plane, on
//! one side and next to each other, are one pit (seeds inside a deep bud
//! are measured from the run nearest to them, not from a plane through the
//! bud itself). Each pit is measured
//! (depth, mouth, neck, the widest part behind the neck) so that pits can
//! be put in the order of their stage and told apart by it, and has a
//! place to cut a cube at: the middle of its mouth, with the axis into it.
//!
//! Lengths are in voxels of the seeds' volume.

use std::collections::HashMap;

use super::shape::Seed;

type P = [f64; 3];

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn unit(a: P) -> P {
    let n = dot(a, a).sqrt();
    if n > 1e-12 {
        a.map(|v| v / n)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// One pit (or bud) of a membrane.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pit {
    /// The middle of the mouth, on the membrane's own run.
    pub mouth_at: P,
    /// Unit vector from the mouth into the pit.
    pub axis: P,
    /// +1 when the pit lies on the side the membrane's normals point to,
    /// −1 on the other.
    pub side: i8,
    /// How far the pit reaches from the membrane's run.
    pub depth: f64,
    /// Width at the mouth.
    pub mouth: f64,
    /// Width at the narrowest place that has a wider one behind it; the
    /// mouth's width when the pit only narrows (no neck yet).
    pub neck: f64,
    /// The widest place behind the neck (`neck` when there is no neck).
    pub bulb: f64,
    /// Indices of its seeds.
    pub seeds: Vec<usize>,
}

impl Pit {
    /// A number that grows as a pit matures: shallow dent, deep pit, bud
    /// on a neck. Depth over mouth, plus how much wider the bulb is than
    /// the neck.
    pub fn stage(&self) -> f64 {
        self.depth / self.mouth.max(1e-9) + (self.bulb / self.neck.max(1e-9) - 1.0)
    }
}

fn cell(p: P, side: f64) -> [i32; 3] {
    std::array::from_fn(|k| (p[k] / side).floor() as i32)
}

/// Indices of the points within `radius` of `p`.
fn within(cells: &HashMap<[i32; 3], Vec<usize>>, side: f64, points: &[P], p: P, radius: f64) -> Vec<usize> {
    let (lo, hi) = (cell([p[0] - radius, p[1] - radius, p[2] - radius], side), cell([p[0] + radius, p[1] + radius, p[2] + radius], side));
    let mut out = vec![];
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                if let Some(v) = cells.get(&[x, y, z]) {
                    out.extend(v.iter().copied().filter(|&j| dot(sub(points[j], p), sub(points[j], p)) <= radius * radius));
                }
            }
        }
    }
    out
}

/// The plane through `points[among]`: its middle and unit normal (the
/// direction of least spread).
fn plane(points: &[P], among: &[usize]) -> (P, P) {
    let k = among.len().max(1) as f64;
    let m = among.iter().fold([0.0; 3], |m, &j| std::array::from_fn(|a| m[a] + points[j][a] / k));
    let mut c = [[0.0; 3]; 3];
    for &j in among {
        let d = sub(points[j], m);
        for a in 0..3 {
            for b in 0..3 {
                c[a][b] += d[a] * d[b];
            }
        }
    }
    // The least eigenvector by inverse power iteration on c + εI is not
    // needed: the cross product of the two largest directions does, and
    // those come from power iteration with deflation.
    let power = |c: &[[f64; 3]; 3], start: P| {
        let mut v = start;
        for _ in 0..60 {
            let w: P = std::array::from_fn(|a| dot(c[a], v));
            if dot(w, w) < 1e-24 {
                break;
            }
            v = unit(w);
        }
        v
    };
    let u = power(&c, [0.577, 0.577, 0.577]);
    let lu = dot(std::array::from_fn(|a| dot(c[a], u)), u);
    let mut d = c;
    for a in 0..3 {
        for b in 0..3 {
            d[a][b] -= lu * u[a] * u[b];
        }
    }
    let start = unit([u[1] - u[2], u[2] - u[0], u[0] - u[1]]);
    let start = if dot(start, start) < 0.5 { unit([u[1], -u[0], 0.0]) } else { start };
    let v = power(&d, start);
    let v = unit(sub(v, u.map(|x| x * dot(v, u))));
    let n = unit([u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]);
    (m, n)
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// The pits of the membrane made of `seeds` (about `spacing` apart, normals
/// to one side throughout). `wide`: the stretch of membrane its own run is
/// judged over, several times a pit's mouth; `off`: how far from that run a
/// seed has to lie to belong to a pit. Shallowest stage first.
pub fn pits(seeds: &[Seed], spacing: f64, wide: f64, off: f64) -> Vec<Pit> {
    let points: Vec<P> = seeds.iter().map(|s| s.pos).collect();
    let mut cells: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        cells.entry(cell(*p, wide)).or_default().push(i);
    }
    // For every seed: how far it lies from the membrane's run, and that run's normal.
    let mut away = vec![0.0f64; seeds.len()];
    let mut run = vec![[0.0f64; 3]; seeds.len()];
    for i in 0..seeds.len() {
        let near = within(&cells, wide, &points, points[i], wide);
        if near.len() < 8 {
            continue;
        }
        let mut kept = near.clone();
        let (mut m, mut n) = plane(&points, &kept);
        for _ in 0..3 {
            let mut d: Vec<f64> = near.iter().map(|&j| dot(sub(points[j], m), n)).collect();
            let mid = median(&mut d.clone());
            let spread = 1.48 * median(&mut d.iter_mut().map(|x| (*x - mid).abs()).collect::<Vec<_>>());
            let limit = 2.5 * spread.max(1.0);
            let next: Vec<usize> = near.iter().copied().filter(|&j| (dot(sub(points[j], m), n) - mid).abs() <= limit).collect();
            if next.len() < 8 || next.len() == kept.len() {
                break;
            }
            kept = next;
            (m, n) = plane(&points, &kept);
        }
        // To the side the membrane's own normals point (those on its run).
        if kept.iter().map(|&j| dot(seeds[j].axis, n)).sum::<f64>() < 0.0 {
            n = n.map(|v| -v);
        }
        away[i] = dot(sub(points[i], m), n);
        run[i] = n;
    }
    // The membrane's run is where seeds lie on their own plane, in stretches
    // at least `wide` across: a ring of seeds that happens to lie on the
    // plane fitted inside a bud is no run.
    let link = 1.9 * spacing;
    let on_run = |i: usize| run[i] != [0.0; 3] && away[i].abs() < 0.5 * off;
    let mut is_run = vec![false; seeds.len()];
    {
        let mut flat_cells: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
        for (i, p) in points.iter().enumerate() {
            if on_run(i) {
                flat_cells.entry(cell(*p, link)).or_default().push(i);
            }
        }
        let mut seen = vec![false; seeds.len()];
        for start in 0..seeds.len() {
            if seen[start] || !on_run(start) {
                continue;
            }
            seen[start] = true;
            let mut group = vec![start];
            let mut head = 0;
            while head < group.len() {
                let i = group[head];
                head += 1;
                for j in within(&flat_cells, link, &points, points[i], link) {
                    if !seen[j] {
                        seen[j] = true;
                        group.push(j);
                    }
                }
            }
            let (lo, hi) = group.iter().fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), &i| (std::array::from_fn(|k| lo[k].min(points[i][k])), std::array::from_fn(|k| hi[k].max(points[i][k]))));
            let across = (0..3).map(|k| (hi[k] - lo[k]).powi(2)).sum::<f64>().sqrt();
            if across >= wide {
                group.iter().for_each(|&i| is_run[i] = true);
            }
        }
    }
    // The others are measured from the run nearest to them: the plane
    // through the run's seeds within `wide` of the nearest one.
    let run_points: Vec<usize> = (0..seeds.len()).filter(|&i| is_run[i]).collect();
    if run_points.is_empty() {
        return vec![];
    }
    let mut run_cells: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for &i in &run_points {
        run_cells.entry(cell(points[i], wide)).or_default().push(i);
    }
    for i in 0..seeds.len() {
        if is_run[i] {
            continue;
        }
        // The nearest seed of the run, looking further out when none is near.
        let mut nearest: Option<(f64, usize)> = None;
        let mut reach = wide;
        while nearest.is_none() && reach <= 8.0 * wide {
            nearest = within(&run_cells, wide, &points, points[i], reach).into_iter().map(|j| (dot(sub(points[j], points[i]), sub(points[j], points[i])), j)).min_by(|a, b| a.0.total_cmp(&b.0));
            reach *= 2.0;
        }
        let Some((_, j)) = nearest else {
            away[i] = 0.0;
            continue;
        };
        let near = within(&run_cells, wide, &points, points[j], wide);
        if near.len() < 8 {
            away[i] = 0.0;
            continue;
        }
        let (m, mut n) = plane(&points, &near);
        if near.iter().map(|&k| dot(seeds[k].axis, n)).sum::<f64>() < 0.0 {
            n = n.map(|v| -v);
        }
        away[i] = dot(sub(points[i], m), n);
        run[i] = n;
    }
    // Seeds off the run, next to each other on one side, are one pit.
    let mut link_cells: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        if !is_run[i] && away[i].abs() >= off {
            link_cells.entry(cell(*p, link)).or_default().push(i);
        }
    }
    let mut seen = vec![false; seeds.len()];
    let mut out = vec![];
    for start in 0..seeds.len() {
        if seen[start] || is_run[start] || away[start].abs() < off {
            continue;
        }
        seen[start] = true;
        let mut group = vec![start];
        let mut head = 0;
        while head < group.len() {
            let i = group[head];
            head += 1;
            for j in within(&link_cells, link, &points, points[i], link) {
                if !seen[j] && (away[j] > 0.0) == (away[start] > 0.0) {
                    seen[j] = true;
                    group.push(j);
                }
            }
        }
        if group.len() < 3 {
            continue;
        }
        let side: f64 = if away[start] > 0.0 { 1.0 } else { -1.0 };
        let k = group.len() as f64;
        let axis = unit(group.iter().fold([0.0; 3], |a, &i| std::array::from_fn(|c| a[c] + side * run[i][c])));
        // Each seed dropped onto the membrane's run; their middle is the mouth's.
        let mouth_at = group.iter().fold([0.0; 3], |m, &i| std::array::from_fn(|c| m[c] + (points[i][c] - away[i] * run[i][c]) / k));
        let depth = group.iter().map(|&i| away[i].abs()).fold(0.0, f64::max);
        // Width against depth, in steps of the spacing.
        let steps = ((depth / spacing).ceil() as usize).max(1);
        let mut rings: Vec<Vec<f64>> = vec![vec![]; steps + 1];
        for &i in &group {
            let d = sub(points[i], mouth_at);
            let h = dot(d, axis);
            let r = dot(d, d) - h * h;
            rings[((h / spacing).floor().max(0.0) as usize).min(steps)].push(r.max(0.0).sqrt());
        }
        let widths: Vec<Option<f64>> = rings.iter_mut().map(|r| (!r.is_empty()).then(|| 2.0 * median(r))).collect();
        let filled: Vec<f64> = widths.iter().flatten().copied().collect();
        let mouth = filled.first().copied().unwrap_or(0.0).max(spacing);
        // The narrowest place with a clearly wider one behind it.
        let (mut neck, mut bulb) = (mouth, mouth);
        let mut ratio = 1.15;
        for (a, w) in filled.iter().enumerate() {
            let behind = filled[a + 1..].iter().copied().fold(0.0, f64::max);
            if *w > 0.0 && behind / w > ratio {
                ratio = behind / w;
                (neck, bulb) = (w.max(spacing), behind);
            }
        }
        out.push(Pit { mouth_at, axis, side: side as i8, depth, mouth, neck, bulb, seeds: group });
    }
    out.sort_by(|a, b| a.stage().total_cmp(&b.stage()));
    out
}

#[cfg(test)]
mod tests {
    use super::super::sheet;
    use super::*;

    const N: [usize; 3] = [150, 60, 64];
    const LEVEL: f64 = 44.0;

    /// The membrane as the border of a solid: above a plane at z = `LEVEL`
    /// with a dent 9 deep at x = 25, a pit 18 deep at x = 70, and at
    /// x = 118 a bud (a ball of radius 10 on a neck of radius 4).
    fn solid(p: P) -> bool {
        let r2 = |x0: f64| (p[0] - x0).powi(2) + (p[1] - 30.0).powi(2);
        let floor = LEVEL - 9.0 * (-r2(25.0) / (2.0 * 49.0)).exp() - 18.0 * (-r2(70.0) / (2.0 * 36.0)).exp();
        let ball = r2(118.0) + (p[2] - (LEVEL - 20.0)).powi(2) <= 100.0;
        let neck = r2(118.0) <= 16.0 && p[2] > LEVEL - 20.0;
        p[2] > floor || ball || neck
    }

    fn membrane() -> Vec<bool> {
        let at = |i: usize| [(i % N[0]) as f64, (i / N[0] % N[1]) as f64, (i / (N[0] * N[1])) as f64];
        (0..N[0] * N[1] * N[2])
            .map(|i| {
                let p = at(i);
                let inside = solid(p);
                // A voxel of the membrane has the other kind within a voxel and a half.
                (-1..=1).any(|dz| (-1..=1).any(|dy| (-1..=1).any(|dx| solid([p[0] + dx as f64 * 1.4, p[1] + dy as f64 * 1.4, p[2] + dz as f64 * 1.4]) != inside)))
            })
            .collect()
    }

    #[test]
    fn a_dent_a_pit_and_a_bud_are_found_measured_and_put_in_order() {
        let mask = membrane();
        let voxels = sheet::touched(&mask, N, &[vec![[4.0, 4.0, LEVEL]]], &[]);
        let spacing = 3.0;
        let seeds = sheet::seeds(&voxels, N, spacing);
        let found = pits(&seeds, spacing, 30.0, 5.0);
        let describe: Vec<String> = found.iter().map(|p| format!("x {:.0} depth {:.1} mouth {:.1} neck {:.1} bulb {:.1} side {} n {}", p.mouth_at[0], p.depth, p.mouth, p.neck, p.bulb, p.side, p.seeds.len())).collect();
        assert_eq!(found.len(), 3, "{describe:#?}");
        let xs: Vec<f64> = found.iter().map(|p| p.mouth_at[0]).collect();
        assert!((xs[0] - 25.0).abs() < 4.0 && (xs[1] - 70.0).abs() < 4.0 && (xs[2] - 118.0).abs() < 4.0, "in the order of stage: {describe:#?}");
        // All on one side, pointing down from the plane.
        assert!(found.iter().all(|p| p.side == found[0].side && p.axis[2] < -0.9), "{describe:#?}");
        for p in &found {
            assert!((p.mouth_at[2] - LEVEL).abs() < 4.0 && (p.mouth_at[1] - 30.0).abs() < 4.0, "{describe:#?}");
        }
        assert!((found[0].depth - 9.0).abs() < 3.0 && (found[1].depth - 18.0).abs() < 4.0 && found[2].depth > 24.0, "{describe:#?}");
        // Only the bud has a neck.
        assert!(found[0].bulb / found[0].neck < 1.2 && found[1].bulb / found[1].neck < 1.2, "{describe:#?}");
        assert!(found[2].bulb / found[2].neck > 1.5 && found[2].neck < 14.0 && found[2].bulb > 14.0, "{describe:#?}");
    }

    #[test]
    fn a_plain_membrane_and_a_vesicle_have_no_pits() {
        let n = [80, 80, 40];
        let flat: Vec<bool> = (0..n[0] * n[1] * n[2]).map(|i| ((i / (n[0] * n[1])) as f64 - 20.0).abs() <= 1.5).collect();
        let voxels = sheet::touched(&flat, n, &[vec![[5.0, 5.0, 20.0]]], &[]);
        assert!(pits(&sheet::seeds(&voxels, n, 3.0), 3.0, 30.0, 5.0).is_empty());
        let m = 90;
        let ball: Vec<bool> = (0..m * m * m)
            .map(|i| {
                let p = [(i % m) as f64 - 45.0, (i / m % m) as f64 - 45.0, (i / (m * m)) as f64 - 45.0];
                (dot(p, p).sqrt() - 36.0).abs() <= 1.5
            })
            .collect();
        let voxels = sheet::touched(&ball, [m; 3], &[vec![[9.0, 45.0, 45.0]]], &[]);
        // Over a stretch much smaller than the vesicle its curve is not a pit.
        let found = pits(&sheet::seeds(&voxels, [m; 3], 3.0), 3.0, 16.0, 5.0);
        assert!(found.is_empty(), "{}", found.len());
    }
}
