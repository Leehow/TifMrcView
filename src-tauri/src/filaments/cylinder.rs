//! Sparse hollow-cylinder correlation and orientation-guided tracing.
//!
//! The Hessian supplies candidate regions, not the final centre line. A
//! 60 nm cylinder compares the wall at 10 nm with the inner lumen at 4 nm
//! and exterior at 16 nm. Normalized correlation includes all three bands;
//! a dark sheet or a solid rod alone is not a hollow cylindrical match.
//! A strong fit seeds a trace; local centre/orientation fits continue it.
//! High/low correlation cutoffs use 6/3 robust SD of the candidate field,
//! like the parent ridge detector, with floors for nearly constant fields.
//! The original length, straightness and lumen gates remain mandatory.
//!
//! This is an independent implementation of the cylinder-correlation and
//! orientation-field approach described in the Amira XTracing documentation.
//! Omitting Z-facing radial samples models missing wall visibility; it does
//! not reconstruct missing Fourier information or establish biological identity.

use super::{
    accept, across, dir, dot, polyline_len, sample, MIN_LENGTH_NM, MIN_STRAIGHT, RADIUS_NM, WORK_NM,
};
use rayon::prelude::*;

#[derive(Clone, Copy, Debug)]
struct CylinderFit {
    center: [f32; 3],
    axis: [f32; 3],
    correlation: f32,
    coverage: f32,
}

// Sparse normalized hollow-cylinder correlation. Physical radii 4/10/16 nm,
// axial extent 60 nm. Z-facing radial directions are not evidence when the
// missing wedge erases those walls; this is a real-space visibility model,
// not a Fourier-space missing-wedge reconstruction.
fn cylinder_score(light: &[f32], m: [usize; 3], c: [f32; 3], axis: [f32; 3]) -> CylinderFit {
    let (u, v) = across(axis);
    let mut sum = 0.0f32;
    let mut sum2 = 0.0;
    let mut dot_t = 0.0;
    let mut t2 = 0.0;
    let mut count = 0.0;
    let mut covered = 0.0;
    let mut angles = 0.0;
    for angle in 0..12 {
        let a = angle as f32 * std::f32::consts::TAU / 12.0;
        let radial = std::array::from_fn::<_, 3, _>(|i| u[i] * a.cos() + v[i] * a.sin());
        if radial[2].abs() > 0.8 {
            continue;
        }
        let mut sums = [0.0f32; 3];
        let mut planes = 0;
        for z in -5..=5 {
            let axial = z as f32 * 3.0;
            let p = std::array::from_fn::<_, 3, _>(|i| c[i] + axis[i] * axial);
            let qs =
                [2.0, 5.0, 8.0].map(|r| std::array::from_fn::<_, 3, _>(|i| p[i] + radial[i] * r));
            if qs
                .iter()
                .any(|q| (0..3).any(|i| q[i] < 0.0 || q[i] > (m[i] - 1) as f32))
            {
                return CylinderFit {
                    center: c,
                    axis,
                    correlation: -1.0,
                    coverage: 0.0,
                };
            }
            let values = qs.map(|q| sample(light, m, q));
            for j in 0..3 {
                sum += values[j];
                sum2 += values[j] * values[j];
                let t = if j == 1 { 1.0 } else { -0.5 };
                dot_t += t * values[j];
                t2 += t * t;
                count += 1.0;
                sums[j] += values[j];
            }
            planes += 1;
        }
        let di = (sums[1] - sums[0]) / planes as f32;
        let do_ = (sums[1] - sums[2]) / planes as f32;
        angles += 1.0;
        if di > 0.0 && do_ > 0.0 {
            covered += 1.0;
        }
    }
    CylinderFit {
        center: c,
        axis,
        correlation: dot_t / (t2 * (sum2 - sum * sum / count).max(1e-12)).sqrt(),
        coverage: covered / angles,
    }
}
fn cylinder_fit(
    light: &[f32],
    m: [usize; 3],
    c: [f32; 3],
    axis: [f32; 3],
    initial: bool,
) -> CylinderFit {
    let (u, v) = across(axis);
    let mut best = cylinder_score(light, m, c, axis);
    let steps = if initial { 3 } else { 1 };
    let spacing = if initial { 2.0 } else { 0.75 };
    let turn = if initial { 0.2 } else { 0.1 };
    let axes = [
        [0.0, 0.0],
        [turn, 0.0],
        [-turn, 0.0],
        [0.0, turn],
        [0.0, -turn],
    ]
    .map(|a| {
        let d = std::array::from_fn(|i| axis[i] + a[0] * u[i] + a[1] * v[i]);
        dir([0.0; 3], d)
    });
    for x in -steps..=steps {
        for y in -steps..=steps {
            if initial && (x * x + y * y) as f32 * spacing * spacing > (RADIUS_NM / WORK_NM).powi(2)
            {
                continue;
            }
            let cc = std::array::from_fn(|i| c[i] + spacing * (x as f32 * u[i] + y as f32 * v[i]));
            for &a in &axes {
                let fit = cylinder_score(light, m, cc, a);
                if fit.correlation > best.correlation {
                    best = fit;
                }
            }
        }
    }
    best
}
pub(super) fn trace(light: &[f32], m: [usize; 3], lines: &[Vec<[f32; 3]>]) -> Vec<Vec<[f32; 3]>> {
    let mut seeds = Vec::new();
    for line in lines {
        let length = polyline_len(line);
        let span = (0..3)
            .map(|a| (line.last().unwrap()[a] - line[0][a]).powi(2))
            .sum::<f32>()
            .sqrt();
        if length < MIN_LENGTH_NM / WORK_NM || span < MIN_STRAIGHT * length {
            continue;
        }
        for fraction in [0.25, 0.5, 0.75] {
            let i = ((line.len() - 1) as f32 * fraction).round() as usize;
            let c = line[i];
            let a = dir(
                line[i.saturating_sub(15)],
                line[(i + 15).min(line.len() - 1)],
            );
            if dot(a, a) < 0.5 {
                continue;
            }
            if seeds.iter().any(|s: &([f32; 3], [f32; 3])| {
                (0..3).map(|k| (s.0[k] - c[k]).powi(2)).sum::<f32>() < 16.0
                    && dot(s.1, a).abs() > 0.9
            }) {
                continue;
            }
            seeds.push((c, a));
        }
    }
    let mut fits: Vec<_> = seeds
        .par_iter()
        .map(|&(c, a)| {
            let coarse = cylinder_fit(light, m, c, a, true);
            cylinder_fit(light, m, coarse.center, coarse.axis, false)
        })
        .collect();
    fits.sort_by(|a, b| b.correlation.total_cmp(&a.correlation));

    let mut values: Vec<_> = fits
        .iter()
        .map(|f| f.correlation)
        .filter(|&v| v >= 0.0)
        .collect();
    if values.is_empty() {
        return Vec::new();
    }
    values.sort_by(f32::total_cmp);
    let med = values[values.len() / 2];
    let mut deviations: Vec<_> = values.iter().map(|v| (v - med).abs()).collect();
    deviations.sort_by(f32::total_cmp);
    let sd = 1.4826 * deviations[deviations.len() / 2];
    let seed_threshold = (med + 6.0 * sd).max(0.3);
    let continuation = (med + 3.0 * sd).max(0.2);
    let mut traces: Vec<Vec<[f32; 3]>> = Vec::new();
    for seed in fits {
        if seed.correlation < seed_threshold || seed.coverage < 0.75 {
            continue;
        }
        if traces.iter().any(|line| {
            line.iter().any(|p| {
                (0..3).map(|a| (p[a] - seed.center[a]).powi(2)).sum::<f32>()
                    < (RADIUS_NM / WORK_NM).powi(2)
            })
        }) {
            continue;
        }
        let grow = |sign: f32| {
            let mut fit = seed;
            fit.axis = fit.axis.map(|v| v * sign);
            let mut points = Vec::new();
            for _ in 0..m.iter().sum::<usize>() {
                let predicted = std::array::from_fn(|a| fit.center[a] + 2.0 * fit.axis[a]);
                let next = cylinder_fit(light, m, predicted, fit.axis, false);
                if next.correlation < continuation || next.coverage < 0.75 {
                    break;
                }
                if points.len() > 8
                    && points[..points.len() - 5].iter().any(|p: &[f32; 3]| {
                        (0..3).map(|a| (p[a] - next.center[a]).powi(2)).sum::<f32>() < 4.0
                    })
                {
                    break;
                }
                points.push(next.center);
                fit = next;
            }
            points
        };
        let mut line = grow(-1.0);
        line.reverse();
        line.push(seed.center);
        line.extend(grow(1.0));
        // Existing lumen statistics assume roughly 2nm path samples.
        let mut sampled = vec![line[0]];
        let mut remaining = 1.0;
        for pair in line.windows(2) {
            let delta = std::array::from_fn::<_, 3, _>(|a| pair[1][a] - pair[0][a]);
            let length = dot(delta, delta).sqrt();
            let mut t = remaining;
            while t <= length {
                sampled.push(std::array::from_fn(|a| pair[0][a] + delta[a] * t / length));
                t += 1.0;
            }
            remaining = t - length;
        }
        if let Some(last) = line.last() {
            if sampled.last() != Some(last) {
                sampled.push(*last);
            }
        }
        let accepted = accept(&sampled, light, m, true);
        if accepted {
            traces.push(sampled);
        }
    }
    traces
}

#[cfg(test)]
mod tests {
    use super::*;

    // Already-normalized working-grid density. Removing Z-facing walls is
    // a visibility control, not a simulation of Fourier missing-wedge physics.
    fn tube(axis: [f32; 3], visible_only: bool, solid: bool) -> Vec<f32> {
        let mut values = vec![0.0; 80 * 80 * 80];
        let mut rng = 31u64;
        for z in 0..80 {
            for y in 0..80 {
                for x in 0..80 {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    let v = [x as f32 - 40.0, y as f32 - 40.0, z as f32 - 40.0];
                    let t = dot(v, axis);
                    let radial = std::array::from_fn(|a| v[a] - t * axis[a]);
                    let r = dot(radial, radial).sqrt();
                    let wall = t.abs() < 30.0
                        && r < 6.25
                        && (solid || r > 3.75)
                        && (!visible_only || radial[2].abs() <= 0.8 * r);
                    values[(z * 80 + y) * 80 + x] =
                        if wall { 1.0 } else { 0.0 } + 0.2 * ((rng % 2000) as f32 / 1000.0 - 1.0);
                }
            }
        }
        values
    }

    #[test]
    fn localizes_oblique_hollow_cylinders_with_partly_visible_walls() {
        for axis in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.6, 0.6, 0.5291503]] {
            for visible_only in [false, true] {
                let axis = dir([0.0; 3], axis);
                let values = tube(axis, visible_only, false);
                let (u, v) = across(axis);
                // Start on a wall, with an inaccurate orientation.
                let initial = std::array::from_fn(|a| 40.0 + 3.0 * u[a]);
                let tilted = dir([0.0; 3], std::array::from_fn(|a| axis[a] + 0.1 * v[a]));
                let coarse = cylinder_fit(&values, [80; 3], initial, tilted, true);
                let fit = cylinder_fit(&values, [80; 3], coarse.center, coarse.axis, false);
                let error = fit.center.map(|a| a - 40.0);
                assert!(
                    dot(error, error).sqrt() < 1.5,
                    "{axis:?}, visible={visible_only}: {fit:?}"
                );
                assert!(dot(axis, fit.axis).abs() > 0.98, "{fit:?}");
                assert!(fit.correlation > 0.5 && fit.coverage >= 0.75, "{fit:?}");
                let wrong = cylinder_score(&values, [80; 3], [40.0; 3], u);
                assert!(
                    fit.correlation > wrong.correlation + 0.2,
                    "wrong orientation: {wrong:?}"
                );
            }
        }
    }

    #[test]
    fn solid_rods_and_parallel_sheets_lack_circumferential_lumen_support() {
        let solid = tube([0.0, 0.0, 1.0], false, true);
        let fit = cylinder_score(&solid, [80; 3], [40.0; 3], [0.0, 0.0, 1.0]);
        assert!(fit.coverage < 0.75, "solid cylinder: {fit:?}");
        let mut sheets = vec![0.0; 80 * 80 * 80];
        for z in 0..80 {
            for y in 0..80 {
                for x in [34, 35, 36, 44, 45, 46] {
                    sheets[(z * 80 + y) * 80 + x] = 1.0;
                }
            }
        }
        let fit = cylinder_score(&sheets, [80; 3], [40.0; 3], [0.0, 0.0, 1.0]);
        assert!(fit.coverage < 0.75, "parallel membranes: {fit:?}");
    }
}
