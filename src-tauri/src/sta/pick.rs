//! Template-free particle finding in a tomogram: blobs of several sizes as
//! local maxima of a difference-of-Gaussians scale space (the 3D form of
//! the blob detector `spa::pick` and `spa::discover` use on micrographs;
//! Lindeberg, Int J Comput Vis 30, 1998; Voss et al., J Struct Biol 2009).
//!
//! For a ball of radius r the response G(σ) − G(1.6σ) is largest at its
//! centre when σ ≈ r/√3, so each diameter asked for has its own σ. The
//! responses are turned into robust z-scores per size (median and MAD of
//! the whole tomogram), which makes sizes comparable; a voxel is a blob
//! centre when it is the largest of its 26 neighbours and scores at least
//! `min_z`. Where blobs of any sizes overlap, the strongest is kept.

use rayon::prelude::*;

use super::check_cancel;
use anyhow::Result;

#[derive(Clone, Copy, Debug)]
pub struct Blob {
    /// Centre in voxels, (x, y, z).
    pub pos: [f32; 3],
    /// Diameter in voxels (the size whose response was strongest).
    pub diameter: f32,
    /// Robust z-score of the response.
    pub score: f32,
}

fn kernel(sigma: f64) -> Vec<f32> {
    let r = (3.0 * sigma).ceil().max(1.0) as isize;
    let k: Vec<f64> = (-r..=r).map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp()).collect();
    let s: f64 = k.iter().sum();
    k.iter().map(|v| (v / s) as f32).collect()
}

/// Separable Gaussian blur of a volume (z, y, x order; edges repeated).
pub fn gauss(v: &[f32], dims: [usize; 3], sigma: f64) -> Vec<f32> {
    let [nx, ny, nz] = dims;
    let k = kernel(sigma);
    let r = (k.len() / 2) as isize;
    let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
    let mut a = vec![0f32; v.len()];
    a.par_chunks_mut(nx).zip(v.par_chunks(nx)).for_each(|(o, row)| {
        for x in 0..nx {
            o[x] = k.iter().enumerate().map(|(j, w)| w * row[clamp(x as isize + j as isize - r, nx)]).sum();
        }
    });
    let mut b = vec![0f32; v.len()];
    b.par_chunks_mut(nx * ny).zip(a.par_chunks(nx * ny)).for_each(|(o, plane)| {
        for y in 0..ny {
            for (j, w) in k.iter().enumerate() {
                let src = clamp(y as isize + j as isize - r, ny) * nx;
                for x in 0..nx {
                    o[y * nx + x] += w * plane[src + x];
                }
            }
        }
    });
    a.par_chunks_mut(nx * ny).enumerate().for_each(|(z, o)| {
        o.fill(0.0);
        for (j, w) in k.iter().enumerate() {
            let src = clamp(z as isize + j as isize - r, nz) * nx * ny;
            for i in 0..nx * ny {
                o[i] += w * b[src + i];
            }
        }
    });
    a
}

/// Median and MAD-based standard deviation from an even subsample.
fn robust(v: &[f32]) -> (f32, f32) {
    let step = (v.len() / 500_000).max(1);
    let mut s: Vec<f32> = v.iter().step_by(step).copied().collect();
    s.sort_by(f32::total_cmp);
    let med = s[s.len() / 2];
    let mut d: Vec<f32> = s.iter().map(|x| (x - med).abs()).collect();
    d.sort_by(f32::total_cmp);
    (med, (1.4826 * d[d.len() / 2]).max(1e-12))
}

/// Blobs of the given diameters (voxels). `dark` when particles are darker
/// than their surroundings, as in cryo-ET tomograms. Strongest first.
pub fn blobs(v: &[f32], dims: [usize; 3], diameters: &[f64], dark: bool, min_z: f32) -> Result<Vec<Blob>> {
    let [nx, ny, nz] = dims;
    assert_eq!(v.len(), nx * ny * nz);
    let mut found: Vec<Blob> = vec![];
    for &d in diameters {
        check_cancel()?;
        let sigma = d / 2.0 / 3f64.sqrt();
        let (fine, coarse) = (gauss(v, dims, sigma), gauss(v, dims, 1.6 * sigma));
        let sign = if dark { -1.0 } else { 1.0 };
        let dog: Vec<f32> = fine.par_iter().zip(&coarse).map(|(a, b)| sign * (a - b)).collect();
        let (med, sd) = robust(&dog);
        let at = |x: usize, y: usize, z: usize| dog[(z * ny + y) * nx + x];
        let layer: Vec<Blob> = (1..nz.saturating_sub(1))
            .into_par_iter()
            .flat_map_iter(|z| {
                let mut out = vec![];
                for y in 1..ny - 1 {
                    for x in 1..nx - 1 {
                        let c = at(x, y, z);
                        let score = (c - med) / sd;
                        if score < min_z {
                            continue;
                        }
                        let mut peak = true;
                        'n: for k in 0..3 {
                            for j in 0..3 {
                                for i in 0..3 {
                                    if (i, j, k) != (1, 1, 1) && at(x + i - 1, y + j - 1, z + k - 1) > c {
                                        peak = false;
                                        break 'n;
                                    }
                                }
                            }
                        }
                        if peak {
                            out.push(Blob { pos: [x as f32, y as f32, z as f32], diameter: d as f32, score });
                        }
                    }
                }
                out
            })
            .collect();
        found.extend(layer);
    }
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    // Strongest first; a weaker blob whose centre lies inside a kept one
    // (or that holds a kept one's centre) is the same object.
    let mut kept: Vec<Blob> = vec![];
    for b in found {
        let clash = kept.iter().any(|k| {
            let d2: f32 = (0..3).map(|i| (k.pos[i] - b.pos[i]).powi(2)).sum();
            let r = 0.5 * k.diameter.max(b.diameter);
            d2 < r * r
        });
        if !clash {
            kept.push(b);
        }
    }
    Ok(kept)
}
