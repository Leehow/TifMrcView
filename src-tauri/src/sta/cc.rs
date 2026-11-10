//! Constrained cross-correlation of subtomograms (Förster et al., PNAS
//! 102, 2005; the scoring Dynamo, PEET and PyTom also use).
//!
//! A subtomogram holds only the Fourier components its tilt series
//! measured (mask W, `wedge`). To compare it with a reference turned by R,
//! the reference is given the same gap, and both are limited to a band B
//! of frequencies (below the current resolution limit, above the slow
//! gradients of ice thickness):
//!
//!   CC(R, t) = Σ_x x(u + t)·p_R(u) / (‖x‖·‖p_R‖),
//!   X = B·W·FT(particle), P_R = B·W·FT(mask · rotate(reference, R)),
//!
//! computed for every shift t at once as the inverse transform of
//! X·conj(P_R). The best (R, t) is the pose; the peak is refined to a
//! fraction of a voxel with a parabola through its neighbours.
//!
//! This is the CPU reference; the GPU version scores the same quantity.

use rayon::prelude::*;

use super::{check_cancel, Pose};
use crate::spa::fourier::{freq, ft3, ift3};
use crate::spa::geom::{mul, transpose, Mat};
use anyhow::Result;

/// 1 where a tilt series from `lo` to `hi` degrees measured the Fourier
/// component, 0 in the missing wedge (cube of side n, unshifted indices).
/// A view at tilt θ measures the plane through the tilt axis (Y) that
/// contains the direction (cos θ, 0, sin θ); views between the first and
/// the last tilt are close enough together to count as a filled range.
pub fn wedge(n: usize, lo: f64, hi: f64) -> Vec<f32> {
    let (lo, hi) = (lo.min(hi) - 1e-6, lo.max(hi) + 1e-6);
    let mut out = vec![0f32; n * n * n];
    out.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
        let kz = freq(z, n) as f64;
        for (i, m) in plane.iter_mut().enumerate() {
            let kx = freq(i % n, n) as f64;
            let (mut a, mut b) = (kx, kz);
            if a < 0.0 || (a == 0.0 && b < 0.0) {
                (a, b) = (-a, -b);
            }
            let angle = if a == 0.0 && b == 0.0 { 0.0 } else { b.atan2(a).to_degrees() };
            *m = if (lo..=hi).contains(&angle) { 1.0 } else { 0.0 };
        }
    });
    out
}

/// out(p) = vol(M·(p − c) + c + t), trilinear, zero outside the cube.
pub fn transform(vol: &[f32], n: usize, m: &Mat, t: [f64; 3]) -> Vec<f32> {
    let c = (n / 2) as f64;
    let mut out = vec![0f32; n * n * n];
    let at = |x: isize, y: isize, z: isize| -> f32 {
        let n = n as isize;
        if x < 0 || y < 0 || z < 0 || x >= n || y >= n || z >= n {
            0.0
        } else {
            vol[((z * n + y) * n + x) as usize]
        }
    };
    out.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
        let pz = z as f64 - c;
        for y in 0..n {
            let py = y as f64 - c;
            for x in 0..n {
                let px = x as f64 - c;
                let q = [
                    m[0][0] * px + m[0][1] * py + m[0][2] * pz + c + t[0],
                    m[1][0] * px + m[1][1] * py + m[1][2] * pz + c + t[1],
                    m[2][0] * px + m[2][1] * py + m[2][2] * pz + c + t[2],
                ];
                let f = [q[0].floor(), q[1].floor(), q[2].floor()];
                let (x0, y0, z0) = (f[0] as isize, f[1] as isize, f[2] as isize);
                let (dx, dy, dz) = ((q[0] - f[0]) as f32, (q[1] - f[1]) as f32, (q[2] - f[2]) as f32);
                let mut v = 0.0;
                for (k, wz) in [(0, 1.0 - dz), (1, dz)] {
                    for (j, wy) in [(0, 1.0 - dy), (1, dy)] {
                        for (i, wx) in [(0, 1.0 - dx), (1, dx)] {
                            let w = wx * wy * wz;
                            if w != 0.0 {
                                v += w * at(x0 + i, y0 + j, z0 + k);
                            }
                        }
                    }
                }
                plane[y * n + x] = v;
            }
        }
    });
    out
}

/// The volume turned by R about its centre: a point q of it moves to R·q.
pub fn rotate(vol: &[f32], n: usize, r: &Mat) -> Vec<f32> {
    transform(vol, n, &transpose(r), [0.0; 3])
}

/// A ball of the given radius (voxels) with a raised-cosine edge.
pub fn ball(n: usize, radius: f64, edge: f64) -> Vec<f32> {
    let c = (n / 2) as f64;
    let mut out = vec![0f32; n * n * n];
    out.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
        for (i, m) in plane.iter_mut().enumerate() {
            let (x, y) = ((i % n) as f64 - c, (i / n) as f64 - c);
            let d = (x * x + y * y + (z as f64 - c).powi(2)).sqrt();
            *m = if d <= radius {
                1.0
            } else if d >= radius + edge {
                0.0
            } else {
                (0.5 + 0.5 * (std::f64::consts::PI * (d - radius) / edge).cos()) as f32
            };
        }
    });
    out
}

/// A particle ready for scoring: B·W·FT(particle) and Σ|·|².
pub struct Prepared {
    re: Vec<f32>,
    im: Vec<f32>,
    norm: f64,
}

/// A turned reference ready for scoring: B·FT(mask · rotate(reference, R)).
pub struct Turned {
    re: Vec<f32>,
    im: Vec<f32>,
}

pub struct Engine {
    pub n: usize,
    band: Vec<f32>,
    mask: Vec<f32>,
}

impl Engine {
    /// `low_pass` and `high_pass` are radii in Fourier voxels (the band
    /// kept lies between them; the low-pass edge rolls off over two
    /// voxels); `radius` is that of the ball the reference is masked with.
    pub fn new(n: usize, low_pass: f64, high_pass: f64, radius: f64) -> Engine {
        assert!(n.is_power_of_two() && n >= 8);
        let mut band = vec![0f32; n * n * n];
        band.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
            let kz = freq(z, n) as f64;
            for (i, b) in plane.iter_mut().enumerate() {
                let (kx, ky) = (freq(i % n, n) as f64, freq(i / n, n) as f64);
                let k = (kx * kx + ky * ky + kz * kz).sqrt();
                *b = if k < high_pass || k >= low_pass + 2.0 {
                    0.0
                } else if k <= low_pass {
                    1.0
                } else {
                    (0.5 + 0.5 * (std::f64::consts::PI * (k - low_pass) / 2.0).cos()) as f32
                };
            }
        });
        Engine { n, band, mask: ball(n, radius, 3.0) }
    }

    pub fn prepare(&self, particle: &[f32], wedge: &[f32]) -> Prepared {
        let (mut re, mut im) = ft3(self.n, particle);
        let mut norm = 0f64;
        for i in 0..re.len() {
            let w = self.band[i] * wedge[i];
            re[i] *= w;
            im[i] *= w;
            norm += (re[i] * re[i] + im[i] * im[i]) as f64;
        }
        Prepared { re, im, norm }
    }

    pub fn turn(&self, reference: &[f32], r: &Mat) -> Turned {
        let mut v = rotate(reference, self.n, r);
        v.iter_mut().zip(&self.mask).for_each(|(v, m)| *v *= m);
        let (mut re, mut im) = ft3(self.n, &v);
        for i in 0..re.len() {
            re[i] *= self.band[i];
            im[i] *= self.band[i];
        }
        Turned { re, im }
    }

    /// The best shift within `max_shift` voxels (per axis) and its score.
    pub fn score(&self, x: &Prepared, wedge: &[f32], p: &Turned, max_shift: usize) -> (f32, [f64; 3]) {
        let n = self.n;
        let mut cr = vec![0f32; n * n * n];
        let mut ci = vec![0f32; n * n * n];
        let mut pn = 0f64;
        for i in 0..cr.len() {
            if wedge[i] == 0.0 {
                continue;
            }
            let (pr, pi) = (p.re[i], p.im[i]);
            pn += (pr * pr + pi * pi) as f64;
            cr[i] = x.re[i] * pr + x.im[i] * pi;
            ci[i] = x.im[i] * pr - x.re[i] * pi;
        }
        let denom = (x.norm * pn).sqrt();
        if denom <= 0.0 {
            return (0.0, [0.0; 3]);
        }
        let cc = ift3(n, &cr, &ci);
        let (c, s) = ((n / 2) as isize, max_shift.min(n / 2 - 1) as isize);
        let at = |x: isize, y: isize, z: isize| cc[(((c + z) * n as isize + c + y) * n as isize + c + x) as usize];
        let mut best = (f32::MIN, [0isize; 3]);
        for z in -s..=s {
            for y in -s..=s {
                for x in -s..=s {
                    let v = at(x, y, z);
                    if v > best.0 {
                        best = (v, [x, y, z]);
                    }
                }
            }
        }
        // Parabola through the peak and its two neighbours along each axis.
        let [x, y, z] = best.1;
        let sub = |lo: f32, hi: f32| -> f64 {
            let d = lo - 2.0 * best.0 + hi;
            if d < 0.0 { (0.5 * (lo - hi) / d).clamp(-0.5, 0.5) as f64 } else { 0.0 }
        };
        let inside = |v: isize| v > -c && v < c - 1;
        let mut t = [x as f64, y as f64, z as f64];
        if inside(x) {
            t[0] += sub(at(x - 1, y, z), at(x + 1, y, z));
        }
        if inside(y) {
            t[1] += sub(at(x, y - 1, z), at(x, y + 1, z));
        }
        if inside(z) {
            t[2] += sub(at(x, y, z - 1), at(x, y, z + 1));
        }
        let scale = (n * n * n) as f64 / denom;
        ((best.0 as f64 * scale) as f32, t)
    }

    /// Every particle against every rotation of one list (a global
    /// search): the reference is turned once per rotation and shared.
    /// `wedge_of[i]` is the index into `wedges` of particle i's mask.
    pub fn global(
        &self,
        particles: &[Prepared],
        wedges: &[Vec<f32>],
        wedge_of: &[usize],
        reference: &[f32],
        rotations: &[Mat],
        max_shift: usize,
        tick: &(dyn Fn(usize, usize) + Sync),
    ) -> Result<Vec<Pose>> {
        let mut best: Vec<Pose> = vec![Pose { r: super::IDENTITY, t: [0.0; 3], score: f32::MIN }; particles.len()];
        for (j, r) in rotations.iter().enumerate() {
            check_cancel()?;
            let p = self.turn(reference, r);
            best.par_iter_mut().enumerate().for_each(|(i, b)| {
                let (score, t) = self.score(&particles[i], &wedges[wedge_of[i]], &p, max_shift);
                if score > b.score {
                    *b = Pose { r: *r, t, score };
                }
            });
            tick(j + 1, rotations.len());
        }
        Ok(best)
    }

    /// One particle around its previous orientation: every `steps` (small
    /// rotations, e.g. `geom::local`) applied on top of `from.r`.
    pub fn local(&self, x: &Prepared, wedge: &[f32], reference: &[f32], from: &Pose, steps: &[Mat], max_shift: usize) -> Pose {
        let mut best = Pose { score: f32::MIN, ..*from };
        for d in steps {
            let r = mul(d, &from.r);
            let (score, t) = self.score(x, wedge, &self.turn(reference, &r), max_shift);
            if score > best.score {
                best = Pose { r, t, score };
            }
        }
        best
    }
}
