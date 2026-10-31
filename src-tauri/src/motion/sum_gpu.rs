//! The method of `sum.rs` on the GPU. Per frame: one upload, the varying
//! part of the local model resampled with Burn's grid sampler, a half-plane
//! FFT (native kernels), the phase shift and dose weight as elementwise ops
//! added into device accumulators. Only the final sums come back.
use super::{
    check_cancel, dose, fourier,
    input::Spool,
    local::Model,
    sum::{border, fill_border, fractional, freq, nyquist, split, varies, Sums},
};
use anyhow::Result;
use burn::tensor::ops::{GridSampleOptions, GridSamplePaddingMode, InterpolateMode};
use burn::tensor::{Device, Tensor, TensorData};

pub fn download(t: Tensor<2>) -> Vec<f32> {
    t.into_data()
        .convert::<f32>()
        .try_into_vec::<f32>()
        .unwrap()
}

/// Normalised sampling coordinates of every pixel, x then y, [-1, 1].
struct Coords {
    x: Tensor<2>,
    y: Tensor<2>,
}

impl Coords {
    fn new(w: usize, h: usize, dev: &Device) -> Coords {
        let line = |n: usize| (0..n).map(|i| 2.0 * i as f32 / (n - 1) as f32 - 1.0).collect::<Vec<_>>();
        let x = Tensor::<2>::from_data(TensorData::new(line(w), [1, w]), dev).expand([h, w]);
        let y = Tensor::<2>::from_data(TensorData::new(line(h), [h, 1]), dev).expand([h, w]);
        Coords { x, y }
    }

    /// Resamples v at x + l(x), l the polynomial in pixels.
    fn warp(&self, v: Tensor<2>, c: &[[f32; 6]; 2], w: usize, h: usize) -> Tensor<2> {
        let (x, y) = (self.x.clone(), self.y.clone());
        let poly = |c: &[f32; 6]| {
            x.clone().mul_scalar(c[1])
                + y.clone().mul_scalar(c[2])
                + (x.clone() * x.clone()).mul_scalar(c[3])
                + (x.clone() * y.clone()).mul_scalar(c[4])
                + (y.clone() * y.clone()).mul_scalar(c[5])
        };
        // Pixels to normalised units (align_corners: ±1 are the edge pixel centres).
        let gx = x.clone() + poly(&c[0]).mul_scalar(2.0 / (w - 1) as f32);
        let gy = y.clone() + poly(&c[1]).mul_scalar(2.0 / (h - 1) as f32);
        let grid = Tensor::stack::<3>(vec![gx, gy], 2).unsqueeze_dim::<4>(0);
        let options = GridSampleOptions::new(InterpolateMode::Bilinear)
            .with_padding_mode(GridSamplePaddingMode::Zeros)
            .with_align_corners(true);
        v.reshape([1, 1, h, w]).grid_sample_2d(grid, options).reshape([h, w])
    }
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
    let (w, h) = (sp.w, sp.h);
    let (fw, fh) = (w.next_power_of_two(), h.next_power_of_two());
    let hw = fw / 2 + 1;
    let coords = Coords::new(w, h, dev);
    let kx = Tensor::<2>::from_data(TensorData::new((0..hw).map(|i| i as f32).collect(), [1, hw]), dev);
    let ky = Tensor::<2>::from_data(TensorData::new((0..fh).map(|i| freq(i, fh) as f32).collect(), [fh, 1]), dev);
    // Per axis: all ones, or zero at the Nyquist bin (fractional shifts).
    let mask = |n: usize, len: usize, shape: [usize; 2]| {
        [false, true].map(|drop| {
            let v = (0..len).map(|i| if drop && nyquist(i, n) { 0.0f32 } else { 1.0 }).collect();
            Tensor::<2>::from_data(TensorData::new(v, shape), dev)
        })
    };
    let (keep_x, keep_y) = (mask(fw, hw, [1, hw]), mask(fh, fh, [fh, 1]));
    let rates = if exposures.is_some() {
        let v = (0..fh * hw)
            .map(|i| {
                let k = (freq(i % hw, fw) / fw as f64).hypot(freq(i / hw, fh) / fh as f64) / pixel;
                dose::critical(k, kv).map(|n| (0.5 / n) as f32)
            })
            .collect::<Result<Vec<_>>>()?;
        Some(Tensor::<2>::from_data(TensorData::new(v, [fh, hw]), dev))
    } else {
        None
    };
    let zeros = || Tensor::<2>::zeros([fh, hw], dev);
    let (mut er, mut ei, mut or, mut oi) = (zeros(), zeros(), zeros(), zeros());
    let (mut wr, mut wi, mut norm) = (zeros(), zeros(), zeros());
    let mut shifts = vec![];
    for (i, &g) in global.iter().enumerate() {
        check_cancel()?;
        let frame = sp.read(i)?;
        let mean = (frame.iter().map(|&a| a as f64).sum::<f64>() / frame.len() as f64) as f32;
        let (c, s) = split(model, sp.times[i], g);
        shifts.push(s);
        let mut v = Tensor::<2>::from_data(TensorData::new(frame, [h, w]), dev);
        if varies(&c) {
            v = coords.warp(v, &c, w, h);
        }
        if w != fw || h != fh {
            v = v.pad([(0, fh - h), (0, fw - w)], burn::tensor::ops::PadMode::Constant(mean));
        }
        let (re, im) = fourier::rfft2(v);
        let theta = kx.clone().mul_scalar((std::f64::consts::TAU * s[0] / fw as f64) as f32)
            + ky.clone().mul_scalar((std::f64::consts::TAU * s[1] / fh as f64) as f32);
        let keep = keep_x[fractional(s[0]) as usize].clone() * keep_y[fractional(s[1]) as usize].clone();
        let (sn, cs) = (theta.clone().sin() * keep.clone(), theta.cos() * keep);
        let r = re.clone() * cs.clone() - im.clone() * sn.clone();
        let m = re * sn + im * cs;
        if let (Some(exposure), Some(rate)) = (exposures, &rates) {
            let (start, d) = exposure[i];
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
        progress("变形求和 GPU", i + 1, sp.len());
    }
    let b = border(&shifts);
    let back = |re: Tensor<2>, im: Tensor<2>| {
        let mut out = download(fourier::irfft2(re, im, fw).slice([0..h, 0..w]));
        fill_border(&mut out, w, h, b);
        out
    };
    let weighted = if exposures.is_some() {
        let scale = norm.sqrt().clamp_min(1e-12).recip().mul_scalar((sp.len() as f32).sqrt());
        Some(back(wr * scale.clone(), wi * scale))
    } else {
        None
    };
    Ok(Sums {
        unweighted: back(er.clone() + or.clone(), ei.clone() + oi.clone()),
        even: back(er, ei),
        odd: back(or, oi),
        weighted,
    })
}
