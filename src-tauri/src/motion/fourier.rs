//! 2D Fourier transforms: the CPU reference uses the viewer's FFT plans,
//! the GPU path Burn's native FFT kernels.
use crate::fft::Fft;
use burn::tensor::{Device, Tensor};
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
/// Burn's native FFT kernels (burn-signal, cubek): radix kernels on the
/// device, no per-stage launches from here. Inverse uses ifft(z) =
/// conj(fft(conj z)) / N.
pub struct Gpu {
    w: usize,
    h: usize,
}
impl Gpu {
    pub fn new(w: usize, h: usize, _dev: &Device) -> Self {
        Self { w, h }
    }
    pub fn transform(&self, re: Tensor<2>, im: Tensor<2>, inverse: bool) -> (Tensor<2>, Tensor<2>) {
        let im = if inverse { im.neg() } else { im };
        let (re, im) = burn::signal::cfft(re, im, 1, None);
        let (re, im) = burn::signal::cfft(re, im, 0, None);
        if inverse {
            let n = (self.w * self.h) as f32;
            (re.div_scalar(n), im.div_scalar(-n))
        } else {
            (re, im)
        }
    }
}

/// Half-plane 2D transform of a real image (width w/2 + 1 along x).
pub fn rfft2(v: Tensor<2>) -> (Tensor<2>, Tensor<2>) {
    let (re, im) = burn::signal::rfft(v, 1, None);
    burn::signal::cfft(re, im, 0, None)
}

/// Inverse of `rfft2`, back to a real h × w image.
pub fn irfft2(re: Tensor<2>, im: Tensor<2>, w: usize) -> Tensor<2> {
    let h = re.dims()[0] as f32;
    let (re, im) = burn::signal::cfft(re, im.neg(), 0, None);
    burn::signal::irfft(re.div_scalar(h), im.div_scalar(-h), 1, Some(w))
}
