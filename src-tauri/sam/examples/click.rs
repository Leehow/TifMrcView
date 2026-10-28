//! cargo run --release -p tmv-sam --features wgpu --example click -- sam2.1_hiera_tiny.pt [out-dir]
//! Segments a synthetic disc and box from one click each and prints how
//! well the masks match. With `out-dir`, writes the image (gray.u8, 800x600)
//! and the masks (disc.u8, box.u8) for comparing with the PyTorch model.
use std::time::Instant;

use tmv_sam::{Point, Sam2Config};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("checkpoint path");
    let out = std::env::args().nth(2).map(std::path::PathBuf::from);
    let t = Instant::now();
    let (p, backend) = tmv_sam::load_best(path.as_ref(), &Sam2Config::tiny())?;
    println!("backend {backend}, load + warm-up {:.2?}", t.elapsed());
    let (w, h) = (800usize, 600usize);
    let mut img = vec![0u8; w * h];
    let mut seed = 1u32;
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = (seed >> 24) as f32 / 255.0 * 24.0 - 12.0;
            let (fx, fy) = (x as f32, y as f32);
            let mut v = 40.0 + 20.0 * (fx / 37.0).sin() + 15.0 * (fy / 23.0).cos();
            if (fx - 300.0).powi(2) + (fy - 250.0).powi(2) < 90f32.powi(2) {
                v = 210.0;
            }
            if (fx - 600.0).abs() < 70.0 && (fy - 400.0).abs() < 50.0 {
                v = 160.0;
            }
            img[y * w + x] = (v + noise).clamp(0.0, 255.0) as u8;
        }
    }
    if let Some(o) = &out {
        std::fs::create_dir_all(o)?;
        std::fs::write(o.join("gray.u8"), &img)?;
    }
    for round in 0..2 {
        let t = Instant::now();
        let e = p.encode_gray(&img, w, h);
        let _ = e.features.embed.clone().sum().into_scalar::<f32>();
        println!("encode {} {:.2?}", if round == 0 { "(first)" } else { "(warm)" }, t.elapsed());
    }
    let enc = p.encode_gray(&img, w, h);
    let check = |name: &str, pts: &[Point], inside: &dyn Fn(usize, usize) -> bool| {
        let t = Instant::now();
        let r = p.predict(&enc, pts, None, None);
        let (mut tp, mut fp, mut fneg) = (0, 0, 0);
        for y in 0..h {
            for x in 0..w {
                match (r.logits[y * w + x] > 0.0, inside(x, y)) {
                    (true, true) => tp += 1,
                    (true, false) => fp += 1,
                    (false, true) => fneg += 1,
                    _ => {}
                }
            }
        }
        println!("{name}: iou_pred {:.3}, real IoU {:.3} (fp {fp}, fn {fneg}) in {:.2?}", r.iou, tp as f32 / (tp + fp + fneg) as f32, t.elapsed());
        if let Some(o) = &out {
            let bytes: Vec<u8> = r.logits.iter().map(|&v| (v > 0.0) as u8).collect();
            std::fs::write(o.join(format!("{name}.u8")), bytes).unwrap();
        }
    };
    check("disc", &[Point { x: 300.0, y: 250.0, positive: true }], &|x, y| (x as f32 - 300.0).powi(2) + (y as f32 - 250.0).powi(2) < 8100.0);
    check("box", &[Point { x: 600.0, y: 400.0, positive: true }], &|x, y| (x as f32 - 600.0).abs() < 70.0 && (y as f32 - 400.0).abs() < 50.0);
    Ok(())
}
