//! Alternating weighted least squares of 3D contour points and view shifts,
//! with Huber reweighting and a profiled global axis search. Translation
//! gauge: shifts of the closest-to-zero view are zero. Global tilt offset
//! is fixed (it is indistinguishable from a rotation of all free 3D points).
//! Residuals are weighted means of Euclidean image errors, never surface fits.

use super::{check_cancel, geometry::projection, median, track::Tracks};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Solution {
    pub axis: f64,
    pub shifts: Vec<[f64; 2]>,
    pub magnifications: Vec<f64>,
    pub points: Vec<[f64; 3]>,
    pub residual: f64,
    pub per_view: Vec<f64>,
    pub weights: Vec<f64>,
}

fn solve3(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> [f64; 3] {
    for i in 0..3 {
        a[i][i] += 1e-8;
        let p = (i..3)
            .max_by(|&j, &k| a[j][i].abs().total_cmp(&a[k][i].abs()))
            .unwrap();
        a.swap(i, p);
        b.swap(i, p);
        let d = a[i][i];
        if d.abs() < 1e-12 {
            return [0.0; 3];
        }
        for v in &mut a[i][i..] {
            *v /= d;
        }
        b[i] /= d;
        for j in 0..3 {
            if i == j {
                continue;
            }
            let d = a[j][i];
            let pivot = a[i];
            for (v, &p) in a[j][i..].iter_mut().zip(&pivot[i..]) {
                *v -= d * p;
            }
            b[j] -= d * b[i];
        }
    }
    b
}

fn predict(p: [[f64; 3]; 2], x: [f64; 3]) -> [f64; 2] {
    std::array::from_fn(|k| (0..3).map(|j| p[k][j] * x[j]).sum())
}

pub fn solve(tr: &Tracks, angles: &[f64], axis: f64, mag_group: usize) -> Result<Solution> {
    let n = angles.len();
    if n < 3 || tr.observations.len() < 12 {
        bail!("投影模型观测不足");
    }
    let zero = (0..n)
        .min_by(|&a, &b| angles[a].abs().total_cmp(&angles[b].abs()))
        .unwrap();
    let ps: Vec<_> = angles.iter().map(|&a| projection(a, axis)).collect();
    let mut shifts = vec![[0.0; 2]; n];
    let mut mags = vec![1.0; n];
    let mut points = vec![[0.0; 3]; tr.points];
    let base: Vec<f64> = tr
        .observations
        .iter()
        .map(|o| o.cc.max(0.01).powi(2))
        .collect();
    let mut weights = base.clone();
    let mut errors = vec![0.0; weights.len()];
    // Warm start the view translations from contour centroids; point positions
    // then remove the different visibility bias as the alternation converges.
    for it in 0..120 {
        check_cancel()?;
        let mut aa = vec![[[0.0; 3]; 3]; tr.points];
        let mut bb = vec![[0.0; 3]; tr.points];
        for (o, &w) in tr.observations.iter().zip(&weights) {
            let p = ps[o.view];
            let m = mags[o.view];
            for (k, row) in p.iter().enumerate() {
                for j in 0..3 {
                    bb[o.point][j] += w * m * row[j] * (o.xy[k] - shifts[o.view][k]);
                    for l in 0..3 {
                        aa[o.point][j][l] += w * m * m * row[j] * row[l];
                    }
                }
            }
        }
        for j in 0..tr.points {
            points[j] = solve3(aa[j], bb[j]);
        }
        let mut sums = vec![[0.0; 2]; n];
        let mut sw = vec![0.0; n];
        for (o, &w) in tr.observations.iter().zip(&weights) {
            let p = predict(ps[o.view], points[o.point]);
            sw[o.view] += w;
            for k in 0..2 {
                sums[o.view][k] += w * (o.xy[k] - mags[o.view] * p[k]);
            }
        }
        let old = shifts.clone();
        for i in 0..n {
            if sw[i] > 0.0 {
                shifts[i] = [sums[i][0] / sw[i], sums[i][1] / sw[i]];
            }
        }
        // Gauge fixing through an equivalent translation of the 3D points.
        let r = super::geometry::rotation(axis);
        let t = shifts[zero];
        let dx = r[0][0] * t[0] + r[1][0] * t[1];
        let dy = r[0][1] * t[0] + r[1][1] * t[1];
        let th = angles[zero].to_radians();
        let delta = [dx * th.cos(), dy, dx * th.sin()];
        for x in &mut points {
            for k in 0..3 {
                x[k] += delta[k];
            }
        }
        for i in 0..n {
            let d = predict(ps[i], delta);
            for k in 0..2 {
                shifts[i][k] -= mags[i] * d[k];
            }
        }
        if mag_group > 0 && it > 10 {
            for start in (0..n).step_by(mag_group) {
                let end = (start + mag_group).min(n);
                if (start..end).contains(&zero) {
                    continue;
                }
                let (mut num, mut den) = (0.0, 0.0);
                for (o, &w) in tr
                    .observations
                    .iter()
                    .zip(&weights)
                    .filter(|(o, _)| (start..end).contains(&o.view))
                {
                    let p = predict(ps[o.view], points[o.point]);
                    for (k, v) in p.iter().enumerate() {
                        num += w * v * (o.xy[k] - shifts[o.view][k]);
                        den += w * v * v;
                    }
                }
                if den > 0.0 {
                    for m in &mut mags[start..end] {
                        *m = (num / den).clamp(0.95, 1.05);
                    }
                }
            }
        }
        for (k, o) in tr.observations.iter().enumerate() {
            let p = predict(ps[o.view], points[o.point]);
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
        points,
        residual,
        per_view,
        weights,
    })
}

pub fn sweep(tr: &Tracks, angles: &[f64], initial: f64, mag_group: usize) -> Result<Solution> {
    let mut best = solve(tr, angles, initial, mag_group)?;
    for i in -5..=5 {
        let s = solve(tr, angles, initial + i as f64, mag_group)?;
        if s.residual < best.residual {
            best = s;
        }
    }
    if (best.axis - initial).abs() >= 4.99 {
        for i in -10i32..=10 {
            if i.abs() > 5 {
                let s = solve(tr, angles, initial + i as f64, mag_group)?;
                if s.residual < best.residual {
                    best = s;
                }
            }
        }
    }
    let centre = best.axis;
    for i in -5..=5 {
        let s = solve(tr, angles, centre + i as f64 * 0.2, mag_group)?;
        if s.residual < best.residual {
            best = s;
        }
    }
    Ok(best)
}

pub fn refine(tr: &Tracks, angles: &[f64], centre: f64, mag_group: usize) -> Result<Solution> {
    let mut best = solve(tr, angles, centre, mag_group)?;
    for i in -5..=5 {
        let s = solve(tr, angles, centre + i as f64 * 0.2, mag_group)?;
        if s.residual < best.residual {
            best = s;
        }
    }
    Ok(best)
}
