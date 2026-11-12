//! "Circle a few, find the rest": the user circles two or three examples
//! of whatever is to be averaged, and the program finds everything in the
//! tomograms that looks like them.
//!
//! The program knows nothing about what the thing is: not that it is a
//! particle, a pit of a membrane or a bud, not that it sits on anything.
//! All it has is the picture inside each circle, and each of those
//! pictures is searched for as it is (`search_gpu::search`, every
//! orientation). What follows from the examples and from nothing else:
//!
//! - Sizes. The circles say how large the thing is. When the user circled
//!   things of different sizes, every example is searched for at each of
//!   those sizes (a picture scaled up or down), so the sizes between and
//!   around are found by whichever example comes nearest.
//! - How alike is alike enough. Each example is also found by the pictures
//!   of the other examples, with some score. Things the user calls the same
//!   score that much with each other, so what scores at least as much with
//!   any example counts as one more of them (`Found::alike`). Where the
//!   scores of all places fall into two groups by themselves, the gap
//!   between the groups counts as well (`search::cutoff`), and neither goes
//!   below what chance scores (the control of `project::chance`).
//! - One frame. The turn that carries the first example's picture onto each
//!   other example comes out of the same searches, so places found by
//!   different examples are averaged together (`Found::frames`).
//!
//! The search is made on the image the user drew on (the clear "look"
//! tomogram when there is one); what is cut out and averaged afterwards
//! comes from the tomogram itself, as on every other route.

use anyhow::{bail, Context, Result};

use super::check_cancel;
use super::project::{allowed_on_look, binned, chance, cube, cube_fits, nearest_bin, pixel_of, rescale_cube, Found, LookSource, Place, Source, BOX, MOST};
use super::search::{allowed, beyond_chance, cutoff, peaks, Peak};
use super::refine::Progress;
use crate::spa::geom::{distance, grid, Mat};

/// One thing the user circled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Example {
    /// Which of the sources.
    pub source: usize,
    /// The circle's centre, voxels (x, y, z) of `Source::path`.
    pub at: [f64; 3],
    /// The circle's radius, in the same voxels.
    pub radius: f64,
}

/// Voxels across the circle in the search: what a search of every
/// orientation can afford, and enough to tell shapes apart (the template
/// search of the cards works at the same sampling).
const ACROSS: f64 = 9.0;
/// Side of the picture searched for.
const N: usize = 16;
/// Side of the picture kept of each example, at twice the sampling of the
/// search, so that it can be brought onto every search grid.
const KEPT: usize = 32;
/// Voxels across the circle in the cubes that are cut and averaged.
pub const SPAN: f64 = 18.0;
/// Circles this close in size (ratio) are one size.
const SAME_SIZE: f64 = 1.15;
/// A circle has to be at least this many voxels of its tomogram across.
const SMALLEST: f64 = 6.0;
/// Fewest voxels the radius may have on the grid searched.
const COARSEST: f64 = 3.0;

/// What the search of the examples gave.
pub struct Circled {
    pub found: Found,
    /// The first example's cube (`BOX`³ at `found.voxel_a`, from the
    /// tomogram itself): what the places are aligned to first.
    pub template: Vec<f32>,
    /// The size the cubes are cut for: the largest circle's diameter, Å.
    pub diameter_a: f64,
    /// The examples' own places in `found.places`.
    pub mine: Vec<usize>,
    /// Which example is the first (`template`, the frame of the average).
    pub first: usize,
    /// Per example, the best score the other examples' pictures have on it
    /// (None: a single example, or the others could not be searched there).
    pub by_others: Vec<Option<f32>>,
}

/// The sizes circled: diameters (Å) that differ by more than `SAME_SIZE`,
/// smallest first, and which of them each of `diameters` is.
pub fn sizes(diameters: &[f64]) -> (Vec<f64>, Vec<usize>) {
    let mut order: Vec<usize> = (0..diameters.len()).collect();
    order.sort_by(|a, b| diameters[*a].total_cmp(&diameters[*b]));
    // Groups: (first diameter, members).
    let mut groups: Vec<(f64, Vec<usize>)> = vec![];
    for i in order {
        match groups.last_mut() {
            Some((first, members)) if diameters[i] <= *first * SAME_SIZE => members.push(i),
            _ => groups.push((diameters[i], vec![i])),
        }
    }
    let mut of = vec![0; diameters.len()];
    let mut out = vec![];
    for (k, (_, members)) in groups.iter().enumerate() {
        // The size of a group is the mean of its circles on the scale sizes live on.
        out.push((members.iter().map(|i| diameters[*i].ln()).sum::<f64>() / members.len() as f64).exp());
        members.iter().for_each(|i| of[*i] = k);
    }
    (out, of)
}

/// An `n`³ cube of `v` about `centre` (voxels, a voxel's centre its
/// index), its samples `step` voxels apart, each the mean of 2³ trilinear
/// readings in its own cell. What lies outside the volume takes the mean of
/// what lies inside; None when less than half is inside or all is flat.
/// Zero mean, unit variance.
fn sampled(v: &[f32], dims: [usize; 3], centre: [f64; 3], n: usize, step: f64) -> Option<Vec<f32>> {
    let [nx, ny, _] = dims;
    let at = |x: usize, y: usize, z: usize| v[(z * ny + y) * nx + x] as f64;
    let read = |p: [f64; 3]| -> Option<f64> {
        if (0..3).any(|k| p[k] < 0.0 || p[k] > dims[k] as f64 - 1.0) {
            return None;
        }
        let f = p.map(|x| x.floor());
        let i = [0, 1, 2].map(|k| (f[k] as usize).min(dims[k].saturating_sub(2)));
        let w = [0, 1, 2].map(|k| p[k] - i[k] as f64);
        let mut s = 0.0;
        for c in 0..8 {
            let o = [c & 1, c >> 1 & 1, c >> 2 & 1];
            let q = [0, 1, 2].map(|k| (i[k] + o[k]).min(dims[k] - 1));
            let weight: f64 = (0..3).map(|k| if o[k] == 1 { w[k] } else { 1.0 - w[k] }).product();
            s += weight * at(q[0], q[1], q[2]);
        }
        Some(s)
    };
    let h = (n / 2) as f64;
    let mut out = vec![f32::NAN; n * n * n];
    let (mut sum, mut sq, mut count) = (0f64, 0f64, 0usize);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let (mut s, mut got) = (0.0, 0);
                for c in 0..8 {
                    let sub = [c & 1, c >> 1 & 1, c >> 2 & 1].map(|b| b as f64 * 0.5 - 0.25);
                    let p: [f64; 3] = std::array::from_fn(|k| centre[k] + ([x, y, z][k] as f64 - h + sub[k]) * step);
                    if let Some(val) = read(p) {
                        s += val;
                        got += 1;
                    }
                }
                if got > 0 {
                    let val = s / got as f64;
                    out[(z * n + y) * n + x] = val as f32;
                    sum += val;
                    sq += val * val;
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

/// A tomogram as one size is searched in it.
struct Grid {
    v: Vec<f32>,
    dims: [usize; 3],
    /// Voxel, Å.
    voxel_a: f64,
    /// Voxels of the file read per voxel here.
    bin: usize,
    /// The file is the look tomogram of the source.
    look: Option<LookSource>,
    /// Where a centre may lie.
    mask: Vec<bool>,
}

impl Grid {
    /// The image searched for things `size_a` across: the look tomogram
    /// when there is one and it shows them large enough, else the
    /// tomogram itself; binned so the size is about `ACROSS` voxels.
    fn of(s: &Source, size_a: f64) -> Result<Grid> {
        let want = size_a / ACROSS;
        let raw_px = pixel_of(&s.path)?;
        let look = s.look.as_ref().and_then(|l| {
            let px = pixel_of(&l.path).ok()?;
            let bin = nearest_bin(px, want);
            (0.5 * size_a / (bin as f64 * px) >= COARSEST).then_some((l.clone(), px, bin))
        });
        match look {
            Some((l, px, bin)) => {
                let (v, dims, voxel_a, bin) = binned(&l.path, bin as f64 * px)?;
                // Gold and borders are told on the tomogram itself
                // (`allowed_on_look`), at about the same sampling.
                let raw_bin = nearest_bin(raw_px, voxel_a);
                let (raw, raw_dims, raw_voxel, raw_bin) = binned(&s.path, raw_bin as f64 * raw_px)?;
                let raw_ok = allowed(&raw, raw_dims, 0.5 * size_a / raw_voxel);
                let mask = allowed_on_look(&raw_ok, raw_dims, 1, raw_bin, dims, 1, bin, &l, 0.5 * size_a / voxel_a);
                Ok(Grid { v, dims, voxel_a, bin, look: Some(l), mask })
            }
            None => {
                let bin = nearest_bin(raw_px, want);
                let (v, dims, voxel_a, bin) = binned(&s.path, bin as f64 * raw_px)?;
                let mask = allowed(&v, dims, 0.5 * size_a / voxel_a);
                Ok(Grid { v, dims, voxel_a, bin, look: None, mask })
            }
        }
    }

    /// A position of the source's tomogram (unbinned voxels) on this grid.
    fn from_raw(&self, p: [f64; 3]) -> [f64; 3] {
        let b = self.bin as f64;
        std::array::from_fn(|k| {
            let file = match &self.look {
                Some(l) => (p[k] - l.offset[k]) / l.scale,
                None => p[k],
            };
            (file - (b - 1.0) / 2.0) / b
        })
    }

    /// A voxel of this grid as a position of the source's tomogram.
    fn to_raw(&self, p: [f64; 3]) -> [f64; 3] {
        let b = self.bin as f64;
        std::array::from_fn(|k| {
            let file = p[k] * b + (b - 1.0) / 2.0;
            match &self.look {
                Some(l) => l.scale * file + l.offset[k],
                None => file,
            }
        })
    }
}

/// The picture of an example: `KEPT`³ with the circle `SPAN` voxels across,
/// from the image the user drew on.
fn picture(s: &Source, e: &Example, raw_px: f64) -> Result<Option<Vec<f32>>> {
    let diameter_a = 2.0 * e.radius * raw_px;
    let voxel = diameter_a / SPAN;
    let (path, px, look) = match &s.look {
        Some(l) => (l.path.as_str(), pixel_of(&l.path)?, Some(l)),
        None => (s.path.as_str(), raw_px, None),
    };
    // Read no coarser than the picture's own voxels.
    let bin = ((voxel / px).floor() as usize).max(1);
    let (v, dims, got, bin) = binned(path, bin as f64 * px)?;
    let b = bin as f64;
    let centre: [f64; 3] = std::array::from_fn(|k| {
        let file = match look {
            Some(l) => (e.at[k] - l.offset[k]) / l.scale,
            None => e.at[k],
        };
        (file - (b - 1.0) / 2.0) / b
    });
    Ok(sampled(&v, dims, centre, KEPT, voxel / got))
}

/// Side of the picture of a place, pixels.
pub const TILE: usize = 64;
/// How many circle diameters a picture shows.
const TILE_SPAN: f64 = 1.5;

/// Pictures of places for the user to look through: at each of `places`
/// (source, centre and radius in voxels of the source's tomogram) the
/// section the user drew on (X across, Y down, at the place's own height,
/// a few sections thick), from the look tomogram when there is one,
/// `TILE` pixels square, grey, its own contrast. None where the place is
/// off the image.
pub fn tiles(sources: &[Source], places: &[(usize, [f64; 3], f64)]) -> Result<Vec<Option<Vec<u8>>>> {
    let mut out: Vec<Option<Vec<u8>>> = vec![None; places.len()];
    for (i, s) in sources.iter().enumerate() {
        let here: Vec<usize> = (0..places.len()).filter(|k| places[*k].0 == i).collect();
        if here.is_empty() {
            continue;
        }
        check_cancel()?;
        let (path, look) = match &s.look {
            Some(l) => (l.path.as_str(), Some(l)),
            None => (s.path.as_str(), None),
        };
        let px = pixel_of(path)?;
        let in_file = |p: [f64; 3]| -> [f64; 3] {
            match look {
                Some(l) => std::array::from_fn(|k| (p[k] - l.offset[k]) / l.scale),
                None => p,
            }
        };
        let scale = look.map_or(1.0, |l| l.scale);
        // Read no finer than the smallest picture's pixels.
        let smallest = here.iter().map(|k| places[*k].2 / scale).fold(f64::MAX, f64::min);
        let bin = ((2.0 * TILE_SPAN * smallest / TILE as f64).floor() as usize).max(1);
        let (v, dims, _, bin) = binned(path, bin as f64 * px)?;
        let b = bin as f64;
        let at = |x: usize, y: usize, z: usize| v[(z * dims[1] + y) * dims[0] + x] as f64;
        for k in here {
            let (_, centre, radius) = places[k];
            let c = in_file(centre).map(|x| (x - (b - 1.0) / 2.0) / b);
            let r = radius / scale / b;
            let step = 2.0 * TILE_SPAN * r / TILE as f64;
            let thick = ((0.15 * r).round() as isize).max(0);
            let z0 = c[2].round() as isize;
            let mut grey: Vec<Option<f64>> = Vec::with_capacity(TILE * TILE);
            for j in 0..TILE {
                for i in 0..TILE {
                    let (x, y) = (c[0] + (i as f64 - TILE as f64 / 2.0 + 0.5) * step, c[1] + (j as f64 - TILE as f64 / 2.0 + 0.5) * step);
                    if x < 0.0 || y < 0.0 || x > dims[0] as f64 - 1.0 || y > dims[1] as f64 - 1.0 {
                        grey.push(None);
                        continue;
                    }
                    let (x0, y0) = ((x.floor() as usize).min(dims[0] - 2), (y.floor() as usize).min(dims[1] - 2));
                    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
                    let (mut sum, mut count) = (0.0, 0);
                    for dz in -thick..=thick {
                        let z = z0 + dz;
                        if z < 0 || z >= dims[2] as isize {
                            continue;
                        }
                        let z = z as usize;
                        sum += (1.0 - fx) * (1.0 - fy) * at(x0, y0, z) + fx * (1.0 - fy) * at(x0 + 1, y0, z) + (1.0 - fx) * fy * at(x0, y0 + 1, z) + fx * fy * at(x0 + 1, y0 + 1, z);
                        count += 1;
                    }
                    grey.push((count > 0).then(|| sum / count as f64));
                }
            }
            let mut known: Vec<f64> = grey.iter().flatten().copied().collect();
            if known.len() * 2 < grey.len() {
                continue;
            }
            known.sort_by(f64::total_cmp);
            let (lo, hi) = (known[known.len() / 100], known[known.len() - 1 - known.len() / 100]);
            let mid = known[known.len() / 2];
            let span = (hi - lo).max(1e-9);
            out[k] = Some(grey.iter().map(|x| (255.0 * (x.unwrap_or(mid) - lo) / span).clamp(0.0, 255.0) as u8).collect());
        }
    }
    Ok(out)
}

/// A place before it is put on the grid of the cubes.
#[derive(Clone, Copy, Debug)]
struct Seen {
    source: usize,
    /// Unbinned voxels of the source's tomogram.
    at: [f64; 3],
    /// The radius of the thing there, in the same voxels.
    radius: f64,
    score: f32,
    turn: usize,
    like: usize,
    size: usize,
}

/// Of `all`, best first, those not within touching distance of a better
/// one (or of one of `first`, which all stay).
fn apart(first: Vec<Seen>, mut all: Vec<Seen>) -> Vec<Seen> {
    all.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut out = first;
    // Looked up by cells of the largest radius.
    let cell = out.iter().chain(&all).map(|s| s.radius).fold(1.0, f64::max) * 2.0;
    let key = |s: &Seen| (s.source, s.at.map(|x| (x / cell).floor() as i64));
    let mut held: std::collections::HashMap<(usize, [i64; 3]), Vec<usize>> = Default::default();
    for (i, s) in out.iter().enumerate() {
        held.entry(key(s)).or_default().push(i);
    }
    for s in all {
        let (src, k) = key(&s);
        let mut free = true;
        'cells: for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    for j in held.get(&(src, [k[0] + dx, k[1] + dy, k[2] + dz])).into_iter().flatten() {
                        let o = &out[*j];
                        let d = (0..3).map(|a| (o.at[a] - s.at[a]).powi(2)).sum::<f64>().sqrt();
                        if d < 0.9 * (o.radius + s.radius) {
                            free = false;
                            break 'cells;
                        }
                    }
                }
            }
        }
        if free {
            held.entry((src, k)).or_default().push(out.len());
            out.push(s);
        }
    }
    out
}

/// The score that parts what is like the examples from the rest. `gap`:
/// where the scores fall into two groups; `alike`: what the examples score
/// with each other; `noise`: the best chance scored; `chance`: where the
/// scores pass what chance scores, when they do.
pub fn limit(gap: Option<f32>, alike: Option<f32>, noise: Option<f32>, chance: Option<f32>) -> Option<f32> {
    // Examples that are no more alike than chance say nothing.
    let alike = alike.filter(|a| noise.is_none_or(|n| *a > n));
    match (gap, alike) {
        (Some(g), Some(a)) => Some(g.min(a)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => chance,
    }
}

/// Everything in `sources` that looks like one of `examples`.
pub fn find(dev: &burn::tensor::Device, sources: &[Source], examples: &[Example], progress: Progress) -> Result<Circled> {
    find_at(dev, sources, examples, &grid(30.0), progress)
}

/// `find` with the orientations searched given (`find`: `grid(30.0)`, which
/// `Place::turn` indexes everywhere else).
fn find_at(dev: &burn::tensor::Device, sources: &[Source], examples: &[Example], turns: &[Mat], progress: Progress) -> Result<Circled> {
    if sources.is_empty() {
        bail!("没有断层图");
    }
    if examples.is_empty() {
        bail!("请先圈出 2 到 3 个要找的东西作例子");
    }
    if let Some(e) = examples.iter().find(|e| e.source >= sources.len()) {
        bail!("圈所在的断层图（第 {} 张）不在要找的断层图里", e.source + 1);
    }
    if examples.iter().any(|e| 2.0 * e.radius < SMALLEST) {
        bail!("有一个圈太小了，请把要找的东西整个圈进去");
    }
    let k = examples.len();
    let raw_px: Vec<f64> = sources.iter().map(|s| pixel_of(&s.path)).collect::<Result<_>>()?;
    let diameters: Vec<f64> = examples.iter().map(|e| 2.0 * e.radius * raw_px[e.source]).collect();
    let (sizes_a, size_of) = sizes(&diameters);
    let diameter_a = *sizes_a.last().unwrap();

    // The examples' pictures, one tomogram in memory at a time.
    progress("取出例子", 0, k);
    let mut pictures: Vec<Vec<f32>> = vec![];
    for (i, e) in examples.iter().enumerate() {
        check_cancel()?;
        let p = picture(&sources[e.source], e, raw_px[e.source])?.with_context(|| format!("第 {} 个圈大半在断层图外面，请圈在图里面", i + 1))?;
        pictures.push(p);
        progress("取出例子", i + 1, k);
    }
    // The picture of example `e` on a grid of `voxel` Å for things `size` Å
    // across: scaled so that its own circle is that size.
    let scaled = |e: usize, size: f64, voxel: f64| rescale_cube(&pictures[e], KEPT, diameters[e] / SPAN, N, voxel * diameters[e] / size);

    let total = sources.len() * sizes_a.len() * (k + 1);
    let mut done = 0;
    let mut seen: Vec<Seen> = vec![];
    let mut by_chance: Vec<(f32, f64)> = vec![];
    // (score, turn, size) of picture a at example b.
    let mut cross: Vec<Vec<Option<(f32, usize, usize)>>> = vec![vec![None; k]; k];
    let mut searched: Vec<(bool, f64)> = vec![];
    for (i, s) in sources.iter().enumerate() {
        for (z, size) in sizes_a.iter().enumerate() {
            check_cancel()?;
            let g = Grid::of(s, *size)?;
            searched.push((g.look.is_some(), g.voxel_a));
            let radius = (0.5 * size / g.voxel_a).min(N as f64 / 2.0 - 2.0);
            let to_raw = (g.to_raw([1.0, 0.0, 0.0])[0] - g.to_raw([0.0; 3])[0]).abs();
            for e in 0..k {
                check_cancel()?;
                let stage = format!("找像例子的 {}/{}", done + 1, total);
                let picture = scaled(e, *size, g.voxel_a);
                let found = super::search_gpu::search(dev, &g.v, g.dims, &picture, N, radius, s.tilts, turns, Some(&g.mask), &|a, b| progress(&stage, a, b))?;
                done += 1;
                for p in peaks(&found, 1.8 * radius, MOST) {
                    seen.push(Seen { source: i, at: g.to_raw(p.pos.map(|x| x as f64)), radius: radius * to_raw, score: p.score, turn: p.turn, like: e, size: z });
                }
                // What this picture scores at each example of this tomogram:
                // the best within half a radius of the circle's centre.
                for (b, x) in examples.iter().enumerate().filter(|(_, x)| x.source == i) {
                    let c = g.from_raw(x.at);
                    let reach = (0.5 * radius).ceil() as isize;
                    let mut best: Option<(f32, usize)> = None;
                    for dz in -reach..=reach {
                        for dy in -reach..=reach {
                            for dx in -reach..=reach {
                                let q = [c[0].round() as isize + dx, c[1].round() as isize + dy, c[2].round() as isize + dz];
                                if (0..3).any(|a| q[a] < 0 || q[a] >= g.dims[a] as isize) {
                                    continue;
                                }
                                let at = (q[2] as usize * g.dims[1] + q[1] as usize) * g.dims[0] + q[0] as usize;
                                let score = found.score[at];
                                if score != f32::MIN && best.is_none_or(|b| score > b.0) {
                                    best = Some((score, found.turn[at] as usize));
                                }
                            }
                        }
                    }
                    if let Some((score, turn)) = best {
                        if cross[e][b].is_none_or(|c| score > c.0) {
                            cross[e][b] = Some((score, turn, z));
                        }
                    }
                }
            }
            // What chance scores on this grid, with a picture of this size.
            let stage = format!("找像例子的 {}/{}（对照）", done + 1, total);
            let e = size_of.iter().position(|x| *x == z).unwrap_or(0);
            let control = chance(dev, &g.v, g.dims, &g.mask, &scaled(e, *size, g.voxel_a), N, radius, s.tilts, turns, &|a, b| progress(&stage, a, b))?;
            done += 1;
            // Every picture had its chances here.
            by_chance.extend(control.into_iter().map(|(score, each)| (score, each * k as f64)));
        }
    }

    // How alike the examples are to each other.
    let by_others: Vec<Option<f32>> = (0..k).map(|b| (0..k).filter(|a| *a != b).filter_map(|a| cross[a][b].map(|c| c.0)).reduce(f32::max)).collect();
    let alike = by_others.iter().copied().collect::<Option<Vec<f32>>>().and_then(|v| v.into_iter().reduce(f32::min));
    // The first example: the one whose picture finds the others best.
    let first = (0..k)
        .max_by(|a, b| {
            let sum = |e: usize| (0..k).filter(|x| *x != e).map(|x| cross[e][x].map_or(0.0, |c| c.0)).sum::<f32>();
            sum(*a).total_cmp(&sum(*b))
        })
        .unwrap_or(0);
    let nearest = |r: &Mat| (0..turns.len()).min_by(|a, b| distance(&turns[*a], r).total_cmp(&distance(&turns[*b], r))).unwrap_or(0);
    let upright = nearest(&super::IDENTITY);
    let frames: Vec<Mat> = (0..k).map(|e| if e == first { super::IDENTITY } else { cross[first][e].map_or(super::IDENTITY, |c| turns[c.1]) }).collect();

    // The examples themselves stay whatever they score; the rest best first.
    let own: Vec<Seen> = examples
        .iter()
        .enumerate()
        .map(|(e, x)| Seen { source: x.source, at: x.at, radius: x.radius, score: f32::MAX, turn: cross[e][e].map_or(upright, |c| c.1), like: e, size: size_of[e] })
        .collect();
    let mut all = apart(own, seen);
    let rest: Vec<Peak> = all[k..].iter().map(|s| Peak { pos: [0; 3], score: s.score, turn: s.turn }).collect();
    let noise = by_chance.iter().map(|c| c.0).reduce(f32::max);
    let limit = limit(cutoff(&rest), alike, noise, beyond_chance(&rest, &by_chance));
    // An example scores what the others' pictures give it; a single one
    // (or one the others cannot reach) leads the list.
    let top = rest.first().map_or(1.0, |p| p.score);
    for e in 0..k {
        all[e].score = by_others[e].unwrap_or(top).min(1.0);
    }

    // Onto the grids the cubes are cut on (as `project::cut_places` bins).
    let px = raw_px[examples[first].source];
    let voxel_a = px * ((diameter_a / SPAN / px).round()).max(1.0);
    let mut places: Vec<(Place, bool)> = vec![];
    let mut template = None;
    for (i, s) in sources.iter().enumerate() {
        check_cancel()?;
        let img = crate::tomo::pipeline::open(&s.path)?;
        let info = img.info();
        let bin = ((voxel_a / raw_px[i]).round() as usize).max(1);
        let dims = [info.width / bin, info.height / bin, info.size_z / bin];
        let on_grid = |at: [f64; 3]| at.map(|x| ((x - (bin as f64 - 1.0) / 2.0) / bin as f64).round().max(0.0) as usize);
        for (n, p) in all.iter().enumerate().filter(|(_, p)| p.source == i) {
            let c = on_grid(p.at);
            if cube_fits(dims, c, BOX) {
                places.push((Place { source: i, c, bin, turn: p.turn, score: p.score, like: p.like as u8, size: p.size as u8 }, n < k));
            }
        }
        if examples[first].source == i {
            let (v, dims, _, _) = binned(&s.path, voxel_a)?;
            template = cube(&v, dims, on_grid(examples[first].at));
        }
    }
    let template = template.context("圈的例子太靠近断层图的边了，请圈一个离边远一些的")?;
    places.sort_by(|a, b| b.0.score.total_cmp(&a.0.score));
    let mine: Vec<usize> = (0..places.len()).filter(|i| places[*i].1).collect();
    let kind = if searched.iter().all(|s| s.0) {
        "look"
    } else if searched.iter().all(|s| !s.0) {
        "raw"
    } else {
        "mixed"
    };
    let found = Found {
        places: places.into_iter().map(|p| p.0).collect(),
        limit,
        noise,
        voxel_a,
        searched: kind.to_string(),
        search_voxel_a: searched.first().map(|s| s.1),
        frames,
        sizes_a,
        alike,
    };
    Ok(Circled { found, template, diameter_a, mine, first, by_others })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spa::geom::{apply, mul, symmetry, transpose};
    use crate::spa::Rng;

    #[test]
    fn circles_of_about_one_size_are_one_size() {
        let (sizes, of) = sizes(&[300.0, 620.0, 310.0, 580.0, 1000.0]);
        assert_eq!(of, vec![0, 1, 0, 1, 2]);
        assert_eq!(sizes.len(), 3);
        assert!((sizes[0] - (300.0f64 * 310.0).sqrt()).abs() < 1e-6 && (sizes[2] - 1000.0).abs() < 1e-6, "{sizes:?}");
        let (one, of) = super::sizes(&[200.0]);
        assert!((one[0] - 200.0).abs() < 1e-6 && of == vec![0]);
    }

    /// What is alike enough: as alike as the examples are to each other or
    /// in the group the scores form by themselves, whichever asks less;
    /// examples no more alike than chance do not say; with neither, what
    /// passes chance.
    #[test]
    fn the_examples_say_how_alike_is_enough() {
        assert_eq!(limit(Some(0.5), Some(0.4), Some(0.2), None), Some(0.4));
        assert_eq!(limit(Some(0.3), Some(0.4), Some(0.2), None), Some(0.3));
        assert_eq!(limit(None, Some(0.4), Some(0.2), Some(0.25)), Some(0.4));
        assert_eq!(limit(None, Some(0.15), Some(0.2), Some(0.25)), Some(0.25));
        assert_eq!(limit(Some(0.5), Some(0.15), Some(0.2), None), Some(0.5));
        assert_eq!(limit(None, None, Some(0.2), None), None);
        assert_eq!(limit(None, Some(0.4), None, None), Some(0.4));
    }

    #[test]
    fn a_place_within_touch_of_a_better_one_goes() {
        let seen = |x: f64, radius: f64, score: f32| Seen { source: 0, at: [x, 50.0, 50.0], radius, score, turn: 0, like: 0, size: 0 };
        // The example stays though it scores less than what overlaps it.
        let own = vec![seen(100.0, 10.0, 0.1)];
        let all = vec![seen(105.0, 10.0, 0.9), seen(130.0, 10.0, 0.5), seen(140.0, 10.0, 0.6), seen(170.0, 20.0, 0.4), seen(100.0, 10.0, 0.3)];
        let kept = apart(own, all);
        let xs: Vec<f64> = kept.iter().map(|s| s.at[0]).collect();
        // 105 touches the example; 130 touches the better 140; 170 (radius 20) is 30 from 140: clear of 0.9 × 30.
        assert_eq!(xs, vec![100.0, 140.0, 170.0]);
        // Another tomogram is another place.
        let other = Seen { source: 1, ..seen(100.0, 10.0, 0.2) };
        assert_eq!(apart(vec![seen(100.0, 10.0, 0.1)], vec![other]).len(), 2);
    }

    #[test]
    fn a_picture_is_read_between_the_voxels() {
        // A ramp along x: a sample is the ramp at its own place.
        let dims = [40usize, 30, 20];
        let v: Vec<f32> = (0..dims[0] * dims[1] * dims[2]).map(|i| (i % dims[0]) as f32).collect();
        let cube = sampled(&v, dims, [20.25, 15.0, 10.0], 8, 1.5).unwrap();
        // Normalised: the difference of neighbours along x is the same everywhere, none along y and z.
        let step = cube[1] - cube[0];
        assert!(step > 0.0);
        for z in 0..8 {
            for y in 0..8 {
                for x in 0..7 {
                    assert!((cube[(z * 8 + y) * 8 + x + 1] - cube[(z * 8 + y) * 8 + x] - step).abs() < 1e-4);
                }
                assert!((cube[(z * 8 + y) * 8] - cube[0]).abs() < 1e-4);
            }
        }
        // Mostly outside: nothing.
        assert!(sampled(&v, dims, [-10.0, 15.0, 10.0], 8, 1.5).is_none());
    }

    #[test]
    fn a_picture_of_a_place_shows_what_is_there() {
        // A bright ball of radius 6 at (30, 24, 10) in an empty tomogram of 10 Å voxels.
        let dims = [64usize, 48, 20];
        let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
        for i in 0..v.len() {
            let p = [(i % dims[0]) as f64 - 30.0, (i / dims[0] % dims[1]) as f64 - 24.0, (i / dims[0] / dims[1]) as f64 - 10.0];
            if p[0] * p[0] + p[1] * p[1] + p[2] * p[2] <= 36.0 {
                v[i] = 1.0;
            }
        }
        let dir = std::env::temp_dir().join(format!("tmv-sta-tiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tomo.mrc").to_string_lossy().into_owned();
        crate::tomo::output::write_mrc(&path, &v, dims, 10.0, false).unwrap();
        let sources = vec![Source { path, tilts: [-60.0, 60.0], look: None, series: None }];
        // Circled a little wider than the ball; one place far off the image.
        let got = tiles(&sources, &[(0, [30.0, 24.0, 10.0], 8.0), (0, [-80.0, 24.0, 10.0], 8.0)]).unwrap();
        assert!(got[1].is_none());
        let t = got[0].as_ref().unwrap();
        assert_eq!(t.len(), TILE * TILE);
        // The picture is 24 voxels across: the ball fills its middle half and is bright, the corners are dark.
        assert!(t[(TILE / 2) * TILE + TILE / 2] > 200 && t[0] < 50 && t[TILE * TILE - 1] < 50, "{} {}", t[(TILE / 2) * TILE + TILE / 2], t[0]);
        let bright = t.iter().filter(|x| **x > 128).count() as f64 / t.len() as f64;
        // A disc of radius 6 in a square of 24: π·36 / 576 = 0.196.
        assert!((bright - 0.196).abs() < 0.04, "{bright}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The thing looked for, of radius 1 about the origin: a bowl (the
    /// lower part of a shell) with a knob beside its rim. Nothing a blob
    /// finder or a membrane tracer would call by a name.
    fn wanted(p: [f64; 3]) -> f64 {
        let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        let shell = (-((r - 0.62) / 0.11).powi(2)).exp() / (1.0 + ((p[2] - 0.1) / 0.08).exp());
        let knob = (-((p[0] - 0.5).powi(2) + p[1] * p[1] + (p[2] - 0.5).powi(2)) / (2.0 * 0.17 * 0.17)).exp();
        shell + 1.2 * knob
    }

    /// Something else of the same size: a rod with a ball beside it.
    fn other(p: [f64; 3]) -> f64 {
        let along = p[0].abs() - 0.6;
        let off = (p[1] * p[1] + p[2] * p[2] + if along > 0.0 { along * along } else { 0.0 }).sqrt();
        let rod = (-(off / 0.16).powi(2)).exp();
        let ball = (-(p[0] * p[0] + (p[1] - 0.55).powi(2) + p[2] * p[2]) / (2.0 * 0.2 * 0.2)).exp();
        rod + ball
    }

    fn plant(v: &mut [f32], dims: [usize; 3], at: [usize; 3], radius: f64, r: &Mat, body: fn([f64; 3]) -> f64) {
        let back = transpose(r);
        let h = (1.2 * radius).ceil() as isize;
        for dz in -h..=h {
            for dy in -h..=h {
                for dx in -h..=h {
                    let q = [at[0] as isize + dx, at[1] as isize + dy, at[2] as isize + dz];
                    if (0..3).any(|k| q[k] < 0 || q[k] >= dims[k] as isize) {
                        continue;
                    }
                    let p = apply(&back, [dx as f64 / radius, dy as f64 / radius, dz as f64 / radius]);
                    v[(q[2] as usize * dims[1] + q[1] as usize) * dims[0] + q[0] as usize] += body(p) as f32;
                }
            }
        }
    }

    /// Heavy (a minute and a half optimised):
    /// `CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test --no-default-features --features cli --lib sta::circled -- --ignored --nocapture`.
    ///
    /// A made-up tomogram holds the thing at two sizes and every which way
    /// up, among as many other things of the same sizes. Two of the small
    /// ones and one of the large are circled, a voxel or two off; nothing
    /// says what they are. The rest of both sizes come back above the
    /// limit, hardly any of the other things do, each is of the size it
    /// was found at, and its orientation is the true one in the frame of
    /// the first example.
    #[test]
    #[ignore]
    fn the_rest_of_what_was_circled_comes_back_at_both_sizes() {
        let dims = [400usize, 240, 96];
        let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
        let mut rng = Rng(0xc1c1);
        // The 24 turns of a cube: any two of them differ by another.
        let turns = symmetry("O").unwrap();
        assert_eq!(turns.len(), 24);
        // (centre, radius, turn, wanted)
        let mut things: Vec<([usize; 3], f64, usize, bool)> = vec![];
        // Small ones (radius 18) on the left, large ones (radius 27) on the right.
        for (k, z) in [26usize, 70].into_iter().enumerate() {
            for j in 0..4 {
                for i in 0..4 {
                    let n = (k * 4 + j) * 4 + i;
                    things.push(([52 + 44 * i, 52 + 44 * j, z], 18.0 * (0.96 + 0.08 * rng.unit()), rng.below(24), n % 2 == (j + k) % 2));
                }
            }
        }
        for j in 0..3 {
            for i in 0..2 {
                things.push(([262 + 64 * i, 56 + 62 * j, 48], 27.0 * (0.96 + 0.08 * rng.unit()), rng.below(24), (i + j) % 3 != 0));
            }
        }
        for (at, radius, turn, yes) in &things {
            plant(&mut v, dims, *at, *radius, &turns[*turn], if *yes { wanted } else { other });
        }
        v.iter_mut().for_each(|x| *x += (1.5 * rng.gauss()) as f32);
        let dir = std::env::temp_dir().join(format!("tmv-sta-circled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tomo.mrc").to_string_lossy().into_owned();
        crate::tomo::output::write_mrc(&path, &v, dims, 10.0, false).unwrap();
        let sources = vec![Source { path, tilts: [-90.0, 90.0], look: None, series: None }];

        let small: Vec<usize> = (0..32).filter(|i| things[*i].3).collect();
        let large: Vec<usize> = (32..things.len()).filter(|i| things[*i].3).collect();
        let circled = [small[1], small[6], large[1]];
        let examples: Vec<Example> = circled
            .iter()
            .map(|i| {
                let (at, radius, _, _) = things[*i];
                Example { source: 0, at: std::array::from_fn(|k| at[k] as f64 + 3.0 * rng.unit() - 1.5), radius: 1.05 * radius }
            })
            .collect();
        let started = std::time::Instant::now();
        let c = find_at(&burn::tensor::Device::flex(), &sources, &examples, &turns, &|_, _, _| {}).unwrap();
        let f = &c.found;
        eprintln!("{:?}: {} places, limit {:?}, alike {:?}, noise {:?}, by others {:?}, sizes {:?}, first {}", started.elapsed(), f.places.len(), f.limit, f.alike, f.noise, c.by_others, f.sizes_a, c.first);
        assert_eq!(f.sizes_a.len(), 2);
        assert!((f.sizes_a[0] / 378.0 - 1.0).abs() < 0.08 && (f.sizes_a[1] / 567.0 - 1.0).abs() < 0.08, "{:?}", f.sizes_a);
        assert_eq!(c.mine.len(), 3);
        let limit = f.limit.expect("a limit");
        // Which thing a place is (within a third of its radius).
        let thing_at = |p: &Place| {
            let at = p.at();
            things.iter().position(|t| (0..3).map(|k| (t.0[k] as f64 - at[k]).powi(2)).sum::<f64>().sqrt() < t.1 / 3.0)
        };
        let kept: Vec<&Place> = f.places.iter().filter(|p| p.score >= limit).collect();
        let hit: Vec<Option<usize>> = kept.iter().map(|p| thing_at(p)).collect();
        let got = |i: &usize| hit.contains(&Some(*i));
        let wrong = hit.iter().filter(|h| h.is_none_or(|i| !things[i].3)).count();
        let (found_small, found_large) = (small.iter().filter(|i| got(i)).count(), large.iter().filter(|i| got(i)).count());
        eprintln!("kept {}: {found_small} of {} small, {found_large} of {} large, {wrong} wrong", kept.len(), small.len(), large.len());
        for (rank, p) in f.places.iter().take(40).enumerate() {
            eprintln!("  {rank:2} {:.3} like {} size {} {:?}", p.score, p.like, p.size, thing_at(p).map(|i| (i, things[i].3)));
        }
        assert!(found_small + 1 >= small.len() && found_large + 1 >= large.len(), "{found_small} {found_large}");
        assert!(wrong * 5 <= kept.len(), "{wrong} of {}", kept.len());
        // The size a thing was found at is its own, and it stands as it was planted.
        let first = &things[circled[c.first]];
        let mut turned = 0;
        for (p, h) in kept.iter().zip(&hit) {
            let Some(i) = h.filter(|i| things[*i].3) else { continue };
            assert_eq!(p.size as usize, usize::from(i >= 32), "thing {i}");
            let want = mul(&turns[things[i].2], &transpose(&turns[first.2]));
            if distance(&f.turn_of(p, &turns).unwrap(), &want) < 1.0 {
                turned += 1;
            }
        }
        let right = hit.iter().filter(|h| h.is_some_and(|i| things[i].3)).count();
        eprintln!("{turned} of {right} stand as planted");
        assert!(turned * 10 >= right * 9, "{turned} of {right}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
