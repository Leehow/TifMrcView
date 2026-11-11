//! "Mark it yourself": the user marks on a tomogram what to average (single
//! particles, a filament, a membrane, a vesicle) and `run_marks` averages it.
//! A mark is a few clicks; `shape` completes the rest (a smooth curve, a
//! surface through the strokes, a sphere), `project::run_drawn` does the work.
//!
//! Coordinates. A click is a voxel (x, y, z) of the file the user had open:
//! the tomogram itself or its clear "look" companion. `run_drawn` wants
//! voxels of `Source::path`; clicks made on the look file are carried over
//! with `project::to_raw` (unbinned voxels on both sides).
//!
//! Particle size is never typed. A sphere takes it from the radius drawn
//! (a protein on a vesicle is a fraction of its radius, `SPHERE_SHARE`);
//! lines and surfaces carry no size, so the workspace offers three
//! (`Size`). Single points have no known direction, which `run_drawn`
//! needs: when the marks are points only they make one template card
//! instead (`project::card_from_points`) and the ordinary template run
//! follows (`auto::auto_points`). Points beside shapes are left out of the
//! run of the shapes (`route`): the two are averaged separately.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::auto::{auto_points, default_out, live_png, note_json, raw_inputs, result_value, resolve_tomogram, write_used, Device, Emit, Note, Outcome, DRAWN, MARKED};
use super::project::{self as sp, to_raw, Drawn, Source};
use super::shape;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Point,
    Line,
    Surface,
    Sphere,
}

/// One thing marked: the clicks, in voxels of the file `tomogram`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub kind: Kind,
    pub tomogram: String,
    pub points: Vec<[f64; 3]>,
    /// Surfaces and spheres: which side faces out. Each seed starts along the
    /// surface normal, so the user says which side is which (as in Dynamo).
    /// False: a sphere's normals point outward and a surface's follow the
    /// first one (`shape::surface`); true: every axis is reversed.
    #[serde(default)]
    pub flip: bool,
}

/// How large the particles on a line or surface are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    Small,
    Medium,
    Large,
}

impl Size {
    pub fn angstrom(self) -> f64 {
        match self {
            Size::Small => 130.0,
            Size::Medium => 220.0,
            Size::Large => 360.0,
        }
    }
    pub fn parse(s: Option<&str>) -> Size {
        match s {
            Some("small") => Size::Small,
            Some("large") => Size::Large,
            _ => Size::Medium,
        }
    }
}

/// What `marks.json` holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default)]
    pub marks: Vec<Mark>,
    #[serde(default)]
    pub size: Option<String>,
}

/// A protein on a vesicle is about this share of the vesicle's radius.
const SPHERE_SHARE: f64 = 0.5;
/// Particle sizes `run_drawn` can serve, Å.
const SIZE_RANGE: (f64, f64) = (90.0, 490.0);
/// Seeds beyond this are thinned out evenly (memory: one cube each).
const MOST_SEEDS: usize = 3000;

fn marks_file(out: &Path) -> PathBuf {
    out.join("marks.json")
}

/// The marks saved in an output folder (empty when none).
pub fn load(out: &Path) -> Saved {
    std::fs::read(marks_file(out)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(out: &Path, saved: &Saved) -> Result<()> {
    std::fs::create_dir_all(out).with_context(|| format!("无法创建 {}", out.display()))?;
    std::fs::write(marks_file(out), serde_json::to_vec_pretty(saved)?).context("无法保存标注")
}

/// The source a mark was made on and the mark's points in voxels of that
/// source's own tomogram.
fn to_source(mark: &Mark, inputs: &[String], sources: &[Source]) -> Option<(usize, Vec<[f64; 3]>)> {
    let same = |a: &str, b: &str| a == b || std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(x, y)| x == y);
    for (i, s) in sources.iter().enumerate() {
        if same(&mark.tomogram, &s.path) || inputs.get(i).is_some_and(|p| same(&mark.tomogram, p) && s.look.as_ref().is_none_or(|l| l.path != *p)) {
            return Some((i, mark.points.clone()));
        }
    }
    for (i, s) in sources.iter().enumerate() {
        if let Some(look) = s.look.as_ref().filter(|l| same(&mark.tomogram, &l.path)) {
            return Some((i, mark.points.iter().map(|p| to_raw(*p, 1, 1, look)).collect()));
        }
    }
    None
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt()
}

/// A surface mark's clicks as strokes: runs of clicks on one section are a
/// stroke; clicks all on different sections are one stroke.
fn strokes(points: &[[f64; 3]]) -> Vec<Vec<[f64; 3]>> {
    let mut out: Vec<Vec<[f64; 3]>> = vec![];
    for p in points {
        match out.last_mut() {
            Some(s) if s[0][2].round() == p[2].round() => s.push(*p),
            _ => out.push(vec![*p]),
        }
    }
    if out.iter().all(|s| s.len() < 2) {
        return vec![points.to_vec()];
    }
    out
}

/// The viewer may have shown the look file, and the workspace lists it among
/// the tomograms: it is not averaged as one of its own beside the tomogram
/// it belongs to.
fn without_looks(inputs: &[String], sources: Vec<Source>) -> (Vec<String>, Vec<Source>) {
    let looks: Vec<String> = sources.iter().filter_map(|s| s.look.as_ref().map(|l| l.path.clone())).collect();
    inputs.iter().cloned().zip(sources).filter(|(_, s)| !looks.contains(&s.path)).unzip()
}

/// What a drawn average is made of, as `auto::used_of` reads it: how many
/// lines, surfaces and spheres were complete enough to be used, and the size (Å).
pub fn shapes_used(marks: &[Mark], size_a: f64) -> Value {
    let n = |k: Kind| marks.iter().filter(|m| m.kind == k && m.points.len() >= 2).count();
    json!({ "kind": "shapes", "lines": n(Kind::Line), "surfaces": n(Kind::Surface), "spheres": n(Kind::Sphere), "sizeA": size_a.round() })
}

/// What a set of marks is averaged by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Only points: one template card from them, then the template run.
    Points,
    /// Lines, surfaces and spheres (or nothing marked yet): `run_drawn`.
    Shapes,
    /// Both: `run_drawn` on the shapes; the points are not averaged in it.
    Mixed,
}

pub fn route(marks: &[Mark]) -> Route {
    let points = marks.iter().filter(|m| m.kind == Kind::Point).count();
    match (points, marks.len() - points) {
        (0, _) => Route::Shapes,
        (_, 0) => Route::Points,
        _ => Route::Mixed,
    }
}

/// The clicks of the point marks as `project::card_from_points` takes them:
/// (source, voxels of that source's tomogram).
pub fn points_of(marks: &[Mark], inputs: &[String], sources: &[Source]) -> Result<Vec<(usize, [f64; 3])>> {
    let mut out = vec![];
    for m in marks.iter().filter(|m| m.kind == Kind::Point) {
        let Some((i, p)) = to_source(m, inputs, sources) else { bail!("有标注所在的断层图没有被勾选") };
        out.extend(p.into_iter().map(|p| (i, p)));
    }
    Ok(out)
}

/// The seeds of one mark (its points `p` in the voxels of one frame, particles
/// `spacing` voxels apart) and whether it is a line, with the mark's `flip`
/// applied: a flipped surface or sphere gives exactly the reversed axes at
/// the same positions.
fn seeds_of(m: &Mark, p: &[[f64; 3]], spacing: f64) -> Result<(Vec<shape::Seed>, bool)> {
    let mut line = false;
    let mut seeds = match m.kind {
        Kind::Line => {
            line = true;
            shape::line(p, spacing)
        }
        Kind::Surface => shape::surface(&strokes(p), spacing, None),
        Kind::Sphere => {
            let r = dist(p[0], p[1]);
            if r < 1.0 {
                bail!("球的中心和球面上的点太近了");
            }
            shape::ellipsoid(p[0], [r; 3], spacing)
        }
        Kind::Point => vec![],
    };
    if m.flip && matches!(m.kind, Kind::Surface | Kind::Sphere) {
        for s in &mut seeds {
            s.axis = s.axis.map(|a| -a);
        }
    }
    Ok((seeds, line))
}

/// What the marks give `run_drawn`: the seeds, whether the sense of the axes
/// is unknown, and the particle size (Å). `pixels` are the voxel sizes (Å)
/// of the sources' tomograms.
pub fn drawn_of(marks: &[Mark], inputs: &[String], sources: &[Source], pixels: &[f64], size: Size) -> Result<(Vec<Drawn>, bool, f64)> {
    let mut drawn: Vec<Drawn> = vec![];
    let mut both = false;
    let mut radii: Vec<f64> = vec![];
    let mut with_size = false;
    let mut points_only = 0;
    // Sphere radius decides the size, so sizes are known before seeds are cut.
    let placed: Vec<(&Mark, usize, Vec<[f64; 3]>)> = marks.iter().filter_map(|m| to_source(m, inputs, sources).map(|(i, p)| (m, i, p))).collect();
    if placed.len() < marks.len() {
        bail!("有标注所在的断层图没有被勾选");
    }
    for (m, i, p) in &placed {
        match m.kind {
            Kind::Sphere if p.len() >= 2 => radii.push(dist(p[0], p[1]) * pixels[*i]),
            Kind::Line | Kind::Surface => with_size = true,
            Kind::Point => points_only += 1,
            _ => {}
        }
    }
    let usable = placed.iter().filter(|(m, _, p)| match m.kind {
        Kind::Point => false,
        Kind::Line => p.len() >= 2,
        Kind::Surface => p.len() >= 2,
        Kind::Sphere => p.len() >= 2,
    });
    if usable.clone().next().is_none() {
        if points_only > 0 {
            bail!("只标了“点”还不能平均，请再标一条线、一片面或一个球");
        }
        bail!("还没有标够：线和面至少要点 2 个点，球要点中心和球面上的一点");
    }
    let size_a = if with_size || radii.is_empty() {
        size.angstrom()
    } else {
        (SPHERE_SHARE * radii.iter().sum::<f64>() / radii.len() as f64).clamp(SIZE_RANGE.0, SIZE_RANGE.1)
    };
    for (m, i, p) in usable {
        let spacing = 0.5 * size_a / pixels[*i];
        let (seeds, line) = seeds_of(m, p, spacing)?;
        both |= line;
        drawn.extend(seeds.into_iter().map(|s| Drawn { source: *i, pos: s.pos, axis: s.axis }));
    }
    if drawn.len() > MOST_SEEDS {
        let n = drawn.len();
        drawn = (0..MOST_SEEDS).map(|k| drawn[k * n / MOST_SEEDS].clone()).collect();
    }
    Ok((drawn, both, size_a))
}

/// Most seeds shown per mark in the overlay.
pub const MOST_PREVIEW: usize = 60;

/// The sampled points of one mark for the overlay.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Preview {
    /// Index of the mark in the list given.
    pub mark: usize,
    pub kind: Kind,
    pub seeds: Vec<PreviewSeed>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PreviewSeed {
    pub pos: [f64; 3],
    pub axis: [f64; 3],
}

/// Where the seeds of each line, surface and sphere mark are and which way
/// they face (thinned evenly to `MOST_PREVIEW` per mark), in the voxels of the
/// file `open` (a source's tomogram or its look file). `pixel` gives a
/// tomogram's voxel size (Å). Marks that cannot be placed or are not
/// complete are left out.
pub fn preview_of(marks: &[Mark], open: &str, size: Size, inputs: &[String], sources: &[Source], pixel: &dyn Fn(&str) -> Option<f64>) -> Vec<Preview> {
    let mut out = vec![];
    for (k, m) in marks.iter().enumerate() {
        if m.kind == Kind::Point || m.points.len() < 2 {
            continue;
        }
        let Some((i, p)) = to_source(m, inputs, sources) else { continue };
        let Some(px) = pixel(&sources[i].path).filter(|px| *px > 0.0) else { continue };
        let size_a = if m.kind == Kind::Sphere { (SPHERE_SHARE * dist(p[0], p[1]) * px).clamp(SIZE_RANGE.0, SIZE_RANGE.1) } else { size.angstrom() };
        let Ok((seeds, _)) = seeds_of(m, &p, 0.5 * size_a / px) else { continue };
        // Raw voxels to the open file's: its own, or the look file's (raw = scale × look + offset).
        let to_open = |v: [f64; 3]| -> Option<[f64; 3]> {
            if sources[i].path == open || inputs.get(i).is_some_and(|x| x == open) {
                return Some(v);
            }
            let look = sources[i].look.as_ref().filter(|l| l.path == open)?;
            Some(std::array::from_fn(|c| (v[c] - look.offset[c]) / look.scale))
        };
        let n = seeds.len();
        let take = n.min(MOST_PREVIEW);
        let seeds: Vec<PreviewSeed> = (0..take).filter_map(|j| seeds.get(j * n / take)).filter_map(|s| Some(PreviewSeed { pos: to_open(s.pos)?, axis: s.axis })).collect();
        if !seeds.is_empty() {
            out.push(Preview { mark: k, kind: m.kind, seeds });
        }
    }
    out
}

/// `preview_of` for the command: resolves the tomograms the marks and the
/// open file belong to.
pub fn preview_marks(marks: &[Mark], open: &str, size: Option<&str>) -> Result<Value> {
    let mut inputs: Vec<String> = vec![];
    for p in marks.iter().map(|m| m.tomogram.as_str()).chain(std::iter::once(open)) {
        for r in raw_inputs(&[p.to_string()])? {
            if !inputs.contains(&r) {
                inputs.push(r);
            }
        }
    }
    let sources: Vec<Source> = inputs.iter().map(|i| resolve_tomogram(i, &|_| {}).source()).collect();
    let (inputs, sources) = without_looks(&inputs, sources);
    let pixel = |path: &str| sp::pixel_of(path).ok();
    Ok(serde_json::to_value(preview_of(marks, open, Size::parse(size), &inputs, &sources, &pixel))?)
}

/// The workspace's run for marks: like `auto::run_inputs`, the same events
/// (`sta-progress`, `sta-live`, `sta-note`) and the same result value; the
/// average goes to `<out>/drawn` so an automatic result in `out` stays.
pub fn run_marks(inputs: &[String], marks: &[Mark], size: Option<&str>, out: Option<&str>, device: Device, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    // Averages are of the tomograms: a look companion handed over is replaced
    // by its tomogram (marks made on the look file are carried over by `to_source`).
    let inputs = raw_inputs(inputs)?;
    let Some(first) = inputs.first() else { bail!("请先选择断层图") };
    let out = match out.filter(|o| !o.is_empty()) {
        Some(o) => PathBuf::from(o),
        None => default_out(first),
    };
    let note = |n: Note| emit("sta-note", note_json(&n));
    let sources: Vec<Source> = inputs.iter().map(|i| resolve_tomogram(i, &note).source()).collect();
    let (inputs, sources) = without_looks(&inputs, sources);
    let route = route(marks);
    let before = Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(Instant::now);
    let last = Mutex::new((before, before));
    let due = |slot: usize, gap: u64| {
        let mut l = last.lock().unwrap();
        let t = if slot == 0 { &mut l.0 } else { &mut l.1 };
        let ok = t.elapsed() >= Duration::from_millis(gap);
        if ok {
            *t = Instant::now();
        }
        ok
    };
    let progress = |label: &str, done: usize, total: usize| {
        if done >= total || due(0, 100) {
            emit("sta-progress", json!({ "label": label, "done": done, "total": total }));
        }
    };
    let live = |vol: &[f32]| {
        if due(1, 350) {
            if let Some(png) = live_png(vol, 120) {
                emit("sta-live", json!({ "png": b64(&png) }));
            }
        }
    };
    if route == Route::Points {
        let points = points_of(marks, &inputs, &sources)?;
        let outcome = auto_points(&sources, &points, &out, device, &progress, &live, &note)?;
        let Outcome::Done(d) = outcome else { bail!("没有得到平均") };
        let dir = out.join(MARKED);
        return Ok(json!({
            "needsChoice": false, "report": serde_json::to_value(&d.report)?, "result": result_value(&dir, b64)?,
            "cards": [], "chosen": Value::Null, "auto": false, "out": dir, "trials": [], "drawn": true, "points": points.len(),
            "sizeA": d.card.measured_a,
        }));
    }
    let pixels: Vec<f64> = sources.iter().map(|s| sp::pixel_of(&s.path)).collect::<Result<_>>()?;
    let (drawn, both, size_a) = drawn_of(marks, &inputs, &sources, &pixels, Size::parse(size))?;
    let dir = out.join(DRAWN);
    let report = sp::run_drawn(&sources, &drawn, both, size_a, &dir.to_string_lossy(), &progress, &live)?;
    write_used(&dir, &shapes_used(marks, size_a));
    Ok(json!({
        "needsChoice": false, "report": serde_json::to_value(&report)?, "result": result_value(&dir, b64)?,
        "cards": [], "chosen": Value::Null, "auto": false, "out": dir, "trials": [], "drawn": true, "sizeA": size_a,
        "pointsLeftOut": route == Route::Mixed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tomo::dataset::Look;

    fn source(path: &str, look: Option<Look>) -> Source {
        Source { path: path.into(), tilts: [-60.0, 60.0], look, series: None }
    }
    fn mark(kind: Kind, tomogram: &str, points: &[[f64; 3]]) -> Mark {
        Mark { kind, tomogram: tomogram.into(), points: points.to_vec(), flip: false }
    }
    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-9)
    }

    #[test]
    fn marks_on_the_tomogram_stay_as_they_are() {
        let sources = vec![source("/t/a.mrc", None)];
        let inputs = vec!["/t/a.mrc".to_string()];
        let m = mark(Kind::Line, "/t/a.mrc", &[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        let (i, p) = to_source(&m, &inputs, &sources).unwrap();
        assert_eq!(i, 0);
        assert!(close(p[1], [4.0, 5.0, 6.0]));
    }

    #[test]
    fn marks_on_the_look_file_are_carried_to_the_tomogram() {
        let look = Look { path: "/t/a_look.mrc".into(), scale: 2.0, offset: [0.5, 1.0, -3.0] };
        let sources = vec![source("/t/b.mrc", None), source("/t/a.mrc", Some(look.clone()))];
        let inputs = vec!["/t/b.mrc".to_string(), "/t/a.mrc".to_string()];
        let m = mark(Kind::Line, "/t/a_look.mrc", &[[10.0, 20.0, 30.0]]);
        let (i, p) = to_source(&m, &inputs, &sources).unwrap();
        assert_eq!(i, 1);
        assert!(close(p[0], [20.5, 41.0, 57.0]));
        assert!(close(p[0], to_raw([10.0, 20.0, 30.0], 1, 1, &look)));
        assert!(to_source(&mark(Kind::Line, "/t/other.mrc", &[]), &inputs, &sources).is_none());
    }

    #[test]
    fn a_sphere_gives_its_size_from_the_radius_in_either_frame() {
        // Look voxels twice as large as raw ones: radius 50 look voxels = 100 raw voxels of 5 Å = 500 Å.
        let look = Look { path: "/t/a_look.mrc".into(), scale: 2.0, offset: [0.0; 3] };
        let sources = vec![source("/t/a.mrc", Some(look))];
        let inputs = vec!["/t/a.mrc".to_string()];
        let on_look = mark(Kind::Sphere, "/t/a_look.mrc", &[[100.0, 100.0, 100.0], [150.0, 100.0, 100.0]]);
        let on_raw = mark(Kind::Sphere, "/t/a.mrc", &[[200.0, 200.0, 200.0], [300.0, 200.0, 200.0]]);
        let (d1, both, s1) = drawn_of(&[on_look], &inputs, &sources, &[5.0], Size::Medium).unwrap();
        let (d2, _, s2) = drawn_of(&[on_raw], &inputs, &sources, &[5.0], Size::Medium).unwrap();
        assert!(!both);
        assert!((s1 - 250.0).abs() < 1e-9 && (s1 - s2).abs() < 1e-9, "{s1} {s2}");
        assert_eq!(d1.len(), d2.len());
        for d in &d1 {
            // Every seed lies 100 raw voxels from the centre (200, 200, 200).
            assert!((dist(d.pos, [200.0; 3]) - 100.0).abs() < 1e-6);
        }
        assert!(d1.len() > 20, "{}", d1.len());
    }

    #[test]
    fn lines_and_surfaces_use_the_chosen_size_and_points_alone_say_why_not() {
        let sources = vec![source("/t/a.mrc", None)];
        let inputs = vec!["/t/a.mrc".to_string()];
        let line = mark(Kind::Line, "/t/a.mrc", &[[10.0, 10.0, 10.0], [110.0, 10.0, 10.0]]);
        let (d, both, size) = drawn_of(&[line], &inputs, &sources, &[10.0], Size::Large).unwrap();
        assert!(both);
        assert_eq!(size, 360.0);
        // Seeds every 18 voxels (half of 36) along 100 voxels, axis along x.
        assert!(d.len() >= 5 && d.iter().all(|s| (s.axis[0] - 1.0).abs() < 1e-9));
        let surf = mark(Kind::Surface, "/t/a.mrc", &[[0.0, 0.0, 5.0], [100.0, 0.0, 5.0], [0.0, 0.0, 9.0], [100.0, 0.0, 9.0]]);
        assert_eq!(strokes(&surf.points).len(), 2);
        let (d, _, _) = drawn_of(&[surf], &inputs, &sources, &[10.0], Size::Small).unwrap();
        assert!(!d.is_empty());
        let pt = mark(Kind::Point, "/t/a.mrc", &[[1.0, 1.0, 1.0]]);
        let e = drawn_of(&[pt], &inputs, &sources, &[10.0], Size::Medium).unwrap_err().to_string();
        assert!(e.contains("点"), "{e}");
        let away = mark(Kind::Line, "/t/zzz.mrc", &[[0.0; 3], [1.0; 3]]);
        assert!(drawn_of(&[away], &inputs, &sources, &[10.0], Size::Medium).is_err());
    }

    #[test]
    fn points_alone_go_to_the_template_path_shapes_to_the_drawn_one() {
        let pt = mark(Kind::Point, "/t/a.mrc", &[[1.0, 1.0, 1.0], [5.0, 5.0, 5.0]]);
        let line = mark(Kind::Line, "/t/a.mrc", &[[0.0; 3], [9.0; 3]]);
        let ball = mark(Kind::Sphere, "/t/a.mrc", &[[0.0; 3], [9.0; 3]]);
        assert_eq!(route(&[]), Route::Shapes);
        assert_eq!(route(&[pt.clone(), pt.clone()]), Route::Points);
        assert_eq!(route(&[line.clone(), ball.clone()]), Route::Shapes);
        assert_eq!(route(&[pt.clone(), line, ball]), Route::Mixed);
        // The clicks of every point mark, with the source they lie on; look-file clicks are carried over.
        let look = Look { path: "/t/a_look.mrc".into(), scale: 2.0, offset: [0.0; 3] };
        let sources = vec![source("/t/a.mrc", Some(look))];
        let inputs = vec!["/t/a.mrc".to_string()];
        let on_look = mark(Kind::Point, "/t/a_look.mrc", &[[10.0, 20.0, 30.0]]);
        let got = points_of(&[pt, on_look], &inputs, &sources).unwrap();
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|g| g.0 == 0) && close(got[2].1, [20.0, 40.0, 60.0]));
        assert!(points_of(&[mark(Kind::Point, "/t/other.mrc", &[[1.0; 3]])], &inputs, &sources).is_err());
    }

    #[test]
    fn a_look_file_in_the_list_is_not_a_tomogram_of_its_own() {
        let look = Look { path: "/t/a_look.mrc".into(), scale: 1.0, offset: [0.0; 3] };
        let sources = vec![source("/t/a_look.mrc", None), source("/t/a.mrc", Some(look))];
        let (inputs, sources) = without_looks(&["/t/a_look.mrc".to_string(), "/t/a.mrc".to_string()], sources);
        assert_eq!(inputs, vec!["/t/a.mrc".to_string()]);
        assert_eq!(sources.len(), 1);
    }

    #[test]
    fn a_marks_run_given_a_look_file_works_on_its_tomogram() {
        use std::sync::Mutex;
        let dir = std::env::temp_dir().join(format!("tmv-marks-look-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["a_raw.mrc", "a_look.mrc"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        let d = json!({ "version": 1, "stack": dir.join("a.mrc"), "angles": [-60.0, 60.0], "tomograms": [
            { "path": dir.join("a_raw.mrc"), "kind": "full", "bin": 4, "thickness": 100, "method": "sart",
              "look": { "path": dir.join("a_look.mrc"), "scale": 2.0, "offset": [0.0, 0.0, 0.0] } } ] });
        std::fs::write(dir.join("a.tomo.json"), d.to_string()).unwrap();
        let seen = Mutex::new(vec![]);
        let emit = |name: &str, v: Value| {
            if name == "sta-note" && v["kind"] == "olderVersion" {
                seen.lock().unwrap().push(v["tomogram"].as_str().unwrap().to_string());
            }
        };
        let device: Device = &|| bail!("no device");
        let line = mark(Kind::Line, &p("a_look.mrc"), &[[1.0; 3], [5.0; 3]]);
        let _ = run_marks(&[p("a_look.mrc")], &[line], None, None, device, &emit, &|_| String::new());
        assert_eq!(*seen.lock().unwrap(), [p("a_raw.mrc")]);
        // A look mark is carried onto the tomogram that the input became.
        let sources = vec![source(&p("a_raw.mrc"), Some(Look { path: p("a_look.mrc"), scale: 2.0, offset: [0.0; 3] }))];
        let (i, pts) = to_source(&mark(Kind::Line, &p("a_look.mrc"), &[[3.0; 3]]), &[p("a_raw.mrc")], &sources).unwrap();
        assert!(i == 0 && close(pts[0], [6.0; 3]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_drawn_average_counts_the_shapes_it_was_made_of() {
        let marks = [
            mark(Kind::Line, "/t/a.mrc", &[[0.0; 3], [9.0; 3]]),
            mark(Kind::Line, "/t/a.mrc", &[[0.0; 3]]),
            mark(Kind::Surface, "/t/a.mrc", &[[0.0; 3], [9.0; 3], [5.0; 3]]),
            mark(Kind::Sphere, "/t/a.mrc", &[[0.0; 3], [9.0; 3]]),
            mark(Kind::Point, "/t/a.mrc", &[[0.0; 3], [9.0; 3]]),
        ];
        let v = shapes_used(&marks, 219.6);
        assert_eq!((v["lines"].as_u64(), v["surfaces"].as_u64(), v["spheres"].as_u64(), v["sizeA"].as_f64()), (Some(1), Some(1), Some(1), Some(220.0)));
    }

    #[test]
    fn flip_gives_exactly_the_negated_axes_at_the_same_positions() {
        let sources = vec![source("/t/a.mrc", None)];
        let inputs = vec!["/t/a.mrc".to_string()];
        let surf = mark(Kind::Surface, "/t/a.mrc", &[[0.0, 0.0, 5.0], [100.0, 0.0, 5.0], [0.0, 0.0, 9.0], [100.0, 0.0, 9.0]]);
        let ball = mark(Kind::Sphere, "/t/a.mrc", &[[50.0, 50.0, 50.0], [70.0, 50.0, 50.0]]);
        for m in [surf, ball] {
            let flipped = Mark { flip: true, ..m.clone() };
            let (a, _, _) = drawn_of(&[m.clone()], &inputs, &sources, &[10.0], Size::Small).unwrap();
            let (b, _, _) = drawn_of(&[flipped], &inputs, &sources, &[10.0], Size::Small).unwrap();
            assert!(!a.is_empty() && a.len() == b.len());
            for (x, y) in a.iter().zip(&b) {
                assert_eq!(x.pos, y.pos);
                assert_eq!(x.axis.map(|v| -v), y.axis);
            }
            if m.kind == Kind::Sphere {
                // Unflipped: outward.
                assert!(a.iter().all(|s| (0..3).map(|k| (s.pos[k] - 50.0) * s.axis[k]).sum::<f64>() > 0.0));
            }
        }
    }

    #[test]
    fn an_old_marks_json_without_flip_loads_unflipped() {
        let old = r#"{"marks":[{"kind":"surface","tomogram":"/t/a.mrc","points":[[1.0,2.0,3.0],[4.0,5.0,6.0]]}],"size":"small"}"#;
        let saved: Saved = serde_json::from_str(old).unwrap();
        assert_eq!(saved.marks.len(), 1);
        assert!(!saved.marks[0].flip);
        let text = serde_json::to_string(&Mark { flip: true, ..saved.marks[0].clone() }).unwrap();
        assert!(text.contains("\"flip\":true"));
    }

    #[test]
    fn the_preview_is_thin_unit_and_near_the_mark() {
        let sources = vec![source("/t/a.mrc", None)];
        let inputs = vec!["/t/a.mrc".to_string()];
        let marks = vec![
            mark(Kind::Surface, "/t/a.mrc", &[[0.0, 0.0, 5.0], [300.0, 0.0, 5.0], [0.0, 0.0, 9.0], [300.0, 0.0, 9.0]]),
            mark(Kind::Sphere, "/t/a.mrc", &[[150.0, 150.0, 50.0], [190.0, 150.0, 50.0]]),
            mark(Kind::Line, "/t/a.mrc", &[[10.0, 10.0, 10.0], [400.0, 10.0, 10.0]]),
            mark(Kind::Point, "/t/a.mrc", &[[1.0, 1.0, 1.0]]),
            mark(Kind::Line, "/t/zzz.mrc", &[[0.0; 3], [1.0; 3]]),
        ];
        let px = |_: &str| Some(1.0);
        let pre = preview_of(&marks, "/t/a.mrc", Size::Small, &inputs, &sources, &px);
        assert_eq!(pre.iter().map(|p| p.mark).collect::<Vec<_>>(), vec![0, 1, 2]);
        for p in &pre {
            let m = &marks[p.mark];
            assert!(!p.seeds.is_empty() && p.seeds.len() <= MOST_PREVIEW);
            // Margin: a particle's size, or the sphere's radius.
            let margin = if m.kind == Kind::Sphere { 40.0 } else { Size::Small.angstrom() };
            for s in &p.seeds {
                for k in 0..3 {
                    let lo = m.points.iter().map(|q| q[k]).fold(f64::MAX, f64::min) - margin;
                    let hi = m.points.iter().map(|q| q[k]).fold(f64::MIN, f64::max) + margin;
                    assert!(s.pos[k] >= lo && s.pos[k] <= hi, "{:?} {k} {:?}", m.kind, s.pos);
                }
                let n = s.axis.iter().map(|v| v * v).sum::<f64>().sqrt();
                assert!((n - 1.0).abs() < 1e-6);
            }
        }
        // A flipped mark shows reversed arrows at the same places.
        let flipped = vec![Mark { flip: true, ..marks[1].clone() }];
        let b = preview_of(&flipped, "/t/a.mrc", Size::Small, &inputs, &sources, &px);
        assert!(b[0].seeds.iter().zip(&pre[1].seeds).all(|(x, y)| x.pos == y.pos && x.axis.map(|v| -v) == y.axis));
    }

    #[test]
    fn the_preview_is_carried_to_the_look_file() {
        let look = Look { path: "/t/a_look.mrc".into(), scale: 2.0, offset: [10.0, 20.0, 30.0] };
        let sources = vec![source("/t/a.mrc", Some(look))];
        let inputs = vec!["/t/a.mrc".to_string()];
        let m = mark(Kind::Line, "/t/a.mrc", &[[10.0, 20.0, 30.0], [210.0, 20.0, 30.0]]);
        let px = |_: &str| Some(1.0);
        let raw = preview_of(&[m.clone()], "/t/a.mrc", Size::Small, &inputs, &sources, &px);
        let lk = preview_of(&[m], "/t/a_look.mrc", Size::Small, &inputs, &sources, &px);
        assert!(close(lk[0].seeds[0].pos, [0.0, 0.0, 0.0]) && close(raw[0].seeds[0].pos, [10.0, 20.0, 30.0]));
        assert!(close(lk[0].seeds[0].axis, raw[0].seeds[0].axis));
    }

    #[test]
    fn marks_json_round_trips() {
        let dir = std::env::temp_dir().join(format!("tmv-marks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), Saved::default());
        let saved = Saved {
            marks: vec![mark(Kind::Sphere, "/t/a.mrc", &[[1.0, 2.0, 3.0], [4.0, 5.0, 6.5]]), mark(Kind::Point, "/t/a.mrc", &[[7.0, 8.0, 9.0]])],
            size: Some("large".into()),
        };
        save(&dir, &saved).unwrap();
        assert_eq!(load(&dir), saved);
        let text = std::fs::read_to_string(dir.join("marks.json")).unwrap();
        assert!(text.contains("\"sphere\"") && text.contains("\"point\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
