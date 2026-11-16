//! The method of `sum.rs` on the GPU. Per frame: the tiles are cut on the
//! CPU (a copy), uploaded as one batch, transformed with Burn's native FFT,
//! phase shifted and dose weighted as elementwise ops and added into device
//! accumulators. The tiles come back once, at the end, to be blended.
use super::{
    check_cancel, dose,
    input::Spool,
    local::Model,
    sum::{fractional, freq, nyquist, Sums},
    tiles::Plan,
};
use anyhow::Result;
use burn::tensor::{Device, Tensor, TensorData};

pub fn download<const D: usize>(t: Tensor<D>) -> Vec<f32> {
    t.into_data().convert::<f32>().try_into_vec::<f32>().unwrap()
}

/// Inverse of the batched half-plane transform: [n][t][t/2+1] → [n][t][t].
fn inverse(re: Tensor<3>, im: Tensor<3>, t: usize) -> Tensor<3> {
    let h = t as f32;
    let (re, im) = burn::signal::cfft(re, im.neg(), 1, None);
    burn::signal::irfft(re.div_scalar(h), im.div_scalar(-h), 2, Some(t))
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    dev: &Device,
    sp: &Spool,
    global: &[[f64; 2]],
    model: &Model,
    exposures: Option<&[(f64, f64)]>,
    pixel: f64,
    kv: f64,
    progress: &(dyn Fn(&str, usize, usize) + Sync),
) -> Result<Sums> {
    let plan = Plan::new(sp.w, sp.h);
    let (t, n) = (plan.t, plan.len());
    let hw = t / 2 + 1;
    let up3 = |v: Vec<f32>, shape: [usize; 3]| Tensor::<3>::from_data(TensorData::new(v, shape), dev);
    let kx = up3((0..hw).map(|i| i as f32).collect(), [1, 1, hw]);
    let ky = up3((0..t).map(|i| freq(i, t) as f32).collect(), [1, t, 1]);
    let rates = match exposures {
        Some(_) => {
            let v = (0..t * hw)
                .map(|i| dose::critical(freq(i % hw, t).hypot(freq(i / hw, t)) / t as f64 / pixel, kv).map(|c| (0.5 / c) as f32))
                .collect::<Result<Vec<_>>>()?;
            Some(up3(v, [1, t, hw]))
        }
        None => None,
    };
    let zeros = || Tensor::<3>::zeros([n, t, hw], dev);
    let (mut er, mut ei, mut or, mut oi) = (zeros(), zeros(), zeros(), zeros());
    let (mut wr, mut wi) = (zeros(), zeros());
    let mut norm = Tensor::<3>::zeros([1, t, hw], dev);
    let mut batch = vec![0.0f32; n * t * t];
    for (i, &g) in global.iter().enumerate() {
        check_cancel()?;
        let frame = sp.read(i)?;
        let mean = (frame.iter().map(|&a| a as f64).sum::<f64>() / frame.len() as f64) as f32;
        let shifts: Vec<[f64; 2]> = (0..n).map(|k| plan.shift(k, model, sp.times[i], g)).collect();
        {
            use rayon::prelude::*;
            batch.par_chunks_mut(t * t).enumerate().for_each(|(k, out)| plan.cut(&frame, k, shifts[k], mean, out));
        }
        drop(frame);
        let fracs: Vec<[f64; 2]> = shifts.iter().map(|s| [s[0] - s[0].round(), s[1] - s[1].round()]).collect();
        let ax = up3(fracs.iter().map(|f| (std::f64::consts::TAU * f[0] / t as f64) as f32).collect(), [n, 1, 1]);
        let ay = up3(fracs.iter().map(|f| (std::f64::consts::TAU * f[1] / t as f64) as f32).collect(), [n, 1, 1]);
        // Nyquist bins of fractionally shifted axes drop out (see sum.rs).
        let keep_x = up3(fracs.iter().flat_map(|f| (0..hw).map(move |j| if fractional(f[0]) && nyquist(j, t) { 0.0 } else { 1.0 })).collect(), [n, 1, hw]);
        let keep_y = up3(fracs.iter().flat_map(|f| (0..t).map(move |j| if fractional(f[1]) && nyquist(j, t) { 0.0 } else { 1.0 })).collect(), [n, t, 1]);
        let x = up3(batch.clone(), [n, t, t]);
        let (re, im) = burn::signal::rfft(x, 2, None);
        let (re, im) = burn::signal::cfft(re, im, 1, None);
        let theta = kx.clone() * ax + ky.clone() * ay;
        let keep = keep_x * keep_y;
        let (sn, cs) = (theta.clone().sin() * keep.clone(), theta.cos() * keep);
        let r = re.clone() * cs.clone() - im.clone() * sn.clone();
        let m = re * sn + im * cs;
        if let (Some(e), Some(rate)) = (exposures, &rates) {
            let (start, d) = e[i];
            let z = rate.clone().mul_scalar(d as f32);
            let exact = (z.clone().neg().exp().neg() + 1.0) / z.clone().clamp_min(1e-20);
            let approx = z.clone().mul_scalar(-0.5) + 1.0 + (z.clone() * z.clone()).div_scalar(6.0);
            let q = rate.clone().mul_scalar(-start as f32).exp() * exact.mask_where(z.lower_elem(1e-3), approx);
            wr = wr + r.clone() * q.clone();
            wi = wi + m.clone() * q.clone();
            norm = norm + q.clone() * q;
        }
        if i % 2 == 0 {
            er = er + r;
            ei = ei + m;
        } else {
            or = or + r;
            oi = oi + m;
        }
        progress("求和 GPU", i + 1, sp.len());
    }
    let back = |re: Tensor<3>, im: Tensor<3>| {
        let v = download(inverse(re, im, t));
        let tiles: Vec<Vec<f32>> = v.chunks(t * t).map(<[f32]>::to_vec).collect();
        plan.blend(&tiles)
    };
    let weighted = exposures.map(|_| {
        let scale = norm.sqrt().clamp_min(1e-12).recip().mul_scalar((sp.len() as f32).sqrt());
        back(wr * scale.clone(), wi * scale)
    });
    Ok(Sums {
        unweighted: back(er.clone() + or.clone(), ei.clone() + oi.clone()),
        even: back(er, ei),
        odd: back(or, oi),
        weighted,
    })
}

/// Same separable Catmull–Rom warp as CPU. Batch frames under a 4M-pixel
/// budget; intermediates and half accumulators stay on the GPU until the end.
pub fn preview(dev: &Device, frames: &[Vec<f32>], w: usize, h: usize, shifts: &[[f64; 2]], scale: f32) -> Result<Sums> {
    use burn::tensor::Int;
    let n = w * h;
    let mut even = Tensor::<2>::zeros([1, n], dev);
    let mut odd = even.clone();
    let ids = Tensor::<1, Int>::arange(0..n as i64, dev).reshape([1, n]);
    let x = ids.clone().remainder_scalar(w as i32);
    let y = ids.div_scalar(w as i32);
    let cubic = |t: f32| {
        let (t2, t3) = (t * t, t * t * t);
        [-0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1., -1.5 * t3 + 2. * t2 + 0.5 * t, 0.5 * t3 - 0.5 * t2]
    };
    let per = ((1usize << 22) / n).max(1);
    for first in (0..frames.len()).step_by(per) {
        check_cancel()?;
        let count = per.min(frames.len() - first);
        let mut src = Tensor::<2>::from_data(TensorData::new(frames[first..first + count].iter().flatten().copied().collect::<Vec<_>>(), [count, n]), dev);
        for axis in 0..2 {
            let len = if axis == 0 { w } else { h };
            let ds = &shifts[first..first + count];
            let offsets = Tensor::<2, Int>::from_data(TensorData::new(ds.iter().map(|d| d[axis].floor() as i32).collect::<Vec<_>>(), [count, 1]), dev);
            let fractions: Vec<_> = ds.iter().map(|d| (d[axis] - d[axis].floor()) as f32).collect();
            let upper = Tensor::<2, Int>::from_data(TensorData::new(fractions.iter().map(|f| len as i32 - if *f > 0. { 1 } else { 0 }).collect::<Vec<_>>(), [count, 1]), dev);
            let position = (if axis == 0 { x.clone() } else { y.clone() }) + offsets;
            let valid = position.clone().greater_equal_elem(0).bool_and(position.clone().lower(upper)).float();
            let mut out = Tensor::<2>::zeros([count, n], dev);
            for j in 0..4 {
                let c = position.clone().add_scalar(j as i32 - 1).clamp(0, len as i32 - 1);
                let index = if axis == 0 { y.clone().mul_scalar(w as i32) + c } else { c.mul_scalar(w as i32) + x.clone() };
                let weights = Tensor::<2>::from_data(TensorData::new(fractions.iter().map(|f| cubic(*f)[j]).collect::<Vec<_>>(), [count, 1]), dev);
                out = out + src.clone().gather(1, index) * weights;
            }
            src = out * valid;
        }
        let mask = |parity| Tensor::<2>::from_data(TensorData::new((first..first + count).map(|i| if i % 2 == parity { scale } else { 0. }).collect::<Vec<_>>(), [count, 1]), dev);
        even = even + (src.clone() * mask(0)).sum_dim(0);
        odd = odd + (src * mask(1)).sum_dim(0);
    }
    let unweighted = download(even.clone() + odd.clone());
    anyhow::ensure!(unweighted.iter().all(|v| v.is_finite()), "GPU preview returned non-finite values");
    Ok(Sums { unweighted, even: download(even), odd: download(odd), weighted: None })
}

pub(super) fn cpu_preview(frames: &[Vec<f32>], w: usize, h: usize, shifts: &[[f64; 2]], scale: f32) -> Sums {
    use rayon::prelude::*;
    let moved: Vec<_> = frames.par_iter().zip(shifts).map(|(v, &d)| crate::align::movie_shift(v, w, h, d)).collect();
    let mut even = vec![0.; w * h];
    let mut odd = even.clone();
    for (i, v) in moved.iter().enumerate() {
        let half = if i % 2 == 0 { &mut even } else { &mut odd };
        half.par_iter_mut().zip(v).for_each(|(o, x)| *o += x * scale);
    }
    let unweighted = even.par_iter().zip(&odd).map(|(a, b)| a + b).collect();
    Sums { unweighted, even, odd, weighted: None }
}

/// End-to-end timing includes transfers, synchronization and kernel launches.
/// Calibrate once per process and input shape; never time only the kernel.
pub(super) fn preview_auto(dev: &Device, frames: &[Vec<f32>], w: usize, h: usize, shifts: &[[f64; 2]], scale: f32, force: bool) -> Result<(Sums, bool, Option<String>)> {
    use std::sync::{LazyLock, Mutex};
    static CHOICE: LazyLock<Mutex<std::collections::HashMap<(usize, usize, usize), (bool, String)>>> = LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
    let key = (w, h, frames.len());
    let cached = CHOICE.lock().unwrap().get(&key).cloned();
    if force { return Ok((preview(dev, frames, w, h, shifts, scale)?, true, None)); }
    if let Some((gpu, reason)) = cached {
        return Ok((if gpu { preview(dev, frames, w, h, shifts, scale)? } else { cpu_preview(frames, w, h, shifts, scale) }, gpu, Some(reason)));
    }
    // Warm the actual batch geometry, then compare the complete workload.
    // A tiny sample can miss CPU parallelism or GPU batching effects.
    let warm = frames.len().min(((1usize << 22) / (w*h)).max(1));
    let _ = preview(dev, &frames[..warm], w, h, &shifts[..warm], scale)?;
    let start = std::time::Instant::now();
    let gpu_out = preview(dev, frames, w, h, shifts, scale)?;
    let gpu_ms = start.elapsed().as_secs_f64() * 1000.;
    let start = std::time::Instant::now();
    let cpu_out = cpu_preview(frames, w, h, shifts, scale);
    let cpu_ms = start.elapsed().as_secs_f64() * 1000.;
    let gpu = gpu_ms < cpu_ms;
    let reason = format!("自动选择{}：{}帧全量校准含传输与同步，CPU {cpu_ms:.1} ms，GPU {gpu_ms:.1} ms", if gpu { "GPU" } else { "CPU" }, frames.len());
    CHOICE.lock().unwrap().insert(key, (gpu, reason.clone()));
    Ok((if gpu { gpu_out } else { cpu_out }, gpu, Some(reason)))
}

#[cfg(test)]mod preview_tests{use super::*;
 #[test] fn gpu_preview_batches_preserve_halves_and_auto_choice() {
  let Some((dev,_))=crate::ctf_gpu::device() else {assert!(std::env::var_os("TMV_REQUIRE_GPU").is_none());return;};
  let (w,h)=(1024,1024);let frames:Vec<Vec<f32>>=(0..5).map(|k|(0..w*h).map(|i|((i*13+k*47)%239) as f32/100.).collect()).collect();let shifts=[[0.,0.],[-2.25,1.7],[3.4,-1.2],[-0.5,-0.5],[1.2,2.5]];
  let reference=cpu_preview(&frames,w,h,&shifts,4.);let got=preview(dev,&frames,w,h,&shifts,4.).unwrap();
  for(a,b)in [(&got.unweighted,&reference.unweighted),(&got.even,&reference.even),(&got.odd,&reference.odd)] {let max=a.iter().zip(b).map(|(a,b)|(a-b).abs()).fold(0.,f32::max);assert!(max<0.0001,"{max}");}
  let (out,_,reason)=preview_auto(dev,&frames,w,h,&shifts,4.,false).unwrap();assert!(reason.is_some());assert!(out.unweighted.iter().zip(&reference.unweighted).all(|(a,b)|(a-b).abs()<0.0001));
 }

 #[test]fn gpu_preview_matches_cpu_cubic_and_borders(){
  let Some((dev,_))=crate::ctf_gpu::device()else{assert!(std::env::var_os("TMV_REQUIRE_GPU").is_none(),"GPU required");return;};
  let(w,h)=(96,80);let frames:Vec<Vec<f32>>=(0..4).map(|k|(0..w*h).map(|i|((i*13+k*47)%239) as f32/100.+((i%w) as f32*0.15).sin()).collect()).collect();let shifts=[[0.,0.],[-2.25,1.7],[3.4,-1.2],[-0.5,-0.5]];let scale=4.;let got=preview(dev,&frames,w,h,&shifts,scale).unwrap();let mut expected=vec![0.;w*h];for(f,d)in frames.iter().zip(shifts){for(a,b)in expected.iter_mut().zip(crate::align::movie_shift(f,w,h,d)){*a+=b*scale;}}
  let max=expected.iter().zip(&got.unweighted).map(|(a,b)|(a-b).abs()).fold(0.,f32::max);eprintln!("GPU cubic preview max error {max}");assert!(max<0.0001,"{max}");
 }
}

#[cfg(test)]mod preview_bench{
 use super::*;use rayon::prelude::*;
 #[test]#[ignore]fn gpu_preview_4k_throughput(){
  let(dev,_)=crate::ctf_gpu::device().expect("GPU required");let(w,h)=(4096,4096);let frames:Vec<Vec<f32>>=(0..4).map(|k|(0..w*h).map(|i|((i*13+k*47)%239)as f32/100.).collect()).collect();let shifts=[[0.,0.],[-2.25,1.7],[3.4,-1.2],[-0.5,-0.5]];
  let _=preview(dev,&frames,w,h,&shifts,4.).unwrap();let start=std::time::Instant::now();let gpu=preview(dev,&frames,w,h,&shifts,4.).unwrap();let gpu_ms=start.elapsed().as_secs_f64()*1000.;
  let start=std::time::Instant::now();let moved:Vec<_>=frames.par_iter().zip(shifts).map(|(v,d)|crate::align::movie_shift(v,w,h,d)).collect();let mut even=vec![0.;w*h];let mut odd=vec![0.;w*h];for(i,v)in moved.iter().enumerate(){let out=if i%2==0{&mut even}else{&mut odd};out.par_iter_mut().zip(v).for_each(|(a,b)|*a+=b*4.);};let cpu:Vec<f32>=even.par_iter().zip(odd).map(|(a,b)|a+b).collect();let cpu_ms=start.elapsed().as_secs_f64()*1000.;let max=cpu.iter().zip(&gpu.unweighted).map(|(a,b)|(a-b).abs()).fold(0.,f32::max);assert!(max<0.0001,"{max}");eprintln!("PREVIEW_BENCH 4096x4096 frames=4 cpuMs={cpu_ms:.3} gpuMs={gpu_ms:.3} maxError={max}");
 }
}
