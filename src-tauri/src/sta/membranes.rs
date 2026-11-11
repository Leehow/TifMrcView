//! "Paint a few membranes, get all of that kind": the membranes the user
//! painted (`paint`) are examples; every other membrane of the
//! segmentations that is like them (`alike::like`) is found in all the
//! tomograms given, and each is turned to the side the examples face
//! (`alike::lean`, `alike::face`). The result is for the user to check
//! before anything is averaged: a membrane can be dropped or flipped.
//!
//! Everything is in voxels of the files the membranes were painted on and
//! segmented in (a tomogram or its look file); `marks` carries the seeds
//! over to the tomograms that are averaged.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::alike::{self, Traits};
use super::marks::{Kind, Mark};
use super::paint;
use super::shape::Seed;
use super::sheet;

/// A membrane is this thick at most, Å: the density beside it is measured
/// from half of this (and a voxel) off its middle.
const THICK_A: f64 = 50.0;

/// One membrane to average from.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Membrane {
    /// The file it was found in (as the marks name it).
    pub tomogram: String,
    /// One the user painted.
    pub example: bool,
    pub voxels: usize,
    pub traits: Traits,
    /// Whether the side was told from the density beside the membrane
    /// (always true for an example: the user saw its arrows).
    pub side_sure: bool,
    /// Middle of the membrane, voxels (x, y, z).
    pub centre: [f64; 3],
    /// Sampling points with the normals as they will be used.
    #[serde(skip)]
    pub seeds: Vec<Seed>,
    /// Sorted voxel indices, for the highlight.
    #[serde(skip)]
    pub mask: Vec<u32>,
}

/// What `find` gives.
pub struct Search {
    pub membranes: Vec<Membrane>,
    /// Files with no segmentation yet (they were not searched).
    pub unsegmented: Vec<String>,
}

fn centre(seeds: &[Seed]) -> [f64; 3] {
    let k = seeds.len().max(1) as f64;
    seeds.iter().fold([0.0; 3], |m, s| std::array::from_fn(|a| m[a] + s.pos[a] / k))
}

/// The density of a file, scaled to unit robust spread so that leans of
/// different tomograms compare.
pub(super) fn density(path: &str) -> Result<(Vec<f32>, [usize; 3])> {
    let (mut v, n, _) = crate::tomo::pipeline::load(path)?;
    let (median, sd) = super::pick::robust(&v);
    let sd = if sd > 0.0 { sd } else { 1.0 };
    v.iter_mut().for_each(|x| *x = (*x - median) / sd);
    Ok((v, n))
}

/// The membranes to average from: the painted ones of `marks` (kind
/// membrane) and all those like them in `files` (the files to search; the
/// marks' own files are searched whether listed or not). `size_a`: the size
/// of the particle on the membrane, Å.
pub fn find(marks: &[Mark], files: &[String], size_a: f64) -> Result<Search> {
    let painted: Vec<&Mark> = marks.iter().filter(|m| m.kind == Kind::Membrane && paint::has_stroke(m)).collect();
    let mut all: Vec<String> = vec![];
    for f in painted.iter().map(|m| &m.tomogram).chain(files) {
        if !all.contains(f) {
            all.push(f.clone());
        }
    }
    let mut out = Search { membranes: vec![], unsegmented: vec![] };
    // The examples first: their traits and the side their density lies on.
    let mut traits: Vec<Traits> = vec![];
    let mut leans: Vec<(f64, f64)> = vec![];
    // (file, voxels) of the examples, to leave them out of the search.
    let mut taken: Vec<(String, std::sync::Arc<Vec<u32>>)> = vec![];
    let mut volume: Option<(String, Vec<f32>, [usize; 3])> = None;
    fn load(volume: &mut Option<(String, Vec<f32>, [usize; 3])>, path: &str) -> Result<()> {
        if volume.as_ref().is_none_or(|v| v.0 != path) {
            let (v, n) = density(path)?;
            *volume = Some((path.to_string(), v, n));
        }
        Ok(())
    }
    let reach = |px: f64| {
        let from = 0.5 * THICK_A / px + 1.0;
        (from, from + (0.5 * size_a / px).max(2.0))
    };
    for m in &painted {
        let Some((mask, voxels)) = paint::touched_of(m)? else { continue };
        if voxels.is_empty() {
            continue;
        }
        let spacing = 0.5 * size_a / mask.px;
        let mut seeds = sheet::seeds(&voxels, mask.n, spacing);
        if m.flip {
            seeds.iter_mut().for_each(|s| s.axis = s.axis.map(|v| -v));
        }
        let Some(t) = alike::traits(&seeds, voxels.len(), spacing) else { continue };
        load(&mut volume, &m.tomogram)?;
        let (_, v, n) = volume.as_ref().unwrap();
        let (from, to) = reach(mask.px);
        leans.push(alike::lean(v, *n, &seeds, from, to));
        traits.push(t);
        taken.push((m.tomogram.clone(), voxels.clone()));
        out.membranes.push(Membrane { tomogram: m.tomogram.clone(), example: true, voxels: voxels.len(), traits: t, side_sure: true, centre: centre(&seeds), seeds, mask: voxels.to_vec() });
    }
    if traits.is_empty() {
        return Ok(out);
    }
    // The examples' side as one lean: the mean, with the error of a mean.
    let k = leans.len() as f64;
    let example = (leans.iter().map(|l| l.0).sum::<f64>() / k, (leans.iter().map(|l| l.1 * l.1).sum::<f64>()).sqrt() / k);
    for file in &all {
        let Some(mask) = paint::mask_of(file)? else {
            out.unsegmented.push(file.clone());
            continue;
        };
        let spacing = 0.5 * size_a / mask.px;
        let (from, to) = reach(mask.px);
        let mut loaded = false;
        for mut piece in alike::pieces(&mask.mask, mask.n, spacing, 8) {
            // A piece most of which the user painted is an example already.
            let painted_share = taken
                .iter()
                .filter(|(f, _)| f == file)
                .map(|(_, v)| piece.voxels.iter().filter(|i| v.binary_search(i).is_ok()).count())
                .sum::<usize>() as f64
                / piece.voxels.len() as f64;
            if painted_share > 0.5 || !alike::like(&traits, &piece.traits) {
                continue;
            }
            if !loaded {
                load(&mut volume, file)?;
                loaded = true;
            }
            let (_, v, n) = volume.as_ref().unwrap();
            let own = alike::lean(v, *n, &piece.seeds, from, to);
            let side_sure = alike::face(&mut piece.seeds, example, own);
            out.membranes.push(Membrane {
                tomogram: file.clone(),
                example: false,
                voxels: piece.voxels.len(),
                traits: piece.traits,
                side_sure,
                centre: centre(&piece.seeds),
                seeds: piece.seeds,
                mask: piece.voxels,
            });
        }
    }
    Ok(out)
}

/// One vesicle to average from.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ball {
    pub tomogram: String,
    /// One the user marked.
    pub example: bool,
    /// Centre and radius (of the membrane's middle), voxels.
    pub centre: [f64; 3],
    pub radius: f64,
    pub side_sure: bool,
    /// True when the normals point inward (the density beside the membrane
    /// lies inside, or the user flipped the example).
    pub inward: bool,
    #[serde(skip)]
    pub seeds: Vec<Seed>,
}

/// What `find_balls` gives.
pub struct BallSearch {
    pub balls: Vec<Ball>,
    pub unsegmented: Vec<String>,
}

/// Vesicles within this factor of the examples' radii are of their kind.
const BALL_SIZE: f64 = 1.3;

/// The vesicles (centre, radius in voxels) of a segmented file: those the
/// segmentation wrote down (`<file>.membranes.json`), else found again in
/// its label map. None when the file is not segmented.
fn vesicles_of(file: &str) -> Result<Option<Vec<([f64; 3], f64)>>> {
    let Some(mask) = paint::mask_of(file)? else { return Ok(None) };
    let of = |v: &crate::membrane::vesicles::Vesicle| (v.center.map(|x| x as f64), 0.5 * v.diameter as f64 * 10.0 / mask.px);
    if let Some(r) = std::fs::read(format!("{file}.membranes.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
        if let Ok(v) = serde_json::from_value::<Vec<crate::membrane::vesicles::Vesicle>>(r["vesicles"].clone()) {
            return Ok(Some(v.iter().map(of).collect()));
        }
    }
    let (found, _) = crate::membrane::vesicles::find(&mask.mask, mask.n, mask.px as f32);
    Ok(Some(found.iter().map(of).collect()))
}

/// The vesicles to average from: those the sphere marks of `marks` are on
/// (a mark is a centre and a point on the membrane; the vesicle found there
/// replaces the clicks, which are rough) and all of about their size in
/// `files`, each turned to the side the examples face. A marked sphere with
/// no segmented vesicle under it is used as drawn. `size_a`: the size of
/// the particle on the membrane, Å; `pixel` gives a file's voxel size, Å.
pub fn find_balls(marks: &[Mark], files: &[String], size_a: f64, pixel: &dyn Fn(&str) -> Option<f64>) -> Result<BallSearch> {
    let spheres: Vec<&Mark> = marks.iter().filter(|m| m.kind == Kind::Sphere && m.points.len() >= 2).collect();
    let mut all: Vec<String> = vec![];
    for f in spheres.iter().map(|m| &m.tomogram).chain(files) {
        if !all.contains(f) {
            all.push(f.clone());
        }
    }
    let dist = |a: [f64; 3], b: [f64; 3]| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt();
    let mut out = BallSearch { balls: vec![], unsegmented: vec![] };
    let mut known: Vec<(String, Option<Vec<([f64; 3], f64)>>)> = vec![];
    for f in &all {
        known.push((f.clone(), vesicles_of(f)?));
    }
    let mut volume: Option<(String, Vec<f32>, [usize; 3])> = None;
    let mut lean_of = |file: &str, seeds: &[Seed], px: f64| -> Result<(f64, f64)> {
        if volume.as_ref().is_none_or(|v| v.0 != file) {
            let (v, n) = density(file)?;
            volume = Some((file.to_string(), v, n));
        }
        let (_, v, n) = volume.as_ref().unwrap();
        let from = 0.5 * THICK_A / px + 1.0;
        Ok(alike::lean(v, *n, seeds, from, from + (0.5 * size_a / px).max(2.0)))
    };
    let ball = |c: [f64; 3], r: f64, px: f64| super::shape::ellipsoid(c, [r; 3], 0.5 * size_a / px);
    // The examples: radii in Å (files may differ in voxel size), and their lean.
    let mut radii_a: Vec<f64> = vec![];
    let mut leans: Vec<(f64, f64)> = vec![];
    // (file, index of the segmented vesicle) taken by an example.
    let mut taken: Vec<(String, usize)> = vec![];
    for m in &spheres {
        let Some(px) = pixel(&m.tomogram) else { continue };
        let (mut c, mut r) = (m.points[0], dist(m.points[0], m.points[1]));
        if r < 1.0 {
            continue;
        }
        if let Some(list) = known.iter().find(|k| k.0 == m.tomogram).and_then(|k| k.1.as_ref()) {
            // The segmented vesicle whose centre the marked sphere holds.
            if let Some((i, v)) = list.iter().enumerate().filter(|(_, v)| dist(v.0, c) < 0.5 * r.max(v.1)).min_by(|a, b| dist(a.1 .0, c).total_cmp(&dist(b.1 .0, c))) {
                (c, r) = *v;
                taken.push((m.tomogram.clone(), i));
            }
        }
        let mut seeds = ball(c, r, px);
        if m.flip {
            seeds.iter_mut().for_each(|s| s.axis = s.axis.map(|v| -v));
        }
        leans.push(lean_of(&m.tomogram, &seeds, px)?);
        radii_a.push(r * px);
        out.balls.push(Ball { tomogram: m.tomogram.clone(), example: true, centre: c, radius: r, side_sure: true, inward: m.flip, seeds });
    }
    if radii_a.is_empty() {
        return Ok(out);
    }
    let k = leans.len() as f64;
    let example = (leans.iter().map(|l| l.0).sum::<f64>() / k, (leans.iter().map(|l| l.1 * l.1).sum::<f64>()).sqrt() / k);
    let (small, large) = (radii_a.iter().copied().fold(f64::MAX, f64::min) / BALL_SIZE, radii_a.iter().copied().fold(0.0, f64::max) * BALL_SIZE);
    // Without a lean to go by, the others face as most examples do.
    let mostly_inward = 2 * out.balls.iter().filter(|b| b.inward).count() > out.balls.len();
    for (file, list) in &known {
        let Some(list) = list else {
            out.unsegmented.push(file.clone());
            continue;
        };
        let Some(px) = pixel(file) else { continue };
        for (i, (c, r)) in list.iter().enumerate() {
            if taken.iter().any(|t| t.0 == *file && t.1 == i) || r * px < small || r * px > large {
                continue;
            }
            // Outward first; `face` turns them when the density says inward.
            let mut seeds = ball(*c, *r, px);
            let own = lean_of(file, &seeds, px)?;
            let before = seeds.first().map(|s| s.axis);
            let side_sure = alike::face(&mut seeds, example, own);
            let mut inward = side_sure && seeds.first().map(|s| s.axis) != before;
            if !side_sure && mostly_inward {
                seeds.iter_mut().for_each(|s| s.axis = s.axis.map(|v| -v));
                inward = true;
            }
            out.balls.push(Ball { tomogram: file.clone(), example: false, centre: *c, radius: *r, side_sure, inward, seeds });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::paint::Stroke;

    const N: usize = 96;

    /// A tomogram with vesicles (centre, radius) and a coat outside each
    /// (inside when `inward`), and its label file.
    fn file(dir: &std::path::Path, name: &str, shells: &[([f64; 3], f64)], inward: &[bool]) -> String {
        let path = dir.join(name).to_string_lossy().into_owned();
        let mut state = 11u32;
        let mut noise = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let at = |i: usize| [(i % N) as f64, (i / N % N) as f64, (i / (N * N)) as f64];
        let off = |p: [f64; 3], c: [f64; 3], r: f64| (0..3).map(|k| (p[k] - c[k]).powi(2)).sum::<f64>().sqrt() - r;
        let mask: Vec<bool> = (0..N * N * N).map(|i| shells.iter().any(|(c, r)| off(at(i), *c, *r).abs() <= 1.5)).collect();
        let vol: Vec<f32> = (0..N * N * N)
            .map(|i| {
                let coat = shells.iter().zip(inward).any(|((c, r), inw)| {
                    let d = off(at(i), *c, *r);
                    if *inw { (-8.0..=-4.0).contains(&d) } else { (4.0..=8.0).contains(&d) }
                });
                // Dark is dense, as in a tomogram.
                -(if mask[i] { 1.0 } else { 0.0 }) - (if coat { 0.5 } else { 0.0 }) + 0.6 * noise()
            })
            .collect();
        crate::tomo::output::write_mrc(&path, &vol, [N; 3], 10.0, false).unwrap();
        crate::membrane::write_labels(&path, [N; 3], &mask, None, 10.0).unwrap();
        path
    }

    fn painted(tomogram: &str, at: [f64; 3]) -> Mark {
        Mark { kind: Kind::Membrane, tomogram: tomogram.into(), points: vec![], flip: false, strokes: vec![Stroke { points: vec![at], cut: false }] }
    }

    #[test]
    fn one_painted_vesicle_finds_the_others_and_turns_them_its_way() {
        let dir = tempfile::tempdir().unwrap();
        // First file: the example (radius 14), one like it, one much larger.
        let a = file(dir.path(), "a.mrc", &[([22.0, 22.0, 30.0], 14.0), ([70.0, 26.0, 34.0], 15.0), ([48.0, 66.0, 60.0], 27.0)], &[false; 3]);
        // Second file: one like it whose coat is inside.
        let b = file(dir.path(), "b.mrc", &[([40.0, 40.0, 40.0], 14.0)], &[true]);
        let c = dir.path().join("c.mrc").to_string_lossy().into_owned();
        crate::tomo::output::write_mrc(&c, &vec![0.0; 8 * 8 * 8], [8; 3], 10.0, false).unwrap();
        let mark = painted(&a, [8.0, 22.0, 30.0]);
        let found = find(&[mark], &[a.clone(), b.clone(), c.clone()], 80.0).unwrap();
        assert_eq!(found.unsegmented, vec![c]);
        let near = |m: &Membrane, p: [f64; 3]| (0..3).map(|k| (m.centre[k] - p[k]).powi(2)).sum::<f64>().sqrt() < 3.0;
        assert_eq!(found.membranes.len(), 3, "{:?}", found.membranes.iter().map(|m| (m.tomogram.clone(), m.centre, m.traits)).collect::<Vec<_>>());
        assert!(found.membranes[0].example && near(&found.membranes[0], [22.0, 22.0, 30.0]));
        let twin = found.membranes.iter().find(|m| m.tomogram == a && !m.example).unwrap();
        assert!(near(twin, [70.0, 26.0, 34.0]) && twin.side_sure);
        // Coat outside as on the example: normals outward.
        assert!(twin.seeds.iter().all(|s| (0..3).map(|k| s.axis[k] * (s.pos[k] - twin.centre[k])).sum::<f64>() > 0.0));
        // Coat inside: the normals follow the coat, inward.
        let other = found.membranes.iter().find(|m| m.tomogram == b).unwrap();
        assert!(other.side_sure);
        assert!(other.seeds.iter().all(|s| (0..3).map(|k| s.axis[k] * (s.pos[k] - other.centre[k])).sum::<f64>() < 0.0));
    }

    #[test]
    fn two_marked_vesicles_find_those_of_their_size_and_their_side() {
        let dir = tempfile::tempdir().unwrap();
        // Coats outside, except on the last vesicle of the size wanted.
        let shells = [([24.0, 24.0, 30.0], 14.0), ([70.0, 26.0, 34.0], 15.0), ([26.0, 70.0, 60.0], 14.5), ([66.0, 68.0, 30.0], 6.0), ([68.0, 68.0, 70.0], 15.0)];
        let a = file(dir.path(), "a.mrc", &shells, &[false, false, false, false, true]);
        // Rough clicks: centres a few voxels off, radii by eye.
        let sphere = |c: [f64; 3], r: f64| Mark { kind: Kind::Sphere, tomogram: a.clone(), points: vec![c, [c[0] + r, c[1], c[2]]], flip: false, strokes: vec![] };
        let marks = [sphere([26.0, 22.0, 31.0], 12.0), sphere([68.0, 27.0, 33.0], 17.0)];
        let found = find_balls(&marks, &[a.clone()], 80.0, &|_| Some(10.0)).unwrap();
        assert!(found.unsegmented.is_empty());
        let near = |b: &Ball, p: [f64; 3]| (0..3).map(|k| (b.centre[k] - p[k]).powi(2)).sum::<f64>().sqrt() < 2.5;
        let describe: Vec<_> = found.balls.iter().map(|b| (b.centre.map(|v| v.round()), b.radius.round(), b.example, b.inward, b.side_sure)).collect();
        assert_eq!(found.balls.len(), 4, "{describe:?}");
        // The examples sit where the segmentation has them, not where clicked.
        assert!(found.balls[0].example && near(&found.balls[0], shells[0].0) && (found.balls[0].radius - 14.0).abs() < 1.5, "{describe:?}");
        assert!(found.balls[1].example && near(&found.balls[1], shells[1].0), "{describe:?}");
        let third = found.balls.iter().find(|b| near(b, shells[2].0)).expect("the third of the size");
        assert!(!third.example && third.side_sure && !third.inward, "{describe:?}");
        assert!(!found.balls.iter().any(|b| near(b, shells[3].0)), "the small one is left out: {describe:?}");
        let last = found.balls.iter().find(|b| near(b, shells[4].0)).expect("the one coated inside");
        assert!(last.side_sure && last.inward, "{describe:?}");
        assert!(last.seeds.iter().all(|s| (0..3).map(|k| s.axis[k] * (s.pos[k] - last.centre[k])).sum::<f64>() < 0.0));
    }
}
