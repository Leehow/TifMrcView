//! Developer tool: SAM on one slice of a real file, to check masks on real
//! data (and compare with the PyTorch predictor).
//! `cargo run --release --features pro --example sam_slice -- <model.pt> <file> <z> <x,y[,-]>... [--sigma S] [--slab N] [--out DIR]`
//! Points are in image pixels; `,-` marks a background point. `--slab N`
//! averages N slices on each side, `--sigma` blurs before encoding. Writes
//! gray.u8 (the encoded 8-bit image), mask.u8 and overlay.png to DIR.
//! `--propagate` then carries the mask up and down the stack as the app
//! does and prints each slice's area (and why it stopped); with `--prompts`
//! it follows the mask by prompts instead of SAM 2's memory.
use std::time::{Duration, Instant};

use tifmrcview_lib::image::OpenImage;
use tifmrcview_lib::labels::{ChannelValues, Values};
use tifmrcview_lib::{formats, sam};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (model, file, z) = (&args[0], &args[1], args[2].parse::<usize>()?);
    let (mut sigma, mut slab, mut out, mut pts, mut along, mut follow_prompts) = (0.0f32, 0usize, std::path::PathBuf::from("sam-slice"), vec![], false, false);
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--sigma" => { sigma = args[i + 1].parse()?; i += 1; }
            "--slab" => { slab = args[i + 1].parse()?; i += 1; }
            "--out" => { out = args[i + 1].clone().into(); i += 1; }
            "--propagate" => along = true,
            "--prompts" => follow_prompts = true,
            p => {
                let v: Vec<&str> = p.split(',').collect();
                pts.push(tmv_sam::Point { x: v[0].parse()?, y: v[1].parse()?, positive: v.get(2) != Some(&"-") });
            }
        }
        i += 1;
    }
    let img = OpenImage::new(1, formats::open(file.as_ref())?, usize::MAX);
    let info = img.info().clone();
    let (w, h, d) = (info.width, info.height, info.size_z);
    let (page, sample) = info.locate(0, z, 0);
    let (lo, hi) = img.stats(page, sample, &img.page(page)?).auto_range();
    let vals = ChannelValues { img: img.clone(), c: 0, t: 0 };
    let slices: Vec<Vec<f32>> = (z.saturating_sub(slab)..=(z + slab).min(d - 1)).map(|k| vals.slice(k)).collect::<Result<_, _>>()?;
    let gray = sam::to_gray(&sam::denoise(&slices, w, h, sigma), lo, hi);
    let t = Instant::now();
    let (p, backend) = tmv_sam::load_best(model.as_ref(), &tmv_sam::Sam2Config::tiny(), Duration::from_secs(600))?;
    println!("{w}x{h}x{d}, range {lo}..{hi}, backend {backend}, load {:.2?}", t.elapsed());
    let t = Instant::now();
    let enc = p.encode_gray(&gray, w, h);
    let r = p.predict(&enc, &pts, None, None);
    let mask: Vec<u8> = r.logits.iter().map(|&v| (v > 0.0) as u8).collect();
    let area = mask.iter().filter(|&&m| m == 1).count();
    println!("iou_pred {:.3}, area {area} ({:.1}% of slice) in {:.2?}", r.iou, 100.0 * area as f32 / (w * h) as f32, t.elapsed());
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("gray.u8"), &gray)?;
    std::fs::write(out.join("mask.u8"), &mask)?;
    let mut rgb = image::RgbImage::new(w as u32, h as u32);
    for (k, px) in rgb.pixels_mut().enumerate() {
        let g = gray[k];
        *px = if mask[k] == 1 { image::Rgb([g / 2 + 127, g / 2, g / 2]) } else { image::Rgb([g, g, g]) };
    }
    for pt in &pts {
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                let (x, y) = (pt.x as i32 + dx, pt.y as i32 + dy);
                if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                    rgb.put_pixel(x as u32, y as u32, if pt.positive { image::Rgb([0, 255, 0]) } else { image::Rgb([0, 120, 255]) });
                }
            }
        }
    }
    rgb.save(out.join("overlay.png"))?;
    if along {
        let sam = sam::Sam::with_predictor(p);
        let denoise = sam::Denoise { slab, sigma };
        let points = pts.iter().map(|p| sam::PromptPoint { x: p.x, y: p.y, positive: p.positive }).collect();
        let req = sam::PredictReq { token: 1, z, c: 0, t: 0, lo, hi, points, bbox: None, denoise };
        sam.predict(&req, (w, h, d), |k| Ok(vals.slice(k)?))?;
        let req = sam::PropagateReq { token: 1, z, c: 0, t: 0, lo, hi, up: true, down: true, max: d, denoise, follow_prompts };
        let t = Instant::now();
        let mut masks = sam.propagate(&req, (w, h, d), |k| Ok(vals.slice(k)?), |_, _| {})?;
        masks.sort_by_key(|m| m.0);
        println!("propagated to {} slices in {:.2?}", masks.len() - 1, t.elapsed());
        for (k, m) in &masks {
            println!("z {k}: {} px", m.iter().filter(|v| **v).count());
        }
    }
    Ok(())
}
