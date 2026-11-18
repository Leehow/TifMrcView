//! Read-only host resources. Device visibility and running jobs remain constraints.
use serde::{Deserialize, Serialize};
use std::path::Path;
#[cfg(not(target_os = "macos"))]
use std::{
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
    /// The disks results can be written to, with their free space (for choosing where results go).
    #[serde(default)]
    pub disks: Vec<Disk>,
    pub warnings: Vec<String>,
}
/// A mounted disk: where it is mounted, its free (usable) and total space.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Disk {
    pub mount: String,
    pub free_mb: u64,
    pub total_mb: u64,
}

/// Free and total MB of the file system holding `path` (or its nearest existing parent).
#[cfg(unix)]
pub fn space(path: &Path) -> Option<(u64, u64)> {
    let p = path.ancestors().find(|p| p.exists()).unwrap_or(path);
    let p = std::ffi::CString::new(p.to_string_lossy().as_bytes()).ok()?;
    let mut s = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(p.as_ptr(), s.as_mut_ptr()) } != 0 {
        return None;
    }
    let s = unsafe { s.assume_init() };
    Some((s.f_bavail as u64 * s.f_frsize as u64 / 1024 / 1024, s.f_blocks as u64 * s.f_frsize as u64 / 1024 / 1024))
}
#[cfg(not(unix))]
pub fn space(_: &Path) -> Option<(u64, u64)> {
    None
}

/// The disks a user can put results on: on Linux the real file systems of
/// /proc/mounts (no proc, tmpfs, overlays of containers, snaps, boot
/// partitions), one per device; on macOS the system disk and /Volumes/*.
/// Small ones (under 1 GB) are left out.
pub fn disks() -> Vec<Disk> {
    let mut mounts: Vec<String> = vec![];
    #[cfg(target_os = "linux")]
    {
        let skip_fs = ["proc", "sysfs", "tmpfs", "devtmpfs", "devpts", "cgroup", "cgroup2", "overlay", "squashfs", "securityfs", "pstore", "debugfs", "tracefs", "configfs", "fusectl", "mqueue", "hugetlbfs", "bpf", "autofs", "binfmt_misc", "efivarfs", "ramfs", "nsfs", "rpc_pipefs", "fuse.lxcfs", "fuse.portal", "fuse.gvfsd-fuse"];
        let mut devices: Vec<String> = vec![];
        for line in std::fs::read_to_string("/proc/mounts").unwrap_or_default().lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 3 || skip_fs.contains(&f[2]) {
                continue;
            }
            let mount = f[1].replace("\\040", " ");
            if mount.starts_with("/boot") || mount.starts_with("/snap") || mount.starts_with("/proc") || mount.starts_with("/sys") || mount.starts_with("/dev") || mount.starts_with("/run") || mount.starts_with("/var/lib/docker") {
                continue;
            }
            if devices.contains(&f[0].to_string()) {
                continue;
            }
            devices.push(f[0].to_string());
            mounts.push(mount);
        }
    }
    #[cfg(target_os = "macos")]
    {
        mounts.push("/".into());
        if let Ok(rd) = std::fs::read_dir("/Volumes") {
            for e in rd.flatten() {
                let p = e.path();
                // The system disk shows up in /Volumes as a link to /.
                if std::fs::canonicalize(&p).map(|c| c == Path::new("/")).unwrap_or(false) {
                    continue;
                }
                mounts.push(p.to_string_lossy().into_owned());
            }
        }
    }
    let mut out: Vec<Disk> = mounts
        .into_iter()
        .filter_map(|m| space(Path::new(&m)).map(|(free_mb, total_mb)| Disk { mount: m, free_mb, total_mb }))
        .filter(|d| d.total_mb >= 1024)
        .collect();
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out
}
/// The device's recommended working set is a performance limit, not free RAM.
#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetalMemory {
    pub name: String,
    pub unified: bool,
    pub recommended_bytes: u64,
    pub allocated_bytes: u64,
    pub max_buffer_bytes: u64,
    pub available_bytes: u64,
}
#[cfg(target_os = "macos")]
pub fn metal_memory() -> Option<MetalMemory> {
    use objc2::{msg_send, rc::Retained, runtime::AnyObject};
    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
    }
    let device = unsafe { Retained::<AnyObject>::from_raw(MTLCreateSystemDefaultDevice()) }?;
    let name: Retained<objc2_foundation::NSString> = unsafe { msg_send![&*device, name] };
    let recommended: usize = unsafe { msg_send![&*device, recommendedMaxWorkingSetSize] };
    let allocated: usize = unsafe { msg_send![&*device, currentAllocatedSize] };
    let max_buffer: usize = unsafe { msg_send![&*device, maxBufferLength] };
    let unified: bool = unsafe { msg_send![&*device, hasUnifiedMemory] };
    let available = if unified {
        (recommended as u64)
            .saturating_sub(allocated as u64)
            .min(memory_available_bytes().saturating_sub(2 * 1024 * 1024 * 1024))
    } else {
        (recommended as u64).saturating_sub(allocated as u64)
    };
    Some(MetalMemory {
        name: name.to_string(),
        unified,
        recommended_bytes: recommended as u64,
        allocated_bytes: allocated as u64,
        max_buffer_bytes: max_buffer as u64,
        available_bytes: available.min(max_buffer as u64),
    })
}
#[allow(deprecated)]
pub fn memory_available_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    unsafe {
        let host = libc::mach_host_self();
        let mut stats: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let result = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            &mut stats as *mut _ as *mut _,
            &mut count,
        );
        unsafe extern "C" {
            fn mach_port_deallocate(task: u32, name: u32) -> i32;
        }
        mach_port_deallocate(libc::mach_task_self(), host);
        if result == 0 {
            // Inactive pages are reclaimable; purgeable pages overlap them.
            return (u64::from(stats.free_count) + u64::from(stats.inactive_count))
                .saturating_mul(libc::sysconf(libc::_SC_PAGESIZE).max(1) as u64);
        }
        return 0;
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("MemAvailable:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|n| n.parse::<u64>().ok())
            })
            .unwrap_or(0)
            .saturating_mul(1024)
    }
}
/// Remaining budget for new resident tensors; no tensor backend is initialized.
pub fn gpu_available_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    {
        return metal_memory().map(|m| m.available_bytes).unwrap_or(0);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let devices = discover();
        let mask = std::env::var("CUDA_VISIBLE_DEVICES").unwrap_or_default();
        let selected = devices
            .iter()
            .find(|g| {
                mask.split(',').next() == Some(g.uuid.as_str())
                    || mask.split(',').next() == Some(g.index.to_string().as_str())
            })
            .or_else(|| devices.first());
        selected
            .map(|g| g.free_mb * 1024 * 1024)
            .unwrap_or(512 * 1024 * 1024)
    }
}
/// Where nvidia-smi lives: on the PATH natively; under WSL the Windows driver puts it in
/// /usr/lib/wsl/lib, which a non-interactive SSH login does not have on its PATH.
#[cfg(not(target_os = "macos"))]
fn query() -> Option<String> {
    ["nvidia-smi", "/usr/lib/wsl/lib/nvidia-smi"].into_iter().find_map(query_with)
}
#[cfg(not(target_os = "macos"))]
fn query_with(program: &str) -> Option<String> {
    let mut c = Command::new(program)
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
    #[cfg(target_os = "macos")]
    return metal_memory()
        .map(|m| {
            vec![Gpu {
                index: 0,
                uuid: "Metal-default".into(),
                name: m.name,
                total_mb: m.recommended_bytes / 1024 / 1024,
                free_mb: m.available_bytes / 1024 / 1024,
                utilization: 0,
            }]
        })
        .unwrap_or_default();
    #[cfg(not(target_os = "macos"))]
    {
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
}
pub fn inspect(output: Option<&Path>) -> Resources {
    let mut warnings = vec![];
    let gpus = discover();
    let memory_available_mb = memory_available_bytes() / 1024 / 1024;
    let output_free_mb = output.and_then(space).map(|(free, _)| free);
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
        warnings.push(format!(
            "当前可用内存限制同时运行 {capacity} 个 GPU worker（每个预留 16 GB）"
        ));
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
        disks: disks(),
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
        // All allowed cards stay visible, the freest first (CUDA device 0):
        // a job can then train on the others too (`multi_gpu`), while the
        // GPU lock and the memory budget follow the first. The snapshot
        // keeps the same order, so `discover()[i]` is CUDA device i.
        let mut order: Vec<&Gpu> = vec![first];
        order.extend(g.iter().filter(|o| o.uuid != first.uuid));
        std::env::set_var(
            "TMV_ALLOWED_GPU_SNAPSHOT",
            serde_json::to_string(&order).unwrap(),
        );
        std::env::set_var("CUDA_VISIBLE_DEVICES", order.iter().map(|o| o.uuid.as_str()).collect::<Vec<_>>().join(","));
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
/// Cards whose GPU lock this process itself holds (`cli::gpu_turn`): a
/// queue of this same job may use them (a `tmv tomo` run holds its card
/// while its own movie workers want every card, that one included).
pub(crate) static HELD_HERE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn try_lease(g: &Gpu) -> anyhow::Result<Option<Lease>> {
    if std::env::var_os("TMV_GPU_SHARE").is_some() || HELD_HERE.lock().unwrap_or_else(|e| e.into_inner()).contains(&g.uuid) {
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

/// A pool with every core but two, for one job running alone in a process
/// whose global pool `configure_threads` sized to share with GPU workers.
pub fn whole_pool() -> Option<rayon::ThreadPool> {
    let cpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let threads = cpu.saturating_sub(2).max(1);
    (threads > rayon::current_num_threads())
        .then(|| rayon::ThreadPoolBuilder::new().num_threads(threads).build().ok())
        .flatten()
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

#[cfg(test)]
mod disk_tests {
    #[test]
    fn the_disks_have_space_and_the_root_is_among_them() {
        let d = super::disks();
        assert!(d.iter().all(|d| d.free_mb <= d.total_mb && d.total_mb >= 1024));
        assert!(super::space(std::path::Path::new("/tmp/does/not/exist")).is_some(), "the nearest existing parent is measured");
    }
}
