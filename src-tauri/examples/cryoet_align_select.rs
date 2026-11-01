//! Scores alignment candidates of a tilt series by held-out projections
//! (tomo::quality::held_out): cryoet_align_select STACK [BIN=4] [THICKNESS=96].
//! The dataset must have a CTF; its alignment is the starting candidate.
use anyhow::{Context, Result};
use std::time::Instant;
use tifmrcview_lib::tomo::{self, dataset::Dataset, model::Groups, pipeline, quality, track::PatchConfig};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let stack = a.get(1).context("cryoet_align_select STACK [BIN] [THICKNESS]")?;
    let bin: usize = a.get(2).map(|s| s.parse()).transpose()?.unwrap_or(4);
    let thickness: usize = a.get(3).map(|s| s.parse()).transpose()?.unwrap_or(96);
    let img = pipeline::open(stack)?;
    let d = Dataset::open(stack, img.info().size_z, None)?;
    let ctf = d.ctf.clone().context("no CTF")?;
    let px = d.optics.pixel_a.context("no pixel size")?;
    let base = tomo::AlignReq { token: 0, c: 0, t: 0, angles: d.angles.clone(), start: None, step: None, axis: d.axis_hint, bin: 0, sweep: true, patch: None, excluded: vec![], groups: None, previous: None };
    let t = Instant::now();
    let first = tomo::align(&img, &base, &|_, _| {})?;
    println!("default: patches {}x{}/{} residual {:.2} ({:.0} s)", first.patch.nx, first.patch.ny, first.patch.size, first.residual, t.elapsed().as_secs_f64());
    let p = first.patch;
    let g = Groups::default_for(d.angles.len());
    let mut cands: Vec<(String, tomo::AlignReq)> = vec![];
    for (name, patch) in [("patch ×0.5", PatchConfig { size: (p.size / 2).max(32), ..p }), ("patch ×1.5", PatchConfig { size: p.size * 3 / 2, ..p }), ("patches ½ count", PatchConfig { nx: (p.nx / 2).max(2), ny: (p.ny / 2).max(2), size: p.size })] {
        cands.push((name.into(), tomo::AlignReq { patch: Some(patch), ..base.clone() }));
    }
    cands.push(("groups ½".into(), tomo::AlignReq { groups: Some(Groups { rotation: (g.rotation / 2).max(1), tilt: (g.tilt / 2).max(1), mag: (g.mag / 2).max(1) }), ..base.clone() }));
    cands.push(("groups ×2".into(), tomo::AlignReq { groups: Some(Groups { rotation: g.rotation * 2, tilt: g.tilt * 2, mag: g.mag * 2 }), ..base.clone() }));
    let score = |al: &tomo::Alignment| quality::held_out(&img, al, Some((&ctf, &d.optics, px)), bin, thickness, 0.0);
    let t = Instant::now();
    let s = score(&first)?;
    println!("{:<18} residual {:>5.2}  FRC.143 {:>5.1} Å  FRC 100–20 Å {:.4}  (score {:.0} s)", "default", first.residual, s.resolution_a, s.frc, t.elapsed().as_secs_f64());
    for (name, req) in cands {
        let al = match tomo::align(&img, &req, &|_, _| {}) {
            Ok(al) => al,
            Err(e) => {
                println!("{name:<18} failed: {e}");
                continue;
            }
        };
        let s = score(&al)?;
        println!("{name:<18} residual {:>5.2}  FRC.143 {:>5.1} Å  FRC 100–20 Å {:.4}", al.residual, s.resolution_a, s.frc);
    }
    Ok(())
}
