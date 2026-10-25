//! Installable plugin packages.
//!
//! A package is a .zip whose root holds `plugin.json` and the plugin's files
//! (a single wrapping folder is tolerated). Installed packages live in
//! `<plugin dir>/<id>/`.

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;

/// Plugin API version this host implements (`PLUGIN_API_VERSION` in plugin-host.js).
pub const API_VERSION: u64 = 1;
/// Plugins that ship inside the app and cannot be installed over.
pub const BUILTIN: &[&str] = &["i18n"];
pub const MANIFEST: &str = "plugin.json";

const MAX_ENTRIES: usize = 500;
const MAX_TOTAL: u64 = 50 << 20;
const MAX_MANIFEST: u64 = 1 << 20;

/// `[a-z0-9-]+`, at most 64 characters.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// One component of a relative path inside a package.
fn valid_component(c: &str) -> bool {
    !c.is_empty() && !c.starts_with('.') && !c.contains(['\\', ':', '\0'])
}

/// Splits a `/`-separated relative path, or `None` if any component is unsafe.
pub fn safe_rel(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in path.split('/') {
        if !valid_component(c) {
            return None;
        }
        out.push(c);
    }
    Some(out)
}

/// Compares dotted numeric versions ("0.10.2" > "0.9"); a pre-release or
/// build suffix ("-beta", "+1") is ignored.
pub fn compare_versions(a: &str, b: &str) -> Result<Ordering> {
    fn parse(v: &str) -> Result<Vec<u64>> {
        let core = v.trim().trim_start_matches('v').split(['-', '+']).next().unwrap_or("");
        core.split('.').map(|p| p.parse::<u64>().map_err(|_| anyhow!("版本号格式不对：{v}"))).collect()
    }
    let (mut a, mut b) = (parse(a)?, parse(b)?);
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    Ok(a.cmp(&b))
}

/// The fields of `plugin.json` the host relies on; `raw` keeps the rest.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub id: String,
    pub main: String,
    pub raw: Value,
}

pub fn parse_manifest(bytes: &[u8], app_version: &str) -> Result<Manifest> {
    let raw: Value = serde_json::from_slice(bytes).context("plugin.json 不是有效的 JSON")?;
    let s = |k: &str| raw.get(k).and_then(Value::as_str);
    let id = s("id").ok_or_else(|| anyhow!("plugin.json 缺少 id"))?.to_string();
    if !valid_id(&id) {
        bail!("插件 id 只能用小写字母、数字和 -：{id}");
    }
    if s("name").is_none_or(str::is_empty) {
        bail!("plugin.json 缺少 name");
    }
    let version = s("version").ok_or_else(|| anyhow!("plugin.json 缺少 version"))?;
    compare_versions(version, "0")?;
    let main = s("main").unwrap_or("index.js").to_string();
    if safe_rel(&main).is_none() || !(main.ends_with(".js") || main.ends_with(".mjs")) {
        bail!("main 不是插件里的 .js 文件：{main}");
    }
    let api = match raw.get("apiVersion") {
        None => 1,
        Some(v) => v.as_u64().ok_or_else(|| anyhow!("apiVersion 必须是整数"))?,
    };
    if api > API_VERSION {
        bail!("插件需要更新的 TifMrcView（插件接口版本 {api}，当前 {API_VERSION}）");
    }
    if let Some(min) = s("minAppVersion") {
        if compare_versions(min, app_version)? == Ordering::Greater {
            bail!("插件需要 TifMrcView {min} 或更新版本（当前 {app_version}）");
        }
    }
    Ok(Manifest { id, main, raw })
}

/// Reads and checks `<pkg>/plugin.json`, including that `main` exists.
pub fn read_installed(pkg: &Path, app_version: &str) -> Result<Manifest> {
    let bytes = fs::read(pkg.join(MANIFEST)).context("读不到 plugin.json")?;
    let m = parse_manifest(&bytes, app_version)?;
    if pkg.file_name().and_then(|n| n.to_str()) != Some(m.id.as_str()) {
        bail!("文件夹名和插件 id（{}）不一致", m.id);
    }
    if !pkg.join(safe_rel(&m.main).unwrap()).is_file() {
        bail!("插件里没有 {}", m.main);
    }
    Ok(m)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub id: String,
    pub dir: String,
    pub main: Option<String>,
    pub manifest: Option<Value>,
    pub error: Option<String>,
}

/// Lists the package folders in `dir`; broken ones carry `error`.
pub fn scan(dir: &Path, app_version: &str) -> Vec<Installed> {
    let Ok(rd) = fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<Installed> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            // Hidden folders are install leftovers.
            if name.starts_with('.') {
                return None;
            }
            let pkg = e.path();
            let dir = pkg.to_string_lossy().into_owned();
            Some(match read_installed(&pkg, app_version) {
                Ok(m) => Installed { id: m.id, dir, main: Some(m.main), manifest: Some(m.raw), error: None },
                Err(err) => Installed { id: name, dir, main: None, manifest: None, error: Some(format!("{err:#}")) },
            })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Whether a package with this id is installed (cheap; used on every native call).
pub fn is_installed(dir: &Path, id: &str) -> bool {
    valid_id(id) && dir.join(id).join(MANIFEST).is_file()
}

/// Where each archive entry goes: `None` to skip it (folders, macOS junk).
fn entry_target(name: &str, strip: &str) -> Result<Option<PathBuf>> {
    if name.starts_with('/') || name.contains('\\') || name.contains(':') {
        bail!("压缩包里有不安全的路径：{name}");
    }
    if name.split('/').any(|c| c == "..") {
        bail!("压缩包里有不安全的路径：{name}");
    }
    let rel = name.strip_prefix(strip).unwrap_or(name);
    let rel = rel.trim_end_matches('/');
    if rel.is_empty() || name.ends_with('/') {
        return Ok(None);
    }
    if rel.split('/').any(|c| c.starts_with('.') || c == "__MACOSX") {
        return Ok(None);
    }
    safe_rel(rel).map(Some).ok_or_else(|| anyhow!("压缩包里有不安全的路径：{name}"))
}

/// The folder every entry sits in when the zip wraps the plugin in one, else "".
fn wrapper(names: &[String]) -> String {
    if names.iter().any(|n| n == MANIFEST) {
        return String::new();
    }
    let real: Vec<&String> = names.iter().filter(|n| !n.starts_with("__MACOSX/")).collect();
    let Some(first) = real.first() else { return String::new() };
    let Some((top, _)) = first.split_once('/') else { return String::new() };
    let prefix = format!("{top}/");
    if real.iter().all(|n| n.starts_with(&prefix)) && names.iter().any(|n| *n == format!("{prefix}{MANIFEST}")) {
        prefix
    } else {
        String::new()
    }
}

fn unique_tmp(dir: &Path, tag: &str, id: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    dir.join(format!(".{tag}-{id}-{}-{nanos}", std::process::id()))
}

/// Installs (or replaces) the package at `zip_path` into `dir/<id>/`.
pub fn install(dir: &Path, zip_path: &Path, app_version: &str) -> Result<Manifest> {
    let file = File::open(zip_path).with_context(|| format!("无法打开 {}", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("不是有效的 zip 插件包")?;
    if zip.len() > MAX_ENTRIES {
        bail!("插件包里的文件太多（{} 个，最多 {MAX_ENTRIES} 个）", zip.len());
    }
    let names: Vec<String> = zip.file_names().map(str::to_string).collect();
    let strip = wrapper(&names);

    let manifest = {
        let mut f = zip.by_name(&format!("{strip}{MANIFEST}")).map_err(|_| anyhow!("插件包里没有 plugin.json"))?;
        let mut buf = vec![];
        (&mut f).take(MAX_MANIFEST).read_to_end(&mut buf)?;
        parse_manifest(&buf, app_version)?
    };
    if BUILTIN.contains(&manifest.id.as_str()) {
        bail!("{} 是内置插件，不能另外安装", manifest.id);
    }

    fs::create_dir_all(dir).context("无法创建插件文件夹")?;
    let tmp = unique_tmp(dir, "install", &manifest.id);
    let res = extract(&mut zip, &strip, &tmp).and_then(|_| {
        if !tmp.join(safe_rel(&manifest.main).unwrap()).is_file() {
            bail!("插件包里没有 {}", manifest.main);
        }
        replace(dir, &manifest.id, &tmp)
    });
    if res.is_err() {
        let _ = fs::remove_dir_all(&tmp);
    }
    res.map(|_| manifest)
}

fn extract(zip: &mut zip::ZipArchive<File>, strip: &str, tmp: &Path) -> Result<()> {
    fs::create_dir_all(tmp)?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let name = f.name().to_string();
        if f.is_symlink() {
            bail!("插件包里不能有符号链接：{name}");
        }
        let Some(rel) = entry_target(&name, strip)? else { continue };
        if f.is_dir() {
            continue;
        }
        let out = tmp.join(rel);
        if let Some(p) = out.parent() {
            fs::create_dir_all(p)?;
        }
        let mut w = File::create(&out).with_context(|| format!("无法写入 {}", out.display()))?;
        // The sizes in the archive can lie; count what actually comes out.
        let left = MAX_TOTAL - total;
        let n = std::io::copy(&mut (&mut f).take(left + 1), &mut w)?;
        total += n;
        if total > MAX_TOTAL {
            bail!("插件包解压后太大（超过 {} MB）", MAX_TOTAL >> 20);
        }
        w.flush()?;
    }
    Ok(())
}

/// Swaps `tmp` in as `dir/<id>`, keeping the old copy until that worked.
fn replace(dir: &Path, id: &str, tmp: &Path) -> Result<()> {
    let target = dir.join(id);
    if !target.exists() {
        return fs::rename(tmp, &target).context("无法安装插件");
    }
    let old = unique_tmp(dir, "old", id);
    fs::rename(&target, &old).context("无法替换旧版本（插件文件可能正被占用）")?;
    if let Err(e) = fs::rename(tmp, &target) {
        let _ = fs::rename(&old, &target);
        return Err(e).context("无法安装插件");
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

/// Removes `dir/<id>/`.
pub fn uninstall(dir: &Path, id: &str) -> Result<()> {
    if !valid_id(id) {
        bail!("插件 id 不对：{id}");
    }
    let target = dir.join(id);
    if !target.is_dir() {
        bail!("没有安装插件 {id}");
    }
    fs::remove_dir_all(&target).with_context(|| format!("无法删除 {}", target.display()))
}

/// Maps a `tmvplugin://` path (already percent-decoded, no leading slash) to
/// a file in `dir`: `<file>.js` for loose legacy plugins, or `<id>/<rel>`
/// inside an installed package.
pub fn resolve(dir: &Path, path: &str) -> Option<(PathBuf, &'static str)> {
    let Some((id, rel)) = path.split_once('/') else {
        if !valid_component(path) || !path.ends_with(".js") {
            return None;
        }
        let file = dir.join(path);
        return file.is_file().then_some((file, "text/javascript; charset=utf-8"));
    };
    if !valid_id(id) {
        return None;
    }
    let rel = safe_rel(rel)?;
    let ext = rel.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "woff2" => "font/woff2",
        _ => return None,
    };
    let pkg = dir.join(id);
    if !pkg.join(MANIFEST).is_file() {
        return None;
    }
    let pkg = pkg.canonicalize().ok()?;
    let file = pkg.join(rel).canonicalize().ok()?;
    (file.starts_with(&pkg) && file.is_file()).then_some((file, mime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    const OK: &str = r#"{"id":"demo","name":"Demo","version":"1.2.0","main":"index.js","apiVersion":1,"minAppVersion":"0.2.0"}"#;

    fn make_zip(path: &Path, files: &[(&str, &str)]) {
        let mut w = zip::ZipWriter::new(File::create(path).unwrap());
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in files {
            if name.ends_with('/') {
                w.add_directory(*name, opts).unwrap();
            } else {
                w.start_file(*name, opts).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
        }
        w.finish().unwrap();
    }

    #[test]
    fn versions() {
        assert_eq!(compare_versions("0.10.0", "0.9.9").unwrap(), Ordering::Greater);
        assert_eq!(compare_versions("0.2", "0.2.0").unwrap(), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0-beta", "1.0.0").unwrap(), Ordering::Equal);
        assert_eq!(compare_versions("v0.2.1", "0.3.0").unwrap(), Ordering::Less);
        assert!(compare_versions("1.x", "1.0").is_err());
    }

    #[test]
    fn manifests() {
        let m = parse_manifest(OK.as_bytes(), "0.2.0").unwrap();
        assert_eq!((m.id.as_str(), m.main.as_str()), ("demo", "index.js"));
        let bad = |json: &str| parse_manifest(json.as_bytes(), "0.2.0").is_err();
        assert!(bad("not json"));
        assert!(bad(r#"{"id":"Demo","name":"x","version":"1.0.0"}"#));
        assert!(bad(r#"{"id":"a b","name":"x","version":"1.0.0"}"#));
        assert!(bad(r#"{"id":"demo","version":"1.0.0"}"#));
        assert!(bad(r#"{"id":"demo","name":"x"}"#));
        assert!(bad(r#"{"id":"demo","name":"x","version":"1.0.0","main":"../x.js"}"#));
        assert!(bad(r#"{"id":"demo","name":"x","version":"1.0.0","main":"index.html"}"#));
        assert!(bad(r#"{"id":"demo","name":"x","version":"1.0.0","apiVersion":2}"#));
        assert!(bad(r#"{"id":"demo","name":"x","version":"1.0.0","minAppVersion":"0.10.0"}"#));
        assert!(!bad(r#"{"id":"demo-2","name":"x","version":"1.0.0","minAppVersion":"0.1.9"}"#));
    }

    #[test]
    fn rejects_unsafe_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins");
        for evil in ["../evil.js", "a/../../evil.js", "/abs.js", "a\\b.js", "C:/x.js"] {
            let z = tmp.path().join("evil.zip");
            make_zip(&z, &[("plugin.json", OK), ("index.js", "1"), (evil, "x")]);
            let err = install(&dir, &z, "0.2.0").unwrap_err();
            assert!(format!("{err:#}").contains("不安全"), "{evil}: {err:#}");
            assert!(!dir.join("demo").exists());
        }
        assert!(!tmp.path().join("evil.js").exists());
        // Nothing left behind.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn rejects_symlinks_builtin_and_missing_main() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins");
        let z = tmp.path().join("p.zip");
        let mut w = zip::ZipWriter::new(File::create(&z).unwrap());
        let opts = SimpleFileOptions::default();
        w.start_file("plugin.json", opts).unwrap();
        w.write_all(OK.as_bytes()).unwrap();
        w.start_file("index.js", opts).unwrap();
        w.add_symlink("link.js", "/etc/passwd", opts).unwrap();
        w.finish().unwrap();
        assert!(format!("{:#}", install(&dir, &z, "0.2.0").unwrap_err()).contains("符号链接"));

        make_zip(&z, &[("plugin.json", &OK.replace("demo", "i18n")), ("index.js", "1")]);
        assert!(install(&dir, &z, "0.2.0").is_err());
        make_zip(&z, &[("plugin.json", OK)]);
        assert!(install(&dir, &z, "0.2.0").is_err());
        make_zip(&z, &[("index.js", "1")]);
        assert!(install(&dir, &z, "0.2.0").is_err());
        assert!(!dir.join("demo").exists() && !dir.join("i18n").exists());
    }

    #[test]
    fn rejects_huge_archives() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins");
        let z = tmp.path().join("big.zip");
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("f{i}.js")).collect();
        let mut files = vec![("plugin.json", OK), ("index.js", "1")];
        files.extend(names.iter().map(|n| (n.as_str(), "")));
        make_zip(&z, &files);
        assert!(install(&dir, &z, "0.2.0").is_err());

        // Compresses to almost nothing, inflates past the limit.
        let big = "0".repeat((MAX_TOTAL + 1) as usize);
        make_zip(&z, &[("plugin.json", OK), ("index.js", "1"), ("blob.js", &big)]);
        assert!(format!("{:#}", install(&dir, &z, "0.2.0").unwrap_err()).contains("太大"));
        assert!(!dir.join("demo").exists());
    }

    #[test]
    fn install_replace_uninstall() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins");
        let z = tmp.path().join("demo.zip");
        make_zip(&z, &[("plugin.json", OK), ("index.js", "v1"), ("lib/", ""), ("lib/a.js", "a"), (".DS_Store", "")]);
        let m = install(&dir, &z, "0.2.0").unwrap();
        assert_eq!(m.raw["version"], "1.2.0");
        assert_eq!(fs::read_to_string(dir.join("demo/index.js")).unwrap(), "v1");
        assert!(dir.join("demo/lib/a.js").is_file() && !dir.join("demo/.DS_Store").exists());
        assert!(is_installed(&dir, "demo"));

        // Upgrade, wrapped in a folder (as a re-zipped download would be).
        make_zip(&z, &[("demo-1.3/plugin.json", &OK.replace("1.2.0", "1.3.0")), ("demo-1.3/index.js", "v2")]);
        install(&dir, &z, "0.2.0").unwrap();
        assert_eq!(fs::read_to_string(dir.join("demo/index.js")).unwrap(), "v2");
        assert!(!dir.join("demo/lib").exists());

        fs::write(dir.join("legacy.js"), "x").unwrap();
        fs::create_dir_all(dir.join("broken")).unwrap();
        let list = scan(&dir, "0.2.0");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "broken");
        assert!(list[0].error.is_some());
        assert_eq!(list[1].id, "demo");
        assert_eq!(list[1].manifest.as_ref().unwrap()["version"], "1.3.0");
        // Only the package and the leftovers we made ourselves remain.
        let mut left: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        left.sort();
        assert_eq!(left, ["broken", "demo", "legacy.js"]);

        assert!(uninstall(&dir, "../plugins").is_err());
        uninstall(&dir, "demo").unwrap();
        assert!(!dir.join("demo").exists() && !is_installed(&dir, "demo"));
        assert!(uninstall(&dir, "demo").is_err());
    }

    #[test]
    fn serve_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins");
        fs::create_dir_all(dir.join("demo/lib")).unwrap();
        fs::write(dir.join("demo/plugin.json"), OK).unwrap();
        fs::write(dir.join("demo/index.js"), "1").unwrap();
        fs::write(dir.join("demo/lib/a.css"), "1").unwrap();
        fs::write(dir.join("demo/.hidden.js"), "1").unwrap();
        fs::write(dir.join("demo/x.exe"), "1").unwrap();
        fs::write(dir.join("legacy.js"), "1").unwrap();
        fs::write(tmp.path().join("secret.js"), "1").unwrap();

        assert_eq!(resolve(&dir, "demo/index.js").unwrap().1, "text/javascript; charset=utf-8");
        assert_eq!(resolve(&dir, "demo/lib/a.css").unwrap().1, "text/css; charset=utf-8");
        assert!(resolve(&dir, "legacy.js").is_some());
        for bad in [
            "../secret.js",
            "demo/../../secret.js",
            "demo/../legacy.js",
            "demo/./index.js",
            "demo//index.js",
            "demo/.hidden.js",
            "demo/lib\\a.css",
            "demo/x.exe",
            "demo/missing.js",
            "Demo/index.js",
            ".hidden.js",
            "legacy.txt",
            "",
        ] {
            assert!(resolve(&dir, bad).is_none(), "{bad}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.path().join("secret.js"), dir.join("demo/out.js")).unwrap();
            assert!(resolve(&dir, "demo/out.js").is_none());
        }
    }
}
