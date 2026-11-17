//! The method of `sum.rs` on the GPU. Per frame: the frame goes up once as
//! recorded (a byte a pixel for counting cameras; the gain is already on the
//! card), its tiles are cut and gained by one kernel, transformed with Burn's native FFT,
//! phase shifted and dose weighted as elementwise ops and added into device
//! accumulators. The tiles come back once, at the end, to be blended.
use super::{
    check_cancel, dose,
    input::{Raw, Spool},
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
    let cutter = Cutter::new(dev, sp)?;
    let profile = std::env::var_os("TMV_GPU_PROFILE").is_some();
    let (mut cut_ms, mut math_ms) = (0.0, 0.0);
    for (i, &g) in global.iter().enumerate() {
        check_cancel()?;
        let shifts: Vec<[f64; 2]> = (0..n).map(|k| plan.shift(k, model, sp.times[i], g)).collect();
        let clock = std::time::Instant::now();
        let x = cutter.cut(dev, sp, i, &plan, &shifts)?;
        cut_ms += clock.elapsed().as_secs_f64() * 1e3;
        let clock = std::time::Instant::now();
        let fracs: Vec<[f64; 2]> = shifts.iter().map(|s| [s[0] - s[0].round(), s[1] - s[1].round()]).collect();
        let ax = up3(fracs.iter().map(|f| (std::f64::consts::TAU * f[0] / t as f64) as f32).collect(), [n, 1, 1]);
        let ay = up3(fracs.iter().map(|f| (std::f64::consts::TAU * f[1] / t as f64) as f32).collect(), [n, 1, 1]);
        // Nyquist bins of fractionally shifted axes drop out (see sum.rs).
        let keep_x = up3(fracs.iter().flat_map(|f| (0..hw).map(move |j| if fractional(f[0]) && nyquist(j, t) { 0.0 } else { 1.0 })).collect(), [n, 1, hw]);
        let keep_y = up3(fracs.iter().flat_map(|f| (0..t).map(move |j| if fractional(f[1]) && nyquist(j, t) { 0.0 } else { 1.0 })).collect(), [n, t, 1]);
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
        math_ms += clock.elapsed().as_secs_f64() * 1e3;
        progress("求和 GPU", i + 1, sp.len());
    }
    if profile {
        eprintln!("GPU_SUM_PROFILE {}x{} frames={} tiles={n}x{t} cut+upload={cut_ms:.1}ms queue={math_ms:.1}ms", sp.w, sp.h, sp.len());
    }
    let backing = std::time::Instant::now();
    let back = |re: Tensor<3>, im: Tensor<3>| {
        let v = download(inverse(re, im, t));
        let tiles: Vec<Vec<f32>> = v.chunks(t * t).map(<[f32]>::to_vec).collect();
        plan.blend(&tiles)
    };
    let weighted = exposures.map(|_| {
        let scale = norm.sqrt().clamp_min(1e-12).recip().mul_scalar((sp.len() as f32).sqrt());
        back(wr * scale.clone(), wi * scale)
    });
    let sums = Sums {
        unweighted: back(er.clone() + or.clone(), ei.clone() + oi.clone()),
        even: back(er, ei),
        odd: back(or, oi),
        weighted,
    };
    if profile {
        eprintln!("GPU_SUM_PROFILE back (inverse+download+blend) {:.1}ms", backing.elapsed().as_secs_f64() * 1e3);
    }
    Ok(sums)
}

mod kernel {
    use burn_cubecl::cubecl;
    use cubecl::prelude::*;
    /// The tiles of one frame (see `Plan::cut`): thread i is pixel i of the
    /// n × t × t batch; `at` holds each tile's top-left corner in the frame,
    /// moved by the whole-pixel part of its shift. Pixels outside the frame
    /// get `fill` (the frame's mean). The frame comes packed in 32-bit words,
    /// `bits` (8 or 16) to a pixel, or as floats when `bits` is 32; it is
    /// multiplied by `gain` when `gained` is set.
    #[cube(launch)]
    pub fn tmv_cut_tiles(src: &[u32], gain: &[f32], at: &[i32], out: &mut [f32], w: usize, h: usize, t: usize, bits: u32, gained: u32, fill: f32) {
        let i = ABSOLUTE_POS;
        if i < out.len() {
            let tt = t * t;
            let k = i / tt;
            let r = i % tt;
            let x = at[2 * k] + i32::cast_from(r % t);
            let y = at[2 * k + 1] + i32::cast_from(r / t);
            let mut v = fill;
            if x >= 0 && y >= 0 && x < i32::cast_from(w) && y < i32::cast_from(h) {
                let p = usize::cast_from(y) * w + usize::cast_from(x);
                if bits == 8 {
                    v = f32::cast_from((src[p / 4] >> u32::cast_from((p % 4) * 8)) & 255);
                } else if bits == 16 {
                    v = f32::cast_from((src[p / 2] >> u32::cast_from((p % 2) * 16)) & 65535);
                } else {
                    v = f32::reinterpret(src[p]);
                }
                if gained != 0 {
                    v *= gain[p];
                }
            }
            out[i] = v;
        }
    }
    /// One frame moved by a whole-plane shift (Catmull–Rom, the rules of
    /// `align::shift_plane` with fill 0) and added, times `scale`, into `out`.
    /// One thread an output pixel; frames are launched in order, so each pixel
    /// adds them up in the same order as the CPU.
    #[cube(launch)]
    pub fn tmv_preview_shift_add(src: &[f32], out: &mut [f32], shift: &[i32], k: &[f32], w: usize, h: usize, scale: f32, first: u32) {
        let i = ABSOLUTE_POS;
        if i < out.len() {
            let wi = i32::cast_from(w);
            let hi = i32::cast_from(h);
            let cx = i32::cast_from(i % w) + shift[0];
            let cy = i32::cast_from(i / w) + shift[1];
            let inside_x = cx >= 0 && cx < wi && !(shift[2] != 0 && cx == wi - 1);
            let inside_y = cy >= 0 && cy < hi && !(shift[3] != 0 && cy == hi - 1);
            let mut v = 0.0f32;
            if inside_x && inside_y {
                #[unroll]
                for j in 0..4usize {
                    let mut r = cy + i32::cast_from(j) - 1;
                    if r < 0 {
                        r = 0;
                    }
                    if r > hi - 1 {
                        r = hi - 1;
                    }
                    let row = usize::cast_from(r) * w;
                    let mut a = 0.0f32;
                    #[unroll]
                    for q in 0..4usize {
                        let mut c = cx + i32::cast_from(q) - 1;
                        if c < 0 {
                            c = 0;
                        }
                        if c > wi - 1 {
                            c = wi - 1;
                        }
                        a += k[q] * src[row + usize::cast_from(c)];
                    }
                    v += k[4 + j] * a;
                }
            }
            if first != 0 {
                out[i] = v * scale;
            } else {
                out[i] += v * scale;
            }
        }
    }
}

/// Cuts a frame's tiles on the GPU. The gain goes up once a movie.
struct Cutter {
    client: burn_cubecl::cubecl::client::Client,
    gain: Option<burn_cubecl::cubecl::server::Handle>,
}
impl Cutter {
    fn new(dev: &Device, sp: &Spool) -> Result<Cutter> {
        let client = client_of(dev)?;
        anyhow::ensure!(sp.w < i32::MAX as usize / 2 && sp.h < i32::MAX as usize / 2, "电影帧过大");
        let gain = sp.gain.as_ref().map(|g| upload(&client, g));
        Ok(Cutter { client, gain })
    }
    fn cut(&self, dev: &Device, sp: &Spool, i: usize, plan: &Plan, shifts: &[[f64; 2]]) -> Result<Tensor<3>> {
        use burn_cubecl::cubecl::prelude::{BufferArg, CubeCount, CubeDim, CubeElement};
        use rayon::prelude::*;
        let (n, t, px) = (plan.len(), plan.t, sp.w * sp.h);
        // The fill is the gained frame's mean, summed in the order of `sum::cpu`.
        let gained = |k: usize, v: f32| sp.gain.as_ref().map_or(v, |g| v * g[k]) as f64;
        let raw = sp.raw(i)?;
        let (bytes, bits, total): (&[u8], u32, f64) = match &raw {
            Raw::U8(v) => (v, 8, v.par_iter().enumerate().map(|(k, &x)| gained(k, x as f32)).sum()),
            Raw::U16(v) => (u16::as_bytes(v), 16, v.par_iter().enumerate().map(|(k, &x)| gained(k, x as f32)).sum()),
            Raw::F32(v) => (f32::as_bytes(v), 32, v.par_iter().enumerate().map(|(k, &x)| gained(k, x)).sum()),
        };
        let mean = (total / px as f64) as f32;
        // Whole 32-bit words: the last one padded.
        let words = bytes.len().div_ceil(4);
        let src = if bytes.len() % 4 == 0 {
            self.client.create(pinned_bytes(&self.client, &[bytes]))
        } else {
            self.client.create(pinned_bytes(&self.client, &[bytes, &[0u8; 3][..words * 4 - bytes.len()]]))
        };
        let at: Vec<i32> = (0..n).flat_map(|k| [(plan.corners[k][0] + shifts[k][0].round() as isize) as i32, (plan.corners[k][1] + shifts[k][1].round() as isize) as i32]).collect();
        let at = self.client.create_from_slice(i32::as_bytes(&at));
        let gain = match &self.gain {
            Some(g) => g.clone(),
            None => self.client.create_from_slice(f32::as_bytes(&[1.0])),
        };
        let out = Tensor::<3>::empty([n, t, t], dev)
            .try_into_primitive::<burn_cubecl::CubeBackend>()
            .map_err(|_| anyhow::anyhow!("GPU 后端不支持自定义计算"))?;
        let len = n * t * t;
        let groups = len.div_ceil(256) as u32;
        kernel::tmv_cut_tiles::launch(
            &self.client,
            CubeCount::Static(groups.min(65535), groups.div_ceil(65535), 1),
            CubeDim::new_1d(256),
            // Each handle holds exactly the element count given here.
            unsafe { BufferArg::from_raw_parts(src, words) },
            unsafe { BufferArg::from_raw_parts(gain, if self.gain.is_some() { px } else { 1 }) },
            unsafe { BufferArg::from_raw_parts(at, 2 * n) },
            unsafe { BufferArg::from_raw_parts(out.handle.clone(), len) },
            sp.w,
            sp.h,
            t,
            bits,
            self.gain.is_some() as u32,
            mean,
        );
        Ok(Tensor::from_primitive::<burn_cubecl::CubeBackend>(out))
    }
}

/// The preview sums on the GPU: each frame uploaded once and moved and
/// added by one kernel into device even/odd sums, which come back once.
/// (The earlier version built the warp from Burn gathers, eight 16-million
/// index gathers a 4096² frame, and was slower than the CPU.)
pub fn preview(dev: &Device, frames: &[Vec<f32>], w: usize, h: usize, shifts: &[[f64; 2]], scale: f32) -> Result<Sums> {
    use burn_cubecl::cubecl;
    use cubecl::prelude::{BufferArg, CubeCount, CubeDim, CubeElement};
    let n = w * h;
    anyhow::ensure!(!frames.is_empty() && frames.len() == shifts.len() && frames.iter().all(|f| f.len() == n), "预览帧尺寸不一致");
    anyhow::ensure!(w < i32::MAX as usize / 2 && h < i32::MAX as usize / 2, "预览帧过大");
    let client = client_of(dev)?;
    let profile = std::env::var_os("TMV_GPU_PROFILE").is_some();
    let clock = std::time::Instant::now();
    let (mut upload_ms, mut launch_ms) = (0.0, 0.0);
    let halves = [client.empty(n * 4), client.empty(n * 4)];
    let groups = n.div_ceil(256) as u32;
    for (i, (frame, d)) in frames.iter().zip(shifts).enumerate() {
        check_cancel()?;
        let t = std::time::Instant::now();
        let split = |s: f64| (s.floor() as i32, crate::align::catmull_rom((s - s.floor()) as f32), s > s.floor());
        let ((ix, kx, fx), (iy, ky, fy)) = (split(d[0]), split(d[1]));
        let src = upload(&client, frame);
        let shift = client.create_from_slice(i32::as_bytes(&[ix, iy, fx as i32, fy as i32]));
        let k: Vec<f32> = kx.into_iter().chain(ky).collect();
        let k = client.create_from_slice(f32::as_bytes(&k));
        upload_ms += t.elapsed().as_secs_f64() * 1e3;
        let t = std::time::Instant::now();
        kernel::tmv_preview_shift_add::launch(
            &client,
            CubeCount::Static(groups.min(1024), groups.div_ceil(1024), 1),
            CubeDim::new_1d(256),
            // Each handle holds exactly the f32/i32 count given here; the
            // kernel checks its index against the output length.
            unsafe { BufferArg::from_raw_parts(src, n) },
            unsafe { BufferArg::from_raw_parts(halves[i % 2].clone(), n) },
            unsafe { BufferArg::from_raw_parts(shift, 4) },
            unsafe { BufferArg::from_raw_parts(k, 8) },
            w,
            h,
            scale,
            (i < 2) as u32,
        );
        launch_ms += t.elapsed().as_secs_f64() * 1e3;
    }
    let t = std::time::Instant::now();
    // A single frame (the warm-up of a 4096² calibration) has no odd half.
    let used = frames.len().min(2);
    let read = client.read(halves[..used].to_vec())?;
    if profile {
        eprintln!("GPU_PREVIEW_PROFILE {w}x{h} frames={} upload={upload_ms:.1}ms launch={launch_ms:.1}ms read+wait={:.1}ms total={:.1}ms", frames.len(), t.elapsed().as_secs_f64() * 1e3, clock.elapsed().as_secs_f64() * 1e3);
    }
    anyhow::ensure!(read.len() == used && read.iter().all(|b| b.len() == n * 4), "GPU 预览读取尺寸不一致");
    let floats = |b: &[u8]| -> Vec<f32> { b.chunks_exact(4).map(|v| f32::from_ne_bytes(v.try_into().unwrap())).collect() };
    let even = floats(&read[0]);
    let odd = read.get(1).map_or_else(|| vec![0.0; n], |b| floats(b));
    let unweighted: Vec<f32> = {
        use rayon::prelude::*;
        even.par_iter().zip(&odd).map(|(a, b)| a + b).collect()
    };
    anyhow::ensure!(unweighted.iter().all(|v| v.is_finite()), "GPU preview returned non-finite values");
    Ok(Sums { unweighted, even, odd, weighted: None })
}

/// Frames laid end to end in host memory from the pinned staging pool, to
/// be uploaded. A plain upload (`create_from_slice`, or `TensorData` of a
/// fresh `Vec`) copies into new pageable memory and measured 0.4 GB/s on a
/// PCIe 3 ×16 card that takes 12 GB/s from pinned memory. Falls back to
/// ordinary memory when the pool has no room.
pub(crate) fn pinned(client: &burn_cubecl::cubecl::client::Client, parts: &[&[f32]]) -> burn_cubecl::cubecl::bytes::Bytes {
    use burn_cubecl::cubecl::prelude::CubeElement;
    let parts: Vec<&[u8]> = parts.iter().map(|p| f32::as_bytes(p)).collect();
    pinned_bytes(client, &parts)
}
/// `pinned` for any bytes.
pub(crate) fn pinned_bytes(client: &burn_cubecl::cubecl::client::Client, parts: &[&[u8]]) -> burn_cubecl::cubecl::bytes::Bytes {
    use burn_cubecl::cubecl::bytes::Bytes;
    use rayon::prelude::*;
    let len: usize = parts.iter().map(|p| p.len()).sum();
    // Untouched zero pages: the staging copy of them costs no page writes.
    let t = std::time::Instant::now();
    let mut b = Bytes::from_bytes_vec(vec![0u8; len]);
    client.staging(std::iter::once(&mut b), false);
    let staged = t.elapsed().as_secs_f64() * 1e3;
    let pinned = matches!(b.property(), burn_cubecl::cubecl::bytes::AllocationProperty::Pinned);
    let t = std::time::Instant::now();
    let mut dst: &mut [u8] = &mut b;
    for p in parts {
        let (head, rest) = dst.split_at_mut(p.len());
        head.par_chunks_mut(1 << 20).zip(p.par_chunks(1 << 20)).for_each(|(d, s)| d.copy_from_slice(s));
        dst = rest;
    }
    if std::env::var_os("TMV_GPU_PROFILE").is_some() {
        eprintln!("GPU_PINNED_PROFILE {} MB pinned={pinned} staging={staged:.1}ms fill={:.1}ms", len >> 20, t.elapsed().as_secs_f64() * 1e3);
    }
    b
}

pub(crate) fn upload(client: &burn_cubecl::cubecl::client::Client, v: &[f32]) -> burn_cubecl::cubecl::server::Handle {
    client.create(pinned(client, &[v]))
}

/// The CubeCL client behind a Burn device.
pub(crate) fn client_of(dev: &Device) -> Result<burn_cubecl::cubecl::client::Client> {
    Ok(Tensor::<1>::zeros([1], dev)
        .try_into_primitive::<burn_cubecl::CubeBackend>()
        .map_err(|_| anyhow::anyhow!("GPU 后端不支持自定义计算"))?
        .client)
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
