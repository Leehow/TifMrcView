//! GPU side of denoising (Pro): non-local means with Burn on the device SAM uses (Metal, DX12, Vulkan or CUDA). The
//! arithmetic is that of the CPU code in `denoise`, which is the fallback
//! when there is no GPU or a kernel fails.

use burn::tensor::module::avg_pool2d;
use burn::tensor::ops::{AvgPoolOptions, PadMode};
use burn::tensor::{Device, Tensor, TensorData};

use crate::denoise::{Method, Params};

/// Largest batch of shifted images NLM builds at once, in elements (f32).
const MAX_ELEMS: usize = 1 << 24;

/// Denoises a `w` × `h` image; only for NLM.
pub fn filter(dev: &Device, v: &[f32], w: usize, h: usize, p: &Params, sigma: f32) -> Vec<f32> {
    let x = Tensor::<2>::from_data(TensorData::new(v.to_vec(), [h, w]), dev);
    let out = match p.method {
        Method::Nlm => nlm(
            x,
            w,
            h,
            p.patch.min(5),
            p.search.min(20),
            p.strength * sigma,
            sigma,
        ),
        m => panic!("{m:?} has no GPU version"),
    };
    out.into_data()
        .convert::<f32>()
        .try_into_vec::<f32>()
        .unwrap()
}

fn nlm(
    v: Tensor<2>,
    w: usize,
    h: usize,
    patch: usize,
    search: usize,
    hh: f32,
    sigma: f32,
) -> Tensor<2> {
    let inv_h2 = 1.0 / (hh * hh).max(f32::MIN_POSITIVE);
    let two_s2 = 2.0 * sigma * sigma;
    let (s, pt) = (search, patch);
    let m = s + pt;
    let padded = v.pad([(m, m), (m, m)], PadMode::Edge);
    // Each pixel with the patch margin around it, and the same for a shift.
    let (hp, wp) = (h + 2 * pt, w + 2 * pt);
    let base = padded.clone().slice([s..s + hp, s..s + wp]);
    let offsets: Vec<(usize, usize)> = (0..=2 * s)
        .flat_map(|oy| (0..=2 * s).map(move |ox| (oy, ox)))
        .collect();
    let batch = (MAX_ELEMS / (hp * wp)).clamp(1, offsets.len());
    let pool = AvgPoolOptions::new([2 * pt + 1, 2 * pt + 1]).with_stride([1, 1]);
    let mut num: Option<Tensor<4>> = None;
    let mut den: Option<Tensor<4>> = None;
    for group in offsets.chunks(batch) {
        let k = group.len();
        let shifted: Vec<Tensor<2>> = group
            .iter()
            .map(|&(oy, ox)| padded.clone().slice([oy..oy + hp, ox..ox + wp]))
            .collect();
        let shifted = Tensor::stack::<3>(shifted, 0).reshape([k, 1, hp, wp]);
        let e = shifted.clone() - base.clone().reshape([1, 1, hp, wp]);
        let d = avg_pool2d(e.clone() * e, pool.clone());
        let wt = d
            .sub_scalar(two_s2)
            .clamp_min(0.0)
            .mul_scalar(-inv_h2)
            .exp();
        let vals = shifted.slice([0..k, 0..1, pt..pt + h, pt..pt + w]);
        let n = (wt.clone() * vals).sum_dim(0);
        let dsum = wt.sum_dim(0);
        num = Some(match num {
            Some(a) => a + n,
            None => n,
        });
        den = Some(match den {
            Some(a) => a + dsum,
            None => dsum,
        });
    }
    (num.unwrap() / den.unwrap()).reshape([h, w])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::denoise::{self, tests::phantom};

    /// Burn's CPU device runs the same tensor code as a GPU would.
    #[test]
    fn matches_the_cpu_code() {
        let dev = Device::flex();
        let (w, h) = (48, 40);
        let (_, noisy) = phantom(w, h, 0.3);
        let sigma = denoise::noise_sigma(&noisy, w, h);
        for (method, strength) in [(Method::Nlm, 0.8)] {
            let p = Params {
                method,
                strength,
                iterations: 8,
                patch: 1,
                search: 4,
                slab: 0,
                device: String::new(),
            };
            let gpu = filter(&dev, &noisy, w, h, &p, sigma);
            let (cpu, _) = denoise::filter(
                noisy.clone(),
                w,
                h,
                &Params {
                    device: "cpu".into(),
                    ..p.clone()
                },
                sigma,
            )
            .unwrap();
            // Patch sums differ within `patch` of the border (padding vs repeated sums).
            let b = if method == Method::Nlm { 2 } else { 0 };
            let worst = (b..h - b)
                .flat_map(|y| (b..w - b).map(move |x| y * w + x))
                .map(|i| (gpu[i] - cpu[i]).abs())
                .fold(0f32, f32::max);
            assert!(worst < 1e-3, "{method:?}: {worst}");
        }
    }
}
