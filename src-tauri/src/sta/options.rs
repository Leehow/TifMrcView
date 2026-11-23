//! Advanced per-project STA options kept in `<out>/options.json`:
//! point-group symmetry (C1/Cn/Dn/T/O/I), particle cap (`maxParticles`),
//! optional custom particle size in Å for shapes, and 2D tilt-image pose
//! refinement toggle.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use super::IDENTITY;
use crate::spa::geom::{symmetry, Mat};

pub const DEFAULT_MAX_PARTICLES: usize = 3000;
pub const MIN_MAX_PARTICLES: usize = 20;
pub const MAX_MAX_PARTICLES: usize = 50_000;

fn default_symmetry() -> String {
    "C1".into()
}
fn default_max_particles() -> usize {
    DEFAULT_MAX_PARTICLES
}
fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    #[serde(default = "default_symmetry")]
    pub symmetry: String,
    #[serde(default = "default_max_particles")]
    pub max_particles: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_size_a: Option<f64>,
    #[serde(default = "default_true")]
    pub refine_2d_poses: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            symmetry: default_symmetry(),
            max_particles: DEFAULT_MAX_PARTICLES,
            custom_size_a: None,
            refine_2d_poses: true,
        }
    }
}

fn file_of(dir: &Path) -> PathBuf {
    dir.join("options.json")
}

/// Reads `<dir>/options.json`, or falls back to parent/grandparent folders
/// (since route outputs live in `<out>/drawn`, `<out>/circled`, `<out>/kinds/kept`).
pub fn load(dir: &Path) -> Options {
    for anc in [Some(dir), dir.parent(), dir.parent().and_then(Path::parent)].into_iter().flatten() {
        if let Ok(bytes) = std::fs::read(file_of(anc)) {
            if let Ok(mut o) = serde_json::from_slice::<Options>(&bytes) {
                o.symmetry = normalize_sym(&o.symmetry).unwrap_or_else(|_| "C1".into());
                o.max_particles = o.max_particles.clamp(MIN_MAX_PARTICLES, MAX_MAX_PARTICLES);
                if o.custom_size_a.is_some_and(|v| !v.is_finite() || v < 40.0 || v > 2000.0) {
                    o.custom_size_a = None;
                }
                return o;
            }
        }
    }
    Options::default()
}

pub fn normalize_sym(s: &str) -> Result<String> {
    let clean = s.trim().to_ascii_uppercase();
    let name = if clean.is_empty() { "C1".to_string() } else { clean };
    let _ = symmetry(&name)?;
    Ok(name)
}

pub fn save(out: &Path, opts: &Options) -> Result<Options> {
    let sym = normalize_sym(&opts.symmetry)?;
    if !(MIN_MAX_PARTICLES..=MAX_MAX_PARTICLES).contains(&opts.max_particles) {
        bail!("颗粒上限需在 {MIN_MAX_PARTICLES} 到 {MAX_MAX_PARTICLES} 之间");
    }
    let custom_size_a = match opts.custom_size_a {
        Some(v) => {
            if !v.is_finite() || !(40.0..=2000.0).contains(&v) {
                bail!("自定义颗粒大小需在 40 到 2000 Å（4 到 200 nm）之间");
            }
            Some(v)
        }
        None => None,
    };
    let clean = Options {
        symmetry: sym,
        max_particles: opts.max_particles,
        custom_size_a,
        refine_2d_poses: opts.refine_2d_poses,
    };
    std::fs::create_dir_all(out)?;
    std::fs::write(file_of(out), serde_json::to_vec_pretty(&clean)?)?;
    Ok(clean)
}

/// Symmetry matrices for `dir` (falls back to `[IDENTITY]` on C1 or missing file).
pub fn symmetry_group(dir: &Path) -> Vec<Mat> {
    let o = load(dir);
    if o.symmetry == "C1" {
        return vec![IDENTITY];
    }
    symmetry(&o.symmetry).unwrap_or_else(|_| vec![IDENTITY])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_round_trip_and_inherit_from_parent() {
        let dir = tempfile::tempdir().unwrap();
        let saved = save(dir.path(), &Options { symmetry: "c6".into(), max_particles: 8000, custom_size_a: Some(180.0), refine_2d_poses: true }).unwrap();
        assert_eq!(saved.symmetry, "C6");
        let sub = dir.path().join("drawn");
        std::fs::create_dir_all(&sub).unwrap();
        let got = load(&sub);
        assert_eq!(got, saved);
        assert_eq!(symmetry_group(&sub).len(), 6);
        assert!(save(dir.path(), &Options { symmetry: "Z9".into(), ..Default::default() }).is_err());
        assert!(save(dir.path(), &Options { max_particles: 5, ..Default::default() }).is_err());
    }
}
