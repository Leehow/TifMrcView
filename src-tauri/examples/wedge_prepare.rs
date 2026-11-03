//! Prepare an aligned, calibrated stack for held-out wedge validation.
//! wedge_prepare STACK BIN NEW_OUTPUT_DIR (reads existing alignment, never saves it).
use anyhow::{Context, Result};
use std::path::PathBuf;
use tifmrcview_lib::tomo::{dataset::Dataset, output, pipeline};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let stack = a.get(1).context("wedge_prepare STACK BIN NEW_OUTPUT_DIR")?;
    let bin: usize = a.get(2).context("BIN")?.parse()?;
    let dir = PathBuf::from(a.get(3).context("NEW_OUTPUT_DIR")?);
    std::fs::create_dir(&dir)?;
    let img = pipeline::open(stack)?;
    let d = Dataset::open(stack, img.info().size_z, None)?;
    let al = d.alignment.as_ref().context("no existing alignment")?;
    let px = d.optics.pixel_a.unwrap_or_else(|| output::pixel_a(&img)) * bin as f64;
    let (values, w, h, angles) = output::aligned(&img, al, bin, 0, 0, &|_, _| {})?;
    output::write_mrc(dir.join("aligned.mrc").to_str().context("output path")?, &values, [w,h,angles.len()], px, false)?;
    std::fs::write(dir.join("tilts.tlt"), angles.iter().map(|x| format!("{x:.10}\n")).collect::<String>())?;
    std::fs::write(dir.join("source-dataset.json"), serde_json::to_vec_pretty(&d)?)?;
    println!("{}", serde_json::json!({"source":stack,"bin":bin,"shape_xyz":[w,h,angles.len()],"pixel_a":px,"angles":angles,"axis":al.axis,"tilt_offset":al.tilt_offset}));
    Ok(())
}
