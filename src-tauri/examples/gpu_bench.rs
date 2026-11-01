//! Developer tool: times real Pro GPU work on every backend this build can
//! reach (CUDA, Vulkan, DX12, Metal, CPU), to choose the order in
//! `tmv_sam::backends`.
//! `cargo run --release --no-default-features --features pro --example gpu_bench -- [sam2.1_hiera_tiny.pt] [--cpu]`
//! Each task runs once cold (kernel compilation included) and then three
//! times warm; the table shows cold and best warm seconds. The CPU is
//! skipped unless `--cpu` is given (it takes minutes).
use std::time::Instant;

use anyhow::Result;
use burn::tensor::{Device, Tensor};
use tifmrcview_lib::tomo::recon::{Geometry, Method, Params};
use tifmrcview_lib::{denoise, denoise_learn, tomo, unet};

fn rng(s: &mut u64) -> f32 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    (*s >> 40) as f32 / (1u64 << 24) as f32
}

fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed | 1;
    (0..n).map(|_| rng(&mut s)).collect()
}

/// A smooth image with noise: blobs on a slope.
fn image(w: usize, h: usize, seed: u64) -> Vec<f32> {
    let mut s = seed | 1;
    (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
            ((x * 9.0).sin() * (y * 7.0).cos()).max(0.0) + 0.3 * x + 0.5 * rng(&mut s)
        })
        .collect()
}

type Task = (&'static str, Box<dyn Fn(&Device) -> Result<()>>);

fn tasks(sam: Option<String>) -> Vec<Task> {
    let mut v: Vec<Task> = vec![];
    v.push(("matmul 4096² ×10", Box::new(|d: &Device| {
        let a = Tensor::<2>::from_data(burn::tensor::TensorData::new(noise(4096 * 4096, 1), [4096, 4096]), d);
        let mut x = a.clone();
        for _ in 0..10 {
            x = x.matmul(a.clone()) * (1.0 / 4096.0);
        }
        let _: f32 = x.sum().into_scalar();
        Ok(())
    })));
    v.push(("tomo SART 512×256, 61 views, 64 rows", Box::new(|d: &Device| {
        let angles: Vec<f64> = (-30..=30).map(|i| i as f64 * 2.0).collect();
        let g = Geometry::new(512, 256, &angles, 0.0)?;
        let rows = 64;
        let sino = noise(rows * g.views() * g.w, 2);
        let p = Params { thickness: g.z, method: Method::Sart, iterations: 3, lambda: 0.5, ..Default::default() };
        tomo::recon_gpu::Plan::new(d, &g, &p)?.slab(&sino, rows)?;
        Ok(())
    })));
    v.push(("tomo WBP 1024×512, 61 views, 64 rows", Box::new(|d: &Device| {
        let angles: Vec<f64> = (-30..=30).map(|i| i as f64 * 2.0).collect();
        let g = Geometry::new(1024, 512, &angles, 0.0)?;
        let rows = 64;
        let sino = noise(rows * g.views() * g.w, 3);
        let p = Params { thickness: g.z, method: Method::Wbp, ..Default::default() };
        tomo::recon_gpu::Plan::new(d, &g, &p)?.slab(&sino, rows)?;
        Ok(())
    })));
    v.push(("patch cross-correlation 512 × 128²", Box::new(|d: &Device| {
        let n = 128;
        let pairs: Vec<_> = (0..512).map(|i| (noise(n * n, 10 + i), noise(n * n, 9000 + i))).collect();
        tomo::correlation_gpu::Batch::new(d, n).correlate(&pairs, 20.0)?;
        Ok(())
    })));
    v.push(("NLM denoise 2048²", Box::new(|d: &Device| {
        let (w, h) = (2048, 2048);
        let img = image(w, h, 4);
        let p = denoise::Params { method: denoise::Method::Nlm, strength: 1.0, iterations: 0, patch: 1, search: 7, slab: 0, device: "gpu".into() };
        tifmrcview_lib::denoise_gpu::filter(d, &img, w, h, &p, 0.3);
        Ok(())
    })));
    v.push(("Noise2Void training 60 steps", Box::new(|d: &Device| {
        let (w, h) = (512, 512);
        let pairs = vec![(image(w, h, 5), None), (image(w, h, 6), None)];
        let cfg = denoise_learn::TrainConfig { steps: 60, ..Default::default() };
        denoise_learn::train(d, &pairs, w, h, denoise_learn::Mode::N2v, &cfg, |_, _, _| {})?;
        Ok(())
    })));
    v.push(("U-Net training 40 steps", Box::new(|d: &Device| {
        let (w, h) = (512, 512);
        let slices: Vec<_> = (0..2)
            .map(|k| {
                let img = image(w, h, 7 + k);
                let lab = img.iter().map(|&x| if x > 0.6 { 2u16 } else { 1 }).collect();
                (img, lab)
            })
            .collect();
        let cfg = unet::TrainConfig { steps: 40, ..Default::default() };
        unet::train(d, &slices, w, h, 0.0, 1.5, &cfg, |_, _, _| {})?;
        Ok(())
    })));
    v.push(("3D U-Net (wedge) training 10 steps, 4 × 64³", Box::new(|d: &Device| {
        use burn::optim::{AdamConfig, GradientsParams};
        let ad = d.clone().autodiff();
        let mut net = tifmrcview_lib::wedge::Net::new(16, 3, &ad);
        let mut opt = AdamConfig::new().init();
        let n = 64;
        for k in 0..10 {
            let x = Tensor::<5>::from_data(burn::tensor::TensorData::new(noise(4 * n * n * n, 20 + k), [4, 1, n, n, n]), &ad);
            let t = Tensor::<5>::from_data(burn::tensor::TensorData::new(noise(4 * n * n * n, 50 + k), [4, 1, n, n, n]), &ad);
            let loss = (net.forward(x) - t).abs().mean();
            let _: f32 = loss.clone().into_scalar();
            let grads = GradientsParams::from_grads(loss.backward(), &net);
            net = opt.step(4e-4, net, grads);
        }
        Ok(())
    })));
    v.push(("3D U-Net (wedge) inference 4 × 96³", Box::new(|d: &Device| {
        let net = tifmrcview_lib::wedge::Net::new(16, 3, d);
        let n = 96;
        for k in 0..4 {
            let x = Tensor::<5>::from_data(burn::tensor::TensorData::new(noise(n * n * n, 80 + k), [1, 1, n, n, n]), d);
            let _: f32 = net.forward(x).sum().into_scalar();
        }
        Ok(())
    })));
    if let Some(path) = sam {
        let model = std::sync::Mutex::new(None::<(Device, tmv_sam::Predictor)>);
        v.push(("SAM 2.1 image encode ×5", Box::new(move |d: &Device| {
            let mut m = model.lock().unwrap();
            if m.as_ref().is_none_or(|(md, _)| md != d) {
                *m = Some((d.clone(), tmv_sam::Predictor::load(path.as_ref(), &tmv_sam::Sam2Config::tiny(), d)?));
            }
            let p = &m.as_ref().unwrap().1;
            let px: Vec<u8> = image(1024, 1024, 8).iter().map(|v| (v * 150.0).clamp(0.0, 255.0) as u8).collect();
            for _ in 0..5 {
                let e = p.encode_gray(&px, 1024, 1024);
                p.predict(&e, &[tmv_sam::Point { x: 500.0, y: 500.0, positive: true }], None, None);
            }
            Ok(())
        })));
    }
    v
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let with_cpu = args.iter().any(|a| a == "--cpu");
    let sam = args.iter().find(|a| !a.starts_with("--")).cloned();
    let tasks = tasks(sam);
    let mut seen = vec![];
    let mut table: Vec<(String, Vec<String>)> = vec![];
    for b in tmv_sam::backends() {
        if seen.contains(&b.name) || (b.name == "CPU" && !with_cpu) {
            continue;
        }
        let dev = match b.device() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{}: no device ({e:#})", b.name);
                continue;
            }
        };
        let ok = tifmrcview_lib::ctf_gpu::guarded(|| {
            let x = Tensor::<2>::ones([8, 8], &dev);
            x.clone().matmul(x).sum().into_scalar::<f32>() == 512.0
        });
        if !matches!(ok, Ok(true)) {
            eprintln!("{}: device does not compute ({ok:?})", b.name);
            continue;
        }
        seen.push(b.name);
        let label = match dev.identity() {
            Some(id) => format!("{} ({})", b.name, id.name),
            None => b.name.to_string(),
        };
        eprintln!("== {label}");
        let mut col = vec![];
        for (name, f) in &tasks {
            let mut times = vec![];
            let mut err = None;
            for _ in 0..4 {
                let t = Instant::now();
                match tifmrcview_lib::ctf_gpu::guarded(|| f(&dev)) {
                    Ok(Ok(())) => times.push(t.elapsed().as_secs_f64()),
                    Ok(Err(e)) | Err(e) => {
                        err = Some(format!("{e:#}"));
                        break;
                    }
                }
            }
            let cell = match err {
                Some(e) => format!("failed: {}", e.chars().take(60).collect::<String>()),
                None => format!("{:.2} / {:.3}", times[0], times[1..].iter().cloned().fold(f64::MAX, f64::min)),
            };
            eprintln!("  {name}: {cell}");
            col.push(cell);
        }
        table.push((label, col));
    }
    println!("| task (cold s / warm s) | {} |", table.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(" | "));
    println!("|---|{}", "---|".repeat(table.len()));
    for (i, (name, _)) in tasks.iter().enumerate() {
        println!("| {name} | {} |", table.iter().map(|t| t.1[i].as_str()).collect::<Vec<_>>().join(" | "));
    }
    Ok(())
}
