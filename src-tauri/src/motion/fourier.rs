//! In-house radix-2 Fourier transforms. CPU reference uses the viewer's
//! FFT plans. Burn uses batched gather butterflies, with no stage readbacks.
use crate::fft::Fft;
use burn::tensor::{Device, Int, Tensor, TensorData};
use rayon::prelude::*;

pub struct Cpu {
    w: usize,
    h: usize,
    x: Fft,
    y: Fft,
}
impl Cpu {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            x: Fft::new(w),
            y: Fft::new(h),
        }
    }
    pub fn transform(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        if inverse {
            im.par_iter_mut().for_each(|v| *v = -*v);
        }
        re.par_chunks_mut(self.w)
            .zip(im.par_chunks_mut(self.w))
            .for_each(|(r, i)| self.x.run(r, i));
        let mut tr = vec![0.0; re.len()];
        let mut ti = vec![0.0; im.len()];
        tr.par_chunks_mut(self.h)
            .zip(ti.par_chunks_mut(self.h))
            .enumerate()
            .for_each(|(x, (rr, ii))| {
                for y in 0..self.h {
                    rr[y] = re[y * self.w + x];
                    ii[y] = im[y * self.w + x];
                }
                self.y.run(rr, ii);
            });
        re.par_chunks_mut(self.w)
            .zip(im.par_chunks_mut(self.w))
            .enumerate()
            .for_each(|(y, (rr, ii))| {
                for x in 0..self.w {
                    rr[x] = tr[x * self.h + y];
                    ii[x] = ti[x * self.h + y];
                }
            });
        if inverse {
            let scale = (self.w * self.h) as f32;
            re.par_iter_mut().zip(im.par_iter_mut()).for_each(|(r, i)| {
                *r /= scale;
                *i = -*i / scale;
            });
        }
    }
}
struct Stage {
    low: Tensor<1, Int>,
    high: Tensor<1, Int>,
    wr: Tensor<1>,
    wi: Tensor<1>,
}
struct Axis {
    reverse: Tensor<1, Int>,
    stages: Vec<Stage>,
    n: usize,
}
impl Axis {
    fn new(n: usize, dev: &Device) -> Self {
        let rev = (0..n)
            .map(|i| (i.reverse_bits() >> (usize::BITS - n.ilog2())) as i32)
            .collect();
        let reverse = Tensor::from_data(TensorData::new(rev, [n]), dev);
        let mut stages = vec![];
        for b in 1..=n.ilog2() {
            let len = 1 << b;
            let half = len / 2;
            let low: Vec<i32> = (0..n).map(|i| (i / len * len + i % half) as i32).collect();
            let high = low.iter().map(|i| *i + half as i32).collect();
            let phase = |i: usize| -std::f64::consts::TAU * (i % half) as f64 / len as f64;
            let sign = |i: usize| if i % len < half { 1.0 } else { -1.0 };
            stages.push(Stage {
                low: Tensor::from_data(TensorData::new(low, [n]), dev),
                high: Tensor::from_data(TensorData::new(high, [n]), dev),
                wr: Tensor::from_data(
                    TensorData::new(
                        (0..n).map(|i| (sign(i) * phase(i).cos()) as f32).collect(),
                        [n],
                    ),
                    dev,
                ),
                wi: Tensor::from_data(
                    TensorData::new(
                        (0..n).map(|i| (sign(i) * phase(i).sin()) as f32).collect(),
                        [n],
                    ),
                    dev,
                ),
            });
        }
        Self { reverse, stages, n }
    }
    fn run(&self, re: Tensor<2>, im: Tensor<2>, dim: usize) -> (Tensor<2>, Tensor<2>) {
        let mut re = re.select(dim, self.reverse.clone());
        let mut im = im.select(dim, self.reverse.clone());
        let shape = if dim == 1 { [1, self.n] } else { [self.n, 1] };
        for s in &self.stages {
            let ar = re.clone().select(dim, s.low.clone());
            let ai = im.clone().select(dim, s.low.clone());
            let br = re.select(dim, s.high.clone());
            let bi = im.select(dim, s.high.clone());
            let wr = s.wr.clone().reshape(shape);
            let wi = s.wi.clone().reshape(shape);
            re = ar + br.clone() * wr.clone() - bi.clone() * wi.clone();
            im = ai + br * wi + bi * wr;
        }
        (re, im)
    }
}
pub struct Gpu {
    x: Axis,
    y: Axis,
    w: usize,
    h: usize,
}
impl Gpu {
    pub fn new(w: usize, h: usize, dev: &Device) -> Self {
        Self {
            x: Axis::new(w, dev),
            y: Axis::new(h, dev),
            w,
            h,
        }
    }
    pub fn transform(&self, re: Tensor<2>, im: Tensor<2>, inverse: bool) -> (Tensor<2>, Tensor<2>) {
        let im = if inverse { im.neg() } else { im };
        let (re, im) = self.x.run(re, im, 1);
        let (re, im) = self.y.run(re, im, 0);
        if inverse {
            (
                re.div_scalar((self.w * self.h) as f32),
                im.div_scalar(-((self.w * self.h) as f32)),
            )
        } else {
            (re, im)
        }
    }
}
