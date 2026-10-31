//! Burn polynomial gather-warp and streaming sums. One group upload, no
//! per-pixel/FFT-stage readback, and final sum downloads only. Slab warping
//! bounds 8K intermediates; frequency weighting stays entirely on device.
use super::{check_cancel, dose, fourier, input::Spool, local::Model, sum::Sums};
use anyhow::Result;
use burn::tensor::{Device, Tensor, TensorData};

pub fn download(t: Tensor<2>) -> Vec<f32> {
    t.into_data()
        .convert::<f32>()
        .try_into_vec::<f32>()
        .unwrap()
}
pub fn warp(
    dev: &Device,
    v: &[f32],
    w: usize,
    h: usize,
    g: [f64; 2],
    model: &Model,
    time: f64,
) -> Tensor<2> {
    let source = Tensor::<1>::from_data(TensorData::new(v.to_vec(), [w * h]), dev);
    let c = model.spatial(time);
    let x = Tensor::<2>::from_data(
        TensorData::new((0..w).map(|x| x as f32).collect(), [1, w]),
        dev,
    );
    let xx = x
        .clone()
        .div_scalar((w - 1) as f32)
        .mul_scalar(2.0)
        .sub_scalar(1.0);
    let mut slabs = vec![];
    let rows = ((1usize << 22) / w).max(1).min(h);
    for start in (0..h).step_by(rows) {
        let n = rows.min(h - start);
        let y = Tensor::<2>::from_data(
            TensorData::new((start..start + n).map(|y| y as f32).collect(), [n, 1]),
            dev,
        );
        let yy = y
            .clone()
            .div_scalar((h - 1) as f32)
            .mul_scalar(2.0)
            .sub_scalar(1.0);
        let poly = |k: usize| {
            xx.clone().mul_scalar(c[k][1])
                + yy.clone().mul_scalar(c[k][2])
                + (xx.clone() * xx.clone()).mul_scalar(c[k][3])
                + (xx.clone() * yy.clone()).mul_scalar(c[k][4])
                + (yy.clone() * yy.clone()).mul_scalar(c[k][5])
                + c[k][0]
                + g[k] as f32
        };
        let sx = x.clone() + poly(0);
        let sy = y + poly(1);
        let valid = sx
            .clone()
            .greater_equal_elem(0.0)
            .bool_and(sx.clone().lower_equal_elem((w - 1) as f32))
            .bool_and(sy.clone().greater_equal_elem(0.0))
            .bool_and(sy.clone().lower_equal_elem((h - 1) as f32));
        let sx = sx.clamp(0.0, (w - 1) as f32);
        let sy = sy.clamp(0.0, (h - 1) as f32);
        let ix = sx.clone().floor();
        let iy = sy.clone().floor();
        let fx = sx - ix.clone();
        let fy = sy - iy.clone();
        let jx = (ix.clone() + 1.0).clamp_max((w - 1) as f32);
        let jy = (iy.clone() + 1.0).clamp_max((h - 1) as f32);
        let get = |x: Tensor<2>, y: Tensor<2>| {
            source
                .clone()
                .select(0, (y.int().mul_scalar(w as i32) + x.int()).reshape([n * w]))
                .reshape([n, w])
        };
        let a = get(ix.clone(), iy.clone());
        let b = get(jx.clone(), iy);
        let cc = get(ix, jy.clone());
        let d = get(jx, jy);
        let top = a * (fx.clone().neg() + 1.0) + b * fx.clone();
        let bottom = cc * (fx.clone().neg() + 1.0) + d * fx;
        slabs.push((top * (fy.clone().neg() + 1.0) + bottom * fy) * valid.float());
    }
    Tensor::cat(slabs, 0)
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
    let mut even = Tensor::<2>::zeros([h, w], dev);
    let mut odd = even.clone();
    let mut sr = Tensor::<2>::zeros([fh, fw], dev);
    let mut si = sr.clone();
    let mut norm = sr.clone();
    let fft = exposures.map(|_| fourier::Gpu::new(fw, fh, dev));
    let rates = if exposures.is_some() {
        let v = (0..fw * fh)
            .map(|i| {
                let (x, y) = (i % fw, i / fw);
                let k = (x.min(fw - x) as f64 / fw as f64).hypot(y.min(fh - y) as f64 / fh as f64)
                    / pixel;
                dose::critical(k, kv).map(|n| (0.5 / n) as f32)
            })
            .collect::<Result<Vec<_>>>()?;
        Some(Tensor::<2>::from_data(TensorData::new(v, [fh, fw]), dev))
    } else {
        None
    };
    for (i, &g) in global.iter().enumerate() {
        check_cancel()?;
        let frame = sp.read(i)?;
        let v = warp(dev, &frame, w, h, g, model, sp.times[i]);
        if i % 2 == 0 {
            even = even + v.clone();
        } else {
            odd = odd + v.clone();
        }
        if let Some(exposure) = exposures {
            let v = if w != fw || h != fh {
                v.pad(
                    [(0, fh - h), (0, fw - w)],
                    burn::tensor::ops::PadMode::Constant(0.0),
                )
            } else {
                v
            };
            let (re, im) = fft
                .as_ref()
                .unwrap()
                .transform(v, Tensor::zeros([fh, fw], dev), false);
            let (start, d) = exposure[i];
            let rate = rates.as_ref().unwrap().clone();
            let z = rate.clone().mul_scalar(d as f32);
            let exact = (z.clone().neg().exp().neg() + 1.0) / z.clone().clamp_min(1e-20);
            let approx = z.clone().mul_scalar(-0.5) + 1.0 + (z.clone() * z.clone()).div_scalar(6.0)
                - (z.clone() * z.clone() * z.clone()).div_scalar(24.0);
            let q =
                rate.mul_scalar(-start as f32).exp() * exact.mask_where(z.lower_elem(1e-3), approx);
            sr = sr + re * q.clone();
            si = si + im * q.clone();
            norm = norm + q.clone() * q;
        }
        progress("变形求和 GPU", i + 1, sp.len());
    }
    let weighted = if let Some(fft) = fft {
        let scale = norm
            .sqrt()
            .clamp_min(1e-12)
            .recip()
            .mul_scalar((sp.len() as f32).sqrt());
        let (re, _) = fft.transform(sr * scale.clone(), si * scale, true);
        Some(download(re.slice([0..h, 0..w])))
    } else {
        None
    };
    let unweighted = download(even.clone() + odd.clone());
    Ok(Sums {
        unweighted,
        even: download(even),
        odd: download(odd),
        weighted,
    })
}
