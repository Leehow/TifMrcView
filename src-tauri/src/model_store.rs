//! Image-local training records. Publish the JSON pointer only after its
//! immutable payload is complete and hashed; a failed save keeps the old model.
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::image::OpenImage;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);
static SAVE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    pub path: String,
    pub bytes: u64,
    pub modified_ns: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub files: Vec<SourceFile>,
    pub dimensions: [usize; 5],
    pub series: usize,
}

fn stamp(path: &Path) -> Result<(u64, String)> {
    let m = fs::metadata(path)?;
    if !m.is_file() {
        bail!("训练输入不是普通文件：{}", path.display());
    }
    Ok((m.len(), m.modified()?.duration_since(UNIX_EPOCH)?.as_nanos().to_string()))
}

pub fn sha256(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn training_labels_sha256(slices: &[(usize, Vec<u16>)]) -> String {
    let mut hash = Sha256::new();
    for (z, labels) in slices {
        hash.update((*z as u64).to_le_bytes());
        hash.update((labels.len() as u64).to_le_bytes());
        for label in labels {
            hash.update(label.to_le_bytes());
        }
    }
    format!("{:x}", hash.finalize())
}

impl Identity {
    pub fn capture(img: &OpenImage) -> Result<Self> {
        let mut cached = img.model_identity.lock().map_err(|_| anyhow::anyhow!("训练数据标识缓存不可用"))?;
        if let Some(identity) = cached.as_ref() {
            identity.unchanged()?;
            return Ok(identity.clone());
        }
        let mut files = Vec::new();
        for path in img.source.model_sources() {
            let (bytes, modified_ns) = stamp(&path)?;
            let digest = sha256(&path)?;
            if stamp(&path)? != (bytes, modified_ns.clone()) {
                bail!("训练输入正在变化：{}", path.display());
            }
            files.push(SourceFile { path: path.to_string_lossy().into(), bytes, modified_ns, sha256: digest });
        }
        if files.is_empty() {
            bail!("无法确定模型的训练输入");
        }
        let i = img.info();
        let identity = Self { files, dimensions: [i.width, i.height, i.size_z, i.size_c, i.size_t], series: i.series_index };
        *cached = Some(identity.clone());
        Ok(identity)
    }

    fn same_pixels(&self, other: &Self) -> bool {
        self.dimensions == other.dimensions
            && self.series == other.series
            && self.files.len() == other.files.len()
            && self.files.iter().zip(&other.files).all(|(a, b)| a.bytes == b.bytes && a.sha256 == b.sha256)
    }

    fn unchanged(&self) -> Result<()> {
        for file in &self.files {
            if stamp(Path::new(&file.path))? != (file.bytes, file.modified_ns.clone()) {
                bail!("训练输入已改变，未覆盖已保存的模型");
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub schema: u32,
    pub kind: String,
    pub architecture: String,
    pub app_version: String,
    pub build_revision: String,
    pub burn_version: String,
    pub source: Identity,
    pub channel: usize,
    pub time: usize,
    pub run_id: String,
    pub started_at_unix_ms: u64,
    pub saved_at_unix_ms: u64,
    pub elapsed_seconds: f64,
    pub complete: bool,
    pub progress: Value,
    pub parameters: Value,
    pub metrics: Value,
    pub backend: String,
    pub capability: String,
    pub payload: String,
    pub payload_sha256: String,
}

fn new_id() -> String {
    format!("{}-{}-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos(), std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

fn valid_kind(kind: &str) -> Result<()> {
    if !["wedge", "denoise", "unet", "classifier"].contains(&kind) {
        bail!("未知的训练模型类型");
    }
    Ok(())
}

pub fn directory(img: &OpenImage, kind: &str, c: usize, t: usize) -> Result<PathBuf> {
    valid_kind(kind)?;
    if c >= img.info().size_c || t >= img.info().size_t {
        bail!("模型通道或时间点无效");
    }
    // A directory of slices has its records inside it; one file has a sidecar.
    let path = Path::new(&img.info().path);
    let root = if path.is_dir() { path.join(".tmv-models") } else { PathBuf::from(format!("{}.tmv-models", path.to_string_lossy())) };
    Ok(root.join(format!("series-{}", img.info().series_index)).join(kind).join(format!("c{c}-t{t}")))
}

#[derive(Clone)]
pub struct Session {
    pub record: Record,
    dir: PathBuf,
    started: std::time::Instant,
}

impl Session {
    pub fn start(img: &OpenImage, kind: &str, c: usize, t: usize, architecture: &str, parameters: Value) -> Result<Self> {
        let dir = directory(img, kind, c, t)?;
        let source = Identity::capture(img)?;
        // Detect unwritable destinations before spending time training.
        fs::create_dir_all(&dir).context("无法创建模型目录；请先将图像复制到可写位置")?;
        let probe = dir.join(format!(".probe-{}", new_id()));
        File::create(&probe).context("模型目录不可写")?.sync_all()?;
        fs::remove_file(probe)?;
        Ok(Self {
            record: Record {
                schema: 1,
                kind: kind.into(),
                architecture: architecture.into(),
                app_version: option_env!("TMV_APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")).into(),
                build_revision: option_env!("TMV_BUILD_REVISION").unwrap_or("development").into(),
                burn_version: "0.22.1".into(),
                source,
                channel: c,
                time: t,
                run_id: new_id(),
                started_at_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
                saved_at_unix_ms: 0,
                elapsed_seconds: 0.0,
                complete: false,
                progress: Value::Null,
                parameters,
                metrics: Value::Null,
                backend: String::new(),
                capability: "inference".into(),
                payload: String::new(),
                payload_sha256: String::new(),
            },
            dir,
            started: std::time::Instant::now(),
        })
    }

    pub fn save(&self, complete: bool, progress: Value, metrics: Value, backend: &str, extension: &str, write: impl FnOnce(&Path) -> Result<()>) -> Result<Record> {
        if !["safetensors", "json"].contains(&extension) {
            bail!("模型载荷格式无效");
        }
        self.record.source.unchanged()?;
        let _lock = SAVE_LOCK.lock().map_err(|_| anyhow::anyhow!("模型保存锁不可用"))?;
        if disabled(&self.dir, &self.record)? {
            bail!("该训练任务已被清除，未覆盖模型记录");
        }
        let pointer = self.dir.join(if complete { "model.json" } else { "checkpoint.json" });
        let previous = read_record(&pointer).ok();
        let payload = format!("weights-{}.{}", new_id(), extension);
        let path = self.dir.join(&payload);
        let mut guard = PendingFile(path.clone());
        write(&path)?;
        File::options().read(true).write(true).open(&path)?.sync_all()?;
        self.record.source.unchanged()?;
        let mut record = self.record.clone();
        record.complete = complete;
        record.progress = progress;
        record.metrics = metrics;
        record.backend = backend.into();
        record.saved_at_unix_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
        record.elapsed_seconds = self.started.elapsed().as_secs_f64();
        record.payload = payload;
        record.payload_sha256 = sha256(&path)?;
        atomic_json(&pointer, &record)?;
        guard.0 = PathBuf::new();
        if let Some(old) = previous {
            if let Ok(p) = payload_path(&self.dir, &old) {
                let _ = fs::remove_file(p);
            }
        }
        if complete {
            let _ = self.clear_partial();
        }
        Ok(record)
    }

    pub fn clear_partial(&self) -> Result<()> {
        let path = self.dir.join("checkpoint.json");
        if let Ok(record) = read_record(&path) {
            if record.run_id == self.record.run_id {
                fs::remove_file(&path)?;
                if let Ok(p) = payload_path(&self.dir, &record) {
                    let _ = fs::remove_file(p);
                }
            }
        }
        Ok(())
    }

    pub fn record_path(&self, complete: bool) -> PathBuf {
        self.dir.join(if complete { "model.json" } else { "checkpoint.json" })
    }
}

struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.0);
        }
    }
}

fn atomic_json(path: &Path, record: &impl Serialize) -> Result<()> {
    let part = path.with_extension(format!("{}.part", new_id()));
    let _guard = PendingFile(part.clone());
    let mut f = File::options().write(true).create_new(true).open(&part)?;
    f.write_all(&serde_json::to_vec_pretty(record)?)?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    drop(f);
    fs::rename(part, path)?;
    Ok(())
}

fn read_record(path: &Path) -> Result<Record> {
    // Folder stacks can have thousands of input-file provenance entries.
    if fs::metadata(path)?.len() > 32 * 1024 * 1024 {
        bail!("模型参数记录过大");
    }
    Ok(serde_json::from_reader(File::open(path)?)?)
}

fn payload_path(dir: &Path, record: &Record) -> Result<PathBuf> {
    let name = &record.payload;
    if !name.starts_with("weights-") || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
        bail!("模型载荷路径无效");
    }
    Ok(dir.join(name))
}

pub fn load(img: &OpenImage, kind: &str, c: usize, t: usize, architecture: &str) -> Result<Option<(Record, PathBuf)>> {
    let dir = directory(img, kind, c, t)?;
    // A completed model takes precedence; an interrupted first run can recover
    // its usable checkpoint, explicitly marked incomplete in the UI.
    for path in [dir.join("model.json"), dir.join("checkpoint.json")] {
        if !path.exists() {
            continue;
        }
        let record = read_record(&path)?;
        if disabled(&dir, &record)? {
            continue;
        }
        if record.schema != 1 || record.kind != kind || record.architecture != architecture || record.channel != c || record.time != t || record.capability != "inference" {
            bail!("模型版本、算法结构或通道不兼容，未加载");
        }
        if !record.source.same_pixels(&Identity::capture(img)?) {
            bail!("训练数据与当前图像不同，未加载已有模型");
        }
        let payload = payload_path(&dir, &record)?;
        if sha256(&payload)? != record.payload_sha256 {
            bail!("模型权重校验失败，未加载");
        }
        return Ok(Some((record, payload)));
    }
    Ok(None)
}

#[derive(Serialize, Deserialize)]
struct Disabled {
    before_ns: String,
}

fn disabled(dir: &Path, record: &Record) -> Result<bool> {
    let marker = dir.parent().unwrap().join("disabled.json");
    if !marker.exists() {
        return Ok(false);
    }
    if fs::metadata(&marker)?.len() > 4096 {
        bail!("模型停用记录无效");
    }
    let disabled: Disabled = serde_json::from_reader(File::open(marker)?)?;
    let before: u128 = disabled.before_ns.parse()?;
    let start: u128 = record.run_id.split('-').next().context("模型训练标识无效")?.parse()?;
    Ok(start <= before)
}

/// Preserve files for recovery, but do not resurrect a classifier the user
/// explicitly cleared. A later training run gets a new run ID and can save.
pub fn disable(img: &OpenImage, kind: &str) -> Result<()> {
    let dir = directory(img, kind, 0, 0)?;
    let _lock = SAVE_LOCK.lock().map_err(|_| anyhow::anyhow!("模型保存锁不可用"))?;
    let root = dir.parent().unwrap();
    fs::create_dir_all(root)?;
    atomic_json(&root.join("disabled.json"), &Disabled { before_ns: SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos().to_string() })
}

pub fn save_network(net: &impl burn_store::ModuleSnapshot, path: &Path) -> Result<()> {
    crate::ctf_gpu::guarded(|| net.save_into(&mut burn_store::SafetensorsStore::from_file(path)))?.map_err(|e| anyhow::anyhow!("保存模型权重失败：{e:?}"))
}

pub fn load_network(net: &mut impl burn_store::ModuleSnapshot, path: &Path) -> Result<()> {
    let applied = net.load_from(&mut burn_store::SafetensorsStore::from_file(path)).map_err(|e| anyhow::anyhow!("读取模型权重失败：{e:?}"))?;
    if !applied.missing.is_empty() || !applied.errors.is_empty() {
        bail!("模型权重不完整");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;

    pub fn image(root: &Path, name: &str) -> Arc<OpenImage> {
        let path = root.join(name);
        let mut f = File::create(&path).unwrap();
        f.write_all(&crate::align::mrc_header(16, 16, 16, 2.5, [0.0, 1.0, 0.5, 0.2], "checkpoint fixture")).unwrap();
        for i in 0..16 * 16 * 16 {
            f.write_all(&((i % 257) as f32 / 257.0).to_le_bytes()).unwrap();
        }
        drop(f);
        OpenImage::new(17, Box::new(crate::formats::mrc::MrcSource::open(&path).unwrap()), 0)
    }

    #[test]
    fn failed_write_and_cancel_preserve_the_completed_model() {
        let temp = tempfile::tempdir().unwrap();
        let img = image(temp.path(), "input.mrc");
        let session = Session::start(&img, "wedge", 0, 0, "test-v1", serde_json::json!({"seed": 7})).unwrap();
        let save = |path: &Path| -> Result<()> {
            fs::write(path, b"completed")?;
            Ok(())
        };
        session.save(true, Value::Null, Value::Null, "CPU", "json", save).unwrap();
        assert!(session
            .save(true, Value::Null, Value::Null, "CPU", "json", |path| {
                fs::write(path, b"half-written")?;
                bail!("disk write failed")
            })
            .is_err());
        let (record, path) = load(&img, "wedge", 0, 0, "test-v1").unwrap().unwrap();
        assert_eq!(fs::read(path).unwrap(), b"completed");
        assert_eq!(record.parameters["seed"], 7);
        let next = Session::start(&img, "wedge", 0, 0, "test-v1", Value::Null).unwrap();
        next.save(false, serde_json::json!({"step": 4}), Value::Null, "CPU", "json", |p| {
            fs::write(p, b"partial")?;
            Ok(())
        })
        .unwrap();
        next.clear_partial().unwrap();
        assert!(load(&img, "wedge", 0, 0, "test-v1").unwrap().unwrap().0.complete);
        assert_eq!(fs::read_dir(&session.dir).unwrap().count(), 2, "one record and one payload, with no failed-write debris");
    }

    #[test]
    fn corrupt_or_different_source_is_rejected_and_first_checkpoint_is_recoverable() {
        let temp = tempfile::tempdir().unwrap();
        let img = image(temp.path(), "input.mrc");
        let session = Session::start(&img, "denoise", 0, 0, "test-v1", Value::Null).unwrap();
        let saved = session
            .save(false, serde_json::json!({"step": 10}), Value::Null, "CPU", "json", |p| {
                fs::write(p, b"usable")?;
                Ok(())
            })
            .unwrap();
        let (record, path) = load(&img, "denoise", 0, 0, "test-v1").unwrap().unwrap();
        assert!(!record.complete);
        assert_eq!(record.progress["step"], 10);
        fs::write(&path, b"corrupt").unwrap();
        assert!(load(&img, "denoise", 0, 0, "test-v1").is_err());
        fs::write(path, b"usable").unwrap();
        let mut changed = fs::read(&img.info().path).unwrap();
        changed[1024..1028].copy_from_slice(&0.123f32.to_le_bytes());
        fs::write(&img.info().path, changed).unwrap();
        assert!(load(&img, "denoise", 0, 0, "test-v1").is_err());
        assert_eq!(saved.source.dimensions, [16, 16, 16, 1, 1]);
    }

    #[test]
    fn record_rejects_architecture_change_and_payload_path_escape() {
        let temp = tempfile::tempdir().unwrap();
        let img = image(temp.path(), "input.mrc");
        let session = Session::start(&img, "unet", 0, 0, "test-v1", Value::Null).unwrap();
        let mut record = session
            .save(true, Value::Null, Value::Null, "CPU", "json", |p| {
                fs::write(p, b"usable")?;
                Ok(())
            })
            .unwrap();
        assert!(load(&img, "unet", 0, 0, "test-v2").is_err());
        record.payload = "../outside".into();
        atomic_json(&session.dir.join("model.json"), &record).unwrap();
        assert!(load(&img, "unet", 0, 0, "test-v1").is_err());
    }

    #[test]
    fn clearing_a_classifier_does_not_restore_it_and_keeps_weights_recoverable() {
        let temp = tempfile::tempdir().unwrap();
        let img = image(temp.path(), "input.mrc");
        let old = Session::start(&img, "classifier", 0, 0, "test-v1", Value::Null).unwrap();
        let record = old
            .save(true, Value::Null, Value::Null, "CPU", "json", |p| {
                fs::write(p, b"old")?;
                Ok(())
            })
            .unwrap();
        disable(&img, "classifier").unwrap();
        assert!(load(&img, "classifier", 0, 0, "test-v1").unwrap().is_none());
        assert!(old.dir.join(record.payload).exists());
        assert!(old.save(true, Value::Null, Value::Null, "CPU", "json", |_| panic!("cleared run must not write")).is_err());
        let next = Session::start(&img, "classifier", 0, 0, "test-v1", Value::Null).unwrap();
        next.save(false, Value::Null, Value::Null, "CPU", "json", |p| {
            fs::write(p, b"new")?;
            Ok(())
        })
        .unwrap();
        assert!(!load(&img, "classifier", 0, 0, "test-v1").unwrap().unwrap().0.complete);
    }

    #[test]
    fn moving_image_and_model_sidecar_preserves_restore() {
        let temp = tempfile::tempdir().unwrap();
        let img = image(temp.path(), "original.mrc");
        let session = Session::start(&img, "denoise", 0, 0, "test-v1", Value::Null).unwrap();
        let record = session
            .save(true, Value::Null, Value::Null, "CPU", "json", |p| {
                fs::write(p, b"portable")?;
                Ok(())
            })
            .unwrap();
        let moved = temp.path().join("renamed.mrc");
        fs::copy(&img.info().path, &moved).unwrap();
        let relocated = OpenImage::new(18, Box::new(crate::formats::mrc::MrcSource::open(&moved).unwrap()), 0);
        let dir = directory(&relocated, "denoise", 0, 0).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::copy(session.dir.join("model.json"), dir.join("model.json")).unwrap();
        fs::copy(session.dir.join(&record.payload), dir.join(&record.payload)).unwrap();
        let (loaded, payload) = load(&relocated, "denoise", 0, 0, "test-v1").unwrap().unwrap();
        assert_eq!(loaded.source.files[0].sha256, record.source.files[0].sha256);
        assert_eq!(fs::read(payload).unwrap(), b"portable");
    }
}
