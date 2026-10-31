//! A single-particle project: a folder `tifmrcview-spa` next to the
//! micrographs (or movies) holding `spa.json` (this struct), the picks, the
//! particle stack with its metadata, and every map. Each step reads what
//! the previous ones left and records its outputs here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::class2d::{self, Class2dParams};
use super::data::{self, Optics, Particle, Stack, StackWriter};
use super::pick::{self, PickParams};
use super::refine::{self, InitialParams, Map, RefineParams};
use super::{check_cancel, extract, post, Progress};
use crate::image::OpenImage;

pub const DIR: &str = "tifmrcview-spa";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicCtf {
    pub df1: f64,
    pub df2: f64,
    pub angle: f64,
    pub score: f64,
    pub fit_res: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mic {
    /// The micrograph (after motion correction when the inputs are movies).
    pub path: String,
    /// The movie it came from.
    #[serde(default)]
    pub movie: Option<String>,
    pub width: usize,
    pub height: usize,
    #[serde(default)]
    pub ctf: Option<MicCtf>,
    #[serde(default)]
    pub picks: usize,
    /// Left out of picking and extraction.
    #[serde(default)]
    pub excluded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackInfo {
    pub path: String,
    pub box_px: usize,
    pub pixel_a: f64,
    pub count: usize,
    pub star: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Class2dInfo {
    pub path: String,
    pub box_px: usize,
    pub pixel_a: f64,
    pub counts: Vec<usize>,
    pub scores: Vec<f32>,
    pub selected: Vec<usize>,
    pub params: Class2dParams,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapInfo {
    pub path: String,
    pub box_px: usize,
    pub pixel_a: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefineInfo {
    pub map: String,
    pub sharpened: String,
    pub half1: String,
    pub half2: String,
    pub mask: String,
    pub star: String,
    pub box_px: usize,
    pub pixel_a: f64,
    pub particles: usize,
    pub res: f64,
    pub res_masked: f64,
    pub bfactor: f64,
    pub fsc: Vec<f64>,
    pub fsc_masked: Vec<f64>,
    pub history: Vec<refine::Iteration>,
    pub params: RefineParams,
    pub backend: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// The project folder.
    pub dir: String,
    /// Where the micrographs or movies are.
    pub source: String,
    pub movies: bool,
    pub optics: Optics,
    pub diameter_a: f64,
    pub mics: Vec<Mic>,
    #[serde(default)]
    pub pick: Option<PickParams>,
    #[serde(default)]
    pub stack: Option<StackInfo>,
    #[serde(default)]
    pub class2d: Option<Class2dInfo>,
    #[serde(default)]
    pub initial: Option<MapInfo>,
    #[serde(default)]
    pub refine: Option<RefineInfo>,
}

const MIC_EXT: [&str; 6] = ["mrc", "mrcs", "tif", "tiff", "eer", "dm4"];

fn open_image(path: &str) -> Result<Arc<OpenImage>> {
    Ok(OpenImage::new(0, crate::formats::open(Path::new(path)).with_context(|| format!("打不开 {path}"))?, 256 << 20))
}

/// The first plane of an image file as f32, with its size.
pub fn read_mic(path: &str) -> Result<(Vec<f32>, usize, usize)> {
    let img = open_image(path)?;
    let info = img.info().clone();
    let p = img.page(info.locate(0, 0, 0).0)?;
    let (w, h) = (p[0].w, p[0].h);
    let (lo, hi) = crate::ctf::clip_range(|i| p[0].data.get_f64(i) as f32, w * h);
    Ok(((0..w * h).map(|i| crate::ctf::clip(p[0].data.get_f64(i) as f32, lo, hi)).collect(), w, h))
}

impl Project {
    pub fn file(dir: &str) -> PathBuf {
        Path::new(dir).join("spa.json")
    }

    pub fn path(&self, name: &str) -> String {
        Path::new(&self.dir).join(name).to_string_lossy().into()
    }

    pub fn save(&self) -> Result<()> {
        let tmp = Project::file(&self.dir).with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, Project::file(&self.dir))?;
        Ok(())
    }

    pub fn load(dir: &str) -> Result<Project> {
        let mut p: Project = serde_json::from_slice(&std::fs::read(Project::file(dir)).with_context(|| format!("没有单颗粒工程：{dir}"))?)?;
        p.dir = dir.into();
        Ok(p)
    }

    /// Opens the project of a folder (or of the folder of a file), creating
    /// it from the micrographs or movies found there.
    pub fn open(source: &str) -> Result<Project> {
        let src = Path::new(source);
        let folder = if src.is_dir() { src.to_path_buf() } else { src.parent().map(Path::to_path_buf).unwrap_or_default() };
        let folder = if folder.file_name().is_some_and(|n| n == DIR) { folder.parent().unwrap().to_path_buf() } else { folder };
        let dir = folder.join(DIR);
        let dir_s: String = dir.to_string_lossy().into();
        if Project::file(&dir_s).exists() {
            return Project::load(&dir_s);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&folder)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()).is_some_and(|x| MIC_EXT.contains(&x.to_ascii_lowercase().as_str())))
            .filter(|p| {
                // Skip motion-correction by-products.
                let s = p.file_stem().unwrap_or_default().to_string_lossy();
                !(s.ends_with("_even") || s.ends_with("_odd") || s.ends_with("_unweighted"))
            })
            .collect();
        files.sort();
        if files.is_empty() {
            bail!("文件夹里没有显微图或电影（MRC / TIFF / EER）");
        }
        // Movies: EER, or a first file with many frames.
        let first = files[0].to_string_lossy().to_string();
        let movies = files[0].extension().is_some_and(|x| x.eq_ignore_ascii_case("eer")) || {
            let img = open_image(&first)?;
            let i = img.info();
            i.size_z.max(i.size_t) > 2
        };
        let img = open_image(&first)?;
        let info = img.info().clone();
        let px = if info.pixel_size.is_some() { crate::align::pixel_a(&info) } else { 1.0 };
        std::fs::create_dir_all(&dir)?;
        let mics = files
            .iter()
            .map(|f| Mic { path: f.to_string_lossy().into(), movie: movies.then(|| f.to_string_lossy().into()), width: info.width, height: info.height, ..Default::default() })
            .collect();
        let p = Project {
            dir: dir_s,
            source: folder.to_string_lossy().into(),
            movies,
            optics: Optics { pixel_a: px, ..Default::default() },
            diameter_a: 150.0,
            mics,
            pick: None,
            stack: None,
            class2d: None,
            initial: None,
            refine: None,
        };
        p.save()?;
        Ok(p)
    }

    fn picks_path(&self) -> PathBuf {
        Path::new(&self.dir).join("picks.json")
    }

    pub fn picks(&self) -> Result<Vec<Vec<[f32; 3]>>> {
        let p = self.picks_path();
        if !p.exists() {
            return Ok(vec![vec![]; self.mics.len()]);
        }
        Ok(serde_json::from_slice(&std::fs::read(p)?)?)
    }

    pub fn particles(&self) -> Result<Vec<Particle>> {
        Ok(serde_json::from_slice(&std::fs::read(self.path("particles.json")).context("还没有提取颗粒")?)?)
    }

    fn save_particles(&self, parts: &[Particle]) -> Result<()> {
        std::fs::write(self.path("particles.json"), serde_json::to_vec(parts)?)?;
        Ok(())
    }
}

/// Motion correction of every movie (the cryo-ET package's
/// `motion::run`), dose-weighted sums written to `motion/`.
pub fn motion(p: &mut Project, params: &crate::motion::Params, progress: Progress) -> Result<()> {
    if !p.movies {
        bail!("输入已是显微图，不需要运动校正");
    }
    let out_dir = Path::new(&p.dir).join("motion");
    std::fs::create_dir_all(&out_dir)?;
    let n = p.mics.len();
    let mut params = params.clone();
    params.even_odd = false;
    params.unweighted = false;
    for i in 0..n {
        check_cancel()?;
        let movie = p.mics[i].movie.clone().unwrap_or_else(|| p.mics[i].path.clone());
        let stem = Path::new(&movie).file_stem().unwrap_or_default().to_string_lossy().to_string();
        let out = out_dir.join(format!("{stem}.mrc"));
        if !out.exists() {
            let req = crate::motion::RunReq { token: None, path: Some(movie.clone()), mdoc: None, output: out.to_string_lossy().into(), c: 0, z: 0, t: 0, params: params.clone() };
            let r = crate::motion::run(&req, None, &|_, _, _| {})?;
            if p.optics.pixel_a <= 1.0 || i == 0 {
                p.optics.pixel_a = r.pixel_a;
            }
            p.mics[i].width = r.width;
            p.mics[i].height = r.height;
        } else {
            let (_, w, h) = read_mic(&out.to_string_lossy())?;
            p.mics[i].width = w;
            p.mics[i].height = h;
        }
        p.mics[i].path = out.to_string_lossy().into();
        progress("运动校正", i + 1, n);
        p.save()?;
    }
    Ok(())
}

/// CTF of every micrograph (`ctf::fit_region` on the whole image).
pub fn ctf(p: &mut Project, params: &crate::ctf::Params, n_fft: usize, progress: Progress) -> Result<()> {
    if p.movies && p.mics.iter().any(|m| m.movie.as_deref() == Some(m.path.as_str())) {
        bail!("请先做运动校正");
    }
    let n = p.mics.len();
    let mut params = params.clone();
    params.pixel_a = p.optics.pixel_a;
    p.optics.kv = params.voltage_kv;
    p.optics.cs_mm = params.cs_mm;
    p.optics.amplitude = params.amp_contrast;
    for i in 0..n {
        check_cancel()?;
        let img = open_image(&p.mics[i].path)?;
        let (w, h) = (img.info().width, img.info().height);
        let req = crate::ctf::FitReq { token: 0, z: 0, t: 0, c: 0, region: [0.0, 0.0, w as f64, h as f64], n: n_fft, params: params.clone() };
        match crate::ctf::fit_region(&img, &req) {
            Ok(f) if f.score.is_finite() => p.mics[i].ctf = Some(MicCtf { df1: f.df1, df2: f.df2, angle: f.angle, score: f.score, fit_res: f.fit_res }),
            Ok(_) => {
                p.mics[i].ctf = None;
                p.mics[i].excluded = true;
                eprintln!("CTF {}: no fit", p.mics[i].path);
            }
            Err(e) => {
                p.mics[i].ctf = None;
                p.mics[i].excluded = true;
                eprintln!("CTF {}: {e}", p.mics[i].path);
            }
        }
        progress("CTF", i + 1, n);
    }
    p.save()
}

/// Picks of one micrograph (for the preview).
pub fn pick_one(p: &Project, i: usize, params: &PickParams) -> Result<Vec<[f32; 3]>> {
    let m = p.mics.get(i).context("没有这张显微图")?;
    let (img, w, h) = read_mic(&m.path)?;
    let mut params = params.clone();
    params.diameter_a = p.diameter_a;
    if params.margin_a <= 0.0 {
        let (big, _) = extract::boxes(p.diameter_a, p.optics.pixel_a, 256);
        params.margin_a = big as f64 / 2.0 * p.optics.pixel_a;
    }
    Ok(pick::pick(&img, w, h, p.optics.pixel_a, &params))
}

pub fn pick_all(p: &mut Project, params: &PickParams, progress: Progress) -> Result<usize> {
    let n = p.mics.len();
    let mut all = vec![];
    for i in 0..n {
        check_cancel()?;
        let picks = if p.mics[i].excluded { vec![] } else { pick_one(p, i, params)? };
        p.mics[i].picks = picks.len();
        all.push(picks);
        progress("挑选颗粒", i + 1, n);
    }
    std::fs::write(p.picks_path(), serde_json::to_vec(&all)?)?;
    p.pick = Some(params.clone());
    p.save()?;
    Ok(all.iter().map(Vec::len).sum())
}

/// Extracts every pick into `particles.mrcs` (+ `particles.json`, `particles.star`).
pub fn extract_all(p: &mut Project, max_box: usize, invert: bool, progress: Progress) -> Result<StackInfo> {
    let picks = p.picks()?;
    if picks.iter().all(Vec::is_empty) {
        bail!("还没有挑选颗粒");
    }
    let (big, out) = extract::boxes(p.diameter_a, p.optics.pixel_a, max_box);
    let px_out = p.optics.pixel_a * big as f64 / out as f64;
    let path = p.path("particles.mrcs");
    let mut w = StackWriter::create(Path::new(&path), out, px_out, "TifMrcView Pro: particles")?;
    let mut parts = vec![];
    let n = p.mics.len();
    for (i, m) in p.mics.iter().enumerate() {
        check_cancel()?;
        if m.excluded || picks[i].is_empty() {
            continue;
        }
        let Some(c) = &m.ctf else { bail!("{} 没有 CTF，请先做 CTF 或排除它", m.path) };
        let (img, iw, ih) = read_mic(&m.path)?;
        let xy: Vec<[f32; 2]> = picks[i].iter().map(|q| [q[0], q[1]]).collect();
        let boxes = extract::particles(&img, iw, ih, &xy, big, out, p.diameter_a / px_out, invert)?;
        for (b, q) in boxes.iter().zip(&picks[i]) {
            w.push(b)?;
            parts.push(Particle { mic: i as u32, x: q[0], y: q[1], df1: c.df1 as f32, df2: c.df2 as f32, angle: c.angle as f32, fom: q[2], class2d: -1, ..Default::default() });
        }
        progress("提取颗粒", i + 1, n);
    }
    let count = w.finish()?;
    p.save_particles(&parts)?;
    let star = p.path("particles.star");
    write_star(p, &parts, &path, out, px_out, None, Path::new(&star))?;
    let info = StackInfo { path, box_px: out, pixel_a: px_out, count, star };
    p.stack = Some(info.clone());
    p.class2d = None;
    p.initial = None;
    p.refine = None;
    p.save()?;
    Ok(info)
}

fn stack_of(p: &Project) -> Result<Stack> {
    let s = p.stack.as_ref().context("还没有提取颗粒")?;
    Stack::open(&s.path)
}

/// Particles to use: those in the selected 2D classes, or all.
fn selected(p: &Project, parts: &[Particle]) -> Vec<usize> {
    match &p.class2d {
        Some(c) if !c.selected.is_empty() => (0..parts.len()).filter(|&i| parts[i].class2d >= 0 && c.selected.contains(&(parts[i].class2d as usize))).collect(),
        _ => (0..parts.len()).collect(),
    }
}

pub fn class2d(p: &mut Project, params: &Class2dParams, progress: Progress) -> Result<Class2dInfo> {
    let stack = stack_of(p)?;
    let mut parts = p.particles()?;
    let ids: Vec<usize> = (0..parts.len()).collect();
    let r = class2d::run(&stack, &mut parts, &ids, &p.optics, p.diameter_a, params, progress)?;
    let path = p.path("class2d.mrcs");
    let mut all = vec![];
    for a in &r.averages {
        all.extend_from_slice(a);
    }
    data::write_mrc(Path::new(&path), &all, [r.b, r.b, r.averages.len()], r.pixel_a, "TifMrcView Pro: 2D classes")?;
    p.save_particles(&parts)?;
    let info = Class2dInfo { path, box_px: r.b, pixel_a: r.pixel_a, counts: r.counts, scores: r.scores, selected: vec![], params: params.clone() };
    p.class2d = Some(info.clone());
    p.save()?;
    Ok(info)
}

pub fn select(p: &mut Project, classes: Vec<usize>) -> Result<usize> {
    let c = p.class2d.as_mut().context("还没有二维分类")?;
    c.selected = classes;
    let parts = p.particles()?;
    let n = selected(p, &parts).len();
    p.save()?;
    Ok(n)
}

pub fn initial(p: &mut Project, params: &InitialParams, progress: Progress) -> Result<MapInfo> {
    let stack = stack_of(p)?;
    let mut parts = p.particles()?;
    let ids = selected(p, &parts);
    let m = refine::initial(&stack, &mut parts, &ids, &p.optics, p.diameter_a, params, progress)?;
    let path = p.path("initial.mrc");
    data::write_mrc(Path::new(&path), &m.v, [m.b; 3], m.px, "TifMrcView Pro: ab initio model")?;
    p.save_particles(&parts)?;
    let info = MapInfo { path, box_px: m.b, pixel_a: m.px };
    p.initial = Some(info.clone());
    p.save()?;
    Ok(info)
}

/// Mirrors the initial model (the hand cannot be told from projections).
pub fn flip_initial(p: &mut Project) -> Result<()> {
    let info = p.initial.clone().context("还没有初始模型")?;
    let (v, dims, px) = data::read_volume(&info.path)?;
    let f = post::flip_hand(&v, dims[0]);
    data::write_mrc(Path::new(&info.path), &f, dims, px, "TifMrcView Pro: ab initio model (hand flipped)")
}

pub fn refine(p: &mut Project, params: &RefineParams, reference: Option<&str>, progress: Progress) -> Result<RefineInfo> {
    let stack = stack_of(p)?;
    let mut parts = p.particles()?;
    let ids = selected(p, &parts);
    let start_path = match reference {
        Some(r) => r.to_string(),
        None => p.initial.as_ref().context("需要初始模型或参考图")?.path.clone(),
    };
    let (v, dims, px) = data::read_volume(&start_path)?;
    if dims[0] != dims[1] || dims[1] != dims[2] {
        bail!("参考图应为立方体");
    }
    let start = Map { b: dims[0], px, v };
    let r = refine::refine(&stack, &mut parts, &ids, &p.optics, p.diameter_a, &start, params, progress)?;
    let b = r.full.b;
    let px = r.full.px;
    let mask = post::auto_mask(&r.full.v, b, px, p.diameter_a / 2.0 / px * 1.2);
    let (sharp, s) = post::sharpen(&r.full.v, &r.half1.v, &r.half2.v, &mask, b, px, None);
    let names = ["refine_map.mrc", "refine_sharp.mrc", "refine_half1.mrc", "refine_half2.mrc", "refine_mask.mrc"];
    for (name, v, label) in [
        (names[0], &r.full.v, "TifMrcView Pro: refined map"),
        (names[1], &sharp, "TifMrcView Pro: sharpened map"),
        (names[2], &r.half1.v, "TifMrcView Pro: half map 1"),
        (names[3], &r.half2.v, "TifMrcView Pro: half map 2"),
        (names[4], &mask, "TifMrcView Pro: mask"),
    ] {
        data::write_mrc(Path::new(&p.path(name)), v, [b; 3], px, label)?;
    }
    p.save_particles(&parts)?;
    let star = p.path("refine.star");
    let stack_path = p.stack.as_ref().unwrap().path.clone();
    let used: Vec<Particle> = ids.iter().map(|&i| parts[i].clone()).collect();
    let idx: Vec<usize> = ids.clone();
    write_star(p, &used, &stack_path, b, px, Some(&idx), Path::new(&star))?;
    let info = RefineInfo {
        map: p.path(names[0]),
        sharpened: p.path(names[1]),
        half1: p.path(names[2]),
        half2: p.path(names[3]),
        mask: p.path(names[4]),
        star,
        box_px: b,
        pixel_a: px,
        particles: ids.len(),
        res: r.res,
        res_masked: s.res_masked,
        bfactor: s.bfactor,
        fsc: r.fsc,
        fsc_masked: s.fsc_masked,
        history: r.history,
        params: params.clone(),
        backend: super::score::backend().into(),
    };
    p.refine = Some(info.clone());
    p.save()?;
    Ok(info)
}

/// RELION 3.1-style STAR file (optics group + particles). `index` maps
/// rows to stack positions when only some particles are written.
fn write_star(p: &Project, parts: &[Particle], stack: &str, b: usize, px: f64, index: Option<&[usize]>, out: &Path) -> Result<()> {
    use std::fmt::Write as _;
    let o = &p.optics;
    let mut s = String::new();
    writeln!(s, "# TifMrcView Pro\n\ndata_optics\n\nloop_\n_rlnOpticsGroup #1\n_rlnOpticsGroupName #2\n_rlnAmplitudeContrast #3\n_rlnSphericalAberration #4\n_rlnVoltage #5\n_rlnImagePixelSize #6\n_rlnImageSize #7\n_rlnImageDimensionality #8")?;
    writeln!(s, "1 opticsGroup1 {:.4} {:.3} {:.1} {:.5} {} 2\n", o.amplitude, o.cs_mm, o.kv, px, b)?;
    let oriented = index.is_some();
    writeln!(s, "\ndata_particles\n\nloop_\n_rlnCoordinateX #1\n_rlnCoordinateY #2\n_rlnMicrographName #3\n_rlnImageName #4\n_rlnDefocusU #5\n_rlnDefocusV #6\n_rlnDefocusAngle #7\n_rlnOpticsGroup #8\n_rlnAutopickFigureOfMerit #9\n_rlnClassNumber #10")?;
    if oriented {
        writeln!(s, "_rlnAngleRot #11\n_rlnAngleTilt #12\n_rlnAnglePsi #13\n_rlnOriginXAngst #14\n_rlnOriginYAngst #15\n_rlnRandomSubset #16")?;
    }
    let stack_name = Path::new(stack).file_name().unwrap_or_default().to_string_lossy();
    for (row, q) in parts.iter().enumerate() {
        let i = index.map(|ix| ix[row]).unwrap_or(row);
        let mic = &p.mics[q.mic as usize].path;
        // RELION measures the defocus angle with y down in the image.
        write!(s, "{:.2} {:.2} {} {:06}@{} {:.1} {:.1} {:.2} 1 {:.3} {}", q.x, q.y, mic, i + 1, stack_name, q.df1, q.df2, (-q.angle).rem_euclid(180.0), q.fom, q.class2d + 1)?;
        if oriented {
            // The particle centre is at −origin from the box centre.
            write!(s, " {:.3} {:.3} {:.3} {:.3} {:.3} {}", q.euler[0], q.euler[1], q.euler[2], -q.shift[0] as f64 * px, -q.shift[1] as f64 * px, q.half)?;
        }
        writeln!(s)?;
    }
    std::fs::write(out, s)?;
    Ok(())
}
