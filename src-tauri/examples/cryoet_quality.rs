//! Compares ways of reconstructing a dataset by held-out projections: views left out of the
//! reconstruction, projected back, compared by FRC with the measured view:
//! cryoet_quality STACK [BIN=4] [THICKNESS=128]. The dataset must have an alignment and a CTF.
use anyhow::{Context, Result};
use std::time::Instant;
use tifmrcview_lib::tomo::{dataset::Dataset, output, pipeline, quality, recon};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let stack = a.get(1).context("cryoet_quality STACK [BIN] [THICKNESS]")?;
    let bin: usize = a.get(2).map(|s| s.parse()).transpose()?.unwrap_or(4);
    let thickness: usize = a.get(3).map(|s| s.parse()).transpose()?.unwrap_or(128);
    let img = pipeline::open(stack)?;
    let d = Dataset::open(stack, img.info().size_z, None)?;
    let al = d.alignment.clone().context("no alignment")?;
    let ctf = d.ctf.clone().context("no CTF")?;
    let px = d.optics.pixel_a.context("no pixel size")?;
    // Held-out views spread over the series: each is left out, the rest
    // reconstructed and projected back along it.
    let mut order: Vec<usize> = (0..al.views.len()).filter(|&i| !al.views[i].excluded).collect();
    order.sort_by(|&i, &j| al.views[i].angle.total_cmp(&al.views[j].angle));
    let picks: Vec<usize> = [0.12, 0.3, 0.45, 0.55, 0.7, 0.88].iter().map(|f| order[((order.len() - 1) as f64 * f).round() as usize]).collect();
    let configs: Vec<(&str, recon::Method, usize, f32, bool, bool)> = vec![
        ("WBP, no CTF", recon::Method::Wbp, 1, 1.0, false, false),
        ("WBP + 2D flip", recon::Method::Wbp, 1, 1.0, true, false),
        ("WBP + 3D CTF", recon::Method::Wbp, 1, 1.0, true, true),
        ("SIRT15 + 2D flip", recon::Method::Sirt, 15, 1.0, true, false),
        ("SART3 + 2D flip", recon::Method::Sart, 3, 0.5, true, false),
    ];
    let pxb = px * bin as f64;
    // All measured views, aligned, plain and phase flipped (stack order = views kept).
    let (plain, w0, h0, _) = output::aligned(&img, &al, bin, 0, 0, &|_, _| {})?;
    let mut flipped = plain.clone();
    output::flip_views(&mut flipped, w0, h0, &al, bin, (&ctf, &d.optics, px), 0.0);
    let kept: Vec<usize> = (0..al.views.len()).filter(|&i| !al.views[i].excluded).collect();
    let view = |k: usize, flip: bool| -> Vec<f32> {
        let at = kept.iter().position(|&i| i == k).unwrap() * w0 * h0;
        (if flip { &flipped } else { &plain })[at..at + w0 * h0].to_vec()
    };
    println!("{:<20} {:>10} {:>12} {:>12} {:>7}", "method", "FRC.143 Å", "100–40 Å", "40–20 Å", "s");
    let only = std::env::var("ONLY").unwrap_or_default();
    for (name, method, iterations, lambda, flip, three) in configs {
        if !only.is_empty() && !only.split(',').any(|o| name.contains(o)) {
            continue;
        }
        let t = Instant::now();
        let params = recon::Params { thickness, method, iterations, lambda, ..Default::default() };
        let (mut cross, mut low, mut mid) = (0.0, 0.0, 0.0);
        for &k in &picks {
            let mut rest = al.clone();
            rest.views[k].excluded = true;
            let (stack, w, h, angles) = output::aligned(&img, &rest, bin, 0, 0, &|_, _| {})?;
            let how = pipeline::Build { alignment: &rest, bin, params: &params, ctf: flip.then_some((&ctf, &d.optics, px)), ctf_3d: three };
            let vol = pipeline::volume(&how, stack, w, h, &angles)?;
            let theta = al.views[k].refined();
            let proj = quality::project(&vol, w, h, thickness, theta, 0.0)?;
            // The measured view, phase flipped like the reconstruction.
            let seen = view(k, flip);
            let curve = quality::frc(&proj, &seen, w, h, pxb);
            cross += 1.0 / quality::crossing(&curve, 0.143) / picks.len() as f64;
            low += quality::mean_between(&curve, 1.0 / 100.0, 1.0 / 40.0) / picks.len() as f64;
            mid += quality::mean_between(&curve, 1.0 / 40.0, 1.0 / 20.0) / picks.len() as f64;
        }
        println!("{:<20} {:>10.1} {:>12.3} {:>12.3} {:>7.1}", name, cross, low, mid, t.elapsed().as_secs_f64());
    }
    Ok(())
}
