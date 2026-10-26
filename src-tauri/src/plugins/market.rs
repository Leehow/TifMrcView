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
pub const MAX_PACKAGE: u64 = 50 << 20;
/// Largest `plugins.json` (bytes).
pub const MAX_INDEX: u64 = 4 << 20;
pub const SCHEME: &str = "tifmrcview";

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
    Ok(Entry { id: id.to_string(), name, version, file, size, sha256 })
}

/// Checks a download against its index entry.
pub fn verify(entry: &Entry, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 != entry.size {
        bail!("下载的插件包大小不对（可能没下载完整），请重试");
    }
    let hash: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    if hash != entry.sha256 {
        bail!("下载的插件包校验失败（SHA-256 不一致），已取消安装");
    }
    Ok(())
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

/// Installs a downloaded, verified package like 加载插件… does.
pub fn install_bytes(dir: &Path, entry: &Entry, bytes: &[u8], app_version: &str) -> Result<Manifest> {
    std::fs::create_dir_all(dir).context("无法创建插件文件夹")?;
    let tmp = package::unique_tmp(dir, "download", &entry.id).with_extension("zip");
    std::fs::write(&tmp, bytes).context("无法保存下载的插件包")?;
    let res = package::install_checked(dir, &tmp, app_version, |unpacked, m| {
        if m.id != entry.id || package::compare_versions(m.raw["version"].as_str().unwrap_or(""), &entry.version)? != Ordering::Equal {
            bail!("插件包和插件市场的信息不一致");
        }
        super::registry::validate_package(unpacked, m)
    });
    let _ = std::fs::remove_file(&tmp);
    res
}

/// What the plugin dialog shows for one market package.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    /// The `plugins.json` entry as published.
    pub entry: Value,
    /// Version installed now, if any.
    pub installed: Option<String>,
    /// "install", "update", "installed", "builtin" or "needsApp".
    pub state: &'static str,
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
            let state = match &have {
                _ if builtin => "builtin",
                Some(h) if !newer(h) => "installed",
                _ if too_old => "needsApp",
                Some(_) => "update",
                None => "install",
            };
            Some(Listed { entry: p.clone(), installed: have, state })
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
    let client = reqwest::Client::builder()
        .https_only(https_only)
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        // The loop below owns retries, including response-body timeouts.
        .retry(reqwest::retry::never())
        .build()
        .map_err(offline)?;
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
            bail!("连接插件市场超时，请检查网络后再试");
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
        bail!("插件市场上找不到 {name}（404）");
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

    const PLUGIN: &str = r#"{"id":"demo","name":"Demo","version":"1.2.0","main":"index.js","minAppVersion":"0.5.0"}"#;

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
             "file": "demo-1.2.0.zip", "size": zip.len(), "sha256": sha(zip)},
            {"id": "i18n", "name": "多语言", "version": "1.0.0", "builtin": true},
        ]})
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
            let err = rt(download(fetch, "https://market.test/", "demo", "0.5.0")).unwrap_err();
            assert!(err.to_string().contains(if truncate { "大小" } else { "SHA-256" }), "{err:#}");
            assert_eq!(calls.get(), 2, "only fetch the index and package once each");
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
        // Exactly the string the interface translates, no reqwest detail.
        assert_eq!(format!("{error:#}"), "连不上插件市场，请检查网络后再试");
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
