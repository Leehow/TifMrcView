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
/// CountRef, in the folder of the first movie, the mdoc's folder or `folder`.
pub fn find_gain(mdoc: &Path, movie: Option<&Path>, folder: Option<&Path>) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = [movie.and_then(Path::parent), mdoc.parent(), folder].into_iter().flatten().map(Path::to_path_buf).collect();
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
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub z: usize,
    pub angle: f64,
    pub movie: String,
    pub pre_exposure: f64,
    pub dose: Option<f64>,
    pub skipped: Option<String>,
    pub result: Option<MotionResult>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesResult {
    pub path: Option<String>,
    pub even: Option<String>,
    pub odd: Option<String>,
    pub angles: Vec<f64>,
    pub records: Vec<Record>,
    pub warnings: Vec<String>,
}
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
    let req = &req;
    output::protect(&req.mdoc, &req.output)?;
    for v in &doc.views {
        if let Some(path) = resolve(
            Path::new(&req.mdoc),
            &v.movie,
            req.folder.as_deref().map(Path::new),
        ) {
            for out in [
                req.output.clone(),
                output::sibling(&req.output, "even"),
                output::sibling(&req.output, "odd"),
            ] {
                output::protect(path.to_str().context("路径编码无效")?, &out)?;
            }
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
    let mut pre = req.params.pre_exposure;
    let mut records = vec![];
    let mut shape: Option<(usize, usize, f64)> = None;
    for (i, v) in doc.views.iter().enumerate() {
        check_cancel()?;
        progress("批次电影", i, doc.views.len());
        let dose = req.params.total_dose.or(v.dose);
        let mut r = Record {
            z: v.z,
            angle: v.angle,
            movie: v.movie.clone(),
            pre_exposure: pre,
            dose,
            skipped: None,
            result: None,
        };
        if let Some(path) = resolve(
            Path::new(&req.mdoc),
            &v.movie,
            req.folder.as_deref().map(Path::new),
        ) {
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
            p.pre_exposure = pre;
            if unknown {
                p.dose_weighted = false;
            }
            let target = output::sibling(&req.output, &format!("tilt-{}", v.z));
            match super::run_movie(
                path.to_str().context("电影路径编码无效")?,
                None,
                &target,
                0,
                0,
                0,
                &p,
                vec![],
                progress,
            ) {
                Ok(result) => {
                    let s = (result.width, result.height, result.pixel_a);
                    if let Some((w, h, px)) = shape {
                        if w != s.0 || h != s.1 || (px - s.2).abs() > 1e-5 {
                            bail!("批次电影尺寸或像素标定不一致");
                        }
                    } else {
                        shape = Some(s);
                    }
                    r.result = Some(result);
                }
                Err(e) => {
                    check_cancel()?;
                    r.skipped = Some(format!("处理失败：{e:#}"));
                }
            }
        } else {
            r.skipped = Some("电影文件不存在".into());
        }
        pre += dose.unwrap_or(0.0).max(0.0);
        records.push(r);
    }
    let mut good: Vec<_> = records.iter().filter(|r| r.result.is_some()).collect();
    good.sort_by(|a, b| a.angle.total_cmp(&b.angle));
    let mut result = SeriesResult {
        path: None,
        even: None,
        odd: None,
        angles: good.iter().map(|r| r.angle).collect(),
        records: vec![],
        warnings,
    };
    if let Some((w, h, px)) = shape {
        let stack = |target: &str, part: &str| -> Result<()> {
            output::write(target, w, h, good.len(), px, |i| {
                let r = good[i].result.as_ref().unwrap();
                let path = match part {
                    "even" => r.even.as_deref(),
                    "odd" => r.odd.as_deref(),
                    _ => Some(r.path.as_str()),
                }
                .context("缺少半叠层")?;
                output::read_plane(path)
            })?;
            output::text(
                &Path::new(target).with_extension("tlt"),
                &good
                    .iter()
                    .map(|r| format!("{:.6}\n", r.angle))
                    .collect::<String>(),
            )
        };
        stack(&req.output, "")?;
        result.path = Some(req.output.clone());
        if req.params.even_odd {
            let a = output::sibling(&req.output, "even");
            let b = output::sibling(&req.output, "odd");
            stack(&a, "even")?;
            stack(&b, "odd")?;
            result.even = Some(a);
            result.odd = Some(b);
        }
        // Hand the tilt series on to the Cryo-ET steps: its dataset, with the
        // acquisition's axis and optics (the stack has no mdoc beside it).
        let mut d = crate::tomo::dataset::Dataset::open(&req.output, good.len(), Some(px))?;
        d.angles = result.angles.clone();
        d.even = result.even.clone();
        d.odd = result.odd.clone();
        d.alignment = None;
        d.ctf = None;
        d.tomograms.clear();
        d.axis_hint = crate::tomo::geometry::parse_mdoc(&std::fs::read_to_string(&req.mdoc)?).ok().and_then(|m| m.axis);
        if let Some(kv) = doc.kv {
            d.optics.kv = kv;
        }
        d.optics.pixel_a = Some(px);
        d.save()?;
    } else {
        result
            .warnings
            .push("没有找到可处理的电影，未写空叠层".into());
    }
    result.records = records;
    output::text(
        Path::new(&format!("{}.motion-series.json", req.output)),
        &serde_json::to_string_pretty(&result)?,
    )?;
    progress("批次完成", doc.views.len(), doc.views.len());
    Ok(result)
}
