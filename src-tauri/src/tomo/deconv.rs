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

/// The filter's gain at a frequency of `kp` cycles per pixel.
#[inline]
fn gain(kp: f64, pixel_a: f64, defocus_a: f64, optics: &Optics, scale: f64) -> f64 {
    let ka = kp / pixel_a;
    let ctf = optics.gamma(defocus_a, ka * ka, 0.0).sin().abs();
    let ramp = (kp / (HIGHPASS * 0.5)).min(1.0) * std::f64::consts::PI;
    let snr = (-kp * 100.0 * FALLOFF / pixel_a).exp() * scale * (1.0 - ramp.cos());
    if snr > 0.0 { ctf / (ctf * ctf + 1.0 / snr) } else { 0.0 }
}

/// A raw pointer shared by rayon tasks that each touch their own elements.
#[derive(Clone, Copy)]
struct Shared(*mut f32);
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}
impl Shared {
    fn at(self, i: usize) -> *mut f32 {
        self.0.wrapping_add(i)
    }
}

/// Deconvolves a volume [z][y][x] in place on all CPU cores.
///
/// Same padding (powers of two) and filter as `cpu_reference`, but the
/// input is real, so the transform keeps only the nx/2+1 non-negative x
/// frequencies (two real rows per complex FFT); the zero padding is
/// skipped; every pass runs in parallel without a single-threaded copy
/// back; and the z pass applies the filter and transforms back in one go.
pub(crate) fn cpu(vol: &mut [f32], [w, h, z]: [usize; 3], pixel_a: f64, defocus_a: f64, optics: &Optics) {
    let (nx, ny, nz) = (w.next_power_of_two().max(2), h.next_power_of_two(), z.next_power_of_two().max(2));
    let hx = nx / 2 + 1;
    let mean = (vol.par_iter().map(|&v| v as f64).sum::<f64>() / vol.len() as f64) as f32;
    let (fx, fy, fz) = (crate::fft::Fft::new(nx), (ny >= 2).then(|| crate::fft::Fft::new(ny)), crate::fft::Fft::new(nz));
    // Half spectrum [nz][ny][hx].
    let mut re = vec![0f32; hx * ny * nz];
    let mut im = vec![0f32; hx * ny * nz];

    // x: rows (k, y) for y < h, k < z; two real rows per complex FFT.
    {
        let rows: Vec<usize> = (0..z).flat_map(|k| (0..h).map(move |y| k * ny + y)).collect();
        let (pr, pi) = (Shared(re.as_mut_ptr()), Shared(im.as_mut_ptr()));
        let vol = &*vol;
        rows.par_chunks(2).for_each_init(
            || (vec![0f32; nx], vec![0f32; nx]),
            |(a, b), pair| {
                let src = |row: usize, out: &mut [f32]| {
                    let (k, y) = (row / ny, row % ny);
                    let s = &vol[(k * h + y) * w..][..w];
                    for (o, &v) in out.iter_mut().zip(s) {
                        *o = v - mean;
                    }
                    out[w..].fill(0.0);
                };
                src(pair[0], a);
                if pair.len() > 1 { src(pair[1], b) } else { b.fill(0.0) }
                fx.run(a, b);
                for (i, &row) in pair.iter().enumerate() {
                    // SAFETY: each row is written by exactly one task.
                    let (r, m) = unsafe {
                        (std::slice::from_raw_parts_mut(pr.at(row * hx), hx), std::slice::from_raw_parts_mut(pi.at(row * hx), hx))
                    };
                    for x in 0..hx {
                        let j = (nx - x) % nx;
                        // A = (Z[x] + conj Z[-x]) / 2, B = (Z[x] - conj Z[-x]) / 2i.
                        if i == 0 {
                            r[x] = 0.5 * (a[x] + a[j]);
                            m[x] = 0.5 * (b[x] - b[j]);
                        } else {
                            r[x] = 0.5 * (b[x] + b[j]);
                            m[x] = -0.5 * (a[x] - a[j]);
                        }
                    }
                }
            },
        );
    }

    // y: one z slab per task, columns in blocks; `inverse` conjugates around a forward FFT.
    let y_pass = |re: &mut [f32], im: &mut [f32], inverse: bool| {
        let Some(fy) = &fy else { return };
        const B: usize = 16;
        re.par_chunks_mut(hx * ny).zip(im.par_chunks_mut(hx * ny)).take(z).for_each_init(
            || (vec![0f32; B * ny], vec![0f32; B * ny]),
            |(br, bi), (r, m)| {
                for x0 in (0..hx).step_by(B) {
                    let nb = B.min(hx - x0);
                    for y in 0..ny {
                        for c in 0..nb {
                            br[c * ny + y] = r[y * hx + x0 + c];
                            bi[c * ny + y] = if inverse { -m[y * hx + x0 + c] } else { m[y * hx + x0 + c] };
                        }
                    }
                    for c in 0..nb {
                        fy.run(&mut br[c * ny..][..ny], &mut bi[c * ny..][..ny]);
                    }
                    for y in 0..ny {
                        for c in 0..nb {
                            r[y * hx + x0 + c] = br[c * ny + y];
                            m[y * hx + x0 + c] = if inverse { -bi[c * ny + y] } else { bi[c * ny + y] };
                        }
                    }
                }
            },
        );
    };
    y_pass(&mut re, &mut im, false);

    // z: per y, forward, filter, inverse (only the slabs k < z are kept).
    {
        let f = |i: usize, n: usize| crate::motion::sum::freq(i, n) / n as f64;
        let scale = 10f64.powf(3.0 * STRENGTH);
        let (pr, pi) = (Shared(re.as_mut_ptr()), Shared(im.as_mut_ptr()));
        (0..ny).into_par_iter().for_each_init(
            || (vec![0f32; nz], vec![0f32; nz]),
            |(br, bi), y| {
                let fy2 = f(y, ny).powi(2);
                for x in 0..hx {
                    let fxy = f(x, nx).powi(2) + fy2;
                    // SAFETY: the elements (k, y, x) for this y belong to this task.
                    for k in 0..nz {
                        let i = (k * ny + y) * hx + x;
                        unsafe {
                            br[k] = *pr.at(i);
                            bi[k] = *pi.at(i);
                        }
                    }
                    if br.iter().chain(bi.iter()).all(|&v| v == 0.0) {
                        continue;
                    }
                    fz.run(br, bi);
                    for k in 0..nz {
                        let g = gain((fxy + f(k, nz).powi(2)).sqrt(), pixel_a, defocus_a, optics, scale) as f32;
                        br[k] *= g;
                        bi[k] = -bi[k] * g;
                    }
                    fz.run(br, bi);
                    for k in 0..z {
                        let i = (k * ny + y) * hx + x;
                        unsafe {
                            *pr.at(i) = br[k];
                            *pi.at(i) = -bi[k];
                        }
                    }
                }
            },
        );
    }
    y_pass(&mut re, &mut im, true);

    // x back to real: two half-spectrum rows as one complex inverse FFT.
    let norm = 1.0 / (nx * ny * nz) as f32;
    let rows: Vec<usize> = (0..z).flat_map(|k| (0..h).map(move |y| k * ny + y)).collect();
    let out = Shared(vol.as_mut_ptr());
    let (re, im) = (&re, &im);
    rows.par_chunks(2).for_each_init(
        || (vec![0f32; nx], vec![0f32; nx], vec![0f32; hx]),
        |(a, b, zero), pair| {
            let row = |i: usize| pair.get(i).map(|&r| (&re[r * hx..][..hx], &im[r * hx..][..hx]));
            let (ar, ai) = row(0).unwrap();
            let (cr, ci) = row(1).unwrap_or((&*zero, &*zero));
            // Z = A + iB over all x, from the Hermitian halves; conjugated for the inverse.
            for x in 0..nx {
                let (j, s) = if x < hx { (x, 1.0) } else { (nx - x, -1.0) };
                let (a_r, a_i, b_r, b_i) = (ar[j], s * ai[j], cr[j], s * ci[j]);
                a[x] = a_r - b_i;
                b[x] = -(a_i + b_r);
            }
            fx.run(a, b);
            for (i, &r) in pair.iter().enumerate() {
                let (k, y) = (r / ny, r % ny);
                let src = if i == 0 { &*a } else { &*b };
                let sign = if i == 0 { norm } else { -norm };
                // SAFETY: each output row is written by exactly one task.
                let dst = unsafe { std::slice::from_raw_parts_mut(out.at((k * h + y) * w), w) };
                for (d, &v) in dst.iter_mut().zip(src) {
                    *d = v * sign;
                }
            }
        },
    );
}

/// The original full complex implementation, kept as the reference.
#[cfg(test)]
pub(crate) fn cpu_reference(vol: &mut [f32], [w, h, z]: [usize; 3], pixel_a: f64, defocus_a: f64, optics: &Optics) {
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
 let available=crate::resources::gpu_available_bytes();
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
#[cfg(test)]
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
 #[cfg(target_os="macos")]
 #[test]
 #[ignore = "requires Metal and at least 1 GiB free unified-memory budget"]
 fn metal_deconvolution_uses_unified_memory() {
  let (dev,name)=crate::ctf_gpu::device().expect("Metal required"); assert_eq!(*name,"Metal");
  // 768 MiB FFT workspace is modest on this 32 GiB M1 Max, but the
  // NVIDIA-only probe incorrectly caps Metal at 512 MiB before dispatch.
  let shape=[256,256,256]; let input=vec![1.0;256*256*256];
  let output=gpu(&input,shape,3.2,25000.,&Optics::of(300.,2.7,0.07),dev)
    .expect("Metal must use its actual unified-memory working-set budget");
  assert!(output.iter().all(|v|v.is_finite() && v.abs()<0.00001));
 }
 /// TMV_DECONV_BENCH=<tomogram.mrc> [TMV_DECONV_REF=1] cargo test --release
 /// --features cli deconv::gpu_tests::bench_file -- --ignored --nocapture
 #[test]
 #[ignore = "needs a tomogram in TMV_DECONV_BENCH"]
 fn bench_file(){
  let path=std::env::var("TMV_DECONV_BENCH").expect("TMV_DECONV_BENCH");
  let t=std::time::Instant::now();let (input,shape,px)=super::super::pipeline::load(&path).unwrap();eprintln!("load {shape:?} {px} Å: {:.1} s",t.elapsed().as_secs_f64());
  let o=Optics::of(300.,2.7,0.07);let df=40000.;
  let mut new=input.clone();let t=std::time::Instant::now();cpu(&mut new,shape,px,df,&o);eprintln!("new CPU ({} threads): {:.1} s",rayon::current_num_threads(),t.elapsed().as_secs_f64());
  if let Some((dev,_))=crate::ctf_gpu::device(){let t=std::time::Instant::now();match gpu(&input,shape,px,df,&o,dev){Ok(_)=>eprintln!("GPU: {:.1} s",t.elapsed().as_secs_f64()),Err(e)=>eprintln!("GPU skipped: {e:#}")}}
  if std::env::var_os("TMV_DECONV_REF").is_some(){
   let mut old=input.clone();let t=std::time::Instant::now();cpu_reference(&mut old,shape,px,df,&o);eprintln!("old CPU: {:.1} s",t.elapsed().as_secs_f64());
   let e=new.iter().zip(&old).map(|(a,b)|(*a as f64-*b as f64).powi(2)).sum::<f64>();let n=old.iter().map(|v|(*v as f64).powi(2)).sum::<f64>();eprintln!("relative RMS new vs old {:e}",(e/n).sqrt());
  }
  let t=std::time::Instant::now();crate::tomo::output::write_mrc(&format!("{path}.deconv-bench.mrc"),&new,shape,px,false).unwrap();eprintln!("write: {:.1} s",t.elapsed().as_secs_f64());
 }
 #[test]
 fn cpu_matches_reference(){
  let o=Optics::of(300.,2.7,0.07);
  for shape in [[17,13,9],[64,32,16],[2,2,1],[40,24,30]]{
   let n=shape[0]*shape[1]*shape[2];
   let input:Vec<f32>=(0..n).map(|i|((i*37%251) as f32/125.-1.)+((i%17) as f32*0.3).sin()).collect();
   let mut reference=input.clone();cpu_reference(&mut reference,shape,3.2,25000.,&o);
   let mut actual=input.clone();cpu(&mut actual,shape,3.2,25000.,&o);
   let error=actual.iter().zip(&reference).map(|(a,b)|(*a as f64-*b as f64).powi(2)).sum::<f64>();let norm=reference.iter().map(|v|(*v as f64).powi(2)).sum::<f64>().max(1e-30);
   let relative=(error/norm).sqrt();eprintln!("{shape:?} CPU vs reference relative RMS {relative}");assert!(relative<1e-5,"{shape:?} {relative}");
  }
 }
 #[test]
 fn gpu_deconvolution_matches_cpu_reference(){
  let Some((dev,_))=crate::ctf_gpu::device()else{assert!(std::env::var_os("TMV_REQUIRE_GPU").is_none(),"GPU required");return;};
  let shape=[17,13,9];let input:Vec<f32>=(0..17*13*9).map(|i|((i*37%251) as f32/125.-1.)+((i%17) as f32*0.3).sin()).collect();let o=Optics::of(300.,2.7,0.07);let mut reference=input.clone();cpu_reference(&mut reference,shape,3.2,25000.,&o);let actual=gpu(&input,shape,3.2,25000.,&o,dev).unwrap();
  let error=actual.iter().zip(&reference).map(|(a,b)|(*a as f64-*b as f64).powi(2)).sum::<f64>();let norm=reference.iter().map(|v|(*v as f64).powi(2)).sum::<f64>();let relative=(error/norm).sqrt();eprintln!("GPU deconvolution relative RMS {relative}");assert!(relative<0.002,"{relative}");
 }
}
