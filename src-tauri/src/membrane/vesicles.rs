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
//! that leave the tomogram. A solid blob, a flat sheet or a tube fails one
//! of the checks and stays plain membrane.
//!
//! Vesicles that touch each other or another membrane make one piece that
//! is no sphere. In such a piece spheres are searched with RANSAC: a sphere
//! through four nearby voxels, kept when enough of the piece lies on it and
//! it passes the same checks; its voxels are then taken out and the search
//! goes on until it keeps failing.

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

/// RANSAC: spheres up to this radius (nm) are searched in pieces that are
/// no sphere, from points this close to the first one.
const SPLIT_R_MAX: f64 = 60.0;
/// Membrane thickness the voxels on a sphere are counted in, nm.
const THICK: f64 = 4.0;
/// Share of the expected voxels a RANSAC sphere must collect.
const MIN_FILL: f64 = 0.4;
/// The search in a piece stops after this many failed tries in a row.
const MAX_FAILS: usize = 400;
/// Pieces with more voxels than this are not searched (whole cell membranes).
const MAX_SPLIT_VOXELS: usize = 4 << 20;

/// The 26-connected pieces of membrane: their voxels (indices, X fastest).
/// Pieces larger than `max_voxels` are left out.
fn pieces(mask: &[bool], n: [usize; 3], max_voxels: usize) -> Vec<Vec<u32>> {
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
        let mut big = false;
        while let Some(i) = stack.pop() {
            let i = i as usize;
            let p = [i % w, (i / w) % h, i / (w * h)];
            if !big {
                if vox.len() >= max_voxels {
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

/// Whether the membrane around a sphere (centre `c`, radius `r`) goes on
/// as a tube: of the points of `ctx` a little outside the sphere, most lie
/// at one distance from the line through `c` along which they spread, on
/// both sides of it. Only a tube does that; a
/// touching flat membrane or vesicle does not. Tubes along the beam look
/// like vesicles in every slice.
fn tube(ctx: &[[f64; 3]], c: [f64; 3], r: f64, hits: usize, size: [f64; 3]) -> bool {
    let beyond: Vec<[f64; 3]> = ctx
        .iter()
        .map(|p| [p[0] - c[0], p[1] - c[1], p[2] - c[2]])
        .filter(|v| {
            let d = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            d > r + THICK && d < 2.0 * r + THICK
        })
        .collect();
    if (beyond.len() as f64) < 0.3 * hits as f64 {
        return false;
    }
    // Main direction of the points beyond (power iteration on their scatter).
    let mut m = [[0f64; 3]; 3];
    for v in &beyond {
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += v[i] * v[j];
            }
        }
    }
    let mut a = [0.3, 0.5, 0.8];
    for _ in 0..50 {
        let b = [0, 1, 2].map(|i| m[i][0] * a[0] + m[i][1] * a[1] + m[i][2] * a[2]);
        let l = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt().max(1e-12);
        a = b.map(|x| x / l);
    }
    let tq: Vec<(f64, f64)> = beyond
        .iter()
        .map(|v| {
            let t = v[0] * a[0] + v[1] * a[1] + v[2] * a[2];
            (t, (v[0] * v[0] + v[1] * v[1] + v[2] * v[2] - t * t).max(0.0).sqrt())
        })
        .collect();
    let mut qs: Vec<f64> = tq.iter().map(|x| x.1).collect();
    qs.sort_by(f64::total_cmp);
    let tube_r = qs[qs.len() / 2];
    let (mut fore, mut back) = (0usize, 0usize);
    for &(t, q) in &tq {
        if (q - tube_r).abs() <= THICK / 2.0 + 1.0 && tube_r <= r + THICK {
            if t > 0.0 {
                fore += 1;
            } else {
                back += 1;
            }
        }
    }
    if (fore + back) * 2 > beyond.len() && fore.min(back) as f64 >= 0.1 * hits as f64 {
        return true;
    }
    // A tube along the beam, the case the missing wedge hides: points at
    // radius r around the vertical through `c`, above and below the sphere
    // (or the tomogram ends there).
    let (mut up, mut down) = (0usize, 0usize);
    for v in &beyond {
        let rxy = (v[0] * v[0] + v[1] * v[1]).sqrt();
        if (rxy - r).abs() <= THICK / 2.0 + 1.0 && v[2].abs() > r * 0.8 {
            if v[2] > 0.0 {
                up += 1;
            } else {
                down += 1;
            }
        }
    }
    let side = |n: usize, z: f64| n as f64 >= 0.1 * hits as f64 || z < 0.0 || z > size[2];
    up.max(down) as f64 >= 0.1 * hits as f64 && side(up, c[2] + 1.5 * r + THICK) && side(down, c[2] - 1.5 * r - THICK)
}

/// Tests the points `pts` of a possible vesicle; `ctx` is the whole piece
/// they belong to and `size` the tomogram, nm.
fn vesicle(pts: &[[f64; 3]], ctx: &[[f64; 3]], size: [f64; 3]) -> Option<([f64; 3], f64)> {
    let (c, r) = fit_sphere(pts)?;
    // Most of a vesicle is inside the tomogram, so its centre is well inside
    // too; arcs of bigger membranes cut by the border fit spheres centred
    // near or beyond it.
    if !(R_MIN..=R_MAX).contains(&r) || (0..3).any(|a| c[a] < 0.25 * r || c[a] > size[a] - 0.25 * r) {
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
    (2 * counted >= band && covered as f64 >= MIN_COVER * counted as f64 && !tube(ctx, c, r, pts.len(), size)).then_some((c, r))
}

/// Small deterministic random numbers for RANSAC.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n.max(1) as u64) as usize
    }
}

/// Spheres in a piece that is no sphere as a whole (see the module notes).
/// `vox_nm3` is the volume of a voxel. Returns per sphere the indices of its
/// points, centre and radius.
fn split(pts: &[[f64; 3]], size: [f64; 3], vox_nm3: f64) -> Vec<(Vec<usize>, [f64; 3], f64)> {
    const CELL: f64 = 10.0;
    let key = |p: &[f64; 3]| [(p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32, (p[2] / CELL).floor() as i32];
    let mut grid: std::collections::HashMap<[i32; 3], Vec<usize>> = std::collections::HashMap::new();
    for (i, p) in pts.iter().enumerate() {
        grid.entry(key(p)).or_default().push(i);
    }
    // Indices of live points within `reach` of `c`.
    let near = |c: &[f64; 3], reach: f64, alive: &[bool]| -> Vec<usize> {
        let (lo, hi) = (key(&c.map(|v| v - reach)), key(&c.map(|v| v + reach)));
        let mut out = vec![];
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    if let Some(v) = grid.get(&[x, y, z]) {
                        out.extend(v.iter().copied().filter(|&i| alive[i]));
                    }
                }
            }
        }
        out
    };
    let on = |i: usize, c: &[f64; 3], r: f64| {
        let d = (0..3).map(|a| (pts[i][a] - c[a]).powi(2)).sum::<f64>().sqrt();
        (d - r).abs() <= THICK / 2.0 + 0.5
    };
    let mut alive = vec![true; pts.len()];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ pts.len() as u64);
    let mut out = vec![];
    let mut fails = 0;
    while fails < MAX_FAILS {
        let left: usize = alive.iter().filter(|&&a| a).count();
        if left < 8 {
            break;
        }
        fails += 1;
        let mut p0 = rng.below(pts.len());
        for _ in 0..16 {
            if alive[p0] {
                break;
            }
            p0 = rng.below(pts.len());
        }
        if !alive[p0] {
            continue;
        }
        let nb = near(&pts[p0], 2.0 * SPLIT_R_MAX, &alive);
        if nb.len() < 8 {
            continue;
        }
        let four = [pts[p0], pts[nb[rng.below(nb.len())]], pts[nb[rng.below(nb.len())]], pts[nb[rng.below(nb.len())]]];
        let Some((c, r)) = fit_sphere(&four) else { continue };
        if !(R_MIN..=SPLIT_R_MAX).contains(&r) {
            continue;
        }
        // Expected voxels of a shell without the wedge caps.
        let expected = 4.0 * std::f64::consts::PI * r * r * WEDGE_COS * THICK / vox_nm3;
        let mut hits: Vec<usize> = near(&c, r + THICK, &alive).into_iter().filter(|&i| on(i, &c, r)).collect();
        if (hits.len() as f64) < MIN_FILL * expected {
            continue;
        }
        // Refit on the points on the sphere a few times, so a touching
        // membrane does not pull it.
        for _ in 0..4 {
            let sub: Vec<[f64; 3]> = hits.iter().map(|&i| pts[i]).collect();
            let Some((c, r)) = fit_sphere(&sub) else { break };
            hits = near(&c, r + THICK, &alive).into_iter().filter(|&i| on(i, &c, r)).collect();
        }
        let sub: Vec<[f64; 3]> = hits.iter().map(|&i| pts[i]).collect();
        let Some((c, r)) = vesicle(&sub, pts, size) else { continue };
        let hits: Vec<usize> = near(&c, r + THICK, &alive).into_iter().filter(|&i| on(i, &c, r)).collect();
        for &i in &hits {
            alive[i] = false;
        }
        out.push((hits, c, r));
        fails = 0;
    }
    out
}

/// Finds the vesicles in a membrane mask (dims `n`, X fastest, `pixel_a` Å
/// per voxel). Returns them and the mask of their voxels.
pub fn find(mask: &[bool], n: [usize; 3], pixel_a: f32) -> (Vec<Vesicle>, Vec<bool>) {
    let nm = pixel_a as f64 / 10.0;
    let size = [n[0] as f64 * nm, n[1] as f64 * nm, n[2] as f64 * nm];
    let found: Vec<(Vec<u32>, [f64; 3], f64)> = pieces(mask, n, MAX_SPLIT_VOXELS)
        .into_par_iter()
        .flat_map_iter(|vox| {
            let pts: Vec<[f64; 3]> = vox
                .iter()
                .map(|&i| {
                    let i = i as usize;
                    [((i % n[0]) as f64 + 0.5) * nm, (((i / n[0]) % n[1]) as f64 + 0.5) * nm, ((i / (n[0] * n[1])) as f64 + 0.5) * nm]
                })
                .collect();
            if let Some((c, r)) = vesicle(&pts, &pts, size) {
                return vec![(vox, c, r)];
            }
            split(&pts, size, nm * nm * nm).into_iter().map(|(idx, c, r)| (idx.iter().map(|&i| vox[i]).collect(), c, r)).collect()
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
    fn finds_vesicles_and_touching_pairs_not_sheets_or_blobs() {
        // 10 Å voxels: 1 voxel = 1 nm.
        let n = [160, 160, 80];
        let mut m = vec![false; n[0] * n[1] * n[2]];
        shell(&mut m, n, [30.0, 30.0, 40.0], 22.0, 4.0);
        shell(&mut m, n, [100.0, 40.0, 40.0], 18.0, 4.0);
        // Cut by the top of the tomogram (centre 10 nm below it): still a vesicle.
        shell(&mut m, n, [40.0, 120.0, 72.0], 20.0, 4.0);
        // Two touching vesicles: one piece, split into two.
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
        assert_eq!(v.len(), 5, "{v:?}");
        let mut d: Vec<f32> = v.iter().map(|v| v.diameter).collect();
        d.sort_by(f32::total_cmp);
        for (got, want) in d.iter().zip([36.0, 36.0, 36.0, 40.0, 44.0]) {
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
    fn vesicle_touching_a_membrane_is_found() {
        let n = [128, 128, 64];
        let mut m = vec![false; n[0] * n[1] * n[2]];
        // A wavy sheet across the whole volume and a vesicle resting on it.
        for z in 0..n[2] {
            for x in 0..n[0] {
                let y0 = 40.0 + 6.0 * (x as f64 / 20.0).sin();
                for y in (y0 - 2.0) as usize..(y0 + 2.0) as usize {
                    m[(z * n[1] + y) * n[0] + x] = true;
                }
            }
        }
        shell(&mut m, n, [64.0, 40.0 + 6.0 * (64.0f64 / 20.0).sin() + 22.0, 32.0], 20.0, 4.0);
        let (v, is) = find(&m, n, 10.0);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!((v[0].diameter - 40.0).abs() < 2.5, "{v:?}");
        // The sheet away from the vesicle stays membrane.
        assert!(!is[(32 * n[1] + 40) * n[0] + 10]);
    }

    #[test]
    fn tube_along_the_beam_is_not_a_vesicle() {
        let n = [64, 64, 96];
        let mut m = vec![false; n[0] * n[1] * n[2]];
        for z in 0..n[2] {
            for y in 0..n[1] {
                for x in 0..n[0] {
                    let r = ((x as f64 + 0.5 - 32.0).powi(2) + (y as f64 + 0.5 - 32.0).powi(2)).sqrt();
                    if (r - 18.0).abs() <= 2.0 {
                        m[(z * n[1] + y) * n[0] + x] = true;
                    }
                }
            }
        }
        let (v, _) = find(&m, n, 10.0);
        assert!(v.is_empty(), "{v:?}");
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
        let (zs, label, ves) = crate::membrane::apply(&mut l, &m, None, n, 10.0, 0);
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
