//! Wiener-like deconvolution of a phase-flipped tomogram (the filter
//! Tegunov's tom_deconv introduced and Warp and IsoNet use): each spatial
//! frequency is scaled by |CTF| / (CTF² + 1/SNR), with an SNR that falls
//! exponentially with frequency and a gentle high-pass at the lowest
//! frequencies. It restores the low-resolution contrast the CTF takes away
//! and damps the frequencies near its zeros, where there is mostly noise,
//! so particles and membranes stand out. The CTF is isotropic at the
//! series' median defocus.

use crate::ctf::Optics;
use rayon::prelude::*;

/// Strength of the filter (log₁₀ SNR scale / 3) and how fast the SNR falls.
pub const STRENGTH: f64 = 1.0;
pub const FALLOFF: f64 = 1.0;
/// Below this fraction of Nyquist the high-pass ramps up.
const HIGHPASS: f64 = 0.02;

/// Deconvolves a volume [z][y][x] in place.
fn cpu(vol: &mut [f32], [w, h, z]: [usize; 3], pixel_a: f64, defocus_a: f64, optics: &Optics) {
    let (nx, ny, nz) = (w.next_power_of_two(), h.next_power_of_two(), z.next_power_of_two().max(2));
    let mean = (vol.par_iter().map(|&v| v as f64).sum::<f64>() / vol.len() as f64) as f32;
    let mut re = vec![0f32; nx * ny * nz];
    let mut im = vec![0f32; nx * ny * nz];
    for k in 0..z {
        for y in 0..h {
            let (s, d) = ((k * h + y) * w, (k * ny + y) * nx);
            for x in 0..w {
                re[d + x] = vol[s + x] - mean;
            }
        }
    }
    fft3(&mut re, &mut im, [nx, ny, nz], false);
    let f = |i: usize, n: usize| crate::motion::sum::freq(i, n) / n as f64;
    re.par_chunks_mut(nx).zip(im.par_chunks_mut(nx)).enumerate().for_each(|(line, (r, m))| {
        let (y, k) = (line % ny, line / ny);
        for x in 0..nx {
            // Spatial frequency in cycles per pixel, then per Å.
            let kp = (f(x, nx).powi(2) + f(y, ny).powi(2) + f(k, nz).powi(2)).sqrt();
            let ka = kp / pixel_a;
            let ctf = optics.gamma(defocus_a, ka * ka, 0.0).sin().abs();
            let ramp = (kp / (HIGHPASS * 0.5)).min(1.0) * std::f64::consts::PI;
            let snr = (-kp * 100.0 * FALLOFF / pixel_a).exp() * 10f64.powf(3.0 * STRENGTH) * (1.0 - ramp.cos());
            let g = if snr > 0.0 { (ctf / (ctf * ctf + 1.0 / snr)) as f32 } else { 0.0 };
            r[x] *= g;
            m[x] *= g;
        }
    });
    fft3(&mut re, &mut im, [nx, ny, nz], true);
    for k in 0..z {
        for y in 0..h {
            let (s, d) = ((k * ny + y) * nx, (k * h + y) * w);
            vol[d..d + w].copy_from_slice(&re[s..s + w]);
        }
    }
}

/// Keep the FFT chain on the GPU when its complete memory budget fits.
pub(crate) fn deconvolve(vol:&mut [f32],shape:[usize;3],pixel:f64,df:f64,optics:&Optics){
 if let Some((dev,_))=crate::ctf_gpu::device(){
  match crate::ctf_gpu::guarded(|| gpu(vol,shape,pixel,df,optics,dev)).and_then(|r|r){
   Ok(v)=>{vol.copy_from_slice(&v);crate::compute::report("反卷积 FFT 与滤波","GPU",None);return;}
   Err(e)=>{if super::check_cancel().is_err(){return;}crate::compute::report("反卷积","CPU",Some(format!("GPU 执行失败：{e:#}")));}
  }
 }else{crate::compute::report("反卷积","CPU",Some(crate::compute::no_gpu()));}
 cpu(vol,shape,pixel,df,optics);
}
fn gpu(vol: &[f32], [w,h,z]: [usize;3], pixel: f64, df: f64, o: &Optics, dev: &burn::tensor::Device) -> anyhow::Result<Vec<f32>> {
 use burn::tensor::{Tensor,TensorData};
 let [nx,ny,nz]=[w.next_power_of_two(),h.next_power_of_two(),z.next_power_of_two().max(2)];
 let count=nx.checked_mul(ny).and_then(|v|v.checked_mul(nz)).ok_or_else(||anyhow::anyhow!("volume too large"))?;
 // Keep the complete FFT chain resident. Budget includes FFT workspace and
 // filter temporaries; insufficient VRAM uses CPU rather than six PCIe trips.
 let devices=crate::resources::discover();
 let mask=std::env::var("CUDA_VISIBLE_DEVICES").unwrap_or_default();
 let selected=devices.iter().find(|g|mask.split(',').next()==Some(g.uuid.as_str())||mask.split(',').next()==Some(g.index.to_string().as_str())).or_else(||devices.first());
 let available=selected.map(|g|g.free_mb*1024*1024).unwrap_or(512*1024*1024);
 anyhow::ensure!((count as u64).saturating_mul(48) < available*85/100,"完整 GPU FFT 与滤波预计需 {:.1} GB，可用显存 {:.1} GB；选择 CPU，避免分轴反复传输",count as f64*48./1e9,available as f64/1e9);
 let mean=(vol.par_iter().map(|&v|v as f64).sum::<f64>()/vol.len() as f64) as f32;
 let mut padded=vec![0.;count];
 for k in 0..z { for y in 0..h { for x in 0..w {padded[(k*ny+y)*nx+x]=vol[(k*h+y)*w+x]-mean;} } }
 let mut re=Tensor::<3>::from_data(TensorData::new(padded,[nz,ny,nx]),dev);
 let mut im=Tensor::<3>::zeros([nz,ny,nx],dev);
 for axis in [2,1,0] { super::check_cancel()?;(re,im)=burn::signal::cfft(re,im,axis,None); }
 let f=|i:usize,n:usize|(crate::motion::sum::freq(i,n)/n as f64) as f32;
 let axis=|n:usize,shape|Tensor::<3>::from_data(TensorData::new((0..n).map(|i|f(i,n).powi(2)).collect::<Vec<_>>(),shape),dev);
 let kp=(axis(nx,[1,1,nx])+axis(ny,[1,ny,1])+axis(nz,[nz,1,1])).sqrt();
 let k2=kp.clone().powf_scalar(2.).div_scalar((pixel*pixel) as f32);
 let ctf=(k2.clone().mul_scalar((o.a*df) as f32)-k2.powf_scalar(2.).mul_scalar(o.b as f32)+o.w as f32).sin().abs();
 let ramp=kp.clone().div_scalar((HIGHPASS*0.5) as f32).clamp(0.,1.).mul_scalar(std::f32::consts::PI);
 let snr=kp.mul_scalar((-100.*FALLOFF/pixel) as f32).exp().mul_scalar(10f32.powf((3.*STRENGTH) as f32))*(ramp.cos().neg()+1.);
 let gain=ctf.clone()/(ctf.powf_scalar(2.)+snr.clamp_min(1e-30).recip());
 re=re*gain.clone();im=im*gain;
 for (axis,n) in [(2,nx),(1,ny),(0,nz)] { super::check_cancel()?;let(r,m)=burn::signal::cfft(re,im.neg(),axis,None);re=r.div_scalar(n as f32);im=m.div_scalar(-(n as f32)); }
 let re=crate::motion::sum_gpu::download(re);
 anyhow::ensure!(re.iter().all(|v|v.is_finite()),"GPU FFT 返回非有限值");
 let mut out=vec![0.;vol.len()];for k in 0..z{for y in 0..h{out[(k*h+y)*w..][..w].copy_from_slice(&re[(k*ny+y)*nx..][..w]);}}
 Ok(out)
}

/// In-place complex 3D FFT (power-of-two sides); the inverse is scaled.
fn fft3(re: &mut [f32], im: &mut [f32], [nx, ny, nz]: [usize; 3], inverse: bool) {
    if inverse {
        im.par_iter_mut().for_each(|v| *v = -*v);
    }
    let run = |re: &mut [f32], im: &mut [f32], n: usize, stride: usize, lines: Vec<usize>| {
        let fft = crate::fft::Fft::new(n);
        let out: Vec<(Vec<f32>, Vec<f32>)> = lines
            .par_iter()
            .map(|&b| {
                let mut r: Vec<f32> = (0..n).map(|k| re[b + k * stride]).collect();
                let mut i: Vec<f32> = (0..n).map(|k| im[b + k * stride]).collect();
                fft.run(&mut r, &mut i);
                (r, i)
            })
            .collect();
        for (&b, (r, i)) in lines.iter().zip(out) {
            for k in 0..n {
                re[b + k * stride] = r[k];
                im[b + k * stride] = i[k];
            }
        }
    };
    run(re, im, nx, 1, (0..ny * nz).map(|l| l * nx).collect());
    run(re, im, ny, nx, (0..nx * nz).map(|l| (l / nx) * nx * ny + l % nx).collect());
    run(re, im, nz, nx * ny, (0..nx * ny).collect());
    if inverse {
        let n = (nx * ny * nz) as f32;
        re.par_iter_mut().for_each(|v| *v /= n);
        im.par_iter_mut().for_each(|v| *v = -*v / n);
    }
}

#[cfg(test)]
mod gpu_tests{
 use super::*;
 #[test]
 fn gpu_deconvolution_matches_cpu_reference(){
  let Some((dev,_))=crate::ctf_gpu::device()else{assert!(std::env::var_os("TMV_REQUIRE_GPU").is_none(),"GPU required");return;};
  let shape=[17,13,9];let input:Vec<f32>=(0..17*13*9).map(|i|((i*37%251) as f32/125.-1.)+((i%17) as f32*0.3).sin()).collect();let o=Optics::of(300.,2.7,0.07);let mut reference=input.clone();cpu(&mut reference,shape,3.2,25000.,&o);let actual=gpu(&input,shape,3.2,25000.,&o,dev).unwrap();
  let error=actual.iter().zip(&reference).map(|(a,b)|(*a as f64-*b as f64).powi(2)).sum::<f64>();let norm=reference.iter().map(|v|(*v as f64).powi(2)).sum::<f64>();let relative=(error/norm).sqrt();eprintln!("GPU deconvolution relative RMS {relative}");assert!(relative<0.002,"{relative}");
 }
}
