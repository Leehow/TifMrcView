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
//! gives up when they are not. A tomogram packed with the particle has one
//! group only; there the peaks are held against those of the same search
//! made in the tomogram with its phases scrambled (`scrambled`), which
//! finds what chance alone scores in noise of that kind (`beyond_chance`). (A Gumbel
//! curve fitted to the scores, to their peaks or to their quartiles, was
//! tried twice and failed on real data: its threshold lay above the best
//! score a correlation can have.) The search itself is kept off the tomogram's X/Y borders and off gold
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
    let flat = flat_variance(v, mean);

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
                            let var = (m2[j] as f64 / count - (m1[j] as f64 / count).powi(2)).max(flat);
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

/// At least this many peaks have to stand out of chance for
/// `beyond_chance` to take them without a second group to part them from.
const FEWEST_ABOVE: usize = 20;
/// Of the peaks taken as beyond chance, at most this share may be chance.
const CHANCE: f64 = 0.01;

/// Where the tomogram varies less than this share of what it does as a
/// whole, there is nothing to compare a template with: the local spread a
/// score is divided by is not taken below it. Without it a flat place (a
/// blank border, a masked-out region, the inside of something a denoiser
/// smoothed away) divides rounding noise by nearly nothing and outscores
/// every particle a thousandfold.
const FLAT: f64 = 0.05;

/// The least variance under the mask a score is divided by (`FLAT`), for
/// a tomogram `v` of mean `mean`.
pub(super) fn flat_variance(v: &[f32], mean: f32) -> f64 {
    let var = v.par_iter().map(|x| (*x as f64 - mean as f64).powi(2)).sum::<f64>() / v.len().max(1) as f64;
    (FLAT * FLAT * var).max(1e-12)
}

/// Side of the cubes the spectrum of a tomogram is measured on.
const SPECTRUM: usize = 32;
/// Half the side of the kernel that flattens a tomogram.
const FLATTEN: usize = 6;
/// The flattened tomogram keeps frequencies up to this share of the
/// sampling limit in full and fades out towards the limit: the last part
/// holds little but what binning and interpolation left there.
const FLAT_TO: f64 = 0.6;

/// The kernel (side `2 · FLATTEN + 1`, the same in every direction) that
/// gives `v` the same power at every spatial frequency: the inverse root
/// of its spectrum, measured over cubes taken through it. None for a
/// tomogram that holds nothing.
pub fn flattener(v: &[f32], dims: [usize; 3]) -> Option<Vec<f32>> {
    use crate::spa::fourier::{freq, ft3, ift3};
    let [nx, ny, nz] = dims;
    assert_eq!(v.len(), nx * ny * nz);
    let n = SPECTRUM;
    // Mirrored back into the volume, so a thin tomogram gives cubes too.
    let within = |i: usize, len: usize| -> usize {
        let k = i % (2 * len);
        if k < len { k } else { 2 * len - 1 - k }
    };
    let window: Vec<f32> = (0..n).map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * (i as f64 + 0.5) / n as f64).cos()) as f32).collect();
    let shells = n / 2 + 1;
    let mut rng = crate::spa::Rng(0xf1a7);
    let starts: Vec<[usize; 3]> = (0..200).map(|_| [rng.below(nx), rng.below(ny), rng.below(nz)]).collect();
    let (power, count) = starts
        .par_iter()
        .map(|o| {
            let mut cube = vec![0f32; n * n * n];
            for z in 0..n {
                let sz = within(o[2] + z, nz);
                for y in 0..n {
                    let sy = within(o[1] + y, ny);
                    for x in 0..n {
                        cube[(z * n + y) * n + x] = v[(sz * ny + sy) * nx + within(o[0] + x, nx)];
                    }
                }
            }
            let mean = cube.iter().map(|x| *x as f64).sum::<f64>() / cube.len() as f64;
            for z in 0..n {
                for y in 0..n {
                    for x in 0..n {
                        let c = &mut cube[(z * n + y) * n + x];
                        *c = (*c - mean as f32) * window[x] * window[y] * window[z];
                    }
                }
            }
            let (re, im) = ft3(n, &cube);
            let (mut power, mut count) = (vec![0f64; shells], vec![0f64; shells]);
            for i in 0..re.len() {
                let k = [freq(i % n, n), freq(i / n % n, n), freq(i / n / n, n)];
                let r = ((k[0] * k[0] + k[1] * k[1] + k[2] * k[2]) as f64).sqrt().round() as usize;
                if r < shells {
                    power[r] += (re[i] as f64).powi(2) + (im[i] as f64).powi(2);
                    count[r] += 1.0;
                }
            }
            (power, count)
        })
        .reduce(
            || (vec![0f64; shells], vec![0f64; shells]),
            |mut a, b| {
                for s in 0..shells {
                    a.0[s] += b.0[s];
                    a.1[s] += b.1[s];
                }
                a
            },
        );
    let power: Vec<f64> = power.iter().zip(&count).map(|(p, c)| p / c.max(1.0)).collect();
    let most = power.iter().skip(1).copied().fold(0.0, f64::max);
    if most <= 0.0 {
        return None;
    }
    // The gain at each shell: the inverse root of the power there (not
    // below a millionth of the most: a shell binning emptied stays empty),
    // nothing at zero frequency, fading out towards the sampling limit.
    let limit = (n / 2) as f64;
    let gain: Vec<f64> = (0..shells)
        .map(|s| {
            if s == 0 {
                return 0.0;
            }
            let f = s as f64 / limit;
            let fade = if f <= FLAT_TO { 1.0 } else { (0.5 * std::f64::consts::PI * (f - FLAT_TO) / (1.0 - FLAT_TO)).cos().powi(2) };
            fade / power[s].max(1e-6 * most).sqrt()
        })
        .collect();
    let mut re = vec![0f32; n * n * n];
    for i in 0..re.len() {
        let k = [freq(i % n, n), freq(i / n % n, n), freq(i / n / n, n)];
        let r = ((k[0] * k[0] + k[1] * k[1] + k[2] * k[2]) as f64).sqrt();
        let (lo, part) = (r.floor() as usize, r.fract());
        if lo + 1 < shells {
            re[i] = (gain[lo] * (1.0 - part) + gain[lo + 1] * part) as f32;
        } else if lo < shells {
            re[i] = (gain[lo] * (1.0 - part)) as f32;
        }
    }
    let full = ift3(n, &re, &vec![0f32; n * n * n]);
    // Its middle, faded to nothing at the edge, summing to zero.
    let c = FLATTEN as isize;
    let k = 2 * FLATTEN + 1;
    let mut kernel = vec![0f64; k * k * k];
    let mut fade = vec![0f64; k * k * k];
    for z in -c..=c {
        for y in -c..=c {
            for x in -c..=c {
                let r = ((x * x + y * y + z * z) as f64).sqrt() / (c + 1) as f64;
                let w = if r < 1.0 { (0.5 * std::f64::consts::PI * r).cos().powi(2) } else { 0.0 };
                let at = (((z + c) * k as isize + y + c) * k as isize + x + c) as usize;
                let from = (((z + (n / 2) as isize) * n as isize + y + (n / 2) as isize) * n as isize + x + (n / 2) as isize) as usize;
                kernel[at] = full[from] as f64 * w;
                fade[at] = w;
            }
        }
    }
    let off = kernel.iter().sum::<f64>() / fade.iter().sum::<f64>();
    let kernel: Vec<f64> = kernel.iter().zip(&fade).map(|(k, w)| k - off * w).collect();
    let norm = kernel.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-30);
    Some(kernel.iter().map(|x| (x / norm) as f32).collect())
}

/// `v` with the same power at every spatial frequency (`flattener`). A
/// tomogram's power lies mostly in its slow variations, and in a cell in
/// its membranes: two pieces of it then correlate whatever they hold, and
/// a picture that holds a piece of membrane fits every membrane. With the
/// spectrum flat, each frequency counts the same, so what is particular
/// to a picture counts as much as what it shares with everything
/// (whitening before matched filtering; as used in template matching,
/// e.g. Rickgauer, Grigorieff and Denk, eLife 6, 2017).
pub fn flattened(v: &[f32], dims: [usize; 3]) -> Vec<f32> {
    let Some(kernel) = flattener(v, dims) else { return v.to_vec() };
    let [nx, ny, nz] = dims;
    let c = FLATTEN;
    let k = 2 * c + 1;
    let within = |i: isize, len: usize| -> usize {
        let period = 2 * len as isize;
        let j = i.rem_euclid(period);
        (if j < len as isize { j } else { period - 1 - j }) as usize
    };
    let mut out = vec![0f32; v.len()];
    out.par_chunks_mut(nx).enumerate().for_each(|(row, o)| {
        let (y, z) = (row % ny, row / ny);
        let mut padded = vec![0f32; nx + 2 * c];
        for dz in 0..k {
            let sz = within(z as isize + dz as isize - c as isize, nz);
            for dy in 0..k {
                let sy = within(y as isize + dy as isize - c as isize, ny);
                let src = &v[(sz * ny + sy) * nx..(sz * ny + sy + 1) * nx];
                for (i, p) in padded.iter_mut().enumerate() {
                    *p = src[within(i as isize - c as isize, nx)];
                }
                let taps = &kernel[(dz * k + dy) * k..(dz * k + dy + 1) * k];
                for (dx, t) in taps.iter().enumerate() {
                    for (a, b) in o.iter_mut().zip(&padded[dx..dx + nx]) {
                        *a += t * b;
                    }
                }
            }
        }
    });
    out
}

/// The tomogram with its phases scrambled, block by block (`block`³, a
/// power of two; blocks at the far faces are filled out by mirroring):
/// everywhere the power it has there at every frequency, in the shape of
/// nothing. What a search of it scores is what the template, with all its
/// orientations as alike as they are, scores by chance in noise of this
/// tomogram's kind (the phase-randomised control: Chen et al.,
/// Ultramicroscopy 135, 2013). Scrambling the template instead was tried:
/// a scrambled template is a different thing at every orientation, so it
/// has thousands of chances where a rounded particle has a few, and its
/// search costs a hundred times the real one.
pub fn scrambled(v: &[f32], dims: [usize; 3], block: usize) -> Vec<f32> {
    use crate::spa::fourier::{ft3, ift3};
    let [nx, ny, nz] = dims;
    assert_eq!(v.len(), nx * ny * nz);
    let b = block;
    let blocks: Vec<[usize; 3]> = (0..nz.div_ceil(b)).flat_map(|z| (0..ny.div_ceil(b)).flat_map(move |y| (0..nx.div_ceil(b)).map(move |x| [x * b, y * b, z * b]))).collect();
    // Index i of a block starting at o, mirrored back into 0..n.
    let within = |o: usize, i: usize, n: usize| -> usize {
        let period = 2 * n;
        let k = (o + i) % period;
        if k < n { k } else { period - 1 - k }
    };
    let made: Vec<Vec<f32>> = blocks
        .par_iter()
        .enumerate()
        .map(|(k, o)| {
            let mut cube = vec![0f32; b * b * b];
            for z in 0..b {
                let sz = within(o[2], z, nz);
                for y in 0..b {
                    let sy = within(o[1], y, ny);
                    for x in 0..b {
                        cube[(z * b + y) * b + x] = v[(sz * ny + sy) * nx + within(o[0], x, nx)];
                    }
                }
            }
            let mean = cube.iter().map(|x| *x as f64).sum::<f64>() / cube.len() as f64;
            cube.iter_mut().for_each(|x| *x -= mean as f32);
            let mut rng = crate::spa::Rng(0x5c7a + k as u64);
            let noise: Vec<f32> = (0..cube.len()).map(|_| rng.gauss() as f32).collect();
            let (mut re, mut im) = ft3(b, &cube);
            // The phases of real noise: those of a real map again.
            let (nr, ni) = ft3(b, &noise);
            for i in 0..re.len() {
                let amplitude = (re[i] * re[i] + im[i] * im[i]).sqrt();
                let length = (nr[i] * nr[i] + ni[i] * ni[i]).sqrt().max(1e-20);
                re[i] = amplitude * nr[i] / length;
                im[i] = amplitude * ni[i] / length;
            }
            let mut out = ift3(b, &re, &im);
            out.iter_mut().for_each(|x| *x += mean as f32);
            out
        })
        .collect();
    let mut out = vec![0f32; v.len()];
    for (o, cube) in blocks.iter().zip(&made) {
        for z in 0..b.min(nz - o[2]) {
            for y in 0..b.min(ny - o[1]) {
                let row = ((o[2] + z) * ny + o[1] + y) * nx + o[0];
                let len = b.min(nx - o[0]);
                out[row..row + len].copy_from_slice(&cube[(z * b + y) * b..(z * b + y) * b + len]);
            }
        }
    }
    out
}

/// The score above which the peaks of a search are particles, when they
/// are one group with nothing to part them from: held against `chance`,
/// the peaks of the same search made in the scrambled tomogram, each with
/// the number of such peaks it stands for (the control searches a part of
/// the tomogram only). The lowest score at which no more than `CHANCE` of
/// the peaks above it are expected to be chance (the step-up rule of
/// Benjamini and Hochberg, J. R. Stat. Soc. B 57, 1995), when at least
/// `FEWEST_ABOVE` peaks pass it. None when the peaks score as chance does.
pub fn beyond_chance(all: &[Peak], chance: &[(f32, f64)]) -> Option<f32> {
    if chance.is_empty() {
        return None;
    }
    let mut real: Vec<f32> = all.iter().map(|p| p.score).collect();
    real.sort_by(|a, b| b.total_cmp(a));
    let mut by_chance = chance.to_vec();
    by_chance.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (mut next, mut expected) = (0, 0f64);
    let mut last = None;
    for (k, score) in real.iter().enumerate() {
        while next < by_chance.len() && by_chance[next].0 >= *score {
            expected += by_chance[next].1;
            next += 1;
        }
        if expected <= CHANCE * (k + 1) as f64 {
            last = Some(k);
        }
    }
    last.filter(|k| k + 1 >= FEWEST_ABOVE).map(|k| real[k])
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
    allowed_but(v, dims, radius, &[])
}

/// The voxels of gold in `v`, as (x, y, z): more than `GOLD` robust standard
/// deviations below the median of what the tomogram holds.
pub fn gold(v: &[f32], dims: [usize; 3]) -> Vec<[usize; 3]> {
    let [nx, ny, nz] = dims;
    assert_eq!(v.len(), nx * ny * nz);
    let limit = gold_limit(v, &blank(v, dims));
    (0..v.len()).filter(|i| v[*i] < limit).map(|i| [i % nx, i / nx % ny, i / (nx * ny)]).collect()
}

/// The value below which a voxel of `v` is gold (`blank`: the voxels that hold nothing).
fn gold_limit(v: &[f32], blank: &[bool]) -> f32 {
    // What the tomogram is like where it holds something.
    let held: Vec<f32> = v.iter().zip(blank).filter(|(_, b)| !**b).map(|(x, _)| *x).collect();
    let (median, sd) = robust(if held.len() >= 1000 { &held } else { v });
    median - GOLD * sd
}

/// `allowed`, with `gold` (voxels of `v`, (x, y, z)) counted as gold too:
/// gold found where it stands out, for a grid so coarse that a bead is
/// averaged away with what is around it (a bead of 100 Å in voxels of 100 Å
/// is no longer `GOLD` deviations dark, and was found as a candidate).
pub fn allowed_but(v: &[f32], dims: [usize; 3], radius: f64, gold: &[[usize; 3]]) -> Vec<bool> {
    let [nx, ny, nz] = dims;
    assert_eq!(v.len(), nx * ny * nz);
    let blank = blank(v, dims);
    let limit = gold_limit(v, &blank);
    // Squared distance to the nearest gold or blank voxel, one axis at a
    // time (exact; Felzenszwalb and Huttenlocher, Theory of Computing 8, 2012).
    const FAR: f32 = 1e9;
    let mut near: Vec<f32> = v.iter().zip(&blank).map(|(x, b)| if *x < limit || *b { 0.0 } else { FAR }).collect();
    for g in gold.iter().filter(|g| (0..3).all(|k| g[k] < dims[k])) {
        near[(g[2] * ny + g[1]) * nx + g[0]] = 0.0;
    }
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

#[cfg(test)]
mod flat_tests {
    use super::*;
    use crate::spa::fourier::{freq, ft3};

    /// Power per shell of a 32³ cube from the corner of `v`.
    fn shells(v: &[f32], dims: [usize; 3]) -> Vec<f64> {
        let n = 32;
        let mut cube = vec![0f32; n * n * n];
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    cube[(z * n + y) * n + x] = v[((z + 8) * dims[1] + y + 8) * dims[0] + x + 8];
                }
            }
        }
        let (re, im) = ft3(n, &cube);
        let (mut power, mut count) = (vec![0f64; 17], vec![0f64; 17]);
        for i in 0..re.len() {
            let k = [freq(i % n, n), freq(i / n % n, n), freq(i / n / n, n)];
            let r = ((k[0] * k[0] + k[1] * k[1] + k[2] * k[2]) as f64).sqrt().round() as usize;
            if r < 17 {
                power[r] += (re[i] as f64).powi(2) + (im[i] as f64).powi(2);
                count[r] += 1.0;
            }
        }
        power.iter().zip(&count).map(|(p, c)| p / c).collect()
    }

    /// Noise with most of its power in slow variations has, flattened,
    /// about the same power at every frequency a picture can tell apart,
    /// up to `FLAT_TO` of the limit, and its level is gone.
    #[test]
    fn a_flattened_tomogram_has_a_flat_spectrum() {
        let dims = [72usize, 64, 56];
        let mut rng = crate::spa::Rng(21);
        let mut v: Vec<f32> = (0..dims[0] * dims[1] * dims[2]).map(|_| rng.gauss() as f32).collect();
        // Smoothed along each axis a few times: a red spectrum.
        for step in [1, dims[0], dims[0] * dims[1]] {
            for _ in 0..3 {
                let a = v.clone();
                for i in step..v.len() - step {
                    v[i] = 0.25 * a[i - step] + 0.5 * a[i] + 0.25 * a[i + step];
                }
            }
        }
        v.iter_mut().for_each(|x| *x += 7.0);
        let before = shells(&v, dims);
        assert!(before[2] > 30.0 * before[9], "{before:.3?}");
        let flat = flattened(&v, dims);
        let after = shells(&flat, dims);
        // Of the slowest variations (longer than a picture is wide) less is
        // left: the short kernel and the window of the measurement both
        // take from them, which does no harm.
        assert!(after[1] < after[6] && after[2] < after[6], "{after:.3?}");
        let part: Vec<f64> = after[4..=9].to_vec();
        let (lo, hi) = (part.iter().copied().fold(f64::MAX, f64::min), part.iter().copied().fold(0.0, f64::max));
        eprintln!("before {before:.3?}\nafter {after:.3?}");
        assert!(hi < 2.5 * lo, "{part:.3?}");
        assert!(after[16] < 0.2 * lo, "{after:.3?}");
        let mean = flat.iter().map(|x| *x as f64).sum::<f64>() / flat.len() as f64;
        let sd = (flat.iter().map(|x| (*x as f64 - mean).powi(2)).sum::<f64>() / flat.len() as f64).sqrt();
        assert!(mean.abs() < 0.05 * sd, "{mean} {sd}");
    }
}
