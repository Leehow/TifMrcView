//! Only the bundled engine is executable; data and operations travel as JSON.
use super::digest;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use ssh2::Session;
use std::{
    io::{Read, Write},
    path::Path,
};
#[cfg(feature = "pro")]
const PACKAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ssh-engine.tar.gz"));
#[cfg(not(feature = "pro"))]
const PACKAGE: &[u8] = &[];
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
pub(super) fn call(session: &Session, args: &Value) -> Result<Value> {
    ensure!(
        !PACKAGE.is_empty(),
        "当前安装包未附带远程计算引擎，请更新 App"
    );
    let sftp = session.sftp()?;
    let home = sftp.realpath(Path::new("."))?;
    let base = home.join(".cache/tifmrcview/engines");
    let hash = digest(PACKAGE);
    let dir = base.join(&hash);
    let binary = dir.join("tmv");
    let q = |p: &Path| quote(&p.to_string_lossy());
    let installed = exec(
        session,
        &format!(
            "uname -sm; test -f {} && printf ready || true",
            q(&dir.join("ready"))
        ),
        &[],
    )?;
    ensure!(
        installed.starts_with(b"Linux x86_64"),
        "远程计算支持 Linux x86_64；此服务器平台不匹配"
    );
    if !installed.ends_with(b"ready") {
        exec(session, &format!("umask 077; mkdir -p {}", q(&base)), &[])?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let partial = base.join(format!(".{hash}-{}-{nonce}.upload", std::process::id()));
        let stage = base.join(format!(".{hash}-{}-{nonce}.stage", std::process::id()));
        let upload = (|| -> Result<()> {
            let mut f = sftp.create(&partial)?;
            f.write_all(PACKAGE)?;
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
