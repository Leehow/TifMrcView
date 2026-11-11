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
    // Which card this average is of (the workspace marks it when reopened).
    let _ = std::fs::write(out.join(CHOSEN), json!({ "card": card.id, "auto": chosen.is_some() }).to_string());
    Ok(Outcome::Done(Box::new(Done { report: r, card, cards, trials, chosen, cards_dir, out: out.to_path_buf(), backend: backend.to_string() })))
}

/// Beside the average: which card it was made from, and whether the program chose it.
pub const CHOSEN: &str = "chosen.json";

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
    crate::spa::render::gallery(&[xy, xz, yz], n, 3, cell)
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
        "particles": report["particles"], "sharpenB": report["sharpenB"],
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
/// `sta-note` (see `note_json`). Never stops to ask: when the program cannot
/// decide it takes the card with the best trial; the result says
/// `needsChoice` only when no card gave an average. Returns `outcome_value`
/// with `out`. The caller clears `CANCEL` before.
pub fn run_inputs(inputs: &[String], pick: Option<usize>, out: Option<&str>, device: Device, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
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
    let mut fell_back = false;
    let mut outcome = auto(&sources, pick, &out, device, &progress, &live, &note)?;
    if let Outcome::NeedsChoice { trials, .. } = &outcome {
        if let Some(n) = best_card(trials) {
            note(Note::Using { card: saved(&out).0.get(n).cloned().context("card missing")?, same: vec![], other: vec![], dir: out.join("cards").to_string_lossy().into_owned() });
            outcome = auto(&sources, Some(n), &out, device, &progress, &live, &note)?;
            // The program took this card, not the user.
            let _ = std::fs::write(out.join(CHOSEN), json!({ "card": n, "auto": true }).to_string());
            fell_back = true;
        }
    }
    let mut v = outcome_value(&outcome, &out, b64)?;
    if fell_back {
        v["auto"] = json!(true);
    }
    Ok(v)
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
        let d = std::env::temp_dir().join(format!("tmv_auto_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
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

    #[test]
    fn a_run_needs_a_tomogram() {
        let device: Device = &|| bail!("no device");
        assert!(run_inputs(&[], None, None, device, &|_, _| {}, &b64).is_err());
    }

    #[test]
    fn the_default_folder_is_beside_the_first_input() {
        assert_eq!(default_out("/data/a/t.mrc"), Path::new("/data/a/t_sta"));
    }
}
