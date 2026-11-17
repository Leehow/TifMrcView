//! Batched patch Fourier correlations on Burn. Both images of every patch
//! in a view pair share native FFT batches. Cross powers and peak search
//! stay on the device; only small peak grids, indices and energies return.
//! A measured CPU/GPU comparison chooses the default, not device presence.
use anyhow::Result;
use burn::tensor::{Device, Int, Tensor, TensorData};

/// A movie's patch planes resident on the device.
pub struct Movie {
    base: Tensor<3>,
    frames: usize,
}

pub struct Batch {
    dev: Device,
    n: usize,
    m: usize,
    window: Tensor<3>,
    filter: Tensor<3>,
    keep: Tensor<1, Int>,
}
impl Batch {
    pub fn new(dev: &Device, n: usize) -> Self {
        let m = n / 2;
        let (win, filter) = crate::align::patch_window(n);
        Self {
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
        let (n, k) = (self.n, pairs.len());
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
        let profile = std::env::var_os("TMV_GPU_PROFILE").is_some();
        let clock = std::time::Instant::now();
        let mut marks: Vec<(&str, f64)> = vec![];
        let mut mark = |name: &'static str, t: Option<Tensor<3>>| {
            if profile {
                if let Some(t) = t { let _: f32 = t.slice([0..1, 0..1, 0..1]).into_scalar(); }
                marks.push((name, clock.elapsed().as_secs_f64() * 1e3));
            }
        };
        let parts: Vec<&[f32]> = pairs.iter().map(|p| p.0.as_slice()).chain(pairs.iter().map(|p| p.1.as_slice())).collect();
        let x = match crate::motion::sum_gpu::client_of(&self.dev) {
            Ok(client) => Tensor::<3>::from_data(
                TensorData::from_bytes(crate::motion::sum_gpu::pinned(&client, &parts), [2 * k, n, n], burn::tensor::DType::F32),
                &self.dev,
            ),
            Err(_) => Tensor::<3>::from_data(TensorData::new(parts.concat(), [2 * k, n, n]), &self.dev),
        };
        mark("upload", Some(x.clone()));
        let out = self.correlate_x(x, k, max);
        mark("peak", None);
        if profile { eprintln!("PATCH_PROFILE k={k} n={n} {marks:?}"); }
        out
    }

    /// Pairs already on the device: the first `k` planes of `x` [2k, n, n]
    /// are the references, the last `k` the images.
    fn correlate_x(&self, x: Tensor<3>, k: usize, max: f64) -> Result<Vec<([f64; 2], f64)>> {
        let (n, m) = (self.n, self.m);
        let mean = x.clone().mean_dim(1).mean_dim(2);
        let x = (x - mean) * self.window.clone();
        let zeros = Tensor::<3>::zeros([2*k,n,n], &self.dev);
        let (fr, fi) = burn::signal::cfft(x, zeros, 2, None);
        let (fr, fi) = burn::signal::cfft(fr, fi, 1, None);
        let band = |x: Tensor<3>| x.select(1, self.keep.clone()).select(2, self.keep.clone());
        let (fr, fi) = (band(fr), band(fi));
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

    /// A movie's patches uploaded once: `base` holds `patches × frames`
    /// planes, patch-major. Every iteration of the local solver then moves,
    /// sums and correlates them on the device; only peaks come back. (Built
    /// on the host each iteration, the pairs were 128 MB of upload a pass,
    /// about 90 % of the GPU time.)
    pub fn movie(&self, base: &[Vec<f32>], frames: usize) -> Result<Movie> {
        let (n, b) = (self.n, base.len());
        anyhow::ensure!(frames > 1 && b % frames == 0 && base.iter().all(|v| v.len() == n * n), "局部 patch 尺寸不一致");
        anyhow::ensure!(b * n * n <= 1 << 27, "局部 patch 超过 GPU 批次预算");
        let parts: Vec<&[f32]> = base.iter().map(Vec::as_slice).collect();
        let client = crate::motion::sum_gpu::client_of(&self.dev)?;
        let base = Tensor::<3>::from_data(
            TensorData::from_bytes(crate::motion::sum_gpu::pinned(&client, &parts), [b, n, n], burn::tensor::DType::F32),
            &self.dev,
        );
        Ok(Movie { base, frames })
    }

    /// One pass of the local solver on the device: every plane moved by its
    /// shift (the CPU's `movie_shift`), each correlated against the mean of
    /// the other moved frames of its patch.
    pub fn correlate_movie(&self, mv: &Movie, shifts: &[[f64; 2]], max: f64) -> Result<Vec<([f64; 2], f64)>> {
        let (n, b, nf) = (self.n, mv.base.dims()[0], mv.frames);
        anyhow::ensure!(shifts.len() == b, "局部位移数量不一致");
        let aligned = crate::motion::sum_gpu::shift_planes(&self.dev, &mv.base, shifts)?;
        let p = b / nf;
        let moved = aligned.reshape([p, nf, n, n]);
        let sums = moved.clone().sum_dim(1);
        let reference = ((sums - moved) / (nf - 1) as f32).reshape([b, n, n]);
        let batch = ((1usize << 24) / (2 * n * n)).max(1);
        let mut out = Vec::with_capacity(b);
        let mut at = 0;
        while at < b {
            let k = batch.min(b - at);
            let x = Tensor::cat(vec![reference.clone().narrow(0, at, k), mv.base.clone().narrow(0, at, k)], 0);
            out.extend(self.correlate_x(x, k, max)?);
            at += k;
        }
        Ok(out)
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
    fn gpu_movie_matches_host_pairs() {
        let Some(dev)=device() else{return;};
        let (n,patches,nf)=(64,3,6);
        let src=pairs(n,patches);
        // Each patch: its frames drift a little, then a non-zero estimate.
        let base:Vec<Vec<f32>>=(0..patches).flat_map(|p|(0..nf).map(move |f|(p,f))).map(|(p,f)|crate::align::movie_shift(&src[p].0,n,n,[0.7*f as f64,-0.4*f as f64])).collect();
        let shifts:Vec<[f64;2]>=(0..patches*nf).map(|i|[-0.6*(i%nf) as f64+0.13,0.35*(i%nf) as f64-0.2]).collect();
        let max=n as f64/8.0;
        let aligned:Vec<_>=base.iter().zip(&shifts).map(|(v,&d)|crate::align::movie_shift(v,n,n,d)).collect();
        let reference:Vec<_>=(0..patches*nf).map(|i|{
            let p=i/nf;let mut sum=vec![0f32;n*n];
            for v in &aligned[p*nf..(p+1)*nf]{for (o,&v) in sum.iter_mut().zip(v){*o+=v;}}
            let r:Vec<f32>=sum.iter().zip(&aligned[i]).map(|(a,b)|(a-b)/(nf-1) as f32).collect();
            crate::align::patch_shift(&r,&base[i],n,max)
        }).collect();
        let b=Batch::new(dev,n);
        let mv=b.movie(&base,nf).unwrap();
        let got=b.correlate_movie(&mv,&shifts,max).unwrap();
        for (i,(a,c)) in got.iter().zip(&reference).enumerate() {
            let error=(a.0[0]-c.0[0]).hypot(a.0[1]-c.0[1]);
            assert!(error<0.05 && (a.1-c.1).abs()<0.01,"i={i} error={error} gpu={a:?} cpu={c:?}");
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
        // A movie's worth resident: 25 patches × 40 frames of 128².
        let (n,nf)=(128,40);let src=pairs(n,1000);
        let base:Vec<Vec<f32>>=src.into_iter().map(|p|p.1).collect();
        let shifts=vec![[0.3,-0.2];base.len()];let max=n as f64/8.;
        let b=Batch::new(dev,n);let mv=b.movie(&base,nf).unwrap();let _=b.correlate_movie(&mv,&shifts,max).unwrap();
        for _ in 0..3 {
            let t=std::time::Instant::now();let mv=b.movie(&base,nf).unwrap();let up=t.elapsed().as_secs_f64()*1e3;
            let t=std::time::Instant::now();let _=b.correlate_movie(&mv,&shifts,max).unwrap();
            eprintln!("PATCH_MOVIE_BENCH upload={up:.1}ms pass={:.1}ms",t.elapsed().as_secs_f64()*1e3);
        }
    }
}
