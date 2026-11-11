//! The whole of "subtomogram averaging with nothing to set", shared by
//! `tmv sta` and the Pro workspace: the tomograms of the inputs → class-average
//! cards (kept, reused while the inputs are the same) → every card tried as a
//! template (kept in `cards/trials.json`) → the card to take → the average.
//! That is `tmv sta`, which runs unattended. In the workspace the program
//! does not choose: the run stops at the cards (`find`) and goes on with the
//! card the user takes, or with what the user marks (`marks`).
//!
//! What the caller used to read off the screen comes as data (`Note`), so the
//! command line writes English lines from it and the workspace Chinese ones.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::project as sp;
use super::refine::{Live, Progress};
use crate::tomo::dataset::{Dataset, Look};

/// What one input is after its tomogram exists.
pub struct Resolved {
    pub input: String,
    pub tomogram: String,
    pub tilts: [f64; 2],
    pub tilts_from: &'static str,
    /// The clear companion `tmv tomo --for sta` made (blobs and the template
    /// are searched on it; the averages are of `tomogram`).
    pub look: Option<Look>,
    /// The dataset of the tilt series, when the tomogram's place in the
    /// series is on record: the final average then comes from the tilt images.
    pub series: Option<String>,
}

impl Resolved {
    pub fn source(&self) -> sp::Source {
        sp::Source { path: self.tomogram.clone(), tilts: self.tilts, look: self.look.clone(), series: self.series.clone() }
    }
}

/// Something worth telling about while a run goes on.
#[derive(Clone, Debug)]
pub enum Note {
    /// No `*.tomo.json` lists the tomogram: tilts -60…60 are assumed.
    NoDataset { tomogram: String },
    /// The look tomogram of `tomogram` is gone: it is used alone.
    LookMissing { look: String, tomogram: String },
    /// Made by an older version: no average from the tilt images.
    OlderVersion { tomogram: String },
    /// Saved cards cannot be used for these inputs.
    CardsStale { why: String },
    CardsReused { dir: String },
    /// The cards were made (their count).
    CardsMade { count: usize },
    TrialsReused { dir: String },
    /// A card's trial; `reused` when it was read from `trials.json`.
    Trial { trial: sp::Trial, reused: bool },
    /// All trials ran and were saved.
    TrialsMade,
    /// The cards could not be compared; each counts as a kind of its own.
    CompareFailed { error: String },
    /// The card taken by itself, and why (`same`: cards showing the same
    /// particle; `other`: other good cards, other kinds).
    Using { card: sp::Card, same: Vec<usize>, other: Vec<usize>, dir: String },
    /// The card made from the particles the user clicked on: how many were
    /// used and the size measured (Å).
    Marked { points: usize, size_a: f64 },
}

/// A finished run.
pub struct Done {
    pub report: sp::Report,
    pub card: sp::Card,
    pub cards: Vec<sp::Card>,
    /// Empty when a card was asked for (the trials are not read then).
    pub trials: Vec<sp::Trial>,
    /// Set when the program chose the card.
    pub chosen: Option<sp::Chosen>,
    pub cards_dir: PathBuf,
    pub out: PathBuf,
    pub backend: String,
}

pub enum Outcome {
    Done(Box<Done>),
    /// Several kinds average well, or no card did, and no card was asked for.
    NeedsChoice { cards: Vec<sp::Card>, trials: Vec<sp::Trial>, good: Vec<usize>, cards_dir: PathBuf },
}

/// The device to compute on, and its name.
pub type Device<'a> = &'a dyn Fn() -> Result<(&'static burn::tensor::Device, &'static str)>;

/// The look record kept with `tomogram` in its dataset, when its file is
/// there (also if the folder was copied: then the file beside the tomogram).
pub(super) fn look_of(d: Option<&Dataset>, tomogram: &str, note: &(dyn Fn(Note) + Sync)) -> Option<Look> {
    let name = Path::new(tomogram).file_name()?;
    let mut look = d?.tomograms.iter().find(|t| Path::new(&t.path).file_name() == Some(name))?.look.clone()?;
    if !Path::new(&look.path).is_file() {
        // The look is in the folder of the tomogram or one that goes with it
        // (`3_clean/` beside `2_tomograms/` in a new project).
        let dir = Path::new(tomogram).parent().unwrap_or(Path::new("."));
        if let Some(found) = crate::project_dirs::find_near(dir, &look.path) {
            look.path = found.to_string_lossy().into_owned();
        } else {
            note(Note::LookMissing { look: look.path.clone(), tomogram: tomogram.to_string() });
            return None;
        }
    }
    Some(look)
}

/// First and last tilt angle (°) of `angles`; None with fewer than two or all equal.
pub fn range_of(angles: &[f64]) -> Option<[f64; 2]> {
    let lo = angles.iter().copied().filter(|a| a.is_finite()).fold(f64::INFINITY, f64::min);
    let hi = angles.iter().copied().filter(|a| a.is_finite()).fold(f64::NEG_INFINITY, f64::max);
    (hi > lo).then_some([lo, hi])
}

/// The tilts used for the tomogram: the refined angles (levelling included)
/// of the alignment's views that were not left out, else the series' angles.
pub fn tilt_range(d: &Dataset) -> Option<([f64; 2], &'static str)> {
    if let Some(a) = &d.alignment {
        let used: Vec<f64> = a.views.iter().filter(|v| !v.excluded).map(|v| v.refined()).collect();
        if let Some(r) = range_of(&used) {
            return Some((r, "alignment"));
        }
    }
    range_of(&d.angles).map(|r| (r, "tilt angles"))
}

/// The dataset (`*.tomo.json`) beside `tomogram` that lists it: in its own
/// folder, and for a file of a step folder (`3_clean/`) in the folders that
/// go with it (`2_tomograms/` holds the datasets of a new project).
pub fn dataset_of(tomogram: &str) -> Option<Dataset> {
    let p = Path::new(tomogram);
    let name = p.file_name()?;
    crate::project_dirs::relatives(p.parent()?).into_iter().find_map(|dir| datasets_in(&dir).into_iter().find(|d| d.tomograms.iter().any(|t| Path::new(&t.path).file_name() == Some(name))))
}

/// The datasets (`*.tomo.json`) in `dir`, by file name.
pub(super) fn datasets_in(dir: &Path) -> Vec<Dataset> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|f| f.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".tomo.json"))).collect();
    files.sort();
    files.into_iter().filter_map(|f| serde_json::from_slice(&std::fs::read(f).ok()?).ok()).collect()
}

/// A tomogram recorded in a dataset as a file of `dir` (also if the folder
/// was copied: then the file of that name beside the dataset), else as recorded.
/// In a new project the folders of the other steps and the project folder
/// count as beside it too.
pub(super) fn beside(dir: &Path, recorded: &str) -> Option<String> {
    crate::project_dirs::find_near(dir, recorded).map(|f| f.to_string_lossy().into_owned())
}

/// When `path` is the clear "look" companion of a tomogram, that tomogram's
/// file (the one to average); None when it is not a look file. An error when
/// it is one but its tomogram is gone.
pub fn raw_of_look(path: &str) -> Result<Option<String>> {
    let p = Path::new(path);
    let (Some(name), Some(dir)) = (p.file_name(), p.parent()) else { return Ok(None) };
    // A look file lies in `3_clean/` of a new project, its dataset in `2_tomograms/`.
    for folder in crate::project_dirs::relatives(dir) {
        for d in datasets_in(&folder) {
            for t in &d.tomograms {
                if t.look.as_ref().is_some_and(|l| Path::new(&l.path).file_name() == Some(name)) {
                    return beside(&folder, &t.path).map(Some).with_context(|| format!("{} 是清晰版断层图，但它对应的断层图 {} 不见了，无法平均", path, t.path));
                }
            }
        }
    }
    Ok(None)
}

/// The inputs as tomograms to average: a look companion becomes its
/// tomogram (averages are never made from the look file), repeats go.
pub fn raw_inputs(inputs: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = vec![];
    for i in inputs {
        let r = raw_of_look(i)?.unwrap_or_else(|| i.clone());
        if !out.contains(&r) {
            out.push(r);
        }
    }
    Ok(out)
}

/// A tomogram of the workspace list, for averaging.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sibling {
    /// The tomogram to average.
    pub raw: String,
    /// Its clear companion, when there is one.
    pub look: Option<String>,
    /// How a look voxel maps onto a raw one: raw = scale × look + offset.
    pub look_scale: Option<f64>,
    pub look_offset: Option<[f64; 3]>,
    pub name: String,
    /// The file open in the viewer is this one (or its look companion).
    pub open: bool,
}

/// The tomograms that can be averaged together with the open file `path`
/// (the tomogram or its look companion): the full tomograms of the datasets
/// beside it that have a known tilt range, their files there; those made for
/// averaging (with a look companion) when there are any. The open file's
/// own tomogram is always listed, first when it belongs to no dataset.
pub fn siblings(path: &str) -> Vec<Sibling> {
    let p = Path::new(path);
    let (Some(name), Some(dir)) = (p.file_name(), p.parent()) else { return vec![] };
    let mut items: Vec<Sibling> = vec![];
    // The folder of the open file, then the folders beside it: a tomogram
    // often has a folder of its own (05/, 06/, …) under the one of the session.
    // (In a new project: the step folders of the project.)
    let folders = crate::project_dirs::scan_folders(dir);
    for (folder, d) in folders.iter().flat_map(|f| datasets_in(f).into_iter().map(move |d| (f, d))) {
        let ranged = tilt_range(&d).is_some();
        for t in d.tomograms.iter().filter(|t| t.kind == "full") {
            let Some(raw) = beside(folder, &t.path) else { continue };
            let raw_name = Path::new(&raw).file_name().map(|n| n.to_owned());
            if items.iter().any(|i| Path::new(&i.raw).file_name().map(|n| n.to_owned()) == raw_name) {
                continue;
            }
            let look = look_of(Some(&d), &raw, &|_| {});
            let open = raw_name.as_deref() == Some(name) || look.as_ref().is_some_and(|l| Path::new(&l.path).file_name() == Some(name));
            if !ranged && !open {
                continue;
            }
            items.push(Sibling {
                name: raw_name.map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                look_scale: look.as_ref().map(|l| l.scale),
                look_offset: look.as_ref().map(|l| l.offset),
                look: look.map(|l| l.path),
                raw,
                open,
            });
        }
    }
    if items.iter().any(|i| i.look.is_some()) {
        items.retain(|i| i.look.is_some() || i.open);
    }
    if !items.iter().any(|i| i.open) {
        items.insert(0, Sibling { raw: path.to_string(), look: None, look_scale: None, look_offset: None, name: name.to_string_lossy().into_owned(), open: true });
    }
    items
}

/// What is known about `tomogram` (from `input`, which made it) to average it:
/// tilt range, look companion and tilt series from its `dataset`.
pub fn resolve(input: &str, tomogram: String, dataset: Option<Dataset>, note: &(dyn Fn(Note) + Sync)) -> Resolved {
    let (tilts, tilts_from) = match dataset.as_ref().and_then(tilt_range) {
        Some(t) => t,
        None => {
            note(Note::NoDataset { tomogram: tomogram.clone() });
            ([-60.0, 60.0], "default -60…60")
        }
    };
    let look = look_of(dataset.as_ref(), &tomogram, note);
    let name = Path::new(&tomogram).file_name();
    let series = dataset.as_ref().and_then(|d| {
        let placed = d.tomograms.iter().any(|t| Path::new(&t.path).file_name() == name && t.z_offset.is_some());
        // Beside the tomogram, also when the folder was moved.
        let name = crate::tomo::dataset::path_for(&d.stack).file_name()?.to_string_lossy().into_owned();
        let file = crate::project_dirs::relatives(Path::new(&tomogram).parent().unwrap_or(Path::new("."))).into_iter().map(|d| d.join(&name)).find(|f| f.is_file());
        placed.then(|| file.map(|f| f.to_string_lossy().into_owned())).flatten()
    });
    if series.is_none() && dataset.is_some() {
        note(Note::OlderVersion { tomogram: tomogram.clone() });
    }
    Resolved { input: input.to_string(), tomogram, tilts, tilts_from, look, series }
}

/// A tomogram of the workspace: resolved from the dataset beside it.
pub fn resolve_tomogram(tomogram: &str, note: &(dyn Fn(Note) + Sync)) -> Resolved {
    resolve(tomogram, tomogram.to_string(), dataset_of(tomogram), note)
}

/// The default output folder: `4_sta` of the project for a tomogram of a
/// new project, else (old flat project, tomogram from elsewhere) `<stem>_sta`
/// beside the first input.
pub fn default_out(first: &str) -> PathBuf {
    crate::project_dirs::sta_dir(first)
}

/// The output folder of the workspace for the first listed tomogram `first`:
/// beside the tomogram to average, also when `first` is its look companion.
pub fn out_of(first: &str) -> PathBuf {
    default_out(&raw_of_look(first).ok().flatten().unwrap_or_else(|| first.to_string()))
}

/// What a run does, to count its steps: making the cards (two steps),
/// trying them as templates (one), then the search, the first alignment and
/// the fine one.
#[derive(Clone, Copy)]
pub struct Plan {
    pub cards: bool,
    pub trials: bool,
    /// False when the run ends with the cards (the user chooses next).
    pub average: bool,
}

impl Plan {
    pub fn steps(self) -> usize {
        3 * usize::from(self.average) + 2 * usize::from(self.cards) + usize::from(self.trials)
    }
}

/// A progress label of the library, prefixed with the step of the whole run
/// ("第 k/N 步 · …", as `tmv tomo` does). The steps follow the sequence of
/// labels `sta::project::cards`, `trials` and `run_template` report; others pass unchanged.
pub fn sta_label(label: &str, plan: Plan) -> String {
    let cards = if plan.cards { 2 } else { 0 };
    let after = cards + usize::from(plan.trials);
    let step = if label.starts_with("找颗粒") {
        1
    } else if label.starts_with("裁出颗粒") || label.starts_with("分类") {
        2
    } else if label.starts_with("试跑卡片") {
        cards + 1
    } else if label.starts_with("搜索") {
        after + 1
    } else if label.starts_with("粗对齐") {
        after + 2
    } else if label.starts_with("裁出更细的颗粒") || label.starts_with("精细对齐") {
        after + 3
    } else {
        return label.to_string();
    };
    format!("第 {step}/{} 步 · {label}", plan.steps())
}

/// The card to take without asking (see `sta::project::choose`): the one
/// that gave the best average of the trials, when the cards close to it show
/// one kind of particle (compared now when there are several, about a second
/// for a pair; a card that cannot be read leaves the choice to the user).
/// Else None.
pub fn auto_pick(cards: &[sp::Card], trials: &[sp::Trial], found: Option<(&str, &[sp::Source])>, note: &(dyn Fn(Note) + Sync)) -> Option<sp::Chosen> {
    let good = sp::good(trials);
    let similar = if good.len() > 1 {
        match sp::likeness(cards, &good, found) {
            Ok(m) => m,
            Err(e) => {
                note(Note::CompareFailed { error: format!("{e:#}") });
                vec![]
            }
        }
    } else {
        vec![]
    };
    sp::choose(cards, trials, &similar)
}

/// The best working trial's card: what the workspace takes when `auto_pick`
/// leaves the choice open (the user is not asked there).
pub fn best_card(trials: &[sp::Trial]) -> Option<usize> {
    let good = sp::good(trials);
    trials.iter().filter(|t| good.contains(&t.card)).max_by(|a, b| a.strength.total_cmp(&b.strength).then(b.card.cmp(&a.card))).map(|t| t.card)
}

pub fn trial_json(t: &sp::Trial) -> Value {
    json!({
        "card": t.card, "ok": t.ok, "reason": t.reason, "particles": t.particles, "candidates": t.candidates,
        "agreement": (t.strength * 1e5).round() / 1e5, "voxelA": t.voxel_a, "fsc": t.fsc, "seconds": (t.seconds * 10.0).round() / 10.0, "elongation": t.elongation, "runsOn": t.runs_on(),
    })
}

pub fn card_json(c: &sp::Card) -> Value {
    json!({ "id": c.id, "diameterA": c.diameter_a, "members": c.members, "compact": (c.compact * 10.0).round() / 10.0, "likely": c.likely, "path": c.path })
}

/// An input tomogram as the cards were made from it (`cards/inputs.json`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InputStamp {
    pub path: String,
    pub size: u64,
    /// Modified time, nanoseconds since the epoch (0 when unknown).
    pub modified_ns: u128,
}

pub fn input_stamps<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<InputStamp> {
    paths
        .map(|p| {
            let abs = std::path::absolute(p).unwrap_or_else(|_| PathBuf::from(p));
            let meta = std::fs::metadata(&abs).ok();
            let modified_ns = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            InputStamp { path: abs.to_string_lossy().into_owned(), size: meta.map_or(0, |m| m.len()), modified_ns }
        })
        .collect()
}

/// Why saved cards cannot be reused for the present inputs (None: they can,
/// the inputs are the same, in the same order).
pub fn stale(saved: Option<&[InputStamp]>, now: &[InputStamp]) -> Option<String> {
    let Some(saved) = saved else {
        return Some("cards/inputs.json is missing or unreadable, so it is not known which tomograms they were made from".into());
    };
    if saved.len() != now.len() {
        return Some(format!("they were made from {} tomogram(s), now there are {}", saved.len(), now.len()));
    }
    for (a, b) in saved.iter().zip(now) {
        if a.path != b.path {
            return Some(format!("they were made from {}, now {}", a.path, b.path));
        }
        if a.size != b.size || a.modified_ns != b.modified_ns {
            return Some(format!("{} has changed since they were made", b.path));
        }
    }
    None
}

/// The cards and trials saved in `out` (`cards/cards.json`, `cards/trials.json`),
/// with card paths repaired when the folder was copied. Either is empty when missing.
pub fn saved(out: &Path) -> (Vec<sp::Card>, Vec<sp::Trial>) {
    let dir = out.join("cards");
    let mut cards: Vec<sp::Card> = std::fs::read(dir.join("cards.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    fix_paths(&mut cards, &dir);
    let trials = std::fs::read(dir.join("trials.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    (cards, trials)
}

/// The folder may have been copied elsewhere.
fn fix_paths(cards: &mut [sp::Card], dir: &Path) {
    for c in cards {
        if !Path::new(&c.path).is_file() {
            c.path = dir.join(format!("card_{}.mrc", c.id)).to_string_lossy().into_owned();
        }
    }
}

/// The cards saved in `cards_dir` when they are of these tomograms (else
/// none), and whether they have to be made.
fn saved_cards(sources: &[sp::Source], cards_dir: &Path, note: &(dyn Fn(Note) + Sync)) -> (Vec<sp::Card>, bool) {
    // The cards are made from the full-detail tomograms alone (not the look ones).
    let stamps = input_stamps(sources.iter().map(|s| s.path.as_str()));
    let mut cards: Vec<sp::Card> = std::fs::read(cards_dir.join("cards.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if !cards.is_empty() {
        let saved: Option<Vec<InputStamp>> = std::fs::read(cards_dir.join("inputs.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        if let Some(why) = stale(saved.as_deref(), &stamps) {
            note(Note::CardsStale { why });
            cards.clear();
        }
    }
    let making = cards.is_empty();
    (cards, making)
}

/// Makes the cards (`making`) or takes the saved ones; fails when there is none.
#[allow(clippy::too_many_arguments)]
fn make_cards(cards: &mut Vec<sp::Card>, making: bool, sources: &[sp::Source], cards_dir: &Path, device: Device, report: Progress, live: Live, note: &(dyn Fn(Note) + Sync)) -> Result<()> {
    if making {
        // Trials of older cards say nothing about these.
        let _ = std::fs::remove_file(cards_dir.join("trials.json"));
        // The device is asked for here too, for the classification; without one the cards are made on the CPU.
        let gpu = device().ok().map(|d| d.0);
        *cards = sp::cards_on(gpu, sources, &cards_dir.to_string_lossy(), report, live)?;
        let stamps = input_stamps(sources.iter().map(|s| s.path.as_str()));
        std::fs::write(cards_dir.join("inputs.json"), serde_json::to_string_pretty(&stamps)?)?;
        note(Note::CardsMade { count: cards.len() });
    } else {
        note(Note::CardsReused { dir: cards_dir.to_string_lossy().into_owned() });
        fix_paths(cards, cards_dir);
    }
    if cards.is_empty() {
        bail!("no card: no kind of blob with enough members was found in these tomograms (is it a tomogram made with --for sta, and does it hold particles?)");
    }
    Ok(())
}

/// The first half of a run in the workspace: the kinds of thing found in the
/// tomograms, as cards (reused while the inputs are the same). No card is
/// tried and none is taken: what to average is the user's choice, a card or
/// something marked (`marks`).
pub fn find(sources: &[sp::Source], out: &Path, device: Device, progress: Progress, live: Live, note: &(dyn Fn(Note) + Sync)) -> Result<Vec<sp::Card>> {
    let cards_dir = out.join("cards");
    let (mut cards, making) = saved_cards(sources, &cards_dir, note);
    let plan = Plan { cards: making, trials: false, average: false };
    let report = |s: &str, i: usize, n: usize| progress(&sta_label(s, plan), i, n);
    make_cards(&mut cards, making, sources, &cards_dir, device, &report, live, note)?;
    Ok(cards)
}

/// The run: cards (reused while the inputs are the same), trials (reused
/// with them), the card to take, the average. `pick` takes that card (and
/// skips the trials); otherwise the program chooses and, when it cannot
/// decide, returns `Outcome::NeedsChoice`. Progress labels carry the step of
/// the whole run.
#[allow(clippy::too_many_arguments)]
pub fn auto(
    sources: &[sp::Source],
    pick: Option<usize>,
    out: &Path,
    device: Device,
    progress: Progress,
    live: Live,
    note: &(dyn Fn(Note) + Sync),
) -> Result<Outcome> {
    let cards_dir = out.join("cards");
    let (mut cards, making) = saved_cards(sources, &cards_dir, note);
    // The trials belong to these cards: reused with them, and only when they
    // are all there (as many as the cards, at most `TRIALS`).
    let trials_json = cards_dir.join("trials.json");
    let mut trials: Vec<sp::Trial> = vec![];
    if !making && pick.is_none() {
        trials = std::fs::read(&trials_json).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if trials.len() != cards.len().min(sp::TRIALS) || trials.iter().zip(0..).any(|(t, i)| t.card != i) {
            trials.clear();
        }
    }
    let plan = Plan { cards: making, trials: pick.is_none() && trials.is_empty(), average: true };
    let report = |s: &str, i: usize, n: usize| progress(&sta_label(s, plan), i, n);
    make_cards(&mut cards, making, sources, &cards_dir, device, &report, live, note)?;

    // Choosing: each card is tried as the template of a coarse run, and the one that gives the best average wins.
    let (dev, backend) = device()?;
    let out_s = out.to_string_lossy().into_owned();
    if pick.is_none() {
        if plan.trials {
            let dir = out.join("trials").to_string_lossy().into_owned();
            let each = |t: &sp::Trial| note(Note::Trial { trial: t.clone(), reused: false });
            trials = crate::ctf_gpu::guarded(|| sp::trials(dev, sources, &cards, &dir, &report, live, &each))??;
            note(Note::TrialsMade);
            std::fs::write(&trials_json, serde_json::to_string_pretty(&trials)?)?;
        } else {
            note(Note::TrialsReused { dir: cards_dir.to_string_lossy().into_owned() });
            for t in &trials {
                note(Note::Trial { trial: t.clone(), reused: true });
            }
        }
    }
    let mut chosen = None;
    let n = match pick {
        Some(n) => n,
        None => match auto_pick(&cards, &trials, Some((&out.join("trials").to_string_lossy(), sources)), note) {
            Some(c) => {
                let n = c.id;
                note(Note::Using { card: cards[n].clone(), same: c.same.clone(), other: c.other.clone(), dir: cards_dir.to_string_lossy().into_owned() });
                chosen = Some(c);
                n
            }
            None => {
                let good = sp::good(&trials);
                return Ok(Outcome::NeedsChoice { cards, trials, good, cards_dir });
            }
        },
    };
    let card = cards.get(n).with_context(|| format!("--pick {n}: there are only {} cards (0 to {})", cards.len(), cards.len() - 1))?.clone();

    // The average.
    let r = average_with(dev, sources, &card, &out_s, &report, live)?;
    // Which card this average is of (the workspace marks it when reopened).
    write_used(out, &json!({ "kind": "auto", "card": card.id, "auto": chosen.is_some(), "sizeA": card.diameter_a.round() }));
    Ok(Outcome::Done(Box::new(Done { report: r, card, cards, trials, chosen, cards_dir, out: out.to_path_buf(), backend: backend.to_string() })))
}

/// The end of every run: the tomograms searched with `card` as the template
/// and the average made of what it found (`sta::project::run_template`).
fn average_with(dev: &burn::tensor::Device, sources: &[sp::Source], card: &sp::Card, out: &str, progress: Progress, live: Live) -> Result<sp::Report> {
    let (cube, voxel) = sp::read_card(&card.path)?;
    crate::ctf_gpu::guarded(|| sp::run_template(dev, sources, &cube, voxel, sp::template_diameter(card), sp::Pass::Fine, out, progress, live))?
}

/// The run for particles the user clicked on (tool "点"): their orientations
/// are unknown, so they become one card (`sta::project::card_from_points`) and
/// the ordinary template run follows, as in `auto` with a card asked for. All
/// goes to `out/marked/`, so the automatic result in `out` and the drawn one in
/// `out/drawn/` stay. `points`: (source, raw-tomogram voxels).
pub fn auto_points(
    sources: &[sp::Source],
    points: &[(usize, [f64; 3])],
    out: &Path,
    device: Device,
    progress: Progress,
    live: Live,
    note: &(dyn Fn(Note) + Sync),
) -> Result<Outcome> {
    let dir = out.join(MARKED);
    // The card (found blobs, cubes, classes) then the three steps of the template run.
    let plan = Plan { cards: true, trials: false, average: true };
    let report = |s: &str, i: usize, n: usize| progress(&sta_label(s, plan), i, n);
    let card = sp::card_from_points(sources, points, &dir.to_string_lossy(), &report, live)?;
    // The size said is the one the search works with (from the blobs under
    // the clicks): measured on the average of a handful of particles it is not to be trusted.
    note(Note::Marked { points: card.members, size_a: card.diameter_a });
    let (dev, backend) = device()?;
    let r = average_with(dev, sources, &card, &dir.to_string_lossy(), &report, live)?;
    write_used(&dir, &json!({ "kind": "points", "points": card.members, "sizeA": card.diameter_a.round() }));
    Ok(Outcome::Done(Box::new(Done { report: r, cards: vec![card.clone()], card, trials: vec![], chosen: None, cards_dir: dir.clone(), out: dir, backend: backend.to_string() })))
}

/// Where `auto_points` keeps its card and its average, under the output folder.
pub const MARKED: &str = "marked";

/// Where the shapes drawn by the user are averaged, under the output folder.
pub const DRAWN: &str = "drawn";

/// Beside the average: what it was made from (`kind`: "auto" with the `card`
/// and whether the program chose it, "points" with their count, "shapes" with
/// the number of lines, surfaces and spheres) and the particle size (`sizeA`).
pub const CHOSEN: &str = "chosen.json";

/// Keeps what an average was made from beside it (`CHOSEN`).
pub fn write_used(dir: &Path, used: &Value) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(CHOSEN), used.to_string());
}

/// What the average in `dir` was made from (`CHOSEN`; files of older versions
/// that hold only the card are read as the automatic kind); Null when unknown.
pub fn used_of(dir: &Path) -> Value {
    let Some(mut v) = std::fs::read(dir.join(CHOSEN)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return Value::Null };
    if v["kind"].is_null() {
        if v["card"].is_null() {
            return Value::Null;
        }
        v["kind"] = json!("auto");
    }
    if v["kind"] == "auto" && v["sizeA"].is_null() {
        if let Some(c) = v["card"].as_u64().and_then(|n| saved(dir).0.into_iter().find(|c| c.id == n as usize)) {
            v["sizeA"] = json!(c.diameter_a.round());
        }
    }
    v
}

/// The results an output folder holds, newest first: the automatic one in
/// `out`, the one of the clicked particles in `out/marked`, the drawn one in `out/drawn`.
pub fn results_of(out: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut found: Vec<(std::time::SystemTime, &'static str, PathBuf)> = [("auto", out.to_path_buf()), ("points", out.join(MARKED)), ("drawn", out.join(DRAWN))]
        .into_iter()
        .filter_map(|(which, dir)| {
            let t = std::fs::metadata(dir.join("average.mrc")).ok().filter(|m| m.is_file())?.modified().ok().unwrap_or(std::time::UNIX_EPOCH);
            Some((t, which, dir))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, w, d)| (w, d)).collect()
}

/// The result of an output folder for the workspace: the newest one, or the
/// one `which` names ("auto", "points", "drawn"), as `result_value` plus
/// `which` and `available` (all the results there, newest first).
pub fn result_pick(out: &Path, which: Option<&str>, b64: &dyn Fn(&[u8]) -> String) -> Result<Option<Value>> {
    let all = results_of(out);
    let Some((w, dir)) = which.and_then(|w| all.iter().find(|(n, _)| *n == w)).or(all.first()) else { return Ok(None) };
    let Some(mut v) = result_value(dir, b64)? else { return Ok(None) };
    v["which"] = json!(w);
    v["available"] = json!(all.iter().map(|(n, _)| *n).collect::<Vec<_>>());
    v["base"] = json!(out);
    Ok(Some(v))
}

/// The three central slices of a cubic volume (`n`³, z-major) side by side
/// (XY, XZ, YZ), each `cell` pixels, as a PNG.
pub fn slices_png(vol: &[f32], n: usize, cell: usize) -> Result<Vec<u8>> {
    if n < 2 || vol.len() != n * n * n {
        bail!("not a cube: {} values for side {n}", vol.len());
    }
    let m = n / 2;
    let xy = vol[m * n * n..(m + 1) * n * n].to_vec();
    let xz: Vec<f32> = (0..n).flat_map(|z| (0..n).map(move |x| (z, x))).map(|(z, x)| vol[z * n * n + m * n + x]).collect();
    let yz: Vec<f32> = (0..n).flat_map(|z| (0..n).map(move |y| (z, y))).map(|(z, y)| vol[z * n * n + y * n + m]).collect();
    // One grey scale for the three panels, from the whole map.
    let range = grey_range(vol);
    crate::spa::render::gallery_with(&[xy, xz, yz], n, 3, cell, &|v| grey_of(v, range))
}

/// The values that are black and white in a preview of `vol`: its 0.5 % and
/// 99.5 % percentiles, so that a few extreme voxels do not decide the grey
/// scale. Voxels that all have the value of the first one (the masked-out
/// corners of a box, when they are more than 1 % of it) are left out.
pub fn grey_range(vol: &[f32]) -> (f32, f32) {
    let flat = vol.first().copied().unwrap_or(0.0);
    let skip = vol.iter().filter(|&&x| x == flat).count() * 100 > vol.len();
    let mut v: Vec<f32> = vol.iter().copied().filter(|x| x.is_finite() && !(skip && *x == flat)).collect();
    if v.is_empty() {
        return (0.0, 1.0);
    }
    v.sort_by(f32::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    let (lo, hi) = (at(0.005), at(0.995));
    if hi > lo { (lo, hi) } else { (lo, lo + 1.0) }
}

/// `v` as grey values 0–255, `range.0` black and `range.1` white.
pub fn grey_of(v: &[f32], range: (f32, f32)) -> Vec<u8> {
    let s = 255.0 / (range.1 - range.0).max(1e-20);
    v.iter().map(|&x| ((x - range.0) * s).clamp(0.0, 255.0).round() as u8).collect()
}

/// A volume handed to the `live` callback as a preview PNG, when it is a cube.
pub fn live_png(vol: &[f32], cell: usize) -> Option<Vec<u8>> {
    let n = (vol.len() as f64).cbrt().round() as usize;
    slices_png(vol, n, cell).ok()
}

/// A note as JSON, for the workspace (`kind` says which).
pub fn note_json(n: &Note) -> Value {
    match n {
        Note::NoDataset { tomogram } => json!({ "kind": "noDataset", "tomogram": tomogram }),
        Note::LookMissing { look, tomogram } => json!({ "kind": "lookMissing", "look": look, "tomogram": tomogram }),
        Note::OlderVersion { tomogram } => json!({ "kind": "olderVersion", "tomogram": tomogram }),
        Note::CardsStale { why } => json!({ "kind": "cardsStale", "why": why }),
        Note::CardsReused { .. } => json!({ "kind": "cardsReused" }),
        Note::CardsMade { count } => json!({ "kind": "cardsMade", "count": count }),
        Note::TrialsReused { .. } => json!({ "kind": "trialsReused" }),
        Note::Trial { trial, reused } => json!({ "kind": "trial", "reused": reused, "trial": trial_view(trial) }),
        Note::TrialsMade => json!({ "kind": "trialsMade" }),
        Note::CompareFailed { error } => json!({ "kind": "compareFailed", "error": error }),
        Note::Using { card, same, other, .. } => json!({ "kind": "using", "card": card.id, "same": same, "other": other }),
        Note::Marked { points, size_a } => json!({ "kind": "marked", "points": points, "sizeA": size_a.round() }),
    }
}

fn trial_view(t: &sp::Trial) -> Value {
    json!({
        "card": t.card, "ok": t.ok, "reason": t.reason, "particles": t.particles,
        "agreement": (t.strength * 1e5).round() / 1e5, "elongation": t.elongation, "runsOn": t.runs_on(),
        "noGroup": t.reason.as_deref().is_some_and(|r| r.contains("分不出两群")),
    })
}

/// The cards saved in `out` for the workspace: each with its three central
/// slices (`png`, via `b64`), size, members, trial (`null` before it ran),
/// `good` (its trial is among the best) and `chosen`; `chosen` also on its own.
pub fn cards_value(out: &Path, b64: &dyn Fn(&[u8]) -> String) -> Result<Value> {
    let (cards, trials) = saved(out);
    let good = sp::good(&trials);
    let chosen = std::fs::read(out.join(CHOSEN)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["card"].as_u64()).map(|n| n as usize);
    let list: Vec<Value> = cards
        .iter()
        .map(|c| {
            let png = sp::read_card(&c.path).ok().and_then(|(cube, _)| slices_png(&cube, 32, 64).ok());
            let trial = trials.iter().find(|t| t.card == c.id);
            json!({
                "id": c.id, "diameterA": c.diameter_a, "members": c.members, "likely": c.likely,
                "trial": trial.map(trial_view), "good": good.contains(&c.id), "chosen": chosen == Some(c.id),
                "runsOn": trial.is_some_and(|t| t.runs_on()), "png": png.map(|p| b64(&p)),
            })
        })
        .collect();
    Ok(json!({ "cards": list, "chosen": chosen, "out": out }))
}

/// The average saved in `out`: three central slices (`png`), the FSC with the
/// spatial frequency of each shell (1/Å), resolution, particles, sharpening.
/// None when there is no `average.mrc` there yet.
pub fn result_value(out: &Path, b64: &dyn Fn(&[u8]) -> String) -> Result<Option<Value>> {
    let map = out.join("average.mrc");
    if !map.is_file() {
        return Ok(None);
    }
    let (vol, dims, _) = crate::spa::data::read_volume(&map.to_string_lossy())?;
    let report: Value = std::fs::read(out.join("sta.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let voxel = report["voxelA"].as_f64().filter(|v| *v > 0.0).unwrap_or(0.0);
    let fsc: Vec<f64> = report["fsc"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    // The shells are 1 / (n · voxel) apart, n the box (twice the last shell).
    let n = 2.0 * fsc.len().saturating_sub(1) as f64;
    let freq: Vec<f64> = if voxel > 0.0 && n > 0.0 { (0..fsc.len()).map(|i| i as f64 / (n * voxel)).collect() } else { vec![] };
    let png = slices_png(&vol, dims[0], 160).ok().map(|p| b64(&p));
    Ok(Some(json!({
        "map": map, "out": out, "png": png, "fsc": fsc, "freq": freq, "voxelA": voxel,
        "resolutionA": report["resolutionA"], "signalA": report["signalA"], "ctfLimited": report["ctfLimited"],
        "particles": report["particles"], "sharpenB": report["sharpenB"], "used": used_of(out),
    })))
}

/// What `sta_run` hands back: the report, the average, the cards.
pub fn outcome_value(o: &Outcome, out: &Path, b64: &dyn Fn(&[u8]) -> String) -> Result<Value> {
    let cards = cards_value(out, b64)?;
    Ok(match o {
        Outcome::Done(d) => json!({
            "needsChoice": false, "report": serde_json::to_value(&d.report)?, "result": result_value(out, b64)?,
            "cards": cards["cards"], "chosen": d.card.id, "auto": d.chosen.is_some(), "out": out,
            "trials": d.trials.iter().map(trial_json).collect::<Vec<_>>(),
        }),
        Outcome::NeedsChoice { good, trials, .. } => json!({
            "needsChoice": true, "good": good, "cards": cards["cards"], "chosen": Value::Null, "out": out,
            "trials": trials.iter().map(trial_json).collect::<Vec<_>>(),
        }),
    })
}

/// Receives (event name, payload): the workspace turns it into window events.
pub type Emit<'a> = &'a (dyn Fn(&str, Value) + Sync);

/// The workspace's run, `tmv sta` for tomograms: `inputs` are tomogram files
/// (those of a cryo-ET dataset), `out` the folder (default: beside the first).
/// Events: `sta-progress` {label, done, total}, `sta-live` {png} (an
/// intermediate average, three central slices, at most 3 a second),
/// `sta-note` (see `note_json`). What to average is the user's choice, so
/// the run has two halves: without `pick` it makes the cards and returns
/// them (`needsChoice` and `waiting` true; no card is tried or taken), with
/// `pick` it averages that card and returns `outcome_value` with `out`. The
/// caller clears `CANCEL` before.
pub fn run_inputs(inputs: &[String], pick: Option<usize>, out: Option<&str>, device: Device, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    // Averages are of the tomograms, whatever file was handed over.
    let inputs = raw_inputs(inputs)?;
    let Some(first) = inputs.first() else { bail!("请先选择断层图") };
    let out = match out.filter(|o| !o.is_empty()) {
        Some(o) => PathBuf::from(o),
        None => default_out(first),
    };
    let note = |n: Note| emit("sta-note", note_json(&n));
    let sources: Vec<sp::Source> = inputs.iter().map(|i| resolve_tomogram(i, &note).source()).collect();
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
    // No card named: the run ends with the cards and the user chooses.
    let Some(n) = pick else {
        find(&sources, &out, device, &progress, &live, &note)?;
        let cards = cards_value(&out, b64)?;
        return Ok(json!({ "needsChoice": true, "waiting": true, "cards": cards["cards"], "chosen": Value::Null, "out": out, "result": result_value(&out, b64)?, "trials": [] }));
    };
    let outcome = auto(&sources, Some(n), &out, device, &progress, &live, &note)?;
    outcome_value(&outcome, &out, b64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: usize, path: &str) -> sp::Card {
        sp::Card { id, diameter_a: 210.0, measured_a: 200.0, voxel_a: 16.0, members: 50, elongation: 1.1, blobs: 1, offset: 0.0, share: 0.3, path: path.into(), compact: 12.0, likely: false }
    }

    fn trial(card: usize, ok: bool, strength: f64, elongation: Option<f64>) -> sp::Trial {
        sp::Trial {
            card,
            ok,
            reason: (!ok).then(|| "分不出两群".to_string()),
            particles: 80,
            candidates: 100,
            fsc: vec![],
            voxel_a: 16.0,
            strength,
            seconds: 1.0,
            elongation,
        }
    }

    fn b64(b: &[u8]) -> String {
        format!("{}b", b.len())
    }

    fn temp(name: &str) -> PathBuf {
        // A folder of its own one level down: the folders beside a tomogram's are looked into.
        let up = std::env::temp_dir().join(format!("tmv_auto_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&up);
        let d = up.join("d");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn tomograms_in_folders_beside_the_open_one_are_listed_too() {
        let a = dataset_folder("beside");
        let b = a.parent().unwrap().join("e");
        std::fs::create_dir_all(&b).unwrap();
        for f in std::fs::read_dir(&a).unwrap().flatten() {
            let name = f.file_name().to_string_lossy().replace("05", "15").replace("06", "16").replace("07", "17");
            let text = std::fs::read(f.path()).unwrap();
            let text = if name.ends_with(".json") { String::from_utf8_lossy(&text).replace("05", "15").replace("06", "16").replace("07", "17").replace("/d/", "/e/").into_bytes() } else { text };
            std::fs::write(b.join(name), text).unwrap();
        }
        let v = siblings(&a.join("05_sart3.mrc").to_string_lossy());
        let names: Vec<&str> = v.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"05_sart3.mrc") && names.contains(&"06_sart3.mrc"), "{names:?}");
        assert!(names.contains(&"15_sart3.mrc") && names.contains(&"16_sart3.mrc"), "{names:?}");
        assert_eq!(v.iter().filter(|i| i.open).count(), 1);
    }

    fn ball(n: usize) -> Vec<f32> {
        let c = n as f32 / 2.0;
        (0..n * n * n).map(|i| ((i % n) as f32 - c).hypot(((i / n) % n) as f32 - c).hypot((i / (n * n)) as f32 - c)).map(|r| (-r * r / 40.0).exp()).collect()
    }

    #[test]
    fn slices_are_one_png_of_three_panels() {
        let png = slices_png(&ball(32), 32, 40).unwrap();
        assert_eq!(&png[..4], b"\x89PNG");
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (120, 40));
        assert!(slices_png(&[0.0; 10], 32, 40).is_err());
        assert!(live_png(&ball(16), 40).is_some());
        assert!(live_png(&[0.0; 10], 40).is_none());
    }

    #[test]
    fn the_best_good_card_is_the_workspace_fallback() {
        // Two close ones of different kinds: the stronger.
        assert_eq!(best_card(&[trial(0, true, 0.02, None), trial(1, true, 0.025, None), trial(2, false, 0.0, None)]), Some(1));
        // Ties: the lower id. A line is passed over for a particle.
        assert_eq!(best_card(&[trial(0, true, 0.02, None), trial(1, true, 0.02, None)]), Some(0));
        assert_eq!(best_card(&[trial(0, true, 0.05, Some(2.5)), trial(1, true, 0.01, Some(1.3))]), Some(1));
        assert_eq!(best_card(&[trial(0, false, 0.0, None)]), None);
        assert_eq!(best_card(&[]), None);
    }

    #[test]
    fn saved_cards_come_with_their_trials_and_marks() {
        let out = temp("cards");
        let dir = out.join("cards");
        std::fs::create_dir_all(&dir).unwrap();
        let mut cards = vec![];
        for id in 0..3 {
            let p = dir.join(format!("card_{id}.mrc"));
            crate::tomo::output::write_mrc(&p.to_string_lossy(), &ball(32), [32; 3], 16.0, false).unwrap();
            // A path from another computer: found again beside cards.json.
            cards.push(card(id, &format!("/elsewhere/card_{id}.mrc")));
        }
        std::fs::write(dir.join("cards.json"), serde_json::to_string(&cards).unwrap()).unwrap();
        std::fs::write(dir.join("trials.json"), serde_json::to_string(&[trial(0, true, 0.03, Some(1.3)), trial(1, true, 0.029, Some(2.4)), trial(2, false, 0.0, None)]).unwrap()).unwrap();
        std::fs::write(out.join(CHOSEN), r#"{"card":0,"auto":true}"#).unwrap();
        let v = cards_value(&out, &b64).unwrap();
        let list = v["cards"].as_array().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(v["chosen"], 0);
        assert_eq!((list[0]["chosen"].as_bool(), list[1]["chosen"].as_bool()), (Some(true), Some(false)));
        assert_eq!(list[1]["runsOn"], true);
        assert_eq!(list[0]["runsOn"], false);
        assert_eq!(list[0]["good"], true);
        assert_eq!(list[2]["good"], false);
        assert_eq!(list[2]["trial"]["noGroup"], true);
        assert!(list[0]["png"].as_str().unwrap().ends_with('b'));
        assert_eq!(list[0]["trial"]["agreement"], 0.03);
        // Nothing saved: no cards, no error.
        let none = cards_value(&temp("nocards"), &b64).unwrap();
        assert_eq!(none["cards"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn the_result_is_read_from_the_average_and_its_report() {
        let out = temp("result");
        assert!(result_value(&out, &b64).unwrap().is_none());
        crate::tomo::output::write_mrc(&out.join("average.mrc").to_string_lossy(), &ball(32), [32; 3], 8.0, false).unwrap();
        std::fs::write(out.join("sta.json"), r#"{"voxelA":8.0,"resolutionA":24.5,"particles":321,"sharpenB":-120.0,"ctfLimited":false,"fsc":[1.0,0.9,0.5,0.1]}"#).unwrap();
        let v = result_value(&out, &b64).unwrap().unwrap();
        assert_eq!(v["resolutionA"], 24.5);
        assert_eq!(v["particles"], 321);
        assert_eq!(v["fsc"].as_array().unwrap().len(), 4);
        // Four shells: a 6-voxel box, shells 1/(6 · 8 Å) apart.
        assert!((v["freq"][1].as_f64().unwrap() - 1.0 / 48.0).abs() < 1e-12);
        assert!(v["png"].as_str().is_some());
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn notes_are_json_with_a_kind() {
        let t = Note::Trial { trial: trial(2, false, 0.0, None), reused: true };
        let v = note_json(&t);
        assert_eq!((v["kind"].as_str(), v["reused"].as_bool(), v["trial"]["noGroup"].as_bool()), (Some("trial"), Some(true), Some(true)));
        assert_eq!(v["trial"]["card"], 2);
        assert_eq!(note_json(&Note::CardsMade { count: 4 })["count"], 4);
    }

    /// A folder as `tmv tomo --for sta` leaves it: two series (05, 06), each
    /// with its tomogram and look companion, and a plain one (07, no look).
    fn dataset_folder(name: &str) -> PathBuf {
        let dir = temp(name);
        let tomo = |raw: &str, look: Option<&str>| {
            json!({
                "path": dir.join(raw), "kind": "full", "bin": 4, "thickness": 200, "method": "sart",
                "look": look.map(|l| json!({ "path": dir.join(l), "scale": 2.0, "offset": [0.5, 0.5, 1.5] })),
            })
        };
        for (ts, raw, look) in [("05", "05_sart3.mrc", Some("05_look.mrc")), ("06", "06_sart3.mrc", Some("06_look.mrc")), ("07", "07_sart3.mrc", None)] {
            std::fs::write(dir.join(raw), b"x").unwrap();
            if let Some(l) = look {
                std::fs::write(dir.join(l), b"x").unwrap();
            }
            let mut tomos = vec![tomo(raw, look)];
            // Not a tomogram to average: a half map.
            tomos.push(json!({ "path": dir.join(format!("{ts}_even.mrc")), "kind": "even", "bin": 4, "thickness": 200, "method": "sart" }));
            let d = json!({ "version": 1, "stack": dir.join(format!("{ts}.mrc")), "angles": [-60.0, 0.0, 60.0], "tomograms": tomos });
            std::fs::write(dir.join(format!("{ts}.tomo.json")), d.to_string()).unwrap();
        }
        dir
    }

    #[test]
    fn the_tomograms_of_the_folder_are_listed_whichever_file_is_open() {
        let dir = dataset_folder("siblings");
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        // The look file open: the same list, its tomogram is the open one; the plain tomogram is left out.
        let v = siblings(&p("05_look.mrc"));
        assert_eq!(v.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["05_sart3.mrc", "06_sart3.mrc"]);
        assert_eq!(v.iter().map(|i| i.open).collect::<Vec<_>>(), [true, false]);
        assert_eq!(v[0].raw, p("05_sart3.mrc"));
        assert_eq!(v[1].look.as_deref(), Some(p("06_look.mrc").as_str()));
        assert_eq!((v[0].look_scale, v[0].look_offset), (Some(2.0), Some([0.5, 0.5, 1.5])));
        // The tomogram open: the same list.
        let w = siblings(&p("06_sart3.mrc"));
        assert_eq!(w.iter().map(|i| (i.name.as_str(), i.open)).collect::<Vec<_>>(), [("05_sart3.mrc", false), ("06_sart3.mrc", true)]);
        // The plain tomogram open: it is listed too, as the open one.
        let x = siblings(&p("07_sart3.mrc"));
        assert_eq!(x.iter().filter(|i| i.open).map(|i| i.name.as_str()).collect::<Vec<_>>(), ["07_sart3.mrc"]);
        assert_eq!(x.len(), 3);
        // A file of no dataset: itself alone.
        let other = temp("siblings_alone");
        let f = other.join("t.mrc").to_string_lossy().into_owned();
        let one = siblings(&f);
        assert_eq!((one.len(), one[0].raw.as_str(), one[0].open, one[0].look.is_none()), (1, f.as_str(), true, true));
        // A tomogram gone from the folder is not listed.
        std::fs::remove_file(dir.join("06_sart3.mrc")).unwrap();
        assert_eq!(siblings(&p("05_look.mrc")).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_look_file_is_replaced_by_its_tomogram_and_shares_its_output_folder() {
        let dir = dataset_folder("rawof");
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        assert_eq!(raw_of_look(&p("05_look.mrc")).unwrap(), Some(p("05_sart3.mrc")));
        assert_eq!(raw_of_look(&p("05_sart3.mrc")).unwrap(), None);
        assert_eq!(raw_inputs(&[p("05_look.mrc"), p("05_sart3.mrc"), p("06_look.mrc"), p("07_sart3.mrc")]).unwrap(), [p("05_sart3.mrc"), p("06_sart3.mrc"), p("07_sart3.mrc")]);
        assert_eq!(out_of(&p("05_look.mrc")), out_of(&p("05_sart3.mrc")));
        assert_eq!(out_of(&p("05_look.mrc")), dir.join("05_sart3_sta"));
        // The look file of a tomogram that is gone cannot be averaged.
        std::fs::remove_file(dir.join("06_sart3.mrc")).unwrap();
        assert!(raw_inputs(&[p("06_look.mrc")]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_resolves_the_tomogram_when_given_its_look_file() {
        use std::sync::Mutex;
        let dir = dataset_folder("runlook");
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        // No series on record: each resolved tomogram says so (an older-version note), naming the file the run works on.
        let seen = Mutex::new(vec![]);
        let emit = |name: &str, v: Value| {
            if name == "sta-note" && v["kind"] == "olderVersion" {
                seen.lock().unwrap().push(v["tomogram"].as_str().unwrap().to_string());
            }
        };
        let device: Device = &|| bail!("no device");
        let _ = run_inputs(&[p("05_look.mrc"), p("05_sart3.mrc"), p("06_look.mrc")], None, None, device, &emit, &b64);
        assert_eq!(*seen.lock().unwrap(), [p("05_sart3.mrc"), p("06_sart3.mrc")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A ball of values spread evenly in [-1, 1] on a field of exactly one value (the masked-out corners), and a few extreme voxels.
    fn masked_map(n: usize) -> Vec<f32> {
        let c = n as f32 / 2.0;
        let mut seed = 12345u32;
        let mut v: Vec<f32> = (0..n * n * n)
            .map(|i| {
                let (x, y, z) = ((i % n) as f32 - c, ((i / n) % n) as f32 - c, (i / (n * n)) as f32 - c);
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                if x.hypot(y).hypot(z) < c - 1.0 { (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0 } else { 0.0 }
            })
            .collect();
        for k in 0..5 {
            v[n * n * (n / 2) + n * (n / 2) + n / 2 + k] = if k % 2 == 0 { 1000.0 } else { -1000.0 };
        }
        v
    }

    #[test]
    fn the_grey_scale_follows_percentiles_of_the_map_not_its_extremes_or_its_empty_corners() {
        let v = masked_map(32);
        let g = grey_of(&v, grey_range(&v));
        let mut inside: Vec<u8> = v.iter().zip(&g).filter(|(x, _)| **x != 0.0).map(|(_, g)| *g).collect();
        inside.sort_unstable();
        let (lo, hi) = (inside[inside.len() / 20], inside[inside.len() * 19 / 20]);
        assert!(hi as i32 - lo as i32 >= 128, "the central 90 % use only {lo}..{hi}");
        // The previews: most of the picture is neither black nor white.
        let png = slices_png(&v, 32, 32).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_luma8();
        let mid = img.pixels().filter(|p| p.0[0] > 20 && p.0[0] < 235).count();
        assert!(mid * 4 > img.pixels().count(), "{mid} mid greys of {}", img.pixels().count());
        // A flat map does not divide by zero.
        let (a, b) = grey_range(&[3.0; 64]);
        assert!(b > a);
    }

    #[test]
    fn what_an_average_was_made_from_stays_with_it_and_the_newest_result_comes_first() {
        let out = temp("used");
        write_used(&out, &json!({ "kind": "auto", "card": 2, "auto": true, "sizeA": 210.0 }));
        assert_eq!(used_of(&out)["card"], 2);
        // An older chosen.json holds only the card: read as automatic, size from the cards.
        let old = temp("used_old");
        std::fs::create_dir_all(old.join("cards")).unwrap();
        std::fs::write(old.join("cards").join("cards.json"), serde_json::to_string(&[card(0, "a"), card(1, "b")]).unwrap()).unwrap();
        std::fs::write(old.join(CHOSEN), r#"{"card":1,"auto":false}"#).unwrap();
        let u = used_of(&old);
        assert_eq!((u["kind"].as_str(), u["card"].as_u64(), u["sizeA"].as_f64()), (Some("auto"), Some(1), Some(210.0)));
        assert!(used_of(&temp("used_none")).is_null());
        // Three results, the points one the newest.
        let ball = ball(16);
        for (d, age) in [(out.clone(), 30), (out.join(MARKED), 0), (out.join(DRAWN), 60)] {
            std::fs::create_dir_all(&d).unwrap();
            let f = d.join("average.mrc");
            crate::tomo::output::write_mrc(&f.to_string_lossy(), &ball, [16; 3], 8.0, false).unwrap();
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age);
            std::fs::File::options().write(true).open(&f).unwrap().set_modified(t).unwrap();
        }
        write_used(&out.join(MARKED), &json!({ "kind": "points", "points": 7, "sizeA": 190.0 }));
        let v = result_pick(&out, None, &b64).unwrap().unwrap();
        assert_eq!(v["which"], "points");
        assert_eq!(v["available"], json!(["points", "auto", "drawn"]));
        assert_eq!(v["used"]["points"], 7);
        let a = result_pick(&out, Some("auto"), &b64).unwrap().unwrap();
        assert_eq!((a["which"].as_str(), a["used"]["card"].as_u64()), (Some("auto"), Some(2)));
        assert!(result_pick(&temp("used_empty"), None, &b64).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_run_needs_a_tomogram() {
        let device: Device = &|| bail!("no device");
        assert!(run_inputs(&[], None, None, device, &|_, _| {}, &b64).is_err());
    }

    #[test]
    fn the_default_folder_is_beside_the_first_input() {
        assert_eq!(default_out("/data/a/t.mrc"), Path::new("/data/a/t_sta"));
    }

    /// A project made with the step folders: tomograms and dataset in `2_tomograms/`, look files in `3_clean/`.
    fn project(name: &str) -> PathBuf {
        let dir = temp(name);
        let (tomograms, clean) = (dir.join("2_tomograms"), dir.join("3_clean"));
        std::fs::create_dir_all(&tomograms).unwrap();
        std::fs::create_dir_all(&clean).unwrap();
        std::fs::write(dir.join("ts.mrc"), b"x").unwrap();
        for f in ["ts_sart3.mrc", "ts_sart3_even.mrc"] {
            std::fs::write(tomograms.join(f), b"x").unwrap();
        }
        std::fs::write(clean.join("ts_look.mrc"), b"x").unwrap();
        let look = json!({ "path": clean.join("ts_look.mrc"), "scale": 2.0, "offset": [0.5, 0.5, 1.5] });
        let full = json!({ "path": tomograms.join("ts_sart3.mrc"), "kind": "full", "bin": 4, "thickness": 200, "method": "sart", "look": look });
        let d = json!({ "version": 1, "stack": dir.join("ts.mrc"), "angles": [-60.0, 0.0, 60.0], "tomograms": [full] });
        std::fs::write(tomograms.join("ts.tomo.json"), d.to_string()).unwrap();
        dir
    }

    #[test]
    fn a_look_file_in_the_clean_folder_finds_its_tomogram_and_dataset_in_the_tomograms_folder() {
        let dir = project("newproject");
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        assert_eq!(raw_of_look(&p("3_clean/ts_look.mrc")).unwrap(), Some(p("2_tomograms/ts_sart3.mrc")));
        assert_eq!(raw_of_look(&p("2_tomograms/ts_sart3.mrc")).unwrap(), None);
        // The dataset is found from the tomogram and from the look file.
        assert_eq!(dataset_of(&p("2_tomograms/ts_sart3.mrc")).unwrap().stack, p("ts.mrc"));
        assert_eq!(dataset_of(&p("3_clean/ts_look.mrc")).map(|d| d.tomograms.len()), None, "the look is no tomogram of the dataset");
        // Both files open give the one tomogram, with its look beside in the clean folder.
        for open in ["3_clean/ts_look.mrc", "2_tomograms/ts_sart3.mrc"] {
            let v = siblings(&p(open));
            assert_eq!(v.iter().map(|i| (i.raw.as_str(), i.look.as_deref(), i.open)).collect::<Vec<_>>(), [(p("2_tomograms/ts_sart3.mrc").as_str(), Some(p("3_clean/ts_look.mrc").as_str()), true)]);
        }
        // Averaging goes to the project's own folder, whichever file is given.
        assert_eq!(out_of(&p("3_clean/ts_look.mrc")), dir.join("4_sta"));
        assert_eq!(out_of(&p("2_tomograms/ts_sart3.mrc")), dir.join("4_sta"));
        assert_eq!(default_out(&p("2_tomograms/ts_sart3.mrc")), dir.join("4_sta"));
        // The look record resolves, and the dataset is the tilt series' record for the tomogram.
        let r = resolve_tomogram(&p("2_tomograms/ts_sart3.mrc"), &|_| {});
        assert_eq!(r.look.map(|l| l.path), Some(p("3_clean/ts_look.mrc")));
        // A copy of the project (recorded paths are stale) still finds the look beside.
        let copy = temp("newproject_copy");
        for f in ["2_tomograms/ts_sart3.mrc", "2_tomograms/ts.tomo.json", "3_clean/ts_look.mrc"] {
            std::fs::create_dir_all(copy.join(f).parent().unwrap()).unwrap();
            std::fs::copy(dir.join(f), copy.join(f)).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
        let c = |n: &str| copy.join(n).to_string_lossy().into_owned();
        assert_eq!(resolve_tomogram(&c("2_tomograms/ts_sart3.mrc"), &|_| {}).look.map(|l| l.path), Some(c("3_clean/ts_look.mrc")));
        assert_eq!(raw_of_look(&c("3_clean/ts_look.mrc")).unwrap(), Some(c("2_tomograms/ts_sart3.mrc")));
        let _ = std::fs::remove_dir_all(&copy);
    }

    #[test]
    fn an_old_flat_project_still_averages_beside_its_tomogram() {
        let dir = dataset_folder("flatsta");
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        assert_eq!(out_of(&p("06_look.mrc")), dir.join("06_sart3_sta"));
        assert_eq!(default_out(&p("07_sart3.mrc")), dir.join("07_sart3_sta"));
        assert_eq!(dataset_of(&p("06_sart3.mrc")).unwrap().stack, dir.join("06.mrc").to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
