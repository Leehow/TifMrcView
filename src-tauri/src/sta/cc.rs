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
//! computed on the band's frequencies only (`Engine`): the turned
//! reference is read from the reference's transform, and the correlation
//! is summed directly for the few shifts wanted. The best (R, t) is the
//! pose; the peak is refined to a fraction of a voxel with a parabola
//! through its neighbours.

use rayon::prelude::*;

use super::{check_cancel, Pose};
use crate::spa::fourier::{freq, ft3, index};
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

/// The frequencies of the band, half of them: a real volume's transform
/// at −k is the conjugate of that at k, so every sum over the band is
/// twice the real part of the sum over a half (the origin counts half).
/// Points are in rows along kx.
struct Band {
    /// (kx, ky, kz).
    k: Vec<[i16; 3]>,
    /// Index in an n³ transform.
    at: Vec<u32>,
    /// Band weight, halved at the origin.
    weight: Vec<f32>,
    /// (ky, kz, first point, one past the last).
    rows: Vec<(i16, i16, u32, u32)>,
    /// Largest |k| per axis.
    reach: i32,
}

/// A particle ready for scoring: B·W·FT(particle) on the band, which
/// frequencies its wedge measured, and Σ|·|².
pub struct Prepared {
    re: Vec<f32>,
    im: Vec<f32>,
    open: Vec<bool>,
    norm: f64,
}

/// A reference ready to be turned: the transform of the masked reference
/// in a box twice the size, so it is sampled at half steps of frequency.
pub struct Reference {
    re: Vec<f32>,
    im: Vec<f32>,
}

/// A turned reference ready for scoring: B·FT(rotate(mask · reference, R))
/// on the band.
pub struct Turned {
    re: Vec<f32>,
    im: Vec<f32>,
}

/// Scoring works on the band alone. A rotation of the reference is a
/// rotation of its transform, so a turned reference is read from the
/// reference's (finely sampled) transform at the turned band points,
/// without a transform per orientation; the shifts wanted are few, so the
/// correlation is summed for them directly, axis by axis, instead of with
/// an inverse transform of the whole cube.
pub struct Engine {
    pub n: usize,
    band: Band,
    mask: Vec<f32>,
}

fn band_weight(k: f64, low_pass: f64, high_pass: f64) -> f32 {
    if k < high_pass || k >= low_pass + 2.0 {
        0.0
    } else if k <= low_pass {
        1.0
    } else {
        (0.5 + 0.5 * (std::f64::consts::PI * (k - low_pass) / 2.0).cos()) as f32
    }
}

impl Engine {
    /// `low_pass` and `high_pass` are radii in Fourier voxels (the band
    /// kept lies between them; the low-pass edge rolls off over two
    /// voxels); `radius` is that of the ball the reference is masked with.
    pub fn new(n: usize, low_pass: f64, high_pass: f64, radius: f64) -> Engine {
        assert!(n.is_power_of_two() && n >= 8);
        let reach = ((low_pass + 2.0).ceil() as i32).min(n as i32 / 2 - 1);
        let mut band = Band { k: vec![], at: vec![], weight: vec![], rows: vec![], reach };
        for kz in 0..=reach {
            for ky in -reach..=reach {
                if kz == 0 && ky < 0 {
                    continue;
                }
                let first = band.k.len() as u32;
                for kx in -reach..=reach {
                    if kz == 0 && ky == 0 && kx < 0 {
                        continue;
                    }
                    let w = band_weight(((kx * kx + ky * ky + kz * kz) as f64).sqrt(), low_pass, high_pass);
                    if w == 0.0 {
                        continue;
                    }
                    band.k.push([kx as i16, ky as i16, kz as i16]);
                    band.at.push(((index(kz, n) * n + index(ky, n)) * n + index(kx, n)) as u32);
                    band.weight.push(if (kx, ky, kz) == (0, 0, 0) { 0.5 * w } else { w });
                }
                if band.k.len() as u32 > first {
                    band.rows.push((ky as i16, kz as i16, first, band.k.len() as u32));
                }
            }
        }
        Engine { n, band, mask: ball(n, radius, 3.0) }
    }

    pub fn prepare(&self, particle: &[f32], wedge: &[f32]) -> Prepared {
        let (fr, fi) = ft3(self.n, particle);
        let m = self.band.k.len();
        let (mut re, mut im, mut open) = (Vec::with_capacity(m), Vec::with_capacity(m), Vec::with_capacity(m));
        let mut norm = 0f64;
        for (at, b) in self.band.at.iter().zip(&self.band.weight) {
            let i = *at as usize;
            let w = b * wedge[i];
            let (r, j) = (fr[i] * w, fi[i] * w);
            norm += (r * r + j * j) as f64;
            re.push(r);
            im.push(j);
            open.push(wedge[i] != 0.0);
        }
        Prepared { re, im, open, norm }
    }

    /// The reference masked and transformed once, for any number of turns.
    pub fn reference(&self, reference: &[f32]) -> Reference {
        let n = self.n;
        let (big, c) = (2 * n, (n / 2) as f64);
        // Reading between the samples of the transform (trilinear) dims
        // the volume by sinc² of the distance from the centre along each
        // axis; divided out here.
        let gain: Vec<f32> = (0..n)
            .map(|i| {
                let u = std::f64::consts::PI * (i as f64 - c) / big as f64;
                if u == 0.0 { 1.0 } else { (u / u.sin()).powi(2) as f32 }
            })
            .collect();
        let mut padded = vec![0f32; big * big * big];
        // The cube's centre n/2 goes to the big cube's centre n.
        let o = n - n / 2;
        padded.par_chunks_mut(big * big).enumerate().for_each(|(z, plane)| {
            if z < o || z >= o + n {
                return;
            }
            let z = z - o;
            for y in 0..n {
                let row = (z * n + y) * n;
                for x in 0..n {
                    plane[(y + o) * big + x + o] = reference[row + x] * self.mask[row + x] * gain[x] * gain[y] * gain[z];
                }
            }
        });
        let (re, im) = ft3(big, &padded);
        Reference { re, im }
    }

    /// FT(rotate(v, R))(k) = FT(v)(Rᵀk).
    pub fn turn(&self, reference: &Reference, r: &Mat) -> Turned {
        let big = 2 * self.n as i32;
        let m = self.band.k.len();
        let (mut re, mut im) = (Vec::with_capacity(m), Vec::with_capacity(m));
        // Rᵀk in half steps of frequency.
        let t: [[f32; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| 2.0 * r[j][i] as f32));
        let wrap = |v: i32| v.rem_euclid(big) as usize;
        let side = big as usize;
        for (k, b) in self.band.k.iter().zip(&self.band.weight) {
            let (kx, ky, kz) = (k[0] as f32, k[1] as f32, k[2] as f32);
            let q = [t[0][0] * kx + t[0][1] * ky + t[0][2] * kz, t[1][0] * kx + t[1][1] * ky + t[1][2] * kz, t[2][0] * kx + t[2][1] * ky + t[2][2] * kz];
            let f = [q[0].floor(), q[1].floor(), q[2].floor()];
            let d = [q[0] - f[0], q[1] - f[1], q[2] - f[2]];
            let (x0, y0, z0) = (f[0] as i32, f[1] as i32, f[2] as i32);
            let xs = [wrap(x0), wrap(x0 + 1)];
            let (mut vr, mut vi) = (0f32, 0f32);
            for (z, wz) in [(wrap(z0), 1.0 - d[2]), (wrap(z0 + 1), d[2])] {
                for (y, wy) in [(wrap(y0), 1.0 - d[1]), (wrap(y0 + 1), d[1])] {
                    let row = (z * side + y) * side;
                    let (a, c) = (row + xs[0], row + xs[1]);
                    let w = wz * wy;
                    vr += w * ((1.0 - d[0]) * reference.re[a] + d[0] * reference.re[c]);
                    vi += w * ((1.0 - d[0]) * reference.im[a] + d[0] * reference.im[c]);
                }
            }
            re.push(vr * b);
            im.push(vi * b);
        }
        Turned { re, im }
    }

    /// The best shift within `max_shift` voxels (per axis) and its score.
    pub fn score(&self, x: &Prepared, p: &Turned, max_shift: usize) -> (f32, [f64; 3]) {
        let n = self.n;
        let mut pn = 0f64;
        for i in 0..p.re.len() {
            if x.open[i] {
                pn += (p.re[i] * p.re[i] + p.im[i] * p.im[i]) as f64;
            }
        }
        let denom = (x.norm * pn).sqrt();
        if denom <= 0.0 {
            return (0.0, [0.0; 3]);
        }
        let s = max_shift.min(n / 2 - 1) as i32;
        let side = (2 * s + 1) as usize;
        let reach = self.band.reach;
        // e^{2πi·k·t/n} for k in −reach..=reach, t in −s..=s.
        let wide = (2 * reach + 1) as usize;
        let mut wave = vec![(0f32, 0f32); wide * side];
        for k in -reach..=reach {
            for t in -s..=s {
                let a = 2.0 * std::f64::consts::PI * (k * t).rem_euclid(n as i32) as f64 / n as f64;
                wave[(k + reach) as usize * side + (t + s) as usize] = (a.cos() as f32, a.sin() as f32);
            }
        }
        let wave_of = |k: i16| -> &[(f32, f32)] { &wave[(k as i32 + reach) as usize * side..][..side] };
        // Along kx for each row, then ky into a plane per kz, then kz.
        let mut plane = vec![(0f32, 0f32); side * side];
        let mut cube = vec![0f32; side * side * side];
        let mut line = vec![(0f32, 0f32); side];
        let mut current = i16::MIN;
        let flush = |kz: i16, plane: &mut Vec<(f32, f32)>, cube: &mut Vec<f32>| {
            let w = wave_of(kz);
            for (tz, (c, d)) in w.iter().enumerate() {
                let out = &mut cube[tz * side * side..][..side * side];
                for (o, (a, b)) in out.iter_mut().zip(plane.iter()) {
                    *o += a * c - b * d;
                }
            }
            plane.iter_mut().for_each(|v| *v = (0.0, 0.0));
        };
        for &(ky, kz, first, last) in &self.band.rows {
            if kz != current {
                if current != i16::MIN {
                    flush(current, &mut plane, &mut cube);
                }
                current = kz;
            }
            line.iter_mut().for_each(|v| *v = (0.0, 0.0));
            for i in first as usize..last as usize {
                // X·conj(P).
                let (pr, pi) = (p.re[i], p.im[i]);
                let (cr, ci) = (x.re[i] * pr + x.im[i] * pi, x.im[i] * pr - x.re[i] * pi);
                if cr == 0.0 && ci == 0.0 {
                    continue;
                }
                for (l, (c, d)) in line.iter_mut().zip(wave_of(self.band.k[i][0])) {
                    l.0 += cr * c - ci * d;
                    l.1 += cr * d + ci * c;
                }
            }
            let w = wave_of(ky);
            for (ty, (c, d)) in w.iter().enumerate() {
                for (o, (a, b)) in plane[ty * side..][..side].iter_mut().zip(&line) {
                    o.0 += a * c - b * d;
                    o.1 += a * d + b * c;
                }
            }
        }
        if current != i16::MIN {
            flush(current, &mut plane, &mut cube);
        }
        let at = |x: i32, y: i32, z: i32| cube[(((z + s) as usize * side) + (y + s) as usize) * side + (x + s) as usize];
        let mut best = (f32::MIN, [0i32; 3]);
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
        let inside = |v: i32| v > -s && v < s;
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
        // Both norms are sums over the half band, as the correlation is.
        ((best.0 as f64 / denom) as f32, t)
    }

    /// Every particle against every rotation of one list (a global
    /// search): the reference is turned once per rotation and shared.
    pub fn global(&self, particles: &[Prepared], reference: &[f32], rotations: &[Mat], max_shift: usize, tick: &(dyn Fn(usize, usize) + Sync)) -> Result<Vec<Pose>> {
        let reference = self.reference(reference);
        let mut best: Vec<Pose> = vec![Pose { r: super::IDENTITY, t: [0.0; 3], score: f32::MIN }; particles.len()];
        for (j, r) in rotations.iter().enumerate() {
            check_cancel()?;
            let p = self.turn(&reference, r);
            best.par_iter_mut().enumerate().for_each(|(i, b)| {
                let (score, t) = self.score(&particles[i], &p, max_shift);
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
    pub fn local(&self, x: &Prepared, reference: &Reference, from: &Pose, steps: &[Mat], max_shift: usize) -> Pose {
        let mut best = Pose { score: f32::MIN, ..*from };
        for d in steps {
            let r = mul(d, &from.r);
            let (score, t) = self.score(x, &self.turn(reference, &r), max_shift);
            if score > best.score {
                best = Pose { r, t, score };
            }
        }
        best
    }
}
