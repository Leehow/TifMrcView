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
        // FT(back)(k) = FT(particle)(R·k): the mask turns with it.
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
    /// the particles are divided as if a tenth had measured them.
    pub fn finish(&self) -> Vec<f32> {
        let (mut re, mut im) = ft3(self.n, &self.data);
        let floor = (0.1 * self.count as f32).max(1.0);
        re.par_iter_mut().zip(im.par_iter_mut()).zip(&self.weight).for_each(|((r, i), w)| {
            let d = w.max(floor);
            *r /= d;
            *i /= d;
        });
        ift3(self.n, &re, &im)
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
