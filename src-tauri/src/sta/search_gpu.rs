//! The whole-tomogram template search of `search` on the GPU (Burn).
//!
//! Burn has no Fourier transform that every backend runs, but it has 3D
//! convolution, and a convolution of the tomogram with a turned template
//! as the kernel is exactly the correlation wanted. This costs kernel
//! volume per voxel, so the search is made at the coarsest sampling that
//! still shows the particle (the kernel is the template cut to its mask,
//! about a dozen voxels across); the peaks are then refined on finer cubes
//! by the alignment code.
//!
//! The template at its thousands of orientations is not thousands of
//! different kernels: a smooth thing a dozen voxels across, turned, stays
//! within a space of about a hundred dimensions. So the tomogram is
//! convolved with a basis of that space only (`basis`), and the score of
//! every orientation at a voxel is a weighted sum of those few results
//! (a matrix product): an order of magnitude less work than a convolution
//! per orientation, for the same scores to within `MISSED`.
//!
//! Scores are those of `search`: divided by the tomogram's standard
//! deviation under the mask, template wedged at each orientation.

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
pub(super) const TILE: usize = 64;
/// Kernels in one convolution.
const BATCH: usize = 32;
/// Orientations scored in one product.
const SCORED: usize = 256;
/// The share of a turned template's power its basis may miss: a score is
/// a correlation of unit vectors, and what is missed of one is the fine
/// noise of turning it, which correlates with nothing.
const MISSED: f64 = 0.01;
/// The basis is not grown past this.
const MOST_BASIS: usize = 512;

/// A basis for `kernels` (rows of `d` values): orthonormal rows, each
/// kernel within `MISSED` of its own power of a weighted sum of them, and
/// those weights. Returns the basis (r × d), the weights (kernels × r)
/// and r. The basis is built from the kernels themselves, the one the
/// basis so far describes worst each time (pivoted Gram–Schmidt), so the
/// bound holds for every kernel, not on average.
pub(super) fn basis(kernels: &[f32], d: usize) -> (Vec<f32>, Vec<f32>, usize) {
    let o = kernels.len() / d;
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum::<f64>();
    let own: Vec<f64> = kernels.par_chunks(d).map(|k| dot(k, k)).collect();
    let mut left = own.clone();
    let mut rows: Vec<Vec<f32>> = vec![];
    // Per basis row, the weight of it in every kernel.
    let mut weights: Vec<Vec<f32>> = vec![];
    while rows.len() < MOST_BASIS {
        let Some(worst) = (0..o).filter(|i| left[*i] > MISSED * own[*i] && own[*i] > 0.0).max_by(|a, b| (left[*a] / own[*a]).total_cmp(&(left[*b] / own[*b]))) else { break };
        let mut q: Vec<f64> = kernels[worst * d..(worst + 1) * d].iter().map(|x| *x as f64).collect();
        // Twice, so that rounding leaves nothing of the rows already there.
        for _ in 0..2 {
            for row in &rows {
                let w: f64 = q.iter().zip(row).map(|(x, y)| x * *y as f64).sum();
                q.iter_mut().zip(row).for_each(|(x, y)| *x -= w * *y as f64);
            }
        }
        let norm = q.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm * norm <= 1e-9 * own[worst] {
            left[worst] = 0.0;
            continue;
        }
        let row: Vec<f32> = q.iter().map(|x| (x / norm) as f32).collect();
        let w: Vec<f32> = kernels.par_chunks(d).map(|k| dot(k, &row) as f32).collect();
        for i in 0..o {
            left[i] -= (w[i] as f64).powi(2);
        }
        rows.push(row);
        weights.push(w);
    }
    if rows.is_empty() {
        // Nothing to find (a template with no power): every score is 0.
        rows.push(vec![0.0; d]);
        weights.push(vec![0.0; o]);
    }
    let r = rows.len();
    let mut by_kernel = vec![0f32; o * r];
    for (j, w) in weights.iter().enumerate() {
        for i in 0..o {
            by_kernel[i * r + j] = w[i];
        }
    }
    (rows.concat(), by_kernel, r)
}

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
    // Not below what a flat place has (`search::flat_variance`), summed
    // under the mask.
    let flat = (count as f64 * super::search::flat_variance(v, mean)) as f32;
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
    // The kernels of all orientations, as weights of a few basis kernels.
    check_cancel()?;
    let d = k * k * k;
    let all: Vec<f32> = turns.par_iter().flat_map_iter(|r| kernel(template, n, &mask, &wedge, r, k)).collect();
    check_cancel()?;
    let (rows, weights, r) = basis(&all, d);
    drop(all);
    let rows_t: Vec<Tensor<5>> = rows.chunks(BATCH * d).map(|c| Tensor::from_data(TensorData::new(c.to_vec(), [c.len() / d, 1, k, k, k]), dev)).collect();
    let weights_t: Vec<Tensor<2>> = weights.chunks(SCORED * r).map(|c| Tensor::from_data(TensorData::new(c.to_vec(), [c.len() / r, r]), dev)).collect();
    let total = tiles.len() * weights_t.len();
    let mut done = 0;
    for t in &tiles {
        check_cancel()?;
        let s = size(*t);
        let len = s[0] * s[1] * s[2];
        let (cube, p) = fill(*t);
        let x = Tensor::<5>::from_data(TensorData::new(cube, [1, 1, p[2], p[1], p[0]]), dev);
        // √(Σ under the mask of (v − local mean)²).
        let m1 = conv3d(x.clone(), mask_t.clone(), None, options.clone());
        let m2 = conv3d(x.clone() * x.clone(), mask_t.clone(), None, options.clone());
        let spread = pull((m2 - m1.clone() * m1 / count).clamp_min(flat).sqrt().reshape([len]))?;
        // The tile seen through each basis kernel: r × voxels.
        let seen: Vec<Tensor<2>> = rows_t.iter().map(|w| conv3d(x.clone(), w.clone(), None, options.clone()).reshape([w.dims()[0], len])).collect();
        let seen = Tensor::cat(seen, 0);
        let mut best = vec![f32::MIN; len];
        let mut which = vec![0u32; len];
        for (b, w) in weights_t.iter().enumerate() {
            check_cancel()?;
            // Orientations × voxels. The spread is the same for all the
            // orientations at a voxel: divided by after the best is known.
            let cc = w.clone().matmul(seen.clone());
            let top = pull(cc.clone().max_dim(0).reshape([len]))?;
            let arg: Vec<i64> = cc.argmax(0).reshape([len]).into_data().convert::<i64>().try_into_vec::<i64>().map_err(|e| anyhow::anyhow!("{e:?}"))?;
            for i in 0..len {
                if top[i] > best[i] {
                    best[i] = top[i];
                    which[i] = (b * SCORED) as u32 + arg[i] as u32;
                }
            }
            done += 1;
            tick(done, total);
        }
        best.iter_mut().zip(&spread).for_each(|(b, s)| *b /= s);
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

    /// A template at a few thousand orientations is spanned by far fewer
    /// kernels, each orientation to within `MISSED` of its power.
    #[test]
    fn a_few_kernels_span_the_template_at_every_orientation() {
        let n = 16;
        let c = (n / 2) as f64;
        let template: Vec<f32> = (0..n * n * n)
            .map(|i| {
                let p = [(i % n) as f64 - c, (i / n % n) as f64 - c, (i / n / n) as f64 - c];
                let ball = |q: [f64; 3], s: f64| (-(0..3).map(|a| (p[a] - q[a]).powi(2)).sum::<f64>() / (2.0 * s * s)).exp();
                (ball([2.5, 0.0, 0.0], 1.6) + 0.8 * ball([-1.5, 2.5, 0.5], 1.4) + 0.6 * ball([0.0, -1.5, -2.5], 1.4)) as f32
            })
            .collect();
        let radius = 5.0;
        let mask = ball(n, radius, 2.0);
        let wedge = wedge(n, -60.0, 60.0);
        let k = 15;
        let d = k * k * k;
        let turns = grid(30.0);
        let all: Vec<f32> = turns.par_iter().flat_map_iter(|r| kernel(&template, n, &mask, &wedge, r, k)).collect();
        let (rows, weights, r) = basis(&all, d);
        assert!(r >= 4 && r * 6 < turns.len(), "{r} kernels for {} orientations", turns.len());
        for (i, kernel) in all.chunks(d).enumerate() {
            let mut left = 0f64;
            let mut own = 0f64;
            for v in 0..d {
                let made: f32 = (0..r).map(|j| weights[i * r + j] * rows[j * d + v]).sum();
                left += ((kernel[v] - made) as f64).powi(2);
                own += (kernel[v] as f64).powi(2);
            }
            assert!(left <= 1.05 * MISSED * own, "orientation {i}: {left} of {own} missed");
        }
        // The rows are orthonormal.
        for a in 0..r.min(8) {
            for b in 0..r.min(8) {
                let dot: f64 = (0..d).map(|v| rows[a * d + v] as f64 * rows[b * d + v] as f64).sum();
                assert!((dot - if a == b { 1.0 } else { 0.0 }).abs() < 1e-4, "{a} {b} {dot}");
            }
        }
    }

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
