//! The Cryo-ET steps from the command line, as the workspace runs them:
//! cryoet_pipeline STACK [PIXEL_A=header] [BIN=4] [THICKNESS=128] [STEPS=align,ctf,rec,n2n,wedge]
//! Writes the outputs beside the stack and central XY/XZ PNGs of each tomogram.
use anyhow::{Context, Result};
use std::{path::Path, time::Instant};
use tifmrcview_lib::{
    formats,
    image::OpenImage,
    tomo::{self, dataset::Dataset, pipeline, recon},
};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let stack = args
        .get(1)
        .context("usage: cryoet_pipeline STACK [PIXEL_A] [BIN] [THICKNESS] [STEPS]")?;
    let pixel: Option<f64> = args
        .get(2)
        .filter(|s| *s != "-")
        .map(|s| s.parse())
        .transpose()?;
    let bin: usize = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(4);
    let thickness: usize = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(128);
    let steps = args
        .get(5)
        .cloned()
        .unwrap_or("align,ctf,rec,n2n,wedge".into());
    let img = OpenImage::new(1, formats::open(Path::new(stack))?, 256 << 20);
    let mut d = Dataset::open(stack, img.info().size_z, pixel)?;
    if pixel.is_some() {
        d.optics.pixel_a = pixel;
    }
    let progress = |stage: &str, i: usize, n: usize| eprint!("\r{stage} {i}/{n}      ");
    let px = d.optics.pixel_a.unwrap_or(1.0);
    if steps.contains("align") {
        let t = Instant::now();
        let req = tomo::AlignReq {
            token: 1,
            c: 0,
            t: 0,
            angles: d.angles.clone(),
            start: None,
            step: None,
            axis: d.axis_hint,
            bin: 0,
            sweep: true,
            patch: None,
            excluded: vec![],
            groups: None,
            previous: None,
        };
        let a = tomo::align(&img, &req, &|i, n| progress("对齐", i, n))?;
        eprintln!();
        println!(
            "align: residual {:.3} px, axis {:.2}, patches {}x{}/{}, bin {}, {:.1} s",
            a.residual,
            a.axis,
            a.patch.nx,
            a.patch.ny,
            a.patch.size,
            a.bin,
            t.elapsed().as_secs_f64()
        );
        d.alignment = Some(a);
        d.save()?;
    }
    if steps.contains("thick") {
        let a = d.alignment.clone().context("align first")?;
        let bin = tomo::auto::output_bin(
            img.info().width,
            img.info().height,
            d.optics.pixel_a.unwrap_or(1.0),
            9.0,
        );
        let mut a = a.clone();
        println!(
            "placement: {:?} at bin {bin}",
            tomo::auto::level(
                &img,
                &mut a,
                d.optics.pixel_a.unwrap_or(1.0),
                bin,
                true,
                &progress
            )?
        );
        return Ok(());
    }
    if steps.contains("auto") {
        let (dev, _) = tifmrcview_lib::ctf_gpu::device().context("GPU")?;
        let t = Instant::now();
        let r = tomo::auto::run(
            dev,
            &mut d,
            &tomo::auto::AutoReq {
                pixel_a: pixel,
                purpose: if steps.contains("sta") {
                    tomo::auto::Purpose::Sta
                } else if steps.contains("survey") {
                    tomo::auto::Purpose::Survey
                } else {
                    tomo::auto::Purpose::Display
                },
                fresh: true,
            },
            &progress,
        )?;
        eprintln!();
        println!(
            "auto: {} (raw {}), bin {}, thickness {}, residual {:.2} px, defocus {:.2} µm, {:.1} s",
            r.path,
            r.raw,
            r.bin,
            r.thickness,
            r.residual,
            r.defocus_um,
            t.elapsed().as_secs_f64()
        );
        png(&r.raw)?;
        png(&r.path)?;
        return Ok(());
    }
    if steps.contains("export") {
        let dir = Path::new(stack).parent().unwrap().join("export");
        std::fs::create_dir_all(&dir)?;
        for format in ["relion5", "imod", "aligned", "table"] {
            let files = tomo::export::run(&d, format, dir.to_str().unwrap(), bin, &|_, _| {})?;
            println!("export {format}: {}", files.join(", "));
        }
    }
    if steps.contains("ctf") {
        let t = Instant::now();
        let params = tifmrcview_lib::ctf::Params {
            voltage_kv: d.optics.kv,
            cs_mm: d.optics.cs_mm,
            amp_contrast: d.optics.amplitude,
            pixel_a: px,
            res_low_a: 30.0,
            res_high_a: (px * 2.5).max(5.0),
            df_min_a: 5000.0,
            df_max_a: 80000.0,
            max_astig_a: 2000.0,
            phase: false,
            backend: None,
        };
        let c = pipeline::ctf(&mut d, &params, 512, &progress)?;
        eprintln!();
        let dfs: Vec<f64> = c.tilts.iter().map(|f| 0.5 * (f.df1 + f.df2)).collect();
        println!(
            "ctf: gradient sign {}, defocus {:.0}..{:.0} Å (median {:.0}), {:.1} s",
            c.gradient,
            dfs.iter().copied().fold(f64::INFINITY, f64::min),
            dfs.iter().copied().fold(0.0, f64::max),
            median(&dfs),
            t.elapsed().as_secs_f64()
        );
        for (f, a) in c.tilts.iter().zip(&d.angles) {
            println!(
                "  tilt {a:7.2}  df {:7.0} / {:7.0} @ {:5.1}°  fit {:5.1} Å  score {:.3}",
                f.df1, f.df2, f.angle, f.fit_res, f.score
            );
        }
    }
    if steps.contains("rec") {
        for (flip, ctf_3d) in [(false, false), (true, false), (true, true)] {
            if flip && d.ctf.is_none() {
                continue;
            }
            let t = Instant::now();
            let step = pipeline::ReconStep {
                bin,
                params: recon::Params {
                    thickness,
                    ..Default::default()
                },
                halves: d.even.is_some() && d.odd.is_some(),
                phase_flip: flip,
                ctf_3d,
            };
            let m = pipeline::reconstruct(&mut d, &step, &progress)?;
            eprintln!();
            println!(
                "rec (flip {flip}, 3D {ctf_3d}): {} on {}, {:.1} s",
                m.paths.join(", "),
                m.backend,
                t.elapsed().as_secs_f64()
            );
            png(&m.paths[0])?;
        }
    }
    if steps.contains("n2n") {
        let (dev, _) = tifmrcview_lib::ctf_gpu::device().context("GPU")?;
        let t = Instant::now();
        let m = pipeline::denoise(
            dev,
            &mut d,
            &pipeline::DenoiseStep {
                source: None,
                steps: 1000,
            },
            &progress,
        )?;
        eprintln!();
        println!("n2n: {} {:.1} s", m.paths[0], t.elapsed().as_secs_f64());
        png(&m.paths[0])?;
    }
    if steps.contains("wedge") {
        let (dev, _) = tifmrcview_lib::ctf_gpu::device().context("GPU")?;
        let t = Instant::now();
        let cfg = tifmrcview_lib::wedge::TrainConfig {
            iterations: 3,
            steps: 60,
            count: 40,
            ..Default::default()
        };
        let source = d
            .tomograms
            .iter()
            .rev()
            .find(|t| t.kind == "denoised")
            .map(|t| t.path.clone());
        let m = pipeline::wedge(
            dev,
            &mut d,
            &pipeline::WedgeStep {
                source,
                cfg: Some(cfg),
                single: false,
                budget_s: None,
            },
            &progress,
        )?;
        eprintln!();
        println!(
            "wedge (short schedule): {} {:.1} s",
            m.paths[0],
            t.elapsed().as_secs_f64()
        );
        png(&m.paths[0])?;
    }
    Ok(())
}

fn median(v: &[f64]) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Central XY section and XZ slice of a tomogram as PNGs beside it.
fn png(path: &str) -> Result<()> {
    let img = OpenImage::new(2, formats::open(Path::new(path))?, 256 << 20);
    let (w, h, z) = (img.info().width, img.info().height, img.info().size_z);
    let section = |k: usize| -> Result<Vec<f32>> {
        let p = img.page(k)?;
        Ok((0..w * h).map(|i| p[0].data.get_f64(i) as f32).collect())
    };
    let xy = section(z / 2)?;
    let mut xz = Vec::with_capacity(w * z);
    for k in 0..z {
        let s = section(k)?;
        xz.extend_from_slice(&s[(h / 2) * w..(h / 2 + 1) * w]);
    }
    save(&format!("{path}.xy.png"), &xy, w, h)?;
    save(&format!("{path}.xz.png"), &xz, w, z)
}

fn save(path: &str, v: &[f32], w: usize, h: usize) -> Result<()> {
    let mut sorted = v.to_vec();
    sorted.sort_by(f32::total_cmp);
    let (lo, hi) = (sorted[sorted.len() / 200], sorted[sorted.len() * 199 / 200]);
    let pixels = v
        .iter()
        .map(|&v| ((v - lo) / (hi - lo).max(1e-20) * 255.0).clamp(0.0, 255.0) as u8)
        .collect();
    image::GrayImage::from_raw(w as u32, h as u32, pixels)
        .context("PNG")?
        .save(path)?;
    Ok(())
}
