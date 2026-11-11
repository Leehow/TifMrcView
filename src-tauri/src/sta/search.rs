//! Looking for a template everywhere in a tomogram, at every orientation
//! of a list: the one search behind all ways of choosing particles
//! (a class average of found blobs, a few clicked examples, what lies on a
//! drawn shape, a map the user has).
//!
//! The score at a place is the correlation of the template with the
//! tomogram there, divided by the tomogram's own standard deviation under
//! the template's mask (locally normalised correlation, Roseman,
//! Ultramicroscopy 94, 2003), so that dense things do not win just by
//! being dense. The template is given the tomogram's missing wedge at each
//! orientation (Frangakis et al., PNAS 99, 2002; Förster et al., 2005).
//!
//! What score makes a particle is not a number to tune: `cutoff` splits the
//! peaks of the search into two groups, high scores (particles) and low
//! ones (everything else), where the groups are most distinct (Otsu), and
//! gives up when they are not. (A Gumbel fit to the chance peaks was tried
//! and failed on real data: its threshold lay above the best score.) The
//! search itself is kept off the tomogram's X/Y borders and off gold
//! markers by `allowed`, where reconstruction artefacts and gold outscore
//! particles.
//!
//! The tomogram is worked through in overlapping cubes of side `TILE`, so
//! the radix-2 transforms of `spa::fourier` do and memory stays small. The
//! same layout is what a GPU version works through in batches.

use rayon::prelude::*;

use super::cc::{ball, rotate, wedge};
use super::check_cancel;
use super::pick::robust;
use super::project::{split, GOLD};
use crate::spa::fourier::{ft3, ift3};
use crate::spa::geom::Mat;
use anyhow::Result;

/// Side of the cubes the tomogram is worked through in.
const TILE: usize = 64;
/// Orientations whose transforms are held at once.
const BATCH: usize = 24;

pub struct Found {
    /// Best score over the orientations at every voxel (z, y, x order);
    /// `f32::MIN` where nothing was looked for.
    pub score: Vec<f32>,
    /// Which orientation gave it.
    pub turn: Vec<u32>,
    pub dims: [usize; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct Peak {
    /// Voxel, (x, y, z).
    pub pos: [usize; 3],
    pub score: f32,
    /// Index into the list of orientations.
    pub turn: usize,
}

/// The template (an n³ cube, particle at the centre, n ≤ TILE / 2) at one
/// orientation, masked, wedged, zero mean, as the transform of a TILE³ cube.
fn turned(template: &[f32], n: usize, mask: &[f32], wedge: &[f32], r: &Mat) -> (Vec<f32>, Vec<f32>, f64) {
    let t = TILE;
    let v = rotate(template, n, r);
    let (mut sum, mut weight) = (0f64, 0f64);
    for i in 0..v.len() {
        sum += (v[i] * mask[i]) as f64;
        weight += mask[i] as f64;
    }
    let mean = (sum / weight) as f32;
    let mut cube = vec![0f32; t * t * t];
    let o = (t - n) / 2;
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let i = (z * n + y) * n + x;
                cube[((z + o) * t + y + o) * t + x + o] = (v[i] - mean) * mask[i];
            }
        }
    }
    let (mut re, mut im) = ft3(t, &cube);
    let mut power = 0f64;
    for i in 0..re.len() {
        re[i] *= wedge[i];
        im[i] *= wedge[i];
        power += (re[i] * re[i] + im[i] * im[i]) as f64;
    }
    // No mean: the level of the tomogram does not enter the score.
    power -= (re[0] * re[0] + im[0] * im[0]) as f64;
    re[0] = 0.0;
    im[0] = 0.0;
    // Σ t² in real space (the transforms are not scaled).
    (re, im, power / (t * t * t) as f64)
}

/// Searches `v` (z, y, x order) for `template` at each of `turns`.
/// `radius`: the particle's, in voxels (the mask). `tilts`: first and last
/// tilt of the tomogram, degrees. `allowed`: where a particle's centre may
/// lie (None: anywhere); tiles with no allowed voxel are skipped.
#[allow(clippy::too_many_arguments)]
pub fn search(
    v: &[f32],
    dims: [usize; 3],
    template: &[f32],
    n: usize,
    radius: f64,
    tilts: [f64; 2],
    turns: &[Mat],
    allowed: Option<&[bool]>,
    tick: &(dyn Fn(usize, usize) + Sync),
) -> Result<Found> {
    let t = TILE;
    assert!(n.is_power_of_two() && n <= t / 2 && template.len() == n * n * n);
    let [nx, ny, nz] = dims;
    assert_eq!(v.len(), nx * ny * nz);
    let mask = ball(n, radius.min(n as f64 / 2.0 - 2.0), 2.0);
    let wedge = wedge(t, tilts[0], tilts[1]);
    let count: f64 = mask.iter().map(|m| *m as f64).sum();
    // The mask as a TILE³ transform, for the local mean and variance.
    let o = (t - n) / 2;
    let mut big = vec![0f32; t * t * t];
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                big[((z + o) * t + y + o) * t + x + o] = mask[(z * n + y) * n + x];
            }
        }
    }
    let (mask_re, mask_im) = ft3(t, &big);
    let mean = (v.iter().map(|x| *x as f64).sum::<f64>() / v.len() as f64) as f32;

    // Each tile answers for the centres in its middle half.
    let step = t / 2;
    let along = |len: usize| -> Vec<isize> { (0..len.div_ceil(step)).map(|k| (k * step) as isize - (step / 2) as isize).collect() };
    let mut tiles: Vec<[isize; 3]> = vec![];
    for oz in along(nz) {
        for oy in along(ny) {
            for ox in along(nx) {
                tiles.push([ox, oy, oz]);
            }
        }
    }
    // The part of the volume a tile answers for: lo..hi per axis.
    let own = |o: [isize; 3]| -> [(usize, usize); 3] {
        std::array::from_fn(|i| {
            let lo = (o[i] + (step / 2) as isize).max(0) as usize;
            (lo, (lo + step).min(dims[i]))
        })
    };
    if let Some(a) = allowed {
        tiles.retain(|o| {
            let [(x0, x1), (y0, y1), (z0, z1)] = own(*o);
            (z0..z1).any(|z| (y0..y1).any(|y| a[(z * ny + y) * nx + x0..(z * ny + y) * nx + x1].iter().any(|b| *b)))
        });
    }
    let fill = |o: [isize; 3]| -> Vec<f32> {
        let mut cube = vec![mean; t * t * t];
        for z in 0..t {
            let sz = o[2] + z as isize;
            if sz < 0 || sz >= nz as isize {
                continue;
            }
            for y in 0..t {
                let sy = o[1] + y as isize;
                if sy < 0 || sy >= ny as isize {
                    continue;
                }
                let (x0, x1) = ((-o[0]).max(0) as usize, ((nx as isize - o[0]).min(t as isize)).max(0) as usize);
                if x0 < x1 {
                    let src = (sz as usize * ny + sy as usize) * nx;
                    cube[(z * t + y) * t + x0..(z * t + y) * t + x1].copy_from_slice(&v[(src as isize + o[0] + x0 as isize) as usize..(src as isize + o[0] + x1 as isize) as usize]);
                }
            }
        }
        cube
    };
    // conj(B)·A at every frequency, back in real space: Σ a(x + s)·b(x).
    let correlate = |ar: &[f32], ai: &[f32], br: &[f32], bi: &[f32]| -> Vec<f32> {
        let mut cr = vec![0f32; ar.len()];
        let mut ci = vec![0f32; ar.len()];
        for i in 0..ar.len() {
            cr[i] = ar[i] * br[i] + ai[i] * bi[i];
            ci[i] = ai[i] * br[i] - ar[i] * bi[i];
        }
        ift3(t, &cr, &ci)
    };

    let mut score = vec![f32::MIN; v.len()];
    let mut turn = vec![0u32; v.len()];
    // Per tile: its best scores so far, over its own part only.
    let mut best: Vec<(Vec<f32>, Vec<u32>)> = tiles
        .iter()
        .map(|o| {
            let [(x0, x1), (y0, y1), (z0, z1)] = own(*o);
            let len = (x1 - x0) * (y1 - y0) * (z1 - z0);
            (vec![f32::MIN; len], vec![0u32; len])
        })
        .collect();
    let batches = turns.len().div_ceil(BATCH);
    for (b, chunk) in turns.chunks(BATCH).enumerate() {
        check_cancel()?;
        let prepared: Vec<_> = chunk.par_iter().map(|r| turned(template, n, &mask, &wedge, r)).collect();
        tiles.par_iter().zip(best.par_iter_mut()).for_each(|(o, (best_score, best_turn))| {
            let cube = fill(*o);
            let (dr, di) = ft3(t, &cube);
            // Standard deviation of the tomogram under the mask at each shift.
            let m1 = correlate(&dr, &di, &mask_re, &mask_im);
            let squared: Vec<f32> = cube.iter().map(|x| x * x).collect();
            let (sr, si) = ft3(t, &squared);
            let m2 = correlate(&sr, &si, &mask_re, &mask_im);
            let [(x0, x1), (y0, y1), (z0, z1)] = own(*o);
            // Volume voxel p is shift p − o − TILE/2, stored at index p − o.
            let at = |x: usize, y: usize, z: usize| {
                (((z as isize - o[2]) as usize * t) + (y as isize - o[1]) as usize) * t + (x as isize - o[0]) as usize
            };
            for (k, (tr, ti, power)) in prepared.iter().enumerate() {
                if *power <= 0.0 {
                    continue;
                }
                let cc = correlate(&dr, &di, tr, ti);
                let mut i = 0;
                for z in z0..z1 {
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let j = at(x, y, z);
                            let var = (m2[j] as f64 / count - (m1[j] as f64 / count).powi(2)).max(1e-12);
                            let s = (cc[j] as f64 / (count * var * power).sqrt()) as f32;
                            if s > best_score[i] {
                                best_score[i] = s;
                                best_turn[i] = (b * BATCH + k) as u32;
                            }
                            i += 1;
                        }
                    }
                }
            }
        });
        tick(b + 1, batches);
    }
    for (o, (s, r)) in tiles.iter().zip(&best) {
        let [(x0, x1), (y0, y1), (z0, z1)] = own(*o);
        let mut i = 0;
        for z in z0..z1 {
            for y in y0..y1 {
                let row = (z * ny + y) * nx;
                let len = x1 - x0;
                score[row + x0..row + x1].copy_from_slice(&s[i..i + len]);
                turn[row + x0..row + x1].copy_from_slice(&r[i..i + len]);
                i += len;
            }
        }
    }
    if let Some(a) = allowed {
        score.iter_mut().zip(a).for_each(|(s, ok)| {
            if !ok {
                *s = f32::MIN;
            }
        });
    }
    Ok(Found { score, turn, dims })
}

/// Local maxima of the score at least `apart` voxels from any better one,
/// best first, at most `most`.
pub fn peaks(found: &Found, apart: f64, most: usize) -> Vec<Peak> {
    let [nx, ny, nz] = found.dims;
    let s = &found.score;
    let mut all: Vec<usize> = (0..s.len())
        .into_par_iter()
        .filter(|&i| {
            let c = s[i];
            if c == f32::MIN {
                return false;
            }
            let (x, y, z) = (i % nx, i / nx % ny, i / nx / ny);
            for dz in -1isize..=1 {
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        let (a, b, d) = (x as isize + dx, y as isize + dy, z as isize + dz);
                        if a < 0 || b < 0 || d < 0 || a >= nx as isize || b >= ny as isize || d >= nz as isize {
                            continue;
                        }
                        if s[(d as usize * ny + b as usize) * nx + a as usize] > c {
                            return false;
                        }
                    }
                }
            }
            true
        })
        .collect();
    all.sort_by(|a, b| s[*b].total_cmp(&s[*a]));
    let cell = apart.max(1.0);
    let key = |p: [usize; 3]| [(p[0] as f64 / cell) as i64, (p[1] as f64 / cell) as i64, (p[2] as f64 / cell) as i64];
    let mut grid: std::collections::HashMap<[i64; 3], Vec<[usize; 3]>> = Default::default();
    let mut out = vec![];
    for i in all {
        if out.len() >= most {
            break;
        }
        let p = [i % nx, i / nx % ny, i / nx / ny];
        let k = key(p);
        let mut clash = false;
        'look: for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(list) = grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                        if list.iter().any(|q| (0..3).map(|a| (q[a] as f64 - p[a] as f64).powi(2)).sum::<f64>() < apart * apart) {
                            clash = true;
                            break 'look;
                        }
                    }
                }
            }
        }
        if !clash {
            grid.entry(k).or_default().push(p);
            out.push(Peak { pos: p, score: s[i], turn: found.turn[i] as usize });
        }
    }
    out
}

/// The score above which a peak is a particle. The peaks of a search fall
/// in two groups: places that hold a particle, which score high, and the
/// rest, which score lower and spread wider. The scores are split where the
/// groups are most distinct (Otsu, `project::split`), on Fisher's z scale
/// (atanh) as for any correlation, which keeps the wide low group from
/// pulling the threshold down into the particles. None when there are too
/// few peaks, or when the split explains no more of the spread than chance
/// does in one group alone: for one bell curve or one extreme-value curve
/// the share is 0.62 to 0.64 (less than 0.66 with 1000 peaks or more, it
/// scatters upward by about 1 / sqrt(n)); two groups with the quantiles
/// measured on EMPIAR-10045 give 0.72.
pub fn cutoff(all: &[Peak]) -> Option<f32> {
    if all.len() < 40 {
        return None;
    }
    let z: Vec<f32> = all.iter().map(|p| p.score.clamp(-0.999, 0.999).atanh()).collect();
    let (threshold, share) = split(&z);
    (share >= 0.62 + 1.2 / (all.len() as f64).sqrt()).then_some(threshold.tanh())
}

/// At most this many scores of a search are sorted for `above_noise`.
const NOISE_SAMPLE: usize = 2_000_000;
/// At least this many peaks have to stand out of the noise for `sure` to
/// take them without a second group to part them from.
const FEWEST_ABOVE: usize = 20;

/// The score a peak has to pass to stand out of what the search scores
/// where there is nothing, or None when the search holds too little to
/// say. A voxel's score is the best over many orientations, so where there
/// is only noise the scores follow an extreme-value (Gumbel) curve: its
/// location and scale are read from the quartiles of all the scores (the
/// bulk of a tomogram is not particle centres, however many particles
/// there are), and the score returned is the one that one voxel in the
/// whole search is expected to pass by chance.
pub fn above_noise(found: &Found) -> Option<f32> {
    let all = found.score.iter().filter(|s| **s != f32::MIN).count();
    let step = all.div_ceil(NOISE_SAMPLE).max(1);
    let mut s: Vec<f32> = found.score.iter().copied().filter(|s| *s != f32::MIN).step_by(step).collect();
    if s.len() < 1000 {
        return None;
    }
    s.sort_by(f32::total_cmp);
    let q = |p: f64| s[((s.len() - 1) as f64 * p).round() as usize] as f64;
    // Gumbel: the quartiles are 1.5725 scales apart and the median 0.3665
    // scales above the location.
    let scale = (q(0.75) - q(0.25)) / 1.5725;
    if scale <= 0.0 {
        return None;
    }
    Some((q(0.5) - 0.3665 * scale + scale * (all as f64).ln()) as f32)
}

/// The score above which a peak is a particle, for the peaks of one or
/// more searches (`floors`: `above_noise` of each). Where the scores fall
/// in two groups, the split between them (`cutoff`). Where they are one
/// group (a tomogram packed with the particle has no other places to
/// score), the highest floor, when enough peaks pass it: they are
/// particles all the same.
pub fn sure(all: &[Peak], floors: &[Option<f32>]) -> Option<f32> {
    cutoff(all).or_else(|| {
        let floor = floors.iter().copied().collect::<Option<Vec<f32>>>()?.into_iter().reduce(f32::max)?;
        (all.iter().filter(|p| p.score >= floor).count() >= FEWEST_ABOVE).then_some(floor)
    })
}

/// The voxels where a tomogram holds nothing: the borders a reconstruction
/// leaves blank where the tilted images did not reach. There every voxel
/// has exactly the value of all its neighbours (data never has), and the
/// blank reaches an X or Y face of the volume. Same layout as `v`.
pub fn blank(v: &[f32], dims: [usize; 3]) -> Vec<bool> {
    let [nx, ny, nz] = dims;
    assert_eq!(v.len(), nx * ny * nz);
    // The same value as both neighbours along x, then that along y, then z.
    let mut flat: Vec<bool> = (0..v.len())
        .into_par_iter()
        .map(|i| {
            let x = i % nx;
            (x == 0 || v[i - 1] == v[i]) && (x + 1 == nx || v[i + 1] == v[i])
        })
        .collect();
    for (len, stride) in [(ny, nx), (nz, nx * ny)] {
        let before = flat.clone();
        flat.par_iter_mut().enumerate().for_each(|(i, f)| {
            let k = i / stride % len;
            *f = *f && (k == 0 || (before[i - stride] && v[i - stride] == v[i])) && (k + 1 == len || (before[i + stride] && v[i + stride] == v[i]));
        });
    }
    // Only what is joined to an X or Y face.
    let mut out = vec![false; v.len()];
    let mut stack: Vec<usize> = (0..v.len()).filter(|i| flat[*i] && (i % nx == 0 || i % nx == nx - 1 || i / nx % ny == 0 || i / nx % ny == ny - 1)).collect();
    stack.iter().for_each(|i| out[*i] = true);
    while let Some(i) = stack.pop() {
        let p = [i % nx, i / nx % ny, i / (nx * ny)];
        for (axis, step, len) in [(0usize, 1usize, nx), (1, nx, ny), (2, nx * ny, nz)] {
            for up in [false, true] {
                if if up { p[axis] + 1 >= len } else { p[axis] == 0 } {
                    continue;
                }
                let j = if up { i + step } else { i - step };
                if flat[j] && !out[j] {
                    out[j] = true;
                    stack.push(j);
                }
            }
        }
    }
    out
}

/// Where a particle centre may lie: false within `radius` voxels of the X
/// and Y faces (reconstruction artefacts; not Z, particles lie near those
/// faces) and within `2 * radius` of gold (a voxel more than `GOLD` robust
/// standard deviations below the median) or of a blank border (`blank`).
/// Same layout as `v`.
pub fn allowed(v: &[f32], dims: [usize; 3], radius: f64) -> Vec<bool> {
    let [nx, ny, nz] = dims;
    assert_eq!(v.len(), nx * ny * nz);
    let blank = blank(v, dims);
    // What the tomogram is like where it holds something.
    let held: Vec<f32> = v.iter().zip(&blank).filter(|(_, b)| !**b).map(|(x, _)| *x).collect();
    let (median, sd) = robust(if held.len() >= 1000 { &held } else { v });
    let limit = median - GOLD * sd;
    // Squared distance to the nearest gold or blank voxel, one axis at a
    // time (exact; Felzenszwalb and Huttenlocher, Theory of Computing 8, 2012).
    const FAR: f32 = 1e9;
    let mut near: Vec<f32> = v.iter().zip(&blank).map(|(x, b)| if *x < limit || *b { 0.0 } else { FAR }).collect();
    if near.iter().any(|d| *d == 0.0) {
        for (len, stride, lines) in [(nx, 1, ny * nz), (ny, nx, nx * nz), (nz, nx * ny, nx * ny)] {
            let mut line = vec![0f64; len];
            let mut out = vec![0f64; len];
            for k in 0..lines {
                // First voxel of line k: the other two axes counted off.
                let first = if stride == 1 {
                    k * nx
                } else if stride == nx {
                    (k / nx) * nx * ny + k % nx
                } else {
                    k
                };
                for i in 0..len {
                    line[i] = near[first + i * stride] as f64;
                }
                squared_distance(&line, &mut out);
                for i in 0..len {
                    near[first + i * stride] = out[i].min(FAR as f64) as f32;
                }
            }
        }
    }
    let keep = 4.0 * radius * radius;
    let mut ok = vec![true; v.len()];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let i = (z * ny + y) * nx + x;
                let edge = (x as f64) < radius || ((nx - 1 - x) as f64) < radius || (y as f64) < radius || ((ny - 1 - y) as f64) < radius;
                ok[i] = !edge && near[i] as f64 > keep;
            }
        }
    }
    ok
}

/// out[q] = min over p of (q - p)² + f[p], in one dimension.
fn squared_distance(f: &[f64], out: &mut [f64]) {
    let n = f.len();
    let mut at = vec![0usize; n];
    let mut cut = vec![0f64; n + 1];
    let mut k = 0;
    cut[0] = f64::NEG_INFINITY;
    cut[1] = f64::INFINITY;
    for q in 1..n {
        loop {
            let p = at[k];
            let s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q - p) as f64);
            if s <= cut[k] {
                k -= 1;
            } else {
                k += 1;
                at[k] = q;
                cut[k] = s;
                cut[k + 1] = f64::INFINITY;
                break;
            }
        }
    }
    let mut k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while cut[k + 1] < q as f64 {
            k += 1;
        }
        let d = q as f64 - at[k] as f64;
        *o = d * d + f[at[k]];
    }
}
