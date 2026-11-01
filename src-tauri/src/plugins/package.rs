//! Installable plugin packages.
//!
//! A package is a .zip whose root holds `plugin.json` and the plugin's files
//! (a single wrapping folder is tolerated). Installed packages live in
//! `<plugin dir>/<id>/`.

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;

/// Highest package API supported. API 1 keeps its trusted legacy loader;
/// API 2 uses the isolated frame bridge in ui/plugin-sandbox.js.
pub const API_VERSION: u64 = 2;
/// API 2 permission vocabulary. Keep in sync with scripts/plugin-manifest.mjs;
/// scripts/tmv-plugin.test.mjs compares both with the sandbox method map.
pub const SANDBOX_PERMISSIONS: &[&str] = &["state", "ui", "metadata", "export", "settings"];
/// Plugins that ship inside the app and cannot be installed over.
pub const BUILTIN: &[&str] = &["i18n"];
pub const MANIFEST: &str = "plugin.json";

const MAX_ENTRIES: usize = 500;
const MAX_TOTAL: u64 = 50 << 20;
pub const MAX_PACKAGE: u64 = 50 << 20;
const MAX_MANIFEST: u64 = 1 << 20;

/// `[a-z0-9-]+`, at most 64 characters.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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
        let core = v
            .trim()
            .trim_start_matches('v')
            .split(['-', '+'])
            .next()
            .unwrap_or("");
        core.split('.')
            .map(|p| p.parse::<u64>().map_err(|_| anyhow!("版本号格式不对：{v}")))
            .collect()
    }
    let (mut a, mut b) = (parse(a)?, parse(b)?);
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    Ok(a.cmp(&b))
}

/// A file type a WebAssembly plugin can open (`formats` in plugin.json).
#[derive(Debug, Clone, PartialEq)]
pub struct FormatDecl {
    /// Lower-case extensions without the dot.
    pub ext: Vec<String>,
    pub name: String,
}

/// The fields of `plugin.json` the host relies on; `raw` keeps the rest.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub main: String,
    /// WebAssembly module (`docs/format-plugins.md`), relative to the package.
    pub wasm: Option<String>,
    pub formats: Vec<FormatDecl>,
    /// Extensions whose files `tmv_metadata` should see.
    pub metadata: Vec<String>,
    /// Project files of acquisition programs that `tmv_stack` turns into a
    /// Z stack of the slice images around them.
    pub stacks: Vec<StackDecl>,
    /// Older packages this one takes over (`replaces`), e.g. when several
    /// plugins of one brand merge. An installed package hides the ones it
    /// replaces; an official install from the market also removes them.
    pub replaces: Vec<String>,
    pub raw: Value,
}

/// `[a-z0-9]{1,16}`, lower-cased, without the dot.
fn parse_exts(v: Option<&Value>, what: &str) -> Result<Vec<String>> {
    let list = v
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("{what} 的 ext 必须是扩展名列表"))?;
    let mut out = vec![];
    for e in list {
        let e = e
            .as_str()
            .unwrap_or("")
            .trim_start_matches('.')
            .to_ascii_lowercase();
        if e.is_empty() || e.len() > 16 || !e.bytes().all(|b| b.is_ascii_alphanumeric()) {
            bail!("{what} 里的扩展名不对：{e}");
        }
        out.push(e);
    }
    if out.is_empty() {
        bail!("{what} 至少要有一个扩展名");
    }
    Ok(out)
}

/// `stacks` entry of plugin.json: file names (`*` matches any run of
/// characters, case is ignored) and a display name.
#[derive(Clone, Debug, PartialEq)]
pub struct StackDecl {
    pub files: Vec<String>,
    pub name: String,
}

impl StackDecl {
    pub fn matches(&self, file_name: &str) -> bool {
        self.files
            .iter()
            .any(|p| glob_match(&p.to_lowercase(), &file_name.to_lowercase()))
    }
}

/// `*` matches any run of characters; everything else matches itself.
fn glob_match(pat: &str, s: &str) -> bool {
    let mut parts = pat.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = s.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for m in middle {
        match rest.find(m) {
            Some(i) => rest = &rest[i + m.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

fn parse_stacks(v: Option<&Value>) -> Result<Vec<StackDecl>> {
    let Some(v) = v else { return Ok(vec![]) };
    let mut out = vec![];
    for d in v.as_array().ok_or_else(|| anyhow!("stacks 必须是列表"))? {
        let files = d
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("stacks 的 files 必须是文件名列表"))?;
        let mut names = vec![];
        for f in files {
            let f = f.as_str().unwrap_or("");
            if f.is_empty()
                || f.len() > 64
                || f.contains(['/', '\\'])
                || f.chars().all(|c| c == '*' || c == '.')
            {
                bail!("stacks 里的文件名不对：{f}");
            }
            names.push(f.to_string());
        }
        if names.is_empty() {
            bail!("stacks 至少要有一个文件名");
        }
        let name = d
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        out.push(StackDecl { files: names, name });
    }
    Ok(out)
}

pub fn parse_manifest(bytes: &[u8], app_version: &str) -> Result<Manifest> {
    let raw: Value = serde_json::from_slice(bytes).context("plugin.json 不是有效的 JSON")?;
    let s = |k: &str| raw.get(k).and_then(Value::as_str);
    let id = s("id")
        .ok_or_else(|| anyhow!("plugin.json 缺少 id"))?
        .to_string();
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
    if api == 0 || api > API_VERSION {
        bail!("插件需要更新的 TifMrcView（插件接口版本 {api}，当前 {API_VERSION}）");
    }
    if api == 2 {
        let permissions = raw.get("permissions").and_then(Value::as_array);
        if permissions.is_none_or(|ps| {
            ps.iter()
                .any(|p| !p.as_str().is_some_and(|p| SANDBOX_PERMISSIONS.contains(&p)))
        }) {
            bail!("API 2 的 permissions 必须是已知权限名称的列表");
        }
        if raw.get("native").is_some() {
            bail!("API 2 插件不能声明 native 字段");
        }
    }
    if let Some(min) = s("minAppVersion") {
        if compare_versions(min, app_version)? == Ordering::Greater {
            bail!("插件需要 TifMrcView {min} 或更新版本（当前 {app_version}）");
        }
    }
    let wasm = match raw.get("wasm") {
        None => None,
        Some(w) => {
            let w = w.as_str().unwrap_or("");
            if safe_rel(w).is_none() || !w.ends_with(".wasm") {
                bail!("wasm 不是插件里的 .wasm 文件：{w}");
            }
            Some(w.to_string())
        }
    };
    let mut formats = vec![];
    if let Some(f) = raw.get("formats") {
        for d in f.as_array().ok_or_else(|| anyhow!("formats 必须是列表"))? {
            let ext = parse_exts(d.get("ext"), "formats")?;
            // The core's own formats cannot be taken over.
            if let Some(core) = ext
                .iter()
                .find(|e| crate::formats::CORE_EXTENSIONS.contains(&e.as_str()))
            {
                bail!("formats 不能接管内置格式 .{core}");
            }
            let name = d
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            formats.push(FormatDecl { ext, name });
        }
    }
    let metadata = match raw.get("metadata") {
        None => vec![],
        Some(m) => parse_exts(m.get("ext"), "metadata")?,
    };
    let stacks = parse_stacks(raw.get("stacks"))?;
    if wasm.is_none() && (!formats.is_empty() || !metadata.is_empty() || !stacks.is_empty()) {
        bail!("声明了 formats、metadata 或 stacks 的插件必须有 wasm 模块");
    }
    let mut replaces = vec![];
    if let Some(r) = raw.get("replaces") {
        for v in r
            .as_array()
            .ok_or_else(|| anyhow!("replaces 必须是插件 id 列表"))?
        {
            let old = v
                .as_str()
                .filter(|o| valid_id(o) && *o != id && !BUILTIN.contains(o));
            let old = old.ok_or_else(|| anyhow!("replaces 里的插件 id 不对：{v}"))?;
            if !replaces.iter().any(|r| r == old) {
                replaces.push(old.to_string());
            }
        }
    }
    let name = s("name").unwrap_or_default().to_string();
    Ok(Manifest {
        id,
        name,
        main,
        wasm,
        formats,
        metadata,
        stacks,
        replaces,
        raw,
    })
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
    if let Some(w) = &m.wasm {
        if !pkg.join(safe_rel(w).unwrap()).is_file() {
            bail!("插件里没有 {w}");
        }
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
    /// Name of the installed package that replaces this one (see
    /// [`Manifest::replaces`]); such a package never runs.
    pub replaced_by: Option<String>,
}

/// Lists the package folders in `dir`; broken ones carry `error`.
pub fn scan(dir: &Path, app_version: &str) -> Vec<Installed> {
    let Ok(rd) = fs::read_dir(dir) else {
        return vec![];
    };
    let mut replacers: Vec<(String, Vec<String>)> = vec![];
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
                Ok(m) => {
                    if !m.replaces.is_empty() {
                        replacers.push((m.name.clone(), m.replaces.clone()));
                    }
                    Installed {
                        id: m.id,
                        dir,
                        main: Some(m.main),
                        manifest: Some(m.raw),
                        error: None,
                        replaced_by: None,
                    }
                }
                Err(err) => {
                    let error = Some(format!("{err:#}"));
                    Installed {
                        id: name,
                        dir,
                        main: None,
                        manifest: None,
                        error,
                        replaced_by: None,
                    }
                }
            })
        })
        .collect();
    for p in &mut out {
        p.replaced_by = replacers
            .iter()
            .find(|(_, ids)| ids.contains(&p.id))
            .map(|(name, _)| name.clone());
    }
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
    if rel
        .split('/')
        .any(|c| c.starts_with('.') || c == "__MACOSX")
    {
        return Ok(None);
    }
    safe_rel(rel)
        .map(Some)
        .ok_or_else(|| anyhow!("压缩包里有不安全的路径：{name}"))
}

/// The folder every entry sits in when the zip wraps the plugin in one, else "".
fn wrapper(names: &[String]) -> String {
    if names.iter().any(|n| n == MANIFEST) {
        return String::new();
    }
    let real: Vec<&String> = names
        .iter()
        .filter(|n| !n.starts_with("__MACOSX/"))
        .collect();
    let Some(first) = real.first() else {
        return String::new();
    };
    let Some((top, _)) = first.split_once('/') else {
        return String::new();
    };
    let prefix = format!("{top}/");
    if real.iter().all(|n| n.starts_with(&prefix))
        && names.iter().any(|n| *n == format!("{prefix}{MANIFEST}"))
    {
        prefix
    } else {
        String::new()
    }
}

pub(crate) fn unique_tmp(dir: &Path, tag: &str, id: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    dir.join(format!(".{tag}-{id}-{}-{nanos}", std::process::id()))
}

/// Installs (or replaces) the package at `zip_path` into `dir/<id>/`.
pub fn install(dir: &Path, zip_path: &Path, app_version: &str) -> Result<Manifest> {
    install_checked(dir, zip_path, app_version, |_, _| Ok(()))
}

/// Like [`install`]; `check` looks at the unpacked files (in a temporary
/// folder) before they replace an installed version.
pub fn install_checked(
    dir: &Path,
    zip_path: &Path,
    app_version: &str,
    check: impl FnOnce(&Path, &Manifest) -> Result<()>,
) -> Result<Manifest> {
    let bytes = read_archive(zip_path)?;
    install_data_checked(dir, &bytes, app_version, check)
}

/// Read one bounded snapshot: inspection, hashing and extraction must see the
/// same bytes, even if the user replaces the selected zip meanwhile.
pub fn read_archive(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("无法打开 {}", path.display()))?;
    let mut bytes = vec![];
    file.take(MAX_PACKAGE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PACKAGE {
        bail!("插件包太大（超过 {} MB）", MAX_PACKAGE >> 20);
    }
    Ok(bytes)
}

/// Reads a manifest without extracting or installing anything.
pub fn inspect(bytes: &[u8], app_version: &str) -> Result<Manifest> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("不是有效的 zip 插件包")?;
    archive_manifest(&mut zip, app_version).map(|(m, _)| m)
}

fn archive_manifest(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    app_version: &str,
) -> Result<(Manifest, String)> {
    if zip.len() > MAX_ENTRIES {
        bail!(
            "插件包里的文件太多（{} 个，最多 {MAX_ENTRIES} 个）",
            zip.len()
        );
    }
    let names: Vec<String> = zip.file_names().map(str::to_string).collect();
    let strip = wrapper(&names);
    let mut f = zip
        .by_name(&format!("{strip}{MANIFEST}"))
        .map_err(|_| anyhow!("插件包里没有 plugin.json"))?;
    let mut buf = vec![];
    (&mut f).take(MAX_MANIFEST).read_to_end(&mut buf)?;
    let manifest = parse_manifest(&buf, app_version)?;
    if BUILTIN.contains(&manifest.id.as_str()) {
        bail!("{} 是内置插件，不能另外安装", manifest.id);
    }
    Ok((manifest, strip))
}

pub fn install_data_checked(
    dir: &Path,
    bytes: &[u8],
    app_version: &str,
    check: impl FnOnce(&Path, &Manifest) -> Result<()>,
) -> Result<Manifest> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("不是有效的 zip 插件包")?;
    let (manifest, strip) = archive_manifest(&mut zip, app_version)?;

    fs::create_dir_all(dir).context("无法创建插件文件夹")?;
    let tmp = unique_tmp(dir, "install", &manifest.id);
    let res = extract(&mut zip, &strip, &tmp).and_then(|_| {
        if !tmp.join(safe_rel(&manifest.main).unwrap()).is_file() {
            bail!("插件包里没有 {}", manifest.main);
        }
        if let Some(w) = &manifest.wasm {
            if !tmp.join(safe_rel(w).unwrap()).is_file() {
                bail!("插件包里没有 {w}");
            }
        }
        check(&tmp, &manifest)?;
        replace(dir, &manifest.id, &tmp)
    });
    if res.is_err() {
        let _ = fs::remove_dir_all(&tmp);
    }
    res.map(|_| manifest)
}

fn extract(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, strip: &str, tmp: &Path) -> Result<()> {
    fs::create_dir_all(tmp)?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let name = f.name().to_string();
        if f.is_symlink() {
            bail!("插件包里不能有符号链接：{name}");
        }
        let Some(rel) = entry_target(&name, strip)? else {
            continue;
        };
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

/// Removes the installed packages `m` replaces (see [`Manifest::replaces`])
/// and returns their ids. A package that cannot be removed stays, hidden by
/// the registry, and is only logged: the new one is already installed.
pub fn retire_replaced(dir: &Path, m: &Manifest) -> Vec<String> {
    let mut gone = vec![];
    for old in &m.replaces {
        if !dir.join(old).is_dir() {
            continue;
        }
        match uninstall(dir, old) {
            Ok(()) => gone.push(old.clone()),
            Err(e) => eprintln!(
                "plugin {}: could not remove the replaced plugin {old}: {e:#}",
                m.id
            ),
        }
    }
    gone
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
        return file
            .is_file()
            .then_some((file, "text/javascript; charset=utf-8"));
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
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
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
        assert_eq!(
            compare_versions("0.10.0", "0.9.9").unwrap(),
            Ordering::Greater
        );
        assert_eq!(compare_versions("0.2", "0.2.0").unwrap(), Ordering::Equal);
        assert_eq!(
            compare_versions("1.0.0-beta", "1.0.0").unwrap(),
            Ordering::Equal
        );
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
        assert!(bad(
            r#"{"id":"demo","name":"x","version":"1.0.0","main":"../x.js"}"#
        ));
        assert!(bad(
            r#"{"id":"demo","name":"x","version":"1.0.0","main":"index.html"}"#
        ));
        assert!(!bad(
            r#"{"id":"demo","name":"x","version":"1.0.0","apiVersion":2,"permissions":[]}"#
        ));
        assert!(bad(
            r#"{"id":"demo","name":"x","version":"1.0.0","apiVersion":3}"#
        ));
        assert!(bad(
            r#"{"id":"demo","name":"x","version":"1.0.0","minAppVersion":"0.10.0"}"#
        ));
        assert!(!bad(
            r#"{"id":"demo-2","name":"x","version":"1.0.0","minAppVersion":"0.1.9"}"#
        ));
    }

    #[test]
    fn api2_permissions_and_native() {
        let mut m: Value = serde_json::from_str(OK).unwrap();
        m["apiVersion"] = 2.into();
        let parse = |m: &Value| parse_manifest(&serde_json::to_vec(m).unwrap(), "0.5.0");
        assert!(parse(&m).is_err(), "permissions required");
        for bad in [
            Value::Null,
            serde_json::json!({}),
            serde_json::json!("state"),
            serde_json::json!(["state", 1]),
            serde_json::json!(["native"]),
        ] {
            m["permissions"] = bad;
            assert!(parse(&m).is_err());
        }
        m["permissions"] = serde_json::json!([]);
        assert!(parse(&m).is_ok());
        m["permissions"] = serde_json::json!(SANDBOX_PERMISSIONS);
        assert!(parse(&m).is_ok());
        for native in [Value::Null, false.into(), true.into(), "recorder".into()] {
            m["native"] = native;
            assert!(
                parse(&m).is_err(),
                "native is forbidden regardless of value"
            );
        }
        m.as_object_mut().unwrap().remove("native");
        m["apiVersion"] = 0.into();
        assert!(parse(&m).is_err());
        m.as_object_mut().unwrap().remove("apiVersion");
        m.as_object_mut().unwrap().remove("permissions");
        assert!(parse(&m).is_ok(), "legacy installed manifests still parse");
    }

    #[test]
    fn manifest_replaces() {
        let parse = |extra: &str| {
            parse_manifest(
                format!(r#"{{"id":"zeiss","name":"Z","version":"1.0.0"{extra}}}"#).as_bytes(),
                "0.6.0",
            )
        };
        assert!(parse("").unwrap().replaces.is_empty());
        assert_eq!(
            parse(r#","replaces":["zen","smartsem","zen"]"#)
                .unwrap()
                .replaces,
            ["zen", "smartsem"]
        );
        for bad in [
            r#""zen""#,
            r#"["../x"]"#,
            r#"["zeiss"]"#,
            r#"["i18n"]"#,
            r#"[1]"#,
        ] {
            assert!(parse(&format!(r#","replaces":{bad}"#)).is_err(), "{bad}");
        }
    }

    #[test]
    fn manifest_wasm_formats_and_metadata() {
        let base = r#""id":"fmt","name":"F","version":"1.0.0""#;
        let parse = |extra: &str| parse_manifest(format!("{{{base},{extra}}}").as_bytes(), "0.4.0");
        let m = parse(r#""wasm":"f.wasm","formats":[{"ext":["ABC",".xyz"],"name":"ABC"}],"metadata":{"ext":["tif"]}"#).unwrap();
        assert_eq!(m.wasm.as_deref(), Some("f.wasm"));
        assert_eq!(
            m.formats,
            [FormatDecl {
                ext: vec!["abc".into(), "xyz".into()],
                name: "ABC".into()
            }]
        );
        assert_eq!(m.metadata, ["tif"]);
        assert!(parse(r#""wasm":"../f.wasm""#).is_err());
        assert!(parse(r#""wasm":"f.js""#).is_err());
        assert!(
            parse(r#""formats":[{"ext":["abc"]}]"#).is_err(),
            "formats without wasm"
        );
        assert!(
            parse(r#""wasm":"f.wasm","formats":[{"ext":["tif"]}]"#).is_err(),
            "core format"
        );
        assert!(parse(r#""wasm":"f.wasm","formats":[{"ext":["a/b"]}]"#).is_err());
        assert!(parse(r#""wasm":"f.wasm","formats":[{"ext":[]}]"#).is_err());
        assert!(parse(r#""wasm":"f.wasm","metadata":{"ext":"tif"}"#).is_err());
        assert!(parse(r#""wasm":"f.wasm","metadata":{"ext":["tif"]}"#).is_ok());
    }

    #[test]
    fn manifest_stacks() {
        let base = r#""id":"vem","name":"V","version":"1.0.0""#;
        let parse = |extra: &str| parse_manifest(format!("{{{base},{extra}}}").as_bytes(), "0.7.0");
        let m = parse(
            r#""wasm":"v.wasm","stacks":[{"files":["ProjectData.dat","*.a3d"],"name":"ASV"}]"#,
        )
        .unwrap();
        let d = &m.stacks[0];
        assert_eq!(d.name, "ASV");
        assert!(
            d.matches("projectdata.DAT")
                && d.matches("run.A3D")
                && !d.matches("ProjectData.dat.bak")
                && !d.matches("x.a3dx")
        );
        assert!(
            parse(r#""stacks":[{"files":["a.dat"]}]"#).is_err(),
            "stacks without wasm"
        );
        for bad in [
            r#"[{"files":[]}]"#,
            r#"[{"files":["a/b.dat"]}]"#,
            r#"[{"files":["*"]}]"#,
            r#"{"files":["a"]}"#,
        ] {
            assert!(
                parse(&format!(r#""wasm":"v.wasm","stacks":{bad}"#)).is_err(),
                "{bad}"
            );
        }
        assert!(
            glob_match("a*b*c", "aXbYc")
                && glob_match("a*", "a")
                && !glob_match("a*b", "ab_")
                && glob_match("*.tif", ".tif")
        );
    }

    #[test]
    fn install_checks_wasm_and_runs_the_check() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins");
        let z = tmp.path().join("f.zip");
        let m = r#"{"id":"fmt","name":"F","version":"1.0.0","wasm":"f.wasm","formats":[{"ext":["abc"]}]}"#;
        make_zip(&z, &[("plugin.json", m), ("index.js", "1")]);
        assert!(format!("{:#}", install(&dir, &z, "0.4.0").unwrap_err()).contains("f.wasm"));
        make_zip(
            &z,
            &[("plugin.json", m), ("index.js", "1"), ("f.wasm", "x")],
        );
        let err = install_checked(&dir, &z, "0.4.0", |tmp, m| {
            assert!(tmp.join("f.wasm").is_file());
            bail!("bad module {}", m.id)
        });
        assert!(format!("{:#}", err.unwrap_err()).contains("bad module fmt"));
        assert!(!dir.join("fmt").exists());
        install(&dir, &z, "0.4.0").unwrap();
        assert!(read_installed(&dir.join("fmt"), "0.4.0").is_ok());
        fs::remove_file(dir.join("fmt/f.wasm")).unwrap();
        assert!(read_installed(&dir.join("fmt"), "0.4.0").is_err());
    }

    #[test]
    fn rejects_unsafe_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins");
        for evil in [
            "../evil.js",
            "a/../../evil.js",
            "/abs.js",
            "a\\b.js",
            "C:/x.js",
        ] {
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

        make_zip(
            &z,
            &[
                ("plugin.json", &OK.replace("demo", "i18n")),
                ("index.js", "1"),
            ],
        );
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
        make_zip(
            &z,
            &[("plugin.json", OK), ("index.js", "1"), ("blob.js", &big)],
        );
        assert!(format!("{:#}", install(&dir, &z, "0.2.0").unwrap_err()).contains("太大"));
        assert!(!dir.join("demo").exists());
    }

    #[test]
    fn install_replace_uninstall() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins");
        let z = tmp.path().join("demo.zip");
        make_zip(
            &z,
            &[
                ("plugin.json", OK),
                ("index.js", "v1"),
                ("lib/", ""),
                ("lib/a.js", "a"),
                (".DS_Store", ""),
            ],
        );
        let m = install(&dir, &z, "0.2.0").unwrap();
        assert_eq!(m.raw["version"], "1.2.0");
        assert_eq!(fs::read_to_string(dir.join("demo/index.js")).unwrap(), "v1");
        assert!(dir.join("demo/lib/a.js").is_file() && !dir.join("demo/.DS_Store").exists());
        assert!(is_installed(&dir, "demo"));

        // Upgrade, wrapped in a folder (as a re-zipped download would be).
        make_zip(
            &z,
            &[
                ("demo-1.3/plugin.json", &OK.replace("1.2.0", "1.3.0")),
                ("demo-1.3/index.js", "v2"),
            ],
        );
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
        let mut left: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
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

        assert_eq!(
            resolve(&dir, "demo/index.js").unwrap().1,
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            resolve(&dir, "demo/lib/a.css").unwrap().1,
            "text/css; charset=utf-8"
        );
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
            std::os::unix::fs::symlink(tmp.path().join("secret.js"), dir.join("demo/out.js"))
                .unwrap();
            assert!(resolve(&dir, "demo/out.js").is_none());
        }
    }
}
