//! The method of `sum.rs` on the GPU. Per frame: the tiles are cut on the
//! CPU (a copy), uploaded as one batch, transformed with Burn's native FFT,
//! phase shifted and dose weighted as elementwise ops and added into device
//! accumulators. The tiles come back once, at the end, to be blended.
use super::{
    check_cancel, dose,
    input::Spool,
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
    let mut batch = vec![0.0f32; n * t * t];
    for (i, &g) in global.iter().enumerate() {
        check_cancel()?;
        let frame = sp.read(i)?;
        let mean = (frame.iter().map(|&a| a as f64).sum::<f64>() / frame.len() as f64) as f32;
        let shifts: Vec<[f64; 2]> = (0..n).map(|k| plan.shift(k, model, sp.times[i], g)).collect();
        {
            use rayon::prelude::*;
            batch.par_chunks_mut(t * t).enumerate().for_each(|(k, out)| plan.cut(&frame, k, shifts[k], mean, out));
        }
        drop(frame);
        let fracs: Vec<[f64; 2]> = shifts.iter().map(|s| [s[0] - s[0].round(), s[1] - s[1].round()]).collect();
        let ax = up3(fracs.iter().map(|f| (std::f64::consts::TAU * f[0] / t as f64) as f32).collect(), [n, 1, 1]);
        let ay = up3(fracs.iter().map(|f| (std::f64::consts::TAU * f[1] / t as f64) as f32).collect(), [n, 1, 1]);
        // Nyquist bins of fractionally shifted axes drop out (see sum.rs).
        let keep_x = up3(fracs.iter().flat_map(|f| (0..hw).map(move |j| if fractional(f[0]) && nyquist(j, t) { 0.0 } else { 1.0 })).collect(), [n, 1, hw]);
        let keep_y = up3(fracs.iter().flat_map(|f| (0..t).map(move |j| if fractional(f[1]) && nyquist(j, t) { 0.0 } else { 1.0 })).collect(), [n, t, 1]);
        let x = up3(batch.clone(), [n, t, t]);
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
        progress("求和 GPU", i + 1, sp.len());
    }
    let back = |re: Tensor<3>, im: Tensor<3>| {
        let v = download(inverse(re, im, t));
        let tiles: Vec<Vec<f32>> = v.chunks(t * t).map(<[f32]>::to_vec).collect();
        plan.blend(&tiles)
    };
    let weighted = exposures.map(|_| {
        let scale = norm.sqrt().clamp_min(1e-12).recip().mul_scalar((sp.len() as f32).sqrt());
        back(wr * scale.clone(), wi * scale)
    });
    Ok(Sums {
        unweighted: back(er.clone() + or.clone(), ei.clone() + oi.clone()),
        even: back(er, ei),
        odd: back(or, oi),
        weighted,
    })
}
