//! The whole-tomogram template search of `search` on the GPU (Burn).
//!
//! Burn has no Fourier transform that every backend runs, but it has 3D
//! convolution, and a convolution of the tomogram with the turned
//! templates as kernels is exactly the correlation wanted: a batch of
//! orientations is one `conv3d` with one output channel each, and the best
//! over the batch is taken on the device. This costs kernel volume per
//! voxel, so the search is made at the coarsest sampling that still shows
//! the particle (the kernel is the template cut to its mask, about a dozen
//! voxels across); the peaks are then refined on finer cubes by the
//! alignment code. Scores are those of `search`: divided by the
//! tomogram's standard deviation under the mask, template wedged at each
//! orientation.

use anyhow::Result;
use burn::tensor::module::conv3d;
use burn::tensor::ops::ConvOptions;
use burn::tensor::{Device, Tensor, TensorData};
use rayon::prelude::*;

use super::cc::{ball, rotate, wedge};
use super::check_cancel;
use super::search::Found;
use crate::spa::fourier::{ft3, ift3};
use crate::spa::geom::Mat;

/// Side of the part of the tomogram one convolution answers for.
const TILE: usize = 64;
/// Orientations in one convolution.
const BATCH: usize = 32;

/// The template at one orientation as a k³ kernel: masked, zero mean,
/// wedged, unit power.
fn kernel(template: &[f32], n: usize, mask: &[f32], wedge: &[f32], r: &Mat, k: usize) -> Vec<f32> {
    let v = rotate(template, n, r);
    let (mut sum, mut weight) = (0f64, 0f64);
    for i in 0..v.len() {
        sum += (v[i] * mask[i]) as f64;
        weight += mask[i] as f64;
    }
    let mean = (sum / weight) as f32;
    let masked: Vec<f32> = v.iter().zip(mask).map(|(v, m)| (v - mean) * m).collect();
    let (mut re, mut im) = ft3(n, &masked);
    for i in 0..re.len() {
        re[i] *= wedge[i];
        im[i] *= wedge[i];
    }
    re[0] = 0.0;
    im[0] = 0.0;
    let full = ift3(n, &re, &im);
    // k is odd: its middle voxel is the template's centre, n/2.
    let o = n / 2 - k / 2;
    let mut out = Vec::with_capacity(k * k * k);
    for z in 0..k {
        for y in 0..k {
            let row = ((z + o) * n + y + o) * n + o;
            out.extend_from_slice(&full[row..row + k]);
        }
    }
    // Zero mean and unit power over the kernel itself.
    let m = out.iter().map(|x| *x as f64).sum::<f64>() / out.len() as f64;
    let p = out.iter().map(|x| (*x as f64 - m).powi(2)).sum::<f64>().sqrt().max(1e-12);
    out.iter_mut().for_each(|x| *x = ((*x as f64 - m) / p) as f32);
    out
}

/// As `search::search`, on `dev`. The template is an n³ cube (n even); the
/// kernel is its middle, as wide as the mask.
#[allow(clippy::too_many_arguments)]
pub fn search(
    dev: &Device,
    v: &[f32],
    dims: [usize; 3],
    template: &[f32],
    n: usize,
    radius: f64,
    tilts: [f64; 2],
    turns: &[Mat],
    allowed: Option<&[bool]>,
    tick: &(dyn Fn(usize, usize) + Sync),
) -> Result<Found> {
    assert!(n.is_power_of_two() && template.len() == n * n * n);
    let [nx, ny, nz] = dims;
    assert_eq!(v.len(), nx * ny * nz);
    let radius = radius.min(n as f64 / 2.0 - 2.0);
    let mask = ball(n, radius, 2.0);
    let wedge = wedge(n, tilts[0], tilts[1]);
    // Odd, so the kernel has a centre voxel: that of the template, n/2.
    let k = ((2.0 * (radius + 2.0)).ceil() as usize | 1).min(n - 1);
    let h = k / 2;
    let o = n / 2 - h;
    let cut = |cube: &[f32]| -> Vec<f32> {
        let mut out = Vec::with_capacity(k * k * k);
        for z in 0..k {
            for y in 0..k {
                let row = ((z + o) * n + y + o) * n + o;
                out.extend_from_slice(&cube[row..row + k]);
            }
        }
        out
    };
    let mask_k = cut(&mask);
    let count: f32 = mask_k.iter().sum();
    let mean = (v.iter().map(|x| *x as f64).sum::<f64>() / v.len() as f64) as f32;
    let options = ConvOptions::new([1; 3], [0; 3], [1; 3], 1);
    let mask_t = Tensor::<5>::from_data(TensorData::new(mask_k, [1, 1, k, k, k]), dev);

    let along = |len: usize| -> Vec<usize> { (0..len.div_ceil(TILE)).map(|i| i * TILE).collect() };
    let mut tiles: Vec<[usize; 3]> = vec![];
    for z in along(nz) {
        for y in along(ny) {
            for x in along(nx) {
                tiles.push([x, y, z]);
            }
        }
    }
    let size = |t: [usize; 3]| -> [usize; 3] { std::array::from_fn(|i| TILE.min(dims[i] - t[i])) };
    if let Some(a) = allowed {
        tiles.retain(|t| {
            let s = size(*t);
            (t[2]..t[2] + s[2]).any(|z| (t[1]..t[1] + s[1]).any(|y| a[(z * ny + y) * nx + t[0]..(z * ny + y) * nx + t[0] + s[0]].iter().any(|b| *b)))
        });
    }
    // A tile with a border of half a kernel, filled with the mean outside.
    let fill = |t: [usize; 3]| -> (Vec<f32>, [usize; 3]) {
        let s = size(t);
        let p = [s[0] + 2 * h, s[1] + 2 * h, s[2] + 2 * h];
        let mut cube = vec![mean; p[0] * p[1] * p[2]];
        for z in 0..p[2] {
            let sz = (t[2] + z) as isize - h as isize;
            if sz < 0 || sz >= nz as isize {
                continue;
            }
            for y in 0..p[1] {
                let sy = (t[1] + y) as isize - h as isize;
                if sy < 0 || sy >= ny as isize {
                    continue;
                }
                for x in 0..p[0] {
                    let sx = (t[0] + x) as isize - h as isize;
                    if sx >= 0 && sx < nx as isize {
                        cube[(z * p[1] + y) * p[0] + x] = v[(sz as usize * ny + sy as usize) * nx + sx as usize];
                    }
                }
            }
        }
        (cube, p)
    };
    let pull = |t: Tensor<1>| -> Result<Vec<f32>> { t.into_data().convert::<f32>().try_into_vec::<f32>().map_err(|e| anyhow::anyhow!("{e:?}")) };

    let mut score = vec![f32::MIN; v.len()];
    let mut turn = vec![0u32; v.len()];
    let batches = turns.len().div_ceil(BATCH);
    // Kernels of all orientations, a batch per tensor.
    let mut kernels: Vec<(Tensor<5>, usize)> = vec![];
    for chunk in turns.chunks(BATCH) {
        check_cancel()?;
        let data: Vec<f32> = chunk.par_iter().flat_map_iter(|r| kernel(template, n, &mask, &wedge, r, k)).collect();
        kernels.push((Tensor::from_data(TensorData::new(data, [chunk.len(), 1, k, k, k]), dev), chunk.len()));
    }
    let total = tiles.len() * batches;
    let mut done = 0;
    for t in &tiles {
        let s = size(*t);
        let len = s[0] * s[1] * s[2];
        let (cube, p) = fill(*t);
        let x = Tensor::<5>::from_data(TensorData::new(cube, [1, 1, p[2], p[1], p[0]]), dev);
        // 1 / √(Σ under the mask of (v − local mean)²).
        let m1 = conv3d(x.clone(), mask_t.clone(), None, options.clone());
        let m2 = conv3d(x.clone() * x.clone(), mask_t.clone(), None, options.clone());
        let spread = (m2 - m1.clone() * m1 / count).clamp_min(1e-12).sqrt();
        let mut best = vec![f32::MIN; len];
        let mut which = vec![0u32; len];
        for (b, (w, _)) in kernels.iter().enumerate() {
            check_cancel()?;
            let cc = conv3d(x.clone(), w.clone(), None, options.clone()) / spread.clone();
            let top = pull(cc.clone().max_dim(1).reshape([len]))?;
            let arg: Vec<i64> = cc.argmax(1).reshape([len]).into_data().convert::<i64>().try_into_vec::<i64>().map_err(|e| anyhow::anyhow!("{e:?}"))?;
            for i in 0..len {
                if top[i] > best[i] {
                    best[i] = top[i];
                    which[i] = (b * BATCH) as u32 + arg[i] as u32;
                }
            }
            done += 1;
            tick(done, total);
        }
        let mut i = 0;
        for z in 0..s[2] {
            for y in 0..s[1] {
                let row = ((t[2] + z) * ny + t[1] + y) * nx + t[0];
                score[row..row + s[0]].copy_from_slice(&best[i..i + s[0]]);
                turn[row..row + s[0]].copy_from_slice(&which[i..i + s[0]]);
                i += s[0];
            }
        }
    }
    if let Some(a) = allowed {
        score.iter_mut().zip(a).for_each(|(s, ok)| {
            if !ok {
                *s = f32::MIN;
            }
        });
    }
    Ok(Found { score, turn, dims })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spa::geom::grid;
    use crate::spa::Rng;

    /// The convolution finds a planted, turned copy where the transform
    /// search does, with the same orientation.
    #[test]
    fn agrees_with_the_transform_search() {
        let n = 16;
        let c = (n / 2) as f64;
        let mut rng = Rng(4);
        // Three balls with no symmetry.
        let template: Vec<f32> = (0..n * n * n)
            .map(|i| {
                let p = [(i % n) as f64 - c, (i / n % n) as f64 - c, (i / n / n) as f64 - c];
                let ball = |q: [f64; 3], s: f64| (-(0..3).map(|a| (p[a] - q[a]).powi(2)).sum::<f64>() / (2.0 * s * s)).exp();
                (ball([2.0, 0.0, 0.0], 1.5) + 0.8 * ball([-1.0, 2.5, 0.0], 1.2) + 0.6 * ball([0.0, -1.0, -2.5], 1.2)) as f32
            })
            .collect();
        let dims = [40usize, 36, 28];
        let turns = grid(60.0);
        let truth = 5;
        let copy = rotate(&template, n, &turns[truth]);
        let at = [22usize, 15, 13];
        let mut v: Vec<f32> = (0..dims[0] * dims[1] * dims[2]).map(|_| 0.05 * rng.gauss() as f32).collect();
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    v[((at[2] + z - n / 2) * dims[1] + at[1] + y - n / 2) * dims[0] + at[0] + x - n / 2] += copy[(z * n + y) * n + x];
                }
            }
        }
        let dev = Device::flex();
        let gpu = search(&dev, &v, dims, &template, n, 5.0, [-90.0, 90.0], &turns, None, &|_, _| {}).unwrap();
        let cpu = super::super::search::search(&v, dims, &template, n, 5.0, [-90.0, 90.0], &turns, None, &|_, _| {}).unwrap();
        let best = |f: &Found| (0..f.score.len()).max_by(|a, b| f.score[*a].total_cmp(&f.score[*b])).unwrap();
        let (g, p) = (best(&gpu), best(&cpu));
        let want = (at[2] * dims[1] + at[1]) * dims[0] + at[0];
        assert_eq!(g, want);
        assert_eq!(p, want);
        assert_eq!(gpu.turn[g] as usize, truth);
        assert_eq!(cpu.turn[p] as usize, truth);
        assert!(gpu.score[g] > 0.8 && (gpu.score[g] - cpu.score[p]).abs() < 0.15, "{} {}", gpu.score[g], cpu.score[p]);
    }
}
