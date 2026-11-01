//! CTF of a tilt series and phase flipping (methods: CTFFIND's fit, which
//! `crate::ctf` implements, applied the way tomography needs it).
//!
//! A tilted specimen has a defocus gradient across the tilt axis: the
//! defocus changes by x·tan θ at a distance x from the axis. Each tilt is
//! fitted on a strip along the axis, narrow enough that the defocus spans
//! at most `SPREAD` Å over it, so its rings do not blur; the fit is the
//! defocus at the axis.
//!
//! The strip fit is then refined on the whole image, as CTFFIND5 does
//! for tilted specimens: every tile's power spectrum is stretched to the
//! defocus at the axis (a tile at defocus D shows at frequency k what the
//! axis shows at k·√(D/D₀)), the stretched spectra are averaged and fitted
//! again, twice, in a band around the strip's value (outliers in the strip
//! fit are first replaced by the median of their neighbouring tilts). Both
//! gradient directions are tried on the steep tilts; the one that fits
//! better is the gradient.
//!
//! A tilt whose fit failed (high tilts often show no usable rings: the fit
//! ends at the search limit or far from the rest of the series) is fitted
//! again around the value interpolated from its good neighbours in tilt
//! angle; if that fails too, it takes the interpolated value (`estimated`).
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
    let fits = crate::ctf::fit_regions(img, &items, 0, 0, n, params, &|i, total| progress(i, 2 * total))?;
    let strip_df: Vec<f64> = fits.iter().map(|f| 0.5 * (f.df1 + f.df2)).collect();
    // Strip fits far from their neighbours' median are replaced by it.
    let start: Vec<f64> = (0..strip_df.len())
        .map(|i| {
            let mut near: Vec<f64> = (i.saturating_sub(3)..(i + 4).min(strip_df.len())).map(|j| strip_df[j]).collect();
            near.sort_by(f64::total_cmp);
            let m = near[near.len() / 2];
            if (strip_df[i] - m).abs() > 0.2 * m { m } else { strip_df[i] }
        })
        .collect();
    let powers: Vec<TilePowers> = (0..d.angles.len())
        .map(|z| {
            progress(d.angles.len() + z, 2 * d.angles.len());
            tile_powers(img, z, n, dir)
        })
        .collect::<Result<_>>()?;
    // Gradient direction: the steeper half of the tilts decides.
    let steep: Vec<usize> = (0..d.angles.len()).filter(|&z| d.angles[z].abs() >= 20.0).collect();
    let score = |sign: f64| -> Result<f64> {
        let scores = steep.par_iter().map(|&z| refine(&powers[z], d.angles[z], start[z], sign, params).map(|f| f.score)).collect::<Result<Vec<_>>>()?;
        Ok(scores.iter().sum())
    };
    let gradient = if steep.is_empty() {
        0i8
    } else {
        let (up, down) = (score(1.0)?, score(-1.0)?);
        if up >= down { 1 } else { -1 }
    };
    let tilts = (0..d.angles.len())
        .into_par_iter()
        .map(|z| {
            let first = refine(&powers[z], d.angles[z], start[z], gradient as f64, params)?;
            let f = refine(&powers[z], d.angles[z], 0.5 * (first.df1 + first.df2), gradient as f64, params)?;
            Ok(TiltCtf { z, df1: f.df1, df2: f.df2, angle: f.angle, fit_res: f.fit_res, score: f.score, estimated: false })
        })
        .collect::<Result<Vec<_>>>()?;
    let tilts = repair(&d.angles, tilts, params, |z, df0| {
        let f = refine(&powers[z], d.angles[z], df0, gradient as f64, params)?;
        Ok(TiltCtf { z, df1: f.df1, df2: f.df2, angle: f.angle, fit_res: f.fit_res, score: f.score, estimated: false })
    })?;
    Ok(Ctf { gradient, tilts })
}

/// Tilts whose fit failed: at the edge of the search range, or far from the
/// rest of the series (beyond 5 robust deviations and 15 % of the median).
fn failed(tilts: &[TiltCtf], params: &Params) -> Vec<bool> {
    let mean = |t: &TiltCtf| 0.5 * (t.df1 + t.df2);
    let mut dfs: Vec<f64> = tilts.iter().map(mean).collect();
    dfs.sort_by(f64::total_cmp);
    let median = dfs[dfs.len() / 2];
    let mut dev: Vec<f64> = dfs.iter().map(|d| (d - median).abs()).collect();
    dev.sort_by(f64::total_cmp);
    let limit = (5.0 * 1.4826 * dev[dev.len() / 2]).max(0.15 * median);
    let edge = 0.02 * (params.df_max_a - params.df_min_a);
    tilts
        .iter()
        .map(|t| {
            let df = mean(t);
            df < params.df_min_a + edge || df > params.df_max_a - edge || (df - median).abs() > limit
        })
        .collect()
}

/// Replaces failed fits (see `failed`): `refit(z, df0)` fits tilt z again
/// around df0, the defocus interpolated in tilt angle between the nearest
/// good tilts either side; a refit that fails or strays more than 15 % from
/// df0 is replaced by df0 itself, with the nearest good tilt's astigmatism.
fn repair(angles: &[f64], mut tilts: Vec<TiltCtf>, params: &Params, refit: impl Fn(usize, f64) -> Result<TiltCtf> + Sync) -> Result<Vec<TiltCtf>> {
    let bad = failed(&tilts, params);
    let good: Vec<usize> = (0..tilts.len()).filter(|&i| !bad[i]).collect();
    if good.is_empty() || good.len() == tilts.len() {
        return Ok(tilts);
    }
    let mean = |t: &TiltCtf| 0.5 * (t.df1 + t.df2);
    let guess = |i: usize| -> (f64, usize) {
        let a = angles[tilts[i].z];
        let below = good.iter().copied().filter(|&j| angles[tilts[j].z] < a).max_by(|&x, &y| angles[tilts[x].z].total_cmp(&angles[tilts[y].z]));
        let above = good.iter().copied().filter(|&j| angles[tilts[j].z] >= a).min_by(|&x, &y| angles[tilts[x].z].total_cmp(&angles[tilts[y].z]));
        match (below, above) {
            (Some(b), Some(c)) => {
                let (ab, ac) = (angles[tilts[b].z], angles[tilts[c].z]);
                let t = if ac > ab { (a - ab) / (ac - ab) } else { 0.5 };
                (mean(&tilts[b]) * (1.0 - t) + mean(&tilts[c]) * t, if t < 0.5 { b } else { c })
            }
            (Some(j), None) | (None, Some(j)) => (mean(&tilts[j]), j),
            (None, None) => unreachable!(),
        }
    };
    let fixes: Vec<(usize, TiltCtf)> = (0..tilts.len())
        .filter(|&i| bad[i])
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|i| {
            let (df0, near) = guess(i);
            let f = refit(tilts[i].z, df0)?;
            let df = mean(&f);
            let edge = 0.02 * (params.df_max_a - params.df_min_a);
            if (df - df0).abs() <= 0.15 * df0 && df > params.df_min_a + edge && df < params.df_max_a - edge {
                return Ok((i, f));
            }
            let n = &tilts[near];
            let half = 0.5 * (n.df1 - n.df2);
            Ok((i, TiltCtf { z: tilts[i].z, df1: df0 + half, df2: df0 - half, angle: n.angle, fit_res: 0.0, score: 0.0, estimated: true }))
        })
        .collect::<Result<_>>()?;
    for (i, t) in fixes {
        tilts[i] = t;
    }
    Ok(tilts)
}

/// Power spectra (unshifted, n × n) of tiles over the whole of one tilt,
/// with each tile centre's distance from the axis (pixels, along `dir`).
struct TilePowers {
    n: usize,
    tiles: Vec<(f64, Vec<f32>)>,
}

fn tile_powers(img: &Arc<OpenImage>, z: usize, n: usize, dir: [f64; 2]) -> Result<TilePowers> {
    let info = img.info();
    let (w, h) = (info.width, info.height);
    let n = n.min(1 << w.min(h).ilog2());
    let (page, sample) = info.locate(0, z, 0);
    let planes = img.page(page)?;
    let p = &planes[sample.min(planes.len() - 1)];
    let (lo, hi) = crate::ctf::clip_range(|i| p.data.get_f64(i) as f32, p.w * p.h);
    let fft = crate::fft::Fft::new(n);
    let win = crate::fft::hann(n);
    let starts = |len: usize| -> Vec<usize> { (0..=(len - n) / (n / 2)).map(|k| k * n / 2).collect() };
    let corners: Vec<(usize, usize)> = starts(h).into_iter().flat_map(|y| starts(w).into_iter().map(move |x| (x, y))).collect();
    let tiles = corners
        .par_iter()
        .map(|&(x0, y0)| {
            let mut tile: Vec<f32> = (0..n * n).map(|i| crate::ctf::clip(p.data.get_f64((y0 + i / n) * p.w + x0 + i % n) as f32, lo, hi)).collect();
            let mean = tile.iter().map(|&v| v as f64).sum::<f64>() / (n * n) as f64;
            for (i, v) in tile.iter_mut().enumerate() {
                *v = (*v - mean as f32) * win[i % n] * win[i / n];
            }
            let c = [x0 as f64 + n as f64 / 2.0 - w as f64 / 2.0, y0 as f64 + n as f64 / 2.0 - h as f64 / 2.0];
            (c[0] * dir[0] + c[1] * dir[1], crate::fft::power_2d(&fft, &tile))
        })
        .collect();
    Ok(TilePowers { n, tiles })
}

/// Fits the average of the tiles' spectra stretched to defocus `df0` at the
/// axis, the gradient running `sign`, searching `df0` ± 25 %.
fn refine(t: &TilePowers, theta: f64, df0: f64, sign: f64, params: &Params) -> Result<crate::ctf::Fit> {
    let n = t.n;
    let slope = sign * params.pixel_a * theta.to_radians().tan();
    let mut sum = vec![0f64; n * n];
    let half = n as f64 / 2.0;
    for (offset, power) in &t.tiles {
        let d = (df0 + slope * offset).max(df0 * 0.2);
        let s = (df0 / d).sqrt();
        // Bilinear lookup in the centred spectrum, unshifted storage.
        let at = |u: f64, v: f64| -> f64 {
            let (u0, v0) = (u.floor(), v.floor());
            let (fu, fv) = (u - u0, v - v0);
            let get = |a: f64, b: f64| -> f64 {
                if a < -half || a >= half || b < -half || b >= half {
                    return 0.0;
                }
                let (iu, iv) = (((a as isize).rem_euclid(n as isize)) as usize, ((b as isize).rem_euclid(n as isize)) as usize);
                power[iv * n + iu] as f64
            };
            (1.0 - fv) * ((1.0 - fu) * get(u0, v0) + fu * get(u0 + 1.0, v0)) + fv * ((1.0 - fu) * get(u0, v0 + 1.0) + fu * get(u0 + 1.0, v0 + 1.0))
        };
        for v in 0..n {
            for u in 0..n {
                let (fu, fv) = (crate::motion::sum::freq(u, n), crate::motion::sum::freq(v, n));
                sum[v * n + u] += at(fu * s, fv * s);
            }
        }
    }
    let spectrum = crate::fft::finish(&sum, n, t.tiles.len());
    let mut p = params.clone();
    p.df_min_a = (df0 * 0.75).max(params.df_min_a);
    p.df_max_a = (df0 * 1.25).min(params.df_max_a);
    if p.df_max_a <= p.df_min_a {
        p.df_min_a = params.df_min_a;
        p.df_max_a = params.df_max_a;
    }
    Ok(crate::ctf::fit_many(&[&spectrum.log], n, &p, &crate::ctf::CpuGrid)?.remove(0))
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
    /// Defocus added everywhere, Å: for 3D CTF correction, the depth of the
    /// slab being reconstructed seen along the beam (see `depth_defocus`).
    pub offset: f64,
}

/// Defocus change (Å) of a point `depth` output pixels above the mid-plane
/// in a view tilted by θ, at the same detector position. A point at detector
/// position d and depth v sits at height h = −d·tan θ + v / cos θ along the
/// beam; the strip flip takes care of the first term (as `gradient`), this
/// is the second, with the same sign convention (defocus grows with −h when
/// the gradient is +1).
pub fn depth_defocus(depth: f64, theta: f64, gradient: i8, pixel_a: f64) -> f64 {
    -(gradient as f64) * depth * pixel_a / theta.to_radians().cos()
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
            let shift = slope * centre + f.offset;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        Params { voltage_kv: 300.0, cs_mm: 2.7, amp_contrast: 0.07, pixel_a: 2.0, res_low_a: 30.0, res_high_a: 5.0, df_min_a: 5000.0, df_max_a: 80000.0, max_astig_a: 2000.0, phase: false, backend: None }
    }

    fn tilt(z: usize, df: f64) -> TiltCtf {
        TiltCtf { z, df1: df + 100.0, df2: df - 100.0, angle: 30.0, fit_res: 8.0, score: 0.2, estimated: false }
    }

    #[test]
    fn failed_high_tilts_take_their_neighbours() {
        let params = params();
        let angles: Vec<f64> = (0..21).map(|i| -60.0 + 6.0 * i as f64).collect();
        // A slow drift in defocus; the two highest tilts hit the search limit.
        let mut tilts: Vec<TiltCtf> = (0..21).map(|z| tilt(z, 39000.0 + 50.0 * z as f64)).collect();
        tilts[0] = tilt(0, 5010.0);
        tilts[20] = tilt(20, 5020.0);
        // The refit fails again for tilt 0 and recovers tilt 20.
        let out = repair(&angles, tilts, &params, |z, df0| Ok(if z == 0 { tilt(z, 5000.0) } else { tilt(z, df0 + 300.0) })).unwrap();
        assert!(out[0].estimated);
        assert!((0.5 * (out[0].df1 + out[0].df2) - 39050.0).abs() < 1.0);
        assert!((out[0].df1 - out[0].df2 - 200.0).abs() < 1e-6);
        assert!(!out[20].estimated);
        assert!((0.5 * (out[20].df1 + out[20].df2) - (39950.0 + 300.0)).abs() < 1.0);
        assert!(out[1..20].iter().all(|t| !t.estimated && t.score == 0.2));
    }

    #[test]
    fn a_good_series_is_left_alone() {
        let params = params();
        let angles: Vec<f64> = (0..11).map(|i| -30.0 + 6.0 * i as f64).collect();
        let tilts: Vec<TiltCtf> = (0..11).map(|z| tilt(z, 38000.0 + 200.0 * (z % 3) as f64)).collect();
        let out = repair(&angles, tilts.clone(), &params, |_, _| panic!("no refit expected")).unwrap();
        assert!(out.iter().zip(&tilts).all(|(a, b)| a.df1 == b.df1 && !a.estimated));
    }
}
