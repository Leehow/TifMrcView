//! Only the bundled engine is executable; data and operations travel as JSON.
use super::digest;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use ssh2::Session;
use std::{
    io::{Read, Write},
    path::Path,
};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

// The Linux engine (tmv + NVRTC, scripts/pack-ssh-engine.mjs) is not in the
// installer: Pro knows its size and SHA-256 (build.rs) and downloads it from
// its version's folder on the download site the first time a server needs
// it. Local test builds may embed it instead (TMV_SSH_ENGINE_EMBED=1).
#[cfg(feature = "pro")]
const EMBEDDED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ssh-engine.tar.gz"));
#[cfg(feature = "pro")]
const ENGINE_SHA256: &str = env!("TMV_SSH_ENGINE_SHA256");
#[cfg(not(feature = "pro"))]
const EMBEDDED: &[u8] = &[];
#[cfg(not(feature = "pro"))]
const ENGINE_SHA256: &str = "";
const ENGINE_SIZE: &str = env!("TMV_SSH_ENGINE_SIZE");

static URLS: OnceLock<Vec<String>> = OnceLock::new();
static FETCHING: AtomicBool = AtomicBool::new(false);
static DONE: AtomicU64 = AtomicU64::new(0);
static TOTAL: AtomicU64 = AtomicU64::new(0);
static FETCH_LOCK: Mutex<()> = Mutex::new(());

/// `roots`: the download site's folders, e.g. https://tifmrcview.com/tifmrcview/.
pub fn set_engine_urls(roots: &[String], version: &str) {
    let _ = URLS.set(roots.iter().map(|r| format!("{r}{version}/tmv-engine_{version}_linux-x86_64.tar.gz")).collect());
}

/// How far the engine download is, for the 运行位置 line.
pub(super) fn fetch_progress() -> Value {
    json!({"active": FETCHING.load(Ordering::Relaxed), "done": DONE.load(Ordering::Relaxed), "total": TOTAL.load(Ordering::Relaxed)})
}

fn engine_size() -> u64 {
    ENGINE_SIZE.parse().unwrap_or(0)
}

fn engine_hash() -> Result<&'static str> {
    ensure!(!ENGINE_SHA256.is_empty(), "当前安装包未附带远程计算引擎，请更新 App");
    Ok(ENGINE_SHA256)
}

fn local_engine(root: &Path) -> Result<std::path::PathBuf> {
    Ok(root.join("engine").join(format!("{}.tar.gz", engine_hash()?)))
}

/// Whether this computer already has the engine (embedded or downloaded).
pub(super) fn have_engine(root: &Path) -> bool {
    !EMBEDDED.is_empty()
        || local_engine(root).is_ok_and(|p| p.metadata().is_ok_and(|m| m.len() == engine_size()))
}

/// Downloads the engine into `root` once; other engines there are removed.
pub(super) fn fetch_engine(root: &Path) -> Result<()> {
    let _guard = FETCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if have_engine(root) {
        return Ok(());
    }
    let path = local_engine(root)?;
    let urls = URLS.get().cloned().unwrap_or_default();
    ensure!(!urls.is_empty(), "不知道从哪里下载远程计算引擎");
    FETCHING.store(true, Ordering::Relaxed);
    let result = tauri::async_runtime::block_on(crate::fetch::to_file(&urls, &path, engine_size(), ENGINE_SHA256, |done, total| {
        DONE.store(done, Ordering::Relaxed);
        TOTAL.store(total, Ordering::Relaxed);
    }));
    FETCHING.store(false, Ordering::Relaxed);
    result.map_err(|e| anyhow::anyhow!("下载远程计算引擎失败：{e}。请检查本机网络后重试"))?;
    if let Ok(entries) = std::fs::read_dir(path.parent().unwrap()) {
        for e in entries.flatten().filter(|e| e.path() != path) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    Ok(())
}

fn package(root: &Path) -> Result<Cow<'static, [u8]>> {
    if !EMBEDDED.is_empty() {
        return Ok(Cow::Borrowed(EMBEDDED));
    }
    fetch_engine(root)?;
    let path = local_engine(root)?;
    let bytes = std::fs::read(&path)?;
    if digest(&bytes) != engine_hash()? {
        let _ = std::fs::remove_file(&path);
        anyhow::bail!("本机缓存的远程计算引擎已损坏，已删除，请重试");
    }
    Ok(Cow::Owned(bytes))
}

/// Where this server keeps the engine, and whether it is installed there.
fn engine_dir(session: &Session) -> Result<(std::path::PathBuf, std::path::PathBuf, bool)> {
    let sftp = session.sftp()?;
    let home = sftp.realpath(Path::new("."))?;
    let base = home.join(".cache/tifmrcview/engines");
    let dir = base.join(engine_hash()?);
    let installed = exec(
        session,
        &format!(
            "uname -sm; test -f {} && printf ready || true",
            quote(&dir.join("ready").to_string_lossy())
        ),
        &[],
    )?;
    ensure!(
        installed.starts_with(b"Linux x86_64"),
        "远程计算支持 Linux x86_64；此服务器平台不匹配"
    );
    Ok((base, dir, installed.ends_with(b"ready")))
}

/// Whether the server already has this engine (then nothing is downloaded).
pub(super) fn server_has_engine(session: &Session) -> Result<bool> {
    Ok(engine_dir(session)?.2)
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn exec(s: &Session, cmd: &str, input: &[u8]) -> Result<Vec<u8>> {
    let mut c = s.channel_session()?;
    c.exec(cmd)?;
    c.write_all(input)?;
    c.send_eof()?;
    let mut out = vec![];
    (&mut c).take(8 * 1024 * 1024 + 1).read_to_end(&mut out)?;
    ensure!(out.len() <= 8 * 1024 * 1024, "远程响应超过限制");
    let mut err = String::new();
    c.stderr().take(8192).read_to_string(&mut err)?;
    c.wait_close()?;
    ensure!(
        c.exit_status()? == 0,
        "服务器处理引擎不可用：{} {}",
        err.trim(),
        String::from_utf8_lossy(&out)
    );
    Ok(out)
}
pub(super) fn call(session: &Session, args: &Value, root: &Path) -> Result<Value> {
    let hash = engine_hash()?;
    let sftp = session.sftp()?;
    let (base, dir, ready) = engine_dir(session)?;
    let binary = dir.join("tmv");
    let q = |p: &Path| quote(&p.to_string_lossy());
    if !ready {
        let package = package(root)?;
        exec(session, &format!("umask 077; mkdir -p {}", q(&base)), &[])?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let partial = base.join(format!(".{hash}-{}-{nonce}.upload", std::process::id()));
        let stage = base.join(format!(".{hash}-{}-{nonce}.stage", std::process::id()));
        let upload = (|| -> Result<()> {
            let mut f = sftp.create(&partial)?;
            f.write_all(&package)?;
            f.close()?;
            let check = quote(&format!("{hash}  {}\n", partial.display()));
            // Extract privately, verify completely, then publish one immutable directory.
            exec(session,&format!("umask 077; printf %s {check} | sha256sum -c --status && mkdir {} && tar -xzf {} -C {} && (cd {} && sha256sum -c --status SHA256SUMS && chmod 700 tmv && touch ready) && (mv -T {} {} 2>/dev/null || test -f {})",q(&stage),q(&partial),q(&stage),q(&stage),q(&stage),q(&dir),q(&dir.join("ready"))),&[])?;
            Ok(())
        })();
        // Exact generated staging paths belong to this transfer, including failed/racing installs.
        let _ = exec(
            session,
            &format!("rm -f {}; rm -rf {}", q(&partial), q(&stage)),
            &[],
        );
        upload?;
    }
    let mut req = args["request"].clone();
    ensure!(req.is_object(), "缺少远程请求");
    req["protocol"] = json!(1);
    if req["action"] == "submit" {
        let reg = crate::plugins::registry::Registry::global();
        let mut packages = std::collections::BTreeMap::new();
        for ext in reg.extensions() {
            if let Some(p) = reg.preview_package(Path::new(&format!("input.{ext}")))? {
                let id = p.manifest["id"].as_str().context("format id")?.to_owned();
                ensure!(crate::plugins::package::valid_id(&id), "invalid format id");
                packages.entry(id).or_insert(p);
            }
        }
        let mut payloads = vec![];
        let mut version = String::new();
        for (id, p) in packages {
            version = p.app_version;
            payloads.extend([
                (format!("{id}/format.wasm"), p.wasm),
                (
                    format!("{id}/index.js"),
                    b"// native format host\n".to_vec(),
                ),
                (
                    format!("{id}/plugin.json"),
                    serde_json::to_vec(&p.manifest)?,
                ),
            ]);
        }
        ensure!(
            payloads.iter().map(|(_, b)| b.len()).sum::<usize>() <= 64 * 1024 * 1024,
            "远程格式模块超过 64 MB，请停用本次不需要的格式插件"
        );
        if !payloads.is_empty() {
            let identity = digest(
                &payloads
                    .iter()
                    .flat_map(|(n, b)| n.as_bytes().iter().chain(b.iter()).copied())
                    .collect::<Vec<_>>(),
            );
            let formats = base.join(format!("formats-{identity}"));
            let mut files = vec![];
            for (n, b) in payloads {
                let path = formats.join(&n);
                exec(
                    session,
                    &format!("umask 077; mkdir -p {}", q(path.parent().unwrap())),
                    &[],
                )?;
                let h = digest(&b);
                if !sftp
                    .stat(&path)
                    .is_ok_and(|s| s.size == Some(b.len() as u64))
                {
                    let mut f = sftp.create(&path)?;
                    f.write_all(&b)?;
                    f.close()?;
                }
                files.push((n, h));
            }
            req["plugin"] = json!({"root":formats,"files":files,"version":version});
        }
    }
    let checksum = dir.join("SHA256SUMS");
    // Recheck the engine itself each invocation; shared CUDA resources were checked on install.
    // /usr/lib/wsl/lib holds the driver's libcuda under WSL (absent elsewhere, so harmless).
    let libraries=q(&dir.join("cuda/lib64"));
    let cmd=format!("umask 077; cd {} && sha256sum -c --status {} && exec env LD_LIBRARY_PATH={libraries}\"${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}\":/usr/lib/wsl/lib {} agent",q(&dir),q(&checksum),q(&binary));
    let mut result: Value = serde_json::from_slice(&exec(session, &cmd, &serde_json::to_vec(&req)?)?)?;
    // A job's own failure comes back as a state with an `error` field; only an answer
    // without a state is the engine refusing the request itself.
    if result["state"].is_null() {
        if let Some(error) = result["error"].as_str() {
            anyhow::bail!("{error}")
        }
    }
    // Whether results can be written where the user wants them (a read-only disk, or a
    // folder of someone else), asked of the nearest existing folder so nothing is created.
    if req["action"] == "inspect" && result["resources"].is_object() {
        if let Some(out) = req["outputRoot"].as_str().filter(|s| s.starts_with('/')) {
            let probe = format!("p={}; while [ ! -e \"$p\" ]; do p=$(dirname \"$p\"); done; if [ -d \"$p\" ] && [ -w \"$p\" ]; then printf yes; else printf no; fi", quote(out));
            if let Ok(answer) = exec(session, &probe, &[]) {
                result["resources"]["outputWritable"] = json!(answer == b"yes");
            }
        }
    }
    Ok(result)
}
