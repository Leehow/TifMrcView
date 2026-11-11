//! "Only on the membranes I marked": when the user marks a surface (a
//! painted membrane, a vesicle, a filament) and also gives examples of a
//! particle on it, the particles looked for are those that sit on such
//! shapes and stand on them the way the others do. This is Dynamo's
//! template matching "with geometry", without anything to set up: the
//! places a template search found (`project::Found`) are kept when they
//! lie near a shape and are turned consistently with its normal there.
//!
//! How the particle stands on the shape is not known beforehand (a card is
//! turned any way). It is read from the search itself: a true particle
//! found at orientation R on a shape with normal n has the normal at Rᵀ·n
//! in the template's own frame, the same direction for every true
//! particle, while false peaks point anywhere. The direction most places
//! agree on is taken (`stand`), and places that disagree are dropped.

use super::shape::Seed;
use crate::spa::geom::{apply, transpose, Mat};

type P = [f64; 3];

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn dist(a: P, b: P) -> f64 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt()
}
fn unit(a: P) -> P {
    let n = dot(a, a).sqrt();
    if n > 1e-12 {
        a.map(|x| x / n)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Places agree on a direction when they are within this of it, degrees
/// (the search turns in steps of 30°).
pub const CONE: f64 = 35.0;
/// Fewer places than this cannot say how the particle stands.
const FEW: usize = 8;
/// The direction must hold this many standard deviations more places than
/// chance would give it (the best of many directions is asked, so chance
/// alone reaches two or three), or the particle is taken to stand any way.
const OVER_CHANCE: f64 = 4.0;

/// For each position the nearest seed within `reach` (same units as the
/// positions).
pub fn nearest(positions: &[P], seeds: &[Seed], reach: f64) -> Vec<Option<usize>> {
    use std::collections::HashMap;
    let side = reach.max(1e-6);
    let key = |p: P| -> [i64; 3] { std::array::from_fn(|k| (p[k] / side).floor() as i64) };
    let mut cells: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (i, s) in seeds.iter().enumerate() {
        cells.entry(key(s.pos)).or_default().push(i);
    }
    positions
        .iter()
        .map(|&p| {
            let c = key(p);
            let mut best: Option<(f64, usize)> = None;
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        for &i in cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]).into_iter().flatten() {
                            let d = dist(p, seeds[i].pos);
                            if d <= reach && best.is_none_or(|b| d < b.0) {
                                best = Some((d, i));
                            }
                        }
                    }
                }
            }
            best.map(|b| b.1)
        })
        .collect()
}

/// How the particle stands on the shapes: the direction of the template's
/// own frame that lies along the normal, read from places found on shapes
/// (`turn` of each and the normal of the shape there; `both`: the sense of
/// the normals is unknown, as on a filament, so a direction and its
/// opposite count as one). None when the places do not agree on one.
pub fn stand(pairs: &[(Mat, P)], both: bool) -> Option<P> {
    if pairs.len() < FEW {
        return None;
    }
    let seen: Vec<P> = pairs.iter().map(|(r, n)| unit(apply(&transpose(r), *n))).collect();
    let near = CONE.to_radians().cos();
    let with = |a: P, b: P| {
        let d = dot(a, b);
        if both { d.abs() } else { d }
    };
    // The direction with the most others around it.
    let (mut best, mut most) = (0, 0);
    for (i, &a) in seen.iter().enumerate() {
        let k = seen.iter().filter(|&&b| with(a, b) >= near).count();
        if k > most {
            (best, most) = (i, k);
        }
    }
    // Chance: the cap of the cone as a share of the sphere (two caps when
    // the sense is unknown).
    let cap = (1.0 - near) / 2.0 * if both { 2.0 } else { 1.0 };
    let chance = cap * seen.len() as f64;
    if (most as f64) < chance + OVER_CHANCE * chance.sqrt() || most < FEW / 2 {
        return None;
    }
    // Its middle: the mean of those around it.
    let a = seen[best];
    let mut m = [0.0; 3];
    for &b in seen.iter().filter(|&&b| with(a, b) >= near) {
        let s = if dot(a, b) < 0.0 { -1.0 } else { 1.0 };
        for k in 0..3 {
            m[k] += s * b[k];
        }
    }
    Some(unit(m))
}

/// What `on_shapes` says of the places.
pub struct OnShapes {
    /// Per place: on a shape, and standing on it as the others do.
    pub keep: Vec<bool>,
    /// How the particle stands (see `stand`); None: any way, so only the
    /// nearness decided.
    pub stand: Option<P>,
    /// Places near a shape (before the standing was asked).
    pub near: usize,
}

/// Which of the places (position and the orientation found, in the units of
/// the seeds) lie within `reach` of the shapes' `seeds` and stand on them
/// as most do. `both`: the sense of the seeds' axes is unknown.
pub fn on_shapes(places: &[(P, Mat)], seeds: &[Seed], reach: f64, both: bool) -> OnShapes {
    let positions: Vec<P> = places.iter().map(|p| p.0).collect();
    let at = nearest(&positions, seeds, reach);
    let pairs: Vec<(Mat, P)> = places.iter().zip(&at).filter_map(|(p, s)| s.map(|s| (p.1, seeds[s].axis))).collect();
    let near = pairs.len();
    let stand = stand(&pairs, both);
    let cone = CONE.to_radians().cos();
    let keep = places
        .iter()
        .zip(&at)
        .map(|(p, s)| {
            let Some(s) = s else { return false };
            let Some(a) = stand else { return true };
            let d = dot(unit(apply(&transpose(&p.1), seeds[*s].axis)), a);
            (if both { d.abs() } else { d }) >= cone
        })
        .collect();
    OnShapes { keep, stand, near }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spa::geom::{axis_angle, grid, mul};

    /// A rotation that carries `a` onto `b` (unit vectors).
    fn onto(a: P, b: P) -> Mat {
        let c = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let s = dot(c, c).sqrt();
        if s < 1e-9 {
            return if dot(a, b) > 0.0 { crate::sta::IDENTITY } else { axis_angle([1.0, 0.0, 0.0], 180.0) };
        }
        axis_angle(unit(c), s.atan2(dot(a, b)).to_degrees())
    }

    /// The grid orientation nearest to `r`.
    fn snapped(r: &Mat, turns: &[Mat]) -> Mat {
        let trace = |m: &Mat| m[0][0] + m[1][1] + m[2][2];
        *turns.iter().max_by(|a, b| trace(&mul(&transpose(a), r)).total_cmp(&trace(&mul(&transpose(b), r)))).unwrap()
    }

    #[test]
    fn a_turned_template_carries_its_directions_with_it() {
        // The convention `stand` relies on: the template turned by R shows
        // its direction a at R·a.
        let n = 16;
        let mut cube = vec![0f32; n * n * n];
        // A mark 4 voxels along +x from the centre.
        cube[(8 * n + 8) * n + 12] = 1.0;
        let r = axis_angle([0.0, 0.0, 1.0], 90.0);
        let turned = crate::sta::cc::rotate(&cube, n, &r);
        let at = apply(&r, [4.0, 0.0, 0.0]);
        let i = ((8.0 + at[2]).round() as usize * n + (8.0 + at[1]).round() as usize) * n + (8.0 + at[0]).round() as usize;
        let brightest = turned.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(brightest, i);
    }

    #[test]
    fn particles_on_a_sphere_are_kept_and_strays_dropped() {
        // Seeds on a sphere of radius 40 about (100, 100, 100), normals out.
        let centre = [100.0, 100.0, 100.0];
        let seeds = crate::sta::shape::ellipsoid(centre, [40.0; 3], 8.0);
        let turns = grid(30.0);
        // The particle stands with its own (1, 1, 0)/√2 along the normal.
        let a = unit([1.0, 1.0, 0.0]);
        let mut places: Vec<(P, Mat)> = vec![];
        let mut seed = 3u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        // 60 true particles on the sphere, spun about the normal at random
        // and found at the nearest orientation of the search grid.
        for s in seeds.iter().step_by(seeds.len() / 60).take(60) {
            let spin = axis_angle(s.axis, 360.0 * rnd());
            let r = mul(&spin, &onto(a, s.axis));
            places.push((std::array::from_fn(|k| s.pos[k] + 2.0 * (rnd() - 0.5)), snapped(&r, &turns)));
        }
        // 30 false peaks on the sphere at random orientations, 30 far off.
        for s in seeds.iter().skip(3).step_by(seeds.len() / 30).take(30) {
            places.push((s.pos, turns[(rnd() * turns.len() as f64) as usize]));
        }
        for _ in 0..30 {
            places.push(([10.0 + 20.0 * rnd(), 10.0 + 20.0 * rnd(), 180.0], turns[(rnd() * turns.len() as f64) as usize]));
        }
        let got = on_shapes(&places, &seeds, 6.0, false);
        assert_eq!(got.near, 90);
        let stand = got.stand.expect("the particles agree on how they stand");
        assert!(dot(stand, a) > 0.95, "{stand:?}");
        let kept = |range: std::ops::Range<usize>| got.keep[range].iter().filter(|&&k| k).count();
        assert!(kept(0..60) >= 55, "true kept {}", kept(0..60));
        assert!(kept(60..90) <= 8, "false on the sphere kept {}", kept(60..90));
        assert_eq!(kept(90..120), 0);
    }

    #[test]
    fn with_no_common_way_of_standing_only_nearness_decides() {
        let seeds = crate::sta::shape::ellipsoid([50.0; 3], [20.0; 3], 6.0);
        let turns = grid(30.0);
        let mut seed = 11u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let places: Vec<(P, Mat)> = seeds.iter().step_by(3).map(|s| (s.pos, turns[(rnd() * turns.len() as f64) as usize])).collect();
        let got = on_shapes(&places, &seeds, 3.0, false);
        assert!(got.stand.is_none());
        assert!(got.keep.iter().all(|&k| k));
    }
}
