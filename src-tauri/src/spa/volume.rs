//! Fourier-space projection and reconstruction (central slice theorem).
//!
//! Projections are slices through the transform of the zero-padded
//! reference, interpolated trilinearly; the reference is first divided by
//! the real-space attenuation of that interpolation (sinc²), so the slices
//! keep their high frequencies. Reconstruction inserts each particle's
//! Σ C·w·X and Σ C²·w at its slice with trilinear weights and divides
//! per voxel, with a Wiener term from the half-map FSC (as in the
//! regularised likelihood methods of Scheres, J Mol Biol 2012), then
//! transforms back. This is an independent implementation.

use rayon::prelude::*;

use super::fourier::{self, Half};
use super::geom::Mat;

fn sinc(u: f64) -> f64 {
    if u.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u) }
}

/// Padding factor for a working box: 2 up to 128 pixels, 1 above (memory).
pub fn pad_for(b: usize) -> usize {
    if b <= 128 { 2 } else { 1 }
}

/// The transform of a padded reference, for extracting slices.
pub struct Projector {
    pub n: usize,
    pub pad: usize,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Projector {
    /// `vol` is b³ (z, y, x), centred at b/2.
    pub fn new(vol: &[f32], b: usize, pad: usize) -> Projector {
        let n = b * pad;
        let o = (n - b) / 2;
        let mut big = vec![0f32; n * n * n];
        let c = |i: usize| sinc((i as f64 - b as f64 / 2.0) / n as f64).powi(2);
        let corr: Vec<f64> = (0..b).map(c).collect();
        big.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
            if z < o || z >= o + b {
                return;
            }
            let zz = z - o;
            for y in 0..b {
                for x in 0..b {
                    let v = vol[(zz * b + y) * b + x] as f64 / (corr[x] * corr[y] * corr[zz]);
                    plane[(y + o) * n + x + o] = v as f32;
                }
            }
        });
        let (re, im) = fourier::ft3(n, &big);
        Projector { n, pad, re, im }
    }

    /// The padded transform (re, im), n³ unshifted.
    pub fn data(&self) -> (&[f32], &[f32]) {
        (&self.re, &self.im)
    }

    #[inline]
    fn at(&self, q: [f64; 3]) -> (f32, f32) {
        let n = self.n;
        let h = (n / 2) as f64 - 1.0;
        if q[0].abs() > h || q[1].abs() > h || q[2].abs() > h {
            return (0.0, 0.0);
        }
        let f = [q[0].floor(), q[1].floor(), q[2].floor()];
        let t = [(q[0] - f[0]) as f32, (q[1] - f[1]) as f32, (q[2] - f[2]) as f32];
        let i0 = [fourier::index(f[0] as i32, n), fourier::index(f[1] as i32, n), fourier::index(f[2] as i32, n)];
        let i1 = [(i0[0] + 1) % n, (i0[1] + 1) % n, (i0[2] + 1) % n];
        let (mut r, mut m) = (0f32, 0f32);
        for (iz, wz) in [(i0[2], 1.0 - t[2]), (i1[2], t[2])] {
            for (iy, wy) in [(i0[1], 1.0 - t[1]), (i1[1], t[1])] {
                for (ix, wx) in [(i0[0], 1.0 - t[0]), (i1[0], t[0])] {
                    let w = wx * wy * wz;
                    let j = (iz * n + iy) * n + ix;
                    r += w * self.re[j];
                    m += w * self.im[j];
                }
            }
        }
        (r, m)
    }

    /// The slice at orientation `r` on `half` (interleaved re/im).
    pub fn slice(&self, r: &Mat, half: &Half, out: &mut [f32]) {
        let p = self.pad as f64;
        for (j, &[kx, ky]) in half.k.iter().enumerate() {
            let (kx, ky) = (kx as f64 * p, ky as f64 * p);
            let q = [r[0][0] * kx + r[1][0] * ky, r[0][1] * kx + r[1][1] * ky, r[0][2] * kx + r[1][2] * ky];
            let (a, b) = self.at(q);
            out[2 * j] = a;
            out[2 * j + 1] = b;
        }
    }
}

/// A value to insert at a point of the padded grid.
#[derive(Clone, Copy)]
pub struct Pt {
    pub q: [f32; 3],
    pub re: f32,
    pub im: f32,
    pub w: f32,
}

/// Σ C·w·X and Σ C²·w on the padded grid.
pub struct Accum {
    pub n: usize,
    pub pad: usize,
    pub b: usize,
    pub re: Vec<f32>,
    pub im: Vec<f32>,
    pub den: Vec<f32>,
}

impl Accum {
    pub fn new(b: usize, pad: usize) -> Accum {
        let n = b * pad;
        Accum { n, pad, b, re: vec![0.0; n * n * n], im: vec![0.0; n * n * n], den: vec![0.0; n * n * n] }
    }

    /// The points of one particle at orientation `r` and its symmetry mates.
    pub fn points(&self, r: &Mat, sym: &[Mat], half: &Half, x: &[f32], c2: &[f32], out: &mut Vec<Pt>) {
        let p = self.pad as f64;
        for g in sym {
            // Volume coordinate of a slice point: gᵀ·Rᵀ·k... the mate of
            // orientation R under symmetry g is R·g.
            let m = super::geom::mul(r, g);
            for (j, &[kx, ky]) in half.k.iter().enumerate() {
                let (kx, ky) = (kx as f64 * p, ky as f64 * p);
                let q = [m[0][0] * kx + m[1][0] * ky, m[0][1] * kx + m[1][1] * ky, m[0][2] * kx + m[1][2] * ky];
                out.push(Pt { q: [q[0] as f32, q[1] as f32, q[2] as f32], re: x[2 * j], im: x[2 * j + 1], w: c2[j] });
            }
        }
    }

    /// Adds points and their Hermitian mates (−q, conjugate).
    pub fn insert(&mut self, pts: &[Pt]) {
        let n = self.n;
        let lim = (n / 2) as f32 - 1.0;
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (i, p) in pts.iter().enumerate() {
            if p.q[0].abs() > lim || p.q[1].abs() > lim || p.q[2].abs() > lim {
                continue;
            }
            buckets[fourier::index(p.q[2].floor() as i32, n)].push((2 * i) as u32);
            buckets[fourier::index((-p.q[2]).floor() as i32, n)].push((2 * i + 1) as u32);
        }
        let nn = n * n;
        self.re.par_chunks_mut(nn).zip(self.im.par_chunks_mut(nn)).zip(self.den.par_chunks_mut(nn)).enumerate().for_each(|(z, ((pr, pi), pd))| {
            let below = (z + n - 1) % n;
            for (bucket, upper) in [(&buckets[z], false), (&buckets[below], true)] {
                for &code in bucket {
                    let p = &pts[(code / 2) as usize];
                    let (s, conj) = if code & 1 == 0 { (1.0f32, 1.0f32) } else { (-1.0, -1.0) };
                    let q = [p.q[0] * s, p.q[1] * s, p.q[2] * s];
                    let tz = q[2] - q[2].floor();
                    let wz = if upper { tz } else { 1.0 - tz };
                    if wz <= 0.0 {
                        continue;
                    }
                    let (fx, fy) = (q[0].floor(), q[1].floor());
                    let (tx, ty) = (q[0] - fx, q[1] - fy);
                    let x0 = fourier::index(fx as i32, n);
                    let y0 = fourier::index(fy as i32, n);
                    let (x1, y1) = ((x0 + 1) % n, (y0 + 1) % n);
                    for (iy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                        for (ix, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                            let w = wx * wy * wz;
                            let j = iy * n + ix;
                            pr[j] += w * p.re;
                            pi[j] += w * p.im * conj;
                            pd[j] += w * p.w;
                        }
                    }
                }
            }
        });
    }

    /// Applies a point group to inserted data: every voxel gathers the
    /// sums at its symmetry mates (trilinear). Particles are then inserted
    /// once instead of once per mate (as RELION does), which on D7 made
    /// insertion 14× cheaper; it was over half of a refinement round.
    pub fn symmetrize(&mut self, sym: &[Mat]) {
        if sym.len() <= 1 {
            return;
        }
        let n = self.n;
        let lim = (n / 2) as f64 - 1.0;
        let (src_re, src_im, src_den) = (&self.re, &self.im, &self.den);
        let sample = |a: &[f32], q: [f64; 3]| -> f32 {
            let (fx, fy, fz) = (q[0].floor(), q[1].floor(), q[2].floor());
            let (tx, ty, tz) = ((q[0] - fx) as f32, (q[1] - fy) as f32, (q[2] - fz) as f32);
            let x0 = fourier::index(fx as i32, n);
            let y0 = fourier::index(fy as i32, n);
            let z0 = fourier::index(fz as i32, n);
            let (x1, y1, z1) = ((x0 + 1) % n, (y0 + 1) % n, (z0 + 1) % n);
            let mut s = 0f32;
            for (iz, wz) in [(z0, 1.0 - tz), (z1, tz)] {
                for (iy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                    for (ix, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                        s += wx * wy * wz * a[(iz * n + iy) * n + ix];
                    }
                }
            }
            s
        };
        let mut re = vec![0f32; n * n * n];
        let mut im = vec![0f32; n * n * n];
        let mut den = vec![0f32; n * n * n];
        re.par_chunks_mut(n * n).zip(im.par_chunks_mut(n * n)).zip(den.par_chunks_mut(n * n)).enumerate().for_each(|(z, ((pr, pi), pd))| {
            let fz = fourier::freq(z, n) as f64;
            for y in 0..n {
                let fy = fourier::freq(y, n) as f64;
                for x in 0..n {
                    let fx = fourier::freq(x, n) as f64;
                    if fx * fx + fy * fy + fz * fz > lim * lim {
                        continue;
                    }
                    let (mut a, mut b, mut c) = (0f32, 0f32, 0f32);
                    for g in sym {
                        let q = [
                            g[0][0] * fx + g[0][1] * fy + g[0][2] * fz,
                            g[1][0] * fx + g[1][1] * fy + g[1][2] * fz,
                            g[2][0] * fx + g[2][1] * fy + g[2][2] * fz,
                        ];
                        a += sample(src_re, q);
                        b += sample(src_im, q);
                        c += sample(src_den, q);
                    }
                    let j = y * n + x;
                    pr[j] = a;
                    pi[j] = b;
                    pd[j] = c;
                }
            }
        });
        self.re = re;
        self.im = im;
        self.den = den;
    }

    pub fn add(&mut self, other: &Accum) {
        self.re.par_iter_mut().zip(&other.re).for_each(|(a, b)| *a += b);
        self.im.par_iter_mut().zip(&other.im).for_each(|(a, b)| *a += b);
        self.den.par_iter_mut().zip(&other.den).for_each(|(a, b)| *a += b);
    }

    pub fn scale(&mut self, f: f32) {
        self.re.par_iter_mut().for_each(|a| *a *= f);
        self.im.par_iter_mut().for_each(|a| *a *= f);
        self.den.par_iter_mut().for_each(|a| *a *= f);
    }

    /// The b³ map. `fsc` (per shell of the b box) sets the Wiener term
    /// mean(Σ C²w)·(1 − FSC)/FSC and cuts frequencies where it falls
    /// below 0.01; without it a small constant term is used.
    pub fn reconstruct(&self, fsc: Option<&[f64]>) -> Vec<f32> {
        let n = self.n;
        let (b, pad) = (self.b, self.pad);
        let shells = n / 2 + 1;
        let radius = |z: usize, y: usize, x: usize| {
            let (fx, fy, fz) = (fourier::freq(x, n), fourier::freq(y, n), fourier::freq(z, n));
            ((fx * fx + fy * fy + fz * fz) as f64).sqrt()
        };
        // Mean of the denominator per shell (over voxels that received data).
        let mut sum = vec![0f64; shells];
        let mut cnt = vec![0f64; shells];
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let r = radius(z, y, x).round() as usize;
                    if r < shells {
                        let d = self.den[(z * n + y) * n + x] as f64;
                        sum[r] += d;
                        cnt[r] += 1.0;
                    }
                }
            }
        }
        let lam: Vec<f64> = (0..shells)
            .map(|r| {
                let mean = sum[r] / cnt[r].max(1.0);
                let f = match fsc {
                    Some(f) => {
                        let rb = r as f64 / pad as f64;
                        let i = rb.floor() as usize;
                        let t = rb - i as f64;
                        let a = f.get(i).copied().unwrap_or(0.0);
                        let c = f.get(i + 1).copied().unwrap_or(a);
                        a * (1.0 - t) + c * t
                    }
                    None => 0.95,
                };
                if f < 0.01 { f64::INFINITY } else { (mean * (1.0 - f.min(0.999)) / f).max(mean * 1e-3).max(1e-30) }
            })
            .collect();
        let mut re = vec![0f32; n * n * n];
        let mut im = vec![0f32; n * n * n];
        re.par_chunks_mut(n * n).zip(im.par_chunks_mut(n * n)).enumerate().for_each(|(z, (pr, pi))| {
            for y in 0..n {
                for x in 0..n {
                    let j = (z * n + y) * n + x;
                    let rr = radius(z, y, x);
                    let r = rr.round() as usize;
                    if r >= shells || rr > (b / 2) as f64 * pad as f64 {
                        continue;
                    }
                    let l = lam[r];
                    if !l.is_finite() {
                        continue;
                    }
                    let d = self.den[(z * n + y) * n + x] as f64 + l;
                    pr[y * n + x] = (self.re[j] as f64 / d) as f32;
                    pi[y * n + x] = (self.im[j] as f64 / d) as f32;
                }
            }
        });
        let big = fourier::ift3(n, &re, &im);
        let o = (n - b) / 2;
        // Slice values are sums over voxels, so the padded grid needs no rescaling.
        let s = 1.0f32;
        let mut out = vec![0f32; b * b * b];
        out.par_chunks_mut(b * b).enumerate().for_each(|(z, plane)| {
            for y in 0..b {
                for x in 0..b {
                    plane[y * b + x] = big[((z + o) * n + y + o) * n + x + o] * s;
                }
            }
        });
        out
    }
}

/// Spherical soft mask in place: radius and edge in pixels.
pub fn mask_sphere(v: &mut [f32], b: usize, radius: f64, edge: f64) {
    let c = b as f64 / 2.0;
    v.par_chunks_mut(b * b).enumerate().for_each(|(z, plane)| {
        for y in 0..b {
            for x in 0..b {
                let d = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2) + (z as f64 - c).powi(2)).sqrt();
                plane[y * b + x] *= super::data::soft(d, radius, edge);
            }
        }
    });
}

/// Projection of `vol` (b³) along z after rotation `r`, in real space, via
/// the slice of the full plane: for tests and the class-average check.
pub fn project(p: &Projector, r: &Mat, b: usize) -> Vec<f32> {
    let pd = p.pad as f64;
    let (mut re, mut im) = (vec![0f32; b * b], vec![0f32; b * b]);
    for y in 0..b {
        for x in 0..b {
            let (kx, ky) = (fourier::freq(x, b) as f64 * pd, fourier::freq(y, b) as f64 * pd);
            let q = [r[0][0] * kx + r[1][0] * ky, r[0][1] * kx + r[1][1] * ky, r[0][2] * kx + r[1][2] * ky];
            let (a, c) = p.at(q);
            re[y * b + x] = a;
            im[y * b + x] = c;
        }
    }
    fourier::ift2(&crate::fft::Fft::new(b), b, &re, &im)
}
