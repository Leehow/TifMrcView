//! SerialEM batch planning. Pre-exposure follows DateTime (ZValue fallback)
//! including movies absent from the local sample. Only final sums are sorted
//! by tilt. Missing source movies are skipped with explicit per-view records.
use super::{check_cancel, output, MotionResult, Params};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub z: usize,
    pub angle: f64,
    pub movie: String,
    pub dose: Option<f64>,
    pub pixel_a: Option<f64>,
    pub date_time: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub views: Vec<View>,
    pub pixel_a: Option<f64>,
    pub kv: Option<f64>,
    /// ImageSize of the recorded (often binned) images that PixelSpacing
    /// describes; the frames may be larger (unbinned or super-resolution).
    pub image_size: Option<[usize; 2]>,
}

impl Doc {
    /// PixelSpacing for frames of this size: the mdoc's spacing is for its
    /// ImageSize, so frames 8× as wide have 1/8 of it.
    pub fn frame_pixel(&self, spacing: f64, frame: (usize, usize)) -> f64 {
        let Some([w, h]) = self.image_size.filter(|[w, h]| *w > 0 && *h > 0) else { return spacing };
        let r = frame.0.max(frame.1) as f64 / w.max(h) as f64;
        if (0.9..=1.1).contains(&r) {
            return spacing;
        }
        // Frames are usually a power-of-two binning apart (maybe cropped a little).
        let p = 2f64.powf(r.log2().round());
        spacing / if (r / p - 1.0).abs() < 0.05 { p } else { r }
    }
}
fn number(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}
pub fn parse(text: &str) -> Result<Doc> {
    let mut d = Doc::default();
    let mut current = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            current = None;
            if let Some((_, v)) = line.strip_prefix("[ZValue").and_then(|l| l.split_once('=')) {
                let z = v.trim().trim_end_matches(']').trim().parse()?;
                d.views.push(View {
                    z,
                    angle: f64::NAN,
                    ..Default::default()
                });
                current = Some(d.views.len() - 1);
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let v = value.trim();
        if let Some(i) = current {
            let view = &mut d.views[i];
            match key.trim() {
                "TiltAngle" => view.angle = number(v).unwrap_or(f64::NAN),
                "SubFramePath" => view.movie = v.to_string(),
                "ExposureDose" => view.dose = number(v).filter(|v| *v >= 0.0),
                "PixelSpacing" => view.pixel_a = number(v).filter(|v| *v > 0.0),
                "DateTime" => view.date_time = Some(v.into()),
                _ => {}
            }
        } else {
            match key.trim() {
                "PixelSpacing" => d.pixel_a = number(v).filter(|v| *v > 0.0),
                "Voltage" => d.kv = number(v),
                "ImageSize" => {
                    let n: Vec<usize> = v.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                    if n.len() == 2 {
                        d.image_size = Some([n[0], n[1]]);
                    }
                }
                _ => {}
            }
        }
    }
    if d.views.is_empty() || d.views.iter().any(|v| !v.angle.is_finite()) {
        bail!("mdoc 缺少 ZValue/TiltAngle");
    }
    let mut zs = std::collections::HashSet::new();
    if d.views.iter().any(|v| !zs.insert(v.z)) {
        bail!("mdoc ZValue 重复");
    }
    if d.views
        .iter()
        .all(|v| v.date_time.as_deref().and_then(date_key).is_some())
    {
        d.views
            .sort_by_key(|v| (date_key(v.date_time.as_deref().unwrap()).unwrap(), v.z));
    } else {
        d.views.sort_by_key(|v| v.z);
    }
    Ok(d)
}
fn date_key(s: &str) -> Option<[u32; 6]> {
    let p: Vec<_> = s.split_whitespace().collect();
    let date: Vec<_> = p.first()?.split('-').collect();
    let time: Vec<_> = p.get(1)?.split(':').collect();
    if date.len() != 3 || time.len() != 3 {
        return None;
    }
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| m.eq_ignore_ascii_case(date[1]))? as u32
        + 1;
    let mut year = date[2].parse::<u32>().ok()?;
    if year < 100 {
        year += 2000;
    }
    Some([
        year,
        month,
        date[0].parse().ok()?,
        time[0].parse().ok()?,
        time[1].parse().ok()?,
        time[2].parse().ok()?,
    ])
}
fn basename(s: &str) -> &str {
    s.rsplit(['/', '\\']).next().unwrap_or(s)
}
impl Doc {
    pub fn for_movie(&self, path: &str) -> Option<(&View, f64)> {
        let mut pre = 0.0;
        for v in &self.views {
            if basename(&v.movie) == basename(path) {
                return Some((v, pre));
            }
            pre += v.dose.unwrap_or(0.0);
        }
        None
    }
}
pub fn resolve(mdoc: &Path, movie: &str, folder: Option<&Path>) -> Option<PathBuf> {
    let parent = mdoc.parent()?;
    let name = basename(movie);
    if name.is_empty() {
        return None;
    }
    let mut dirs = vec![parent.to_path_buf()];
    if let Some(p) = parent.parent() {
        dirs.push(p.to_path_buf());
    }
    if let Some(p) = folder {
        dirs.insert(0, p.to_path_buf());
    }
    for d in dirs {
        for sub in ["", "eer", "frames", "Frames", "EER"] {
            let p = d.join(sub).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The gain reference beside the movies, when there is exactly one: a
/// `.gain` file (Falcon EER) or an MRC/TIFF whose name says gain or
/// CountRef, in the folder of the first movie, the mdoc's folder, `folder`,
/// or a folder named for gain beside them (CryoET Data Portal: `Gains/`).
pub fn find_gain(mdoc: &Path, movie: Option<&Path>, folder: Option<&Path>) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = [movie.and_then(Path::parent), mdoc.parent(), folder].into_iter().flatten().map(Path::to_path_buf).collect();
    let near: Vec<PathBuf> = dirs.iter().flat_map(|d| [Some(d.clone()), d.parent().map(Path::to_path_buf)]).flatten().collect();
    for d in near {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
            if p.is_dir() && name.contains("gain") {
                dirs.push(p);
            }
        }
    }
    dirs.sort();
    dirs.dedup();
    let mut found: Vec<PathBuf> = vec![];
    for d in dirs {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
            let ext = p.extension().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
            let named = name.contains("gain") || name.contains("countref");
            if p.is_file() && (ext == "gain" || (named && matches!(ext.as_str(), "mrc" | "tif" | "tiff"))) && !found.contains(&p) {
                found.push(p);
            }
        }
    }
    (found.len() == 1).then(|| found.remove(0))
}

/// Bounded, local sidecar discovery. Do not search the user's whole disk.
pub fn find_mdoc(movie: &str) -> Option<String> {
    let parent = Path::new(movie).parent()?;
    let mut dirs = vec![
        parent.to_path_buf(),
        parent.join("mdocfolder"),
        parent.join("mdoc"),
    ];
    if let Some(p) = parent.parent() {
        dirs.push(p.join("mdocfolder"));
        dirs.push(p.join("mdoc"));
        dirs.push(p.to_path_buf());
    }
    for dir in dirs {
        let Ok(files) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<_> = files
            .filter_map(|f| f.ok().map(|f| f.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("mdoc"))
            })
            .collect();
        files.sort();
        for p in files.into_iter().take(128) {
            if std::fs::metadata(&p)
                .ok()
                .is_none_or(|m| m.len() > 16 << 20)
            {
                continue;
            }
            if std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| parse(&s).ok())
                .is_some_and(|d| d.for_movie(movie).is_some())
            {
                return Some(p.to_string_lossy().into());
            }
        }
    }
    None
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesReq {
    pub mdoc: String,
    pub folder: Option<String>,
    pub output: String,
    pub params: Params,
    /// Kept for older callers; a tilt's own files are no longer written.
    #[serde(default)]
    pub drop_tilts: bool,
    /// Start over even when part of the series was corrected before.
    #[serde(default)]
    pub fresh: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub z: usize,
    pub angle: f64,
    pub movie: String,
    pub pre_exposure: f64,
    pub dose: Option<f64>,
    pub skipped: Option<String>,
    pub result: Option<MotionResult>,
    /// Where the view lies in the stacks (None: not in them).
    #[serde(default)]
    pub section: Option<usize>,
    /// Mean and opaque part of the view (`tomo::triage::measure`), and the
    /// quick CTF fit of the whole view: defocus (µm), ring resolution (Å).
    #[serde(default)]
    pub mean: f64,
    #[serde(default)]
    pub dark: f64,
    #[serde(default)]
    pub defocus_um: Option<f64>,
    #[serde(default)]
    pub fit_res_a: Option<f64>,
    /// Least, greatest, sum and sum of squares of the view in the full,
    /// even and odd stacks (for their headers).
    #[serde(default)]
    stats: Vec<[f64; 4]>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesResult {
    pub path: Option<String>,
    pub even: Option<String>,
    pub odd: Option<String>,
    /// The views at a small size (`SMALL` pixels), same sections.
    #[serde(default)]
    pub small: Option<String>,
    pub angles: Vec<f64>,
    pub records: Vec<Record>,
    pub warnings: Vec<String>,
    /// The mdoc this was made from, and whether every movie is done: a run
    /// that was stopped goes on from the records of the views it finished.
    #[serde(default)]
    pub mdoc: String,
    #[serde(default)]
    pub complete: bool,
    /// Size of the views and how many the stacks were laid out for.
    #[serde(default)]
    pub shape: [usize; 3],
    #[serde(default)]
    pub pixel_a: f64,
}

/// Longer side of the small copies of the views, pixels.
pub const SMALL: usize = 512;

/// A view box-averaged to at most `SMALL` pixels along its longer side.
pub fn shrink(v: &[f32], w: usize, h: usize) -> (Vec<f32>, usize, usize) {
    let bin = w.max(h).div_ceil(SMALL).max(1);
    let (sw, sh) = ((w / bin).max(1), (h / bin).max(1));
    let mut out = vec![0f32; sw * sh];
    use rayon::prelude::*;
    out.par_chunks_mut(sw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut sum = 0.0;
            for dy in 0..bin {
                let at = (y * bin + dy) * w + x * bin;
                sum += v[at..at + bin].iter().sum::<f32>();
            }
            *o = sum / (bin * bin) as f32;
        }
    });
    (out, sw, sh)
}

/// The records of an earlier, unfinished run of the same mdoc into the same
/// stacks, when its files are still as it left them.
fn earlier(state: &Path, req: &SeriesReq, sections: usize) -> Option<SeriesResult> {
    let prior: SeriesResult = serde_json::from_slice(&std::fs::read(state).ok()?).ok()?;
    let [w, h, n] = prior.shape;
    let same = prior.mdoc == req.mdoc && (prior.complete || n == sections) && w > 0 && h > 0;
    let files = [Some(req.output.clone()), req.params.even_odd.then(|| output::sibling(&req.output, "even")), req.params.even_odd.then(|| output::sibling(&req.output, "odd"))];
    let kept = prior.records.iter().filter(|r| r.section.is_some()).count();
    let n = if prior.complete { kept } else { n };
    (same && n > 0 && files.iter().flatten().all(|f| output::Sections::fits(f, w, h, n))).then_some(prior)
}

/// Motion correction of every movie of an mdoc into a tilt series, a view
/// at a time in the order they were recorded (the untilted view of a
/// dose-symmetric series comes first), each written straight to its place
/// in the stacks (sorted by tilt) and shown (`crate::live`). What was
/// measured of each view is kept as it is done, so a run that was stopped
/// goes on where it was.
pub fn run(
    req: &SeriesReq,
    progress: &(dyn Fn(&str, usize, usize) + Sync),
) -> Result<SeriesResult> {
    req.params.validate()?;
    let doc = parse(&std::fs::read_to_string(&req.mdoc)?)?;
    let mut req = req.clone();
    let mut found_gain = None;
    if req.params.gain_path.is_none() {
        let first = doc.views.iter().find_map(|v| resolve(Path::new(&req.mdoc), &v.movie, req.folder.as_deref().map(Path::new)));
        if let Some(g) = find_gain(Path::new(&req.mdoc), first.as_deref(), req.folder.as_deref().map(Path::new)) {
            req.params.gain_path = Some(g.to_string_lossy().into_owned());
            found_gain = Some(g);
        }
    }
    // A tilt series made from frames goes to `1_motion/` of the project (the
    // frames' folder), its halves and the run's records with it; an old flat
    // project (its dataset beside the stack) stays flat.
    req.output = crate::project_dirs::motion_output(&req.output);
    let dir = Path::new(&req.output).parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
    std::fs::create_dir_all(&dir)?;
    let req = &req;
    output::protect(&req.mdoc, &req.output)?;
    let movies: Vec<Option<PathBuf>> = doc.views.iter().map(|v| resolve(Path::new(&req.mdoc), &v.movie, req.folder.as_deref().map(Path::new))).collect();
    let (even_path, odd_path, small_path) = (output::sibling(&req.output, "even"), output::sibling(&req.output, "odd"), output::sibling(&req.output, "small"));
    for path in movies.iter().flatten() {
        for out in [&req.output, &even_path, &odd_path, &small_path] {
            output::protect(path.to_str().context("路径编码无效")?, out)?;
        }
    }
    let unknown =
        doc.views.iter().any(|v| v.dose.unwrap_or(0.0) <= 0.0) && req.params.total_dose.is_none();
    let mut warnings = vec![];
    if let Some(g) = &found_gain {
        warnings.push(format!("使用找到的增益参考 {}", g.file_name().and_then(|n| n.to_str()).unwrap_or("")));
    }
    if unknown && req.params.dose_weighted {
        warnings.push(
            "mdoc 含未知/零剂量，累计前曝光不可信；此批不应用剂量加权，请指定每倾角总剂量".into(),
        );
    }
    // Where each view with a movie lies in the stacks: sorted by tilt.
    let mut planned: Vec<usize> = (0..doc.views.len()).filter(|&i| movies[i].is_some()).collect();
    planned.sort_by(|&a, &b| doc.views[a].angle.total_cmp(&doc.views[b].angle));
    let section_of = |i: usize| planned.iter().position(|&p| p == i);
    let state = PathBuf::from(format!("{}.motion-series.json", req.output));
    let prior = if req.fresh { None } else { earlier(&state, req, planned.len()) };
    if let Some(p) = prior.as_ref().filter(|p| p.complete) {
        progress("批次完成", doc.views.len(), doc.views.len());
        return Ok(SeriesResult { records: p.records.clone(), warnings: p.warnings.clone(), angles: p.angles.clone(), path: p.path.clone(), even: p.even.clone(), odd: p.odd.clone(), small: p.small.clone(), mdoc: p.mdoc.clone(), complete: true, shape: p.shape, pixel_a: p.pixel_a });
    }
    let done = |v: &View| prior.as_ref().and_then(|p| p.records.iter().find(|r| r.z == v.z && r.movie == v.movie && r.result.is_some() && r.section.is_some()).cloned());
    let mut result = SeriesResult { warnings, mdoc: req.mdoc.clone(), ..Default::default() };
    if let Some(p) = &prior {
        result.shape = p.shape;
        result.pixel_a = p.pixel_a;
    }
    struct Stacks {
        full: output::Sections,
        halves: Option<[output::Sections; 2]>,
        small: output::Sections,
    }
    let mut stacks: Option<Stacks> = None;
    let open = |w: usize, h: usize, px: f64, keep: bool| -> Result<Stacks> {
        let n = planned.len();
        let (_, sw, sh) = shrink(&vec![0.0; w * h], w, h);
        Ok(Stacks {
            full: output::Sections::open(&req.output, w, h, n, px, keep)?,
            halves: if req.params.even_odd { Some([output::Sections::open(&even_path, w, h, n, px, keep)?, output::Sections::open(&odd_path, w, h, n, px, keep)?]) } else { None },
            small: output::Sections::open(&small_path, sw, sh, n, px * (w as f64 / sw as f64), keep && output::Sections::fits(&small_path, sw, sh, n))?,
        })
    };
    let save = |r: &SeriesResult, records: &[Record]| -> Result<()> {
        let mut v = serde_json::to_value(r)?;
        v["records"] = serde_json::to_value(records)?;
        output::text(&state, &serde_json::to_string(&v)?)
    };
    // What each view is corrected with, and its record so far.
    let mut pre = req.params.pre_exposure;
    let mut plan: Vec<(Record, Option<Params>)> = vec![];
    for (i, v) in doc.views.iter().enumerate() {
        let dose = req.params.total_dose.or(v.dose);
        let mut r = Record { z: v.z, angle: v.angle, movie: v.movie.clone(), pre_exposure: pre, dose, skipped: None, result: None, section: None, mean: 0.0, dark: 0.0, defocus_um: None, fit_res_a: None, stats: vec![] };
        pre += dose.unwrap_or(0.0).max(0.0);
        if let Some(earlier) = done(v) {
            plan.push((earlier, None));
            continue;
        }
        if movies[i].is_none() {
            r.skipped = Some("电影文件不存在".into());
            plan.push((r, None));
            continue;
        }
        let mut p = req.params.clone();
        if p.total_dose.is_none() && p.dose_per_frame.is_none() {
            p.total_dose = dose.filter(|v| *v > 0.0);
        }
        if p.pixel_a.is_none() {
            p.pixel_a = v.pixel_a.or(doc.pixel_a);
            p.mdoc_size = doc.image_size;
        }
        if p.voltage_auto {
            p.voltage_kv = doc.kv.unwrap_or(p.voltage_kv);
        }
        p.pre_exposure = r.pre_exposure;
        if unknown {
            p.dose_weighted = false;
        }
        // A tilt image holds a few electrons per Å²: its patches cannot
        // carry the local model's 18 terms a component, only the whole
        // frame's shift can be measured.
        if p.total_dose.is_some_and(|d| d < LOCAL_DOSE) {
            p.local = false;
        }
        plan.push((r, Some(p)));
    }
    // Two movies are under way at a time: the next one is read and decoded
    // (processor and disk) while the one before is measured and summed (GPU).
    let mut records: Vec<Record> = vec![];
    let (plan, movies, dir) = (&plan, &movies, &dir);
    std::thread::scope(|scope| -> Result<()> {
        let (send, read) = std::sync::mpsc::sync_channel::<(usize, Result<super::Prepared>)>(0);
        scope.spawn(move || {
            for (i, (_, p)) in plan.iter().enumerate() {
                let (Some(p), Some(path)) = (p, &movies[i]) else { continue };
                if check_cancel().is_err() {
                    break;
                }
                let prepared = path.to_str().context("电影路径编码无效").and_then(|path| super::prepare(path, None, dir, 0, 0, 0, p, vec![], &|_, _, _| {}));
                if send.send((i, prepared)).is_err() {
                    break;
                }
            }
        });
        for (i, (r, p)) in plan.iter().enumerate() {
            check_cancel()?;
            let v = &doc.views[i];
            let label = format!("运动校正 {}/{} · {:+.0}°", i + 1, doc.views.len(), v.angle);
            progress(&label, i, doc.views.len());
            let mut r = r.clone();
            let Some(p) = p else {
                records.push(r);
                continue;
            };
            let (at, prepared) = read.recv().map_err(|_| anyhow::anyhow!("已取消"))?;
            debug_assert_eq!(at, i);
            let corrected = prepared.and_then(|m| super::finish(m, None, p, &|stage, a, b| progress(&format!("{label} · {stage}"), a, b)));
            match corrected {
                Ok(super::Corrected { mut sums, result: m }) => {
                    let (mut w, mut h, mut px) = (m.width, m.height, m.pixel_a);
                    // Super-resolution frames carry little beyond the
                    // detector's own pixel: the views are kept at that.
                    if super_resolution(w, h) {
                        for v in [Some(&mut sums.unweighted), sums.weighted.as_mut(), Some(&mut sums.even), Some(&mut sums.odd)].into_iter().flatten() {
                            if !v.is_empty() {
                                *v = halve(v, w, h);
                            }
                        }
                        (w, h, px) = (w / 2, h / 2, px * 2.0);
                    }
                    if stacks.is_none() {
                        if prior.is_some() && (result.shape[0] != w || result.shape[1] != h) {
                            bail!("批次电影尺寸与之前校正的不一致，请重新开始");
                        }
                        stacks = Some(open(w, h, px, prior.is_some())?);
                        result.shape = [w, h, planned.len()];
                        result.pixel_a = px;
                    }
                    if result.shape[0] != w || result.shape[1] != h || (px - result.pixel_a).abs() > 1e-5 {
                        bail!("批次电影尺寸或像素标定不一致");
                    }
                    let st = stacks.as_mut().unwrap();
                    let section = section_of(i).unwrap();
                    let view = sums.weighted.as_deref().unwrap_or(&sums.unweighted);
                    r.stats.push(st.full.write(section, view)?);
                    if let Some([even, odd]) = &mut st.halves {
                        r.stats.push(even.write(section, &sums.even)?);
                        r.stats.push(odd.write(section, &sums.odd)?);
                    }
                    let (small, sw, sh) = shrink(view, w, h);
                    st.small.write(section, &small)?;
                    (r.mean, r.dark) = crate::tomo::triage::measure(&sums.unweighted, w, h);
                    if let Some(f) = quick_ctf(&sums.unweighted, w, h, px, p.voltage_kv) {
                        r.defocus_um = Some(0.5 * (f.df1 + f.df2) / 1e4);
                        r.fit_res_a = Some(f.fit_res);
                    }
                    crate::live::show(&format!("{} · {:+.0}° · 漂移 {:.1} Å{}", name_of(&req.output), v.angle, m.total_drift * m.pixel_a, r.defocus_um.map_or(String::new(), |d| format!(" · 离焦 {d:.1} µm"))), sw, sh, &small, &[]);
                    r.section = Some(section);
                    r.result = Some(m);
                }
                Err(e) => {
                    check_cancel()?;
                    r.skipped = Some(format!("处理失败：{e:#}"));
                }
            }
            records.push(r);
            save(&result, &records)?;
        }
        Ok(())
    })?;
    // The stacks hold a place for every movie found; the ones that could
    // not be corrected are closed up.
    let mut good: Vec<usize> = (0..records.len()).filter(|&k| records[k].section.is_some()).collect();
    good.sort_by_key(|&k| records[k].section);
    if good.is_empty() {
        result.warnings.push("没有找到可处理的电影，未写空叠层".into());
        result.records = records;
        save(&result, &result.records)?;
        progress("批次完成", doc.views.len(), doc.views.len());
        return Ok(result);
    }
    let [w, h, _] = result.shape;
    let px = result.pixel_a;
    let mut st = match stacks {
        Some(s) => s,
        // Every view came from the earlier run.
        None => open(w, h, px, true)?,
    };
    let keep: Vec<usize> = good.iter().map(|&k| records[k].section.unwrap()).collect();
    if keep.len() < st.full.n {
        st.full.compact(&keep)?;
        st.small.compact(&keep)?;
        if let Some(halves) = &mut st.halves {
            for s in halves {
                s.compact(&keep)?;
            }
        }
        for (section, &k) in good.iter().enumerate() {
            records[k].section = Some(section);
        }
    }
    let stats = |which: usize| -> Vec<[f64; 4]> { good.iter().filter_map(|&k| records[k].stats.get(which).copied()).collect() };
    st.full.finish(&stats(0))?;
    st.small.finish(&[])?;
    let (sw, sh) = (st.small.w, st.small.h);
    let small: Vec<Vec<f32>> = (0..st.small.n).map(|i| st.small.read(i)).collect::<Result<_>>()?;
    if let Some([even, odd]) = &mut st.halves {
        even.finish(&stats(1))?;
        odd.finish(&stats(2))?;
        result.even = Some(even_path.clone());
        result.odd = Some(odd_path.clone());
    }
    drop(st);
    result.angles = good.iter().map(|&k| records[k].angle).collect();
    let tlt: String = result.angles.iter().map(|a| format!("{a:.6}\n")).collect();
    for target in [Some(&req.output), result.even.as_ref(), result.odd.as_ref()].into_iter().flatten() {
        output::text(&Path::new(target).with_extension("tlt"), &tlt)?;
    }
    result.path = Some(req.output.clone());
    result.small = Some(small_path.clone());
    result.shape = [w, h, good.len()];
    // Hand the tilt series on to the Cryo-ET steps: its dataset, with the
    // acquisition's axis and optics (the stack has no mdoc beside it), and
    // what each view shows on its own.
    let mut d = crate::tomo::dataset::Dataset::open(&req.output, good.len(), Some(px))?;
    d.angles = result.angles.clone();
    d.even = result.even.clone();
    d.odd = result.odd.clone();
    d.alignment = None;
    d.ctf = None;
    d.aligned = None;
    d.tomograms.clear();
    d.axis_hint = crate::tomo::geometry::parse_mdoc(&std::fs::read_to_string(&req.mdoc)?).ok().and_then(|m| m.axis);
    if let Some(kv) = doc.kv {
        d.optics.kv = kv;
    }
    d.optics.pixel_a = Some(px);
    let same_dose = good.iter().any(|&k| records[k].dose.is_none_or(|d| d <= 0.0));
    d.checks = good
        .iter()
        .enumerate()
        .map(|(z, &k)| {
            let r = &records[k];
            crate::tomo::triage::ViewCheck { z, angle: r.angle, mean: r.mean, dark: r.dark, dose: if same_dose { None } else { r.dose }, drift_a: r.result.as_ref().map(|m| m.total_drift * px), defocus_um: r.defocus_um, fit_res_a: r.fit_res_a, ..Default::default() }
        })
        .collect();
    crate::tomo::triage::look(&mut d, &small, sw, sh);
    d.save()?;
    result.complete = true;
    result.records = records;
    save(&result, &result.records)?;
    progress("批次完成", doc.views.len(), doc.views.len());
    Ok(result)
}

/// Whether frames of this size are super-resolution ones of a counting
/// camera (K2: 7676 × 7420, K3: 11520 × 8184), twice its pixels a side.
fn super_resolution(w: usize, h: usize) -> bool {
    matches!((w.max(h), w.min(h)), (7676, 7420) | (11520, 8184))
}

/// A view at half its size: the sum of each 2 × 2 pixels.
fn halve(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    use rayon::prelude::*;
    let (hw, hh) = (w / 2, h / 2);
    let mut out = vec![0f32; hw * hh];
    out.par_chunks_mut(hw).enumerate().for_each(|(y, row)| {
        let (a, b) = (&v[2 * y * w..], &v[(2 * y + 1) * w..]);
        for (x, o) in row.iter_mut().enumerate() {
            *o = a[2 * x] + a[2 * x + 1] + b[2 * x] + b[2 * x + 1];
        }
    });
    out
}

/// Dose of a movie (e⁻/Å²) below which only its whole-frame motion is fitted.
const LOCAL_DOSE: f64 = 15.0;

fn name_of(path: &str) -> String {
    Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
}

/// A CTF fit of the whole view as recorded (not yet knowing its tilt axis:
/// the series' own fit, `tomo::ctf`, comes later), to show while the movies
/// are corrected and to tell a blurred view from a sharp one.
fn quick_ctf(v: &[f32], w: usize, h: usize, pixel_a: f64, kv: f64) -> Option<crate::ctf::Fit> {
    let (lo, hi) = crate::ctf::clip_range(|i| v[i], v.len());
    let spectrum = crate::fft::spectrum(|x, y| crate::ctf::clip(v[y * w + x], lo, hi), [0, 0, w, h], 512).ok()?;
    let params = crate::ctf::Params { voltage_kv: kv, cs_mm: 2.7, amp_contrast: 0.07, pixel_a, res_low_a: 30.0, res_high_a: (2.5 * pixel_a).max(5.0), df_min_a: 5000.0, df_max_a: 80000.0, max_astig_a: 2000.0, phase: false, backend: Some("cpu".into()) };
    crate::ctf::fit_spectrum(&spectrum.log, spectrum.n, &params).ok()
}
