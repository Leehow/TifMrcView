//! The average of the membrane itself: what a membrane looks like across
//! (its thickness, its two leaflets, which side is denser), from the
//! membranes the user labelled with the label brush (单层膜 / 双层膜
//! layers, `layers`).
//!
//! The layers hold what the user and the label tools made of it, and not
//! all of that is membrane: a line carried along Z can drift onto
//! something else, a found one can be a rim or a streak. So the
//! membranes are cut into small patches and every patch is judged by how
//! its profile across the membrane agrees with the common one of all the
//! others: patches that do not look like the membrane are left out (and
//! said why), the common profile is made again from the rest, and that is
//! done until it settles. Nothing is thrown away silently: every patch,
//! kept or not, is in `patches.csv` with its place on the image the user
//! labelled, and every membrane has its count of kept and left-out
//! patches, so the user can see which membranes (or which stretch of one)
//! were wrong.
//!
//! The steps reuse what the drawn shapes have: the layers become surface
//! marks (`layers::marks_of`), the marks seeds on the raw tomogram with
//! their normals (`marks::drawn_of`, carried over from the look file),
//! the seeds are put on the layer that is really there (`flat::snap`), and
//! the average patch is cut unbent along the surface (`flat::patches`,
//! `flat::cut`). A membrane has no direction in its own plane, so only the
//! place along the normal is aligned; nothing is turned.
//!
//! Everything is measured on the raw tomogram (never on the clear look
//! file, which is denoised and deconvolved and would change the profile).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::Serialize;

use super::flat;
use super::marks::{self, Kind, Mark, Size};
use super::project::{binned, Drawn, Source};
use super::refine::Progress;
use super::shape::{frame, Seed};
use crate::tomo::output;

type P = [f64; 3];

/// Patches are this far apart along the membrane, Å.
pub const SPACING_A: f64 = 60.0;
/// Each profile is averaged over a disc of this radius in the membrane's
/// plane, Å.
const DISC_A: f64 = 30.0;
/// The profile reaches this far to either side of the membrane, Å.
const REACH_A: f64 = 150.0;
/// A patch is compared with the common profile over this much to either
/// side of the membrane's middle, Å (the membrane, not its surroundings,
/// which differ from place to place).
const JUDGE_A: f64 = 60.0;
/// A patch may move at most this far along its normal to agree, Å.
const SLIDE_A: f64 = 25.0;
/// Edge of the average patch, Å.
pub const BOX_A: f64 = 240.0;
/// Coarse voxels still need room: the slide and the judged part are at
/// least this many voxels (a line drawn on a 30 Å section is easily one or
/// two voxels off the membrane's middle).
const FEWEST_VOXELS: f64 = 3.0;

/// The slide, the judged half width and the profile's half length, voxels.
fn reaches(voxel: f64) -> (f64, f64, f64) {
    let slide = (SLIDE_A / voxel).max(FEWEST_VOXELS);
    let judge = (JUDGE_A / voxel).max(FEWEST_VOXELS);
    (slide, judge, (REACH_A / voxel).max(judge + slide + 2.0))
}

/// Profile step, voxels.
const STEP: f64 = 0.5;
/// Rounds of judging and averaging again.
const ROUNDS: usize = 4;
/// A patch is left out when its agreement lies this many robust spreads
/// below the median of all.
const SPREADS: f64 = 2.5;
/// ... or is below this whatever the others do.
const FLOOR: f64 = 0.2;
/// A patch whose membrane faces the beam this much or more is hidden by
/// the missing wedge (`flat::shows` below this) and left out.
const HIDDEN: f64 = 0.25;

/// Why a patch is not in the average.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Out {
    /// Kept.
    Kept,
    /// Its profile does not look like the membrane's (wrong place, other
    /// structure, noise).
    Unlike,
    /// Agreed only after sliding as far as allowed: the line is off the
    /// membrane there.
    Off,
    /// The membrane faces the beam there; the missing wedge hides it.
    Hidden,
    /// Too near the tomogram's edge to measure.
    Edge,
    /// The user took the whole membrane out.
    Dropped,
}

/// One patch.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    /// Index into `Report::membranes`.
    pub membrane: usize,
    /// Place on the raw tomogram, voxels (x, y, z), after the slide.
    pub raw: P,
    /// Place on the image the membrane was labelled on, voxels.
    pub image: P,
    /// Outward normal (towards the side the user named outside).
    pub normal: P,
    /// Agreement with the common profile, −1..1.
    pub score: f64,
    /// How far it slid along the normal, Å.
    pub slide_a: f64,
    pub out: Out,
}

/// One membrane (one sheet of a layer, joined across sections).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Membrane {
    /// The image it was labelled on.
    pub image: String,
    pub label: u16,
    pub layer: String,
    /// Sections it spans on that image.
    pub z: [f64; 2],
    pub patches: usize,
    pub kept: usize,
    pub unlike: usize,
    pub off: usize,
    pub hidden: usize,
    /// Median agreement of its measured patches.
    pub score: f64,
    /// The user took it out / turned it round (`Edits`).
    pub dropped: bool,
    pub flipped: bool,
    /// Its kept patches agree better with the common profile turned round:
    /// inside and outside are probably swapped on this membrane.
    pub side_doubt: bool,
    /// Its own profile (kept patches; all measured ones when none was kept),
    /// on the offsets of `Report::profile`.
    pub profile: Vec<f64>,
}

/// What `run` gives (also `membrane.json`).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub dir: String,
    /// Voxel of the measurements, Å.
    pub voxel_a: f64,
    /// Offsets of the profile from the membrane's middle, Å (negative:
    /// inside, positive: outside).
    pub offsets_a: Vec<f64>,
    /// The common profile of the kept patches (zero mean, unit spread over
    /// the membrane part).
    pub profile: Vec<f64>,
    /// Whether the membrane is darker (−1) or brighter (+1) than around it
    /// in the tomogram.
    pub dense: i8,
    /// Width of the membrane at half its height above the surroundings, Å.
    pub thickness_a: Option<f64>,
    /// Distance between the two leaflets' head groups when the profile
    /// shows them apart, Å.
    pub leaflets_a: Option<f64>,
    /// Which side is denser next to the membrane: "inside", "outside" or
    /// "even".
    pub denser_side: String,
    pub patches: usize,
    pub kept: usize,
    pub membranes: Vec<Membrane>,
    /// The average patch (unbent, the membrane flat in X/Y, outside up in Z).
    pub map: Option<String>,
    /// The average made again from the tilt images themselves, at their
    /// own pixel (`project::average_from_tilts`): what the membrane looks
    /// like at the detail the tomogram's coarse voxel cannot show.
    pub fine: Option<Fine>,
    pub seconds: f64,
}

/// The kept patches averaged from the tilt images.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fine {
    pub voxel_a: f64,
    pub offsets_a: Vec<f64>,
    pub profile: Vec<f64>,
    pub dense: i8,
    pub thickness_a: Option<f64>,
    pub leaflets_a: Option<f64>,
    pub denser_side: String,
    /// Where the halves' FSC falls below 0.143, Å (halves alternate along
    /// the membranes, so neighbouring patches can share noise: optimistic).
    pub resolution_a: Option<f64>,
    pub map: String,
}

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn col(m: &crate::spa::geom::Mat, k: usize) -> P {
    [m[0][k], m[1][k], m[2][k]]
}

/// The volume at `p` (x, y, z) between voxels; None outside.
fn sample(v: &[f32], dims: [usize; 3], p: P) -> Option<f32> {
    let f = p.map(|x| x.floor());
    if (0..3).any(|k| f[k] < 0.0 || f[k] + 1.0 > (dims[k] - 1) as f64) {
        return None;
    }
    let (ix, iy, iz) = (f[0] as usize, f[1] as usize, f[2] as usize);
    let (tx, ty, tz) = ((p[0] - f[0]) as f32, (p[1] - f[1]) as f32, (p[2] - f[2]) as f32);
    let at = |dx: usize, dy: usize, dz: usize| v[((iz + dz) * dims[1] + iy + dy) * dims[0] + ix + dx];
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    Some(lerp(
        lerp(lerp(at(0, 0, 0), at(1, 0, 0), tx), lerp(at(0, 1, 0), at(1, 1, 0), tx), ty),
        lerp(lerp(at(0, 0, 1), at(1, 0, 1), tx), lerp(at(0, 1, 1), at(1, 1, 1), tx), ty),
        tz,
    ))
}

/// The profile across the membrane at a seed: the volume along the normal
/// from −`half` to +`half` steps (`STEP` voxels each), averaged over a disc
/// of `disc` voxels in the membrane's plane. None near the edge.
pub fn profile(v: &[f32], dims: [usize; 3], s: &Seed, disc: f64, half: isize) -> Option<Vec<f64>> {
    let f = frame(s.axis);
    let (e1, e2, n) = (col(&f, 0), col(&f, 1), col(&f, 2));
    let r = disc.ceil() as i32;
    let spots: Vec<(f64, f64)> =
        (-r..=r).flat_map(|y| (-r..=r).map(move |x| (x as f64, y as f64))).filter(|(x, y)| x * x + y * y <= disc * disc + 1e-9).collect();
    let mut out = Vec::with_capacity((2 * half + 1) as usize);
    for j in -half..=half {
        let off = j as f64 * STEP;
        let (mut sum, mut count) = (0f64, 0usize);
        for (x, y) in &spots {
            if let Some(value) = sample(v, dims, std::array::from_fn(|k| s.pos[k] + x * e1[k] + y * e2[k] + off * n[k])) {
                sum += value as f64;
                count += 1;
            }
        }
        if count * 2 < spots.len() {
            return None;
        }
        out.push(sum / count as f64);
    }
    Some(out)
}

/// Normalised correlation of `a` and `b` (same length).
fn ncc(a: &[f64], b: &[f64]) -> f64 {
    let k = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / k, b.iter().sum::<f64>() / k);
    let (mut d, mut sa, mut sb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        d += (x - ma) * (y - mb);
        sa += (x - ma).powi(2);
        sb += (y - mb).powi(2);
    }
    if sa > 0.0 && sb > 0.0 { d / (sa * sb).sqrt() } else { 0.0 }
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// The judging: each profile (all `2·half + 1` long) is slid by at most
/// `slide` steps until its middle `judge` steps to either side agree best
/// with the common middle of the kept ones; patches far below the others
/// or at the end of the slide are left out, and that is repeated. Returns
/// per profile (best slide in steps, agreement, kept?, slid to the end?)
/// and the common profile over the whole length (the kept ones slid).
pub fn judge(profiles: &[Option<Vec<f64>>], weights: &[f64], half: isize, judge: isize, slide: isize) -> (Vec<(isize, f64, bool, bool)>, Vec<f64>) {
    let len = (2 * half + 1) as usize;
    let m = profiles.len();
    let mut state: Vec<(isize, f64, bool, bool)> = profiles.iter().zip(weights).map(|(p, w)| (0, 0.0, p.is_some() && *w > 0.0, false)).collect();
    let mut common = vec![0f64; len];
    // The part of a profile slid by s that is compared: offsets −judge..judge.
    let part = |p: &[f64], s: isize| -> Vec<f64> { (-judge..=judge).map(|j| p[(half + j + s) as usize]).collect() };
    let all = |state: &[(isize, f64, bool, bool)]| -> Vec<f64> {
        let mut c = vec![0f64; len];
        let mut w = vec![0f64; len];
        for i in 0..m {
            let (s, score, keep, _) = state[i];
            let Some(p) = &profiles[i] else { continue };
            if !keep {
                continue;
            }
            let wt = weights[i] * score.max(0.05);
            for k in 0..len as isize {
                let from = k + s;
                if from >= 0 && from < len as isize {
                    c[k as usize] += wt * p[from as usize];
                    w[k as usize] += wt;
                }
            }
        }
        c.iter().zip(&w).map(|(c, w)| if *w > 0.0 { c / w } else { 0.0 }).collect()
    };
    // First common profile: everything, as it lies.
    for st in state.iter_mut() {
        st.1 = 1.0;
    }
    common = all(&state);
    for _ in 0..ROUNDS {
        let mid = part(&common, 0);
        let scored: Vec<(isize, f64, bool)> = profiles
            .par_iter()
            .map(|p| {
                let Some(p) = p else { return (0, 0.0, false) };
                let mut best = (0isize, f64::NEG_INFINITY);
                for s in -slide..=slide {
                    if half + s - judge < 0 || half + s + judge >= len as isize {
                        continue;
                    }
                    let c = ncc(&part(p, s), &mid);
                    if c > best.1 {
                        best = (s, c);
                    }
                }
                (best.0, best.1, best.0.abs() >= slide)
            })
            .collect();
        let mut measured: Vec<f64> = (0..m).filter(|&i| profiles[i].is_some() && weights[i] > 0.0).map(|i| scored[i].1).collect();
        let med = median(&mut measured);
        let mut dev: Vec<f64> = measured.iter().map(|x| (x - med).abs()).collect();
        let spread = 1.4826 * median(&mut dev);
        let limit = (med - SPREADS * spread).max(FLOOR);
        let before: Vec<bool> = state.iter().map(|s| s.2).collect();
        for i in 0..m {
            let ok = profiles[i].is_some() && weights[i] > 0.0;
            let (s, c, end) = scored[i];
            state[i] = (s, c, ok && c >= limit && !end, end);
        }
        common = all(&state);
        if before == state.iter().map(|s| s.2).collect::<Vec<_>>() {
            break;
        }
    }
    (state, common)
}

/// What a common profile says: whether the membrane is dark or bright, its
/// width at half height, the leaflets' distance when two peaks show, and
/// which side is denser. `offsets` in Å, the membrane's middle at 0.
pub fn read_profile(offsets: &[f64], p: &[f64]) -> (i8, Option<f64>, Option<f64>, String) {
    let n = p.len();
    if n < 5 {
        return (0, None, None, "even".into());
    }
    // The surroundings: beyond 2/3 of the reach on either side.
    let far = 0.66 * offsets[n - 1].abs();
    let mut outer: Vec<f64> = (0..n).filter(|&k| offsets[k].abs() >= far).map(|k| p[k]).collect();
    let base = median(&mut outer);
    let near: Vec<usize> = (0..n).filter(|&k| offsets[k].abs() <= JUDGE_A).collect();
    let peak = *near.iter().max_by(|a, b| (p[**a] - base).abs().total_cmp(&(p[**b] - base).abs())).unwrap();
    let dense: i8 = if p[peak] < base { -1 } else { 1 };
    let d: Vec<f64> = p.iter().map(|x| dense as f64 * (x - base)).collect();
    let top = d[peak];
    // Width at half height, between the crossings either side of the peak.
    let half = 0.5 * top;
    let cross = |dir: isize| -> Option<f64> {
        let mut k = peak as isize;
        while k + dir >= 0 && (k + dir) < n as isize {
            let (a, b) = (d[k as usize], d[(k + dir) as usize]);
            if b < half {
                let t = (a - half) / (a - b);
                return Some(offsets[k as usize] + t * (offsets[(k + dir) as usize] - offsets[k as usize]));
            }
            k += dir;
        }
        None
    };
    let thickness = cross(-1).zip(cross(1)).map(|(a, b)| b - a);
    // Two leaflets: the two highest local maxima within the membrane part,
    // with a dip between them of at least a tenth of the lower one.
    let mut tops: Vec<usize> = (1..n - 1).filter(|&k| offsets[k].abs() <= JUDGE_A && d[k] > d[k - 1] && d[k] >= d[k + 1] && d[k] > 0.3 * top).collect();
    tops.sort_by(|a, b| d[*b].total_cmp(&d[*a]));
    let leaflets = (tops.len() >= 2)
        .then(|| {
            let (a, b) = (tops[0].min(tops[1]), tops[0].max(tops[1]));
            let dip = (a..=b).map(|k| d[k]).fold(f64::INFINITY, f64::min);
            (d[a].min(d[b]) - dip >= 0.1 * d[a].min(d[b])).then(|| offsets[b] - offsets[a])
        })
        .flatten();
    // The side: density beside the membrane (beyond its half width, within
    // the judged part and a bit), inside vs outside.
    let w = thickness.unwrap_or(40.0) / 2.0;
    let side = |sign: f64| -> f64 {
        let ks: Vec<usize> = (0..n).filter(|&k| sign * offsets[k] > w && sign * offsets[k] <= w + 60.0).collect();
        ks.iter().map(|&k| d[k]).sum::<f64>() / ks.len().max(1) as f64
    };
    let (inside, outside) = (side(-1.0), side(1.0));
    let denser = if (inside - outside).abs() < 0.1 * top {
        "even"
    } else if inside > outside {
        "inside"
    } else {
        "outside"
    };
    (dense, thickness, leaflets, denser.into())
}

/// One membrane to measure: its mark (a surface in voxels of `image`) and
/// where it came from.
pub struct Given {
    pub mark: Mark,
    /// Turned round by the user (the mark's `flip` already says so).
    pub flipped: bool,
    pub image: String,
    pub label: u16,
    pub layer: String,
}

/// The membranes of the ticked label layers ([image, label] pairs) as surface
/// marks, each with its layer.
pub fn given(ticked: &[(String, u16)]) -> Result<Vec<Given>> {
    let mut out = vec![];
    for (image, label) in ticked {
        let names = super::layers::layers_of(std::slice::from_ref(image));
        let layer = names.iter().find(|l| l.label == *label).map(|l| l.name.clone()).unwrap_or_else(|| format!("标签 {label}"));
        let structure = names.iter().find(|l| l.label == *label).map(|l| l.structure.clone()).unwrap_or_default();
        if !structure.starts_with("membrane") {
            bail!("图层「{layer}」不是膜（单层膜或双层膜）");
        }
        for mark in super::layers::marks_of(&[(image.clone(), *label)])? {
            if mark.kind == Kind::Surface {
                out.push(Given { mark, flipped: false, image: image.clone(), label: *label, layer: layer.clone() });
            }
        }
    }
    if out.is_empty() {
        bail!("勾选的图层里没有膜");
    }
    Ok(out)
}

/// What the user changed on the check list: membranes (indices of `given`)
/// turned round or taken out.
#[derive(Clone, Debug, Default, serde::Deserialize, Serialize)]
#[serde(default)]
pub struct Edits {
    pub flip: Vec<usize>,
    pub drop: Vec<usize>,
}

/// The workspace's run: the ticked label layers ([image, label]; the image is
/// a tomogram or its clear companion), averaged into `<out>/membrane/`. Gives
/// the report and every patch as [membrane, x, y, z, result] on its image
/// (result: 0 kept, 1 unlike, 2 off, 3 hidden, 4 edge, 5 dropped). The
/// report and patches are also saved (`membrane.json`, `patches.csv`).
pub fn run_ticked(ticked: &[(String, u16)], edits: &Edits, out: &str, progress: Progress) -> Result<serde_json::Value> {
    let mut given = given(ticked)?;
    for &k in &edits.flip {
        if let Some(g) = given.get_mut(k) {
            g.mark.flip = !g.mark.flip;
            g.flipped = true;
        }
    }
    let mut inputs: Vec<String> = vec![];
    for (image, _) in ticked {
        if !inputs.contains(image) {
            inputs.push(image.clone());
        }
    }
    let sources: Vec<Source> = inputs
        .iter()
        .map(|i| {
            let tomogram = super::auto::raw_of_look(i)?.unwrap_or_else(|| i.clone());
            let dataset = super::auto::dataset_of(&tomogram);
            Ok(super::auto::resolve(i, tomogram, dataset, &|_| {}).source())
        })
        .collect::<Result<_>>()?;
    let report = run(&sources, &inputs, &given, &edits.drop, out, progress)?;
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(Path::new(&report.dir).join("patches.csv"))
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            // From the end: a layer's name may hold commas.
            let n = f.len();
            let num = |k: usize| f.get(k).and_then(|v| v.parse::<f64>().ok());
            if n < 14 {
                return None;
            }
            let code = match *f.last()? {
                "kept" => 0,
                "unlike" => 1,
                "off" => 2,
                "hidden" => 3,
                "edge" => 4,
                _ => 5,
            };
            Some(serde_json::json!([num(0)? as usize - 1, num(n - 12)?, num(n - 11)?, num(n - 10)?, code]))
        })
        .collect();
    std::fs::write(Path::new(&report.dir).join("edits.json"), serde_json::to_vec(edits)?)?;
    let mut v = serde_json::to_value(&report)?;
    v["patchList"] = serde_json::Value::Array(rows);
    Ok(v)
}

/// Raw tomogram voxel → voxel of the image `image` (a look file of the
/// source or the tomogram itself).
fn to_image(p: P, source: &Source, image: &str) -> P {
    match &source.look {
        Some(l) if l.path == image || Path::new(&l.path).file_name() == Path::new(image).file_name() => {
            std::array::from_fn(|k| (p[k] - l.offset[k]) / l.scale)
        }
        _ => p,
    }
}

/// Measures and averages the membranes `given` (in the tomograms
/// `sources`; `inputs` are the images as the user gave them, in the same
/// order) into `<out_dir>/membrane/`.
pub fn run(sources: &[Source], inputs: &[String], given: &[Given], dropped: &[usize], out_dir: &str, progress: Progress) -> Result<Report> {
    let started = std::time::Instant::now();
    let dir = PathBuf::from(out_dir).join("membrane");
    std::fs::create_dir_all(&dir).with_context(|| format!("建不了文件夹 {}", dir.display()))?;
    // Seeds of every membrane on the raw tomograms.
    let pixels: Vec<f64> = sources
        .iter()
        .map(|s| crate::tomo::pipeline::open(&s.path).map(|img| output::pixel_a(&img)))
        .collect::<Result<_>>()?;
    let mut seeds: Vec<(usize, Drawn)> = vec![];
    for (k, g) in given.iter().enumerate() {
        progress("膜上取点", k, given.len());
        let (drawn, _, _) = marks::drawn_of(std::slice::from_ref(&g.mark), inputs, sources, &pixels, Size::Custom((2.0 * SPACING_A) as u32))?;
        seeds.extend(drawn.into_iter().map(|d| (k, d)));
    }
    if seeds.is_empty() {
        bail!("膜上没有取到点");
    }
    let mut patches: Vec<Patch> = vec![];
    let mut profiles: Vec<Option<Vec<f64>>> = vec![];
    let mut weights: Vec<f64> = vec![];
    let mut voxel_a = 0.0;
    let mut half = 0isize;
    let mut cubes_from: Vec<(usize, Seed)> = vec![];
    // Each tomogram once: snap its seeds, measure their profiles.
    for (si, s) in sources.iter().enumerate() {
        let mine: Vec<usize> = (0..seeds.len()).filter(|&i| seeds[i].1.source == si).collect();
        if mine.is_empty() {
            continue;
        }
        progress("读断层图", si, sources.len());
        // The tomogram's own voxel, binned only when it is very large.
        let img = crate::tomo::pipeline::open(&s.path)?;
        let info = img.info();
        let px = pixels[si];
        let bytes = 4.0 * info.width as f64 * info.height as f64 * info.size_z as f64;
        let bin = ((bytes / 6e9).cbrt().ceil() as usize).max(1);
        drop(img);
        let (v, dims, voxel, bin) = binned(&s.path, px * bin as f64)?;
        voxel_a = voxel;
        let (range, _, reach) = reaches(voxel);
        half = (reach / STEP).round() as isize;
        let disc = DISC_A / voxel;
        let here: Vec<Seed> = mine
            .iter()
            .map(|&i| {
                let d = &seeds[i].1;
                Seed { pos: d.pos.map(|p| (p - (bin as f64 - 1.0) / 2.0) / bin as f64), axis: d.axis }
            })
            .collect();
        // Snap each membrane on its own (two membranes near each other must
        // not be fitted as one surface).
        let mut snapped = here.clone();
        let mut by: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
        for (j, &i) in mine.iter().enumerate() {
            by.entry(seeds[i].0).or_default().push(j);
        }
        for js in by.values() {
            let part: Vec<Seed> = js.iter().map(|&j| here[j]).collect();
            let moved = flat::snap(&v, dims, &part, disc, range, 3.0 * SPACING_A / voxel, s.tilts);
            for (j, m) in js.iter().zip(moved) {
                snapped[*j] = m;
            }
        }
        progress("量膜的剖面", si, sources.len());
        let measured: Vec<Option<Vec<f64>>> = snapped.par_iter().map(|sd| profile(&v, dims, sd, disc, half)).collect();
        for (j, &i) in mine.iter().enumerate() {
            let (m, _) = seeds[i];
            let sd = snapped[j];
            let shows = flat::shows(sd.axis, s.tilts[0], s.tilts[1]);
            let raw: P = sd.pos.map(|p| p * bin as f64 + (bin as f64 - 1.0) / 2.0);
            patches.push(Patch {
                membrane: m,
                raw,
                image: to_image(raw, s, &given[m].image),
                normal: sd.axis,
                score: 0.0,
                slide_a: 0.0,
                out: if dropped.contains(&m) { Out::Dropped } else if measured[j].is_none() { Out::Edge } else if shows < HIDDEN { Out::Hidden } else { Out::Kept },
            });
            weights.push(if shows < HIDDEN || dropped.contains(&m) { 0.0 } else { shows });
            profiles.push(measured[j].clone());
            cubes_from.push((si, sd));
        }
    }
    if half == 0 {
        bail!("膜上没有取到点");
    }
    progress("判断每一小片是不是膜", 0, 1);
    let (slide_v, judge_v, _) = reaches(voxel_a);
    let judged = (judge_v / STEP).round() as isize;
    let slide = (slide_v / STEP).round().max(1.0) as isize;
    let (state, common) = judge(&profiles, &weights, half, judged, slide);
    for (p, &(s, score, keep, end)) in patches.iter_mut().zip(&state) {
        p.score = score;
        p.slide_a = s as f64 * STEP * voxel_a;
        if p.out == Out::Kept && !keep {
            p.out = if end { Out::Off } else { Out::Unlike };
        }
        p.raw = std::array::from_fn(|k| p.raw[k] + p.slide_a / voxel_a * p.normal[k]);
    }
    for (i, p) in patches.iter_mut().enumerate() {
        let (si, _) = cubes_from[i];
        p.image = to_image(p.raw, &sources[si], &given[p.membrane].image);
    }
    let offsets: Vec<f64> = (-half..=half).map(|j| j as f64 * STEP * voxel_a).collect();
    // The common profile, scaled over the membrane part.
    let judge_a = judge_v * voxel_a;
    let mid: Vec<f64> = (0..offsets.len()).filter(|&k| offsets[k].abs() <= judge_a).map(|k| common[k]).collect();
    let mean = mid.iter().sum::<f64>() / mid.len().max(1) as f64;
    let sd = (mid.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / mid.len().max(1) as f64).sqrt().max(1e-12);
    let profile_n: Vec<f64> = common.iter().map(|x| (x - mean) / sd).collect();
    let (dense, thickness_a, leaflets_a, denser_side) = read_profile(&offsets, &profile_n);
    // Per membrane.
    let mut membranes: Vec<Membrane> = given
        .iter()
        .enumerate()
        .map(|(m, g)| {
            let zs: Vec<f64> = g.mark.points.iter().map(|p| p[2]).collect();
            Membrane {
                image: g.image.clone(),
                label: g.label,
                layer: g.layer.clone(),
                z: [zs.iter().copied().fold(f64::INFINITY, f64::min), zs.iter().copied().fold(f64::NEG_INFINITY, f64::max)],
                patches: 0,
                kept: 0,
                unlike: 0,
                off: 0,
                hidden: 0,
                score: 0.0,
                dropped: dropped.contains(&m),
                flipped: g.flipped,
                side_doubt: false,
                profile: vec![],
            }
        })
        .collect();
    let judged_mid: Vec<usize> = (0..offsets.len()).filter(|&k| offsets[k].abs() <= judge_a).collect();
    for (m, mem) in membranes.iter_mut().enumerate() {
        let mine: Vec<usize> = (0..patches.len()).filter(|&i| patches[i].membrane == m).collect();
        mem.patches = mine.len();
        for &i in &mine {
            match patches[i].out {
                Out::Kept => mem.kept += 1,
                Out::Unlike => mem.unlike += 1,
                Out::Off => mem.off += 1,
                Out::Hidden => mem.hidden += 1,
                Out::Edge | Out::Dropped => {}
            }
        }
        let measured: Vec<usize> = mine.iter().copied().filter(|&i| matches!(patches[i].out, Out::Kept | Out::Unlike | Out::Off) || (patches[i].out == Out::Dropped && profiles[i].is_some())).collect();
        mem.score = median(&mut measured.iter().map(|&i| patches[i].score).collect::<Vec<_>>());
        let use_: Vec<usize> = if mine.iter().any(|&i| patches[i].out == Out::Kept) {
            mine.iter().copied().filter(|&i| patches[i].out == Out::Kept).collect()
        } else {
            measured
        };
        if use_.is_empty() {
            continue;
        }
        let len = offsets.len();
        let mut own = vec![0f64; len];
        let mut w = vec![0f64; len];
        for &i in &use_ {
            let Some(p) = &profiles[i] else { continue };
            let s = state[i].0;
            for k in 0..len as isize {
                let from = k + s;
                if from >= 0 && from < len as isize {
                    own[k as usize] += p[from as usize];
                    w[k as usize] += 1.0;
                }
            }
        }
        let own: Vec<f64> = own.iter().zip(&w).map(|(a, b)| if *b > 0.0 { a / b } else { 0.0 }).collect();
        let a: Vec<f64> = judged_mid.iter().map(|&k| own[k]).collect();
        let c: Vec<f64> = judged_mid.iter().map(|&k| common[k]).collect();
        let c_turned: Vec<f64> = judged_mid.iter().rev().map(|&k| common[k]).collect();
        mem.side_doubt = use_.len() >= 3 && ncc(&a, &c_turned) > ncc(&a, &c) + 0.1;
        let m0 = a.iter().sum::<f64>() / a.len().max(1) as f64;
        let s0 = (a.iter().map(|x| (x - m0).powi(2)).sum::<f64>() / a.len().max(1) as f64).sqrt().max(1e-12);
        mem.profile = own.iter().map(|x| (x - m0) / s0).collect();
    }
    // The average patch, cut unbent along each kept patch's own membrane.
    progress("平均膜片", 0, 1);
    let n = (((BOX_A / voxel_a).round() as usize) / 2 * 2).clamp(16, 96);
    let mut sum = vec![0f64; n * n * n];
    let mut count = 0usize;
    for (si, s) in sources.iter().enumerate() {
        let kept: Vec<usize> = (0..patches.len()).filter(|&i| cubes_from[i].0 == si && patches[i].out == Out::Kept).collect();
        if kept.is_empty() {
            continue;
        }
        let (v, dims, _, bin) = binned(&s.path, voxel_a)?;
        for m in 0..given.len() {
            let mine: Vec<usize> = kept.iter().copied().filter(|&i| patches[i].membrane == m).collect();
            let seeds_m: Vec<Seed> = mine
                .iter()
                .map(|&i| Seed { pos: patches[i].raw.map(|p| (p - (bin as f64 - 1.0) / 2.0) / bin as f64), axis: patches[i].normal })
                .collect();
            let cut: Vec<Vec<f32>> = flat::patches(&seeds_m, 3.0 * SPACING_A / voxel_a)
                .par_iter()
                .filter_map(|p| flat::cut(&v, dims, p, n, 0.5 * n as f64 * 0.5))
                .collect();
            for c in &cut {
                for (a, b) in sum.iter_mut().zip(c) {
                    *a += *b as f64;
                }
            }
            count += cut.len();
        }
    }
    let map = if count > 0 {
        let avg: Vec<f32> = sum.iter().map(|x| (x / count as f64) as f32).collect();
        let path = dir.join("membrane_patch.mrc");
        output::write_mrc(&path.to_string_lossy(), &avg, [n; 3], voxel_a, false)?;
        Some(path.to_string_lossy().into_owned())
    } else {
        None
    };
    let kept = patches.iter().filter(|p| p.out == Out::Kept).count();
    let fine = if kept >= 4 && sources.iter().all(|s| s.series.is_some()) {
        match fine_average(sources, &patches, &cubes_from, &dir, progress) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("bilayer: no average from the tilt images: {e:#}");
                None
            }
        }
    } else {
        None
    };
    let report = Report {
        dir: dir.to_string_lossy().into_owned(),
        voxel_a,
        offsets_a: offsets.clone(),
        profile: profile_n,
        dense,
        thickness_a,
        leaflets_a,
        denser_side,
        patches: patches.len(),
        kept,
        membranes,
        map,
        fine,
        seconds: started.elapsed().as_secs_f64(),
    };
    save(&dir, &report, &patches)?;
    progress("平均膜片", 1, 1);
    Ok(report)
}

/// The pixel of the tilt images of a source, Å.
fn tilt_pixel(s: &Source) -> Option<f64> {
    let f = s.series.as_ref()?;
    let d: crate::tomo::dataset::Dataset = serde_json::from_slice(&std::fs::read(f).ok()?).ok()?;
    d.optics.pixel_a
}

/// The profile of an unbent average (z, y, x; the membrane flat in X/Y):
/// the mean of each section within `radius` voxels of the middle.
pub fn profile_of_map(map: &[f32], n: usize, radius: f64) -> Vec<f64> {
    let c = (n / 2) as f64;
    (0..n)
        .map(|z| {
            let (mut sum, mut count) = (0f64, 0usize);
            for y in 0..n {
                for x in 0..n {
                    if (x as f64 - c).powi(2) + (y as f64 - c).powi(2) <= radius * radius {
                        sum += map[(z * n + y) * n + x] as f64;
                        count += 1;
                    }
                }
            }
            sum / count.max(1) as f64
        })
        .collect()
}

fn fine_average(sources: &[Source], patches: &[Patch], from: &[(usize, Seed)], dir: &Path, progress: Progress) -> Result<Fine> {
    let px = sources.iter().filter_map(tilt_pixel).fold(0.0, f64::max);
    if px <= 0.0 {
        bail!("不知道倾斜图像的像素大小");
    }
    // The tilt images' own pixel, or a multiple of it when the box would be too large.
    let mut voxel = px;
    while BOX_A / voxel > 48.0 {
        voxel += px;
    }
    let n = (((BOX_A / voxel).round() as usize).div_ceil(2) * 2).max(16);
    let places: Vec<(usize, P, crate::spa::geom::Mat)> =
        patches.iter().zip(from).filter(|(p, _)| p.out == Out::Kept).map(|(p, (si, _))| (*si, p.raw, frame(p.normal))).collect();
    let aligned = super::project::average_from_tilts(sources, &places, n, voxel, BOX_A, 2, progress)?;
    let path = dir.join("membrane_patch_fine.mrc");
    output::write_mrc(&path.to_string_lossy(), &aligned.map, [n; 3], voxel, false)?;
    let raw = profile_of_map(&aligned.map, n, 0.5 * BOX_A / voxel * 0.5);
    let offsets: Vec<f64> = (0..n).map(|z| (z as f64 - (n / 2) as f64) * voxel).collect();
    let mid: Vec<f64> = (0..n).filter(|&k| offsets[k].abs() <= JUDGE_A).map(|k| raw[k]).collect();
    let mean = mid.iter().sum::<f64>() / mid.len().max(1) as f64;
    let sd = (mid.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / mid.len().max(1) as f64).sqrt().max(1e-12);
    let profile: Vec<f64> = raw.iter().map(|x| (x - mean) / sd).collect();
    let (dense, thickness_a, leaflets_a, denser_side) = read_profile(&offsets, &profile);
    let resolution_a = (aligned.shell > 0.0).then(|| n as f64 * voxel / aligned.shell);
    Ok(Fine { voxel_a: voxel, offsets_a: offsets, profile, dense, thickness_a, leaflets_a, denser_side, resolution_a, map: path.to_string_lossy().into_owned() })
}

fn save(dir: &Path, report: &Report, patches: &[Patch]) -> Result<()> {
    std::fs::write(dir.join("membrane.json"), serde_json::to_vec_pretty(report)?)?;
    let mut f = std::io::BufWriter::new(std::fs::File::create(dir.join("profile.csv"))?);
    writeln!(f, "offset_A,common{}", (0..report.membranes.len()).map(|m| format!(",membrane_{}", m + 1)).collect::<String>())?;
    for (k, o) in report.offsets_a.iter().enumerate() {
        write!(f, "{o:.2},{:.4}", report.profile[k])?;
        for m in &report.membranes {
            match m.profile.get(k) {
                Some(v) => write!(f, ",{v:.4}")?,
                None => write!(f, ",")?,
            }
        }
        writeln!(f)?;
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(dir.join("patches.csv"))?);
    writeln!(f, "membrane,layer,image_x,image_y,image_z,raw_x,raw_y,raw_z,normal_x,normal_y,normal_z,score,slide_A,result")?;
    for p in patches {
        let out = match p.out {
            Out::Kept => "kept",
            Out::Unlike => "unlike",
            Out::Off => "off",
            Out::Hidden => "hidden",
            Out::Edge => "edge",
            Out::Dropped => "dropped",
        };
        writeln!(
            f,
            "{},{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.3},{:.3},{:.3},{:.3},{:.1},{}",
            p.membrane + 1,
            report.membranes[p.membrane].layer,
            p.image[0], p.image[1], p.image[2], p.raw[0], p.raw[1], p.raw[2], p.normal[0], p.normal[1], p.normal[2], p.score, p.slide_a, out
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bilayer-like profile: two dark leaflets `apart` steps apart.
    fn bilayer(len: usize, centre: f64, apart: f64) -> Vec<f64> {
        (0..len)
            .map(|k| {
                let x = k as f64 - centre;
                -((-(x - apart / 2.0).powi(2) / 8.0).exp() + (-(x + apart / 2.0).powi(2) / 8.0).exp())
            })
            .collect()
    }

    fn noise(seed: u64, len: usize, amp: f64) -> Vec<f64> {
        let mut rng = crate::spa::Rng(seed);
        (0..len).map(|_| amp * rng.gauss()).collect()
    }

    #[test]
    fn wrong_patches_are_left_out_and_shifts_found() {
        let half = 60isize;
        let len = (2 * half + 1) as usize;
        let mut profiles = vec![];
        let mut truth = vec![];
        for i in 0..60 {
            let shift = (i % 7) as f64 - 3.0;
            let p: Vec<f64> = bilayer(len, half as f64 + shift, 10.0).iter().zip(noise(i, len, 0.3)).map(|(a, b)| a + b).collect();
            profiles.push(Some(p));
            truth.push(true);
        }
        // Twelve that are not membrane: noise, or a single bright line.
        for i in 0..12 {
            let p = if i % 2 == 0 {
                noise(100 + i, len, 0.5)
            } else {
                (0..len).map(|k| (-((k as f64 - half as f64 - 2.0).powi(2)) / 30.0).exp()).zip(noise(200 + i, len, 0.2)).map(|(a, b)| a + b).collect()
            };
            profiles.push(Some(p));
            truth.push(false);
        }
        let weights = vec![1.0; profiles.len()];
        let (state, common) = judge(&profiles, &weights, half, 20, 8);
        let kept_true = (0..60).filter(|&i| state[i].2).count();
        let kept_false = (60..72).filter(|&i| state[i].2).count();
        assert!(kept_true >= 54, "real kept {kept_true}");
        assert!(kept_false <= 1, "wrong kept {kept_false}");
        // The slides undo the shifts (up to a common offset).
        let d0 = state[0].0 - (-3);
        for i in 0..60 {
            if state[i].2 {
                let want = (i % 7) as isize - 3;
                assert!((state[i].0 - want - d0).abs() <= 1, "patch {i}: slide {} want {}", state[i].0, want);
            }
        }
        // Two leaflets ~10 steps apart in the common profile.
        let offsets: Vec<f64> = (-half..=half).map(|j| j as f64).collect();
        let (dense, thick, leaf, _) = read_profile(&offsets, &common);
        assert_eq!(dense, -1);
        assert!(thick.is_some());
        let leaf = leaf.expect("leaflets");
        assert!((leaf - 10.0).abs() <= 1.5, "leaflets {leaf}");
    }

    #[test]
    fn one_dense_side_is_named() {
        let half = 60isize;
        let len = (2 * half + 1) as usize;
        let offsets: Vec<f64> = (-half..=half).map(|j| j as f64).collect();
        // Dark membrane, a dark coat outside (positive side).
        let p: Vec<f64> = bilayer(len, half as f64, 8.0)
            .iter()
            .enumerate()
            .map(|(k, v)| v - 0.5 * (-((offsets[k] - 25.0).powi(2)) / 60.0).exp())
            .collect();
        let (_, _, _, side) = read_profile(&offsets, &p);
        assert_eq!(side, "outside");
    }
}
