//! File persistence for editable merges. First publication never overwrites a
//! name; later saves replace the complete file atomically in the same folder.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::Value;

static NEXT: AtomicU64 = AtomicU64::new(0);

fn text(project: &Value) -> Result<String> {
    if project["format"] != "tifmrcview-merge" || project["version"] != 1 {
        bail!("无效的合成工程或版本");
    }
    let text = serde_json::to_string_pretty(project)?;
    if text.len() > 4 * 1024 * 1024 { bail!("合成工程文件过大"); }
    Ok(text)
}

fn name(project: &Value) -> String {
    let raw = project["name"].as_str().unwrap_or("合成图");
    let raw = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let raw = if raw.to_ascii_lowercase().ends_with(".tmvmerge") { &raw[..raw.len() - 9] } else { raw };
    let mut safe = String::new();
    for ch in raw.chars() {
        let ch = if ch.is_control() || "<>:\"/\\|?*".contains(ch) { '_' } else { ch };
        if safe.len() + ch.len_utf8() > 160 { break; }
        safe.push(ch);
    }
    let safe = safe.trim_matches(|ch: char| ch == '.' || ch.is_whitespace());
    if safe.is_empty() { return "合成图".into(); }
    let first = safe.split('.').next().unwrap_or("").to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&first.as_str())
        || (first.len() == 4 && (first.starts_with("COM") || first.starts_with("LPT")) && matches!(first.as_bytes()[3], b'1'..=b'9')) {
        return format!("_{safe}");
    }
    safe.into()
}

struct Staged(PathBuf);
impl Staged {
    fn write(dir: &Path, text: &str) -> Result<Self> {
        for _ in 0..8 {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let path = dir.join(format!(".tmvmerge-{}-{stamp}-{}.part", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            };
            let staged = Self(path);
            let written = file.write_all(text.as_bytes()).and_then(|_| file.sync_all());
            drop(file); // Close before cleanup as Windows cannot unlink an open file.
            written?;
            return Ok(staged);
        }
        bail!("无法创建合成工程临时文件")
    }
}
impl Drop for Staged {
    fn drop(&mut self) { let _ = fs::remove_file(&self.0); }
}

#[cfg(target_os = "macos")]
fn publish_new(staged: &Path, target: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e));
    let (from, to) = (path(staged)?, path(target)?);
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(not(target_os = "macos"))]
fn publish_new(staged: &Path, target: &Path) -> io::Result<()> {
    // A hard link publishes an already-complete file and fails if any entry
    // (including a symlink) owns this name. Unsupported filesystems fall back
    // to the App's own storage. Staged removes the temporary name on drop.
    fs::hard_link(staged, target)
}

fn sync_parent(dir: &Path) {
    // Some platforms cannot open/sync directories; the payload is already synced.
    if let Ok(file) = File::open(dir) { let _ = file.sync_all(); }
}

fn create_in(dir: &Path, name: &str, text: &str) -> Result<PathBuf> {
    let dir = fs::canonicalize(dir)?;
    let staged = Staged::write(&dir, text)?;
    for n in 0..10_000 {
        let leaf = if n == 0 { format!("{name}.tmvmerge") } else { format!("{name} ({n}).tmvmerge") };
        let path = dir.join(leaf);
        match publish_new(&staged.0, &path) {
            Ok(()) => { sync_parent(&dir); return Ok(path); }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    bail!("同名合成工程过多，请更改名称")
}

pub(crate) fn create(source: &Path, fallback: Option<&Path>, project: &Value) -> Result<PathBuf> {
    let text = text(project)?;
    let name = name(project);
    let primary = if source.is_dir() { source } else { source.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")) };
    match create_in(primary, &name, &text) {
        Ok(path) => Ok(path),
        Err(primary_error) => {
            let fallback = fallback.context(format!("原图旁无法保存（{primary_error}），也无法确定应用保存目录"))?;
            fs::create_dir_all(fallback).with_context(|| format!("原图旁无法保存（{primary_error}），应用保存目录也不可写"))?;
            create_in(fallback, &name, &text).with_context(|| format!("原图旁无法保存（{primary_error}），应用保存目录也无法保存"))
        }
    }
}

pub(crate) fn save(path: &Path, project: &Value) -> Result<()> {
    let text = text(project)?;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let staged = Staged::write(dir, &text)?;
    fs::rename(&staged.0, path)?;
    sync_parent(dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(name: &str, revision: usize) -> Value { json!({"format": "tifmrcview-merge", "version": 1, "name": name, "revision": revision, "layers": []}) }
    fn read(path: &Path) -> Value { serde_json::from_slice(&fs::read(path).unwrap()).unwrap() }

    #[test]
    fn creates_beside_source_without_overwriting_and_accepts_folder_sources() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("合成图.tmvmerge");
        fs::write(&existing, b"previous user data").unwrap();
        let p = project("合成图", 1);
        let a = create(&dir.path().join("image.mrc"), None, &p).unwrap();
        let b = create(dir.path(), None, &p).unwrap();
        assert_eq!(a.file_name().unwrap(), "合成图 (1).tmvmerge");
        assert_eq!(b.file_name().unwrap(), "合成图 (2).tmvmerge");
        assert!(a.is_absolute()); assert_eq!(read(&a), p);
        assert_eq!(fs::read(existing).unwrap(), b"previous user data");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn safe_names_cannot_escape_source_and_existing_symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let p = create(dir.path(), None, &project("C:\\elsewhere\\../..//安全:名称.tmvmerge", 0)).unwrap();
        assert_eq!(p.parent().unwrap(), fs::canonicalize(dir.path()).unwrap());
        assert_eq!(p.file_name().unwrap(), "安全_名称.tmvmerge");
        assert!(name(&project("CON", 0)).starts_with('_'));
        assert!(name(&project(&"🙂".repeat(100), 0)).len() <= 160);
        #[cfg(unix)] {
            let target = dir.path().join("keep.txt"); fs::write(&target, b"keep").unwrap();
            std::os::unix::fs::symlink(&target, dir.path().join("linked.tmvmerge")).unwrap();
            let made = create(dir.path(), None, &project("linked", 0)).unwrap();
            assert_eq!(made.file_name().unwrap(), "linked (1).tmvmerge");
            assert_eq!(fs::read(target).unwrap(), b"keep");
        }
    }

    #[test]
    fn unusable_source_folder_falls_back_to_app_storage_and_both_failures_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-folder"); fs::write(&blocker, b"keep").unwrap();
        let fallback = dir.path().join("app/merge-projects");
        let p = create(&blocker.join("image.mrc"), Some(&fallback), &project("merge", 0)).unwrap();
        assert_eq!(p.parent().unwrap(), fs::canonicalize(fallback).unwrap());
        assert!(create(&blocker.join("image.mrc"), Some(&blocker), &project("merge", 0)).is_err());
        assert_eq!(fs::read(blocker).unwrap(), b"keep");
    }

    #[test]
    fn concurrent_creations_publish_distinct_complete_projects() {
        let dir = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..8).map(|i| { let path = dir.path(); scope.spawn(move || create(path, None, &project("merge", i)).unwrap()) }).collect();
            let paths: std::collections::HashSet<_> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
            assert_eq!(paths.len(), 8);
            let revisions: std::collections::HashSet<_> = paths.iter().map(|p| read(p)["revision"].as_u64().unwrap()).collect();
            assert_eq!(revisions.len(), 8);
        });
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 8);
    }

    #[test]
    fn updates_publish_latest_revision_and_failures_preserve_existing_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = create(dir.path(), None, &project("merge", 0)).unwrap();
        for revision in 1..5 { save(&path, &project("merge", revision)).unwrap(); assert_eq!(read(&path)["revision"], revision); }
        let before = fs::read(&path).unwrap();
        assert!(save(&path, &json!({"format": "bad", "version": 1})).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        let destination_dir = dir.path().join("a-directory"); fs::create_dir(&destination_dir).unwrap();
        assert!(save(&destination_dir, &project("merge", 5)).is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2, "failed writes clean temporary files");
    }
}
