//! Vesicles among the segmented membranes: a 26-connected piece of membrane
//! that lies on a sphere is a vesicle, and its membrane-centre diameter is
//! measured (what CryoVesNet reports for synaptic vesicles).
//!
//! Per piece: an algebraic least-squares sphere through its voxels (in nm),
//! then three checks. The voxels must lie close to that sphere (spread no
//! more than a membrane's thickness plus a little), the radius must be that
//! of a vesicle, and the membrane must go round it: seen from the centre it
//! covers most directions. Directions near the beam axis are not counted,
//! since the missing wedge erases membranes facing the beam, nor directions
//! that leave the tomogram. A solid blob, a flat sheet, a tube or two
//! touching vesicles fail one of the checks and stay plain membrane.

use rayon::prelude::*;
use serde::Serialize;

/// Vesicle radius range (membrane centre), nm.
const R_MIN: f64 = 8.0;
const R_MAX: f64 = 100.0;
/// Largest RMS distance from the sphere: membrane thickness spread plus a
/// share of the radius for slightly oval vesicles, nm.
const RMS_BASE: f64 = 2.0;
const RMS_PER_R: f64 = 0.08;
/// Directions with |cos θ| to the beam above this are not counted.
const WEDGE_COS: f64 = 0.7;
/// Share of the counted directions the membrane must reach.
const MIN_COVER: f64 = 0.6;
/// Direction bins: rings in cos θ and sectors in φ.
const RINGS: usize = 10;
const SECTORS: usize = 24;

#[derive(Clone, Debug, Serialize)]
pub struct Vesicle {
    /// Centre in voxels (x, y, z) of the tomogram.
    pub center: [f32; 3],
    /// Membrane-centre diameter, nm.
    pub diameter: f32,
    pub voxels: usize,
}

/// One piece of membrane: its voxels (indices, X fastest), or None when it
/// is too big to be a vesicle.
fn pieces(mask: &[bool], n: [usize; 3], max_side: usize) -> Vec<Vec<u32>> {
    let (w, h, d) = (n[0], n[1], n[2]);
    let mut seen = vec![0u64; mask.len().div_ceil(64)];
    let mut out = vec![];
    let mut stack: Vec<u32> = vec![];
    for start in 0..mask.len() {
        if !mask[start] || seen[start / 64] >> (start % 64) & 1 == 1 {
            continue;
        }
        seen[start / 64] |= 1 << (start % 64);
        stack.push(start as u32);
        let mut vox = vec![];
        let (mut lo, mut hi) = ([usize::MAX; 3], [0usize; 3]);
        let mut big = false;
        while let Some(i) = stack.pop() {
            let i = i as usize;
            let p = [i % w, (i / w) % h, i / (w * h)];
            if !big {
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
                if (0..3).any(|a| hi[a] - lo[a] > max_side) {
                    big = true;
                    vox = vec![];
                } else {
                    vox.push(i as u32);
                }
            }
            for dz in -1i64..=1 {
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (x, y, z) = (p[0] as i64 + dx, p[1] as i64 + dy, p[2] as i64 + dz);
                        if x < 0 || y < 0 || z < 0 || x >= w as i64 || y >= h as i64 || z >= d as i64 {
                            continue;
                        }
                        let j = (z as usize * h + y as usize) * w + x as usize;
                        if mask[j] && seen[j / 64] >> (j % 64) & 1 == 0 {
                            seen[j / 64] |= 1 << (j % 64);
                            stack.push(j as u32);
                        }
                    }
                }
            }
        }
        if !big {
            out.push(vox);
        }
    }
    out
}

/// Least-squares sphere through points: |p|² = 2 c·p + k. Returns centre and radius.
fn fit_sphere(pts: &[[f64; 3]]) -> Option<([f64; 3], f64)> {
    let mut a = [[0f64; 4]; 4];
    let mut b = [0f64; 4];
    for p in pts {
        let row = [2.0 * p[0], 2.0 * p[1], 2.0 * p[2], 1.0];
        let rhs = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
        for i in 0..4 {
            for j in 0..4 {
                a[i][j] += row[i] * row[j];
            }
            b[i] += row[i] * rhs;
        }
    }
    // Gaussian elimination with partial pivoting.
    for c in 0..4 {
        let piv = (c..4).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
        if a[piv][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, piv);
        b.swap(c, piv);
        for r in c + 1..4 {
            let f = a[r][c] / a[c][c];
            for k in c..4 {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = [0f64; 4];
    for c in (0..4).rev() {
        let s: f64 = (c + 1..4).map(|k| a[c][k] * x[k]).sum();
        x[c] = (b[c] - s) / a[c][c];
    }
    let r2 = x[3] + x[0] * x[0] + x[1] * x[1] + x[2] * x[2];
    (r2 > 0.0).then(|| ([x[0], x[1], x[2]], r2.sqrt()))
}

/// Tests one piece; `size` is the tomogram in nm.
fn vesicle(pts: &[[f64; 3]], size: [f64; 3]) -> Option<([f64; 3], f64)> {
    let (c, r) = fit_sphere(pts)?;
    // Most of a vesicle is inside the tomogram, so its centre is too; arcs of
    // bigger membranes cut by the border fit spheres centred outside.
    if !(R_MIN..=R_MAX).contains(&r) || (0..3).any(|a| c[a] < 0.0 || c[a] > size[a]) {
        return None;
    }
    let ms = pts.iter().map(|p| ((0..3).map(|a| (p[a] - c[a]).powi(2)).sum::<f64>().sqrt() - r).powi(2)).sum::<f64>() / pts.len() as f64;
    if ms.sqrt() > RMS_BASE + RMS_PER_R * r {
        return None;
    }
    // Bin of a direction: ring by cos θ (θ from the beam, Z), sector by φ.
    let bin = |u: [f64; 3]| {
        let ring = (((u[2] + 1.0) / 2.0 * RINGS as f64) as usize).min(RINGS - 1);
        let sector = (((u[1].atan2(u[0]) / std::f64::consts::TAU + 0.5) * SECTORS as f64) as usize).min(SECTORS - 1);
        ring * SECTORS + sector
    };
    let mut hit = vec![false; RINGS * SECTORS];
    for p in pts {
        let v = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
        hit[bin([v[0] / l, v[1] / l, v[2] / l])] = true;
    }
    let (mut counted, mut covered) = (0usize, 0usize);
    for ring in 0..RINGS {
        let cz = (ring as f64 + 0.5) / RINGS as f64 * 2.0 - 1.0;
        if cz.abs() > WEDGE_COS {
            continue;
        }
        let s = (1.0 - cz * cz).sqrt();
        for sector in 0..SECTORS {
            let phi = ((sector as f64 + 0.5) / SECTORS as f64 - 0.5) * std::f64::consts::TAU;
            let q = [c[0] + r * s * phi.cos(), c[1] + r * s * phi.sin(), c[2] + r * cz];
            if (0..3).any(|a| q[a] < 0.0 || q[a] > size[a]) {
                continue;
            }
            counted += 1;
            covered += hit[ring * SECTORS + sector] as usize;
        }
    }
    // Enough of the counted band must be inside the tomogram to judge.
    let band = (0..RINGS).filter(|&k| (((k as f64 + 0.5) / RINGS as f64) * 2.0 - 1.0).abs() <= WEDGE_COS).count() * SECTORS;
    (2 * counted >= band && covered as f64 >= MIN_COVER * counted as f64).then_some((c, r))
}

/// Finds the vesicles in a membrane mask (dims `n`, X fastest, `pixel_a` Å
/// per voxel). Returns them and the mask of their voxels.
pub fn find(mask: &[bool], n: [usize; 3], pixel_a: f32) -> (Vec<Vesicle>, Vec<bool>) {
    let nm = pixel_a as f64 / 10.0;
    let max_side = ((2.0 * R_MAX + 20.0) / nm).ceil() as usize;
    let size = [n[0] as f64 * nm, n[1] as f64 * nm, n[2] as f64 * nm];
    let found: Vec<(Vec<u32>, [f64; 3], f64)> = pieces(mask, n, max_side)
        .into_par_iter()
        .filter_map(|vox| {
            let pts: Vec<[f64; 3]> = vox
                .iter()
                .map(|&i| {
                    let i = i as usize;
                    [((i % n[0]) as f64 + 0.5) * nm, (((i / n[0]) % n[1]) as f64 + 0.5) * nm, ((i / (n[0] * n[1])) as f64 + 0.5) * nm]
                })
                .collect();
            let (c, r) = vesicle(&pts, size)?;
            Some((vox, c, r))
        })
        .collect();
    let mut is = vec![false; mask.len()];
    let mut out = Vec::with_capacity(found.len());
    for (vox, c, r) in found {
        for &i in &vox {
            is[i as usize] = true;
        }
        out.push(Vesicle { center: c.map(|v| (v / nm - 0.5) as f32), diameter: (2.0 * r) as f32, voxels: vox.len() });
    }
    out.sort_by(|a, b| a.center[2].total_cmp(&b.center[2]).then(a.center[1].total_cmp(&b.center[1])).then(a.center[0].total_cmp(&b.center[0])));
    (out, is)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adds a shell (radius r, thickness t voxels) without the caps the
    /// missing wedge removes.
    fn shell(m: &mut [bool], n: [usize; 3], c: [f64; 3], r: f64, t: f64) {
        for z in 0..n[2] {
            for y in 0..n[1] {
                for x in 0..n[0] {
                    let v = [x as f64 + 0.5 - c[0], y as f64 + 0.5 - c[1], z as f64 + 0.5 - c[2]];
                    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    if (l - r).abs() <= t / 2.0 && (v[2] / l.max(1e-9)).abs() < 0.8 {
                        m[(z * n[1] + y) * n[0] + x] = true;
                    }
                }
            }
        }
    }

    #[test]
    fn finds_vesicles_not_sheets_blobs_or_pairs() {
        // 10 Å voxels: 1 voxel = 1 nm.
        let n = [160, 160, 80];
        let mut m = vec![false; n[0] * n[1] * n[2]];
        shell(&mut m, n, [30.0, 30.0, 40.0], 22.0, 4.0);
        shell(&mut m, n, [100.0, 40.0, 40.0], 18.0, 4.0);
        // Cut by the top of the tomogram (centre 10 nm below it): still a vesicle.
        shell(&mut m, n, [40.0, 120.0, 72.0], 20.0, 4.0);
        // Two touching vesicles: one piece, not a sphere.
        shell(&mut m, n, [100.0, 110.0, 40.0], 18.0, 4.0);
        shell(&mut m, n, [134.0, 110.0, 40.0], 18.0, 4.0);
        // A flat sheet and a solid ball.
        for z in 30..34 {
            for y in 140..158 {
                for x in 60..158 {
                    m[(z * n[1] + y) * n[0] + x] = true;
                }
            }
        }
        for z in 0..n[2] {
            for y in 0..n[1] {
                for x in 0..n[0] {
                    if ((x as f64 - 70.0).powi(2) + (y as f64 - 75.0).powi(2) + (z as f64 - 40.0).powi(2)).sqrt() < 9.0 {
                        m[(z * n[1] + y) * n[0] + x] = true;
                    }
                }
            }
        }
        let (v, is) = find(&m, n, 10.0);
        assert_eq!(v.len(), 3, "{v:?}");
        let mut d: Vec<f32> = v.iter().map(|v| v.diameter).collect();
        d.sort_by(f32::total_cmp);
        for (got, want) in d.iter().zip([36.0, 40.0, 44.0]) {
            assert!((got - want).abs() < 2.0, "{d:?}");
        }
        assert!(is[(40 * n[1] + 30) * n[0] + 8]);
        assert!(!is[(31 * n[1] + 150) * n[0] + 100]);
    }

    /// On a real segmentation: VES_MRC=<membrane mask .mrc> cargo test --release -- --ignored vesicles_on_file
    #[test]
    #[ignore]
    fn vesicles_on_file() {
        let path = std::env::var("VES_MRC").unwrap();
        let (v, n, px) = crate::tomo::pipeline::load(&path).unwrap();
        let mask: Vec<bool> = v.iter().map(|&x| x > 0.5).collect();
        let t = std::time::Instant::now();
        let (ves, _) = find(&mask, n, px as f32);
        let d: Vec<f32> = ves.iter().map(|v| v.diameter).collect();
        let mean = d.iter().sum::<f32>() / d.len().max(1) as f32;
        let sd = (d.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / d.len().max(1) as f32).sqrt();
        eprintln!("{n:?} at {px} Å: {} vesicles, diameter {mean:.1} ± {sd:.1} nm, {:.2} s", ves.len(), t.elapsed().as_secs_f32());
        for v in ves.iter().take(40) {
            eprintln!("  {:?} {:.1} nm {} voxels", v.center.map(|c| c.round()), v.diameter, v.voxels);
        }
    }

    #[test]
    fn membranes_and_vesicles_are_one_undo_step() {
        let n = [96, 96, 64];
        let mut m = vec![false; n[0] * n[1] * n[2]];
        shell(&mut m, n, [40.0, 40.0, 32.0], 20.0, 4.0);
        for z in 10..14 {
            for y in 0..n[1] {
                for x in 80..96 {
                    m[(z * n[1] + y) * n[0] + x] = true;
                }
            }
        }
        let mut l = crate::labels::Labels::new(n[0], n[1], n[2]);
        let (zs, label, ves) = crate::membrane::apply(&mut l, &m, n, 10.0, 0);
        assert_eq!(ves.len(), 1);
        assert!(!zs.is_empty());
        let vlabel = l.materials.iter().find(|m| m.name == crate::membrane::VESICLE_MATERIAL).unwrap().label;
        assert_ne!(vlabel, label);
        assert_eq!(l.get(85, 5, 11), label);
        assert_eq!(l.get(20, 40, 32), vlabel);
        l.undo();
        assert!(l.is_empty());
    }

    #[test]
    fn sphere_fit_is_exact_on_a_sphere() {
        let pts: Vec<[f64; 3]> = (0..200)
            .map(|i| {
                let (t, p) = (i as f64 * 0.37, i as f64 * 1.13);
                [5.0 + 7.0 * t.sin() * p.cos(), -2.0 + 7.0 * t.sin() * p.sin(), 1.0 + 7.0 * t.cos()]
            })
            .collect();
        let (c, r) = fit_sphere(&pts).unwrap();
        assert!((r - 7.0).abs() < 1e-9 && (c[0] - 5.0).abs() < 1e-9 && (c[1] + 2.0).abs() < 1e-9 && (c[2] - 1.0).abs() < 1e-9);
    }
}
