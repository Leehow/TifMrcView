//! Batched patch Fourier correlations on Burn. Both images of every patch
//! in a view pair share native FFT batches. Cross powers and peak search
//! stay on the device; only small peak grids, indices and energies return.
//! A measured CPU/GPU comparison chooses the default, not device presence.
use anyhow::Result;
use burn::tensor::{Device, Int, Tensor, TensorData};

pub struct Batch {
    dev: Device,
    n: usize,
    m: usize,
    window: Tensor<3>,
    filter: Tensor<3>,
    keep: Tensor<1, Int>,
    /// Band DFT tables [n, m]: samples × kept frequencies.
    cos: Tensor<2>,
    sin: Tensor<2>,
}
impl Batch {
    pub fn new(dev: &Device, n: usize) -> Self {
        let m = n / 2;
        let (win, filter) = crate::align::patch_window(n);
        let freqs: Vec<i64> = (0..m as i64).map(|i| i - (m / 2) as i64).collect();
        let samples: Vec<i64> = (0..n as i64).collect();
        let (cos, sin) = crate::align::dft_tables(&samples, &freqs, n, dev);
        Self {
            cos,
            sin,
            dev: dev.clone(),
            n,
            m,
            window: Tensor::from_data(TensorData::new(win, [1, n, n]), dev),
            filter: Tensor::from_data(TensorData::new(filter, [1, m, m]), dev),
            keep: Tensor::from_data(
                TensorData::new(
                    (0..m)
                        .map(|i| (i as i32 - m as i32 / 2).rem_euclid(n as i32))
                        .collect(),
                    [m],
                ),
                dev,
            ),
        }
    }
    pub fn correlate(
        &self,
        pairs: &[(Vec<f32>, Vec<f32>)],
        max: f64,
    ) -> Result<Vec<([f64; 2], f64)>> {
        let (n, m, k) = (self.n, self.m, pairs.len());
        // 16 M values a batch (512 pairs of 128²): fewer, larger launches
        // and peak read-backs.
        let batch = ((1usize << 24) / (2 * n * n)).max(1);
        if k > batch {
            return pairs
                .chunks(batch)
                .map(|p| self.correlate(p, max))
                .collect::<Result<Vec<_>>>()
                .map(|v| v.into_iter().flatten().collect());
        }

        if k == 0 {
            return Ok(vec![]);
        }
        // Straight into pinned memory: a plain upload measured 0.4 GB/s, and
        // with 1000 pairs a movie the upload alone made the GPU lose to the CPU.
        let parts: Vec<&[f32]> = pairs.iter().map(|p| p.0.as_slice()).chain(pairs.iter().map(|p| p.1.as_slice())).collect();
        let x = match crate::motion::sum_gpu::client_of(&self.dev) {
            Ok(client) => Tensor::<3>::from_data(
                TensorData::from_bytes(crate::motion::sum_gpu::pinned(&client, &parts), [2 * k, n, n], burn::tensor::DType::F32),
                &self.dev,
            ),
            Err(_) => Tensor::<3>::from_data(TensorData::new(parts.concat(), [2 * k, n, n]), &self.dev),
        };
        let mean = x.clone().mean_dim(1).mean_dim(2);
        let x = (x - mean) * self.window.clone();
        // Only the kept band is needed: a DFT by matrix products computes
        // just those m² frequencies, where the full complex FFT did two real
        // FFTs (one of zeros) and mirror copies per axis over n².
        let (fr, fi) = if crate::align::fft_path() {
            let zeros = Tensor::<3>::zeros([2*k,n,n], &self.dev);
            let (fr, fi) = burn::signal::cfft(x, zeros, 2, None);
            let (fr, fi) = burn::signal::cfft(fr, fi, 1, None);
            let band = |x: Tensor<3>| x.select(1, self.keep.clone()).select(2, self.keep.clone());
            (band(fr), band(fi))
        } else {
            crate::align::band_dft(x, &self.cos, &self.sin)
        };
        let fr = (fr * self.filter.clone()).reshape([2*k,m*m]);
        let fi = (fi * self.filter.clone()).reshape([2*k,m*m]);
        let ra = fr.clone().narrow(0, 0, k);
        let rb = fr.narrow(0, k, k);
        let ia = fi.clone().narrow(0, 0, k);
        let ib = fi.narrow(0, k, k);
        let ea = (ra.clone() * ra.clone() + ia.clone() * ia.clone()).sum_dim(1);
        let eb = (rb.clone() * rb.clone() + ib.clone() * ib.clone()).sum_dim(1);
        let pr = rb.clone() * ra.clone() + ib.clone() * ia.clone();
        let pi = ib * ra - rb * ia;
        crate::align::patch_gpu_peak(
            pr.reshape([k,m,m]), pi.reshape([k,m,m]),
            Tensor::cat(vec![ea, eb], 1), n, max, &self.dev,
        )
    }
}

#[cfg(test)]
mod throughput_tests {
    use super::*;
    use rayon::prelude::*;
    fn pairs(n: usize, count: usize) -> Vec<(Vec<f32>, Vec<f32>)> {
        (0..count).map(|k| {
            let a: Vec<f32> = (0..n*n).map(|i| {
                let x=(i%n) as f32; let y=(i/n) as f32;
                (x*0.37+y*0.21+k as f32).sin() + (x*0.13-y*0.43).cos()
                    + ((((i as u64).wrapping_mul(6364136223846793005u64).wrapping_add(k as u64))%997) as f32/997.0)*0.3
            }).collect();
            let shifts=[[0.,0.],[3.25,-1.8],[-5.4,2.7],[11.2,-9.3]];
            let b=crate::align::movie_shift(&a,n,n,shifts[k%4]);
            (a,b)
        }).collect()
    }
    fn device() -> Option<&'static Device> {
        let d=crate::ctf_gpu::device().map(|p|&p.0);
        assert!(d.is_some() || std::env::var_os("TMV_REQUIRE_GPU").is_none(), "GPU required");
        d
    }
    #[test]
    fn gpu_patch_batch_matches_cpu_across_shapes() {
        let Some(dev)=device() else{return;};
        for n in [32,64,128,256] {
            let pairs=pairs(n,8);let max=n as f64*0.22;
            let reference:Vec<_>=pairs.par_iter().map(|(a,b)|crate::align::patch_shift(a,b,n,max)).collect();
            let got=Batch::new(dev,n).correlate(&pairs,max).unwrap();
            for (i,(a,b)) in got.iter().zip(&reference).enumerate() {
                let error=(a.0[0]-b.0[0]).hypot(a.0[1]-b.0[1]);
                assert!(error<0.05 && (a.1-b.1).abs()<0.01,"n={n} i={i} error={error} gpu={a:?} cpu={b:?}");
            }
            assert!(Batch::new(dev,n).correlate(&[],max).unwrap().is_empty());
        }
    }
    #[test]
    #[ignore]
    fn gpu_patch_batch_throughput() {
        let dev=device().expect("GPU required");
        for (n,count) in [(128,64),(256,64),(128,1000)] {
            let pairs=pairs(n,count);let max=n as f64*0.22;
            let batch=Batch::new(dev,n);
            let cpu=||pairs.par_iter().map(|(a,b)|crate::align::patch_shift(a,b,n,max)).collect::<Vec<_>>();
            let _=cpu(); let _=batch.correlate(&pairs,max).unwrap();
            let mut times=vec![];
            for _ in 0..3 {
                let t=std::time::Instant::now();let c=cpu();let cm=t.elapsed().as_secs_f64()*1000.;
                let t=std::time::Instant::now();let g=batch.correlate(&pairs,max).unwrap();let gm=t.elapsed().as_secs_f64()*1000.;
                let error=g.iter().zip(&c).map(|(a,b)|(a.0[0]-b.0[0]).hypot(a.0[1]-b.0[1])).fold(0.,f64::max);
                assert!(error<0.05,"{error}"); times.push((cm,gm));
            }
            eprintln!("PATCH_BENCH n={n} count={count} cpuGpuMs={times:?}");
        }
    }
}
