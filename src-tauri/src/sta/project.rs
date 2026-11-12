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
//! Results go to `<out>/`: `average.mrc` (weighted by the half agreement,
//! sharpened, cut at the resolution), `average_raw.mrc`, `half1.mrc`, `half2.mrc`,
//! `particles.csv` (tomogram, position in tomogram voxels, RELION-style
//! rot/tilt/psi, score, half, and what the blob looked like) and `sta.json`.

use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::pick::{blobs, Blob};
use super::refine::{align, align_on, classes_on, first_model, Class, Live, Progress, Set};
use super::{cc, check_cancel, Pose};
use crate::spa::geom::{angles, axis_angle, grid, mul, Mat};
use crate::spa::fourier::resample3;
use crate::spa::Rng;
use crate::tomo::{output, pipeline};

/// Voxel size the tomograms are searched at, Å.
const SEARCH_VOXEL: f64 = 16.0;
/// Blob diameters looked for, Å.
pub(super) const LADDER: [f64; 7] = [90.0, 120.0, 160.0, 210.0, 280.0, 370.0, 490.0];
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
pub(super) const BOX: usize = 32;
pub(super) const SPAN: f64 = 13.0;
/// Particles in the first model, and most particles averaged here.
const FIRST: usize = 200;
pub(super) const MOST: usize = 3000;

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
    /// The B factor (Å², negative: sharpening) applied to `average.mrc`; none
    /// when the shells were too few to fit one and the map was only weighted
    /// and low-passed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sharpen_b: Option<f64>,
    /// `resolution_a` is limited at a zero of the contrast transfer.
    pub ctf_limited: bool,
    pub fsc: Vec<f64>,
    /// What the average was made from, when there was a choice: "tilt
    /// images" or "tomogram"; and the FSC of the one not kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub averaged_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fsc_other: Option<Vec<f64>>,
    /// With a tilt-image average: whether the refinement of the views (their
    /// image shifts and the tomograms' defocus, `tilt::views`) made the
    /// halves agree better and was kept. None when it was not tried.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub views_refined: Option<bool>,
    /// On a drawn surface: the pattern that was found repeating in its
    /// layers and aligned (`flat::lattice`). None where there was none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<Pattern>,
    pub map: Option<String>,
    pub halves: Option<[String; 2]>,
    pub table: Option<String>,
    pub ms: u128,
}

/// A pattern repeating in the layers of a surface, as measured.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pattern {
    /// How far apart its rows are, Å.
    pub rows_a: f64,
    /// How far inside the drawn surface it lies, Å (negative: outside).
    pub inside_a: f64,
    /// How many times round it repeats itself about the axes its places
    /// are on (1: it does not).
    pub fold: usize,
    /// Whether the average was made with every place at each of those
    /// turns (`refined`).
    pub symmetric: bool,
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
/// See `resolution`.
const FILLED: f64 = 0.3;

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
            // A dip that stays well above the line is no limit: particles at
            // different defocus fill each other's zeros (0.36 at the bottom
            // with seven tomograms, where two of one defocus gave 0.07 to
            // 0.28). Just above the line it still is: the same data falls
            // through it in the next run.
            if fsc[m] >= FILLED {
                return Shells { limit: first, signal: first, dip: false };
            }
            Shells { limit: if crossed { first } else { m as f64 }, signal, dip: true }
        }
    }
}

/// A split has to explain this much of the scores' spread before the low
/// group is taken for other things than the particle and left out.
const DISTINCT: f64 = 0.75;

pub(super) struct Cut {
    pub(super) source: usize,
    /// Centre in voxels of the source tomogram (unbinned).
    pub(super) at: [f64; 3],
    /// Source voxels per working voxel.
    bin: usize,
    score: f32,
    /// The response of the blob detector itself (0 for drawn places).
    strength: f32,
    /// The darkest voxel within the particle, in robust standard
    /// deviations below the tomogram's median: gold and other very dense
    /// things stand far out.
    darkest: f32,
    pub(super) cube: Vec<f32>,
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
pub(super) fn cube(v: &[f32], dims: [usize; 3], c: [usize; 3]) -> Option<Vec<f32>> {
    cube_n(v, dims, c, BOX)
}

/// As `cube`, of side `n`.
fn cube_n(v: &[f32], dims: [usize; 3], c: [usize; 3], n: usize) -> Option<Vec<f32>> {
    // Across the beam the cube has to be whole: the borders of a tomogram
    // hold reconstruction artefacts, not particles.
    if c[2] >= dims[2] || (0..2).any(|i| c[i] < n / 2 || c[i] + n / 2 > dims[i]) {
        return None;
    }
    cube_around(v, dims, c, n, 2)
}

/// The cube of side `n` around voxel `c` (inside the tomogram), zero mean
/// and unit variance, filled with the mean wherever it reaches past the
/// tomogram; at least one `least`-th of it has to be inside. For the
/// surroundings of a thing (`by_surroundings`): the thing itself is whole
/// in its own cube, and what is around it may end at a border.
pub(super) fn cube_around(v: &[f32], dims: [usize; 3], c: [usize; 3], n: usize, least: usize) -> Option<Vec<f32>> {
    let h = (n / 2) as isize;
    if (0..3).any(|i| c[i] >= dims[i]) {
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
    if count * least < out.len() {
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

/// One tomogram searched for blobs of the ladder sizes at `SEARCH_VOXEL`:
/// the blobs (diameters reported on the ladder exactly, whatever the voxel
/// size) and the binned tomogram they were found in.
struct Searched {
    blobs: Vec<Blob>,
    v: Vec<f32>,
    dims: [usize; 3],
    voxel: f64,
    bin: usize,
}

fn search_one(path: &str) -> Result<Searched> {
    let (v, dims, voxel, bin) = binned(path, SEARCH_VOXEL)?;
    let sizes: Vec<f64> = LADDER.iter().map(|d| d / voxel).collect();
    let mut found = blobs(&v, dims, &sizes, true, MIN_Z)?;
    for b in &mut found {
        let k = sizes.iter().position(|d| (*d as f32 - b.diameter).abs() < 1e-3).unwrap_or(0);
        b.diameter = (LADDER[k] / SEARCH_VOXEL) as f32;
    }
    Ok(Searched { blobs: found, v, dims, voxel, bin })
}

/// Step 1 of `run`: every tomogram searched for blobs of the ladder sizes
/// at `SEARCH_VOXEL`, and the sizes found.
fn find_kinds(sources: &[Source], progress: Progress) -> Result<Vec<Kind>> {
    let mut all: Vec<Blob> = vec![];
    let mut profiles: Vec<Profile> = LADDER.iter().map(|d| Profile::new(*d)).collect();
    for (i, s) in sources.iter().enumerate() {
        progress("找颗粒", i, sources.len());
        let Searched { blobs: found, v, dims, voxel, .. } = search_one(&s.path)?;
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
        sharpen_b: None,
        ctf_limited: false,
        fsc: vec![],
        averaged_from: None,
        fsc_other: None,
        views_refined: None,
        pattern: None,
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
    output::write_mrc(&file("average_raw.mrc"), &out.map, shape, voxel_a, false)?;
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
    // What is reported: the agreement of the halves inside the particle's
    // sphere, less what the sphere itself adds to it (the same halves with
    // their phases scrambled beyond where they agree well still agree a
    // little under one mask).
    let radius = (0.5 * size / voxel_a + 2.0).min(n as f64 / 2.0 - 3.0) + 2.0;
    let masked = |v: &[f32]| {
        let mut v = v.to_vec();
        crate::spa::volume::mask_sphere(&mut v, n, radius, 6.0);
        v
    };
    let fsc = crate::spa::post::corrected_fsc(&out.halves[0], &out.halves[1], &masked, n);
    let shells = resolution(&fsc);
    report.resolution_a = (shells.limit > 0.0).then(|| n as f64 * voxel_a / shells.limit);
    report.signal_a = (shells.dip && shells.signal > 0.0).then(|| n as f64 * voxel_a / shells.signal);
    report.ctf_limited = shells.dip;
    // The map to look at: weighted by how well the halves agree, sharpened
    // and cut at the resolution. The raw one stays next to it.
    let cut = if shells.dip { shells.signal } else { shells.limit };
    let (polished, sharpen_b) = polish(&out.halves[0], &out.halves[1], &fsc, cut, n, voxel_a, radius);
    output::write_mrc(&file("average.mrc"), &polished, shape, voxel_a, false)?;
    report.sharpen_b = sharpen_b;
    report.fsc = fsc;
    report.map = Some(file("average.mrc"));
    report.halves = Some([file("half1.mrc"), file("half2.mrc")]);
    report.table = Some(file("particles.csv"));
    report.ms = started.elapsed().as_millis();
    std::fs::write(file("sta.json"), serde_json::to_string_pretty(&report)?)?;
    Ok(())
}

/// Guinier fit of the sharpening: shells coarser than this (Å) are left out,
/// because at low frequency the amplitudes follow the particle's shape and
/// not the fall-off of the data.
const SHARPEN_COARSE_A: f64 = 60.0;
/// Fewest shells the fit needs. Our maps are 15 to 30 Å, where the usual fit
/// (10 Å and finer) does not exist; a few shells between 60 Å and the
/// resolution limit are all there is, and fewer than this say nothing.
const SHARPEN_MIN_SHELLS: usize = 5;
/// Limits of the B factor, Å². Only sharpening (never blurring), and not more
/// than this: at low resolution the fit reads the mask and the shape as much
/// as the fall-off, and a large boost would only amplify noise.
const SHARPEN_B_MIN: f64 = -600.0;

/// The average to present, from the unfiltered half maps: their mean inside
/// the soft sphere, each shell weighted by sqrt(2·FSC/(1+FSC)) (FSC floored at
/// 0, the corrected one), sharpened with the B factor of a Guinier fit over
/// the shells from `SHARPEN_COARSE_A` to `cut` (the resolution limit in
/// shells), low-passed softly at `cut`, and masked again. Returns the map and
/// the B used (none when the fit was not possible and the map was only
/// weighted and low-passed).
fn polish(h1: &[f32], h2: &[f32], fsc: &[f64], cut: f64, n: usize, voxel_a: f64, radius: f64) -> (Vec<f32>, Option<f64>) {
    use crate::spa::fourier;
    let mut mean: Vec<f32> = h1.iter().zip(h2).map(|(a, b)| 0.5 * (a + b)).collect();
    if cut <= 0.0 || fsc.is_empty() {
        return (mean, None);
    }
    // The fall of the amplitudes is measured on the particle alone (masked);
    // the map shown is the whole box, filtered.
    let whole = mean.clone();
    crate::spa::volume::mask_sphere(&mut mean, n, radius, 6.0);
    let (re, im) = fourier::ft3(n, &mean);
    let shell_of = |z: usize, y: usize, x: usize| {
        let (fx, fy, fz) = (fourier::freq(x, n), fourier::freq(y, n), fourier::freq(z, n));
        ((fx * fx + fy * fy + fz * fz) as f64).sqrt()
    };
    // Power per shell of the mean and of the noise in it (half the difference
    // of the halves): the noise floor would otherwise flatten the fall-off
    // and the fit would find too little blur.
    let shells = n / 2 + 1;
    let power = |re: &[f32], im: &[f32]| {
        let (mut p, mut c) = (vec![0f64; shells], vec![0f64; shells]);
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let r = shell_of(z, y, x).round() as usize;
                    if r < shells {
                        let j = (z * n + y) * n + x;
                        p[r] += (re[j] * re[j] + im[j] * im[j]) as f64;
                        c[r] += 1.0;
                    }
                }
            }
        }
        (p, c)
    };
    let (pm, cnt) = power(&re, &im);
    let diff = masked_diff(h1, h2, n, radius);
    let (dr, di) = fourier::ft3(n, &diff);
    let (pn, _) = power(&dr, &di);
    let amp: Vec<f64> = (0..shells).map(|r| (pm[r] - pn[r]).max(0.0).sqrt()).collect();
    let width = n as f64 * voxel_a;
    let r_lo = ((width / SHARPEN_COARSE_A).ceil() as usize).max(1);
    let r_hi = (cut.floor() as usize).min(shells - 1);
    let pts: Vec<(f64, f64)> =
        (r_lo..=r_hi).filter(|&r| amp[r] > 0.0).map(|r| ((r as f64 / width).powi(2), (amp[r] / cnt[r].sqrt()).ln())).collect();
    let b = (pts.len() >= SHARPEN_MIN_SHELLS).then(|| {
        let m = pts.len() as f64;
        let (mx, my) = (pts.iter().map(|p| p.0).sum::<f64>() / m, pts.iter().map(|p| p.1).sum::<f64>() / m);
        let num: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        let den: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
        // ln|F| = c + slope·s², and the amplitudes fall as exp(−B/4·s²) for a
        // blur of B, so the sharpening B of the convention is 4·slope (< 0).
        if den > 0.0 { (4.0 * num / den).clamp(SHARPEN_B_MIN, 0.0) } else { 0.0 }
    });
    let (mut re, mut im) = fourier::ft3(n, &whole);
    re.par_chunks_mut(n * n).zip(im.par_chunks_mut(n * n)).enumerate().for_each(|(z, (pr, pi))| {
        for y in 0..n {
            for x in 0..n {
                let s = shell_of(z, y, x);
                let f = fsc.get(s.round() as usize).copied().unwrap_or(0.0).max(0.0);
                let w = (2.0 * f / (1.0 + f)).sqrt();
                let boost = b.map_or(1.0, |b| (-b / 4.0 * (s / width).powi(2)).exp());
                let g = (w * boost * crate::spa::data::soft(s, cut, 2.0) as f64) as f32;
                pr[y * n + x] *= g;
                pi[y * n + x] *= g;
            }
        }
    });
    let mut out = fourier::ift3(n, &re, &im);
    // The map shown keeps what surrounds the particle: weighted and cut at
    // the resolution it carries little noise there, and a mask close about
    // the particle cuts through the bright ring the transfer leaves around
    // it, which shows as an edge. Only the corners of the box are taken
    // away, softly, from the level the map has there.
    let c = n as f64 / 2.0;
    let (wide, edge) = ((c - 10.0).max(radius.min(c - 4.0)), 8.0);
    let (mut sum, mut count) = (0f64, 0usize);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let d = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2) + (z as f64 - c).powi(2)).sqrt();
                if d >= wide && d < wide + edge {
                    sum += out[(z * n + y) * n + x] as f64;
                    count += 1;
                }
            }
        }
    }
    let level = (sum / count.max(1) as f64) as f32;
    out.iter_mut().for_each(|v| *v -= level);
    crate::spa::volume::mask_sphere(&mut out, n, wide, edge);
    (out, b)
}

/// Half the difference of the halves inside the sphere: the noise of their mean.
fn masked_diff(h1: &[f32], h2: &[f32], n: usize, radius: f64) -> Vec<f32> {
    let mut d: Vec<f32> = h1.iter().zip(h2).map(|(a, b)| 0.5 * (a - b)).collect();
    crate::spa::volume::mask_sphere(&mut d, n, radius, 6.0);
    d
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
    /// particle (see `is_likely`). Information only: `choose` goes by `Trial`s.
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
///   `λ2 / λ3` large for a layer. When the piece at the centre (above half
///   the maximum) ends inside that reach, it is no line and no layer: the
///   value is then the smaller of that piece's own and the one above, and
///   stays below `ELONGATION`.
/// * `blobs`: the number of 6-connected pieces of the density above half its
///   maximum that hold at least `BLOB_SHARE` of that part. Where the piece
///   lies does not matter here.
/// * `offset`: with exactly one piece, the distance of its centroid from the
///   centre in particle radii (`radius / 1.2`); otherwise 0.
pub fn shape(avg: &[f32], n: usize, radius: f64) -> (f64, f64, usize, f64) {
    let (compact, elongation, blobs, offset, _, _) = measure(avg, n, radius);
    (compact, elongation, blobs, offset)
}

/// What `shape`'s elongation is made of: that of all the density, and,
/// when a piece stands at the centre, that piece's own and whether it runs
/// on to the edge of the reach.
#[cfg(test)]
pub(super) fn elongations(avg: &[f32], n: usize, radius: f64) -> (f64, Option<(f64, bool)>) {
    measure(avg, n, radius).5
}

/// The class average moved so that the centroid of its single piece (when
/// there is exactly one and it is within `OFF_CENTRE` of the centre) sits at
/// the cube centre `n / 2`; unchanged otherwise. A centred template makes
/// the peaks of the later search sit on particle centres.
pub fn centred(avg: Vec<f32>, n: usize, radius: f64) -> Vec<f32> {
    let (_, _, blobs, offset, at, _) = measure(&avg, n, radius);
    if blobs != 1 || offset >= OFF_CENTRE || offset == 0.0 {
        return avg;
    }
    let c = (n / 2) as f64;
    // transform: out(p) = vol(p + t), so t = centroid - centre brings it to the centre.
    cc::transform(&avg, n, &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], [at[0] - c, at[1] - c, at[2] - c])
}

#[allow(clippy::type_complexity)]
fn measure(avg: &[f32], n: usize, radius: f64) -> (f64, f64, usize, f64, [f64; 3], (f64, Option<(f64, bool)>)) {
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
        return (0.0, 0.0, 0, 0.0, [c; 3], (0.0, None));
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
        return (compact, 0.0, 0, 0.0, [c; 3], (0.0, None));
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
    let mut elongation = if l[0] <= 0.0 { 0.0 } else { (l[0] / l[2].max(1e-9 * l[0])).sqrt() };
    let mut parts = (elongation, None);

    // What stands at the centre: the piece above half the maximum nearest
    // it, followed as far as it goes. Where it ends before the moments'
    // reach does, it is a thing of its own (particles packed side by side
    // are no layer): the elongation is the smaller of its own and that of
    // all the density, and below `ELONGATION`. Where it runs on to the
    // edge, the elongation of all the density stays.
    let top = dens.iter().cloned().fold(0.0, f64::max);
    let off = |i: usize| ((i % n) as f64 - c).powi(2) + ((i / n % n) as f64 - c).powi(2) + ((i / n / n) as f64 - c).powi(2);
    let nearest = (0..dens.len()).filter(|i| dens[*i] >= 0.5 * top && off(*i) <= 0.25 * radius * radius).min_by(|a, b| off(*a).total_cmp(&off(*b)));
    if let Some(start) = nearest {
        let mut seen = vec![false; dens.len()];
        seen[start] = true;
        let mut stack = vec![start];
        let mut piece = vec![];
        let mut runs_on = false;
        while let Some(i) = stack.pop() {
            piece.push(i);
            runs_on |= off(i).sqrt() >= wide - 1.5;
            let p = [i % n, i / n % n, i / n / n];
            for (axis, step) in [(0usize, 1usize), (1, n), (2, n * n)] {
                for up in [false, true] {
                    if if up { p[axis] + 1 >= n } else { p[axis] == 0 } {
                        continue;
                    }
                    let j = if up { i + step } else { i - step };
                    if dens[j] >= 0.5 * top && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        {
            let m: f64 = piece.iter().map(|i| dens[*i]).sum();
            let mut centre = [0.0f64; 3];
            for &i in &piece {
                let p = [(i % n) as f64, (i / n % n) as f64, (i / n / n) as f64];
                for k in 0..3 {
                    centre[k] += dens[i] * p[k] / m;
                }
            }
            let mut cov = [[0.0f64; 3]; 3];
            for &i in &piece {
                let p = [(i % n) as f64 - centre[0], (i / n % n) as f64 - centre[1], (i / n / n) as f64 - centre[2]];
                for r in 0..3 {
                    for q in 0..3 {
                        cov[r][q] += dens[i] * p[r] * p[q] / m;
                    }
                }
            }
            let l = eigenvalues(cov);
            // What ends is no line and no layer, however it is shaped and
            // whatever stands beside it.
            let own = if l[0] > 0.0 { (l[0] / l[2].max(1e-9 * l[0])).sqrt() } else { elongation };
            parts.1 = Some((own, runs_on));
            if !runs_on {
                elongation = elongation.min(own).min(ELONGATION - 0.01);
            }
        }
    }

    // Pieces above half the maximum.
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
    (compact, elongation, big.len(), offset, at, parts)
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
pub(super) fn is_likely(compact: f64, elongation: f64, blobs: usize, offset: f64) -> bool {
    compact > COMPACT && elongation < ELONGATION && blobs == 1 && offset < OFF_CENTRE
}

const COMPACT: f64 = 7.5;
/// Most `elongation` of a particle: the missing wedge stretches a ball along
/// Z by up to about 1.3 to 1.5.
pub const ELONGATION: f64 = 1.6;
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

/// Sorts the cubes of `set` (cut at `BOX`³) into `k` classes, reference-free,
/// at half sampling (the classification of `cards`).
fn coarse_classes(dev: Option<&burn::tensor::Device>, set: &Set, sources: &[Source], k: usize, progress: Progress, live: Live) -> Result<Vec<Class>> {
    let half = BOX / 2;
    let small: Vec<Vec<f32>> = set.particles.par_iter().map(|p| resample3(p, BOX, half)).collect();
    let small_wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(half, s.tilts[0], s.tilts[1])).collect();
    let coarse = Set { n: half, particles: &small, wedges: &small_wedges, wedge_of: set.wedge_of, radius: set.radius / 2.0 };
    classes_on(dev, &coarse, k, 1, progress, live)
}

/// The card (and its `BOX`³ average, particle centred) of one class found by
/// `coarse_classes` of `set`; `size` is the ladder size the cubes were cut for.
fn class_card(set: &Set, class: &Class, size: f64, measured_a: f64, voxel_a: f64) -> (Card, Vec<f32>) {
    // The poses at full sampling: shifts double.
    let mut poses = vec![Pose { r: super::IDENTITY, t: [0.0; 3], score: 0.0 }; set.particles.len()];
    let mut member = vec![false; set.particles.len()];
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
    (
        Card {
            id: 0,
            diameter_a: size,
            measured_a,
            voxel_a,
            members: class.members.len(),
            share: class.members.len() as f64 / set.particles.len() as f64,
            path: String::new(),
            compact,
            elongation,
            blobs,
            offset,
            likely,
        },
        average,
    )
}

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
    cards_on(None, sources, out_dir, progress, live)
}

/// `cards`, with the classification's global searches on `dev` when one is
/// given (the same scores as on the CPU, `cc_gpu`).
pub fn cards_on(dev: Option<&burn::tensor::Device>, sources: &[Source], out_dir: &str, progress: Progress, live: Live) -> Result<Vec<Card>> {
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
        let (mut cuts, voxel_a) = kind_cuts(sources, size, progress)?;
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
        let kind_progress = |s: &str, a: usize, b: usize| progress(&format!("{s} {size} Å"), a, b);
        for class in coarse_classes(dev, &set, sources, 3, &kind_progress, live)? {
            if class.members.len() < CARD_MEMBERS {
                continue;
            }
            found.push(class_card(&set, &class, size, kind.measured_a, voxel_a));
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

/// The blobs of ladder size `size` (Å) in every source, as cubes at the
/// working voxel size (returned), one tomogram in memory at a time.
fn kind_cuts(sources: &[Source], size: f64, progress: Progress) -> Result<(Vec<Cut>, f64)> {
    use super::search::allowed;
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
    Ok((cuts, voxel_a))
}

/// The card for a few clicks (fewer than `MIN_POINTS`): too few cubes to
/// average by themselves, but enough to say which kind is meant. The blobs
/// of the clicked size in all the tomograms are sorted into classes as
/// `cards` does, and the class most of the clicked particles fell into is
/// the card: an average of many particles, chosen by the user's examples.
/// `place`: the clicks (source, raw voxels) after the snap to their blobs.
#[allow(clippy::too_many_arguments)]
fn card_of_examples(sources: &[Source], place: &[(usize, [f64; 3])], size: f64, measured_a: f64, progress: Progress, live: Live) -> Result<(Card, Vec<f32>)> {
    let (mut cuts, voxel_a) = kind_cuts(sources, size, progress)?;
    without_gold(&mut cuts, size, voxel_a);
    // A blob is a clicked one when a click lies within half a particle of it.
    let clicked = |c: &Cut| {
        let reach = 0.5 * size / (voxel_a / c.bin as f64);
        place.iter().any(|(s, p)| *s == c.source && (0..3).map(|k| (p[k] - c.at[k]).powi(2)).sum::<f64>().sqrt() <= reach)
    };
    // The clicked ones stay whatever their score.
    cuts.sort_by(|a, b| clicked(b).cmp(&clicked(a)).then(b.score.total_cmp(&a.score)));
    cuts.truncate(FIRST);
    let marked: Vec<bool> = cuts.iter().map(clicked).collect();
    if !marked.iter().any(|m| *m) {
        bail!("点的位置上没有找到颗粒，请点在颗粒中心，或再多点几个");
    }
    if cuts.len() < CARD_MEMBERS {
        bail!("这种大小的东西在断层图里只找到 {} 个，太少了；请多点几个（至少 {MIN_POINTS} 个）", cuts.len());
    }
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let particles: Vec<Vec<f32>> = cuts.iter().map(|c| c.cube.clone()).collect();
    let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius: 0.8 * size / voxel_a };
    let kind_progress = |s: &str, a: usize, b: usize| progress(&format!("{s} {size} Å"), a, b);
    let classes = coarse_classes(None, &set, sources, 3, &kind_progress, live)?;
    let hits = |c: &Class| c.members.iter().filter(|i| marked[**i]).count();
    let class = classes.iter().max_by_key(|c| (hits(c), c.members.len())).context("没有对齐出平均")?;
    if hits(class) == 0 || class.members.len() < CARD_MEMBERS {
        bail!("和你点的同类的只找到 {} 个，太少了；请多点几个（至少 {MIN_POINTS} 个）", class.members.len());
    }
    Ok(class_card(&set, class, size, measured_a, voxel_a))
}

/// Fewest particles `card_from_points` averages by themselves. `classes`
/// (the routine the automatic cards align with) refuses fewer than 4 cubes
/// (`Set::check`), and 4 is also about the least for which the average
/// of cubes of unknown orientation is more than one noisy cube; more clicks
/// give a better card (10 or more is better). Fewer clicks (2 or 3 are
/// asked for, 1 is taken) are examples of a kind instead: `card_of_examples`.
pub const MIN_POINTS: usize = 4;

/// A card made from particles the user clicked on (tool "点"): the clicks are
/// positions in voxels of the raw tomogram `Source::path` (`Drawn::pos`'s
/// frame), `(source, (x, y, z))`; their orientations are unknown.
///
/// * Size. The blob finder of `cards` runs on the tomograms with clicks; each
///   click takes the nearest blob within one blob diameter (of its own size).
///   The particle's size is the median ladder size of those blobs, and
///   `measured_a` is measured on their density as `kinds` does. Clicks with no
///   blob nearby do not vote.
/// * Snap. A click with a blob nearby moves to that blob's centre (users click
///   roughly); the others stay where they were clicked.
/// * Cubes are cut as `cards` cuts them for that size (binned so the size
///   spans `SPAN` voxels of the `BOX` cube, zero mean, unit variance), and
///   aligned reference-free into ONE class (`coarse_classes`, `class_card`),
///   which is centred and measured as an automatic card is.
///
/// Written as `<out_dir>/card.mrc` and `<out_dir>/card.json`, apart from
/// the automatic `cards/`. The card's id is 0 and means nothing outside
/// `out_dir`: a marked card never goes into a list of automatic cards, so
/// ids cannot collide; `run_template` takes the card by its cube and size.
pub fn card_from_points(sources: &[Source], points: &[(usize, [f64; 3])], out_dir: &str, progress: Progress, live: Live) -> Result<Card> {
    if sources.is_empty() {
        bail!("没有断层图");
    }
    if points.is_empty() {
        bail!("请先在颗粒上点 2 到 3 个作例子");
    }
    if let Some((s, _)) = points.iter().find(|(s, _)| *s >= sources.len()) {
        bail!("点所在的断层图（第 {} 张）不在要平均的断层图里", s + 1);
    }
    // Where each click goes (raw voxels), and the blob it voted with.
    let mut place: Vec<[f64; 3]> = points.iter().map(|p| p.1).collect();
    let mut votes: Vec<usize> = vec![];
    let mut profiles: Vec<Profile> = LADDER.iter().map(|d| Profile::new(*d)).collect();
    let tomograms: Vec<usize> = (0..sources.len()).filter(|i| points.iter().any(|p| p.0 == *i)).collect();
    for (n, &i) in tomograms.iter().enumerate() {
        progress("找颗粒", n, tomograms.len());
        let Searched { blobs: found, v, dims, voxel, bin } = search_one(&sources[i].path)?;
        let half = (bin as f64 - 1.0) / 2.0;
        for (k, (_, raw)) in points.iter().enumerate().filter(|(_, p)| p.0 == i) {
            // The click in voxels of the search volume.
            let at = raw.map(|x| (x - half) / bin as f64);
            let near = found
                .iter()
                .map(|b| ((0..3).map(|a| (b.pos[a] as f64 - at[a]).powi(2)).sum::<f64>().sqrt(), b))
                .filter(|(d, b)| *d <= b.diameter as f64)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, b)) = near {
                // `search_one` reports diameters on the ladder exactly.
                let ladder = LADDER.iter().position(|d| (d / SEARCH_VOXEL - b.diameter as f64).abs() < 1e-3).unwrap_or(0);
                votes.push(ladder);
                profiles[ladder].add(&v, dims, voxel, b.pos);
                place[k] = b.pos.map(|x| x.round() as f64 * bin as f64 + half);
            }
        }
    }
    progress("找颗粒", tomograms.len(), tomograms.len());
    if votes.is_empty() {
        bail!("点的位置上没有找到颗粒，请点在颗粒中心");
    }
    votes.sort_unstable();
    let ladder = votes[votes.len() / 2];
    let size = LADDER[ladder];
    let measured_a = profiles[ladder].diameter().unwrap_or(size);
    let write = |mut card: Card, average: Vec<f32>| -> Result<Card> {
        std::fs::create_dir_all(out_dir).with_context(|| format!("无法创建 {out_dir}"))?;
        card.path = Path::new(out_dir).join("card.mrc").to_string_lossy().into_owned();
        output::write_mrc(&card.path, &average, [BOX; 3], card.voxel_a, false)?;
        std::fs::write(Path::new(out_dir).join("card.json"), serde_json::to_string_pretty(&card)?)?;
        Ok(card)
    };
    if points.len() < MIN_POINTS {
        let snapped: Vec<(usize, [f64; 3])> = points.iter().zip(&place).map(|(p, at)| (p.0, *at)).collect();
        let (card, average) = card_of_examples(sources, &snapped, size, measured_a, progress, live)?;
        return write(card, average);
    }

    // Cubes at the working voxel size, one tomogram in memory at a time.
    let mut particles: Vec<Vec<f32>> = vec![];
    let mut wedge_of: Vec<usize> = vec![];
    let mut voxel_a = size / SPAN;
    for (n, &i) in tomograms.iter().enumerate() {
        progress(&format!("裁出颗粒 {size} Å"), n, tomograms.len());
        let (v, dims, voxel, bin) = binned(&sources[i].path, size / SPAN)?;
        voxel_a = voxel;
        for (k, _) in points.iter().enumerate().filter(|(_, p)| p.0 == i) {
            // Source voxel p lies in working voxel p / bin (as `run_drawn`).
            let c = place[k].map(|p| (p / bin as f64).floor().max(0.0) as usize);
            if let Some(cube) = cube(&v, dims, c) {
                particles.push(cube);
                wedge_of.push(i);
            }
        }
    }
    progress(&format!("裁出颗粒 {size} Å"), tomograms.len(), tomograms.len());
    if particles.len() < MIN_POINTS {
        bail!("点的位置里能用的只有 {} 个（太靠近断层图边缘的不能用），至少要 {MIN_POINTS} 个", particles.len());
    }
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius: 0.8 * size / voxel_a };
    let kind_progress = |s: &str, a: usize, b: usize| progress(&format!("{s} {size} Å"), a, b);
    let class = coarse_classes(None, &set, sources, 1, &kind_progress, live)?.into_iter().next().context("没有对齐出平均")?;
    let (card, average) = class_card(&set, &class, size, measured_a, voxel_a);
    write(card, average)
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

/// Two cards show the same particle when their best masked normalised
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

/// `same_kind` between the cards `which` (indices into `cards`), as a matrix
/// indexed like `cards` (1 on the diagonal; the pairs that involve another
/// card are not computed and are 0). Reads each of those cards' file; they are
/// usually two or three, a pair takes about a second.
pub fn likeness(cards: &[Card], which: &[usize], found: Option<(&str, &[Source])>) -> Result<Vec<Vec<f64>>> {
    let cubes: Vec<Vec<f32>> = which.iter().map(|i| read_card(&cards[*i].path).map(|c| c.0)).collect::<Result<_>>()?;
    // Where each card's trial found its particles (`trials` writes them under
    // `found.0`), and the tomograms' voxel, Å.
    let places: Vec<Vec<(String, [f64; 3])>> = which.iter().map(|i| found.map_or(vec![], |(dir, _)| places_of(&Path::new(dir).join(format!("card_{}", cards[*i].id)).join("particles.csv")))).collect();
    let px = found.and_then(|(_, s)| s.first()).and_then(|s| pixel_of(&s.path).ok()).unwrap_or(0.0);
    let mut m = vec![vec![0.0; cards.len()]; cards.len()];
    for (k, &i) in which.iter().enumerate() {
        m[i][i] = 1.0;
        for (l, &j) in which.iter().enumerate().skip(k + 1) {
            m[i][j] = same_kind(CardCube::of(&cards[i], &cubes[k]), CardCube::of(&cards[j], &cubes[l]));
            // Two cards that find their particles in the same places show
            // the same particle, however unlike they look (one the whole of
            // it, the other a part).
            if px > 0.0 && together(&places[k], &places[l], TOGETHER_REACH * cards[i].diameter_a.min(cards[j].diameter_a) / px) >= TOGETHER {
                m[i][j] = 1.0;
            }
            m[j][i] = m[i][j];
        }
    }
    Ok(m)
}

/// The share of one card's places that must be another's too, and how near
/// (in sizes of the smaller card) counts as the same place. By chance a
/// tenth coincide where particles stand close.
const TOGETHER: f64 = 0.5;
const TOGETHER_REACH: f64 = 0.4;

/// (tomogram, place in its voxels) per row of a `particles.csv`; empty when
/// it cannot be read.
fn places_of(csv: &Path) -> Vec<(String, [f64; 3])> {
    let Ok(text) = std::fs::read_to_string(csv) else { return vec![] };
    text.lines()
        .skip(1)
        .filter_map(|line| {
            // The tomogram's path may hold commas: the numbers are the last eleven fields.
            let f: Vec<&str> = line.rsplitn(12, ',').collect();
            if f.len() < 12 {
                return None;
            }
            let n = |k: usize| f[k].trim().parse::<f64>().ok();
            Some((f[11].to_string(), [n(10)?, n(9)?, n(8)?]))
        })
        .collect()
}

/// The share of the smaller set's places that have one of the other set
/// within `reach` voxels in the same tomogram.
fn together(a: &[(String, [f64; 3])], b: &[(String, [f64; 3])], reach: f64) -> f64 {
    let (few, many) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if few.is_empty() {
        return 0.0;
    }
    let near = |p: &(String, [f64; 3])| many.iter().any(|q| q.0 == p.0 && (0..3).map(|k| (q.1[k] - p.1[k]).powi(2)).sum::<f64>() <= reach * reach);
    few.iter().filter(|p| near(p)).count() as f64 / few.len() as f64
}

/// The most cards tried as a template (the first ones in card order).
pub const TRIALS: usize = 12;
/// A card is as good as the best one when its `strength` is at least this
/// share of the best's.
pub const CLOSE: f64 = 0.75;

/// The size `run_template` is asked for with a card: the particle's diameter,
/// 1.33 × the size the card was made for (the blobs are a little smaller than
/// the particle they sit in).
pub const DIAMETER_FACTOR: f64 = 1.33;

pub fn template_diameter(card: &Card) -> f64 {
    DIAMETER_FACTOR * card.diameter_a
}

/// What one card gave as a template: the coarse average of what the search
/// found with it (`run_template` without the fine pass). A card that is a
/// layer, a line or a small blob finds no group of particles that stands out
/// (`ok` false, `reason` says why); a particle gives an average whose halves
/// agree far out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trial {
    /// The card's id.
    pub card: usize,
    pub ok: bool,
    /// Why it failed.
    pub reason: Option<String>,
    pub particles: usize,
    pub candidates: usize,
    /// FSC between the halves of that average, per shell (0 to the Nyquist).
    pub fsc: Vec<f64>,
    /// The voxel size of that average, Å.
    pub voxel_a: f64,
    /// `strength` of the `fsc`.
    pub strength: f64,
    pub seconds: f64,
    /// `shape`'s elongation of that average: about 1.4 for a particle, 1.8
    /// and more for what runs out of the box (a line, a layer, an edge).
    #[serde(default)]
    pub elongation: Option<f64>,
}

impl Trial {
    /// Whether the average is of something that runs on beyond the box.
    /// Such things average well (they are stiff and there is much of them)
    /// and are averaged on purpose, drawn by the user; left to itself the
    /// program takes a particle.
    pub fn runs_on(&self) -> bool {
        self.elongation.is_some_and(|e| e >= ELONGATION)
    }
}

/// How far out and how well the two halves of an average agree: the area
/// under the FSC over the spatial frequency, 1/Å (the shells are `1 / (n ·
/// voxel_a)` apart, `n` the box size, twice the last shell). Negative values
/// count as 0, shell 0 (always 1) is left out. Comparable between cards of
/// different voxel sizes, which a resolution read at one threshold is not
/// when the curve ends at the Nyquist limit.
pub fn strength(fsc: &[f64], voxel_a: f64) -> f64 {
    if fsc.len() < 2 || voxel_a <= 0.0 {
        return 0.0;
    }
    let n = 2.0 * (fsc.len() - 1) as f64;
    fsc.iter().skip(1).map(|v| v.max(0.0)).sum::<f64>() / (n * voxel_a)
}

/// Every card (the first `TRIALS`, in card order) as the template of a coarse
/// run, written to `<out_dir>/card_<id>/`. Judging a card by what it makes
/// avoids guessing from its look. A run that fails (no particles stand out, too few
/// found) is a failed trial, not an error; only a cancellation is. `each` is
/// told of every trial as it finishes.
pub fn trials(
    dev: &burn::tensor::Device,
    sources: &[Source],
    cards: &[Card],
    out_dir: &str,
    progress: Progress,
    live: Live,
    each: &(dyn Fn(&Trial) + Sync),
) -> Result<Vec<Trial>> {
    let total = cards.len().min(TRIALS);
    let mut out = vec![];
    for (k, card) in cards.iter().take(total).enumerate() {
        check_cancel()?;
        let started = Instant::now();
        let label = format!("试跑卡片 {}/{total}", k + 1);
        let (cube, voxel) = read_card(&card.path)?;
        let dir = Path::new(out_dir).join(format!("card_{}", card.id)).to_string_lossy().into_owned();
        let inner = |_: &str, a: usize, b: usize| progress(&label, a, b);
        progress(&label, 0, 1);
        let r = run_template(dev, &sources[..sources.len().min(TRIAL_SOURCES)], &cube, voxel, template_diameter(card), Pass::Trial, &dir, &inner, live);
        let seconds = started.elapsed().as_secs_f64();
        let trial = match r {
            Ok(r) => {
                let voxel_a = if r.voxel_a > 0.0 { r.voxel_a } else { voxel };
                let elongation = r.map.as_ref().and_then(|m| read_card(m).ok()).map(|(avg, v)| {
                    let n = (avg.len() as f64).cbrt().round() as usize;
                    shape(&avg, n, 0.5 * template_diameter(card) / v).1
                });
                Trial { card: card.id, ok: true, reason: None, particles: r.particles, candidates: r.candidates, strength: strength(&r.fsc, voxel_a), fsc: r.fsc, voxel_a, seconds, elongation }
            }
            // A cancelled run says so with the same kind of error: not a verdict on the card.
            Err(e) if super::CANCEL.load(std::sync::atomic::Ordering::Relaxed) => return Err(e),
            Err(e) => Trial { card: card.id, ok: false, reason: Some(format!("{e:#}")), particles: 0, candidates: 0, fsc: vec![], voxel_a: voxel, strength: 0.0, seconds, elongation: None },
        };
        each(&trial);
        out.push(trial);
    }
    Ok(out)
}

/// The cards (ids, ascending) whose trial worked and gave at least `CLOSE`
/// of the best strength.
pub fn good(trials: &[Trial]) -> Vec<usize> {
    // Among the particles when there is one; else among all that worked.
    let particle = trials.iter().any(|t| t.ok && !t.runs_on());
    let counts = |t: &&Trial| t.ok && (!particle || !t.runs_on());
    let best = trials.iter().filter(counts).map(|t| t.strength).fold(0.0, f64::max);
    let mut ids: Vec<usize> = trials.iter().filter(counts).filter(|t| t.strength >= CLOSE * best).map(|t| t.card).collect();
    ids.sort_unstable();
    ids
}

/// What `choose` took.
#[derive(Clone, Debug, PartialEq)]
pub struct Chosen {
    /// The card to use.
    pub id: usize,
    /// The good cards of its kind (it among them), ascending.
    pub same: Vec<usize>,
    /// The good cards of other kinds, ascending.
    pub other: Vec<usize>,
}

/// The card to take without asking: of the `good` ones (see `trials`) the
/// one with the highest strength, when they all show one kind of particle
/// (connected groups of cards whose pairwise `similar[i][j]`, see `likeness`,
/// is `SAME` or more; only the good cards need entries). With several kinds
/// it is the kind of the strongest card, and `other` names the rest. None
/// only when no trial worked. Ties: the lower id.
pub fn choose(cards: &[Card], trials: &[Trial], similar: &[Vec<f64>]) -> Option<Chosen> {
    let strong: std::collections::HashMap<usize, f64> = trials.iter().filter(|t| t.ok).map(|t| (t.card, t.strength)).collect();
    let good: Vec<usize> = good(trials).into_iter().filter(|i| *i < cards.len()).collect();
    if good.is_empty() {
        return None;
    }
    let alike = |i: usize, j: usize| similar.get(i).and_then(|r| r.get(j)).is_some_and(|s| *s >= SAME);
    // Kinds: the first good card of each group claims the cards linked to it.
    let mut kinds: Vec<Vec<usize>> = vec![];
    let mut seen = vec![false; cards.len()];
    for &s in &good {
        if seen[s] {
            continue;
        }
        seen[s] = true;
        let mut group = vec![s];
        let mut at = 0;
        while at < group.len() {
            let i = group[at];
            at += 1;
            for &j in &good {
                if !seen[j] && (alike(i, j) || alike(j, i)) {
                    seen[j] = true;
                    group.push(j);
                }
            }
        }
        group.sort_unstable();
        kinds.push(group);
    }
    // Several kinds that all average well (a particle, a filament beside
    // it, something smaller): the one whose halves agree best is taken and
    // the others are named, to be chosen instead. How often each was found
    // does not tell them apart: every search fills up to its limit.
    let best = |kind: &Vec<usize>| kind.iter().map(|i| strong[i]).fold(0.0, f64::max);
    kinds.sort_by(|a, b| best(b).total_cmp(&best(a)).then(a[0].cmp(&b[0])));
    let same = kinds.remove(0);
    let id = same.iter().copied().max_by(|&i, &j| strong[&i].total_cmp(&strong[&j]).then(j.cmp(&i)))?;
    let mut other: Vec<usize> = kinds.into_iter().flatten().collect();
    other.sort_unstable();
    Some(Chosen { id, same, other })
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
/// `given`: each of `drawn` is one particle the user marked with its axis
/// (`marks::Kind::Stand`): there is no layer to find and none is left out
/// for matching worse than the rest, and a few are enough.
pub fn run_drawn(sources: &[Source], drawn: &[Drawn], both: bool, given: bool, size_a: f64, out_dir: &str, progress: Progress, live: Live) -> Result<Report> {
    let started = Instant::now();
    let mut size_a = size_a;
    let mut cuts: Vec<Cut> = vec![];
    let mut axes: Vec<[f64; 3]> = vec![];
    let mut voxel_a = size_a / SPAN;
    // On a surface the places are first put on the layer that is really
    // there, and a pattern that repeats in it is aligned for itself.
    let mut on_layer: Vec<Drawn> = vec![];
    let mut lattice = None;
    if !both && !given {
        match on_surfaces(sources, drawn, size_a, true, progress, live)? {
            Surfaces::Lattice(l) => lattice = Some(l),
            Surfaces::Plain(d) => on_layer = d,
        }
    }
    let mut start: Option<(Vec<f32>, Vec<Pose>)> = None;
    let mut apart = None;
    let mut detail = None;
    let mut pattern: Option<Pattern> = None;
    let drawn: &[Drawn] = if both || given || lattice.is_some() { drawn } else { &on_layer };
    if let Some(l) = lattice {
        cuts = l.cuts;
        size_a = l.size_a;
        voxel_a = l.voxel_a;
        apart = Some(l.apart);
        detail = Some((l.band, l.slide));
        pattern = Some(l.pattern);
        start = Some((l.model, l.poses));
    } else {
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
    }
    progress("裁出颗粒", sources.len(), sources.len());
    if given && cuts.len() < FEWEST_CIRCLED {
        bail!("只取到 {} 个立着的东西（太靠近图边缘的切不出完整的方块），至少要 {FEWEST_CIRCLED} 个", cuts.len());
    }
    if !given && cuts.len() < 20 {
        bail!("画出的范围里只取到 {} 个位置，太少了", cuts.len());
    }
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let radius = 0.8 * size_a / voxel_a;
    let particles: Vec<Vec<f32>> = cuts.iter().map(|c| c.cube.clone()).collect();
    let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    let (model, poses) = match start {
        Some(start) => start,
        None if given => super::refine::first_model_given(&set, &axes, 1, progress, live)?,
        None => super::refine::first_model_known(&set, &axes, both, 1, progress, live)?,
    };
    // Places closer than this are one particle.
    let apart = apart.unwrap_or(0.5 * size_a / voxel_a);

    // Seeds that moved onto the same particle: the best of each stays.
    let mut keep: Vec<usize> = vec![];
    for s in 0..sources.len() {
        let of: Vec<usize> = (0..cuts.len()).filter(|i| cuts[*i].source == s).collect();
        let at: Vec<[f64; 3]> = of.iter().map(|i| std::array::from_fn(|k| cuts[*i].at[k] / cuts[*i].bin as f64 + poses[*i].t[k])).collect();
        let scores: Vec<f32> = of.iter().map(|i| poses[*i].score).collect();
        keep.extend(super::shape::merge(&at, &scores, apart).into_iter().map(|k| of[k]));
    }
    // Stretches of the shape that carry no particle match clearly worse.
    let scores: Vec<f32> = keep.iter().map(|i| poses[*i].score).collect();
    let (threshold, distinct) = split(&scores);
    if !given && distinct >= DISTINCT && scores.iter().filter(|s| **s >= threshold).count() >= 4 {
        keep.retain(|i| poses[*i].score >= threshold);
    }
    keep.sort_unstable();
    let candidates = cuts.len();
    // `align` takes even and odd places as the two halves. Places cut along
    // one shape overlap their neighbours, so a half must take whole shapes
    // (or whole pieces of one), or the halves share data and their FSC
    // stays high to Nyquist.
    let at: Vec<[f64; 3]> = keep.iter().map(|i| std::array::from_fn(|k| cuts[*i].at[k] / cuts[*i].bin as f64 + poses[*i].t[k])).collect();
    let of: Vec<usize> = keep.iter().map(|i| cuts[*i].source).collect();
    let keep: Vec<usize> = halves_by_groups(&at, &of, 1.5 * 0.5 * size_a / voxel_a).into_iter().map(|j| keep[j]).collect();
    let poses: Vec<Pose> = keep.iter().map(|i| poses[*i]).collect();
    let particles: Vec<Vec<f32>> = keep.iter().map(|i| particles[*i].clone()).collect();
    let wedge_of: Vec<usize> = keep.iter().map(|i| wedge_of[*i]).collect();
    let mut taken: Vec<Option<Cut>> = cuts.into_iter().map(Some).collect();
    let cuts: Vec<Cut> = keep.iter().map(|i| taken[*i].take().expect("each place once")).collect();
    if cuts.len() < 4 {
        bail!("合并重复位置后只剩 {} 个颗粒", cuts.len());
    }
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    let out = match detail {
        // The pattern was found at this detail: the first round must see it
        // too and may not slide a place onto the next row.
        Some((band, slide)) => super::refine::align_from(&set, &model, &poses, band, slide, progress, live)?,
        None => align(&set, &model, &poses, progress, live)?,
    };
    // A pattern is found at a sampling that only just shows it; what its
    // places look like is averaged from finer voxels where there are any.
    let (mut cuts, mut out) = (cuts, out);
    if detail.is_some() {
        let mut radius = radius;
        for _ in 0..FINE_LEVELS {
            let Some((fine, average, voxel)) = finer(sources, &cuts, &out, radius, voxel_a, progress, live)? else { break };
            radius = (radius * voxel_a / voxel).min(FINE_BOX as f64 / 2.0 - 3.0);
            (cuts, out, voxel_a) = (fine, average, voxel);
            // There the places are aligned again: the poses had only seen
            // the coarse sampling.
            let Some((better, with_turns)) = refined(sources, &cuts, &out, radius, pattern.map_or(1, |p| p.fold), progress, live)? else { break };
            out = better;
            if let Some(p) = &mut pattern {
                p.symmetric = with_turns;
            }
            // Finer still only when the halves agree nearly as far as this
            // sampling holds.
            if out.shell < NEAR_FULL * FINE_BOX as f64 / 2.0 {
                break;
            }
        }
    }
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
        sharpen_b: None,
        ctf_limited: false,
        fsc: vec![],
        averaged_from: None,
        fsc_other: None,
        views_refined: None,
        pattern,
        map: None,
        halves: None,
        table: None,
        ms: 0,
    };
    save(&mut report, sources, &cuts, &out, out_dir, size_a, voxel_a, started)?;
    Ok(report)
}

/// One row of a saved `particles.csv`.
struct Row {
    source: usize,
    /// Centre, voxels of the source tomogram.
    at: [f64; 3],
    r: crate::spa::geom::Mat,
    score: f32,
    /// 1 or 2.
    half: usize,
    blob: f32,
    strength: f32,
    darkest: f32,
}

/// A saved average as the steps after it need it: its particles, the size
/// (Å), the voxel (Å) and the side of its map, and its resolution (Å).
struct Saved {
    rows: Vec<Row>,
    size_a: f64,
    voxel_a: f64,
    n: usize,
    resolution_a: Option<f64>,
}

fn saved(sources: &[Source], dir: &str) -> Result<Saved> {
    let dir = Path::new(dir);
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("sta.json")).with_context(|| format!("{} 里没有平均结果", dir.display()))?)?;
    let same = |a: &str, b: &str| a == b || std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(x, y)| x == y);
    let text = std::fs::read_to_string(dir.join("particles.csv")).context("读不到颗粒表")?;
    let mut rows = vec![];
    for line in text.lines().skip(1) {
        // The tomogram's path may hold commas: the numbers are the last eleven fields.
        let f: Vec<&str> = line.rsplitn(12, ',').collect();
        if f.len() < 12 {
            continue;
        }
        let n = |k: usize| f[k].trim().parse::<f64>().with_context(|| format!("颗粒表里有读不懂的一行：{line}"));
        let Some(source) = sources.iter().position(|s| same(&s.path, f[11])) else { bail!("平均里用到的断层图不在这次选的里面：{}", f[11]) };
        rows.push(Row {
            source,
            at: [n(10)?, n(9)?, n(8)?],
            r: crate::spa::geom::euler(n(7)?, n(6)?, n(5)?),
            score: n(4)? as f32,
            half: if n(3)? as usize == 2 { 2 } else { 1 },
            blob: n(2)? as f32,
            strength: n(1)? as f32,
            darkest: n(0)? as f32,
        });
    }
    let number = |key: &str| report[key].as_f64();
    let fsc = report["fsc"].as_array().map_or(0, |a| a.len());
    if fsc < 3 {
        bail!("平均结果不完整");
    }
    Ok(Saved { rows, size_a: number("diameterA").context("平均结果不完整")?, voxel_a: number("voxelA").context("平均结果不完整")?, n: 2 * (fsc - 1), resolution_a: number("resolutionA") })
}

/// Where a row lies on a tomogram's grid of `bin`: the voxel its cube is
/// cut about and what is left to that voxel's centre.
pub(super) fn on_grid(at: [f64; 3], bin: usize) -> ([usize; 3], [f64; 3]) {
    let half = (bin as f64 - 1.0) / 2.0;
    let q = at.map(|x| (x - half) / bin as f64);
    let c = q.map(|x| x.round().max(0.0) as usize);
    (c, std::array::from_fn(|k| q[k] - c[k] as f64))
}

/// The kinds among the particles of the saved average in `dir`
/// (`kinds::kinds`): each kind's average is written to
/// `<dir>/kinds/kind_<k>.mrc` and the whole to `<dir>/kinds/kinds.json`
/// (`kinds`: id from 1, count, map, rows of `particles.csv` from 0; empty
/// when the particles are all one kind), which is also returned.
pub fn kinds_of(sources: &[Source], dir: &str, progress: Progress) -> Result<serde_json::Value> {
    let saved = saved(sources, dir)?;
    // Compared on cubes of side `BOX`, however fine the average is.
    let want = saved.voxel_a * (saved.n as f64 / BOX as f64).max(1.0);
    let mut particles: Vec<Vec<f32>> = vec![];
    let mut poses: Vec<Pose> = vec![];
    let mut row_of: Vec<usize> = vec![];
    let mut voxel = want;
    for s in 0..sources.len() {
        if !saved.rows.iter().any(|r| r.source == s) {
            continue;
        }
        check_cancel()?;
        progress("裁出颗粒", s, sources.len());
        let (v, dims, got, bin) = binned(&sources[s].path, want)?;
        voxel = got;
        for (i, row) in saved.rows.iter().enumerate().filter(|(_, r)| r.source == s) {
            let (c, t) = on_grid(row.at, bin);
            if let Some(cube) = cube(&v, dims, c) {
                particles.push(cube);
                poses.push(Pose { r: row.r, t, score: row.score });
                row_of.push(i);
            }
        }
    }
    progress("裁出颗粒", sources.len(), sources.len());
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let wedge_of: Vec<usize> = row_of.iter().map(|i| saved.rows[*i].source).collect();
    let radius = (0.5 * saved.size_a / voxel + 2.0).min(BOX as f64 / 2.0 - 3.0);
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    let shell = saved.resolution_a.map_or(super::kinds::BAND, |r| BOX as f64 * voxel / r);
    let found = super::kinds::kinds(&set, &poses, shell, progress)?;
    let out = Path::new(dir).join("kinds");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).with_context(|| format!("无法创建 {}", out.display()))?;
    let mut list = vec![];
    for (k, kind) in found.iter().flat_map(|f| f.kinds.iter()).enumerate() {
        let map = out.join(format!("kind_{}.mrc", k + 1)).to_string_lossy().into_owned();
        output::write_mrc(&map, &kind.map, [BOX; 3], voxel, false)?;
        let rows: Vec<usize> = kind.members.iter().map(|i| row_of[*i]).collect();
        list.push(serde_json::json!({ "id": k + 1, "count": rows.len(), "map": map, "rows": rows }));
    }
    let value = serde_json::json!({ "kinds": list, "apart": found.as_ref().map(|f| f.apart), "particles": row_of.len(), "voxelA": voxel });
    std::fs::write(out.join("kinds.json"), serde_json::to_vec_pretty(&value)?)?;
    Ok(value)
}

/// The average of the particles of the kinds `keep` (ids of
/// `kinds_of`) alone, at the poses they have, from the tomograms at the
/// saved average's own voxel: written like any average to
/// `<dir>/kinds/kept`. The two halves stay the halves they were (the
/// larger loses its extra particles).
pub fn keep_kinds(sources: &[Source], dir: &str, keep: &[usize], progress: Progress, live: Live) -> Result<Report> {
    use super::average::Sum;
    let started = Instant::now();
    let saved = saved(sources, dir)?;
    let listed: serde_json::Value = serde_json::from_slice(&std::fs::read(Path::new(dir).join("kinds").join("kinds.json")).context("还没有分过种类")?)?;
    let mut rows: Vec<usize> = listed["kinds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|k| k["id"].as_u64().is_some_and(|id| keep.contains(&(id as usize))))
        .flat_map(|k| k["rows"].as_array().into_iter().flatten().filter_map(|r| r.as_u64().map(|r| r as usize)))
        .filter(|r| *r < saved.rows.len())
        .collect();
    rows.sort_unstable();
    rows.dedup();
    // Even places one half, odd the other, as everywhere.
    let of_half = |h: usize| rows.iter().copied().filter(|r| saved.rows[*r].half == h).collect::<Vec<_>>();
    let (first, second) = (of_half(1), of_half(2));
    let order: Vec<usize> = first.iter().zip(&second).flat_map(|(a, b)| [*a, *b]).collect();
    if order.len() < 4 {
        bail!("选中的种类里颗粒太少（{} 个）", rows.len());
    }
    let n = saved.n;
    let empty = || [Sum::new(n), Sum::new(n)];
    let mut sums = empty();
    let mut cuts: Vec<Option<Cut>> = (0..order.len()).map(|_| None).collect();
    let mut poses = vec![Pose { r: saved.rows[0].r, t: [0.0; 3], score: 0.0 }; order.len()];
    let mut voxel = saved.voxel_a;
    let mut done = 0;
    for s in 0..sources.len() {
        let of: Vec<usize> = (0..order.len()).filter(|k| saved.rows[order[*k]].source == s).collect();
        if of.is_empty() {
            continue;
        }
        check_cancel()?;
        let (v, dims, got, bin) = binned(&sources[s].path, saved.voxel_a)?;
        voxel = got;
        let wedge = cc::wedge(n, sources[s].tilts[0], sources[s].tilts[1]);
        let half = (bin as f64 - 1.0) / 2.0;
        for k in &of {
            let row = &saved.rows[order[*k]];
            let (c, t) = on_grid(row.at, bin);
            poses[*k] = Pose { r: row.r, t, score: row.score };
            cuts[*k] = Some(Cut { source: s, at: c.map(|x| x as f64 * bin as f64 + half), bin, score: row.blob, strength: row.strength, darkest: row.darkest, cube: vec![] });
        }
        for chunk in of.chunks(32) {
            check_cancel()?;
            let part = chunk
                .par_iter()
                .fold(empty, |mut sum, &k| {
                    let (c, _) = on_grid(saved.rows[order[k]].at, bin);
                    if let Some(cube) = cube_n(&v, dims, c, n) {
                        sum[k % 2].add(&cube, &wedge, &poses[k]);
                    }
                    sum
                })
                .reduce(empty, |mut a, b| {
                    a[0].merge(&b[0]);
                    a[1].merge(&b[1]);
                    a
                });
            sums[0].merge(&part[0]);
            sums[1].merge(&part[1]);
            done += chunk.len();
            progress("平均选中的种类", done, order.len());
        }
    }
    let halves = [sums[0].finish(), sums[1].finish()];
    let radius = (0.5 * saved.size_a / voxel + 2.0).min(n as f64 / 2.0 - 3.0);
    let fsc = crate::spa::post::fsc(&halves[0], &halves[1], n, radius + 2.0);
    let shell = super::refine::cut(&fsc);
    let [mut both, other] = sums;
    both.merge(&other);
    let map = both.finish();
    live(&map);
    let cuts: Vec<Cut> = cuts.into_iter().map(|c| c.expect("every place has its tomogram")).collect();
    let out = super::refine::Aligned { map, halves, poses, fsc, shell };
    let mut report = Report {
        kinds: vec![],
        diameter_a: None,
        particles: 0,
        candidates: saved.rows.len(),
        distinct: None,
        voxel_a: 0.0,
        searched: None,
        search_voxel_a: None,
        resolution_a: None,
        signal_a: None,
        sharpen_b: None,
        ctf_limited: false,
        fsc: vec![],
        averaged_from: None,
        fsc_other: None,
        views_refined: None,
        pattern: None,
        map: None,
        halves: None,
        table: None,
        ms: 0,
    };
    let kept = Path::new(dir).join("kinds").join("kept");
    save(&mut report, sources, &cuts, &out, &kept.to_string_lossy(), saved.size_a, voxel, started)?;
    Ok(report)
}

/// Side of the cubes of `finer`.
const FINE_BOX: usize = 64;
/// At most this many times is the sampling halved after a pattern was
/// aligned.
const FINE_LEVELS: usize = 3;
/// The sampling is halved again when the halves agree to this part of the
/// way to its limit.
const NEAR_FULL: f64 = 0.75;
/// At most this many places have their finer cubes held at once for
/// `refined` (a cube is a megabyte).
const FINE_HOLD: usize = 4000;

/// The places of `finer` aligned on their finer cubes (`cuts` and `out`
/// as it returns them; `radius` in those voxels): each half against its
/// own average, a few degrees and voxels about where it stands
/// (`refine::align_fine`), down to the detail the halves agree to.
///
/// `fold` is how many times round the pattern repeats itself about the
/// axis the places are on (`flat::axis`; the model's Z). The average is
/// then also made with every place at each of those turns, and that one is
/// kept when its halves agree at least as far: turns the pattern really
/// has add data to every voxel, turns it does not have blur what differs
/// and the halves agree less. Whether it was kept is returned.
///
/// None when the cubes are too many to hold or one cannot be cut.
fn refined(sources: &[Source], cuts: &[Cut], out: &super::refine::Aligned, radius: f64, fold: usize, progress: Progress, live: Live) -> Result<Option<(super::refine::Aligned, bool)>> {
    use super::average::Sum;
    let n = FINE_BOX;
    if cuts.len() > FINE_HOLD {
        return Ok(None);
    }
    let mut particles: Vec<Vec<f32>> = vec![vec![]; cuts.len()];
    for s in 0..sources.len() {
        let of: Vec<usize> = (0..cuts.len()).filter(|i| cuts[*i].source == s).collect();
        let Some(&first) = of.first() else { continue };
        check_cancel()?;
        let bin = cuts[first].bin;
        let img = pipeline::open(&sources[s].path)?;
        let (v, dims, _, got) = binned(&sources[s].path, output::pixel_a(&img) * bin as f64)?;
        if got != bin {
            return Ok(None);
        }
        let half = (bin as f64 - 1.0) / 2.0;
        let cubes: Vec<Option<Vec<f32>>> = of.par_iter().map(|&i| cube_n(&v, dims, cuts[i].at.map(|x| ((x - half) / bin as f64).round().max(0.0) as usize), n)).collect();
        for (i, cube) in of.into_iter().zip(cubes) {
            let Some(cube) = cube else { return Ok(None) };
            particles[i] = cube;
        }
    }
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(n, s.tilts[0], s.tilts[1])).collect();
    let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
    let set = Set { n, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    let aligned = super::refine::align_fine(&set, &out.map, &out.poses, out.shell + 1.0, 2, progress, live)?;
    if fold < 2 {
        return Ok(Some((aligned, false)));
    }
    progress("按对称性平均", 0, fold);
    let mut sums = [Sum::new(n), Sum::new(n)];
    for k in 0..fold {
        check_cancel()?;
        // The model is the same turned by S, so a place at R is as well at R·S.
        let turn = super::cc_axis(360.0 * k as f64 / fold as f64);
        let turned: Vec<Pose> = aligned.poses.iter().map(|p| Pose { r: mul(&p.r, &turn), ..*p }).collect();
        for (half, sum) in sums.iter_mut().enumerate() {
            sum.merge(&set.average(&turned, |i| i % 2 == half));
        }
        progress("按对称性平均", k + 1, fold);
    }
    let halves = [sums[0].finish(), sums[1].finish()];
    let fsc = crate::spa::post::fsc(&halves[0], &halves[1], n, radius + 2.0);
    let shell = super::refine::cut(&fsc);
    if shell < aligned.shell {
        return Ok(Some((aligned, false)));
    }
    let [mut both, other] = sums;
    both.merge(&other);
    let map = both.finish();
    live(&map);
    Ok(Some((super::refine::Aligned { map, halves, poses: aligned.poses, fsc, shell }, true)))
}
/// The most voxels of a tomogram `finer` loads.
const FINE_MOST: usize = 600_000_000;

/// The average of aligned places from cubes of half the voxel they were
/// aligned on (`voxel_a`; `radius` in those voxels), at the poses found,
/// where every tomogram has voxels that fine. The cubes are cut, added and
/// dropped a few at a time. The poses never saw detail beyond the coarse
/// sampling, so what the halves share beyond it is in the data.
/// Returns the places on the finer grid, the average and its voxel (Å);
/// None when there is nothing finer.
fn finer(sources: &[Source], cuts: &[Cut], out: &super::refine::Aligned, radius: f64, voxel_a: f64, progress: Progress, live: Live) -> Result<Option<(Vec<Cut>, super::refine::Aligned, f64)>> {
    use super::average::Sum;
    let n = FINE_BOX;
    let m = cuts.len();
    // The finer grid of each tomogram, as `binned` makes it.
    let mut grids: Vec<Option<(usize, [usize; 3])>> = vec![None; sources.len()];
    for c in cuts {
        if grids[c.source].is_none() {
            let img = pipeline::open(&sources[c.source].path)?;
            let info = img.info();
            let bin = ((0.5 * voxel_a / output::pixel_a(&img)).round() as usize).max(1);
            let dims = [info.width / bin, info.height / bin, info.size_z / bin];
            if bin >= c.bin || dims.iter().product::<usize>() > FINE_MOST {
                return Ok(None);
            }
            grids[c.source] = Some((bin, dims));
        }
    }
    // Where each place is on its finer grid, and whether a cube fits there.
    let place = |i: usize| -> ([usize; 3], [f64; 3], bool) {
        let c = &cuts[i];
        let (bin, dims) = grids[c.source].expect("every source of a cut");
        let half = (bin as f64 - 1.0) / 2.0;
        let q: [f64; 3] = std::array::from_fn(|k| (c.at[k] + out.poses[i].t[k] * c.bin as f64 - half) / bin as f64);
        let at = q.map(|x| x.round().max(0.0) as usize);
        let whole = (0..2).all(|k| at[k] >= n / 2 && at[k] + n / 2 <= dims[k]) && {
            let (lo, hi) = (at[2] as isize - (n / 2) as isize, at[2] as isize + (n / 2) as isize);
            2 * (hi.min(dims[2] as isize) - lo.max(0)).max(0) as usize >= n
        };
        (at, std::array::from_fn(|k| q[k] - at[k] as f64), whole)
    };
    let places: Vec<([usize; 3], [f64; 3], bool)> = (0..m).map(place).collect();
    // Even and odd places are the halves: a place that does not fit takes
    // the other of its pair with it, so the rest stay in their halves.
    let kept: Vec<usize> = (0..m).filter(|i| places[*i].2 && (i ^ 1 >= m || places[i ^ 1].2)).collect();
    if kept.len() < 4 {
        return Ok(None);
    }
    let order: std::collections::HashMap<usize, usize> = kept.iter().enumerate().map(|(k, i)| (*i, k)).collect();
    let empty = || [Sum::new(n), Sum::new(n)];
    let mut sums = empty();
    let mut fine_voxel = 0.5 * voxel_a;
    let mut scale = 2.0;
    let mut done = 0;
    for s in 0..sources.len() {
        let of: Vec<usize> = kept.iter().copied().filter(|i| cuts[*i].source == s).collect();
        if of.is_empty() {
            continue;
        }
        check_cancel()?;
        let (v, dims, voxel, bin) = binned(&sources[s].path, 0.5 * voxel_a)?;
        fine_voxel = voxel;
        scale = cuts[of[0]].bin as f64 / bin as f64;
        let wedge = cc::wedge(n, sources[s].tilts[0], sources[s].tilts[1]);
        for chunk in of.chunks(32) {
            check_cancel()?;
            let part = chunk
                .par_iter()
                .fold(empty, |mut sum, &i| {
                    if let Some(cube) = cube_n(&v, dims, places[i].0, n) {
                        sum[order[&i] % 2].add(&cube, &wedge, &Pose { r: out.poses[i].r, t: places[i].1, score: out.poses[i].score });
                    }
                    sum
                })
                .reduce(empty, |mut a, b| {
                    a[0].merge(&b[0]);
                    a[1].merge(&b[1]);
                    a
                });
            sums[0].merge(&part[0]);
            sums[1].merge(&part[1]);
            done += chunk.len();
            progress("精细平均", done, kept.len());
        }
    }
    let halves = [sums[0].finish(), sums[1].finish()];
    let fsc = crate::spa::post::fsc(&halves[0], &halves[1], n, (radius * scale + 2.0).min(n as f64 / 2.0 - 1.0));
    let shell = super::refine::cut(&fsc);
    let [mut both, other] = sums;
    both.merge(&other);
    let map = both.finish();
    live(&map);
    let fine: Vec<Cut> = kept
        .iter()
        .map(|&i| {
            let c = &cuts[i];
            let (bin, _) = grids[c.source].expect("every source of a cut");
            let half = (bin as f64 - 1.0) / 2.0;
            Cut { source: c.source, at: places[i].0.map(|x| x as f64 * bin as f64 + half), bin, score: c.score, strength: c.strength, darkest: c.darkest, cube: vec![] }
        })
        .collect();
    let poses = kept.iter().map(|&i| Pose { r: out.poses[i].r, t: places[i].1, score: out.poses[i].score }).collect();
    Ok(Some((fine, super::refine::Aligned { map, halves, poses, fsc, shell }, fine_voxel)))
}

/// A profile across a membrane is averaged over a disc of this radius, Å.
const SNAP_DISC_A: f64 = 60.0;
/// A drawn surface may be this far off the layer it stands for, Å.
const SNAP_RANGE_A: f64 = 100.0;

/// What `on_surfaces` found on the drawn surfaces.
enum Surfaces {
    /// A pattern that repeats in the layer: the places on it, aligned.
    Lattice(OnLattice),
    /// No such pattern: the drawn places, moved onto the real layer.
    Plain(Vec<Drawn>),
}

struct OnLattice {
    /// One cube per place of the pattern, cut straight from the tomogram.
    cuts: Vec<Cut>,
    poses: Vec<Pose>,
    /// The size the cubes were cut for, Å: the one asked for, or the one
    /// that shows the pattern when that did not (`pattern_elsewhere`).
    size_a: f64,
    /// Their average at those poses.
    model: Vec<f32>,
    voxel_a: f64,
    /// Places closer than this (voxels) are the same one.
    apart: f64,
    /// The detail the pattern needs (cycles across the box) and how far a
    /// place may still slide (voxels) without leaving its own.
    band: f64,
    slide: usize,
    pattern: Pattern,
}

/// One tomogram at the working sampling.
struct Held {
    which: usize,
    v: Vec<f32>,
    dims: [usize; 3],
    voxel: f64,
    bin: usize,
    median: f32,
    sd: f32,
}

fn hold<'a>(held: &'a mut Option<Held>, sources: &[Source], which: usize, voxel_a: f64) -> Result<&'a Held> {
    if held.as_ref().is_none_or(|h| h.which != which) {
        *held = None;
        let (v, dims, voxel, bin) = binned(&sources[which].path, voxel_a)?;
        let (median, sd) = super::pick::robust(&v);
        *held = Some(Held { which, v, dims, voxel, bin, median, sd });
    }
    Ok(held.as_ref().expect("just loaded"))
}

/// The drawn places of surfaces (`drawn`, their normals known), put on the
/// layer that is really there (`flat::snap`), and, when a pattern repeats
/// in the layers (`flat::lattice`), the places of that pattern: unbent
/// cubes centred on its depth are aligned in the layer (how the pattern
/// lies in each is first read from the cube, `flat::rows`, then
/// `refine::in_layer`: turn about the normal, slide along the surface),
/// what was found is
/// carried back to the tomogram, and cubes are cut straight there.
fn on_surfaces(sources: &[Source], drawn: &[Drawn], size_a: f64, probe: bool, progress: Progress, live: Live) -> Result<Surfaces> {
    use super::flat;
    use super::shape::Seed;
    let mut held: Option<Held> = None;
    let mut seeds: Vec<Vec<Seed>> = vec![vec![]; sources.len()];
    let mut scale: Vec<(usize, f64)> = vec![(1, size_a / SPAN); sources.len()];
    let mut found: Option<flat::Lattice> = None;
    let used: Vec<usize> = (0..sources.len()).filter(|i| drawn.iter().any(|d| d.source == *i)).collect();
    for (step, &i) in used.iter().enumerate() {
        progress("找准膜面", step, used.len());
        check_cancel()?;
        let h = hold(&mut held, sources, i, size_a / SPAN)?;
        let radius = 0.8 * size_a / h.voxel;
        let half = (h.bin as f64 - 1.0) / 2.0;
        // Source voxel p is working voxel (p − half) / bin.
        let at: Vec<Seed> = drawn.iter().filter(|d| d.source == i).map(|d| Seed { pos: d.pos.map(|p| (p - half) / h.bin as f64), axis: d.axis }).collect();
        let on_layer = flat::snap(&h.v, h.dims, &at, SNAP_DISC_A / h.voxel, SNAP_RANGE_A / h.voxel, 1.5 * radius, sources[i].tilts);
        let lattice = flat::lattice(&h.v, h.dims, &flat::patches(&on_layer, 1.5 * radius), BOX, radius.ceil() as usize);
        if let Some(l) = lattice.filter(|l| found.is_none_or(|f| l.strength > f.strength)) {
            found = Some(l);
        }
        seeds[i] = on_layer;
        scale[i] = (h.bin, h.voxel);
    }
    progress("找准膜面", used.len(), used.len());
    // No pattern at this sampling, or a faint one: a pattern finer or
    // coarser than the sampling shows, and a faint ring may be the noise of
    // a few sections. The samplings beside this one are looked at as well,
    // and where one of them shows a pattern clearly better, the same again
    // there.
    if probe && found.is_none_or(|l| l.strength < SURE) {
        drop(held.take());
        if let Some((size, denser)) = pattern_elsewhere(sources, &used, &seeds, &scale, size_a, found.map(|l| l.strength))? {
            if let Surfaces::Lattice(l) = on_surfaces(sources, &denser, size, false, progress, live)? {
                return Ok(Surfaces::Lattice(l));
            }
        }
    }
    let Some(lattice) = found else {
        let on_layer = used
            .iter()
            .flat_map(|&i| {
                let (bin, _) = scale[i];
                let half = (bin as f64 - 1.0) / 2.0;
                seeds[i].iter().map(move |s| Drawn { source: i, pos: s.pos.map(|p| p * bin as f64 + half), axis: s.axis })
            })
            .collect();
        return Ok(Surfaces::Plain(on_layer));
    };

    // Unbent cubes about the pattern's depth.
    let mut cubes: Vec<Vec<f32>> = vec![];
    let mut wedges: Vec<Vec<f32>> = vec![];
    let mut origin: Vec<(usize, flat::Patch)> = vec![];
    let mut radius = 0.8 * size_a / scale[used[0]].1;
    for (step, &i) in used.iter().enumerate() {
        progress("裁出颗粒", step, used.len());
        check_cancel()?;
        let h = hold(&mut held, sources, i, size_a / SPAN)?;
        radius = 0.8 * size_a / h.voxel;
        let moved: Vec<Seed> = seeds[i].iter().map(|s| Seed { pos: std::array::from_fn(|k| s.pos[k] + lattice.depth * s.axis[k]), axis: s.axis }).collect();
        for patch in flat::patches(&moved, 1.5 * radius) {
            if let Some(cube) = flat::cut(&h.v, h.dims, &patch, BOX, radius) {
                cubes.push(flat::pattern(&cube, BOX, 2.0 * lattice.period));
                wedges.push(flat::wedge(BOX, &patch.frame, sources[i].tilts[0], sources[i].tilts[1]));
                origin.push((i, patch));
            }
        }
    }
    progress("裁出颗粒", used.len(), used.len());
    if cubes.len() < 20 {
        bail!("画出的范围里只取到 {} 个位置，太少了", cubes.len());
    }
    let band = (1.5 * BOX as f64 / lattice.period).min(BOX as f64 / 2.0 - 3.0);
    let slide = (0.5 * lattice.period).ceil() as usize + 1;
    let wedge_of: Vec<usize> = (0..cubes.len()).collect();
    let set = Set { n: BOX, particles: &cubes, wedges: &wedges, wedge_of: &wedge_of, radius };
    let start = flat::rows(&cubes, BOX, lattice.period);
    let (unbent, mut found) = super::refine::in_layer(&set, band, slide, start.as_deref(), 1, progress, live)?;
    drop((cubes, wedges));
    // A pattern that repeats itself turned: its axis is the place (see
    // `flat::axis`). The model moved by a is each cube moved by R·a.
    let axis = flat::axis(&unbent, BOX, lattice.period, radius);
    if let Some((_, a)) = axis {
        for p in &mut found {
            for j in 0..3 {
                p.t[j] += p.r[j][0] * a[0] + p.r[j][1] * a[1];
            }
        }
    }

    // Back to the tomogram: where each place is and how it stands there.
    let mut cuts: Vec<Cut> = vec![];
    let mut poses: Vec<Pose> = vec![];
    let apart = 0.5 * lattice.period;
    for (step, &i) in used.iter().enumerate() {
        progress("放回原图", step, used.len());
        check_cancel()?;
        let of: Vec<usize> = (0..origin.len()).filter(|k| origin[*k].0 == i).collect();
        let at: Vec<[f64; 3]> = of.iter().map(|k| origin[*k].1.lift(found[*k].t)).collect();
        let scores: Vec<f32> = of.iter().map(|k| found[*k].score).collect();
        let h = hold(&mut held, sources, i, size_a / SPAN)?;
        let half = (h.bin as f64 - 1.0) / 2.0;
        for j in super::shape::merge(&at, &scores, apart) {
            let (p, k) = (at[j], of[j]);
            if p.iter().any(|x| *x < 0.0) {
                continue;
            }
            let c = p.map(|x| x.round() as usize);
            let Some(cube) = cube(&h.v, h.dims, c) else { continue };
            let r = mul(&origin[k].1.frame_at(found[k].t[0], found[k].t[1]), &found[k].r);
            let darkest = darkest(&h.v, h.dims, c, 0.5 * size_a / h.voxel, h.median, h.sd);
            cuts.push(Cut { source: i, at: c.map(|x| x as f64 * h.bin as f64 + half), bin: h.bin, score: 0.0, strength: 0.0, darkest, cube });
            poses.push(Pose { r, t: std::array::from_fn(|d| p[d] - c[d] as f64), score: found[k].score });
        }
    }
    progress("放回原图", used.len(), used.len());
    if cuts.len() < 20 {
        bail!("画出的范围里只取到 {} 个位置，太少了", cuts.len());
    }
    let voxel_a = scale[used[0]].1;
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let particles: Vec<Vec<f32>> = cuts.iter().map(|c| c.cube.clone()).collect();
    let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    let model = set.average(&poses, |_| true).finish();
    live(&model);
    let pattern = Pattern { rows_a: lattice.period * voxel_a, inside_a: -lattice.depth * voxel_a, fold: axis.map_or(1, |a| a.0), symmetric: false };
    Ok(Surfaces::Lattice(OnLattice { cuts, poses, size_a, model, voxel_a, apart, band, slide: 1, pattern }))
}

/// A ring that stands out this much at the sampling the size asks for is
/// taken as it is; below, the samplings beside it are looked at too.
const SURE: f64 = 1.8;
/// To be taken instead, a ring at another sampling stands out this many
/// times further (its excess over 1 against the excess here).
const CLEARER: f64 = 1.5;
/// Samplings looked at beside the one the size asks for: this many,
/// nearest first.
const ELSEWHERE: usize = 3;
/// They lie between these, Å per voxel.
const ELSEWHERE_A: (f64, f64) = (6.0, 40.0);
/// Voxels per row of a pattern at the sampling it is then aligned at.
const ROW_VOXELS: f64 = 4.0;

/// Whether a pattern repeats in the layers of the surfaces at a sampling
/// other than the one the particle size asks for (rows closer than two and
/// a half voxels, or further apart than a third of the box, do not show).
/// `seeds` are the places on their layers and `scale` the (bin, voxel) of
/// each source, as `on_surfaces` has them; the source with the most places
/// is looked at. Gives the size (Å) whose sampling has `ROW_VOXELS` voxels
/// per row, and the same surfaces sampled for that size (`flat::respaced`).
/// `here` is how far the ring stands out at the size's own sampling, when
/// one does: only a ring `CLEARER` than it counts.
fn pattern_elsewhere(sources: &[Source], used: &[usize], seeds: &[Vec<super::shape::Seed>], scale: &[(usize, f64)], size_a: f64, here: Option<f64>) -> Result<Option<(f64, Vec<Drawn>)>> {
    use super::flat;
    use super::shape::Seed;
    let Some(&i) = used.iter().max_by_key(|i| seeds[**i].len()) else { return Ok(None) };
    let (bin, voxel) = scale[i];
    let px = voxel / bin as f64;
    // Source voxels of a source's places.
    let raw = |i: usize| -> Vec<Seed> {
        let (bin, _) = scale[i];
        let half = (bin as f64 - 1.0) / 2.0;
        seeds[i].iter().map(|s| Seed { pos: s.pos.map(|p| p * bin as f64 + half), axis: s.axis }).collect()
    };
    let mut bins: Vec<usize> = (1..=2 * bin).filter(|b| *b != bin && (ELSEWHERE_A.0..=ELSEWHERE_A.1).contains(&(px * *b as f64))).collect();
    bins.sort_by_key(|b| (b.abs_diff(bin), *b));
    // The clearest of them: (strength, rows in Å).
    let mut best: Option<(f64, f64)> = None;
    let least = here.map_or(1.0, |s| 1.0 + CLEARER * (s - 1.0));
    for b in bins.into_iter().take(ELSEWHERE) {
        check_cancel()?;
        let Ok((v, dims, voxel, b)) = binned(&sources[i].path, px * b as f64) else { continue };
        let half = (b as f64 - 1.0) / 2.0;
        let at: Vec<Seed> = raw(i).iter().map(|s| Seed { pos: s.pos.map(|p| (p - half) / b as f64), axis: s.axis }).collect();
        let radius = 0.8 * size_a / voxel;
        if let Some(l) = flat::lattice(&v, dims, &flat::patches(&at, 1.5 * radius), BOX, radius.ceil() as usize) {
            if l.strength > least && best.is_none_or(|(s, _)| l.strength > s) {
                best = Some((l.strength, l.period * voxel));
            }
        }
    }
    let Some((_, rows_a)) = best else { return Ok(None) };
    let size = SPAN * px * (rows_a / ROW_VOXELS / px).round().max(1.0);
    let mut denser: Vec<Drawn> = vec![];
    for &i in used {
        let px = scale[i].1 / scale[i].0 as f64;
        let again = flat::respaced(&raw(i), 1.5 * 0.8 * size_a / px, 0.5 * size_a / px, 0.5 * size / px);
        denser.extend(again.into_iter().map(|s| Drawn { source: i, pos: s.pos, axis: s.axis }));
    }
    let most = super::marks::MOST_SEEDS;
    if denser.len() > most {
        let n = denser.len();
        denser = (0..most).map(|k| denser[k * n / most].clone()).collect();
    }
    Ok(Some((size, denser)))
}

/// The order in which places become the two halves of `align` (even
/// places one half, odd the other): places closer than `gap` (same source)
/// are one group, a shape or a piece of one, and whole groups are dealt to
/// the half that has fewer so far, largest first. A group of more than
/// `MOST_OF_ONE` of all is cut in two by a plane through its middle across
/// its longest extent first, so one large shape does not make one half.
/// The larger half loses its extra places, so the two interleave exactly.
fn halves_by_groups(at: &[[f64; 3]], source: &[usize], gap: f64) -> Vec<usize> {
    const MOST_OF_ONE: f64 = 0.6;
    let n = at.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for i in 0..n {
        for j in i + 1..n {
            if source[i] == source[j] && (0..3).map(|k| (at[i][k] - at[j][k]).powi(2)).sum::<f64>() <= gap * gap {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = vec![];
    let mut index = std::collections::HashMap::new();
    for i in 0..n {
        let r = root(&mut parent, i);
        let g = *index.entry(r).or_insert_with(|| {
            groups.push(vec![]);
            groups.len() - 1
        });
        groups[g].push(i);
    }
    // Cut the groups too large to share out.
    let mut k = 0;
    while k < groups.len() {
        if groups[k].len() as f64 > MOST_OF_ONE * n as f64 && groups[k].len() >= 2 {
            let g = std::mem::take(&mut groups[k]);
            let mean: [f64; 3] = std::array::from_fn(|d| g.iter().map(|i| at[*i][d]).sum::<f64>() / g.len() as f64);
            let spread: Vec<f64> = (0..3).map(|d| g.iter().map(|i| (at[*i][d] - mean[d]).powi(2)).sum::<f64>()).collect();
            let d = (0..3).max_by(|a, b| spread[*a].total_cmp(&spread[*b])).unwrap();
            let mut sorted = g;
            sorted.sort_by(|a, b| at[*a][d].total_cmp(&at[*b][d]));
            let back = sorted.split_off(sorted.len() / 2);
            groups[k] = sorted;
            groups.push(back);
        } else {
            k += 1;
        }
    }
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    let mut half: [Vec<usize>; 2] = [vec![], vec![]];
    for g in groups {
        let h = if half[0].len() <= half[1].len() { 0 } else { 1 };
        half[h].extend(g);
    }
    let m = half[0].len().min(half[1].len());
    (0..m).flat_map(|i| [half[0][i], half[1][i]]).collect()
}

/// The pixel size of a tomogram file, Å.
pub(crate) fn pixel_of(path: &str) -> Result<f64> {
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
/// one integer (at least 1) that brings its voxel nearest, in the ratio (ties
/// to the finer grid), to twice the template's, the voxel the raw tomogram is
/// searched at. Returns (bin, search voxel).
pub(crate) fn look_search(look_px: f64, want: f64) -> (usize, f64) {
    let bin = nearest_bin(look_px, 2.0 * want);
    (bin, bin as f64 * look_px)
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
/// template's twice (`look_search`), as the raw search's, and the template is resampled to that voxel (`rescale_cube`)
/// for the search only. Each peak is then mapped to the tomogram itself
/// (`to_raw`) and the cubes for alignment and averaging, in both passes, are
/// cut from that, so the averages are of the unfiltered data. The template,
/// made from look data, is only the reference the alignment starts from, at
/// its own voxel size. The look tomogram is left out only when it cannot be
/// read. `Report::searched` and `search_voxel_a` say what was done.
#[allow(clippy::too_many_arguments)]
/// How far `run_template` goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// A card tried as a template: a rougher search, the coarse average.
    Trial,
    /// The search and the coarse average.
    Coarse,
    /// And the fine pass after it.
    Fine,
}

/// The cards are tried on the first tomograms only: which card to take
/// shows in two as well as in twenty.
const TRIAL_SOURCES: usize = 2;

/// Voxels across the particle in the search of a trial.
const TRIAL_ACROSS: f64 = 6.5;

/// A place the template search found: a candidate particle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// Which of the sources.
    pub source: usize,
    /// Centre in voxels of the source tomogram binned by `bin`, (x, y, z).
    pub c: [usize; 3],
    /// Source voxels per working voxel.
    pub bin: usize,
    /// Index into `grid(30.0)`: the orientation the template matched at.
    pub turn: usize,
    pub score: f32,
    /// Which of the user's examples it looks like (`circled`): the one
    /// whose picture found it. 0 on every other route.
    #[serde(default)]
    pub like: u8,
    /// Which of `Found::sizes_a` it was found at (`circled`).
    #[serde(default)]
    pub size: u8,
}

impl Place {
    /// The centre in voxels of the source tomogram (unbinned).
    pub fn at(&self) -> [f64; 3] {
        self.c.map(|x| x as f64 * self.bin as f64 + (self.bin as f64 - 1.0) / 2.0)
    }
}

/// What a template search found, before anything is averaged: the
/// workspace shows it for the user to check (`run_places` averages what is
/// kept).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    /// Every peak, best first.
    pub places: Vec<Place>,
    /// The score that parts the particles from the rest: between the two
    /// groups the scores fall into (`search::cutoff`), or, when they are
    /// one group, where they pass what chance scores
    /// (`search::beyond_chance`).
    pub limit: Option<f32>,
    /// The best score chance had in the control search (`chance`), when
    /// the scores were one group and one was made.
    #[serde(default)]
    pub noise: Option<f32>,
    /// Voxel of the cubes the places are cut at, Å.
    pub voxel_a: f64,
    /// "look", "raw" or "mixed": which tomograms were searched.
    pub searched: String,
    pub search_voxel_a: Option<f64>,
    /// `circled`: per example, the turn that carries the first example's
    /// picture onto it, so that places found by different examples are
    /// averaged in one frame (`turn_of`). Empty on every other route.
    #[serde(default)]
    pub frames: Vec<Mat>,
    /// `circled`: the sizes searched at (diameters, Å), `Place::size`
    /// indexes them.
    #[serde(default)]
    pub sizes_a: Vec<f64>,
    /// `circled`: how alike the examples are to each other: the lowest
    /// score one of them has in the search with the others' pictures.
    #[serde(default)]
    pub alike: Option<f32>,
    /// `circled`: how many times as wide as circled the pictures searched
    /// for were (1: as circled; more when the examples are told by what is
    /// around them).
    #[serde(default = "one")]
    pub wide: f64,
    /// `circled`: the sizes as the user circled them, per entry of
    /// `sizes_a`, where the program drew a circle in (`circled::tightened`);
    /// empty when the sizes are as drawn.
    #[serde(default)]
    pub drawn_a: Vec<f64>,
}

fn one() -> f64 {
    1.0
}

impl Found {
    /// The search of one template: nothing of what `circled` adds.
    pub fn plain(places: Vec<Place>, limit: Option<f32>, noise: Option<f32>, voxel_a: f64, searched: &str, search_voxel_a: Option<f64>) -> Found {
        Found { places, limit, noise, voxel_a, searched: searched.to_string(), search_voxel_a, frames: vec![], sizes_a: vec![], alike: None, wide: 1.0, drawn_a: vec![] }
    }

    /// The orientation of the thing at `p` in the frame of the template
    /// (`turns`: `grid(30.0)`): what turned the picture that found it, after
    /// what turns the template onto that picture.
    pub fn turn_of(&self, p: &Place, turns: &[Mat]) -> Option<Mat> {
        let r = turns.get(p.turn)?;
        Some(match self.frames.get(p.like as usize) {
            Some(f) => mul(r, f),
            None => *r,
        })
    }

    /// Fewest places that are averaged: a kind of particle fills a tomogram
    /// by the hundred, and fewer than `FEWEST` found say the search failed;
    /// what the user circled may be large and rare.
    pub fn fewest(&self) -> usize {
        if self.sizes_a.is_empty() { FEWEST } else { FEWEST_CIRCLED }
    }

    /// How many places the program would keep by itself.
    pub fn sure(&self) -> usize {
        match self.limit {
            Some(l) => self.places.iter().filter(|p| p.score >= l).count().min(MOST),
            None => 0,
        }
    }
}

/// Fewest particles `run_places` averages.
pub const FEWEST: usize = 20;
/// The same for circled examples (the alignment wants four cubes at least).
pub const FEWEST_CIRCLED: usize = 4;

/// Whether `cube_n` gives a cube of side `n` around voxel `c`.
pub(super) fn cube_fits(dims: [usize; 3], c: [usize; 3], n: usize) -> bool {
    c[2] < dims[2] && (0..2).all(|i| c[i] >= n / 2 && c[i] + n / 2 <= dims[i])
}

/// The search of `run_template` alone: every place in the tomograms that
/// looks like `template` (a `BOX`³ cube at `voxel_a`; `diameter_a` the
/// particle's size), best first. A trial searches more coarsely.
pub fn find_template(dev: &burn::tensor::Device, sources: &[Source], template: &[f32], voxel_a: f64, diameter_a: f64, trial: bool, progress: Progress) -> Result<Found> {
    use super::search::{beyond_chance, cutoff, peaks};
    if sources.is_empty() {
        bail!("没有断层图");
    }
    assert_eq!(template.len(), BOX * BOX * BOX);
    let turns = grid(30.0);
    let mut voxel = voxel_a;
    let mut places: Vec<Place> = vec![];
    // (look searched, search voxel Å) per source.
    let mut searched: Vec<(bool, f64)> = vec![];
    for (i, s) in sources.iter().enumerate() {
        check_cancel()?;
        let g = SearchGrid::of(s, template, voxel_a, diameter_a, trial)?;
        voxel = g.voxel_a;
        searched.push((g.look.is_some(), g.search_voxel_a));
        let stage = format!("搜索 {}/{}", i + 1, sources.len());
        let found = super::search_gpu::search(dev, &g.coarse, g.cdims, &g.small, g.small_n, g.radius, s.tilts, &turns, Some(&g.mask), &|a, b| progress(&stage, a, b))?;
        // Particles do not overlap, but may touch.
        for p in peaks(&found, 1.8 * g.radius, MOST) {
            // Coarse voxel p holds fine voxels 2p and 2p + 1.
            let c = match &g.look {
                None => p.pos.map(|x| 2 * x),
                // On the look tomogram the peak is at a voxel of the grid binned by
                // `look_bin`; mapped onto the raw tomogram's binned voxels.
                Some((look, look_bin)) => {
                    let raw = to_raw(p.pos.map(|x| x as f64), *look_bin, g.bin, look);
                    if raw.iter().any(|x| !x.is_finite() || x.round() < 0.0) {
                        continue;
                    }
                    raw.map(|x| x.round() as usize)
                }
            };
            if cube_fits(g.dims, c, BOX) {
                places.push(Place { source: i, c, bin: g.bin, turn: p.turn, score: p.score, like: 0, size: 0 });
            }
        }
    }
    places.sort_by(|a, b| b.score.total_cmp(&a.score));
    let scores: Vec<super::search::Peak> = places.iter().map(|p| super::search::Peak { pos: p.c, score: p.score, turn: p.turn }).collect();
    let mut limit = cutoff(&scores);
    let mut noise = None;
    if limit.is_none() && !scores.is_empty() {
        // One group: all particles (a tomogram packed with them) or none.
        // What chance scores here says which.
        let mut by_chance: Vec<(f32, f64)> = vec![];
        for (i, s) in sources.iter().enumerate() {
            check_cancel()?;
            let g = SearchGrid::of(s, template, voxel_a, diameter_a, trial)?;
            let stage = format!("搜索对照 {}/{}", i + 1, sources.len());
            by_chance.extend(chance(dev, &g.coarse, g.cdims, &g.mask, &g.small, g.small_n, g.radius, s.tilts, &turns, &|a, b| progress(&stage, a, b))?);
        }
        noise = by_chance.iter().map(|c| c.0).reduce(f32::max);
        limit = beyond_chance(&scores, &by_chance);
    }
    let kind = if searched.iter().all(|s| s.0) {
        "look"
    } else if searched.iter().all(|s| !s.0) {
        "raw"
    } else {
        "mixed"
    };
    Ok(Found::plain(places, limit, noise, voxel, kind, searched.first().map(|s| s.1)))
}

/// What one tomogram is searched as: the grid, where a particle may lie on
/// it, and the template at its voxel.
struct SearchGrid {
    /// The raw tomogram's binned size, its voxel (Å) and bin: what the
    /// particles are cut from.
    dims: [usize; 3],
    voxel_a: f64,
    bin: usize,
    /// The look tomogram searched and its bin, when there is one.
    look: Option<(LookSource, usize)>,
    coarse: Vec<f32>,
    cdims: [usize; 3],
    search_voxel_a: f64,
    /// The particle's radius in search voxels.
    radius: f64,
    mask: Vec<bool>,
    small: Vec<f32>,
    small_n: usize,
}

impl SearchGrid {
    /// The raw tomogram the particles are cut from, and the one searched:
    /// the look tomogram when there is one, else the raw one. The look
    /// tomogram is binned by the integer that brings its voxel nearest
    /// twice the template's (`look_search`), the raw search's voxel, and the
    /// template is resampled to that voxel for the search only.
    fn of(s: &Source, template: &[f32], voxel_a: f64, diameter_a: f64, trial: bool) -> Result<SearchGrid> {
        use super::search::allowed;
        let (v, dims, got, bin) = binned(&s.path, voxel_a)?;
        let looked = match &s.look {
            Some(l) => match pixel_of(&l.path).and_then(|px| {
                // A trial only has to show whether particles stand out: it
                // searches where the particle is `TRIAL_ACROSS` voxels across,
                // several times less work for a small one.
                let lbin = if trial { nearest_bin(px, diameter_a / TRIAL_ACROSS) } else { look_search(px, got).0 };
                binned(&l.path, lbin as f64 * px)
            }) {
                Ok(b) => Some((l.clone(), b)),
                Err(e) => {
                    eprintln!("sta: the look tomogram of {} cannot be used ({e:#}): searching the tomogram itself", s.path);
                    None
                }
            },
            None => None,
        };
        let (coarse, cdims, search_voxel_a) = match &looked {
            Some((_, b)) => (b.0.clone(), b.1, b.2),
            None => {
                let (c, d) = halve(&v, dims);
                (c, d, 2.0 * got)
            }
        };
        let radius = 0.5 * diameter_a / search_voxel_a;
        let mask = match &looked {
            None => allowed(&coarse, cdims, radius),
            // Exclusion is decided on the raw tomogram, as without a look
            // one, and carried over to the look grid (plus its own borders).
            Some((l, b)) => {
                let (raw_coarse, raw_cdims) = halve(&v, dims);
                let raw_ok = allowed(&raw_coarse, raw_cdims, 0.25 * diameter_a / got);
                allowed_on_look(&raw_ok, raw_cdims, 2, bin, cdims, 1, b.3, l, radius)
            }
        };
        // The template for the search, at the search voxel: the raw path
        // halves it; on the look grid it is resampled about its centre.
        let (small, small_n) = if looked.is_none() {
            (crate::spa::fourier::resample3(template, BOX, BOX / 2), BOX / 2)
        } else {
            let n = if 2.0 * radius + 4.0 <= (BOX / 2) as f64 { BOX / 2 } else { BOX };
            (rescale_cube(template, BOX, got, n, search_voxel_a), n)
        };
        Ok(SearchGrid { dims, voxel_a: got, bin, look: looked.map(|(l, b)| (l, b.3)), coarse, cdims, search_voxel_a, radius, mask, small, small_n })
    }
}

/// The control search takes this share of the tomogram's tiles (and never
/// fewer than `CONTROL_FEWEST`, or all there are).
const CONTROL_SHARE: usize = 4;
const CONTROL_FEWEST: usize = 8;

/// What chance scores in a search: the peaks of the same search in the
/// tomogram with its phases scrambled (`search::scrambled`), over some of
/// the tomogram's tiles spread through it, each peak with the number of
/// peaks of the whole tomogram it stands for. For `search::beyond_chance`.
#[allow(clippy::too_many_arguments)]
pub(super) fn chance(
    dev: &burn::tensor::Device,
    v: &[f32],
    dims: [usize; 3],
    allowed: &[bool],
    template: &[f32],
    n: usize,
    radius: f64,
    tilts: [f64; 2],
    turns: &[crate::spa::geom::Mat],
    tick: &(dyn Fn(usize, usize) + Sync),
) -> Result<Vec<(f32, f64)>> {
    use super::search::{peaks, scrambled};
    let t = super::search_gpu::TILE;
    let [nx, ny, nz] = dims;
    let tiles = [nx.div_ceil(t), ny.div_ceil(t), nz.div_ceil(t)];
    let tile_of = |i: usize| (i / (nx * ny) / t * tiles[1] + i / nx % ny / t) * tiles[0] + i % nx / t;
    // Allowed voxels per tile.
    let mut held = vec![0usize; tiles[0] * tiles[1] * tiles[2]];
    for (i, ok) in allowed.iter().enumerate() {
        if *ok {
            held[tile_of(i)] += 1;
        }
    }
    let mut order: Vec<usize> = (0..held.len()).filter(|k| held[*k] > 0).collect();
    if order.is_empty() {
        return Ok(vec![]);
    }
    // Spread through the tomogram, the same every time.
    order.sort_by_key(|k| (*k as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let take = order.len().div_ceil(CONTROL_SHARE).max(CONTROL_FEWEST.min(order.len()));
    let mut taken = vec![false; held.len()];
    order[..take].iter().for_each(|k| taken[*k] = true);
    let part: Vec<bool> = allowed.iter().enumerate().map(|(i, ok)| *ok && taken[tile_of(i)]).collect();
    let all: usize = held.iter().sum();
    let here: usize = order[..take].iter().map(|k| held[*k]).sum();
    let each = all as f64 / here as f64;
    let found = super::search_gpu::search(dev, &scrambled(v, dims, t), dims, template, n, radius, tilts, turns, Some(&part), tick)?;
    Ok(peaks(&found, 1.8 * radius, MOST).into_iter().map(|p| (p.score, each)).collect())
}

/// The cubes at `places` (and the poses the search found them at), one
/// tomogram in memory at a time. Places whose cube cannot be cut are left out.
pub(super) fn cut_places(sources: &[Source], found: &Found, places: &[Place]) -> Result<(Vec<Cut>, Vec<Pose>)> {
    let voxel_a = found.voxel_a;
    let turns = grid(30.0);
    let mut cuts: Vec<(usize, Cut, Pose)> = vec![];
    for (i, s) in sources.iter().enumerate() {
        if !places.iter().any(|p| p.source == i) {
            continue;
        }
        check_cancel()?;
        let (v, dims, _, bin) = binned(&s.path, voxel_a)?;
        for (k, p) in places.iter().enumerate().filter(|(_, p)| p.source == i && p.bin == bin) {
            if let (Some(cube), Some(r)) = (cube(&v, dims, p.c), found.turn_of(p, &turns)) {
                cuts.push((k, Cut { source: i, at: p.at(), bin, score: p.score, strength: 0.0, darkest: 0.0, cube }, Pose { r, t: [0.0; 3], score: p.score }));
            }
        }
    }
    // In the order of `places` (best first), as the search gave them.
    cuts.sort_by_key(|c| c.0);
    Ok(cuts.into_iter().map(|(_, c, p)| (c, p)).unzip())
}

/// The whole template run: `find_template`, the places whose scores stand
/// out (`Found::limit`), then `run_places`.
#[allow(clippy::too_many_arguments)]
pub fn run_template(
    dev: &burn::tensor::Device,
    sources: &[Source],
    template: &[f32],
    voxel_a: f64,
    diameter_a: f64,
    pass: Pass,
    out_dir: &str,
    progress: Progress,
    live: Live,
) -> Result<Report> {
    let found = find_template(dev, sources, template, voxel_a, diameter_a, pass == Pass::Trial, progress)?;
    let Some(limit) = found.limit else {
        let best = found.places.first().map_or(0.0, |p| p.score);
        match found.noise {
            Some(noise) => bail!("没有找到明显像模板的颗粒（{} 个候选的分数分不出两群，也不比在打乱的图里碰巧得到的分数高：最高 {best:.3}，碰巧的最高 {noise:.3}）", found.places.len()),
            None => bail!("没有找到明显像模板的颗粒（{} 个候选的分数分不出两群）", found.places.len()),
        }
    };
    let keep: Vec<Place> = found.places.iter().filter(|p| p.score >= limit).take(MOST).copied().collect();
    run_places(dev, sources, template, diameter_a, &found, &keep, pass, out_dir, progress, live)
}

/// Fewest places `probed` measures a body in.
const PROBED_FEWEST: usize = 16;

/// The dark body in the average of all the places of `found` (a search from
/// circled examples), aligned as `run_places` first aligns such places (their
/// own average, then every orientation against it): how far across it is
/// (`body`, voxels of `found`), in how many pieces and how long for its width
/// (`shape`). Unaligned, the average is as wide as the thing at its widest
/// and blurred by where the search put each place (measured: ribosomes of
/// 270 Å come out 371 Å across). None when too few places can be cut out or
/// nothing dark stands there.
pub(super) fn probed(dev: &burn::tensor::Device, sources: &[Source], found: &Found, diameter_a: f64, progress: Progress) -> Result<Option<(f64, usize, f64)>> {
    let (cuts, poses) = cut_places(sources, found, &found.places)?;
    if cuts.len() < PROBED_FEWEST {
        return Ok(None);
    }
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let radius = (0.5 * diameter_a / found.voxel_a + 2.0).min(BOX as f64 / 2.0 - 3.0);
    let particles: Vec<Vec<f32>> = cuts.iter().map(|c| c.cube.clone()).collect();
    let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    let own_average = set.average(&poses, |_| true).finish();
    let (average, _) = super::refine::settled_on(Some(dev), &set, &own_average, 1, progress, &|_| {})?;
    let (_, elongation, blobs, _) = shape(&average, BOX, radius);
    Ok(body(&average, BOX, radius).map(|across| (across, blobs, elongation)))
}

/// How many times as wide as circled `by_surroundings` looks when the
/// search did not say (`Found::wide`).
const AROUND: f64 = 2.0;
/// Most circled particles `run_places` also aligns by their surroundings
/// without the search having asked for it: at this many the second
/// alignment takes seconds.
const AROUND_MOST: usize = 100;

/// The particles of `cuts` aligned by what is around them: a thing that sits
/// on something larger (a small structure on a membrane) is turned the right
/// way by the larger thing, which a cube as wide as its circle hardly holds
/// (measured: 20 circled crowns of 283 Å average to pieces of membrane in
/// their own cubes, and to the ring on its neck at twice the width). Cubes
/// `by` times as wide, at `by` times the voxel, are aligned to their own
/// average over every orientation (`settled_on`, from `poses`). The centre of
/// each circle then lies somewhere in that average, and where most of them
/// lie is where the thing is: each particle is cut again at `voxel_a` about
/// that point of the average as its own pose places it, so a circle drawn
/// off the thing is put right by the others. Returns those cuts with the
/// orientations found (no shift left), and the wide average; None when too
/// few have surroundings to cut.
#[allow(clippy::too_many_arguments)]
fn by_surroundings(dev: &burn::tensor::Device, sources: &[Source], cuts: &[Cut], poses: &[Pose], voxel_a: f64, diameter_a: f64, by: f64, progress: Progress, live: Live) -> Result<Option<(Vec<Cut>, Vec<Pose>, Vec<f32>, f64)>> {
    let label = |s: &str, a: usize, b: usize| progress(&format!("连周围一起 · {s}"), a, b);
    // The wide cubes: per cut, the cube and its centre in source voxels.
    let mut wide: Vec<Option<(Vec<f32>, [f64; 3])>> = cuts.iter().map(|_| None).collect();
    let mut wide_bin = vec![1usize; sources.len()];
    let mut wide_voxel = voxel_a * by;
    for (i, s) in sources.iter().enumerate() {
        if !cuts.iter().any(|c| c.source == i) {
            continue;
        }
        check_cancel()?;
        label("裁出周围", i, sources.len());
        let (v, dims, got, bin) = binned(&s.path, voxel_a * by)?;
        wide_voxel = got;
        wide_bin[i] = bin;
        for (k, c) in cuts.iter().enumerate().filter(|(_, c)| c.source == i) {
            let (at, _) = on_grid(c.at, bin);
            if let Some(cube) = cube_around(&v, dims, at, BOX, 4) {
                wide[k] = Some((cube, at.map(|x| x as f64 * bin as f64 + (bin as f64 - 1.0) / 2.0)));
            }
        }
    }
    let with: Vec<usize> = (0..cuts.len()).filter(|k| wide[*k].is_some()).collect();
    if with.len() < FEWEST_CIRCLED {
        return Ok(None);
    }
    let particles: Vec<Vec<f32>> = with.iter().map(|k| wide[*k].as_ref().unwrap().0.clone()).collect();
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let wedge_of: Vec<usize> = with.iter().map(|k| cuts[*k].source).collect();
    let radius = (0.5 * by * diameter_a / wide_voxel + 2.0).min(BOX as f64 / 2.0 - 3.0);
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    // A thing on a sheet (a membrane, the wall of a vesicle) stands on it:
    // each one's axis is read off the sheet around it (`axial`), and only
    // the turn about it is searched, then a few degrees every way. The
    // agreement of the halves cannot choose between this and turning every
    // way: with as few particles as are circled the two come out alike also
    // where only the axes are right (measured on made-up things on balls:
    // axes read within 3°, turned every way 10–67° off, strengths 0.0099
    // and 0.0100).
    let (model, settled) = match by_axes(dev, &set, 0.5 * diameter_a / wide_voxel, &label, live)? {
        Some(axial) => (axial.map, axial.poses),
        None => {
            let start: Vec<Pose> = with.iter().map(|k| Pose { t: [0.0; 3], ..poses[*k] }).collect();
            let own_average = set.average(&start, |_| true).finish();
            let (model, settled) = super::refine::settled_on(Some(dev), &set, &own_average, 1, &label, live)?;
            // Then a few degrees at a time: the orientations tried are 30°
            // apart, and the cubes cut next are turned no further than 12°.
            let a = align_on(Some(dev), &set, &model, &settled, &label, live)?;
            (a.map, a.poses)
        }
    };
    // Where each circle's centre lies in the average (a point m of the
    // average stands at t + R·m in the particle; the circle's centre is the
    // cube's), and the middle of them.
    let centres: Vec<[f64; 3]> = settled.iter().map(|p| std::array::from_fn(|i| -(0..3).map(|j| p.r[j][i] * p.t[j]).sum::<f64>())).collect();
    let middle: [f64; 3] = std::array::from_fn(|i| {
        let mut v: Vec<f64> = centres.iter().map(|c| c[i]).collect();
        v.sort_by(f64::total_cmp);
        0.5 * (v[(v.len() - 1) / 2] + v[v.len() / 2])
    });
    let mut off: Vec<f64> = centres.iter().map(|c| (0..3).map(|i| (c[i] - middle[i]).powi(2)).sum::<f64>().sqrt() * wide_voxel).collect();
    off.sort_by(f64::total_cmp);
    eprintln!(
        "sta: by the surroundings ({by}× as wide, {wide_voxel:.2} Å voxels, {} of {} particles): the circles' centres lie {:.0} Å from their middle (median; the farthest {:.0} Å)",
        with.len(),
        cuts.len(),
        off[off.len() / 2],
        off[off.len() - 1]
    );
    // Each particle about that point, at the voxel of the average.
    let mut out: Vec<(usize, Cut, Pose)> = vec![];
    for (i, s) in sources.iter().enumerate() {
        if !with.iter().any(|k| cuts[*k].source == i) {
            continue;
        }
        check_cancel()?;
        label("按周围重新裁出", i, sources.len());
        let (v, dims, _, bin) = binned(&s.path, voxel_a)?;
        for (n, k) in with.iter().enumerate().filter(|(_, k)| cuts[**k].source == i) {
            let p = &settled[n];
            let centre = wide[*k].as_ref().unwrap().1;
            let at: [f64; 3] = std::array::from_fn(|a| centre[a] + (p.t[a] + (0..3).map(|j| p.r[a][j] * middle[j]).sum::<f64>()) * wide_bin[i] as f64);
            if at.iter().any(|x| *x < 0.0) {
                continue;
            }
            let (c, _) = on_grid(at, bin);
            if let Some(cube) = cube(&v, dims, c) {
                let old = &cuts[*k];
                out.push((*k, Cut { source: i, at: c.map(|x| x as f64 * bin as f64 + (bin as f64 - 1.0) / 2.0), bin, score: old.score, strength: old.strength, darkest: old.darkest, cube }, Pose { r: p.r, t: [0.0; 3], score: p.score }));
            }
        }
    }
    // In the order of `cuts`, which the halves follow.
    out.sort_by_key(|c| c.0);
    if out.len() < FEWEST_CIRCLED {
        return Ok(None);
    }
    let (cuts, poses) = out.into_iter().map(|(_, c, p)| (c, p)).unzip();
    Ok(Some((cuts, poses, model, wide_voxel)))
}

/// Least median flatness (`axial::sheet`) of the surroundings for their
/// normals to be taken as axes: noise alone reads about 0.1.
const SHEET_FLAT: f64 = 0.3;

/// The wide cubes of `set` (a thing in each, `radius` voxels across its
/// circle) aligned with each one's axis read off the sheet it sits on
/// (`axial::sheet`, either sense) and only the turn about it searched, then
/// a few degrees every way (`align_on`). None when what is around them is
/// not sheet-like.
fn by_axes(dev: &burn::tensor::Device, set: &Set, radius: f64, progress: Progress, live: Live) -> Result<Option<super::refine::Aligned>> {
    let read: Vec<([f64; 3], f64)> = set.particles.par_iter().map(|c| super::axial::sheet(c, set.n, radius.max(2.0))).collect();
    let mut flat: Vec<f64> = read.iter().map(|r| r.1).collect();
    flat.sort_by(f64::total_cmp);
    let middle = flat[flat.len() / 2];
    eprintln!("sta: what the things sit on is this sheet-like (median of {}): {middle:.2}{}", flat.len(), if middle < SHEET_FLAT { "; turned every way" } else { "; each stood on it, turned about its axis" });
    if middle < SHEET_FLAT {
        return Ok(None);
    }
    let axes: Vec<[f64; 3]> = read.iter().map(|r| r.0).collect();
    let (model, poses) = super::refine::first_model_known(set, &axes, true, 1, progress, live)?;
    Ok(Some(align_on(Some(dev), set, &model, &poses, progress, live)?))
}

/// The average of the particles at `places` (those of `found` the caller
/// keeps: the program's own choice in `run_template`, the user's in the
/// workspace): aligned to `template`, then as `pass` says.
#[allow(clippy::too_many_arguments)]
pub fn run_places(
    dev: &burn::tensor::Device,
    sources: &[Source],
    template: &[f32],
    diameter_a: f64,
    found: &Found,
    places: &[Place],
    pass: Pass,
    out_dir: &str,
    progress: Progress,
    live: Live,
) -> Result<Report> {
    let fine = pass == Pass::Fine;
    let started = Instant::now();
    assert_eq!(template.len(), BOX * BOX * BOX);
    let candidates = found.places.len();
    let mut voxel = found.voxel_a;
    let (mut cuts, poses) = cut_places(sources, found, places)?;
    if cuts.len() < found.fewest() {
        bail!("只找到 {} 个，太少了（至少要 {} 个）", cuts.len(), found.fewest());
    }
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    let radius = (0.5 * diameter_a / voxel + 2.0).min(BOX as f64 / 2.0 - 3.0);
    let particles: Vec<Vec<f32>> = cuts.iter().map(|c| c.cube.clone()).collect();
    let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    // Told apart in the labels when there are two passes.
    let first_pass = |s: &str, a: usize, b: usize| progress(&if fine { s.replacen("对齐", "粗对齐", 1) } else { s.to_string() }, a, b);
    // Places found from circled examples start from their own average at
    // the orientations the search gave: the template there is one cube cut
    // around one example, as noisy as any particle, and aligning hundreds
    // of particles to it leaves them where its noise puts them. Those
    // orientations are rough (the search sees the particle nine voxels
    // across), so every particle is first searched over all of them
    // against that average (`settled_on`).
    let settled;
    let (circled_a, searched) = (diameter_a, poses.clone());
    let (template, poses, mut diameter_a) = if found.frames.is_empty() {
        (template, poses, diameter_a)
    } else {
        let own_average = set.average(&poses, |_| true).finish();
        settled = super::refine::settled_on(Some(dev), &set, &own_average, 1, progress, live)?;
        // The circle is as wide as it was drawn; the masks from here on
        // follow the body that stands in the average (never wider than the
        // circle, and not less than half of it: a poor average says little).
        let measured = body(&settled.0, BOX, radius).map_or(diameter_a, |d| (d * voxel).clamp(0.5 * diameter_a, diameter_a));
        eprintln!("sta: circled at {diameter_a:.0} Å, the body in the average is {measured:.0} Å across");
        (&settled.0[..], settled.1.clone(), measured)
    };
    let radius = (0.5 * diameter_a / voxel + 2.0).min(BOX as f64 / 2.0 - 3.0);
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    let mut out = align_on(Some(dev), &set, template, &poses, &first_pass, live)?;
    // What was circled is also aligned by its surroundings first, where the
    // search was told by them (`Found::wide`) or the particles are few; the
    // one of the two averages whose halves agree over more is kept (both
    // within the circle as drawn).
    if !found.frames.is_empty() && (found.wide > 1.0 || cuts.len() <= AROUND_MOST) {
        let by = if found.wide > 1.0 { found.wide } else { AROUND };
        if let Some((cuts2, start2, around, around_voxel)) = by_surroundings(dev, sources, &cuts, &searched, voxel, circled_a, by, progress, live)? {
            let particles2: Vec<Vec<f32>> = cuts2.iter().map(|c| c.cube.clone()).collect();
            let wedge_of2: Vec<usize> = cuts2.iter().map(|c| c.source).collect();
            let whole = (0.5 * circled_a / voxel + 2.0).min(BOX as f64 / 2.0 - 3.0);
            let set2 = Set { n: BOX, particles: &particles2, wedges: &wedges, wedge_of: &wedge_of2, radius: whole };
            let own = set2.average(&start2, |_| true).finish();
            let measured = body(&own, BOX, whole).map_or(circled_a, |d| (d * voxel).clamp(0.5 * circled_a, circled_a));
            let set2 = Set { radius: (0.5 * measured / voxel + 2.0).min(BOX as f64 / 2.0 - 3.0), ..set2 };
            let out2 = align_on(Some(dev), &set2, &own, &start2, &|s: &str, a: usize, b: usize| first_pass(&format!("连周围一起 · {s}"), a, b), live)?;
            let agree = |a: &super::refine::Aligned| strength(&crate::spa::post::fsc(&a.halves[0], &a.halves[1], BOX, whole + 2.0), voxel);
            let (direct, wide) = (agree(&out), agree(&out2));
            let better = wide > direct;
            eprintln!(
                "sta: aligned by the surroundings first: strength {wide:.5} ({} particles) against {direct:.5} as circled ({}), keeping the {}",
                cuts2.len(),
                cuts.len(),
                if better { "one by the surroundings" } else { "one as circled" }
            );
            std::fs::create_dir_all(out_dir).with_context(|| format!("无法创建 {out_dir}"))?;
            output::write_mrc(&Path::new(out_dir).join("around.mrc").to_string_lossy(), &around, [BOX, BOX, BOX], around_voxel, false)?;
            if better {
                cuts = cuts2;
                out = out2;
                diameter_a = measured;
            }
            live(&out.map);
        }
    }
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
        out = align_on(Some(dev), &set, &model, &start, &|s: &str, a: usize, b: usize| progress(&s.replacen("对齐", "精细对齐", 1), a, b), live)?;
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
        sharpen_b: None,
        ctf_limited: false,
        fsc: vec![],
        averaged_from: None,
        fsc_other: None,
        views_refined: None,
        pattern: None,
        map: None,
        halves: None,
        table: None,
        ms: 0,
    };
    report.searched = Some(found.searched.clone());
    report.search_voxel_a = found.search_voxel_a;
    // The same particles, where they now are, from the tilt images.
    let mut spare: Option<super::refine::Aligned> = None;
    let mut final_voxel = voxel;
    let mut tilted_kept = false;
    if fine && sources.iter().all(|s| s.series.is_some()) {
        let n = (out.map.len() as f64).cbrt().round() as usize;
        match from_tilts(sources, &cuts, &out.poses, n, voxel, diameter_a, None, progress) {
            Ok(tilted) => {
                let area = |f: &[f64]| f.iter().skip(1).map(|v| v.max(0.0)).sum::<f64>();
                let better = area(&tilted.fsc) >= area(&out.fsc);
                let (kept, other) = if better { (tilted, out) } else { (out, tilted) };
                report.averaged_from = Some(if better { "tilt images" } else { "tomogram" }.into());
                report.fsc_other = Some(other.fsc.clone());
                spare = Some(other);
                out = kept;
                tilted_kept = better;
                live(&out.map);
                // Where the halves agree nearly to the limit of this sampling,
                // the views hold more than it shows: the same particles, the
                // same poses, voxels half the size. Kept when the halves then
                // agree over more (the area under the curve, 1/Å).
                if better && super::refine::extent(&out.fsc) >= 0.8 * (n / 2) as f64 {
                    match from_tilts(sources, &cuts, &out.poses, 2 * n, voxel / 2.0, diameter_a, None, progress) {
                        Ok(finer) => {
                            eprintln!("sta: at half the voxel size, FSC {}", finer.fsc.iter().map(|f| format!("{f:.3}")).collect::<Vec<_>>().join(" "));
                            if strength(&finer.fsc, voxel / 2.0) >= strength(&out.fsc, voxel) {
                                out = finer;
                                final_voxel = voxel / 2.0;
                                live(&out.map);
                            }
                        }
                        Err(e) => eprintln!("sta: no average at half the voxel size ({e:#})"),
                    }
                }
            }
            Err(e) => eprintln!("sta: the average from the tilt images could not be made ({e}); keeping the one from the tomograms"),
        }
    }
    // The views themselves: what all particles of a tomogram share (each
    // view's image shift, the tomogram's defocus) is found from the
    // particles together (`tilt::views`) against the halves just made, and
    // the average made again with it. Gold standard: the fixes are shared by
    // both halves (a few numbers per view against thousands of particles) and
    // each particle is compared only with the reference of its own half, so
    // the halves stay independent as far as the agreement tells. One round.
    if tilted_kept {
        let n = (out.map.len() as f64).cbrt().round() as usize;
        let weight = shell_weight(&out.fsc);
        match from_tilts(sources, &cuts, &out.poses, n, final_voxel, diameter_a, Some((&out.halves, &weight)), progress) {
            Ok(refined) => {
                // Kept only for a clear gain: the strengths of two builds
                // from the same particles differ a little by chance.
                const GAIN: f64 = 1.01;
                let (before, after) = (strength(&out.fsc, final_voxel), strength(&refined.fsc, final_voxel));
                let better = after > before * GAIN;
                eprintln!("sta: views refined: strength {after:.5} against {before:.5}, keeping the {}", if better { "refined average" } else { "one without" });
                report.views_refined = Some(better);
                if better {
                    out = refined;
                    live(&out.map);
                }
            }
            Err(e) => eprintln!("sta: the views could not be refined ({e:#})"),
        }
    }
    save(&mut report, sources, &cuts, &out, out_dir, diameter_a, final_voxel, started)?;
    if let Some(other) = spare {
        // The one not kept, to compare with.
        let n = (other.map.len() as f64).cbrt().round() as usize;
        for (name, v) in [("other_average.mrc", &other.map), ("other_half1.mrc", &other.halves[0]), ("other_half2.mrc", &other.halves[1])] {
            output::write_mrc(&Path::new(out_dir).join(name).to_string_lossy(), v, [n, n, n], voxel, false)?;
        }
    }
    Ok(report)
}

/// The share of the darkest value from which a voxel counts as the body.
const BODY_FROM: f32 = 0.3;
/// The share of the body's darkness the diameter holds.
const BODY_SHARE: f64 = 0.95;

/// The diameter (voxels) of the dark body at the middle of an average made
/// inside a sphere of `radius` voxels, or None when nothing stands out. A
/// circle drawn around an example is as wide as the hand drew it (a
/// ribosome circled at 347 Å where the body is some 250 Å); the masks of
/// the alignment and of the measured resolution then hold more of the
/// surroundings than of the particle. Darkness is counted below the level
/// of the sphere's rim, from `BODY_FROM` of the darkest value on (what
/// neighbours leave around the average is fainter), about its own centre.
pub(super) fn body(avg: &[f32], n: usize, radius: f64) -> Option<f64> {
    let c = (n / 2) as f64;
    let at = |i: usize| [(i % n) as f64 - c, ((i / n) % n) as f64 - c, (i / (n * n)) as f64 - c];
    let far = |i: usize| at(i).iter().map(|x| x * x).sum::<f64>().sqrt();
    let mut rim: Vec<f32> = (0..avg.len()).filter(|i| (far(*i) - radius).abs() <= 1.5).map(|i| avg[i]).collect();
    if rim.is_empty() {
        return None;
    }
    rim.sort_by(f32::total_cmp);
    let level = rim[rim.len() / 2];
    let dark: Vec<f32> = (0..avg.len()).map(|i| if far(i) <= radius { (level - avg[i]).max(0.0) } else { 0.0 }).collect();
    let top = dark.iter().copied().fold(0.0, f32::max);
    if top <= 0.0 {
        return None;
    }
    let body: Vec<(usize, f64)> = dark.iter().enumerate().filter(|(_, d)| **d >= BODY_FROM * top).map(|(i, d)| (i, *d as f64)).collect();
    let all: f64 = body.iter().map(|b| b.1).sum();
    let centre: [f64; 3] = std::array::from_fn(|k| body.iter().map(|(i, d)| at(*i)[k] * d).sum::<f64>() / all);
    let mut out: Vec<(f64, f64)> = body.iter().map(|(i, d)| ((0..3).map(|k| (at(*i)[k] - centre[k]).powi(2)).sum::<f64>().sqrt(), *d)).collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut sum = 0.0;
    let reach = out.iter().find(|(_, d)| {
        sum += d;
        sum >= BODY_SHARE * all
    })?;
    Some(2.0 * (reach.0 + 1.0))
}

/// Weights of the shells for comparing views with a model whose halves
/// agree as `fsc`: the amplitude of the whole map's FSC, √(2·FSC / (1 +
/// FSC)), from shell 2 to where the halves fall below 0.143, 0 beyond.
fn shell_weight(fsc: &[f64]) -> Vec<f64> {
    let cut = super::refine::cut(fsc);
    fsc.iter().enumerate().map(|(s, f)| if s >= 2 && s as f64 <= cut { (2.0 * f.max(0.0) / (1.0 + f.max(0.0))).sqrt() } else { 0.0 }).collect()
}

/// The average of the aligned particles made from the tilt images (`tilt`),
/// on the grid of `out` (voxels of `voxel` Å), with the same halves. With
/// `refine` (the half maps at this `n` and `voxel`, and the shell weights)
/// the views of each series are first refined by all its particles
/// (`tilt::views`).
#[allow(clippy::too_many_arguments)]
fn from_tilts(sources: &[Source], cuts: &[Cut], poses: &[Pose], n: usize, voxel: f64, diameter_a: f64, refine: Option<(&[Vec<f32>; 2], &[f64])>, progress: Progress) -> Result<super::refine::Aligned> {
    use super::tilt::{Particle, Series, Sums};
    // One series in memory at a time (half a gigabyte each). The particles
    // stay where the tomograms have them: put where the views have them
    // (`tilt::settle`) they moved by half a voxel and the halves agreed no
    // better, so those moves were noise; that waits for the refinement of
    // the views themselves.
    let mut sums = Sums::new(n, n as f64 / 2.0 - 2.0);
    let total = cuts.len().max(1);
    let mut before = 0;
    for (i, s) in sources.iter().enumerate() {
        let particles: Vec<Particle> = cuts
            .iter()
            .zip(poses)
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
                // Also in the project's other folders (the stack is in the parent of `2_tomograms/`).
                d.stack = crate::project_dirs::find_near(dir, &d.stack).unwrap_or_else(|| dir.join(name)).to_string_lossy().into_owned();
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
        // Views twice as fine as the average where they can be: see `Series::fold`.
        // (Not when the finer views do not fit in memory.)
        let open = |fold: usize| Series::open(&d, &t, dims, bin / fold, fold, &|a, b| progress(&label, a, b));
        let (mut series, fold) = match if bin % 2 == 0 { open(2).ok() } else { None } {
            Some(s) => (s, 2),
            None => (open(1)?, 1),
        };
        // The particle's disc and what the transfer spreads beyond it.
        series.window = fold as f64 * ((0.5 * diameter_a / voxel + 2.0).min(n as f64 / 2.0 - 3.0) + 6.0).min(n as f64 / 2.0 - 2.0);
        if let Some((refs, weight)) = refine {
            // Within 3 voxels of where the views have them.
            const VIEW_REACH: f64 = 3.0;
            let found = super::tilt::view_shifts(&series, &particles, refs, n, weight, VIEW_REACH);
            let trusted: Vec<f64> = found.iter().filter(|f| f.shift != [0.0, 0.0]).map(|f| (f.shift[0].powi(2) + f.shift[1].powi(2)).sqrt() * series.pixel_a).collect();
            eprintln!(
                "sta: {}: views refined, {} of {} with a shift (mean {:.1} Å)",
                Path::new(&s.path).file_name().map_or(String::new(), |f| f.to_string_lossy().into_owned()),
                trusted.len(),
                found.len(),
                trusted.iter().sum::<f64>() / trusted.len().max(1) as f64
            );
            for (fix, f) in series.fixes.iter_mut().zip(&found) {
                fix.shift[0] += f.shift[0];
                fix.shift[1] += f.shift[1];
            }
            // The defocus is not looked for (`tilt::views` would). On seven real
            // tomograms it was zero within 50 Å for four, and +0.44 and
            // +0.32 µm for two, beyond the range their own views were fitted
            // in: with defocus of micrometres the transfer spreads the
            // particle beyond its patch, and the fit leans one way. It waits
            // for patches that hold what the transfer spreads.
            // Where each view was moved, for whoever wants to plot it against the tilt.
            let moved: Vec<String> = series.views.iter().zip(&found).map(|(v, f)| format!("{:.0}°:{:+.1},{:+.1}", v.theta, f.shift[0] * series.pixel_a, f.shift[1] * series.pixel_a)).collect();
            eprintln!("sta:   view shifts (Å): {}", moved.join(" "));
        }
        sums.add(&series, &particles, &|a, _| progress("从倾斜图像平均", before + a, total))?;
        before += particles.len();
    }
    if sums.count < 4 {
        bail!("颗粒太少");
    }
    let radius = (0.5 * diameter_a / voxel + 2.0).min(n as f64 / 2.0 - 3.0);
    let (map, halves, fsc) = sums.finish(radius + 2.0);
    let shell = super::refine::cut(&fsc);
    Ok(super::refine::Aligned { map, halves, poses: poses.to_vec(), fsc, shell })
}

#[cfg(test)]
mod card_tests {
    use super::*;

    /// A circle drawn wide around a particle: the masks follow the body in
    /// the average, and what neighbours leave around it does not count.
    #[test]
    fn the_body_in_an_average_is_measured_inside_a_wide_circle() {
        let n = 32;
        let c = (n / 2) as f64;
        let made = |r_body: f64, ring: f64| -> Vec<f32> {
            (0..n * n * n)
                .map(|i| {
                    let p = [(i % n) as f64 - c, ((i / n) % n) as f64 - c, (i / (n * n)) as f64 - c];
                    let r = p.iter().map(|x| x * x).sum::<f64>().sqrt();
                    // A soft dark ball, and a faint ring of neighbours two radii out.
                    let ball = -1.0 / (1.0 + ((r - r_body) / 0.8).exp());
                    let ring = -ring * (-(r - 2.0 * r_body).powi(2) / 4.0).exp();
                    (ball + ring) as f32
                })
                .collect()
        };
        for r in [5.0, 7.0] {
            let d = body(&made(r, 0.15), n, 12.0).unwrap();
            assert!(d >= 2.0 * r - 1.0 && d <= 2.0 * r + 3.5, "radius {r}: {d}");
        }
        // Nothing darker than the rim: nothing to measure.
        assert!(body(&vec![0.0; n * n * n], n, 12.0).is_none());
    }

    fn likely_card(id: usize, likely: bool, compact: f64, members: usize) -> Card {
        Card { id, diameter_a: 160.0, measured_a: 150.0, voxel_a: 12.0, members, share: 0.3, path: String::new(), compact, elongation: 1.1, blobs: 1, offset: 0.0, likely }
    }

    // FSC curves (shells 0 to 16, box 32) of the coarse averages that real cards gave as templates.
    const FSC0: [f64; 17] = [1.0, 0.986, 0.995, 0.983, 0.981, 0.948, 0.879, 0.882, 0.806, 0.740, 0.634, 0.569, 0.498, 0.510, 0.444, 0.336, 0.242];
    const FSC1: [f64; 17] = [1.0, 0.997, 0.996, 0.992, 0.981, 0.966, 0.930, 0.872, 0.865, 0.832, 0.754, 0.668, 0.632, 0.406, 0.228, 0.034, 0.000];
    const FSC3: [f64; 17] = [1.0, 0.979, 0.936, 0.758, 0.578, 0.428, 0.270, 0.234, 0.199, 0.140, 0.152, 0.057, 0.065, 0.051, 0.083, 0.035, 0.005];
    const FSC4: [f64; 17] = [1.0, 0.988, 0.995, 0.966, 0.941, 0.915, 0.448, 0.647, 0.433, 0.137, 0.032, 0.119, 0.017, -0.001, -0.017, 0.034, 0.016];
    const FSC7: [f64; 17] = [1.0, 0.998, 0.995, 0.964, 0.934, 0.840, 0.606, 0.310, 0.238, 0.074, -0.019, 0.012, 0.009, 0.038, 0.030, -0.022, -0.009];

    fn worked(card: usize, fsc: &[f64], voxel_a: f64) -> Trial {
        Trial { card, ok: true, reason: None, particles: 100, candidates: 300, fsc: fsc.to_vec(), voxel_a, strength: strength(fsc, voxel_a), seconds: 1.0, elongation: None }
    }

    fn failed(card: usize) -> Trial {
        Trial { card, ok: false, reason: Some("没有找到明显像模板的颗粒".into()), particles: 0, candidates: 0, fsc: vec![], voxel_a: 10.0, strength: 0.0, seconds: 1.0, elongation: None }
    }

    #[test]
    fn strength_is_the_area_under_the_fsc() {
        // The areas of the real curves (the task quoted them to two digits, a few percent off for cards 3 and 4).
        let near = |a: f64, b: f64| assert!((a / b - 1.0).abs() < 0.01, "{a} vs {b}");
        near(strength(&FSC1, 13.02), 0.02677);
        near(strength(&FSC0, 17.36), 0.02058);
        near(strength(&FSC3, 9.77), 0.01590);
        near(strength(&FSC7, 13.02), 0.01452);
        near(strength(&FSC4, 17.36), 0.01204);
        // Shell 0 and negative values do not count; nothing to measure gives 0.
        assert_eq!(strength(&[1.0, -0.5, -0.1], 10.0), 0.0);
        assert!((strength(&[1.0, 1.0, 1.0], 10.0) - 2.0 / 40.0).abs() < 1e-12);
        assert_eq!(strength(&[], 10.0), 0.0);
        assert_eq!(strength(&[1.0], 10.0), 0.0);
    }

    #[test]
    fn a_line_that_averages_best_is_not_taken_while_there_is_a_particle() {
        // Seven tomograms tried on two: the line (card 2) agrees best.
        let trial = |card: usize, strength: f64, elongation: f64| Trial { card, ok: true, reason: None, particles: 900, candidates: 3000, fsc: vec![], voxel_a: 13.0, strength, seconds: 1.0, elongation: Some(elongation) };
        let trials = [trial(0, 0.0204, 1.39), trial(1, 0.0156, 1.3), trial(2, 0.0229, 1.9), trial(5, 0.0164, 2.4)];
        assert_eq!(good(&trials), vec![0, 1]);
        // Only things that run on: the best of them.
        assert_eq!(good(&[trial(2, 0.0229, 1.9), trial(5, 0.0164, 2.4)]), vec![2]);
    }

    #[test]
    fn cards_that_find_the_same_places_are_together() {
        let dir = std::env::temp_dir().join(format!("tmv-sta-places-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("particles.csv");
        std::fs::write(&file, "tomogram,x,y,z,rot,tilt,psi,score,half,blob,strength,darkest\n/a,b/t.mrc,10.00,20.00,30.00,0,0,0,0.5,0,1,1,1\n/a,b/t.mrc,100.00,20.00,30.00,0,0,0,0.5,1,1,1,1\n").unwrap();
        let a = places_of(&file);
        assert_eq!(a, vec![("/a,b/t.mrc".to_string(), [10.0, 20.0, 30.0]), ("/a,b/t.mrc".to_string(), [100.0, 20.0, 30.0])]);
        let b = vec![("/a,b/t.mrc".to_string(), [12.0, 21.0, 30.0]), ("/other.mrc".to_string(), [100.0, 20.0, 30.0]), ("/a,b/t.mrc".to_string(), [300.0, 0.0, 0.0])];
        assert_eq!(together(&a, &b, 5.0), 0.5);
        assert_eq!(together(&a, &[], 5.0), 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn of_several_kinds_that_average_well_the_strongest_is_chosen_and_the_others_named() {
        // Two tomograms: cards 0 and 1 are the particle, card 2 a straight line.
        let cards: Vec<Card> = (0..3).map(|i| likely_card(i, true, 8.0, 50)).collect();
        let trial = |card: usize, particles: usize, strength: f64| Trial { card, ok: true, reason: None, particles, candidates: 3000, fsc: vec![], voxel_a: 13.0, strength, seconds: 1.0, elongation: None };
        let sim = vec![vec![1.0, 0.9, 0.2], vec![0.9, 1.0, 0.2], vec![0.2, 0.2, 1.0]];
        let got = choose(&cards, &[trial(0, 880, 0.0241), trial(1, 1023, 0.0305), trial(2, 336, 0.0264)], &sim);
        assert_eq!(got, Some(Chosen { id: 1, same: vec![0, 1], other: vec![2] }));
        // However often the other was found.
        assert_eq!(choose(&cards, &[trial(0, 880, 0.0241), trial(1, 1023, 0.0305), trial(2, 2900, 0.0264)], &sim).map(|c| c.id), Some(1));
    }

    #[test]
    fn the_card_that_makes_the_best_average_is_chosen() {
        let sim = |v: &[&[f64]]| -> Vec<Vec<f64>> { v.iter().map(|r| r.to_vec()).collect() };
        let cards: Vec<Card> = (0..9).map(|i| likely_card(i, i % 2 == 0, 8.0, 50)).collect();
        // The measured case: cards 2, 5, 6 and 8 find nothing; 0 and 1 are the same particle (0.88).
        let trials = [
            worked(0, &FSC0, 17.36),
            worked(1, &FSC1, 13.02),
            failed(2),
            worked(3, &FSC3, 9.77),
            worked(4, &FSC4, 17.36),
            failed(5),
            failed(6),
            worked(7, &FSC7, 13.02),
            failed(8),
        ];
        assert_eq!(good(&trials), vec![0, 1]);
        let mut m = vec![vec![0.0; 9]; 9];
        for i in 0..9 {
            m[i][i] = 1.0;
        }
        m[0][1] = 0.88;
        m[1][0] = 0.88;
        assert_eq!(choose(&cards, &trials, &m), Some(Chosen { id: 1, same: vec![0, 1], other: vec![] }));
        // Another order of the cards makes no difference.
        let swapped = [worked(0, &FSC1, 13.02), worked(1, &FSC0, 17.36)];
        assert_eq!(choose(&cards, &swapped, &sim(&[&[1.0, 0.88], &[0.88, 1.0]])).map(|c| c.id), Some(0));
        // The same two cards are two kinds when they do not look alike (0.53 at most): the user decides.
        m[0][1] = 0.53;
        m[1][0] = 0.53;
        assert_eq!(choose(&cards, &trials, &m), Some(Chosen { id: 1, same: vec![1], other: vec![0] }));
        // The best alone: the others are too weak (card 3 at 0.0159 is 0.59 of card 1).
        assert_eq!(choose(&cards, &[worked(1, &FSC1, 13.02), worked(3, &FSC3, 9.77), failed(2)], &[]), Some(Chosen { id: 1, same: vec![1], other: vec![] }));
        // Only cards 4 and 7 worked: within CLOSE of each other (0.0120 and 0.0145), so both are good;
        // one kind picks the stronger (7), two kinds leave the choice.
        let weak = [worked(4, &FSC4, 17.36), worked(7, &FSC7, 13.02)];
        assert_eq!(good(&weak), vec![4, 7]);
        let mut m = vec![vec![0.0; 9]; 9];
        m[4][7] = 0.8;
        m[7][4] = 0.8;
        assert_eq!(choose(&cards, &weak, &m), Some(Chosen { id: 7, same: vec![4, 7], other: vec![] }));
        m[4][7] = 0.3;
        m[7][4] = 0.3;
        assert_eq!(choose(&cards, &weak, &m), Some(Chosen { id: 7, same: vec![7], other: vec![4] }));
        // The alikeness is read either way round, and a chain is one kind (SAME is enough).
        let mut m = vec![vec![0.0; 9]; 9];
        m[7][4] = SAME;
        assert_eq!(choose(&cards, &weak, &m).map(|c| c.id), Some(7));
        // Nothing worked; nothing tried.
        assert_eq!(choose(&cards, &[failed(0), failed(1)], &[]), None);
        assert_eq!(choose(&cards, &[], &[]), None);
        assert_eq!(choose(&[], &[], &[]), None);
        // Equal strength: the lower id.
        let tie = [worked(2, &FSC1, 13.02), worked(5, &FSC1, 13.02)];
        let mut m = vec![vec![0.0; 9]; 9];
        m[2][5] = 0.9;
        assert_eq!(choose(&cards, &tie, &m).map(|c| c.id), Some(2));
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

    /// Particles packed side by side in a layer: the one at the centre and
    /// six about it, fainter (they do not stand at the same place in every
    /// member of the class). It ends where the gaps are, so it is not
    /// stretched; a slab runs on.
    #[test]
    fn a_particle_among_packed_neighbours_is_not_a_layer() {
        let around: Vec<(f64, f64)> = (0..6).map(|k| (60.0 * k as f64).to_radians()).map(|a| (11.0 * a.cos(), 11.0 * a.sin())).collect();
        let packed = dark(|x, y, z| blob(x, y, z / 1.3, 3.0) + around.iter().map(|(u, v)| 0.6 * blob(x - u, y - v, z / 1.3, 3.0)).sum::<f64>(), 0.03);
        let c = card(&packed, 50);
        report("packed layer", &c);
        assert!(c.elongation < ELONGATION, "{c:?}");
        let slab = card(&dark(|_, _, z| blob(0.0, 0.0, z, 2.5), 0.03), 50);
        assert!(slab.elongation >= ELONGATION, "{slab:?}");
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
        // Seven tomograms at different defocus: 0.355 at the bottom is no
        // limit; the halves agree to shell 29.
        let filled = [
            1.0, 1.0, 1.0, 0.998, 0.998, 0.996, 0.989, 0.989, 0.986, 0.973, 0.963, 0.956, 0.954, 0.954, 0.939, 0.919, 0.894, 0.832, 0.691, 0.461, 0.355, 0.466, 0.613, 0.647, 0.625,
            0.546, 0.383, 0.231, 0.147, 0.149, 0.115, 0.065, 0.017,
        ];
        let f = resolution(&filled);
        assert!(!f.dip && (f.limit - 29.2).abs() < 0.2 && f.signal == f.limit, "{f:?}");
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
        // The search grid: the look voxel × m nearest to 2 × the card's, the
        // raw search's voxel. The real case: raw 4.34 Å, look 8.68 Å, card 13.02 Å.
        for (px, card, m, voxel) in [(8.68, 13.02, 3, 26.04), (8.68, 17.36, 4, 34.72), (8.68, 8.68, 2, 17.36), (10.0, 12.0, 2, 20.0)] {
            let (bin, v) = look_search(px, card);
            assert_eq!(bin, m, "look {px}, card {card}");
            assert!((v - voxel).abs() < 1e-9, "look {px}, card {card}: {v}");
        }
        // A look voxel coarser than twice the card's: not binned.
        assert_eq!(look_search(40.0, 10.0), (1, 40.0));
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

#[cfg(test)]
mod polish_tests {
    use super::*;
    use crate::spa::{fourier, Rng};

    const N: usize = 64;
    const PX: f64 = 4.0;
    const RADIUS: f64 = 22.0;

    /// Multiplies the amplitudes by exp(−B/4·s²): a blur of B Å².
    fn blur(v: &[f32], b: f64) -> Vec<f32> {
        let (mut re, mut im) = fourier::ft3(N, v);
        for z in 0..N {
            for y in 0..N {
                for x in 0..N {
                    let (fx, fy, fz) = (fourier::freq(x, N), fourier::freq(y, N), fourier::freq(z, N));
                    let s = ((fx * fx + fy * fy + fz * fz) as f64).sqrt() / (N as f64 * PX);
                    let g = (-b / 4.0 * s * s).exp() as f32;
                    let j = (z * N + y) * N + x;
                    re[j] *= g;
                    im[j] *= g;
                }
            }
        }
        fourier::ift3(N, &re, &im)
    }

    fn masked(v: &[f32]) -> Vec<f32> {
        let mut v = v.to_vec();
        crate::spa::volume::mask_sphere(&mut v, N, RADIUS, 6.0);
        v
    }

    fn corr(a: &[f32], b: &[f32]) -> f64 {
        let (a, b) = (masked(a), masked(b));
        let (ma, mb) = (a.iter().sum::<f32>() as f64 / a.len() as f64, b.iter().sum::<f32>() as f64 / b.len() as f64);
        let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
        for (x, y) in a.iter().zip(&b) {
            let (x, y) = (*x as f64 - ma, *y as f64 - mb);
            ab += x * y;
            aa += x * x;
            bb += y * y;
        }
        ab / (aa * bb).sqrt()
    }

    fn halves(sigma: f64) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let mut rng = Rng(7);
        let phantom = masked(&(0..N * N * N).map(|_| rng.gauss() as f32).collect::<Vec<_>>());
        let blurred = blur(&phantom, 300.0);
        let half = |seed: u64| {
            let mut r = Rng(seed);
            blurred.iter().map(|v| v + (sigma * r.gauss()) as f32).collect::<Vec<_>>()
        };
        let (h1, h2) = (half(11), half(12));
        (phantom, h1, h2)
    }

    #[test]
    fn sharpening_undoes_the_blur_and_cuts_the_noise() {
        let (phantom, h1, h2) = halves(1.5);
        let fsc = crate::spa::post::corrected_fsc(&h1, &h2, &masked, N);
        let shells = resolution(&fsc);
        let cut = if shells.dip { shells.signal } else { shells.limit };
        eprintln!("limit {:.1} Å", N as f64 * PX / cut);
        let (out, b) = polish(&h1, &h2, &fsc, cut, N, PX, RADIUS);
        let mean: Vec<f32> = h1.iter().zip(&h2).map(|(a, c)| 0.5 * (a + c)).collect();
        let reference = crate::spa::refine::lowpass(&phantom, N, PX, N as f64 * PX / cut);
        let (before, after) = (corr(&mean, &reference), corr(&out, &reference));
        let b = b.expect("enough shells for a fit");
        eprintln!("corr {before:.3} -> {after:.3}, B {b:.0}");
        assert!(out.iter().all(|v| v.is_finite()));
        assert!(after > before, "{before} -> {after}");
        assert!((-450.0..=-200.0).contains(&b), "B {b} for an applied blur of 300");
    }

    #[test]
    fn at_30_a_it_stays_finite_and_skips_or_clamps() {
        let (_, h1, h2) = halves(4.0);
        // The FSC of a 30 Å map: good to shell 3, gone by shell 9.
        let fsc: Vec<f64> = (0..=N / 2).map(|r| (1.0 - (r as f64 - 3.0).max(0.0) / 6.0).clamp(0.0, 1.0)).collect();
        let cut = N as f64 * PX / 30.0;
        let (out, b) = polish(&h1, &h2, &fsc, cut, N, PX, RADIUS);
        assert!(out.iter().all(|v| v.is_finite()));
        assert_eq!(b, None, "four shells between 60 Å and 30 Å are too few");
        // 20 Å: the fit runs, and stays within the limits.
        let (out, b) = polish(&h1, &h2, &fsc, N as f64 * PX / 20.0, N, PX, RADIUS);
        assert!(out.iter().all(|v| v.is_finite()));
        assert!(b.is_none_or(|b| (SHARPEN_B_MIN..=0.0).contains(&b)), "{b:?}");
        // Nothing to cut at: the plain mean.
        assert_eq!(polish(&h1, &h2, &[], 0.0, N, PX, RADIUS).1, None);
    }
}

#[cfg(test)]
mod halves_tests {
    use super::halves_by_groups;

    /// Places along three separate lines: no line may give places to both halves.
    #[test]
    fn whole_shapes_go_to_one_half() {
        let mut at = vec![];
        let mut source = vec![];
        for (line, len) in [(0.0, 10), (100.0, 7), (200.0, 5)] {
            for k in 0..len {
                at.push([k as f64 * 2.0, line, 0.0]);
                source.push(0);
            }
        }
        let order = halves_by_groups(&at, &source, 3.0);
        let line_of = |i: usize| (at[i][1] / 100.0).round() as usize;
        let mut lines = [[false; 2]; 3];
        for (k, i) in order.iter().enumerate() {
            lines[line_of(*i)][k % 2] = true;
        }
        assert!(lines.iter().all(|l| !(l[0] && l[1])), "{lines:?}");
        // 10 against 7 + 5: each half keeps 10.
        assert_eq!(order.len(), 20);
    }

    /// One shape with nearly everything is cut by a plane: each half takes one side.
    #[test]
    fn one_large_shape_is_cut_in_two() {
        let at: Vec<[f64; 3]> = (0..40).map(|k| [k as f64, 0.0, 0.0]).collect();
        let order = halves_by_groups(&at, &vec![0; 40], 1.5);
        assert_eq!(order.len(), 40);
        let (a, b): (Vec<_>, Vec<_>) = order.iter().enumerate().partition(|(k, _)| k % 2 == 0);
        let side = |v: &Vec<(usize, &usize)>| v.iter().all(|(_, i)| **i < 20) || v.iter().all(|(_, i)| **i >= 20);
        assert!(side(&a) && side(&b));
    }

    /// The same place in two tomograms is not one group.
    #[test]
    fn sources_are_apart() {
        let at = vec![[0.0; 3], [0.0; 3]];
        let order = halves_by_groups(&at, &[0, 1], 5.0);
        assert_eq!(order.len(), 2);
    }
}

#[cfg(test)]
mod around_tests {
    use super::*;
    use crate::spa::geom::{distance, transpose};

    /// Heavy: `CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test --no-default-features --features cli --lib sta::project::around_tests -- --ignored --nocapture`.
    ///
    /// A ball that looks the same from every side, with three other blobs
    /// around it beyond its circle, nine times and every which way up. The
    /// circles are drawn six voxels off the ball. In cubes as wide as the
    /// circle there is nothing to turn the ball by; by the surroundings each
    /// stands as it was planted, and the cubes are cut again about the ball.
    #[test]
    #[ignore]
    fn what_is_around_a_thing_turns_it_the_right_way() {
        let dims = [448usize, 448, 128];
        let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
        let mut rng = Rng(0xa40d);
        // (centre, radius, weight) in the thing's own frame.
        let blobs: [([f64; 3], f64, f32); 4] = [([0.0, 0.0, 0.0], 12.0, 1.0), ([34.0, 0.0, -6.0], 11.0, 1.0), ([0.0, 22.0, -10.0], 6.0, 1.2), ([-20.0, -12.0, 18.0], 8.0, 0.9)];
        let mut things: Vec<([f64; 3], Mat)> = vec![];
        for j in 0..3 {
            for i in 0..3 {
                let axis = [rng.gauss(), rng.gauss(), rng.gauss()];
                things.push(([96.0 + 128.0 * i as f64, 96.0 + 128.0 * j as f64, 64.0], axis_angle(axis, 360.0 * rng.unit())));
            }
        }
        for (at, r) in &things {
            for (q, radius, weight) in &blobs {
                let c: [f64; 3] = std::array::from_fn(|a| at[a] + (0..3).map(|b| r[a][b] * q[b]).sum::<f64>());
                let h = radius.ceil() as isize;
                for dz in -h..=h {
                    for dy in -h..=h {
                        for dx in -h..=h {
                            let p = [c[0].round() as isize + dx, c[1].round() as isize + dy, c[2].round() as isize + dz];
                            if (0..3).all(|k| p[k] >= 0 && p[k] < dims[k] as isize) && ((dx * dx + dy * dy + dz * dz) as f64) <= radius * radius {
                                v[(p[2] as usize * dims[1] + p[1] as usize) * dims[0] + p[0] as usize] += weight;
                            }
                        }
                    }
                }
            }
        }
        v.iter_mut().for_each(|x| *x += (0.6 * rng.gauss()) as f32);
        let dir = std::env::temp_dir().join(format!("tmv-sta-around-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tomo.mrc").to_string_lossy().into_owned();
        output::write_mrc(&path, &v, dims, 10.0, false).unwrap();
        let sources = vec![Source { path: path.clone(), tilts: [-90.0, 90.0], look: None, series: None }];
        // Circles of radius 18 (360 Å, cubes of 20 Å voxels), six voxels off.
        let (voxel_a, diameter_a) = (20.0, 360.0);
        let (binned, bdims, _, bin) = binned(&path, voxel_a).unwrap();
        let mut cuts = vec![];
        for (at, _) in &things {
            let off = [rng.gauss(), rng.gauss(), rng.gauss()];
            let norm = off.iter().map(|x| x * x).sum::<f64>().sqrt();
            let (c, _) = on_grid(std::array::from_fn(|a| at[a] + 6.0 * off[a] / norm), bin);
            cuts.push(Cut { source: 0, at: c.map(|x| x as f64 * bin as f64 + (bin as f64 - 1.0) / 2.0), bin, score: 1.0, strength: 0.0, darkest: 0.0, cube: cube(&binned, bdims, c).unwrap() });
        }
        let poses = vec![Pose { r: super::super::IDENTITY, t: [0.0; 3], score: 1.0 }; cuts.len()];
        let far = |c: &Cut, k: usize| (0..3).map(|a| (c.at[a] - things[k].0[a]).powi(2)).sum::<f64>().sqrt();
        let before: Vec<f64> = cuts.iter().enumerate().map(|(k, c)| far(c, k)).collect();
        let started = Instant::now();
        let (again, found, around, wide_voxel) = by_surroundings(&burn::tensor::Device::flex(), &sources, &cuts, &poses, voxel_a, diameter_a, AROUND, &|_, _, _| {}, &|_| {}).unwrap().expect("surroundings");
        assert_eq!((again.len(), around.len(), wide_voxel), (things.len(), BOX * BOX * BOX, 40.0));
        // As planted, up to the one turn all share: a point m of the average
        // stands at R·m in the particle, a point of the thing at T·m, so
        // Tᵀ·R is the same for all. Each against the one most others agree with.
        let shared: Vec<Mat> = (0..things.len()).map(|k| mul(&transpose(&things[k].1), &found[k].r)).collect();
        let agree = |k: usize| shared.iter().filter(|s| distance(s, &shared[k]) < 30.0).count();
        let most = (0..shared.len()).max_by_key(|k| agree(*k)).unwrap();
        let turned: Vec<f64> = shared.iter().map(|s| distance(s, &shared[most])).collect();
        let after: Vec<f64> = again.iter().enumerate().map(|(k, c)| far(c, k)).collect();
        eprintln!("{:?}: turned {turned:.0?}°, off the ball before {before:.1?}, after {after:.1?}", started.elapsed());
        assert!(turned.iter().all(|d| *d < 15.0), "{turned:.0?}");
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        assert!(mean(&after) < 0.7 * mean(&before), "{after:.1?} against {before:.1?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Heavy, as above.
    ///
    /// A small ring of blobs on the shell of a large ball, twelve times and
    /// every which way up, with the turn about the ring's axis random: what
    /// can be known of each is its axis, and the ball's shell gives it.
    #[test]
    #[ignore]
    fn a_thing_on_a_ball_is_stood_up_by_the_ball() {
        let dims = [512usize, 384, 160];
        let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
        let mut rng = Rng(0xba11);
        let mut things: Vec<([f64; 3], [f64; 3])> = vec![];
        for j in 0..3 {
            for i in 0..4 {
                let a = [rng.gauss(), rng.gauss(), rng.gauss()];
                let d = a.iter().map(|x| x * x).sum::<f64>().sqrt();
                things.push(([64.0 + 128.0 * i as f64, 64.0 + 128.0 * j as f64, 80.0], a.map(|x| x / d)));
            }
        }
        let mut put = |c: [f64; 3], radius: f64, shell: Option<f64>, weight: f32| {
            let h = radius.ceil() as isize + 1;
            for dz in -h..=h {
                for dy in -h..=h {
                    for dx in -h..=h {
                        let p = [c[0].round() as isize + dx, c[1].round() as isize + dy, c[2].round() as isize + dz];
                        let r = ((dx * dx + dy * dy + dz * dz) as f64).sqrt();
                        let inside = match shell { Some(t) => (r - radius).abs() <= t, None => r <= radius };
                        if inside && (0..3).all(|k| p[k] >= 0 && p[k] < dims[k] as isize) {
                            v[(p[2] as usize * dims[1] + p[1] as usize) * dims[0] + p[0] as usize] += weight;
                        }
                    }
                }
            }
        };
        for (at, a) in &things {
            // The ball (radius 40) under the thing, its shell through it.
            put(std::array::from_fn(|k| at[k] - 40.0 * a[k]), 40.0, Some(1.5), 1.0);
            // A ring of six blobs about the axis, a turn of its own, and a knob on top.
            let f = super::super::shape::frame(*a);
            let turn = 360.0 * rng.unit();
            for b in 0..6 {
                let t = (turn + 60.0 * b as f64).to_radians();
                let q = [7.0 * t.cos(), 7.0 * t.sin(), 3.0];
                put(std::array::from_fn(|k| at[k] + (0..3).map(|m| f[k][m] * q[m]).sum::<f64>()), 2.5, None, 1.2);
            }
            put(std::array::from_fn(|k| at[k] + 8.0 * a[k]), 3.0, None, 1.0);
        }
        v.iter_mut().for_each(|x| *x = -*x + (0.8 * rng.gauss()) as f32);
        let dir = std::env::temp_dir().join(format!("tmv-sta-onball-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tomo.mrc").to_string_lossy().into_owned();
        output::write_mrc(&path, &v, dims, 10.0, false).unwrap();
        let sources = vec![Source { path: path.clone(), tilts: [-90.0, 90.0], look: None, series: None }];
        let (voxel_a, diameter_a) = (20.0, 360.0);
        let (binned, bdims, _, bin) = binned(&path, voxel_a).unwrap();
        let cuts: Vec<Cut> = things
            .iter()
            .map(|(at, _)| {
                let (c, _) = on_grid(*at, bin);
                Cut { source: 0, at: c.map(|x| x as f64 * bin as f64 + (bin as f64 - 1.0) / 2.0), bin, score: 1.0, strength: 0.0, darkest: 0.0, cube: cube(&binned, bdims, c).unwrap() }
            })
            .collect();
        let poses = vec![Pose { r: super::super::IDENTITY, t: [0.0; 3], score: 1.0 }; cuts.len()];
        let (_, found, _, _) = by_surroundings(&burn::tensor::Device::flex(), &sources, &cuts, &poses, voxel_a, diameter_a, AROUND, &|_, _, _| {}, &|_| {}).unwrap().expect("surroundings");
        // The axis of the average is one direction m for all: each particle's
        // R carries it onto its planted axis, so Rᵀ·a is the same for all (up to sense).
        let back: Vec<[f64; 3]> = found.iter().zip(&things).map(|(p, (_, a))| std::array::from_fn(|i| (0..3).map(|j| p.r[j][i] * a[j]).sum::<f64>())).collect();
        let near = |k: usize| back.iter().filter(|b| super::super::axial::apart(**b, back[k]) < 20.0).count();
        let most = (0..back.len()).max_by_key(|k| near(*k)).unwrap();
        let off: Vec<f64> = back.iter().map(|b| super::super::axial::apart(*b, back[most])).collect();
        eprintln!("axes off the shared one: {off:.0?}°");
        assert!(off.iter().filter(|d| **d < 20.0).count() >= 10, "{off:.0?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
