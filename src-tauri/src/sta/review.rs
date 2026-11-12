//! "Find them all, show them, then average" for particles (the workspace's
//! card route, clicked-particles route and the route of a map the user
//! brings, `own_map`): the template search runs alone
//! (`project::find_template`), what it found is saved beside the cards as
//! `found.json` together with the user's edits, and only when the user says
//! so are the kept places averaged (`project::run_places`).
//!
//! The state is plain data (`State`), so the edits, the places shown on a
//! tomogram and the list that is averaged are all computed without a
//! tomogram and are unit-tested that way. The functions that search and
//! average (`find_card`, `find_points`, `average_found`) wrap them with the
//! same events as `auto::run_inputs` (`sta-progress`, `sta-live`, `sta-note`).
//!
//! Two things sit on top of the user's own edits:
//! - "Only on the shapes I marked": when the user has a shapes review for the
//!   same output folder, the places found are checked against it once, when
//!   they are found (`shapes_filter`, with `near`); the ones that are not on
//!   a marked shape are "off" while the filter is on.
//! - "Find again" (`find_again`): the places the user did not touch are
//!   judged again by what the user kept and dropped (`taught`); the places
//!   that go and come back are two more lists in the edits. A place the user
//!   clicked is never changed by either.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::auto::{self, card_json, input_stamps, live_png, note_json, raw_inputs, resolve_tomogram, result_value, slices_png, write_used, Device, Emit, InputStamp, Note, MARKED};
use super::marks::{self, Mark};
use super::project::{self as sp, Found, Place, BOX};
use super::refine::{Live, Progress};
use super::shape::Seed;
use super::{cc, circled, near, own_map, shapes, taught};
use crate::spa::geom::{apply as turn_by, distance, grid, transpose};
use crate::tomo::dataset::Look;

/// The file in the output folder of a route that holds the search and the edits.
pub const FILE: &str = "found.json";

/// The time an average takes: a minute of search and set-up, then, measured
/// on an M1 Max, about 0.15 s per particle after the search.
pub const ETA_BASE_S: f64 = 60.0;
pub const ETA_PER_PARTICLE_S: f64 = 0.15;

/// Fewest particles that are averaged (circled examples: `Found::fewest`).
pub const MIN_KEPT: usize = sp::FEWEST;
/// How many places beyond the kept ones are shown on a tomogram, to add from.
pub const MARGIN: usize = 300;
/// Most scores handed to the score curve.
pub const MOST_SCORES: usize = 3000;
/// How many places are kept at first when the scores do not fall into two
/// groups (the user decides with the slider).
pub const UNSURE_KEEP: usize = 300;
/// Most particles `project::run_places` takes (it cuts the rest off).
pub const MOST_KEPT: usize = 3000;

fn yes() -> bool {
    true
}

/// What the user changed from the program's suggestion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edits {
    /// The slider: how many of the best-first places are kept.
    pub keep: usize,
    /// Places among the first `keep` the user dropped.
    #[serde(default)]
    pub removed: Vec<usize>,
    /// Places after the first `keep` the user added.
    #[serde(default)]
    pub added: Vec<usize>,
    /// Only the places on the shapes the user marked count (when there are
    /// such shapes, `State::filter`).
    #[serde(default = "yes")]
    pub on_shapes: bool,
    /// "Find again" took these among the first `keep` out, as unlike what the user kept ...
    #[serde(default)]
    pub taught_out: Vec<usize>,
    /// ... and brought these, after the first `keep`, in as like it.
    #[serde(default)]
    pub taught_in: Vec<usize>,
    /// "Find again" has been run (it may have changed nothing).
    #[serde(default)]
    pub again: bool,
    /// Circled examples: the groups (which example a place is like, which
    /// size it has) the user unticked; their places do not count.
    #[serde(default)]
    pub groups_off: Vec<[u8; 2]>,
}

impl Edits {
    fn new(keep: usize) -> Edits {
        Edits { keep, removed: vec![], added: vec![], on_shapes: true, taught_out: vec![], taught_in: vec![], again: false, groups_off: vec![] }
    }
}

/// Which places lie on the shapes the user marked and stand on them as most
/// do, worked out when the places were found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filter {
    /// Per place of `Found::places`.
    pub on: Vec<bool>,
    /// A common way of standing on the shapes was found.
    pub standing: bool,
    /// The kinds of shape there are: "membrane", "ball", "line".
    pub kinds: Vec<String>,
}

/// How the template was made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    /// "card" (with `card`, its id), "points" (with `points`, the clicks),
    /// "circled" (with `points`, the examples circled) or "own" (a map the
    /// user brought).
    pub kind: String,
    #[serde(default)]
    pub card: Option<usize>,
    #[serde(default)]
    pub points: Option<usize>,
}

/// What `found.json` holds.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub found: Found,
    /// The template's MRC (`card_N.mrc`, or `marked/card.mrc`).
    pub template: String,
    /// The particle size the search worked with (`template_diameter`), Å.
    pub diameter_a: f64,
    pub card: sp::Card,
    /// The tomograms searched (raw files), in source order, as they were then.
    pub inputs: Vec<String>,
    pub stamps: Vec<InputStamp>,
    /// Per source: its look companion (to show places on it) and the voxel size (Å).
    #[serde(default)]
    pub looks: Vec<Option<Look>>,
    #[serde(default)]
    pub pixels: Vec<f64>,
    pub edits: Edits,
    pub route: Route,
    /// The scores did not fall into two groups: the program cannot say which are particles.
    pub unsure: bool,
    /// The places against the shapes the user marked (None: there were none).
    #[serde(default)]
    pub filter: Option<Filter>,
    /// Circled examples: their own places in `found.places`. They are kept
    /// whatever they score, until the user drops one.
    #[serde(default)]
    pub mine: Vec<usize>,
    /// Circled examples: per example, the best score the other examples'
    /// pictures have on it (`circled::Circled::by_others`).
    #[serde(default)]
    pub by_others: Vec<Option<f32>>,
}

/// What a place is, for the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Kept,
    Below,
    Removed,
    Added,
    /// Not on a shape the user marked, while only those count.
    Off,
    /// Taken out by "find again" (it looked like what the user dropped).
    TaughtOut,
    /// Brought in by "find again" (it looked like what the user kept).
    TaughtIn,
    /// Of a group of the circled examples the user unticked.
    GroupOff,
}

impl Shown {
    pub fn name(self) -> &'static str {
        match self {
            Shown::Kept => "kept",
            Shown::Below => "below",
            Shown::Removed => "removed",
            Shown::Added => "added",
            Shown::Off => "off",
            Shown::TaughtOut => "taughtOut",
            Shown::TaughtIn => "taughtIn",
            Shown::GroupOff => "groupOff",
        }
    }
}

/// The edits' lists as sets, for looking places up quickly.
struct Lists {
    removed: std::collections::HashSet<usize>,
    added: std::collections::HashSet<usize>,
    out: std::collections::HashSet<usize>,
    inn: std::collections::HashSet<usize>,
}

/// A tomogram of the search, as the open file shows it.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub source: usize,
    /// The open file is the look companion (raw = scale × look + offset).
    pub look: Option<&'a Look>,
}

fn same(a: &str, b: &str) -> bool {
    a == b || std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(x, y)| x == y)
}

impl State {
    /// A fresh state: the program's suggestion kept.
    #[allow(clippy::too_many_arguments)]
    pub fn new(found: Found, template: String, diameter_a: f64, card: sp::Card, inputs: Vec<String>, looks: Vec<Option<Look>>, pixels: Vec<f64>, route: Route) -> State {
        let stamps = input_stamps(inputs.iter().map(String::as_str));
        let unsure = found.limit.is_none();
        let mut s = State { found, template, diameter_a, card, inputs, stamps, looks, pixels, edits: Edits::new(0), route, unsure, filter: None, mine: vec![], by_others: vec![] };
        s.edits.keep = s.suggest();
        s
    }

    /// The most places there can be.
    pub fn total(&self) -> usize {
        self.found.places.len()
    }

    /// What the program would keep: the places above the limit; when there is
    /// none (the scores do not part), the best `UNSURE_KEEP`.
    pub fn suggest(&self) -> usize {
        let n = match self.found.limit {
            Some(_) => self.found.sure(),
            None => UNSURE_KEEP,
        };
        n.clamp(1, MOST_KEPT).min(self.total())
    }

    /// What a place is. `taught`: the lists of "find again" count; `filtered`:
    /// the places off the marked shapes are "off". The user's own clicks
    /// (`removed`, `added`) win over both.
    fn state_with(&self, l: &Lists, i: usize, taught: bool, filtered: bool) -> Shown {
        // What the user circled is what the user wants.
        if self.mine.contains(&i) {
            return if l.removed.contains(&i) { Shown::Removed } else { Shown::Kept };
        }
        if !self.edits.groups_off.is_empty() && self.found.places.get(i).is_some_and(|p| self.edits.groups_off.contains(&[p.like, p.size])) {
            return Shown::GroupOff;
        }
        if filtered && self.filter.as_ref().is_some_and(|f| f.on.get(i) == Some(&false)) {
            return Shown::Off;
        }
        let (rm, add) = (l.removed.contains(&i), l.added.contains(&i));
        if i < self.edits.keep {
            if rm {
                Shown::Removed
            } else if taught && l.out.contains(&i) {
                if add { Shown::Added } else { Shown::TaughtOut }
            } else {
                Shown::Kept
            }
        } else if add {
            Shown::Added
        } else if taught && l.inn.contains(&i) && !rm {
            Shown::TaughtIn
        } else {
            Shown::Below
        }
    }

    fn lists(&self) -> Lists {
        let e = &self.edits;
        Lists { removed: e.removed.iter().copied().collect(), added: e.added.iter().copied().collect(), out: e.taught_out.iter().copied().collect(), inn: e.taught_in.iter().copied().collect() }
    }

    pub fn state_of(&self, i: usize) -> Shown {
        self.state_with(&self.lists(), i, true, self.edits.on_shapes)
    }

    fn counts(s: Shown) -> bool {
        matches!(s, Shown::Kept | Shown::Added | Shown::TaughtIn)
    }

    /// The places that are averaged, as indices into `found.places` (best
    /// first): the first `keep` but the removed and the ones "find again"
    /// took out, the added and the ones it brought in; none that is off.
    pub fn kept(&self) -> Vec<usize> {
        let l = self.lists();
        let mut out: Vec<usize> = (0..self.total()).filter(|i| Self::counts(self.state_with(&l, *i, true, self.edits.on_shapes))).collect();
        out.truncate(MOST_KEPT);
        out
    }

    /// (removed, added) that count: those the slider has not made meaningless.
    pub fn changes(&self) -> (usize, usize) {
        let keep = self.edits.keep.min(self.total());
        let removed = self.edits.removed.iter().filter(|i| **i < keep).count();
        let mut added: Vec<usize> = self.edits.added.iter().copied().filter(|i| *i >= keep && *i < self.total()).collect();
        added.sort_unstable();
        added.dedup();
        (removed, added.len())
    }

    /// (taken out, brought in) by "find again" that count: not overruled by a click.
    pub fn again_counts(&self) -> (usize, usize) {
        let l = self.lists();
        let (mut out, mut inn) = (0, 0);
        for i in 0..self.total() {
            match self.state_with(&l, i, true, self.edits.on_shapes) {
                Shown::TaughtOut => out += 1,
                Shown::TaughtIn => inn += 1,
                _ => {}
            }
        }
        (out, inn)
    }

    /// Places on the marked shapes among those that would count without the
    /// filter; 0 when there are no shapes.
    pub fn on_shapes_count(&self) -> usize {
        let Some(f) = &self.filter else { return 0 };
        let l = self.lists();
        (0..self.total()).filter(|i| f.on.get(*i) != Some(&false) && Self::counts(self.state_with(&l, *i, true, false))).count()
    }

    /// Where `open` (a searched tomogram or its look companion) is among the sources.
    pub fn frame(&self, open: &str) -> Option<Frame<'_>> {
        if let Some(source) = self.inputs.iter().position(|p| same(p, open)) {
            return Some(Frame { source, look: None });
        }
        self.looks.iter().enumerate().find_map(|(source, l)| l.as_ref().filter(|l| same(&l.path, open)).map(|look| Frame { source, look: Some(look) }))
    }

    /// The places of source `s` shown on its tomogram: the best `keep + MARGIN`
    /// of them (those that are off the marked shapes do not use up the margin).
    pub fn shown(&self, s: usize) -> Vec<usize> {
        let most = self.edits.keep + MARGIN;
        let l = self.lists();
        let mut out = vec![];
        let mut on = 0;
        for i in (0..self.total()).filter(|i| self.found.places[*i].source == s) {
            let off = matches!(self.state_with(&l, i, true, self.edits.on_shapes), Shown::Off | Shown::GroupOff);
            if !off {
                on += 1;
                if on > most {
                    break;
                }
            }
            out.push(i);
        }
        // The examples and what the user brought in show wherever they stand in the list.
        out.extend(self.mine.iter().chain(&self.edits.added).copied().filter(|i| self.found.places.get(*i).is_some_and(|p| p.source == s)));
        out.sort_unstable();
        out.dedup();
        out
    }

    /// A voxel position of the open file as raw voxels.
    pub fn to_raw(frame: &Frame, p: [f64; 3]) -> [f64; 3] {
        match frame.look {
            Some(l) => std::array::from_fn(|k| l.scale * p[k] + l.offset[k]),
            None => p,
        }
    }

    /// A raw voxel position as the open file's voxels (as `marks::preview_of`).
    pub fn to_open(frame: &Frame, p: [f64; 3]) -> [f64; 3] {
        match frame.look {
            Some(l) => std::array::from_fn(|k| (p[k] - l.offset[k]) / l.scale),
            None => p,
        }
    }

    /// The radius of a particle in raw voxels.
    fn radius_raw(&self, source: usize) -> Option<f64> {
        let px = self.pixels.get(source).copied().filter(|p| *p > 0.0)?;
        Some(0.5 * self.diameter_a / px)
    }

    /// The groups the places of circled examples fall into, as (like, size),
    /// in order; empty on the other routes.
    pub fn groups(&self) -> Vec<[u8; 2]> {
        if self.route.kind != "circled" {
            return vec![];
        }
        let mut out: Vec<[u8; 2]> = self.found.places.iter().map(|p| [p.like, p.size]).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Ticks or unticks a group.
    pub fn set_group(&mut self, group: [u8; 2], on: bool) {
        self.edits.groups_off.retain(|g| *g != group);
        if !on {
            self.edits.groups_off.push(group);
        }
    }

    /// The radius of place `i` in raw voxels: of its own size when the
    /// search had several (circled examples).
    fn radius_of(&self, i: usize) -> Option<f64> {
        let p = self.found.places.get(i)?;
        let px = self.pixels.get(p.source).copied().filter(|p| *p > 0.0)?;
        Some(0.5 * self.found.sizes_a.get(p.size as usize).copied().unwrap_or(self.diameter_a) / px)
    }

    /// The radius of a particle in the open file's voxels.
    pub fn radius(&self, frame: &Frame) -> Option<f64> {
        let r = self.radius_raw(frame.source)?;
        Some(r / frame.look.map_or(1.0, |l| l.scale))
    }

    pub fn set_keep(&mut self, keep: usize) {
        self.edits.keep = keep.clamp(usize::from(self.total() > 0), self.total().min(MOST_KEPT));
    }

    /// Back to the program's suggestion, no edits.
    pub fn reset(&mut self) {
        self.edits = Edits::new(self.suggest());
    }

    /// Takes back what "find again" did (the user's own clicks stay).
    pub fn undo_again(&mut self) {
        self.edits.taught_out.clear();
        self.edits.taught_in.clear();
        self.edits.again = false;
    }

    /// Clicking a place: kept → removed, removed → kept again, below → added,
    /// added → below again.
    pub fn toggle(&mut self, i: usize) -> Result<()> {
        if i >= self.total() {
            bail!("没有第 {i} 个");
        }
        // A place "find again" took out comes back by a click (the click is
        // an "added"), one it brought in goes by a click (a "removed").
        let list = match self.state_of(i) {
            Shown::Off => bail!("这个不在你标的形状上"),
            Shown::GroupOff => bail!("这一组没有勾选"),
            Shown::Kept | Shown::Removed | Shown::TaughtIn => &mut self.edits.removed,
            Shown::Below | Shown::Added | Shown::TaughtOut => &mut self.edits.added,
        };
        match list.iter().position(|x| *x == i) {
            Some(at) => {
                list.remove(at);
            }
            None => list.push(i),
        }
        Ok(())
    }

    /// A click where no circle is: the nearest place of that tomogram that is
    /// not kept, within one radius, is brought in. Where the search found
    /// nothing at all, the click itself becomes a place on the route of
    /// circled examples (the user sees one the program missed): upright,
    /// since every orientation is tried again before the average (`run_places`).
    /// `pos` is in the open file's voxels.
    pub fn add_at(&mut self, open: &str, pos: [f64; 3]) -> Result<usize> {
        const NONE: &str = "这里没有找到像例子的东西";
        let frame = self.frame(open).context("这张断层图不在找出的断层图里")?;
        let source = frame.source;
        let r = self.radius_raw(source).context(NONE)?;
        let at = Self::to_raw(&frame, pos);
        let l = self.lists();
        let near: Vec<(usize, Shown, f64)> = (0..self.total())
            .filter(|i| self.found.places[*i].source == source)
            .map(|i| {
                let c = self.found.places[i].at();
                (i, self.state_with(&l, i, true, self.edits.on_shapes), (0..3).map(|k| (c[k] - at[k]).powi(2)).sum::<f64>().sqrt())
            })
            .filter(|(_, _, d)| *d <= r)
            .collect();
        let best = near.iter().filter(|(_, s, _)| matches!(s, Shown::Below | Shown::Removed | Shown::TaughtOut)).min_by(|a, b| a.2.total_cmp(&b.2));
        let i = match best {
            Some((i, _, _)) => *i,
            None if self.found.frames.is_empty() => bail!(NONE),
            None if near.iter().any(|(_, s, _)| Self::counts(*s)) => bail!("这里已经有一个了"),
            None => {
                let px = self.pixels.get(source).copied().filter(|p| *p > 0.0).context(NONE)?;
                let places = &self.found.places;
                let bin = places.iter().find(|p| p.source == source).map_or(((self.found.voxel_a / px).round() as usize).max(1), |p| p.bin);
                let c = at.map(|x| ((x - (bin as f64 - 1.0) / 2.0) / bin as f64).round().max(0.0) as usize);
                let turns = grid(30.0);
                let turn = (0..turns.len()).min_by(|a, b| distance(&turns[*a], &super::IDENTITY).total_cmp(&distance(&turns[*b], &super::IDENTITY))).unwrap_or(0);
                // Of the first example's kind; last in the list, which is best first.
                let (like, size) = self.mine.first().and_then(|i| places.get(*i)).map_or((0, 0), |p| (p.like, p.size));
                let score = places.last().map_or(0.0, |p| p.score);
                self.found.places.push(Place { source, c, bin, turn, score, like, size });
                self.total() - 1
            }
        };
        self.toggle(i)?;
        Ok(i)
    }

    /// Why the tomograms cannot be taken as the ones searched (None: the same).
    pub fn stale(&self) -> Option<String> {
        let now = input_stamps(self.inputs.iter().map(String::as_str));
        auto::stale(Some(&self.stamps), &now).map(|_| "断层图变了，请重新找".to_string())
    }

    /// Seconds an average of `n` particles takes, about.
    pub fn eta(n: usize) -> f64 {
        ETA_BASE_S + ETA_PER_PARTICLE_S * n as f64
    }
}

pub fn path_of(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Saves the state (written to a side file and renamed, so a crash leaves the old one).
pub fn save(dir: &Path, s: &State) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("无法创建 {}", dir.display()))?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(s)?)?;
    std::fs::rename(&tmp, path_of(dir))?;
    Ok(())
}

/// The state saved in `dir`; None when there is none or it cannot be read.
pub fn load(dir: &Path) -> Option<State> {
    let mut s: State = std::fs::read(path_of(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok())?;
    // The folder may have been copied elsewhere.
    if !Path::new(&s.template).is_file() {
        let guess = match s.route.kind.as_str() {
            "points" | "own" => dir.join("card.mrc"),
            _ => dir.join("cards").join(format!("card_{}.mrc", s.card.id)),
        };
        if guess.is_file() {
            s.template = guess.to_string_lossy().into_owned();
        }
    }
    s.edits.keep = s.edits.keep.min(s.total());
    Some(s)
}

fn millis(p: &Path) -> Option<u64> {
    let t = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64)
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// `scores` thinned evenly to at most `MOST_SCORES`, best first.
fn scores_of(found: &Found) -> Vec<f32> {
    let n = found.places.len();
    let take = n.min(MOST_SCORES);
    (0..take).map(|j| found.places[j * n / take.max(1)].score).collect()
}

/// What the review panel and the overlay show for the tomogram `open`:
/// `{ total, keep, kept, sure, unsure, limit, removed, added, perTomogram, scores, etaSeconds,
///    places, radius, open, route, card, stale, foundMs, averageMs, onShapes, again }`.
/// `places` are those of the open tomogram only, as `{i, pos, score, state}`
/// in the open file's voxels.
pub fn review_value(s: &State, dir: &Path, open: &str) -> Value {
    let kept = s.kept();
    let per: Vec<Value> = s
        .inputs
        .iter()
        .enumerate()
        .map(|(src, p)| {
            let name = Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
            json!({ "name": name, "path": p, "kept": kept.iter().filter(|i| s.found.places[**i].source == src).count() })
        })
        .collect();
    let (removed, added) = s.changes();
    let (again_out, again_in) = s.again_counts();
    let lists = s.lists();
    let frame = s.frame(open);
    let circled = s.route.kind == "circled";
    let (places, radius, opened): (Vec<Value>, Value, Value) = match &frame {
        Some(f) => {
            let list = s
                .shown(f.source)
                .into_iter()
                .map(|i| {
                    let p = State::to_open(f, s.found.places[i].at());
                    let place = &s.found.places[i];
                    let mut v = json!({ "i": i, "pos": [r1(p[0]), r1(p[1]), r1(p[2])], "score": (place.score * 1e4).round() / 1e4, "state": s.state_with(&lists, i, true, s.edits.on_shapes).name() });
                    if circled {
                        v["like"] = json!(place.like);
                        v["size"] = json!(place.size);
                        v["example"] = json!(s.mine.contains(&i));
                        v["r"] = s.radius_of(i).map_or(Value::Null, |r| json!(r1(r / f.look.map_or(1.0, |l| l.scale))));
                    }
                    v
                })
                .collect();
            (list, s.radius(f).map_or(Value::Null, |r| json!(r1(r))), json!({ "source": f.source, "look": f.look.is_some() }))
        }
        None => (vec![], Value::Null, Value::Null),
    };
    let mut circled_v = circled_value(s, &lists);
    if !circled_v.is_null() {
        // Told apart from another search by when its first example's cube
        // was written: `found.json` is rewritten by every edit.
        circled_v["sig"] = json!(millis(&dir.join("card.mrc")));
    }
    json!({
        "total": s.total(), "keep": s.edits.keep, "kept": kept.len(), "sure": s.suggest(), "unsure": s.unsure,
        "limit": s.found.limit, "removed": removed, "added": added, "perTomogram": per,
        "scores": scores_of(&s.found), "etaSeconds": State::eta(kept.len()).round(),
        "places": places, "radius": radius, "open": opened, "route": s.route, "card": card_json(&s.card),
        "stale": s.stale(), "searched": s.found.searched, "out": dir,
        "foundMs": millis(&path_of(dir)), "averageMs": millis(&dir.join("average.mrc")),
        "onShapes": on_shapes_value(s),
        "again": { "out": again_out, "in": again_in, "ran": s.edits.again },
        "circled": circled_v, "fewest": s.found.fewest(),
    })
}

/// `circled` of the review, for the route of circled examples (null on the
/// others): `examples` (how many were circled), `alike` (what they score
/// with each other; null with one example), `byOthers` (per example, what
/// the others' pictures score on it), `noise` (the best chance scored),
/// `sizesA`, and `groups`: per (like, size) `{like, size, sizeA, on, found,
/// kept}`, where `found` are its places the slider and the user's clicks
/// keep and `kept` those of them that count now; and `list`: the best
/// `tiles` places of all tomograms (and the examples), best first, as
/// `{i, source, score, like, size, example, state}`: each has a picture
/// (`tile_path`). `review_value` adds `sig`, which changes only with a new
/// search (the pictures are that search's).
fn circled_value(s: &State, lists: &Lists) -> Value {
    if s.route.kind != "circled" {
        return Value::Null;
    }
    // What each group would give when ticked.
    let mut ticked = s.clone();
    ticked.edits.groups_off.clear();
    let groups: Vec<Value> = s
        .groups()
        .into_iter()
        .map(|g| {
            let on = !s.edits.groups_off.contains(&g);
            let of_group = |i: &usize| s.found.places[*i].like == g[0] && s.found.places[*i].size == g[1];
            let counted = |s: &State| (0..s.total()).filter(of_group).filter(|i| State::counts(s.state_with(lists, *i, true, s.edits.on_shapes))).count();
            json!({ "like": g[0], "size": g[1], "sizeA": s.found.sizes_a.get(g[1] as usize).map(|a| a.round()), "on": on, "found": counted(&ticked), "kept": counted(s) })
        })
        .collect();
    // The places with a picture, best first, whatever tomogram they are in:
    // what the strip of pictures shows.
    let list: Vec<Value> = (0..s.total().min(MOST_TILES))
        .chain(s.mine.iter().chain(&s.edits.added).copied().filter(|i| *i >= MOST_TILES && *i < s.total()))
        .map(|i| {
            let p = &s.found.places[i];
            json!({ "i": i, "source": p.source, "score": (p.score * 1e4).round() / 1e4, "like": p.like, "size": p.size, "example": s.mine.contains(&i), "state": s.state_with(lists, i, true, s.edits.on_shapes).name() })
        })
        .collect();
    json!({
        "examples": s.route.points, "alike": s.found.alike, "byOthers": s.by_others, "noise": s.found.noise,
        "sizesA": s.found.sizes_a.iter().map(|a| a.round()).collect::<Vec<_>>(), "groups": groups,
        "list": list, "tiles": MOST_TILES,
        // How many circle diameters a picture shows, and how many times as wide as circled the search looked.
        "span": circled::TILE_SPAN * s.found.wide, "wide": s.found.wide,
    })
}

/// `onShapes` of the review: `available` (shapes were marked when the places
/// were found), `on` (only the places on them count), `near` (places on them
/// that would count), `standing` (a common way of standing on them was found),
/// `kinds` (which shapes: "membrane", "ball", "line").
fn on_shapes_value(s: &State) -> Value {
    match &s.filter {
        Some(f) => json!({ "available": true, "on": s.edits.on_shapes, "near": s.on_shapes_count(), "standing": f.standing, "kinds": f.kinds }),
        None => json!({ "available": false, "on": false, "near": 0, "standing": false, "kinds": [] }),
    }
}

/// The review of `dir` for the open tomogram; None when nothing was found there.
pub fn review(dir: &Path, open: &str) -> Result<Option<Value>> {
    Ok(load(dir).map(|s| review_value(&s, dir, open)))
}

/// Reading, changing and writing `found.json` happens one at a time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A change to the edits (any of the fields).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub keep: Option<usize>,
    pub toggle: Option<usize>,
    /// A click on the open tomogram, in its voxels (where no circle is).
    pub add_at: Option<[f64; 3]>,
    /// Back to the program's suggestion.
    #[serde(default)]
    pub reset: bool,
    /// Only the places on the marked shapes count (or all of them again).
    pub on_shapes: Option<bool>,
    /// Takes back what "find again" did.
    #[serde(default)]
    pub undo_again: bool,
    /// Circled examples: ticks or unticks the group of the places that
    /// are like example `like` at size `size`.
    pub group: Option<GroupEdit>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct GroupEdit {
    pub like: u8,
    pub size: u8,
    pub on: bool,
}

/// Applies `edit` to the saved state of `dir`, saves it and returns the review of `open`.
pub fn set_review(dir: &Path, open: &str, edit: &Edit) -> Result<Value> {
    // Edits read the file, change it and write it back: one at a time.
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load(dir).context("这里还没有找出的结果")?;
    apply(&mut s, open, edit)?;
    save(dir, &s)?;
    Ok(review_value(&s, dir, open))
}

pub fn apply(s: &mut State, open: &str, edit: &Edit) -> Result<()> {
    if edit.reset {
        s.reset();
    }
    if edit.undo_again {
        s.undo_again();
    }
    if let Some(on) = edit.on_shapes {
        s.edits.on_shapes = on;
    }
    if let Some(g) = edit.group {
        s.set_group([g.like, g.size], g.on);
    }
    if let Some(k) = edit.keep {
        s.set_keep(k);
    }
    if let Some(i) = edit.toggle {
        s.toggle(i)?;
    }
    if let Some(p) = edit.add_at {
        s.add_at(open, p)?;
    }
    Ok(())
}

/// What goes into `average_found`: the kept places, or why not.
pub fn kept_places(s: &State) -> Result<Vec<Place>> {
    if let Some(why) = s.stale() {
        bail!(why);
    }
    let kept = s.kept();
    if kept.len() < s.found.fewest() {
        bail!("保留的只有 {} 个，至少要 {} 个", kept.len(), s.found.fewest());
    }
    Ok(kept.into_iter().map(|i| s.found.places[i]).collect())
}

/// The progress, live and note callbacks of a run, as `auto::run_inputs` makes them.
pub(super) fn events<R>(emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync), f: impl FnOnce(Progress, Live, &(dyn Fn(Note) + Sync)) -> Result<R>) -> Result<R> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
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
    let note = |n: Note| emit("sta-note", note_json(&n));
    f(&progress, &live, &note)
}

fn out_of(inputs: &[String], out: &str) -> Result<PathBuf> {
    let Some(first) = inputs.first() else { bail!("请先选择断层图") };
    Ok(if out.is_empty() { auto::default_out(first) } else { PathBuf::from(out) })
}

/// The search of one template over the sources, saved with the suggestion
/// as the edits. `card.path` is the template.
#[allow(clippy::too_many_arguments)]
fn search_and_save(root: &Path, dir: &Path, card: sp::Card, inputs: Vec<String>, sources: &[sp::Source], route: Route, device: Device, progress: Progress) -> Result<State> {
    let (dev, _) = device()?;
    let (cube, voxel) = sp::read_card(&card.path)?;
    let diameter = sp::template_diameter(&card);
    let found = crate::ctf_gpu::guarded(|| sp::find_template(dev, sources, &cube, voxel, diameter, false, progress))??;
    if found.places.is_empty() {
        bail!("没有找到像例子的东西");
    }
    let looks = sources.iter().map(|s| s.look.clone()).collect();
    let pixels = sources.iter().map(|s| sp::pixel_of(&s.path).unwrap_or(0.0)).collect();
    let mut state = State::new(found, card.path.clone(), diameter, card, inputs, looks, pixels, route);
    state.filter = shapes_filter(root, &state);
    save(dir, &state)?;
    Ok(state)
}

/// Finds everything like card `pick` in the tomograms of `inputs` (the cards
/// must exist or are made, as `auto::find`) and saves it in `out/found.json`;
/// returns `review_value` for the first tomogram.
pub fn find_card(inputs: &[String], pick: usize, out: &str, device: Device, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    let inputs = raw_inputs(inputs)?;
    let out = out_of(&inputs, out)?;
    events(emit, b64, |progress, live, note| {
        let sources: Vec<sp::Source> = inputs.iter().map(|i| resolve_tomogram(i, note).source()).collect();
        let cards = auto::find(&sources, &out, device, progress, live, note)?;
        let card = cards.iter().find(|c| c.id == pick).with_context(|| format!("没有第 {pick} 张卡片（共 {} 张）", cards.len()))?.clone();
        let route = Route { kind: "card".into(), card: Some(card.id), points: None };
        let state = search_and_save(&out, &out, card, inputs.clone(), &sources, route, device, progress)?;
        Ok(review_value(&state, &out, &inputs[0]))
    })
}

/// Finds everything like the map the user brought (`own_map`) in the tomograms
/// of `inputs` and saves it in `out/own/found.json`; returns `review_value`
/// for the first tomogram.
pub fn find_own(inputs: &[String], out: &str, device: Device, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    let inputs = raw_inputs(inputs)?;
    let out = out_of(&inputs, out)?;
    let dir = own_dir(&out);
    let mut card = own_card(&dir).context("请先选一张现成的密度图")?;
    card.path = dir.join("card.mrc").to_string_lossy().into_owned();
    events(emit, b64, |progress, _live, note| {
        let sources: Vec<sp::Source> = inputs.iter().map(|i| resolve_tomogram(i, note).source()).collect();
        let route = Route { kind: "own".into(), card: None, points: None };
        let state = search_and_save(&out, &dir, card.clone(), inputs.clone(), &sources, route, device, progress)?;
        Ok(review_value(&state, &dir, &inputs[0]))
    })
}

/// Finds everything like the particles clicked on (`marks`, point marks; one
/// is allowed, two or three are better) and saves it in `out/marked/found.json`.
pub fn find_points(inputs: &[String], marks: &[Mark], out: &str, device: Device, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    let inputs = raw_inputs(inputs)?;
    let out = out_of(&inputs, out)?;
    let dir = out.join(MARKED);
    events(emit, b64, |progress, live, note| {
        let sources: Vec<sp::Source> = inputs.iter().map(|i| resolve_tomogram(i, note).source()).collect();
        let (inputs, sources) = marks::without_looks(&inputs, sources);
        let points = marks::points_of(marks, &inputs, &sources)?;
        if points.is_empty() {
            bail!("请先在颗粒上点 1 到 3 个作例子");
        }
        let card = sp::card_from_points(&sources, &points, &dir.to_string_lossy(), progress, live)?;
        note(Note::Marked { clicked: points.len(), members: card.members, size_a: card.measured_a });
        let route = Route { kind: "points".into(), card: None, points: Some(points.len()) };
        let state = search_and_save(&out, &dir, card, inputs.clone(), &sources, route, device, progress)?;
        Ok(review_value(&state, &dir, &inputs[0]))
    })
}

/// Where the route of circled examples keeps its search and its average,
/// under the output folder.
pub const CIRCLED: &str = "circled";

/// Finds everything that looks like the examples circled (`marks`, example
/// marks; two or three are asked for, one is taken) and saves it in
/// `out/circled/found.json`. Nothing is asked of what the examples are.
pub fn find_circled(inputs: &[String], marks: &[Mark], out: &str, device: Device, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    circled_review(inputs, marks, out, Some(device), emit, b64)
}

/// What was circled and nothing else, saved as `find_circled` saves what it
/// finds (`circled::given`): every circle a place to average, all kept.
pub fn given_circled(inputs: &[String], marks: &[Mark], out: &str, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    circled_review(inputs, marks, out, None, emit, b64)
}

/// `find_circled` (with a device) or `given_circled` (without: nothing is searched for).
fn circled_review(inputs: &[String], marks: &[Mark], out: &str, device: Option<Device>, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    let inputs = raw_inputs(inputs)?;
    let out = out_of(&inputs, out)?;
    let dir = out.join(CIRCLED);
    events(emit, b64, |progress, _live, note| {
        let sources: Vec<sp::Source> = inputs.iter().map(|i| resolve_tomogram(i, note).source()).collect();
        let (inputs, sources) = marks::without_looks(&inputs, sources);
        let examples = marks::examples_of(marks, &inputs, &sources)?;
        if examples.is_empty() {
            bail!("请先圈出 2 到 3 个要找的东西作例子");
        }
        let c = match device {
            Some(device) => {
                let (dev, _) = device()?;
                crate::ctf_gpu::guarded(|| circled::find(dev, &sources, &examples, progress))??
            }
            None => circled::given(&sources, &examples)?,
        };
        std::fs::create_dir_all(&dir).with_context(|| format!("无法创建 {}", dir.display()))?;
        let path = dir.join("card.mrc").to_string_lossy().into_owned();
        crate::tomo::output::write_mrc(&path, &c.template, [BOX; 3], c.found.voxel_a, false)?;
        let (compact, elongation, blobs, offset) = sp::shape(&c.template, BOX, 0.5 * c.diameter_a / c.found.voxel_a);
        // The card of this route is the first example as it is: one cube, not an average.
        let card = sp::Card {
            id: 0,
            diameter_a: c.diameter_a / sp::DIAMETER_FACTOR,
            measured_a: c.diameter_a,
            voxel_a: c.found.voxel_a,
            members: examples.len(),
            share: 0.0,
            path: path.clone(),
            compact,
            elongation,
            blobs,
            offset,
            likely: false,
        };
        std::fs::write(dir.join("card.json"), serde_json::to_string_pretty(&card)?)?;
        let looks = sources.iter().map(|s| s.look.clone()).collect();
        let pixels = sources.iter().map(|s| sp::pixel_of(&s.path).unwrap_or(0.0)).collect();
        let route = Route { kind: "circled".into(), card: None, points: Some(examples.len()) };
        let mut state = State::new(c.found, path, c.diameter_a, card, inputs.clone(), looks, pixels, route);
        state.mine = c.mine;
        state.by_others = c.by_others;
        state.filter = shapes_filter(&out, &state);
        save(&dir, &state)?;
        progress("画出找到的", 0, 1);
        if let Err(e) = write_tiles(&dir, &state, &sources) {
            eprintln!("sta: the pictures of the places could not be written: {e:#}");
        }
        progress("画出找到的", 1, 1);
        Ok(review_value(&state, &dir, &inputs[0]))
    })
}

/// How many of the best places get a picture (`tile_path`); the examples
/// get one wherever they stand.
pub const MOST_TILES: usize = 600;

/// The picture of place `i` of the review in `dir` (`circled::tiles`).
pub fn tile_path(dir: &Path, i: usize) -> PathBuf {
    dir.join("tiles").join(format!("{i}.png"))
}

/// Writes the pictures of the best places of `s`, new for this search.
fn write_tiles(dir: &Path, s: &State, sources: &[sp::Source]) -> Result<()> {
    let folder = dir.join("tiles");
    let _ = std::fs::remove_dir_all(&folder);
    let mut ids: Vec<usize> = (0..s.total().min(MOST_TILES)).chain(s.mine.iter().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    write_tiles_of(dir, s, sources, &ids)
}

/// Writes the pictures of the places `ids` of `s`.
fn write_tiles_of(dir: &Path, s: &State, sources: &[sp::Source], ids: &[usize]) -> Result<()> {
    let folder = dir.join("tiles");
    std::fs::create_dir_all(&folder).with_context(|| format!("无法创建 {}", folder.display()))?;
    let places: Vec<(usize, [f64; 3], f64)> = ids.iter().filter_map(|i| Some((s.found.places.get(*i)?.source, s.found.places[*i].at(), s.radius_of(*i)?))).collect();
    if places.len() != ids.len() {
        bail!("没有像素大小");
    }
    let side = circled::TILE as u32;
    for (i, grey) in ids.iter().zip(circled::tiles(sources, &places, circled::TILE_SPAN * s.found.wide)?) {
        if let Some(grey) = grey {
            ::image::GrayImage::from_raw(side, side, grey).context("tile")?.save(tile_path(dir, *i)).with_context(|| format!("无法写入 {}", tile_path(dir, *i).display()))?;
        }
    }
    Ok(())
}

/// The picture (PNG) of place `i` of the review in `dir`. A place the user
/// brought in by a click where the search had found nothing has none yet:
/// it is made now.
pub fn tile(dir: &Path, i: usize) -> Result<Vec<u8>> {
    let png = tile_path(dir, i);
    if !png.is_file() {
        let s = load(dir).context("这里还没有找出的结果")?;
        if i < s.total() && s.edits.added.contains(&i) {
            let sources: Vec<sp::Source> = s.inputs.iter().map(|i| resolve_tomogram(i, &|_| {}).source()).collect();
            write_tiles_of(dir, &s, &sources, &[i])?;
        }
    }
    std::fs::read(&png).with_context(|| format!("{}", png.display()))
}

/// Averages the kept places of `dir`'s `found.json` (the first `keep` without
/// the removed, and the added), aligned and refined (`run_places`, fine pass),
/// into `dir`. Returns what the ordinary run of that route returns, so the
/// workspace shows it as it does today.
pub fn average_found(dir: &Path, device: Device, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    let s = load(dir).context("这里还没有找出的结果")?;
    let places = kept_places(&s)?;
    let (removed, added) = s.changes();
    events(emit, b64, |progress, live, note| {
        let sources: Vec<sp::Source> = s.inputs.iter().map(|i| resolve_tomogram(i, note).source()).collect();
        let (dev, _) = device()?;
        let (cube, _) = sp::read_card(&s.template)?;
        let report = crate::ctf_gpu::guarded(|| sp::run_places(dev, &sources, &cube, s.diameter_a, &s.found, &places, sp::Pass::Fine, &dir.to_string_lossy(), progress, live))??;
        let (again_out, again_in) = s.again_counts();
        let counts = json!({ "found": s.total(), "kept": places.len(), "removed": removed, "added": added, "onShapes": s.filter.is_some() && s.edits.on_shapes, "againOut": again_out, "againIn": again_in });
        let mut used = match s.route.kind.as_str() {
            "points" => json!({ "kind": "points", "clicked": s.route.points, "members": s.card.members, "sizeA": s.card.measured_a.round() }),
            "circled" => json!({ "kind": "circled", "examples": s.route.points, "sizeA": s.card.measured_a.round(), "sizesA": s.found.sizes_a.iter().map(|a| a.round()).collect::<Vec<_>>(), "alike": s.found.alike }),
            "own" => json!({ "kind": "own", "name": own_file(dir).map(|o| o.name), "sizeA": s.card.measured_a.round() }),
            _ => json!({ "kind": "auto", "card": s.card.id, "auto": false, "sizeA": s.card.measured_a.round() }),
        };
        // `kept` places went in; `averaged` of them could be cut out of their tomogram
        // (`run_places` leaves out those whose cube reaches past the edge of the tomogram).
        used["averaged"] = json!(report.particles);
        used = auto::with_tilt(used, sources.iter().map(|s| s.path.as_str()));
        for (k, v) in counts.as_object().into_iter().flatten() {
            used[k] = v.clone();
        }
        write_used(dir, &used);
        let result = result_value(dir, b64)?;
        Ok(if s.route.kind != "card" {
            json!({
                "needsChoice": false, "report": serde_json::to_value(&report)?, "result": result,
                "cards": [], "chosen": Value::Null, "auto": false, "out": dir, "trials": [], "drawn": true,
                "points": s.route.points, "sizeA": s.card.measured_a, "own": s.route.kind == "own",
            })
        } else {
            let cards = auto::cards_value(dir, b64)?;
            json!({
                "needsChoice": false, "report": serde_json::to_value(&report)?, "result": result,
                "cards": cards["cards"], "chosen": s.card.id, "auto": false, "out": dir, "trials": [],
            })
        })
    })
}

/// Which folder `which` ("card", "marked", "circled", "own") names under the output folder `out`.
pub fn dir_of(out: &str, which: &str) -> Result<PathBuf> {
    match which {
        "card" => Ok(PathBuf::from(out)),
        "marked" => Ok(Path::new(out).join(MARKED)),
        CIRCLED => Ok(Path::new(out).join(CIRCLED)),
        OWN => Ok(own_dir(Path::new(out))),
        _ => bail!("which 应为 card、marked、circled 或 own"),
    }
}

// ---- the map the user brings ----

/// Where the route of a map the user brings keeps its card, its search and its average, under the output folder.
pub const OWN: &str = "own";
/// Beside the card: which map it is made from.
const OWN_FILE: &str = "own.json";

pub fn own_dir(out: &Path) -> PathBuf {
    out.join(OWN)
}

/// The map a card was made from, as it was when the card was made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnFile {
    pub map: String,
    pub name: String,
    pub size: u64,
    pub modified_ns: u128,
    /// The map did not say its voxel size: the tomogram's was taken.
    pub assumed: bool,
}

pub fn own_file(dir: &Path) -> Option<OwnFile> {
    serde_json::from_slice(&std::fs::read(dir.join(OWN_FILE)).ok()?).ok()
}

/// The card of the map saved in `dir`.
fn own_card(dir: &Path) -> Option<sp::Card> {
    own_file(dir)?;
    serde_json::from_slice(&std::fs::read(dir.join("card.json")).ok()?).ok()
}

/// The map's size and modified time now.
fn stamp_of(path: &str) -> (u64, u128) {
    let m = std::fs::metadata(path).ok();
    let ns = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    (m.map_or(0, |m| m.len()), ns)
}

/// What the workspace shows of the map: `{ card, png, name, assumed, changed }`
/// (`changed`: the file is gone or was changed after the card was made from it).
fn own_value(dir: &Path, b64: &dyn Fn(&[u8]) -> String) -> Option<Value> {
    let o = own_file(dir)?;
    let mut card = own_card(dir)?;
    card.path = dir.join("card.mrc").to_string_lossy().into_owned();
    let png = sp::read_card(&card.path).ok().and_then(|(cube, _)| slices_png(&cube, BOX, 64).ok()).map(|p| b64(&p));
    let changed = stamp_of(&o.map) != (o.size, o.modified_ns);
    Some(json!({ "card": card_json(&card), "png": png, "name": o.name, "assumed": o.assumed, "changed": changed }))
}

/// The map saved for output folder `out`, or None.
pub fn own_saved(out: &str, b64: &dyn Fn(&[u8]) -> String) -> Option<Value> {
    own_value(&own_dir(Path::new(out)), b64)
}

/// Makes a card of the map `map` for the tomograms of `inputs` (it takes their
/// voxel size, the first one's) and keeps it under `out`; a second map
/// replaces the first, with what was found with it. Returns `own_value`.
pub fn own_map(inputs: &[String], map: &str, out: &str, b64: &dyn Fn(&[u8]) -> String) -> Result<Value> {
    let inputs = raw_inputs(inputs)?;
    let out = out_of(&inputs, out)?;
    let dir = own_dir(&out);
    let tomogram_a = sp::pixel_of(&inputs[0])?;
    let (_card, assumed) = own_map::card_from_map(map, tomogram_a, &dir.to_string_lossy())?;
    // The map is good: what belonged to the one before goes.
    for e in std::fs::read_dir(&dir)?.flatten() {
        let name = e.file_name();
        if e.path().is_file() && name != "card.mrc" && name != "card.json" {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let (size, modified_ns) = stamp_of(map);
    let name = Path::new(map).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| map.to_string());
    std::fs::write(dir.join(OWN_FILE), serde_json::to_vec(&OwnFile { map: map.to_string(), name, size, modified_ns, assumed })?)?;
    own_value(&dir, b64).context("无法保存这张密度图")
}

/// `auto::result_pick` (which lists the map route, `out/own`, too).
pub fn result_pick(out: &Path, which: Option<&str>, b64: &dyn Fn(&[u8]) -> String) -> Result<Option<Value>> {
    auto::result_pick(out, which, b64)
}

// ---- only on the shapes the user marked ----

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let n = dot(a, a).sqrt();
    if n > 1e-12 { a.map(|x| x / n) } else { [0.0, 0.0, 1.0] }
}

/// The places of `s` against the shapes the user marked for the output folder
/// `out` (`<out>/drawn/shapes.json`): None when there is no such review, it
/// is stale, or nothing is kept in it. Per source the places within `reach`
/// of a kept shape's sampling points (one card diameter and half the spacing
/// of the points, raw voxels) are on a shape; of those, the ones standing on
/// it as most do (`near::stand`, over all the sources) stay.
pub fn shapes_filter(out: &Path, s: &State) -> Option<Filter> {
    let dir = shapes::dir_of(&out.to_string_lossy());
    let marked = shapes::load(&dir)?;
    if marked.stale().is_some() || marked.kept().next().is_none() {
        return None;
    }
    let (drawn, both) = shapes::kept_drawn(&marked, &dir).ok()?;
    // The shapes' tomograms among the places' tomograms.
    let source_of: Vec<Option<usize>> = marked.inputs.iter().map(|p| s.inputs.iter().position(|q| same(p, q))).collect();
    let turns = grid(30.0);
    let places = &s.found.places;
    // Per place: the axis of the nearest sampling point.
    let mut axis: Vec<Option<[f64; 3]>> = vec![None; places.len()];
    for src in 0..s.inputs.len() {
        let seeds: Vec<Seed> = drawn.iter().filter(|d| source_of.get(d.source).copied().flatten() == Some(src)).map(|d| Seed { pos: d.pos, axis: d.axis }).collect();
        let px = s.pixels.get(src).copied().unwrap_or(0.0);
        if seeds.is_empty() || px <= 0.0 {
            continue;
        }
        // The points are half a particle apart; a particle's centre is within a diameter of its membrane.
        let reach = s.card.diameter_a / px + 0.5 * (0.5 * marked.size_a / px);
        let idx: Vec<usize> = (0..places.len()).filter(|i| places[*i].source == src).collect();
        let positions: Vec<[f64; 3]> = idx.iter().map(|i| places[*i].at()).collect();
        for (k, at) in near::nearest(&positions, &seeds, reach).into_iter().enumerate() {
            axis[idx[k]] = at.map(|j| seeds[j].axis);
        }
    }
    let turn_of = |i: usize| s.found.turn_of(&places[i], &turns);
    let pairs: Vec<(crate::spa::geom::Mat, [f64; 3])> = (0..places.len()).filter_map(|i| Some((turn_of(i)?, axis[i]?))).collect();
    let stand = near::stand(&pairs, both);
    let cone = near::CONE.to_radians().cos();
    let on: Vec<bool> = (0..places.len())
        .map(|i| {
            let (Some(a), Some(r)) = (axis[i], turn_of(i)) else { return false };
            let Some(st) = stand else { return true };
            let d = dot(unit(turn_by(&transpose(&r), a)), st);
            (if both { d.abs() } else { d }) >= cone
        })
        .collect();
    let mut kinds = vec![];
    for (kind, name) in [(shapes::ItemKind::Membrane, "membrane"), (shapes::ItemKind::Ball, "ball"), (shapes::ItemKind::Line, "line")] {
        // Hand-drawn surfaces are membranes to the user.
        if marked.kept().any(|i| i.kind == kind) || (kind == shapes::ItemKind::Membrane && marked.surfaces > 0) {
            kinds.push(name.to_string());
        }
    }
    Some(Filter { on, standing: stand.is_some(), kinds })
}

// ---- find again ----

/// What "find again" learns from and what it judges.
pub struct Teaching {
    /// Places the user keeps (as the review counts them without "find again"): learned as kept.
    pub kept: Vec<usize>,
    /// Places the user dropped by hand: learned as dropped.
    pub dropped: Vec<usize>,
    /// Places not kept that are close enough to what is kept to be asked about.
    pub below: Vec<usize>,
}

impl State {
    /// The places "find again" learns from and judges, from the user's own
    /// edits alone (an earlier "find again" is not looked at: it is redone).
    pub fn teaching(&self) -> Teaching {
        let l = self.lists();
        let (mut kept, mut dropped, mut rest) = (vec![], vec![], vec![]);
        for i in 0..self.total() {
            match self.state_with(&l, i, false, self.edits.on_shapes) {
                Shown::Kept | Shown::Added => kept.push(i),
                Shown::Removed => dropped.push(i),
                Shown::Below => rest.push(i),
                _ => {}
            }
        }
        // Not kept ones are asked about when they score as high as the weakest kept one
        // less the gap from it to the middle one (the places far below are plain noise).
        let mut scores: Vec<f32> = kept.iter().filter(|i| !l.added.contains(i)).map(|i| self.found.places[*i].score).collect();
        scores.sort_by(f32::total_cmp);
        let below = match (scores.first(), scores.get(scores.len() / 2)) {
            (Some(lo), Some(mid)) => {
                let floor = lo - (mid - lo);
                rest.into_iter().filter(|i| self.found.places[*i].score >= floor).collect()
            }
            _ => vec![],
        };
        Teaching { kept, dropped, below }
    }

    /// Whether there is enough to learn from.
    pub fn can_learn_again(&self) -> bool {
        let t = self.teaching();
        t.kept.len() >= 2 && !t.dropped.is_empty()
    }

    /// Takes the judgement: `belongs[i]` says whether place i is like what
    /// is kept (None: not asked; `taught::Bar`). A kept place the user did
    /// not click goes when it does not belong; one that was not kept comes
    /// in when it does. What the user clicked is never changed. Returns
    /// (out, in).
    pub fn judge(&mut self, belongs: &[Option<bool>]) -> (usize, usize) {
        let l = self.lists();
        let t = self.teaching();
        let says = |i: usize| belongs.get(i).copied().flatten();
        let out: Vec<usize> = t.kept.iter().copied().filter(|i| !l.added.contains(i) && says(*i) == Some(false)).collect();
        let inn: Vec<usize> = t.below.iter().copied().filter(|i| !l.removed.contains(i) && !l.added.contains(i) && says(*i) == Some(true)).collect();
        self.edits.taught_out = out;
        self.edits.taught_in = inn;
        self.edits.again = true;
        self.again_counts()
    }
}

/// The cubes at the places `idx` of source `src`, each turned back by the
/// orientation the search found, so that they are in the template's frame:
/// (index, cube). Cut as `run_places` cuts them, and as many times wider as
/// the search looked (`Found::wide`): what told the places apart is what
/// they are compared by. Places whose cube cannot be cut are left out.
pub(super) fn cubes_of(sources: &[sp::Source], found: &Found, src: usize, idx: &[usize]) -> Result<Vec<(usize, Vec<f32>)>> {
    let ids: Vec<usize> = idx.iter().copied().filter(|i| found.places[*i].source == src).collect();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let turns = grid(30.0);
    let (v, dims, _, bin) = sp::binned(&sources[src].path, found.voxel_a * found.wide.max(1.0))?;
    let on_grid = |x: f64| ((x - (bin as f64 - 1.0) / 2.0) / bin as f64).round().max(0.0) as usize;
    Ok(ids
        .into_iter()
        .filter_map(|i| {
            let p = &found.places[i];
            let cube = sp::cube(&v, dims, p.at().map(on_grid))?;
            Some((i, cc::rotate(&cube, BOX, &transpose(&found.turn_of(p, &turns)?))))
        })
        .collect())
}

/// Judges the places of `dir`'s `found.json` again by what the user kept and
/// dropped and saves the two lists; returns the review of `open`.
pub fn find_again(dir: &Path, open: &str, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    const NEED: &str = "先去掉几个不要的，再按这个";
    let s = load(dir).context("这里还没有找出的结果")?;
    if let Some(why) = s.stale() {
        bail!(why);
    }
    let plan = s.teaching();
    if plan.kept.len() < 2 || plan.dropped.is_empty() {
        bail!(NEED);
    }
    events(emit, b64, |progress, _live, note| {
        let sources: Vec<sp::Source> = s.inputs.iter().map(|i| resolve_tomogram(i, note).source()).collect();
        let n = sources.len();
        let radius = 0.6 * s.card.diameter_a / s.found.voxel_a;
        let mut t = taught::Taught::new(BOX, radius);
        // First every kept and dropped cube, one tomogram at a time; then every cube to judge.
        for src in 0..n {
            progress("重新判断：看你留下和去掉的", src, 2 * n);
            for (i, cube) in cubes_of(&sources, &s.found, src, &[plan.kept.as_slice(), plan.dropped.as_slice()].concat())? {
                t.learn(&cube, plan.kept.binary_search(&i).is_ok());
            }
        }
        if !t.ready() {
            bail!(NEED);
        }
        let mut likes: Vec<Option<taught::Likes>> = vec![None; s.total()];
        for src in 0..n {
            progress("重新判断：逐个比较", n + src, 2 * n);
            for (i, cube) in cubes_of(&sources, &s.found, src, &[plan.kept.as_slice(), plan.below.as_slice()].concat())? {
                let member = plan.kept.binary_search(&i).is_ok().then_some(true);
                likes[i] = Some(t.likes(&cube, member));
            }
        }
        // Only what is clear of the kept places' own scatter is changed.
        let bar = taught::Bar::of(&plan.kept.iter().filter_map(|i| likes[*i]).collect::<Vec<_>>());
        let belongs: Vec<Option<bool>> = (0..s.total()).map(|i| likes[i].map(|l| if plan.kept.binary_search(&i).is_ok() { !bar.goes(l) } else { bar.comes(l) })).collect();
        eprintln!(
            "sta: judged again by {} kept and {} dropped: {} of the kept go, {} of {} below come in",
            plan.kept.len(),
            plan.dropped.len(),
            plan.kept.iter().filter(|i| belongs[**i] == Some(false)).count(),
            plan.below.iter().filter(|i| belongs[**i] == Some(true)).count(),
            plan.below.len()
        );
        progress("重新判断：逐个比较", 2 * n, 2 * n);
        // Edits made meanwhile are kept: read again, change only the lists.
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let mut now = load(dir).context("这里还没有找出的结果")?;
        now.judge(&belongs);
        save(dir, &now)
    })?;
    let s = load(dir).context("这里还没有找出的结果")?;
    Ok(review_value(&s, dir, open))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(source: usize, x: usize, score: f32) -> Place {
        Place { source, c: [x, 10, 10], bin: 2, turn: 0, score, like: 0, size: 0 }
    }

    fn card() -> sp::Card {
        sp::Card { id: 3, diameter_a: 200.0, measured_a: 190.0, voxel_a: 16.0, members: 40, elongation: 1.1, blobs: 1, offset: 0.0, share: 0.3, path: "/nowhere/card_3.mrc".into(), compact: 12.0, likely: false }
    }

    /// 60 places over two tomograms, scores falling, `limit` between the 30th and 31st.
    fn state(limit: Option<f32>) -> State {
        let places: Vec<Place> = (0..60).map(|i| place(i % 2, 10 + 5 * i, 1.0 - i as f32 * 0.01)).collect();
        let found = Found::plain(places, limit, None, 16.0, "look", Some(32.0));
        let look = Look { path: "/t/a_look.mrc".into(), scale: 2.0, offset: [4.0, 6.0, 8.0] };
        State::new(found, "/nowhere/card_3.mrc".into(), 266.0, card(), vec!["/t/a.mrc".into(), "/t/b.mrc".into()], vec![Some(look), None], vec![10.0, 10.0], Route { kind: "card".into(), card: Some(3), points: None })
    }

    #[test]
    fn the_suggestion_is_the_places_above_the_limit() {
        let s = state(Some(0.705));
        // Scores 1.00 … 0.71 are above: 30 places.
        assert_eq!(s.edits.keep, 30);
        assert!(!s.unsure);
        assert_eq!(s.kept().len(), 30);
        let u = state(None);
        assert!(u.unsure);
        // Fewer places than UNSURE_KEEP: all of them.
        assert_eq!(u.edits.keep, 60);
    }

    /// Circled examples: the places come in groups (which example they are
    /// like, which size); a group unticked does not count, the examples
    /// themselves always do until the user drops one, and the average may
    /// be of a few things.
    #[test]
    fn circled_examples_come_in_groups_and_stay() {
        let mut s = state(Some(0.705));
        s.route = Route { kind: "circled".into(), card: None, points: Some(2) };
        s.found.sizes_a = vec![300.0, 600.0];
        for (i, p) in s.found.places.iter_mut().enumerate() {
            p.like = (i % 3 == 0) as u8;
            p.size = (i % 4 == 0) as u8;
        }
        // One example stands far below the limit.
        s.mine = vec![2, 50];
        assert_eq!(s.groups(), vec![[0, 0], [0, 1], [1, 0], [1, 1]]);
        assert_eq!(s.state_of(50), Shown::Kept);
        assert_eq!(s.kept().len(), 31);
        assert!(s.shown(0).contains(&50));
        // Places like example 1 at the small size: 3, 6, 9, 15, 18, 21, 27 of the first 30.
        s.set_group([1, 0], false);
        assert_eq!(s.state_of(3), Shown::GroupOff);
        assert_eq!(s.kept().len(), 24);
        assert!(s.toggle(3).is_err());
        let v = review_value(&s, Path::new("/nowhere"), "/t/a.mrc");
        let g = &v["circled"]["groups"][2];
        assert_eq!((g["like"].as_u64(), g["size"].as_u64(), g["on"].as_bool(), g["found"].as_u64(), g["kept"].as_u64()), (Some(1), Some(0), Some(false), Some(7), Some(0)), "{g}");
        assert_eq!(v["circled"]["groups"][3]["sizeA"], 600.0);
        assert_eq!(v["fewest"], sp::FEWEST_CIRCLED);
        // The radius of a place is that of its own size (raw voxels of 10 Å; the open file is a.mrc itself).
        let big = v["places"].as_array().unwrap().iter().find(|p| p["size"] == 1).unwrap();
        assert_eq!(big["r"], 30.0);
        // An example of an unticked group stays; the user can still drop it.
        s.mine.push(9);
        assert_eq!(s.state_of(9), Shown::Kept);
        s.toggle(9).unwrap();
        assert_eq!(s.state_of(9), Shown::Removed);
        s.set_group([1, 0], true);
        assert_eq!(s.state_of(3), Shown::Kept);
        apply(&mut s, "/t/a.mrc", &Edit { group: Some(GroupEdit { like: 0, size: 1, on: false }), ..Edit::default() }).unwrap();
        assert_eq!(s.edits.groups_off, vec![[0, 1]]);
        // Other routes have no groups.
        assert!(state(Some(0.705)).groups().is_empty());
        assert_eq!(review_value(&state(None), Path::new("/nowhere"), "/t/a.mrc")["circled"], Value::Null);
    }

    #[test]
    fn toggling_walks_through_the_four_states() {
        let mut s = state(Some(0.705));
        assert_eq!(s.state_of(4), Shown::Kept);
        s.toggle(4).unwrap();
        assert_eq!(s.state_of(4), Shown::Removed);
        s.toggle(4).unwrap();
        assert_eq!(s.state_of(4), Shown::Kept);
        assert_eq!(s.state_of(40), Shown::Below);
        s.toggle(40).unwrap();
        assert_eq!(s.state_of(40), Shown::Added);
        s.toggle(40).unwrap();
        assert_eq!(s.state_of(40), Shown::Below);
        assert!(s.toggle(60).is_err());
    }

    #[test]
    fn the_kept_list_is_the_slider_minus_removed_plus_added() {
        let mut s = state(Some(0.705));
        s.toggle(0).unwrap();
        s.toggle(7).unwrap();
        s.toggle(45).unwrap();
        s.toggle(31).unwrap();
        let k = s.kept();
        assert_eq!(k.len(), 30 - 2 + 2);
        assert!(!k.contains(&0) && !k.contains(&7) && k.contains(&45) && k.contains(&31));
        assert!(k.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(s.changes(), (2, 2));
        // The slider past an added place makes it an ordinary kept one; a removed one beyond the slider is forgotten but remembered.
        s.set_keep(40);
        assert_eq!(s.state_of(31), Shown::Kept);
        assert_eq!(s.kept().len(), 40 - 2 + 1);
        assert_eq!(s.changes(), (2, 1));
        s.set_keep(5);
        assert_eq!(s.changes(), (1, 2));
        assert_eq!(s.kept().len(), 4 + 2);
    }

    #[test]
    fn the_slider_stays_within_one_and_the_total() {
        let mut s = state(Some(0.705));
        s.set_keep(0);
        assert_eq!(s.edits.keep, 1);
        s.set_keep(10_000);
        assert_eq!(s.edits.keep, 60);
        s.reset();
        assert_eq!(s.edits.keep, 30);
        s.toggle(1).unwrap();
        s.toggle(50).unwrap();
        s.set_keep(10);
        s.reset();
        assert_eq!((s.edits.keep, s.edits.removed.len(), s.edits.added.len()), (30, 0, 0));
    }

    #[test]
    fn a_click_where_nothing_was_found_makes_a_place_of_circled_examples() {
        let mut s = state(Some(0.705));
        s.found.frames = vec![crate::sta::IDENTITY];
        let total = s.total();
        // Tomogram b (source 1): bin 2, so the click lands on the grid of the cubes.
        let i = s.add_at("/t/b.mrc", [401.0, 301.0, 201.0]).unwrap();
        assert_eq!((i, s.total()), (total, total + 1));
        let p = s.found.places[i];
        assert_eq!((p.source, p.c, p.bin), (1, [200, 150, 100], 2));
        assert_eq!(s.state_of(i), Shown::Added);
        assert!(s.kept().contains(&i) && s.shown(1).contains(&i));
        // Its orientation is the one nearest to upright.
        let turns = grid(30.0);
        let off = |t: &crate::spa::geom::Mat| distance(t, &crate::sta::IDENTITY);
        assert!(turns.iter().all(|t| off(t) >= off(&turns[p.turn])));
        // Another click there: one is there already.
        assert_eq!(s.add_at("/t/b.mrc", [403.0, 301.0, 201.0]).unwrap_err().to_string(), "这里已经有一个了");
        // Clicked on (`toggle`), it is not kept any more, and a click there brings it back, no second one.
        s.toggle(i).unwrap();
        assert!(!s.kept().contains(&i));
        assert_eq!((s.add_at("/t/b.mrc", [403.0, 301.0, 201.0]).unwrap(), s.total()), (i, total + 1));
        // The other routes find the orientation near where the search left it: no place is made.
        let mut s = state(Some(0.705));
        assert!(s.add_at("/t/b.mrc", [401.0, 301.0, 201.0]).is_err());
    }

    #[test]
    fn a_click_adds_the_nearest_place_within_a_radius() {
        let mut s = state(Some(0.705));
        // Source 1 (tomogram b, voxel 10 Å): radius 0.5 × 266 / 10 = 13.3 voxels. Place 41 (source 1) is below.
        let at = s.found.places[41].at();
        let i = s.add_at("/t/b.mrc", [at[0] + 5.0, at[1], at[2]]).unwrap();
        assert_eq!(i, 41);
        assert_eq!(s.state_of(41), Shown::Added);
        // It is kept now: another click there finds nothing else near.
        let err = s.add_at("/t/b.mrc", [at[0] + 5.0, at[1], at[2]]).unwrap_err().to_string();
        assert_eq!(err, "这里没有找到像例子的东西");
        // Far from everything.
        assert!(s.add_at("/t/b.mrc", [5000.0, 5000.0, 5000.0]).is_err());
        // A removed place comes back by a click.
        s.toggle(3).unwrap();
        let at = s.found.places[3].at();
        assert_eq!(s.add_at("/t/b.mrc", at).unwrap(), 3);
        assert_eq!(s.state_of(3), Shown::Kept);
        // A tomogram that was not searched.
        assert!(s.add_at("/t/other.mrc", at).is_err());
    }

    #[test]
    fn a_click_on_the_look_file_is_carried_to_the_raw_voxels() {
        let mut s = state(Some(0.705));
        // Place 40 is of source 0 (tomogram a, with a look file of scale 2, radius 13.3 raw voxels = 6.65 look voxels).
        let raw = s.found.places[40].at();
        let f = Frame { source: 0, look: s.looks[0].as_ref() };
        let on_look = State::to_open(&f, raw);
        assert!((State::to_raw(&f, on_look)[0] - raw[0]).abs() < 1e-9);
        assert_eq!(s.add_at("/t/a_look.mrc", [on_look[0] + 3.0, on_look[1], on_look[2]]).unwrap(), 40);
        // The same click, 8 look voxels away, is over one radius (6.65).
        assert!(s.add_at("/t/a_look.mrc", [on_look[0], on_look[1] + 8.0, on_look[2]]).is_err());
    }

    #[test]
    fn the_review_shows_the_open_tomogram_in_its_own_voxels() {
        let mut s = state(Some(0.705));
        s.toggle(2).unwrap();
        let dir = std::env::temp_dir();
        let raw = review_value(&s, &dir, "/t/a.mrc");
        let look = review_value(&s, &dir, "/t/a_look.mrc");
        assert_eq!(raw["total"], 60);
        assert_eq!(raw["keep"], 30);
        assert_eq!(raw["kept"], 29);
        assert_eq!(raw["removed"], 1);
        assert_eq!(raw["perTomogram"][0]["kept"], 14);
        assert_eq!(raw["perTomogram"][1]["kept"], 15);
        assert_eq!(raw["etaSeconds"], (60.0 + 0.15 * 29.0_f64).round());
        // Source 0 has the even places.
        let rp = raw["places"].as_array().unwrap();
        assert_eq!(rp.len(), 30);
        assert_eq!(rp[1]["i"], 2);
        assert_eq!(rp[1]["state"], "removed");
        assert_eq!(rp[0]["state"], "kept");
        assert_eq!(rp.last().unwrap()["state"], "below");
        let lp = look["places"].as_array().unwrap();
        let (x_raw, x_look) = (rp[0]["pos"][0].as_f64().unwrap(), lp[0]["pos"][0].as_f64().unwrap());
        // (raw − offset) / scale, with offset 4 and scale 2; the radius halves (13.3 → 6.65).
        assert!((x_look - (x_raw - 4.0) / 2.0).abs() < 0.11, "{x_raw} {x_look}");
        assert!((raw["radius"].as_f64().unwrap() / look["radius"].as_f64().unwrap() - 2.0).abs() < 0.05);
        assert_eq!(raw["open"]["look"], false);
        assert_eq!(look["open"]["look"], true);
        // A file that was not searched shows no places.
        assert!(review_value(&s, &dir, "/t/zzz.mrc")["places"].as_array().unwrap().is_empty());
        assert_eq!(raw["scores"].as_array().unwrap().len(), 60);
    }

    #[test]
    fn only_the_best_keep_plus_a_margin_are_shown_per_tomogram() {
        let places: Vec<Place> = (0..2000).map(|i| place(0, i, 2.0 - i as f32 * 0.001)).collect();
        let found = Found::plain(places, Some(1.8995), None, 16.0, "raw", None);
        let s = State::new(found, "x".into(), 266.0, card(), vec!["/t/a.mrc".into()], vec![None], vec![10.0], Route { kind: "card".into(), card: Some(3), points: None });
        assert_eq!(s.edits.keep, 101);
        assert_eq!(s.shown(0).len(), 101 + MARGIN);
        let r = review_value(&s, &std::env::temp_dir(), "/t/a.mrc");
        assert_eq!(r["places"].as_array().unwrap().len(), 101 + MARGIN);
        assert_eq!(r["scores"].as_array().unwrap().len(), 2000);
    }

    #[test]
    fn too_few_kept_is_an_error_with_the_count() {
        let mut s = state(Some(0.705));
        s.stamps = vec![]; // no stamps to compare: replaced below
        s.stamps = input_stamps(s.inputs.iter().map(String::as_str));
        s.set_keep(19);
        let err = kept_places(&s).unwrap_err().to_string();
        assert_eq!(err, "保留的只有 19 个，至少要 20 个");
        s.set_keep(30);
        assert_eq!(kept_places(&s).unwrap().len(), 30);
        s.toggle(0).unwrap();
        for i in 1..11 {
            s.toggle(i).unwrap();
        }
        assert_eq!(kept_places(&s).unwrap_err().to_string(), "保留的只有 19 个，至少要 20 个");
        // Adding brings it back to 20.
        s.toggle(55).unwrap();
        assert_eq!(kept_places(&s).unwrap().len(), 20);
    }

    #[test]
    fn changed_tomograms_say_to_find_again() {
        let dir = std::env::temp_dir().join(format!("tmv_review_stale_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tomo = dir.join("t.mrc");
        std::fs::write(&tomo, b"abc").unwrap();
        let mut s = state(Some(0.705));
        s.inputs = vec![tomo.to_string_lossy().into_owned(), "/t/b.mrc".into()];
        s.stamps = input_stamps(s.inputs.iter().map(String::as_str));
        assert!(s.stale().is_none());
        assert!(kept_places(&s).is_ok());
        std::fs::write(&tomo, b"abcdef").unwrap();
        assert_eq!(s.stale().as_deref(), Some("断层图变了，请重新找"));
        assert_eq!(kept_places(&s).unwrap_err().to_string(), "断层图变了，请重新找");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_state_survives_a_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("tmv_review_file_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = state(Some(0.705));
        s.toggle(5).unwrap();
        s.toggle(44).unwrap();
        save(&dir, &s).unwrap();
        let back = load(&dir).unwrap();
        assert_eq!(back.edits, s.edits);
        assert_eq!(back.route, s.route);
        assert_eq!(back.found.places, s.found.places);
        assert_eq!(back.found.limit, s.found.limit);
        assert_eq!(back.inputs, s.inputs);
        assert_eq!(back.stamps, s.stamps);
        assert_eq!(back.looks, s.looks);
        assert_eq!(back.kept(), s.kept());
        assert!(load(&dir.join("nothing")).is_none());
        // Edits through the file, as the commands do.
        let r = set_review(&dir, "/t/b.mrc", &Edit { keep: Some(12), ..Edit::default() }).unwrap();
        assert_eq!(r["keep"], 12);
        assert_eq!(load(&dir).unwrap().edits.keep, 12);
        let r = set_review(&dir, "/t/b.mrc", &Edit { reset: true, ..Edit::default() }).unwrap();
        assert_eq!((r["keep"].as_u64(), r["removed"].as_u64(), r["added"].as_u64()), (Some(30), Some(0), Some(0)));
        assert!(set_review(&dir.join("nothing"), "/t/b.mrc", &Edit::default()).is_err());
        // The same JSON as the command receives it.
        let e: Edit = serde_json::from_str(r#"{"toggle":4,"addAt":[1,2,3]}"#).unwrap();
        assert_eq!((e.toggle, e.add_at, e.keep, e.reset), (Some(4), Some([1.0, 2.0, 3.0]), None, false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_folder_of_each_route() {
        assert_eq!(dir_of("/o", "card").unwrap(), PathBuf::from("/o"));
        assert_eq!(dir_of("/o", "marked").unwrap(), PathBuf::from("/o/marked"));
        assert!(dir_of("/o", "drawn").is_err());
    }

    // ---- only on the shapes ----

    /// A shapes review on disk for output folder `root`: one vesicle of radius 60 about (200, 200, 200) that
    /// `a` is searched for (10 Å voxels, its seeds 8 voxels apart).
    fn marked_sphere(root: &Path, a: &str) -> Vec<Seed> {
        let seeds = crate::sta::shape::ellipsoid([200.0; 3], [60.0; 3], 8.0);
        let item = shapes::Item {
            id: 0,
            kind: shapes::ItemKind::Ball,
            source: 0,
            on_look: false,
            example: true,
            side_sure: true,
            centre: [200.0; 3],
            radius: 60.0,
            voxels: 0,
            n: [0; 3],
            seeds: seeds.len(),
            score: 1.0,
            length: 0.0,
            px: 0.0,
            weak: false,
        };
        let state = shapes::State {
            items: vec![item],
            edits: shapes::Edits::default(),
            inputs: vec![a.to_string()],
            stamps: input_stamps([a].into_iter()),
            looks: vec![None],
            size_a: 100.0,
            both: false,
            surfaces: 0,
            drawn: vec![],
        };
        shapes::save_new(&shapes::dir_of(&root.to_string_lossy()), &state, &[shapes::Side { seeds: seeds.clone(), mask: vec![] }]).unwrap();
        seeds
    }

    /// 60 places of one tomogram, best first: 40 on the sphere (every 4th one far from it: 20 far off).
    fn on_the_sphere(a: &str, seeds: &[Seed]) -> State {
        let mut places = vec![];
        for i in 0..60 {
            let c = if i % 3 == 2 {
                [20 + 3 * i, 30, 40]
            } else {
                let p = seeds[(i * seeds.len()) / 60].pos;
                [p[0].round() as usize, p[1].round() as usize, p[2].round() as usize]
            };
            places.push(Place { source: 0, c, bin: 1, turn: 0, score: 1.0 - i as f32 * 0.01, like: 0, size: 0 });
        }
        let found = Found::plain(places, Some(0.0), None, 10.0, "raw", None);
        State::new(found, "x".into(), 266.0, card(), vec![a.to_string()], vec![None], vec![10.0], Route { kind: "card".into(), card: Some(3), points: None })
    }

    #[test]
    fn only_the_places_on_the_marked_shapes_count_and_switching_off_restores_them() {
        let root = std::env::temp_dir().join(format!("tmv_review_shapes_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.mrc").to_string_lossy().into_owned();
        std::fs::write(&a, b"tomogram").unwrap();
        // No shapes marked: no filter, everything counts.
        let mut s = on_the_sphere(&a, &crate::sta::shape::ellipsoid([200.0; 3], [60.0; 3], 8.0));
        assert!(shapes_filter(&root, &s).is_none());
        assert_eq!(review_value(&s, &root, &a)["onShapes"]["available"], false);
        assert_eq!(s.kept().len(), 60);
        let seeds = marked_sphere(&root, &a);
        s = on_the_sphere(&a, &seeds);
        s.filter = shapes_filter(&root, &s);
        let f = s.filter.as_ref().expect("the sphere is marked");
        assert_eq!(f.kinds, vec!["ball".to_string()]);
        assert_eq!(f.on.iter().filter(|x| **x).count(), 40);
        let r = review_value(&s, &root, &a);
        assert_eq!((r["kept"].as_u64(), r["onShapes"]["available"].as_bool(), r["onShapes"]["on"].as_bool(), r["onShapes"]["near"].as_u64()), (Some(40), Some(true), Some(true), Some(40)));
        // Places off the sphere are "off" and not counted; the others keep their states.
        assert_eq!(s.state_of(2), Shown::Off);
        assert_eq!(s.state_of(0), Shown::Kept);
        assert_eq!(r["perTomogram"][0]["kept"], 40);
        let states: Vec<&str> = r["places"].as_array().unwrap().iter().map(|p| p["state"].as_str().unwrap()).collect();
        assert_eq!(states.iter().filter(|x| **x == "off").count(), 20);
        // An off place cannot be clicked; the others still can.
        assert!(s.toggle(2).is_err());
        s.toggle(0).unwrap();
        assert_eq!(s.kept().len(), 39);
        assert_eq!(kept_places(&s).unwrap().len(), 39);
        // Switched off: all the others are back, the click stays.
        let mut e = Edit::default();
        e.on_shapes = Some(false);
        apply(&mut s, &a, &e).unwrap();
        assert_eq!(s.kept().len(), 59);
        let r = review_value(&s, &root, &a);
        assert_eq!((r["kept"].as_u64(), r["onShapes"]["on"].as_bool(), r["onShapes"]["near"].as_u64(), r["onShapes"]["available"].as_bool()), (Some(59), Some(false), Some(39), Some(true)));
        assert_eq!(s.state_of(2), Shown::Kept);
        // It is saved with the other edits, and the filter with the places.
        save(&root, &s).unwrap();
        let back = load(&root).unwrap();
        assert!(!back.edits.on_shapes);
        assert_eq!(back.filter, s.filter);
        let r = set_review(&root, &a, &Edit { on_shapes: Some(true), ..Edit::default() }).unwrap();
        assert_eq!(r["kept"], 39);
        // Back to the program's suggestion: the filter is on again.
        let r = set_review(&root, &a, &Edit { on_shapes: Some(false), ..Edit::default() }).unwrap();
        assert_eq!(r["kept"], 59);
        let r = set_review(&root, &a, &Edit { reset: true, ..Edit::default() }).unwrap();
        assert_eq!((r["kept"].as_u64(), r["onShapes"]["on"].as_bool()), (Some(40), Some(true)));
        // Shapes that no longer fit the tomogram are not used.
        std::fs::write(&a, b"another tomogram").unwrap();
        assert!(shapes_filter(&root, &s).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_without_the_new_fields_reads_with_the_filter_on_and_nothing_taught() {
        let e: Edits = serde_json::from_str(r#"{"keep":3,"removed":[1]}"#).unwrap();
        assert_eq!((e.on_shapes, e.taught_out.len(), e.taught_in.len(), e.again), (true, 0, 0, false));
    }

    // ---- find again ----

    #[test]
    fn find_again_judges_what_the_user_did_not_touch_and_hand_edits_win() {
        let mut s = state(Some(0.705));
        s.toggle(3).unwrap();
        // One dropped, 29 kept.
        assert!(s.can_learn_again());
        s.toggle(40).unwrap(); // hand-added
        let t = s.teaching();
        assert_eq!((t.kept.len(), t.dropped.clone()), (30, vec![3]));
        // Below places are asked about only when close to the kept ones' scores (0.71 down to 0.57).
        assert!(t.below.contains(&31) && t.below.contains(&42) && !t.below.contains(&46));
        let mut likes: Vec<Option<bool>> = vec![None; 60];
        for &i in &t.kept {
            likes[i] = Some(true);
        }
        likes[10] = Some(false);
        likes[11] = Some(false);
        likes[5] = Some(false);
        likes[40] = Some(false);
        likes[31] = Some(true);
        likes[32] = Some(false);
        likes[33] = Some(true);
        s.toggle(33).unwrap(); // the user added it by hand: not judged
        s.toggle(5).unwrap();
        s.toggle(5).unwrap();
        let (out, inn) = s.judge(&likes);
        assert_eq!((out, inn), (3, 1));
        assert_eq!(s.edits.taught_out, vec![5, 10, 11]);
        assert_eq!(s.edits.taught_in, vec![31]);
        assert!(s.edits.again);
        // The hand-added place stays, though it was unlike; the hand-dropped one stays dropped.
        assert_eq!(s.state_of(40), Shown::Added);
        assert_eq!(s.state_of(3), Shown::Removed);
        assert_eq!(s.state_of(10), Shown::TaughtOut);
        assert_eq!(s.state_of(31), Shown::TaughtIn);
        assert_eq!(s.state_of(32), Shown::Below);
        // 30 first, minus 3 (hand) and 3 taught out, plus 40 and 33 (hand) and 31 (taught).
        let kept = s.kept();
        assert_eq!(kept.len(), 30 - 1 - 3 + 2 + 1);
        assert!(kept.contains(&31) && kept.contains(&40) && !kept.contains(&10) && !kept.contains(&3));
        assert_eq!(s.again_counts(), (3, 1));
        // A click on a place taken out brings it back, a click on one brought in sends it away; clicking again undoes the click.
        s.toggle(10).unwrap();
        assert_eq!(s.state_of(10), Shown::Added);
        assert_eq!(s.again_counts(), (2, 1));
        s.toggle(31).unwrap();
        assert_eq!(s.state_of(31), Shown::Below);
        s.toggle(10).unwrap();
        assert_eq!(s.state_of(10), Shown::TaughtOut);
        // The slider takes them along: past 31 it would be kept, but the click that sent it away stays.
        s.set_keep(40);
        assert_eq!(s.state_of(31), Shown::Removed);
        // Another run starts from the hand edits only, as it was not run before.
        let again = s.teaching();
        assert!(again.kept.contains(&10) && !again.kept.contains(&3));
        // Taking it back leaves the clicks.
        s.undo_again();
        assert!(s.edits.taught_out.is_empty() && s.edits.taught_in.is_empty() && !s.edits.again);
        assert_eq!(s.state_of(10), Shown::Kept);
        assert_eq!(s.state_of(3), Shown::Removed);
        assert_eq!(s.again_counts(), (0, 0));
        // Reset takes everything back.
        s.judge(&likes);
        s.reset();
        assert_eq!(s.edits, Edits::new(30));
    }

    #[test]
    fn find_again_needs_two_kept_and_one_dropped_by_hand() {
        let mut s = state(Some(0.705));
        assert!(!s.can_learn_again());
        s.set_keep(2);
        s.toggle(0).unwrap();
        assert!(!s.can_learn_again());
        s.set_keep(3);
        assert!(s.can_learn_again());
        // The dropped one beyond the slider does not count.
        s.set_keep(1);
        assert!(!s.can_learn_again());
    }

    #[test]
    fn find_again_through_the_review_and_the_file() {
        let dir = std::env::temp_dir().join(format!("tmv_review_again_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = state(Some(0.705));
        s.toggle(3).unwrap();
        s.judge(&{
            let mut v = vec![None; 60];
            v[7] = Some(false);
            v[8] = Some(true);
            v[31] = Some(true);
            v
        });
        save(&dir, &s).unwrap();
        let r = review(&dir, "/t/b.mrc").unwrap().unwrap();
        assert_eq!((r["again"]["out"].as_u64(), r["again"]["in"].as_u64(), r["again"]["ran"].as_bool()), (Some(1), Some(1), Some(true)));
        assert_eq!(r["kept"], 29);
        assert_eq!(r["places"].as_array().unwrap().iter().filter(|p| p["state"] == "taughtOut").count(), 1);
        let r = set_review(&dir, "/t/b.mrc", &Edit { undo_again: true, ..Edit::default() }).unwrap();
        assert_eq!((r["again"]["ran"].as_bool(), r["kept"].as_u64(), r["removed"].as_u64()), (Some(false), Some(29), Some(1)));
        let r = set_review(&dir, "/t/b.mrc", &Edit { reset: true, ..Edit::default() }).unwrap();
        assert_eq!(r["kept"], 30);
        // The command's JSON.
        let e: Edit = serde_json::from_str(r#"{"onShapes":false,"undoAgain":true}"#).unwrap();
        assert_eq!((e.on_shapes, e.undo_again), (Some(false), true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_own_route_has_its_folder_and_its_average_is_a_result() {
        assert_eq!(dir_of("/o", "own").unwrap(), PathBuf::from("/o/own"));
        let root = std::env::temp_dir().join(format!("tmv_review_own_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let b64 = |b: &[u8]| format!("{}", b.len());
        assert!(result_pick(&root, None, &b64).unwrap().is_none());
        assert!(own_saved(&root.to_string_lossy(), &b64).is_none());
    }
}
