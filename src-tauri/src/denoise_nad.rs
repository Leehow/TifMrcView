//! IMOD nad_eed_3d adapter. Run the installed program on a full float volume,
//! then share its disk-backed result between previews and export. No 2D fallback.
//! Parameter reference: https://bio3d.colorado.edu/imod/doc/man/nad_eed_3d.html

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::denoise::Params;
use crate::formats::mrc::MrcSource;
use crate::image::{OpenImage, Source};

static CANCEL_EPOCH: AtomicU64 = AtomicU64::new(0);
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub fn cancel() {
    CANCEL_EPOCH.fetch_add(1, Ordering::SeqCst);
}

#[derive(PartialEq)]
struct Key {
    c: usize,
    t: usize,
    strength: f32,
    iterations: usize,
    executable: PathBuf,
}

pub(crate) struct Filtered {
    // Drop the mapping before removing its directory (also works on Windows).
    source: MrcSource,
    _scratch: Scratch,
    key: Key,
}

impl Filtered {
    pub fn plane(&self, z: usize) -> Result<Vec<f32>> {
        if z >= self.source.info().size_z {
            bail!("层号超出范围");
        }
        let page = self.source.read_page(z)?;
        let p = &page[0];
        Ok((0..p.w * p.h).map(|i| p.data.get_f64(i) as f32).collect())
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        for _ in 0..32 {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("tmv-nad-{}-{id}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        bail!("无法创建 NAD 临时目录")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run_command(mut command: Command, check: impl Fn() -> Result<()>) -> Result<std::process::ExitStatus> {
    let mut child = Running(command.spawn().with_context(|| format!("无法运行 {}", command.get_program().to_string_lossy()))?);
    loop {
        check()?;
        if let Some(status) = child.0.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn executable() -> Result<PathBuf> {
    #[cfg(windows)]
    let name = "nad_eed_3d.exe";
    #[cfg(not(windows))]
    let name = "nad_eed_3d";
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("IMOD_NAD_PATH") {
        let path = PathBuf::from(path);
        if !path.is_file() {
            bail!("IMOD_NAD_PATH 指定的 nad_eed_3d 不存在：{}", path.display());
        }
        return Ok(path);
    }
    if let Some(dir) = std::env::var_os("IMOD_DIR") {
        candidates.push(PathBuf::from(dir).join("bin").join(name));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join(name)));
    }
    // Finder-launched apps do not inherit shell startup files.
    for dir in ["/Applications/IMOD", "/usr/local/IMOD", "/opt/IMOD", "C:/IMOD"] {
        candidates.push(PathBuf::from(dir).join("bin").join(name));
    }
    candidates.into_iter().find(|p| p.is_file()).context("未找到 IMOD nad_eed_3d；请安装 IMOD，或设置 IMOD_DIR / IMOD_NAD_PATH 后重新打开应用")
}

/// Full-volume, true 3D EED. K is specified as a multiple of the volume's
/// standard deviation, so it is independent of raw data intensity scaling.
pub(crate) fn prepare(img: &OpenImage, c: usize, t: usize, p: &Params) -> Result<Arc<Filtered>> {
    if !p.strength.is_finite() || p.strength <= 0.0 || !(1..=200).contains(&p.iterations) || p.slab != 0 {
        bail!("NAD 3D 需要 K > 0、迭代 1–200 次；不能同时使用 Z 邻层平均");
    }
    let info = img.info();
    if info.is_rgb || info.size_z < 2 || c >= info.size_c || t >= info.size_t {
        bail!("IMOD NAD 3D 需要至少两层的灰度体数据及有效的通道、时间点");
    }
    let epoch = CANCEL_EPOCH.load(Ordering::SeqCst);
    let check = || -> Result<()> {
        if CANCEL_EPOCH.load(Ordering::SeqCst) != epoch || img.cancelled.load(Ordering::SeqCst) {
            bail!("已取消");
        }
        Ok(())
    };
    let key = Key { c, t, strength: p.strength, iterations: p.iterations, executable: executable()? };
    // One process per image; a simultaneous save/preview reuses the same result.
    let mut cached = img.denoise_nad.lock().map_err(|_| anyhow::anyhow!("NAD 缓存不可用"))?;
    check()?;
    if let Some(result) = cached.as_ref().filter(|r| r.key == key) {
        return Ok(result.clone());
    }
    *cached = None;
    let scratch = Scratch::new()?;
    let input = scratch.0.join("input.mrc");
    let output = scratch.0.join("output.mrc");
    let log = scratch.0.join("nad.log");
    let (w, h, nz) = (info.width, info.height, info.size_z);
    let mut f = BufWriter::new(File::create(&input)?);
    f.write_all(&[0u8; 1024])?;
    // Welford statistics avoid cancellation when the mean is much larger than SD.
    let (mut count, mut mean, mut m2) = (0u64, 0f64, 0f64);
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for z in 0..nz {
        check()?;
        let values = crate::denoise::input(img, c, z, t, 0)?;
        let mut bytes = Vec::with_capacity(values.len() * 4);
        for v in values {
            if !v.is_finite() {
                bail!("NAD 输入包含非有限数值");
            }
            count += 1;
            let delta = v as f64 - mean;
            mean += delta / count as f64;
            m2 += delta * (v as f64 - mean);
            lo = lo.min(v);
            hi = hi.max(v);
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        f.write_all(&bytes)?;
    }
    let sd = (m2 / count as f64).max(0.0).sqrt() as f32;
    let k = (p.strength * sd).max(f32::MIN_POSITIVE);
    if !k.is_finite() {
        bail!("NAD 的 K 值过大，请减小强度或缩放输入数据");
    }
    use std::io::{Seek, SeekFrom};
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&crate::align::mrc_header(w, h, nz, crate::align::pixel_a(info), [lo, hi, mean as f32, sd], "TifMrcView: IMOD NAD 3D input"))?;
    f.flush()?;
    drop(f);
    check()?;
    let mut command = Command::new(&key.executable);
    let diagnostic = File::create(&log)?;
    command
        .args(["-k", &k.to_string(), "-n", &p.iterations.to_string(), "-s", "0", "-t", "0.1", "-m", "2", "-F", "MRC"])
        .arg(&input)
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(diagnostic.try_clone()?)
        .stderr(diagnostic);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let status = run_command(command, &check)?;
    if !status.success() {
        let message = fs::read_to_string(&log).unwrap_or_default();
        bail!("IMOD NAD 3D 失败（{status}）：{}", message.chars().rev().take(2000).collect::<String>().chars().rev().collect::<String>());
    }
    let source = MrcSource::open(&output)?;
    let out = source.info();
    if [out.width, out.height, out.size_z] != [w, h, nz] {
        bail!("IMOD NAD 3D 输出尺寸与输入不一致");
    }
    check()?;
    fs::remove_file(input)?;
    let result = Arc::new(Filtered { source, _scratch: scratch, key });
    *cached = Some(result.clone());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::denoise::{self, Method, PreviewReq, SaveReq};

    fn params() -> Params {
        Params { method: Method::Nad3d, strength: 0.8, iterations: 3, patch: 1, search: 7, slab: 0, device: "cpu".into() }
    }

    fn fixture(dir: &std::path::Path) -> Arc<OpenImage> {
        let (w, h, nz) = (16, 12, 6);
        let values: Vec<f32> = (0..w * h * nz)
            .map(|i| {
                let (x, y, z) = (i % w, i / w % h, i / (w * h));
                (x as f32 * 0.3).sin() + y as f32 * 0.06 + z as f32 * 0.4 + ((i * 37 % 101) as f32 - 50.0) * 0.02
            })
            .collect();
        let path = dir.join("input.mrc");
        let mut f = File::create(&path).unwrap();
        f.write_all(&crate::align::mrc_header(w, h, nz, 2.5, [-1.0, 5.0, 1.0, 1.0], "NAD test")).unwrap();
        for v in values {
            f.write_all(&v.to_le_bytes()).unwrap();
        }
        drop(f);
        OpenImage::new(7, Box::new(MrcSource::open(&path).unwrap()), 0)
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_reaps_child_and_cleans_temporary_files() {
        let scratch = Scratch::new().unwrap();
        let path = scratch.0.clone();
        let pid_file = path.join("pid");
        let mut command = Command::new("sh");
        command.args(["-c", "echo $$ > \"$1\"; exec sleep 30", "nad-test"]).arg(&pid_file);
        let started = std::time::Instant::now();
        let result = run_command(command, || {
            if pid_file.is_file() {
                bail!("已取消");
            }
            if started.elapsed().as_secs() >= 2 {
                bail!("test child did not start");
            }
            Ok(())
        });
        assert!(result.unwrap_err().to_string().contains("已取消"));
        let pid = fs::read_to_string(pid_file).unwrap();
        let alive = Command::new("sh").args(["-c", "kill -0 \"$1\" 2>/dev/null", "nad-probe", pid.trim()]).status().unwrap();
        assert!(!alive.success(), "cancellation must kill and wait for the child");
        drop(scratch);
        assert!(!path.exists());
    }

    #[test]
    fn rejects_invalid_volume_parameters_instead_of_using_2d() {
        let dir = tempfile::tempdir().unwrap();
        let img = fixture(dir.path());
        let mut p = params();
        p.slab = 1;
        assert!(prepare(&img, 0, 0, &p).is_err());
        p.slab = 0;
        p.strength = f32::NAN;
        assert!(prepare(&img, 0, 0, &p).is_err());
        assert!(denoise::filter(vec![1.0; 16], 4, 4, &params(), 1.0).is_err());
    }

    #[test]
    #[ignore = "requires installed IMOD nad_eed_3d"]
    fn actual_imod_matches_preview_all_z_and_export() {
        let dir = tempfile::tempdir().unwrap();
        let img = fixture(dir.path());
        let original = fs::read(dir.path().join("input.mrc")).unwrap();
        let p = params();
        let result = prepare(&img, 0, 0, &p).unwrap();
        let all: Vec<f32> = (0..6).flat_map(|z| crate::denoise::input(&img, 0, z, 0, 0).unwrap()).collect();
        let mean = all.iter().map(|&v| v as f64).sum::<f64>() / all.len() as f64;
        let sd = (all.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / all.len() as f64).sqrt() as f32;
        let direct_path = dir.path().join("direct.mrc");
        let direct = Command::new(executable().unwrap())
            .args(["-k", &(sd * p.strength).to_string(), "-n", "3", "-s", "0", "-t", "0.1", "-m", "2", "-F", "MRC"])
            .arg(dir.path().join("input.mrc"))
            .arg(&direct_path)
            .output()
            .unwrap();
        assert!(direct.status.success(), "{}", String::from_utf8_lossy(&direct.stderr));
        let expected = MrcSource::open(&direct_path).unwrap();
        let mut changed = false;
        for z in 0..6 {
            let preview = denoise::preview(&img, &PreviewReq { token: 7, z, c: 0, t: 0, region: [0.0, 0.0, 16.0, 12.0], params: p.clone() }).unwrap();
            let plane = expected.read_page(z).unwrap();
            for (i, &v) in preview.values.iter().enumerate() {
                assert!(v.is_finite());
                assert!((v - plane[0].data.get_f64(i) as f32).abs() < 1e-6, "Z={z} pixel={i}");
                changed |= (v - all[z * 192 + i]).abs() > 1e-4;
            }
            assert_eq!(preview.backend, "IMOD NAD 3D");
        }
        assert!(changed, "IMOD must actually filter the input");
        assert!(Arc::ptr_eq(&result, &prepare(&img, 0, 0, &p).unwrap()), "Z changes reuse the full-volume result");
        for zs in [vec![], vec![4]] {
            let path = dir.path().join(format!("saved-{}.mrc", zs.len()));
            denoise::save(&img, &SaveReq { token: 7, zs: zs.clone(), c: 0, t: 0, path: path.to_string_lossy().into(), params: p.clone() }, &|_, _| {}).unwrap();
            let saved = MrcSource::open(&path).unwrap();
            assert_eq!(saved.info().size_z, if zs.is_empty() { 6 } else { 1 });
            assert_eq!(saved.info().pixel_size.unwrap()[0], 2.5);
            for z in 0..saved.info().size_z {
                let saved_plane = saved.read_page(z).unwrap();
                let expected_plane = expected.read_page(if zs.is_empty() { z } else { zs[z] }).unwrap();
                for i in 0..192 {
                    assert_eq!(saved_plane[0].data.get_f64(i), expected_plane[0].data.get_f64(i));
                }
            }
        }
        assert_eq!(fs::read(dir.path().join("input.mrc")).unwrap(), original);
        let cached_path = result._scratch.0.clone();
        drop(result);
        drop(img);
        assert!(!cached_path.exists(), "closing the image removes its NAD cache");
    }
}
