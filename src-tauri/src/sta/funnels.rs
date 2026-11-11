//! "Click a few pits, get all of them": the user clicks into two or three
//! places where a membrane takes something in or buds off (`Kind::Pit` of
//! `marks`); every such place on every segmented membrane of the tomograms
//! is found (`pits::pits`), on the side of the membrane the examples lie on,
//! for the user to check before anything is averaged. They come in every
//! size and stage, so what is shown is in the order of the stage
//! (`Pit::stage`) and the user takes all of them or a part of that order.
//!
//! Nothing is asked about sizes. A pit is judged against the run of its
//! membrane over a stretch several times its mouth; the stretch is tried
//! from small to large (`LADDER_A`) and the smallest at which a click lies
//! in a pit is the examples' own.
//!
//! Everything is in voxels of the files the clicks were made on and that
//! were segmented (a tomogram or its look file).

use anyhow::{bail, Result};

use super::alike;
use super::marks::{Kind, Mark};
use super::membranes::density;
use super::paint;
use super::pits::{pits, Pit};
use super::shape::Seed;

type P = [f64; 3];

/// The stretches of membrane a pit is judged against, Å, smallest first.
const LADDER_A: [f64; 4] = [300.0, 600.0, 1200.0, 2400.0];
/// Sampling points across a stretch.
const ACROSS: f64 = 10.0;
/// A pit leaves the membrane's run by this share of the stretch at least.
const OFF: f64 = 0.125;
/// A pit's mouth is at most this share of the stretch it is judged against
/// (a wider one needs the next stretch).
const MOUTH: f64 = 0.8;
/// A stretch shows a pit whole when it shows this share of the depth the
/// best stretch shows.
const WHOLE: f64 = 0.8;
/// A stretch is at least this many voxels, or the file is too coarse for it.
const FEWEST: f64 = 12.0;
/// The density beside a membrane is measured from this far off its middle
/// and over this much, Å (as `membranes`).
const BESIDE_A: (f64, f64) = (25.0, 50.0);
/// Side of a pit's picture, pixels.
pub const TILE: usize = 96;

/// One pit to average from.
#[derive(Clone, Debug)]
pub struct Funnel {
    /// The file it was found in (as the marks name it).
    pub tomogram: String,
    /// One the user clicked.
    pub example: bool,
    /// Whether its membrane's side was told (from an example on the same
    /// membrane, or from the density beside it). When not, the pits of both
    /// sides of that membrane are given.
    pub side_sure: bool,
    /// Voxels of `tomogram`; `seeds` is left empty.
    pub pit: Pit,
    /// Voxel size of `tomogram`, Å.
    pub px: f64,
}

/// What `find` gives.
pub struct Search {
    /// In the order of the stage, shallowest first.
    pub funnels: Vec<Funnel>,
    /// Files with no segmentation yet (they were not searched).
    pub unsegmented: Vec<String>,
    /// The stretch the pits were judged against, Å.
    pub wide_a: f64,
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P, b: P) -> P {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// (spacing, wide, off) in voxels of a file with voxel `px` for a stretch
/// of `wide_a`; None when the file is too coarse for that stretch.
fn scale(wide_a: f64, px: f64) -> Option<(f64, f64, f64)> {
    let wide = wide_a / px;
    if wide < FEWEST {
        return None;
    }
    let spacing = (wide / ACROSS).max(2.0);
    Some((spacing, wide, (OFF * wide).max(spacing)))
}

/// How far a click lies from the pit's wall, when it lies in the pit (or
/// on its wall): within half its width and a little of the nearest seed.
fn holds(pit: &Pit, seeds: &[Seed], click: P, spacing: f64) -> Option<f64> {
    let reach = 0.5 * pit.mouth.max(pit.bulb) + 2.0 * spacing;
    let nearest = pit.seeds.iter().map(|&i| dot(sub(seeds[i].pos, click), sub(seeds[i].pos, click))).fold(f64::MAX, f64::min).sqrt();
    (nearest <= reach).then_some(nearest)
}

/// The clicks of the pit marks: (file, position).
fn clicks(marks: &[Mark]) -> Vec<(String, P)> {
    marks.iter().filter(|m| m.kind == Kind::Pit).flat_map(|m| m.points.iter().map(|p| (m.tomogram.clone(), *p))).collect()
}

/// Whether the marks have a pit clicked.
pub fn marked(marks: &[Mark]) -> bool {
    marks.iter().any(|m| m.kind == Kind::Pit && !m.points.is_empty())
}

/// The lean of a membrane piece as `alike::lean` measures it for `find`.
fn lean(vol: &[f32], n: [usize; 3], seeds: &[Seed], px: f64) -> (f64, f64) {
    let from = BESIDE_A.0 / px + 1.0;
    alike::lean(vol, n, seeds, from, from + (BESIDE_A.1 / px).max(2.0))
}

/// The pits to average from: those the user clicked (`marks` of kind pit)
/// and all others on the membranes of `files` (the marks' own files are
/// searched whether listed or not).
pub fn find(marks: &[Mark], files: &[String]) -> Result<Search> {
    let clicks = clicks(marks);
    let mut all: Vec<String> = vec![];
    for f in clicks.iter().map(|c| &c.0).chain(files) {
        if !all.contains(f) {
            all.push(f.clone());
        }
    }
    let mut out = Search { funnels: vec![], unsegmented: vec![], wide_a: 0.0 };
    // The examples first: for every click the stretches at which it lies in
    // a pit, with the pit's depth (Å) and the side of its membrane the pit
    // is on, as the density beside the membrane tells it (the lean with the
    // normals towards the pit).
    let mut seen: Vec<&String> = vec![];
    let mut held: Vec<Vec<(usize, f64, (f64, f64))>> = vec![];
    for (file, _) in &clicks {
        if seen.contains(&file) {
            continue;
        }
        seen.push(file);
        let Some(mask) = paint::mask_of(file)? else { continue };
        let here: Vec<P> = clicks.iter().filter(|c| &c.0 == file).map(|c| c.1).collect();
        let mut of: Vec<Vec<(usize, f64, (f64, f64))>> = vec![vec![]; here.len()];
        let mut vol: Option<(Vec<f32>, [usize; 3])> = None;
        for (k, wide_a) in LADDER_A.iter().enumerate() {
            let Some((spacing, wide, off)) = scale(*wide_a, mask.px) else { continue };
            for piece in alike::pieces(&mask.mask, mask.n, spacing, 8) {
                let near = |c: P| piece.seeds.iter().any(|s| dot(sub(s.pos, c), sub(s.pos, c)) <= wide * wide);
                if !here.iter().any(|c| near(*c)) {
                    continue;
                }
                let found: Vec<Pit> = pits(&piece.seeds, spacing, wide, off).into_iter().filter(|p| p.mouth <= MOUTH * wide).collect();
                for (c, of) in here.iter().zip(of.iter_mut()) {
                    let at = found.iter().filter_map(|p| holds(p, &piece.seeds, *c, spacing).map(|d| (d, p))).min_by(|a, b| a.0.total_cmp(&b.0));
                    let Some((_, pit)) = at else { continue };
                    if vol.is_none() {
                        vol = Some(density(file)?);
                    }
                    let (v, n) = vol.as_ref().unwrap();
                    let own = lean(v, *n, &piece.seeds, mask.px);
                    of.push((k, pit.depth * mask.px, (pit.side as f64 * own.0, own.1)));
                }
            }
        }
        held.extend(of);
    }
    // A stretch too small for a pit shows pieces of it, shallower than the
    // pit is; from the right stretch on the depth stays. Each click's own
    // stretch is the smallest that shows nearly its whole depth, and the
    // search uses the largest of them.
    let mut step: Option<usize> = None;
    let mut leans: Vec<(f64, f64)> = vec![];
    for of in &held {
        let deepest = of.iter().map(|h| h.1).fold(0.0, f64::max);
        if let Some(h) = of.iter().find(|h| h.1 >= WHOLE * deepest) {
            step = Some(step.map_or(h.0, |s| s.max(h.0)));
            leans.push(h.2);
        }
    }
    let Some(step) = step else {
        if clicks.iter().all(|c| paint::mask_of(&c.0).ok().flatten().is_none()) {
            out.unsegmented = all.into_iter().filter(|f| paint::mask_of(f).ok().flatten().is_none()).collect();
            return Ok(out);
        }
        bail!("点的地方没有找到膜上的凹陷：请点在凹陷的中间（膜向里弯进去的地方）");
    };
    out.wide_a = LADDER_A[step];
    let k = leans.len() as f64;
    let example = (leans.iter().map(|l| l.0).sum::<f64>() / k, (leans.iter().map(|l| l.1 * l.1).sum::<f64>()).sqrt() / k);
    for file in &all {
        let Some(mask) = paint::mask_of(file)? else {
            out.unsegmented.push(file.clone());
            continue;
        };
        let Some((spacing, wide, off)) = scale(out.wide_a, mask.px) else { continue };
        let here: Vec<P> = clicks.iter().filter(|c| &c.0 == file).map(|c| c.1).collect();
        let mut vol: Option<(Vec<f32>, [usize; 3])> = None;
        for piece in alike::pieces(&mask.mask, mask.n, spacing, 8) {
            let found: Vec<Pit> = pits(&piece.seeds, spacing, wide, off).into_iter().filter(|p| p.mouth <= MOUTH * wide).collect();
            if found.is_empty() {
                continue;
            }
            let clicked: Vec<bool> = found.iter().map(|p| here.iter().any(|c| holds(p, &piece.seeds, *c, spacing).is_some())).collect();
            // The side of this membrane the pits are wanted on: +1 that of
            // its normals, −1 the other, None when it cannot be told.
            let side: Option<f64> = match found.iter().zip(&clicked).find(|(_, c)| **c) {
                Some((p, _)) => Some(p.side as f64),
                None => {
                    if vol.is_none() {
                        vol = Some(density(file)?);
                    }
                    let (v, n) = vol.as_ref().unwrap();
                    let own = lean(v, *n, &piece.seeds, mask.px);
                    let mut turned = piece.seeds.clone();
                    alike::face(&mut turned, example, own).then(|| if dot(turned[0].axis, piece.seeds[0].axis) < 0.0 { -1.0 } else { 1.0 })
                }
            };
            for (mut pit, example) in found.into_iter().zip(clicked) {
                if side.is_some_and(|s| s * pit.side as f64 <= 0.0) {
                    continue;
                }
                pit.seeds = vec![];
                out.funnels.push(Funnel { tomogram: file.clone(), example, side_sure: side.is_some(), pit, px: mask.px });
            }
        }
    }
    // Stages compare across files as they are: ratios of lengths.
    out.funnels.sort_by(|a, b| a.pit.stage().total_cmp(&b.pit.stage()));
    Ok(out)
}

fn sample(vol: &[f32], n: [usize; 3], p: P) -> Option<f32> {
    if (0..3).any(|k| p[k] < 0.0 || p[k] > (n[k] - 1) as f64) {
        return None;
    }
    let i: [usize; 3] = std::array::from_fn(|k| (p[k].floor() as usize).min(n[k].saturating_sub(2)));
    let f: [f64; 3] = std::array::from_fn(|k| p[k] - i[k] as f64);
    let at = |dx: usize, dy: usize, dz: usize| vol[(i[0] + dx).min(n[0] - 1) + n[0] * ((i[1] + dy).min(n[1] - 1) + n[1] * (i[2] + dz).min(n[2] - 1))] as f64;
    let mut v = 0.0;
    for (dz, wz) in [(0, 1.0 - f[2]), (1, f[2])] {
        for (dy, wy) in [(0, 1.0 - f[1]), (1, f[1])] {
            for (dx, wx) in [(0, 1.0 - f[0]), (1, f[0])] {
                v += wx * wy * wz * at(dx, dy, dz);
            }
        }
    }
    Some(v as f32)
}

/// A picture of a pit, `TILE` pixels square, grey: the section through its
/// mouth along its axis, the membrane running across the upper part and
/// the pit pointing down. Of the sections along the axis the one nearest
/// to the tomogram's own sections is taken (they are the sharpest), a few
/// voxels thick.
pub fn tile(vol: &[f32], n: [usize; 3], pit: &Pit) -> Vec<u8> {
    let a = pit.axis;
    let u = cross([0.0, 0.0, 1.0], a);
    let len = dot(u, u).sqrt();
    let u = if len > 0.2 { u.map(|v| v / len) } else { [1.0, 0.0, 0.0] };
    let w = cross(a, u);
    let half = 0.9 * pit.mouth.max(pit.bulb).max(pit.depth).max(4.0);
    let step = 2.0 * half / TILE as f64;
    let thick = (0.1 * pit.mouth).max(1.0);
    let mut v: Vec<Option<f32>> = Vec::with_capacity(TILE * TILE);
    for j in 0..TILE {
        for i in 0..TILE {
            let (x, y) = ((i as f64 - 0.5 * TILE as f64) * step, (j as f64 - 0.3 * TILE as f64) * step);
            let (mut sum, mut count) = (0.0f32, 0);
            for t in [-thick, 0.0, thick] {
                if let Some(s) = sample(vol, n, std::array::from_fn(|k| pit.mouth_at[k] + x * u[k] + y * a[k] + t * w[k])) {
                    sum += s;
                    count += 1;
                }
            }
            v.push((count > 0).then(|| sum / count as f32));
        }
    }
    let mut known: Vec<f32> = v.iter().flatten().copied().collect();
    if known.is_empty() {
        return vec![128; TILE * TILE];
    }
    known.sort_by(f32::total_cmp);
    let (lo, hi) = (known[known.len() / 100], known[known.len() - 1 - known.len() / 100]);
    let mid = known[known.len() / 2];
    let span = (hi - lo).max(1e-6);
    v.iter().map(|x| (255.0 * (x.unwrap_or(mid) - lo) / span).clamp(0.0, 255.0) as u8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const N: [usize; 3] = [150, 60, 64];
    const LEVEL: f64 = 44.0;

    /// A solid above a plane at z = `LEVEL` with pits (x, depth, half
    /// width) in it at y = 30, and, where `bud` is given, a ball of radius
    /// 10 on a neck of radius 4 at that x.
    fn solid(p: P, dents: &[(f64, f64, f64)], bud: Option<f64>) -> bool {
        let r2 = |x0: f64| (p[0] - x0).powi(2) + (p[1] - 30.0).powi(2);
        let floor = LEVEL - dents.iter().map(|(x, d, s)| d * (-r2(*x) / (2.0 * s * s)).exp()).sum::<f64>();
        let ball = bud.is_some_and(|x| r2(x) + (p[2] - (LEVEL - 20.0)).powi(2) <= 100.0);
        let neck = bud.is_some_and(|x| r2(x) <= 16.0 && p[2] > LEVEL - 20.0);
        p[2] > floor || ball || neck
    }

    /// A tomogram of that solid (dark, in noise) with its border as the
    /// membrane of the label file beside it. Voxel 20 Å.
    fn file(dir: &Path, name: &str, dents: &[(f64, f64, f64)], bud: Option<f64>) -> String {
        let path = dir.join(name).to_string_lossy().into_owned();
        let mut state = 7u32;
        let mut noise = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let at = |i: usize| [(i % N[0]) as f64, (i / N[0] % N[1]) as f64, (i / (N[0] * N[1])) as f64];
        let total = N[0] * N[1] * N[2];
        let inside: Vec<bool> = (0..total).map(|i| solid(at(i), dents, bud)).collect();
        let mask: Vec<bool> = (0..total)
            .map(|i| {
                let p = at(i);
                (-1..=1).any(|dz| (-1..=1).any(|dy| (-1..=1).any(|dx| solid([p[0] + dx as f64 * 1.4, p[1] + dy as f64 * 1.4, p[2] + dz as f64 * 1.4], dents, bud) != inside[i])))
            })
            .collect();
        let vol: Vec<f32> = (0..total).map(|i| -(if mask[i] { 1.0 } else { 0.0 }) - (if inside[i] { 0.5 } else { 0.0 }) + 0.4 * noise()).collect();
        crate::tomo::output::write_mrc(&path, &vol, N, 20.0, false).unwrap();
        crate::membrane::write_labels(&path, N, &mask, None, 20.0).unwrap();
        path
    }

    fn click(tomogram: &str, at: P) -> Mark {
        Mark { kind: Kind::Pit, tomogram: tomogram.into(), points: vec![at], flip: false, strokes: vec![] }
    }

    #[test]
    fn a_click_into_one_pit_finds_the_pits_of_every_tomogram_in_the_order_of_their_stage() {
        let dir = tempfile::tempdir().unwrap();
        // a: a dent, a pit (clicked) and a bud. b: two pits of other depths.
        let a = file(dir.path(), "a.mrc", &[(25.0, 9.0, 7.0), (70.0, 18.0, 6.0)], Some(118.0));
        let b = file(dir.path(), "b.mrc", &[(40.0, 13.0, 6.0), (105.0, 22.0, 6.0)], None);
        let found = find(&[click(&a, [70.0, 30.0, LEVEL - 8.0])], &[a.clone(), b.clone()]).unwrap();
        let describe: Vec<String> = found
            .funnels
            .iter()
            .map(|f| format!("{} x {:.0} depth {:.1} mouth {:.1} stage {:.2} example {} sure {}", Path::new(&f.tomogram).file_name().unwrap().to_string_lossy(), f.pit.mouth_at[0], f.pit.depth, f.pit.mouth, f.pit.stage(), f.example, f.side_sure))
            .collect();
        assert!(found.unsegmented.is_empty());
        assert_eq!(found.funnels.len(), 5, "wide {} {describe:#?}", found.wide_a);
        // One example: the pit that was clicked.
        let examples: Vec<&Funnel> = found.funnels.iter().filter(|f| f.example).collect();
        assert!(examples.len() == 1 && examples[0].tomogram == a && (examples[0].pit.mouth_at[0] - 70.0).abs() < 4.0, "{describe:#?}");
        // All point down from the plane, and the side of b was told from the density.
        assert!(found.funnels.iter().all(|f| f.pit.axis[2] < -0.9 && f.side_sure), "{describe:#?}");
        // In the order of the stage: the dent first, the bud last, depths rising between.
        assert!((found.funnels[0].pit.mouth_at[0] - 25.0).abs() < 4.0 && found.funnels[0].tomogram == a, "{describe:#?}");
        assert!((found.funnels[4].pit.mouth_at[0] - 118.0).abs() < 4.0 && found.funnels[4].tomogram == a, "{describe:#?}");
        let stages: Vec<f64> = found.funnels.iter().map(|f| f.pit.stage()).collect();
        assert!(stages.windows(2).all(|w| w[0] <= w[1]), "{describe:#?}");
    }

    #[test]
    fn a_click_on_a_plain_membrane_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let a = file(dir.path(), "a.mrc", &[(70.0, 18.0, 6.0)], None);
        let e = find(&[click(&a, [20.0, 30.0, LEVEL])], &[a.clone()]).err().expect("no pit there");
        assert!(e.to_string().contains("凹陷"), "{e}");
    }

    #[test]
    fn the_picture_of_a_pit_shows_it_pointing_down() {
        let dir = tempfile::tempdir().unwrap();
        let a = file(dir.path(), "a.mrc", &[(70.0, 18.0, 6.0)], None);
        let found = find(&[click(&a, [70.0, 30.0, LEVEL - 8.0])], &[a.clone()]).unwrap();
        let (vol, n) = density(&a).unwrap();
        let pic = tile(&vol, n, &found.funnels[0].pit);
        assert_eq!(pic.len(), TILE * TILE);
        // The solid (dark) lies above the mouth and fills the pit below it
        // in the middle; beside the pit at the same height it is open (bright).
        let mean = |x0: usize, y0: usize| (0..36).map(|k| pic[(y0 + k / 6) * TILE + x0 + k % 6] as f64).sum::<f64>() / 36.0;
        let (in_pit, beside, above) = (mean(TILE / 2 - 3, TILE * 3 / 10 + 20), mean(8, TILE * 3 / 10 + 20), mean(TILE / 2 - 3, 4));
        assert!(beside > in_pit + 30.0 && beside > above + 30.0, "in {in_pit} beside {beside} above {above}");
    }
}
