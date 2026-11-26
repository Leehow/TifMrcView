//! Several GPUs for one job (Pro). A network is trained on every free card
//! at once: each card takes its share of the batch, the gradients are added
//! on the first card, which takes the optimiser step, so the result is the
//! same as on one card with the whole batch. A trained network is applied
//! with the tiles or sections handed out to the cards as they come free.
//!
//! The first card is the one `ctf_gpu::device` picked; the others are the
//! further cards of the same backend (CUDA devices 1.., further discrete
//! Vulkan adapters) that answer a small computation, have at least
//! `MIN_FREE_MB` free and are not held by another TifMrcView job (the same
//! lock files as `resources::try_lease`). A GPU worker process (one card
//! per series or movie, `gpu_workers`) sees only its own card, so work that
//! is already spread over the cards is not spread again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Result};
use burn::module::Module;
use burn::optim::{GradientsAccumulator, GradientsParams};
use burn::tensor::{Device, Tensor};

/// Least free memory a further card must have to join, MB.
const MIN_FREE_MB: u64 = 4096;

/// The cards one job uses, the primary first, held for the job (a further
/// card's lock is released when this is dropped).
pub struct Gpus {
    pub devs: Vec<Device>,
    _leases: Vec<crate::resources::Lease>,
}

impl Gpus {
    pub fn one(dev: &Device) -> Gpus {
        Gpus { devs: vec![dev.clone()], _leases: vec![] }
    }
    pub fn len(&self) -> usize {
        self.devs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.devs.is_empty()
    }
    /// The cards with autodiff, for training.
    pub fn autodiff(&self) -> Vec<Device> {
        self.devs.iter().map(|d| d.clone().autodiff()).collect()
    }
    /// What to show: "2 块 GPU" or nothing for one.
    pub fn label(&self) -> Option<String> {
        (self.len() > 1).then(|| format!("{} 块 GPU", self.len()))
    }
}

/// The cards of the backend in use that ran a small computation (tried
/// once per process), by number (CUDA: as `resources::discover` lists
/// them, the process's first card 0 included; Vulkan: the further discrete
/// adapters 1..), with their nvidia-smi entry where there is one.
fn extras() -> &'static [(usize, Device, Option<crate::resources::Gpu>)] {
    static EXTRAS: OnceLock<Vec<(usize, Device, Option<crate::resources::Gpu>)>> = OnceLock::new();
    EXTRAS.get_or_init(|| {
        if std::env::var_os("TMV_GPU_WORKER").is_some() || std::env::var_os("TMV_ONE_GPU").is_some() || std::env::var("TMV_GPU").as_deref() == Ok("cpu") {
            return vec![];
        }
        let Some((first, name)) = crate::ctf_gpu::device_first() else { return vec![] };
        let listed = if *name == "CUDA" { crate::resources::discover() } else { vec![] };
        let mut cards = vec![];
        if *name == "CUDA" {
            cards.push((0, first.clone(), listed.first().cloned()));
        }
        cards.extend(tmv_sam::more_devices(name, listed.len()).into_iter().enumerate().filter_map(|(i, d)| {
            let entry = listed.get(i + 1).cloned();
            // A card another job fills is not even started on.
            if entry.as_ref().is_some_and(|g| g.free_mb < MIN_FREE_MB) {
                return None;
            }
            let works = crate::ctf_gpu::guarded(|| {
                let x = Tensor::<2>::ones([8, 8], &d);
                let s: f32 = x.clone().matmul(x).sum().into_scalar();
                s == 512.0
            });
            match works {
                Ok(true) => {
                    eprintln!("GPU: {name} device {} also usable", i + 1);
                    Some((i + 1, d, entry))
                }
                other => {
                    eprintln!("GPU: {name} device {} not usable: {other:?}", i + 1);
                    None
                }
            }
        }));
        cards
    })
}

/// The cards for a job on `primary`: it, plus each further card that is
/// free now (and, when the job keeps to chosen cards, `resources::keep`,
/// is one of them). Only `primary` when it is not the GPU (the CPU fallback).
pub fn gpus(primary: &Device) -> Gpus {
    let mut g = Gpus::one(primary);
    let Some((_, name)) = crate::ctf_gpu::device().filter(|(d, _)| d == primary) else {
        return g;
    };
    let extras = extras();
    if extras.is_empty() {
        return g;
    }
    let kept = if *name == "CUDA" { crate::resources::kept() } else { None };
    let now = crate::resources::discover();
    for (i, d, entry) in extras {
        if d == primary || kept.as_ref().is_some_and(|k| !k.contains(i)) {
            continue;
        }
        if let Some(e) = entry {
            let free = now.iter().find(|n| n.uuid == e.uuid).map_or(0, |n| n.free_mb);
            if free < MIN_FREE_MB {
                continue;
            }
            match crate::resources::try_lease(e) {
                Ok(Some(l)) => g._leases.push(l),
                _ => continue,
            }
        }
        g.devs.push(d.clone());
    }
    g
}

/// How many of `n` samples card k of `cards` takes (as even as can be, the
/// first cards one more).
pub fn share(n: usize, cards: usize, k: usize) -> usize {
    n / cards + usize::from(k < n % cards)
}

/// The gradients of one training step with the batch split over `ads` (the
/// cards with autodiff, the primary first; `net` lives on the primary).
/// `parts[k]` is card k's share of the batch and how many samples it holds
/// (0: the card sits this step out); `loss(net, part, k)` builds that
/// share's mean loss on card k. The gradients are those of the whole
/// batch's mean loss, on the primary, with that loss.
pub fn step_grads<M, P>(net: &M, ads: &[Device], parts: Vec<(P, usize)>, loss: &(dyn Fn(&M, P, usize) -> Tensor<1> + Sync)) -> (GradientsParams, f32)
where
    M: Module + Clone + Send,
    P: Send,
{
    let total = parts.iter().map(|p| p.1).sum::<usize>().max(1);
    let mut parts = parts.into_iter();
    let first = parts.next().expect("one part per card");
    if ads.len() == 1 {
        let l = loss(net, first.0, 0);
        let v: f32 = l.clone().into_scalar();
        return (GradientsParams::from_grads(l.backward(), net), v);
    }
    let primary = ads[0].clone().inner();
    std::thread::scope(|s| {
        let handles: Vec<_> = parts
            .enumerate()
            .filter(|(_, p)| p.1 > 0)
            .map(|(i, (p, n))| {
                let k = i + 1;
                let (replica, ad, primary) = (net.clone(), ads[k].clone(), primary.clone());
                s.spawn(move || {
                    let r = replica.fork(&ad);
                    let l = loss(&r, p, k) * (n as f32 / total as f32);
                    let v: f32 = l.clone().into_scalar();
                    (GradientsParams::from_grads(l.backward(), &r).to_device(&primary, &r), v)
                })
            })
            .collect();
        let mut acc = GradientsAccumulator::<M>::new();
        let mut lv = 0f32;
        if first.1 > 0 {
            let l = loss(net, first.0, 0) * (first.1 as f32 / total as f32);
            lv += l.clone().into_scalar::<f32>();
            acc.accumulate(net, GradientsParams::from_grads(l.backward(), net));
        }
        for h in handles {
            match h.join() {
                Ok((g, v)) => {
                    lv += v;
                    acc.accumulate(net, g);
                }
                // A failed kernel on another card fails the step as one on
                // the primary would (callers catch it with `guarded`).
                Err(e) => std::panic::resume_unwind(e),
            }
        }
        (acc.grads(), lv)
    })
}

/// Does `work(k, item)` for every item, card k taking the next item as it
/// comes free (one thread per card); `got` receives the results on this
/// thread as they finish. Stops at the first error (of `work` or `got`).
pub fn spread<T, R>(cards: usize, items: Vec<T>, work: &(dyn Fn(usize, T) -> Result<R> + Sync), got: &mut dyn FnMut(R) -> Result<()>) -> Result<()>
where
    T: Send,
    R: Send,
{
    if cards <= 1 {
        for item in items {
            got(work(0, item)?)?;
        }
        return Ok(());
    }
    let queue = Mutex::new(items.into_iter());
    let stop = AtomicBool::new(false);
    let (send, recv) = std::sync::mpsc::sync_channel::<Result<R>>(cards);
    std::thread::scope(|s| {
        for k in 0..cards {
            let (queue, stop, send) = (&queue, &stop, send.clone());
            s.spawn(move || loop {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Some(item) = queue.lock().unwrap_or_else(|e| e.into_inner()).next() else { break };
                let r = crate::ctf_gpu::guarded(|| work(k, item)).and_then(|r| r);
                let failed = r.is_err();
                if send.send(r).is_err() || failed {
                    break;
                }
            });
        }
        drop(send);
        let mut first_err = None;
        for r in recv {
            if first_err.is_some() {
                continue;
            }
            if let Err(e) = r.and_then(|v| got(v)) {
                stop.store(true, Ordering::SeqCst);
                first_err = Some(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    })
    .map_err(|e: anyhow::Error| anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use burn::nn::{Linear, LinearConfig};
    use burn::tensor::{Distribution, TensorData};

    #[test]
    fn shares_cover_the_batch() {
        for (n, c) in [(4, 2), (5, 2), (3, 4), (8, 3)] {
            assert_eq!((0..c).map(|k| share(n, c, k)).sum::<usize>(), n);
        }
    }

    /// Two "cards" that are the same CPU device: the split batch gives the
    /// gradients of the whole batch.
    #[test]
    fn split_batch_gives_the_whole_batchs_gradients() {
        let ad = Device::flex().autodiff();
        let net: Linear = LinearConfig::new(6, 3).init(&ad);
        let xs = Tensor::<2>::random([5, 6], Distribution::Default, &ad).into_data().convert::<f32>().to_vec::<f32>().unwrap();
        let loss = |m: &Linear, rows: Vec<f32>, k: usize| {
            let ads = [ad.clone(), ad.clone()];
            let n = rows.len() / 6;
            let x = Tensor::<2>::from_data(TensorData::new(rows, [n, 6]), &ads[k]);
            let y = m.forward(x);
            (y.clone() * y).sum() / (n * 3) as f32
        };
        let (whole, lw) = step_grads(&net, &[ad.clone()], vec![(xs.clone(), 5)], &loss);
        let parts = vec![(xs[..18].to_vec(), 3), (xs[18..].to_vec(), 2)];
        let (split, ls) = step_grads(&net, &[ad.clone(), ad.clone()], parts, &loss);
        assert!((lw - ls).abs() < 1e-5, "{lw} {ls}");
        let a = whole.get::<2>(net.weight.id).unwrap().into_data().convert::<f32>().to_vec::<f32>().unwrap();
        let b = split.get::<2>(net.weight.id).unwrap().into_data().convert::<f32>().to_vec::<f32>().unwrap();
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < 1e-5, "{x} {y}");
        }
    }

    #[test]
    fn spread_hands_every_item_out_once() {
        let mut seen = vec![];
        spread(3, (0..20).collect(), &|k, i: usize| Ok((k, i)), &mut |(_, i)| {
            seen.push(i);
            Ok(())
        })
        .unwrap();
        seen.sort();
        assert_eq!(seen, (0..20).collect::<Vec<_>>());
        let err = spread(2, (0..20).collect(), &|_, i: usize| if i == 5 { anyhow::bail!("five") } else { Ok(i) }, &mut |_| Ok(()));
        assert!(err.is_err());
    }
}
