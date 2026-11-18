//! Which way of training Noise2Noise on several tilt series denoises best,
//! scored on data no network saw:
//! n2n_compare STEPS EVEN1,ODD1 EVEN2,ODD2 ...
//!
//! Every series' half tomograms are standardised on their own. Sections are
//! picked as the pipeline does (at most 64, spread over the depth); the left
//! three quarters of each section (X) are for training, the right quarter
//! is held out. The score of a network on a series is the held-out
//! Noise2Noise loss, E|f(even) − odd|² both ways round, divided by that of
//! doing nothing (E|even − odd|²): lower is better, and since the noise of
//! the target is independent, a difference between methods is a difference
//! in their error against the true signal.
//!
//! Methods (N2N_METHODS, comma separated; default all):
//! - sep: one network per series, STEPS steps (what the pipeline does now)
//! - pool: one network on all series, STEPS steps
//! - pool_long: one network on all series, STEPS × series steps (the same
//!   compute as sep)
//! - pool_ft: pool, then STEPS/4 steps on each series alone at a tenth of
//!   the learning rate
//! - loo: one network on all the other series (a series it never saw)
//! - sep_signal / sep_val / sep_both: sep with patches only where the halves
//!   share signal / with held-out cells and early stopping / both
//! N2N_ONE_CARD=1 trains on one GPU only.
use anyhow::{Context, Result};
use std::time::Instant;
use tifmrcview_lib::denoise_learn::{fit, Fit, Fitting, Img, Mode, TrainConfig};
use tifmrcview_lib::tomo::pipeline::load;

struct Series {
    name: String,
    /// Training crops (even, odd), w × h each.
    train: Vec<(Vec<f32>, Vec<f32>)>,
    tw: usize,
    /// Held-out crops.
    held: Vec<(Vec<f32>, Vec<f32>)>,
    hw: usize,
    h: usize,
    /// E|even − odd|² on the held-out crops.
    identity: f64,
}

fn crop(v: &[f32], w: usize, h: usize, z: usize, x0: usize, x1: usize) -> Vec<f32> {
    let plane = &v[z * w * h..(z + 1) * w * h];
    (0..h).flat_map(|y| plane[y * w + x0..y * w + x1].iter().copied()).collect()
}

fn standardised(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.len() as f64;
    let m = v.iter().map(|&x| x as f64).sum::<f64>() / n;
    let s = (v.iter().map(|&x| (x as f64 - m).powi(2)).sum::<f64>() / n).sqrt().max(1e-12);
    v.iter_mut().for_each(|x| *x = ((*x as f64 - m) / s) as f32);
    v
}

fn imgs(s: &Series) -> Vec<Img<'_>> {
    s.train.iter().map(|(a, b)| Img { a, b: Some(b), w: s.tw, h: s.h }).collect()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: n2n_compare STEPS EVEN1,ODD1 EVEN2,ODD2 ...";
    let steps: usize = args.get(1).context(usage)?.parse()?;
    let methods: Vec<String> = std::env::var("N2N_METHODS").unwrap_or("sep,pool,pool_long,pool_ft,loo,sep_signal,sep_val,sep_both".into()).split(',').map(str::to_string).collect();
    let mut series = vec![];
    for arg in &args[2..] {
        let (e, o) = arg.split_once(',').context(usage)?;
        let (ev, [w, h, nz], _) = load(e)?;
        let (od, shape, _) = load(o)?;
        anyhow::ensure!(shape == [w, h, nz], "{arg}: halves differ in size");
        let (ev, od) = (standardised(ev), standardised(od));
        let split = (w * 3 / 4) / 4 * 4;
        let picks: Vec<usize> = (0..nz).step_by((nz / 64).max(1)).collect();
        let train: Vec<_> = picks.iter().map(|&z| (crop(&ev, w, h, z, 0, split), crop(&od, w, h, z, 0, split))).collect();
        let held: Vec<_> = picks.iter().map(|&z| (crop(&ev, w, h, z, split, w), crop(&od, w, h, z, split, w))).collect();
        let identity = held.iter().map(|(a, b)| a.iter().zip(b).map(|(x, y)| ((x - y) as f64).powi(2)).sum::<f64>()).sum::<f64>() / held.iter().map(|(a, _)| a.len()).sum::<usize>() as f64;
        let name = std::path::Path::new(e).file_stem().unwrap().to_string_lossy().into_owned();
        eprintln!("{name}: {w}×{h}×{nz}, {} sections, identity loss {identity:.4}", picks.len());
        series.push(Series { name, train, tw: split, held, hw: w - split, h, identity });
    }
    let primary = tifmrcview_lib::classify::device().0;
    let gpus = tifmrcview_lib::multi_gpu::gpus(&primary);
    let cards = if std::env::var_os("N2N_ONE_CARD").is_some() { &gpus.devs[..1] } else { &gpus.devs[..] };
    eprintln!("{} card(s)", cards.len());
    let cfg = TrainConfig { steps, ..Default::default() };
    let holdout = |s: &Series| -> Vec<(Vec<f32>, Option<Vec<f32>>, usize, usize)> { s.held.iter().map(|(a, b)| (a.clone(), Some(b.clone()), s.hw, s.h)).collect() };
    let run = |images: &[Img], cfg: &TrainConfig, how: Fitting| -> Result<(Fit, f64)> {
        let t = Instant::now();
        let f = fit(cards, images, Mode::Frames, cfg, &Fitting { scale: Some((0.0, 1.0)), ..how }, |_, _, _| {}, &mut |_, _, _, _, _| {})?;
        Ok((f, t.elapsed().as_secs_f64()))
    };
    // Score of `net` on series `s` (relative to doing nothing).
    let score = |net: &tifmrcview_lib::denoise_learn::Net, s: &Series| -> Result<f64> {
        let one = TrainConfig { steps: 0, ..cfg.clone() };
        let f = fit(&cards[..1], &imgs(s)[..1], Mode::Frames, &one, &Fitting { scale: Some((0.0, 1.0)), start: Some(net.clone()), holdout: holdout(s), ..Default::default() }, |_, _, _| {}, &mut |_, _, _, _, _| {})?;
        Ok(f.holdout.unwrap() as f64 / s.identity)
    };
    let mut rows: Vec<(String, String, f64, usize, f64)> = vec![];
    let all: Vec<Img> = series.iter().flat_map(|s| imgs(s)).collect();
    let mut pooled = None;
    for m in &methods {
        match m.as_str() {
            "sep" | "sep_signal" | "sep_val" | "sep_both" => {
                for s in &series {
                    let how = Fitting { signal: m == "sep_signal" || m == "sep_both", signal_keep: 0.6, validate: m == "sep_val" || m == "sep_both", patience: 8, holdout: holdout(s), ..Default::default() };
                    let (f, t) = run(&imgs(s), &cfg, how)?;
                    rows.push((m.clone(), s.name.clone(), f.holdout.unwrap() as f64 / s.identity, f.steps, t));
                }
            }
            "pool" | "pool_long" => {
                let c = TrainConfig { steps: if m == "pool" { steps } else { steps * series.len() }, ..cfg.clone() };
                let (f, t) = run(&all, &c, Fitting::default())?;
                for s in &series {
                    rows.push((m.clone(), s.name.clone(), score(&f.net, s)?, f.steps, t));
                }
                if m == "pool" {
                    pooled = Some(f.net);
                }
            }
            "pool_ft" => {
                let base = match pooled.clone() {
                    Some(n) => n,
                    None => run(&all, &cfg, Fitting::default())?.0.net,
                };
                for s in &series {
                    let c = TrainConfig { steps: (steps / 4).max(20), lr: cfg.lr / 10.0, ..cfg.clone() };
                    let (f, t) = run(&imgs(s), &c, Fitting { start: Some(base.clone()), holdout: holdout(s), ..Default::default() })?;
                    rows.push((m.clone(), s.name.clone(), f.holdout.unwrap() as f64 / s.identity, f.steps, t));
                }
            }
            "loo" => {
                for (i, s) in series.iter().enumerate() {
                    let others: Vec<Img> = series.iter().enumerate().filter(|(j, _)| *j != i).flat_map(|(_, o)| imgs(o)).collect();
                    if others.is_empty() {
                        continue;
                    }
                    let (f, t) = run(&others, &cfg, Fitting::default())?;
                    rows.push((m.clone(), s.name.clone(), score(&f.net, s)?, f.steps, t));
                }
            }
            other => eprintln!("unknown method {other}"),
        }
        for r in rows.iter().filter(|r| &r.0 == m) {
            println!("{:<10} {:<28} score {:.4}  steps {:>5}  {:.1} s", r.0, r.1, r.2, r.3, r.4);
        }
    }
    println!("\nmean score per method (lower is better):");
    for m in &methods {
        let v: Vec<f64> = rows.iter().filter(|r| &r.0 == m).map(|r| r.2).collect();
        if !v.is_empty() {
            println!("  {:<10} {:.4}", m, v.iter().sum::<f64>() / v.len() as f64);
        }
    }
    Ok(())
}
