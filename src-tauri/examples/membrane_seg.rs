//! Segments the membranes of a tomogram with the app's code path
//! (`membrane::segment`) and writes the mask as MRC (0/1), for comparisons.
//! Not part of the app. Scores as the app (and MemBrain-seg's own
//! `segment`) does, or with `--budget s` the faster `Pass::Budget`;
//! `--scores FILE` also writes the scores, to compare with its output.
//!
//! membrane_seg IN.mrc MODEL.ckpt|.safetensors OUT.mrc [--pixel Å] [--budget s] [--scores FILE]

use anyhow::{bail, Result};
use tifmrcview_lib::{membrane, tomo};

fn main() -> Result<()> {
    let v: Vec<String> = std::env::args().skip(1).collect();
    if v.len() < 3 {
        bail!("membrane_seg IN.mrc MODEL.ckpt|.safetensors OUT.mrc [--pixel Å] [--budget s] [--scores FILE]");
    }
    let arg = |k: &str| v.iter().position(|a| a == k).and_then(|i| v.get(i + 1));
    let opt = |k: &str| arg(k).map(|s| s.parse::<f64>()).transpose();
    let pass = opt("--budget")?.map_or(membrane::Pass::Official, membrane::Pass::Budget);
    let (dev, backend) = tifmrcview_lib::classify::device();
    let t0 = std::time::Instant::now();
    let (vals, n, px) = tomo::pipeline::load(&v[0])?;
    let px = opt("--pixel")?.unwrap_or(px);
    let t1 = std::time::Instant::now();
    let net = membrane::load(std::path::Path::new(&v[1]), &dev)?;
    eprintln!("{} {:?} at {px} Å on {backend}, read in {:.1} s, model loaded in {:.1} s", v[0], n, t1.duration_since(t0).as_secs_f64(), t1.elapsed().as_secs_f64());
    let seg = membrane::segment(&dev, &net, &vals, n, px as f32, pass, &mut |p| {
        if p.done % 10 == 0 || p.done == p.total {
            eprintln!("  window {}/{}", p.done, p.total);
        }
    })?;
    let out: Vec<f32> = seg.mask.iter().map(|&b| b as u8 as f32).collect();
    tomo::output::write_mrc(&v[2], &out, n, px, false)?;
    if let Some(f) = arg("--scores") {
        tomo::output::write_mrc(f, &seg.score, n, px, false)?;
    }
    eprintln!(
        "{} components, {} voxels, {} copies over {} windows on grid {:?}, network {:.1} s, total {:.1} s",
        seg.components,
        seg.mask.iter().filter(|&&b| b).count(),
        seg.stats.copies,
        seg.stats.windows,
        seg.stats.grid,
        seg.stats.ms / 1e3,
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}
