//! Isolated seed regression for both missing-wedge training paths.
//! cargo run --release --no-default-features --features pro --example wedge_repro -- --cpu
//! Use no --cpu to test the selected native GPU. Small input; no external data.
use anyhow::{ensure, Result};
use burn::tensor::Device;
use sha2::{Digest, Sha256};
use tifmrcview_lib::wedge;

fn fingerprint(v: &[f32]) -> String {
    let mut h = Sha256::new();
    for x in v { h.update(x.to_le_bytes()); }
    format!("{:x}", h.finalize())
}

fn main() -> Result<()> {
    let (dev, backend) = if std::env::args().any(|s| s == "--cpu") {
        (Device::flex(), "CPU")
    } else {
        tifmrcview_lib::classify::device()
    };
    let n = [16; 3];
    let signal: Vec<f32> = (0..16usize.pow(3)).map(|i| {
        let (x, y, z) = ((i % 16) as f32, ((i / 16) % 16) as f32, (i / 256) as f32);
        (x * 0.47).sin() + (y * 0.31 + z * 0.23).cos()
    }).collect();
    let a: Vec<f32> = signal.iter().enumerate().map(|(i, s)| s + 0.1 * ((i * 17 % 31) as f32 - 15.0) / 15.0).collect();
    let b: Vec<f32> = signal.iter().enumerate().map(|(i, s)| s + 0.1 * ((i * 23 % 37) as f32 - 18.0) / 18.0).collect();
    let mut failed = false;
    for halves in [false, true] {
        let mut hashes = vec![];
        for seed in [7, 7, 8] {
            let cfg = wedge::TrainConfig { cube: 16, count: 4, iterations: 1, steps: 3, batch: 2, base: 4, levels: 2, lr: 1e-3, seed, ..Default::default() };
            let out = if halves {
                wedge::train_halves_and_apply(&dev, &a, &b, n, &[-60., -30., 0., 30., 60.], &cfg, |_| {})?
            } else {
                wedge::train_and_apply(&dev, &a, n, [-60., 60.], &cfg, |_| {})?
            };
            ensure!(out.iter().all(|v| v.is_finite()), "non-finite result");
            hashes.push(fingerprint(&out));
        }
        let same = hashes[0] == hashes[1];
        let different = hashes[1] != hashes[2];
        println!("{}", serde_json::json!({"backend":backend,"halves":halves,"same_seed_equal":same,"different_seed_changes_output":different,"sha256":hashes}));
        failed |= !same || !different;
    }
    ensure!(!failed, "cfg.seed does not reproduce both training paths");
    Ok(())
}
