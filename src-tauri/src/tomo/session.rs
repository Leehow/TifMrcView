//! A folder of tilt series worked on as one: what there is in it and how
//! far each series has come (`scan`), the first pass over all of them
//! (`prepare`: motion correction of the movies, a look at every view,
//! alignment and a quick tomogram) and, once the user has looked at what
//! that made, the tomograms themselves (`finish`).
//!
//! The program proposes which series and views to use (`triage`); the user
//! decides (`pick`, `take`), and what the user chose is never changed.
//! A series that fails is noted and the next one goes on.

use super::{auto, dataset::Dataset, pipeline, triage::Verdict};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How far a series has come.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// Movies not yet (all) corrected.
    Movies,
    /// A tilt series, not yet aligned.
    Stack,
    Aligned,
    /// The quick tomogram is there.
    Quick,
    /// The tomogram asked for is there.
    Done,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub name: String,
    /// The mdoc of the movies (None: the stack was there already).
    pub mdoc: Option<String>,
    /// The tilt series (it may not exist yet).
    pub stack: String,
    pub stage: Stage,
    /// Movies of the mdoc, and how many are corrected.
    pub movies: usize,
    pub corrected: usize,
    /// Views of the stack, and how many are left out.
    pub views: usize,
    pub left_out: usize,
    pub verdict: Option<Verdict>,
    /// The user's own choice for the series, and what holds: it, else the verdict.
    pub take: Option<bool>,
    pub chosen: bool,
    pub residual: Option<f64>,
    pub quality: Option<String>,
    /// Small copies of the views, the aligned ones, the quick tomogram and
    /// the tomogram made at the end.
    pub small: Option<String>,
    pub aligned: Option<String>,
    pub quick: Option<String>,
    pub result: Option<String>,
    /// Why the last run on it stopped.
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub root: String,
    pub series: Vec<Series>,
}

/// What went wrong per series (by name), kept in the project.
#[derive(Default, Serialize, Deserialize)]
struct Notes {
    errors: BTreeMap<String, String>,
}

fn notes_path(root: &Path) -> PathBuf {
    root.join(crate::project_dirs::TOMOGRAMS).join("session.json")
}

fn notes(root: &Path) -> Notes {
    std::fs::read(notes_path(root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn note(root: &Path, name: &str, error: Option<String>) {
    let mut n = notes(root);
    match error {
        Some(e) => n.errors.insert(name.into(), e),
        None => n.errors.remove(name),
    };
    let file = notes_path(root);
    if n.errors.is_empty() && !file.exists() {
        return;
    }
    let _ = file.parent().map(std::fs::create_dir_all);
    let _ = std::fs::write(file, serde_json::to_vec_pretty(&n).unwrap_or_default());
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Name of the series an mdoc describes: `TS_01.mrc.mdoc` and `TS_01.mdoc` → `TS_01`.
fn series_name(mdoc: &Path) -> String {
    let n = mdoc.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = n.strip_suffix(".mdoc").unwrap_or(&n);
    ["mrc", "st", "mrcs", "tif", "tiff", "eer"].iter().find_map(|e| n.strip_suffix(&format!(".{e}"))).unwrap_or(n).to_string()
}

fn exists(p: &Option<String>) -> Option<String> {
    p.clone().filter(|p| Path::new(p).exists())
}

/// The series of the project folder `folder` (or of the project a step
/// folder belongs to): every mdoc with movies, and every tilt series lying
/// there with its tilt angles.
pub fn scan(folder: &str) -> Result<Session> {
    let dir = Path::new(folder);
    let root = crate::project_dirs::listing_root(dir).unwrap_or_else(|| dir.to_path_buf());
    if !root.is_dir() {
        bail!("不是文件夹：{folder}");
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&root)?.flatten().map(|e| e.path()).filter(|p| p.is_file() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).collect();
    files.sort();
    let known = notes(&root);
    let mut series: Vec<Series> = vec![];
    let ext = |p: &Path| p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    // Tilt series lying in the folder: a stack with its angles beside it.
    let mut stacks: BTreeMap<String, PathBuf> = BTreeMap::new();
    for f in files.iter().filter(|f| ["mrc", "st", "mrcs"].contains(&ext(f).as_str()) && !crate::project_dirs::is_helper(f)) {
        let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let angles = ["rawtlt", "tlt", "mdoc"].iter().any(|e| f.with_extension(e).exists()) || Path::new(&format!("{}.mdoc", f.display())).exists();
        if angles {
            stacks.entry(stem).or_insert_with(|| f.clone());
        }
    }
    for f in files.iter().filter(|f| ext(f) == "mdoc") {
        let name = series_name(f);
        if stacks.contains_key(&name) || series.iter().any(|s| s.name == name) {
            continue;
        }
        let Ok(doc) = std::fs::read_to_string(f).map_err(anyhow::Error::from).and_then(|t| crate::motion::series::parse(&t)) else { continue };
        let movies = doc.views.iter().filter(|v| crate::motion::series::resolve(f, &v.movie, None).is_some()).count();
        let stack = crate::project_dirs::motion_output(&text(&root.join(format!("{name}.mrc"))));
        let state = std::fs::read(format!("{stack}.motion-series.json")).ok().and_then(|b| serde_json::from_slice::<crate::motion::series::SeriesResult>(&b).ok()).filter(|r| Path::new(&r.mdoc).file_name() == f.file_name());
        let corrected = state.as_ref().map_or(0, |r| r.records.iter().filter(|r| r.section.is_some()).count());
        let complete = state.as_ref().is_some_and(|r| r.complete) && Path::new(&stack).exists();
        if movies == 0 && !complete {
            continue;
        }
        let mut s = blank(name, Some(text(f)), stack);
        s.movies = movies.max(corrected);
        s.corrected = corrected;
        s.small = exists(&state.and_then(|r| r.small));
        if complete {
            fill(&mut s);
        }
        series.push(s);
    }
    for (name, f) in stacks {
        let mut s = blank(name, None, text(&f));
        fill(&mut s);
        series.push(s);
    }
    series.sort_by(|a, b| crate::formats::sequence::natural_cmp(&a.name, &b.name));
    for s in &mut series {
        s.error = known.errors.get(&s.name).cloned();
        s.chosen = s.take.unwrap_or(s.verdict.as_ref().is_none_or(|v| v.usable));
    }
    Ok(Session { root: text(&root), series })
}

fn blank(name: String, mdoc: Option<String>, stack: String) -> Series {
    Series { name, mdoc, stack, stage: Stage::Movies, movies: 0, corrected: 0, views: 0, left_out: 0, verdict: None, take: None, chosen: true, residual: None, quality: None, small: None, aligned: None, quick: None, result: None, error: None }
}

/// What the dataset of an existing stack says about it.
fn fill(s: &mut Series) {
    s.stage = Stage::Stack;
    let file = super::dataset::path_for(&s.stack);
    let Some(d) = std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Dataset>(&b).ok()) else { return };
    s.views = d.angles.len();
    s.left_out = match &d.alignment {
        Some(a) => a.views.iter().filter(|v| v.excluded).count(),
        None => super::triage::excluded(&d).len(),
    };
    s.verdict = d.verdict.clone();
    s.take = d.take;
    s.aligned = exists(&d.aligned);
    s.quick = exists(&d.made.get("survey").cloned());
    s.result = ["display", "sta"].iter().find_map(|k| exists(&d.made.get(*k).cloned()));
    if let Some(a) = &d.alignment {
        s.residual = Some(a.residual);
        s.quality = Some(a.quality.clone());
        s.stage = Stage::Aligned;
    }
    if s.quick.is_some() {
        s.stage = Stage::Quick;
    }
    if s.result.is_some() {
        s.stage = Stage::Done;
    }
}

/// The dataset of a series' stack as saved.
fn dataset(stack: &str) -> Result<Dataset> {
    let img = pipeline::open(stack)?;
    // The stack's own pixel size, not the mdoc's (that of the camera's
    // binned images, not of what was made from the frames).
    let px = img.info().pixel_size.is_some().then(|| crate::align::pixel_a(img.info()));
    Dataset::open(&img.info().path, img.info().size_z, px)
}

/// The user's choice for view `z` of a series: keep it, drop it, or (None)
/// leave it to the program again.
pub fn pick(stack: &str, z: usize, keep: Option<bool>) -> Result<Dataset> {
    let mut d = dataset(stack)?;
    if z >= d.angles.len() {
        bail!("没有这一张");
    }
    match keep {
        Some(k) => d.picks.insert(z, k),
        None => d.picks.remove(&z),
    };
    if !d.checks.is_empty() {
        let dropped: Vec<usize> = d.picks.iter().filter(|(_, keep)| !**keep).map(|(z, _)| *z).collect();
        d.verdict = Some(super::triage::verdict(&d.checks, &dropped));
    }
    d.save()?;
    Ok(d)
}

/// The user's choice for a whole series (None: the program's verdict).
pub fn take(stack: &str, take: Option<bool>) -> Result<Dataset> {
    let mut d = dataset(stack)?;
    d.take = take;
    d.save()?;
    Ok(d)
}

/// Section `z` (None: the middle one) of an image of the project as a
/// picture (`live::encode`) at most `max` pixels a side: the views of a
/// series, its aligned views, a section of a tomogram.
pub fn view(path: &str, z: Option<usize>, max: usize) -> Result<Vec<u8>> {
    let img = pipeline::open(path)?;
    let info = img.info();
    let z = z.unwrap_or(info.size_z / 2).min(info.size_z - 1);
    let (page, sample) = info.locate(0, z, 0);
    let p = img.page(page)?;
    let p = &p[sample.min(p.len() - 1)];
    let values: Vec<f32> = (0..p.w * p.h).map(|i| p.data.get_f64(i) as f32).collect();
    Ok(crate::live::encode(&crate::live::Picture { title: format!("z {} / {}", z + 1, info.size_z), w: p.w, h: p.h, values, mask: vec![] }, max.clamp(32, 2048)))
}

fn cancelled(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains("已取消")
}

/// One series through `work`; a failure is noted and does not stop the rest.
fn each(
    folder: &str,
    wanted: &dyn Fn(&Series) -> bool,
    progress: pipeline::Progress,
    changed: &(dyn Fn() + Sync),
    work: &dyn Fn(&Series, pipeline::Progress) -> Result<()>,
) -> Result<Session> {
    let session = scan(folder)?;
    let root = PathBuf::from(&session.root);
    let todo: Vec<&Series> = session.series.iter().filter(|s| wanted(s)).collect();
    for (i, s) in todo.iter().enumerate() {
        super::check_cancel()?;
        let label = format!("{}（{}/{}）", s.name, i + 1, todo.len());
        note(&root, &s.name, None);
        match work(s, &|stage, a, b| progress(&format!("{label} · {stage}"), a, b)) {
            Ok(()) => {}
            Err(e) if cancelled(&e) => return Err(e),
            Err(e) => note(&root, &s.name, Some(format!("{e:#}"))),
        }
        changed();
    }
    scan(folder)
}

/// The tilt series of `s`, its movies corrected first when it has any (as
/// previews, or in full: a series of previews is then corrected again).
fn stack_of(s: &Series, preview: bool, progress: pipeline::Progress) -> Result<String> {
    let Some(mdoc) = &s.mdoc else { return Ok(s.stack.clone()) };
    crate::motion::begin();
    let name = Path::new(&s.stack).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let output = text(&crate::project_dirs::root_of(Path::new(&s.stack)).join(name));
    let req = crate::motion::series::SeriesReq { mdoc: mdoc.clone(), folder: None, output, params: Default::default(), drop_tilts: true, fresh: false, preview };
    let r = crate::motion::series::run(&req, progress)?;
    match r.path {
        Some(p) if r.angles.len() >= 3 => Ok(p),
        _ => bail!("能用的电影不到三个"),
    }
}

/// The first pass over every series of the folder not yet through it (or
/// only those named in `only`).
pub fn prepare(dev: &burn::tensor::Device, folder: &str, only: &[String], progress: pipeline::Progress, changed: &(dyn Fn() + Sync)) -> Result<Session> {
    each(folder, &|s| if only.is_empty() { s.stage != Stage::Quick && s.stage != Stage::Done } else { only.contains(&s.name) }, progress, changed, &|s, progress| {
        let stack = stack_of(s, true, progress)?;
        changed();
        let mut d = dataset(&stack)?;
        // A series the program would not use is not aligned for nothing;
        // the user who takes it gets it with the next run.
        if d.checks.len() != d.angles.len() {
            super::triage::survey(&pipeline::open(&stack)?, &mut d)?;
            d.save()?;
        }
        if !d.take.unwrap_or(d.verdict.as_ref().is_none_or(|v| v.usable)) {
            return Ok(());
        }
        let req = auto::AutoReq { pixel_a: None, purpose: auto::Purpose::Survey, fresh: false };
        crate::ctf_gpu::guarded(|| auto::run(dev, &mut d, &req, progress))??;
        Ok(())
    })
}

/// The tomograms for `purpose` of every series the user (else the program) chose.
pub fn finish(dev: &burn::tensor::Device, folder: &str, purpose: auto::Purpose, only: &[String], progress: pipeline::Progress, changed: &(dyn Fn() + Sync)) -> Result<Session> {
    each(folder, &|s| s.chosen && (only.is_empty() || only.contains(&s.name)), progress, changed, &|s, progress| {
        // Averaging needs the views in full (every movie corrected at its
        // own pixel, local motion and dose weighting); what was made for
        // looking is enough for a tomogram to show.
        let stack = stack_of(s, purpose != auto::Purpose::Sta, progress)?;
        let mut d = dataset(&stack)?;
        let req = auto::AutoReq { pixel_a: None, purpose, fresh: false };
        crate::ctf_gpu::guarded(|| auto::run(dev, &mut d, &req, progress))??;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_mdoc_names_its_series() {
        assert_eq!(series_name(Path::new("/a/TS_01.mrc.mdoc")), "TS_01");
        assert_eq!(series_name(Path::new("/a/TS_01.mdoc")), "TS_01");
        assert_eq!(series_name(Path::new("/a/lam2.pos3.st.mdoc")), "lam2.pos3");
    }

    #[test]
    fn a_folder_lists_stacks_with_angles_and_mdocs_with_movies() {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str| dir.path().join(n);
        std::fs::write(p("b.mrc"), b"x").unwrap();
        std::fs::write(p("b.tlt"), b"0\n").unwrap();
        std::fs::write(p("lone.mrc"), b"x").unwrap();
        std::fs::write(p("a.mdoc"), "[ZValue = 0]\nTiltAngle = 0\nSubFramePath = a_0.eer\n").unwrap();
        std::fs::write(p("a_0.eer"), b"x").unwrap();
        std::fs::write(p("gone.mdoc"), "[ZValue = 0]\nTiltAngle = 0\nSubFramePath = nowhere.eer\n").unwrap();
        let s = scan(&text(dir.path())).unwrap();
        let names: Vec<(&str, Stage, usize)> = s.series.iter().map(|s| (s.name.as_str(), s.stage, s.movies)).collect();
        assert_eq!(names, [("a", Stage::Movies, 1), ("b", Stage::Stack, 0)]);
        assert!(s.series[0].stack.ends_with("1_motion/a.mrc") && s.series.iter().all(|s| s.chosen));
        note(dir.path(), "a", Some("坏了".into()));
        assert_eq!(scan(&text(dir.path())).unwrap().series[0].error.as_deref(), Some("坏了"));
        note(dir.path(), "a", None);
        assert!(scan(&text(dir.path())).unwrap().series[0].error.is_none());
    }
}
