//! Trains the membrane network (`tifmrcview_lib::membrane`) on NIfTI
//! training patches laid out as nnU-Net/MemBrain-seg data: `imagesTr/X_0000.nii.gz`
//! with labels `labelsTr/X.nii.gz` (0 background, 1 membrane, 2 ignore),
//! all at 10 Å per voxel. Not part of the app.
//!
//! cargo run --release --features pro,train-cuda --example membrane_train -- \
//!     --data DIR --out membrane.safetensors [--steps 100000] [--batch 2] [--patch 128]
//!     [--lr 0.01] [--val 20] [--resume FILE] [--bench 20] [--backend cuda|vulkan|tch|auto]
//!     [--start-step K] [--skel 1.0] [--skel-level 0]
//! membrane_train --export IN.safetensors OUT.safetensors   (half precision, to publish)
//!
//! Optimiser as nnU-Net: SGD, Nesterov momentum 0.99, weight decay 3e-5,
//! gradient norm clipped at 12, learning rate × (1 − step/steps)^0.9.
//! Loss: Dice + cross-entropy at full, half and quarter resolution
//! (weights 1, ½, ¼), plus a skeleton Dice term at one level (full
//! resolution by default). Augmentation runs on CPU threads beside training.

use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use burn::module::Module;
use burn::optim::decay::WeightDecayConfig;
use burn::optim::momentum::MomentumConfig;
use burn::optim::{GradientsParams, SgdConfig};
use burn::grad_clipping::GradientClippingConfig;
use burn::tensor::{Device, Tensor, TensorData};
use tifmrcview_lib::membrane::{self, augment, nifti, MemNet, NetConfig};

struct Args {
    data: PathBuf,
    out: PathBuf,
    steps: usize,
    start: usize,
    batch: usize,
    patch: usize,
    lr: f64,
    val: usize,
    resume: Option<PathBuf>,
    bench: usize,
    backend: String,
    workers: usize,
    skel: f32,
    skel_level: usize,
}

fn args() -> Result<Args> {
    let mut a = Args {
        data: PathBuf::new(),
        out: PathBuf::from("membrane.safetensors"),
        steps: 100_000,
        start: 0,
        batch: 2,
        patch: 128,
        lr: 0.01,
        val: 20,
        resume: None,
        bench: 0,
        backend: "auto".into(),
        skel: 1.0,
        skel_level: 0,
        workers: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8).saturating_sub(2).clamp(2, 12),
    };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < v.len() {
        let val = v.get(i + 1).cloned().unwrap_or_default();
        match v[i].as_str() {
            "--data" => a.data = val.into(),
            "--out" => a.out = val.into(),
            "--steps" => a.steps = val.parse()?,
            "--start-step" => a.start = val.parse()?,
            "--batch" => a.batch = val.parse()?,
            "--patch" => a.patch = val.parse()?,
            "--lr" => a.lr = val.parse()?,
            "--val" => a.val = val.parse()?,
            "--resume" => a.resume = Some(val.into()),
            "--bench" => a.bench = val.parse()?,
            "--backend" => a.backend = val,
            "--workers" => a.workers = val.parse()?,
            "--skel" => a.skel = val.parse()?,
            "--skel-level" => a.skel_level = val.parse()?,
            o => bail!("unknown argument {o}"),
        }
        i += 2;
    }
    Ok(a)
}

fn device(name: &str) -> Device {
    match name {
        #[cfg(feature = "train-tch")]
        #[allow(deprecated)]
        "tch" => Device::libtorch_cuda(0),
        #[cfg(feature = "train-cuda")]
        "cuda" => Device::cuda(0),
        #[cfg(feature = "train-vulkan")]
        "vulkan" => Device::vulkan(burn::tensor::DeviceKind::DiscreteGpu(0)),
        "cpu" => Device::flex(),
        _ => {
            #[cfg(feature = "train-cuda")]
            return Device::cuda(0);
            #[cfg(all(feature = "train-tch", not(feature = "train-cuda")))]
            #[allow(deprecated)]
            return Device::libtorch_cuda(0);
            #[allow(unreachable_code)]
            tifmrcview_lib::classify::device().0
        }
    }
}

/// (image, labels) file pairs.
fn pairs(dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir.join("imagesTr")).context("imagesTr")? {
        let p = e?.path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".nii.gz") && !name.ends_with(".nii") {
            continue;
        }
        let lab = name.replacen("_0000.nii", ".nii", 1);
        let lp = dir.join("labelsTr").join(&lab);
        if lp.exists() {
            out.push((p, lp));
        } else {
            eprintln!("no labels for {name}");
        }
    }
    out.sort();
    Ok(out)
}

struct Patch {
    n: [usize; 3],
    img: Vec<f32>,
    lab: Vec<u8>,
}

fn read(p: &(PathBuf, PathBuf)) -> Result<Patch> {
    let i = nifti::read(&p.0)?;
    let l = nifti::read(&p.1)?;
    if i.n != l.n {
        bail!("{}: image {:?} vs labels {:?}", p.0.display(), i.n, l.n);
    }
    let n = i.v.len() as f64;
    let m = i.v.iter().map(|&x| x as f64).sum::<f64>() / n;
    let s = (i.v.iter().map(|&x| (x as f64 - m).powi(2)).sum::<f64>() / n).sqrt().max(1e-6);
    Ok(Patch { n: i.n, img: i.v.iter().map(|&x| ((x as f64 - m) / s) as f32).collect(), lab: l.v.iter().map(|&x| x.round().clamp(0.0, 2.0) as u8).collect() })
}

struct Batch {
    x: Vec<f32>,
    y: Vec<Vec<u8>>,
}

fn upload_labels(l: &[u8], b: usize, s: usize, dev: &Device) -> Tensor<5> {
    Tensor::from_data(TensorData::new(l.iter().map(|&v| v as f32).collect::<Vec<f32>>(), [b, 1, s, s, s]), dev)
}

/// Dice of membrane (score > 0) on the voxels not ignored.
fn dice(score: &[f32], lab: &[u8]) -> (f64, f64, f64) {
    let (mut tp, mut fp, mut fn_) = (0f64, 0f64, 0f64);
    for (&s, &l) in score.iter().zip(lab) {
        if l == augment::IGNORE {
            continue;
        }
        match (s > 0.0, l == augment::MEM) {
            (true, true) => tp += 1.0,
            (true, false) => fp += 1.0,
            (false, true) => fn_ += 1.0,
            _ => {}
        }
    }
    (tp, fp, fn_)
}

fn main() -> Result<()> {
    // `--export IN OUT`: IN (single precision) → OUT in half precision, to publish.
    let v: Vec<String> = std::env::args().collect();
    if v.get(1).map(String::as_str) == Some("--export") && v.len() == 4 {
        let dev = Device::flex();
        let net = membrane::load(Path::new(&v[2]), &dev)?;
        membrane::save(&net, Path::new(&v[3]), true)?;
        let back = membrane::load(Path::new(&v[3]), &dev)?;
        let x = Tensor::<5>::random([1, 1, 32, 32, 32], burn::tensor::Distribution::Normal(0.0, 1.0), &dev);
        let d = (net.forward(x.clone()) - back.forward(x)).abs().max().into_scalar::<f32>();
        eprintln!("exported {} (max logit difference {d:.4})", v[3]);
        return Ok(());
    }
    let a = args()?;
    let dev = device(&a.backend);
    eprintln!("device {dev:?}");
    let all = pairs(&a.data)?;
    if all.len() <= a.val {
        bail!("only {} pairs", all.len());
    }
    // Every k-th patch is held out for validation.
    let k = if a.val > 0 { all.len() / a.val } else { usize::MAX };
    let (val, train): (Vec<_>, Vec<_>) = all.iter().cloned().enumerate().partition(|(i, _)| a.val > 0 && i % k == 0 && i / k < a.val);
    let train: Arc<Vec<(PathBuf, PathBuf)>> = Arc::new(train.into_iter().map(|(_, p)| p).collect());
    let val: Vec<(PathBuf, PathBuf)> = val.into_iter().map(|(_, p)| p).collect();
    eprintln!("{} training, {} validation patches", train.len(), val.len());

    let p = a.patch;
    let cfg = NetConfig::default();
    let heads = cfg.heads;
    // Producers: each reads a random patch and makes several samples.
    let (tx, rx) = sync_channel::<(Vec<f32>, Vec<Vec<u8>>)>(4 * a.batch + 8);
    for w in 0..a.workers {
        let tx = tx.clone();
        let train = train.clone();
        std::thread::spawn(move || {
            let mut rng = augment::Rng(0x9E37_79B9_7F4A_7C15 ^ ((w as u64 + 1) * 0x1234_5678_9ABC_DEF1) ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64);
            loop {
                let pair = &train[rng.below(train.len())];
                let patch = match read(pair) {
                    Ok(x) => x,
                    Err(e) => {
                        eprintln!("{e:#}");
                        continue;
                    }
                };
                for _ in 0..4 {
                    let (x, l) = augment::sample(&patch.img, &patch.lab, patch.n, p, &mut rng);
                    let mut ys = vec![l];
                    for h in 1..heads {
                        let next = augment::halve(&ys[h - 1], p >> (h - 1));
                        ys.push(next);
                    }
                    if tx.send((x, ys)).is_err() {
                        return;
                    }
                }
            }
        });
    }
    drop(tx);
    let next_batch = || -> Batch {
        let mut x = Vec::with_capacity(a.batch * p * p * p);
        let mut y: Vec<Vec<u8>> = vec![vec![]; heads];
        for _ in 0..a.batch {
            let (xi, yi) = rx.recv().expect("data workers stopped");
            x.extend(xi);
            for (h, l) in yi.into_iter().enumerate() {
                y[h].extend(l);
            }
        }
        Batch { x, y }
    };
    // Validation: the middle `p`³ of each held-out patch, unaugmented.
    let val_data: Vec<(Vec<f32>, Vec<u8>)> = val
        .iter()
        .filter_map(|pair| {
            let pt = read(pair).ok()?;
            let o: [isize; 3] = std::array::from_fn(|i| (pt.n[i] as isize - p as isize) / 2);
            let vol = tifmrcview_lib::wedge::Vol { n: pt.n, v: pt.img.clone() };
            let x = vol.cube(o, [p; 3]);
            let lv = tifmrcview_lib::wedge::Vol { n: pt.n, v: pt.lab.iter().map(|&v| v as f32).collect() };
            let mut l: Vec<u8> = lv.cube(o, [p; 3]).iter().map(|&v| v as u8).collect();
            // Mirrored voxels outside the patch do not count.
            for z in 0..p {
                for y in 0..p {
                    for xx in 0..p {
                        let c = [xx as isize + o[0], y as isize + o[1], z as isize + o[2]];
                        if (0..3).any(|i| c[i] < 0 || c[i] >= pt.n[i] as isize) {
                            l[(z * p + y) * p + xx] = augment::IGNORE;
                        }
                    }
                }
            }
            Some((x, l))
        })
        .collect();

    let ad = dev.clone().autodiff();
    let mut net = MemNet::new(&cfg, &ad);
    if let Some(r) = &a.resume {
        net = membrane::load(r, &ad)?;
        eprintln!("resumed from {}", r.display());
    }
    let mut opt = SgdConfig::new()
        .with_momentum(Some(MomentumConfig::new().with_momentum(0.99).with_dampening(0.0).with_nesterov(true)))
        .with_weight_decay(Some(WeightDecayConfig::new(3e-5)))
        .with_gradient_clipping(Some(GradientClippingConfig::Norm(12.0)))
        .init();
    let weights = [1.0f32, 0.5, 0.25, 0.125, 0.0625];
    let wsum: f32 = weights[..heads].iter().sum();
    let total = if a.bench > 0 { a.bench } else { a.steps };
    let (mut avg, mut t_data, mut t_step) = (0f32, 0f64, 0f64);
    let t0 = Instant::now();
    let mut best = 0f64;
    for step in a.start..total {
        let td = Instant::now();
        let b = next_batch();
        t_data += td.elapsed().as_secs_f64();
        let ts = Instant::now();
        let x = Tensor::<5>::from_data(TensorData::new(b.x, [a.batch, 1, p, p, p]), &ad);
        let outs = net.forward_all(x);
        let mut loss: Option<Tensor<1>> = None;
        for (h, o) in outs.into_iter().enumerate() {
            let s = p >> h;
            let y = upload_labels(&b.y[h], a.batch, s, &ad);
            let mut l = membrane::net::loss(o.clone(), y.clone());
            if h == a.skel_level && a.skel > 0.0 {
                // Dice+CE and the skeleton term share this level's weight
                // (a coarser level is much cheaper).
                l = (l + membrane::net::skeleton_loss(o, y) * a.skel) / (1.0 + a.skel);
            }
            let l = l * (weights[h] / wsum);
            loss = Some(match loss {
                Some(t) => t + l,
                None => l,
            });
        }
        let loss = loss.unwrap();
        let lv: f32 = loss.clone().into_scalar();
        avg = if step == a.start { lv } else { avg + 0.02 * (lv - avg) };
        let grads = GradientsParams::from_grads(loss.backward(), &net);
        let lr = a.lr * (1.0 - step as f64 / a.steps as f64).max(0.0).powf(0.9);
        net = opt.step(lr, net, grads);
        t_step += ts.elapsed().as_secs_f64();
        let k = step + 1 - a.start;
        if k % 20 == 0 || a.bench > 0 {
            eprintln!(
                "step {} loss {:.4} (avg {:.4}) lr {:.5} | {:.3} s/step (data wait {:.3}) | {:.1} min",
                step + 1,
                lv,
                avg,
                lr,
                t_step / k as f64,
                t_data / k as f64,
                t0.elapsed().as_secs_f64() / 60.0
            );
        }
        let last = step + 1 == total;
        if a.bench == 0 && ((step + 1) % 2000 == 0 || last) {
            let valid = net.valid();
            let (mut tp, mut fp, mut fn_) = (0f64, 0f64, 0f64);
            for (x, l) in &val_data {
                let xt = Tensor::<5>::from_data(TensorData::new(x.clone(), [1, 1, p, p, p]), &dev);
                let s = valid.forward(xt).into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
                let (a1, b1, c1) = dice(&s, l);
                tp += a1;
                fp += b1;
                fn_ += c1;
            }
            let d = 2.0 * tp / (2.0 * tp + fp + fn_).max(1.0);
            eprintln!("step {} validation dice {:.4} (precision {:.3}, recall {:.3})", step + 1, d, tp / (tp + fp).max(1.0), tp / (tp + fn_).max(1.0));
            membrane::save(&valid, &a.out, false)?;
            if d > best {
                best = d;
                membrane::save(&valid, &a.out.with_extension("best.safetensors"), false)?;
            }
        }
    }
    Ok(())
}
