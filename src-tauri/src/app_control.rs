//! Lets a coding agent (Codex, Claude Code…) drive the running Pro app, so
//! the user watches it happen in the window.
//!
//! The app listens on 127.0.0.1 only, on a port chosen by the system, and
//! writes the port and a random token to `agent.json` in its config folder
//! (readable by this user only). `tmv app …` and `tmv mcp` read that file and
//! post `{action, args}` here; the call is handed to the window, whose page
//! runs it with the same code its buttons use (ui/pro/agent.js) and answers
//! with `agent_reply`. Nothing listens on other interfaces; a request must
//! carry the token, name 127.0.0.1/localhost as its host (no DNS rebinding)
//! and come from no web page (no Origin header). The page also asks the user
//! once before the first call is run.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

/// The app's bundle identifier: its config folder carries this name.
const IDENTIFIER: &str = "com.leehow.tifmrcview.pro";

/// Where the running app says how to reach it (TMV_APP_CONTROL overrides).
pub fn discovery_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TMV_APP_CONTROL") {
        return Some(PathBuf::from(p));
    }
    Some(dirs::config_dir()?.join(IDENTIFIER).join("agent.json"))
}

/// What `agent.json` holds.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    #[serde(default)]
    pub version: String,
}

pub fn read_endpoint() -> Option<Endpoint> {
    let text = std::fs::read_to_string(discovery_path()?).ok()?;
    serde_json::from_str(&text).ok()
}

// ---------- base64 (PNG pictures in answers) ----------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64_decode(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' => continue,
            _ => bail!("invalid base64"),
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

// ---------- client (tmv app, tmv mcp) ----------

/// Calls `action` in the running app; the answer is `{ok, …}` as the page gave it.
pub fn call(action: &str, args: &Value, timeout: Duration) -> Result<Value> {
    let ep = read_endpoint().ok_or_else(|| anyhow!(NOT_RUNNING))?;
    let body = serde_json::to_vec(&json!({ "action": action, "args": args, "timeoutMs": timeout.as_millis() as u64 }))?;
    post(&ep, "/v1/call", &body, timeout + Duration::from_secs(10))
}

pub const NOT_RUNNING: &str = "TifMrcView Pro 没有在运行（或版本太旧，不支持 AI 助手）。请先打开 TifMrcView Pro";

/// True when the app answers.
pub fn running() -> Option<Endpoint> {
    let ep = read_endpoint()?;
    post(&ep, "/v1/ping", b"{}", Duration::from_secs(3)).ok().map(|_| ep)
}

fn post(ep: &Endpoint, path: &str, body: &[u8], timeout: Duration) -> Result<Value> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], ep.port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|_| anyhow!(NOT_RUNNING))?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        ep.port,
        ep.token,
        body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    let mut r = BufReader::new(s);
    let mut status = String::new();
    r.read_line(&mut status).context("TifMrcView Pro 没有回应")?;
    let code: u16 = status.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse::<usize>().ok();
            }
        }
    }
    let mut buf = Vec::new();
    match len {
        Some(n) => {
            buf.resize(n, 0);
            r.read_exact(&mut buf)?;
        }
        None => {
            r.read_to_end(&mut buf)?;
        }
    }
    let v: Value = serde_json::from_slice(&buf).with_context(|| format!("TifMrcView Pro 的回答不是 JSON（HTTP {code}）"))?;
    if code == 401 || code == 403 {
        bail!("{}", v.get("error").and_then(Value::as_str).unwrap_or("拒绝访问"));
    }
    Ok(v)
}

/// Starts the app when it is not running and waits until it answers.
pub fn ensure_running(wait: Duration) -> Result<Endpoint> {
    if let Some(ep) = running() {
        return Ok(ep);
    }
    launch()?;
    let start = std::time::Instant::now();
    while start.elapsed() < wait {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(ep) = running() {
            return Ok(ep);
        }
    }
    bail!("已尝试打开 TifMrcView Pro，但 {} 秒内没有连上；请手动打开它再试", wait.as_secs())
}

fn launch() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let ok = std::process::Command::new("open").args(["-g", "-b", IDENTIFIER]).status().map(|s| s.success()).unwrap_or(false);
        if ok {
            return Ok(());
        }
    }
    // `tmv` run from inside the app's own binary (`tifmrcview-pro tmv …`, or a
    // `tmv` link to it): start that binary as the app.
    let exe = std::env::current_exe()?;
    let name = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let real = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    if name.starts_with("tifmrcview") || real.starts_with("tifmrcview") {
        std::process::Command::new(exe)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        return Ok(());
    }
    bail!(NOT_RUNNING)
}


// ---------- tmv app / tmv mcp ----------

pub const APP_HELP: &str = r#"tmv app — drive the open TifMrcView Pro window (the user sees every step)

USAGE
  tmv app                         List the actions the app offers (JSON)
  tmv app <action> [key=value …] [--json '{…}'] [--timeout S] [--no-launch]

  Values are read as JSON when they parse (z=12, on=true, pts=[[1,2],[3,4]]),
  else as text (path=/data/ts01.mrc, text=找候选). The app is started when it
  is not running (--no-launch: fail instead). The first call asks the user in
  the window to allow the agent. The answer is one JSON object on stdout;
  exit status 0 when "ok" is true, else 1.

  Typical: tmv app status · tmv app open path=… · tmv app ui · tmv app click text=…
           tmv app set ref=… value=… · tmv app wait · tmv app screenshot
"#;

fn parse_value(v: &str) -> Value {
    serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_string()))
}

/// `tmv app …`; returns the exit status.
pub fn cli_app(args: &[String]) -> i32 {
    if args.first().is_some_and(|a| matches!(a.as_str(), "-h" | "--help")) {
        print!("{APP_HELP}");
        return 0;
    }
    let mut action = None;
    let mut map = serde_json::Map::new();
    let mut timeout = 120u64;
    let mut launch_app = true;
    let mut it = args.iter();
    let fail = |msg: String| {
        println!("{}", json!({"ok": false, "error": msg}));
        1
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => match it.next().map(|j| serde_json::from_str::<Value>(j)) {
                Some(Ok(Value::Object(o))) => map.extend(o),
                _ => return fail("--json 后面要跟一个 JSON 对象".into()),
            },
            "--timeout" => match it.next().and_then(|t| t.parse::<f64>().ok()) {
                Some(t) if t > 0.0 => timeout = t.ceil() as u64,
                _ => return fail("--timeout 后面要跟秒数".into()),
            },
            "--no-launch" => launch_app = false,
            _ if action.is_none() && !a.contains('=') => action = Some(a.clone()),
            _ => match a.split_once('=') {
                Some((k, v)) => {
                    map.insert(k.to_string(), parse_value(v));
                }
                None => return fail(format!("看不懂参数 `{a}`（写成 key=value）")),
            },
        }
    }
    let action = action.unwrap_or_else(|| "help".into());
    let ready = if launch_app { ensure_running(Duration::from_secs(60)).map(|_| ()) } else { running().map(|_| ()).ok_or_else(|| anyhow!(NOT_RUNNING)) };
    if let Err(e) = ready {
        return fail(e.to_string());
    }
    match call(&action, &Value::Object(map), Duration::from_secs(timeout)) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            if v.get("ok").and_then(Value::as_bool) == Some(true) { 0 } else { 1 }
        }
        Err(e) => fail(e.to_string()),
    }
}

const MCP_INSTRUCTIONS: &str = "TifMrcView Pro (cryo-EM / microscopy viewer and processing) is open on the user's screen; these tools drive that window and the user watches every step. Start with app_actions (what the app offers) and app_call action=status. Work like a careful user: open the data, look (app_screenshot, action=ui lists the buttons and fields that are visible), then click/set the same controls a person would. Long jobs (reconstruction, STA, SPA) run in the window: use action=wait to follow them. Never invent processing parameters; the app chooses them. What to average or pick is the user's choice: show them the candidates and ask.";

fn mcp_tools() -> Value {
    json!([
        {"name": "app_actions", "description": "List the actions the open TifMrcView Pro window offers, with their arguments. Starts the app if needed.", "inputSchema": {"type": "object", "properties": {}}},
        {"name": "app_call", "description": "Run one action in the TifMrcView Pro window (status, open, tabs, ui, click, set, wait, labels, screenshot, …; see app_actions). The user sees it happen.", "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string", "description": "Action name from app_actions"},
            "args": {"type": "object", "description": "Arguments of the action"},
            "timeoutSeconds": {"type": "number", "description": "How long to wait for the answer (default 120)"}
        }, "required": ["action"]}},
        {"name": "app_screenshot", "description": "A picture of the image view the user sees in TifMrcView Pro.", "inputSchema": {"type": "object", "properties": {}}}
    ])
}

fn png_paths(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s.ends_with(".png") && s.contains("tifmrcview-agent") => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| png_paths(x, out)),
        Value::Object(o) => o.values().for_each(|x| png_paths(x, out)),
        _ => {}
    }
}

fn mcp_tool(name: &str, args: &Value) -> Value {
    let (action, a, timeout) = match name {
        "app_actions" => ("help".to_string(), json!({}), 60),
        "app_screenshot" => ("screenshot".to_string(), json!({}), 60),
        "app_call" => (
            args.get("action").and_then(Value::as_str).unwrap_or("").to_string(),
            args.get("args").cloned().unwrap_or(json!({})),
            args.get("timeoutSeconds").and_then(Value::as_f64).map(|t| t.max(1.0) as u64).unwrap_or(120),
        ),
        _ => return json!({"content": [{"type": "text", "text": format!("unknown tool {name}")}], "isError": true}),
    };
    let result = ensure_running(Duration::from_secs(60)).and_then(|_| call(&action, &a, Duration::from_secs(timeout)));
    match result {
        Ok(v) => {
            let mut content = vec![json!({"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()})];
            let mut pngs = Vec::new();
            png_paths(&v, &mut pngs);
            for p in pngs.into_iter().take(3) {
                if let Ok(bytes) = std::fs::read(&p) {
                    content.push(json!({"type": "image", "data": b64_encode(&bytes), "mimeType": "image/png"}));
                }
            }
            json!({"content": content, "isError": v.get("ok").and_then(Value::as_bool) != Some(true)})
        }
        Err(e) => json!({"content": [{"type": "text", "text": e.to_string()}], "isError": true}),
    }
}

/// `tmv mcp`: a Model Context Protocol server on stdin/stdout for agents
/// that prefer tools to a shell (claude mcp add / codex mcp add).
pub fn cli_mcp(version: &str) -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            let _ = writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}}));
            continue;
        };
        let Some(id) = msg.get("id").cloned() else { continue }; // a notification
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "tifmrcview", "version": version},
                "instructions": MCP_INSTRUCTIONS,
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": mcp_tools()})),
            "tools/call" => Ok(mcp_tool(params.get("name").and_then(Value::as_str).unwrap_or(""), params.get("arguments").unwrap_or(&json!({})))),
            _ => Err(json!({"code": -32601, "message": format!("method not found: {method}")})),
        };
        let reply = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
        };
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
    0
}

// ---------- the `tmv` command of the desktop app ----------

/// Where the app puts its `tmv` command: beside the one the server installer
/// uses (~/.local/bin), so agents find the same name everywhere.
pub fn tmv_link() -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    Some(dirs::home_dir()?.join(".local").join("bin").join("tmv"))
}

/// Links `tmv` to this app's binary (the binary runs as `tmv` under that
/// name). A real `tmv` already there (the server package) is left alone.
pub fn install_tmv_link() -> Result<PathBuf> {
    let link = tmv_link().ok_or_else(|| anyhow!("Windows 上请直接用程序的完整路径加 tmv（见对话框里的命令）"))?;
    let exe = std::env::current_exe()?;
    if let Ok(meta) = std::fs::symlink_metadata(&link) {
        if !meta.file_type().is_symlink() {
            bail!("{} 已经是另一个 tmv（服务器版），没有替换它；可以直接用对话框里的完整路径", link.display());
        }
        std::fs::remove_file(&link)?;
    }
    std::fs::create_dir_all(link.parent().unwrap())?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&exe, &link)?;
    Ok(link)
}

// ---------- server (in the app) ----------

#[cfg(feature = "desktop")]
pub mod server {
    use super::*;
    use std::collections::HashMap;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use tauri::{AppHandle, Emitter, Manager};

    /// Calls handed to a window and not answered yet.
    #[derive(Default)]
    pub struct Pending {
        next: AtomicU64,
        waiting: Mutex<HashMap<u64, mpsc::Sender<Value>>>,
    }

    impl Pending {
        pub fn answer(&self, id: u64, reply: Value) -> bool {
            match self.waiting.lock().unwrap().remove(&id) {
                Some(tx) => tx.send(reply).is_ok(),
                None => false,
            }
        }
    }

    fn token() -> String {
        let mut b = [0u8; 24];
        if getrandom::fill(&mut b).is_err() {
            // No OS randomness (never seen): a weaker but unguessable-enough mix.
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            for (i, x) in b.iter_mut().enumerate() {
                *x = (t >> ((i % 16) * 8)) as u8 ^ (std::process::id() >> (i % 4 * 8)) as u8;
            }
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(text.as_bytes())?;
        drop(f);
        std::fs::rename(&tmp, path)
    }

    /// Starts listening (once per app); errors only print, the app works without it.
    pub fn start(app: AppHandle, pending: Arc<Pending>) {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(l) => l,
            Err(e) => return eprintln!("agent control: cannot listen: {e}"),
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let ep = Endpoint { port, token: token(), pid: std::process::id(), version: app.package_info().version.to_string() };
        match discovery_path() {
            Some(p) => {
                if let Err(e) = write_private(&p, &serde_json::to_string_pretty(&ep).unwrap()) {
                    eprintln!("agent control: cannot write {}: {e}", p.display());
                }
            }
            None => eprintln!("agent control: no config folder"),
        }
        let ep = Arc::new(ep);
        std::thread::Builder::new()
            .name("agent-control".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { continue };
                    let (app, pending, ep) = (app.clone(), pending.clone(), ep.clone());
                    std::thread::spawn(move || {
                        let _ = handle(conn, &app, &pending, &ep);
                    });
                }
            })
            .ok();
    }

    /// Removes `agent.json` when it is still ours (on quit).
    pub fn stop() {
        if let (Some(p), Some(ep)) = (discovery_path(), read_endpoint()) {
            if ep.pid == std::process::id() {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    struct Request {
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    fn read_request(s: &mut TcpStream) -> Result<Request> {
        let mut r = BufReader::new(s.try_clone()?);
        let mut line = String::new();
        r.read_line(&mut line)?;
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        if method != "POST" {
            bail!("method");
        }
        let mut headers = HashMap::new();
        let mut total = 0;
        loop {
            let mut h = String::new();
            let n = r.read_line(&mut h)?;
            total += n;
            if n == 0 || total > 16 * 1024 {
                bail!("headers");
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        if len > 4 << 20 {
            bail!("body");
        }
        let mut body = vec![0; len];
        r.read_exact(&mut body)?;
        Ok(Request { path, headers, body })
    }

    fn respond(s: &mut TcpStream, code: u16, v: &Value) -> std::io::Result<()> {
        let body = serde_json::to_vec(v).unwrap_or_default();
        let reason = match code {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            _ => "Error",
        };
        write!(s, "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
        s.write_all(&body)
    }

    /// Constant-time comparison of the token.
    fn same(a: &str, b: &str) -> bool {
        a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }

    /// Why a request may not be served, if it may not.
    pub(super) fn refuse(headers: &HashMap<String, String>, port: u16, token: &str) -> Option<(u16, &'static str)> {
        // Web pages always send Origin with a POST (and Sec-Fetch-*): never theirs.
        if headers.contains_key("origin") || headers.keys().any(|k| k.starts_with("sec-fetch-")) {
            return Some((403, "不接受来自网页的请求"));
        }
        let host = headers.get("host").map(String::as_str).unwrap_or("");
        if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
            return Some((403, "只接受本机的请求"));
        }
        let auth = headers.get("authorization").and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("");
        if !same(auth, token) {
            return Some((401, "令牌不对：请用 tmv app 连接（它会读取 TifMrcView Pro 写下的令牌）"));
        }
        None
    }

    fn handle(mut s: TcpStream, app: &AppHandle, pending: &Pending, ep: &Endpoint) -> Result<()> {
        if !s.peer_addr()?.ip().is_loopback() {
            return Ok(());
        }
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        let req = match read_request(&mut s) {
            Ok(r) => r,
            Err(_) => return Ok(respond(&mut s, 400, &json!({"ok": false, "error": "bad request"}))?),
        };
        if let Some((code, msg)) = refuse(&req.headers, ep.port, &ep.token) {
            return Ok(respond(&mut s, code, &json!({"ok": false, "error": msg}))?);
        }
        let reply = match req.path.as_str() {
            "/v1/ping" => json!({"ok": true, "app": app.package_info().name, "version": ep.version, "pid": ep.pid}),
            "/v1/call" => match serde_json::from_slice::<Value>(&req.body) {
                Ok(v) => dispatch(app, pending, v),
                Err(e) => json!({"ok": false, "error": format!("请求不是 JSON：{e}")}),
            },
            _ => return Ok(respond(&mut s, 404, &json!({"ok": false, "error": "not found"}))?),
        };
        respond(&mut s, 200, &reply)?;
        Ok(())
    }

    /// The window an agent works in: the one in front, else the main one.
    fn window(app: &AppHandle, want: Option<&str>) -> Option<tauri::WebviewWindow> {
        let windows = app.webview_windows();
        if let Some(w) = want.and_then(|l| windows.get(l)) {
            return Some(w.clone());
        }
        windows
            .values()
            .find(|w| w.is_focused().unwrap_or(false))
            .or_else(|| windows.get("main"))
            .or_else(|| windows.values().min_by_key(|w| w.label().to_string()))
            .cloned()
    }

    fn dispatch(app: &AppHandle, pending: &Pending, v: Value) -> Value {
        let Some(action) = v.get("action").and_then(Value::as_str).map(str::to_string) else {
            return json!({"ok": false, "error": "缺少 action"});
        };
        let args = v.get("args").cloned().unwrap_or(json!({}));
        let timeout = v.get("timeoutMs").and_then(Value::as_u64).unwrap_or(60_000).clamp(1_000, 30 * 60_000);
        if action == "windows" {
            let list: Vec<Value> = app
                .webview_windows()
                .values()
                .map(|w| json!({"label": w.label(), "title": w.title().unwrap_or_default(), "focused": w.is_focused().unwrap_or(false)}))
                .collect();
            return json!({"ok": true, "windows": list});
        }
        let Some(w) = window(app, args.get("window").and_then(Value::as_str)) else {
            return json!({"ok": false, "error": "TifMrcView Pro 没有打开的窗口"});
        };
        let id = pending.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::channel();
        pending.waiting.lock().unwrap().insert(id, tx);
        if let Err(e) = app.emit_to(w.label(), "agent-call", json!({"id": id, "action": action, "args": args, "timeoutMs": timeout})) {
            pending.waiting.lock().unwrap().remove(&id);
            return json!({"ok": false, "error": format!("无法交给窗口：{e}")});
        }
        let reply = rx.recv_timeout(Duration::from_millis(timeout));
        pending.waiting.lock().unwrap().remove(&id);
        match reply {
            Ok(mut r) => {
                save_pictures(&mut r);
                r
            }
            Err(_) => json!({"ok": false, "error": format!("{action} 在 {} 秒内没有完成（界面还在做的话可以用 wait 等它）", timeout / 1000), "timeout": true}),
        }
    }

    /// A picture comes back from the page as a data URL; the agent gets a PNG file instead.
    fn save_pictures(v: &mut Value) {
        match v {
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if let Some(png) = x.as_str().and_then(|s| s.strip_prefix("data:image/png;base64,")) {
                        if let Ok(bytes) = b64_decode(png) {
                            let dir = std::env::temp_dir().join("tifmrcview-agent");
                            let _ = std::fs::create_dir_all(&dir);
                            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
                            let p = dir.join(format!("{k}-{stamp}.png"));
                            if std::fs::write(&p, bytes).is_ok() {
                                *x = json!(p.to_string_lossy());
                            }
                        }
                    } else {
                        save_pictures(x);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(save_pictures),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        for n in 0..10 {
            let data: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(b64_decode(&b64_encode(&data)).unwrap(), data);
        }
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
    }

    #[test]
    fn app_arguments_read_json_or_text() {
        assert_eq!(parse_value("12"), json!(12));
        assert_eq!(parse_value("[[1,2]]"), json!([[1, 2]]));
        assert_eq!(parse_value("/data/ts01.mrc"), json!("/data/ts01.mrc"));
        assert_eq!(parse_value("找候选"), json!("找候选"));
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn only_local_requests_with_the_token_are_served() {
        use std::collections::HashMap;
        let h = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
        let ok = h(&[("host", "127.0.0.1:4000"), ("authorization", "Bearer abc")]);
        assert!(server::refuse(&ok, 4000, "abc").is_none());
        assert!(server::refuse(&h(&[("host", "localhost:4000"), ("authorization", "Bearer abc")]), 4000, "abc").is_none());
        assert_eq!(server::refuse(&h(&[("host", "127.0.0.1:4000"), ("authorization", "Bearer abd")]), 4000, "abc").unwrap().0, 401);
        assert_eq!(server::refuse(&h(&[("host", "evil.example:4000"), ("authorization", "Bearer abc")]), 4000, "abc").unwrap().0, 403);
        assert_eq!(server::refuse(&h(&[("host", "127.0.0.1:4000"), ("authorization", "Bearer abc"), ("origin", "https://x")]), 4000, "abc").unwrap().0, 403);
        assert_eq!(server::refuse(&h(&[("host", "127.0.0.1:4000"), ("authorization", "Bearer abc"), ("sec-fetch-mode", "cors")]), 4000, "abc").unwrap().0, 403);
    }
}
