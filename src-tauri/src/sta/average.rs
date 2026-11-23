//! The average of aligned subtomograms with missing-wedge compensation.
//!
//! Each particle is brought back into the reference frame by its pose and
//! summed; its wedge mask, turned the same way, is summed too. Dividing
//! the transform of the summed particles by the summed masks gives every
//! Fourier component the mean over the particles that measured it, so
//! copies at different orientations fill each other's wedge (the weighting
//! of Förster et al. 2005, as in Dynamo and PEET). Components that few
//! particles measured are divided by a floor instead, which damps them.

use rayon::prelude::*;

use super::cc::transform;
use super::Pose;
use crate::spa::fourier::{freq, ft3, ift3, index};

pub struct Sum {
    pub n: usize,
    data: Vec<f32>,
    weight: Vec<f32>,
    pub count: usize,
}

impl Sum {
    pub fn new(n: usize) -> Sum {
        Sum { n, data: vec![0.0; n * n * n], weight: vec![0.0; n * n * n], count: 0 }
    }

    pub fn add(&mut self, particle: &[f32], wedge: &[f32], pose: &Pose) {
        let n = self.n;
        // back(p) = particle(R·(p − c) + c + t) = reference(p).
        let back = transform(particle, n, &pose.r, pose.t);
        self.data.par_iter_mut().zip(&back).for_each(|(d, b)| *d += b);
        // FT(back)(k) = FT(particle)(R·k): the binary wedge mask turns with it.
        // Keep exact nearest-index lookup rather than linear blending across the
        // 0/1 missing-wedge cliff so boundary voxels are never divided by 0.5.
        let r = pose.r;
        let half = (n / 2) as i32;
        self.weight.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
            let kz = freq(z, n) as f64;
            for (i, w) in plane.iter_mut().enumerate() {
                let (kx, ky) = (freq(i % n, n) as f64, freq(i / n, n) as f64);
                let q = [
                    (r[0][0] * kx + r[0][1] * ky + r[0][2] * kz).round() as i32,
                    (r[1][0] * kx + r[1][1] * ky + r[1][2] * kz).round() as i32,
                    (r[2][0] * kx + r[2][1] * ky + r[2][2] * kz).round() as i32,
                ];
                if q.iter().all(|v| *v >= -half && *v < half) {
                    *w += wedge[(index(q[2], n) * n + index(q[1], n)) * n + index(q[0], n)];
                }
            }
        });
        self.count += 1;
    }

    pub fn merge(&mut self, other: &Sum) {
        self.data.iter_mut().zip(&other.data).for_each(|(a, b)| *a += b);
        self.weight.iter_mut().zip(&other.weight).for_each(|(a, b)| *a += b);
        self.count += other.count;
    }

    /// The weighted average. Components measured by fewer than a tenth of
    /// the particles are divided as if a tenth had measured them; unphysical
    /// Fourier cube corners strictly beyond the Nyquist sphere (|k| > n/2)
    /// are smoothly rolled off so rotated-box corner noise is never amplified
    /// by the weight floor without truncating any shell inside |k| <= n/2.
    pub fn finish(&self) -> Vec<f32> {
        let n = self.n;
        let nyq = (n / 2) as f64;
        let (mut re, mut im) = ft3(n, &self.data);
        let floor = (0.1 * self.count as f32).max(1.0);
        re.par_chunks_mut(n * n)
            .zip(im.par_chunks_mut(n * n))
            .zip(self.weight.par_chunks(n * n))
            .enumerate()
            .for_each(|(z, ((rp, ip), wp))| {
                let kz = freq(z, n) as f64;
                let kz2 = kz * kz;
                for i in 0..n * n {
                    let (kx, ky) = (freq(i % n, n) as f64, freq(i / n, n) as f64);
                    let r = (kx * kx + ky * ky + kz2).sqrt();
                    let corner = if r <= nyq {
                        1.0f32
                    } else if r >= nyq + 2.0 {
                        0.0f32
                    } else {
                        (0.5 + 0.5 * (std::f64::consts::PI * (r - nyq) / 2.0).cos()) as f32
                    };
                    if corner == 0.0 {
                        rp[i] = 0.0;
                        ip[i] = 0.0;
                    } else {
                        let d = wp[i].max(floor);
                        rp[i] = rp[i] / d * corner;
                        ip[i] = ip[i] / d * corner;
                    }
                }
            });
        ift3(n, &re, &im)
    }

    /// The share of Fourier components (inside the sphere of radius n/2)
    /// that at least a tenth of the particles measured: 1 when the
    /// orientations cover the wedge.
    pub fn coverage(&self) -> f64 {
        let n = self.n;
        let floor = (0.1 * self.count as f32).max(1.0);
        let r2 = ((n / 2) * (n / 2)) as i32;
        let (mut inside, mut filled) = (0usize, 0usize);
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let (a, b, c) = (freq(x, n), freq(y, n), freq(z, n));
                    if a * a + b * b + c * c < r2 {
                        inside += 1;
                        filled += (self.weight[(z * n + y) * n + x] >= floor) as usize;
                    }
                }
            }
        }
        filled as f64 / inside.max(1) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fourier cube corners strictly beyond Nyquist (|k| >= n/2 + 2) are damped
    /// to zero while valid shells inside |k| <= n/2 remain completely intact.
    #[test]
    fn finish_damps_unphysical_corners_beyond_nyquist_without_affecting_inner_shells() {
        let n = 16usize;
        let mut sum = Sum::new(n);
        let wedge = vec![1.0f32; n * n * n];
        // Build a signal with one component inside Nyquist (k=(4,0,0)) and one
        // pure corner component beyond Nyquist (k=(8,8,8), |k| ≈ 13.9 > n/2 + 2).
        let vol: Vec<f32> = (0..n * n * n)
            .map(|i| {
                let (x, y, z) = (i % n, i / n % n, i / n / n);
                let inner = (2.0 * std::f64::consts::PI * 4.0 * x as f64 / n as f64).cos();
                let corner = if (x + y + z) % 2 == 0 { 1.0 } else { -1.0 };
                (inner + corner) as f32
            })
            .collect();
        let pose = Pose { r: super::super::IDENTITY, t: [0.0; 3], score: 1.0 };
        sum.add(&vol, &wedge, &pose);
        let out = sum.finish();
        let (re, im) = ft3(n, &out);
        let inner_idx = (index(0, n) * n + index(0, n)) * n + index(4, n);
        let corner_idx = (index(8, n) * n + index(8, n)) * n + index(8, n);
        let inner_amp = (re[inner_idx].hypot(im[inner_idx])) as f64 / (n * n * n) as f64;
        let corner_amp = (re[corner_idx].hypot(im[corner_idx])) as f64 / (n * n * n) as f64;
        assert!((inner_amp - 0.5).abs() < 1e-4, "inner shell amplitude altered: {inner_amp}");
        assert!(corner_amp < 1e-6, "corner beyond Nyquist should be zeroed: {corner_amp}");
    }
}
