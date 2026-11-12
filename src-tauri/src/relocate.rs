//! Project files (`spa.json`, `<stack>.tomo.json`) store absolute paths.
//! When a whole folder is moved, for example processed on a server with
//! the command-line tool (`crate::cli`) and copied back to a desktop, the
//! paths under the old folder are taken to the same place under the new one.

use std::path::{Path, MAIN_SEPARATOR};

/// The folder part of `path`, written with either separator (the file may
/// come from another system).
pub fn parent(path: &str) -> &str {
    let p = path.trim_end_matches(['/', '\\']);
    match p.rfind(['/', '\\']) {
        Some(0) => &p[..1],
        Some(i) => &p[..i],
        None => "",
    }
}

/// `s` moved from under `old` to under `new`, or None when it is not under `old`.
pub fn moved(s: &str, old: &str, new: &str) -> Option<String> {
    let old = old.trim_end_matches(['/', '\\']);
    if old.is_empty() || old == new {
        return None;
    }
    let rest = s.strip_prefix(old)?;
    if rest.is_empty() {
        return Some(new.to_string());
    }
    let rest = rest.strip_prefix(['/', '\\'])?;
    let rest: String = rest.chars().map(|c| if c == '/' || c == '\\' { MAIN_SEPARATOR } else { c }).collect();
    Some(Path::new(new).join(rest).to_string_lossy().into_owned())
}

/// Rewrites every string in `v` under `old` to the same path under `new`.
pub fn rebase(v: &mut serde_json::Value, old: &str, new: &str) {
    use serde_json::Value;
    match v {
        Value::String(s) => {
            if let Some(m) = moved(s, old, new) {
                *s = m;
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rebase(x, old, new)),
        Value::Object(o) => o.values_mut().for_each(|x| rebase(x, old, new)),
        _ => {}
    }
}

/// `value` with the paths under the folder of `was` moved to the folder of
/// `now` (two files at the same place in their folders), when the folders differ.
pub fn relocate<T: serde::Serialize + serde::de::DeserializeOwned>(value: T, was: &str, now: &str) -> T {
    relocate_dirs(value, parent(was), parent(now))
}

/// `value` with the paths under the folder `old` moved to the same place
/// under `new` (a project folder, for the step folders inside it), when they differ.
pub fn relocate_dirs<T: serde::Serialize + serde::de::DeserializeOwned>(value: T, old: &str, new: &str) -> T {
    if old.is_empty() || new.is_empty() || old == new {
        return value;
    }
    let Ok(mut v) = serde_json::to_value(&value) else { return value };
    rebase(&mut v, old, new);
    serde_json::from_value(v).unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_paths_under_the_folder_only() {
        let mut v = serde_json::json!({
            "stack": "/data/run1/ts.mrc",
            "tomograms": [{"path": "/data/run1/ts_rec.mrc"}, {"path": "/other/x.mrc"}],
            "note": "/data/run10/y.mrc",
            "n": 3
        });
        rebase(&mut v, "/data/run1", "/Users/me/run1");
        let sep = MAIN_SEPARATOR;
        assert_eq!(v["stack"], format!("/Users/me/run1{sep}ts.mrc"));
        assert_eq!(v["tomograms"][0]["path"], format!("/Users/me/run1{sep}ts_rec.mrc"));
        assert_eq!(v["tomograms"][1]["path"], "/other/x.mrc");
        assert_eq!(v["note"], "/data/run10/y.mrc");
    }

    #[test]
    fn parent_of_either_separator() {
        assert_eq!(parent("/a/b/c.mrc"), "/a/b");
        assert_eq!(parent("C:\\a\\c.mrc"), "C:\\a");
        assert_eq!(parent("/c.mrc"), "/");
        assert_eq!(parent("c.mrc"), "");
    }
}
