//! Particle extraction: a power-of-two box around each pick, contrast
//! inverted for cryo-EM (protein bright), a plane fitted to the background
//! ring removed, Fourier-binned to the output box, and normalised to mean 0
//! and σ 1 outside the particle (the background normalisation of RELION).

use anyhow::Result;
use rayon::prelude::*;

use super::data::normalise;
use super::fourier;

/// Extraction box at the micrograph pixel (power of two, at least 1.5
/// diameters) and the output box after Fourier binning (at most `max_out`).
pub fn boxes(diameter_a: f64, px: f64, max_out: usize) -> (usize, usize) {
    let want = (1.5 * diameter_a / px).ceil().max(16.0) as usize;
    let big = want.next_power_of_two();
    let out = big.min(max_out.next_power_of_two().max(16));
    (big, out)
}

/// Cuts a box of side `big` centred on (x, y), filling outside the
/// micrograph with the mean of what is inside.
pub fn cut(img: &[f32], w: usize, h: usize, x: f32, y: f32, big: usize) -> Vec<f32> {
    let (x0, y0) = (
        x.round() as isize - (big / 2) as isize,
        y.round() as isize - (big / 2) as isize,
    );
    let mut out = vec![f32::NAN; big * big];
    let (mut s, mut n) = (0f64, 0usize);
    for j in 0..big {
        let yy = y0 + j as isize;
        if yy < 0 || yy >= h as isize {
            continue;
        }
        for i in 0..big {
            let xx = x0 + i as isize;
            if xx < 0 || xx >= w as isize {
                continue;
            }
            let v = img[yy as usize * w + xx as usize];
            out[j * big + i] = v;
            s += v as f64;
            n += 1;
        }
    }
    let mean = if n > 0 { (s / n as f64) as f32 } else { 0.0 };
    for v in &mut out {
        if v.is_nan() {
            *v = mean;
        }
    }
    out
}

/// Subtracts the least-squares plane through the pixels outside `radius`.
pub fn remove_plane(img: &mut [f32], b: usize, radius: f64) {
    let c = b as f64 / 2.0;
    // Normal equations for v = a + bx + cy.
    let mut m = [[0f64; 3]; 3];
    let mut r = [0f64; 3];
    for y in 0..b {
        for x in 0..b {
            let (dx, dy) = (x as f64 - c, y as f64 - c);
            if dx * dx + dy * dy <= radius * radius {
                continue;
            }
            let f = [1.0, dx, dy];
            let v = img[y * b + x] as f64;
            for i in 0..3 {
                r[i] += f[i] * v;
                for j in 0..3 {
                    m[i][j] += f[i] * f[j];
                }
            }
        }
    }
    let Some(s) = solve3(m, r) else { return };
    for y in 0..b {
        for x in 0..b {
            let (dx, dy) = (x as f64 - c, y as f64 - c);
            img[y * b + x] -= (s[0] + s[1] * dx + s[2] * dy) as f32;
        }
    }
}

fn solve3(m: [[f64; 3]; 3], r: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(&m);
    if d.abs() < 1e-12 {
        return None;
    }
    let mut out = [0f64; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let mut mi = m;
        for (j, row) in mi.iter_mut().enumerate() {
            row[i] = r[j];
        }
        *o = det(&mi) / d;
    }
    Some(out)
}

/// Particles of one micrograph: `big` box cut at each pick, binned to `out`.
pub fn particles(
    img: &[f32],
    w: usize,
    h: usize,
    picks: &[[f32; 2]],
    big: usize,
    out: usize,
    diameter_px_out: f64,
    invert: bool,
) -> Result<Vec<Vec<f32>>> {
    let radius_big = diameter_px_out / 2.0 * (big as f64 / out as f64) * 1.1;
    let radius_out = diameter_px_out / 2.0 * 1.1;
    Ok(picks
        .par_iter()
        .map(|p| {
            let mut b = cut(img, w, h, p[0], p[1], big);
            if invert {
                b.iter_mut().for_each(|v| *v = -*v);
            }
            remove_plane(&mut b, big, radius_big.min(big as f64 * 0.45));
            let mut o = fourier::resample2(&b, big, out);
            normalise(&mut o, out, radius_out.min(out as f64 * 0.45));
            o
        })
        .collect())
}
