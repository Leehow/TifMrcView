//! Batched patch Fourier correlations on Burn. Both images of every patch
//! in a view pair share DFT matrices; one readback supplies all cross powers
//! and energies to the same CPU subpixel peak finder used by the reference.
//! A measured CPU/GPU comparison chooses the default, not device presence.
use anyhow::Result;
use burn::tensor::{Device, Int, Tensor, TensorData};

pub struct Batch {
    dev: Device,
    n: usize,
    m: usize,
    wr: Tensor<2>,
    wi: Tensor<2>,
    window: Tensor<3>,
    filter: Tensor<3>,
    keep: Tensor<1, Int>,
}
impl Batch {
    pub fn new(dev: &Device, n: usize) -> Self {
        let m = n / 2;
        let phase = |i: usize| -std::f64::consts::TAU * ((i / n) * (i % n) % n) as f64 / n as f64;
        let wr = Tensor::from_data(
            TensorData::new((0..n * n).map(|i| phase(i).cos() as f32).collect(), [n, n]),
            dev,
        );
        let wi = Tensor::from_data(
            TensorData::new((0..n * n).map(|i| phase(i).sin() as f32).collect(), [n, n]),
            dev,
        );
        let (win, filter) = crate::align::patch_window(n);
        Self {
            dev: dev.clone(),
            n,
            m,
            wr,
            wi,
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
        let batch = ((1usize << 22) / (2 * n * n)).max(1);
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
        let all: Vec<f32> = pairs
            .iter()
            .flat_map(|p| p.0.iter().copied())
            .chain(pairs.iter().flat_map(|p| p.1.iter().copied()))
            .collect();
        let x = Tensor::<3>::from_data(TensorData::new(all, [2 * k, n, n]), &self.dev);
        let mean = x.clone().mean_dim(1).mean_dim(2);
        let x = ((x - mean) * self.window.clone()).reshape([2 * k * n, n]);
        let ar = x.clone().matmul(self.wr.clone());
        let ai = x.matmul(self.wi.clone());
        let tr = |x: Tensor<2>| {
            x.reshape([2 * k, n, n])
                .swap_dims(1, 2)
                .reshape([2 * k * n, n])
        };
        let ar = tr(ar);
        let ai = tr(ai);
        let fr = ar.clone().matmul(self.wr.clone()) - ai.clone().matmul(self.wi.clone());
        let fi = ar.matmul(self.wi.clone()) + ai.matmul(self.wr.clone());
        let band = |x: Tensor<2>| {
            (x.reshape([2 * k, n, n])
                .swap_dims(1, 2)
                .select(1, self.keep.clone())
                .select(2, self.keep.clone())
                * self.filter.clone())
            .reshape([2 * k, m * m])
        };
        let fr = band(fr);
        let fi = band(fi);
        let ra = fr.clone().narrow(0, 0, k);
        let rb = fr.narrow(0, k, k);
        let ia = fi.clone().narrow(0, 0, k);
        let ib = fi.narrow(0, k, k);
        let ea = (ra.clone() * ra.clone() + ia.clone() * ia.clone()).sum_dim(1);
        let eb = (rb.clone() * rb.clone() + ib.clone() * ib.clone()).sum_dim(1);
        let pr = rb.clone() * ra.clone() + ib.clone() * ia.clone();
        let pi = ib * ra - rb * ia;
        let data = Tensor::cat(vec![pr, pi, ea, eb], 1)
            .into_data()
            .convert::<f32>()
            .try_into_vec::<f32>()
            .unwrap();
        let stride = 2 * m * m + 2;
        Ok(data
            .chunks(stride)
            .map(|r| {
                crate::align::patch_cross_peak(
                    &r[..m * m],
                    &r[m * m..2 * m * m],
                    n,
                    [r[2 * m * m] as f64, r[2 * m * m + 1] as f64],
                    max,
                )
            })
            .collect())
    }
}
