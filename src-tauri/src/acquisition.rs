//! One recursive acquisition index per operation. Recorded paths are relocation hints,
//! never a reason to prefer an old machine's file over an ambiguous local basename.
use anyhow::{bail, Context, Result};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};
#[derive(Debug)]
pub struct Index {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub warnings: Vec<String>,
    names: BTreeMap<String, Vec<PathBuf>>,
}
impl Index {
    pub fn scan(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("无法读取采集目录 {}", root.display()))?;
        let mut out = Self {
            root: root.clone(),
            files: vec![],
            warnings: vec![],
            names: BTreeMap::new(),
        };
        let mut seen = BTreeSet::new();
        let mut pending = vec![(root, 0usize)];
        let mut count = 0usize;
        let output =
            std::env::var_os("TMV_OUTPUT_ROOT").and_then(|p| std::fs::canonicalize(p).ok());
        while let Some((dir, depth)) = pending.pop() {
            let real = match dir.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    out.warnings.push(format!("{}：{e}", dir.display()));
                    continue;
                }
            };
            if output.as_ref().is_some_and(|o| real.starts_with(o)) || !seen.insert(real.clone()) {
                continue;
            }
            if depth > 64 {
                bail!("采集目录超过 64 层，请缩小检索范围：{}", dir.display())
            }
            let entries = match std::fs::read_dir(&real) {
                Ok(e) => e,
                Err(e) => {
                    out.warnings
                        .push(format!("无法检索 {}：{e}", dir.display()));
                    continue;
                }
            };
            for entry in entries {
                let e = entry.with_context(|| format!("检索 {} 失败", dir.display()))?;
                count += 1;
                if count > 1_000_000 {
                    bail!("采集目录超过一百万项，请选择更具体的数据目录")
                }
                if e.file_name() == ".git" {
                    continue;
                }
                let p = e.path();
                let meta = match std::fs::metadata(&p) {
                    Ok(m) => m,
                    Err(e) => {
                        out.warnings.push(format!("无法读取 {}：{e}", p.display()));
                        continue;
                    }
                };
                if meta.is_dir() {
                    pending.push((p, depth + 1));
                } else if meta.is_file() {
                    let path = p.canonicalize()?;
                    out.names
                        .entry(e.file_name().to_string_lossy().into_owned())
                        .or_default()
                        .push(path.clone());
                    out.files.push(path);
                }
            }
        }
        out.files.sort();
        out.files.dedup();
        for paths in out.names.values_mut() {
            paths.sort();
            paths.dedup();
        }
        out.warnings.truncate(128);
        Ok(out)
    }
    pub fn gains(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|p| {
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase();
                let ext = p
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase();
                ext == "gain"
                    || ((name.contains("gain")
                        || name.contains("countref")
                        || name.starts_with("norm-"))
                        && matches!(ext.as_str(), "mrc" | "tif" | "tiff"))
            })
            .cloned()
            .collect()
    }
    pub fn candidates(&self, recorded: &str) -> Vec<PathBuf> {
        let name = recorded
            .trim()
            .trim_matches('"')
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("");
        if name.is_empty() {
            return vec![];
        }
        if let Some(paths) = self.names.get(name) {
            return paths.clone();
        }
        // Windows acquisition names sometimes changed case during transfer. Still unique only.
        let mut found = self
            .names
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .flat_map(|(_, p)| p.iter().cloned())
            .collect::<Vec<_>>();
        found.sort();
        found.dedup();
        found
    }
}
thread_local! {static ACTIVE:RefCell<Vec<Arc<Index>>>=const{RefCell::new(Vec::new())};}
pub struct Scope;
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|v| {
            v.borrow_mut().pop();
        });
    }
}
pub fn scope(root: &Path) -> Result<Scope> {
    let root = root.canonicalize()?;
    let index = ACTIVE.with(|v| v.borrow().last().filter(|i| i.root == root).cloned());
    let index = match index {
        Some(i) => i,
        None => Arc::new(Index::scan(&root)?),
    };
    ACTIVE.with(|v| v.borrow_mut().push(index));
    Ok(Scope)
}
pub fn active() -> Option<Arc<Index>> {
    ACTIVE.with(|v| v.borrow().last().cloned())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recursively_matches_formats_by_name_and_rejects_duplicates() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        for folder in [
            "records/session/day",
            "movies/camera/tiffs",
            "movies/other/mrc",
            "movies/third",
        ] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        for name in [
            "records/session/day/ts.mdoc",
            "movies/camera/tiffs/A.tiff",
            "movies/other/mrc/B.mrcs",
            "movies/third/C.eer",
        ] {
            std::fs::write(root.join(name), b"fixture").unwrap();
        }
        let i = Index::scan(root).unwrap();
        assert_eq!(i.files.len(), 4);
        for recorded in [r"Z:\old\A.tiff", "/old/work/B.mrcs", "C.eer"] {
            assert_eq!(i.candidates(recorded).len(), 1);
        }
        std::fs::write(root.join("movies/third/A.tiff"), b"another").unwrap();
        assert_eq!(Index::scan(root).unwrap().candidates("A.tiff").len(), 2);
    }
    #[cfg(unix)]
    #[test]
    fn directory_symlinks_do_not_loop_or_duplicate_inputs() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("movie.tif"), b"x").unwrap();
        std::os::unix::fs::symlink(t.path(), t.path().join("loop")).unwrap();
        assert_eq!(
            Index::scan(t.path()).unwrap().candidates("movie.tif").len(),
            1
        );
    }
}
