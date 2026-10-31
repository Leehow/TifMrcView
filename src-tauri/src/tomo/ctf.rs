//! CTF of a tilt series and phase flipping (methods: CTFFIND's fit, which
//! `crate::ctf` implements, applied the way tomography needs it).
//!
//! A tilted specimen has a defocus gradient across the tilt axis: the
//! defocus changes by x·tan θ at a distance x from the axis. Each tilt is
//! fitted on a strip along the axis, narrow enough that the defocus spans
//! at most `SPREAD` Å over it, so its rings do not blur; the fit is the
//! defocus at the axis. Which way the gradient runs depends on the
//! microscope and camera, so it is measured: on the most tilted views, two
//! strips on either side of the axis are fitted and the sign of their
//! difference (against tan θ) is voted on.
//!
//! Phase flipping works on the aligned views (axis vertical): strips
//! parallel to the axis, narrow enough that the defocus changes by at most
//! `FLIP_STEP` Å over one, each Fourier transformed with a margin, multiplied
//! by the sign of sin γ at its own defocus and transformed back.

use super::dataset::{Ctf, Dataset, TiltCtf};
use crate::ctf::{Optics, Params};
use crate::image::OpenImage;
use anyhow::{bail, Result};
use rayon::prelude::*;
use std::sync::Arc;

/// Largest defocus range over one fitting strip, Å.
const SPREAD: f64 = 1500.0;
/// Largest defocus change over one phase-flip strip, Å.
const FLIP_STEP: f64 = 100.0;
/// Margin either side of a flip strip, pixels (the CTF delocalises detail).
const MARGIN: usize = 32;

/// Direction perpendicular to the tilt axis in the stack's image, as a unit
/// vector: where +x of the aligned view points (see `output::aligned`).
fn across(axis: f64) -> [f64; 2] {
    let (s, c) = (90.0 - axis).to_radians().sin_cos();
    [c, s]
}

/// A square-ish region of side `side` centred at (cx, cy), inside the image.
fn region(cx: f64, cy: f64, wd: f64, ht: f64, w: usize, h: usize) -> [f64; 4] {
    let wd = wd.min(w as f64);
    let ht = ht.min(h as f64);
    let x = (cx - wd / 2.0).clamp(0.0, w as f64 - wd);
    let y = (cy - ht / 2.0).clamp(0.0, h as f64 - ht);
    [x.round(), y.round(), wd.round(), ht.round()]
}

/// Fits every tilt of the dataset's stack. `params.pixel_a` is the stack's.
pub fn fit_series(
    img: &Arc<OpenImage>,
    d: &Dataset,
    params: &Params,
    n: usize,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Ctf> {
    let info = img.info();
    let (w, h) = (info.width, info.height);
    if d.angles.len() != info.size_z {
        bail!("倾转角数与叠层不一致");
    }
    let axis = d.alignment.as_ref().map(|a| a.axis).or(d.axis_hint).unwrap_or(90.0);
    let dir = across(axis);
    let vertical = dir[0].abs() >= dir[1].abs();
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let side = (w.min(h) as f64).max(n as f64);
    // Strip width across the axis; multiples of n so tilts share batches.
    let strip = |theta: f64| -> f64 {
        let t = theta.to_radians().tan().abs().max(1e-6);
        let wd = (SPREAD / (params.pixel_a * t)).max(n as f64);
        ((wd / n as f64).floor() * n as f64).min(side)
    };
    let items: Vec<(usize, [f64; 4])> = d
        .angles
        .iter()
        .enumerate()
        .map(|(z, &a)| {
            let s = strip(a);
            let r = if vertical { region(cx, cy, s, h as f64, w, h) } else { region(cx, cy, w as f64, s, w, h) };
            (z, r)
        })
        .collect();
    let fits = crate::ctf::fit_regions(img, &items, 0, 0, n, params, progress)?;
    let tilts: Vec<TiltCtf> = fits
        .iter()
        .enumerate()
        .map(|(z, f)| TiltCtf { z, df1: f.df1, df2: f.df2, angle: f.angle, fit_res: f.fit_res, score: f.score })
        .collect();
    Ok(Ctf { gradient: gradient(img, d, params, n, axis)?, tilts })
}

/// The sign of the defocus gradient, from the most tilted views.
fn gradient(img: &Arc<OpenImage>, d: &Dataset, params: &Params, n: usize, axis: f64) -> Result<i8> {
    let info = img.info();
    let (w, h) = (info.width, info.height);
    let dir = across(axis);
    let offset = (w.min(h) as f64 / 4.0).max(n as f64 * 0.75);
    let mut order: Vec<usize> = (0..d.angles.len()).filter(|&z| d.angles[z].abs() >= 30.0).collect();
    order.sort_by(|&a, &b| d.angles[b].abs().total_cmp(&d.angles[a].abs()));
    order.truncate(4);
    if order.is_empty() {
        return Ok(0);
    }
    let mut items = Vec::new();
    for &z in &order {
        for side in [-1.0, 1.0] {
            let (x, y) = (w as f64 / 2.0 + side * offset * dir[0], h as f64 / 2.0 + side * offset * dir[1]);
            items.push((z, region(x, y, n as f64, n as f64, w, h)));
        }
    }
    let fits = crate::ctf::fit_regions(img, &items, 0, 0, n, params, &|_, _| {})?;
    let mut vote = 0i32;
    for (k, &z) in order.iter().enumerate() {
        let (a, b) = (&fits[2 * k], &fits[2 * k + 1]);
        if a.score <= 0.0 || b.score <= 0.0 {
            continue;
        }
        let diff = 0.5 * (b.df1 + b.df2) - 0.5 * (a.df1 + a.df2);
        let expected = 2.0 * offset * params.pixel_a * d.angles[z].to_radians().tan();
        // Count a vote only when the difference is at least a third of the geometry's.
        if diff.abs() >= expected.abs() / 3.0 {
            vote += if diff.signum() == expected.signum() { 1 } else { -1 };
        }
    }
    Ok(vote.signum() as i8)
}

/// What phase flipping needs for one aligned view.
pub(crate) struct Flip {
    pub optics: Optics,
    /// Defocus at the axis, Å, and the direction of df1 in the aligned view (rad).
    pub df1: f64,
    pub df2: f64,
    pub angle: f64,
    pub theta: f64,
    pub gradient: i8,
    /// Pixel size of the aligned view, Å.
    pub pixel_a: f64,
}

/// Phase flips an aligned view (axis vertical) in strips along the axis.
pub(crate) fn flip(plane: &mut [f32], w: usize, h: usize, f: &Flip) {
    let tan = f.theta.to_radians().tan();
    let slope = f.gradient as f64 * f.pixel_a * tan;
    let step = if slope.abs() > 1e-9 { (FLIP_STEP / slope.abs()).floor() as usize } else { w };
    let step = step.clamp(16, w);
    let tile = (step + 2 * MARGIN).next_power_of_two().min(w.next_power_of_two().max(16));
    let th = h.next_power_of_two();
    let fft = crate::motion::fourier::Cpu::new(tile, th);
    let mean = (plane.iter().map(|&v| v as f64).sum::<f64>() / plane.len() as f64) as f32;
    let starts: Vec<usize> = (0..w).step_by(step).collect();
    let src = plane.to_vec();
    let strips: Vec<(usize, Vec<f32>)> = starts
        .par_iter()
        .map(|&x0| {
            let width = step.min(w - x0);
            let left = x0 as isize - ((tile - width) / 2) as isize;
            let mut re = vec![mean; tile * th];
            let mut im = vec![0.0; tile * th];
            for y in 0..h {
                for i in 0..tile {
                    let x = left + i as isize;
                    if x >= 0 && (x as usize) < w {
                        re[y * tile + i] = src[y * w + x as usize];
                    }
                }
            }
            fft.transform(&mut re, &mut im, false);
            let centre = x0 as f64 + width as f64 / 2.0 - (w as f64 - 1.0) / 2.0;
            let shift = slope * centre;
            for ky in 0..th {
                let fy = crate::motion::sum::freq(ky, th) / (th as f64 * f.pixel_a);
                for kx in 0..tile {
                    let fx = crate::motion::sum::freq(kx, tile) / (tile as f64 * f.pixel_a);
                    let s = f.optics.flip(f.df1 + shift, f.df2 + shift, f.angle, fx, fy);
                    re[ky * tile + kx] *= s;
                    im[ky * tile + kx] *= s;
                }
            }
            fft.transform(&mut re, &mut im, true);
            let off = (x0 as isize - left) as usize;
            let mut out = vec![0.0; width * h];
            for y in 0..h {
                out[y * width..(y + 1) * width].copy_from_slice(&re[y * tile + off..y * tile + off + width]);
            }
            (x0, out)
        })
        .collect();
    for (x0, out) in strips {
        let width = out.len() / h;
        for y in 0..h {
            plane[y * w + x0..y * w + x0 + width].copy_from_slice(&out[y * width..(y + 1) * width]);
        }
    }
}
