//! The scoring of `cc` (`Engine::score`) on the GPU (Burn), for many
//! (particle, orientation) pairs at once.
//!
//! Burn has no 3D grid sampling and no Fourier transform of a cube, but it
//! has gather (`select`) and matrix products, and `Engine` needs nothing
//! more: it never turns a volume, it reads the reference's transform at the
//! turned band points (trilinear) and sums the correlation over the few
//! shifts wanted. Both are done here on the same numbers:
//!
//! - the turned reference of every pair is read from the reference's
//!   transform with four gathers per band point (the two neighbours along
//!   kx sit side by side in the table, so one row holds both);
//! - the products X·conj(P) of the band go into a dense (kz, ky, kx) block
//!   (zero off the band) and the sum over the shifts is three small matrix
//!   products, one per axis, with the matrix of e^{2πi k t / n};
//! - the norm of P over the frequencies the particle's wedge measured, the
//!   division and the maximum over the shifts are taken on the device, and
//!   only the best (value, shift) of each pair comes back.
//!
//! This is the score of `Engine`, not an approximation of it: same band,
//! same weights, same trilinear read, same normalisation, summed in another
//! order. What the device does not do is the sub-voxel step: the winner of
//! each particle is scored again with `Engine::score` on the CPU (one score
//! per particle instead of one per orientation), which gives its pose
//! exactly as the CPU search does. The orientations are not kernels, so
//! `local` (each particle around its own pose) costs the same as `global`
//! per pair: nothing is built per particle but nine numbers.

use anyhow::Result;
use burn::tensor::{Device, Int, Tensor, TensorData};
use rayon::prelude::*;

use super::cc::{Engine, Prepared, Reference};
use super::{check_cancel, Pose};
use crate::spa::geom::{mul, Mat};

/// Values in the largest block a batch builds (the dense products); a
/// batch holds as many pairs as fit, so the device never needs much memory.
const BLOCK: usize = 1 << 26;
/// Orientations in a batch of a global search at most; the turned reference
/// of those is read once and shared by all the particles of the batch.
const MOST_TURNS: usize = 64;
/// Bytes of prepared particles a chunk of `local_chunk` holds.
const CHUNK_BYTES: usize = 256 << 20;

/// Geometry of one `Engine` and one shift range on the device.
pub struct Gpu<'a> {
    dev: &'a Device,
    engine: &'a Engine,
    max_shift: usize,
    /// Band points.
    m: usize,
    /// Shifts per axis.
    side: usize,
    /// Frequencies kept per axis: −reach..=reach (kx, ky) and 0..=reach (kz).
    wide: usize,
    deep: usize,
    /// (1, 1, m).
    k: [Tensor<3>; 3],
    weight: Tensor<3>,
    /// For each dense position (kz, ky, kx) the band point there (0 off the band).
    dense: Tensor<1, Int>,
    /// 1 on the dense positions of the band, else 0.
    keep: Tensor<2>,
    /// e^{2πi k t/n} as the real matrix [[c, s], [−s, c]] that takes
    /// (re, im) of a row over k to (re, im) over t.
    wave: Tensor<2>,
    /// The same for kz from 0, real part only: [[c], [−s]].
    wave_z: Tensor<2>,
}

/// An `Engine` reference as the device reads it.
pub struct Bound<'b, 'a> {
    gpu: &'b Gpu<'a>,
    cpu: &'b Reference,
    /// Row (z·big + y)·big + x: re and im at x, re and im at x + 1.
    table: Tensor<2>,
}

fn upload<const D: usize>(v: Vec<f32>, shape: [usize; D], dev: &Device) -> Tensor<D> {
    Tensor::from_data(TensorData::new(v, shape), dev)
}

fn download<const D: usize>(t: Tensor<D>) -> Result<Vec<f32>> {
    t.into_data().convert::<f32>().try_into_vec::<f32>().map_err(|e| anyhow::anyhow!("{e:?}"))
}

fn download_ints<const D: usize>(t: Tensor<D, Int>) -> Result<Vec<i64>> {
    t.into_data().convert::<i64>().try_into_vec::<i64>().map_err(|e| anyhow::anyhow!("{e:?}"))
}

/// Rows of coefficients that take (re, im) over `k` in `ks` to (re, im)
/// over the shifts −s..=s: e^{2πi k t/n}, k·t reduced mod n for accuracy
/// (the sums of `Engine::score` use the same table).
fn waves(ks: &[i32], s: i32, n: usize) -> (Vec<f32>, Vec<f32>) {
    let side = (2 * s + 1) as usize;
    let mut c = vec![0f32; ks.len() * side];
    let mut d = vec![0f32; ks.len() * side];
    for (i, k) in ks.iter().enumerate() {
        for t in -s..=s {
            let a = 2.0 * std::f64::consts::PI * (k * t).rem_euclid(n as i32) as f64 / n as f64;
            c[i * side + (t + s) as usize] = a.cos() as f32;
            d[i * side + (t + s) as usize] = a.sin() as f32;
        }
    }
    (c, d)
}

impl<'a> Gpu<'a> {
    /// `max_shift` as given to `Engine::score` (the same limit applies).
    pub fn new(dev: &'a Device, engine: &'a Engine, max_shift: usize) -> Gpu<'a> {
        let n = engine.n;
        // The gather index is a float on the device: exact up to 2^24.
        assert!(n <= 128, "the box is too large for the gather index");
        let band = &engine.band;
        let m = band.k.len();
        let reach = band.reach;
        let s = max_shift.min(n / 2 - 1) as i32;
        let side = (2 * s + 1) as usize;
        let wide = (2 * reach + 1) as usize;
        let deep = (reach + 1) as usize;
        let column = |f: &dyn Fn(&[i16; 3]) -> f32| upload(band.k.iter().map(f).collect(), [1, 1, m], dev);
        let k = [column(&|k| k[0] as f32), column(&|k| k[1] as f32), column(&|k| k[2] as f32)];
        let weight = upload(band.weight.clone(), [1, 1, m], dev);
        let mut slot = vec![0i64; deep * wide * wide];
        let mut keep = vec![0f32; deep * wide * wide];
        for (i, p) in band.k.iter().enumerate() {
            let at = (p[2] as usize * wide + (p[1] as i32 + reach) as usize) * wide + (p[0] as i32 + reach) as usize;
            slot[at] = i as i64;
            keep[at] = 1.0;
        }
        let dense = Tensor::<1, Int>::from_ints(TensorData::new(slot, [deep * wide * wide]), dev);
        let keep = upload(keep, [1, deep * wide * wide], dev);
        // Rows: re of each k, then im of each k; columns: re over t, then im.
        let all: Vec<i32> = (-reach..=reach).collect();
        let (c, d) = waves(&all, s, n);
        let mut w = vec![0f32; 2 * wide * 2 * side];
        for i in 0..wide {
            for t in 0..side {
                w[i * 2 * side + t] = c[i * side + t];
                w[i * 2 * side + side + t] = d[i * side + t];
                w[(wide + i) * 2 * side + t] = -d[i * side + t];
                w[(wide + i) * 2 * side + side + t] = c[i * side + t];
            }
        }
        let wave = upload(w, [2 * wide, 2 * side], dev);
        let zs: Vec<i32> = (0..=reach).collect();
        let (c, d) = waves(&zs, s, n);
        let mut w = vec![0f32; 2 * deep * side];
        for i in 0..deep {
            for t in 0..side {
                w[i * side + t] = c[i * side + t];
                w[(deep + i) * side + t] = -d[i * side + t];
            }
        }
        let wave_z = upload(w, [2 * deep, side], dev);
        Gpu { dev, engine, max_shift, m, side, wide, deep, k, weight, dense, keep, wave, wave_z }
    }

    /// Pairs one batch holds.
    fn pairs(&self) -> usize {
        (BLOCK / (2 * self.deep * self.wide * self.wide)).max(1)
    }

    /// The reference as the gathers read it.
    pub fn bind<'b>(&'b self, reference: &'b Reference) -> Bound<'b, 'a> {
        let big = 2 * self.engine.n;
        let mut v = vec![0f32; big * big * big * 4];
        v.par_chunks_mut(4 * big).enumerate().for_each(|(row, out)| {
            for x in 0..big {
                let (a, c) = (row * big + x, row * big + (x + 1) % big);
                out[4 * x..4 * x + 4].copy_from_slice(&[reference.re[a], reference.im[a], reference.re[c], reference.im[c]]);
            }
        });
        Bound { gpu: self, cpu: reference, table: upload(v, [big * big * big, 4], self.dev) }
    }
}

/// Orientations as the device takes them: 2·Rᵀ, row-major (`Engine::turn`
/// reads the transform in half steps of frequency).
fn turns_data(rots: &[Mat]) -> Vec<f32> {
    rots.iter().flat_map(|r| (0..3).flat_map(move |i| (0..3).map(move |j| 2.0 * r[j][i] as f32))).collect()
}

/// Σ over the last axis of (re, im) · e^{…}: `w` takes 2K inputs to 2S
/// outputs, K the last axis of both.
fn contract(re: Tensor<4>, im: Tensor<4>, w: &Tensor<2>, side: usize) -> (Tensor<4>, Tensor<4>) {
    let [a, b, c, k] = re.dims();
    let x = Tensor::cat(vec![re, im], 3).reshape([a * b * c, 2 * k]);
    let y = x.matmul(w.clone()).reshape([a, b, c, 2 * side]);
    (y.clone().narrow(3, 0, side), y.narrow(3, side, side))
}

/// The pose `Engine::score` gives for `x` turned by `r`: the sub-voxel
/// shift and the score the CPU search would report for it.
fn settle(engine: &Engine, reference: &Reference, max_shift: usize, x: &Prepared, r: Mat) -> Pose {
    let (score, t) = engine.score(x, &engine.turn(reference, &r), max_shift);
    Pose { r, t, score }
}

impl Bound<'_, '_> {
    /// B·FT(turned reference) at the band points for `rot` (bx, br, 9):
    /// `Engine::turn`, for bx·br orientations.
    fn turned(&self, rot: Tensor<3>) -> (Tensor<3>, Tensor<3>) {
        let g = self.gpu;
        let [bx, br, _] = rot.dims();
        let big = (2 * g.engine.n) as f32;
        let at = |i: usize, j: usize| rot.clone().narrow(2, 3 * i + j, 1);
        // q = 2·Rᵀ·k in half steps of the reference's transform.
        let q: Vec<Tensor<3>> = (0..3).map(|i| at(i, 0) * g.k[0].clone() + at(i, 1) * g.k[1].clone() + at(i, 2) * g.k[2].clone()).collect();
        let floor: Vec<Tensor<3>> = q.iter().map(|v| v.clone().floor()).collect();
        let frac: Vec<Tensor<3>> = q.iter().zip(&floor).map(|(v, f)| v.clone() - f.clone()).collect();
        // The index of a frequency on the wrapped grid (the offset keeps it positive).
        let wrap = |v: Tensor<3>| {
            let v = v + big;
            v.clone() - (v / big).floor() * big
        };
        let x0 = wrap(floor[0].clone());
        let (y0, y1) = (wrap(floor[1].clone()), wrap(floor[1].clone() + 1.0));
        let (z0, z1) = (wrap(floor[2].clone()), wrap(floor[2].clone() + 1.0));
        let (dx, dy, dz) = (frac[0].clone(), frac[1].clone(), frac[2].clone());
        let len = bx * br * g.m;
        let mut out: Option<(Tensor<3>, Tensor<3>)> = None;
        for (z, wz) in [(&z0, dz.clone().neg() + 1.0), (&z1, dz.clone())] {
            for (y, wy) in [(&y0, dy.clone().neg() + 1.0), (&y1, dy.clone())] {
                let idx: Tensor<1, Int> = ((z.clone() * big + y.clone()) * big + x0.clone()).reshape([len]).int();
                let row = self.table.clone().select(0, idx).reshape([bx, br, g.m, 4]);
                let part = |c: usize| row.clone().narrow(3, c, 1).reshape([bx, br, g.m]);
                let w = wz.clone() * wy;
                let lerp = |a: Tensor<3>, b: Tensor<3>| (a.clone() + dx.clone() * (b - a)) * w.clone();
                let (re, im) = (lerp(part(0), part(2)), lerp(part(1), part(3)));
                out = Some(match out {
                    Some((r, i)) => (r + re, i + im),
                    None => (re, im),
                });
            }
        }
        let (re, im) = out.unwrap();
        (re * g.weight.clone(), im * g.weight.clone())
    }

    /// For each (particle, orientation) of a batch: the best over the
    /// shifts of X·P, divided by √(Σ|X|²·Σ|P|² over the open frequencies),
    /// and where. `x` is (bp, 1, m) and `p` (bp or 1, br, m).
    #[allow(clippy::too_many_arguments)]
    fn best(&self, x: (Tensor<3>, Tensor<3>), open: Tensor<3>, norm: Tensor<3>, p: (Tensor<3>, Tensor<3>)) -> Result<(Vec<f32>, Vec<i64>)> {
        let g = self.gpu;
        let [bp, _, _] = x.0.dims();
        let [_, br, _] = p.0.dims();
        let b = bp * br;
        let (xr, xi) = x;
        let (pr, pi) = p;
        let power = ((pr.clone() * pr.clone() + pi.clone() * pi.clone()) * open).sum_dim(2).reshape([bp, br, 1]) * norm;
        // X·conj(P).
        let cr = (xr.clone() * pr.clone() + xi.clone() * pi.clone()).reshape([b, g.m]);
        let ci = (xi * pr - xr * pi).reshape([b, g.m]);
        let block = |c: Tensor<2>| c.select(1, g.dense.clone()) * g.keep.clone();
        let shape = [b, g.deep, g.wide, g.wide];
        let (re, im) = (block(cr).reshape(shape), block(ci).reshape(shape));
        // Along kx, then ky, then kz.
        let (re, im) = contract(re, im, &g.wave, g.side);
        let (re, im) = contract(re.swap_dims(2, 3), im.swap_dims(2, 3), &g.wave, g.side);
        let (re, im) = (re.swap_dims(1, 3), im.swap_dims(1, 3));
        let [a, c, d, k] = re.dims();
        let cube = Tensor::cat(vec![re, im], 3).reshape([a * c * d, 2 * k]).matmul(g.wave_z.clone()).reshape([b, g.side * g.side * g.side]);
        let (top, at) = cube.max_dim_with_indices(1);
        // A pair with nothing to compare scores 0, as `Engine::score` does.
        let both = power.clone().reshape([b, 1]);
        let live = both.clone().greater_elem(0.0).float();
        let value = top / both.clamp_min(1e-30).sqrt() * live;
        Ok((download(value)?, download_ints(at)?))
    }

    /// The prepared particles of a batch as (bp, 1, m) tensors: re, im, the
    /// frequencies their wedges measured, and Σ|X|².
    fn particles(&self, xs: &[&Prepared]) -> ((Tensor<3>, Tensor<3>), Tensor<3>, Tensor<3>) {
        let g = self.gpu;
        let bp = xs.len();
        let cat = |f: &dyn Fn(&Prepared) -> Vec<f32>| -> Vec<f32> { xs.iter().flat_map(|x| f(x)).collect() };
        let re = upload(cat(&|x| x.re.clone()), [bp, 1, g.m], g.dev);
        let im = upload(cat(&|x| x.im.clone()), [bp, 1, g.m], g.dev);
        let open = upload(cat(&|x| x.open.iter().map(|o| *o as u8 as f32).collect()), [bp, 1, g.m], g.dev);
        let norm = upload(xs.iter().map(|x| x.norm as f32).collect(), [bp, 1, 1], g.dev);
        ((re, im), open, norm)
    }

    /// What the pose of each winner is scored with, off the device.
    fn finisher(&self) -> impl Fn(&Prepared, Mat) -> Pose + Sync + '_ {
        let (engine, cpu, shift) = (self.gpu.engine, self.cpu, self.gpu.max_shift);
        move |x, r| settle(engine, cpu, shift, x, r)
    }

    /// `Engine::global`: every particle against every orientation of one
    /// list, the best of each. Orientations are read from the reference
    /// once per batch and shared by all particles.
    pub fn global(&self, particles: &[Prepared], rotations: &[Mat], tick: &(dyn Fn(usize, usize) + Sync)) -> Result<Vec<Pose>> {
        let g = self.gpu;
        let mut best: Vec<(f32, usize)> = vec![(f32::MIN, 0); particles.len()];
        let pairs = g.pairs();
        let turns = pairs.min(MOST_TURNS).min(rotations.len()).max(1);
        let per = (pairs / turns).max(1);
        let all: Vec<&Prepared> = particles.iter().collect();
        for (c, chunk) in rotations.chunks(turns).enumerate() {
            check_cancel()?;
            let rot = upload(turns_data(chunk), [1, chunk.len(), 9], g.dev);
            let p = self.turned(rot);
            for (j, xs) in all.chunks(per).enumerate() {
                let (x, open, norm) = self.particles(xs);
                let (value, _) = self.best(x, open, norm, p.clone())?;
                for (i, row) in value.chunks(chunk.len()).enumerate() {
                    for (r, v) in row.iter().enumerate() {
                        let b = &mut best[j * per + i];
                        if *v > b.0 {
                            *b = (*v, c * turns + r);
                        }
                    }
                }
            }
            for i in 0..chunk.len() {
                tick(c * turns + i + 1, rotations.len());
            }
        }
        let settle = self.finisher();
        Ok(particles.par_iter().zip(&best).map(|(x, b)| settle(x, rotations[b.1])).collect())
    }

    /// `Engine::local` for many particles: each around its own pose
    /// (`from`), every `steps` applied on top of it.
    pub fn local(&self, particles: &[&Prepared], from: &[Pose], steps: &[Mat]) -> Result<Vec<Pose>> {
        let g = self.gpu;
        if steps.is_empty() {
            return Ok(from.iter().map(|f| Pose { score: f32::MIN, ..*f }).collect());
        }
        let per = (g.pairs() / steps.len()).max(1);
        let mut out = Vec::with_capacity(particles.len());
        for (xs, fs) in particles.chunks(per).zip(from.chunks(per)) {
            check_cancel()?;
            let rots: Vec<Mat> = fs.iter().flat_map(|f| steps.iter().map(|d| mul(d, &f.r))).collect();
            let rot = upload(turns_data(&rots), [xs.len(), steps.len(), 9], g.dev);
            let (x, open, norm) = self.particles(xs);
            let (value, _) = self.best(x, open, norm, self.turned(rot))?;
            let settle = self.finisher();
            let found: Vec<Pose> = (0..xs.len())
                .into_par_iter()
                .map(|i| {
                    let row = &value[i * steps.len()..(i + 1) * steps.len()];
                    let top = (0..steps.len()).fold(0, |a, s| if row[s] > row[a] { s } else { a });
                    settle(xs[i], rots[i * steps.len() + top])
                })
                .collect();
            out.extend(found);
        }
        Ok(out)
    }
}

/// How many prepared particles fit in a chunk of `CHUNK_BYTES`.
pub fn chunk_of(engine: &Engine) -> usize {
    (CHUNK_BYTES / (9 * engine.band.k.len()).max(1)).clamp(16, 4096)
}

/// `Engine::global` on `dev`.
pub fn global(dev: &Device, engine: &Engine, particles: &[Prepared], reference: &[f32], rotations: &[Mat], max_shift: usize, tick: &(dyn Fn(usize, usize) + Sync)) -> Result<Vec<Pose>> {
    let gpu = Gpu::new(dev, engine, max_shift);
    let reference = engine.reference(reference);
    gpu.bind(&reference).global(particles, rotations, tick)
}

#[cfg(test)]
mod tests {
    use super::super::average::Sum;
    use super::super::cc::wedge;
    use super::super::tests::{corr, particle, phantom};
    use super::*;
    use crate::spa::geom::{axis_angle, distance, grid, local};
    use crate::spa::Rng;

    const N: usize = 32;

    /// Noisy, wedged copies of the phantom at random poses, in two wedges.
    fn set(count: usize, seed: u64) -> (Vec<Vec<f32>>, Vec<Vec<f32>>, Vec<usize>, Vec<Mat>) {
        let truth = phantom();
        let sd = (truth.iter().map(|v| (v * v) as f64).sum::<f64>() / truth.len() as f64).sqrt();
        let wedges = vec![wedge(N, -60.0, 60.0), wedge(N, -45.0, 60.0)];
        let mut rng = Rng(seed);
        let (mut parts, mut which, mut turns) = (vec![], vec![], vec![]);
        for i in 0..count {
            let r = axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 360.0 * rng.unit());
            let t = [4.0 * rng.unit() - 2.0, 4.0 * rng.unit() - 2.0, 4.0 * rng.unit() - 2.0];
            parts.push(particle(&truth, &r, t, &wedges[i % 2], 1.0 * sd, &mut rng));
            which.push(i % 2);
            turns.push(r);
        }
        (parts, wedges, which, turns)
    }

    fn shift_gap(a: &Pose, b: &Pose) -> f64 {
        (0..3).map(|i| (a.t[i] - b.t[i]).powi(2)).sum::<f64>().sqrt()
    }

    /// The device finds the orientation and shift the CPU search finds, and
    /// the average made from either set of poses is the same.
    #[test]
    fn global_agrees_with_the_cpu() {
        let dev = Device::flex();
        let truth = phantom();
        let (parts, wedges, which, _) = set(32, 7);
        let engine = Engine::new(N, 8.0, 1.0, 14.0);
        let prepared: Vec<_> = parts.iter().enumerate().map(|(i, p)| engine.prepare(p, &wedges[which[i]])).collect();
        let rotations = grid(40.0);
        let start = super::super::pick::gauss(&truth, [N, N, N], 1.0);
        let cpu = engine.global(&prepared, &start, &rotations, 4, &|_, _| {}).unwrap();
        let ticks = std::sync::Mutex::new(vec![]);
        let gpu = global(&dev, &engine, &prepared, &start, &rotations, 4, &|a, b| ticks.lock().unwrap().push((a, b))).unwrap();
        // The progress steps by one up to the total, as the CPU search's.
        let ticks = ticks.into_inner().unwrap();
        assert!(ticks.iter().enumerate().all(|(i, t)| *t == (i + 1, rotations.len())));
        assert_eq!(ticks.len(), rotations.len());
        let same = cpu.iter().zip(&gpu).filter(|(a, b)| distance(&a.r, &b.r) < 0.05).count();
        let near = cpu.iter().zip(&gpu).filter(|(a, b)| shift_gap(a, b) < 0.5).count();
        let worst = cpu.iter().zip(&gpu).map(|(a, b)| (a.score - b.score).abs()).fold(0f32, f32::max);
        eprintln!("same rotation {same}/{}, shifts within 0.5 voxel {near}, largest score difference {worst}", cpu.len());
        assert!(same * 100 >= 95 * cpu.len(), "{same} of {} pick the CPU's rotation", cpu.len());
        assert!(near * 100 >= 95 * cpu.len(), "{near} of {} within half a voxel", cpu.len());
        let average = |poses: &[Pose]| {
            let mut sum = Sum::new(N);
            for (i, (p, f)) in parts.iter().zip(poses).enumerate() {
                sum.add(p, &wedges[which[i]], f);
            }
            sum.finish()
        };
        let c = corr(&average(&cpu), &average(&gpu));
        eprintln!("average from the GPU poses against the CPU's: {c:.4}");
        assert!(c >= 0.98, "{c}");
    }

    /// The same for particles around poses of their own.
    #[test]
    fn local_agrees_with_the_cpu() {
        let dev = Device::flex();
        let truth = phantom();
        let (parts, wedges, which, turns) = set(24, 11);
        let engine = Engine::new(N, 9.0, 1.0, 14.0);
        let prepared: Vec<_> = parts.iter().enumerate().map(|(i, p)| engine.prepare(p, &wedges[which[i]])).collect();
        let mut rng = Rng(5);
        // Starts a few degrees off the truth, as after a coarse search.
        let from: Vec<Pose> = turns
            .iter()
            .map(|r| Pose { r: mul(&axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 6.0), r), t: [0.0; 3], score: 0.0 })
            .collect();
        let steps = local(8.0, 2);
        let reference = engine.reference(&truth);
        let cpu: Vec<Pose> = prepared.iter().zip(&from).map(|(x, f)| engine.local(x, &reference, f, &steps, 4)).collect();
        let gpu = Gpu::new(&dev, &engine, 4);
        let refs: Vec<&Prepared> = prepared.iter().collect();
        let found = gpu.bind(&reference).local(&refs, &from, &steps).unwrap();
        let same = cpu.iter().zip(&found).filter(|(a, b)| distance(&a.r, &b.r) < 0.05).count();
        let near = cpu.iter().zip(&found).filter(|(a, b)| shift_gap(a, b) < 0.5).count();
        let worst = cpu.iter().zip(&found).map(|(a, b)| (a.score - b.score).abs()).fold(0f32, f32::max);
        eprintln!("local: same step {same}/{}, shifts within 0.5 voxel {near}, largest score difference {worst}", cpu.len());
        assert!(same * 100 >= 95 * cpu.len() && near * 100 >= 95 * cpu.len());
        // The scores of one pair, the engine's and the device's, are the same number.
        let x = &prepared[0];
        let r = mul(&steps[3], &from[0].r);
        let (want, _) = engine.score(x, &engine.turn(&reference, &r), 4);
        let rot = upload(turns_data(&[r]), [1, 1, 9], &dev);
        let bound = gpu.bind(&reference);
        let (px, open, norm) = bound.particles(&[x]);
        let (value, _) = bound.best(px, open, norm, bound.turned(rot)).unwrap();
        assert!((value[0] - want).abs() < 2e-3, "{} {want}", value[0]);
    }

    /// A whole `align` on the device (halves, rounds, chunks) ends where
    /// the CPU's does.
    #[test]
    fn align_on_the_device_agrees_with_the_cpu() {
        use super::super::refine::{align, align_on, Set};
        let dev = Device::flex();
        let truth = phantom();
        let (parts, wedges, which, turns) = set(20, 21);
        let mut rng = Rng(8);
        let start: Vec<Pose> = turns
            .iter()
            .map(|r| Pose { r: mul(&axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 8.0), r), t: [0.0; 3], score: 0.0 })
            .collect();
        let model = super::super::pick::gauss(&truth, [N, N, N], 1.5);
        let set = Set { n: N, particles: &parts, wedges: &wedges, wedge_of: &which, radius: 10.0 };
        let cpu = align(&set, &model, &start, &|_, _, _| {}, &|_| {}).unwrap();
        let seen = std::sync::Mutex::new(vec![]);
        let gpu = align_on(Some(&dev), &set, &model, &start, &|s, a, b| seen.lock().unwrap().push((s.to_string(), a, b)), &|_| {}).unwrap();
        let same = cpu.poses.iter().zip(&gpu.poses).filter(|(a, b)| distance(&a.r, &b.r) < 0.05 && shift_gap(a, b) < 0.5).count();
        let c = corr(&cpu.map, &gpu.map);
        eprintln!("align: same pose for {same}/{} particles, maps correlate {c:.4}", parts.len());
        assert!(same * 100 >= 95 * parts.len() && c >= 0.98);
        // Every round reports up to the number of particles.
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.iter().filter(|e| e.1 == e.2).count(), 3);
    }

    /// Classes with the global searches on the device: the progress steps
    /// as on the CPU and the kinds are found.
    #[test]
    fn classes_on_the_device_count_their_progress_steadily() {
        use super::super::refine::{classes_on, Set};
        let dev = Device::flex();
        let n = 16;
        let w = wedge(n, -60.0, 60.0);
        let mut rng = Rng(5);
        let balls: Vec<f32> = (0..n * n * n)
            .map(|i| {
                let (x, y, z) = ((i % n) as f64 - 8.0, (i / n % n) as f64 - 8.0, (i / n / n) as f64 - 8.0);
                (-((x - 2.0).powi(2) + y * y + z * z) / 8.0).exp() as f32 + (rng.gauss() * 0.05) as f32
            })
            .collect();
        let particles: Vec<Vec<f32>> = (0..8).map(|_| balls.iter().map(|v| v + (rng.gauss() * 0.05) as f32).collect()).collect();
        let wedges = vec![w];
        let wedge_of = vec![0; particles.len()];
        let set = Set { n, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius: 4.0 };
        let seen = std::sync::Mutex::new(vec![]);
        let found = classes_on(Some(&dev), &set, 2, 1, &|s, a, b| seen.lock().unwrap().push((s.to_string(), a, b)), &|_| {}).unwrap();
        assert!(!found.is_empty());
        let seen = seen.into_inner().unwrap();
        let each = grid(30.0).len();
        assert!(seen.iter().all(|e| e.2 == 2 * each));
        assert!(seen.windows(2).all(|p| p[0].0 != p[1].0 || p[1].1 == p[0].1 + 1 || p[1].1 == 1));
        assert_eq!(seen.len(), 10 * 2 * each);
    }

    /// Heavy, for a machine with a GPU: the stages of the CPU engine and the
    /// device path on 500 particles at the sizes of the coarse (32) and fine
    /// (64) passes, local steps of a round of `refine::align` and the grid
    /// of a global search (`CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test
    /// --features pro --lib sta::cc_gpu::tests::times -- --ignored
    /// --nocapture`). Without a GPU the device is Burn's CPU fallback and
    /// the device numbers say nothing about a GPU.
    #[test]
    #[ignore]
    fn times() {
        use super::super::cc::ball;
        use std::time::Instant;
        let flex;
        let dev = match crate::ctf_gpu::device() {
            Some((d, name)) => {
                eprintln!("device: {name}");
                d
            }
            None => {
                eprintln!("device: no GPU, Burn's CPU fallback");
                flex = Device::flex();
                &flex
            }
        };
        let count = 500;
        for (n, lows, radius) in [(32usize, [5.0, 9.0, 13.0], 10.0), (64, [10.0, 20.0, 29.0], 20.0)] {
            let mut rng = Rng(3);
            let w = wedge(n, -60.0, 60.0);
            let edge = ball(n, n as f64 * 0.3, 3.0);
            let mut cube = || -> Vec<f32> { edge.iter().map(|b| b * (1.0 + 0.3 * rng.gauss() as f32) + 0.5 * rng.gauss() as f32).collect() };
            let parts: Vec<Vec<f32>> = (0..count).map(|_| cube()).collect();
            let model = cube();
            let shift = (radius / 2.0f64).ceil().clamp(2.0, n as f64 / 4.0) as usize;
            for low in lows {
                let e = Engine::new(n, low, 1.0, radius + 2.0);
                let t = Instant::now();
                let prep: Vec<Prepared> = parts.par_iter().map(|x| e.prepare(x, &w)).collect();
                let t_prepare = t.elapsed().as_secs_f64();
                let reference = e.reference(&model);
                let from: Vec<Pose> = (0..count).map(|i| Pose { r: grid(30.0)[i % 500], t: [0.0; 3], score: 0.0 }).collect();
                let steps = local(12.0, 2);
                let t = Instant::now();
                let cpu: Vec<Pose> = (0..count).into_par_iter().map(|i| e.local(&prep[i], &reference, &from[i], &steps, shift)).collect();
                let t_cpu = t.elapsed().as_secs_f64();
                let gpu = Gpu::new(dev, &e, shift);
                let bound = gpu.bind(&reference);
                let xs: Vec<&Prepared> = prep.iter().collect();
                // The first call also compiles the kernels for these shapes;
                // a run repeats them over chunks and rounds, so the second
                // call is what a run pays.
                let t = Instant::now();
                let _ = bound.local(&xs, &from, &steps).unwrap();
                let t_first = t.elapsed().as_secs_f64();
                let t = Instant::now();
                let found = bound.local(&xs, &from, &steps).unwrap();
                let t_dev = t.elapsed().as_secs_f64();
                let same = cpu.iter().zip(&found).filter(|(a, b)| distance(&a.r, &b.r) < 0.05).count();
                eprintln!("box {n} low-pass {low}: prepare {t_prepare:.2}s; local {} steps x {count} particles: CPU {t_cpu:.2}s, device {t_dev:.2}s (first call {t_first:.2}s; same step for {same})", steps.len());
            }
        }
        // The global search of the first model: box 32, the 30 degree grid.
        let n = 32;
        let mut rng = Rng(4);
        let w = wedge(n, -60.0, 60.0);
        let edge = ball(n, 9.6, 3.0);
        let mut cube = || -> Vec<f32> { edge.iter().map(|b| b * (1.0 + 0.3 * rng.gauss() as f32) + 0.5 * rng.gauss() as f32).collect() };
        let parts: Vec<Vec<f32>> = (0..count).map(|_| cube()).collect();
        let model = cube();
        let e = Engine::new(n, 9.0, 1.0, 12.0);
        let prep: Vec<Prepared> = parts.par_iter().map(|x| e.prepare(x, &w)).collect();
        let rotations = grid(30.0);
        let t = Instant::now();
        let cpu = e.global(&prep, &model, &rotations, 5, &|_, _| {}).unwrap();
        let t_cpu = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let _ = global(dev, &e, &prep, &model, &rotations, 5, &|_, _| {}).unwrap();
        let t_first = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let found = global(dev, &e, &prep, &model, &rotations, 5, &|_, _| {}).unwrap();
        let t_dev = t.elapsed().as_secs_f64();
        let same = cpu.iter().zip(&found).filter(|(a, b)| distance(&a.r, &b.r) < 0.05).count();
        eprintln!("global {} rotations x {count} particles (box 32): CPU {t_cpu:.2}s, device {t_dev:.2}s (first call {t_first:.2}s; same rotation for {same})", rotations.len());
    }
}
