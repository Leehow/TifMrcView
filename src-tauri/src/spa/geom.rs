//! Orientations: rotation matrices from ZYZ Euler angles in the convention
//! of RELION's STAR files (rot, tilt, psi), so exported angles mean the same
//! thing there; uniform orientation grids; point-group symmetry.
//!
//! An orientation R maps volume coordinates to the particle frame: the
//! particle is the projection along z of the volume rotated by R, so by the
//! central slice theorem its transform at (kx, ky) is the volume's at Rᵀ·(kx, ky, 0).

use std::f64::consts::PI;

pub type Mat = [[f64; 3]; 3];

pub fn euler(rot: f64, tilt: f64, psi: f64) -> Mat {
    let (ca, sa) = (rot.to_radians().cos(), rot.to_radians().sin());
    let (cb, sb) = (tilt.to_radians().cos(), tilt.to_radians().sin());
    let (cg, sg) = (psi.to_radians().cos(), psi.to_radians().sin());
    let (cc, cs, sc, ss) = (cb * ca, cb * sa, sb * ca, sb * sa);
    [
        [cg * cc - sg * sa, cg * cs + sg * ca, -cg * sb],
        [-sg * cc - cg * sa, -sg * cs + cg * ca, sg * sb],
        [sc, ss, cb],
    ]
}

/// (rot, tilt, psi) in degrees of a rotation matrix.
pub fn angles(a: &Mat) -> [f64; 3] {
    let sb = (a[0][2] * a[0][2] + a[1][2] * a[1][2]).sqrt();
    if sb > 1e-6 {
        let psi = a[1][2].atan2(-a[0][2]);
        let rot = a[2][1].atan2(a[2][0]);
        let tilt = sb.atan2(a[2][2]);
        [rot.to_degrees(), tilt.to_degrees(), psi.to_degrees()]
    } else {
        let tilt = if a[2][2] > 0.0 { 0.0 } else { 180.0 };
        let psi = (-a[1][0]).atan2(a[0][0]);
        // With tilt 0 or 180 only rot ± psi is defined; put it all in psi.
        let psi = if a[2][2] > 0.0 { psi } else { -psi };
        [0.0, tilt, psi.to_degrees()]
    }
}

pub fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut c = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            c[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    c
}

pub fn transpose(a: &Mat) -> Mat {
    let mut t = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            t[i][j] = a[j][i];
        }
    }
    t
}

pub const IDENTITY: Mat = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Rotation by `deg` about the unit axis `u` (Rodrigues).
pub fn axis_angle(u: [f64; 3], deg: f64) -> Mat {
    let n = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
    let [x, y, z] = [u[0] / n, u[1] / n, u[2] / n];
    let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
    let t = 1.0 - c;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

/// Angle (degrees) of the rotation between two orientations.
pub fn distance(a: &Mat, b: &Mat) -> f64 {
    let m = mul(a, &transpose(b));
    let tr = (m[0][0] + m[1][1] + m[2][2] - 1.0) / 2.0;
    tr.clamp(-1.0, 1.0).acos().to_degrees()
}

/// Projection directions spread evenly over the sphere (a Fibonacci
/// lattice) about `step` degrees apart, as (rot, tilt).
pub fn directions(step: f64) -> Vec<[f64; 2]> {
    let n = ((4.0 * PI) / step.to_radians().powi(2)).round().max(2.0) as usize;
    let golden = PI * (3.0 - 5f64.sqrt());
    (0..n)
        .map(|i| {
            let z = 1.0 - (2.0 * i as f64 + 1.0) / n as f64;
            let tilt = z.clamp(-1.0, 1.0).acos().to_degrees();
            let rot = (golden * i as f64).rem_euclid(2.0 * PI).to_degrees();
            [rot, tilt]
        })
        .collect()
}

/// Every orientation of a global search: directions × in-plane angles.
pub fn grid(step: f64) -> Vec<Mat> {
    let psis = (360.0 / step).round().max(1.0) as usize;
    directions(step)
        .into_iter()
        .flat_map(|[rot, tilt]| {
            (0..psis).map(move |j| euler(rot, tilt, j as f64 * 360.0 / psis as f64))
        })
        .collect()
}

/// `grid(step)` without orientations that a point group repeats: for Cn
/// and Dn (n-fold along z, 2-fold along x) only views whose direction
/// (third row of R; a mate R·g has gᵀ·d) lies in the sector 0 ≤ φ < 360°/n,
/// and for Dn the upper half. 2n times fewer for Dn; other groups whole.
pub fn grid_for(step: f64, symmetry: &str) -> Vec<Mat> {
    let s = symmetry.trim().to_uppercase();
    let n: usize = s.get(1..).and_then(|v| v.parse().ok()).unwrap_or(1);
    let (cyclic, dihedral) = (s.starts_with('C'), s.starts_with('D'));
    if n <= 1 && !dihedral || !(cyclic || dihedral) {
        return grid(step);
    }
    let sector = 360.0 / n as f64;
    grid(step)
        .into_iter()
        .filter(|r| {
            let d = r[2];
            let phi = d[1].atan2(d[0]).to_degrees().rem_euclid(360.0);
            phi < sector - 1e-6 && (!dihedral || d[2] > -1e-9)
        })
        .collect()
}

/// Small rotations within ±`range` degrees about each axis, `steps` per
/// side, for a local search around a known orientation.
pub fn local(range: f64, steps: usize) -> Vec<Mat> {
    let n = steps as i32;
    let d = range / steps.max(1) as f64;
    let mut out = vec![];
    for i in -n..=n {
        for j in -n..=n {
            for k in -n..=n {
                let (a, b, c) = (i as f64 * d, j as f64 * d, k as f64 * d);
                if (a * a + b * b + c * c).sqrt() > range * 1.01 {
                    continue;
                }
                let m = mul(
                    &axis_angle([1.0, 0.0, 0.0], a),
                    &mul(
                        &axis_angle([0.0, 1.0, 0.0], b),
                        &axis_angle([0.0, 0.0, 1.0], c),
                    ),
                );
                out.push(m);
            }
        }
    }
    out
}

/// The rotations of a point group: C1, Cn, Dn (2-fold along x), T, O, I
/// (2-folds along x, y, z; the 5-folds in the yz plane, as in RELION's I2).
pub fn symmetry(name: &str) -> anyhow::Result<Vec<Mat>> {
    let s = name.trim().to_ascii_uppercase();
    let fold = |s: &str| {
        s[1..]
            .parse::<usize>()
            .ok()
            .filter(|&n| (1..=60).contains(&n))
    };
    let gens: Vec<Mat> = match s.chars().next() {
        Some('C') => vec![axis_angle(
            [0.0, 0.0, 1.0],
            360.0 / fold(&s).ok_or_else(|| anyhow::anyhow!("对称性无效：{name}"))? as f64,
        )],
        Some('D') => {
            let n = fold(&s).ok_or_else(|| anyhow::anyhow!("对称性无效：{name}"))?;
            vec![
                axis_angle([0.0, 0.0, 1.0], 360.0 / n as f64),
                axis_angle([1.0, 0.0, 0.0], 180.0),
            ]
        }
        Some('T') if s == "T" => vec![
            axis_angle([0.0, 0.0, 1.0], 180.0),
            axis_angle([1.0, 1.0, 1.0], 120.0),
        ],
        Some('O') if s == "O" => vec![
            axis_angle([0.0, 0.0, 1.0], 90.0),
            axis_angle([1.0, 1.0, 1.0], 120.0),
        ],
        Some('I') if s == "I" => {
            let phi = (1.0 + 5f64.sqrt()) / 2.0;
            vec![
                axis_angle([0.0, 0.0, 1.0], 180.0),
                axis_angle([1.0, 1.0, 1.0], 120.0),
                axis_angle([0.0, 1.0, phi], 72.0),
            ]
        }
        _ => anyhow::bail!("对称性无效：{name}（可用 C1、Cn、Dn、T、O、I）"),
    };
    let mut group = vec![IDENTITY];
    loop {
        let mut added = false;
        for i in 0..group.len() {
            for g in &gens {
                let m = mul(&group[i], g);
                if !group.iter().any(|h| distance(h, &m) < 1e-3) {
                    group.push(m);
                    added = true;
                }
            }
        }
        if !added || group.len() > 60 {
            break;
        }
    }
    Ok(group)
}

/// m · v
#[inline]
pub fn apply(m: &Mat, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euler_round_trip() {
        for &(r, t, p) in &[
            (10.0, 20.0, 30.0),
            (-150.0, 100.0, 170.0),
            (45.0, 179.0, -60.0),
        ] {
            let m = euler(r, t, p);
            let [r2, t2, p2] = angles(&m);
            assert!(distance(&m, &euler(r2, t2, p2)) < 1e-6);
        }
    }

    #[test]
    fn group_orders() {
        for (name, n) in [
            ("C1", 1),
            ("C7", 7),
            ("D3", 6),
            ("T", 12),
            ("O", 24),
            ("I", 60),
        ] {
            assert_eq!(symmetry(name).unwrap().len(), n, "{name}");
        }
    }

    #[test]
    fn grid_covers_the_sphere() {
        let g = directions(10.0);
        // Every direction is within ~step of a grid point.
        for i in 0..200 {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / 200.0;
            let a = i as f64 * 2.4;
            let v = [
                (1.0 - z * z).sqrt() * a.cos(),
                (1.0 - z * z).sqrt() * a.sin(),
                z,
            ];
            let best = g
                .iter()
                .map(|[r, t]| {
                    let (r, t) = (r.to_radians(), t.to_radians());
                    let u = [t.sin() * r.cos(), t.sin() * r.sin(), t.cos()];
                    (u[0] * v[0] + u[1] * v[1] + u[2] * v[2])
                        .clamp(-1.0, 1.0)
                        .acos()
                        .to_degrees()
                })
                .fold(180.0, f64::min);
            assert!(best < 10.0, "{best}");
        }
    }
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    #[test]
    fn reduced_grid_covers_every_orientation() {
        // Each orientation has a symmetry mate within the grid's step of
        // the reduced grid.
        for name in ["C7", "D7", "D2"] {
            let full = grid(15.0);
            let red = grid_for(15.0, name);
            let g = symmetry(name).unwrap();
            assert!(
                red.len() * g.len() <= full.len() * 11 / 10
                    && red.len() * g.len() >= full.len() * 9 / 10,
                "{name}: {} × {} vs {}",
                red.len(),
                g.len(),
                full.len()
            );
            let mut rng = crate::spa::Rng(3);
            for _ in 0..200 {
                let r = crate::spa::refine::random_rotation(&mut rng);
                let best = red
                    .iter()
                    .flat_map(|q| g.iter().map(move |m| distance(&mul(q, m), &r)))
                    .fold(f64::MAX, f64::min);
                assert!(best < 15.0, "{name}: {best}");
            }
        }
    }
}
