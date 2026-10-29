//! GPU side of the CTF fit (Pro), with Burn on the device SAM uses
//! (Metal, DX12 or Vulkan through wgpu): the tiled power spectrum as
//! batched DFT matrix products, and the scores of many CTF models at once.
//! `ctf` falls back to its CPU code when there is no GPU or a kernel fails.

use std::f64::consts::PI;
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use burn::tensor::{Device, Tensor, TensorData};

use crate::ctf::{Model, Optics, Px, Scorer};
use crate::fft;

/// Largest tensor the code builds, in elements (f32), to bound GPU memory.
const MAX_ELEMS: usize = 1 << 23;

/// The GPU and its name, or None when only the CPU is available.
pub fn device() -> Option<&'static (Device, &'static str)> {
    static GPU: OnceLock<Option<(Device, &'static str)>> = OnceLock::new();
    GPU.get_or_init(|| {
        let (d, name) = tmv_sam::pick_device();
        (name != "CPU").then_some((d, name))
    })
    .as_ref()
}

/// Runs `f`, turning a panic (how Burn reports a failed kernel) into an error.
pub fn guarded<T>(f: impl FnOnce() -> T) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|e| {
        let msg = e.downcast_ref::<String>().map(String::as_str).or_else(|| e.downcast_ref::<&str>().copied()).unwrap_or("panic");
        anyhow!("GPU: {}", msg.lines().next().unwrap_or(msg))
    })
}

fn upload<const D: usize>(v: Vec<f32>, shape: [usize; D], dev: &Device) -> Tensor<D> {
    Tensor::from_data(TensorData::new(v, shape), dev)
}

fn download<const D: usize>(t: Tensor<D>) -> Vec<f32> {
    t.into_data().convert::<f32>().try_into_vec::<f32>().unwrap()
}

/// The same spectrum as `fft::spectrum`, with the transforms on the GPU.
pub fn spectrum(dev: &Device, values: impl Fn(usize, usize) -> f32 + Sync, region: [usize; 4], want_n: usize) -> Result<fft::Spectrum> {
    use rayon::prelude::*;
    let (n, xs, ys) = fft::tiling(region, want_n)?;
    let origins: Vec<(usize, usize)> = ys.iter().flat_map(|&y| xs.iter().map(move |&x| (x, y))).collect();
    // DFT matrix W[j][k] = exp(−2πi jk / n), with jk reduced mod n for accuracy.
    let (mut wr, mut wi) = (vec![0f32; n * n], vec![0f32; n * n]);
    for j in 0..n {
        for k in 0..n {
            let a = -2.0 * PI * ((j * k) % n) as f64 / n as f64;
            wr[j * n + k] = a.cos() as f32;
            wi[j * n + k] = a.sin() as f32;
        }
    }
    let wr = upload(wr, [n, n], dev);
    let wi = upload(wi, [n, n], dev);
    let win = fft::hann(n);
    let window = upload((0..n * n).map(|i| win[i / n] * win[i % n]).collect(), [1, n, n], dev);
    let per = (MAX_ELEMS / (n * n)).clamp(1, origins.len());
    let mut sum = vec![0f64; n * n];
    for chunk in origins.chunks(per) {
        let t = chunk.len();
        let mut tiles = vec![0f32; t * n * n];
        tiles.par_chunks_mut(n * n).zip(chunk).for_each(|(tile, &(tx, ty))| {
            for (i, o) in tile.iter_mut().enumerate() {
                *o = values(tx + i % n, ty + i / n);
            }
            let mean = tile.iter().map(|&v| v as f64).sum::<f64>() / (n * n) as f64;
            for o in tile.iter_mut() {
                *o -= mean as f32;
            }
        });
        let x = upload(tiles, [t, n, n], dev) * window.clone();
        // Rows: A = X W.
        let x2 = x.reshape([t * n, n]);
        let ar = x2.clone().matmul(wr.clone());
        let ai = x2.matmul(wi.clone());
        // Columns on the transposed tiles: Aᵀ W = (W A)ᵀ, so the result is Fᵀ.
        let tr = |a: Tensor<2>| a.reshape([t, n, n]).swap_dims(1, 2).reshape([t * n, n]);
        let (ar, ai) = (tr(ar), tr(ai));
        let fr = ar.clone().matmul(wr.clone()) - ai.clone().matmul(wi.clone());
        let fi = ar.matmul(wi.clone()) + ai.matmul(wr.clone());
        let power = (fr.clone() * fr + fi.clone() * fi).reshape([t, n, n]).sum_dim(0);
        let p = download(power);
        // p[u][v] is frequency (row v, column u).
        for u in 0..n {
            for v in 0..n {
                sum[v * n + u] += p[u * n + v] as f64;
            }
        }
    }
    Ok(fft::finish(&sum, n, origins.len()))
}

/// Pixel constants on the GPU, for scoring batches of models.
pub struct GpuScorer {
    dev: Device,
    /// a·k², a·k²·cos 2θ, a·k²·sin 2θ, b·k⁴ − w: each [1, P].
    ak2: Tensor<2>,
    ac: Tensor<2>,
    as_: Tensor<2>,
    bk: Tensor<2>,
    /// The spectrum values as a column [P, 1].
    x: Tensor<2>,
    count: f64,
    sx: f64,
    sxx: f64,
}

impl GpuScorer {
    pub(crate) fn new(dev: &Device, px: &[Px], o: Optics) -> GpuScorer {
        let p = px.len();
        let ak2: Vec<f32> = px.iter().map(|q| (o.a * q.k2 as f64) as f32).collect();
        let ac = px.iter().zip(&ak2).map(|(q, &a)| a * q.c2).collect();
        let as_ = px.iter().zip(&ak2).map(|(q, &a)| a * q.s2).collect();
        let bk = px.iter().map(|q| (o.b * (q.k2 as f64).powi(2) - o.w) as f32).collect();
        let xs: Vec<f32> = px.iter().map(|q| q.x).collect();
        let sx = xs.iter().map(|&v| v as f64).sum();
        let sxx = xs.iter().map(|&v| (v as f64).powi(2)).sum();
        GpuScorer {
            dev: dev.clone(),
            ak2: upload(ak2, [1, p], dev),
            ac: upload(ac, [1, p], dev),
            as_: upload(as_, [1, p], dev),
            bk: upload(bk, [1, p], dev),
            x: upload(xs, [p, 1], dev),
            count: p as f64,
            sx,
            sxx,
        }
    }

    fn run(&self, models: &[Model]) -> Vec<f64> {
        let m = models.len();
        let col = |f: fn(&Model) -> f64| upload(models.iter().map(|x| f(x) as f32).collect(), [m, 1], &self.dev);
        let (mean, p, q, ph) = (col(|x| x.mean), col(|x| x.p), col(|x| x.q), col(|x| x.phase));
        // γ = a·Δf(θ)·k² − b·k⁴ + w + φ, reduced mod π (CTF² has period π)
        // so the GPU's sin is only asked about small angles.
        let g = mean * self.ak2.clone() + p * self.ac.clone() + q * self.as_.clone() - self.bk.clone() + ph;
        let g = g.clone() - (g.div_scalar(PI as f32)).round().mul_scalar(PI as f32);
        let s = g.sin();
        let v = s.clone() * s;
        let sv = download(v.clone().sum_dim(1));
        let svv = download((v.clone() * v.clone()).sum_dim(1));
        let sxv = download(v.matmul(self.x.clone()));
        (0..m).map(|i| crate::ctf::corr(self.count, self.sx, sv[i] as f64, self.sxx, svv[i] as f64, sxv[i] as f64)).collect()
    }
}

impl Scorer for GpuScorer {
    fn scores(&self, models: &[Model]) -> Vec<f64> {
        let per = (MAX_ELEMS / (self.count as usize).max(1)).max(1);
        models.chunks(per).flat_map(|c| self.run(c)).collect()
    }
    fn batched(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctf::tests::{params_pub, synth_pub};

    /// The tensor code on Burn's CPU backend, against the CPU implementation.
    #[test]
    fn spectrum_matches_the_cpu() {
        let dev = Device::flex();
        // Hash noise plus a lattice, like a micrograph with a crystal in it.
        let f = |x: usize, y: usize| {
            let h = ((x as u64 * 73_856_093) ^ (y as u64 * 19_349_663)).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40;
            (h % 1000) as f32 / 100.0 + (0.3 * x as f32).sin() * 5.0
        };
        let a = fft::spectrum(f, [3, 1, 300, 200], 128).unwrap();
        let b = spectrum(&dev, f, [3, 1, 300, 200], 128).unwrap();
        assert_eq!((a.n, a.tiles), (b.n, b.tiles));
        // Compare powers relative to the strongest: bins that are almost
        // empty differ a lot in log10 from rounding alone.
        let pw = |l: &f32| 10f64.powf(*l as f64);
        let top = a.log.iter().map(pw).fold(0f64, f64::max);
        let worst = a.log.iter().zip(&b.log).map(|(x, y)| (pw(x) - pw(y)).abs() / top).fold(0f64, f64::max);
        assert!(worst < 1e-4, "power differs by {worst} of the peak");
        let close = a.log.iter().zip(&b.log).filter(|(x, y)| (*x - *y).abs() < 0.01).count();
        assert!(close as f64 > 0.95 * a.log.len() as f64, "{close} of {} bins agree", a.log.len());
    }

    #[test]
    fn fit_matches_the_cpu() {
        let dev = Device::flex();
        let mut p = params_pub();
        for phase in [false, true] {
            p.phase = phase;
            let s = synth_pub(256, &p, 13000.0, 12200.0, 70.0, if phase { 60.0 } else { 0.0 }, 60.0, 0.4, 21);
            let cpu = crate::ctf::fit_spectrum(&s, 256, &p).unwrap();
            let gpu = crate::ctf::fit_with(&s, 256, &p, &|px, o, _| Box::new(GpuScorer::new(&dev, px, o))).unwrap();
            assert!((cpu.df1 - gpu.df1).abs() < 30.0 && (cpu.df2 - gpu.df2).abs() < 30.0, "cpu {cpu:?} gpu {gpu:?}");
            assert!((cpu.angle - gpu.angle).abs() < 2.0 && (cpu.phase - gpu.phase).abs() < 3.0);
            assert!((cpu.df1 - 13000.0).abs() < 130.0 && (gpu.df2 - 12200.0).abs() < 130.0);
        }
    }
}
