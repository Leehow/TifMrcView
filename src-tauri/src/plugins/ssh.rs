//! Read-only image access over SFTP. Remote writes are limited to the bundled
//! preview helper in the user's cache; passwords and key contents never persist.
use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ssh2::{CheckResult, KnownHostFileKind, Session};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "ssh_preview.rs"]
mod preview;
#[path = "ssh_processing.rs"]
mod processing;
#[path = "ssh_tcp.rs"]
mod tcp;

#[derive(Default)]
pub struct Ssh {
    connections: Mutex<HashMap<String, Arc<Connection>>>,
    sequence: AtomicU64,
    #[cfg(test)]
    storage: Option<PathBuf>,
}

struct Connection {
    preview_helper: Mutex<Option<PathBuf>>,
    /// How this connection logged in, kept in memory only, so a connection the server
    /// dropped (or that refuses new channels) is opened again without asking.
    login: Mutex<Option<Value>>,
    session: Mutex<Session>,
    host: String,
    port: u16,
    fingerprint: String,
    needs_trust: bool,
    created: Instant,
    authenticated: AtomicBool,
    closed: AtomicBool,
    cancelled: AtomicBool,
    done: AtomicU64,
    total: AtomicU64,
    cache: PathBuf,
}

fn text<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args[name]
        .as_str()
        .filter(|s| !s.contains('\0'))
        .with_context(|| format!("缺少 {name}"))
}

fn known_hosts_path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("无法定位设置目录")?
        .join("com.leehow.tifmrcview")
        .join("ssh_known_hosts"))
}

fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn cache_path(root: &Path, remote: &Path) -> Result<PathBuf> {
    let name = remote
        .file_name()
        .and_then(|v| v.to_str())
        .context("文件名无法表示为 UTF-8")?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let safe = safe.trim_end_matches(['.', ' ']);
    if safe.is_empty() {
        bail!("无效文件名");
    }
    // Prefix defeats Windows reserved names; hash avoids sanitization collisions.
    let stem = safe.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ]
    .contains(&stem.as_str());
    let name = if reserved {
        format!("ssh-{safe}")
    } else {
        safe.to_owned()
    };
    Ok(root
        .join(digest(remote.to_string_lossy().as_bytes()))
        .join(name))
}

/// Logs `session` in with the user's choice (password, key file or agent).
fn login(session: &Session, args: &Value) -> Result<()> {
    let username = text(args, "username")?;
    match args["authMethod"].as_str().unwrap_or("password") {
        "password" => session
            .userauth_password(username, text(args, "password")?)
            .context("身份验证失败，请检查用户名、密码以及服务器是否允许密码登录")?,
        "keyFile" => {
            let value = text(args, "keyPath")?;
            let path = if let Some(relative) = value.strip_prefix("~/") {
                dirs::home_dir().context("无法定位主目录")?.join(relative)
            } else {
                PathBuf::from(value)
            };
            let metadata = path
                .metadata()
                .context("无法读取私钥，请重新选择本地私钥文件")?;
            if !metadata.is_file() || metadata.len() > 1024 * 1024 {
                bail!("请选择有效的 SSH 私钥文件（小于 1 MB）");
            }
            let passphrase = args["passphrase"].as_str().filter(|s| !s.is_empty());
            if let Err(error) =
                session.userauth_pubkey_file(username, None, &path, passphrase)
            {
                // Use only the unlocked agent identity matching the selected key.
                let from_agent = (|| -> Result<()> {
                    if passphrase.is_some() {
                        return Err(error.into());
                    }
                    let mut public_path = path.as_os_str().to_os_string();
                    public_path.push(".pub");
                    let public = std::fs::read_to_string(PathBuf::from(public_path))?;
                    let encoded =
                        public.split_whitespace().nth(1).context("公钥格式无效")?;
                    let blob = base64::engine::general_purpose::STANDARD.decode(encoded)?;
                    let mut agent = session.agent()?;
                    agent.connect()?;
                    agent.list_identities()?;
                    let identity = agent
                        .identities()?
                        .into_iter()
                        .find(|key| key.blob() == blob)
                        .context("选中的密钥尚未在本机解锁")?;
                    agent.userauth(username, &identity)?;
                    Ok(())
                })();
                from_agent.context("密钥登录失败：请检查私钥和用户名；加密私钥请填写口令，或先在本机解锁对应密钥")?;
            }
        }
        _ => bail!("不支持的 SSH 登录方式"),
    }
    Ok(())
}

/// A transport failure of the connection itself, as opposed to a refused request.
fn broken(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        if let Some(e) = c.downcast_ref::<ssh2::Error>() {
            // CHANNEL_FAILURE, SOCKET_SEND, SOCKET_DISCONNECT, SOCKET_RECV
            return matches!(e.code(), ssh2::ErrorCode::Session(-21 | -7 | -13 | -43));
        }
        c.downcast_ref::<std::io::Error>().is_some_and(|e| {
            matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::UnexpectedEof
            )
        })
    })
}

impl Ssh {
    fn trust_path(&self) -> Result<PathBuf> {
        #[cfg(test)]
        if let Some(root) = &self.storage {
            return Ok(root.join("known_hosts"));
        }
        known_hosts_path()
    }

    fn cache_root(&self) -> Result<PathBuf> {
        #[cfg(test)]
        if let Some(root) = &self.storage {
            return Ok(root.join("cache"));
        }
        Ok(dirs::cache_dir()
            .context("无法定位缓存目录")?
            .join("com.leehow.tifmrcview/ssh"))
    }

    pub fn call(&self, method: &str, args: Value) -> Result<Value> {
        if method == "probe" {
            return self.probe(&args);
        }
        let id = text(&args, "connection")?;
        let conn = self
            .connections
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("连接已断开，请重新连接")?;
        if method == "disconnect" {
            conn.closed.store(true, Ordering::Relaxed);
            conn.cancelled.store(true, Ordering::Relaxed);
            self.connections.lock().unwrap().remove(id);
            return Ok(Value::Null);
        }
        if method == "cancel" {
            conn.cancelled.store(true, Ordering::Relaxed);
            return Ok(Value::Null);
        }
        if method == "progress" {
            return Ok(
                json!({"done": conn.done.load(Ordering::Relaxed), "total": conn.total.load(Ordering::Relaxed)}),
            );
        }
        let mut session = conn.session.lock().unwrap();
        if conn.closed.load(Ordering::Relaxed) {
            bail!("连接已断开");
        }
        if method == "authenticate" {
            if conn.created.elapsed() > Duration::from_secs(120) {
                bail!("连接确认已过期，请重新连接");
            }
            if conn.needs_trust && args["trust"] != true {
                bail!("请先确认服务器指纹");
            }
            login(&session, &args)?;
            if !session.authenticated() {
                bail!("服务器未接受身份验证");
            }
            // Test SFTP before declaring the connection ready.
            let home = session.sftp()?.realpath(Path::new("."))?;
            if conn.needs_trust {
                let path = self.trust_path()?;
                std::fs::create_dir_all(path.parent().unwrap())?;
                // Serialize read/append/write across simultaneous connections.
                static TRUST_WRITE: Mutex<()> = Mutex::new(());
                let _guard = TRUST_WRITE.lock().unwrap();
                let mut hosts = session.known_hosts()?;
                if path.exists() {
                    hosts.read_file(&path, KnownHostFileKind::OpenSSH)?;
                }
                let (key, kind) = session.host_key().context("服务器未提供密钥")?;
                match hosts.check_port(&conn.host, conn.port, key) {
                    CheckResult::Mismatch | CheckResult::Failure => {
                        bail!("服务器密钥与已保存记录不一致")
                    }
                    CheckResult::NotFound => {
                        let host = if conn.port == 22 {
                            conn.host.clone()
                        } else {
                            format!("[{}]:{}", conn.host, conn.port)
                        };
                        hosts.add(&host, key, "TifMrcView SSH", kind.into())?;
                        hosts.write_file(&path, KnownHostFileKind::OpenSSH)?;
                    }
                    CheckResult::Match => {}
                }
            }
            conn.authenticated.store(true, Ordering::Relaxed);
            *conn.login.lock().unwrap() = Some(args.clone());
            return Ok(
                json!({"path": home.to_str().context("远程主目录不是 UTF-8")?, "fingerprint": conn.fingerprint}),
            );
        }
        if !conn.authenticated.load(Ordering::Relaxed) {
            bail!("请先登录");
        }
        let first = self.run(&conn, &session, method, &args);
        match first {
            Err(e) if broken(&e) && conn.login.lock().unwrap().is_some() => {
                // The server dropped this connection or refuses new channels on it
                // (sshd restarted, too many sessions, a tunnel that came back): open a
                // fresh one to the same, already trusted server and try once more.
                let fresh = self
                    .reopen(&conn)
                    .with_context(|| format!("与服务器的连接断了，自动重连也没成功（{e}）。请检查服务器或网络，然后重新连接"))?;
                *session = fresh;
                self.run(&conn, &session, method, &args)
            }
            other => other,
        }
    }

    fn reopen(&self, conn: &Connection) -> Result<Session> {
        let (session, fingerprint, _) = self.open(&conn.host, conn.port)?;
        ensure!(fingerprint == conn.fingerprint, "服务器密钥变了，已停止自动重连");
        let args = conn.login.lock().unwrap().clone().context("请重新连接")?;
        login(&session, &args)?;
        ensure!(session.authenticated(), "服务器未接受身份验证");
        Ok(session)
    }

    fn run(&self, conn: &Connection, session: &Session, method: &str, args: &Value) -> Result<Value> {
        if method == "processing" { return processing::call(session, args); }
        if method == "thumbnail" {
            return preview::render(
                session,
                &mut conn.preview_helper.lock().unwrap(),
                &conn.cache,
                args,
            );
        }
        if conn.cancelled.load(Ordering::Relaxed)
            && !["list", "download", "readText", "resolveMovie"].contains(&method)
        {
            bail!("操作已取消");
        }
        let sftp = session.sftp().context("无法打开 SFTP，请重新连接")?;
        let remote = sftp
            .realpath(Path::new(text(args, "path")?))
            .context("无法访问路径，请检查路径及读取权限")?;
        match method {
            "resolveMovie" => {
                let name = text(args, "movie")?
                    .rsplit(['/', '\\'])
                    .next()
                    .context("缺少电影文件名")?;
                if name.is_empty() || name == "." || name == ".." {
                    bail!("无效电影文件名");
                }
                let parent = remote.parent().context("缺少 MDOC 目录")?;
                let mut dirs = vec![parent.to_path_buf()];
                if let Some(root) = parent.parent() {
                    dirs.push(root.to_path_buf());
                }
                for root in dirs.clone() {
                    let mut count = 0;
                    for (path, stat) in sftp.readdir(&root)? {
                        if stat.is_dir()
                            && !path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .starts_with('.')
                        {
                            count += 1;
                            if count > 128 {
                                break;
                            }
                            dirs.push(path);
                        }
                    }
                }
                let mut found = std::collections::HashSet::new();
                for dir in dirs {
                    let path = dir.join(name);
                    if sftp.stat(&path).is_ok_and(|s| s.is_file()) {
                        found.insert(sftp.realpath(&path)?);
                    }
                }
                if found.len() > 1 {
                    bail!("找到多个同名电影，请在侧栏选择正确的电影目录");
                }
                if let Some(path) = found.into_iter().next() {
                    return Ok(
                        json!({"path":path.to_string_lossy(),"folder":path.parent().unwrap().to_string_lossy()}),
                    );
                }
                bail!("未在 MDOC 目录、上级目录及其子目录中找到 {name}，请在侧栏中选择电影所在目录")
            }
            "readText" => {
                let mut input = sftp.open(&remote)?;
                let before = input.stat()?;
                if !before.is_file()
                    || before.size != args["size"].as_u64()
                    || before.mtime != args["modified"].as_u64()
                {
                    bail!("文件已变化，请刷新远程目录");
                }
                let extension = remote
                    .extension()
                    .and_then(|v| v.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if ![
                    "mdoc", "txt", "log", "json", "star", "xml", "yaml", "yml", "py", "sh", "csv",
                    "tlt", "rawtlt", "md", "ini", "cfg", "pid",
                ]
                .contains(&extension.as_str())
                {
                    return Ok(
                        json!({"text":"此文件可在目录中查看。它不是图像或支持的文本格式。", "truncated":false}),
                    );
                }
                let mut bytes = Vec::new();
                (&mut input).take(256 * 1024 + 1).read_to_end(&mut bytes)?;
                let truncated = bytes.len() > 256 * 1024;
                bytes.truncate(256 * 1024);
                if bytes.contains(&0) {
                    bail!("此文件包含二进制内容，无法作为文本查看");
                }
                let after = input.stat()?;
                if before.size != after.size || before.mtime != after.mtime {
                    bail!("文件读取时发生变化，请刷新目录");
                }
                Ok(json!({"text":String::from_utf8_lossy(&bytes),"truncated":truncated}))
            }
            "list" => {
                let mut entries = vec![];
                let mut dir = sftp.opendir(&remote).context("无法读取目录")?;
                loop {
                    let (name, mut stat) = match dir.readdir() {
                        Ok(e) => e,
                        Err(e) if e.code() == ssh2::ErrorCode::Session(-16) => break, // LIBSSH2_ERROR_FILE = end of directory
                        Err(e) => return Err(e).context("目录读取中断"),
                    };
                    let name = name.to_str().context("目录包含非 UTF-8 文件名")?;
                    if name == "." || name == ".." {
                        continue;
                    }
                    if entries.len() >= 50_000 {
                        bail!("目录条目超过 50,000，请直接输入更具体的子目录路径");
                    }
                    let path = remote.join(name);
                    if stat.file_type().is_symlink() {
                        if let Ok(target) = sftp.stat(&path) {
                            stat = target;
                        }
                    }
                    let supported = stat.is_file()
                        && crate::formats::is_supported(&path)
                        && !crate::labels::is_labels_file(&path);
                    entries.push(json!({"name": name, "path": path.to_str().context("路径不是 UTF-8")?,
                        "directory": stat.is_dir(), "supported": supported, "size": stat.size.unwrap_or(0),
                        "modified": stat.mtime, "local": cache_path(&conn.cache.join(format!("{}-{}", stat.size.unwrap_or(0), stat.mtime.unwrap_or(0))), &path)?.to_string_lossy()}));
                }
                entries.sort_by(|a, b| {
                    b["directory"]
                        .as_bool()
                        .cmp(&a["directory"].as_bool())
                        .then_with(|| {
                            crate::formats::sequence::natural_cmp(
                                a["name"].as_str().unwrap(),
                                b["name"].as_str().unwrap(),
                            )
                        })
                });
                Ok(json!({"path": remote.to_str().context("路径不是 UTF-8")?, "entries": entries}))
            }
            "download" => {
                conn.cancelled.store(false, Ordering::Relaxed);
                conn.done.store(0, Ordering::Relaxed);
                conn.total.store(0, Ordering::Relaxed);
                let mut input = sftp.open(&remote).context("无法读取远程文件")?;
                let before = input.stat()?;
                if !before.is_file() {
                    bail!("请选择普通图像文件");
                }
                let size = before.size.context("服务器没有提供文件大小")?;
                conn.total.store(size, Ordering::Relaxed);
                if args["size"].as_u64() != Some(size) || args["modified"].as_u64() != before.mtime
                {
                    bail!("文件自目录加载后已变化，请刷新远程目录后重试");
                }
                let local = cache_path(
                    &conn
                        .cache
                        .join(format!("{}-{}", size, before.mtime.unwrap_or(0))),
                    Path::new(text(args, "path")?),
                )?;
                let stamp = local.with_extension("ssh-stamp");
                let version = json!({"size": size, "modified": before.mtime});
                // Missing mtimes cannot establish freshness, so always re-fetch.
                if before.mtime.is_some()
                    && local.metadata().is_ok_and(|m| m.len() == size)
                    && std::fs::read(&stamp)
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                        == Some(version.clone())
                {
                    conn.done.store(size, Ordering::Relaxed);
                    return Ok(json!({"path": local.to_string_lossy()}));
                }
                std::fs::create_dir_all(local.parent().unwrap())?;
                let partial = local.with_extension("ssh-part");
                let transfer = (|| -> Result<()> {
                    let mut output = std::fs::File::create(&partial)?;
                    let mut buf = vec![0; 256 * 1024];
                    let mut done = 0u64;
                    loop {
                        if conn.closed.load(Ordering::Relaxed)
                            || conn.cancelled.load(Ordering::Relaxed)
                        {
                            bail!("传输已取消");
                        }
                        let n = input.read(&mut buf).context("传输中断，请重试")?;
                        if n == 0 {
                            break;
                        }
                        done += n as u64;
                        if done > size {
                            bail!("远程文件正在变化，请稍后重试");
                        }
                        output.write_all(&buf[..n])?;
                        conn.done.store(done, Ordering::Relaxed);
                    }
                    let after = input.stat()?;
                    if done != size || after.size != before.size || after.mtime != before.mtime {
                        bail!("远程文件正在变化，请稍后重试");
                    }
                    output.sync_all()?;
                    drop(output);
                    // A replacement must not invalidate an image still mapped by a viewer.
                    if local.exists() {
                        bail!("该文件的缓存版本已变化，请断开后重新连接加载新版本");
                    }
                    std::fs::rename(&partial, &local)?;
                    std::fs::write(&stamp, serde_json::to_vec(&version)?)?;
                    Ok(())
                })();
                if transfer.is_err() {
                    let _ = std::fs::remove_file(&partial);
                }
                transfer?;
                Ok(json!({"path": local.to_string_lossy()}))
            }
            _ => bail!("未知 SSH 操作"),
        }
    }

    /// A handshaken session to `host:port` whose key is checked against the trusted hosts.
    fn open(&self, host: &str, port: u16) -> Result<(Session, String, bool)> {
        let addresses: Vec<_> = (host, port)
            .to_socket_addrs()
            .context("无法解析主机地址")?
            .take(4)
            .collect();
        let started = Instant::now();
        let socket = tcp::connect(
            &addresses,
            cfg!(target_os = "macos"),
            |address, timeout| TcpStream::connect_timeout(&address, timeout),
            || started.elapsed(),
            std::thread::sleep,
        )?;
        socket.set_read_timeout(Some(Duration::from_secs(20)))?;
        socket.set_write_timeout(Some(Duration::from_secs(20)))?;
        let mut session = Session::new()?;
        session.set_timeout(20_000);
        session.set_tcp_stream(socket);
        session.handshake().context("SSH 握手失败")?;
        let (key, _) = session.host_key().context("服务器未提供密钥")?;
        let fingerprint = format!(
            "SHA256:{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(key))
        );
        let mut hosts = session.known_hosts()?;
        let read_user_hosts = {
            #[cfg(test)]
            {
                self.storage.is_none()
            }
            #[cfg(not(test))]
            {
                true
            }
        };
        if let Some(home) = dirs::home_dir().filter(|_| read_user_hosts) {
            let path = home.join(".ssh/known_hosts");
            if path.exists() {
                hosts
                    .read_file(&path, KnownHostFileKind::OpenSSH)
                    .context("无法读取 SSH known_hosts")?;
            }
        }
        let path = self.trust_path()?;
        if path.exists() {
            hosts.read_file(&path, KnownHostFileKind::OpenSSH)?;
        }
        let needs_trust = match hosts.check_port(host, port, key) {
            CheckResult::Match => false,
            CheckResult::NotFound => true,
            _ => bail!(
                "服务器密钥与已保存记录不一致，已停止连接。请向管理员核对服务器指纹：{fingerprint}"
            ),
        };
        Ok((session, fingerprint, needs_trust))
    }

    fn probe(&self, args: &Value) -> Result<Value> {
        let host = text(args, "host")?.trim();
        if host.is_empty()
            || host.len() > 253
            || host.chars().any(char::is_whitespace)
            || host.contains(['/', '\\', '@'])
        {
            bail!("请输入主机名或 IP 地址，端口单独填写");
        }
        let port = args["port"]
            .as_u64()
            .filter(|&p| p > 0 && p <= 65535)
            .context("端口应在 1–65535 之间")? as u16;
        {
            let mut map = self.connections.lock().unwrap();
            map.retain(|_, c| {
                c.authenticated.load(Ordering::Relaxed)
                    || c.created.elapsed() < Duration::from_secs(120)
            });
            if map.len() >= 8 {
                bail!("连接数已达上限，请先断开其他连接");
            }
        }
        let (session, fingerprint, needs_trust) = self.open(host, port)?;
        let serial = self.sequence.fetch_add(1, Ordering::Relaxed);
        let id = digest(
            format!(
                "{}-{serial}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
            )
            .as_bytes(),
        );
        let cache = self.cache_root()?.join(&id);
        self.connections.lock().unwrap().insert(
            id.clone(),
            Arc::new(Connection {
                preview_helper: Mutex::new(None),
                login: Mutex::new(None),
                session: Mutex::new(session),
                host: host.into(),
                port,
                fingerprint: fingerprint.clone(),
                needs_trust,
                created: Instant::now(),
                authenticated: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                done: AtomicU64::new(0),
                total: AtomicU64::new(0),
                cache,
            }),
        );
        Ok(json!({"connection": id, "fingerprint": fingerprint, "needsTrust": needs_trust}))
    }
}

#[cfg(feature = "desktop")]
impl super::NativePlugin for Ssh {
    fn id(&self) -> &'static str {
        "ssh"
    }
    fn call(&self, method: &str, args: Value, _body: &[u8]) -> Result<Value> {
        self.call(method, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_names_cannot_escape_and_preserve_image_extension() {
        let root = Path::new("cache");
        let a = cache_path(root, Path::new("/data/CON.tif")).unwrap();
        let b = cache_path(root, Path::new("/other/CON.tif")).unwrap();
        assert!(a.starts_with(root));
        assert_eq!(a.extension().unwrap(), "tif");
        assert_ne!(a, b);
        assert!(a.file_name().unwrap().to_str().unwrap().starts_with("ssh-"));
    }
    #[test]
    fn requests_require_live_connection_and_valid_host() {
        let ssh = Ssh::default();
        assert!(ssh
            .call("probe", json!({"host": "user@host", "port": 22}))
            .is_err());
        assert!(ssh
            .call("probe", json!({"host": "host", "port": 65536}))
            .is_err());
        assert!(ssh
            .call("download", json!({"connection": "missing", "path": "/x"}))
            .is_err());
    }

    // Started by scripts/test-ssh-native.py: an isolated loopback SFTP server.
    #[test]
    #[ignore = "requires an explicitly configured read-only SSH acceptance target"]
    fn configured_key_read_only_acceptance() {
        if let Some(path) = std::env::var_os("TMV_SSH_ACCEPT_PLUGIN_DIR") {
            crate::thumb::configure_plugins_version(Some(path.into()), "0.7.212");
        }
        let dir = tempfile::tempdir().unwrap();
        let ssh = Ssh {
            storage: Some(dir.path().into()),
            ..Ssh::default()
        };
        let host = std::env::var("TMV_SSH_ACCEPT_HOST").unwrap();
        let probe = ssh
            .call("probe", json!({"host": host, "port": 22}))
            .unwrap();
        assert_eq!(
            probe["fingerprint"],
            std::env::var("TMV_SSH_ACCEPT_FINGERPRINT").unwrap()
        );
        let id = &probe["connection"];
        ssh.call(
            "authenticate",
            json!({"connection": id, "trust": true, "authMethod": "keyFile",
            "username": std::env::var("TMV_SSH_ACCEPT_USER").unwrap(),
            "keyPath": std::env::var("TMV_SSH_ACCEPT_KEY").unwrap(), "passphrase": ""}),
        )
        .unwrap();
        let listing = ssh
            .call(
                "list",
                json!({"connection": id, "path": std::env::var("TMV_SSH_ACCEPT_FOLDER").unwrap()}),
            )
            .unwrap();
        let name = std::env::var("TMV_SSH_ACCEPT_FILE").unwrap();
        let file = listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap();
        if std::env::var_os("TMV_SSH_ACCEPT_PREVIEW").is_some() {
            assert!(!Path::new(file["local"].as_str().unwrap()).exists());
            let args = json!({"connection":id,"path":file["path"],"size":file["size"],"modified":file["modified"]});
            let preview = ssh.call("thumbnail", args.clone()).unwrap();
            let png = base64::engine::general_purpose::STANDARD
                .decode(
                    preview["url"]
                        .as_str()
                        .unwrap()
                        .strip_prefix("data:image/png;base64,")
                        .unwrap(),
                )
                .unwrap();
            let decoded = ::image::load_from_memory(&png).unwrap();
            assert!(decoded.width() <= 256 && decoded.height() <= 256);
            assert!(
                !Path::new(file["local"].as_str().unwrap()).exists(),
                "preview must not fetch original"
            );
            assert_eq!(
                preview,
                ssh.call("thumbnail", args).unwrap(),
                "preview cache reuse"
            );
            println!(
                "Verified remote binary preview: {} bytes, {}x{}; original not downloaded",
                png.len(),
                decoded.width(),
                decoded.height()
            );
            if std::env::var_os("TMV_SSH_ACCEPT_PREVIEW_ONLY").is_some() {
                ssh.call("disconnect", json!({"connection":id})).unwrap();
                return;
            }
        }
        let cached = ssh.call("download", json!({"connection": id, "path": file["path"], "size": file["size"], "modified": file["modified"]})).unwrap();
        let path = Path::new(cached["path"].as_str().unwrap());
        assert_eq!(
            digest(&std::fs::read(path).unwrap()),
            std::env::var("TMV_SSH_ACCEPT_SHA256").unwrap()
        );
        let source = crate::formats::open(path).unwrap();
        assert!(!source.read_page(0).unwrap().is_empty());
        println!(
            "Verified SSH key login, SFTP listing, SHA-256 and native image decoding: {}x{}",
            source.info().width,
            source.info().height
        );
        ssh.call("disconnect", json!({"connection": id})).unwrap();
    }

    // Started by scripts/test-ssh-native.py: an isolated loopback SFTP server.
    #[test]
    #[ignore = "requires the local fixture server"]
    fn live_sftp_roundtrip() {
        let port: u16 = std::env::var("TMV_SSH_TEST_PORT").unwrap().parse().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let ssh = Arc::new(Ssh {
            storage: Some(dir.path().into()),
            ..Ssh::default()
        });
        let probe = || {
            ssh.call("probe", json!({"host": "127.0.0.1", "port": port}))
                .unwrap()
        };
        let p = probe();
        assert_eq!(p["needsTrust"], true);
        let id = p["connection"].as_str().unwrap().to_owned();
        assert!(ssh
            .call(
                "authenticate",
                json!({"connection": id, "username": "test", "password": "fixture", "trust": false})
            )
            .is_err());
        assert!(ssh
            .call(
                "authenticate",
                json!({"connection": id, "username": "test", "password": "wrong", "trust": true})
            )
            .is_err());
        ssh.call(
            "authenticate",
            json!({"connection": id, "username": "test", "password": "fixture", "trust": true}),
        )
        .unwrap();
        let listing = ssh
            .call("list", json!({"connection": id, "path": "/"}))
            .unwrap();
        let entries = listing["entries"].as_array().unwrap();
        let mdoc_listing = ssh
            .call("list", json!({"connection":id,"path":"/metadata"}))
            .unwrap();
        let mdoc = mdoc_listing["entries"].as_array().unwrap().iter().find(|f|f["name"]=="series.mdoc").unwrap();
        let text = ssh.call("readText", json!({"connection":id,"path":mdoc["path"],"size":mdoc["size"],"modified":mdoc["modified"]})).unwrap();
        assert!(text["text"].as_str().unwrap().contains("SubFramePath"));
        let linked = ssh
            .call(
                "resolveMovie",
                json!({"connection":id,"path":mdoc["path"],"movie":"Z:\\old\\fraction.eer"}),
            )
            .unwrap();
        assert_eq!(linked["path"], "/movies-renamed/fraction.eer");
        assert!(ssh.call("resolveMovie",json!({"connection":id,"path":mdoc["path"],"movie":"ambiguous.eer"})).unwrap_err().to_string().contains("多个"));
        let long = mdoc_listing["entries"].as_array().unwrap().iter().find(|f|f["name"]=="long.txt").unwrap();
        let text = ssh.call("readText",json!({"connection":id,"path":long["path"],"size":long["size"],"modified":long["modified"]})).unwrap();
        assert_eq!(text["text"].as_str().unwrap().len(),256 * 1024);
        assert_eq!(text["truncated"],true);
        assert!(entries
            .iter()
            .any(|e| e["name"] == "中文 folder" && e["directory"] == true));
        assert!(entries
            .iter()
            .any(|e| e["name"] == "folder link" && e["directory"] == true));
        assert!(entries
            .iter()
            .any(|e| e["name"] == "notes.txt" && e["supported"] == false));
        let entry = entries
            .iter()
            .find(|e| e["name"] == "file link.mrc")
            .unwrap();
        let request = json!({"connection": id, "path": entry["path"], "size": entry["size"], "modified": entry["modified"]});
        let result = ssh.call("download", request.clone()).unwrap();
        assert_eq!(result["path"], entry["local"]);
        assert_eq!(
            std::fs::read(result["path"].as_str().unwrap()).unwrap(),
            b"SFTP binary fixture\0\xff"
        );
        assert_eq!(ssh.call("download", request.clone()).unwrap(), result);
        let valid = entries.iter().find(|e| e["name"] == "valid.mrc").unwrap();
        let cached = ssh.call("download", json!({"connection": id, "path": valid["path"], "size": valid["size"], "modified": valid["modified"]})).unwrap();
        let source = crate::formats::open(Path::new(cached["path"].as_str().unwrap())).unwrap();
        assert_eq!(
            (
                source.info().width,
                source.info().height,
                source.info().size_z
            ),
            (2, 2, 1)
        );
        let planes = source.read_page(0).unwrap();
        match &planes[0].data {
            crate::image::PlaneData::F32(values) => assert_eq!(values, &[1.0, 2.0, 3.0, 4.0]),
            _ => panic!("MRC fixture decoded with unexpected pixel type"),
        }
        drop(source);
        let mut stale = request;
        stale["size"] = json!(999);
        assert!(ssh
            .call("download", stale)
            .unwrap_err()
            .to_string()
            .contains("变化"));
        assert!(ssh
            .call("list", json!({"connection": id, "path": "/missing"}))
            .is_err());
        // A slow multi-MB file makes cancellation observable and deterministic.
        let big = entries.iter().find(|e| e["name"] == "slow.mrc").unwrap();
        let req = json!({"connection": id, "path": big["path"], "size": big["size"], "modified": big["modified"]});
        let worker = ssh.clone();
        let transfer = std::thread::spawn(move || worker.call("download", req));
        let start = Instant::now();
        while ssh.call("progress", json!({"connection": id})).unwrap()["done"]
            .as_u64()
            .unwrap()
            < 32768
        {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(10));
        }
        ssh.call("cancel", json!({"connection": id})).unwrap();
        assert!(transfer
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("取消"));
        assert!(!Path::new(big["local"].as_str().unwrap()).exists());
        assert!(!Path::new(big["local"].as_str().unwrap())
            .with_extension("ssh-part")
            .exists());
        ssh.call("disconnect", json!({"connection": id})).unwrap();
        assert!(ssh
            .call("list", json!({"connection": id, "path": "/"}))
            .is_err());
        let known = probe();
        assert_eq!(known["needsTrust"], false);
        ssh.call("disconnect", json!({"connection": known["connection"]}))
            .unwrap();
        for (variable, encrypted) in [
            ("TMV_SSH_TEST_KEY", false),
            ("TMV_SSH_TEST_ENCRYPTED_KEY", true),
        ] {
            let key = probe();
            let request = json!({"connection": key["connection"], "username": "test", "authMethod": "keyFile",
                "keyPath": std::env::var(variable).unwrap(), "passphrase": if encrypted { "fixture-key-passphrase" } else { "" }, "trust": true});
            if encrypted {
                let mut wrong = request.clone();
                wrong["passphrase"] = json!("wrong");
                assert!(ssh.call("authenticate", wrong).is_err());
            }
            ssh.call("authenticate", request).unwrap();
            ssh.call(
                "list",
                json!({"connection": key["connection"], "path": "/中文 folder"}),
            )
            .unwrap();
            ssh.call("disconnect", json!({"connection": key["connection"]}))
                .unwrap();
        }
        // Substitute an unrelated valid host key: mismatch must fail closed.
        let trust = ssh.trust_path().unwrap();
        let stored = std::fs::read_to_string(&trust).unwrap();
        let bad_key = std::env::var("TMV_SSH_WRONG_HOST_KEY").unwrap();
        let mut parts: Vec<_> = stored.trim().split(' ').map(str::to_owned).collect();
        parts[2] = bad_key;
        std::fs::write(trust, parts.join(" ") + "\n").unwrap();
        assert!(ssh
            .call("probe", json!({"host": "127.0.0.1", "port": port}))
            .unwrap_err()
            .to_string()
            .contains("不一致"));
    }
}
