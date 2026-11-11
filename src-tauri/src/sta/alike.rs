//! "Mark one, find the rest" for membranes: the user paints one or a few
//! membranes (`sheet::touched`) and the program finds the others of that
//! kind in the segmentation and turns them all the same way up.
//!
//! - `pieces` splits the segmentation into separate membranes and describes
//!   each by what does not depend on where it lies (`Traits`: how large, how
//!   closed, how bent, how thick).
//! - `like` says whether a membrane is of the kind of the examples.
//! - `lean` measures which side of a membrane carries more density. A
//!   membrane alone is the same from both sides; what sits on it (proteins
//!   on one face, a coat, a lumen of another density) is not. The examples'
//!   lean, with the side the user confirmed, tells the side of every other
//!   membrane: the one on which its lean has the same sign.
//!
//! Positions are voxels (x, y, z) of the segmented volume, as in `sheet`.

use super::shape::Seed;
use super::sheet;

type P = [f64; 3];

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// What a membrane is like, wherever it lies and however it is turned.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Traits {
    /// Root-mean-square distance of the membrane from its middle, voxels.
    pub extent: f64,
    /// 0 for a closed membrane (a vesicle: it surrounds its middle evenly),
    /// towards 1 for an open piece seen from one side (a cap, a flat sheet
    /// has about 0 too but is told apart by `round`).
    pub open: f64,
    /// How evenly far from the middle the membrane is: the spread of the
    /// distances over their mean. 0 for a sphere, about 0.35 for a flat disc.
    pub round: f64,
    /// Membrane voxels per unit of sheet area: the thickness, voxels.
    pub thick: f64,
}

/// One separate membrane of the segmentation.
pub struct Piece {
    /// Sorted voxel indices.
    pub voxels: Vec<u32>,
    pub seeds: Vec<Seed>,
    pub traits: Traits,
}

/// The traits of a membrane from its seeds (`sheet::seeds` at `spacing`)
/// and its number of voxels. None for fewer than 4 seeds.
pub fn traits(seeds: &[Seed], voxels: usize, spacing: f64) -> Option<Traits> {
    if seeds.len() < 4 {
        return None;
    }
    let k = seeds.len() as f64;
    let mut c = [0.0; 3];
    for s in seeds {
        (0..3).for_each(|a| c[a] += s.pos[a] / k);
    }
    let dist: Vec<f64> = seeds.iter().map(|s| dot(sub(s.pos, c), sub(s.pos, c)).sqrt()).collect();
    let mean = dist.iter().sum::<f64>() / k;
    let extent = (dist.iter().map(|d| d * d).sum::<f64>() / k).sqrt();
    let round = (dist.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / k).sqrt() / mean.max(1e-9);
    // Normals face away from the middle on a closed membrane and sum to
    // nothing; on a cap they add up.
    let mut sum = [0.0; 3];
    for s in seeds {
        (0..3).for_each(|a| sum[a] += s.axis[a] / k);
    }
    let open = dot(sum, sum).sqrt();
    let thick = voxels as f64 / (k * spacing * spacing);
    Some(Traits { extent, open, round, thick })
}

/// The separate pieces of `mask` (dims `n`): the sorted voxel indices of
/// each, voxels joined through faces, edges or corners. One pass over the
/// mask, whatever the number of pieces (a segmentation of a large tomogram
/// has thousands of specks).
pub fn components(mask: &[bool], n: [usize; 3]) -> Vec<Vec<u32>> {
    assert_eq!(mask.len(), n[0] * n[1] * n[2]);
    let mut left = mask.to_vec();
    let mut out = vec![];
    for start in 0..left.len() {
        if !left[start] {
            continue;
        }
        left[start] = false;
        let mut queue: Vec<u32> = vec![start as u32];
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
                        if left[j] {
                            left[j] = false;
                            queue.push(j as u32);
                        }
                    }
                }
            }
        }
        queue.sort_unstable();
        out.push(queue);
    }
    out
}

/// The separate membranes of `mask` (dims `n`) with at least `least` seeds
/// at `spacing`, largest first.
pub fn pieces(mask: &[bool], n: [usize; 3], spacing: f64, least: usize) -> Vec<Piece> {
    let mut out = vec![];
    for voxels in components(mask, n) {
        // A seed takes several voxels: a speck cannot give `least` of them.
        if voxels.len() < least.max(4) {
            continue;
        }
        let seeds = sheet::seeds(&voxels, n, spacing);
        if seeds.len() < least.max(4) {
            continue;
        }
        if let Some(traits) = traits(&seeds, voxels.len(), spacing) {
            out.push(Piece { voxels, seeds, traits });
        }
    }
    out.sort_by(|a, b| b.voxels.len().cmp(&a.voxels.len()));
    out
}

/// Sizes within this factor of an example's are the same kind.
const SIZE: f64 = 1.7;
/// `open` and `round` within these of an example's.
const OPEN: f64 = 0.3;
const ROUND: f64 = 0.15;
/// Thickness within this factor (single against double membranes).
const THICK: f64 = 1.6;

/// Whether a membrane with traits `t` is of the kind of one of the examples.
pub fn like(examples: &[Traits], t: &Traits) -> bool {
    let within = |a: f64, b: f64, f: f64| a > 0.0 && b > 0.0 && a / b < f && b / a < f;
    examples.iter().any(|e| within(e.extent, t.extent, SIZE) && (e.open - t.open).abs() < OPEN && (e.round - t.round).abs() < ROUND && within(e.thick, t.thick, THICK))
}

fn sample(vol: &[f32], n: [usize; 3], p: P) -> Option<f64> {
    if (0..3).any(|k| p[k] < 0.0 || p[k] > (n[k] - 1) as f64) {
        return None;
    }
    let f: [usize; 3] = std::array::from_fn(|k| (p[k].floor() as usize).min(n[k] - 2.min(n[k] - 1)));
    let t: [f64; 3] = std::array::from_fn(|k| p[k] - f[k] as f64);
    let mut v = 0.0;
    for (dz, wz) in [(0, 1.0 - t[2]), (1, t[2])] {
        for (dy, wy) in [(0, 1.0 - t[1]), (1, t[1])] {
            for (dx, wx) in [(0, 1.0 - t[0]), (1, t[0])] {
                let w = wx * wy * wz;
                if w > 0.0 {
                    let q = [(f[0] + dx).min(n[0] - 1), (f[1] + dy).min(n[1] - 1), (f[2] + dz).min(n[2] - 1)];
                    v += w * vol[(q[2] * n[1] + q[1]) * n[0] + q[0]] as f64;
                }
            }
        }
    }
    Some(v)
}

/// How much more density lies on the side the seeds' normals point to than
/// on the other: the mean over the seeds of (density `from`..`to` voxels
/// along the normal) − (the same against it), and that mean's standard
/// error. The sign reverses when the normals are reversed. (0, ∞) when
/// nothing could be measured.
pub fn lean(vol: &[f32], n: [usize; 3], seeds: &[Seed], from: f64, to: f64) -> (f64, f64) {
    let steps = ((to - from).ceil() as usize).max(1);
    let mut d: Vec<f64> = vec![];
    for s in seeds {
        let (mut sum, mut count) = (0.0, 0);
        for k in 0..=steps {
            let r = from + (to - from) * k as f64 / steps as f64;
            let out = sample(vol, n, std::array::from_fn(|a| s.pos[a] + r * s.axis[a]));
            let back = sample(vol, n, std::array::from_fn(|a| s.pos[a] - r * s.axis[a]));
            if let (Some(o), Some(b)) = (out, back) {
                sum += o - b;
                count += 1;
            }
        }
        if count > 0 {
            d.push(sum / count as f64);
        }
    }
    if d.len() < 2 {
        return (0.0, f64::INFINITY);
    }
    let k = d.len() as f64;
    let mean = d.iter().sum::<f64>() / k;
    let var = d.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (k - 1.0);
    (mean, (var / k).sqrt())
}

/// A lean this many standard errors from zero tells the side.
pub const SURE: f64 = 3.0;

/// Turns `seeds` to the side the examples face: `example` is the examples'
/// lean (on the side the user confirmed), `own` this membrane's (as its
/// seeds stand). Returns whether the side is sure (both leans clear of
/// zero); when it is not the seeds are left as they are.
pub fn face(seeds: &mut [Seed], example: (f64, f64), own: (f64, f64)) -> bool {
    let clear = |l: (f64, f64)| l.1.is_finite() && l.0.abs() > SURE * l.1;
    if !clear(example) || !clear(own) {
        return false;
    }
    if (example.0 > 0.0) != (own.0 > 0.0) {
        for s in seeds {
            s.axis = s.axis.map(|v| -v);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = 96;

    fn at(i: usize) -> P {
        [(i % N) as f64, (i / N % N) as f64, (i / (N * N)) as f64]
    }

    /// Shells (centre, radius) three voxels thick, and a flat sheet at z = 90.
    fn scene(shells: &[(P, f64)], sheet: bool) -> Vec<bool> {
        (0..N * N * N)
            .map(|i| {
                let p = at(i);
                shells.iter().any(|(c, r)| (dot(sub(p, *c), sub(p, *c)).sqrt() - r).abs() <= 1.5) || (sheet && (p[2] - 90.0).abs() <= 1.5)
            })
            .collect()
    }

    #[test]
    fn vesicles_like_the_example_are_found_and_others_are_not() {
        let shells = [([20.0, 20.0, 30.0], 12.0), ([60.0, 24.0, 34.0], 13.0), ([40.0, 66.0, 40.0], 26.0)];
        let mask = scene(&shells, true);
        let all = pieces(&mask, [N; 3], 5.0, 8);
        assert_eq!(all.len(), 4, "three vesicles and the sheet");
        let middle = |p: &Piece| {
            let k = p.seeds.len() as f64;
            p.seeds.iter().fold([0.0; 3], |m, s| std::array::from_fn(|a| m[a] + s.pos[a] / k))
        };
        let near = |p: &Piece, c: P| dot(sub(middle(p), c), sub(middle(p), c)).sqrt() < 3.0;
        let example = all.iter().find(|p| near(p, shells[0].0)).expect("the first vesicle").traits;
        let found: Vec<&Piece> = all.iter().filter(|p| like(&[example], &p.traits)).collect();
        assert_eq!(found.len(), 2, "{:?}", all.iter().map(|p| p.traits).collect::<Vec<_>>());
        assert!(found.iter().any(|p| near(p, shells[1].0)), "the vesicle of nearly the same size");
        assert!(!found.iter().any(|p| near(p, shells[2].0)), "not the large one");
        // The sheet as the example finds only itself.
        let flat = all.iter().find(|p| p.traits.round > 0.2).expect("the sheet").traits;
        assert_eq!(all.iter().filter(|p| like(&[flat], &p.traits)).count(), 1);
    }

    #[test]
    fn the_side_with_more_density_is_told_and_followed() {
        let shells = [([30.0, 30.0, 40.0], 16.0), ([66.0, 60.0, 44.0], 17.0)];
        let mask = scene(&shells, false);
        // Density: the membrane, and a coat 3 to 6 voxels outside each shell.
        let mut state = 7u32;
        let mut noise = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let vol: Vec<f32> = (0..N * N * N)
            .map(|i| {
                let p = at(i);
                let coat = shells.iter().any(|(c, r)| {
                    let d = dot(sub(p, *c), sub(p, *c)).sqrt() - r;
                    (3.0..=6.0).contains(&d)
                });
                (if mask[i] { 1.0 } else { 0.0 }) + (if coat { 0.4 } else { 0.0 }) + 0.6 * noise()
            })
            .collect();
        let mut all = pieces(&mask, [N; 3], 5.0, 8);
        assert_eq!(all.len(), 2);
        // Normals face outward (`sheet::seeds`): the coat is on their side.
        let a = lean(&vol, [N; 3], &all[0].seeds, 2.5, 7.0);
        assert!(a.0 > SURE * a.1, "{a:?}");
        // The second vesicle inside out: its lean is the other way, and `face` turns it back.
        for s in &mut all[1].seeds {
            s.axis = s.axis.map(|v| -v);
        }
        let b = lean(&vol, [N; 3], &all[1].seeds, 2.5, 7.0);
        assert!(b.0 < -SURE * b.1, "{b:?}");
        let (first, rest) = all.split_at_mut(1);
        let _ = first;
        assert!(face(&mut rest[0].seeds, a, b));
        let c = shells.iter().map(|s| s.0).find(|c| dot(sub(rest[0].seeds[0].pos, *c), sub(rest[0].seeds[0].pos, *c)).sqrt() < 22.0).unwrap();
        assert!(rest[0].seeds.iter().all(|s| dot(s.axis, sub(s.pos, c)) > 0.0), "outward again");
        // No coat: the side is not sure and nothing is turned.
        let plain: Vec<f32> = mask.iter().map(|&m| if m { 1.0 } else { 0.0 } + 0.6 * noise()).collect();
        let flat = lean(&plain, [N; 3], &rest[0].seeds, 2.5, 7.0);
        assert!(!face(&mut rest[0].seeds, a, flat), "{flat:?}");
    }
}
