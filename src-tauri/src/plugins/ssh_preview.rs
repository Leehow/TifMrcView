//! The only remote executable is our bundled, content-addressed Linux helper.
use super::{digest, text};
use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use ssh2::{FileStat, Session};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

const HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ssh-preview-helper"));
const MAX_PNG: u64 = 512 * 1024;

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn command(session: &Session, cmd: &str, input: &[u8], limit: u64) -> Result<Vec<u8>> {
    let mut c = session.channel_session()?;
    c.exec(cmd)?;
    c.write_all(input)?;
    c.send_eof()?;
    let mut output = Vec::new();
    (&mut c).take(limit + 1).read_to_end(&mut output)?;
    if output.len() as u64 > limit {
        let _ = c.close();
        bail!("远程预览返回过大");
    }
    let mut error = String::new();
    c.stderr().take(4096).read_to_string(&mut error)?;
    c.wait_close()?;
    ensure!(c.exit_status()? == 0, "远程预览不可用：{}", error.trim());
    Ok(output)
}

pub(super) fn render(
    session: &Session,
    helper: &mut Option<PathBuf>,
    cache: &Path,
    args: &Value,
) -> Result<Value> {
    ensure!(!HELPER.is_empty(), "此安装包未附带 Linux 预览工具");
    let path = text(args, "path")?;
    ensure!(Path::new(path).is_absolute(), "请选择绝对路径");
    let bytes = args["size"].as_u64().context("缺少文件大小")?;
    let modified = args["modified"]
        .as_u64()
        .context("服务器未提供文件修改时间")?;
    let package = if crate::formats::CORE_EXTENSIONS.contains(
        &Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
    ) {
        None
    } else {
        crate::plugins::registry::Registry::global().preview_package(Path::new(path))?
    };
    let mut package_files = Vec::new();
    let mut package_version = String::new();
    let mut package_hash = String::new();
    if let Some(package) = package {
        let id = package.manifest["id"].as_str().context("格式插件缺少 id")?;
        ensure!(crate::plugins::package::valid_id(id), "格式插件 id 无效");
        package_files.push((format!("{id}/format.wasm"), package.wasm));
        package_files.push((
            format!("{id}/index.js"),
            b"// Preview host does not execute JavaScript.\n".to_vec(),
        ));
        package_files.push((
            format!("{id}/plugin.json"),
            serde_json::to_vec(&package.manifest)?,
        ));
        package_version = package.app_version;
        let mut identity = Vec::new();
        for (name, data) in &package_files {
            identity.extend_from_slice(name.as_bytes());
            identity.extend_from_slice(data);
        }
        package_hash = digest(&identity);
    }
    let key = digest(
        format!(
            "{}:{package_hash}:{path}:{bytes}:{modified}",
            digest(HELPER)
        )
        .as_bytes(),
    );
    let local = cache.join("previews").join(format!("{key}.png"));
    let sftp = session.sftp()?;
    let stat = sftp.stat(Path::new(path))?;
    ensure!(
        stat.is_file() && stat.size == Some(bytes) && stat.mtime == Some(modified),
        "文件已变化，请刷新远程目录"
    );
    if let Ok(png) = std::fs::read(&local) {
        return reply(png);
    }
    if helper.is_none() {
        let system = command(session, "uname -s -m", &[], 128)?;
        ensure!(
            String::from_utf8_lossy(&system).trim() == "Linux x86_64",
            "当前远程预览工具支持 Linux x86_64"
        );
        let home = sftp.realpath(Path::new("."))?;
        let mut root = home.join(".cache");
        // Private application cache; original image directories are never written.
        for part in ["", "tifmrcview", "ssh-preview"] {
            if !part.is_empty() {
                root.push(part);
            }
            match sftp.stat(&root) {
                Ok(s) => ensure!(s.is_dir(), "远程缓存路径不是文件夹"),
                Err(_) => {
                    sftp.mkdir(&root, 0o700)?;
                }
            }
        }
        let executable = root.join(format!("tmv-preview-{}", digest(HELPER)));
        if sftp.stat(&executable).is_err() {
            let partial = root.join(format!(
                ".upload-{}-{}",
                std::process::id(),
                digest(cache.to_string_lossy().as_bytes())
            ));
            let upload = (|| -> Result<()> {
                let mut f = sftp.create(&partial)?;
                f.write_all(HELPER)?;
                f.close()?;
                sftp.setstat(
                    &partial,
                    FileStat {
                        size: None,
                        uid: None,
                        gid: None,
                        perm: Some(0o700),
                        atime: None,
                        mtime: None,
                    },
                )?;
                sftp.rename(&partial, &executable, Some(ssh2::RenameFlags::OVERWRITE))?;
                Ok(())
            })();
            if upload.is_err() {
                let _ = sftp.unlink(&partial);
            }
            upload?;
        }
        *helper = Some(executable);
    }
    let executable_path = helper.as_ref().unwrap();
    let mut plugin = Value::Null;
    if !package_files.is_empty() {
        let root = executable_path
            .parent()
            .unwrap()
            .join(format!("formats-{package_hash}"));
        if sftp.stat(&root).is_err() {
            sftp.mkdir(&root, 0o700)?;
        }
        let mut hashes = Vec::new();
        for (name, data) in package_files {
            let file = root.join(&name);
            if sftp.stat(file.parent().unwrap()).is_err() {
                sftp.mkdir(file.parent().unwrap(), 0o700)?;
            }
            if !sftp
                .stat(&file)
                .is_ok_and(|m| m.size == Some(data.len() as u64))
            {
                let mut output = sftp.create(&file)?;
                output.write_all(&data)?;
                output.close()?;
            }
            hashes.push((name, digest(&data)));
        }
        plugin = json!({"root":root.to_str().context("插件缓存路径编码错误")?,"version":package_version,"files":hashes});
    }
    let executable = helper
        .as_ref()
        .unwrap()
        .to_str()
        .context("远程缓存路径编码错误")?;
    // Verify on every invocation, including a helper retained from an earlier connection.
    // Paths are shell quoted; image paths are sent exclusively as JSON on stdin.
    let expected = quote(&format!("{}  {}\n", digest(HELPER), executable));
    let cmd = format!(
        "printf %s {expected} | sha256sum -c --status && exec env RAYON_NUM_THREADS=2 {}",
        quote(executable)
    );
    let request = serde_json::to_vec(
        &json!({"path":path,"size":256,"bytes":bytes,"modified":modified,"plugin":plugin}),
    )?;
    let png = command(session, &cmd, &request, MAX_PNG)?;
    let after = sftp.stat(Path::new(path))?;
    ensure!(
        after.size == Some(bytes) && after.mtime == Some(modified),
        "文件已变化，请刷新远程目录"
    );
    let result = reply(png.clone())?;
    std::fs::create_dir_all(local.parent().unwrap())?;
    std::fs::write(local, png)?;
    Ok(result)
}

fn reply(png: Vec<u8>) -> Result<Value> {
    ensure!(
        png.len() as u64 <= MAX_PNG && png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "无效远程缩略图"
    );
    Ok(
        json!({"url":format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_cannot_escape_shell_quotes() {
        assert_eq!(quote("a'b; $(touch x)"), "'a'\\''b; $(touch x)'");
    }
    #[test]
    fn rejects_non_png_reply() {
        assert!(reply(b"error".to_vec()).is_err());
    }
}
