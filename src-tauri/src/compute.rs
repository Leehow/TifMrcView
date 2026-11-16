//! Actual compute placement, emitted only by the code that executed a stage.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub operation: String,
    pub backend: String,
    pub device: Option<String>,
    pub reason: Option<String>,
    #[serde(default)]
    pub worker: Option<String>,
    #[serde(default)]
    pub fallback: bool,
}
type Sink = Arc<dyn Fn(Placement) + Send + Sync>;
static SINK: Mutex<Option<Sink>> = Mutex::new(None);
static LAST: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub fn sink(f: Sink) {
    *SINK.lock().unwrap() = Some(f);
    LAST.lock().unwrap().clear();
}
pub fn report(operation: &str, backend: &str, reason: Option<String>) {
    let worker = std::env::var("TMV_GPU_LABEL").ok();
    let fallback = backend == "CPU" && reason.is_some();
    publish(Placement {
        fallback,
        operation: operation.into(),
        backend: backend.into(),
        device: if backend.contains("GPU") || backend.contains("CUDA") {
            worker.clone().or_else(|| Some(backend.into()))
        } else {
            None
        },
        reason,
        worker,
    });
}
pub fn publish(p: Placement) {
    let key = format!("{}|{}", p.worker.as_deref().unwrap_or("local"), p.operation);
    let value = format!("{}|{:?}", p.backend, p.reason);
    {
        let mut last = LAST.lock().unwrap();
        if last.get(&key) == Some(&value) {
            return;
        }
        last.insert(key, value);
    }
    if let Some(f) = SINK.lock().unwrap().clone() {
        f(p);
    }
}
pub fn no_gpu() -> String {
    crate::ctf_gpu::unavailable_reason()
        .unwrap_or("没有可用 GPU：设备不可见或尚未完成后端初始化")
        .into()
}

pub fn cpu_stage(operation: &str, reason: &str) {
    publish(Placement {
        operation: operation.into(),
        backend: "CPU".into(),
        device: None,
        worker: std::env::var("TMV_GPU_LABEL").ok(),
        reason: Some(reason.into()),
        fallback: false,
    });
}
