//! Single-particle workflow from the command line, on a folder of
//! micrographs or movies (the same steps as the Pro workspace):
//!
//!   cargo run --release --features pro --example spa_run -- <folder> <diameter Å> [symmetry] [pixel Å]
//!
//! Writes everything into <folder>/tifmrcview-spa and prints a summary.
use tifmrcview_lib::spa::{class2d, pick, project, refine};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        anyhow::bail!("usage: spa_run <folder> <diameter Å> [symmetry] [pixel Å]");
    }
    let sym = args.get(3).cloned().unwrap_or_else(|| "C1".into());
    let t0 = std::time::Instant::now();
    let log = |s: &str, d: usize, t: usize| {
        if d == t || d % 10 == 0 {
            eprintln!("[{:7.1} s] {s} {d}/{t}", t0.elapsed().as_secs_f64());
        }
    };
    let mut p = project::Project::open(&args[1])?;
    p.diameter_a = args[2].parse()?;
    if let Some(px) = args.get(4) {
        p.optics.pixel_a = px.parse()?;
    }
    p.save()?;
    if p.movies {
        project::motion(&mut p, &tifmrcview_lib::motion::Params::default(), &log)?;
    }
    let px = p.optics.pixel_a;
    let cp = tifmrcview_lib::ctf::Params { voltage_kv: p.optics.kv, cs_mm: p.optics.cs_mm, amp_contrast: p.optics.amplitude, pixel_a: px, res_low_a: 30.0, res_high_a: (px * 2.5).max(4.0), df_min_a: 3000.0, df_max_a: 50000.0, max_astig_a: 2000.0, phase: false, backend: None };
    project::ctf(&mut p, &cp, 512, &log)?;
    let n = project::pick_all(&mut p, &pick::PickParams { diameter_a: p.diameter_a, ..Default::default() }, &log)?;
    eprintln!("picked {n}");
    let s = project::extract_all(&mut p, 128, true, &log)?;
    eprintln!("extracted {} at {} px, {:.2} Å/px", s.count, s.box_px, s.pixel_a);
    let c = project::class2d(&mut p, &class2d::Class2dParams::default(), &log)?;
    eprintln!("2D classes {:?}", c.counts);
    project::initial(&mut p, &refine::InitialParams { symmetry: sym.clone(), ..Default::default() }, &log)?;
    let r = project::refine(&mut p, &refine::RefineParams { symmetry: sym, ..Default::default() }, None, &log)?;
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "resolution": r.res, "resolutionMasked": r.res_masked, "bfactor": r.bfactor,
        "particles": r.particles, "map": r.sharpened, "seconds": t0.elapsed().as_secs_f64(),
    }))?);
    Ok(())
}
