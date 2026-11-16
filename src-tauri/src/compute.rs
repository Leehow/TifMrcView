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
// Desktop tasks carry their own sink through the shared CPU pool. The CLI
// retains its process-level sink; simultaneous windows never share dedup state.
pub struct Context {
    sink: Sink,
    last: Mutex<HashMap<String, String>>,
}
thread_local! { static CURRENT: std::cell::RefCell<Option<Arc<Context>>> = const { std::cell::RefCell::new(None) }; }
pub fn context() -> Option<Arc<Context>> {
    CURRENT.with(|c| c.borrow().clone())
}
pub fn with_context<T>(ctx: Option<Arc<Context>>, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<Arc<Context>>);
    impl Drop for Reset {
        fn drop(&mut self) {
            CURRENT.with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let _reset = Reset(CURRENT.with(|c| c.replace(ctx)));
    f()
}
pub fn scoped<T>(sink: Sink, f: impl FnOnce() -> T) -> T {
    with_context(
        Some(Arc::new(Context {
            sink,
            last: Mutex::new(HashMap::new()),
        })),
        f,
    )
}
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
    let ctx = context();
    let target = ctx.as_ref().map(|c| &c.last).unwrap_or(&LAST);
    {
        let mut last = target.lock().unwrap();
        if last.get(&key) == Some(&value) {
            return;
        }
        last.insert(key, value);
    }
    let sink = ctx
        .map(|c| c.sink.clone())
        .or_else(|| SINK.lock().unwrap().clone());
    if let Some(f) = sink {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_context_isolated_across_shared_workers_and_unwind() {
        use rayon::prelude::*;
        let outputs: Vec<_> = (0..2).map(|_| Arc::new(Mutex::new(Vec::new()))).collect();
        std::thread::scope(|s| {
            for (id, output) in outputs.iter().enumerate() {
                let output = output.clone();
                s.spawn(move || {
                    scoped(Arc::new(move |p| output.lock().unwrap().push(p)), || {
                        let ctx = context();
                        (0..12).into_par_iter().for_each(|_| {
                            with_context(ctx.clone(), || report("FFT", "GPU", Some(id.to_string())))
                        });
                        let _ =
                            std::panic::catch_unwind(|| with_context(None, || panic!("restore")));
                        report("CPU", "CPU", Some(id.to_string()));
                    })
                });
            }
        });
        for (id, output) in outputs.iter().enumerate() {
            let output = output.lock().unwrap();
            assert_eq!(output.len(), 2);
            assert!(output
                .iter()
                .all(|p| p.reason.as_deref() == Some(id.to_string().as_str())));
        }
        assert!(context().is_none());
    }
}

/// Used after a learned stage completes with its explicitly supplied device.
pub fn executed(operation: &str, device: &burn::tensor::Device) {
    if let Some((_, name)) = crate::ctf_gpu::device().filter(|(gpu, _)| gpu == device) {
        report(operation, name, None);
    } else {
        report(operation, "CPU", Some(no_gpu()));
    }
}
