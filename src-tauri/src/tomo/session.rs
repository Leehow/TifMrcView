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
    /// What was done after reconstructing `result` ("精对齐", "N2N 降噪", …).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub done: Vec<String>,
    /// Why the last run on it stopped.
    pub error: Option<String>,
    /// Something the user should know about what was made (a step left out).
    pub note: Option<String>,
    /// Pixel size of the tilt series (Å) and its views' width and height,
    /// for choosing the binning; before motion correction the mdoc's
    /// (approximate: the frames may be made at another pixel size).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixel_a: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<[usize; 2]>,
    /// Binning of the tomogram shown on the card (`result`, else `quick`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub root: String,
    pub series: Vec<Series>,
    pub warnings: Vec<String>,
    pub gains: Vec<String>,
}

/// What went wrong per series (by name), kept in the project.
#[derive(Default, Serialize, Deserialize)]
struct Notes {
    errors: BTreeMap<String, String>,
    #[serde(default)]
    notes: BTreeMap<String, String>,
}

fn notes_path(root: &Path) -> PathBuf {
    std::env::var_os("TMV_OUTPUT_ROOT").map(PathBuf::from).unwrap_or_else(||root.to_path_buf()).join(crate::project_dirs::TOMOGRAMS).join("session.json")
}

fn notes(root: &Path) -> Notes {
    std::fs::read(notes_path(root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn note(root: &Path, name: &str, error: Option<String>) {
    remark(root, name, error, true)
}

/// Keeps (or with None forgets) what went wrong with a series, or a note on it.
fn remark(root: &Path, name: &str, text: Option<String>, error: bool) {
    merge_notes(&notes_path(root),name,text,error);
}
fn merge_notes(file:&Path,name:&str,text:Option<String>,error:bool){
    if let Some(parent)=file.parent(){if std::fs::create_dir_all(parent).is_err(){return;}}
    // Different GPU processes update this shared summary. Lock the whole
    // read/modify/rename, not only the final write.
    let Ok(lock)=std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(file.with_extension("lock"))else{return;};
    if lock.lock().is_err(){return;}
    let mut n:Notes=std::fs::read(file).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_default();
    let map=if error{&mut n.errors}else{&mut n.notes};
    match text{Some(e)=>map.insert(name.into(),e),None=>map.remove(name)};
    if n.errors.is_empty()&&n.notes.is_empty()&&!file.exists(){return;}
    if let Ok(text)=serde_json::to_string_pretty(&n){let _=crate::motion::output::text(file,&text);}
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Name of the series an mdoc describes: `TS_01.mrc.mdoc` and `TS_01.mdoc` → `TS_01`.
fn series_name(mdoc: &Path) -> String {
    let n = mdoc.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n=if n.to_ascii_lowercase().ends_with(".mdoc"){&n[..n.len()-5]}else{&n};
    let ext=Path::new(n).extension().and_then(|e|e.to_str()).unwrap_or("");
    if ["mrc","st","mrcs","tif","tiff","eer"].iter().any(|e|e.eq_ignore_ascii_case(ext)){n[..n.len()-ext.len()-1].to_string()}else{n.to_string()}
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
    let _index=crate::acquisition::scope(&root)?;
    let index=crate::acquisition::active().unwrap();
    let files=index.files.clone();
    let mut warnings=index.warnings.clone();
    let known = notes(&root);
    let mut series: Vec<Series> = vec![];
    let ext = |p: &Path| p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    // Tilt series lying in the folder: a stack with its angles beside it.
    let mut stacks: BTreeMap<String, PathBuf> = BTreeMap::new();
    for f in files.iter().filter(|f| ["mrc", "st", "mrcs"].contains(&ext(f).as_str()) && !crate::project_dirs::is_helper(f)) {
        let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let angles = ["rawtlt", "tlt", "mdoc"].iter().any(|e| f.with_extension(e).exists()) || Path::new(&format!("{}.mdoc", f.display())).exists();
        if angles {
            if let Some(old)=stacks.insert(stem.clone(),f.clone()){if old!=*f {bail!("发现同名倾转叠层 {stem}：{} 和 {}，请缩小采集范围",old.display(),f.display());}}
        }
    }
    for f in files.iter().filter(|f| ext(f) == "mdoc") {
        let name = series_name(f);
        if stacks.contains_key(&name) {continue;}
        if series.iter().any(|s|s.name==name){bail!("存在多个同名采集记录 {name}，请缩小到一个采集目录，避免混合不同序列");}
        let doc=match std::fs::read_to_string(f).map_err(anyhow::Error::from).and_then(|t|crate::motion::series::parse(&t)){Ok(d)=>d,Err(e)=>{warnings.push(format!("{}：{e}",f.display()));continue}};
        let movies = doc.views.iter().filter(|v| crate::motion::series::resolve(f, &v.movie, None).is_some()).count();
        let stack = crate::project_dirs::motion_output(&text(&root.join(format!("{name}.mrc"))));
        let state = std::fs::read(format!("{stack}.motion-series.json")).ok().and_then(|b| serde_json::from_slice::<crate::motion::series::SeriesResult>(&b).ok()).filter(|r| Path::new(&r.mdoc).file_name() == f.file_name());
        let corrected = state.as_ref().map_or(0, |r| r.records.iter().filter(|r| r.section.is_some()).count());
        let complete = state.as_ref().is_some_and(|r| r.complete) && Path::new(&stack).exists();

        let mut s = blank(name, Some(text(f)), stack);
        s.movies = movies.max(corrected);
        if movies==0 && !complete {s.error=Some("记录中没有可唯一匹配的电影，请检查文件名、缺失项和重名文件".into());}
        let expected=doc.views.len();
        if movies < expected { s.note=Some(format!("采集记录共 {expected} 个电影，匹配 {movies} 个；{} 个缺失或匹配不唯一，处理时会保留跳过原因",expected-movies)); }
        s.corrected = corrected;
        s.small = exists(&state.and_then(|r| r.small));
        s.pixel_a = doc.pixel_a;
        s.size = doc.image_size;
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
        s.error = known.errors.get(&s.name).cloned().or(s.error.clone());
        s.note = known.notes.get(&s.name).cloned().or(s.note.clone());
        s.chosen = s.take.unwrap_or(s.verdict.as_ref().is_none_or(|v| v.usable));
    }
    Ok(Session { root: text(&root), series, warnings, gains:index.gains().iter().map(|p|text(p)).collect() })
}

fn blank(name: String, mdoc: Option<String>, stack: String) -> Series {
    Series { name, mdoc, stack, stage: Stage::Movies, movies: 0, corrected: 0, views: 0, left_out: 0, verdict: None, take: None, chosen: true, residual: None, quality: None, small: None, aligned: None, quick: None, result: None, done: vec![], error: None, note: None, pixel_a: None, size: None, bin: None }
}

/// What the dataset of an existing stack says about it.
fn fill(s: &mut Series) {
    s.stage = Stage::Stack;
    if let Ok(img) = pipeline::open(&s.stack) {
        let i = img.info();
        s.size = Some([i.width, i.height]);
        if i.pixel_size.is_some() {
            s.pixel_a = Some(crate::align::pixel_a(i));
        }
    }
    let file = super::dataset::path_for(&s.stack);
    let Some(d) = std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Dataset>(&b).ok()) else { return };
    s.views = d.angles.len();
    s.left_out = match &d.alignment {
        Some(a) => a.views.iter().filter(|v| v.excluded).count(),
        None => super::triage::excluded(&d).len(),
    };
    s.pixel_a = d.optics.pixel_a.or(s.pixel_a);
    s.verdict = d.verdict.clone();
    s.take = d.take;
    s.aligned = exists(&d.aligned);
    s.quick = exists(&d.made.get("survey").cloned());
    let result = ["display", "sta"].into_iter().find_map(|k| exists(&d.made.get(k).cloned()).map(|p| (k, p)));
    s.done = result.as_ref().and_then(|(k, _)| d.made_steps.get(*k).cloned()).unwrap_or_default();
    s.result = result.map(|(_, p)| p);
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
    let shown = s.result.as_ref().or(s.quick.as_ref());
    // A clean companion (N2N, deconvolved) is not listed itself: it has its full tomogram's binning.
    s.bin = shown.and_then(|p| d.tomograms.iter().find(|t| &t.path == p).or_else(|| d.tomograms.iter().rev().find(|t| t.kind == "full")).map(|t| t.bin));
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

/// A small copy of a whole tomogram for looking through it on another
/// computer (the review's 断层图 tab over SSH): every section box-averaged to
/// at most `max` pixels a side, as bytes (one contrast for all sections, from
/// the middle one), in an MRC file in this computer's cache. Made once per
/// file version; answers its path.
pub fn preview_stack(path: &str, max: usize) -> Result<String> {
    use sha2::Digest;
    use std::io::Write;
    let meta = std::fs::metadata(path)?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let key = format!("{path}|{}|{mtime}|{max}", meta.len());
    let hash: String = sha2::Sha256::digest(key.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect();
    let dir = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("tifmrcview/previews");
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{hash}.mrc"));
    if out.exists() {
        return Ok(text(&out));
    }
    let img = pipeline::open(path)?;
    let info = img.info();
    let (w, h, d) = (info.width, info.height, info.size_z);
    let f = w.max(h).div_ceil(max.clamp(64, 2048)).max(1);
    let (sw, sh) = (w / f, h / f);
    anyhow::ensure!(sw > 0 && sh > 0 && d > 0, "断层图太小");
    let section = |z: usize| -> Result<Vec<f32>> {
        let (page, sample) = info.locate(0, z, 0);
        let p = img.page(page)?;
        let p = &p[sample.min(p.len() - 1)];
        let mut o = vec![0f32; sw * sh];
        for y in 0..sh {
            for x in 0..sw {
                let mut acc = 0f64;
                for dy in 0..f {
                    let row = (y * f + dy) * p.w;
                    for dx in 0..f {
                        acc += p.data.get_f64(row + x * f + dx);
                    }
                }
                o[y * sw + x] = (acc / (f * f) as f64) as f32;
            }
        }
        Ok(o)
    };
    let mut mid = section(d / 2)?;
    mid.retain(|v| v.is_finite());
    anyhow::ensure!(!mid.is_empty(), "断层图中间一层是空的");
    mid.sort_by(f32::total_cmp);
    let (lo, hi) = (mid[mid.len() / 200], mid[mid.len() - 1 - mid.len() / 200]);
    let scale = if hi > lo { 255.0 / (hi - lo) } else { 1.0 };
    let tmp = dir.join(format!("{hash}.part"));
    let mut file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    let mut head = vec![0u8; 1024];
    let mut put = |at: usize, v: [u8; 4]| head[at..at + 4].copy_from_slice(&v);
    for (i, n) in [sw, sh, d, 0, 0, 0, 0, sw, sh, d].iter().enumerate() {
        put(i * 4, (*n as i32).to_le_bytes());
    }
    let px = if info.pixel_size.is_some() { crate::align::pixel_a(info) * f as f64 } else { 1.0 };
    for (i, n) in [sw, sh, d].iter().enumerate() {
        put(40 + i * 4, ((*n as f64 * px) as f32).to_le_bytes());
        put(52 + i * 4, 90f32.to_le_bytes());
        put(64 + i * 4, ((i + 1) as i32).to_le_bytes());
    }
    put(76, 0f32.to_le_bytes());
    put(80, 255f32.to_le_bytes());
    put(84, 127f32.to_le_bytes());
    put(152, 1146047817i32.to_le_bytes());
    put(156, 0i32.to_le_bytes());
    head[208..212].copy_from_slice(b"MAP ");
    head[212..216].copy_from_slice(&[0x44, 0x44, 0, 0]);
    file.write_all(&head)?;
    for z in 0..d {
        super::check_cancel()?;
        let v = section(z)?;
        let bytes: Vec<u8> = v.iter().map(|x| if x.is_finite() { ((x - lo) * scale).round().clamp(0.0, 255.0) as u8 } else { 0 }).collect();
        file.write_all(&bytes)?;
    }
    file.flush()?;
    drop(file);
    std::fs::rename(&tmp, &out)?;
    Ok(text(&out))
}

fn cancelled(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains("已取消")
}

/// One series through `work`; a failure is noted and does not stop the rest.
/// A series that failed for want of memory (a server busy with something
/// else, a Windows disk under WSL refusing a big write) is not given up: its
/// turn waits until memory is free again and it goes on from where it
/// stopped (the steps done are kept), a few times; and every series still
/// failed when the others are through is tried once more, as its card's
/// 「继续」 would, so the user need not press it.
fn each(
    folder: &str,
    wanted: &dyn Fn(&Series) -> bool,
    progress: pipeline::Progress,
    changed: &(dyn Fn() + Sync),
    work: &dyn Fn(&Series, pipeline::Progress) -> Result<Option<String>>,
) -> Result<Session> {
    let session = scan(folder)?;
    let root = PathBuf::from(&session.root);
    let todo: Vec<&Series> = session.series.iter().filter(|s| wanted(s)).collect();
    let mut failed = vec![];
    for (i, s) in todo.iter().enumerate() {
        let label = format!("{}（{}/{}）", s.name, i + 1, todo.len());
        if !one(&root, s, &label, MEMORY_TRIES, progress, changed, work)? {
            failed.push(*s);
        }
    }
    for (i, s) in failed.iter().enumerate() {
        let label = format!("{}（再试失败的 {}/{}）", s.name, i + 1, failed.len());
        one(&root, s, &label, 1, progress, changed, work)?;
    }
    scan(folder)
}

/// Times a series is tried in its turn while it fails for want of memory.
const MEMORY_TRIES: usize = 4;

/// `s` through `work`, again (up to `tries` in all) while it fails for want
/// of memory, after the memory is free again. Whether it went through; Err
/// only when cancelled.
fn one(
    root: &Path,
    s: &Series,
    label: &str,
    tries: usize,
    progress: pipeline::Progress,
    changed: &(dyn Fn() + Sync),
    work: &dyn Fn(&Series, pipeline::Progress) -> Result<Option<String>>,
) -> Result<bool> {
    for k in 1..=tries.max(1) {
        super::check_cancel()?;
        note(root, &s.name, None);
        let r = work(s, &|stage, a, b| progress(&format!("{label} · {stage}"), a, b));
        release_memory();
        match r {
            Ok(said) => {
                remark(root, &s.name, said, false);
                changed();
                return Ok(true);
            }
            Err(e) if cancelled(&e) => return Err(e),
            Err(e) if k < tries && short_of_memory(&e) => {
                note(root, &s.name, Some(format!("内存不够（{e:#}），等内存空出来后自动接着做（第 {k} 次）")));
                changed();
                wait_for_memory(&|left| progress(&format!("{label} · 内存不够，等内存空出来后接着做（第 {k} 次重试，最多再等 {left} 秒）"), 0, 1))?;
            }
            Err(e) => {
                note(root, &s.name, Some(format!("{e:#}")));
                changed();
                return Ok(false);
            }
        }
    }
    Ok(false)
}

/// Whether `e` came from the system (or the GPU) being short of memory.
fn short_of_memory(e: &anyhow::Error) -> bool {
    if e.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some_and(crate::bigbuf::short_of_memory)) {
        return true;
    }
    let t = format!("{e:#}").to_lowercase();
    ["cannot allocate memory", "os error 12", "out of memory", "outofmemory", "memory allocation", "内存不足", "内存不够", "显存不足"].iter().any(|k| t.contains(k))
}

/// Gives the memory freed by the series before back to the system (the
/// allocator keeps freed pages for itself otherwise, and a long batch's
/// process grows until the system has none left for anyone).
fn release_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Waits (at most five minutes; `waiting` is told the seconds left) until
/// 8 GB of memory are free or nothing more is being given back.
fn wait_for_memory(waiting: &dyn Fn(u64)) -> Result<()> {
    const MOST: u64 = 300;
    let start = std::time::Instant::now();
    let mut last = 0u64;
    let mut still = 0;
    loop {
        super::check_cancel()?;
        release_memory();
        let free = crate::resources::memory_available_bytes();
        let waited = start.elapsed().as_secs();
        if free == 0 || waited >= MOST || free >= 8 << 30 {
            return Ok(());
        }
        // Not freeing any more for half a minute: try again anyway (the
        // steps go slower, in smaller pieces, when memory is short).
        still = if free <= last + (64 << 20) { still + 1 } else { 0 };
        if waited >= 10 && still >= 6 {
            return Ok(());
        }
        last = free;
        waiting(MOST - waited);
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
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
    let _index=crate::acquisition::scope(Path::new(folder))?;
    each(folder, &|s| if only.is_empty() { s.stage != Stage::Quick && s.stage != Stage::Done } else { only.contains(&s.name) }, progress, changed, &|s, progress| {
        let stack = stack_of(s, true, progress)?;
        changed();
        let _turn=if std::env::var_os("TMV_PARENT_GPU_LOCK_DEFERRED").is_some(){crate::cli::gpu_turn()}else{None};
        let mut d = dataset(&stack)?;
        // A series the program would not use is not aligned for nothing;
        // the user who takes it gets it with the next run.
        if d.checks.len() != d.angles.len() {
            super::triage::survey(&pipeline::open(&stack)?, &mut d)?;
            d.save()?;
        }
        if !d.take.unwrap_or(d.verdict.as_ref().is_none_or(|v| v.usable)) {
            return Ok(None);
        }
        let req = auto::AutoReq { pixel_a: None, purpose: auto::Purpose::Survey, fresh: false, membranes: false };
        Ok(crate::ctf_gpu::guarded(|| auto::run(dev, &mut d, &req, progress))??.note)
    })
}

/// The tomograms for `purpose` of every series the user (else the program) chose.
pub fn finish(dev: &burn::tensor::Device, folder: &str, purpose: auto::Purpose, only: &[String], progress: pipeline::Progress, changed: &(dyn Fn() + Sync)) -> Result<Session> {
    let _index=crate::acquisition::scope(Path::new(folder))?;
    each(folder, &|s| s.chosen && (only.is_empty() || only.contains(&s.name)), progress, changed, &|s, progress| {
        // Averaging needs the views in full (every movie corrected at its
        // own pixel, local motion and dose weighting); what was made for
        // looking is enough for a tomogram to show.
        let stack = stack_of(s, purpose != auto::Purpose::Sta, progress)?;
        let _turn=if std::env::var_os("TMV_PARENT_GPU_LOCK_DEFERRED").is_some(){crate::cli::gpu_turn()}else{None};
        let mut d = dataset(&stack)?;
        // A series not looked at before (its movies came in with this run) is
        // judged first, as the first pass does: one the program would not use
        // is not reconstructed for nothing unless the user chose it. Ticking
        // its card again and reconstructing takes it (`tomo_take`).
        if d.checks.len() != d.angles.len() {
            super::triage::survey(&pipeline::open(&stack)?, &mut d)?;
            d.save()?;
            if !d.take.unwrap_or(d.verdict.as_ref().is_none_or(|v| v.usable)) {
                return Ok(Some("程序判断这个序列不能用，已跳过；仍要重构就再点一次「重构勾选序列」".into()));
            }
        }
        let req = auto::AutoReq { pixel_a: None, purpose, fresh: false, membranes: false };
        Ok(crate::ctf_gpu::guarded(|| auto::run(dev, &mut d, &req, progress))??.note)
    })
}

/// 「重置」 of one series (its card): what the Cryo-ET steps made of it is
/// removed, so its next run starts from its movies (or its tilt series) again.
/// Only the step folders of the intermediate results are looked in
/// (`project_dirs` 1–3: motion-corrected stack, dataset, tomograms, clean
/// companions); the raw data is never touched, and averages (4_sta) and
/// figures (5_figures), results of their own, are kept. A file there belongs to
/// the series whose name it starts with, followed by "." or "_" (TS_1.mrc,
/// TS_1_even.mrc, TS_1.tomo.json); when several names fit (TS_1 and TS_1_b),
/// the longest one owns it. On a server the step folders lie under the job's
/// output root (`TMV_OUTPUT_ROOT`). A project of the old flat layout (datasets
/// beside the raw stacks) is refused: its intermediate files cannot be told
/// from the user's own by folder. Answers the files removed.
pub fn reset(folder: &str, name: &str) -> Result<Vec<String>> {
    use anyhow::Context;
    use crate::project_dirs::{CLEAN, MOTION, TOMOGRAMS};
    let dir = Path::new(folder);
    anyhow::ensure!(dir.is_dir(), "找不到项目文件夹：{folder}");
    let legacy = std::fs::read_dir(dir)?.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".tomo.json"));
    anyhow::ensure!(!legacy, "这是旧版的项目（中间结果和原始数据放在同一个文件夹里），不能一键清理，请手动删除");
    let names: Vec<String> = scan(folder)?.series.into_iter().map(|s| s.name).collect();
    anyhow::ensure!(names.iter().any(|n| n == name), "这个文件夹里没有序列 {name}");
    let root = crate::project_dirs::listing_root(dir).unwrap_or_else(|| dir.to_path_buf());
    let out = std::env::var_os("TMV_OUTPUT_ROOT").map(PathBuf::from).unwrap_or_else(|| root.clone());
    let owner = |file: &str| names.iter().filter(|n| file == n.as_str() || file.strip_prefix(n.as_str()).is_some_and(|r| r.starts_with('.') || r.starts_with('_'))).max_by_key(|n| n.len()).cloned();
    let mut removed = vec![];
    for step in [MOTION, TOMOGRAMS, CLEAN] {
        let Ok(rd) = std::fs::read_dir(out.join(step)) else { continue };
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            if owner(&file).as_deref() != Some(name) {
                continue;
            }
            let p = e.path();
            if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) }.with_context(|| format!("无法删除 {}", p.display()))?;
            removed.push(text(&p));
        }
    }
    merge_notes(&notes_path(&root), name, None, true);
    merge_notes(&notes_path(&root), name, None, false);
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_stack_is_a_small_byte_copy_of_every_section() {
        let dir = std::env::temp_dir().join(format!("tmv-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("t.mrc");
        let (w, h, d) = (200usize, 100usize, 7usize);
        let data: Vec<f32> = (0..w * h * d).map(|i| (i % w) as f32 + (i / (w * h)) as f32 * 10.0).collect();
        crate::tomo::output::write_mrc(&text(&src), &data, [w, h, d], 5.0, false).unwrap();
        let out = preview_stack(&text(&src), 64).unwrap();
        let img = pipeline::open(&out).unwrap();
        let i = img.info();
        assert_eq!((i.width, i.height, i.size_z), (50, 25, 7));
        assert_eq!(preview_stack(&text(&src), 64).unwrap(), out, "made once");
        let p = &img.page(i.locate(0, 3, 0).0).unwrap()[0];
        let (a, b) = (p.data.get_f64(0), p.data.get_f64(49));
        assert!(a < b && (0.0..=255.0).contains(&a) && (0.0..=255.0).contains(&b), "{a} {b}");
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        assert_eq!(names, [("a", Stage::Movies, 1), ("b", Stage::Stack, 0), ("gone", Stage::Movies, 0)]);
        assert!(s.series[2].error.is_some(), "missing movies must stay visible with a diagnostic");
        assert!(s.series[0].stack.ends_with("1_motion/a.mrc") && s.series.iter().all(|s| s.chosen));
        note(dir.path(), "a", Some("坏了".into()));
        assert_eq!(scan(&text(dir.path())).unwrap().series[0].error.as_deref(), Some("坏了"));
        note(dir.path(), "a", None);
        assert!(scan(&text(dir.path())).unwrap().series[0].error.is_none());
    }

    #[test]
    fn a_reset_removes_what_was_made_of_one_series_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str| dir.path().join(n);
        for n in ["TS_1", "TS_1_b", "TS_10"] {
            std::fs::write(p(&format!("{n}.mrc")), b"x").unwrap();
            std::fs::write(p(&format!("{n}.tlt")), b"0\n").unwrap();
        }
        for d in ["1_motion", "2_tomograms", "3_clean", "4_sta"] {
            std::fs::create_dir(p(d)).unwrap();
        }
        let made = ["2_tomograms/TS_1.tomo.json", "2_tomograms/TS_1_sart3_b8.mrc", "3_clean/TS_1_look.mrc", "2_tomograms/TS_1_b.tomo.json", "2_tomograms/TS_10.tomo.json", "3_clean/TS_10_look.mrc", "4_sta/TS_1_sta.json"];
        for f in made {
            std::fs::write(p(f), b"{}").unwrap();
        }
        let removed = reset(&text(dir.path()), "TS_1").unwrap();
        assert_eq!(removed.len(), 3, "{removed:?}");
        assert!(!p("2_tomograms/TS_1.tomo.json").exists() && !p("3_clean/TS_1_look.mrc").exists());
        for keep in ["2_tomograms/TS_1_b.tomo.json", "2_tomograms/TS_10.tomo.json", "3_clean/TS_10_look.mrc", "4_sta/TS_1_sta.json", "TS_1.mrc", "TS_1.tlt"] {
            assert!(p(keep).exists(), "{keep} must stay");
        }
        assert!(reset(&text(dir.path()), "nope").is_err());
        std::fs::write(p("old.tomo.json"), b"{}").unwrap();
        assert!(reset(&text(dir.path()), "TS_10").is_err(), "an old flat project is refused");
    }
}

#[cfg(test)]mod parallel_notes_tests{use super::*;#[test]fn multi_gpu_notes_merge_without_lost_updates(){let dir=tempfile::tempdir().unwrap();let path=dir.path().join("session.json");std::thread::scope(|scope|{for i in 0..16{let path=&path;scope.spawn(move||merge_notes(path,&format!("TS_{i}"),Some(format!("warning_{i}")),true));}});let n:Notes=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();assert_eq!(n.errors.len(),16);}}
