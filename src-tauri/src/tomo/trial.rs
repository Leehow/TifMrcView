//! 「重构试验」: quick reconstructions of one tilt series with a method and
//! parameters the user picks (the toolbar's reconstruction button, `tmv
//! recon`), to compare WBP, SIRT and SART and their settings side by side.
//! Nothing here reconstructs: the series is aligned with `tomo::align` when
//! its dataset has no alignment yet and built by `pipeline::reconstruct`,
//! each result named by its settings so that every trial stays.

use super::{dataset::Dataset, pipeline, recon, AlignReq};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// What one trial is made with.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialReq {
    /// Output binning of the aligned views (1 = the stack's pixels).
    pub bin: usize,
    pub params: recon::Params,
    /// Phase flip each view with the dataset's CTF (needs a CTF fit).
    #[serde(default)]
    pub phase_flip: bool,
}

/// Settings a first trial starts from: binned so the tomogram is at most
/// 512 pixels wide, and about 300 nm thick (1/3 of the width when the pixel
/// size is unknown).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    pub bin: usize,
    pub thickness: usize,
}

pub fn defaults(width: usize, pixel_a: Option<f64>) -> Defaults {
    let mut bin = 1;
    while width / bin > 512 && bin < 16 {
        bin *= 2;
    }
    let w = (width / bin).max(1);
    let z = match pixel_a.filter(|p| *p > 0.1) {
        Some(px) => (3000.0 / (px * bin as f64)).round() as usize,
        None => w / 3,
    };
    Defaults { bin, thickness: (z.clamp(16, w.max(16)) + 7) / 8 * 8 }
}

/// The tilt series behind `path`: itself when it has a tilt angle for each
/// section, else the series whose dataset lists it (a tomogram made from it,
/// or the small aligned film of its views).
pub fn source(path: &str) -> Option<String> {
    let img = pipeline::open(path).ok()?;
    let nz = img.info().size_z;
    if nz >= 3 && super::geometry::metadata(path, nz).is_ok_and(|m| m.angles.len() == nz) {
        return Some(path.to_string());
    }
    let p = Path::new(path);
    let name = p.file_name()?;
    let same = |f: &str| Path::new(f).file_name() == Some(name);
    for dir in crate::project_dirs::relatives(p.parent()?) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|f| f.to_string_lossy().ends_with(".tomo.json")).collect();
        files.sort();
        for f in files {
            let Some(d) = std::fs::read(&f).ok().and_then(|b| serde_json::from_slice::<Dataset>(&b).ok()) else { continue };
            if (d.tomograms.iter().any(|t| same(&t.path)) || d.aligned.as_deref().is_some_and(same)) && Path::new(&d.stack).is_file() {
                return Some(d.stack);
            }
        }
    }
    None
}

/// What the reconstruction button needs to know of the open image: None
/// when it is no tilt series and no dataset lists it; else the series, its
/// dataset, the settings a first trial starts from, its width and pixel size.
/// The app (`tomo_trial_source`) and the SSH engine both answer with this.
pub fn describe(path: &str) -> Result<Option<serde_json::Value>> {
    let Some(stack) = source(path) else { return Ok(None) };
    let img = pipeline::open(&stack)?;
    let info = img.info();
    let d = Dataset::open(&info.path, info.size_z, info.pixel_size.is_some().then(|| crate::align::pixel_a(info)))?;
    if d.angles.len() != info.size_z {
        return Ok(None);
    }
    let px = d.optics.pixel_a.or_else(|| super::output::pixel_known(&img));
    let defaults = defaults(info.width, px);
    Ok(Some(serde_json::json!({ "stack": stack, "dataset": d, "defaults": defaults, "width": info.width, "pixelA": px })))
}

/// The part of the file name that tells trials apart; the method, its
/// iterations, the binning and phase flipping are named by `reconstruct`.
pub fn label(req: &TrialReq) -> String {
    let p = &req.params;
    let mut l = format!("try_z{}", p.thickness);
    if p.method == recon::Method::Wbp && p.filter != recon::Filter::Ramp {
        l += &format!("_{:?}", p.filter).to_lowercase();
    }
    if p.method != recon::Method::Wbp {
        let default = if p.method == recon::Method::Sart { 0.5 } else { 1.0 };
        if (p.lambda - default).abs() > 1e-6 {
            l += &format!("_l{}", p.lambda);
        }
        if p.non_negative {
            l += "_pos";
        }
    }
    if p.z_offset != 0.0 {
        l += &format!("_off{}", p.z_offset);
    }
    l
}

/// What a trial made.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trial {
    pub path: String,
    pub backend: String,
    pub ms: f64,
    /// The series was aligned for this trial (it had no alignment yet).
    pub aligned: bool,
    /// Settings in words, for the list of trials.
    pub summary: String,
}

pub fn summary(req: &TrialReq) -> String {
    let p = &req.params;
    let mut s = match p.method {
        recon::Method::Wbp => format!("WBP · {:?}", p.filter),
        recon::Method::Sirt => format!("SIRT × {}", p.iterations),
        recon::Method::Sart => format!("SART × {}", p.iterations),
    };
    s += &format!(" · bin {} · Z {}", req.bin, p.thickness);
    if p.method != recon::Method::Wbp && p.non_negative {
        s += " · 非负";
    }
    if p.z_offset != 0.0 {
        s += &format!(" · 偏移 {}", p.z_offset);
    }
    if req.phase_flip {
        s += " · 相位翻转";
    }
    s
}

/// Aligns the series when its dataset has none (patch tracking with the
/// sweep over axes and patch layouts, as step ③ does by default).
pub fn ensure_aligned(d: &mut Dataset, progress: pipeline::Progress) -> Result<bool> {
    if d.alignment.is_some() {
        return Ok(false);
    }
    if d.angles.is_empty() {
        bail!("没有找到倾转角（.mdoc / .tlt / .rawtlt），无法重构");
    }
    let img = pipeline::open(&d.stack)?;
    let req = AlignReq {
        token: 0,
        c: 0,
        t: 0,
        angles: d.angles.clone(),
        start: None,
        step: None,
        axis: d.axis_hint,
        bin: 0,
        sweep: true,
        patch: None,
        excluded: Vec::new(),
        groups: None,
        previous: None,
    };
    let a = super::align(&img, &req, &|i, n| progress("倾转对齐", i, n))?;
    let mut angles = vec![0.0; a.views.len()];
    for v in &a.views {
        angles[v.file_index] = v.angle;
    }
    d.angles = angles;
    d.alignment = Some(a);
    d.save()?;
    Ok(true)
}

/// One trial: aligns when needed, then reconstructs with the settings given.
pub fn run(d: &mut Dataset, req: &TrialReq, progress: pipeline::Progress) -> Result<Trial> {
    let p = &req.params;
    if req.bin == 0 || req.bin > 16 {
        bail!("binning 应在 1–16 之间");
    }
    let aligned = ensure_aligned(d, progress)?;
    let step = pipeline::ReconStep { bin: req.bin, params: p.clone(), halves: false, phase_flip: req.phase_flip, ctf_3d: false, label: Some(label(req)) };
    let made = pipeline::reconstruct(d, &step, progress)?;
    let Some(path) = made.paths.first().cloned() else { bail!("重构没有输出") };
    // A trial is not the series' tomogram: STA and the flow keep theirs.
    for t in d.tomograms.iter_mut().filter(|t| t.path == path) {
        t.kind = "trial".into();
    }
    d.save()?;
    Ok(Trial { path, backend: made.backend, ms: made.ms, aligned, summary: summary(req) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: recon::Method) -> TrialReq {
        TrialReq { bin: 2, params: recon::Params { method, thickness: 96, ..Default::default() }, phase_flip: false }
    }

    #[test]
    fn defaults_bin_to_512_and_300_nm() {
        let d = defaults(4096, Some(1.5));
        assert_eq!(d.bin, 8);
        assert_eq!(d.thickness, 256);
        let d = defaults(512, Some(30.5));
        assert_eq!(d.bin, 1);
        assert_eq!(d.thickness, 104);
        assert_eq!(defaults(1024, None).thickness, 176);
    }

    #[test]
    fn labels_tell_trials_apart() {
        let wbp = req(recon::Method::Wbp);
        assert_eq!(label(&wbp), "try_z96");
        let mut cos = wbp.clone();
        cos.params.filter = recon::Filter::Cosine;
        assert_eq!(label(&cos), "try_z96_cosine");
        let mut sirt = req(recon::Method::Sirt);
        sirt.params.non_negative = true;
        sirt.params.z_offset = -4.0;
        assert_eq!(label(&sirt), "try_z96_pos_off-4");
        let mut sart = req(recon::Method::Sart);
        sart.params.lambda = 0.5;
        assert_eq!(label(&sart), "try_z96");
        sart.params.lambda = 0.3;
        assert_eq!(label(&sart), "try_z96_l0.3");
        assert!(summary(&sirt).starts_with("SIRT × 15 · bin 2 · Z 96"));
    }
}
