//! EER grouping speed: eer_group FILE.eer [FRAMES=40] [UP=1]. Renders the
//! fractions into FRAMES groups with the band decoder and with the earlier
//! frame-per-worker one, checks they agree, and prints both times.
use anyhow::{Context, Result};
use tifmrcview_lib::motion::eer::Eer;
fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let e = Eer::open(std::path::Path::new(a.get(1).context("eer_group FILE.eer [FRAMES] [UP]")?))?;
    let frames: usize = a.get(2).map(|s| s.parse()).transpose()?.unwrap_or(40);
    let up: usize = a.get(3).map(|s| s.parse()).transpose()?.unwrap_or(1);
    let n = e.len();
    let groups: Vec<_> = (0..frames.min(n)).map(|i| (i * n / frames, (i + 1) * n / frames - i * n / frames)).collect();
    let time = |f: &dyn Fn(usize, usize) -> Result<Vec<f32>>| -> Result<(f64, Vec<f64>)> {
        let t = std::time::Instant::now();
        let mut sums = vec![];
        for &(s, c) in &groups {
            sums.push(f(s, c)?.iter().map(|&v| v as f64).sum());
        }
        Ok((t.elapsed().as_secs_f64() * 1000.0, sums))
    };
    let (band, a1) = time(&|s, c| e.group(s, c, up))?;
    let (worker, b1) = time(&|s, c| e.group_per_worker(s, c, up))?;
    let (band2, _) = time(&|s, c| e.group(s, c, up))?;
    // Every pixel too, for one group.
    anyhow::ensure!(e.group(groups[0].0, groups[0].1, up)? == e.group_per_worker(groups[0].0, groups[0].1, up)?, "group 0 differs");
    anyhow::ensure!(a1 == b1, "event totals differ");
    println!(
        "EER {}x{} fractions={n} groups={} up={up} threads={} events={:.0}: band {band:.0} ms (again {band2:.0} ms), per-worker {worker:.0} ms",
        e.w, e.h, groups.len(), rayon::current_num_threads(), a1.iter().sum::<f64>()
    );
    Ok(())
}
