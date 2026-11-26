//! Versioned, bounded SSH job protocol. Workers outlive their control connection.
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

const LIMIT: u64 = 8 * 1024 * 1024;
fn hash(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn root() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("home unavailable")?
        .join(".cache/tifmrcview/processing"))
}
fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .filter(|s| !s.contains('\0'))
        .with_context(|| format!("missing {k}"))
}
fn write(p: &Path, v: &Value) -> Result<()> {
    let temp = p.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec(v)?;
    ensure!(
        bytes.len() as u64 <= LIMIT,
        "响应超过大小限制，请缩小处理范围"
    );
    fs::write(&temp, bytes)?;
    fs::rename(temp, p)?;
    Ok(())
}
fn read(p: &Path) -> Result<Value> {
    ensure!(fs::metadata(p)?.len() <= LIMIT, "result exceeds limit");
    Ok(serde_json::from_slice(&fs::read(p)?)?)
}
fn job_path(id: &str) -> Result<PathBuf> {
    ensure!(
        id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid job id"
    );
    Ok(root()?.join("jobs").join(id))
}
fn long(cmd: &str) -> bool {
    matches!(
        cmd,
        "tomo_session_run" | "spa_run" | "tomo_dataset_save" | "tomo_take" | "tomo_pick"
    )
}

pub fn main(args: &[String]) -> i32 {
    #[cfg(unix)]
    unsafe {
        libc::umask(0o077);
    }
    if args.first().is_some_and(|s|s=="agent-worker"){crate::resources::pin_primary();crate::resources::configure_threads();}
    crate::cli::use_bundled_cuda();
    // Explicit backend avoids falling back to a second GPU through Vulkan.
    #[cfg(target_os = "linux")]
    // /dev/dxg: an NVIDIA card under WSL, which has no /dev/nvidiactl.
    if (Path::new("/dev/nvidiactl").exists() || Path::new("/dev/dxg").exists()) && std::env::var_os("TMV_GPU").is_none() {
        std::env::set_var("TMV_GPU", "cuda");
    }
    let out = (|| -> Result<Value> {
        if args[0] == "agent-worker" {
            return worker(args.get(1).context("missing job")?);
        }
        let mut b = Vec::new();
        std::io::stdin().take(LIMIT + 1).read_to_end(&mut b)?;
        ensure!(b.len() as u64 <= LIMIT, "request exceeds limit");
        let v: Value = serde_json::from_slice(&b)?;
        ensure!(v["protocol"] == 1, "unsupported protocol");
        match string(&v, "action")? {
            "inspect" => Ok(
                json!({"protocol":1,"version":crate::cli::VERSION,"platform":format!("{} {}",std::env::consts::OS,std::env::consts::ARCH),"root":root()?,"gpu":null,"gpuProbe":"deferredToTask","resources":crate::resources::inspect(v["outputRoot"].as_str().map(Path::new).or(Some(root()?.as_path())))}),
            ),
            "submit" => submit(&v),
            "jobs" => jobs(v["project"].as_str()),
            "status" | "cancel" => {
                let dir = job_path(string(&v, "job")?)?;
                if v["action"] == "cancel" {
                    fs::write(dir.join("cancel"), [])?;
                }
                let mut state = read(&dir.join("state.json"))?;
                if !matches!(
                    state["state"].as_str(),
                    Some("completed" | "failed" | "cancelled" | "needsChoice")
                ) {
                    if let Some(_pid) = state["pid"].as_u64() {
                        #[cfg(target_os = "linux")]
                        if !Path::new(&format!("/proc/{_pid}")).exists() {
                            state["state"] = json!("failed");
                            state["error"] =
                                json!("工作进程已退出；请检查日志，已完成步骤可在下次复用");
                        }
                    }
                }
                let after = v["after"].as_u64().unwrap_or(0);
                let events = fs::File::open(dir.join("events.jsonl"))
                    .ok()
                    .map(|f| {
                        use std::io::BufRead;
                        std::io::BufReader::new(f)
                            .lines()
                            .map_while(Result::ok)
                            .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
                            .filter(|e| e["seq"].as_u64().unwrap_or(0) > after)
                            .take(128)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                state["events"] = json!(events);
                state["job"] = v["job"].clone();
                if state["state"] == "completed" || state["state"] == "needsChoice" {
                    state["result"] = read(&dir.join("result.json"))?;
                }
                Ok(state)
            }
            _ => bail!("unknown action"),
        }
    })();
    match out {
        Ok(v) => {
            println!("{v}");
            0
        }
        Err(e) => {
            if args.first().is_some_and(|s| s == "agent-worker") {
                if let Some(id) = args.get(1) {
                    if let Ok(dir) = job_path(id) {
                        let _ = write(
                            &dir.join("state.json"),
                            &json!({"state":"failed","error":format!("{e:#}")}),
                        );
                    }
                }
            }
            println!("{}", json!({"error":format!("{e:#}")}));
            1
        }
    }
}
/// Long jobs on this server, newest first, so a client that was closed (or
/// another computer) finds them again: running ones to follow, recently
/// finished ones to report. `project` narrows to one folder and its subfolders.
fn jobs(project: Option<&str>) -> Result<Value> {
    let week = std::time::Duration::from_secs(7 * 24 * 3600);
    let mut out = vec![];
    let Ok(entries) = fs::read_dir(root()?.join("jobs")) else {
        return Ok(json!({"jobs":[]}));
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(id) = dir.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        if job_path(&id).is_err() {
            continue;
        }
        let Ok(request) = read(&dir.join("request.json")) else { continue };
        let command = request["command"].as_str().unwrap_or("");
        if !matches!(command, "tomo_session_run" | "spa_run") {
            continue;
        }
        let job_project = request["project"].as_str().unwrap_or("");
        if let Some(p) = project {
            let p = p.trim_end_matches('/');
            if job_project != p && !job_project.starts_with(&format!("{p}/")) {
                continue;
            }
        }
        let Ok(mut state) = read(&dir.join("state.json")) else { continue };
        let modified = ["events.jsonl", "state.json"]
            .iter()
            .filter_map(|n| fs::metadata(dir.join(n)).and_then(|m| m.modified()).ok())
            .max();
        let age = modified.and_then(|m| m.elapsed().ok()).unwrap_or_default();
        let mut name = state["state"].as_str().unwrap_or("").to_owned();
        if matches!(name.as_str(), "queued" | "running") {
            // Same liveness rule as `status`: a vanished worker is a failed job.
            #[cfg(target_os = "linux")]
            if let Some(pid) = state["pid"].as_u64() {
                if !Path::new(&format!("/proc/{pid}")).exists() {
                    name = "failed".into();
                    state["error"] = json!("工作进程已退出；请检查日志，已完成步骤可在下次复用");
                }
            }
        } else if age > week {
            continue;
        }
        // The last progress line, read from the end of a possibly long log.
        let mut last = Value::Null;
        if let Ok(mut f) = fs::File::open(dir.join("events.jsonl")) {
            use std::io::{Seek, SeekFrom};
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let _ = f.seek(SeekFrom::Start(len.saturating_sub(64 * 1024)));
            let mut tail = vec![];
            let _ = f.read_to_end(&mut tail);
            last = String::from_utf8_lossy(&tail)
                .lines()
                .rev()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .find(|e| e["type"].is_null())
                .unwrap_or(Value::Null);
        }
        let since = modified
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        out.push(json!({
            "job": id,
            "requestId": request["requestId"],
            "command": command,
            "args": request["args"],
            "project": job_project,
            "outputRoot": request["outputRoot"],
            "state": name,
            "error": state["error"],
            "progress": last,
            "updated": since,
        }));
    }
    out.sort_by_key(|j| std::cmp::Reverse(j["updated"].as_u64().unwrap_or(0)));
    out.truncate(20);
    Ok(json!({ "jobs": out }))
}
fn submit(v: &Value) -> Result<Value> {
    let cmd = string(v, "command")?;
    ensure!(supported(cmd), "unsupported operation {cmd}");
    let request_id = string(v, "requestId")?;
    ensure!(
        request_id.len() <= 128 && request_id.len() >= 8,
        "invalid request id"
    );
    let id = hash(request_id.as_bytes());
    let dir = job_path(&id)?;
    fs::create_dir_all(dir.parent().unwrap())?;
    match fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // An id identifies exactly one request, never silently rebind it.
            for _ in 0..20 {
                if dir.join("request.json").exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            let previous = read(&dir.join("request.json"))?;
            ensure!(
                ["command", "args", "project", "outputRoot"]
                    .iter()
                    .all(|k| previous[*k] == v[*k]),
                "request id already belongs to different parameters"
            );
            // A disconnect can kill the control process after persisting the request
            // but before spawning a worker. Re-spawn only this pre-execution state;
            // the worker lock makes concurrent retries harmless.
            if read(&dir.join("state.json"))
                .is_ok_and(|s| s["state"] == "queued" && s["pid"].is_null())
            {
                launch(&id, &dir)?;
            }
            return Ok(json!({"job":id,"reused":true}));
        }
        Err(e) => return Err(e.into()),
    }
    write(&dir.join("request.json"), v)?;
    write(&dir.join("state.json"), &json!({"state":"queued"}))?;
    launch(&id, &dir)?;
    Ok(json!({"job":id,"reused":false}))
}
fn launch(id: &str, dir: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("stderr.log"))?;
    let mut c = std::process::Command::new(exe);
    // The cards the user ticked (Cryo-ET, a server with several): the job
    // sees only those, and everything after (the freest leading, GPU
    // workers, the further cards of a training) follows from that.
    if let Ok(req) = read(&dir.join("request.json")) {
        let ticked: Vec<String> = serde_json::from_value(req["args"]["req"]["gpus"].clone()).unwrap_or_default();
        if !ticked.is_empty() {
            match crate::resources::visible_for(&ticked) {
                Some(v) => {
                    c.env("CUDA_VISIBLE_DEVICES", v);
                }
                None => {
                    let e = "勾选的 GPU 在这台服务器上找不到了，请重新勾选";
                    write(&dir.join("state.json"), &json!({"state":"failed","error":e}))?;
                    bail!(e)
                }
            }
        }
    }
    c.args(["agent-worker", id])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            c.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    if let Err(e) = c.spawn() {
        write(
            &dir.join("state.json"),
            &json!({"state":"failed","error":e.to_string()}),
        )?;
        return Err(e.into());
    }
    Ok(())
}

fn supported(c: &str) -> bool {
    matches!(
        c,
        "gpu_info"
            | "tomo_dataset"
            | "tomo_dataset_save"
            | "tomo_session"
            | "tomo_session_reset"
            | "tomo_session_run"
            | "tomo_view"
            | "tomo_preview_stack"
            | "tomo_take"
            | "tomo_pick"
            | "spa_open"
            | "spa_settings"
            | "spa_run"
            | "spa_preview"
            | "spa_classes"
            | "spa_kinds"
    )
}
fn worker(id: &str) -> Result<Value> {
    let dir = job_path(id)?;
    let v = read(&dir.join("request.json"))?;
    let own = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("worker.lock"))?;
    match own.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(Value::Null),
        Err(e) => bail!("无法锁定任务：{e}"),
    }
    // A retry-spawned process can arrive after the original worker finished.
    // Only the original pre-execution state is eligible to begin work.
    let initial = read(&dir.join("state.json"))?;
    if initial["state"] != "queued" || !initial["pid"].is_null() {
        return Ok(Value::Null);
    }
    let pid = std::process::id();
    write(
        &dir.join("state.json"),
        &json!({"state":"queued","pid":pid}),
    )?;
    let cancel = dir.join("cancel");
    std::thread::spawn(move || loop {
        if cancel.exists() {
            crate::motion::cancel();
            crate::tomo::CANCEL.store(true, Ordering::SeqCst);
            crate::spa::CANCEL.store(true, Ordering::SeqCst);
            crate::ctf::CANCEL.store(true, Ordering::SeqCst);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    });
    let command = string(&v, "command")?;
    let args = &v["args"];
    let project = string(&v, "project")?;
    ensure!(Path::new(project).is_absolute(), "absolute input required");
    let base = if let Some(out) = v["outputRoot"].as_str().filter(|s| !s.is_empty()) {
        let out = PathBuf::from(out);
        ensure!(out.is_absolute(), "输出目录需为服务器绝对路径");
        ensure!(
            !out.starts_with(project),
            "请在原始采集目录之外选择输出位置"
        );
        out
    } else {
        root()?.join("projects")
    };
    let project_root = base.join(hash(project.as_bytes()));
    fs::create_dir_all(&project_root)?;
    std::env::set_var("TMV_OUTPUT_ROOT", &project_root);
    if let Some(plugin) = v.get("plugin") {
        let plugin_root = PathBuf::from(string(plugin, "root")?);
        ensure!(
            plugin_root.starts_with(
                dirs::home_dir()
                    .context("home")?
                    .join(".cache/tifmrcview/engines")
            ),
            "invalid plugin root"
        );
        for item in plugin["files"].as_array().context("plugin hashes")? {
            let name = item[0].as_str().context("plugin name")?;
            ensure!(
                Path::new(name)
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
                "invalid plugin path"
            );
            ensure!(
                hash(&fs::read(plugin_root.join(name))?) == item[1].as_str().unwrap_or(""),
                "plugin hash mismatch"
            );
        }
        std::env::set_var("TMV_WORKER_PLUGIN_ROOT",&plugin_root);std::env::set_var("TMV_WORKER_PLUGIN_VERSION",string(plugin,"version")?);
        crate::thumb::configure_plugins_version(Some(plugin_root), string(plugin, "version")?);
    }
    if command == "tomo_session_run" {
        let scan = crate::tomo::session::scan(string(&args["req"], "folder")?)?;
        let only: Vec<String> =
            serde_json::from_value(args["req"]["only"].clone()).unwrap_or_default();
        let mut estimated = 1024u64 * 1024 * 1024;
        for series in scan
            .series
            .iter()
            .filter(|s| only.is_empty() || only.contains(&s.name))
        {
            if series.corrected >= series.movies && series.corrected > 0 {
                continue;
            }
            if let Some(mdoc) = &series.mdoc {
                let doc = crate::motion::series::parse(&fs::read_to_string(mdoc)?)?;
                let [w, h] = doc.image_size.unwrap_or([4096, 4096]);
                estimated = estimated.saturating_add(
                    (w as u64)
                        .saturating_mul(h as u64)
                        .saturating_mul(12)
                        .saturating_mul(doc.views.len() as u64),
                );
            }
        }
        #[cfg(unix)]
        {
            let path = std::ffi::CString::new(project_root.to_string_lossy().as_bytes())?;
            let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } == 0 {
                let stat = unsafe { stat.assume_init() };
                let free = (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64);
                ensure!(
                    free > estimated,
                    "输出磁盘空间不足：本次预计至少需要 {:.1} GB，可用 {:.1} GB；请选择更少序列",
                    estimated as f64 / 1e9,
                    free as f64 / 1e9
                );
            }
        }
    }
    let serial = if long(command) || command == "spa_settings" || command == "tomo_dataset_save" {
        let f = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(project_root.join("processing.lock"))?;
        loop {
            if dir.join("cancel").exists() {
                write(
                    &dir.join("state.json"),
                    &json!({"state":"cancelled","pid":pid}),
                )?;
                return Ok(Value::Null);
            }
            if f.try_lock().is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Some(f)
    } else {
        None
    };
    std::env::set_var("TMV_CANCEL_FILE", dir.join("cancel"));
    let deferred=(command=="tomo_session_run"||command=="spa_run"&&matches!(args["req"]["step"].as_str(),Some("motion"|"ctf"))) && crate::gpu_workers::enabled();
    if deferred{std::env::set_var("TMV_PARENT_GPU_LOCK_DEFERRED","1");}
    let _gpu = if long(command) && !deferred {
        crate::cli::gpu_turn()
    } else {
        None
    };
    if dir.join("cancel").exists() {
        write(
            &dir.join("state.json"),
            &json!({"state":"cancelled","pid":pid}),
        )?;
        return Ok(Value::Null);
    }
    write(
        &dir.join("state.json"),
        &json!({"state":"running","pid":pid,"outputRoot":project_root}),
    )?;
    let throttle = Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
    let seq = std::sync::Arc::new(AtomicU64::new(0));
    let output = std::sync::Arc::new(Mutex::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events.jsonl"))?,
    ));
    {let output=output.clone();let seq=seq.clone();let history=dir.join("backends.jsonl");crate::compute::sink(std::sync::Arc::new(move|p|{let mut out=output.lock().unwrap();let n=seq.fetch_add(1,Ordering::SeqCst)+1;let event=json!({"seq":n,"type":"backend","placement":p});let _=writeln!(out,"{event}");drop(out);if let Ok(mut f)=fs::OpenOptions::new().create(true).append(true).open(&history){let _=writeln!(f,"{event}");}}));}
    let emit = |stage: &str, done: usize, total: usize| {
        let mut last = throttle.lock().unwrap();
        if done != total && last.elapsed() < std::time::Duration::from_millis(200) {
            return;
        }
        *last = std::time::Instant::now();
        let mut f = output.lock().unwrap();
        let n = seq.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = writeln!(
            f,
            "{}",
            json!({"seq":n,"stage":stage,"done":done,"total":total,"worker":if stage.starts_with("GPU "){stage.split(" · ").next().map(str::to_owned)}else{std::env::var("TMV_GPU_LABEL").ok().map(|s|s.split(" · ").next().unwrap_or(&s).to_owned())}})
        );
    };
    emit("正在服务器上运行", 0, 1);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dispatch(command, args, &emit)
    }))
    .unwrap_or_else(|_| Err(anyhow::anyhow!("处理引擎异常退出")));
    drop(serial);
    match result {
        Ok(value) => {
            write(&dir.join("result.json"), &value)?;
            let state = if dir.join("cancel").exists() {
                "cancelled"
            } else if value["result"]["needsChoice"] == true
                || (command == "spa_run" && args["req"]["step"] == "discover")
            {
                "needsChoice"
            } else {
                "completed"
            };
            write(
                &dir.join("state.json"),
                &json!({"state":state,"pid":pid,"outputRoot":project_root}),
            )?;
        }
        Err(e) => {
            write(
                &dir.join("state.json"),
                &json!({"state":if dir.join("cancel").exists(){"cancelled"}else{"failed"},"pid":pid,"error":format!("{e:#}"),"outputRoot":project_root}),
            )?;
        }
    }
    Ok(Value::Null)
}
fn dispatch(cmd: &str, a: &Value, emit: &(dyn Fn(&str, usize, usize) + Sync)) -> Result<Value> {
    use crate::spa::project as sp;
    let req = &a["req"];
    let val = match cmd {
        "gpu_info" => {
            json!({"backend":crate::ctf_gpu::device().map(|(_,n)|*n).unwrap_or("CPU"),"card":crate::ctf_gpu::device().and_then(|(d,_)|d.identity().map(|i|i.name)),"resources":crate::resources::inspect(None)})
        }
        "tomo_dataset" => serde_json::to_value(dataset(string(a, "stack")?)?)?,
        "tomo_dataset_save" => {
            let input: crate::tomo::dataset::Dataset =
                serde_json::from_value(a["dataset"].clone())?;
            let mut d = dataset(&input.stack)?;
            ensure!(
                d.angles.is_empty() || d.angles.len() == input.angles.len(),
                "倾转角数与叠层不一致"
            );
            d.angles = input.angles;
            d.optics = input.optics;
            d.axis_hint = input.axis_hint;
            d.save()?;
            Value::Null
        }
        "tomo_session" => serde_json::to_value(crate::tomo::session::scan(string(a, "folder")?)?)?,
        "tomo_session_reset" => serde_json::to_value(crate::tomo::session::reset(string(a, "folder")?, string(a, "name")?)?)?,
        "tomo_session_run" => {
            let cpu=burn::tensor::Device::default();
            let dev=crate::ctf_gpu::device().map(|(d,_)|d).unwrap_or(&cpu);
            let folder = string(req, "folder")?;
            let only: Vec<String> = serde_json::from_value(req["only"].clone()).unwrap_or_default();
            if crate::gpu_workers::enabled(){
             let snapshot=crate::tomo::session::scan(folder)?;
             let names:Vec<_>=snapshot.series.iter().filter(|s|if only.is_empty(){if req["purpose"].is_null(){s.stage!=crate::tomo::session::Stage::Quick&&s.stage!=crate::tomo::session::Stage::Done}else{s.chosen}}else{only.contains(&s.name)}).map(|s|s.name.clone()).collect();
             if names.len()>1{
              let tasks:Vec<_>=names.iter().map(|name|json!({"kind":"series","folder":folder,"name":name,"purpose":req["purpose"]})).collect();
              let failures=Mutex::new(vec![]);crate::gpu_workers::map(&tasks,emit,&|_|crate::tomo::check_cancel(),&|i,r|{if let Err(e)=r{failures.lock().unwrap().push(format!("{}：{e:#}",names[i]));}})?;
              let errors=failures.into_inner().unwrap();ensure!(errors.is_empty(),"{}",errors.join("；"));return Ok(serde_json::to_value(crate::tomo::session::scan(folder)?)?);
             }
            }
            // One series: this process runs it alone, so it gets the cores the
            // GPU workers would have shared (the global pool was sized for them).
            let run = || -> Result<_> { Ok(
            // The initial pass uses the user's selected sequence list.
            if req["purpose"].is_null() {
                crate::tomo::session::prepare(dev, folder, &only, emit, &|| {})?
            } else {
                crate::tomo::session::finish(
                    dev,
                    folder,
                    serde_json::from_value(req["purpose"].clone())?,
                    &only,
                    emit,
                    &|| {},
                )?
            }) };
            let s = match crate::gpu_workers::enabled().then(crate::resources::whole_pool).flatten() {
                Some(pool) => pool.install(run)?,
                None => run()?,
            };
            serde_json::to_value(s)?
        }
        "tomo_preview_stack" => json!({"path": crate::tomo::session::preview_stack(
            string(a, "path")?,
            a["max"].as_u64().unwrap_or(512) as usize,
        )?}),
        "tomo_view" => {
            let b = crate::tomo::session::view(
                string(a, "path")?,
                a["z"].as_u64().map(|n| n as usize),
                (a["max"].as_u64().unwrap_or(256) as usize).min(768),
            )?;
            json!({"binary":b64(&b)})
        }
        "tomo_take" => serde_json::to_value(crate::tomo::session::take(
            string(a, "stack")?,
            a["take"].as_bool(),
        )?)?,
        "tomo_pick" => serde_json::to_value(crate::tomo::session::pick(
            string(a, "stack")?,
            a["z"].as_u64().context("z")? as usize,
            a["keep"].as_bool(),
        )?)?,
        "spa_open" => serde_json::to_value(sp::Project::open(string(a, "source")?)?)?,
        "spa_settings" => {
            let mut p = sp::Project::load(string(req, "dir")?)?;
            if let Some(d) = req["diameterA"].as_f64().filter(|v| *v > 0.) {
                p.diameter_a = d;
            }
            if let Some(px) = req["pixelA"].as_f64().filter(|v| *v > 0.) {
                p.optics.pixel_a = px;
                p.optics_found.pixel = true;
            }
            if req["kvFromUser"] == true {
                if let Some(kv) = req["kv"].as_f64().filter(|v| *v > 0.) {
                    p.optics.kv = kv;
                    p.optics_found.kv = true;
                }
            }
            if let Some(ex) = req["excluded"].as_array() {
                for (i, m) in p.mics.iter_mut().enumerate() {
                    m.excluded = ex.iter().any(|e| e.as_u64() == Some(i as u64));
                }
            }
            p.save()?;
            serde_json::to_value(p)?
        }
        "spa_run" => spa_run(req, emit)?,
        "spa_kinds" => {
            let p = sp::Project::load(string(a, "dir")?)?;
            let d = p.discovery.context("还没有寻找颗粒类型")?;
            let mut kinds = vec![];
            for k in d.kinds {
                let png = fs::read(&k.png)?;
                let mut v = serde_json::to_value(k)?;
                v["png"] = json!(b64(&png));
                kinds.push(v);
            }
            json!(kinds)
        }
        "spa_classes" => {
            let p = sp::Project::load(string(a, "dir")?)?;
            let c = p.class2d.context("还没有二维分类")?;
            let (v, dims, _) = crate::spa::data::read_volume(&c.path)?;
            let b = dims[0];
            let avgs = v.chunks(b * b).map(|v| v.to_vec()).collect::<Vec<_>>();
            json!({"png":b64(&crate::spa::render::gallery(&avgs,b,8,72)?),"cols":8,"cell":72,"counts":c.counts,"scores":c.scores,"res":c.res,"selected":c.selected})
        }
        "spa_preview" => {
            let p = sp::Project::load(string(a, "dir")?)?;
            let i = a["mic"].as_u64().context("mic")? as usize;
            let picks = if a["params"].is_null() {
                p.picks()?.get(i).cloned().unwrap_or_default()
            } else {
                sp::pick_one(&p, i, &serde_json::from_value(a["params"].clone())?)?
            };
            let m = p.mics.get(i).context("没有这张显微图")?;
            let (img, w, h) = sp::read_mic(&m.path)?;
            let (png, scale) = crate::spa::render::mic_preview(
                &img,
                w,
                h,
                &picks,
                p.diameter_a / 2. / p.optics.pixel_a,
                900,
            )?;
            json!({"png":b64(&png),"picks":picks.len(),"scale":scale})
        }
        _ => bail!("此远程步骤尚不支持：{cmd}"),
    };
    Ok(val)
}
fn b64(b: &[u8]) -> String {
    // base64 is optional on headless builds.
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char)
            } else {
                s.push('=')
            }
        }
    }
    s
}
fn spa_run(req: &Value, emit: &(dyn Fn(&str, usize, usize) + Sync)) -> Result<Value> {
    use crate::spa::project as sp;
    let mut p = sp::Project::load(string(req, "dir")?)?;
    let v = req["params"].clone();
    let result = match string(req, "step")? {
        "motion" => {
            crate::motion::begin();
            sp::motion(&mut p, &serde_json::from_value(v)?, emit)?;
            Value::Null
        }
        "ctf" => {
            sp::ctf(
                &mut p,
                &serde_json::from_value(v["params"].clone())?,
                v["n"].as_u64().unwrap_or(512) as usize,
                emit,
            )?;
            Value::Null
        }
        "pick" => json!(sp::pick_all(&mut p, &serde_json::from_value(v)?, emit)?),
        "extract" => serde_json::to_value(sp::extract_all(
            &mut p,
            v["maxBox"].as_u64().unwrap_or(256) as usize,
            v["invert"].as_bool().unwrap_or(true),
            emit,
        )?)?,
        "class2d" => serde_json::to_value(sp::class2d(&mut p, &serde_json::from_value(v)?, emit)?)?,
        "select" => json!(sp::select(&mut p, serde_json::from_value(v)?)?),
        "initial" => serde_json::to_value(sp::initial(&mut p, &serde_json::from_value(v)?, emit)?)?,
        "flip" => {
            sp::flip_initial(&mut p)?;
            Value::Null
        }
        "discover" => serde_json::to_value(sp::discover(&mut p, None, emit)?)?,
        "choose" => json!(sp::choose(&mut p, serde_json::from_value(v)?)?),
        s @ ("examples" | "examplesUse") => crate::spa::examples::step(&mut p, s, &v, emit)?,
        "auto" => serde_json::to_value(sp::auto(&mut p, &serde_json::from_value(v)?, None, emit)?)?,
        "refine" => serde_json::to_value(sp::refine(
            &mut p,
            &serde_json::from_value(v["params"].clone())?,
            v["reference"].as_str(),
            emit,
        )?)?,
        s => bail!("未知远程单颗粒步骤 {s}"),
    };
    Ok(json!({"project":p,"result":result}))
}

fn dataset(stack: &str) -> Result<crate::tomo::dataset::Dataset> {
    let img = crate::tomo::pipeline::open(stack)?;
    let info = img.info();
    crate::tomo::dataset::Dataset::open(
        &info.path,
        info.size_z,
        info.pixel_size
            .is_some()
            .then(|| crate::align::pixel_a(info)),
    )
}
