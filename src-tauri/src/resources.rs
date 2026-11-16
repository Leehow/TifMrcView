//! Read-only host resources. Device visibility and running jobs remain constraints.
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    pub index: usize,
    pub uuid: String,
    pub name: String,
    pub total_mb: u64,
    pub free_mb: u64,
    pub utilization: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resources {
    pub cpu_threads: usize,
    pub memory_available_mb: u64,
    pub gpus: Vec<Gpu>,
    pub output_free_mb: Option<u64>,
    pub warnings: Vec<String>,
}
fn query() -> Option<String> {
    let mut c = Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,uuid,name,memory.total,memory.free,utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match c.try_wait() {
            Ok(Some(s)) => {
                if !s.success() {
                    return None;
                }
                break;
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(30)),
            _ => {
                let _ = c.kill();
                let _ = c.wait();
                return None;
            }
        }
    }
    use std::io::Read;
    let mut s = String::new();
    c.stdout.take()?.take(65536).read_to_string(&mut s).ok()?;
    Some(s)
}
pub fn discover() -> Vec<Gpu> {
    let snapshot = std::env::var("TMV_ALLOWED_GPU_SNAPSHOT")
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Gpu>>(&s).ok());
    let gpus: Vec<Gpu> = query()
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let p: Vec<_> = l.split(',').map(str::trim).collect();
            if p.len() != 6 || !p[1].starts_with("GPU-") {
                return None;
            }
            Some(Gpu {
                index: p[0].parse().ok()?,
                uuid: p[1].into(),
                name: p[2].into(),
                total_mb: p[3].parse().ok()?,
                free_mb: p[4].parse().ok()?,
                utilization: p[5].parse().ok()?,
            })
        })
        .collect();
    let mask = if let Some(v) = snapshot {
        v.into_iter().map(|g| g.uuid).collect::<Vec<_>>().join(",")
    } else {
        match std::env::var("CUDA_VISIBLE_DEVICES") {
            Ok(v) => v,
            Err(_) => return gpus,
        }
    };
    // Preserve mask order: logical device zero need not be physical GPU zero.
    mask.split(',')
        .filter_map(|s| {
            let s = s.trim();
            let matches: Vec<_> = gpus
                .iter()
                .filter(|g| {
                    s == g.index.to_string() || g.uuid.starts_with(s) && s.starts_with("GPU-")
                })
                .collect();
            (matches.len() == 1).then(|| matches[0].clone())
        })
        .collect()
}
pub fn inspect(output: Option<&Path>) -> Resources {
    let mut warnings = vec![];
    let gpus = discover();
    let memory_available_mb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemAvailable:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u64>().ok())
        })
        .unwrap_or(0)
        / 1024;
    let mut output_free_mb = None;
    #[cfg(unix)]
    if let Some(p) = output {
        let p = p.ancestors().find(|p| p.exists()).unwrap_or(p);
        let p = std::ffi::CString::new(p.to_string_lossy().as_bytes());
        if let Ok(p) = p {
            let mut s = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            if unsafe { libc::statvfs(p.as_ptr(), s.as_mut_ptr()) } == 0 {
                let s = unsafe { s.assume_init() };
                output_free_mb = Some(s.f_bavail as u64 * s.f_frsize as u64 / 1024 / 1024);
            }
        }
    }
    for g in &gpus {
        if g.free_mb < 2048 {
            warnings.push(format!(
                "GPU {} 可用显存不足 2 GB，暂不分配并行 worker",
                g.index
            ));
        }
    }
    let ready = gpus.iter().filter(|g| g.free_mb >= 2048).count();
    let capacity = (memory_available_mb / 16384).max(1) as usize;
    if memory_available_mb > 0 && ready > capacity {
        warnings.push(format!("当前可用内存限制同时运行 {capacity} 个 GPU worker（每个预留 16 GB）"));
    }
    if gpus.is_empty() {
        warnings.push("未探测到可见 NVIDIA GPU，将检查本机其他计算后端".into());
    }
    Resources {
        cpu_threads: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        memory_available_mb,
        gpus,
        output_free_mb,
        warnings,
    }
}
/// Called before the parent's first CUDA initialization. Child workers get a
/// single UUID; the parent retains the original allowed inventory for scheduling.
pub fn pin_primary() {
    if std::env::consts::OS != "linux" {
        return;
    }
    let g = discover();
    if let Some(first) = g.iter().max_by_key(|g| g.free_mb) {
        // Vulkan does not honor CUDA_VISIBLE_DEVICES. Automatic multi-card
        // execution must use CUDA to make the UUID placement truthful.
        if g.len() > 1 && std::env::var_os("TMV_GPU").is_none() {
            std::env::set_var("TMV_GPU", "cuda");
        }
        std::env::set_var(
            "TMV_ALLOWED_GPU_SNAPSHOT",
            serde_json::to_string(&g).unwrap(),
        );
        std::env::set_var("CUDA_VISIBLE_DEVICES", &first.uuid);
        std::env::set_var(
            "TMV_GPU_LABEL",
            format!("GPU {} · {}", first.index, first.name),
        );
    }
}
pub fn usable() -> Vec<Gpu> {
    let r = inspect(None);
    let memory_limit = if r.memory_available_mb == 0 {
        r.gpus.len()
    } else {
        (r.memory_available_mb / 16384).max(1) as usize
    };
    r.gpus
        .into_iter()
        .filter(|g| g.free_mb >= 2048)
        .take(memory_limit)
        .collect()
}

// A queue thread claims a device before claiming a task, so a busy card
// never strands work that an idle card could do. All legacy lock aliases
// are acquired together; a partial acquisition is released before retry.
pub struct Lease {
    _files: Vec<std::fs::File>,
}
pub fn try_lease(g: &Gpu) -> anyhow::Result<Option<Lease>> {
    if std::env::var_os("TMV_GPU_SHARE").is_some() {
        return Ok(Some(Lease { _files: vec![] }));
    }
    let mut names = vec![
        format!(
            "tifmrcview-gpu-{}.lock",
            g.uuid
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
        ),
        format!("tifmrcview-gpu-{}.lock", g.index),
    ];
    if g.index == 0 {
        names.push("tifmrcview-gpu.lock".into());
    }
    names.sort();
    names.dedup();
    let mut files = vec![];
    for name in names {
        let path = std::env::temp_dir().join(name);
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .or_else(|_| std::fs::File::open(path))?;
        match f.try_lock() {
            Ok(()) => files.push(f),
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(Some(Lease { _files: files }))
}

pub fn configure_threads() {
    if std::env::var_os("RAYON_NUM_THREADS").is_some() {
        return;
    }
    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let gpus = usable().len();
    let threads = if gpus > 1 {
        cpu / (gpus + 1)
    } else {
        cpu.saturating_sub(2)
    }
    .max(1);
    std::env::set_var("RAYON_NUM_THREADS", threads.to_string());
}
