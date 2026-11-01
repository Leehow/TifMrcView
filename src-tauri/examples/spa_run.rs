//! Single-particle workflow from the command line, on a folder of
//! micrographs or movies (the same steps as the Pro workspace):
//!
//!   cargo run --release --features pro --example spa_run -- <folder> <diameter Å> [symmetry] [pixel Å]
//!
//! With `auto` as the diameter it runs the one-click mode: the particle
//! kinds are found and kind `SPA_KIND` (1-based, default 1) is taken;
//! a symmetry of `auto` (or none) is guessed.
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
    // About every 5 % of each stage.
    let last = std::sync::atomic::AtomicUsize::new(usize::MAX);
    let log = |s: &str, d: usize, t: usize| {
        let tick = d * 20 / t.max(1);
        if last.swap(tick, std::sync::atomic::Ordering::Relaxed) != tick || d == t {
            eprintln!("[{:7.1} s] {s} {d}/{t}", t0.elapsed().as_secs_f64());
        }
    };
    let mut p = project::Project::open(&args[1])?;
    let one_click = args[2] == "auto";
    if !one_click {
        p.diameter_a = args[2].parse()?;
    }
    if let Some(px) = args.get(4) {
        p.optics.pixel_a = px.parse()?;
    }
    p.save()?;
    if p.movies {
        project::motion(&mut p, &tifmrcview_lib::motion::Params::default(), &log)?;
    }
    let px = p.optics.pixel_a;
    let cp = tifmrcview_lib::ctf::Params { voltage_kv: p.optics.kv, cs_mm: p.optics.cs_mm, amp_contrast: p.optics.amplitude, pixel_a: px, res_low_a: 30.0, res_high_a: (px * 2.5).max(4.0), df_min_a: 3000.0, df_max_a: 50000.0, max_astig_a: 2000.0, phase: false, backend: None };
    // Steps already done are kept; SPA_FROM=ctf|pick|class2d|initial|refine redoes from there.
    let from = std::env::var("SPA_FROM").unwrap_or_default();
    let redo = |step: &str| ["ctf", "pick", "class2d", "initial", "refine"].iter().position(|s| *s == step) >= ["ctf", "pick", "class2d", "initial", "refine"].iter().position(|s| *s == from.as_str());
    if redo("ctf") && !from.is_empty() || p.mics.iter().any(|m| m.ctf.is_none() && !m.excluded) {
        project::ctf(&mut p, &cp, 512, &log)?;
    }
    if one_click {
        // SPA_SAM=<sam2.1_hiera_tiny.pt> outlines the blobs with SAM 2.1, as
        // the app does when its model is downloaded.
        let sam = std::env::var("SPA_SAM").ok().map(|path| -> anyhow::Result<tifmrcview_lib::sam::Sam> {
            let (pr, name) = tmv_sam::load_best(std::path::Path::new(&path), &tmv_sam::Sam2Config::tiny(), std::time::Duration::from_secs(90))?;
            eprintln!("SAM on {name}");
            Ok(tifmrcview_lib::sam::Sam::with_predictor(pr))
        }).transpose()?;
        let outline = |img: &[f32], w: usize, h: usize, pts: &[[f32; 2]]| sam.as_ref().unwrap().outline(img, w, h, pts);
        let outliner: Option<tifmrcview_lib::spa::discover::Outliner> = sam.is_some().then_some(&outline);
        let d = project::discover(&mut p, outliner, &log)?;
        eprintln!("outlined by SAM: {}", d.outlined);
        for (i, k) in d.kinds.iter().enumerate() {
            eprintln!("kind {}: {:.0} Å, {:.0}%, {}", i + 1, k.diameter_a, k.share * 100.0, k.png);
        }
        let k: usize = std::env::var("SPA_KIND").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        eprintln!("diameter {} Å", project::choose(&mut p, vec![k - 1])?);
        let symmetry = if sym == "auto" || args.get(3).is_none() { String::new() } else { sym.clone() };
        // SPA_SAM_FILTER=1 also drops picks SAM outlines as clumps or fragments.
        let filter = std::env::var("SPA_SAM_FILTER").is_ok_and(|v| v == "1");
        let r = project::auto(&mut p, &project::AutoParams { symmetry }, if filter { outliner } else { None }, &log)?;
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "resolution": r.res, "resolutionMasked": r.res_masked, "bfactor": r.bfactor, "symmetry": r.params.symmetry,
            "particles": r.particles, "map": r.sharpened, "seconds": t0.elapsed().as_secs_f64(),
        }))?);
        return Ok(());
    }
    if (redo("pick") && !from.is_empty()) || p.stack.is_none() {
        let d = p.diameter_a;
        let n = project::pick_all(&mut p, &pick::PickParams { diameter_a: d, ..Default::default() }, &log)?;
        eprintln!("picked {n}");
        let s = project::extract_all(&mut p, 128, true, &log)?;
        eprintln!("extracted {} at {} px, {:.2} Å/px", s.count, s.box_px, s.pixel_a);
    }
    if (redo("class2d") && !from.is_empty()) || p.class2d.is_none() {
        let c = project::class2d(&mut p, &class2d::Class2dParams::default(), &log)?;
        eprintln!("2D classes {:?}", c.counts);
    }
    // SPA_SELECT=0,1,2 keeps those 2D classes (by rank) for the 3D steps.
    if let Ok(sel) = std::env::var("SPA_SELECT") {
        let n = project::select(&mut p, sel.split(',').map(|s| s.trim().parse()).collect::<Result<_, _>>()?)?;
        eprintln!("selected {n} particles");
    }
    if (redo("initial") && !from.is_empty()) || p.initial.is_none() {
        project::initial(&mut p, &refine::InitialParams { symmetry: sym.clone(), ..Default::default() }, &log)?;
    }
    let r = project::refine(&mut p, &refine::RefineParams { symmetry: sym, ..Default::default() }, None, &log)?;
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
        "resolution": r.res, "resolutionMasked": r.res_masked, "bfactor": r.bfactor,
        "particles": r.particles, "map": r.sharpened, "seconds": t0.elapsed().as_secs_f64(),
    }))?);
    Ok(())
}
