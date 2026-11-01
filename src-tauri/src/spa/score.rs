//! The search shared by 2D classification and 3D refinement: for every
//! particle, the reference (projection or rotated class average) and shift
//! with the highest normalised cross-correlation
//!
//!   NCC(n, s, o) = Σ_k Re(X_n,s(k) · conj P_o(k)) · C_n(k) w(k) / √(Σ_k C_n(k)² w(k) |P_o(k)|²)
//!
//! Both sums are matrix products: particles × shifts against references
//! for the numerator, particles' C²w against references' |P|² for the
//! denominator, so they run on the GPU (Burn) when there is one and as
//! blocked dot products on the CPU otherwise.

use anyhow::Result;
use rayon::prelude::*;

use super::data::Coeffs;

/// References on the half plane: o × 2k interleaved re/im, and o × k |P|².
pub struct Refs {
    pub o: usize,
    pub k: usize,
    pub v: Vec<f32>,
    pub p2: Vec<f32>,
}

impl Refs {
    pub fn new(k: usize) -> Refs {
        Refs { o: 0, k, v: vec![], p2: vec![] }
    }
    pub fn push(&mut self, coeffs: &[f32]) {
        debug_assert_eq!(coeffs.len(), 2 * self.k);
        self.v.extend_from_slice(coeffs);
        self.p2.extend(coeffs.chunks_exact(2).map(|c| c[0] * c[0] + c[1] * c[1]));
        self.o += 1;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Best {
    pub o: u32,
    pub s: u16,
    pub score: f32,
}

impl Default for Best {
    fn default() -> Self {
        Best { o: 0, s: 0, score: f32::NEG_INFINITY }
    }
}

#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let (ra, rb) = (ca.remainder(), cb.remainder());
    for (x, y) in ca.zip(cb) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    let mut s: f32 = acc.iter().sum();
    for (x, y) in ra.iter().zip(rb) {
        s += x * y;
    }
    s
}

/// Best reference and shift for each particle of `c` (with `shifts` rows
/// each). Uses the GPU when there is one.
pub fn best(c: &Coeffs, shifts: usize, refs: &Refs) -> Result<Vec<Best>> {
    #[cfg(feature = "pro")]
    if let Some((dev, _)) = crate::ctf_gpu::device() {
        if let Ok(Ok(b)) = crate::ctf_gpu::guarded(|| gpu_best(dev, c, shifts, refs)) {
            return Ok(b);
        }
    }
    Ok(cpu_best(c, shifts, refs))
}

/// Where the search runs.
pub fn backend() -> &'static str {
    #[cfg(feature = "pro")]
    if let Some((_, name)) = crate::ctf_gpu::device() {
        return name;
    }
    "CPU"
}

pub fn cpu_best(c: &Coeffs, shifts: usize, refs: &Refs) -> Vec<Best> {
    const PB: usize = 16;
    const RB: usize = 32;
    let k = c.k;
    let n = c.c2.len() / k.max(1);
    let mut out = vec![Best::default(); n];
    for p0 in (0..n).step_by(PB) {
        let p1 = (p0 + PB).min(n);
        let chunks = refs.o.div_ceil(RB);
        let parts: Vec<Vec<Best>> = (0..chunks)
            .into_par_iter()
            .map(|ch| {
                let mut best = vec![Best::default(); p1 - p0];
                for o in ch * RB..((ch + 1) * RB).min(refs.o) {
                    let v = &refs.v[o * 2 * k..(o + 1) * 2 * k];
                    let p2 = &refs.p2[o * k..(o + 1) * k];
                    for p in p0..p1 {
                        let norm = dot(&c.c2[p * k..(p + 1) * k], p2).max(1e-20).sqrt();
                        for s in 0..shifts {
                            let row = &c.x[(p * shifts + s) * 2 * k..(p * shifts + s + 1) * 2 * k];
                            let sc = dot(row, v) / norm;
                            let b = &mut best[p - p0];
                            if sc > b.score {
                                *b = Best { o: o as u32, s: s as u16, score: sc };
                            }
                        }
                    }
                }
                best
            })
            .collect();
        for part in parts {
            for (i, b) in part.into_iter().enumerate() {
                if b.score > out[p0 + i].score {
                    out[p0 + i] = b;
                }
            }
        }
    }
    out
}

#[cfg(feature = "pro")]
fn gpu_best(dev: &burn::tensor::Device, c: &Coeffs, shifts: usize, refs: &Refs) -> Result<Vec<Best>> {
    use burn::tensor::{Tensor, TensorData};
    let k = c.k;
    let n = c.c2.len() / k.max(1);
    let mut out = vec![Best::default(); n];
    // Reference chunks of at most 2^24 values; particle blocks so the
    // score matrix stays under 2^25.
    let oc = ((1usize << 24) / (2 * k).max(1)).clamp(1, refs.o.max(1));
    let to_vec = |t: Tensor<2>| t.into_data().convert::<f32>().try_into_vec::<f32>().map_err(|e| anyhow::anyhow!("{e:?}"));
    for o0 in (0..refs.o).step_by(oc) {
        let o1 = (o0 + oc).min(refs.o);
        let m = o1 - o0;
        // Transposed: 2k × m and k × m.
        let mut vt = vec![0f32; 2 * k * m];
        let mut pt = vec![0f32; k * m];
        for o in 0..m {
            for j in 0..2 * k {
                vt[j * m + o] = refs.v[(o0 + o) * 2 * k + j];
            }
            for j in 0..k {
                pt[j * m + o] = refs.p2[(o0 + o) * k + j];
            }
        }
        let vt: Tensor<2> = Tensor::from_data(TensorData::new(vt, [2 * k, m]), dev);
        let pt: Tensor<2> = Tensor::from_data(TensorData::new(pt, [k, m]), dev);
        let pb = ((1usize << 25) / (shifts * m).max(1)).clamp(1, 4096);
        for p0 in (0..n).step_by(pb) {
            let p1 = (p0 + pb).min(n);
            let b = p1 - p0;
            let x: Tensor<2> = Tensor::from_data(TensorData::new(c.x[p0 * shifts * 2 * k..p1 * shifts * 2 * k].to_vec(), [b * shifts, 2 * k]), dev);
            let w: Tensor<2> = Tensor::from_data(TensorData::new(c.c2[p0 * k..p1 * k].to_vec(), [b, k]), dev);
            let cross = x.matmul(vt.clone()).reshape([b, shifts, m]);
            let norm = w.matmul(pt.clone()).clamp_min(1e-20).sqrt().reshape([b, 1, m]);
            let sc = (cross / norm).reshape([b, shifts * m]);
            let max = to_vec(sc.clone().max_dim(1))?;
            let arg = sc.argmax(1).into_data().convert::<i64>().try_into_vec::<i64>().map_err(|e| anyhow::anyhow!("{e:?}"))?;
            for i in 0..b {
                if max[i] > out[p0 + i].score {
                    let a = arg[i] as usize;
                    out[p0 + i] = Best { o: (o0 + a % m) as u32, s: (a / m) as u16, score: max[i] };
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_matching_reference_and_shift() {
        let k = 50;
        let mut refs = Refs::new(k);
        let mut seed = 1u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
        };
        let all: Vec<Vec<f32>> = (0..40).map(|_| (0..2 * k).map(|_| rnd()).collect()).collect();
        for r in &all {
            refs.push(r);
        }
        // Particle 0 matches ref 7 in its second shift row, particle 1 ref 30 in the first.
        let mut x = vec![0f32; 2 * 3 * 2 * k];
        x[2 * k..4 * k].copy_from_slice(&all[7]);
        x[0..2 * k].iter_mut().for_each(|v| *v = rnd());
        x[4 * k..6 * k].iter_mut().for_each(|v| *v = rnd());
        x[6 * k..8 * k].copy_from_slice(&all[30]);
        for v in &mut x[8 * k..12 * k] {
            *v = rnd();
        }
        let c = Coeffs { k, x, c2: vec![1.0; 2 * k] };
        let b = cpu_best(&c, 3, &refs);
        assert_eq!((b[0].o, b[0].s), (7, 1));
        assert_eq!((b[1].o, b[1].s), (30, 0));
        #[cfg(feature = "pro")]
        if let Some((dev, _)) = crate::ctf_gpu::device() {
            let g = gpu_best(dev, &c, 3, &refs).unwrap();
            assert_eq!((g[0].o, g[0].s, g[1].o, g[1].s), (7, 1, 30, 0));
        }
    }
}
