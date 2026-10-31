//! Template-free particle picking: a difference of Gaussians tuned to the
//! particle radius (a blob detector, as in DoG Picker, Voss et al., J
//! Struct Biol 2009), local maxima at least a minimum distance apart,
//! robust z-scores for the threshold, and a large-scale filter that skips
//! ice contamination, carbon edges and other big bright or dark regions.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PickParams {
    /// Particle diameter, Å.
    pub diameter_a: f64,
    /// Particles darker than the background (cryo-EM); false for negative stain.
    pub dark: bool,
    /// Robust z-score a peak must reach.
    pub threshold: f64,
    /// Smallest distance between picks, in particle diameters.
    pub distance: f64,
    /// Skip picks where the large-scale z-score is beyond this (0 = off).
    pub contamination: f64,
    /// Margin from the edges, Å (normally half the extraction box).
    pub margin_a: f64,
    /// Most picks per micrograph (0 = any).
    pub max_per_mic: usize,
}

impl Default for PickParams {
    fn default() -> Self {
        PickParams { diameter_a: 150.0, dark: true, threshold: 3.0, distance: 0.9, contamination: 3.0, margin_a: 0.0, max_per_mic: 0 }
    }
}

/// Block average by `f`.
pub fn bin(img: &[f32], w: usize, h: usize, f: usize) -> (Vec<f32>, usize, usize) {
    if f <= 1 {
        return (img.to_vec(), w, h);
    }
    let (bw, bh) = (w / f, h / f);
    let mut out = vec![0f32; bw * bh];
    out.par_chunks_mut(bw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0f32;
            for dy in 0..f {
                let r = &img[(y * f + dy) * w + x * f..(y * f + dy) * w + x * f + f];
                s += r.iter().sum::<f32>();
            }
            *o = s / (f * f) as f32;
        }
    });
    (out, bw, bh)
}

/// Separable Gaussian blur, edges clamped.
pub fn gauss(img: &[f32], w: usize, h: usize, sigma: f64) -> Vec<f32> {
    if sigma < 0.3 {
        return img.to_vec();
    }
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp() as f32).collect();
    let s: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / s).collect();
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &img[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            let mut a = 0f32;
            for (j, kv) in k.iter().enumerate() {
                let xx = (x as isize + j as isize - r).clamp(0, w as isize - 1) as usize;
                a += kv * src[xx];
            }
            *o = a;
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (j, kv) in k.iter().enumerate() {
            let yy = (y as isize + j as isize - r).clamp(0, h as isize - 1) as usize;
            let src = &tmp[yy * w..(yy + 1) * w];
            for (o, v) in row.iter_mut().zip(src) {
                *o += kv * v;
            }
        }
    });
    out
}

/// Median and 1.4826 × median absolute deviation (a robust σ).
pub fn robust(v: &[f32]) -> (f32, f32) {
    let step = (v.len() / 200_000).max(1);
    let mut s: Vec<f32> = v.iter().step_by(step).copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return (0.0, 1.0);
    }
    s.sort_by(f32::total_cmp);
    let med = s[s.len() / 2];
    let mut d: Vec<f32> = s.iter().map(|x| (x - med).abs()).collect();
    d.sort_by(f32::total_cmp);
    (med, (d[d.len() / 2] * 1.4826).max(1e-12))
}

/// Picks in micrograph pixels: (x, y, z-score), best first.
pub fn pick(img: &[f32], w: usize, h: usize, px: f64, p: &PickParams) -> Vec<[f32; 3]> {
    let r_px = p.diameter_a / 2.0 / px;
    // Bin so the radius is about 10 pixels.
    let f = ((r_px / 10.0).floor() as usize).max(1);
    let (mut b, bw, bh) = bin(img, w, h, f);
    if bw < 8 || bh < 8 {
        return vec![];
    }
    let (med, sd) = robust(&b);
    let sign = if p.dark { -1.0 } else { 1.0 };
    // Clipped so that small, very dark specks (ice crystals, gold) do not
    // outshine the particles.
    b.par_iter_mut().for_each(|v| *v = (sign * (*v - med) / sd).clamp(-4.0, 4.0));
    let r = r_px / f as f64;
    let sigma = r / std::f64::consts::SQRT_2;
    let g1 = gauss(&b, bw, bh, sigma / 1.3);
    let g2 = gauss(&b, bw, bh, sigma * 1.3);
    let resp: Vec<f32> = g1.iter().zip(&g2).map(|(a, c)| a - c).collect();
    // The z-scores are against the noise alone: in a crowded field the
    // spread of the response is mostly particles, which would raise the
    // threshold until only the outliers are left. Shot noise is white, so
    // its level follows from neighbour differences, and the filter's gain
    // on white noise from its response to an impulse.
    let diffs: Vec<f32> = b.chunks(bw).flat_map(|row| row.windows(2).map(|p| p[1] - p[0])).collect();
    let noise = robust(&diffs).1 / std::f32::consts::SQRT_2;
    let gain = {
        let n = (8.0 * sigma).ceil() as usize * 2 + 1;
        let mut d = vec![0f32; n * n];
        d[n / 2 * n + n / 2] = 1.0;
        let g1 = gauss(&d, n, n, sigma / 1.3);
        let g2 = gauss(&d, n, n, sigma * 1.3);
        g1.iter().zip(&g2).map(|(a, c)| (a - c) * (a - c)).sum::<f32>().sqrt()
    };
    let rs = (noise * gain).max(1e-12);
    let rm = robust(&resp).0.min(0.0);
    let z: Vec<f32> = resp.iter().map(|v| (v - rm) / rs).collect();
    // Large-scale background, for contamination.
    let big = if p.contamination > 0.0 {
        let g = gauss(&b, bw, bh, 3.0 * r);
        let (gm, gs) = robust(&g);
        Some(g.into_iter().map(|v| (v - gm) / gs).collect::<Vec<f32>>())
    } else {
        None
    };
    let md = (p.distance * 2.0 * r).max(1.0);
    let win = (md / 2.0).floor().max(1.0) as isize;
    let margin = ((p.margin_a / px / f as f64).ceil() as isize).max(win);
    let thr = p.threshold as f32;
    // Candidates: local maxima in a (2·win+1)² window above the threshold.
    let mut cand: Vec<[f32; 3]> = (margin..bh as isize - margin)
        .into_par_iter()
        .flat_map_iter(|y| {
            let z = &z;
            let big = &big;
            (margin..bw as isize - margin).filter_map(move |x| {
                let v = z[y as usize * bw + x as usize];
                if v < thr {
                    return None;
                }
                if let Some(g) = big {
                    if g[y as usize * bw + x as usize].abs() > p.contamination as f32 {
                        return None;
                    }
                }
                for dy in -win..=win {
                    for dx in -win..=win {
                        let (xx, yy) = (x + dx, y + dy);
                        if xx < 0 || yy < 0 || xx >= bw as isize || yy >= bh as isize || (dx == 0 && dy == 0) {
                            continue;
                        }
                        let u = z[yy as usize * bw + xx as usize];
                        if u > v || (u == v && (dy, dx) < (0, 0)) {
                            return None;
                        }
                    }
                }
                Some([x as f32, y as f32, v])
            })
        })
        .collect();
    cand.sort_by(|a, b| b[2].total_cmp(&a[2]));
    // Greedy suppression within the minimum distance.
    let cell = md.max(1.0);
    let gw = (bw as f64 / cell).ceil() as usize + 1;
    let gh = (bh as f64 / cell).ceil() as usize + 1;
    let mut grid: Vec<Vec<[f32; 2]>> = vec![Vec::new(); gw * gh];
    let mut out = vec![];
    for c in cand {
        let (gx, gy) = ((c[0] as f64 / cell) as usize, (c[1] as f64 / cell) as usize);
        let mut ok = true;
        'n: for yy in gy.saturating_sub(1)..(gy + 2).min(gh) {
            for xx in gx.saturating_sub(1)..(gx + 2).min(gw) {
                for q in &grid[yy * gw + xx] {
                    if ((q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2)) < (md * md) as f32 {
                        ok = false;
                        break 'n;
                    }
                }
            }
        }
        if !ok {
            continue;
        }
        grid[gy * gw + gx].push([c[0], c[1]]);
        // Back to micrograph pixels (centre of the binned pixel).
        out.push([(c[0] + 0.5) * f as f32 - 0.5, (c[1] + 0.5) * f as f32 - 0.5, c[2]]);
        if p.max_per_mic > 0 && out.len() >= p.max_per_mic {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_dark_blobs() {
        let (w, h) = (512, 512);
        let mut img = vec![0f32; w * h];
        let mut seed = 5u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
        };
        let centres: Vec<(f32, f32)> = (0..5).flat_map(|i| (0..5).map(move |j| (60.0 + i as f32 * 95.0, 55.0 + j as f32 * 97.0))).collect();
        for y in 0..h {
            for x in 0..w {
                let mut v = rnd() * 3.0;
                for &(cx, cy) in &centres {
                    let d2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
                    if d2 < 15.0 * 15.0 {
                        v -= 1.0;
                    }
                }
                img[y * w + x] = v;
            }
        }
        let p = PickParams { diameter_a: 30.0, margin_a: 20.0, ..Default::default() };
        let picks = pick(&img, w, h, 1.0, &p);
        let hit = centres.iter().filter(|c| picks.iter().any(|q| (q[0] - c.0).hypot(q[1] - c.1) < 5.0)).count();
        assert!(hit >= 23, "found {hit} of 25 in {} picks", picks.len());
        assert!(picks.len() <= 30, "{} picks", picks.len());
    }
}
