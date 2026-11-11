//! The whole of "subtomogram averaging with nothing to set", shared by
//! `tmv sta` and the Pro workspace: the tomograms of the inputs → class-average
//! cards (kept, reused while the inputs are the same) → every card tried as a
//! template (kept in `cards/trials.json`) → the card to take → the average.
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
fn look_of(d: Option<&Dataset>, tomogram: &str, note: &(dyn Fn(Note) + Sync)) -> Option<Look> {
    let name = Path::new(tomogram).file_name()?;
    let mut look = d?.tomograms.iter().find(|t| Path::new(&t.path).file_name() == Some(name))?.look.clone()?;
    if !Path::new(&look.path).is_file() {
        let beside = Path::new(tomogram).with_file_name(Path::new(&look.path).file_name()?);
        if beside.is_file() {
            look.path = beside.to_string_lossy().into_owned();
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

/// The dataset (`*.tomo.json`) beside `tomogram` that lists it.
pub fn dataset_of(tomogram: &str) -> Option<Dataset> {
    let p = Path::new(tomogram);
    let name = p.file_name()?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(p.parent()?)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|f| f.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".tomo.json")))
        .collect();
    files.sort();
    files.into_iter().find_map(|f| {
        let d: Dataset = serde_json::from_slice(&std::fs::read(f).ok()?).ok()?;
        d.tomograms.iter().any(|t| Path::new(&t.path).file_name() == Some(name)).then_some(d)
    })
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
        let file = Path::new(&tomogram).with_file_name(crate::tomo::dataset::path_for(&d.stack).file_name()?);
        (placed && file.is_file()).then(|| file.to_string_lossy().into_owned())
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

/// The default output folder: `<stem>_sta` beside the first input.
pub fn default_out(first: &str) -> PathBuf {
    let p = Path::new(first);
    let stem = p.file_stem().unwrap_or_default().to_string_lossy();
    p.with_file_name(format!("{stem}_sta"))
}

/// What a run does, to count its steps: making the cards (two steps),
/// trying them as templates (one), then the search, the first alignment and
/// the fine one.
#[derive(Clone, Copy)]
pub struct Plan {
    pub cards: bool,
    pub trials: bool,
}

impl Plan {
    pub fn steps(self) -> usize {
        3 + if self.cards { 2 } else { 0 } + usize::from(self.trials)
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
    let cards_json = cards_dir.join("cards.json");
    let inputs_json = cards_dir.join("inputs.json");
    // The cards are made from the full-detail tomograms alone (not the look ones).
    let stamps = input_stamps(sources.iter().map(|s| s.path.as_str()));
    let mut cards: Vec<sp::Card> = std::fs::read(&cards_json).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if !cards.is_empty() {
        let saved: Option<Vec<InputStamp>> = std::fs::read(&inputs_json).ok().and_then(|b| serde_json::from_slice(&b).ok());
        if let Some(why) = stale(saved.as_deref(), &stamps) {
            note(Note::CardsStale { why });
            cards.clear();
        }
    }
    let making = cards.is_empty();
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
    let plan = Plan { cards: making, trials: pick.is_none() && trials.is_empty() };
    let report = |s: &str, i: usize, n: usize| progress(&sta_label(s, plan), i, n);
    if making {
        // Trials of older cards say nothing about these.
        let _ = std::fs::remove_file(&trials_json);
        cards = sp::cards(sources, &cards_dir.to_string_lossy(), &report, live)?;
        std::fs::write(&inputs_json, serde_json::to_string_pretty(&stamps)?)?;
        note(Note::CardsMade { count: cards.len() });
    } else {
        note(Note::CardsReused { dir: cards_dir.to_string_lossy().into_owned() });
        fix_paths(&mut cards, &cards_dir);
    }
    if cards.is_empty() {
        bail!("no card: no kind of blob with enough members was found in these tomograms (is it a tomogram made with --for sta, and does it hold particles?)");
    }

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
    let (cube, voxel) = sp::read_card(&card.path)?;
    let r = crate::ctf_gpu::guarded(|| sp::run_template(dev, sources, &cube, voxel, sp::template_diameter(&card), sp::Pass::Fine, &out_s, &report, live))??;
    Ok(Outcome::Done(Box::new(Done { report: r, card, cards, trials, chosen, cards_dir, out: out.to_path_buf(), backend: backend.to_string() })))
}
