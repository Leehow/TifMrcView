//! Isolated UUID-bound workers. Inputs remain read-only; only owned scratch
//! products cross process boundaries. One request at a time per device.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn scratch() -> Result<Scratch> {
    let base = std::env::temp_dir();
    for i in 0..1000 {
        let p = base.join(format!("tmv-gpu-{}-{i}", std::process::id()));
        match fs::create_dir(&p) {
            Ok(_) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&p, fs::Permissions::from_mode(0o700))?;
                }
                return Ok(Scratch(p));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("无法创建 GPU 工作缓存")
}
pub fn enabled() -> bool {
    std::env::var("TMV_GPU").as_deref() != Ok("cpu")
        && std::env::var_os("TMV_GPU_WORKER").is_none()
        && crate::resources::usable().len() > 1
}
/// Results are delivered as they complete; callbacks may apply backpressure.
pub fn map(
    tasks: &[Value],
    progress: crate::tomo::pipeline::Progress,
    before: &(dyn Fn(usize) -> Result<()> + Sync),
    done: &(dyn Fn(usize, Result<Value>) + Sync),
) -> Result<()> {
    let gpus = crate::resources::usable();
    ensure!(!gpus.is_empty(), "没有满足显存约束的 GPU");
    let temp = scratch()?;
    let next = AtomicUsize::new(0);
    let errors = Mutex::new(vec![]);
    let cpu = crate::resources::inspect(None).cpu_threads;
    let worker_count = gpus.len().max(1);
    std::thread::scope(|scope| {
        for g in &gpus {
            let dir = &temp.0;
            let next = &next;
            let errors = &errors;
            let compute_context = crate::compute::context();
            scope.spawn(move || {
                let _context = compute_context;
                crate::compute::with_context(_context, || {
                let attempt = (|| -> Result<()> {
                    let mut p = Process(
                        Command::new(std::env::current_exe()?)
                            .arg("gpu-worker")
                            .env("CUDA_VISIBLE_DEVICES", &g.uuid)
                            .env("TMV_GPU_LABEL", format!("GPU {} · {}", g.index, g.name))
                            .env("TMV_GPU_WORKER", "1")
                            .env("TMV_GPU_SHARE", "1")
                            .env_remove("TMV_ALLOWED_GPU_SNAPSHOT")
                            .env_remove("TMV_PARENT_GPU_LOCK_DEFERRED")
                            .env(
                                "RAYON_NUM_THREADS",
                                (cpu / (worker_count + 1)).max(1).to_string(),
                            )
                            .env("TMV_GPU", "cuda")
                            .stdin(Stdio::piped())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::from(fs::File::create(
                                dir.join(format!("gpu-{}.log", g.index)),
                            )?))
                            .spawn()?,
                    );
                    let mut input = p.0.stdin.take().context("worker stdin")?;
                    let mut output = BufReader::new(p.0.stdout.take().context("worker stdout")?);
                    loop {
                        if next.load(Ordering::SeqCst) >= tasks.len() {
                            break;
                        }
                        before(next.load(Ordering::SeqCst).min(tasks.len() - 1))?;
                        let mut waiting = false;
                        let lease = loop {
                            crate::tomo::check_cancel()?;
                            if next.load(Ordering::SeqCst) >= tasks.len() {
                                return Ok(());
                            }
                            if let Some(lease) = crate::resources::try_lease(g)? {
                                break lease;
                            }
                            if !waiting {
                                progress(&format!("GPU {} · 等待已有任务释放资源", g.index), 0, 1);
                                waiting = true;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(150));
                        };
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        if i >= tasks.len() {
                            break;
                        }
                        crate::tomo::check_cancel()?;
                        let mut request = tasks[i].clone();
                        request["scratch"] = json!(dir.join(format!("task-{i}")));
                        writeln!(input, "{request}")?;
                        input.flush()?;
                        let result = (|| -> Result<Value> {
                            loop {
                                let mut line = String::new();
                                ensure!(
                                    output.read_line(&mut line)? > 0,
                                    "GPU {} worker 意外退出；检查驱动或可用显存",
                                    g.index
                                );
                                let event: Value =
                                    serde_json::from_str(&line).context("GPU worker 协议")?;
                                match event["type"].as_str() {
                                    Some("backend") => {
                                        if let Ok(p) =
                                            serde_json::from_value(event["placement"].clone())
                                        {
                                            crate::compute::publish(p);
                                        }
                                    }
                                    Some("progress") => progress(
                                        &format!(
                                            "GPU {} · {}",
                                            g.index,
                                            event["stage"].as_str().unwrap_or("处理")
                                        ),
                                        event["done"].as_u64().unwrap_or(0) as usize,
                                        event["total"].as_u64().unwrap_or(1) as usize,
                                    ),
                                    Some("result") => {
                                        if let Some(e) = event["error"].as_str() {
                                            anyhow::bail!("GPU {}：{e}", g.index);
                                        }
                                        return Ok(event["value"].clone());
                                    }
                                    _ => {}
                                }
                            }
                        })();
                        drop(lease); // never hold a GPU while delivering a backpressured result
                        done(i, result);
                    }
                    Ok(())
                })();
                if let Err(e) = attempt {
                    errors
                        .lock()
                        .unwrap()
                        .push(format!("GPU {}：{e:#}", g.index));
                }
                });
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    ensure!(errors.is_empty(), "{}", errors.join("；"));
    Ok(())
}

fn send(v: Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}
pub fn worker() -> i32 {
    crate::cli::use_bundled_cuda();
    if let (Some(root), Ok(version)) = (
        std::env::var_os("TMV_WORKER_PLUGIN_ROOT"),
        std::env::var("TMV_WORKER_PLUGIN_VERSION"),
    ) {
        crate::thumb::configure_plugins_version(Some(root.into()), &version);
    }
    if let Some(cancel) = std::env::var_os("TMV_CANCEL_FILE") {
        std::thread::spawn(move || loop {
            if Path::new(&cancel).exists() {
                crate::motion::cancel();
                crate::tomo::CANCEL.store(true, Ordering::SeqCst);
                crate::ctf::CANCEL.store(true, Ordering::SeqCst);
                crate::spa::CANCEL.store(true, Ordering::SeqCst);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        });
    }
    crate::compute::sink(std::sync::Arc::new(|p| {
        send(json!({"type":"backend","placement":p}))
    }));
    let progress =
        |s: &str, a: usize, b: usize| send(json!({"type":"progress","stage":s,"done":a,"total":b}));
    for line in std::io::stdin().lock().lines() {
        let value = (|| -> Result<Value> {
            let r: Value = serde_json::from_str(&line?)?;
            crate::tomo::check_cancel()?;
            let _turn = crate::cli::gpu_turn();
            match r["kind"].as_str() {
                Some("spa") => {
                    std::env::set_var("TMV_SPA_SHARD", "1");
                    let mut p: crate::spa::project::Project =
                        serde_json::from_value(r["project"].clone())?;
                    match r["step"].as_str() {
                        Some("motion") => {
                            crate::motion::begin();
                            crate::spa::project::motion(
                                &mut p,
                                &serde_json::from_value(r["params"].clone())?,
                                &progress,
                            )?;
                        }
                        Some("ctf") => crate::spa::project::ctf(
                            &mut p,
                            &serde_json::from_value(r["params"].clone())?,
                            r["nFft"].as_u64().unwrap_or(512) as usize,
                            &progress,
                        )?,
                        _ => anyhow::bail!("unknown SPA shard step"),
                    }
                    Ok(serde_json::to_value(p)?)
                }
                Some("series") => {
                    let cpu = burn::tensor::Device::default();
                    let dev = crate::ctf_gpu::device().map(|(d, _)| d).unwrap_or_else(|| {
                        crate::compute::report("序列计算", "CPU", Some(crate::compute::no_gpu()));
                        &cpu
                    });
                    let root = r["folder"].as_str().context("folder")?;
                    let only = vec![r["name"].as_str().context("name")?.to_owned()];
                    let s = if r["purpose"].is_null() {
                        crate::tomo::session::prepare(dev, root, &only, &progress, &|| {})?
                    } else {
                        crate::tomo::session::finish(
                            dev,
                            root,
                            serde_json::from_value(r["purpose"].clone())?,
                            &only,
                            &progress,
                            &|| {},
                        )?
                    };
                    Ok(serde_json::to_value(s)?)
                }
                Some("movie") => {
                    let dir = PathBuf::from(r["scratch"].as_str().context("scratch")?);
                    fs::create_dir(&dir)?;
                    let params = serde_json::from_value(r["params"].clone())?;
                    let c = crate::motion::correct(
                        r["path"].as_str().context("path")?,
                        None,
                        &dir,
                        None,
                        0,
                        0,
                        0,
                        &params,
                        vec![],
                        &progress,
                    )?;
                    let path = dir.join("sums.f32");
                    let mut f = fs::File::create(&path)?;
                    for data in [&c.sums.unweighted, &c.sums.even, &c.sums.odd] {
                        for v in data {
                            f.write_all(&v.to_le_bytes())?;
                        }
                    }
                    if let Some(w) = &c.sums.weighted {
                        for v in w {
                            f.write_all(&v.to_le_bytes())?;
                        }
                    }
                    f.sync_all()?;
                    Ok(json!({"path":path,"weighted":c.sums.weighted.is_some(),"result":c.result}))
                }
                _ => anyhow::bail!("unknown worker task"),
            }
        })();
        match value {
            Ok(v) => send(json!({"type":"result","value":v})),
            Err(e) => send(json!({"type":"result","error":format!("{e:#}")})),
        }
    }
    0
}
pub fn movie(v: Value) -> Result<crate::motion::Corrected> {
    let result: crate::motion::MotionResult = serde_json::from_value(v["result"].clone())?;
    let mut b = Vec::new();
    fs::File::open(v["path"].as_str().context("movie product")?)?.read_to_end(&mut b)?;
    let n = result.width * result.height;
    let weighted = v["weighted"] == true;
    ensure!(
        b.len() == n * (if weighted { 4 } else { 3 }) * 4,
        "GPU 电影产品截断"
    );
    let values: Vec<f32> = b
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    ensure!(values.iter().all(|v| v.is_finite()), "GPU 电影产品非有限值");
    let chunk = |i| values[i * n..(i + 1) * n].to_vec();
    let _ = fs::remove_dir_all(Path::new(v["path"].as_str().unwrap()).parent().unwrap());
    Ok(crate::motion::Corrected {
        result,
        sums: crate::motion::sum::Sums {
            unweighted: chunk(0),
            even: chunk(1),
            odd: chunk(2),
            weighted: weighted.then(|| chunk(3)),
        },
    })
}
