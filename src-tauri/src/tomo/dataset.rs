//! A tilt series and what the Cryo-ET steps made of it, kept in
//! `<stem>.tomo.json` (in `2_tomograms/` of the project, beside the stack
//! for an old flat project) so the steps hand their results on
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
        Optics { kv: 300.0, cs_mm: 2.7, amplitude: 0.07, pixel_a: None }
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
    /// For a full tomogram made for subtomogram averaging: the clear
    /// (denoised) companion to look at and pick on, and how its voxels map
    /// onto this one's.
    #[serde(default)]
    pub look: Option<Look>,
    /// Where the tomogram's middle section lies from the tilt axis, in its
    /// sections (`recon::Params::z_offset`); None for one made with a slope
    /// along Y or before this was kept. With it a point of the tomogram can
    /// be found again in the tilt images (`sta::tilt`).
    #[serde(default)]
    pub z_offset: Option<f64>,
}

/// A companion tomogram of the same specimen, sampled twice as coarsely and
/// denoised, to look at and find particles on. A voxel index (x, y, z) of the
/// computing tomogram is `scale × look index + offset`, per axis; indices are
/// voxel centres, so a voxel's centre is its integer index.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Look {
    pub path: String,
    pub scale: f64,
    pub offset: [f64; 3],
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
    /// What each view shows on its own and whether the program would use
    /// it (`triage`); empty until the views were looked at.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<super::triage::ViewCheck>,
    /// The user's own choice per section: true keeps a view the program
    /// would leave out, false drops one. Never changed by the program.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub picks: std::collections::BTreeMap<usize, bool>,
    /// What the program makes of the series as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<super::triage::Verdict>,
    /// The user's choice for the whole series (None: the program's verdict).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take: Option<bool>,
    /// The aligned views at a small size, to look at how well they align.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aligned: Option<String>,
    /// The tomogram to look at that the one-click reconstruction made last
    /// for each purpose ("survey", "display", "sta").
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub made: std::collections::BTreeMap<String, String>,
}

/// Where the dataset of a stack is kept: `2_tomograms/<stem>.tomo.json` of
/// its project, or beside the stack for a project made with the old flat
/// layout (see `project_dirs`).
pub fn path_for(stack: &str) -> PathBuf {
    crate::project_dirs::dataset_path(stack)
}

impl Dataset {
    /// The dataset saved for the stack (`path_for`), or a new one from its metadata.
    pub fn open(stack: &str, sections: usize, pixel_a: Option<f64>) -> Result<Dataset> {
        let file = path_for(stack);
        if file.exists() {
            let mut d: Dataset = serde_json::from_slice(&std::fs::read(&file)?)?;
            // The project folder was moved (processed elsewhere and copied
            // here): what lies under the old project folder is now under this one.
            if d.stack != stack {
                let was = d.stack.clone();
                d = crate::relocate::relocate_dirs(d, crate::project_dirs::root_str(&was), crate::project_dirs::root_str(stack));
                d.stack = stack.into();
            }
            if d.version <= VERSION && d.angles.len() == sections {
                return Ok(d);
            }
            eprintln!("tomo: {} does not match the stack, starting over", file.display());
        }
        let meta = geometry::metadata(stack, sections).unwrap_or_default();
        Ok(Dataset {
            version: VERSION,
            stack: stack.into(),
            even: sibling(stack, "even"),
            odd: sibling(stack, "odd"),
            angles: meta.angles,
            axis_hint: meta.axis,
            optics: Optics { pixel_a: pixel_a.or(meta.pixel_a), ..Default::default() },
            alignment: None,
            ctf: None,
            tomograms: Vec::new(),
            checks: Vec::new(),
            picks: Default::default(),
            verdict: None,
            take: None,
            aligned: None,
            made: Default::default(),
        })
    }

    pub fn save(&self) -> Result<()> {
        if self.angles.is_empty() {
            bail!("还没有倾转角，无法保存数据集");
        }
        let file = path_for(&self.stack);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
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
    let f = p.with_file_name(format!("{}_{half}.{}", p.file_stem()?.to_str()?, p.extension()?.to_str()?));
    f.exists().then(|| f.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn started(stack: &str) -> Dataset {
        let mut d = Dataset::open(stack, 3, Some(2.0)).unwrap();
        d.angles = vec![-30.0, 0.0, 30.0];
        d
    }

    fn copy_all(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let (f, t) = (e.path(), to.join(e.file_name()));
            if f.is_dir() {
                copy_all(&f, &t);
            } else {
                std::fs::copy(&f, &t).unwrap();
            }
        }
    }

    #[test]
    fn a_new_project_keeps_its_dataset_in_the_tomograms_folder_and_survives_a_copy() {
        let up = tempfile::tempdir().unwrap();
        let old = up.path().join("disk1/proj");
        std::fs::create_dir_all(&old).unwrap();
        let stack = s(&old.join("ts.mrc"));
        let mut d = started(&stack);
        d.add_tomogram(Tomogram { path: s(&old.join("2_tomograms/ts_wbp.mrc")), kind: "full".into(), bin: 4, thickness: 10, method: "wbp".into(), phase_flipped: false, halves: None, look: Some(Look { path: s(&old.join("3_clean/ts_look.mrc")), scale: 2.0, offset: [0.0; 3] }), z_offset: None });
        d.save().unwrap();
        assert_eq!(path_for(&stack), old.join("2_tomograms/ts.tomo.json"));
        assert!(old.join("2_tomograms/ts.tomo.json").is_file());
        assert!(!old.join("ts.tomo.json").exists(), "nothing beside the raw stack");
        assert_eq!(Dataset::open(&stack, 3, None).unwrap().tomograms.len(), 1);
        // The whole project folder copied to another disk: the recorded paths are re-rooted.
        let new = up.path().join("disk2/proj");
        copy_all(&old, &new);
        let moved = Dataset::open(&s(&new.join("ts.mrc")), 3, None).unwrap();
        assert_eq!(moved.stack, s(&new.join("ts.mrc")));
        assert_eq!(moved.tomograms[0].path, s(&new.join("2_tomograms/ts_wbp.mrc")));
        assert_eq!(moved.tomograms[0].look.as_ref().unwrap().path, s(&new.join("3_clean/ts_look.mrc")));
    }

    #[test]
    fn a_stack_made_from_frames_lies_in_the_motion_folder_and_the_whole_project_moves_with_it() {
        let up = tempfile::tempdir().unwrap();
        let old = up.path().join("a/frames");
        std::fs::create_dir_all(old.join("1_motion")).unwrap();
        let stack = s(&old.join("1_motion/ts.mrc"));
        let mut d = started(&stack);
        d.even = Some(s(&old.join("1_motion/ts_even.mrc")));
        d.add_tomogram(Tomogram { path: s(&old.join("2_tomograms/ts_wbp.mrc")), kind: "full".into(), bin: 4, thickness: 10, method: "wbp".into(), phase_flipped: false, halves: None, look: None, z_offset: None });
        d.save().unwrap();
        assert!(old.join("2_tomograms/ts.tomo.json").is_file());
        assert!(!old.join("1_motion/ts.tomo.json").exists());
        let new = up.path().join("b/frames");
        copy_all(&old, &new);
        let moved = Dataset::open(&s(&new.join("1_motion/ts.mrc")), 3, None).unwrap();
        assert_eq!(moved.even.as_deref(), Some(s(&new.join("1_motion/ts_even.mrc")).as_str()));
        assert_eq!(moved.tomograms[0].path, s(&new.join("2_tomograms/ts_wbp.mrc")));
    }

    #[test]
    fn an_old_flat_project_keeps_its_dataset_beside_the_stack() {
        let up = tempfile::tempdir().unwrap();
        let dir = up.path().join("flat");
        std::fs::create_dir_all(&dir).unwrap();
        let stack = s(&dir.join("ts.mrc"));
        // Made by an earlier version: the json lies beside the stack.
        let old = serde_json::json!({ "version": 1, "stack": stack, "angles": [-30.0, 0.0, 30.0], "tomograms": [{ "path": s(&dir.join("ts_wbp.mrc")), "kind": "full", "bin": 4, "thickness": 10, "method": "wbp" }] });
        std::fs::write(dir.join("ts.tomo.json"), old.to_string()).unwrap();
        let mut d = Dataset::open(&stack, 3, None).unwrap();
        assert_eq!(d.tomograms.len(), 1);
        d.optics.kv = 200.0;
        d.save().unwrap();
        assert_eq!(path_for(&stack), dir.join("ts.tomo.json"));
        assert_eq!(Dataset::open(&stack, 3, None).unwrap().optics.kv, 200.0);
        let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["ts.tomo.json"], "no step folder appears and nothing moves");
    }
}
