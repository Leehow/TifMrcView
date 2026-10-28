//! cargo run --release --example click -- sam2.1_hiera_tiny.pt
//! Segments a synthetic disc and box from one click each and prints how
//! well the masks match.
use std::time::Instant;

use tmv_sam::{Point, Predictor, Sam2Config};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("checkpoint path");
    let device = burn::tensor::Device::default();
    let t = Instant::now();
    let p = Predictor::load(path.as_ref(), &Sam2Config::tiny(), &device)?;
    println!("load {:.2?}", t.elapsed());
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
    let t = Instant::now();
    let enc = p.encode_gray(&img, w, h);
    println!("encode {:.2?}", t.elapsed());
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
    };
    check("disc", &[Point { x: 300.0, y: 250.0, positive: true }], &|x, y| (x as f32 - 300.0).powi(2) + (y as f32 - 250.0).powi(2) < 8100.0);
    check("box", &[Point { x: 600.0, y: 400.0, positive: true }], &|x, y| (x as f32 - 600.0).abs() < 70.0 && (y as f32 - 400.0).abs() < 50.0);
    Ok(())
}
