//! Bundle adjustment of patch contours (the projection model of Kremer,
//! Mastronarde & McIntosh 1996, our own solver). A 3D point X seen in view
//! i lands at p = m_i · R(α + ρ_i) · [cos θ'_i·x + sin θ'_i·z, y] + t_i, with
//! θ'_i = θ_i + τ_i. Shifts t_i are solved per view; in-plane rotation ρ,
//! tilt correction τ and magnification m are shared within groups of
//! neighbouring tilts (`Groups`, 0 = not solved), as tiltalign groups them.
//!
//! Alternation: points by 3×3 least squares, then view parameters by
//! Gauss–Newton with the shifts eliminated per group (Schur complement) and
//! a weak prior that keeps ρ, τ and m − 1 small where the data say little,
//! then Huber reweighting from the robust spread of the residuals. Gauges:
//! the view nearest zero tilt has no shift (points are translated to keep
//! it so), and its group keeps ρ = τ = 0, m = 1. The global axis α is
//! profiled by `sweep`. Residuals are weighted means of the image errors in
//! working pixels, never surface fits.

use super::{check_cancel, median, track::Tracks};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Solution {
    pub axis: f64,
    pub shifts: Vec<[f64; 2]>,
    pub magnifications: Vec<f64>,
    /// In-plane rotation added to the axis (°) and tilt correction (°) per view.
    pub rotations: Vec<f64>,
    pub tilts: Vec<f64>,
    pub points: Vec<[f64; 3]>,
    pub residual: f64,
    pub per_view: Vec<f64>,
    pub weights: Vec<f64>,
}

/// Group sizes for the shared view parameters (0: not solved).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Groups {
    pub rotation: usize,
    pub tilt: usize,
    pub mag: usize,
}

impl Groups {
    pub const NONE: Groups = Groups {
        rotation: 0,
        tilt: 0,
        mag: 0,
    };

    /// About eight groups over the series, at least three views each.
    pub fn default_for(views: usize) -> Groups {
        let g = (views / 8).max(3);
        Groups {
            rotation: g,
            tilt: g,
            mag: g,
        }
    }
}

/// Prior spreads of ρ (°), τ (°) and m − 1 where the data say little.
const PRIOR: [f64; 3] = [2.0, 1.0, 0.03];

fn turn(alpha: f64) -> ([[f64; 2]; 2], [[f64; 2]; 2]) {
    // R(α) rotates by ψ = 90° − α; dR/dα = −dR/dψ (per radian).
    let (s, c) = (90.0 - alpha).to_radians().sin_cos();
    ([[c, -s], [s, c]], [[s, c], [-c, s]])
}

struct ViewModel {
    r: [[f64; 2]; 2],
    dr: [[f64; 2]; 2],
    trig: [f64; 2],
}

fn views(angles: &[f64], axis: f64, rot: &[f64], tilt: &[f64]) -> Vec<ViewModel> {
    (0..angles.len())
        .map(|i| {
            let (r, dr) = turn(axis + rot[i]);
            let (s, c) = (angles[i] + tilt[i]).to_radians().sin_cos();
            ViewModel {
                r,
                dr,
                trig: [c, s],
            }
        })
        .collect()
}

fn mul(r: &[[f64; 2]; 2], q: [f64; 2]) -> [f64; 2] {
    [
        r[0][0] * q[0] + r[0][1] * q[1],
        r[1][0] * q[0] + r[1][1] * q[1],
    ]
}

/// Solves a small symmetric system by Gaussian elimination with pivoting.
fn solve_n<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> [f64; N] {
    for i in 0..N {
        let p = (i..N)
            .max_by(|&j, &k| a[j][i].abs().total_cmp(&a[k][i].abs()))
            .unwrap();
        a.swap(i, p);
        b.swap(i, p);
        if a[i][i].abs() < 1e-12 {
            return [0.0; N];
        }
        for j in i + 1..N {
            let f = a[j][i] / a[i][i];
            for k in i..N {
                a[j][k] -= f * a[i][k];
            }
            b[j] -= f * b[i];
        }
    }
    let mut x = [0.0; N];
    for i in (0..N).rev() {
        x[i] = (b[i] - (i + 1..N).map(|k| a[i][k] * x[k]).sum::<f64>()) / a[i][i];
    }
    x
}

/// Dense symmetric solve by Gaussian elimination with partial pivoting.
fn solve_dense(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for i in 0..n {
        let p = (i..n)
            .max_by(|&j, &k| a[j][i].abs().total_cmp(&a[k][i].abs()))
            .unwrap();
        a.swap(i, p);
        b.swap(i, p);
        if a[i][i].abs() < 1e-15 {
            return vec![0.0; n];
        }
        for j in i + 1..n {
            let f = a[j][i] / a[i][i];
            if f != 0.0 {
                for k in i..n {
                    a[j][k] -= f * a[i][k];
                }
                b[j] -= f * b[i];
            }
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = (b[i] - (i + 1..n).map(|k| a[i][k] * x[k]).sum::<f64>()) / a[i][i];
    }
    x
}

pub fn solve(tr: &Tracks, angles: &[f64], axis: f64, groups: Groups) -> Result<Solution> {
    let n = angles.len();
    if n < 3 || tr.observations.len() < 12 {
        bail!("投影模型观测不足");
    }
    let zero = (0..n)
        .min_by(|&a, &b| angles[a].abs().total_cmp(&angles[b].abs()))
        .unwrap();
    // Group index of each view for each shared parameter; the zero view's group is fixed.
    let group_of = |size: usize| -> Vec<Option<usize>> {
        (0..n)
            .map(|i| {
                if size > 0 && i / size != zero / size {
                    Some(i / size)
                } else {
                    None
                }
            })
            .collect()
    };
    let gs = [
        group_of(groups.rotation),
        group_of(groups.tilt),
        group_of(groups.mag),
    ];
    let mut shifts = vec![[0.0; 2]; n];
    let (mut rot, mut tilt, mut mags) = (vec![0.0; n], vec![0.0; n], vec![1.0; n]);
    let mut points = vec![[0.0; 3]; tr.points];
    let base: Vec<f64> = tr
        .observations
        .iter()
        .map(|o| o.cc.max(0.01).powi(2))
        .collect();
    let mut weights = base.clone();
    let mut errors = vec![0.0; weights.len()];
    let shared = groups != Groups::NONE;
    // Shared parameter index of each view and kind, and a view of each parameter.
    let mut index = vec![[None; 3]; n];
    let mut owner: Vec<(usize, usize)> = Vec::new();
    for (kind, g) in gs.iter().enumerate() {
        let mut seen = std::collections::BTreeMap::new();
        for i in 0..n {
            if let Some(id) = g[i] {
                let p = *seen.entry(id).or_insert_with(|| {
                    owner.push((kind, i));
                    owner.len() - 1
                });
                index[i][kind] = Some(p);
            }
        }
    }
    let params = owner.len();
    for it in 0..150 {
        check_cancel()?;
        let vm = views(angles, axis, &rot, &tilt);
        // Points.
        let mut aa = vec![[[0.0; 3]; 3]; tr.points];
        let mut bb = vec![[0.0; 3]; tr.points];
        for (o, &w) in tr.observations.iter().zip(&weights) {
            let v = &vm[o.view];
            let m = mags[o.view];
            let [c, s] = v.trig;
            // Rows of m·R·[[c, 0, s], [0, 1, 0]].
            let rows: [[f64; 3]; 2] =
                std::array::from_fn(|k| [m * v.r[k][0] * c, m * v.r[k][1], m * v.r[k][0] * s]);
            for (k, row) in rows.iter().enumerate() {
                for j in 0..3 {
                    bb[o.point][j] += w * row[j] * (o.xy[k] - shifts[o.view][k]);
                    for l in 0..3 {
                        aa[o.point][j][l] += w * row[j] * row[l];
                    }
                }
            }
        }
        for j in 0..tr.points {
            for d in 0..3 {
                aa[j][d][d] += 1e-8;
            }
            points[j] = solve_n(aa[j], bb[j]);
        }
        // Views: per view the normal equations in (t, ρ, τ, m); shared
        // parameters of a group are solved after eliminating its shifts.
        let old = shifts.clone();
        let mut htt = vec![0.0; n];
        let mut hts = vec![[[0.0; 3]; 2]; n];
        let mut hss = vec![[[0.0; 3]; 3]; n];
        let mut gt = vec![[0.0; 2]; n];
        let mut gsv = vec![[0.0; 3]; n];
        for (o, &w) in tr.observations.iter().zip(&weights) {
            let (v, i, x) = (&vm[o.view], o.view, points[o.point]);
            let [c, s] = v.trig;
            let q = [c * x[0] + s * x[2], x[1]];
            let dq = [(-s * x[0] + c * x[2]) * std::f64::consts::PI / 180.0, 0.0];
            let rq = mul(&v.r, q);
            let pred = [
                mags[i] * rq[0] + shifts[i][0],
                mags[i] * rq[1] + shifts[i][1],
            ];
            let jr = mul(&v.dr, q).map(|a| mags[i] * a * std::f64::consts::PI / 180.0);
            let jt = mul(&v.r, dq).map(|a| mags[i] * a);
            for k in 0..2 {
                let e = o.xy[k] - pred[k];
                let js = [jr[k], jt[k], rq[k]];
                htt[i] += w;
                gt[i][k] += w * e;
                for a in 0..3 {
                    hts[i][k][a] += w * js[a];
                    gsv[i][a] += w * js[a] * e;
                    for b in 0..3 {
                        hss[i][a][b] += w * js[a] * js[b];
                    }
                }
            }
        }
        let mut delta = vec![[0.0; 3]; n];
        if shared && it >= 3 {
            // All shared parameters at once, the shifts eliminated view by
            // view (Schur complement; H_tt = W·I per view).
            let mut s = vec![vec![0.0; params]; params];
            let mut r = vec![0.0; params];
            let mut weight = vec![0.0; params];
            for i in 0..n {
                if htt[i] <= 0.0 {
                    continue;
                }
                let wt = htt[i] / 2.0;
                for a in 0..3 {
                    let Some(p) = index[i][a] else { continue };
                    weight[p] += htt[i];
                    r[p] += gsv[i][a] - (hts[i][0][a] * gt[i][0] + hts[i][1][a] * gt[i][1]) / wt;
                    for bb in 0..3 {
                        let Some(q) = index[i][bb] else { continue };
                        s[p][q] += hss[i][a][bb]
                            - (hts[i][0][a] * hts[i][0][bb] + hts[i][1][a] * hts[i][1][bb]) / wt;
                    }
                }
            }
            for (p, &(kind, view)) in owner.iter().enumerate() {
                let current = [rot[view], tilt[view], mags[view] - 1.0][kind];
                let prior = 1e-2 * weight[p] / PRIOR[kind].powi(2) + 1e-9;
                s[p][p] += prior;
                r[p] -= prior * current;
            }
            let step = solve_dense(s, r);
            for i in 0..n {
                for a in 0..3 {
                    if let Some(p) = index[i][a] {
                        delta[i][a] = step[p];
                    }
                }
            }
        }
        for i in 0..n {
            if htt[i] <= 0.0 {
                continue;
            }
            // Shifts given the shared steps.
            for k in 0..2 {
                let coupled: f64 = (0..3).map(|a| hts[i][k][a] * delta[i][a]).sum();
                shifts[i][k] += (gt[i][k] - coupled) / (htt[i] / 2.0);
            }
            rot[i] = (rot[i] + delta[i][0]).clamp(-10.0, 10.0);
            tilt[i] = (tilt[i] + delta[i][1]).clamp(-10.0, 10.0);
            mags[i] = (mags[i] + delta[i][2]).clamp(0.9, 1.1);
        }
        // Gauge: translate points so the zero view has no shift.
        let v = &views(angles, axis, &rot, &tilt)[zero];
        let t = shifts[zero];
        // Inverse of m·R on the zero view (m = 1 there), then lift into 3D.
        let local = [
            v.r[0][0] * t[0] + v.r[1][0] * t[1],
            v.r[0][1] * t[0] + v.r[1][1] * t[1],
        ];
        let [c, s] = v.trig;
        let lift = [local[0] * c, local[1], local[0] * s];
        for x in &mut points {
            for k in 0..3 {
                x[k] += lift[k];
            }
        }
        let vm = views(angles, axis, &rot, &tilt);
        for i in 0..n {
            let [c, s] = vm[i].trig;
            let d = mul(&vm[i].r, [c * lift[0] + s * lift[2], lift[1]]);
            for k in 0..2 {
                shifts[i][k] -= mags[i] * d[k];
            }
        }
        for (k, o) in tr.observations.iter().enumerate() {
            let (v, x) = (&vm[o.view], points[o.point]);
            let [c, s] = v.trig;
            let p = mul(&v.r, [c * x[0] + s * x[2], x[1]]);
            errors[k] = (o.xy[0] - mags[o.view] * p[0] - shifts[o.view][0])
                .hypot(o.xy[1] - mags[o.view] * p[1] - shifts[o.view][1]);
        }
        if it > 10 {
            let scale = (median(&errors) * 1.4826).max(0.05);
            for k in 0..weights.len() {
                weights[k] = base[k] * (1.5 * scale / errors[k].max(1e-12)).min(1.0);
            }
        }
        let change = shifts
            .iter()
            .zip(old)
            .map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]))
            .fold(0.0, f64::max);
        if it > 30 && change < 1e-6 {
            break;
        }
    }
    let mut per_view = vec![0.0; n];
    let mut sw = vec![0.0; n];
    for ((o, &e), &w) in tr.observations.iter().zip(&errors).zip(&weights) {
        per_view[o.view] += w * e;
        sw[o.view] += w;
    }
    let residual = per_view.iter().sum::<f64>() / sw.iter().sum::<f64>().max(1e-12);
    for i in 0..n {
        per_view[i] /= sw[i].max(1e-12);
    }
    Ok(Solution {
        axis,
        shifts,
        magnifications: mags,
        rotations: rot,
        tilts: tilt,
        points,
        residual,
        per_view,
        weights,
    })
}

/// Whether shared parameters stayed well inside their bounds (a solve that
/// ran into them is fitting something the model does not describe).
pub fn plausible(s: &Solution) -> bool {
    s.rotations.iter().all(|r| r.abs() < 5.0)
        && s.tilts.iter().all(|t| t.abs() < 5.0)
        && s.magnifications.iter().all(|m| (m - 1.0).abs() < 0.05)
}

pub fn sweep(tr: &Tracks, angles: &[f64], initial: f64, groups: Groups) -> Result<Solution> {
    let mut best = solve(tr, angles, initial, groups)?;
    for i in -5..=5 {
        let s = solve(tr, angles, initial + i as f64, groups)?;
        if s.residual < best.residual {
            best = s;
        }
    }
    if (best.axis - initial).abs() >= 4.99 {
        for i in -10i32..=10 {
            if i.abs() > 5 {
                let s = solve(tr, angles, initial + i as f64, groups)?;
                if s.residual < best.residual {
                    best = s;
                }
            }
        }
    }
    let centre = best.axis;
    for i in -5..=5 {
        let s = solve(tr, angles, centre + i as f64 * 0.2, groups)?;
        if s.residual < best.residual {
            best = s;
        }
    }
    Ok(best)
}

pub fn refine(tr: &Tracks, angles: &[f64], centre: f64, groups: Groups) -> Result<Solution> {
    let mut best = solve(tr, angles, centre, groups)?;
    for i in -5..=5 {
        let s = solve(tr, angles, centre + i as f64 * 0.2, groups)?;
        if s.residual < best.residual {
            best = s;
        }
    }
    Ok(best)
}
