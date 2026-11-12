//! The pixel size of tomograms whose files do not say it (the header has 0
//! or 1, no mdoc beside them). The user types it once for a project; it is
//! kept in `<out>/pixel.json` and given to `tomo::output::set_pixel` again
//! whenever a command works on the project.

use std::path::Path;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::tomo::output::{pixel_known, pixel_override, set_pixel};

/// What `<out>/pixel.json` holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    /// Å per pixel for every file of the project that does not say its own.
    pub pixel_a: f64,
    /// The files the user filled it in for.
    pub files: Vec<String>,
}

fn file_of(out: &Path) -> std::path::PathBuf {
    out.join("pixel.json")
}

/// The saved pixel size of the project of `out`, when there is a usable one.
pub fn load(out: &Path) -> Option<Saved> {
    let s: Saved = serde_json::from_slice(&std::fs::read(file_of(out)).ok()?).ok()?;
    (s.pixel_a.is_finite() && s.pixel_a > 0.0).then_some(s)
}

fn unknown(file: &str) -> bool {
    pixel_override(file).is_none()
        && crate::formats::open(Path::new(file)).map(|s| crate::image::OpenImage::new(1, s, 1 << 20)).map(|img| pixel_known(&img).is_none()).unwrap_or(false)
}

/// Gives the saved pixel size of `out` to the files of the project that do not
/// say theirs: the files it was filled in for and the `files` given.
pub fn apply(out: &Path, files: &[String]) {
    let Some(saved) = load(out) else { return };
    for f in saved.files.iter() {
        set_pixel(f, Some(saved.pixel_a));
    }
    for f in files {
        if !saved.files.contains(f) && unknown(f) {
            set_pixel(f, Some(saved.pixel_a));
        }
    }
}

/// The files among `files` whose pixel size is not known (after `apply` of
/// the saved one for `out`).
pub fn needed(out: &Path, files: &[String]) -> Vec<String> {
    apply(out, files);
    files.iter().filter(|f| unknown(f)).cloned().collect()
}

/// Remembers `pixel_a` for `files` in `out` and applies it.
pub fn set(out: &Path, files: &[String], pixel_a: f64) -> Result<()> {
    if !pixel_a.is_finite() || pixel_a <= 0.0 || pixel_a > 1000.0 {
        bail!("像素大小要是一个大于 0 的数（单位 Å）");
    }
    let mut all = load(out).map(|s| s.files).unwrap_or_default();
    for f in files {
        if !all.contains(f) {
            all.push(f.clone());
        }
    }
    std::fs::create_dir_all(out)?;
    std::fs::write(file_of(out), serde_json::to_vec_pretty(&Saved { pixel_a, files: all.clone() })?)?;
    for f in &all {
        set_pixel(f, Some(pixel_a));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_pixel_is_given_back_to_the_files() {
        let dir = std::env::temp_dir().join(format!("tmv_pixelsta_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.mrc").to_string_lossy().into_owned();
        crate::tomo::output::write_mrc(&f, &vec![0.5; 4 * 4 * 2], [4, 4, 2], 0.0, false).unwrap();
        let files = vec![f.clone()];
        assert_eq!(needed(&dir, &files), files);
        assert!(set(&dir, &files, 0.0).is_err());
        set(&dir, &files, 4.2).unwrap();
        assert_eq!(pixel_override(&f), Some(4.2));
        set_pixel(&f, None);
        assert!(needed(&dir, &files).is_empty());
        assert_eq!(pixel_override(&f), Some(4.2));
        set_pixel(&f, None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
