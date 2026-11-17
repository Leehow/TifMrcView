//! Actual quick-view movie path. INPUT REPORT [cpu|auto|gpu] [REPEATS=3].
//! Scratch/report live outside source; original movie is read-only.
use anyhow::{Context, Result};
use std::{path::Path, sync::Arc};
use tifmrcview_lib::motion;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let input = a
        .get(1)
        .context("quick_motion INPUT REPORT [cpu|auto|gpu] [REPEATS]")?;
    let report = a.get(2).context("report path required")?;
    let backend = a.get(3).map(String::as_str).unwrap_or("auto");
    let repeats: usize = a.get(4).map(|s| s.parse()).transpose()?.unwrap_or(3);
    let placements = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = placements.clone();
    tifmrcview_lib::compute::sink(Arc::new(move |p| {
        seen.lock().unwrap().push(p);
    }));
    let mut runs = vec![];
    let mut last = vec![];
    for i in 0..repeats {
        motion::begin();
        let p = motion::Params {
            preview: true,
            backend: backend.into(),
            ..Default::default()
        };
        let out = motion::correct(
            input,
            None,
            Path::new(report).parent().unwrap(),
            None,
            0,
            0,
            0,
            &p,
            vec![],
            &|_, _, _| {},
        )?;
        anyhow::ensure!(
            out.sums.unweighted.iter().all(|v| v.is_finite()),
            "non-finite output"
        );
        eprintln!(
            "run {i}: prepare {:.1} decode {:.1} align {:.1} sum {:.1} total {:.1} ms / {}",
            out.result.prepare_ms,
            out.result.decode_ms,
            out.result.alignment_ms,
            out.result.sum_ms,
            out.result.total_ms,
            out.result.backend
        );
        last = out.sums.unweighted;
        runs.push(out.result);
    }
    let bytes: Vec<u8> = last.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(format!("{report}.f32"), bytes)?;
    let data = serde_json::json!({"input":input,"backend":backend,"runs":runs,"placements":*placements.lock().unwrap()});
    std::fs::write(report, serde_json::to_vec_pretty(&data)?)?;
    if let Ok(ms) = std::env::var("TMV_ASSERT_QUICK_ALIGNMENT_MS") {
        let limit: f64 = ms.parse()?;
        let mut times: Vec<_> = runs.iter().skip(1).map(|r| r.alignment_ms).collect();
        anyhow::ensure!(!times.is_empty(), "two or more runs needed");
        times.sort_by(f64::total_cmp);
        anyhow::ensure!(
            times[times.len() / 2] < limit,
            "warm alignment exceeds {limit} ms: {}",
            times[times.len() / 2]
        );
    }
    Ok(())
}
