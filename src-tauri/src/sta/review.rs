//! "Find them all, show them, then average" for particles (the workspace's
//! card route and clicked-particles route): the template search runs alone
//! (`project::find_template`), what it found is saved beside the cards as
//! `found.json` together with the user's edits, and only when the user says
//! so are the kept places averaged (`project::run_places`).
//!
//! The state is plain data (`State`), so the edits, the places shown on a
//! tomogram and the list that is averaged are all computed without a
//! tomogram and are unit-tested that way. The functions that search and
//! average (`find_card`, `find_points`, `average_found`) wrap them with the
//! same events as `auto::run_inputs` (`sta-progress`, `sta-live`, `sta-note`).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::auto::{self, card_json, input_stamps, live_png, note_json, raw_inputs, resolve_tomogram, result_value, write_used, Device, Emit, InputStamp, Note, MARKED};
use super::marks::{self, Mark};
use super::project::{self as sp, Found, Place};
use super::refine::{Live, Progress};
use crate::tomo::dataset::Look;

/// The file in the output folder of a route that holds the search and the edits.
pub const FILE: &str = "found.json";

/// The time an average takes: a minute of search and set-up, then, measured
/// on an M1 Max, about 0.15 s per particle after the search.
pub const ETA_BASE_S: f64 = 60.0;
pub const ETA_PER_PARTICLE_S: f64 = 0.15;

/// Fewest particles that are averaged.
pub const MIN_KEPT: usize = 20;
/// How many places beyond the kept ones are shown on a tomogram, to add from.
pub const MARGIN: usize = 300;
/// Most scores handed to the score curve.
pub const MOST_SCORES: usize = 3000;
/// How many places are kept at first when the scores do not fall into two
/// groups (the user decides with the slider).
pub const UNSURE_KEEP: usize = 300;
/// Most particles `project::run_places` takes (it cuts the rest off).
pub const MOST_KEPT: usize = 3000;

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
}

/// How the template was made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    /// "card" (with `card`, its id) or "points" (with `points`, the clicks).
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
}

/// What a place is, for the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Kept,
    Below,
    Removed,
    Added,
}

impl Shown {
    pub fn name(self) -> &'static str {
        match self {
            Shown::Kept => "kept",
            Shown::Below => "below",
            Shown::Removed => "removed",
            Shown::Added => "added",
        }
    }
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
        let mut s = State { found, template, diameter_a, card, inputs, stamps, looks, pixels, edits: Edits { keep: 0, removed: vec![], added: vec![] }, route, unsure };
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

    pub fn state_of(&self, i: usize) -> Shown {
        if i < self.edits.keep {
            if self.edits.removed.contains(&i) { Shown::Removed } else { Shown::Kept }
        } else if self.edits.added.contains(&i) {
            Shown::Added
        } else {
            Shown::Below
        }
    }

    /// The places that are averaged: the first `keep` but the removed, and
    /// the added, as indices into `found.places` (best first).
    pub fn kept(&self) -> Vec<usize> {
        let keep = self.edits.keep.min(self.total());
        let mut out: Vec<usize> = (0..keep).filter(|i| !self.edits.removed.contains(i)).collect();
        let mut added: Vec<usize> = self.edits.added.iter().copied().filter(|i| *i >= keep && *i < self.total()).collect();
        added.sort_unstable();
        added.dedup();
        out.extend(added);
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

    /// Where `open` (a searched tomogram or its look companion) is among the sources.
    pub fn frame(&self, open: &str) -> Option<Frame<'_>> {
        if let Some(source) = self.inputs.iter().position(|p| same(p, open)) {
            return Some(Frame { source, look: None });
        }
        self.looks.iter().enumerate().find_map(|(source, l)| l.as_ref().filter(|l| same(&l.path, open)).map(|look| Frame { source, look: Some(look) }))
    }

    /// The places of source `s` shown on its tomogram: the best `keep + MARGIN` of them.
    pub fn shown(&self, s: usize) -> Vec<usize> {
        let most = self.edits.keep + MARGIN;
        (0..self.total()).filter(|i| self.found.places[*i].source == s).take(most).collect()
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
        self.edits = Edits { keep: self.suggest(), removed: vec![], added: vec![] };
    }

    /// Clicking a place: kept → removed, removed → kept again, below → added,
    /// added → below again.
    pub fn toggle(&mut self, i: usize) -> Result<()> {
        if i >= self.total() {
            bail!("没有第 {i} 个");
        }
        let list = match self.state_of(i) {
            Shown::Kept | Shown::Removed => &mut self.edits.removed,
            Shown::Below | Shown::Added => &mut self.edits.added,
        };
        match list.iter().position(|x| *x == i) {
            Some(at) => {
                list.remove(at);
            }
            None => list.push(i),
        }
        Ok(())
    }

    /// A click where no circle is: the nearest place of that tomogram (shown,
    /// not kept) within one radius is brought in. `pos` is in the open file's voxels.
    pub fn add_at(&mut self, open: &str, pos: [f64; 3]) -> Result<usize> {
        const NONE: &str = "这里没有找到像例子的东西";
        let frame = self.frame(open).context("这张断层图不在找出的断层图里")?;
        let r = self.radius_raw(frame.source).context(NONE)?;
        let at = Self::to_raw(&frame, pos);
        let best = self
            .shown(frame.source)
            .into_iter()
            .filter(|i| matches!(self.state_of(*i), Shown::Below | Shown::Removed))
            .map(|i| {
                let c = self.found.places[i].at();
                (i, (0..3).map(|k| (c[k] - at[k]).powi(2)).sum::<f64>().sqrt())
            })
            .filter(|(_, d)| *d <= r)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((i, _)) = best else { bail!(NONE) };
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
            "points" => dir.join("card.mrc"),
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
///    places, radius, open, route, card, stale, foundMs, averageMs }`.
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
    let frame = s.frame(open);
    let (places, radius, opened): (Vec<Value>, Value, Value) = match &frame {
        Some(f) => {
            let list = s
                .shown(f.source)
                .into_iter()
                .map(|i| {
                    let p = State::to_open(f, s.found.places[i].at());
                    json!({ "i": i, "pos": [r1(p[0]), r1(p[1]), r1(p[2])], "score": (s.found.places[i].score * 1e4).round() / 1e4, "state": s.state_of(i).name() })
                })
                .collect();
            (list, s.radius(f).map_or(Value::Null, |r| json!(r1(r))), json!({ "source": f.source, "look": f.look.is_some() }))
        }
        None => (vec![], Value::Null, Value::Null),
    };
    json!({
        "total": s.total(), "keep": s.edits.keep, "kept": kept.len(), "sure": s.suggest(), "unsure": s.unsure,
        "limit": s.found.limit, "removed": removed, "added": added, "perTomogram": per,
        "scores": scores_of(&s.found), "etaSeconds": State::eta(kept.len()).round(),
        "places": places, "radius": radius, "open": opened, "route": s.route, "card": card_json(&s.card),
        "stale": s.stale(), "searched": s.found.searched, "out": dir,
        "foundMs": millis(&path_of(dir)), "averageMs": millis(&dir.join("average.mrc")),
    })
}

/// The review of `dir` for the open tomogram; None when nothing was found there.
pub fn review(dir: &Path, open: &str) -> Result<Option<Value>> {
    Ok(load(dir).map(|s| review_value(&s, dir, open)))
}

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
}

/// Applies `edit` to the saved state of `dir`, saves it and returns the review of `open`.
pub fn set_review(dir: &Path, open: &str, edit: &Edit) -> Result<Value> {
    // Edits read the file, change it and write it back: one at a time.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
    if kept.len() < MIN_KEPT {
        bail!("保留的只有 {} 个，至少要 {MIN_KEPT} 个", kept.len());
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
fn search_and_save(dir: &Path, card: sp::Card, inputs: Vec<String>, sources: &[sp::Source], route: Route, device: Device, progress: Progress) -> Result<State> {
    let (dev, _) = device()?;
    let (cube, voxel) = sp::read_card(&card.path)?;
    let diameter = sp::template_diameter(&card);
    let found = crate::ctf_gpu::guarded(|| sp::find_template(dev, sources, &cube, voxel, diameter, false, progress))??;
    if found.places.is_empty() {
        bail!("没有找到像例子的东西");
    }
    let looks = sources.iter().map(|s| s.look.clone()).collect();
    let pixels = sources.iter().map(|s| sp::pixel_of(&s.path).unwrap_or(0.0)).collect();
    let state = State::new(found, card.path.clone(), diameter, card, inputs, looks, pixels, route);
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
        let state = search_and_save(&out, card, inputs.clone(), &sources, route, device, progress)?;
        Ok(review_value(&state, &out, &inputs[0]))
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
        note(Note::Marked { points: card.members, size_a: card.diameter_a });
        let route = Route { kind: "points".into(), card: None, points: Some(points.len()) };
        let state = search_and_save(&dir, card, inputs.clone(), &sources, route, device, progress)?;
        Ok(review_value(&state, &dir, &inputs[0]))
    })
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
        let counts = json!({ "found": s.total(), "kept": places.len(), "removed": removed, "added": added });
        let mut used = match s.route.kind.as_str() {
            "points" => json!({ "kind": "points", "points": s.card.members, "sizeA": s.card.diameter_a.round() }),
            _ => json!({ "kind": "auto", "card": s.card.id, "auto": false, "sizeA": s.card.diameter_a.round() }),
        };
        for (k, v) in counts.as_object().into_iter().flatten() {
            used[k] = v.clone();
        }
        write_used(dir, &used);
        let result = result_value(dir, b64)?;
        Ok(if s.route.kind == "points" {
            json!({
                "needsChoice": false, "report": serde_json::to_value(&report)?, "result": result,
                "cards": [], "chosen": Value::Null, "auto": false, "out": dir, "trials": [], "drawn": true,
                "points": s.route.points, "sizeA": s.card.measured_a,
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

/// Which folder `which` ("card", "marked") names under the output folder `out`.
pub fn dir_of(out: &str, which: &str) -> Result<PathBuf> {
    match which {
        "card" => Ok(PathBuf::from(out)),
        "marked" => Ok(Path::new(out).join(MARKED)),
        _ => bail!("which 应为 card 或 marked"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(source: usize, x: usize, score: f32) -> Place {
        Place { source, c: [x, 10, 10], bin: 2, turn: 0, score }
    }

    fn card() -> sp::Card {
        sp::Card { id: 3, diameter_a: 200.0, measured_a: 190.0, voxel_a: 16.0, members: 40, elongation: 1.1, blobs: 1, offset: 0.0, share: 0.3, path: "/nowhere/card_3.mrc".into(), compact: 12.0, likely: false }
    }

    /// 60 places over two tomograms, scores falling, `limit` between the 30th and 31st.
    fn state(limit: Option<f32>) -> State {
        let places: Vec<Place> = (0..60).map(|i| place(i % 2, 10 + 5 * i, 1.0 - i as f32 * 0.01)).collect();
        let found = Found { places, limit, voxel_a: 16.0, searched: "look".into(), search_voxel_a: Some(32.0) };
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
        let found = Found { places, limit: Some(1.8995), voxel_a: 16.0, searched: "raw".into(), search_voxel_a: None };
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
}
