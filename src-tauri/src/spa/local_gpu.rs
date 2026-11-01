//! The local orientation search of 3D refinement on the GPU (Burn).
//!
//! For a block of particles and their candidate rotations, the slices of
//! the padded reference transform are cut on the device (trilinear: eight
//! gathers from the flattened volume), then scored against every shifted
//! copy of each particle with one batched matrix product, as in
//! `score.rs`: NCC = Σ Re(X·conj P)·C·w / √(Σ C²w |P|²). The CPU version
//! (`refine::local`) spent most of a round in the interpolation.

use anyhow::Result;
use burn::tensor::{Device, Int, Tensor, TensorData};

use super::data::Coeffs;
use super::fourier::Half;
use super::geom::Mat;
use super::volume::Projector;

pub struct LocalGpu {
    dev: Device,
    n: usize,
    re: Tensor<1>,
    im: Tensor<1>,
    /// Slice coordinates of the half plane on the padded grid, [1, k].
    kx: Tensor<2>,
    ky: Tensor<2>,
    k: usize,
}

/// Best candidate per particle: (rotation index, shift index, score).
pub type Found = (usize, usize, f32);

impl LocalGpu {
    pub fn new(dev: &Device, proj: &Projector, half: &Half) -> LocalGpu {
        let (re, im) = proj.data();
        let up1 = |v: &[f32]| Tensor::<1>::from_data(TensorData::new(v.to_vec(), [v.len()]), dev);
        let p = proj.pad as f32;
        let k = half.len();
        let kx: Vec<f32> = half.k.iter().map(|q| q[0] * p).collect();
        let ky: Vec<f32> = half.k.iter().map(|q| q[1] * p).collect();
        LocalGpu {
            dev: dev.clone(),
            n: proj.n,
            re: up1(re),
            im: up1(im),
            kx: Tensor::from_data(TensorData::new(kx, [1, k]), dev),
            ky: Tensor::from_data(TensorData::new(ky, [1, k]), dev),
            k,
        }
    }

    /// Slices at `rots` (r rows), as (re, im) [r, k].
    pub(crate) fn slices(&self, rots: &[Mat]) -> (Tensor<2>, Tensor<2>) {
        let r = rots.len();
        let n = self.n as f32;
        let h = (self.n / 2) as f32 - 1.0;
        let col = |f: &dyn Fn(&Mat) -> f64| Tensor::<2>::from_data(TensorData::new(rots.iter().map(|m| f(m) as f32).collect::<Vec<_>>(), [r, 1]), &self.dev);
        // q = Rᵀ·(kx, ky, 0): q_i = R[0][i]·kx + R[1][i]·ky.
        let q: Vec<Tensor<2>> = (0..3).map(|i| col(&|m| m[0][i]) * self.kx.clone() + col(&|m| m[1][i]) * self.ky.clone()).collect();
        let inside = q
            .iter()
            .map(|t| t.clone().abs().lower_equal_elem(h).float())
            .reduce(|a, b| a * b)
            .unwrap();
        let fl: Vec<Tensor<2>> = q.iter().map(|t| t.clone().floor()).collect();
        let fr: Vec<Tensor<2>> = q.iter().zip(&fl).map(|(t, f)| t.clone() - f.clone()).collect();
        // Index of a frequency on the unshifted grid: f < 0 wraps to f + n.
        let wrap = |f: Tensor<2>| f.clone() + f.lower_elem(0.0).float() * n;
        let lo: Vec<Tensor<2>> = fl.into_iter().map(wrap).collect();
        let hi: Vec<Tensor<2>> = lo.iter().map(|t| {
            let u = t.clone() + 1.0;
            u.clone() - u.greater_equal_elem(n).float() * n
        }).collect();
        let (mut out_re, mut out_im): (Option<Tensor<2>>, Option<Tensor<2>>) = (None, None);
        for corner in 0..8 {
            let pick = |a: usize| corner >> a & 1 == 1;
            let (ix, iy, iz) = (
                if pick(0) { &hi[0] } else { &lo[0] },
                if pick(1) { &hi[1] } else { &lo[1] },
                if pick(2) { &hi[2] } else { &lo[2] },
            );
            let w = |a: usize| if pick(a) { fr[a].clone() } else { fr[a].clone().neg() + 1.0 };
            let weight = w(0) * w(1) * w(2) * inside.clone();
            let idx: Tensor<1, Int> = ((iz.clone() * n + iy.clone()) * n + ix.clone()).reshape([r * self.k]).int();
            let a = self.re.clone().select(0, idx.clone()).reshape([r, self.k]) * weight.clone();
            let b = self.im.clone().select(0, idx).reshape([r, self.k]) * weight;
            out_re = Some(match out_re { Some(t) => t + a, None => a });
            out_im = Some(match out_im { Some(t) => t + b, None => b });
        }
        (out_re.unwrap(), out_im.unwrap())
    }

    /// For each particle of `c` (with `s` shifts), the best of its `d`
    /// candidate rotations (`rots`, particle-major) and shifts.
    pub fn search(&self, c: &Coeffs, s: usize, rots: &[Mat], d: usize) -> Result<Vec<Found>> {
        let k = self.k;
        let n = c.c2.len() / k.max(1);
        let mut out = Vec::with_capacity(n);
        // Blocks of particles with at most 2^23 slice values.
        let pb = ((1usize << 23) / (d * k).max(1)).clamp(1, 256);
        for p0 in (0..n).step_by(pb) {
            let p1 = (p0 + pb).min(n);
            let b = p1 - p0;
            let (vr, vi) = self.slices(&rots[p0 * d..p1 * d]);
            let (vr, vi) = (vr.reshape([b, d, k]), vi.reshape([b, d, k]));
            let mut xr = vec![0f32; b * s * k];
            let mut xi = vec![0f32; b * s * k];
            for (j, chunk) in c.x[p0 * s * 2 * k..p1 * s * 2 * k].chunks_exact(2).enumerate() {
                xr[j] = chunk[0];
                xi[j] = chunk[1];
            }
            let xr: Tensor<3> = Tensor::from_data(TensorData::new(xr, [b, s, k]), &self.dev);
            let xi: Tensor<3> = Tensor::from_data(TensorData::new(xi, [b, s, k]), &self.dev);
            let c2: Tensor<3> = Tensor::from_data(TensorData::new(c.c2[p0 * k..p1 * k].to_vec(), [b, 1, k]), &self.dev);
            let cross = xr.matmul(vr.clone().swap_dims(1, 2)) + xi.matmul(vi.clone().swap_dims(1, 2));
            let p2 = vr.clone() * vr + vi.clone() * vi;
            let norm = (p2 * c2).sum_dim(2).clamp_min(1e-20).sqrt().reshape([b, 1, d]);
            let sc = (cross / norm).reshape([b, s * d]);
            let max = sc.clone().max_dim(1).into_data().convert::<f32>().try_into_vec::<f32>().map_err(|e| anyhow::anyhow!("{e:?}"))?;
            let arg = sc.argmax(1).into_data().convert::<i64>().try_into_vec::<i64>().map_err(|e| anyhow::anyhow!("{e:?}"))?;
            for i in 0..b {
                let a = arg[i] as usize;
                out.push((a % d, a / d, max[i]));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spa::geom;

    #[test]
    fn slices_and_search_match_the_cpu() {
        let dev = Device::flex();
        let b = 32;
        // A lumpy test object.
        let mut v = vec![0f32; b * b * b];
        for (i, x) in v.iter_mut().enumerate() {
            let (z, y, xx) = ((i / (b * b)) as f32, ((i / b) % b) as f32, (i % b) as f32);
            let d = |cx: f32, cy: f32, cz: f32| (-((xx - cx).powi(2) + (y - cy).powi(2) + (z - cz).powi(2)) / 6.0).exp();
            *x = d(14.0, 16.0, 16.0) + 0.7 * d(19.0, 13.0, 17.0) + 0.5 * d(16.0, 19.0, 12.0);
        }
        let proj = Projector::new(&v, b, 2);
        let half = Half::new(b, b as f64 / 2.0 - 1.0);
        let g = LocalGpu::new(&dev, &proj, &half);
        let rots: Vec<Mat> = geom::grid(30.0).into_iter().step_by(11).take(6).collect();
        let (re, im) = g.slices(&rots);
        let re = re.into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
        let im = im.into_data().convert::<f32>().try_into_vec::<f32>().unwrap();
        let k = half.len();
        let mut buf = vec![0f32; 2 * k];
        let mut worst = 0f32;
        for (r, m) in rots.iter().enumerate() {
            proj.slice(m, &half, &mut buf);
            for j in 0..k {
                worst = worst.max((buf[2 * j] - re[r * k + j]).abs()).max((buf[2 * j + 1] - im[r * k + j]).abs());
            }
        }
        let scale = buf.iter().fold(0f32, |a, x| a.max(x.abs()));
        assert!(worst < 1e-3 * scale, "slice error {worst} of {scale}");
        // Two particles that are slices at rotations 4 and 1 of their own
        // candidate lists; one shift each.
        let mut x = vec![];
        for &t in &[4usize, 1] {
            proj.slice(&rots[t], &half, &mut buf);
            x.extend_from_slice(&buf);
        }
        let c = Coeffs { k, x, c2: vec![1.0; 2 * k] };
        let cand: Vec<Mat> = rots.iter().chain(rots.iter()).copied().collect();
        let found = g.search(&c, 1, &cand, rots.len()).unwrap();
        assert_eq!((found[0].0, found[1].0), (4, 1), "{found:?}");
    }
}
