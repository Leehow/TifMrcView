//! "Find them all, show them, then average" for membranes and vesicles (the
//! same loop as `review` has for particles). The user paints 2 to 3
//! membranes or marks 2 to 3 vesicles as examples; the program finds every
//! membrane (`membranes::find`) and every vesicle (`membranes::find_balls`)
//! of that kind in all the ticked tomograms, turns each to the side the
//! examples face, and saves it all beside the average in
//! `<out>/drawn/shapes.json`. The user checks what was found (drops one,
//! flips one, flips all) and only then are the kept ones averaged
//! (`project::run_drawn`).
//!
//! Where things are. `shapes.json` holds the items, the edits, the inputs
//! with the stamps they had, and the particle size; what is too big for JSON
//! (the sampling points of an item, the voxels of a membrane for the
//! highlight) is in `shapes/<id>.bin`, little endian:
//!
//! ```text
//! 0   4 bytes  magic "TMS1"
//! 4   u32      n seeds
//! 8   n × 6 × f32   pos x y z, axis x y z (axis as the program decided: flips of the examples applied, the user's flip not)
//!     u32      m voxel indices (0 for a vesicle)
//!     m × u32  voxel indices (z·w·h + y·w + x of the item's file), ascending
//! ```
//!
//! Coordinates. An item is in voxels of the file it was found in: the
//! tomogram or its look companion (`Item::on_look`). Everything shown is
//! carried to the file the viewer has open; seeds are carried to the raw
//! tomogram when they are averaged.
//!
//! Lines are searched for like membranes: the lines the user drew are the
//! examples, every filament of that thickness in the ticked tomograms is
//! found (`filaments::find`) and is an item with its sampling points along
//! it. A filament much weaker than the examples is found but not taken
//! unless the user takes it (`Edits::kept`). Hand-drawn surfaces are not
//! searched for: they are kept as seeds (raw voxels) and averaged as drawn.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::auto::{self, input_stamps, note_json, raw_inputs, resolve_tomogram, result_value, write_used, Emit, InputStamp, DRAWN};
use super::marks::{self, Kind, Mark, Size, MOST_SEEDS, SIZE_RANGE, SPHERE_SHARE};
use super::filaments;
use super::membranes;
use super::paint;
use super::project::{self as sp, to_raw, Drawn, Source};
use super::review::events;
use super::shape::{self, Seed};
use crate::tomo::dataset::Look;

/// The file in `<out>/drawn` that holds the search and the edits.
pub const FILE: &str = "shapes.json";
/// The folder beside it with one `.bin` per item.
const SIDE: &str = "shapes";

/// A rough figure, to be measured: a minute of set-up and about a quarter of
/// a second per sampling point.
pub const ETA_BASE_S: f64 = 60.0;
pub const ETA_PER_SEED_S: f64 = 0.25;
/// Fewest sampling points that are averaged.
pub const MIN_SEEDS: usize = 20;
/// Most arrows shown per item.
pub const MOST_ARROWS: usize = 40;
/// Most points of a filament sent to the screen.
pub const MOST_LINE: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Membrane,
    Ball,
    /// A filament: its sampling points lie along its middle line.
    Line,
}

/// One membrane, vesicle or filament found (or given as an example).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: usize,
    pub kind: ItemKind,
    /// Which of `State::inputs` it lies in.
    pub source: usize,
    /// Its voxels are the look companion's (else the tomogram's).
    pub on_look: bool,
    pub example: bool,
    /// Whether the side was told from the density (always for an example).
    pub side_sure: bool,
    pub centre: [f64; 3],
    /// Vesicles: radius of the membrane's middle, voxels.
    #[serde(default)]
    pub radius: f64,
    /// Membranes: voxels of the segmentation it takes.
    #[serde(default)]
    pub voxels: usize,
    /// Membranes: the grid of its file (the voxel indices are of it).
    #[serde(default)]
    pub n: [usize; 3],
    /// How many sampling points it has.
    pub seeds: usize,
    /// Filaments: strength as a share of the examples' (1 for an example).
    #[serde(default)]
    pub score: f32,
    /// Filaments: length along the line, voxels of its file.
    #[serde(default)]
    pub length: f64,
    /// Filaments: the voxel size of its file, Å (0: not known).
    #[serde(default)]
    pub px: f64,
    /// Filaments: found, but weaker than `filaments::SURE` and not an example:
    /// not taken unless the user takes it.
    #[serde(default)]
    pub weak: bool,
}

/// What the user changed from the program's suggestion (item ids).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Edits {
    #[serde(default)]
    pub dropped: Vec<usize>,
    #[serde(default)]
    pub flipped: Vec<usize>,
    /// Weak items the user takes after all.
    #[serde(default)]
    pub kept: Vec<usize>,
}

/// A sampling point of a line or hand-drawn surface, raw voxels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub source: usize,
    pub pos: [f64; 3],
    pub axis: [f64; 3],
}

/// What `shapes.json` holds.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub items: Vec<Item>,
    pub edits: Edits,
    /// The tomograms searched (raw files), in source order, as they were then.
    pub inputs: Vec<String>,
    pub stamps: Vec<InputStamp>,
    /// Per input: its look companion.
    pub looks: Vec<Option<Look>>,
    /// The particle size, Å.
    pub size_a: f64,
    /// The sense of the axes is unknown for what was drawn (see `State::both`).
    #[serde(default)]
    pub both: bool,
    /// How many hand-drawn surfaces were used (named `lines` in older files).
    #[serde(alias = "lines", default)]
    pub surfaces: usize,
    /// Their sampling points.
    pub drawn: Vec<Placed>,
}

/// What a search gives.
pub enum Found {
    /// Files that have no membrane segmentation yet: nothing was searched.
    NeedSegment(Vec<String>),
    /// The state and the sides of its items (`Side`, by id).
    Done(Box<State>, Vec<Side>),
}

/// The part of an item too big for JSON.
#[derive(Clone, Debug, Default)]
pub struct Side {
    pub seeds: Vec<Seed>,
    pub mask: Vec<u32>,
}

fn same(a: &str, b: &str) -> bool {
    a == b || std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(x, y)| x == y)
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt()
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn millis(p: &Path) -> Option<u64> {
    let t = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// Which input a file belongs to and whether it is that input's look companion.
fn where_is(file: &str, inputs: &[String], sources: &[Source]) -> Option<(usize, bool)> {
    for (i, s) in sources.iter().enumerate() {
        let is_look = s.look.as_ref().is_some_and(|l| same(file, &l.path));
        if !is_look && (same(file, &s.path) || inputs.get(i).is_some_and(|p| same(file, p))) {
            return Some((i, false));
        }
    }
    sources.iter().position(|s| s.look.as_ref().is_some_and(|l| same(file, &l.path))).map(|i| (i, true))
}

/// The particle size (Å) of a set of marks, as `marks::drawn_of` takes it: a
/// vesicle alone gives it by its radius, anything else the chosen size.
pub fn size_of(marks: &[Mark], size: Size, pixel: &dyn Fn(&str) -> Option<f64>) -> f64 {
    let with_size = marks.iter().any(|m| matches!(m.kind, Kind::Line | Kind::Surface | Kind::Membrane));
    let radii: Vec<f64> = marks.iter().filter(|m| m.kind == Kind::Sphere && m.points.len() >= 2).filter_map(|m| Some(dist(m.points[0], m.points[1]) * pixel(&m.tomogram)?)).collect();
    if with_size || radii.is_empty() {
        size.angstrom()
    } else {
        (SPHERE_SHARE * radii.iter().sum::<f64>() / radii.len() as f64).clamp(SIZE_RANGE.0, SIZE_RANGE.1)
    }
}

/// The search. `sources` are the ticked tomograms (raw files with their look
/// companions), `inputs` their paths; `marks` hold the examples (membranes
/// painted, spheres marked) and lines and surfaces to be used as drawn;
/// `size_a` is the particle size (`size_of`), `size` the chosen one (for the
/// lines). The files searched are, for every input, the kind of file the
/// examples were marked on (the look companion when they are on those; an
/// input without one is searched on its raw file). When any of them is not
/// segmented nothing is searched and the list of them comes back (lines need
/// no segmentation).
pub fn find_in(sources: &[Source], inputs: &[String], marks: &[Mark], size: Size, size_a: f64, pixel: &dyn Fn(&str) -> Option<f64>, progress: &dyn Fn(&str, usize, usize)) -> Result<Found> {
    let membranes_marked = marks.iter().any(|m| m.kind == Kind::Membrane && paint::has_stroke(m));
    let spheres_marked = marks.iter().any(|m| m.kind == Kind::Sphere && m.points.len() >= 2);
    let lines_marked = marks.iter().any(|m| m.kind == Kind::Line && m.points.len() >= 2);
    if !membranes_marked && !spheres_marked && !lines_marked {
        bail!("请先在膜上涂一笔，或标一个球、画一条线作例子");
    }
    // Every mark must lie on a tomogram that is ticked.
    let mut on_look = 0;
    let mut on_raw = 0;
    for m in marks.iter().filter(|m| m.kind != Kind::Point) {
        let Some((_, look)) = where_is(&m.tomogram, inputs, sources) else { bail!("有标注所在的断层图没有被勾选") };
        let example = (m.kind == Kind::Membrane && paint::has_stroke(m)) || (m.kind == Kind::Sphere && m.points.len() >= 2) || (m.kind == Kind::Line && m.points.len() >= 2);
        if example {
            if look { on_look += 1 } else { on_raw += 1 }
        }
    }
    let use_look = on_look > 0 && on_look >= on_raw;
    let files: Vec<String> = sources
        .iter()
        .map(|s| match (&s.look, use_look) {
            (Some(l), true) => l.path.clone(),
            _ => s.path.clone(),
        })
        .collect();
    // The files to read: those searched and those the examples are on.
    let mut all = files.clone();
    for m in marks.iter().filter(|m| matches!(m.kind, Kind::Membrane | Kind::Sphere)) {
        if !all.contains(&m.tomogram) {
            all.push(m.tomogram.clone());
        }
    }
    let mut need: Vec<String> = vec![];
    // Only membranes and vesicles are read off the segmentation.
    for f in all.iter().filter(|_| membranes_marked || spheres_marked) {
        if paint::mask_of(f)?.is_none() && !need.contains(f) {
            need.push(f.clone());
        }
    }
    if !need.is_empty() {
        return Ok(Found::NeedSegment(need));
    }
    let mut items: Vec<Item> = vec![];
    let mut sides: Vec<Side> = vec![];
    let place = |file: &str| where_is(file, inputs, sources).context("有标注所在的断层图没有被勾选");
    if membranes_marked {
        progress("找同类的膜", 0, 3);
        let found = membranes::find(marks, &files, size_a)?;
        if !found.unsegmented.is_empty() {
            return Ok(Found::NeedSegment(found.unsegmented));
        }
        if found.membranes.is_empty() {
            return Err(paint::nothing_touched());
        }
        for m in found.membranes {
            let (source, on_look) = place(&m.tomogram)?;
            let n = paint::mask_of(&m.tomogram)?.map_or([0; 3], |k| k.n);
            items.push(Item { id: items.len(), kind: ItemKind::Membrane, source, on_look, example: m.example, side_sure: m.side_sure, centre: m.centre, radius: 0.0, voxels: m.voxels, n, seeds: m.seeds.len(), score: 0.0, length: 0.0, px: 0.0, weak: false });
            sides.push(Side { seeds: m.seeds, mask: m.mask });
        }
    }
    if spheres_marked {
        progress("找同类的囊泡", 1, 3);
        let found = membranes::find_balls(marks, &files, size_a, pixel)?;
        if !found.unsegmented.is_empty() {
            return Ok(Found::NeedSegment(found.unsegmented));
        }
        for b in found.balls {
            let (source, on_look) = place(&b.tomogram)?;
            items.push(Item { id: items.len(), kind: ItemKind::Ball, source, on_look, example: b.example, side_sure: b.side_sure, centre: b.centre, radius: b.radius, voxels: 0, n: [0; 3], seeds: b.seeds.len(), score: 0.0, length: 0.0, px: 0.0, weak: false });
            sides.push(Side { seeds: b.seeds, mask: vec![] });
        }
    }
    if lines_marked {
        progress("找同类的纤维", 2, 3);
        let found = filaments::find(marks, &files)?;
        for f in found.filaments {
            let (source, on_look) = place(&f.tomogram)?;
            let px = pixel(&f.tomogram).context("读不出体素大小")?;
            // As for membranes: half a particle apart, in voxels of the file it was found in.
            let seeds = shape::line(&f.filament.line, 0.5 * size_a / px);
            if seeds.is_empty() {
                continue;
            }
            let line = &f.filament.line;
            let mid = line[line.len() / 2];
            let weak = f.filament.score < filaments::SURE && !f.filament.example;
            items.push(Item { id: items.len(), kind: ItemKind::Line, source, on_look, example: f.filament.example, side_sure: true, centre: mid, radius: 0.0, voxels: 0, n: [0; 3], seeds: seeds.len(), score: f.filament.score, length: f.filament.length, px, weak });
            sides.push(Side { seeds, mask: vec![] });
        }
    }
    if items.is_empty() {
        bail!("没有找到像例子的东西");
    }
    // Hand-drawn surfaces are used as drawn.
    let others: Vec<Mark> = marks.iter().filter(|m| m.kind == Kind::Surface && m.points.len() >= 2).cloned().collect();
    let (mut drawn, mut both) = (vec![], false);
    if !others.is_empty() {
        let pixels: Vec<f64> = sources.iter().map(|s| pixel(&s.path).context("读不出体素大小")).collect::<Result<_>>()?;
        let (d, b, _) = marks::drawn_of(&others, inputs, sources, &pixels, size)?;
        drawn = d.into_iter().map(|d| Placed { source: d.source, pos: d.pos, axis: d.axis }).collect();
        both = b;
    }
    progress("找同类", 3, 3);
    let state = State {
        items,
        edits: Edits::default(),
        inputs: inputs.to_vec(),
        stamps: input_stamps(inputs.iter().map(String::as_str)),
        looks: sources.iter().map(|s| s.look.clone()).collect(),
        size_a,
        both,
        surfaces: others.len(),
        drawn,
    };
    Ok(Found::Done(Box::new(state), sides))
}

// ---- files ----

pub fn path_of(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

fn side_path(dir: &Path, id: usize) -> PathBuf {
    dir.join(SIDE).join(format!("{id}.bin"))
}

/// Writes the sidecar of one item (layout in the module text).
pub fn write_side(dir: &Path, id: usize, side: &Side) -> Result<()> {
    let mut b: Vec<u8> = Vec::with_capacity(12 + side.seeds.len() * 24 + side.mask.len() * 4);
    b.extend_from_slice(b"TMS1");
    b.extend_from_slice(&(side.seeds.len() as u32).to_le_bytes());
    for s in &side.seeds {
        for v in s.pos.iter().chain(&s.axis) {
            b.extend_from_slice(&(*v as f32).to_le_bytes());
        }
    }
    b.extend_from_slice(&(side.mask.len() as u32).to_le_bytes());
    for v in &side.mask {
        b.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(side_path(dir, id), b).with_context(|| format!("无法保存第 {id} 个"))
}

pub fn read_side(dir: &Path, id: usize) -> Result<Side> {
    let b = std::fs::read(side_path(dir, id)).with_context(|| format!("读不到第 {id} 个的采样点，请重新找"))?;
    let bad = || anyhow::anyhow!("第 {id} 个的文件损坏了，请重新找");
    let u32_at = |o: usize| -> Result<u32> { Ok(u32::from_le_bytes(b.get(o..o + 4).ok_or_else(bad)?.try_into().unwrap())) };
    if b.get(..4) != Some(b"TMS1") {
        return Err(bad());
    }
    let n = u32_at(4)? as usize;
    let mut seeds = Vec::with_capacity(n);
    for k in 0..n {
        let f = |j: usize| -> Result<f64> { Ok(f32::from_bits(u32_at(8 + k * 24 + j * 4)?) as f64) };
        seeds.push(Seed { pos: [f(0)?, f(1)?, f(2)?], axis: [f(3)?, f(4)?, f(5)?] });
    }
    let at = 8 + n * 24;
    let m = u32_at(at)? as usize;
    let mask = (0..m).map(|k| u32_at(at + 4 + k * 4)).collect::<Result<Vec<u32>>>()?;
    Ok(Side { seeds, mask })
}

/// Saves a fresh search: the old one's files go. Written to a side file and
/// renamed, so a crash leaves the old state.
pub fn save_new(dir: &Path, s: &State, sides: &[Side]) -> Result<()> {
    let _ = std::fs::remove_dir_all(dir.join(SIDE));
    std::fs::create_dir_all(dir.join(SIDE)).with_context(|| format!("无法创建 {}", dir.display()))?;
    for (i, side) in sides.iter().enumerate() {
        write_side(dir, i, side)?;
    }
    save(dir, s)
}

pub fn save(dir: &Path, s: &State) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("无法创建 {}", dir.display()))?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(s)?)?;
    std::fs::rename(&tmp, path_of(dir))?;
    Ok(())
}

/// The state saved in `dir`; None when there is none or it cannot be read.
pub fn load(dir: &Path) -> Option<State> {
    std::fs::read(path_of(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// The folder of the shapes of output folder `out`.
pub fn dir_of(out: &str) -> PathBuf {
    Path::new(out).join(DRAWN)
}

// ---- the state ----

/// Where `open` (a searched tomogram or its look companion) is: (input, is the look file).
impl State {
    pub fn frame(&self, open: &str) -> Option<(usize, bool)> {
        if let Some(i) = self.inputs.iter().position(|p| same(p, open)) {
            if !self.looks[i].as_ref().is_some_and(|l| same(&l.path, open)) {
                return Some((i, false));
            }
        }
        self.looks.iter().position(|l| l.as_ref().is_some_and(|l| same(&l.path, open))).map(|i| (i, true))
    }

    /// Whether an item is left out: one the user dropped, or a weak one the
    /// user did not take.
    pub fn dropped(&self, id: usize) -> bool {
        if self.items.get(id).is_some_and(|i| i.weak) {
            !self.edits.kept.contains(&id)
        } else {
            self.edits.dropped.contains(&id)
        }
    }

    /// Weak items that are not taken.
    pub fn left_weak(&self) -> usize {
        self.items.iter().filter(|i| i.weak && self.dropped(i.id)).count()
    }

    /// Whether the sense of the axes is unknown: a filament is among what is
    /// averaged (nothing tells which way along it is up), or the drawn ones say so.
    pub fn both(&self) -> bool {
        self.both || self.kept().any(|i| i.kind == ItemKind::Line)
    }

    pub fn flipped(&self, id: usize) -> bool {
        self.edits.flipped.contains(&id)
    }

    /// Items that are averaged.
    pub fn kept(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| !self.dropped(i.id))
    }

    /// Kept items the program could not tell the side of and the user did not look at (flip).
    pub fn unsure(&self) -> usize {
        self.kept().filter(|i| !i.example && !i.side_sure && !self.flipped(i.id)).count()
    }

    /// Sampling points that will be averaged (before thinning to `MOST_SEEDS`).
    pub fn seeds(&self) -> usize {
        self.kept().map(|i| i.seeds).sum::<usize>() + self.drawn.len()
    }

    pub fn eta(seeds: usize) -> f64 {
        ETA_BASE_S + ETA_PER_SEED_S * seeds.min(MOST_SEEDS) as f64
    }

    /// Why the tomograms cannot be taken as the ones searched (None: the same).
    pub fn stale(&self) -> Option<String> {
        let now = input_stamps(self.inputs.iter().map(String::as_str));
        auto::stale(Some(&self.stamps), &now).map(|_| "断层图变了，请重新找".to_string())
    }

    fn toggle(list: &mut Vec<usize>, id: usize) {
        match list.iter().position(|x| *x == id) {
            Some(at) => {
                list.remove(at);
            }
            None => list.push(id),
        }
    }

    fn check(&self, id: usize) -> Result<()> {
        if id >= self.items.len() {
            bail!("没有第 {id} 个");
        }
        Ok(())
    }

    /// Drops an item, or takes a dropped one back; for a weak item (left out
    /// at first) that is taking it, or leaving it out again.
    pub fn toggle_drop(&mut self, id: usize) -> Result<()> {
        self.check(id)?;
        if self.items[id].weak {
            Self::toggle(&mut self.edits.kept, id);
        } else {
            Self::toggle(&mut self.edits.dropped, id);
        }
        Ok(())
    }

    /// Turns an item to the other side (a filament has none: nothing happens).
    pub fn toggle_flip(&mut self, id: usize) -> Result<()> {
        self.check(id)?;
        if self.items[id].kind != ItemKind::Line {
            Self::toggle(&mut self.edits.flipped, id);
        }
        Ok(())
    }

    /// Turns every item to the other side (filaments have none).
    pub fn flip_all(&mut self) {
        let all: Vec<usize> = (0..self.items.len()).filter(|i| self.items[*i].kind != ItemKind::Line).collect();
        self.edits.flipped = all.into_iter().filter(|i| !self.flipped(*i)).collect();
    }

    pub fn reset(&mut self) {
        self.edits = Edits::default();
    }

    /// A voxel position of the file an item lies in, as the open file's voxels.
    fn convert(&self, source: usize, from_look: bool, to_look: bool, p: [f64; 3]) -> [f64; 3] {
        let Some(l) = self.looks.get(source).and_then(|l| l.as_ref()) else { return p };
        let raw: [f64; 3] = if from_look { std::array::from_fn(|k| l.scale * p[k] + l.offset[k]) } else { p };
        if to_look { std::array::from_fn(|k| (raw[k] - l.offset[k]) / l.scale) } else { raw }
    }

    /// How many voxels of the open file one voxel of the item's file is wide.
    fn zoom(&self, source: usize, from_look: bool, to_look: bool) -> f64 {
        let scale = self.looks.get(source).and_then(|l| l.as_ref()).map_or(1.0, |l| l.scale);
        match (from_look, to_look) {
            (true, false) => scale,
            (false, true) => 1.0 / scale,
            _ => 1.0,
        }
    }
}

/// What the review panel and the overlay show for the tomogram `open`:
/// `{ total, kept, dropped, flipped, membranes, balls, lines, surfaces, weak, unsure, seeds, perTomogram: [{name, path,
///    membranes, balls, lines}], items, open, etaSeconds, stale, needSegment: [], out, foundMs, averageMs }`.
/// `membranes`, `balls`, `lines` (filaments), the per-tomogram counts and
/// `kept` count what is kept; `total` also the dropped; `dropped` is what the
/// user dropped, `weak` the weak filaments not taken, `surfaces` the hand-drawn
/// surfaces. `items` are those of the open tomogram:
/// `{id, kind, example, state, sideSure, flipped, centre, radius?, voxels?,
/// arrows: [{pos, axis}]}` in the open file's voxels, arrows with the flip applied;
/// a filament (`kind: "line"`) has `line: [[x, y, z], …]` (its sampling points,
/// at most `MOST_LINE` of them, no arrows), `score`, `weak`, and its length as
/// `lengthA` (Å) when the voxel size is known, else `lengthVoxels`.
pub fn review_value(s: &State, dir: &Path, open: &str) -> Value {
    let count = |kind: ItemKind, src: Option<usize>| s.kept().filter(|i| i.kind == kind && src.is_none_or(|x| x == i.source)).count();
    let per: Vec<Value> = s
        .inputs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
            json!({ "name": name, "path": p, "membranes": count(ItemKind::Membrane, Some(i)), "balls": count(ItemKind::Ball, Some(i)), "lines": count(ItemKind::Line, Some(i)) })
        })
        .collect();
    let frame = s.frame(open);
    let items: Vec<Value> = match frame {
        Some((src, to_look)) => s
            .items
            .iter()
            .filter(|i| i.source == src)
            .map(|i| {
                let dropped = s.dropped(i.id);
                let flipped = s.flipped(i.id);
                let c = s.convert(src, i.on_look, to_look, i.centre);
                let mut arrows: Vec<Value> = vec![];
                if !dropped && i.kind != ItemKind::Line {
                    if let Ok(side) = read_side(dir, i.id) {
                        let n = side.seeds.len();
                        let take = n.min(MOST_ARROWS);
                        let sign = if flipped { -1.0 } else { 1.0 };
                        arrows = (0..take)
                            .map(|j| {
                                let k = side.seeds[j * n / take];
                                let p = s.convert(src, i.on_look, to_look, k.pos);
                                json!({ "pos": [r1(p[0]), r1(p[1]), r1(p[2])], "axis": [r3(sign * k.axis[0]), r3(sign * k.axis[1]), r3(sign * k.axis[2])] })
                            })
                            .collect();
                    }
                }
                let mut v = json!({
                    "id": i.id, "kind": i.kind, "example": i.example, "state": if dropped { "dropped" } else { "kept" },
                    "sideSure": i.side_sure, "flipped": flipped, "centre": [r1(c[0]), r1(c[1]), r1(c[2])], "arrows": arrows,
                });
                match i.kind {
                    ItemKind::Ball => v["radius"] = json!(r1(i.radius * s.zoom(src, i.on_look, to_look))),
                    ItemKind::Membrane => v["voxels"] = json!(i.voxels),
                    ItemKind::Line => {
                        // Dropped ones are drawn too (grey, dashed), so the line is always there.
                        let pts: Vec<Value> = match read_side(dir, i.id) {
                            Ok(side) => {
                                let n = side.seeds.len();
                                let take = n.min(MOST_LINE);
                                (0..take)
                                    .map(|j| {
                                        // Evenly, the last point included.
                                        let k = if take > 1 { j * (n - 1) / (take - 1) } else { 0 };
                                        let p = s.convert(src, i.on_look, to_look, side.seeds[k].pos);
                                        json!([r1(p[0]), r1(p[1]), r1(p[2])])
                                    })
                                    .collect()
                            }
                            Err(_) => vec![],
                        };
                        v["line"] = json!(pts);
                        v["score"] = json!(r3(i.score as f64));
                        v["weak"] = json!(i.weak);
                        if i.px > 0.0 {
                            v["lengthA"] = json!((i.length * i.px).round());
                        } else {
                            v["lengthVoxels"] = json!((i.length * s.zoom(src, i.on_look, to_look)).round());
                        }
                    }
                }
                v
            })
            .collect(),
        None => vec![],
    };
    let seeds = s.seeds();
    json!({
        "total": s.items.len(), "kept": s.kept().count(), "dropped": s.edits.dropped.len(), "flipped": s.edits.flipped.len(),
        "membranes": count(ItemKind::Membrane, None), "balls": count(ItemKind::Ball, None), "lines": count(ItemKind::Line, None), "surfaces": s.surfaces, "weak": s.left_weak(),
        "unsure": s.unsure(), "seeds": seeds.min(MOST_SEEDS), "perTomogram": per, "items": items,
        "open": frame.map(|(source, look)| json!({ "source": source, "look": look })),
        "etaSeconds": State::eta(seeds).round(), "stale": s.stale(), "needSegment": [],
        "out": dir, "foundMs": millis(&path_of(dir)), "averageMs": millis(&dir.join("average.mrc")),
    })
}

/// The review of `dir` for the open tomogram; None when nothing was found there.
pub fn review(dir: &Path, open: &str) -> Option<Value> {
    load(dir).map(|s| review_value(&s, dir, open))
}

/// A change to the edits.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    /// An item's id: dropped, or taken back when it was dropped.
    pub drop: Option<usize>,
    /// An item's id: turned to the other side.
    pub flip: Option<usize>,
    #[serde(default)]
    pub flip_all: bool,
    /// Back to the program's suggestion.
    #[serde(default)]
    pub reset: bool,
}

pub fn apply(s: &mut State, edit: &Edit) -> Result<()> {
    if edit.reset {
        s.reset();
    }
    if edit.flip_all {
        s.flip_all();
    }
    if let Some(i) = edit.flip {
        s.toggle_flip(i)?;
    }
    if let Some(i) = edit.drop {
        s.toggle_drop(i)?;
    }
    Ok(())
}

/// Applies `edit` to the saved state of `dir`, saves it, returns the review of `open`.
pub fn set_review(dir: &Path, open: &str, edit: &Edit) -> Result<Value> {
    // Edits read the file, change it and write it back: one at a time.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load(dir).context("这里还没有找出的结果")?;
    apply(&mut s, edit)?;
    save(dir, &s)?;
    Ok(review_value(&s, dir, open))
}

/// The runs `[y, x0, x1)` of a membrane item on section `z` of the open
/// file, for the highlight; empty for a vesicle, a tomogram that was not
/// searched, and a section the item does not reach. When the open file is not
/// the file the item was found in, the voxels are carried over (a section of
/// the open file takes the nearest section of the other).
pub fn section_runs(s: &State, dir: &Path, id: usize, open: &str, z: usize) -> Result<Vec<[usize; 3]>> {
    let Some(item) = s.items.get(id).filter(|i| i.kind == ItemKind::Membrane) else { return Ok(vec![]) };
    let Some((src, to_look)) = s.frame(open) else { return Ok(vec![]) };
    if src != item.source {
        return Ok(vec![]);
    }
    let [w, h, d] = item.n;
    if w == 0 {
        return Ok(vec![]);
    }
    // Open voxel = a × item voxel + b.
    let (a, b): (f64, [f64; 3]) = match (item.on_look, to_look) {
        (x, y) if x == y => (1.0, [0.0; 3]),
        (from, _) => {
            let Some(l) = s.looks[src].as_ref() else { return Ok(vec![]) };
            if from { (l.scale, l.offset) } else { (1.0 / l.scale, l.offset.map(|o| -o / l.scale)) }
        }
    };
    let zi = ((z as f64 - b[2]) / a).round();
    if zi < 0.0 || zi >= d as f64 {
        return Ok(vec![]);
    }
    let zi = zi as usize;
    let mask = read_side(dir, id)?.mask;
    let plane = w * h;
    let lo = mask.partition_point(|&i| (i as usize) < zi * plane);
    let hi = mask.partition_point(|&i| (i as usize) < (zi + 1) * plane);
    // The cells of the open file a voxel covers on one axis, [from, to).
    let cover = |v: usize, axis: usize| -> (i64, i64) {
        let c = a * v as f64 + b[axis];
        if a >= 1.0 {
            let f = (c - a / 2.0 + 0.5 + 1e-6).floor() as i64;
            (f.max(0), (f + a.round() as i64).max(0))
        } else {
            let f = c.round().max(0.0) as i64;
            (f, f + 1)
        }
    };
    let mut cells: Vec<[i64; 3]> = vec![];
    for &i in &mask[lo..hi] {
        let r = i as usize - zi * plane;
        let (y, x) = (r / w, r % w);
        let (x0, x1) = cover(x, 0);
        let (y0, y1) = cover(y, 1);
        for yy in y0..y1 {
            cells.push([yy, x0, x1]);
        }
    }
    cells.sort_unstable();
    let mut out: Vec<[usize; 3]> = vec![];
    for [y, x0, x1] in cells {
        match out.last_mut() {
            Some(l) if l[0] == y as usize && x0 as usize <= l[2] => l[2] = l[2].max(x1 as usize),
            _ => out.push([y as usize, x0 as usize, x1 as usize]),
        }
    }
    Ok(out)
}

/// The sampling points that are averaged, in raw voxels: those of the kept
/// items with the user's flips applied, and the lines as drawn, thinned
/// evenly to `MOST_SEEDS`; whether the sense of the axes is unknown.
pub fn kept_drawn(s: &State, dir: &Path) -> Result<(Vec<Drawn>, bool)> {
    if let Some(why) = s.stale() {
        bail!(why);
    }
    let mut out: Vec<Drawn> = vec![];
    for item in s.kept() {
        let side = read_side(dir, item.id)?;
        let sign = if s.flipped(item.id) { -1.0 } else { 1.0 };
        let look = if item.on_look { Some(s.looks[item.source].as_ref().context("清晰版断层图不见了，请重新找")?) } else { None };
        out.extend(side.seeds.iter().map(|k| Drawn { source: item.source, pos: look.map_or(k.pos, |l| to_raw(k.pos, 1, 1, l)), axis: k.axis.map(|a| sign * a) }));
    }
    out.extend(s.drawn.iter().map(|p| Drawn { source: p.source, pos: p.pos, axis: p.axis }));
    if out.len() < MIN_SEEDS {
        bail!("采样点只有 {} 个，至少要 {MIN_SEEDS} 个：请少去掉几个，或多标几个例子", out.len());
    }
    if out.len() > MOST_SEEDS {
        let n = out.len();
        out = (0..MOST_SEEDS).map(|k| out[k * n / MOST_SEEDS].clone()).collect();
    }
    Ok((out, s.both()))
}

/// What the average's `chosen.json` says it was made of.
pub fn used_of(s: &State) -> Value {
    let n = |k: ItemKind| s.kept().filter(|i| i.kind == k).count();
    json!({
        "kind": "shapes", "membranes": n(ItemKind::Membrane), "balls": n(ItemKind::Ball), "spheres": n(ItemKind::Ball), "lines": n(ItemKind::Line), "surfaces": s.surfaces,
        "dropped": s.edits.dropped.len(), "flipped": s.edits.flipped.len(), "sizeA": s.size_a.round(),
    })
}

fn out_of(inputs: &[String], out: &str) -> Result<PathBuf> {
    let Some(first) = inputs.first() else { bail!("请先选择断层图") };
    Ok(if out.is_empty() { auto::default_out(first) } else { PathBuf::from(out) })
}

/// The command's search: resolves the tomograms, `find_in`, saves. Returns
/// `{ needSegment: [files] }` when some file has no segmentation (nothing
/// was done: segment them, `paint::segment`, and call again), else the
/// review (`review_value`) of the first tomogram.
pub fn find_shapes(inputs: &[String], marks: &[Mark], size: Option<&str>, out: &str, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    let inputs = raw_inputs(inputs)?;
    let out = out_of(&inputs, out)?;
    events(emit, b64, |progress, _live, note| {
        let sources: Vec<Source> = inputs.iter().map(|i| resolve_tomogram(i, note).source()).collect();
        let (inputs, sources) = marks::without_looks(&inputs, sources);
        let pixel = |p: &str| sp::pixel_of(p).ok();
        let size = Size::parse(size);
        let size_a = size_of(marks, size, &pixel);
        match find_in(&sources, &inputs, marks, size, size_a, &pixel, progress)? {
            Found::NeedSegment(files) => Ok(json!({ "needSegment": files })),
            Found::Done(state, sides) => {
                let dir = out.join(DRAWN);
                save_new(&dir, &state, &sides)?;
                Ok(review_value(&state, &dir, &inputs[0]))
            }
        }
    })
}

/// Averages what the review of `<out>/drawn` keeps (`kept_drawn`) into that
/// folder. Same events and result as `marks::run_marks` for shapes.
pub fn average_shapes(out: &str, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    let dir = dir_of(out);
    let s = load(&dir).context("这里还没有找出的结果")?;
    let (drawn, both) = kept_drawn(&s, &dir)?;
    events(emit, b64, |progress, live, _note| {
        let note = |n: auto::Note| emit("sta-note", note_json(&n));
        let sources: Vec<Source> = s.inputs.iter().map(|i| resolve_tomogram(i, &note).source()).collect();
        let report = sp::run_drawn(&sources, &drawn, both, s.size_a, &dir.to_string_lossy(), progress, live)?;
        write_used(&dir, &used_of(&s));
        Ok(json!({
            "needsChoice": false, "report": serde_json::to_value(&report)?, "result": result_value(&dir, b64)?,
            "cards": [], "chosen": Value::Null, "auto": false, "out": dir, "trials": [], "drawn": true, "sizeA": s.size_a,
            "pointsLeftOut": false,
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::paint::Stroke;

    const N: usize = 96;

    /// A tomogram with vesicles (centre, radius), a coat outside each (inside
    /// where `inward`), and its label file (as the tests of `membranes`).
    fn file(dir: &Path, name: &str, shells: &[([f64; 3], f64)], inward: &[bool]) -> String {
        let path = dir.join(name).to_string_lossy().into_owned();
        let mut state = 11u32;
        let mut noise = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let at = |i: usize| [(i % N) as f64, (i / N % N) as f64, (i / (N * N)) as f64];
        let off = |p: [f64; 3], c: [f64; 3], r: f64| dist(p, c) - r;
        let mask: Vec<bool> = (0..N * N * N).map(|i| shells.iter().any(|(c, r)| off(at(i), *c, *r).abs() <= 1.5)).collect();
        let vol: Vec<f32> = (0..N * N * N)
            .map(|i| {
                let coat = shells.iter().zip(inward).any(|((c, r), inw)| {
                    let d = off(at(i), *c, *r);
                    if *inw { (-8.0..=-4.0).contains(&d) } else { (4.0..=8.0).contains(&d) }
                });
                -(if mask[i] { 1.0 } else { 0.0 }) - (if coat { 0.5 } else { 0.0 }) + 0.6 * noise()
            })
            .collect();
        crate::tomo::output::write_mrc(&path, &vol, [N; 3], 10.0, false).unwrap();
        crate::membrane::write_labels(&path, [N; 3], &mask, None, 10.0).unwrap();
        path
    }

    fn source(path: &str, look: Option<Look>) -> Source {
        Source { path: path.into(), tilts: [-60.0, 60.0], look, series: None }
    }

    fn painted(tomogram: &str, at: [f64; 3]) -> Mark {
        Mark { kind: Kind::Membrane, tomogram: tomogram.into(), points: vec![], flip: false, strokes: vec![Stroke { points: vec![at], cut: false }] }
    }

    fn px(_: &str) -> Option<f64> {
        Some(10.0)
    }

    fn quiet(_: &str, _: usize, _: usize) {}

    fn done(f: Found) -> (State, Vec<Side>) {
        match f {
            Found::Done(s, sides) => (*s, sides),
            Found::NeedSegment(v) => panic!("needs segmentation: {v:?}"),
        }
    }

    /// Two tomograms: `a` with the example (radius 14), one like it and a much larger one; `b` with one like it coated inside.
    fn two(dir: &Path) -> (String, String) {
        let a = file(dir, "a.mrc", &[([22.0, 22.0, 30.0], 14.0), ([70.0, 26.0, 34.0], 15.0), ([48.0, 66.0, 60.0], 27.0)], &[false; 3]);
        let b = file(dir, "b.mrc", &[([40.0, 40.0, 40.0], 14.0)], &[true]);
        (a, b)
    }

    fn side_with(n: usize, axis: [f64; 3]) -> Side {
        Side { seeds: (0..n).map(|k| Seed { pos: [10.0 + k as f64, 20.0, 30.0], axis }).collect(), mask: vec![] }
    }

    /// A hand-made state in a temp folder: items of `looks`' first input, each with 12 seeds along +x.
    fn made(dir: &Path, items: &[(ItemKind, bool, bool)]) -> State {
        let look = Look { path: "/t/a_look.mrc".into(), scale: 2.0, offset: [4.0, 6.0, 8.0] };
        let inputs = vec!["/t/a.mrc".to_string(), "/t/b.mrc".to_string()];
        let list: Vec<Item> = items
            .iter()
            .enumerate()
            .map(|(id, (kind, on_look, example))| Item { id, kind: *kind, source: id % 2, on_look: *on_look && id % 2 == 0, example: *example, side_sure: *example, centre: [10.0, 20.0, 30.0], radius: if *kind == ItemKind::Ball { 7.0 } else { 0.0 }, voxels: 0, n: [0; 3], seeds: 12, score: 0.0, length: 0.0, px: 0.0, weak: false })
            .collect();
        let s = State { items: list, edits: Edits::default(), stamps: input_stamps(inputs.iter().map(String::as_str)), inputs, looks: vec![Some(look), None], size_a: 130.0, both: false, surfaces: 0, drawn: vec![] };
        let sides: Vec<Side> = s.items.iter().map(|_| side_with(12, [1.0, 0.0, 0.0])).collect();
        save_new(dir, &s, &sides).unwrap();
        s
    }

    #[test]
    fn a_search_round_trips_through_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = two(dir.path());
        let sources = vec![source(&a, None), source(&b, None)];
        let inputs = vec![a.clone(), b.clone()];
        let (s, sides) = done(find_in(&sources, &inputs, &[painted(&a, [8.0, 22.0, 30.0])], Size::Small, 80.0, &px, &quiet).unwrap());
        assert_eq!(s.items.len(), 3, "{:?}", s.items);
        assert!(s.items[0].example && s.items[0].side_sure && s.items[0].kind == ItemKind::Membrane);
        assert_eq!((s.items[1].source, s.items[2].source), (0, 1));
        assert!(s.items.iter().all(|i| !i.on_look && i.seeds == sides[i.id].seeds.len() && i.voxels == sides[i.id].mask.len() && i.n == [N; 3]));
        let out = dir.path().join("out");
        save_new(&out, &s, &sides).unwrap();
        let back = load(&out).unwrap();
        assert_eq!(back.items, s.items);
        assert_eq!(back.stamps, s.stamps);
        assert_eq!(back.size_a, 80.0);
        let side = read_side(&out, 1).unwrap();
        assert_eq!(side.mask, sides[1].mask);
        assert_eq!(side.seeds.len(), sides[1].seeds.len());
        assert!(dist(side.seeds[3].pos, sides[1].seeds[3].pos) < 1e-3 && dist(side.seeds[3].axis, sides[1].seeds[3].axis) < 1e-6);
        // The review of the first tomogram: its two membranes, arrows thinned and unit.
        let r = review(&out, &a).unwrap();
        assert_eq!((r["total"].as_u64(), r["kept"].as_u64(), r["membranes"].as_u64(), r["balls"].as_u64()), (Some(3), Some(3), Some(3), Some(0)));
        assert_eq!(r["perTomogram"][0]["membranes"], 2);
        assert_eq!(r["perTomogram"][1]["membranes"], 1);
        assert_eq!(r["items"].as_array().unwrap().len(), 2);
        let arrows = r["items"][1]["arrows"].as_array().unwrap();
        assert!(!arrows.is_empty() && arrows.len() <= MOST_ARROWS);
        assert_eq!(r["unsure"], 0);
        assert_eq!(r["etaSeconds"], (60.0 + 0.25 * s.seeds() as f64).round());
        assert!(review(&out.join("nothing"), &a).is_none());
        // The same JSON as the command receives it.
        let e: Edit = serde_json::from_str(r#"{"drop":2,"flipAll":true}"#).unwrap();
        assert_eq!((e.drop, e.flip, e.flip_all, e.reset), (Some(2), None, true, false));
    }

    #[test]
    fn a_file_without_labels_asks_for_segmentation_and_nothing_else_happens() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = two(dir.path());
        let c = dir.path().join("c.mrc").to_string_lossy().into_owned();
        crate::tomo::output::write_mrc(&c, &vec![0.0; 8 * 8 * 8], [8; 3], 10.0, false).unwrap();
        let sources = vec![source(&a, None), source(&b, None), source(&c, None)];
        let inputs = vec![a.clone(), b.clone(), c.clone()];
        match find_in(&sources, &inputs, &[painted(&a, [8.0, 22.0, 30.0])], Size::Small, 80.0, &px, &quiet).unwrap() {
            Found::NeedSegment(v) => assert_eq!(v, vec![c.clone()]),
            Found::Done(..) => panic!("searched without segmentation"),
        }
        // No example: say what to do. An example on a tomogram that is not ticked: say so.
        assert!(find_in(&sources, &inputs, &[], Size::Small, 80.0, &px, &quiet).err().unwrap().to_string().contains("涂一笔"));
        let other = painted("/t/zzz.mrc", [1.0; 3]);
        let err = find_in(&sources, &inputs, &[painted(&a, [8.0, 22.0, 30.0]), other], Size::Small, 80.0, &px, &quiet).err().unwrap();
        assert!(err.to_string().contains("没有被勾选"));
    }

    #[test]
    fn marked_vesicles_are_found_as_balls_with_their_radius() {
        let dir = tempfile::tempdir().unwrap();
        let shells = [([24.0, 24.0, 30.0], 14.0), ([70.0, 26.0, 34.0], 15.0), ([26.0, 70.0, 60.0], 14.5), ([68.0, 68.0, 70.0], 15.0)];
        let a = file(dir.path(), "a.mrc", &shells, &[false, false, false, true]);
        let sphere = |c: [f64; 3], r: f64| Mark { kind: Kind::Sphere, tomogram: a.clone(), points: vec![c, [c[0] + r, c[1], c[2]]], flip: false, strokes: vec![] };
        let marks = [sphere([26.0, 22.0, 31.0], 12.0), sphere([68.0, 27.0, 33.0], 17.0)];
        // A vesicle alone gives the size by its radius.
        let size_a = size_of(&marks, Size::Large, &px);
        // Radii 12 and 17 voxels of 10 Å: 0.5 × 145 Å = 72 Å, held to the lowest size run_drawn serves.
        assert_eq!(size_a, 90.0);
        let (s, sides) = done(find_in(&[source(&a, None)], &[a.clone()], &marks, Size::Large, 80.0, &px, &quiet).unwrap());
        assert_eq!(s.items.len(), 4, "{:?}", s.items);
        assert!(s.items.iter().all(|i| i.kind == ItemKind::Ball && i.radius > 10.0 && i.n == [0; 3] && sides[i.id].mask.is_empty()));
        assert_eq!(s.items.iter().filter(|i| i.example).count(), 2);
        assert_eq!(s.items.iter().filter(|i| i.seeds > 0).count(), 4);
        // Lines and sizes: with a membrane or line mark the chosen size is taken.
        let line = Mark { kind: Kind::Line, tomogram: a.clone(), points: vec![[1.0; 3], [9.0; 3]], flip: false, strokes: vec![] };
        let mixed: Vec<Mark> = marks.iter().cloned().chain([line]).collect();
        assert_eq!(size_of(&mixed, Size::Large, &px), 360.0);
        let (m, _) = done(find_in(&[source(&a, None)], &[a.clone()], &mixed, Size::Small, 130.0, &px, &quiet).unwrap());
        // A line is searched for, not used as drawn: the one drawn is an example (the sheets of this volume may add weak "filaments").
        assert!(m.surfaces == 0 && !m.both && m.drawn.is_empty());
        let lines: Vec<&Item> = m.items.iter().filter(|i| i.kind == ItemKind::Line).collect();
        assert!(lines.iter().filter(|i| i.example).count() == 1 && lines.iter().all(|i| i.example || i.weak == (i.score < filaments::SURE)), "{lines:?}");
        assert!(m.both(), "a line is kept, so the sense of the axes is unknown");
    }


    /// Noise of unit spread with three dark threads along x (y = 30, 64, 98; z = 24), half-width 2.
    fn threads(dir: &Path) -> String {
        let n = [128usize, 128, 48];
        let mut seed = 7u64;
        let mut noise = || {
            let mut sum = 0.0f32;
            for _ in 0..12 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                sum += (seed >> 40) as f32 / (1u64 << 24) as f32;
            }
            sum - 6.0
        };
        let mut v = vec![0f32; n[0] * n[1] * n[2]];
        for z in 0..n[2] {
            for y in 0..n[1] {
                for x in 0..n[0] {
                    let d = [30.0, 64.0, 98.0].iter().map(|yc| ((y as f64 - yc).powi(2) + (z as f64 - 24.0).powi(2)).sqrt()).fold(f64::INFINITY, f64::min);
                    let on = if (14..=114).contains(&x) { (-3.0 * (-0.5 * (d / 2.0).powi(2)).exp()) as f32 } else { 0.0 };
                    v[(z * n[1] + y) * n[0] + x] = noise() + on;
                }
            }
        }
        let path = dir.join("threads.mrc").to_string_lossy().into_owned();
        crate::tomo::output::write_mrc(&path, &v, n, 10.0, false).unwrap();
        path
    }

    fn drawn_line(tomogram: &str) -> Mark {
        let points = (20..=80).step_by(12).enumerate().map(|(k, x)| [x as f64, 30.0 + if k % 2 == 0 { 1.0 } else { -1.0 }, 24.0]).collect();
        Mark { kind: Kind::Line, tomogram: tomogram.into(), points, flip: false, strokes: vec![] }
    }

    #[test]
    fn one_drawn_line_finds_all_filaments_like_it() {
        let dir = tempfile::tempdir().unwrap();
        let a = threads(dir.path());
        // Lines alone are a valid search: no segmentation is needed.
        let (s, sides) = done(find_in(&[source(&a, None)], &[a.clone()], &[drawn_line(&a)], Size::Small, 80.0, &px, &quiet).unwrap());
        assert_eq!(s.items.len(), 3, "{:?}", s.items.iter().map(|i| (i.score, i.length)).collect::<Vec<_>>());
        assert!(s.items.iter().all(|i| i.kind == ItemKind::Line && i.side_sure && i.px == 10.0 && i.seeds == sides[i.id].seeds.len() && i.seeds >= 10 && !i.weak));
        assert_eq!(s.items.iter().filter(|i| i.example).count(), 1);
        // Half a particle (4 voxels) apart along a filament about 100 voxels long.
        assert!(s.items.iter().all(|i| (15..=35).contains(&i.seeds)), "{:?}", s.items.iter().map(|i| i.seeds).collect::<Vec<_>>());
        assert!(s.drawn.is_empty() && s.surfaces == 0 && s.both());
        // The review carries the course of each, counts them and says which were kept.
        let out = dir.path().join("out");
        save_new(&out, &s, &sides).unwrap();
        let r = review(&out, &a).unwrap();
        assert_eq!((r["lines"].as_u64(), r["membranes"].as_u64(), r["weak"].as_u64(), r["surfaces"].as_u64()), (Some(3), Some(0), Some(0), Some(0)));
        assert_eq!(r["perTomogram"][0]["lines"], 3);
        let it = &r["items"][0];
        assert!(it["kind"] == "line" && it["arrows"].as_array().unwrap().is_empty() && it["lengthA"].as_f64().unwrap() > 800.0);
        let pts = it["line"].as_array().unwrap();
        assert!(pts.len() == s.items[0].seeds && pts.len() <= MOST_LINE);
        // No membrane segmentation was needed, and a line gives nothing to the section highlight.
        assert!(section_runs(&s, &out, 0, &a, 24).unwrap().is_empty());
    }

    #[test]
    fn weak_filaments_wait_for_the_user_and_flips_do_nothing_to_lines() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = made(dir.path(), &[(ItemKind::Line, false, true), (ItemKind::Line, false, false), (ItemKind::Line, false, false), (ItemKind::Membrane, false, false)]);
        for i in &mut s.items[..3] {
            i.score = 0.9;
            i.length = 50.0;
        }
        s.items[0].score = 1.0;
        s.items[1].weak = true;
        s.items[1].score = 0.4;
        s.items[1].px = 10.0;
        s.edits.dropped = vec![3];
        save(dir.path(), &s).unwrap();
        // The weak one is left out at first.
        assert!(s.dropped(1) && !s.dropped(0) && !s.dropped(2) && s.dropped(3));
        assert_eq!((s.kept().count(), s.left_weak(), s.seeds()), (2, 1, 24));
        let r = review(dir.path(), "/t/a.mrc").unwrap();
        assert_eq!((r["kept"].as_u64(), r["lines"].as_u64(), r["weak"].as_u64(), r["dropped"].as_u64()), (Some(2), Some(2), Some(1), Some(1)));
        let weak = r["items"].as_array().unwrap().iter().find(|i| i["id"] == 0).unwrap();
        assert!(weak["weak"] == false && weak["state"] == "kept" && weak["lengthVoxels"] == 50.0);
        // Items 0 and 2 are input 0's (id % 2), 1 and 3 input 1's.
        let b = review(dir.path(), "/t/b.mrc").unwrap();
        let weak = b["items"].as_array().unwrap().iter().find(|i| i["id"] == 1).unwrap();
        assert!(weak["weak"] == true && weak["state"] == "dropped" && weak["lengthA"] == 500.0 && (weak["score"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        // Only the kept lines are averaged; the sense along a filament is unknown.
        let (d, both) = kept_drawn(&s, dir.path()).unwrap();
        assert!(both && !s.both);
        assert_eq!(d.len(), 24);
        assert!(d[..12].iter().all(|x| x.source == 0) && d[12..].iter().all(|x| x.source == 0));
        // Taking the weak one, and leaving it out again.
        let r = set_review(dir.path(), "/t/a.mrc", &Edit { drop: Some(1), ..Edit::default() }).unwrap();
        assert_eq!((r["kept"].as_u64(), r["weak"].as_u64(), r["dropped"].as_u64()), (Some(3), Some(0), Some(1)));
        assert_eq!(load(dir.path()).unwrap().edits.kept, vec![1]);
        assert_eq!(kept_drawn(&load(dir.path()).unwrap(), dir.path()).unwrap().0.len(), 36);
        let r = set_review(dir.path(), "/t/a.mrc", &Edit { drop: Some(1), ..Edit::default() }).unwrap();
        assert_eq!(r["kept"], 2);
        // A strong one is dropped the usual way; a flip does nothing to a line.
        set_review(dir.path(), "/t/a.mrc", &Edit { drop: Some(2), flip: Some(2), ..Edit::default() }).unwrap();
        let r = set_review(dir.path(), "/t/a.mrc", &Edit { flip_all: true, ..Edit::default() }).unwrap();
        let s = load(dir.path()).unwrap();
        assert_eq!((s.edits.dropped.clone(), s.edits.flipped.clone()), (vec![3, 2], vec![3]));
        assert_eq!(r["items"][0]["flipped"], false);
        // Reset: the weak one is out again, the user's drops are forgotten.
        set_review(dir.path(), "/t/a.mrc", &Edit { drop: Some(1), ..Edit::default() }).unwrap();
        let r = set_review(dir.path(), "/t/a.mrc", &Edit { reset: true, ..Edit::default() }).unwrap();
        let s = load(dir.path()).unwrap();
        assert_eq!((s.edits.kept.len(), s.edits.dropped.len(), s.edits.flipped.len()), (0, 0, 0));
        assert_eq!((r["kept"].as_u64(), r["weak"].as_u64()), (Some(3), Some(1)));
        // A saved file of before has no `kept`, `weak` or `score`, and `lines` meant surfaces.
        let old: Edits = serde_json::from_str(r#"{"dropped":[1],"flipped":[]}"#).unwrap();
        assert!(old.kept.is_empty());
    }

    #[test]
    fn drop_flip_flip_all_and_reset_are_kept_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = made(dir.path(), &[(ItemKind::Membrane, false, true), (ItemKind::Membrane, false, false), (ItemKind::Ball, false, false), (ItemKind::Ball, false, false)]);
        assert_eq!(s.items.len(), 4);
        let r = set_review(dir.path(), "/t/a.mrc", &Edit { drop: Some(2), ..Edit::default() }).unwrap();
        assert_eq!((r["kept"].as_u64(), r["total"].as_u64(), r["dropped"].as_u64(), r["membranes"].as_u64(), r["balls"].as_u64()), (Some(3), Some(4), Some(1), Some(2), Some(1)));
        // The dropped one has no arrows and says so.
        let it = r["items"].as_array().unwrap().iter().find(|i| i["id"] == 2).unwrap();
        assert!(it["state"] == "dropped" && it["arrows"].as_array().unwrap().is_empty());
        // Taking it back.
        let r = set_review(dir.path(), "/t/a.mrc", &Edit { drop: Some(2), ..Edit::default() }).unwrap();
        assert_eq!(r["kept"], 4);
        // One flip reverses its arrows only; a second flip restores them.
        let before = review(dir.path(), "/t/a.mrc").unwrap();
        let r = set_review(dir.path(), "/t/a.mrc", &Edit { flip: Some(0), ..Edit::default() }).unwrap();
        let (x0, x1) = (before["items"][0]["arrows"][0]["axis"][0].as_f64().unwrap(), r["items"][0]["arrows"][0]["axis"][0].as_f64().unwrap());
        assert!((x0 + x1).abs() < 1e-9 && x0 != 0.0);
        assert_eq!(r["items"][0]["flipped"], true);
        // (The tomogram a holds items 0 and 2.)
        assert_eq!(r["items"][1]["arrows"][0]["axis"][0], before["items"][1]["arrows"][0]["axis"][0]);
        // Flip all turns every one the other way, the flipped one back.
        let r = set_review(dir.path(), "/t/a.mrc", &Edit { flip_all: true, ..Edit::default() }).unwrap();
        assert_eq!(r["flipped"], 3);
        assert_eq!(r["items"][0]["flipped"], false);
        assert_eq!(load(dir.path()).unwrap().edits.flipped, vec![1, 2, 3]);
        // Reset forgets it all.
        let r = set_review(dir.path(), "/t/a.mrc", &Edit { reset: true, ..Edit::default() }).unwrap();
        assert_eq!((r["flipped"].as_u64(), r["dropped"].as_u64(), r["kept"].as_u64()), (Some(0), Some(0), Some(4)));
        assert!(set_review(dir.path(), "/t/a.mrc", &Edit { drop: Some(9), ..Edit::default() }).is_err());
        assert!(set_review(&dir.path().join("nothing"), "/t/a.mrc", &Edit::default()).is_err());
    }

    #[test]
    fn unsure_items_are_counted_until_the_user_flips_or_drops_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = made(dir.path(), &[(ItemKind::Membrane, false, true), (ItemKind::Membrane, false, false), (ItemKind::Ball, false, false)]);
        s.items[1].side_sure = false;
        s.items[2].side_sure = true;
        assert_eq!(s.unsure(), 1);
        s.toggle_flip(1).unwrap();
        assert_eq!(s.unsure(), 0);
        s.toggle_flip(1).unwrap();
        s.toggle_drop(1).unwrap();
        assert_eq!(s.unsure(), 0);
    }

    #[test]
    fn the_averaged_seeds_have_the_flips_applied_and_the_lines_added() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = made(dir.path(), &[(ItemKind::Membrane, false, true), (ItemKind::Membrane, false, false), (ItemKind::Ball, false, false), (ItemKind::Ball, false, false)]);
        s.drawn = vec![Placed { source: 1, pos: [5.0; 3], axis: [0.0, 1.0, 0.0] }; 3];
        s.both = true;
        s.toggle_flip(1).unwrap();
        s.toggle_drop(3).unwrap();
        save(dir.path(), &s).unwrap();
        let (d, both) = kept_drawn(&s, dir.path()).unwrap();
        assert!(both);
        // Items 0, 1, 2 with 12 seeds each, then the 3 line points.
        assert_eq!(d.len(), 36 + 3);
        assert!(d[..12].iter().all(|x| x.axis == [1.0, 0.0, 0.0] && x.source == 0));
        assert!(d[12..24].iter().all(|x| x.axis == [-1.0, 0.0, 0.0] && x.source == 1));
        assert!(d[24..36].iter().all(|x| x.axis == [1.0, 0.0, 0.0] && x.source == 0));
        assert!(d[36..].iter().all(|x| x.pos == [5.0; 3] && x.source == 1));
        let u = used_of(&s);
        assert_eq!((u["membranes"].as_u64(), u["balls"].as_u64(), u["dropped"].as_u64(), u["flipped"].as_u64(), u["kind"].as_str()), (Some(2), Some(1), Some(1), Some(1), Some("shapes")));
        // Too few: one item of 12 seeds.
        let mut few = s.clone();
        few.drawn.clear();
        few.edits.dropped = vec![1, 2, 3];
        let err = kept_drawn(&few, dir.path()).err().unwrap().to_string();
        assert!(err.contains("采样点只有 12 个") && err.contains("至少要 20 个"), "{err}");
    }

    #[test]
    fn seeds_found_on_the_look_file_are_carried_to_the_raw_voxels() {
        let dir = tempfile::tempdir().unwrap();
        let s = made(dir.path(), &[(ItemKind::Membrane, true, true), (ItemKind::Membrane, false, true), (ItemKind::Ball, true, true)]);
        // Item 0 is on the look file of input 0 (raw = 2 × look + (4, 6, 8)), item 1 is raw, item 2 is input 0's.
        assert!(s.items[0].on_look && !s.items[1].on_look && s.items[2].on_look);
        let mut s = s;
        s.drawn = vec![Placed { source: 0, pos: [0.0; 3], axis: [1.0, 0.0, 0.0] }; 3];
        let (d, _) = kept_drawn(&s, dir.path()).unwrap();
        assert!((d[0].pos[0] - (2.0 * 10.0 + 4.0)).abs() < 1e-3 && (d[0].pos[1] - (2.0 * 20.0 + 6.0)).abs() < 1e-3 && (d[0].pos[2] - (2.0 * 30.0 + 8.0)).abs() < 1e-3);
        assert!((d[12].pos[0] - 10.0).abs() < 1e-3, "the raw item stays");
        // Shown on the raw file, the look item's centre and radius are carried too; on the look file they stay.
        let on_raw = review_value(&s, dir.path(), "/t/a.mrc");
        let on_look = review_value(&s, dir.path(), "/t/a_look.mrc");
        let ball = |v: &Value| v["items"].as_array().unwrap().iter().find(|i| i["id"] == 2).unwrap().clone();
        assert_eq!(ball(&on_raw)["centre"], json!([24.0, 46.0, 68.0]));
        assert_eq!(ball(&on_look)["centre"], json!([10.0, 20.0, 30.0]));
        assert_eq!((ball(&on_raw)["radius"].as_f64(), ball(&on_look)["radius"].as_f64()), (Some(14.0), Some(7.0)));
        assert_eq!(ball(&on_raw)["arrows"][0]["pos"], json!([24.0, 46.0, 68.0]));
        assert_eq!(on_look["open"]["look"], true);
        assert_eq!(on_raw["open"]["look"], false);
        // A file that was not searched shows nothing.
        assert!(review_value(&s, dir.path(), "/t/zzz.mrc")["items"].as_array().unwrap().is_empty());
    }

    #[test]
    fn section_runs_of_a_membrane_in_either_frame() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = made(dir.path(), &[(ItemKind::Membrane, true, true), (ItemKind::Ball, false, false)]);
        // A plate of voxels x 4..8, y 3..5 on look sections 5 and 6 (grid 20 × 10 × 10).
        let (w, h) = (20usize, 10usize);
        let mask: Vec<u32> = [5usize, 6].iter().flat_map(|z| (3..5).flat_map(move |y| (4..8).map(move |x| (z * w * h + y * w + x) as u32))).collect();
        s.items[0].n = [w, h, 10];
        s.items[0].voxels = mask.len();
        write_side(dir.path(), 0, &Side { seeds: vec![Seed { pos: [5.0, 3.0, 5.0], axis: [1.0, 0.0, 0.0] }], mask }).unwrap();
        // Open on the look file: as written.
        let runs = section_runs(&s, dir.path(), 0, "/t/a_look.mrc", 5).unwrap();
        assert_eq!(runs, vec![[3, 4, 8], [4, 4, 8]]);
        assert!(section_runs(&s, dir.path(), 0, "/t/a_look.mrc", 4).unwrap().is_empty());
        assert!(section_runs(&s, dir.path(), 0, "/t/a_look.mrc", 99).unwrap().is_empty());
        // Open on the raw file (raw = 2 × look + offset): section 18 is look section 5; each look voxel is 2 wide.
        let raw = section_runs(&s, dir.path(), 0, "/t/a.mrc", 18).unwrap();
        // x 4..8 → centres 12, 14, 16, 18, each two cells wide → cells 11..19; y 3, 4 → centres 12, 14 → rows 11 to 14.
        assert_eq!(raw.len(), 4, "{raw:?}");
        assert!(raw.iter().all(|r| r[1] == 11 && r[2] == 19), "{raw:?}");
        assert_eq!(raw.iter().map(|r| r[0]).collect::<Vec<_>>(), vec![11, 12, 13, 14]);
        assert!(section_runs(&s, dir.path(), 0, "/t/a.mrc", 3).unwrap().is_empty());
        // A vesicle, a file that was not searched, an item that is not there: nothing.
        assert!(section_runs(&s, dir.path(), 1, "/t/b.mrc", 5).unwrap().is_empty());
        assert!(section_runs(&s, dir.path(), 0, "/t/zzz.mrc", 5).unwrap().is_empty());
        assert!(section_runs(&s, dir.path(), 7, "/t/a.mrc", 5).unwrap().is_empty());
        // An item on the raw file seen on the look file: one cell per voxel, the nearest section (look 0 is raw section 8).
        s.items[0].on_look = false;
        let plate: Vec<u32> = (3..5).flat_map(|y| (4..8).map(move |x| (8 * w * h + y * w + x) as u32)).collect();
        write_side(dir.path(), 0, &Side { seeds: vec![], mask: plate }).unwrap();
        let on_look = section_runs(&s, dir.path(), 0, "/t/a_look.mrc", 0).unwrap();
        assert!(!on_look.is_empty() && on_look.iter().all(|r| r[1] < r[2]), "{on_look:?}");
    }

    #[test]
    fn changed_tomograms_say_to_find_again() {
        let dir = tempfile::tempdir().unwrap();
        let tomo = dir.path().join("t.mrc");
        std::fs::write(&tomo, b"abc").unwrap();
        let mut s = made(&dir.path().join("out"), &[(ItemKind::Membrane, false, true), (ItemKind::Membrane, false, false), (ItemKind::Ball, false, false), (ItemKind::Ball, false, false)]);
        s.inputs = vec![tomo.to_string_lossy().into_owned(), "/t/b.mrc".into()];
        s.stamps = input_stamps(s.inputs.iter().map(String::as_str));
        assert!(s.stale().is_none());
        assert!(review_value(&s, dir.path(), "/t/b.mrc")["stale"].is_null());
        assert!(kept_drawn(&s, &dir.path().join("out")).is_ok());
        std::fs::write(&tomo, b"abcdef").unwrap();
        assert_eq!(s.stale().as_deref(), Some("断层图变了，请重新找"));
        assert_eq!(review_value(&s, dir.path(), "/t/b.mrc")["stale"], "断层图变了，请重新找");
        assert_eq!(kept_drawn(&s, &dir.path().join("out")).err().unwrap().to_string(), "断层图变了，请重新找");
    }

    #[test]
    fn a_new_search_clears_the_old_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let s = made(dir.path(), &[(ItemKind::Membrane, false, true), (ItemKind::Ball, false, false)]);
        assert!(side_path(dir.path(), 1).is_file());
        let mut fewer = s.clone();
        fewer.items.truncate(1);
        save_new(dir.path(), &fewer, &[side_with(3, [0.0, 0.0, 1.0])]).unwrap();
        assert!(!side_path(dir.path(), 1).exists());
        assert_eq!(read_side(dir.path(), 0).unwrap().seeds.len(), 3);
        assert!(read_side(dir.path(), 1).is_err());
        std::fs::write(side_path(dir.path(), 0), b"junk").unwrap();
        assert!(read_side(dir.path(), 0).is_err());
    }
}
