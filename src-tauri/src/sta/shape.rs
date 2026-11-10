//! Shapes the user draws on a tomogram to say what to average: a few
//! clicks define a line (a filament), a tube, a sphere (a vesicle) or a
//! surface (a membrane), and the shape gives the places to cut cubes at,
//! each with the one direction the shape knows (along the filament, or out
//! of the surface). The idea of geometric models that turn clicks into
//! cutting points and first orientations is Dynamo's (Castaño-Díez et al.,
//! J Struct Biol 197, 2017); here the spacing is not asked for: shapes are
//! sampled densely and `merge` drops the copies that land on one particle
//! after alignment.
//!
//! Positions are in voxels, (x, y, z). The known direction is where the
//! reference's Z axis points in the tomogram, so a pose that respects it is
//! `frame(axis)` turned about Z by an unknown angle (`turns`).

use crate::spa::geom::{axis_angle, mul, Mat};

type P = [f64; 3];

#[derive(Clone, Copy, Debug)]
pub struct Seed {
    pub pos: P,
    /// Unit vector: the filament axis or the outward surface normal.
    pub axis: P,
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: P, s: f64) -> P {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P, b: P) -> P {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: P) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: P) -> P {
    let n = norm(a);
    if n > 1e-12 {
        scale(a, 1.0 / n)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// A unit vector at right angles to `a`.
fn side(a: P) -> P {
    let other = if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    unit(cross(a, other))
}

/// A rotation whose third column is `axis`: it carries the reference's Z
/// axis onto `axis`.
pub fn frame(axis: P) -> Mat {
    let z = unit(axis);
    let x = side(z);
    let y = cross(z, x);
    [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]]
}

/// The rotations a search has to try for a seed: every turn about the known
/// axis in steps of `step` degrees. With `both`, also the axis reversed
/// (a filament's polarity is not known from its drawing).
pub fn turns(axis: P, step: f64, both: bool) -> Vec<Mat> {
    let n = (360.0 / step).round().max(1.0) as usize;
    let mut out = vec![];
    for a in if both { vec![axis, scale(axis, -1.0)] } else { vec![axis] } {
        let f = frame(a);
        for i in 0..n {
            out.push(mul(&f, &axis_angle([0.0, 0.0, 1.0], i as f64 * 360.0 / n as f64)));
        }
    }
    out
}

/// A smooth curve through the clicked points (centripetal Catmull-Rom,
/// which does not overshoot between unevenly spaced clicks), as a dense
/// polyline.
fn curve(clicks: &[P]) -> Vec<P> {
    if clicks.len() < 3 {
        return clicks.to_vec();
    }
    let n = clicks.len();
    let at = |i: isize| -> P {
        // Ends are continued in a straight line.
        if i < 0 {
            sub(scale(clicks[0], 2.0), clicks[1])
        } else if i as usize >= n {
            sub(scale(clicks[n - 1], 2.0), clicks[n - 2])
        } else {
            clicks[i as usize]
        }
    };
    let mut out = vec![];
    for s in 0..n - 1 {
        let p = [at(s as isize - 1), at(s as isize), at(s as isize + 1), at(s as isize + 2)];
        let mut t = [0.0; 4];
        for i in 1..4 {
            t[i] = t[i - 1] + norm(sub(p[i], p[i - 1])).sqrt().max(1e-6);
        }
        let lerp = |a: P, b: P, ta: f64, tb: f64, u: f64| add(scale(a, (tb - u) / (tb - ta)), scale(b, (u - ta) / (tb - ta)));
        let steps = (norm(sub(p[2], p[1])).ceil() as usize * 2).max(2);
        for k in 0..steps {
            let u = t[1] + (t[2] - t[1]) * k as f64 / steps as f64;
            let a1 = lerp(p[0], p[1], t[0], t[1], u);
            let a2 = lerp(p[1], p[2], t[1], t[2], u);
            let a3 = lerp(p[2], p[3], t[2], t[3], u);
            let b1 = lerp(a1, a2, t[0], t[2], u);
            let b2 = lerp(a2, a3, t[1], t[3], u);
            out.push(lerp(b1, b2, t[1], t[2], u));
        }
    }
    out.push(clicks[n - 1]);
    out
}

/// Points every `spacing` along a polyline, with the direction of travel.
fn walk(line: &[P], spacing: f64) -> Vec<Seed> {
    let mut out = vec![];
    if line.len() < 2 {
        return out;
    }
    let mut next = 0.0;
    let mut gone = 0.0;
    for w in line.windows(2) {
        let d = sub(w[1], w[0]);
        let len = norm(d);
        if len < 1e-9 {
            continue;
        }
        while next <= gone + len {
            out.push(Seed { pos: add(w[0], scale(d, (next - gone) / len)), axis: unit(d) });
            next += spacing;
        }
        gone += len;
    }
    out
}

/// A filament: seeds every `spacing` voxels along the smooth curve through
/// the clicks, with the curve's direction as the axis.
pub fn line(clicks: &[P], spacing: f64) -> Vec<Seed> {
    walk(&curve(clicks), spacing)
}

/// A tube (a microtubule, a tubular membrane): seeds on the wall at `radius`
/// from the curve through the clicks, about `spacing` apart, facing outward.
pub fn tube(clicks: &[P], radius: f64, spacing: f64) -> Vec<Seed> {
    let around = ((2.0 * std::f64::consts::PI * radius / spacing).round() as usize).max(3);
    let mut out = vec![];
    let mut x = None;
    for s in walk(&curve(clicks), spacing) {
        // Carry the side vector along the curve so rings do not twist.
        let prev: P = x.unwrap_or_else(|| side(s.axis));
        let u = unit(sub(prev, scale(s.axis, dot(prev, s.axis))));
        let v = cross(s.axis, u);
        x = Some(u);
        for i in 0..around {
            let a = 2.0 * std::f64::consts::PI * i as f64 / around as f64;
            let n = add(scale(u, a.cos()), scale(v, a.sin()));
            out.push(Seed { pos: add(s.pos, scale(n, radius)), axis: n });
        }
    }
    out
}

/// A vesicle: seeds spread evenly over an ellipsoid with the given half
/// axes along x, y, z (a sphere when equal), about `spacing` apart, facing
/// outward (Fibonacci spiral).
pub fn ellipsoid(centre: P, radii: P, spacing: f64) -> Vec<Seed> {
    let r = (radii[0] * radii[1] * radii[2]).cbrt();
    let count = ((4.0 * std::f64::consts::PI * r * r / (spacing * spacing)).round() as usize).max(4);
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..count)
        .map(|i| {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / count as f64;
            let s = (1.0 - z * z).sqrt();
            let a = golden * i as f64;
            let d = [s * a.cos(), s * a.sin(), z];
            let pos = [centre[0] + radii[0] * d[0], centre[1] + radii[1] * d[1], centre[2] + radii[2] * d[2]];
            // The normal of x²/a² + y²/b² + z²/c² = 1.
            let axis = unit([d[0] / radii[0], d[1] / radii[1], d[2] / radii[2]]);
            Seed { pos, axis }
        })
        .collect()
}

/// The sphere that best fits points on its surface (algebraic least
/// squares): centre and radius. None when the points do not fix one.
pub fn fit_sphere(points: &[P]) -> Option<(P, f64)> {
    if points.len() < 4 {
        return None;
    }
    // |p|² = 2 p·c + (r² − |c|²): linear in (c, k).
    let mut m = [[0.0f64; 5]; 4];
    for p in points {
        let row = [2.0 * p[0], 2.0 * p[1], 2.0 * p[2], 1.0];
        let rhs = dot(*p, *p);
        for i in 0..4 {
            for j in 0..4 {
                m[i][j] += row[i] * row[j];
            }
            m[i][4] += row[i] * rhs;
        }
    }
    for i in 0..4 {
        let pivot = (i..4).max_by(|a, b| m[*a][i].abs().total_cmp(&m[*b][i].abs()))?;
        m.swap(i, pivot);
        if m[i][i].abs() < 1e-9 {
            return None;
        }
        for r in 0..4 {
            if r != i {
                let f = m[r][i] / m[i][i];
                for c in i..5 {
                    m[r][c] -= f * m[i][c];
                }
            }
        }
    }
    let c = [m[0][4] / m[0][0], m[1][4] / m[1][1], m[2][4] / m[2][2]];
    let r2 = m[3][4] / m[3][3] + dot(c, c);
    (r2 > 0.0).then(|| (c, r2.sqrt()))
}

/// A membrane of any form: the user draws a stroke along it on a few
/// sections; the surface runs through the strokes in the order given.
/// Seeds lie about `spacing` apart on it. Normals all point to one side of
/// the surface; `inside` (a point on the side they should face away from,
/// such as the inside of a cell) picks which.
pub fn surface(strokes: &[Vec<P>], spacing: f64, inside: Option<P>) -> Vec<Seed> {
    let lines: Vec<Vec<P>> = strokes.iter().filter(|s| s.len() >= 2).map(|s| curve(s)).collect();
    if lines.is_empty() {
        return vec![];
    }
    let length = |l: &[P]| l.windows(2).map(|w| norm(sub(w[1], w[0]))).sum::<f64>();
    let longest = lines.iter().map(|l| length(l)).fold(0.0, f64::max);
    let across = ((longest / spacing).round() as usize).max(1) + 1;
    // Each stroke as the same number of points, evenly along its length.
    let mut rows: Vec<Vec<P>> = lines
        .iter()
        .map(|l| {
            let step = length(l) / (across - 1) as f64;
            let mut r: Vec<P> = walk(l, step.max(1e-9)).into_iter().map(|s| s.pos).collect();
            r.resize(across, *l.last().unwrap());
            r
        })
        .collect();
    // Strokes drawn in opposite directions would cross the surface over.
    for i in 1..rows.len() {
        let same: f64 = norm(sub(rows[i][0], rows[i - 1][0])) + norm(sub(rows[i][across - 1], rows[i - 1][across - 1]));
        let flipped: f64 = norm(sub(rows[i][across - 1], rows[i - 1][0])) + norm(sub(rows[i][0], rows[i - 1][across - 1]));
        if flipped < same {
            rows[i].reverse();
        }
    }
    // A grid: strokes one way, a smooth curve through matching points the other.
    let columns: Vec<Vec<P>> = (0..across)
        .map(|j| {
            let through: Vec<P> = rows.iter().map(|r| r[j]).collect();
            if through.len() == 1 {
                through
            } else {
                walk(&curve(&through), spacing).into_iter().map(|s| s.pos).collect()
            }
        })
        .collect();
    let down = columns.iter().map(Vec::len).min().unwrap_or(0);
    let mut out = vec![];
    for j in 0..across {
        for i in 0..down {
            let p = columns[j][i];
            let u = sub(columns[(j + 1).min(across - 1)][i], columns[j.saturating_sub(1)][i]);
            let v = if down > 1 {
                sub(columns[j][(i + 1).min(down - 1)], columns[j][i.saturating_sub(1)])
            } else {
                // One stroke only: the surface is taken to run along the beam.
                [0.0, 0.0, 1.0]
            };
            let mut n = unit(cross(u, v));
            if let Some(q) = inside {
                if dot(n, sub(p, q)) < 0.0 {
                    n = scale(n, -1.0);
                }
            }
            out.push(Seed { pos: p, axis: n });
        }
    }
    if inside.is_none() {
        // One side throughout: follow the first normal.
        for k in 1..out.len() {
            let near = if k % down.max(1) == 0 { k - down.max(1) } else { k - 1 };
            if dot(out[k].axis, out[near].axis) < 0.0 {
                out[k].axis = scale(out[k].axis, -1.0);
            }
        }
    }
    out
}

/// After alignment, seeds sampled more densely than the particles sit have
/// run together: of positions closer than `distance`, keep the one with the
/// best score. Returns the indices kept.
pub fn merge(positions: &[P], scores: &[f32], distance: f64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..positions.len()).collect();
    order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    let cell = |p: P| [(p[0] / distance).floor() as i64, (p[1] / distance).floor() as i64, (p[2] / distance).floor() as i64];
    let mut grid: std::collections::HashMap<[i64; 3], Vec<usize>> = Default::default();
    let mut kept = vec![];
    for i in order {
        let c = cell(positions[i]);
        let mut clash = false;
        'look: for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(list) = grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                        if list.iter().any(|k| norm(sub(positions[*k], positions[i])) < distance) {
                            clash = true;
                            break 'look;
                        }
                    }
                }
            }
        }
        if !clash {
            grid.entry(c).or_default().push(i);
            kept.push(i);
        }
    }
    kept.sort_unstable();
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spa::geom::apply;

    #[test]
    fn frame_and_turns_keep_the_axis() {
        let axis = unit([0.3, -0.8, 0.5]);
        for r in turns(axis, 30.0, false) {
            let z = apply(&r, [0.0, 0.0, 1.0]);
            assert!(norm(sub(z, axis)) < 1e-9);
            // A proper rotation.
            let x = apply(&r, [1.0, 0.0, 0.0]);
            let y = apply(&r, [0.0, 1.0, 0.0]);
            assert!(norm(sub(cross(x, y), z)) < 1e-9);
        }
        assert_eq!(turns(axis, 30.0, true).len(), 24);
    }

    #[test]
    fn line_is_evenly_spaced_and_passes_the_clicks() {
        let clicks = [[0.0, 0.0, 0.0], [30.0, 10.0, 0.0], [60.0, 0.0, 5.0], [100.0, -20.0, 5.0]];
        let seeds = line(&clicks, 4.0);
        assert!(seeds.len() > 20);
        for w in seeds.windows(2) {
            let d = norm(sub(w[1].pos, w[0].pos));
            assert!((d - 4.0).abs() < 0.2, "{d}");
            assert!(dot(w[0].axis, w[1].axis) > 0.9);
        }
        for c in clicks {
            let near = line(&clicks, 0.5).iter().map(|s| norm(sub(s.pos, c))).fold(f64::MAX, f64::min);
            assert!(near < 0.5, "{near}");
        }
    }

    #[test]
    fn tube_wall_faces_outward() {
        let seeds = tube(&[[0.0, 0.0, 0.0], [0.0, 50.0, 0.0], [10.0, 100.0, 0.0]], 12.0, 4.0);
        let axis = line(&[[0.0, 0.0, 0.0], [0.0, 50.0, 0.0], [10.0, 100.0, 0.0]], 0.5);
        for s in &seeds {
            let (d, a) = axis.iter().map(|a| (norm(sub(s.pos, a.pos)), a)).min_by(|x, y| x.0.total_cmp(&y.0)).unwrap();
            assert!((d - 12.0).abs() < 0.3, "{d}");
            assert!(dot(s.axis, a.axis).abs() < 0.05);
            assert!(dot(s.axis, sub(s.pos, a.pos)) > 0.0);
        }
    }

    #[test]
    fn sphere_is_covered_and_fitted() {
        let centre = [40.0, 50.0, 60.0];
        let seeds = ellipsoid(centre, [30.0; 3], 6.0);
        let expect = 4.0 * std::f64::consts::PI * 900.0 / 36.0;
        assert!((seeds.len() as f64 - expect).abs() < 2.0);
        for s in &seeds {
            assert!((norm(sub(s.pos, centre)) - 30.0).abs() < 1e-9);
            assert!(norm(sub(s.axis, unit(sub(s.pos, centre)))) < 1e-9);
        }
        // Even: every seed has a neighbour near the spacing asked for.
        for s in &seeds {
            let d = seeds.iter().map(|o| norm(sub(o.pos, s.pos))).filter(|d| *d > 0.0).fold(f64::MAX, f64::min);
            assert!(d > 3.5 && d < 8.0, "{d}");
        }
        let points: Vec<P> = seeds.iter().take(40).map(|s| s.pos).collect();
        let (c, r) = fit_sphere(&points).unwrap();
        assert!(norm(sub(c, centre)) < 1e-6 && (r - 30.0).abs() < 1e-6);
        assert!(fit_sphere(&[[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]]).is_none());
    }

    #[test]
    fn surface_through_strokes_on_a_cylinder() {
        // Half a cylinder of radius 40 about the z axis, drawn on three
        // sections, the middle one in the opposite direction.
        let arc = |z: f64, back: bool| -> Vec<P> {
            let mut a: Vec<P> = (0..=6).map(|i| (i as f64 * 30.0).to_radians()).map(|t| [40.0 * t.cos(), 40.0 * t.sin(), z]).collect();
            if back {
                a.reverse();
            }
            a
        };
        let seeds = surface(&[arc(0.0, false), arc(20.0, true), arc(40.0, false)], 5.0, Some([0.0, 10.0, 20.0]));
        assert!(seeds.len() > 100, "{}", seeds.len());
        for s in &seeds {
            let r = (s.pos[0].powi(2) + s.pos[1].powi(2)).sqrt();
            assert!((r - 40.0).abs() < 1.0, "{r}");
            let out = unit([s.pos[0], s.pos[1], 0.0]);
            assert!(dot(s.axis, out) > 0.97, "{:?} {:?}", s.axis, out);
        }
        // Without a hint all normals still face one side.
        let free = surface(&[arc(0.0, false), arc(40.0, false)], 5.0, None);
        let first = dot(free[0].axis, unit([free[0].pos[0], free[0].pos[1], 0.0])).signum();
        assert!(free.iter().all(|s| dot(s.axis, unit([s.pos[0], s.pos[1], 0.0])) * first > 0.9));
    }

    #[test]
    fn merge_keeps_the_best_of_each_cluster() {
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.5, 0.5, 0.0], [30.0, 0.0, 0.0]];
        let scores = [0.2, 0.9, 0.5, 0.1, 0.3];
        assert_eq!(merge(&positions, &scores, 3.0), vec![1, 2, 4]);
    }
}
