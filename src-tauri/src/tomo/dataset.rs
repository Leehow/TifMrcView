//! A tilt series and what the Cryo-ET steps made of it, kept in
//! `<stack>.tomo.json` beside the stack so the steps hand their results on
//! (and a session can be resumed): the even/odd half stacks from motion
//! correction, angles, optics, the alignment, per-tilt CTF and the
//! tomograms. Paths are stored as given (absolute).

use super::{geometry, Alignment};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Optics {
    pub kv: f64,
    pub cs_mm: f64,
    pub amplitude: f64,
    /// Pixel size of the stack, Å (None: unknown).
    pub pixel_a: Option<f64>,
}

impl Default for Optics {
    fn default() -> Self {
        Optics {
            kv: 300.0,
            cs_mm: 2.7,
            amplitude: 0.07,
            pixel_a: None,
        }
    }
}

/// CTF of one tilt at the tilt axis (the image centre).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TiltCtf {
    /// Section of the stack.
    pub z: usize,
    pub df1: f64,
    pub df2: f64,
    /// Direction of df1 in the stack's image, degrees.
    pub angle: f64,
    pub fit_res: f64,
    pub score: f64,
    /// The fit failed (no usable rings, typical at high tilt): the defocus
    /// is taken from the neighbouring tilts instead.
    #[serde(default)]
    pub estimated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ctf {
    pub tilts: Vec<TiltCtf>,
    /// +1 or −1: whether defocus grows (+1) or shrinks across the axis
    /// toward +x of the aligned view for positive tilts; 0 when unknown.
    pub gradient: i8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tomogram {
    pub path: String,
    /// "full", "even", "odd", "denoised", "wedge".
    pub kind: String,
    pub bin: usize,
    pub thickness: usize,
    pub method: String,
    #[serde(default)]
    pub phase_flipped: bool,
    /// For a full tomogram: its even and odd half tomograms, same geometry.
    #[serde(default)]
    pub halves: Option<[String; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dataset {
    pub version: u32,
    /// The tilt series (sections in any order; `angles` per section).
    pub stack: String,
    /// Half stacks from the even and odd frames of each movie, same layout.
    #[serde(default)]
    pub even: Option<String>,
    #[serde(default)]
    pub odd: Option<String>,
    pub angles: Vec<f64>,
    /// Axis angle from the acquisition metadata, our convention (see geometry).
    #[serde(default)]
    pub axis_hint: Option<f64>,
    #[serde(default)]
    pub optics: Optics,
    #[serde(default)]
    pub alignment: Option<Alignment>,
    #[serde(default)]
    pub ctf: Option<Ctf>,
    #[serde(default)]
    pub tomograms: Vec<Tomogram>,
}

/// `<dir>/<stem>.tomo.json` for a stack.
pub fn path_for(stack: &str) -> PathBuf {
    let p = Path::new(stack);
    p.with_file_name(format!(
        "{}.tomo.json",
        p.file_stem().and_then(|s| s.to_str()).unwrap_or("stack")
    ))
}

impl Dataset {
    /// The dataset saved beside the stack, or a new one from its metadata.
    pub fn open(stack: &str, sections: usize, pixel_a: Option<f64>) -> Result<Dataset> {
        let file = path_for(stack);
        if file.exists() {
            let d: Dataset = serde_json::from_slice(&std::fs::read(&file)?)?;
            if d.version <= VERSION && d.angles.len() == sections {
                return Ok(d);
            }
            eprintln!(
                "tomo: {} does not match the stack, starting over",
                file.display()
            );
        }
        let meta = geometry::metadata(stack, sections).unwrap_or_default();
        Ok(Dataset {
            version: VERSION,
            stack: stack.into(),
            even: sibling(stack, "even"),
            odd: sibling(stack, "odd"),
            angles: meta.angles,
            axis_hint: meta.axis,
            optics: Optics {
                pixel_a: pixel_a.or(meta.pixel_a),
                ..Default::default()
            },
            alignment: None,
            ctf: None,
            tomograms: Vec::new(),
        })
    }

    pub fn save(&self) -> Result<()> {
        if self.angles.is_empty() {
            bail!("还没有倾转角，无法保存数据集");
        }
        let file = path_for(&self.stack);
        let part = file.with_extension("json.part");
        std::fs::write(&part, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&part, &file)?;
        Ok(())
    }

    /// Replaces any tomogram at the same path, else adds it.
    pub fn add_tomogram(&mut self, t: Tomogram) {
        self.tomograms.retain(|o| o.path != t.path);
        self.tomograms.push(t);
    }
}

/// `x_even.mrc` beside `x.mrc`, if it exists (motion correction writes them).
fn sibling(stack: &str, half: &str) -> Option<String> {
    let p = Path::new(stack);
    let f = p.with_file_name(format!(
        "{}_{half}.{}",
        p.file_stem()?.to_str()?,
        p.extension()?.to_str()?
    ));
    f.exists().then(|| f.to_string_lossy().into_owned())
}
