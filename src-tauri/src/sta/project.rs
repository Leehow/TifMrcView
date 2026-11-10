//! From tomograms to a coarse average with no parameters to set.
//!
//! 1. Each tomogram is binned to about `SEARCH_VOXEL` Å and searched for
//!    dark blobs of a ladder of sizes (`pick`). The sizes found are the
//!    "kinds" offered to the user; gold beads, ice and dense debris make
//!    kinds of their own that are simply not chosen.
//! 2. For the chosen size the tomograms are binned again so that size
//!    spans about `SPAN` voxels of a `BOX`³ cube, the cubes are cut at the
//!    blob centres and brought to zero mean and unit variance.
//! 3. The strongest `FIRST` blobs make a first model (`refine::first_model`),
//!    every particle is then placed on it by a global search, and the two
//!    halves are aligned locally (`refine::align`).
//!
//! Results go to `<out>/`: `average.mrc`, `half1.mrc`, `half2.mrc`,
//! `particles.csv` (tomogram, position in tomogram voxels, RELION-style
//! rot/tilt/psi, score, half, and what the blob looked like) and `sta.json`.

use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::pick::{blobs, Blob};
use super::refine::{align, classes, first_model, Live, Progress, Set};
use super::{cc, check_cancel, Pose};
use crate::spa::geom::{angles, axis_angle, grid, mul};
use crate::spa::fourier::resample3;
use crate::spa::Rng;
use crate::tomo::{output, pipeline};

/// Voxel size the tomograms are searched at, Å.
const SEARCH_VOXEL: f64 = 16.0;
/// Blob diameters looked for, Å.
const LADDER: [f64; 7] = [90.0, 120.0, 160.0, 210.0, 280.0, 370.0, 490.0];
/// Robust z-score a blob needs, as a kind and as a candidate of the chosen
/// kind. (Weaker candidates were tried on EMPIAR-10045: at z ≥ 3.5 they
/// more than doubled the candidates, the match with the average did not
/// tell them apart, and the average got worse.)
const MIN_Z: f32 = 5.0;
const LOOSE_Z: f32 = MIN_Z;
/// A candidate whose darkest voxel is this many robust standard deviations
/// below the tomogram's median is a gold marker (see `Cut::darkest`).
pub(crate) const GOLD: f32 = 10.0;
/// Side of the cubes, and how many voxels of it the particle spans.
const BOX: usize = 32;
const SPAN: f64 = 13.0;
/// Particles in the first model, and most particles averaged here.
const FIRST: usize = 200;
const MOST: usize = 3000;

/// The clear companion of a source's tomogram (`tomo::auto`, `--for sta`):
/// raw voxel index = `scale` × look voxel index + `offset`, per axis (x, y, z).
pub use crate::tomo::dataset::Look as LookSource;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// A tomogram (X, Y = tilt axis, Z = beam): the one particles are cut
    /// from and averaged.
    pub path: String,
    /// First and last tilt used for it, degrees.
    pub tilts: [f64; 2],
    /// A clear (denoised) version of it, for the eyes and for the template
    /// search (`run_template` finds positions on it and maps them back to
    /// `path`). Kinds, cards, cutting and averaging never use it: anything
    /// that becomes a reference or an average comes from `path`.
    #[serde(default)]
    pub look: Option<LookSource>,
    /// The tilt series' dataset (`<stack>.tomo.json`) the tomogram was made
    /// from: with it the final average is made from the tilt images
    /// themselves (`tilt`).
    #[serde(default)]
    pub series: Option<String>,
}

/// Where a position on the binned look tomogram lies on the binned raw one.
/// `pos` is a (fractional) index of a look voxel binned by `look_bin`, a
/// voxel's centre being its integer index. Binned voxel p averages the
/// `bin` unbinned voxels from p·bin, so its centre is the unbinned index
/// p·bin + (bin − 1)/2 (the half-voxel terms); the look map takes unbinned
/// look indices to unbinned raw ones.
pub fn to_raw(pos: [f64; 3], look_bin: usize, raw_bin: usize, look: &LookSource) -> [f64; 3] {
    let (lb, rb) = (look_bin as f64, raw_bin as f64);
    std::array::from_fn(|k| {
        let look_voxel = pos[k] * lb + (lb - 1.0) / 2.0;
        let raw_voxel = look.scale * look_voxel + look.offset[k];
        (raw_voxel - (rb - 1.0) / 2.0) / rb
    })
}

/// The exclusion mask of a look tomogram, from the raw one. Exclusion (X/Y
/// borders, gold) is a property of the specimen, and its rule (voxels more than
/// `GOLD` robust deviations dark) is calibrated on raw reconstructions: a
/// denoised tomogram has a much smaller spread and the rule misfires there.
/// `raw_ok` is `allowed` of the raw tomogram on a grid `raw_dims` whose voxel
/// p is the binned raw voxel(s) `raw_h` p .. `raw_h` p + `raw_h` − 1 (1 for the
/// binned tomogram, 2 for its halved copy); the result is on the look grid
/// `look_dims`, built the same way with `look_h`. A look voxel is allowed when
/// the raw voxel nearest its centre is (outside the raw grid: not), and it
/// lies at least `look_radius` voxels from the look grid's X and Y faces.
#[allow(clippy::too_many_arguments)]
pub(crate) fn allowed_on_look(
    raw_ok: &[bool],
    raw_dims: [usize; 3],
    raw_h: usize,
    raw_bin: usize,
    look_dims: [usize; 3],
    look_h: usize,
    look_bin: usize,
    look: &LookSource,
    look_radius: f64,
) -> Vec<bool> {
    let [nx, ny, nz] = look_dims;
    let mut out = vec![false; nx * ny * nz];
    // Voxel p of a grid made of `h`-fold voxels is centred on binned voxel h p + (h − 1)/2.
    let (lh, rh) = ((look_h as f64 - 1.0) / 2.0, (raw_h as f64 - 1.0) / 2.0);
    out.par_chunks_mut(nx * ny).enumerate().for_each(|(z, plane)| {
        for y in 0..ny {
            for x in 0..nx {
                let edge = (x as f64) < look_radius || ((nx - 1 - x) as f64) < look_radius || (y as f64) < look_radius || ((ny - 1 - y) as f64) < look_radius;
                if edge {
                    continue;
                }
                let at = [x, y, z].map(|p| p as f64 * look_h as f64 + lh);
                let q = to_raw(at, look_bin, raw_bin, look);
                let q = q.map(|b| ((b - rh) / raw_h as f64).round());
                if (0..3).all(|k| q[k] >= 0.0 && q[k] < raw_dims[k] as f64) {
                    plane[y * nx + x] = raw_ok[((q[2] as usize) * raw_dims[1] + q[1] as usize) * raw_dims[0] + q[0] as usize];
                }
            }
        }
    });
    out
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Kind {
    /// The size on the ladder the blobs answered to (what `run` is asked for).
    pub diameter_a: f64,
    /// The size measured on the blobs' mean radial density, Å: where it has
    /// fallen half way to the surroundings. The ladder size tends to be
    /// small for particles that are not even balls.
    pub measured_a: f64,
    pub count: usize,
    /// Median robust z-score of its blobs.
    pub contrast: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// The sizes found (the blob path only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<Kind>,
    /// None when the user has to choose a kind first.
    pub diameter_a: Option<f64>,
    pub particles: usize,
    /// Blobs of the chosen size tried, and how distinct the ones left out
    /// were from the ones kept (see `split`).
    pub candidates: usize,
    /// None where there was no such split (the template search).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distinct: Option<f64>,
    pub voxel_a: f64,
    /// The template search only: which tomogram was searched ("look" or
    /// "raw"; "mixed" when the sources differ) and the voxel the search ran
    /// at, Å (the first source's).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searched: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_voxel_a: Option<f64>,
    /// Resolution at FSC 0.143 between the halves, Å.
    pub resolution_a: Option<f64>,
    /// With a dip in the FSC (`resolution`, a zero of the contrast transfer):
    /// the resolution where the halves last agree, Å, finer than
    /// `resolution_a`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_a: Option<f64>,
    /// `resolution_a` is limited at a zero of the contrast transfer.
    pub ctf_limited: bool,
    pub fsc: Vec<f64>,
    /// What the average was made from, when there was a choice: "tilt
    /// images" or "tomogram"; and the FSC of the one not kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub averaged_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fsc_other: Option<Vec<f64>>,
    pub map: Option<String>,
    pub halves: Option<[String; 2]>,
    pub table: Option<String>,
    pub ms: u128,
}

/// A tomogram averaged over `bin`³ voxels: values, size, voxel size (Å).
pub fn binned(path: &str, voxel_a: f64) -> Result<(Vec<f32>, [usize; 3], f64, usize)> {
    let img = pipeline::open(path)?;
    let info = img.info();
    let px = output::pixel_a(&img);
    let bin = ((voxel_a / px).round() as usize).max(1);
    let (w, h, nz) = (info.width / bin, info.height / bin, info.size_z / bin);
    if w < BOX || h < BOX || nz < 4 {
        bail!("断层图太小：{}", path);
    }
    let mut out = vec![0f32; w * h * nz];
    let scale = 1.0 / (bin * bin * bin) as f32;
    for z in 0..nz * bin {
        check_cancel()?;
        let (page, s) = info.locate(0, z, 0);
        let p = img.page(page)?;
        let p = &p[s.min(p.len() - 1)];
        let plane = &mut out[(z / bin) * w * h..(z / bin + 1) * w * h];
        plane.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for j in 0..bin {
                let src = (y * bin + j) * info.width;
                for (x, o) in row.iter_mut().enumerate() {
                    for i in 0..bin {
                        *o += p.data.get_f64(src + x * bin + i) as f32 * scale;
                    }
                }
            }
        });
    }
    Ok((out, [w, h, nz], px * bin as f64, bin))
}

/// The sizes found, most common first; sizes with very few blobs are left out.
pub fn kinds(found: &[Blob], voxel_a: f64, profiles: &[Profile]) -> Vec<Kind> {
    let mut out: Vec<Kind> = LADDER
        .iter()
        .zip(profiles)
        .filter_map(|(&d, profile)| {
            let mut z: Vec<f32> = found.iter().filter(|b| ((b.diameter as f64 * voxel_a) - d).abs() < 1.0).map(|b| b.score).collect();
            if z.len() < 8 {
                return None;
            }
            z.sort_by(f32::total_cmp);
            Some(Kind { diameter_a: d, measured_a: profile.diameter().unwrap_or(d), count: z.len(), contrast: z[z.len() / 2] as f64 })
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count));
    out
}

/// Mean density of blobs of one ladder size by distance from their centres.
#[derive(Clone, Debug)]
pub struct Profile {
    /// Width of a bin, Å.
    step: f64,
    sum: Vec<f64>,
    count: Vec<f64>,
}

/// How far out the profile goes, in ladder diameters.
const REACH: f64 = 1.5;
/// Blobs of one size and tomogram that go into the profile.
const SAMPLE: usize = 150;

impl Profile {
    fn new(diameter_a: f64) -> Self {
        let bins = 48;
        Profile { step: REACH * diameter_a / bins as f64, sum: vec![0.0; bins], count: vec![0.0; bins] }
    }

    /// Adds the surroundings of one centre (voxels of `voxel` Å).
    fn add(&mut self, v: &[f32], dims: [usize; 3], voxel: f64, c: [f32; 3]) {
        let reach = self.step * self.sum.len() as f64 / voxel;
        let r = reach.ceil() as isize;
        let c = c.map(|x| x.round() as isize);
        for z in (c[2] - r).max(0)..=(c[2] + r).min(dims[2] as isize - 1) {
            for y in (c[1] - r).max(0)..=(c[1] + r).min(dims[1] as isize - 1) {
                for x in (c[0] - r).max(0)..=(c[0] + r).min(dims[0] as isize - 1) {
                    let d = (((x - c[0]).pow(2) + (y - c[1]).pow(2) + (z - c[2]).pow(2)) as f64).sqrt() * voxel;
                    let k = (d / self.step) as usize;
                    if k < self.sum.len() {
                        self.sum[k] += v[(z as usize * dims[1] + y as usize) * dims[0] + x as usize] as f64;
                        self.count[k] += 1.0;
                    }
                }
            }
        }
    }

    /// Twice the distance at which the density has come half way from its
    /// extreme near the centre to the level far out.
    pub fn diameter(&self) -> Option<f64> {
        // Bins that hold voxels: (distance in Å, mean density).
        let p: Vec<(f64, f64)> =
            (0..self.sum.len()).filter(|k| self.count[*k] > 0.0).map(|k| ((k as f64 + 0.5) * self.step, self.sum[k] / self.count[k])).collect();
        let reach = self.step * self.sum.len() as f64;
        let far: Vec<f64> = p.iter().filter(|x| x.0 > 0.8 * reach).map(|x| x.1).collect();
        if far.is_empty() {
            return None;
        }
        let far = far.iter().sum::<f64>() / far.len() as f64;
        // Particles are dark: depth below the surroundings. The extreme is
        // looked for out to half the ladder diameter.
        let depth: Vec<(f64, f64)> = p.iter().map(|x| (x.0, far - x.1)).collect();
        let at = (0..depth.len()).filter(|k| depth[*k].0 < reach / 3.0).max_by(|a, b| depth[*a].1.total_cmp(&depth[*b].1))?;
        let peak = depth[at].1;
        if peak <= 0.0 {
            return None;
        }
        let k = (at + 1..depth.len()).find(|k| depth[*k].1 < 0.5 * peak)?;
        let f = ((depth[k - 1].1 - 0.5 * peak) / (depth[k - 1].1 - depth[k].1).max(1e-12)).clamp(0.0, 1.0);
        Some(2.0 * (depth[k - 1].0 + f * (depth[k].0 - depth[k - 1].0)))
    }
}

/// Splits scores into a low and a high group where the groups are most
/// distinct (Otsu, IEEE Trans SMC 9, 1979): the threshold, and how much of
/// the spread the split explains (0 to 1; about 0.64 for one bell curve).
pub fn split(scores: &[f32]) -> (f32, f64) {
    let mut s: Vec<f32> = scores.to_vec();
    s.sort_by(f32::total_cmp);
    let n = s.len();
    if n < 4 {
        return (f32::MIN, 0.0);
    }
    let total: f64 = s.iter().map(|x| *x as f64).sum();
    let mean = total / n as f64;
    let var: f64 = s.iter().map(|x| (*x as f64 - mean).powi(2)).sum::<f64>() / n as f64;
    if var <= 0.0 {
        return (f32::MIN, 0.0);
    }
    let (mut best, mut cut, mut low) = (0.0, f32::MIN, 0.0);
    for i in 1..n {
        low += s[i - 1] as f64;
        let (a, b) = (low / i as f64, (total - low) / (n - i) as f64);
        let between = i as f64 * (n - i) as f64 / (n as f64 * n as f64) * (a - b).powi(2);
        if between > best {
            best = between;
            cut = 0.5 * (s[i - 1] + s[i]);
        }
    }
    (cut, best / var)
}

/// Where a resolution is read on an FSC curve, in (fractional) shells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shells {
    /// The resolution limit: where the curve first falls through 0.143, or,
    /// with a dip, the dip when the curve has not fallen through it by then.
    pub limit: f64,
    /// Where the halves last agree (the curve crosses 0.143 for the last
    /// time, with a neighbour above the line too: `refine::extent`). Equal to
    /// `limit` without a dip.
    pub signal: f64,
    /// The curve has a dip: a local minimum at shell 2 or later below 0.5 that
    /// is followed, at a higher shell, by a value at least 0.15 above it. That
    /// is not noise taking over: with every particle at about the same defocus
    /// the halves agree again beyond a zero of the contrast transfer, so the
    /// limit is where the zero lies and the data holds more.
    pub dip: bool,
}

/// The resolution limit and signal extent of an FSC curve (by shell, from the
/// lowest frequency), which do not flip with the noise at a CTF zero: whether
/// the curve dips to 0.17 or 0.11 there makes no difference to either.
pub(crate) fn resolution(fsc: &[f64]) -> Shells {
    use super::refine::{cut, extent};
    if fsc.is_empty() {
        return Shells { limit: 0.0, signal: 0.0, dip: false };
    }
    let falls = |s: usize| fsc[s - 1] > fsc[s] && fsc.get(s + 1).is_none_or(|n| fsc[s] <= *n);
    let dip = (2..fsc.len()).find(|&m| fsc[m] < 0.5 && falls(m) && fsc[m + 1..].iter().any(|&x| x >= fsc[m] + 0.15));
    let first = cut(fsc);
    match dip {
        None => Shells { limit: first, signal: first, dip: false },
        Some(m) => {
            // `cut` is the first crossing; it counts if it is not behind the dip.
            let crossed = (2..=m).any(|s| fsc[s] < 0.143);
            let s = extent(fsc);
            let a = fsc.get(s as usize).copied().unwrap_or(0.0);
            let signal = match fsc.get(s as usize + 1) {
                Some(&b) if a > b => s + (a - 0.143) / (a - b),
                _ => s,
            };
            Shells { limit: if crossed { first } else { m as f64 }, signal, dip: true }
        }
    }
}

/// A split has to explain this much of the scores' spread before the low
/// group is taken for other things than the particle and left out.
const DISTINCT: f64 = 0.75;

struct Cut {
    source: usize,
    /// Centre in voxels of the source tomogram (unbinned).
    at: [f64; 3],
    /// Source voxels per working voxel.
    bin: usize,
    score: f32,
    /// The response of the blob detector itself (0 for drawn places).
    strength: f32,
    /// The darkest voxel within the particle, in robust standard
    /// deviations below the tomogram's median: gold and other very dense
    /// things stand far out.
    darkest: f32,
    cube: Vec<f32>,
}

/// See `Cut::darkest`. `c` and `radius` in voxels of `v`.
fn darkest(v: &[f32], dims: [usize; 3], c: [usize; 3], radius: f64, median: f32, sd: f32) -> f32 {
    let r = radius.ceil() as isize;
    let mut low = f32::MAX;
    for z in (c[2] as isize - r).max(0)..=(c[2] as isize + r).min(dims[2] as isize - 1) {
        for y in (c[1] as isize - r).max(0)..=(c[1] as isize + r).min(dims[1] as isize - 1) {
            for x in (c[0] as isize - r).max(0)..=(c[0] as isize + r).min(dims[0] as isize - 1) {
                let d2 = (x - c[0] as isize).pow(2) + (y - c[1] as isize).pow(2) + (z - c[2] as isize).pow(2);
                if (d2 as f64) <= radius * radius {
                    low = low.min(v[(z as usize * dims[1] + y as usize) * dims[0] + x as usize]);
                }
            }
        }
    }
    (median - low) / sd
}

/// The cube around voxel `c`, zero mean and unit variance. Where it reaches
/// past the tomogram (thin tomograms are hardly thicker than the cube) it
/// along the beam it is filled with the mean, so particles near a face are
/// kept; the centre itself and at least half the cube have to be inside.
fn cube(v: &[f32], dims: [usize; 3], c: [usize; 3]) -> Option<Vec<f32>> {
    cube_n(v, dims, c, BOX)
}

/// As `cube`, of side `n`.
fn cube_n(v: &[f32], dims: [usize; 3], c: [usize; 3], n: usize) -> Option<Vec<f32>> {
    let h = (n / 2) as isize;
    // Across the beam the cube has to be whole: the borders of a tomogram
    // hold reconstruction artefacts, not particles.
    if c[2] >= dims[2] || (0..2).any(|i| c[i] < n / 2 || c[i] + n / 2 > dims[i]) {
        return None;
    }
    let c = c.map(|x| x as isize);
    let inside = |p: isize, n: usize| p >= 0 && p < n as isize;
    let mut out = vec![f32::NAN; n * n * n];
    let (mut sum, mut sq, mut count) = (0f64, 0f64, 0usize);
    for z in 0..n as isize {
        let sz = c[2] - h + z;
        if !inside(sz, dims[2]) {
            continue;
        }
        for y in 0..n as isize {
            let sy = c[1] - h + y;
            if !inside(sy, dims[1]) {
                continue;
            }
            for x in 0..n as isize {
                let sx = c[0] - h + x;
                if inside(sx, dims[0]) {
                    let val = v[(sz as usize * dims[1] + sy as usize) * dims[0] + sx as usize];
                    out[((z * n as isize + y) * n as isize + x) as usize] = val;
                    sum += val as f64;
                    sq += (val as f64).powi(2);
                    count += 1;
                }
            }
        }
    }
    if count * 2 < out.len() {
        return None;
    }
    let mean = sum / count as f64;
    let sd = (sq / count as f64 - mean * mean).max(0.0).sqrt();
    if sd <= 0.0 {
        return None;
    }
    out.iter_mut().for_each(|x| *x = if x.is_nan() { 0.0 } else { ((*x as f64 - mean) / sd) as f32 });
    Some(out)
}

/// Step 1 of `run`: every tomogram searched for blobs of the ladder sizes
/// at `SEARCH_VOXEL`, and the sizes found.
fn find_kinds(sources: &[Source], progress: Progress) -> Result<Vec<Kind>> {
    let mut all: Vec<Blob> = vec![];
    let mut profiles: Vec<Profile> = LADDER.iter().map(|d| Profile::new(*d)).collect();
    for (i, s) in sources.iter().enumerate() {
        progress("找颗粒", i, sources.len());
        let (v, dims, voxel, _) = binned(&s.path, SEARCH_VOXEL)?;
        let sizes: Vec<f64> = LADDER.iter().map(|d| d / voxel).collect();
        let mut found = blobs(&v, dims, &sizes, true, MIN_Z)?;
        // Report sizes on the ladder exactly, whatever the voxel size.
        for b in &mut found {
            let k = sizes.iter().position(|d| (*d as f32 - b.diameter).abs() < 1e-3).unwrap_or(0);
            b.diameter = (LADDER[k] / SEARCH_VOXEL) as f32;
        }
        // Strongest first: the first of each size give its profile.
        profiles.par_iter_mut().enumerate().for_each(|(k, profile)| {
            let size = (LADDER[k] / SEARCH_VOXEL) as f32;
            for b in found.iter().filter(|b| b.diameter == size).take(SAMPLE) {
                profile.add(&v, dims, voxel, b.pos);
            }
        });
        all.extend(found);
    }
    progress("找颗粒", sources.len(), sources.len());
    Ok(kinds(&all, SEARCH_VOXEL, &profiles))
}

/// Leaves out the candidates that are gold markers or lie in their streaks:
/// on EMPIAR-10045 gold is 17 to 25 deviations dark, everything else under 8.
/// Gold also stands out from the candidates themselves (they were at 4.6 in
/// the median); where all are that dark, they are the particles.
fn without_gold(cuts: &mut Vec<Cut>, size: f64, voxel_a: f64) {
    let mut dark: Vec<f32> = cuts.iter().map(|c| c.darkest).collect();
    dark.sort_by(f32::total_cmp);
    let limit = GOLD.max(2.0 * dark.get(dark.len() / 2).copied().unwrap_or(0.0));
    let gold: Vec<(usize, [f64; 3])> = cuts.iter().filter(|c| c.darkest > limit).map(|c| (c.source, c.at)).collect();
    cuts.retain(|c| {
        let reach = size / voxel_a * c.bin as f64;
        !gold.iter().any(|(s, at)| *s == c.source && (0..3).map(|i| (at[i] - c.at[i]).powi(2)).sum::<f64>() < reach * reach)
    });
}

/// `diameter_a` None finds the kinds only (the report then has no map).
pub fn run(sources: &[Source], diameter_a: Option<f64>, out_dir: &str, progress: Progress, live: Live) -> Result<Report> {
    let started = Instant::now();
    if sources.is_empty() {
        bail!("没有断层图");
    }
    // 1. Kinds.
    let kinds = find_kinds(sources, progress)?;
    let mut report = Report {
        kinds: kinds.clone(),
        diameter_a: None,
        particles: 0,
        candidates: 0,
        distinct: Some(0.0),
        voxel_a: 0.0,
        searched: None,
        search_voxel_a: None,
        resolution_a: None,
        signal_a: None,
        ctf_limited: false,
        fsc: vec![],
        averaged_from: None,
        fsc_other: None,
        map: None,
        halves: None,
        table: None,
        ms: 0,
    };
    let Some(want) = diameter_a else {
        report.ms = started.elapsed().as_millis();
        return Ok(report);
    };
    // The ladder size nearest to what was asked for.
    let size = *LADDER.iter().min_by(|a, b| (*a - want).abs().total_cmp(&(*b - want).abs())).unwrap();

    // 2. Cubes at the working voxel size.
    let mut cuts: Vec<Cut> = vec![];
    let mut voxel_a = size / SPAN;
    for (i, s) in sources.iter().enumerate() {
        progress("裁出颗粒", i, sources.len());
        let (v, dims, voxel, bin) = binned(&s.path, size / SPAN)?;
        voxel_a = voxel;
        // Blobs again at this sampling, at the one size: centres are finer.
        let found = blobs(&v, dims, &[size / voxel], true, LOOSE_Z)?;
        let (median, sd) = super::pick::robust(&v);
        for b in found {
            let c = [b.pos[0].round() as usize, b.pos[1].round() as usize, b.pos[2].round() as usize];
            if let Some(cube) = cube(&v, dims, c) {
                // The centre of working voxel c in source voxels.
                let at = c.map(|v| v as f64 * bin as f64 + (bin as f64 - 1.0) / 2.0);
                let darkest = darkest(&v, dims, c, 0.5 * size / voxel, median, sd);
                cuts.push(Cut { source: i, at, bin, score: b.score, strength: b.strength, darkest, cube });
            }
        }
    }
    progress("裁出颗粒", sources.len(), sources.len());
    let candidates = cuts.len();
    without_gold(&mut cuts, size, voxel_a);
    if cuts.len() < 20 {
        bail!("这种尺寸的颗粒只找到 {} 个，太少了", cuts.len());
    }
    cuts.sort_by(|a, b| b.score.total_cmp(&a.score));
    cuts.truncate(MOST);
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    // Sizes from the scale space are rough and tend to be small: masks
    // and search ranges allow for a particle 1.6 times as large.
    let radius = 0.8 * size / voxel_a;

    // 3. First model from the strongest, then everything.
    let first = cuts.len().min(FIRST);
    let particles: Vec<Vec<f32>> = cuts.iter().map(|c| c.cube.clone()).collect();
    let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
    let head = Set { n: BOX, particles: &particles[..first], wedges: &wedges, wedge_of: &wedge_of[..first], radius };
    let (model, mut poses) = first_model(&head, 1, progress, live)?;
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    if cuts.len() > first {
        let engine = cc::Engine::new(BOX, (BOX as f64 / (2.0 * radius) * 4.0).min(BOX as f64 / 2.0 - 3.0), 1.0, radius + 2.0);
        let rest: Vec<_> = (first..cuts.len()).into_par_iter().map(|i| engine.prepare(&particles[i], &wedges[wedge_of[i]])).collect();
        let mut rng = Rng(2);
        let turn = axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 360.0 * rng.unit());
        let rotations: Vec<_> = grid(30.0).iter().map(|r| mul(r, &turn)).collect();
        let shift = (radius / 2.0).ceil().max(2.0) as usize;
        let placed = engine.global(&rest, &model, &rotations, shift, &|a, b| progress("放置全部颗粒", a, b))?;
        poses.extend(placed);
    }
    let out = align(&set, &model, &poses, progress, live)?;

    // (Leaving out the candidates that match the average worst was tried
    // on EMPIAR-10045 and removed: at this resolution dense debris matches
    // better than the particles do.)
    report.candidates = candidates;

    save(&mut report, sources, &cuts, &out, out_dir, size, voxel_a, started)?;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn save(report: &mut Report, sources: &[Source], cuts: &[Cut], out: &super::refine::Aligned, out_dir: &str, size: f64, voxel_a: f64, started: Instant) -> Result<()> {
    // Results.
    std::fs::create_dir_all(out_dir).with_context(|| format!("无法创建 {out_dir}"))?;
    let file = |name: &str| Path::new(out_dir).join(name).to_string_lossy().into_owned();
    let n = (out.map.len() as f64).cbrt().round() as usize;
    let shape = [n, n, n];
    output::write_mrc(&file("average.mrc"), &out.map, shape, voxel_a, false)?;
    output::write_mrc(&file("half1.mrc"), &out.halves[0], shape, voxel_a, false)?;
    output::write_mrc(&file("half2.mrc"), &out.halves[1], shape, voxel_a, false)?;
    let mut csv = String::from("tomogram,x,y,z,rot,tilt,psi,score,half,blob,strength,darkest\n");
    for (i, (c, p)) in cuts.iter().zip(&out.poses).enumerate() {
        let Pose { r, t, score } = *p;
        let [rot, tilt, psi] = angles(&r);
        let k = c.bin as f64;
        csv += &format!(
            "{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.4},{},{:.2},{:.4},{:.2}\n",
            sources[c.source].path,
            c.at[0] + t[0] * k,
            c.at[1] + t[1] * k,
            c.at[2] + t[2] * k,
            rot,
            tilt,
            psi,
            score,
            i % 2 + 1,
            c.score,
            c.strength,
            c.darkest
        );
    }
    std::fs::write(file("particles.csv"), csv)?;
    report.diameter_a = Some(size);
    report.particles = cuts.len();
    report.voxel_a = voxel_a;
    let shells = resolution(&out.fsc);
    report.resolution_a = (shells.limit > 0.0).then(|| n as f64 * voxel_a / shells.limit);
    report.signal_a = (shells.dip && shells.signal > 0.0).then(|| n as f64 * voxel_a / shells.signal);
    report.ctf_limited = shells.dip;
    report.fsc = out.fsc.clone();
    report.map = Some(file("average.mrc"));
    report.halves = Some([file("half1.mrc"), file("half2.mrc")]);
    report.table = Some(file("particles.csv"));
    report.ms = started.elapsed().as_millis();
    std::fs::write(file("sta.json"), serde_json::to_string_pretty(&report)?)?;
    Ok(())
}

/// A class average of the blobs of one size, to choose a template from.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: usize,
    /// The ladder size of the blobs it was made from (what `run` is asked for).
    pub diameter_a: f64,
    /// The size measured on the blobs of that size (see `Kind`).
    pub measured_a: f64,
    pub voxel_a: f64,
    pub members: usize,
    /// Members as a share of the blobs of its size that were classified.
    pub share: f64,
    /// A `BOX`³ MRC of the class average.
    pub path: String,
    /// How much denser the average's variation is inside the central ball
    /// than a uniform one would be (see `shape`).
    pub compact: f64,
    /// How stretched the particle density is: the square root of the ratio of
    /// the largest to the smallest eigenvalue of its second-moment matrix
    /// (1 a ball, large a line or a layer; see `shape`).
    pub elongation: f64,
    /// Separate blobs in the density at half its maximum; 1 when it is a
    /// single one at the centre, 0 when it has none there (see `shape`).
    pub blobs: usize,
    /// How far the single piece's centroid lies from the centre, in particle
    /// radii (0 when there is not exactly one piece; see `shape`).
    #[serde(default)]
    pub offset: f64,
    /// A single compact, not stretched particle: worth offering as the
    /// particle (see `is_likely`).
    pub likely: bool,
}

/// What a class average (`n`³, z-major, the particle dark) says about its
/// shape, within the ball of `radius` voxels around the centre `n / 2`
/// (the particle itself fills about 1/1.2 of it):
///
/// * `compact`: the share of the squared deviation from the mean (over the
///   whole box) that lies inside the ball, divided by the ball's share of the
///   box volume. A particle concentrated in the ball scores far above 1; a
///   line or a layer running through the box scores low.
/// * `elongation`: the density is the darkness below the box's median, kept
///   where positive, out to `MOMENTS` ball radii. `sqrt(λ1 / λ3)` of the eigenvalues of its second-moment
///   matrix about its centroid: about 1 for a ball (the missing wedge
///   stretches it along Z by up to 1.3 to 1.5), `λ1 / λ2` large for a line,
///   `λ2 / λ3` large for a layer.
/// * `blobs`: the number of 6-connected pieces of the density above half its
///   maximum that hold at least `BLOB_SHARE` of that part. Where the piece
///   lies does not matter here.
/// * `offset`: with exactly one piece, the distance of its centroid from the
///   centre in particle radii (`radius / 1.2`); otherwise 0.
pub fn shape(avg: &[f32], n: usize, radius: f64) -> (f64, f64, usize, f64) {
    let (compact, elongation, blobs, offset, _) = measure(avg, n, radius);
    (compact, elongation, blobs, offset)
}

/// The class average moved so that the centroid of its single piece (when
/// there is exactly one and it is within `OFF_CENTRE` of the centre) sits at
/// the cube centre `n / 2`; unchanged otherwise. A centred template makes
/// the peaks of the later search sit on particle centres.
pub fn centred(avg: Vec<f32>, n: usize, radius: f64) -> Vec<f32> {
    let (_, _, blobs, offset, at) = measure(&avg, n, radius);
    if blobs != 1 || offset >= OFF_CENTRE || offset == 0.0 {
        return avg;
    }
    let c = (n / 2) as f64;
    // transform: out(p) = vol(p + t), so t = centroid - centre brings it to the centre.
    cc::transform(&avg, n, &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], [at[0] - c, at[1] - c, at[2] - c])
}

fn measure(avg: &[f32], n: usize, radius: f64) -> (f64, f64, usize, f64, [f64; 3]) {
    let c = (n / 2) as f64;
    let mean = avg.iter().map(|v| *v as f64).sum::<f64>() / avg.len() as f64;
    let (mut all, mut inside, mut voxels) = (0.0, 0.0, 0usize);
    let mut ball = vec![false; avg.len()];
    let mut reach = vec![false; avg.len()];
    let wide = (MOMENTS * radius).min(c);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let i = (z * n + y) * n + x;
                let d = (avg[i] as f64 - mean).abs();
                all += d * d;
                let (a, b, e) = (x as f64 - c, y as f64 - c, z as f64 - c);
                reach[i] = a * a + b * b + e * e <= wide * wide;
                if a * a + b * b + e * e <= radius * radius {
                    inside += d * d;
                    voxels += 1;
                    ball[i] = true;
                }
            }
        }
    }
    if all <= 0.0 || voxels == 0 {
        return (0.0, 0.0, 0, 0.0, [c; 3]);
    }
    let compact = (inside / all) / (voxels as f64 / avg.len() as f64);

    // The particle density: darkness below the background, in the ball.
    let mut sorted: Vec<f32> = avg.to_vec();
    sorted.sort_by(f32::total_cmp);
    let background = sorted[sorted.len() / 2] as f64;
    let dens: Vec<f64> = avg.iter().zip(&reach).map(|(v, b)| if *b { (background - *v as f64).max(0.0) } else { 0.0 }).collect();
    let (mut m, mut mx, mut my, mut mz) = (0.0, 0.0, 0.0, 0.0);
    for (i, d) in dens.iter().enumerate().filter(|(_, d)| **d > 0.0) {
        m += d;
        mx += d * (i % n) as f64;
        my += d * (i / n % n) as f64;
        mz += d * (i / n / n) as f64;
    }
    if m <= 0.0 {
        return (compact, 0.0, 0, 0.0, [c; 3]);
    }
    let centroid = [mx / m, my / m, mz / m];
    let mut cov = [[0.0f64; 3]; 3];
    for (i, d) in dens.iter().enumerate().filter(|(_, d)| **d > 0.0) {
        let p = [(i % n) as f64 - centroid[0], (i / n % n) as f64 - centroid[1], (i / n / n) as f64 - centroid[2]];
        for r in 0..3 {
            for q in 0..3 {
                cov[r][q] += d * p[r] * p[q] / m;
            }
        }
    }
    let l = eigenvalues(cov);
    let elongation = if l[0] <= 0.0 { 0.0 } else { (l[0] / l[2].max(1e-9 * l[0])).sqrt() };

    // Pieces above half the maximum.
    let top = dens.iter().cloned().fold(0.0, f64::max);
    let above: Vec<usize> = (0..dens.len()).filter(|i| ball[*i] && dens[*i] >= 0.5 * top).collect();
    let mut label = vec![usize::MAX; dens.len()];
    let mut pieces: Vec<(usize, [f64; 3])> = vec![];
    for &start in &above {
        if label[start] != usize::MAX {
            continue;
        }
        let id = pieces.len();
        let (mut count, mut sum) = (0usize, [0.0f64; 3]);
        label[start] = id;
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            let p = [i % n, i / n % n, i / n / n];
            count += 1;
            for k in 0..3 {
                sum[k] += p[k] as f64;
            }
            for (axis, step) in [(0usize, 1usize), (1, n), (2, n * n)] {
                for up in [false, true] {
                    if if up { p[axis] + 1 >= n } else { p[axis] == 0 } {
                        continue;
                    }
                    let j = if up { i + step } else { i - step };
                    if ball[j] && dens[j] >= 0.5 * top && label[j] == usize::MAX {
                        label[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
        pieces.push((count, sum.map(|s| s / count as f64)));
    }
    let big: Vec<&(usize, [f64; 3])> = pieces.iter().filter(|p| p.0 as f64 >= BLOB_SHARE * above.len() as f64).collect();
    let (mut offset, mut at) = (0.0, [c; 3]);
    if big.len() == 1 {
        at = big[0].1;
        offset = at.iter().map(|v| (v - c) * (v - c)).sum::<f64>().sqrt() / (radius / 1.2);
    }
    (compact, elongation, big.len(), offset, at)
}

/// Eigenvalues of a symmetric 3x3 matrix, largest first (Jacobi sweeps).
fn eigenvalues(mut a: [[f64; 3]; 3]) -> [f64; 3] {
    for _ in 0..30 {
        let off = a[0][1] * a[0][1] + a[0][2] * a[0][2] + a[1][2] * a[1][2];
        if off < 1e-24 * (a[0][0] * a[0][0] + a[1][1] * a[1][1] + a[2][2] * a[2][2]).max(1e-300) {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-300 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let (c, s) = (1.0 / (t * t + 1.0).sqrt(), t / (t * t + 1.0).sqrt());
            for k in 0..3 {
                let (x, y) = (a[k][p], a[k][q]);
                a[k][p] = c * x - s * y;
                a[k][q] = s * x + c * y;
            }
            for k in 0..3 {
                let (x, y) = (a[p][k], a[q][k]);
                a[p][k] = c * x - s * y;
                a[q][k] = s * x + c * y;
            }
        }
    }
    let mut l = [a[0][0], a[1][1], a[2][2]];
    l.sort_by(|x, y| y.total_cmp(x));
    l
}

/// A class is `likely` the particle when it is compact above `COMPACT`
/// (7.5 means 45% of the variation in a ball that is 6% of the box; lines,
/// layers and pairs of particles measured 3.8 to 7.2 on a real tomogram,
/// ribosomes 8.0 and 11.1), not stretched (below `ELONGATION`), one blob and
/// not far off the centre (`OFF_CENTRE`).
fn is_likely(compact: f64, elongation: f64, blobs: usize, offset: f64) -> bool {
    compact > COMPACT && elongation < ELONGATION && blobs == 1 && offset < OFF_CENTRE
}

const COMPACT: f64 = 7.5;
/// Most `elongation` of a particle: the missing wedge stretches a ball along
/// Z by up to about 1.3 to 1.5.
const ELONGATION: f64 = 1.6;
/// A piece smaller than this share of the voxels above half the maximum is
/// not a blob.
/// The moments are taken out to this many ball radii (at most the box's
/// half width): a line or a layer is cut short by the ball and looks rounder.
const MOMENTS: f64 = 2.0;
const BLOB_SHARE: f64 = 0.15;
/// A single piece further than this from the centre, in particle radii, is a
/// neighbour, not the particle.
const OFF_CENTRE: f64 = 0.5;

/// Kinds with fewer blobs, and sizes below this (Å), are not offered: at
/// this sampling they are noise.
const CARD_BLOBS: usize = 40;
const CARD_SIZE: f64 = 120.0;
/// Classes with fewer members make no card.
const CARD_MEMBERS: usize = 10;

/// Class averages of the strongest blobs of each kind found, written to
/// `<out_dir>/card_<id>.mrc` and listed in `<out_dir>/cards.json`. The user
/// chooses the template for `run_template` from them. The classes are found
/// on cubes at half sampling (eight times fewer voxels); each card is then
/// averaged from the full cubes at the
/// poses found. Everything here (blobs, gold test, cutting, classes,
/// averages) uses the raw tomogram `Source::path`; a source's `look` is
/// ignored, as a look (denoised, deconvolved) tomogram loses the internal
/// structure the classes and the averages are about. Cards come best first: the `likely` ones, then by
/// `compact × members` (a compact class that many blobs share); ids follow
/// that order.
pub fn cards(sources: &[Source], out_dir: &str, progress: Progress, live: Live) -> Result<Vec<Card>> {
    use super::search::allowed;
    if sources.is_empty() {
        bail!("没有断层图");
    }
    let kinds = find_kinds(sources, progress)?;
    let mut sizes: Vec<&Kind> = kinds.iter().filter(|k| k.count >= CARD_BLOBS && k.diameter_a >= CARD_SIZE).collect();
    sizes.sort_by(|a, b| a.diameter_a.total_cmp(&b.diameter_a));
    std::fs::create_dir_all(out_dir).with_context(|| format!("无法创建 {out_dir}"))?;
    let file = |name: &str| Path::new(out_dir).join(name).to_string_lossy().into_owned();
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let mut found: Vec<(Card, Vec<f32>)> = vec![];
    for kind in sizes {
        let size = kind.diameter_a;
        // Cubes at the working voxel size, one tomogram in memory at a time.
        let mut cuts: Vec<Cut> = vec![];
        let mut voxel_a = size / SPAN;
        for (i, s) in sources.iter().enumerate() {
            progress(&format!("裁出颗粒 {size} Å"), i, sources.len());
            let (v, dims, voxel, bin) = binned(&s.path, size / SPAN)?;
            voxel_a = voxel;
            let found = blobs(&v, dims, &[size / voxel], true, LOOSE_Z)?;
            let (median, sd) = super::pick::robust(&v);
            let ok = allowed(&v, dims, 0.5 * size / voxel);
            for b in found {
                let c = [b.pos[0].round() as usize, b.pos[1].round() as usize, b.pos[2].round() as usize];
                if c.iter().zip(dims).any(|(c, d)| *c >= d) || !ok[(c[2] * dims[1] + c[1]) * dims[0] + c[0]] {
                    continue;
                }
                if let Some(cube) = cube(&v, dims, c) {
                    let at = c.map(|v| v as f64 * bin as f64 + (bin as f64 - 1.0) / 2.0);
                    let darkest = darkest(&v, dims, c, 0.5 * size / voxel, median, sd);
                    cuts.push(Cut { source: i, at, bin, score: b.score, strength: b.strength, darkest, cube });
                }
            }
        }
        progress(&format!("裁出颗粒 {size} Å"), sources.len(), sources.len());
        without_gold(&mut cuts, size, voxel_a);
        cuts.sort_by(|a, b| b.score.total_cmp(&a.score));
        cuts.truncate(FIRST);
        if cuts.len() < CARD_BLOBS {
            continue;
        }
        let particles: Vec<Vec<f32>> = cuts.iter().map(|c| c.cube.clone()).collect();
        let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
        let radius = 0.8 * size / voxel_a;
        let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
        // Classification at half sampling.
        let half = BOX / 2;
        let small: Vec<Vec<f32>> = particles.par_iter().map(|p| resample3(p, BOX, half)).collect();
        let small_wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(half, s.tilts[0], s.tilts[1])).collect();
        let coarse = Set { n: half, particles: &small, wedges: &small_wedges, wedge_of: &wedge_of, radius: radius / 2.0 };
        let kind_progress = |s: &str, a: usize, b: usize| progress(&format!("{s} {size} Å"), a, b);
        for class in classes(&coarse, 3, 1, &kind_progress, live)? {
            if class.members.len() < CARD_MEMBERS {
                continue;
            }
            // The poses at full sampling: shifts double.
            let mut poses = vec![Pose { r: super::IDENTITY, t: [0.0; 3], score: 0.0 }; cuts.len()];
            let mut member = vec![false; cuts.len()];
            for (i, p) in class.members.iter().zip(&class.poses) {
                poses[*i] = Pose { t: p.t.map(|v| 2.0 * v), ..*p };
                member[*i] = true;
            }
            // Only the written average is moved to put the particle at the
            // centre; the member poses are not stored or used afterwards.
            let average = set.average(&poses, |i| member[i]).finish();
            let ball = 0.6 * size / voxel_a;
            let average = centred(average, BOX, ball);
            let (compact, elongation, blobs, offset) = shape(&average, BOX, ball);
            let likely = is_likely(compact, elongation, blobs, offset);
            found.push((
                Card {
                    id: 0,
                    diameter_a: size,
                    measured_a: kind.measured_a,
                    voxel_a,
                    members: class.members.len(),
                    share: class.members.len() as f64 / cuts.len() as f64,
                    path: String::new(),
                    compact,
                    elongation,
                    blobs,
                    offset,
                    likely,
                },
                average,
            ));
        }
    }
    found.sort_by(|a, b| rank(&b.0).total_cmp(&rank(&a.0)));
    for (id, (card, average)) in found.iter_mut().enumerate() {
        card.id = id;
        card.path = file(&format!("card_{id}.mrc"));
        output::write_mrc(&card.path, average, [BOX; 3], card.voxel_a, false)?;
    }
    let out: Vec<Card> = found.into_iter().map(|f| f.0).collect();
    std::fs::write(file("cards.json"), serde_json::to_string_pretty(&out)?)?;
    Ok(out)
}

/// Reads a card (or any `BOX`³ template MRC): the cube, z-major, and its
/// voxel size, Å.
pub fn read_card(path: &str) -> Result<(Vec<f32>, f64)> {
    let img = pipeline::open(path).with_context(|| format!("cannot open {path}"))?;
    let info = img.info();
    if info.width != BOX || info.height != BOX || info.size_z != BOX {
        bail!("the template must be {BOX}x{BOX}x{BOX}, {path} is {}x{}x{}", info.width, info.height, info.size_z);
    }
    let voxel = output::pixel_a(&img);
    let mut cube = vec![0f32; BOX * BOX * BOX];
    for z in 0..BOX {
        let (page, s) = info.locate(0, z, 0);
        let p = img.page(page)?;
        let p = &p[s.min(p.len() - 1)];
        for (i, o) in cube[z * BOX * BOX..(z + 1) * BOX * BOX].iter_mut().enumerate() {
            *o = p.data.get_f64(i) as f32;
        }
    }
    Ok((cube, voxel))
}

/// Sort key of a card: the likely ones first, then `compact × members`.
fn rank(c: &Card) -> f64 {
    c.compact * c.members as f64 + if c.likely { 1e12 } else { 0.0 }
}

/// Two likely cards show the same particle when their best masked normalised
/// cross-correlation (see `same_kind`) is at least this. Measured on made-up
/// cards (`sta::tests::cards_of_the_same_particle_correlate`): the same
/// object turned, smoothed, noisy and at another voxel size 0.93 to 0.96; a
/// rod 0.59; a ball twice as large 0.09; another arrangement of three balls
/// 0.79.
pub const SAME: f64 = 0.7;
/// Orientation step of the all-orientations pass of `same_kind`, degrees, and
/// the band of its comparison (Fourier voxels of the `BOX` cube: the lowest
/// frequencies, which only say "a blob in a ball", are left out).
const SAME_STEP: f64 = 20.0;
const SAME_LOW_PASS: f64 = 10.0;
const SAME_HIGH_PASS: f64 = 3.0;
/// Most shift (voxels) allowed between two cards.
const SAME_SHIFT: usize = 2;

/// A card's cube for `same_kind`: `BOX`³ voxels of `voxel_a` Å, and the
/// radius (Å) of the particle it holds.
#[derive(Clone, Copy)]
pub struct CardCube<'a> {
    pub cube: &'a [f32],
    pub voxel_a: f64,
    pub radius_a: f64,
}

impl<'a> CardCube<'a> {
    /// The cube of a card read with `read_card`; the particle radius is the
    /// 0.8 × the card's size `run_template` and `cards` align within.
    pub fn of(card: &Card, cube: &'a [f32]) -> CardCube<'a> {
        CardCube { cube, voxel_a: card.voxel_a, radius_a: 0.8 * card.diameter_a }
    }
}

/// How alike two cards are: the best normalised cross-correlation between
/// them over every orientation (a coarse `SAME_STEP` grid, then two local
/// refinements of its best three) and a shift of a few voxels, within a ball
/// of the smaller card's radius. 1 is identical, near 0 unrelated. The card
/// with the smaller voxel is first resampled to the other's voxel size. Cards
/// are class averages and hold no missing wedge worth masking, so none is
/// applied; the comparison is low-passed, as the cards are only a few resolved
/// features across.
pub fn same_kind(a: CardCube, b: CardCube) -> f64 {
    use crate::spa::geom::local;
    let n = BOX;
    let voxel = a.voxel_a.max(b.voxel_a);
    let radius = a.radius_a.min(b.radius_a) / voxel;
    let mask = cc::ball(n, radius, 3.0);
    let prepare = |c: CardCube| -> Vec<f32> {
        let f = voxel / c.voxel_a;
        let mut v = if (f - 1.0).abs() < 1e-6 { c.cube.to_vec() } else { cc::transform(c.cube, n, &[[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, f]], [0.0; 3]) };
        // Zero mean within the ball, then the ball itself.
        let (sum, weight) = v.iter().zip(&mask).fold((0f64, 0f64), |(s, w), (x, m)| (s + (*x * *m) as f64, w + *m as f64));
        let mean = (sum / weight.max(1e-9)) as f32;
        v.iter_mut().zip(&mask).for_each(|(x, m)| *x = (*x - mean) * m);
        v
    };
    let (x, reference) = (prepare(a), prepare(b));
    let engine = cc::Engine::new(n, SAME_LOW_PASS, SAME_HIGH_PASS, radius);
    let ones = vec![1f32; n * n * n];
    let x = engine.prepare(&x, &ones);
    let reference = engine.reference(&reference);
    let mut tried: Vec<Pose> = grid(SAME_STEP)
        .par_iter()
        .map(|r| {
            let (score, t) = engine.score(&x, &engine.turn(&reference, r), SAME_SHIFT);
            Pose { r: *r, t, score }
        })
        .collect();
    tried.sort_by(|p, q| q.score.total_cmp(&p.score));
    let best = tried
        .iter()
        .take(3)
        .map(|start| {
            let mut pose = *start;
            for (range, steps) in [(SAME_STEP, 2), (SAME_STEP / 3.0, 2)] {
                pose = engine.local(&x, &reference, &pose, &local(range, steps), SAME_SHIFT);
            }
            pose.score
        })
        .fold(f32::MIN, f32::max);
    (best as f64).clamp(-1.0, 1.0)
}

/// `same_kind` between the likely cards, as a matrix indexed like `cards`
/// (1 on the diagonal; the pairs that involve a card that is not likely are
/// not computed and are 0). Reads each likely card's file; the cards are
/// usually two, a pair takes about a second.
pub fn likeness(cards: &[Card]) -> Result<Vec<Vec<f64>>> {
    let likely: Vec<usize> = (0..cards.len()).filter(|i| cards[*i].likely).collect();
    let cubes: Vec<Vec<f32>> = likely.iter().map(|i| read_card(&cards[*i].path).map(|c| c.0)).collect::<Result<_>>()?;
    let mut m = vec![vec![0.0; cards.len()]; cards.len()];
    for (k, &i) in likely.iter().enumerate() {
        m[i][i] = 1.0;
        for (l, &j) in likely.iter().enumerate().skip(k + 1) {
            m[i][j] = same_kind(CardCube::of(&cards[i], &cubes[k]), CardCube::of(&cards[j], &cubes[l]));
            m[j][i] = m[i][j];
        }
    }
    Ok(m)
}

/// The card to take without asking: the only likely one; or, when several are
/// likely and every pair of them is `SAME` or more alike (`similar[i][j]`, see
/// `likeness`), the one with the largest `compact × members` (what `rank`
/// sorts by, so the first of them); otherwise None, and the user chooses.
pub fn choose(cards: &[Card], similar: &[Vec<f64>]) -> Option<usize> {
    let likely: Vec<usize> = (0..cards.len()).filter(|i| cards[*i].likely).collect();
    let first = *likely.first()?;
    let alike = likely.iter().all(|&i| likely.iter().all(|&j| i >= j || similar.get(i).and_then(|r| r.get(j)).is_some_and(|s| *s >= SAME)));
    alike.then(|| likely.iter().copied().max_by(|&i, &j| rank(&cards[i]).total_cmp(&rank(&cards[j])).then(j.cmp(&i))).unwrap_or(first))
}

/// A place on a drawn shape (see `shape`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Drawn {
    /// Which of the sources.
    pub source: usize,
    /// Position in voxels of that tomogram, (x, y, z).
    pub pos: [f64; 3],
    /// The filament axis or the surface normal there.
    pub axis: [f64; 3],
}

/// The average of what lies on drawn shapes. `drawn` samples the shapes
/// more densely than the particles sit (about `size_a` / 2 apart; `size_a`
/// is the size of one particle, Å); `both` when the sense of the axes is
/// not known. Seeds that end on the same particle are merged.
pub fn run_drawn(sources: &[Source], drawn: &[Drawn], both: bool, size_a: f64, out_dir: &str, progress: Progress, live: Live) -> Result<Report> {
    let started = Instant::now();
    let mut cuts: Vec<Cut> = vec![];
    let mut axes: Vec<[f64; 3]> = vec![];
    let mut voxel_a = size_a / SPAN;
    for (i, s) in sources.iter().enumerate() {
        progress("裁出颗粒", i, sources.len());
        if !drawn.iter().any(|d| d.source == i) {
            continue;
        }
        let (v, dims, voxel, bin) = binned(&s.path, size_a / SPAN)?;
        voxel_a = voxel;
        let (median, sd) = super::pick::robust(&v);
        for d in drawn.iter().filter(|d| d.source == i) {
            // Source voxel p lies in working voxel p / bin.
            let c = d.pos.map(|p| (p / bin as f64).floor().max(0.0) as usize);
            if let Some(cube) = cube(&v, dims, c) {
                let at = c.map(|v| v as f64 * bin as f64 + (bin as f64 - 1.0) / 2.0);
                let darkest = darkest(&v, dims, c, 0.5 * size_a / voxel, median, sd);
                cuts.push(Cut { source: i, at, bin, score: 0.0, strength: 0.0, darkest, cube });
                axes.push(d.axis);
            }
        }
    }
    progress("裁出颗粒", sources.len(), sources.len());
    if cuts.len() < 20 {
        bail!("画出的范围里只取到 {} 个位置，太少了", cuts.len());
    }
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let radius = 0.8 * size_a / voxel_a;
    let particles: Vec<Vec<f32>> = cuts.iter().map(|c| c.cube.clone()).collect();
    let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    let (model, poses) = super::refine::first_model_known(&set, &axes, both, 1, progress, live)?;

    // Seeds that moved onto the same particle: the best of each stays.
    let mut keep: Vec<usize> = vec![];
    for s in 0..sources.len() {
        let of: Vec<usize> = (0..cuts.len()).filter(|i| cuts[*i].source == s).collect();
        let at: Vec<[f64; 3]> = of.iter().map(|i| std::array::from_fn(|k| cuts[*i].at[k] / cuts[*i].bin as f64 + poses[*i].t[k])).collect();
        let scores: Vec<f32> = of.iter().map(|i| poses[*i].score).collect();
        keep.extend(super::shape::merge(&at, &scores, 0.5 * size_a / voxel_a).into_iter().map(|k| of[k]));
    }
    // Stretches of the shape that carry no particle match clearly worse.
    let scores: Vec<f32> = keep.iter().map(|i| poses[*i].score).collect();
    let (threshold, distinct) = split(&scores);
    if distinct >= DISTINCT && scores.iter().filter(|s| **s >= threshold).count() >= 4 {
        keep.retain(|i| poses[*i].score >= threshold);
    }
    keep.sort_unstable();
    let candidates = cuts.len();
    let poses: Vec<Pose> = keep.iter().map(|i| poses[*i]).collect();
    let particles: Vec<Vec<f32>> = keep.iter().map(|i| particles[*i].clone()).collect();
    let wedge_of: Vec<usize> = keep.iter().map(|i| wedge_of[*i]).collect();
    let mut at = 0;
    cuts.retain(|_| {
        at += 1;
        keep.binary_search(&(at - 1)).is_ok()
    });
    if cuts.len() < 4 {
        bail!("合并重复位置后只剩 {} 个颗粒", cuts.len());
    }
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    let out = align(&set, &model, &poses, progress, live)?;
    let mut report = Report {
        kinds: vec![],
        diameter_a: None,
        particles: 0,
        candidates,
        distinct: Some(distinct),
        voxel_a: 0.0,
        searched: None,
        search_voxel_a: None,
        resolution_a: None,
        signal_a: None,
        ctf_limited: false,
        fsc: vec![],
        averaged_from: None,
        fsc_other: None,
        map: None,
        halves: None,
        table: None,
        ms: 0,
    };
    save(&mut report, sources, &cuts, &out, out_dir, size_a, voxel_a, started)?;
    Ok(report)
}

/// The pixel size of a tomogram file, Å.
fn pixel_of(path: &str) -> Result<f64> {
    Ok(output::pixel_a(&pipeline::open(path)?))
}

/// The integer binning that brings a grid of `px` Å voxels nearest to `want` Å
/// voxels (nearest in the ratio, not the difference), at least 1.
pub(crate) fn nearest_bin(px: f64, want: f64) -> usize {
    let r = want / px;
    let lo = (r.floor() as usize).max(1);
    let hi = lo + 1;
    // Equal ratio steps either side of sqrt(lo·hi).
    if r > ((lo * hi) as f64).sqrt() {
        hi
    } else {
        lo
    }
}

/// Which grid a template search runs on, for a look tomogram of `look_px` Å
/// voxels and a template of `want` Å voxels: the look tomogram binned by the
/// integer nearest to the template's voxel, and then halved (`2`) or not (`1`)
/// so the search voxel is nearest to twice the template's, the voxel the raw
/// tomogram is searched at. Returns (bin, halving, search voxel).
pub(crate) fn look_search(look_px: f64, want: f64) -> (usize, usize, f64) {
    let bin = nearest_bin(look_px, want);
    let grid = bin as f64 * look_px;
    let far = |h: usize| (h as f64 * grid / (2.0 * want)).ln().abs();
    let half = if far(2) <= far(1) { 2 } else { 1 };
    (bin, half, half as f64 * grid)
}

/// The `n_in`³ cube `cube` (voxels of `from` Å, centre voxel `n_in / 2`) on a
/// `n_out`³ grid of `to` Å voxels about the same centre (`n_out / 2`): every
/// output voxel is the mean of trilinear samples over its own footprint, so a
/// coarser grid is not aliased. What lies outside the input is the mean of its
/// faces.
pub(crate) fn rescale_cube(cube: &[f32], n_in: usize, from: f64, n_out: usize, to: f64) -> Vec<f32> {
    assert_eq!(cube.len(), n_in * n_in * n_in);
    let r = to / from;
    let at = |x: usize, y: usize, z: usize| cube[(z * n_in + y) * n_in + x] as f64;
    let mut edge = (0.0, 0usize);
    for z in 0..n_in {
        for y in 0..n_in {
            for x in 0..n_in {
                if [x, y, z].iter().any(|&p| p == 0 || p == n_in - 1) {
                    edge = (edge.0 + at(x, y, z), edge.1 + 1);
                }
            }
        }
    }
    let fill = edge.0 / edge.1.max(1) as f64;
    let sample = |p: [f64; 3]| -> f64 {
        let f = p.map(|v| v.floor());
        let t: [f64; 3] = std::array::from_fn(|k| p[k] - f[k]);
        let mut s = 0.0;
        for c in 0..8 {
            let (mut w, mut q, mut inside) = (1.0, [0usize; 3], true);
            for k in 0..3 {
                let up = (c >> k) & 1;
                w *= if up == 1 { t[k] } else { 1.0 - t[k] };
                let i = f[k] as i64 + up as i64;
                if i < 0 || i >= n_in as i64 {
                    inside = false;
                } else {
                    q[k] = i as usize;
                }
            }
            if w > 0.0 {
                s += w * if inside { at(q[0], q[1], q[2]) } else { fill };
            }
        }
        s
    };
    let m = (r - 1e-9).ceil().max(1.0) as usize;
    let (ci, co) = ((n_in / 2) as f64, (n_out / 2) as f64);
    let mut out = vec![0f32; n_out * n_out * n_out];
    out.par_chunks_mut(n_out * n_out).enumerate().for_each(|(z, plane)| {
        for y in 0..n_out {
            for x in 0..n_out {
                let mut s = 0.0;
                for k in 0..m * m * m {
                    let sub = [k % m, k / m % m, k / m / m];
                    let p: [f64; 3] = std::array::from_fn(|a| {
                        let o = [x, y, z][a] as f64 - co + ((sub[a] as f64 + 0.5) / m as f64 - 0.5);
                        ci + o * r
                    });
                    s += sample(p);
                }
                plane[y * n_out + x] = (s / (m * m * m) as f64) as f32;
            }
        }
    });
    out
}

/// Half the sampling: the mean of every 2×2×2 voxels.
fn halve(v: &[f32], dims: [usize; 3]) -> (Vec<f32>, [usize; 3]) {
    let [nx, ny, nz] = dims;
    let (w, h, d) = (nx / 2, ny / 2, nz / 2);
    let mut out = vec![0f32; w * h * d];
    out.par_chunks_mut(w * h).enumerate().for_each(|(z, plane)| {
        for y in 0..h {
            for x in 0..w {
                let mut s = 0f32;
                for k in 0..2 {
                    for j in 0..2 {
                        let row = ((2 * z + k) * ny + 2 * y + j) * nx + 2 * x;
                        s += v[row] + v[row + 1];
                    }
                }
                plane[y * w + x] = s / 8.0;
            }
        }
    });
    (out, [w, h, d])
}

/// The average of everything in the tomograms that looks like `template`
/// (a `BOX`³ cube of `voxel_a` Å voxels: a class average of found blobs, of
/// clicked examples, or a map the user has). Each tomogram is searched at
/// half the template's sampling on `dev`, away from its borders and gold
/// markers; the peaks the threshold keeps are cut at the template's
/// sampling, start at the orientation the search found and are aligned in
/// two halves. `diameter_a` is the particle's size.
///
/// A source with a look tomogram (`Source::look`, a clear version of the same
/// tomogram) is searched on that one instead (the search, the peaks and the
/// threshold; the exclusion mask is made from the raw tomogram and carried
/// over, see `allowed_on_look`). Its voxel needs no relation to the
/// template's: it is binned by the integer that brings it nearest to the
/// template's, halved or not (`look_search`) so the search voxel is nearest to
/// the raw search's, and the template is resampled to that voxel (`rescale_cube`)
/// for the search only. Each peak is then mapped to the tomogram itself
/// (`to_raw`) and the cubes for alignment and averaging, in both passes, are
/// cut from that, so the averages are of the unfiltered data. The template,
/// made from look data, is only the reference the alignment starts from, at
/// its own voxel size. The look tomogram is left out only when it cannot be
/// read. `Report::searched` and `search_voxel_a` say what was done.
#[allow(clippy::too_many_arguments)]
pub fn run_template(
    dev: &burn::tensor::Device,
    sources: &[Source],
    template: &[f32],
    voxel_a: f64,
    diameter_a: f64,
    fine: bool,
    out_dir: &str,
    progress: Progress,
    live: Live,
) -> Result<Report> {
    use super::search::{allowed, cutoff, peaks};
    let started = Instant::now();
    if sources.is_empty() {
        bail!("没有断层图");
    }
    assert_eq!(template.len(), BOX * BOX * BOX);
    let turns = grid(30.0);
    let mut cuts: Vec<Cut> = vec![];
    let mut poses: Vec<Pose> = vec![];
    let mut voxel = voxel_a;
    // (source, coarse peak) of every tomogram, to share one threshold.
    let mut all: Vec<(usize, super::search::Peak)> = vec![];
    let mut volumes = vec![];
    // (look searched, search voxel Å) per source.
    let mut searched: Vec<(bool, f64)> = vec![];
    for (i, s) in sources.iter().enumerate() {
        check_cancel()?;
        // The raw tomogram the particles are cut from, and the one searched:
        // the look tomogram when there is one, else the raw one. The look
        // tomogram is binned by the integer that brings its voxel nearest
        // the template's (`look_search`), halved or not so the search voxel
        // is nearest to the raw search's (twice the template's), and the
        // template is resampled to that voxel for the search only.
        let (v, dims, got, bin) = binned(&s.path, voxel_a)?;
        voxel = got;
        let looked = match &s.look {
            Some(l) => match pixel_of(&l.path).and_then(|px| {
                let (lbin, half, _) = look_search(px, got);
                binned(&l.path, lbin as f64 * px).map(|b| (b, half))
            }) {
                Ok((b, half)) => Some((l.clone(), b, half)),
                Err(e) => {
                    eprintln!("sta: the look tomogram of {} cannot be used ({e:#}): searching the tomogram itself", s.path);
                    None
                }
            },
            None => None,
        };
        let (found_on, found_dims, found_voxel, half) = match &looked {
            Some((_, b, h)) => (&b.0, b.1, b.2, *h),
            None => (&v, dims, got, 2),
        };
        let (coarse, cdims) = if half == 2 { halve(found_on, found_dims) } else { (found_on.clone(), found_dims) };
        let search_voxel = half as f64 * found_voxel;
        searched.push((looked.is_some(), search_voxel));
        let radius = 0.5 * diameter_a / search_voxel;
        let mask = match &looked {
            None => allowed(&coarse, cdims, radius),
            // Exclusion is decided on the raw tomogram, as without a look
            // one, and carried over to the look grid (plus its own borders).
            Some((l, b, h)) => {
                let (raw_coarse, raw_cdims) = halve(&v, dims);
                let raw_ok = allowed(&raw_coarse, raw_cdims, 0.25 * diameter_a / got);
                allowed_on_look(&raw_ok, raw_cdims, 2, bin, cdims, *h, b.3, l, radius)
            }
        };
        // The template for the search, at the search voxel: the raw path
        // halves it; on the look grid it is resampled about its centre.
        let (small, small_n) = if looked.is_none() {
            (crate::spa::fourier::resample3(template, BOX, BOX / 2), BOX / 2)
        } else {
            let n = if 2.0 * radius + 4.0 <= (BOX / 2) as f64 { BOX / 2 } else { BOX };
            (rescale_cube(template, BOX, got, n, search_voxel), n)
        };
        let stage = format!("搜索 {}/{}", i + 1, sources.len());
        let found = super::search_gpu::search(dev, &coarse, cdims, &small, small_n, radius, s.tilts, &turns, Some(&mask), &|a, b| progress(&stage, a, b))?;
        // Particles do not overlap, but may touch.
        all.extend(peaks(&found, 1.8 * radius, MOST).into_iter().map(|p| (i, p)));
        volumes.push((v, dims, bin, looked.map(|(l, b, h)| (l, b.3, h))));
    }
    let scores: Vec<super::search::Peak> = all.iter().map(|p| p.1).collect();
    let candidates = scores.len();
    let Some(limit) = cutoff(&scores) else {
        bail!("没有找到明显像模板的颗粒（{} 个候选的分数分不出两群）", candidates);
    };
    all.retain(|p| p.1.score >= limit);
    all.sort_by(|a, b| b.1.score.total_cmp(&a.1.score));
    all.truncate(MOST);
    for (i, p) in &all {
        let (v, dims, bin, look) = &volumes[*i];
        // Coarse voxel p holds fine voxels 2p and 2p + 1.
        let c = match look {
            None => p.pos.map(|x| 2 * x),
            // On the look tomogram the peak is at the middle of the h voxels
            // it holds, h p + (h − 1)/2; mapped onto the raw tomogram's
            // binned voxels.
            Some((look, look_bin, h)) => {
                let raw = to_raw(p.pos.map(|x| *h as f64 * x as f64 + (*h as f64 - 1.0) / 2.0), *look_bin, *bin, look);
                if raw.iter().any(|x| !x.is_finite() || x.round() < 0.0) {
                    continue;
                }
                raw.map(|x| x.round() as usize)
            }
        };
        if let Some(cube) = cube(v, *dims, c) {
            let at = c.map(|x| x as f64 * *bin as f64 + (*bin as f64 - 1.0) / 2.0);
            cuts.push(Cut { source: *i, at, bin: *bin, score: p.score, strength: 0.0, darkest: 0.0, cube });
            poses.push(Pose { r: turns[p.turn], t: [0.0; 3], score: p.score });
        }
    }
    drop(volumes);
    if cuts.len() < 20 {
        bail!("只找到 {} 个颗粒，太少了", cuts.len());
    }
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let radius = (0.5 * diameter_a / voxel + 2.0).min(BOX as f64 / 2.0 - 3.0);
    let particles: Vec<Vec<f32>> = cuts.iter().map(|c| c.cube.clone()).collect();
    let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    // Told apart in the labels when there are two passes.
    let first_pass = |s: &str, a: usize, b: usize| progress(&if fine { s.replacen("对齐", "粗对齐", 1) } else { s.to_string() }, a, b);
    let mut out = align(&set, template, &poses, &first_pass, live)?;
    if fine {
        // The same particles at twice the sampling, from where they now
        // are: at the coarse sampling the resolution ends at its Nyquist
        // limit, which says nothing about how good the average is.
        let n = 2 * BOX;
        let coarse = voxel;
        let want = voxel / 2.0;
        let mut big: Vec<Vec<f32>> = vec![];
        let mut start: Vec<Pose> = vec![];
        let mut kept: Vec<bool> = vec![];
        for (i, s) in sources.iter().enumerate() {
            if !cuts.iter().any(|c| c.source == i) {
                continue;
            }
            check_cancel()?;
            progress("裁出更细的颗粒", i, sources.len());
            let (v, dims, got, bin) = binned(&s.path, want)?;
            for (c, p) in cuts.iter_mut().zip(&out.poses).filter(|(c, _)| c.source == i) {
                // The particle's centre in source voxels, then in these.
                let centre: [f64; 3] = std::array::from_fn(|k| c.at[k] + p.t[k] * c.bin as f64);
                let at = centre.map(|x| ((x - (bin as f64 - 1.0) / 2.0) / bin as f64).round().max(0.0) as usize);
                match cube_n(&v, dims, at, n) {
                    Some(cube) => {
                        big.push(cube);
                        start.push(Pose { r: p.r, t: [0.0; 3], score: p.score });
                        c.at = at.map(|x| x as f64 * bin as f64 + (bin as f64 - 1.0) / 2.0);
                        c.bin = bin;
                        kept.push(true);
                    }
                    None => kept.push(false),
                }
            }
            voxel = got;
        }
        // `kept` follows the order of the sources, as `cuts` does only when
        // sorted by source: sort both the same way first.
        let mut order: Vec<usize> = (0..cuts.len()).collect();
        order.sort_by_key(|i| cuts[*i].source);
        let mut it = kept.iter();
        let stays: Vec<bool> = {
            let mut flags = vec![false; cuts.len()];
            for i in &order {
                flags[*i] = *it.next().unwrap();
            }
            flags
        };
        let mut sorted: Vec<Cut> = vec![];
        let mut taken: Vec<Option<Cut>> = cuts.drain(..).map(Some).collect();
        for i in &order {
            if stays[*i] {
                sorted.push(taken[*i].take().unwrap());
            }
        }
        cuts = sorted;
        let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(n, s.tilts[0], s.tilts[1])).collect();
        let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
        let radius = (0.5 * diameter_a / voxel + 2.0).min(n as f64 / 2.0 - 3.0);
        // The tomogram bins by whole numbers only, so the finer voxel is
        // not always half the coarse one (13.0 Å gives 8.7 Å, not 6.5 Å).
        let model = if (coarse / voxel - 2.0).abs() < 1e-3 { crate::spa::fourier::resample3(&out.map, BOX, n) } else { rescale_cube(&out.map, BOX, coarse, n, voxel) };
        let set = Set { n, particles: &big, wedges: &wedges, wedge_of: &wedge_of, radius };
        out = align(&set, &model, &start, &|s: &str, a: usize, b: usize| progress(&s.replacen("对齐", "精细对齐", 1), a, b), live)?;
    }
    let mut report = Report {
        kinds: vec![],
        diameter_a: None,
        particles: 0,
        candidates,
        distinct: None,
        voxel_a: 0.0,
        searched: None,
        search_voxel_a: None,
        resolution_a: None,
        signal_a: None,
        ctf_limited: false,
        fsc: vec![],
        averaged_from: None,
        fsc_other: None,
        map: None,
        halves: None,
        table: None,
        ms: 0,
    };
    report.searched = Some(
        if searched.iter().all(|s| s.0) {
            "look"
        } else if searched.iter().all(|s| !s.0) {
            "raw"
        } else {
            "mixed"
        }
        .to_string(),
    );
    report.search_voxel_a = searched.first().map(|s| s.1);
    // The same particles, where they now are, from the tilt images.
    if fine && sources.iter().all(|s| s.series.is_some()) {
        match from_tilts(sources, &cuts, &out, voxel, diameter_a, progress) {
            Ok(tilted) => {
                let area = |f: &[f64]| f.iter().skip(1).map(|v| v.max(0.0)).sum::<f64>();
                let better = area(&tilted.fsc) >= area(&out.fsc);
                let (kept, other) = if better { (tilted, out) } else { (out, tilted) };
                report.averaged_from = Some(if better { "tilt images" } else { "tomogram" }.into());
                report.fsc_other = Some(other.fsc);
                out = kept;
                live(&out.map);
            }
            Err(e) => eprintln!("sta: the average from the tilt images could not be made ({e}); keeping the one from the tomograms"),
        }
    }
    save(&mut report, sources, &cuts, &out, out_dir, diameter_a, voxel, started)?;
    Ok(report)
}

/// The average of the aligned particles made from the tilt images (`tilt`),
/// on the grid of `out` (voxels of `voxel` Å), with the same halves.
fn from_tilts(sources: &[Source], cuts: &[Cut], out: &super::refine::Aligned, voxel: f64, diameter_a: f64, progress: Progress) -> Result<super::refine::Aligned> {
    use super::tilt::{Particle, Series, Sums};
    let n = (out.map.len() as f64).cbrt().round() as usize;
    let mut sums = Sums::new(n, n as f64 / 2.0 - 2.0);
    for (i, s) in sources.iter().enumerate() {
        let particles: Vec<Particle> = cuts
            .iter()
            .zip(&out.poses)
            .enumerate()
            .filter(|(_, (c, _))| c.source == i)
            .map(|(j, (c, p))| Particle { at: std::array::from_fn(|k| c.at[k] + p.t[k] * c.bin as f64), r: p.r, half: j % 2 })
            .collect();
        if particles.is_empty() {
            continue;
        }
        check_cancel()?;
        let file = s.series.as_ref().unwrap();
        let mut d: crate::tomo::dataset::Dataset = serde_json::from_slice(&std::fs::read(file).with_context(|| format!("无法读取 {file}"))?)?;
        // The folder may have been moved: then the stack is beside the dataset.
        if !Path::new(&d.stack).is_file() {
            if let (Some(dir), Some(name)) = (Path::new(file).parent(), Path::new(&d.stack).file_name()) {
                d.stack = dir.join(name).to_string_lossy().into_owned();
            }
        }
        let name = Path::new(&s.path).file_name();
        let Some(t) = d.tomograms.iter().find(|t| Path::new(&t.path).file_name() == name).cloned() else { bail!("{file} 里没有 {} 的记录", s.path) };
        let info = pipeline::open(&s.path)?;
        let dims = [info.info().width, info.info().height, info.info().size_z];
        let Some(px) = d.optics.pixel_a else { bail!("不知道像素大小") };
        let bin = (voxel / px).round().max(1.0) as usize;
        if (bin as f64 * px / voxel - 1.0).abs() > 0.01 {
            bail!("倾斜图像的像素（{px:.3} Å）凑不成平均图的体素（{voxel:.3} Å）");
        }
        let label = format!("从倾斜图像平均 {}/{}", i + 1, sources.len());
        let series = Series::open(&d, &t, dims, bin, &|a, b| progress(&label, a, 2 * b))?;
        sums.add(&series, &particles, &|a, b| progress(&label, b + a, 2 * b))?;
    }
    if sums.count < 4 {
        bail!("颗粒太少");
    }
    let radius = (0.5 * diameter_a / voxel + 2.0).min(n as f64 / 2.0 - 3.0);
    let (map, halves, fsc) = sums.finish(radius + 2.0);
    let shell = super::refine::cut(&fsc);
    Ok(super::refine::Aligned { map, halves, poses: out.poses.clone(), fsc, shell })
}

#[cfg(test)]
mod card_tests {
    use super::*;

    fn likely_card(id: usize, likely: bool, compact: f64, members: usize) -> Card {
        Card { id, diameter_a: 160.0, measured_a: 150.0, voxel_a: 12.0, members, share: 0.3, path: String::new(), compact, elongation: 1.1, blobs: 1, offset: 0.0, likely }
    }

    #[test]
    fn several_likely_cards_are_chosen_only_when_they_show_the_same_particle() {
        let sim = |v: &[&[f64]]| -> Vec<Vec<f64>> { v.iter().map(|r| r.to_vec()).collect() };
        // The real case: 160 Å (79 members, 7.72) and 210 Å (52, 10.14), both likely.
        let two = [likely_card(0, true, 7.72, 79), likely_card(1, true, 10.14, 52)];
        assert_eq!(choose(&two, &sim(&[&[1.0, 0.8], &[0.8, 1.0]])), Some(0));
        // The larger compact × members wins whatever the order.
        let swapped = [likely_card(0, true, 10.14, 52), likely_card(1, true, 7.72, 79)];
        assert_eq!(choose(&swapped, &sim(&[&[1.0, 0.8], &[0.8, 1.0]])), Some(1));
        let reversed = [likely_card(0, true, 5.0, 40), likely_card(1, true, 10.14, 52)];
        assert_eq!(choose(&reversed, &sim(&[&[1.0, 0.8], &[0.8, 1.0]])), Some(1));
        // Not alike: the user decides.
        assert_eq!(choose(&two, &sim(&[&[1.0, 0.3], &[0.3, 1.0]])), None);
        assert_eq!(choose(&two, &sim(&[&[1.0, SAME - 0.01], &[SAME - 0.01, 1.0]])), None);
        assert_eq!(choose(&two, &sim(&[&[1.0, SAME], &[SAME, 1.0]])), Some(0));
        // Three: one odd pair spoils it. Cards that are not likely do not count.
        let three = [likely_card(0, true, 8.0, 60), likely_card(1, false, 12.0, 90), likely_card(2, true, 9.0, 50), likely_card(3, true, 7.6, 40)];
        let m = sim(&[&[1.0, 0.0, 0.9, 0.8], &[0.0; 4], &[0.9, 0.0, 1.0, 0.75], &[0.8, 0.0, 0.75, 1.0]]);
        assert_eq!(choose(&three, &m), Some(0));
        let m = sim(&[&[1.0, 0.0, 0.9, 0.5], &[0.0; 4], &[0.9, 0.0, 1.0, 0.75], &[0.5, 0.0, 0.75, 1.0]]);
        assert_eq!(choose(&three, &m), None);
        // One likely card (the others may be anything); none; empty; no matrix needed for one.
        assert_eq!(choose(&[likely_card(0, false, 12.0, 90), likely_card(1, true, 8.0, 50)], &[]), Some(1));
        assert_eq!(choose(&[likely_card(0, false, 12.0, 90)], &[]), None);
        assert_eq!(choose(&[], &[]), None);
    }

    const N: usize = 32;

    /// A box filled by `f(x, y, z)`, coordinates from the centre.
    fn volume(mut f: impl FnMut(f64, f64, f64) -> f64) -> Vec<f32> {
        let c = (N / 2) as f64;
        (0..N * N * N).map(|i| f((i % N) as f64 - c, (i / N % N) as f64 - c, (i / N / N) as f64 - c) as f32).collect()
    }

    fn blob(x: f64, y: f64, z: f64, s: f64) -> f64 {
        (-(x * x + y * y + z * z) / (2.0 * s * s)).exp()
    }

    /// The radius `cards` uses for a particle 13 voxels across.
    const R: f64 = 7.8;

    fn card(avg: &[f32], members: usize) -> Card {
        let (compact, elongation, blobs, offset) = shape(avg, N, R);
        let likely = is_likely(compact, elongation, blobs, offset);
        Card { id: 0, diameter_a: 0.0, measured_a: 0.0, voxel_a: 1.0, members, share: 0.0, path: String::new(), compact, elongation, blobs, offset, likely }
    }

    /// A class average: the particle dark, a little noise on it.
    fn dark(f: impl Fn(f64, f64, f64) -> f64, noise: f64) -> Vec<f32> {
        let mut rng = Rng(5);
        volume(|x, y, z| -f(x, y, z) + noise * rng.gauss()).iter().map(|v| *v as f32).collect()
    }

    /// Ball (sigma 3.5), optionally stretched along Z.
    fn ball(stretch: f64) -> impl Fn(f64, f64, f64) -> f64 {
        move |x, y, z| blob(x, y, z / stretch, 3.5)
    }

    fn report(name: &str, c: &Card) {
        eprintln!("{name}: compact {:.2}, elongation {:.2}, blobs {}, offset {:.2}, likely {}", c.compact, c.elongation, c.blobs, c.offset, c.likely);
    }

    #[test]
    fn single_particles_are_likely() {
        for (name, avg) in [
            ("ball", dark(ball(1.0), 0.0)),
            ("wedge-stretched ball 1.3", dark(ball(1.3), 0.0)),
            ("ball with noise", dark(ball(1.0), 0.05)),
            ("stretched ball with noise", dark(ball(1.3), 0.05)),
        ] {
            let c = card(&avg, 50);
            report(name, &c);
            assert!(c.likely, "{name}");
            assert!(c.elongation < 1.5, "{name}");
        }
    }

    #[test]
    fn lines_layers_and_pairs_are_not_likely() {
        let cases = [
            ("rod", dark(|x, y, _| blob(x, y, 0.0, 2.5), 0.0)),
            ("rod along x with noise", dark(|_, y, z| blob(0.0, y, z, 2.5), 0.05)),
            ("slab", dark(|_, _, z| blob(0.0, 0.0, z, 2.5), 0.0)),
            ("slab with noise", dark(|_, _, z| blob(0.0, 0.0, z, 2.5), 0.05)),
            ("two balls", dark(|x, y, z| blob(x - 4.5, y, z, 2.5) + blob(x + 4.5, y, z, 2.5), 0.0)),
            ("two balls with noise", dark(|x, y, z| blob(x - 4.5, y, z, 2.5) + blob(x + 4.5, y, z, 2.5), 0.05)),
            ("ball off the centre by 0.8 radius", dark(|x, y, z| blob(x - 5.2, y, z, 2.5), 0.0)),
        ];
        for (name, avg) in cases {
            let c = card(&avg, 50);
            report(name, &c);
            assert!(!c.likely, "{name}");
        }
    }

    #[test]
    fn a_ball_a_little_off_centre_is_likely_and_gets_centred() {
        let off = dark(|x, y, z| blob(x - 1.95, y, z, 3.5), 0.0);
        let c = card(&off, 50);
        report("ball off by 0.3 radius", &c);
        assert!(c.likely && c.blobs == 1 && (c.offset - 0.3).abs() < 0.05, "{c:?}");
        let fixed = centred(off, N, R);
        let c = card(&fixed, 50);
        report("  centred", &c);
        assert!(c.likely && c.offset < 0.1, "{c:?}");
        // Not moved when far off the centre.
        let far = dark(|x, y, z| blob(x - 5.2, y, z, 2.5), 0.0);
        assert_eq!(centred(far.clone(), N, R), far);
    }

    #[test]
    fn centring_moves_a_planted_blob_to_the_centre() {
        let planted = dark(|x, y, z| blob(x - 2.0, y + 1.0, z - 3.0, 3.0), 0.0);
        let moved = centred(planted, N, 12.0);
        let c = (N / 2) as f64;
        let (mut m, mut p) = (0.0, [0.0; 3]);
        for (i, v) in moved.iter().enumerate() {
            let w = (-*v as f64).max(0.0);
            m += w;
            p[0] += w * (i % N) as f64;
            p[1] += w * (i / N % N) as f64;
            p[2] += w * (i / N / N) as f64;
        }
        for k in 0..3 {
            assert!((p[k] / m - c).abs() < 0.2, "axis {k}: {}", p[k] / m);
        }
    }

    #[test]
    fn eigenvalues_of_a_known_matrix() {
        let l = eigenvalues([[2.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 0.0, 5.0]]);
        assert!((l[0] - 5.0).abs() < 1e-9 && (l[1] - 3.0).abs() < 1e-9 && (l[2] - 1.0).abs() < 1e-9, "{l:?}");
    }

    #[test]
    fn the_compact_particle_ranks_first() {
        // A blurred ball with four sub-blobs.
        let particle = dark(|x, y, z| blob(x, y, z, 3.5) + [(4.0, 0.0, 0.0), (-4.0, 0.0, 0.0), (0.0, 4.0, 1.0), (0.0, -3.0, -3.0)].iter().map(|p| 0.5 * blob(x - p.0, y - p.1, z - p.2, 1.5)).sum::<f64>(), 0.0);
        let line = dark(|x, y, _| blob(x, y, 0.0, 1.5), 0.0);
        let layer = dark(|_, _, z| blob(0.0, 0.0, z, 2.0), 0.0);
        let mut cards = vec![card(&line, 60), card(&layer, 70), card(&particle, 50)];
        for c in &cards {
            report(&format!("members {}", c.members), c);
        }
        cards.sort_by(|a, b| rank(b).total_cmp(&rank(a)));
        assert_eq!(cards[0].members, 50);
        assert_eq!(cards.iter().filter(|c| c.likely).count(), 1);
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;

    #[test]
    fn a_card_reads_back_with_its_voxel_size() {
        let dir = std::env::temp_dir().join(format!("tmv_card_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("card_0.mrc").to_string_lossy().into_owned();
        let cube: Vec<f32> = (0..BOX * BOX * BOX).map(|i| (i % 97) as f32 - 40.0).collect();
        output::write_mrc(&path, &cube, [BOX; 3], 6.25, false).unwrap();
        let (back, voxel) = read_card(&path).unwrap();
        assert_eq!(back, cube);
        assert!((voxel - 6.25).abs() < 1e-3, "voxel {voxel}");
        let flat = dir.join("flat.mrc").to_string_lossy().into_owned();
        output::write_mrc(&flat, &vec![0f32; BOX * BOX * 8], [BOX, BOX, 8], 6.25, false).unwrap();
        assert!(read_card(&flat).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod fsc_tests {
    use super::resolution;

    /// Two real curves (EMPIAR data): the FSC falls to about 0.1, comes back
    /// above 0.3 beyond a CTF zero and falls again.
    const DIP_1: [f64; 33] = [
        1.0, 0.999, 0.998, 0.995, 0.991, 0.982, 0.969, 0.970, 0.957, 0.942, 0.919, 0.915, 0.910, 0.898, 0.857, 0.845, 0.804, 0.726, 0.592, 0.406, 0.147, 0.107, 0.192, 0.305, 0.366,
        0.379, 0.317, 0.169, 0.063, 0.011, 0.013, 0.060, 0.076,
    ];
    const DIP_2: [f64; 33] = [
        1.0, 0.996, 0.998, 0.998, 0.995, 0.992, 0.990, 0.988, 0.982, 0.975, 0.971, 0.968, 0.966, 0.959, 0.941, 0.928, 0.914, 0.877, 0.810, 0.608, 0.232, 0.078, 0.306, 0.518, 0.577,
        0.556, 0.495, 0.324, 0.152, 0.054, 0.026, 0.075, 0.077,
    ];

    /// A real run: the FSC dips to 0.177 at the zero (not below 0.143), where an
    /// earlier run of the same data dipped to 0.107.
    const DIP_3: [f64; 33] = [
        1.0, 0.998, 0.998, 0.993, 0.992, 0.990, 0.983, 0.983, 0.979, 0.971, 0.958, 0.955, 0.956, 0.947, 0.930, 0.927, 0.902, 0.865, 0.805, 0.650, 0.378, 0.177, 0.326, 0.496, 0.565,
        0.575, 0.488, 0.329, 0.143, 0.021, 0.060, 0.109, 0.125,
    ];

    #[test]
    fn a_dip_that_rises_again_is_a_ctf_zero() {
        for dip in [&DIP_1[..], &DIP_2[..], &DIP_3[..]] {
            assert!(resolution(dip).dip);
        }
        // The curve that dips to 0.177: the limit is the dip (shell 21), the
        // signal reaches shell 28.
        let r = resolution(&DIP_3);
        assert_eq!(r.limit, 21.0);
        assert!((r.signal - 28.0).abs() < 0.05, "{}", r.signal);
        // The same data in an earlier run, 0.107 at the dip: the same numbers.
        let mut lower = DIP_3;
        lower[21] = 0.107;
        let l = resolution(&lower);
        assert!((l.limit - 20.0 - (0.378 - 0.143) / (0.378 - 0.107)).abs() < 1e-9 && l.dip);
        assert!((l.limit - r.limit).abs() < 1.0 && l.signal == r.signal);
        // Falling through 0.143 before the dip: the crossing is the limit.
        let r = resolution(&DIP_1);
        assert!((r.limit - 20.1).abs() < 0.01 && (r.signal - 27.0).abs() < 1.0, "{r:?}");
        let r = resolution(&DIP_2);
        assert!((r.limit - 20.58).abs() < 0.01 && (r.signal - 28.0).abs() < 1.0, "{r:?}");
    }

    #[test]
    fn a_curve_that_only_falls_or_never_falls_is_not() {
        let falling: Vec<f64> = (0..33).map(|i| (1.0 - i as f64 / 24.0).max(0.0)).collect();
        let r = resolution(&falling);
        assert!(!r.dip && r.limit == r.signal && (r.limit - 20.5).abs() < 1.0, "{r:?}");
        // Noise that wobbles a little after the crossing is not a rise of 0.15.
        let wobble = [1.0, 0.9, 0.5, 0.2, 0.13, 0.16, 0.1, 0.12, 0.05];
        assert!(!resolution(&wobble).dip);
        // Never below 0.143, or too short to say.
        assert!(!resolution(&[1.0, 0.9, 0.8, 0.5, 0.4, 0.35]).dip);
        assert!(!resolution(&[]).dip);
        // A dip at 0.5 or above, or before shell 2, is not a zero.
        assert!(!resolution(&[1.0, 0.9, 0.8, 0.6, 0.55, 0.8, 0.9, 0.7, 0.3, 0.1]).dip);
        assert!(!resolution(&[1.0, 0.2, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1, 0.05]).dip);
    }
}

#[cfg(test)]
mod look_grid_tests {
    use super::*;

    #[test]
    fn the_look_tomogram_is_binned_to_the_voxel_nearest_the_templates() {
        // The real case: look 8.68 Å, card 13.02 Å (raw 4.34 Å ×3): ×2.
        assert_eq!(nearest_bin(8.68, 13.02), 2);
        assert_eq!(nearest_bin(8.68, 17.36), 2);
        assert_eq!(nearest_bin(8.68, 8.68), 1);
        assert_eq!(nearest_bin(8.68, 4.0), 1);
        assert_eq!(nearest_bin(8.68, 12.0), 1);
        let (bin, half, voxel) = look_search(8.68, 13.02);
        assert_eq!((bin, half), (2, 2));
        assert!((voxel - 34.72).abs() < 1e-9);
        // A 17.36 Å card: the same as the raw search's 2 × 17.36.
        let (bin, half, voxel) = look_search(8.68, 17.36);
        assert_eq!((bin, half), (2, 2));
        assert!((voxel - 34.72).abs() < 1e-9);
        // The look voxel equal to the card's: halved, as the raw tomogram is.
        assert_eq!(look_search(17.5, 17.5), (1, 2, 35.0));
    }

    /// Centroid of the cube's excess over its minimum, in voxels.
    fn centroid(v: &[f32], n: usize) -> [f64; 3] {
        let (mut w, mut c) = (0.0, [0.0; 3]);
        for i in 0..v.len() {
            let x = v[i] as f64;
            w += x;
            c[0] += x * (i % n) as f64;
            c[1] += x * (i / n % n) as f64;
            c[2] += x * (i / n / n) as f64;
        }
        c.map(|s| s / w)
    }

    #[test]
    fn a_rescaled_cube_keeps_its_centre_and_scales_offsets_by_the_voxel_ratio() {
        let n = 32;
        // A blob 5 voxels right of, 3 below and 2 behind the centre (16), at
        // another place on each axis, so a swapped axis shows.
        let at = [21.0, 13.0, 18.0];
        let cube: Vec<f32> = (0..n * n * n)
            .map(|i| {
                let p = [(i % n) as f64, (i / n % n) as f64, (i / n / n) as f64];
                let d: f64 = (0..3).map(|k| (p[k] - at[k]).powi(2)).sum();
                (-d / (2.0 * 2.0 * 2.0)).exp() as f32
            })
            .collect();
        for (to, n_out) in [(2.0, 16usize), (1.5, 16), (4.0 / 3.0, 16), (2.0, 32), (1.0, 32)] {
            let out = rescale_cube(&cube, n, 1.0, n_out, to);
            let c = centroid(&out, n_out);
            for k in 0..3 {
                let want = (n_out / 2) as f64 + (at[k] - 16.0) / to;
                assert!((c[k] - want).abs() < 0.06, "ratio {to} axis {k}: {} not {want}", c[k]);
            }
        }
        // The same voxel: the same cube.
        let same = rescale_cube(&cube, n, 1.0, n, 1.0);
        assert!(same.iter().zip(&cube).all(|(a, b)| (a - b).abs() < 1e-5));
    }
}
