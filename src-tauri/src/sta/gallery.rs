//! The tomograms of a project at a glance: the workspace's choose step
//! shows every tomogram of the folder as a tile with a thumbnail, so the
//! user sees which are there, which can be used and which is which, and
//! opens the one to mark examples on.
//!
//! `tiles` lists them from the datasets (`*.tomo.json`) in the folder of
//! the open file and the folders beside it, and the tilt series there that
//! were never reconstructed. `thumbnail` draws one: the section with the
//! most in it, not the middle one (the middle of a tomogram is often
//! empty), found as the section whose coarse picture varies most. Thumbnails
//! are kept in `thumbnails/` beside the tomogram and made again only when
//! the file changed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::auto::{beside, datasets_in, grey_of, grey_range, look_of, tilt_range};
use crate::tomo::pipeline;

/// One tomogram (or tilt series) of the project.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tile {
    pub name: String,
    /// The tomogram to average, when it is there.
    pub raw: Option<String>,
    /// Its clean companion (what the thumbnail shows and the user marks on).
    pub look: Option<String>,
    /// The tilt series it is made from.
    pub stack: Option<String>,
    /// "ready" (tomogram and clean companion), "noLook" (usable, but only
    /// the tomogram itself to look at), "notBuilt" (a tilt series with no
    /// tomogram yet), "unusable" (see `reason`).
    pub status: &'static str,
    pub reason: Option<String>,
    /// The file open in the viewer is this one.
    pub open: bool,
}

impl Tile {
    /// The file a thumbnail is made of.
    pub fn shown(&self) -> Option<&str> {
        self.look.as_deref().or(self.raw.as_deref())
    }
}

/// Extensions a tilt series comes in.
const STACKS: [&str; 5] = ["mrc", "mrcs", "st", "ali", "tif"];

/// Whether tilt angles lie beside `stack` (SerialEM .mdoc, IMOD .tlt/.rawtlt).
fn has_angles(stack: &Path) -> bool {
    let with = |ext: &str| stack.with_extension(ext).is_file();
    let mdoc = PathBuf::from(format!("{}.mdoc", stack.to_string_lossy()));
    with("tlt") || with("rawtlt") || with("mdoc") || mdoc.is_file()
}

/// The tomograms and tilt series of the project of `path` (a tomogram, its
/// clean companion, or a folder): in that folder and the folders beside it.
pub fn tiles(path: &str) -> Vec<Tile> {
    let p = Path::new(path);
    let (dir, open_name) = if p.is_dir() { (p.to_path_buf(), None) } else { (p.parent().map(Path::to_path_buf).unwrap_or_default(), p.file_name().map(|n| n.to_owned())) };
    let mut folders: Vec<PathBuf> = vec![dir.clone()];
    if let Some(rd) = dir.parent().and_then(|up| std::fs::read_dir(up).ok()) {
        let mut beside_it: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|f| f.is_dir() && *f != dir).collect();
        beside_it.sort();
        folders.extend(beside_it.into_iter().take(200));
    }
    let name_of = |f: &str| Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out: Vec<Tile> = vec![];
    let mut stacks_seen: Vec<String> = vec![];
    for folder in &folders {
        for d in datasets_in(folder) {
            stacks_seen.push(name_of(&d.stack));
            let stack = beside(folder, &d.stack);
            let full: Vec<_> = d.tomograms.iter().filter(|t| t.kind == "full").collect();
            let mut any = false;
            for t in &full {
                let Some(raw) = beside(folder, &t.path) else { continue };
                any = true;
                if out.iter().any(|o| o.raw.as_deref().map(name_of) == Some(name_of(&raw))) {
                    continue;
                }
                let look = look_of(Some(&d), &raw, &|_| {}).map(|l| l.path);
                let open = open_name.as_ref().is_some_and(|n| Path::new(&raw).file_name() == Some(n) || look.as_ref().is_some_and(|l| Path::new(l).file_name() == Some(n)));
                let (status, reason) = if tilt_range(&d).is_none() {
                    ("unusable", Some("缺倾斜角：不知道这张断层图是从哪些角度重建的".to_string()))
                } else if look.is_some() {
                    ("ready", None)
                } else {
                    ("noLook", None)
                };
                out.push(Tile { name: name_of(&raw), raw: Some(raw), look, stack: stack.clone(), status, reason, open });
            }
            if !any {
                let reason = if full.is_empty() { None } else { Some("重建出的断层图文件找不到了".to_string()) };
                out.push(Tile { name: name_of(&d.stack), raw: None, look: None, stack, status: if full.is_empty() { "notBuilt" } else { "unusable" }, reason, open: false });
            }
        }
        // Tilt series nobody has worked on yet: a stack with its angles beside it.
        let Ok(rd) = std::fs::read_dir(folder) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|f| f.is_file()).collect();
        files.sort();
        for f in files.into_iter().take(2000) {
            let ext = f.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let name = name_of(&f.to_string_lossy());
            if STACKS.contains(&ext.as_str()) && has_angles(&f) && !stacks_seen.contains(&name) && !out.iter().any(|o| o.name == name) {
                out.push(Tile { name, raw: None, look: None, stack: Some(f.to_string_lossy().into_owned()), status: "notBuilt", reason: None, open: false });
            }
        }
    }
    // The open file itself when no dataset lists it: a tomogram from elsewhere.
    if let Some(n) = &open_name {
        if !out.iter().any(|o| o.open) {
            out.insert(0, Tile { name: n.to_string_lossy().into_owned(), raw: Some(path.to_string()), look: None, stack: None, status: "noLook", reason: None, open: true });
        }
    }
    out
}

/// At most this many sections are looked at to find the fullest.
const SAMPLED: usize = 48;
/// A section is judged on a picture this many pixels across.
const COARSE: usize = 64;
/// Longer side of a thumbnail, pixels.
pub const THUMB: usize = 320;

/// Section `z` of an open image as block means, the longer side `side`
/// pixels at most: (pixels, width, height).
fn coarse(img: &crate::image::OpenImage, z: usize, side: usize) -> Result<(Vec<f32>, usize, usize)> {
    let info = img.info();
    let (w, h) = (info.width, info.height);
    let f = w.max(h).div_ceil(side).max(1);
    let (cw, ch) = ((w / f).max(1), (h / f).max(1));
    let (page, s) = info.locate(0, z, 0);
    let p = img.page(page)?;
    let p = &p[s.min(p.len() - 1)];
    let mut out = vec![0f32; cw * ch];
    for y in 0..ch {
        for x in 0..cw {
            let mut sum = 0f64;
            for dy in 0..f {
                for dx in 0..f {
                    sum += p.data.get_f64((y * f + dy) * w + x * f + dx);
                }
            }
            out[y * cw + x] = (sum / (f * f) as f64) as f32;
        }
    }
    Ok((out, cw, ch))
}

/// The section of `file` with the most in it: of sections spread over the
/// middle four fifths, the one whose coarse picture varies most (noise
/// averages out in the block means; membranes, particles and ice edges do
/// not).
pub fn fullest(file: &str) -> Result<usize> {
    let img = pipeline::open(file).with_context(|| format!("cannot open {file}"))?;
    let nz = img.info().size_z;
    if nz < 3 {
        return Ok(0);
    }
    let (lo, hi) = (nz / 10, nz - 1 - nz / 10);
    let count = (hi - lo + 1).min(SAMPLED);
    let mut best = (f64::MIN, nz / 2);
    for k in 0..count {
        let z = lo + if count > 1 { k * (hi - lo) / (count - 1) } else { 0 };
        let (v, _, _) = coarse(&img, z, COARSE)?;
        let mean = v.iter().map(|x| *x as f64).sum::<f64>() / v.len() as f64;
        let var = v.iter().map(|x| (*x as f64 - mean).powi(2)).sum::<f64>() / v.len() as f64;
        if var > best.0 {
            best = (var, z);
        }
    }
    Ok(best.1)
}

/// What is kept beside a thumbnail to know whether it is still good.
#[derive(Serialize, Deserialize, PartialEq)]
struct Stamp {
    len: u64,
    modified: Option<u64>,
    section: usize,
}

/// The thumbnail of `file` (a tomogram or its clean companion): the PNG's
/// path and the section it shows. Made in `thumbnails/` beside the file and
/// reused until the file changes.
pub fn thumbnail(file: &str) -> Result<(PathBuf, usize)> {
    let p = Path::new(file);
    let meta = std::fs::metadata(p).with_context(|| format!("cannot read {file}"))?;
    let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
    let dir = p.parent().unwrap_or(Path::new(".")).join("thumbnails");
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (png, note) = (dir.join(format!("{name}.png")), dir.join(format!("{name}.json")));
    if let Some(s) = std::fs::read(&note).ok().and_then(|b| serde_json::from_slice::<Stamp>(&b).ok()) {
        if s.len == meta.len() && s.modified == modified && png.is_file() {
            return Ok((png, s.section));
        }
    }
    let section = fullest(file)?;
    let img = pipeline::open(file)?;
    let (v, w, h) = coarse(&img, section, THUMB)?;
    let grey = grey_of(&v, grey_range(&v));
    std::fs::create_dir_all(&dir).with_context(|| format!("无法创建 {}", dir.display()))?;
    ::image::GrayImage::from_raw(w as u32, h as u32, grey).context("thumbnail")?.save(&png).with_context(|| format!("无法写入 {}", png.display()))?;
    std::fs::write(&note, serde_json::to_vec(&Stamp { len: meta.len(), modified, section })?)?;
    Ok((png, section))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thumbnail_shows_the_section_with_the_most_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t_look.mrc").to_string_lossy().into_owned();
        let (w, h, nz) = (96usize, 80usize, 30usize);
        let mut state = 5u32;
        let mut noise = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        // Noise everywhere; discs in sections 8 to 10, far from the middle (15).
        let vol: Vec<f32> = (0..w * h * nz)
            .map(|i| {
                let (x, y, z) = ((i % w) as f32, (i / w % h) as f32, i / (w * h));
                let disc = (8..=10).contains(&z) && [(30.0, 30.0), (66.0, 50.0)].iter().any(|(cx, cy)| (x - cx).powi(2) + (y - cy).powi(2) < 144.0);
                noise() - if disc { 1.0 } else { 0.0 }
            })
            .collect();
        crate::tomo::output::write_mrc(&file, &vol, [w, h, nz], 10.0, false).unwrap();
        let z = fullest(&file).unwrap();
        assert!((8..=10).contains(&z), "{z}");
        let (png, section) = thumbnail(&file).unwrap();
        assert_eq!(section, z);
        assert_eq!(png, dir.path().join("thumbnails").join("t_look.mrc.png"));
        let pic = ::image::open(&png).unwrap().to_luma8();
        assert_eq!((pic.width(), pic.height()), (96, 80));
        // The discs are dark in it.
        let patch = |cx: u32, cy: u32| (0..25).map(|k| pic.get_pixel(cx - 2 + k % 5, cy - 2 + k / 5).0[0] as f64).sum::<f64>() / 25.0;
        assert!(patch(30, 30) < 100.0 && patch(66, 50) < 100.0 && patch(8, 70) > 140.0, "{} {} {}", patch(30, 30), patch(66, 50), patch(8, 70));
        // Reused while the file is the same, made again when it changes.
        let made = std::fs::metadata(&png).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(thumbnail(&file).unwrap(), (png.clone(), z));
        assert_eq!(std::fs::metadata(&png).unwrap().modified().unwrap(), made);
        crate::tomo::output::write_mrc(&file, &vol[..w * h * 12], [w, h, 12], 10.0, false).unwrap();
        thumbnail(&file).unwrap();
        assert!(std::fs::metadata(&png).unwrap().modified().unwrap() > made);
    }

    #[test]
    fn tiles_tell_what_can_be_used() {
        let up = tempfile::tempdir().unwrap();
        let dir = up.path().join("session");
        std::fs::create_dir_all(&dir).unwrap();
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        let write = |n: &str| std::fs::write(dir.join(n), b"x").unwrap();
        let dataset = |ts: &str, angles: serde_json::Value, tomograms: serde_json::Value| {
            let d = serde_json::json!({ "version": 1, "stack": p(&format!("{ts}.mrc")), "angles": angles, "tomograms": tomograms });
            std::fs::write(dir.join(format!("{ts}.tomo.json")), d.to_string()).unwrap();
        };
        let full = |raw: &str, look: Option<&str>| {
            serde_json::json!({ "path": p(raw), "kind": "full", "bin": 4, "thickness": 200, "method": "sart", "look": look.map(|l| serde_json::json!({ "path": p(l), "scale": 2.0, "offset": [0.5, 0.5, 1.5] })) })
        };
        // 05: tomogram and clean companion. 06: tomogram only. 07: dataset, nothing reconstructed.
        // 08: its tomogram file is gone. 09: a tilt series nobody has opened. notes.mrc: no angles, not a series.
        for f in ["05.mrc", "05_sart.mrc", "05_look.mrc", "06.mrc", "06_sart.mrc", "07.mrc", "08.mrc", "09.st", "09.rawtlt", "notes.mrc"] {
            write(f);
        }
        dataset("05", serde_json::json!([-60.0, 0.0, 60.0]), serde_json::json!([full("05_sart.mrc", Some("05_look.mrc"))]));
        dataset("06", serde_json::json!([-60.0, 0.0, 60.0]), serde_json::json!([full("06_sart.mrc", None)]));
        dataset("07", serde_json::json!([-60.0, 0.0, 60.0]), serde_json::json!([]));
        dataset("08", serde_json::json!([-60.0, 0.0, 60.0]), serde_json::json!([full("08_sart.mrc", None)]));
        let t = tiles(&p("05_look.mrc"));
        let got: Vec<(&str, &str, bool)> = t.iter().map(|x| (x.name.as_str(), x.status, x.open)).collect();
        assert_eq!(got, [("05_sart.mrc", "ready", true), ("06_sart.mrc", "noLook", false), ("07.mrc", "notBuilt", false), ("08.mrc", "unusable", false), ("09.st", "notBuilt", false)]);
        assert_eq!(t[0].shown(), Some(p("05_look.mrc").as_str()));
        assert_eq!(t[1].shown(), Some(p("06_sart.mrc").as_str()));
        assert!(t[3].reason.as_deref().is_some_and(|r| r.contains("找不到")));
        assert_eq!(t[4].stack.as_deref(), Some(p("09.st").as_str()));
        // A folder works as well as a file of it; then nothing is open.
        assert_eq!(tiles(&dir.to_string_lossy()).iter().filter(|x| x.open).count(), 0);
        assert_eq!(tiles(&dir.to_string_lossy()).len(), 5);
    }
}
