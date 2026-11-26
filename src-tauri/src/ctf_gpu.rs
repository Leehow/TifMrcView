//! GPU side of the CTF fit (Pro), with Burn on the device SAM uses
//! (Vulkan, CUDA, DX12 or Metal, see `tmv_sam::backends`): the tiled power spectrum as
//! batched DFT matrix products, and the scores of many CTF models at once.
//! `ctf` falls back to its CPU code when there is no GPU or a kernel fails.

use std::f64::consts::PI;
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use burn::tensor::{Device, Tensor, TensorData};

use crate::ctf::{CpuGrid, Grid, Model, Optics, Px};
use crate::fft;

/// Largest tensor the code builds, in elements (f32), to bound GPU memory.
const MAX_ELEMS: usize = 1 << 24;

static INIT_FAILURE:OnceLock<String>=OnceLock::new();
/// The GPU and its name, only after a real kernel succeeds.
/// While a job keeps to chosen cards (`resources::keep`), its own card.
pub fn device()->Option<&'static(Device,&'static str)>{
 let base=device_first()?;
 if base.1=="CUDA"{if let Some(&i)=crate::resources::kept().as_ref().and_then(|k|k.first()){if i!=0{if let Some(c)=card(i){return Some(c);}}}}
 Some(base)
}
/// CUDA card `i` (a number `device` gives a job), once it ran a small computation.
fn card(i:usize)->Option<&'static(Device,&'static str)>{
 static CARDS:std::sync::Mutex<Vec<(usize,Option<&'static(Device,&'static str)>)>>=std::sync::Mutex::new(Vec::new());
 let mut cards=CARDS.lock().unwrap_or_else(|e|e.into_inner());
 if let Some((_,c))=cards.iter().find(|(n,_)|*n==i){return *c;}
 let made=tmv_sam::cuda_device(i).and_then(|d|{let ok=guarded(||{let x=Tensor::<2>::ones([8,8],&d);let got:f32=x.clone().matmul(x).sum().into_scalar();got==512.});match ok{Ok(true)=>Some(&*Box::leak(Box::new((d,"CUDA")))),other=>{eprintln!("GPU: CUDA device {i} not usable: {other:?}");None}}});
 cards.push((i,made));made
}
/// The process's first GPU, whatever card a job keeps to.
pub fn device_first()->Option<&'static(Device,&'static str)>{
 static GPU:OnceLock<Option<(Device,&'static str)>>=OnceLock::new();
 GPU.get_or_init(||{let mut failures=vec![];for b in tmv_sam::backends(){if b.name=="CPU"{continue;}
  let tried=guarded(||->Result<Device>{let d=b.device()?;let x=Tensor::<2>::ones([8,8],&d);let got:f32=x.clone().matmul(x).sum().into_scalar();anyhow::ensure!(got==512.,"初始化校验返回 {got}，预期 512");Ok(d)}).and_then(|r|r);
  match tried{Ok(d)=>{eprintln!("GPU: using {}",b.name);return Some((d,b.name));},Err(e)=>failures.push(format!("{} 初始化失败：{e:#}",b.name))}
 }
 let reason=if failures.is_empty(){if std::env::var("TMV_GPU").as_deref()==Ok("cpu"){"已指定 CPU 后端，未尝试 GPU".into()}else{"没有可供当前程序使用的 GPU 后端".into()}}else{failures.join("；")};let _=INIT_FAILURE.set(reason);None}).as_ref()
}
pub fn unavailable_reason()->Option<&'static str>{INIT_FAILURE.get().map(String::as_str)}

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
        let (fr,fi)=burn::signal::rfft(x,2,None);
        let (fr,fi)=burn::signal::cfft(fr,fi,1,None);
        let half=download((fr.clone()*fr+fi.clone()*fi).sum_dim(0));
        // Reconstruct the redundant half using conjugate symmetry.
        let hw=n/2+1;let mut p=vec![0.0;n*n];
        for y in 0..n {for x in 0..n {p[y*n+x]=if x<hw {half[y*hw+x]}else{half[((n-y)%n)*hw+n-x]};}}
        // p[u][v] is frequency (row v, column u).
        for u in 0..n {
            for v in 0..n {
                sum[v * n + u] += p[v * n + u] as f64;
            }
        }
    }
    Ok(fft::finish(&sum, n, origins.len()))
}

/// Grid search on the GPU. All spectra share the bin geometry, so each
/// candidate's CTF² is computed once and scored against every spectrum
/// with one matrix product: V [models × bins] · [w, w·x₁ … w·x_S]. Each
/// spectrum then takes the best of everyone's candidates, which also
/// helps a tilt whose own 1D search was misled.
pub struct GpuGrid<'d>(pub &'d Device);

impl Grid for GpuGrid<'_> {
    fn best(&self, px: &[&[Px]], o: Optics, cands: &[Vec<Model>]) -> Vec<Model> {
        let dev = self.0;
        let b = px[0].len();
        let same = px.iter().all(|q| q.len() == b && q.iter().zip(px[0]).all(|(a, c)| a.r == c.r && a.c2 == c.c2));
        if !same || b == 0 {
            return CpuGrid.best(px, o, cands);
        }
        let g0 = px[0];
        let ns = px.len();
        let ak2: Vec<f32> = g0.iter().map(|q| (o.a * q.k2 as f64) as f32).collect();
        let ac = g0.iter().zip(&ak2).map(|(q, &a)| a * q.c2).collect();
        let as_ = g0.iter().zip(&ak2).map(|(q, &a)| a * q.s2).collect();
        let bk = g0.iter().map(|q| (o.b * (q.k2 as f64).powi(2) - o.w) as f32).collect();
        let (ak2, ac, as_, bk) = (upload(ak2, [1, b], dev), upload(ac, [1, b], dev), upload(as_, [1, b], dev), upload(bk, [1, b], dev));
        // [w, w·x per spectrum], bins down the rows.
        let mut wx = vec![0f32; b * (ns + 1)];
        for i in 0..b {
            wx[i * (ns + 1)] = g0[i].w;
            for (s, q) in px.iter().enumerate() {
                wx[i * (ns + 1) + s + 1] = q[i].w * q[i].x;
            }
        }
        let wx = upload(wx, [b, ns + 1], dev);
        let w = wx.clone().narrow(1, 0, 1);
        let all: Vec<Model> = cands.iter().flatten().copied().collect();
        let per = (MAX_ELEMS / b).max(1);
        let mut parts = Vec::new();
        for chunk in all.chunks(per) {
            let m = chunk.len();
            let col = |f: fn(&Model) -> f64| upload(chunk.iter().map(|x| f(x) as f32).collect(), [m, 1], dev);
            let (mean, p, q, ph) = (col(|x| x.mean), col(|x| x.p), col(|x| x.q), col(|x| x.phase));
            // γ = a·Δf(θ)·k² − b·k⁴ + w + φ, reduced mod π (CTF² has period π)
            // so the GPU's sin is only asked about small angles.
            let g = mean * ak2.clone() + p * ac.clone() + q * as_.clone() - bk.clone() + ph;
            let g = g.clone() - (g.div_scalar(PI as f32)).round().mul_scalar(PI as f32);
            let s = g.sin();
            let v = s.clone() * s;
            parts.push(Tensor::cat(vec![(v.clone() * v.clone()).matmul(w.clone()), v.matmul(wx.clone())], 1));
        }
        // One read back for everything: per model [Σwv², Σwv, Σwvx₁ … Σwvx_S].
        let sums = download(Tensor::cat(parts, 0));
        let cols = ns + 2;
        let sw: f64 = g0.iter().map(|q| q.w as f64).sum();
        (0..ns)
            .map(|s| {
                let (sx, sxx) = px[s].iter().fold((0f64, 0f64), |(a, c), q| (a + (q.w * q.x) as f64, c + (q.w * q.x * q.x) as f64));
                let score = |i: usize| {
                    let r = &sums[i * cols..(i + 1) * cols];
                    crate::ctf::corr(sw, sx, r[1] as f64, sxx, r[0] as f64, r[s + 2] as f64)
                };
                all[(0..all.len()).max_by(|&a, &c| score(a).total_cmp(&score(c))).unwrap()]
            })
            .collect()
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
            let gpu = crate::ctf::fit_many(&[&s], 256, &p, &GpuGrid(&dev)).unwrap().pop().unwrap();
            assert!((cpu.df1 - gpu.df1).abs() < 30.0 && (cpu.df2 - gpu.df2).abs() < 30.0, "cpu {cpu:?} gpu {gpu:?}");
            assert!((cpu.angle - gpu.angle).abs() < 2.0 && (cpu.phase - gpu.phase).abs() < 3.0);
            assert!((cpu.df1 - 13000.0).abs() < 130.0 && (gpu.df2 - 12200.0).abs() < 130.0);
        }
    }
}

/// CPU against GPU on a 4096² synthetic micrograph:
/// cargo test --release --features pro --lib ctf_gpu::bench -- --ignored --nocapture
#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore]
    fn bench() {
        let p = crate::ctf::tests::params_pub();
        let size = 4096;
        let img = crate::ctf::tests::ctf_image(size, &p, 14000.0, 400.0, 50.0, 7);
        let values = |x: usize, y: usize| img[y * size + x];
        let t = Instant::now();
        let dev = device();
        eprintln!("device: {:?} in {:?}", dev.map(|d| d.1), t.elapsed());
        for n in [512, 1024] {
            for round in 0..3 {
                let t = Instant::now();
                let s = fft::spectrum(values, [0, 0, size, size], n).unwrap();
                let ts = t.elapsed();
                let f = crate::ctf::fit_spectrum(&s.log, s.n, &p).unwrap();
                eprintln!("n {n} #{round} CPU: spectrum {ts:?}, total {:?}: df {:.0}/{:.0} {:.1}°", t.elapsed(), f.df1, f.df2, f.angle);
                if let Some((d, name)) = dev {
                    let t = Instant::now();
                    let s = spectrum(d, values, [0, 0, size, size], n).unwrap();
                    let ts = t.elapsed();
                    let f = crate::ctf::fit_many(&[&s.log], s.n, &p, &GpuGrid(d)).unwrap().pop().unwrap();
                    eprintln!("n {n} #{round} {name}: spectrum {ts:?}, total {:?}: df {:.0}/{:.0} {:.1}°", t.elapsed(), f.df1, f.df2, f.angle);
                }
            }
        }
    }

    /// A 41-slice tilt series of 512² spectra, grid search on the CPU and the GPU.
    #[test]
    #[ignore]
    fn stack() {
        let p = crate::ctf::tests::params_pub();
        let specs: Vec<Vec<f32>> = (0..41).map(|i| crate::ctf::tests::synth_pub(512, &p, 15000.0 + 120.0 * (i as f64 - 20.0), 14400.0 + 120.0 * (i as f64 - 20.0), 40.0, 0.0, 60.0, 0.5, i)).collect();
        let refs: Vec<&[f32]> = specs.iter().map(|s| &s[..]).collect();
        let Some((d, name)) = device() else { return eprintln!("no GPU") };
        for round in 0..3 {
            let t = Instant::now();
            let a = crate::ctf::fit_many(&refs, 512, &p, &crate::ctf::CpuGrid).unwrap();
            let tc = t.elapsed();
            let t = Instant::now();
            let b = crate::ctf::fit_many(&refs, 512, &p, &GpuGrid(d)).unwrap();
            let tg = t.elapsed();
            let worst = a.iter().zip(&b).map(|(x, y)| (x.df1 - y.df1).abs().max((x.df2 - y.df2).abs())).fold(0f64, f64::max);
            eprintln!("#{round} 41 slices: CPU {tc:?}, {name} {tg:?}, largest df difference {worst:.0} Å");
        }
    }

    /// Fits real data, CPU and GPU: TMV_CTF_FILE=x.mrc [TMV_CTF_PIXEL=Å]
    /// [TMV_CTF_KV=300] cargo test --release --features pro --lib ctf_gpu::bench::file -- --ignored --nocapture
    /// Fits up to 5 slices spread through a stack (tilt series).
    #[test]
    #[ignore]
    fn file() {
        let path = std::env::var("TMV_CTF_FILE").unwrap();
        let img = crate::image::OpenImage::new(1, crate::formats::open(std::path::Path::new(&path)).unwrap(), 1 << 30);
        let info = img.info();
        let unit_a = match info.unit.as_str() {
            "nm" => 10.0,
            "µm" | "um" => 1e4,
            _ => 1.0,
        };
        let px = std::env::var("TMV_CTF_PIXEL").ok().and_then(|v| v.parse().ok()).or(info.pixel_size.map(|p| p[0] * unit_a)).expect("pixel size");
        let mut p = crate::ctf::tests::params_pub();
        p.pixel_a = px;
        p.voltage_kv = std::env::var("TMV_CTF_KV").ok().and_then(|v| v.parse().ok()).unwrap_or(300.0);
        p.res_high_a = (2.5 * px).max(4.0);
        p.df_min_a = 3000.0;
        eprintln!("{path}: {} × {} × {}, pixel {px:.3} Å", info.width, info.height, info.size_z);
        let dev = device();
        let zs: Vec<usize> = if info.size_z <= 5 { (0..info.size_z).collect() } else { (0..5).map(|i| i * (info.size_z - 1) / 4).collect() };
        for z in zs {
            for (name, backend) in [("CPU", "cpu"), ("auto", "auto"), ("GPU", "gpu")] {
                if backend != "cpu" && dev.is_none() {
                    continue;
                }
                let mut q = p.clone();
                q.backend = Some(backend.to_string());
                let req = crate::ctf::FitReq { token: 1, z, t: 0, c: 0, region: [0.0, 0.0, info.width as f64, info.height as f64], n: 512, params: q };
                match crate::ctf::fit_region(&img, &req) {
                    Ok(f) => eprintln!("z {z} {name} ({}): {:.0} ms, df {:.0}/{:.0} Å {:.1}°, fit {:.2} Å, cc {:.3}", f.backend, f.ms, f.df1, f.df2, f.angle, f.fit_res, f.score),
                    Err(e) => eprintln!("z {z} {name}: {e:#}"),
                }
            }
        }
    }
}

#[cfg(test)]mod fft_tests{use super::*;
 #[test]fn native_fft_spectrum_matches_cpu(){
  let Some((dev,_))=device()else{assert!(std::env::var_os("TMV_REQUIRE_GPU").is_none(),"GPU required");return;};let value=|x:usize,y:usize|((x*37+y*113)%251) as f32/125.+(x as f32*0.17).sin();let region=[0,0,256,256];let cpu=fft::spectrum(value,region,128).unwrap();let gpu=spectrum(dev,value,region,128).unwrap();let rms=(gpu.log.iter().zip(&cpu.log).map(|(a,b)|(*a as f64-*b as f64).powi(2)).sum::<f64>()/gpu.log.len() as f64).sqrt();eprintln!("GPU CTF log spectrum RMS {rms}");assert!(rms<0.002,"{rms}");
 }
}
