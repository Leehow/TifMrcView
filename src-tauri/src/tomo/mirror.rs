//! A Cryo-ET project of an SSH server processed on this computer: the files
//! a run needs are copied into a local folder (the "mirror", marked
//! `.tmv-mirror`) at the same places as in the project (what was made in
//! the step folders, the raw data as under the project folder), the run
//! goes there, and what it made can be sent back into the server's output
//! folder. The project files store absolute paths; they are rewritten for
//! where they lie (`relocate::rebase`) on the way in and on the way out.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use serde::Serialize;

const MARK: &str = ".tmv-mirror";
const STAGING: &str = ".tmv-upload";

/// The mirror folder: `dir` (a folder the user chose to keep the results
/// in) or one in this computer's cache for `key`; made and marked.
pub fn prepare(key: &str, dir: Option<&str>) -> Result<String> {
    use sha2::Digest;
    let path = match dir {
        Some(d) => PathBuf::from(d),
        None => {
            let hash: String = sha2::Sha256::digest(key.as_bytes()).iter().take(10).map(|b| format!("{b:02x}")).collect();
            dirs::cache_dir().context("无法定位缓存目录")?.join("tifmrcview/local-runs").join(hash)
        }
    };
    ensure!(path.is_absolute(), "本机文件夹需为绝对路径");
    std::fs::create_dir_all(&path)?;
    std::fs::write(path.join(MARK), b"")?;
    Ok(path.to_string_lossy().into_owned())
}

fn marked(folder: &Path) -> Result<()> {
    ensure!(folder.join(MARK).is_file(), "不是本机处理文件夹：{}", folder.display());
    Ok(())
}

/// Every file of the mirror (not its marks, staging, partial transfers or
/// locks), by its path relative to the mirror with `/`.
fn walk(folder: &Path) -> Vec<(String, PathBuf)> {
    let mut out = vec![];
    let mut stack = vec![folder.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let n = e.file_name().to_string_lossy().into_owned();
            if n == MARK || n == STAGING || n.ends_with(".ssh-part") || n.ends_with(".lock") {
                continue;
            }
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => {
                    let rel = p.strip_prefix(folder).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                    out.push((rel, p));
                }
                _ => {}
            }
        }
    }
    out.sort();
    out
}

fn is_json(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

/// After the copy: the project files' paths under the server's output
/// folder and project folder now point into the mirror.
pub fn rebase_in(folder: &str, project: &str, output: &str) -> Result<()> {
    let f = Path::new(folder);
    marked(f)?;
    for (_, p) in walk(f).into_iter().filter(|(_, p)| is_json(p)) {
        let Ok(mut v) = std::fs::read(&p).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice::<serde_json::Value>(&b)?)) else { continue };
        crate::relocate::rebase(&mut v, output, folder);
        crate::relocate::rebase(&mut v, project, folder);
        std::fs::write(&p, serde_json::to_vec_pretty(&v)?)?;
    }
    Ok(())
}

/// Size and time of every file of the mirror, to tell later what a run made.
pub fn manifest(folder: &str) -> Result<BTreeMap<String, (u64, u128)>> {
    let f = Path::new(folder);
    marked(f)?;
    Ok(walk(f).into_iter().filter_map(|(rel, p)| {
        let m = std::fs::metadata(&p).ok()?;
        let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
        Some((rel, (m.len(), t)))
    }).collect())
}

#[derive(Serialize)]
pub struct Upload {
    pub local: String,
    pub remote: String,
    pub size: u64,
}

/// What the run made or changed in the step folders (against `before`),
/// each with where it goes in the server's output folder. A project file
/// goes as a copy (in the mirror's staging folder) with its paths pointing
/// at the server again: into the output folder for the step folders, the
/// project folder for the rest.
pub fn changes(folder: &str, before: &BTreeMap<String, (u64, u128)>, project: &str, output: &str) -> Result<Vec<Upload>> {
    let f = Path::new(folder);
    marked(f)?;
    let now = manifest(folder)?;
    let staging = f.join(STAGING);
    let _ = std::fs::remove_dir_all(&staging);
    let mut out = vec![];
    for (rel, stamp) in now {
        let step = rel.split('/').next().is_some_and(crate::project_dirs::is_step_folder);
        if !step || before.get(&rel) == Some(&stamp) {
            continue;
        }
        let local = f.join(&rel);
        let send = if is_json(&local) {
            let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&local)?)?;
            rebase_out(&mut v, folder, project, output);
            let copy = staging.join(&rel);
            std::fs::create_dir_all(copy.parent().unwrap())?;
            std::fs::write(&copy, serde_json::to_vec_pretty(&v)?)?;
            copy
        } else {
            local
        };
        let size = std::fs::metadata(&send)?.len();
        out.push(Upload { local: send.to_string_lossy().into_owned(), remote: format!("{}/{rel}", output.trim_end_matches('/')), size });
    }
    Ok(out)
}

fn rebase_out(v: &mut serde_json::Value, folder: &str, project: &str, output: &str) {
    use serde_json::Value;
    match v {
        Value::String(s) => {
            let base = folder.trim_end_matches(['/', '\\']);
            if let Some(rest) = s.strip_prefix(base).and_then(|r| r.strip_prefix(['/', '\\'])) {
                let rest = rest.replace('\\', "/");
                let step = rest.split('/').next().is_some_and(crate::project_dirs::is_step_folder);
                *s = format!("{}/{rest}", if step { output } else { project }.trim_end_matches('/'));
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rebase_out(x, folder, project, output)),
        Value::Object(o) => o.values_mut().for_each(|x| rebase_out(x, folder, project, output)),
        _ => {}
    }
}

/// Removes the mirror (only a marked one).
pub fn remove(folder: &str) -> Result<()> {
    let f = Path::new(folder);
    marked(f)?;
    std::fs::remove_dir_all(f)?;
    Ok(())
}

/// The mirror kept as the user's own project folder: its marks go.
pub fn keep(folder: &str) -> Result<()> {
    let f = Path::new(folder);
    marked(f)?;
    let _ = std::fs::remove_dir_all(f.join(STAGING));
    std::fs::remove_file(f.join(MARK))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_go_in_and_back_out() {
        let dir = tempfile::tempdir().unwrap();
        let m = dir.path().join("m");
        let m = prepare("k", Some(m.to_str().unwrap())).unwrap();
        std::fs::create_dir_all(Path::new(&m).join("2_tomograms")).unwrap();
        let js = Path::new(&m).join("2_tomograms/ts.tomo.json");
        std::fs::write(&js, r#"{"stack":"/srv/out/1_motion/ts.mrc","mdoc":"/data/p/ts.mdoc","other":"/x/y"}"#).unwrap();
        rebase_in(&m, "/data/p", "/srv/out").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&js).unwrap()).unwrap();
        assert_eq!(v["stack"], format!("{m}/1_motion/ts.mrc"));
        assert_eq!(v["mdoc"], format!("{m}/ts.mdoc"));
        assert_eq!(v["other"], "/x/y");
        let before = manifest(&m).unwrap();
        std::fs::write(Path::new(&m).join("raw.txt"), b"x").unwrap();
        std::fs::create_dir_all(Path::new(&m).join("3_clean")).unwrap();
        std::fs::write(Path::new(&m).join("3_clean/t.mrc"), b"abc").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&js, serde_json::to_vec(&v).unwrap()).unwrap();
        let up = changes(&m, &before, "/data/p", "/srv/out").unwrap();
        let remotes: Vec<_> = up.iter().map(|u| u.remote.as_str()).collect();
        assert_eq!(remotes, ["/srv/out/2_tomograms/ts.tomo.json", "/srv/out/3_clean/t.mrc"]);
        let back: serde_json::Value = serde_json::from_slice(&std::fs::read(&up[0].local).unwrap()).unwrap();
        assert_eq!(back["stack"], "/srv/out/1_motion/ts.mrc");
        assert_eq!(back["mdoc"], "/data/p/ts.mdoc");
        remove(&m).unwrap();
        assert!(!Path::new(&m).exists());
    }
}
