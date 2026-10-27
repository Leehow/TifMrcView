//! One-click installs from the official plugin market.
//!
//! The market is a static folder next to the updater feed (see
//! [`super::market_url`]): `plugins.json` (written by
//! `scripts/pack-plugins.mjs`) lists every package with its file name, size
//! and SHA-256. Installing by id downloads that file over HTTPS, checks size
//! and hash against the index, and then installs it exactly like a zip picked
//! with 加载插件… ([`package::install_checked`] + [`registry::validate_package`]).
//!
//! The website starts an install with a `tifmrcview://install-plugin?id=<id>`
//! link ([`parse_install_link`]). The link only names an id: what gets
//! installed always comes from the official index, and the UI asks first.

use std::cmp::Ordering;
use std::future::Future;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::package::{self, Manifest};

/// Largest package the market may serve (bytes).
pub use super::package::MAX_PACKAGE;
/// Largest `plugins.json` (bytes).
pub const MAX_INDEX: u64 = 4 << 20;
pub const SCHEME: &str = "tifmrcview";

/// Legacy packages shipped in plugins/ at the API 2 transition. A name alone
/// grants nothing: the exact version and archive hash must also match the
/// backend's official HTTPS index. Remove this exception as they migrate.
pub const OFFICIAL_API1_IDS: &[&str] = &["fluoview", "imaris", "lasx", "nis", "recorder"];
pub const LEGACY_REJECTED: &str = "不再接受第三方 API 1 插件，请联系开发者升级到 API 2；官方旧版插件须与已读取的插件市场索引完全匹配，请先刷新插件市场";

/// What a `tifmrcview://install-plugin?id=…&version=…` link asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallLink {
    pub id: String,
    /// The version the page showed; informational only.
    pub version: Option<String>,
}

/// Parses an install link. Anything that is not exactly
/// `tifmrcview://install-plugin?id=<valid id>[&version=<x.y.z>]` is `None`.
pub fn parse_install_link(url: &str) -> Option<InstallLink> {
    if url.len() > 512 {
        return None;
    }
    let u = tauri::Url::parse(url).ok()?;
    if u.scheme() != SCHEME || !u.username().is_empty() || u.password().is_some() || u.port().is_some() {
        return None;
    }
    // `tifmrcview://install-plugin?…` (host form) or `tifmrcview:install-plugin?…`.
    let target = match u.host_str() {
        Some(h) if u.path().is_empty() || u.path() == "/" => h.to_string(),
        Some(_) => return None,
        None => u.path().trim_matches('/').to_string(),
    };
    if !target.eq_ignore_ascii_case("install-plugin") {
        return None;
    }
    let (mut id, mut version) = (None, None);
    for (k, v) in u.query_pairs() {
        let slot = match k.as_ref() {
            "id" => &mut id,
            "version" => &mut version,
            _ => continue,
        };
        if slot.replace(v.into_owned()).is_some() {
            return None; // repeated key
        }
    }
    let id = id.filter(|i| package::valid_id(i))?;
    let version = match version {
        None => None,
        Some(v) if v.len() <= 32 && package::compare_versions(&v, "0").is_ok() => Some(v),
        Some(_) => return None,
    };
    Some(InstallLink { id, version })
}

/// One package in `plugins.json`, the fields an install needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub api_version: u64,
    pub permissions: Option<Value>,
    /// Plugins this one takes over (`replaces`).
    pub replaces: Vec<String>,
}

/// The `replaces` ids of an index entry or manifest, ignoring junk.
fn replaces(p: &Value) -> Vec<String> {
    let ids = p.get("replaces").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
    ids.filter(|i| package::valid_id(i)).map(str::to_string).collect()
}

/// The package list of a parsed `plugins.json`.
pub fn entries(index: &Value) -> Result<&Vec<Value>> {
    index.get("plugins").and_then(Value::as_array).ok_or_else(|| anyhow!("插件市场的插件列表格式不对"))
}

pub fn parse_index(bytes: &[u8]) -> Result<Value> {
    let v: Value = serde_json::from_slice(bytes).context("插件市场的插件列表格式不对")?;
    entries(&v)?;
    Ok(v)
}

/// A plain file name in the market folder, e.g. `zen-2.0.0.zip`.
fn valid_file(f: &str) -> bool {
    f.len() <= 128
        && f.ends_with(".zip")
        && !f.starts_with('.')
        && f.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// Finds `id` in the index and checks it can be installed into this app.
pub fn find_entry(index: &Value, id: &str, app_version: &str) -> Result<Entry> {
    let p = entries(index)?
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| anyhow!("插件市场上没有插件 {id}"))?;
    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let name = Some(s("name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string());
    if p.get("builtin").and_then(Value::as_bool) == Some(true) || package::BUILTIN.contains(&id) {
        bail!("{id} 是内置插件，不能另外安装");
    }
    let (version, file, sha256) = (s("version"), s("file"), s("sha256").to_ascii_lowercase());
    let size = p.get("size").and_then(Value::as_u64).unwrap_or(0);
    let hex = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid_file(&file) || !hex || size == 0 || package::compare_versions(&version, "0").is_err() {
        bail!("插件市场上 {id} 的信息不完整");
    }
    if size > MAX_PACKAGE {
        bail!("插件包太大（超过 {} MB）", MAX_PACKAGE >> 20);
    }
    if let Some(min) = p.get("minAppVersion").and_then(Value::as_str) {
        if package::compare_versions(min, app_version)? == Ordering::Greater {
            bail!("「{name}」需要 TifMrcView {min} 或更新版本（当前 {app_version}），请先在下面「检查更新」");
        }
    }
    let api_version = p.get("apiVersion").map_or(Some(1), Value::as_u64).ok_or_else(|| anyhow!("apiVersion 必须是整数"))?;
    let permissions = p.get("permissions").cloned();
    Ok(Entry { id: id.to_string(), name, version, file, size, sha256, api_version, permissions, replaces: replaces(p) })
}

/// Checks a download against its index entry.
pub fn verify(entry: &Entry, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 != entry.size {
        bail!("下载的插件包大小不对（可能没下载完整），请重试");
    }
    let hash = sha256(bytes);
    if hash != entry.sha256 {
        bail!("下载的插件包校验失败（SHA-256 不一致），已取消安装");
    }
    Ok(())
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Install policy only; never used by scanning/loading already installed code.
pub fn check_install_policy(m: &Manifest, bytes: &[u8], entry: Option<&Entry>) -> Result<()> {
    if m.raw.get("apiVersion").and_then(Value::as_u64).unwrap_or(1) == 2 {
        return Ok(());
    }
    let official = entry.is_some_and(|e| {
        OFFICIAL_API1_IDS.contains(&m.id.as_str()) && m.id == e.id
            && m.raw["version"].as_str() == Some(e.version.as_str()) && verify(e, bytes).is_ok()
    });
    if !official {
        bail!("{LEGACY_REJECTED}");
    }
    Ok(())
}

/// Bind the permission confirmation to the manifest that is actually installed.
pub fn matches_confirmation(a: &Value, b: &Value) -> bool {
    ["id", "version", "name", "permissions"].iter().all(|k| a.get(k) == b.get(k))
        && a.get("apiVersion").and_then(Value::as_u64).unwrap_or(1)
            == b.get("apiVersion").and_then(Value::as_u64).unwrap_or(1)
}

/// Downloads `<base>plugins.json` and `<base><file>` with `fetch(url, max_bytes)`
/// and returns the checked package.
pub async fn download<F, Fut>(fetch: F, base: &str, id: &str, app_version: &str) -> Result<(Value, Entry, Vec<u8>)>
where
    F: Fn(String, u64) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let index = parse_index(&fetch(format!("{base}plugins.json"), MAX_INDEX).await?)?;
    let entry = find_entry(&index, id, app_version)?;
    let bytes = fetch(format!("{base}{}", entry.file), entry.size).await?;
    verify(&entry, &bytes)?;
    Ok((index, entry, bytes))
}

/// Try each host only for transport/HTTP failures. An index and its package
/// always come from one host; validation failures stop immediately.
pub async fn download_from_bases<F, Fut>(fetch: F, bases: &[String], id: &str, app_version: &str) -> Result<(Value, Entry, Vec<u8>)>
where
    F: Fn(String, u64) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let mut error = None;
    for base in bases {
        match download(&fetch, base, id, app_version).await {
            Err(e) if e.is::<NetError>() => error = Some(e),
            result => return result,
        }
    }
    Err(error.unwrap_or_else(|| anyhow!("连不上插件市场，请检查网络后再试")))
}

/// Listing uses the same ordered hosts, without retrying a malformed index.
pub async fn fetch_index<F, Fut>(fetch: F, bases: &[String]) -> Result<Value>
where
    F: Fn(String, u64) -> Fut,
    Fut: Future<Output = Result<Vec<u8>>>,
{
    let mut error = None;
    for base in bases {
        match fetch(format!("{base}plugins.json"), MAX_INDEX).await {
            Err(e) if e.is::<NetError>() => error = Some(e),
            result => return parse_index(&result?),
        }
    }
    Err(error.unwrap_or_else(|| anyhow!("连不上插件市场，请检查网络后再试")))
}

/// Installs a downloaded, verified package like 加载插件… does.
pub fn install_bytes(dir: &Path, entry: &Entry, bytes: &[u8], app_version: &str) -> Result<Manifest> {
    verify(entry, bytes)?;
    package::install_data_checked(dir, bytes, app_version, |unpacked, m| {
        if m.id != entry.id || m.name != entry.name || m.raw["version"].as_str() != Some(entry.version.as_str())
            || m.raw.get("apiVersion").and_then(Value::as_u64).unwrap_or(1) != entry.api_version
            || m.raw.get("permissions") != entry.permissions.as_ref() || m.replaces != entry.replaces {
            bail!("插件包和插件市场的信息不一致");
        }
        check_install_policy(m, bytes, Some(entry))?;
        super::registry::validate_package(unpacked, m)
    })
    .inspect(|m| {
        package::retire_replaced(dir, m);
    })
}

/// Whether `bytes` are exactly the official market's package of `m`, the
/// condition for a hand-picked zip to remove the plugins it replaces.
pub fn is_official(index: Option<&Value>, m: &Manifest, bytes: &[u8], app_version: &str) -> bool {
    let Some(e) = index.and_then(|i| find_entry(i, &m.id, app_version).ok()) else { return false };
    m.raw["version"].as_str() == Some(e.version.as_str()) && m.replaces == e.replaces && verify(&e, bytes).is_ok()
}

/// What the plugin dialog shows for one market package.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    /// The `plugins.json` entry as published.
    pub entry: Value,
    /// Version installed now, if any.
    pub installed: Option<String>,
    /// "install", "update", "installed", "builtin", "needsApp" or "replace"
    /// (not installed, but takes over the installed plugins in `replaces`).
    pub state: &'static str,
    /// Installed plugins this package replaces.
    pub replaces: Vec<String>,
}

/// Pairs the market list with what is installed in `dir`.
pub fn list(index: &Value, dir: Option<&Path>, app_version: &str) -> Vec<Listed> {
    let installed = dir.map(|d| package::scan(d, app_version)).unwrap_or_default();
    let Ok(list) = entries(index) else { return vec![] };
    list.iter()
        .filter_map(|p| {
            let id = p.get("id").and_then(Value::as_str).filter(|i| package::valid_id(i))?;
            let version = p.get("version").and_then(Value::as_str).unwrap_or("0");
            let have = installed
                .iter()
                .find(|i| i.id == id)
                .and_then(|i| i.manifest.as_ref())
                .and_then(|m| m["version"].as_str())
                .map(str::to_string);
            let builtin = p.get("builtin").and_then(Value::as_bool) == Some(true);
            let too_old = p
                .get("minAppVersion")
                .and_then(Value::as_str)
                .is_some_and(|m| !matches!(package::compare_versions(m, app_version), Ok(Ordering::Less | Ordering::Equal)));
            let newer = |h: &str| package::compare_versions(version, h).is_ok_and(|o| o == Ordering::Greater);
            let mut replacing = replaces(p);
            replacing.retain(|old| old != id && installed.iter().any(|i| &i.id == old));
            let state = match &have {
                _ if builtin => "builtin",
                Some(h) if !newer(h) => "installed",
                _ if too_old => "needsApp",
                Some(_) => "update",
                None if !replacing.is_empty() => "replace",
                None => "install",
            };
            Some(Listed { entry: p.clone(), installed: have, state, replaces: replacing })
        })
        .collect()
}

/// Fetches `url` with at most `max` bytes. `https_only` is off only in tests.
pub async fn http_get(url: String, max: u64, https_only: bool, user_agent: &str) -> Result<Vec<u8>> {
    if https_only && !url.starts_with("https://") {
        bail!("只能从 HTTPS 地址下载插件");
    }
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    // reqwest's hyper-util connector already races IPv4/IPv6 (Happy
    // Eyeballs, 300 ms fallback), including on Windows. Keep that default.
    let direct = tauri::Url::parse(&url).is_ok_and(|u| u.port() == Some(9443));
    let client = reqwest::Client::builder()
        .https_only(https_only)
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(if direct { 5 } else { 20 }))
        .read_timeout(Duration::from_secs(if direct { 15 } else { 60 }))
        // The loop below owns retries, including response-body timeouts.
        .retry(reqwest::retry::never())
        .build()
        .map_err(offline)?;
    // One direct attempt lets a blocked port fall back after ~5 seconds.
    // Keep #25's longer budget and retries for the Cloudflare route.
    if direct {
        return http_get_once(&client, &url, max, Duration::from_secs(300)).await;
    }
    http_get_retry(&client, &url, max, Duration::from_secs(300), [Duration::from_secs(1), Duration::from_secs(3)]).await
}

/// A network failure shown to the user as just `message` (the interface
/// translates these exact strings), remembering whether a retry may help.
#[derive(Debug)]
struct NetError {
    message: String,
    retry: bool,
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NetError {}

fn offline(e: reqwest::Error) -> anyhow::Error {
    let message = if e.is_timeout() { "连接插件市场超时，请检查网络后再试" } else { "连不上插件市场，请检查网络后再试" };
    anyhow::Error::new(NetError { message: message.into(), retry: e.is_connect() || e.is_timeout() })
}

fn retryable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<NetError>().is_some_and(|e| e.retry)
}

/// Three attempts at most; the budget includes downloads and backoff. The
/// remaining budget becomes each request's total timeout (headers and body).
async fn http_get_retry(client: &reqwest::Client, url: &str, max: u64, budget: Duration, backoff: [Duration; 2]) -> Result<Vec<u8>> {
    let started = Instant::now();
    let mut delays = backoff.into_iter();
    loop {
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(anyhow::Error::new(NetError { message: "连接插件市场超时，请检查网络后再试".into(), retry: true }));
        }
        match http_get_once(client, url, max, remaining).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                if !retryable(&error) {
                    return Err(error);
                }
                let Some(delay) = delays.next() else { return Err(error) };
                if delay >= budget.saturating_sub(started.elapsed()) {
                    return Err(error);
                }
                // Tauri does not re-export an async timer. Sleep on its blocking
                // pool so backoff never blocks an async worker or the UI thread.
                tauri::async_runtime::spawn_blocking(move || std::thread::sleep(delay)).await?;
            }
        }
    }
}

async fn http_get_once(client: &reqwest::Client, url: &str, max: u64, timeout: Duration) -> Result<Vec<u8>> {
    let mut resp = client.get(url).timeout(timeout).send().await.map_err(offline)?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        let name = url.rsplit('/').next().unwrap_or_default();
        return Err(anyhow::Error::new(NetError { message: format!("插件市场上找不到 {name}（404）"), retry: false }));
    }
    if !status.is_success() {
        let retry = status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
        return Err(anyhow::Error::new(NetError { message: format!("插件市场返回错误 {}", status.as_u16()), retry }));
    }
    let too_big = || anyhow!("下载的文件太大（超过 {} MB）", max.div_ceil(1 << 20));
    if resp.content_length().is_some_and(|n| n > max) {
        return Err(too_big());
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(offline)? {
        if (out.len() + chunk.len()) as u64 > max {
            return Err(too_big());
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use tauri::async_runtime::block_on as rt;

    const PLUGIN: &str = r#"{"id":"demo","name":"Demo","version":"1.2.0","main":"index.js","minAppVersion":"0.5.0","apiVersion":2,"permissions":[]}"#;

    fn zip_bytes(manifest: &str) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("plugin.json", opts).unwrap();
        w.write_all(manifest.as_bytes()).unwrap();
        w.start_file("index.js", opts).unwrap();
        w.write_all(b"TifMrcView.register({id:'demo',activate(){}})").unwrap();
        w.finish().unwrap().into_inner()
    }

    fn sha(b: &[u8]) -> String {
        Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
    }

    fn index_for(zip: &[u8], min: &str) -> Value {
        serde_json::json!({"plugins": [
            {"id": "demo", "name": "Demo", "version": "1.2.0", "minAppVersion": min,
             "file": "demo-1.2.0.zip", "size": zip.len(), "sha256": sha(zip), "apiVersion": 2, "permissions": []},
            {"id": "i18n", "name": "多语言", "version": "1.0.0", "builtin": true},
        ]})
    }

    #[test]
    fn api1_requires_official_id_exact_version_and_archive() {
        let tmp = tempfile::tempdir().unwrap();
        for api in [None, Some(1)] {
            let mut manifest: Value = serde_json::from_str(PLUGIN).unwrap();
            manifest.as_object_mut().unwrap().remove("permissions");
            manifest.as_object_mut().unwrap().remove("apiVersion");
            if let Some(api) = api { manifest["apiVersion"] = api.into(); }
            for id in OFFICIAL_API1_IDS.iter().copied().chain(["third-party"]) {
                manifest["id"] = id.into();
                manifest["official"] = true.into(); // Self-asserted trust is ignored.
                let bytes = zip_bytes(&manifest.to_string());
                let m = package::inspect(&bytes, "0.5.0").unwrap();
                let mut index = index_for(&bytes, "0.5.0");
                index["plugins"][0]["id"] = id.into();
                index["plugins"][0].as_object_mut().unwrap().remove("apiVersion");
                index["plugins"][0].as_object_mut().unwrap().remove("permissions");
                let e = find_entry(&index, id, "0.5.0").unwrap();
                assert!(check_install_policy(&m, &bytes, None).is_err());
                assert!(super::super::inspect_installable(&bytes, None, "0.5.0").is_err());
                let official = OFFICIAL_API1_IDS.contains(&id);
                assert_eq!(super::super::inspect_installable(&bytes, Some(&index), "0.5.0").is_ok(), official);
                assert_eq!(install_bytes(tmp.path(), &e, &bytes, "0.5.0").is_ok(), official);
                assert_eq!(super::super::install_confirmed_zip(tmp.path(), &bytes, &sha256(&bytes), Some(&index), "0.5.0").is_ok(), official);
                if official {
                    let mut bad = e.clone();
                    bad.version = "1.2".into(); // Numerically equal is insufficient.
                    assert!(check_install_policy(&m, &bytes, Some(&bad)).is_err());
                    bad = e.clone(); bad.sha256 = "0".repeat(64);
                    assert!(check_install_policy(&m, &bytes, Some(&bad)).is_err());
                    let mut altered = bytes.clone(); *altered.last_mut().unwrap() ^= 1;
                    assert!(check_install_policy(&m, &altered, Some(&e)).is_err());
                }
            }
        }
        // A wasm declaration is not an exception for unrecognised API 1 code.
        let m = package::parse_manifest(br#"{"id":"other","name":"Other","version":"1.0.0","wasm":"f.wasm"}"#, "0.5.0").unwrap();
        assert!(check_install_policy(&m, b"anything", None).is_err());
        // The loading path is unchanged for an already installed third-party API 1 package.
        let archive = tmp.path().join("legacy.zip");
        std::fs::write(&archive, zip_bytes(&PLUGIN.replace(",\"apiVersion\":2,\"permissions\":[]", ""))).unwrap();
        package::install(tmp.path(), &archive, "0.5.0").unwrap();
        assert!(package::read_installed(&tmp.path().join("demo"), "0.5.0").is_ok());
    }

    #[test]
    fn confirmation_matches_actual_permissions_and_api() {
        let m: Value = serde_json::from_str(PLUGIN).unwrap();
        assert!(matches_confirmation(&m, &m));
        for (key, value) in [("id", "other".into()), ("version", "1.2".into()), ("name", "Changed".into()), ("apiVersion", 1.into()), ("permissions", serde_json::json!(["export"]))] {
            let mut changed = m.clone(); changed[key] = value;
            assert!(!matches_confirmation(&m, &changed));
        }
        let bytes = zip_bytes(PLUGIN);
        let mut e = find_entry(&index_for(&bytes, "0.5.0"), "demo", "0.5.0").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        e.permissions = Some(serde_json::json!(["export"]));
        assert!(install_bytes(tmp.path(), &e, &bytes, "0.5.0").is_err());
        assert!(!package::is_installed(tmp.path(), "demo"));
        e.permissions = Some(serde_json::json!([])); e.api_version = 1;
        assert!(install_bytes(tmp.path(), &e, &bytes, "0.5.0").is_err());
    }

    #[test]
    fn install_links() {
        let ok = |u: &str| parse_install_link(u);
        assert_eq!(ok("tifmrcview://install-plugin?id=zen"), Some(InstallLink { id: "zen".into(), version: None }));
        assert_eq!(
            ok("tifmrcview://install-plugin?id=smart-sem2&version=2.0.0"),
            Some(InstallLink { id: "smart-sem2".into(), version: Some("2.0.0".into()) })
        );
        assert_eq!(ok("TIFMRCVIEW://Install-Plugin/?id=zen&utm=x#frag").unwrap().id, "zen");
        assert_eq!(ok("tifmrcview:install-plugin?id=zen").unwrap().id, "zen");
        for bad in [
            "tifmrcview://install-plugin",
            "tifmrcview://install-plugin?id=",
            "tifmrcview://install-plugin?id=Zen",
            "tifmrcview://install-plugin?id=../zen",
            "tifmrcview://install-plugin?id=a%2Fb",
            "tifmrcview://install-plugin?id=zen&id=other",
            "tifmrcview://install-plugin?id=zen&version=latest",
            "tifmrcview://install-plugin?id=zen&version=1.0&version=2.0",
            "tifmrcview://install-plugin/extra?id=zen",
            "tifmrcview://uninstall-plugin?id=zen",
            "tifmrcview://open?file=/etc/passwd",
            "tifmrcview://user@install-plugin?id=zen",
            "tifmrcview://install-plugin:80?id=zen",
            "https://install-plugin?id=zen",
            "file:///tmp/a.tif",
            "/tmp/a.tif",
            "",
        ] {
            assert_eq!(parse_install_link(bad), None, "{bad}");
        }
        let long = format!("tifmrcview://install-plugin?id=zen&x={}", "a".repeat(600));
        assert_eq!(parse_install_link(&long), None);
    }

    #[test]
    fn index_lookup_and_app_version() {
        let zip = zip_bytes(PLUGIN);
        let index = index_for(&zip, "0.5.0");
        let e = find_entry(&index, "demo", "0.5.0").unwrap();
        assert_eq!((e.file.as_str(), e.size), ("demo-1.2.0.zip", zip.len() as u64));
        let err = format!("{:#}", find_entry(&index, "demo", "0.4.9").unwrap_err());
        assert!(err.contains("需要 TifMrcView 0.5.0") && err.contains("检查更新"), "{err}");
        assert!(format!("{:#}", find_entry(&index, "nope", "0.5.0").unwrap_err()).contains("没有插件 nope"));
        assert!(format!("{:#}", find_entry(&index, "i18n", "0.5.0").unwrap_err()).contains("内置"));

        let tweak = |k: &str, v: Value| {
            let mut i = index.clone();
            i["plugins"][0][k] = v;
            find_entry(&i, "demo", "0.5.0").is_err()
        };
        assert!(tweak("file", "../evil.zip".into()));
        assert!(tweak("file", "https://evil/x.zip".into()));
        assert!(tweak("file", "demo.exe".into()));
        assert!(tweak("sha256", "abc".into()));
        assert!(tweak("size", 0.into()));
        assert!(tweak("size", (MAX_PACKAGE + 1).into()));
        assert!(tweak("version", "x".into()));
        assert!(parse_index(b"[]").is_err() && parse_index(b"nope").is_err());
    }

    #[test]
    fn rejects_hash_and_size_mismatch() {
        let zip = zip_bytes(PLUGIN);
        let e = find_entry(&index_for(&zip, "0.1.0"), "demo", "0.5.0").unwrap();
        verify(&e, &zip).unwrap();
        let mut bad = zip.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(format!("{:#}", verify(&e, &bad).unwrap_err()).contains("SHA-256"));
        assert!(format!("{:#}", verify(&e, &zip[1..]).unwrap_err()).contains("大小"));
    }

    #[test]
    fn listing_states() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = zip_bytes(PLUGIN);
        let mut index = index_for(&zip, "0.5.0");
        index["plugins"].as_array_mut().unwrap().push(serde_json::json!(
            {"id": "future", "name": "F", "version": "1.0.0", "minAppVersion": "9.0.0", "file": "f.zip", "size": 1, "sha256": "0"}
        ));
        let states = |index: &Value| list(index, Some(tmp.path()), "0.5.0").iter().map(|l| l.state).collect::<Vec<_>>();
        assert_eq!(states(&index), ["install", "builtin", "needsApp"]);
        let e = find_entry(&index, "demo", "0.5.0").unwrap();
        install_bytes(tmp.path(), &e, &zip, "0.5.0").unwrap();
        assert_eq!(states(&index), ["installed", "builtin", "needsApp"]);
        index["plugins"][0]["version"] = "1.10.0".into();
        assert_eq!(states(&index), ["update", "builtin", "needsApp"]);
        assert_eq!(list(&index, Some(tmp.path()), "0.5.0")[0].installed.as_deref(), Some("1.2.0"));
    }

    #[test]
    fn merged_plugin_replaces_the_installed_old_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let old = |id: &str| {
            let archive = tmp.path().join(format!("{id}.zip"));
            std::fs::write(&archive, zip_bytes(&PLUGIN.replace("\"demo\"", &format!("\"{id}\"")))).unwrap();
            package::install(tmp.path(), &archive, "0.5.0").unwrap();
            std::fs::remove_file(archive).unwrap();
        };
        old("old-a");
        old("old-b");
        let merged = PLUGIN.replace(r#""permissions":[]"#, r#""permissions":[],"replaces":["old-a","old-b","old-c"]"#);
        let zip = zip_bytes(&merged);
        let mut index = index_for(&zip, "0.5.0");
        index["plugins"][0]["replaces"] = serde_json::json!(["old-a", "old-b", "old-c"]);
        let listed = list(&index, Some(tmp.path()), "0.5.0");
        assert_eq!((listed[0].state, listed[0].replaces.clone()), ("replace", vec!["old-a".to_string(), "old-b".into()]));

        // An index entry that does not match the package's replaces is refused.
        let mut e = find_entry(&index, "demo", "0.5.0").unwrap();
        e.replaces.pop();
        assert!(install_bytes(tmp.path(), &e, &zip, "0.5.0").is_err());
        assert!(package::is_installed(tmp.path(), "old-a"));

        // A hand-picked copy of the official package removes them too, any other only hides them.
        let other = zip_bytes(&merged.replace("1.2.0", "1.2.1"));
        super::super::install_confirmed_zip(tmp.path(), &other, &sha256(&other), Some(&index), "0.5.0").unwrap();
        assert!(package::is_installed(tmp.path(), "old-a") && package::is_installed(tmp.path(), "old-b"));
        super::super::install_confirmed_zip(tmp.path(), &zip, &sha256(&zip), Some(&index), "0.5.0").unwrap();
        assert!(!package::is_installed(tmp.path(), "old-a") && !package::is_installed(tmp.path(), "old-b"));
        assert_eq!(list(&index, Some(tmp.path()), "0.5.0")[0].state, "installed");

        old("old-a");
        let e = find_entry(&index, "demo", "0.5.0").unwrap();
        install_bytes(tmp.path(), &e, &zip, "0.5.0").unwrap();
        assert!(!package::is_installed(tmp.path(), "old-a") && package::is_installed(tmp.path(), "demo"));
    }

    #[test]
    fn install_rejects_package_that_differs_from_index() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = zip_bytes(&PLUGIN.replace("1.2.0", "1.3.0"));
        let mut e = find_entry(&index_for(&zip, "0.5.0"), "demo", "0.5.0").unwrap();
        assert!(format!("{:#}", install_bytes(tmp.path(), &e, &zip, "0.5.0").unwrap_err()).contains("不一致"));
        e.version = "1.3.0".into();
        e.id = "other".into();
        assert!(install_bytes(tmp.path(), &e, &zip, "0.5.0").is_err());
        assert!(!package::is_installed(tmp.path(), "demo"));
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0, "temporary files left behind");
    }

    #[test]
    fn download_with_injected_fetch() {
        let zip = zip_bytes(PLUGIN);
        let base = "https://market.test/plugins/";
        let mut files: HashMap<String, Vec<u8>> = HashMap::new();
        files.insert(format!("{base}plugins.json"), serde_json::to_vec(&index_for(&zip, "0.5.0")).unwrap());
        files.insert(format!("{base}demo-1.2.0.zip"), zip.clone());
        let fetch = |files: HashMap<String, Vec<u8>>| {
            move |url: String, max: u64| {
                let r = files.get(&url).cloned().ok_or_else(|| anyhow!("插件市场上找不到 {url}（404）"));
                async move {
                    let b = r?;
                    if b.len() as u64 > max {
                        bail!("too big");
                    }
                    Ok(b)
                }
            }
        };
        let (_, e, bytes) = rt(download(fetch(files.clone()), base, "demo", "0.5.0")).unwrap();
        assert_eq!(bytes, zip);
        assert_eq!(e.version, "1.2.0");

        // A tampered package never reaches the installer.
        let mut evil = files.clone();
        let mut z = zip.clone();
        z[10] ^= 0xff;
        evil.insert(format!("{base}demo-1.2.0.zip"), z);
        assert!(format!("{:#}", rt(download(fetch(evil), base, "demo", "0.5.0")).unwrap_err()).contains("SHA-256"));
        let mut gone = files.clone();
        gone.remove(&format!("{base}demo-1.2.0.zip"));
        assert!(format!("{:#}", rt(download(fetch(gone), base, "demo", "0.5.0")).unwrap_err()).contains("404"));
        assert!(rt(download(fetch(files), base, "demo", "0.4.0")).is_err());
    }

    #[test]
    fn download_does_not_retry_integrity_failures() {
        let zip = zip_bytes(PLUGIN);
        let index = serde_json::to_vec(&index_for(&zip, "0.5.0")).unwrap();
        for truncate in [false, true] {
            let mut bad = zip.clone();
            if truncate {
                bad.pop();
            } else {
                bad[10] ^= 0xff;
            }
            let calls = std::cell::Cell::new(0);
            let fetch = |url: String, _: u64| {
                calls.set(calls.get() + 1);
                let bytes = if url.ends_with("plugins.json") { index.clone() } else { bad.clone() };
                async move { Ok(bytes) }
            };
            let err = rt(download_from_bases(fetch, &["https://direct.test/".into(), "https://cloudflare.test/".into()], "demo", "0.5.0")).unwrap_err();
            assert!(err.to_string().contains(if truncate { "大小" } else { "SHA-256" }), "{err:#}");
            assert_eq!(calls.get(), 2, "only fetch the index and package once each");
        }
    }

    #[test]
    fn download_direct_first_and_network_fallback_keep_hosts_together() {
        let zip = zip_bytes(PLUGIN);
        let direct_index = serde_json::to_vec(&index_for(&zip, "0.5.0")).unwrap();
        let mut fallback_index = index_for(&zip, "0.5.0");
        fallback_index["plugins"][0]["file"] = "demo-fallback.zip".into();
        let fallback_index = serde_json::to_vec(&fallback_index).unwrap();
        let bases = vec!["https://direct.test/".into(), "https://cloudflare.test/".into()];
        // The direct host works, or fails fetching either the index or package.
        // Non-retryable HTTP errors (e.g. 404) also permit host fallback.
        for fail_at in [None, Some("plugins.json"), Some("demo-1.2.0.zip")] {
            for retry in [false, true] {
                let calls = std::cell::RefCell::new(Vec::new());
                let fetch = |url: String, _: u64| {
                    calls.borrow_mut().push(url.clone());
                    let result = if url.starts_with(&bases[0]) && fail_at.is_some_and(|f| url.ends_with(f)) {
                        Err(anyhow::Error::new(NetError { message: "network/HTTP failure".into(), retry }))
                    } else if url == format!("{}plugins.json", bases[0]) {
                        Ok(direct_index.clone())
                    } else if url == format!("{}plugins.json", bases[1]) {
                        Ok(fallback_index.clone())
                    } else if url == format!("{}demo-1.2.0.zip", bases[0]) || url == format!("{}demo-fallback.zip", bases[1]) {
                        Ok(zip.clone())
                    } else { panic!("mixed index/package hosts: {url}") };
                    async move { result }
                };
                let (_, entry, bytes) = rt(download_from_bases(fetch, &bases, "demo", "0.5.0")).unwrap();
                assert_eq!(bytes, zip);
                let mut expected = vec![format!("{}plugins.json", bases[0])];
                if fail_at != Some("plugins.json") { expected.push(format!("{}demo-1.2.0.zip", bases[0])); }
                if fail_at.is_some() {
                    expected.extend([format!("{}plugins.json", bases[1]), format!("{}demo-fallback.zip", bases[1])]);
                    assert_eq!(entry.file, "demo-fallback.zip");
                }
                assert_eq!(*calls.borrow(), expected);
            }
        }
    }

    #[test]
    fn listing_uses_ordered_fallback_but_bad_indexes_stop() {
        let valid = serde_json::to_vec(&index_for(&zip_bytes(PLUGIN), "0.5.0")).unwrap();
        let bases = vec!["https://direct.test/".into(), "https://cloudflare.test/".into()];
        for mode in ["direct", "network", "json", "index", "size"] {
            let calls = std::cell::Cell::new(0);
            let fetch = |url: String, _: u64| {
                calls.set(calls.get() + 1);
                let result = if url.starts_with(&bases[1]) { Ok(valid.clone()) } else {
                    match mode {
                        "network" => Err(anyhow::Error::new(NetError { message: "offline".into(), retry: true })),
                        "json" => Ok(b"invalid JSON".to_vec()),
                        "index" => Ok(b"{}".to_vec()),
                        "size" => Err(anyhow!("file too large")),
                        _ => Ok(valid.clone()),
                    }
                };
                async move { result }
            };
            assert_eq!(rt(fetch_index(fetch, &bases)).is_ok(), matches!(mode, "direct" | "network"));
            assert_eq!(calls.get(), if mode == "network" { 2 } else { 1 });
        }
    }

    #[test]
    fn download_bad_index_does_not_fall_back() {
        let bases = vec!["https://direct.test/".into(), "https://cloudflare.test/".into()];
        for bytes in [b"invalid JSON".to_vec(), b"{}".to_vec(), br#"{"plugins":[{"id":"demo"}]}"#.to_vec()] {
            let calls = std::cell::Cell::new(0);
            let fetch = |_: String, _: u64| {
                calls.set(calls.get() + 1);
                let bytes = bytes.clone();
                async move { Ok(bytes) }
            };
            assert!(rt(download_from_bases(fetch, &bases, "demo", "0.5.0")).is_err());
            assert_eq!(calls.get(), 1);
        }
    }

    /// Serves `routes` over plain HTTP on localhost, one request per connection.
    fn serve(routes: HashMap<&'static str, Vec<u8>>, requests: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(requests) {
                let mut s = stream.unwrap();
                let mut buf = vec![0u8; 4096];
                let mut req = Vec::new();
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = s.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                }
                let line = String::from_utf8_lossy(&req).lines().next().unwrap_or_default().to_string();
                let path = line.split(' ').nth(1).unwrap_or("/");
                let (status, body) = match routes.get(path) {
                    Some(b) => ("200 OK", b.clone()),
                    None => ("404 Not Found", b"missing".to_vec()),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        format!("http://{addr}/plugins/")
    }

    #[test]
    fn install_round_trip_over_http() {
        let zip = zip_bytes(PLUGIN);
        let mut routes = HashMap::new();
        routes.insert("/plugins/plugins.json", serde_json::to_vec(&index_for(&zip, "0.5.0")).unwrap());
        routes.insert("/plugins/demo-1.2.0.zip", zip.clone());
        let base = serve(routes, 4);
        let get = |url: String, max: u64| http_get(url, max, false, "TifMrcView-test");

        let (index, entry, bytes) = rt(download(get, &base, "demo", "0.5.0")).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let m = install_bytes(tmp.path(), &entry, &bytes, "0.5.0").unwrap();
        assert_eq!((m.id.as_str(), m.raw["version"].as_str()), ("demo", Some("1.2.0")));
        assert!(package::read_installed(&tmp.path().join("demo"), "0.5.0").is_ok());
        assert_eq!(list(&index, Some(tmp.path()), "0.5.0")[0].state, "installed");

        // 404, size cap, HTTPS only, nobody listening.
        let err = rt(http_get(format!("{base}nope.zip"), 10, false, "t")).unwrap_err();
        assert!(format!("{err:#}").contains("404"), "{err:#}");
        let err = rt(http_get(format!("{base}demo-1.2.0.zip"), 10, false, "t")).unwrap_err();
        assert!(format!("{err:#}").contains("太大"), "{err:#}");
        assert!(format!("{:#}", rt(http_get(base.clone(), 10, true, "t")).unwrap_err()).contains("HTTPS"));
        let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let err = rt(http_get(format!("http://{dead}/x"), 10, false, "t")).unwrap_err();
        assert!(format!("{err:#}").contains("连不上"), "{err:#}");
    }

    /// Scripted responses, with an optional stall after the headers. Dropping
    /// `stop` shuts the server down even when the client makes fewer requests.
    fn serve_sequence(
        responses: Vec<(u16, Vec<u8>, Duration)>,
    ) -> (String, std::sync::mpsc::Sender<()>, std::thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let (stop, stopped) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut requests = 0;
            while matches!(stopped.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut buf = [0u8; 4096];
                let mut req = Vec::new();
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                if !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    continue;
                }
                let (status, body, delay) = &responses[requests.min(responses.len() - 1)];
                requests += 1;
                let _ = write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let split = if delay.is_zero() { 0 } else { body.len().min(1) };
                let _ = stream.write_all(&body[..split]);
                std::thread::sleep(*delay);
                let _ = stream.write_all(&body[split..]);
            }
            requests
        });
        (format!("http://{addr}/plugins/"), stop, server)
    }

    #[test]
    fn http_retries_transient_status() {
        let responses = [503, 429, 200].into_iter().map(|s| (s, b"ok".to_vec(), Duration::ZERO)).collect();
        let (base, stop, server) = serve_sequence(responses);
        let result = rt(http_get(format!("{base}plugins.json"), 10, false, "t"));
        drop(stop);
        assert_eq!(server.join().unwrap(), 3);
        assert_eq!(result.unwrap(), b"ok");
    }

    fn test_client(read_timeout: Duration) -> reqwest::Client {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder().no_proxy().read_timeout(read_timeout).retry(reqwest::retry::never()).build().unwrap()
    }

    async fn test_get(client: &reqwest::Client, url: &str, max: u64) -> Result<Vec<u8>> {
        http_get_retry(client, url, max, Duration::from_secs(3), [Duration::from_millis(5), Duration::from_millis(15)]).await
    }

    #[test]
    fn http_retry_status_policy_and_attempt_limit() {
        let client = test_client(Duration::from_secs(1));
        for status in [500, 502, 503, 504, 599, 429, 400, 401, 403, 404, 408] {
            let (base, stop, server) = serve_sequence(vec![(status, b"error".to_vec(), Duration::ZERO)]);
            let result = rt(test_get(&client, &base, 10));
            drop(stop);
            let attempts = if status >= 500 || status == 429 { 3 } else { 1 };
            assert_eq!(server.join().unwrap(), attempts, "HTTP {status}");
            let error = result.unwrap_err();
            assert!(error.to_string().contains(&status.to_string()), "{error:#}");
            assert_eq!(retryable(&error), status >= 500 || status == 429);
        }
    }

    #[test]
    fn http_retries_body_timeout_from_scratch() {
        let client = test_client(Duration::from_millis(300));
        let (base, stop, server) = serve_sequence(vec![
            (200, b"stale".to_vec(), Duration::from_millis(450)),
            (200, b"ok".to_vec(), Duration::ZERO),
        ]);
        let result = rt(test_get(&client, &base, 10));
        drop(stop);
        assert_eq!(server.join().unwrap(), 2);
        assert_eq!(result.unwrap(), b"ok");
    }

    #[test]
    fn http_overall_budget_limits_requests_and_backoff() {
        let client = test_client(Duration::from_secs(1));
        for (status, stall, backoff) in [(200, 300, 5), (503, 0, 1000)] {
            let (base, stop, server) = serve_sequence(vec![(status, b"ok".to_vec(), Duration::from_millis(stall))]);
            let started = Instant::now();
            let result = rt(http_get_retry(&client, &base, 10, Duration::from_millis(100), [Duration::from_millis(backoff); 2]));
            let elapsed = started.elapsed();
            drop(stop);
            assert_eq!(server.join().unwrap(), 1);
            assert!(result.is_err());
            assert!(elapsed < Duration::from_millis(500), "budget exceeded: {elapsed:?}");
        }
    }

    #[test]
    fn http_connection_errors_retry_but_invalid_urls_and_size_caps_do_not() {
        let client = test_client(Duration::from_secs(1));
        let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let error = rt(http_get_once(&client, &format!("http://{dead}/"), 10, Duration::from_secs(1))).unwrap_err();
        assert!(retryable(&error));
        // Exactly a string the interface translates, no reqwest detail. A
        // refused port can also time out (Windows retries the SYN for ~2 s).
        let shown = format!("{error:#}");
        assert!(["连不上插件市场，请检查网络后再试", "连接插件市场超时，请检查网络后再试"].contains(&shown.as_str()), "{shown}");
        let error = rt(test_get(&client, "invalid://url", 10)).unwrap_err();
        assert!(!retryable(&error));

        let (base, stop, server) = serve_sequence(vec![(200, b"too big".to_vec(), Duration::ZERO)]);
        let result = rt(test_get(&client, &base, 1));
        drop(stop);
        assert_eq!(server.join().unwrap(), 1);
        let error = result.unwrap_err();
        assert!(error.to_string().contains("太大"));
        assert!(!retryable(&error));
    }

    #[test]
    fn http_download_retries_index_and_zip() {
        let zip = zip_bytes(PLUGIN);
        let index = serde_json::to_vec(&index_for(&zip, "0.5.0")).unwrap();
        let (base, stop, server) = serve_sequence(vec![
            (503, vec![], Duration::ZERO),
            (200, index, Duration::ZERO),
            (429, vec![], Duration::ZERO),
            (200, zip.clone(), Duration::ZERO),
        ]);
        let client = test_client(Duration::from_secs(1));
        let fetch = |url: String, max: u64| {
            let client = &client;
            async move { test_get(client, &url, max).await }
        };
        let result = rt(download(fetch, &base, "demo", "0.5.0"));
        drop(stop);
        assert_eq!(server.join().unwrap(), 4);
        assert_eq!(result.unwrap().2, zip);
    }
}
