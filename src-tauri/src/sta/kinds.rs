//! Kinds among the particles of an average: after the alignment, are they
//! all the same thing, or two or three different ones (with and without a
//! partner bound, an empty and a full one, the particle and what is not
//! one)? The user asks with one button and gets the kinds as pictures with
//! their counts; which to keep is theirs to say.
//!
//! The particles are already aligned, so they are compared as they lie on
//! the average, voxel by voxel, with no further search. Two things keep
//! the missing wedge from deciding (particles differ most by which part of
//! them the wedge took, and sorting by that sorts by orientation):
//!
//! * each particle is compared with the average only in the part of the
//!   spectrum it has itself (its wedge, turned with it onto the average;
//!   the "wedge-masked difference" of Heumann, Hoenger and Mastronarde,
//!   J Struct Biol 175, 2011, used as the method, not their code);
//! * the differences are sorted along the few directions in which the
//!   particles differ most from each other (principal components, from
//!   the particles' products with each other), and kinds are only reported
//!   when those directions stand out of the rest and the groups found in
//!   them stand apart.

use anyhow::Result;
use rayon::prelude::*;

use super::cc::transform;
use super::refine::{Progress, Set};
use super::{check_cancel, Pose};
use crate::spa::fourier::{ft3, index};
use crate::spa::Rng;

/// Detail the particles are compared to, cycles across the box, at most.
const BAND: f64 = 8.0;
/// Directions worked out; the last ones say what the rest looks like.
const DIRECTIONS: usize = 12;
/// A direction counts when the particles spread this much more along it
/// than along the rest.
const STANDS_OUT: f64 = 1.5;
/// At most this many kinds.
const MOST: usize = 4;
/// Two kinds are apart when their centres are this many of their own
/// spreads from each other (one cloud cut in two gives about 2.7).
const APART: f64 = 3.5;
/// A kind has at least this many particles, and this share of all.
const FEWEST: usize = 5;
const SHARE: f64 = 0.03;

/// One kind of particle.
pub struct Kind {
    /// Its particles (indices in the set), and their average.
    pub members: Vec<usize>,
    pub map: Vec<f32>,
}

/// The kinds found, most populous first.
pub struct Kinds {
    pub kinds: Vec<Kind>,
    /// How far apart the two nearest of them are, in their own spreads.
    pub apart: f64,
}

/// What each particle has that the average has not, in the part of the
/// spectrum the particle has: real and imaginary parts over half the ball
/// of `band` cycles.
fn differences(set: &Set, poses: &[Pose], average: &[f32], band: f64, progress: Progress) -> Vec<Vec<f32>> {
    let n = set.n;
    let b = band.floor() as i32;
    let mut ks: Vec<[i32; 3]> = vec![];
    for z in -b..=b {
        for y in -b..=b {
            for x in -b..=b {
                let first = z > 0 || (z == 0 && (y > 0 || (y == 0 && x > 0)));
                if first && ((x * x + y * y + z * z) as f64) <= band * band {
                    ks.push([x, y, z]);
                }
            }
        }
    }
    let masked = |v: &[f32]| {
        let mut v = v.to_vec();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        v.iter_mut().for_each(|x| *x -= mean);
        crate::spa::volume::mask_sphere(&mut v, n, set.radius + 2.0, 3.0);
        v
    };
    let (are, aim) = ft3(n, &masked(average));
    let at = |k: [i32; 3]| (index(k[2], n) * n + index(k[1], n)) * n + index(k[0], n);
    let done = std::sync::atomic::AtomicUsize::new(0);
    (0..poses.len())
        .into_par_iter()
        .map(|i| {
            let Pose { r, t, .. } = poses[i];
            // The particle as it lies on the average.
            let (re, im) = ft3(n, &masked(&transform(&set.particles[i], n, &r, t)));
            let wedge = set.wedge(i);
            // What it has: the average's frequency k is the particle's R·k.
            let has: Vec<bool> = ks
                .iter()
                .map(|k| {
                    let q: [i32; 3] = std::array::from_fn(|j| (0..3).map(|c| r[j][c] * k[c] as f64).sum::<f64>().round() as i32);
                    q.iter().all(|v| v.abs() < n as i32 / 2) && wedge[at(q)] > 0.5
                })
                .collect();
            // The average at the particle's own contrast.
            let (mut dot, mut norm) = (0f64, 0f64);
            for (k, h) in ks.iter().zip(&has) {
                if *h {
                    let j = at(*k);
                    dot += (re[j] * are[j] + im[j] * aim[j]) as f64;
                    norm += (are[j] * are[j] + aim[j] * aim[j]) as f64;
                }
            }
            let s = if norm > 0.0 { (dot / norm) as f32 } else { 0.0 };
            let mut out = Vec::with_capacity(2 * ks.len());
            for (k, h) in ks.iter().zip(&has) {
                let j = at(*k);
                out.push(if *h { re[j] - s * are[j] } else { 0.0 });
                out.push(if *h { im[j] - s * aim[j] } else { 0.0 });
            }
            progress("比较颗粒", done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, poses.len());
            out
        })
        .collect()
}

/// The `count` directions along which the rows of `d` (less their mean)
/// spread most: each row's place along them (rows × count, most spread
/// first) and the spread itself along each. Worked out from the rows'
/// products with each other, by repeated multiplying.
fn directions(d: &[Vec<f32>], count: usize, rng: &mut Rng) -> (Vec<Vec<f64>>, Vec<f64>) {
    let m = d.len();
    let len = d[0].len();
    let mut mean = vec![0f64; len];
    for row in d {
        for (a, v) in mean.iter_mut().zip(row) {
            *a += *v as f64 / m as f64;
        }
    }
    let gram: Vec<Vec<f64>> = (0..m)
        .into_par_iter()
        .map(|i| (0..m).map(|j| d[i].iter().zip(&d[j]).zip(&mean).map(|((a, b), c)| (*a as f64 - c) * (*b as f64 - c)).sum()).collect())
        .collect();
    let count = count.min(m.saturating_sub(1)).max(1);
    let mut q: Vec<Vec<f64>> = (0..count).map(|_| (0..m).map(|_| rng.gauss()).collect()).collect();
    let mut spread = vec![0f64; count];
    for _ in 0..60 {
        let mut z: Vec<Vec<f64>> = q.par_iter().map(|v| gram.iter().map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum()).collect()).collect();
        // Each made square to the ones before it, and of length one.
        for a in 0..count {
            for b in 0..a {
                let dot: f64 = z[a].iter().zip(&z[b]).map(|(x, y)| x * y).sum();
                let (head, tail) = z.split_at_mut(a);
                tail[0].iter_mut().zip(&head[b]).for_each(|(x, y)| *x -= dot * y);
            }
            let norm = z[a].iter().map(|x| x * x).sum::<f64>().sqrt();
            spread[a] = norm;
            if norm > 0.0 {
                z[a].iter_mut().for_each(|x| *x /= norm);
            }
        }
        q = z;
    }
    let places = (0..m).map(|i| (0..count).map(|a| q[a][i] * spread[a].max(0.0).sqrt()).collect()).collect();
    (places, spread)
}

fn far(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum()
}

/// `k` groups of the points by their nearest centre (k-means, the best of
/// a few starts): which group each point is in, and the sum of the squared
/// distances to the centres.
fn groups(points: &[Vec<f64>], k: usize, rng: &mut Rng) -> (Vec<usize>, f64) {
    let m = points.len();
    let dim = points[0].len();
    let mut best: (Vec<usize>, f64) = (vec![0; m], f64::MAX);
    for _ in 0..10 {
        // Starts far from each other.
        let mut centres = vec![points[rng.below(m)].clone()];
        while centres.len() < k {
            let near: Vec<f64> = points.iter().map(|p| centres.iter().map(|c| far(p, c)).fold(f64::MAX, f64::min)).collect();
            let total: f64 = near.iter().sum();
            let mut pick = rng.unit() * total;
            let mut chosen = m - 1;
            for (i, d) in near.iter().enumerate() {
                pick -= d;
                if pick <= 0.0 {
                    chosen = i;
                    break;
                }
            }
            centres.push(points[chosen].clone());
        }
        let mut of = vec![0usize; m];
        for _ in 0..50 {
            let next: Vec<usize> = points.iter().map(|p| (0..k).min_by(|a, b| far(p, &centres[*a]).total_cmp(&far(p, &centres[*b]))).unwrap()).collect();
            let same = next == of;
            of = next;
            for (c, centre) in centres.iter_mut().enumerate() {
                let members: Vec<&Vec<f64>> = points.iter().zip(&of).filter(|(_, o)| **o == c).map(|(p, _)| p).collect();
                if !members.is_empty() {
                    *centre = (0..dim).map(|j| members.iter().map(|p| p[j]).sum::<f64>() / members.len() as f64).collect();
                }
            }
            if same {
                break;
            }
        }
        let cost: f64 = points.iter().zip(&of).map(|(p, o)| far(p, &centres[*o])).sum();
        if cost < best.1 {
            best = (of, cost);
        }
    }
    best
}

/// How far apart the two nearest groups are: the distance of their centres
/// in units of the points' spread about their own centres along the line
/// between them. None when a group is too small to be a kind.
fn apart(points: &[Vec<f64>], of: &[usize], k: usize) -> Option<f64> {
    let m = points.len();
    let dim = points[0].len();
    let mut centres = vec![vec![0f64; dim]; k];
    let mut count = vec![0usize; k];
    for (p, o) in points.iter().zip(of) {
        count[*o] += 1;
        centres[*o].iter_mut().zip(p).for_each(|(c, v)| *c += v);
    }
    if count.iter().any(|c| *c < FEWEST.max((SHARE * m as f64).ceil() as usize)) {
        return None;
    }
    for (c, n) in centres.iter_mut().zip(&count) {
        c.iter_mut().for_each(|v| *v /= *n as f64);
    }
    let mut least = f64::MAX;
    for a in 0..k {
        for b in a + 1..k {
            let line: Vec<f64> = centres[a].iter().zip(&centres[b]).map(|(x, y)| y - x).collect();
            let length = line.iter().map(|x| x * x).sum::<f64>().sqrt();
            if length <= 0.0 {
                return None;
            }
            let along = |p: &Vec<f64>, c: &Vec<f64>| p.iter().zip(c).zip(&line).map(|((x, y), l)| (x - y) * l).sum::<f64>() / length;
            let (mut sq, mut n) = (0f64, 0usize);
            for (p, o) in points.iter().zip(of) {
                if *o == a || *o == b {
                    sq += along(p, &centres[*o]).powi(2);
                    n += 1;
                }
            }
            let spread = (sq / n as f64).sqrt();
            least = least.min(if spread > 0.0 { length / spread } else { f64::MAX });
        }
    }
    Some(least)
}

/// The kinds among the particles of `set` as they lie at `poses` on their
/// average, compared to the detail of `shell` cycles across the box (where
/// the average's halves still agree). None when they are all one kind, as
/// far as can be told.
pub fn kinds(set: &Set, poses: &[Pose], shell: f64, progress: Progress) -> Result<Option<Kinds>> {
    set.check()?;
    let m = poses.len();
    if m < 4 * FEWEST {
        return Ok(None);
    }
    let average = set.average(poses, |_| true).finish();
    check_cancel()?;
    let band = shell.clamp(3.0, BAND.min(set.n as f64 / 2.0 - 2.0));
    let d = differences(set, poses, &average, band, progress);
    check_cancel()?;
    let mut rng = Rng(17);
    let (places, spread) = directions(&d, DIRECTIONS, &mut rng);
    drop(d);
    // The directions that stand out of the rest (the later half).
    let mut rest: Vec<f64> = spread[spread.len() / 2..].to_vec();
    rest.sort_by(f64::total_cmp);
    let usual = rest[rest.len() / 2];
    let clear = spread.iter().take_while(|s| **s > STANDS_OUT * usual).count().min(MOST - 1);
    if clear == 0 {
        return Ok(None);
    }
    let points: Vec<Vec<f64>> = places.iter().map(|p| p[..clear].to_vec()).collect();
    // As many kinds as still stand apart, the most first.
    let mut found: Option<(Vec<usize>, usize, f64)> = None;
    for k in (2..=(clear + 1).min(MOST)).rev() {
        let (of, _) = groups(&points, k, &mut rng);
        if let Some(a) = apart(&points, &of, k).filter(|a| *a >= APART) {
            found = Some((of, k, a));
            break;
        }
    }
    let Some((of, k, apart)) = found else { return Ok(None) };
    let mut kinds: Vec<Kind> = (0..k)
        .map(|c| {
            let members: Vec<usize> = (0..m).filter(|i| of[*i] == c).collect();
            let map = set.average(poses, |i| of[i] == c).finish();
            Kind { members, map }
        })
        .collect();
    kinds.sort_by(|a, b| b.members.len().cmp(&a.members.len()));
    Ok(Some(Kinds { kinds, apart }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spa::geom::axis_angle;
    use crate::sta::cc::wedge;
    use crate::sta::tests::{particle, phantom};

    const N: usize = 32;

    /// `count` particles at random poses under a wedge and noise; every
    /// third (with `two`) has a partner bound at one side.
    fn made(count: usize, two: bool) -> (Vec<Vec<f32>>, Vec<Pose>, Vec<bool>, Vec<f32>) {
        let plain = phantom();
        let c = (N / 2) as f64;
        let bound: Vec<f32> = plain
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let (x, y, z) = ((i % N) as f64 - c, (i / N % N) as f64 - c, (i / N / N) as f64 - c);
                v + 1.2 * (-((x - 5.0).powi(2) + (y + 4.0).powi(2) + (z + 3.0).powi(2)) / (2.0 * 2.5 * 2.5)).exp() as f32
            })
            .collect();
        let w = wedge(N, -60.0, 60.0);
        let mut rng = Rng(23);
        let (mut particles, mut poses, mut with) = (vec![], vec![], vec![]);
        for i in 0..count {
            let has = two && i % 3 == 0;
            let r = axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 360.0 * rng.unit());
            let t = [rng.gauss(), rng.gauss(), rng.gauss()];
            particles.push(particle(if has { &bound } else { &plain }, &r, t, &w, 1.0, &mut rng));
            poses.push(Pose { r, t, score: 0.0 });
            with.push(has);
        }
        (particles, poses, with, w)
    }

    #[test]
    fn particles_with_and_without_a_partner_are_two_kinds() {
        let (particles, poses, with, w) = made(150, true);
        let wedges = vec![w];
        let wedge_of = vec![0; particles.len()];
        let set = Set { n: N, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius: 10.0 };
        let found = kinds(&set, &poses, 8.0, &|_, _, _| {}).unwrap().expect("two kinds");
        assert_eq!(found.kinds.len(), 2, "apart {:.1}", found.apart);
        // The larger kind is the plain one.
        let right = found.kinds[0].members.iter().filter(|i| !with[**i]).count() + found.kinds[1].members.iter().filter(|i| with[**i]).count();
        assert!(right >= 140, "{right} of 150 with their own kind (apart {:.1})", found.apart);
    }

    #[test]
    fn particles_that_are_all_the_same_are_one_kind() {
        let (particles, poses, _, w) = made(150, false);
        let wedges = vec![w];
        let wedge_of = vec![0; particles.len()];
        let set = Set { n: N, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius: 10.0 };
        let found = kinds(&set, &poses, 8.0, &|_, _, _| {}).unwrap();
        assert!(found.is_none(), "{} kinds, apart {:.1}", found.as_ref().map_or(0, |f| f.kinds.len()), found.as_ref().map_or(0.0, |f| f.apart));
    }
}
