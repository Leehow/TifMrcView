//! Offline motion validation: motion_run INPUT OUTPUT [MDOC|-] [BACKEND=auto]
//! [WORKING=1024]. A .mdoc INPUT runs the sparse series batch. "bench"
//! benchmarks full CPU/Metal warping/summing with one decoded alignment.
use anyhow::{Context, Result};
use tifmrcview_lib::motion::{self, Params, RunReq};
fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let input = a
        .get(1)
        .context("motion_run INPUT OUTPUT [MDOC|-] [auto|cpu|gpu|bench] [WORKING]")?;
    let output = a.get(2).context("OUTPUT")?;
    let backend = a.get(4).map(String::as_str).unwrap_or("auto");
    let p = Params {
        backend: if backend == "bench" { "auto" } else { backend }.into(),
        benchmark: backend == "bench",
        diagnostics: true,
        working_size: a.get(5).map(|s| s.parse()).transpose()?.unwrap_or(1024),
        ..Default::default()
    };
    motion::begin();
    let mut last = String::new();
    let progress = |stage: &str, done: usize, total: usize| {
        if done == 1 || done == total {
            eprintln!("{stage}: {done}/{total}");
        }
    };
    if input.ends_with(".mdoc") {
        let r = motion::series::run(
            &motion::series::SeriesReq {
                mdoc: input.clone(),
                folder: None,
                output: output.clone(),
                params: p,
                drop_tilts: std::env::var("DROP_TILTS").is_ok(),
            },
            &progress,
        )?;
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        let r = motion::run(
            &RunReq {
                token: None,
                path: Some(input.clone()),
                mdoc: a.get(3).filter(|s| *s != "-").cloned(),
                output: output.clone(),
                c: 0,
                z: 0,
                t: 0,
                params: p,
            },
            None,
            &progress,
        )?;
        println!("frames={} raw={} shape={}x{} decode={:.3}s align={:.3}s sum={:.3}s total={:.3}s CPU={:?}ms GPU={:?}ms backend={} drift={:.3}px local={:.3}px patchCPU={:.3}ms patchGPU={:?}ms",r.frames,r.raw_fractions,r.width,r.height,r.decode_ms/1000.0,r.alignment_ms/1000.0,r.sum_ms/1000.0,r.total_ms/1000.0,r.cpu_ms,r.gpu_ms,r.backend,r.total_drift,r.mean_local_motion,r.local.cpu_pair_ms,r.local.gpu_pair_ms);
        println!("warnings={:?}", r.warnings);
        last = r.path;
    }
    let _ = last;
    Ok(())
}
