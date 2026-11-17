//! Quick-view movie alignment. Upload binned input frames once; sampling,
//! taper, batched FFT and filtered spectra remain on the same GPU for both
//! alignment passes. The existing solver retains its bounded CPU numerical
//! calibration and small peak/convergence readbacks.
use super::*;
use burn::tensor::{Device, Int, Tensor, TensorData};

/// One real-workload calibration per geometry in this process. The selected
/// device is process-fixed by ctf_gpu (and each multi-GPU worker is pinned).
/// Explicit GPU bypasses the timing choice; auto preserves the faster CPU
/// reference on small movies rather than paying GPU launch/sync overhead.
pub(super) fn select(
    frames: &[Vec<f32>],
    w: usize,
    h: usize,
    progress: &(dyn Fn(usize, usize) + Sync),
    backend: &str,
) -> Result<Aligned> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    static CHOICE: LazyLock<Mutex<HashMap<(usize, usize, usize), (bool, String)>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let report = |gpu, reason: String| {
        crate::compute::report(
            "电影预览运动后端选择",
            if gpu { "GPU + CPU" } else { "CPU" },
            Some(reason),
        )
    };
    if backend == "cpu" {
        report(false, "指定 CPU 后端".into());
        return movie_planes_on(frames, w, h, progress, false);
    }
    let Some((dev, _)) = crate::ctf_gpu::device() else {
        report(false, crate::compute::no_gpu());
        return movie_planes_on(frames, w, h, progress, false);
    };
    let run_gpu =
        || crate::ctf_gpu::guarded(|| movie_planes(frames, w, h, progress, dev)).and_then(|r| r);
    let cancelled =
        || CANCEL.load(Ordering::SeqCst) || crate::motion::CANCEL.load(Ordering::SeqCst);
    if backend == "gpu" {
        // Do not hide an explicit GPU failure behind an apparently successful
        // CPU execution. The existing preview caller handles NoSignal itself.
        let out = run_gpu()?;
        report(true, "指定 GPU：帧与频谱驻留".into());
        return Ok(out);
    }
    let key = (w, h, frames.len());
    let cached = CHOICE.lock().unwrap().get(&key).cloned();
    if let Some((gpu, reason)) = cached {
        if !gpu {
            report(false, reason);
            return movie_planes_on(frames, w, h, progress, false);
        }
        match run_gpu() {
            Ok(out) => {
                report(true, reason);
                return Ok(out);
            }
            Err(e) if cancelled() => return Err(e),
            Err(e) => {
                report(false, format!("GPU 驻留执行失败，回退 CPU：{e:#}"));
                return movie_planes_on(frames, w, h, progress, false);
            }
        }
    }
    crate::compute::report(
        "电影预览后端校准",
        "GPU + CPU",
        Some("首次按当前尺寸和帧数测完整求解；后续电影复用选择，初始化另计".into()),
    );
    let start = std::time::Instant::now();
    let cpu = movie_planes_on(frames, w, h, progress, false)?;
    let cpu_ms = start.elapsed().as_secs_f64() * 1000.;
    // Warm the complete candidate geometry before measuring steady-state work.
    // The caller's alignment_ms still includes this first-call setup cost.
    let result = (|| -> Result<(Aligned, f64)> {
        let _ = run_gpu()?;
        let start = std::time::Instant::now();
        let gpu = run_gpu()?;
        Ok((gpu, start.elapsed().as_secs_f64() * 1000.))
    })();
    let (gpu, gpu_ms) = match result {
        Ok(v) => v,
        Err(e) if cancelled() => return Err(e),
        Err(e) => {
            // Remembered like a slower GPU: otherwise every later movie
            // would pay the CPU solve and the failed GPU attempt again.
            let reason = format!("GPU 驻留校准未通过，使用 CPU：{e:#}");
            CHOICE.lock().unwrap().insert(key, (false, reason.clone()));
            report(false, reason);
            return Ok(cpu);
        }
    };
    let error = cpu
        .shifts
        .iter()
        .zip(&gpu.shifts)
        .map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]))
        .fold(0f64, f64::max);
    let cc_error = cpu
        .cc
        .iter()
        .zip(&gpu.cc)
        .map(|(a, b)| (a - b).abs())
        .fold(0f64, f64::max);
    let valid = error < 0.05 && cc_error < 0.01;
    let use_gpu = valid && gpu_ms < cpu_ms;
    let reason = if valid {
        format!("{}×{}、{}帧完整求解校准（含传输/同步）：CPU {cpu_ms:.1} ms，驻留 GPU {gpu_ms:.1} ms；选择{}",w,h,frames.len(),if use_gpu {"GPU"}else{"CPU"})
    } else {
        format!("GPU 驻留数值校准未通过（位移差 {error:.5} px、CC差 {cc_error:.5}），选择 CPU")
    };
    CHOICE
        .lock()
        .unwrap()
        .insert(key, (use_gpu, reason.clone()));
    report(use_gpu, reason);
    Ok(if use_gpu { gpu } else { cpu })
}

struct Spectra {
    re: Tensor<3>,
    im: Tensor<3>,
    energy: Vec<f64>,
}

fn spectra(
    input: &Tensor<2>,
    g: &Geom,
    offsets: &[[isize; 2]],
    filter: &[f32],
    dev: &Device,
) -> Result<Spectra> {
    let k = offsets.len();
    let n = g.n;
    let x = Tensor::<1, Int>::arange(0..n as i64, dev)
        .reshape([1, 1, n])
        .mul_scalar(g.bin as i32);
    let y = Tensor::<1, Int>::arange(0..n as i64, dev)
        .reshape([1, n, 1])
        .mul_scalar(g.bin as i32);
    let origin = |axis: usize, start: usize| {
        Tensor::<3, Int>::from_data(
            TensorData::new(
                offsets
                    .iter()
                    .map(|o| (o[axis] + start as isize) as i32)
                    .collect::<Vec<_>>(),
                [k, 1, 1],
            ),
            dev,
        )
    };
    let (ox, oy) = (origin(0, g.x0), origin(1, g.y0));
    let mut sampled = Tensor::<2>::zeros([k, n * n], dev);
    for dy in 0..g.bin {
        for dx in 0..g.bin {
            let index = ((y.clone() + oy.clone())
                .add_scalar(dy as i32)
                .mul_scalar(g.w as i32)
                + x.clone()
                + ox.clone())
            .add_scalar(dx as i32)
            .reshape([k, n * n]);
            sampled = sampled + input.clone().gather(1, index);
        }
    }
    let sampled = sampled
        .div_scalar((g.bin * g.bin) as f32)
        .reshape([k, n, n]);
    // Two-stage reduction avoids one serial reduction over an entire frame.
    let mean = sampled.clone().mean_dim(2).mean_dim(1);
    let edge = (n / 16).max(2);
    let taper: Vec<f32> = (0..n)
        .map(|i| {
            let d = i.min(n - 1 - i);
            if d >= edge {
                1.
            } else {
                (0.5 - 0.5 * (PI * (d as f64 + 0.5) / edge as f64).cos()) as f32
            }
        })
        .collect();
    let taper = Tensor::<1>::from_data(TensorData::new(taper, [n]), dev);
    let re = (sampled - mean) * taper.clone().reshape([1, 1, n]) * taper.reshape([1, n, 1]);
    let (re, im) = burn::signal::cfft(re, Tensor::<3>::zeros([k, n, n], dev), 2, None);
    let (re, im) = burn::signal::cfft(re, im, 1, None);
    let m = g.m;
    let keep = Tensor::<1, Int>::arange(0..m as i64, dev)
        .add_scalar((n - m / 2) as i32)
        .remainder_scalar(n as i32);
    let filter = Tensor::<3>::from_data(TensorData::new(filter.to_vec(), [1, m, m]), dev);
    let re = re.select(1, keep.clone()).select(2, keep.clone()) * filter.clone();
    let im = im.select(1, keep.clone()).select(2, keep) * filter;
    let energy = crate::motion::sum_gpu::download(
        (re.clone() * re.clone() + im.clone() * im.clone())
            .sum_dim(2)
            .sum_dim(1),
    )
    .into_iter()
    .map(|x| x as f64)
    .collect::<Vec<_>>();
    anyhow::ensure!(
        energy.iter().all(|e| e.is_finite()),
        "GPU 电影频谱包含非有限能量"
    );
    Ok(Spectra { re, im, energy })
}

fn solve(
    s: Spectra,
    g: &Geom,
    max: f64,
    tick: &(dyn Fn() + Sync),
    dev: &Device,
) -> Result<(Vec<[f64; 2]>, Vec<f64>, usize)> {
    gpu::movie_tensors(s.re, s.im, &s.energy, g, max, &|_| tick(), dev)
}

pub(super) fn movie_planes(
    frames: &[Vec<f32>],
    w: usize,
    h: usize,
    progress: &(dyn Fn(usize, usize) + Sync),
    dev: &Device,
) -> Result<Aligned> {
    movie_planes_in(frames, w, h, progress, dev, 1 << 27)
}

/// `movie_planes` with the frames kept in device chunks of at most `cap`
/// values each.
fn movie_planes_in(
    frames: &[Vec<f32>],
    w: usize,
    h: usize,
    progress: &(dyn Fn(usize, usize) + Sync),
    dev: &Device,
    cap: usize,
) -> Result<Aligned> {
    let start = std::time::Instant::now();
    anyhow::ensure!(
        frames.len() >= 2 && frames.iter().all(|f| f.len() == w * h),
        "电影尺寸不一致"
    );
    crate::motion::check_cancel()?;
    let g = Geom::new([0, 0, w, h], 1024, w, h)?;
    let k = frames.len();
    let input_len = k
        .checked_mul(w)
        .and_then(|n| n.checked_mul(h))
        .ok_or_else(|| anyhow::anyhow!("电影批次溢出"))?;
    // Frames stay on the device in chunks of at most 1<<27 values (eight
    // 4096² frames): one buffer per chunk, so no single allocation or gather
    // outgrows what every backend accepts. A whole movie used to have to fit
    // one such buffer, and larger movies always fell back to the CPU.
    let per = (cap / (w * h)).max(1);
    anyhow::ensure!(
        w * h <= 1 << 27 && k * g.m * g.m <= 1 << 25,
        "电影超过 GPU 驻留批次预算"
    );
    // The low-signal retry can double the working side. Budget for it too.
    let max_n = (1usize << w.min(h).ilog2()).min(1024);
    anyhow::ensure!(
        k * (max_n / 2) * (max_n / 2) <= 1 << 25,
        "低信号重试超过 GPU 频谱预算"
    );
    let estimated = (input_len as u64
        + (k * max_n * max_n) as u64 * 10
        + (k * (max_n / 2) * (max_n / 2)) as u64 * 24)
        * 4;
    anyhow::ensure!(
        estimated <= crate::resources::gpu_available_bytes() / 2,
        "GPU 驻留工作区约需 {} MiB，当前预算不足",
        estimated / (1 << 20)
    );
    let input: Vec<Tensor<2>> = frames
        .chunks(per)
        .map(|c| {
            Tensor::<2>::from_data(
                TensorData::new(c.iter().flatten().copied().collect::<Vec<_>>(), [c.len(), w * h]),
                dev,
            )
        })
        .collect();
    let filter = make_filter(g.m);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = 2 * k + 2 * MAX_ITER;
    let tick = || progress((done.fetch_add(1, Ordering::Relaxed) + 1).min(total), total);
    let get = |g: &Geom, offsets: &[[isize; 2]], filter: &[f32]| -> Result<Spectra> {
        crate::motion::check_cancel()?;
        if CANCEL.load(Ordering::SeqCst) {
            bail!("已取消");
        }
        let mut parts = input
            .iter()
            .zip(offsets.chunks(per))
            .map(|(c, o)| spectra(c, g, o, filter, dev))
            .collect::<Result<Vec<_>>>()?;
        let s = if parts.len() == 1 {
            parts.pop().unwrap()
        } else {
            Spectra {
                energy: parts.iter().flat_map(|p| p.energy.iter().copied()).collect(),
                re: Tensor::cat(parts.iter().map(|p| p.re.clone()).collect(), 0),
                im: Tensor::cat(parts.into_iter().map(|p| p.im).collect(), 0),
            }
        };
        for _ in 0..k {
            tick();
        }
        Ok(s)
    };
    let max = g.n as f64 / 4.;
    let (d1, _, it1) = solve(get(&g, &vec![[0; 2]; k], &filter)?, &g, max, &tick, dev)?;
    let mid = d1[k / 2];
    let offsets: Vec<_> = d1
        .iter()
        .map(|d| g.offset([d[0] - mid[0], d[1] - mid[1]]))
        .collect();
    let (d2, mut cc, it2) = solve(
        get(&g, &offsets, &filter)?,
        &g,
        (max / 4.).max(8.),
        &tick,
        dev,
    )?;
    let mut shifts: Vec<[f64; 2]> = d2
        .iter()
        .zip(&offsets)
        .map(|(d, o)| {
            [
                d[0] * g.bin as f64 + o[0] as f64,
                d[1] * g.bin as f64 + o[1] as f64,
            ]
        })
        .collect();
    let mean: [f64; 2] =
        std::array::from_fn(|a| shifts.iter().map(|d| d[a]).sum::<f64>() / k as f64);
    for d in &mut shifts {
        for a in 0..2 {
            d[a] -= mean[a];
        }
    }
    let mut iterations = it1 + it2;
    let median = |v: &[f64]| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    if median(&cc) < 0.08 {
        // Preserve the original full-field, narrower-band low-contrast retry.
        let n = (1usize << w.min(h).ilog2()).min(1024);
        let bin = w.min(h) / n;
        let wide = Geom {
            x0: (w - n * bin) / 2,
            y0: (h - n * bin) / 2,
            bin,
            n,
            m: n / 2,
            w,
            h,
        };
        let mut low = make_filter(n / 2);
        for (i, v) in low.iter_mut().enumerate() {
            let x = (i % (n / 2)) as f64 - (n / 4) as f64;
            let y = (i / (n / 2)) as f64 - (n / 4) as f64;
            *v *= (-0.5 * (x * x + y * y) / (n as f64 * 0.04).powi(2)).exp() as f32;
        }
        let (mut d, c, it) = solve(
            get(&wide, &vec![[0; 2]; k], &low)?,
            &wide,
            n as f64 / 16.,
            &tick,
            dev,
        )?;
        if median(&c) > median(&cc) {
            let mean: [f64; 2] =
                std::array::from_fn(|a| d.iter().map(|v| v[a]).sum::<f64>() / k as f64);
            for p in &mut d {
                for a in 0..2 {
                    p[a] = (p[a] - mean[a]) * bin as f64;
                }
            }
            shifts = d;
            cc = c;
            iterations += it;
        }
        if median(&cc) < 0.12 {
            return Err(NoSignal(median(&cc)).into());
        }
    }
    crate::compute::report(
        "电影预览驻留运动求解",
        "GPU + CPU",
        Some(
            "GPU 批量取窗、加窗、FFT、常驻频谱与迭代求解；CPU 小量峰值、收敛判断与数值校准".into(),
        ),
    );
    Ok(Aligned {
        zs: (0..k).collect(),
        shifts,
        cc,
        bin: g.bin,
        n: g.n,
        iterations,
        ms: start.elapsed().as_secs_f64() * 1000.,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "actual GPU numerical acceptance"]
    fn resident_spectra_match_cpu_sampling_offsets_and_filter() {
        let (dev, _) = crate::ctf_gpu::device().expect("GPU required");
        let (w, h, k) = (192, 176, 3);
        let frames: Vec<Vec<f32>> = (0..k)
            .map(|f| {
                (0..w * h)
                    .map(|i| {
                        500. + ((i * 17 + f * 31) % 197) as f32 * 0.3
                            + ((i % w) as f32 * 0.11).sin()
                    })
                    .collect()
            })
            .collect();
        let input = Tensor::<2>::from_data(
            TensorData::new(
                frames.iter().flatten().copied().collect::<Vec<_>>(),
                [k, w * h],
            ),
            dev,
        );
        let g = Geom {
            x0: 17,
            y0: 12,
            bin: 2,
            n: 64,
            m: 32,
            w,
            h,
        };
        let offsets = [[0, 0], [5, -7], [-8, 6]];
        let filter = make_filter(g.m);
        let got = spectra(&input, &g, &offsets, &filter, dev).unwrap();
        let re = crate::motion::sum_gpu::download(got.re);
        let im = crate::motion::sum_gpu::download(got.im);
        let len = g.m * g.m;
        for i in 0..k {
            let want = spectrum(&g, &Fft::new(g.n), &filter, offsets[i], |x, y| {
                frames[i][y * w + x]
            });
            let mut error = 0.;
            let mut norm = 0.;
            for (a, b) in re[i * len..(i + 1) * len]
                .iter()
                .chain(&im[i * len..(i + 1) * len])
                .zip(want.re.iter().chain(&want.im))
            {
                error += (*a as f64 - *b as f64).powi(2);
                norm += (*b as f64).powi(2);
            }
            let relative = (error / norm).sqrt();
            eprintln!("resident spectrum frame {i}: relative L2={relative:e}");
            assert!(
                relative < 2e-4,
                "frame {i}: spectrum relative L2 {relative}"
            );
            assert!(
                (got.energy[i] / want.energy - 1.).abs() < 2e-4,
                "energy mismatch"
            );
        }
    }

    #[test]
    #[ignore = "actual GPU motion and no-signal acceptance"]
    fn resident_movie_preserves_two_pass_shifts_and_no_signal() {
        let (dev, _) = crate::ctf_gpu::device().expect("GPU required");
        let (w, h) = (192, 176);
        let base: Vec<f32> = (0..w * h)
            .map(|i| ((i * 37 % 197) as f32 / 100. - 1.) + ((i % w) as f32 * 0.07).sin())
            .collect();
        let offsets = [[0., 0.], [1.3, -0.4], [-0.7, 1.8], [2.1, 0.6]];
        let frames: Vec<_> = offsets
            .iter()
            .map(|&d| crate::align::movie_shift(&base, w, h, d))
            .collect();
        let cpu = movie_planes_on(&frames, w, h, &|_, _| {}, false).unwrap();
        let got = movie_planes(&frames, w, h, &|_, _| {}, dev).unwrap();
        for (i, (a, b)) in got.shifts.iter().zip(&cpu.shifts).enumerate() {
            let err = (a[0] - b[0]).hypot(a[1] - b[1]);
            eprintln!("resident two-pass frame {i}: shift error={err:0.6} px");
            assert!(err < 0.05, "frame {i}: shift error {err}");
            assert!((got.cc[i] - cpu.cc[i]).abs() < 0.01, "correlation mismatch");
        }
        // One frame a device chunk, as a 4096² movie has eight.
        let chunked = movie_planes_in(&frames, w, h, &|_, _| {}, dev, w * h).unwrap();
        for (a, b) in chunked.shifts.iter().zip(&got.shifts) {
            assert!((a[0] - b[0]).hypot(a[1] - b[1]) < 1e-3, "chunked shifts differ");
        }
        let blank = vec![vec![7.; w * h]; 4];
        let err = movie_planes(&blank, w, h, &|_, _| {}, dev).unwrap_err();
        assert!(err.downcast_ref::<NoSignal>().is_some(), "{err:#}");
    }
}
