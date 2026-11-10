//! The average made from the tilt images themselves.
//!
//! A subtomogram is a small tomogram: every tilt image has been resampled
//! into it, phase flipped with one defocus per strip, and the missing wedge
//! is all the weighting it carries. The images hold more than that. Here a
//! particle is its stack of patches, one per view, cut from the aligned
//! tilt images where the particle's centre projects; a patch is a
//! projection of the particle, so by the central slice theorem its
//! transform is a plane through the particle's transform. Each patch goes
//! into the average at that plane with its own contrast transfer C (the
//! view's defocus at the tilt axis, plus what the particle's place across
//! the axis and its depth add at that tilt): the average is
//! Σ C·X / (Σ C² + λ), `spa::volume::Accum`, as in single-particle
//! reconstruction. Nothing is interpolated twice, zeros of one view's
//! transfer are filled by the others where they differ, and the same
//! forward model (reference → plane → C → patch) is what refinement
//! of poses, defocus and tilt geometry differentiates.
//!
//! Geometry (that of `tomo::recon`): after alignment the tilt axis is the
//! image Y axis. For a view at tilt θ a tomogram point at u = x − (W − 1)/2
//! across the axis and depth v = z − (Z − 1)/2 + offset projects to the
//! detector column cos θ·u + sin θ·v + (w − 1)/2 and stays in its row. The
//! view's image axes in tomogram coordinates are therefore
//! e_u = (cos θ, 0, sin θ) and e_v = (0, 1, 0), and with a particle that is
//! the reference turned by R (`Pose`), the patch's transform at (ku, kv) is
//! the reference's at Rᵀ(ku·e_u + kv·e_v): rows 0 and 1 of E_θ·R.

use anyhow::{bail, Result};
use rayon::prelude::*;

use super::check_cancel;
use crate::fft::Fft;
use crate::spa::fourier::{ft2, ift2, index, Half};
use crate::spa::geom::{mul, Mat};
use crate::spa::volume::{pad_for, Accum, Projector, Pt};
use crate::spa::post;

/// One aligned view of the series.
#[derive(Clone, Debug)]
pub struct View {
    /// Tilt, degrees (refined, levelling included).
    pub theta: f64,
    /// Defocus at the tilt axis, Å, and the direction of df1 in the aligned
    /// view, radians; None when the series has no CTF (then C = 1).
    pub ctf: Option<[f64; 3]>,
}

/// The tomogram's grid on the aligned views.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    /// Voxels along X, Y, Z.
    pub dims: [usize; 3],
    /// Size of a voxel in pixels of the views.
    pub scale: f64,
    /// `recon::Params::z_offset`, voxels.
    pub z_offset: f64,
}

/// The aligned (not phase flipped) views of a tilt series.
pub struct Series {
    /// Views one after another, each w × h.
    pub stack: Vec<f32>,
    pub w: usize,
    pub h: usize,
    pub views: Vec<View>,
    /// Pixel size of the views, Å.
    pub pixel_a: f64,
    /// See `tomo::dataset::Ctf::gradient`.
    pub gradient: i8,
    /// (kV, Cs mm, amplitude contrast).
    pub optics: [f64; 3],
    pub grid: Grid,
}

/// A particle: its centre in tomogram voxels, its orientation and its half.
#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub at: [f64; 3],
    pub r: Mat,
    pub half: usize,
}

impl Series {
    /// The series of `dataset` on the grid of its tomogram `tomogram`
    /// (`dims` voxels), views binned by `bin`.
    pub fn open(dataset: &crate::tomo::dataset::Dataset, tomogram: &crate::tomo::dataset::Tomogram, dims: [usize; 3], bin: usize, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Series> {
        let Some(a) = &dataset.alignment else { bail!("倾斜序列还没有对齐") };
        let Some(z_offset) = tomogram.z_offset else { bail!("这张断层图是旧版本重建的，没有记录它在倾斜序列里的位置：请重新重建一次") };
        let Some(px) = dataset.optics.pixel_a else { bail!("不知道像素大小") };
        let img = crate::tomo::pipeline::open(&dataset.stack)?;
        let (stack, w, h, angles) = crate::tomo::output::aligned(&img, a, bin, 0, 0, progress)?;
        let kept: Vec<&crate::tomo::View> = a.views.iter().filter(|v| !v.excluded).collect();
        let views = kept
            .iter()
            .zip(&angles)
            .map(|(v, theta)| {
                let fit = dataset.ctf.as_ref().and_then(|c| c.tilts.iter().find(|f| f.z == v.file_index));
                // As `tomo::output::flip_views`: the fit's angle in the aligned view.
                let psi = 90.0 - (a.axis + v.rotation);
                View { theta: *theta, ctf: fit.map(|f| [f.df1, f.df2, (f.angle - psi).to_radians()]) }
            })
            .collect();
        Ok(Series {
            stack,
            w,
            h,
            views,
            pixel_a: px * bin as f64,
            gradient: dataset.ctf.as_ref().map_or(0, |c| c.gradient),
            optics: [dataset.optics.kv, dataset.optics.cs_mm, dataset.optics.amplitude],
            grid: Grid { dims, scale: tomogram.bin as f64 / bin as f64, z_offset },
        })
    }

    /// Where the tomogram point `at` (voxels) falls in view `i` (column,
    /// row) and the defocus it has there beyond the view's at the axis, Å.
    pub fn place(&self, at: [f64; 3], i: usize) -> ([f64; 2], f64) {
        let g = &self.grid;
        let t = self.views[i].theta.to_radians();
        let u = (at[0] - (g.dims[0] as f64 - 1.0) / 2.0) * g.scale;
        let v = (at[2] - (g.dims[2] as f64 - 1.0) / 2.0 + g.z_offset) * g.scale;
        let across = t.cos() * u + t.sin() * v;
        let row = (at[1] - (g.dims[1] as f64 - 1.0) / 2.0) * g.scale + (self.h as f64 - 1.0) / 2.0;
        // As `tomo::ctf::flip`: the strip's defocus, then the depth's.
        let defocus = self.gradient as f64 * self.pixel_a * (t.tan() * across - v / t.cos());
        ([across + (self.w as f64 - 1.0) / 2.0, row], defocus)
    }

    /// The b × b patch of view `i` about `centre`, mean 0 and deviation 1,
    /// its edge tapered, and how far the centre lies from pixel b/2 of it
    /// (a fraction of a pixel); None when it leaves the image.
    fn patch(&self, i: usize, centre: [f64; 2], b: usize) -> Option<(Vec<f32>, [f64; 2])> {
        let whole = [centre[0].round(), centre[1].round()];
        let (x0, y0) = (whole[0] as isize - (b / 2) as isize, whole[1] as isize - (b / 2) as isize);
        if x0 < 0 || y0 < 0 || x0 as usize + b > self.w || y0 as usize + b > self.h {
            return None;
        }
        let plane = &self.stack[i * self.w * self.h..][..self.w * self.h];
        let mut out = Vec::with_capacity(b * b);
        for y in 0..b {
            let row = (y0 as usize + y) * self.w + x0 as usize;
            out.extend_from_slice(&plane[row..row + b]);
        }
        let mean = out.iter().map(|v| *v as f64).sum::<f64>() / out.len() as f64;
        let sd = (out.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / out.len() as f64).sqrt().max(1e-12);
        let edge = (b as f64 / 16.0).max(2.0);
        let taper = |i: usize| -> f64 {
            let d = (i.min(b - 1 - i)) as f64;
            if d >= edge { 1.0 } else { 0.5 - 0.5 * (std::f64::consts::PI * (d + 0.5) / edge).cos() }
        };
        for y in 0..b {
            for x in 0..b {
                let v = &mut out[y * b + x];
                *v = (((*v as f64 - mean) / sd) * taper(x) * taper(y)) as f32;
            }
        }
        Some((out, [centre[0] - whole[0], centre[1] - whole[1]]))
    }

    /// sin γ at the half plane's frequencies for view `i` with `extra` Å of
    /// defocus (1 everywhere without a CTF).
    fn transfer(&self, i: usize, extra: f64, half: &Half) -> Vec<f32> {
        let Some([df1, df2, angle]) = self.views[i].ctf else { return vec![1.0; half.len()] };
        let o = crate::ctf::Optics::of(self.optics[0], self.optics[1], self.optics[2]);
        let scale = 1.0 / (half.b as f64 * self.pixel_a);
        let (mean, astig) = (0.5 * (df1 + df2) + extra, 0.5 * (df1 - df2));
        half.k
            .iter()
            .map(|&[kx, ky]| {
                let (kx, ky) = (kx as f64 * scale, ky as f64 * scale);
                let df = mean + astig * (2.0 * (ky.atan2(kx) - angle)).cos();
                o.gamma(df, kx * kx + ky * ky, 0.0).sin() as f32
            })
            .collect()
    }
}

/// Rows e_u, e_v and the beam of a view at tilt θ (degrees).
pub fn frame(theta: f64) -> Mat {
    let (s, c) = theta.to_radians().sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// What one particle adds to an average: for every view it is in, the
/// points of its plane with C·X and C².
pub fn points(series: &Series, p: &Particle, half: &Half, fft: &Fft, acc: &Accum, out: &mut Vec<Pt>) {
    let b = half.b;
    let k = half.len();
    let (mut x, mut c2) = (vec![0f32; 2 * k], vec![0f32; k]);
    for i in 0..series.views.len() {
        let (centre, defocus) = series.place(p.at, i);
        let Some((patch, off)) = series.patch(i, centre, b) else { continue };
        let (re, im) = ft2(fft, b, &patch);
        let c = series.transfer(i, defocus, half);
        for j in 0..k {
            let [kx, ky] = half.k[j];
            // The particle's centre lies `off` beyond pixel b/2: moving it
            // there multiplies the transform by e^{2πi k·off/b}.
            let a = 2.0 * std::f64::consts::PI * (kx as f64 * off[0] + ky as f64 * off[1]) / b as f64;
            let (sn, cs) = (a.sin() as f32, a.cos() as f32);
            let (r, m) = (re[half.idx[j] as usize], im[half.idx[j] as usize]);
            x[2 * j] = (r * cs - m * sn) * c[j];
            x[2 * j + 1] = (r * sn + m * cs) * c[j];
            c2[j] = c[j] * c[j];
        }
        let r = mul(&frame(series.views[i].theta), &p.r);
        acc.points(&r, &[super::IDENTITY], half, &x, &c2, out);
    }
}

/// Moves each particle to where the tilt images themselves put it.
///
/// The poses come from subtomograms: a tomogram resamples every view, and a
/// particle found in it can sit a fraction of a voxel from where the views
/// have it. Here every patch is compared with the projection of the
/// particle's own half reference (`refs`, b³) under that view's transfer:
/// the cross-correlation of the two is a map over shifts in the view, and a
/// shift t of the particle in the tomogram moves it by (e_u·t, e_v·t) there.
/// The t with the largest sum over the views is taken, searched within
/// `reach` voxels. Shells count by `weight` (0 where the reference is not
/// trusted) after whitening both sides, so that fine detail decides and
/// not the strong low frequencies. Returns the shifts made, in voxels.
pub fn settle(series: &Series, particles: &mut [Particle], refs: &[Vec<f32>; 2], b: usize, weight: &[f64], reach: f64) -> Vec<[f64; 3]> {
    let pad = pad_for(b);
    let projectors = [Projector::new(&refs[0], b, pad), Projector::new(&refs[1], b, pad)];
    let top = weight.iter().rposition(|w| *w > 0.0).unwrap_or(0).min(b / 2 - 2);
    let half = Half::new(b, top as f64);
    let fft = Fft::new(b);
    let k = half.len();
    let steps = (2.0 * reach).ceil() as i32;
    let shifts: Vec<[f64; 3]> = particles
        .par_iter()
        .map(|p| {
            let mut maps: Vec<(f64, f64, Vec<f32>)> = vec![];
            let mut slice = vec![0f32; 2 * k];
            for i in 0..series.views.len() {
                let (centre, defocus) = series.place(p.at, i);
                let Some((patch, off)) = series.patch(i, centre, b) else { continue };
                let (re, im) = ft2(&fft, b, &patch);
                let c = series.transfer(i, defocus, &half);
                let e = frame(series.views[i].theta);
                projectors[p.half].slice(&mul(&e, &p.r), &half, &mut slice);
                // Per shell: the power of the patch and of the model.
                let mut power = vec![[0f64; 2]; top + 2];
                for j in 0..k {
                    let s = (half.shell[j] as usize).min(top + 1);
                    let (r, m) = (re[half.idx[j] as usize] as f64, im[half.idx[j] as usize] as f64);
                    power[s][0] += r * r + m * m;
                    power[s][1] += ((slice[2 * j] as f64).powi(2) + (slice[2 * j + 1] as f64).powi(2)) * (c[j] as f64).powi(2);
                }
                let (mut cr, mut ci) = (vec![0f32; b * b], vec![0f32; b * b]);
                for j in 0..k {
                    let s = (half.shell[j] as usize).min(top + 1);
                    let w = weight.get(s).copied().unwrap_or(0.0);
                    let norm = (power[s][0] * power[s][1]).sqrt();
                    if w <= 0.0 || norm <= 0.0 {
                        continue;
                    }
                    let [kx, ky] = half.k[j];
                    let a = 2.0 * std::f64::consts::PI * (kx as f64 * off[0] + ky as f64 * off[1]) / b as f64;
                    let (sn, cs) = (a.sin() as f32, a.cos() as f32);
                    let (r, m) = (re[half.idx[j] as usize], im[half.idx[j] as usize]);
                    let (xr, xi) = (r * cs - m * sn, r * sn + m * cs);
                    let (pr, pi) = (slice[2 * j] * c[j], slice[2 * j + 1] * c[j]);
                    // X · conj(C·P), and its mirror at −k.
                    let f = (w / norm) as f32;
                    let (vr, vi) = ((xr * pr + xi * pi) * f, (xi * pr - xr * pi) * f);
                    let at = half.idx[j] as usize;
                    let mate = index(-(ky as i32), b) * b + index(-(kx as i32), b);
                    cr[at] = vr;
                    ci[at] = vi;
                    cr[mate] = vr;
                    ci[mate] = -vi;
                }
                maps.push((e[0][0], e[0][2], ift2(&fft, b, &cr, &ci)));
            }
            let g = series.grid.scale;
            let read = |map: &[f32], x: f64, y: f64| -> f64 {
                let (x, y) = (x + (b / 2) as f64, y + (b / 2) as f64);
                let (fx, fy) = (x.floor(), y.floor());
                if fx < 0.0 || fy < 0.0 || fx as usize + 1 >= b || fy as usize + 1 >= b {
                    return 0.0;
                }
                let (dx, dy) = (x - fx, y - fy);
                let j = fy as usize * b + fx as usize;
                (map[j] as f64 * (1.0 - dx) + map[j + 1] as f64 * dx) * (1.0 - dy) + (map[j + b] as f64 * (1.0 - dx) + map[j + b + 1] as f64 * dx) * dy
            };
            let score = |t: [f64; 3]| -> f64 { maps.iter().map(|(c, s, map)| read(map, g * (c * t[0] + s * t[2]), g * t[1])).sum() };
            // Half a voxel apart over the reach, then an eighth about the best.
            let mut best = ([0.0; 3], score([0.0; 3]));
            for (step, count) in [(0.5, steps), (0.125, 4)] {
                let about = best.0;
                for z in -count..=count {
                    for y in -count..=count {
                        for x in -count..=count {
                            let t = [about[0] + step * x as f64, about[1] + step * y as f64, about[2] + step * z as f64];
                            let v = score(t);
                            if v > best.1 {
                                best = (t, v);
                            }
                        }
                    }
                }
            }
            best.0
        })
        .collect();
    for (p, t) in particles.iter_mut().zip(&shifts) {
        for a in 0..3 {
            p.at[a] += t[a];
        }
    }
    shifts
}

/// The sums of two half averages on a box of `b` pixels.
pub struct Sums {
    pub b: usize,
    pub half: Half,
    pub halves: [Accum; 2],
    pub count: usize,
}

impl Sums {
    /// `r_max`: the highest frequency taken from the patches, Fourier pixels.
    pub fn new(b: usize, r_max: f64) -> Sums {
        assert!(b.is_power_of_two());
        let pad = pad_for(b);
        Sums { b, half: Half::new(b, r_max), halves: [Accum::new(b, pad), Accum::new(b, pad)], count: 0 }
    }

    /// Adds the particles of one series.
    pub fn add(&mut self, series: &Series, particles: &[Particle], tick: &(dyn Fn(usize, usize) + Sync)) -> Result<()> {
        let fft = Fft::new(self.b);
        let mut done = 0;
        for chunk in particles.chunks(32) {
            check_cancel()?;
            let half = &self.half;
            let acc = &self.halves[0];
            let pts: Vec<(usize, Vec<Pt>)> = chunk
                .par_iter()
                .map(|p| {
                    let mut out = vec![];
                    points(series, p, half, &fft, acc, &mut out);
                    (p.half, out)
                })
                .collect();
            for h in 0..2 {
                let all: Vec<Pt> = pts.iter().filter(|(half, _)| *half == h).flat_map(|(_, v)| v.iter().copied()).collect();
                if !all.is_empty() {
                    self.halves[h].insert(&all);
                }
            }
            self.count += chunk.len();
            done += chunk.len();
            tick(done, particles.len());
        }
        Ok(())
    }

    /// The half maps, their FSC inside `radius` voxels, and the average of
    /// all particles weighted by it.
    pub fn finish(mut self, radius: f64) -> (Vec<f32>, [Vec<f32>; 2], Vec<f64>) {
        let halves = [self.halves[0].reconstruct(None), self.halves[1].reconstruct(None)];
        let fsc = post::fsc(&halves[0], &halves[1], self.b, radius);
        let [mut all, other] = self.halves;
        all.add(&other);
        self.halves = [Accum::new(2, 1), Accum::new(2, 1)];
        // The FSC of two halves, as that of the whole: 2·FSC / (1 + FSC).
        let whole: Vec<f64> = fsc.iter().map(|f| (2.0 * f.max(0.0) / (1.0 + f.max(0.0))).min(1.0)).collect();
        (all.reconstruct(Some(&whole)), halves, fsc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spa::geom::axis_angle;
    use crate::spa::Rng;

    const N: usize = 32;

    /// Three balls with no symmetry, about the centre N/2.
    fn phantom() -> Vec<f32> {
        let c = (N / 2) as f64;
        (0..N * N * N)
            .map(|i| {
                let p = [(i % N) as f64 - c, (i / N % N) as f64 - c, (i / N / N) as f64 - c];
                let ball = |q: [f64; 3], s: f64| (-(0..3).map(|a| (p[a] - q[a]).powi(2)).sum::<f64>() / (2.0 * s * s)).exp();
                (ball([4.0, 0.0, 1.0], 2.5) + 0.8 * ball([-3.0, 4.0, 0.0], 2.0) + 0.6 * ball([0.0, -3.0, -5.0], 2.0)) as f32
            })
            .collect()
    }

    fn corr(a: &[f32], b: &[f32]) -> f64 {
        let c = (N / 2) as f64;
        let inside: Vec<usize> = (0..a.len())
            .filter(|i| {
                let p = [(i % N) as f64 - c, (i / N % N) as f64 - c, (i / N / N) as f64 - c];
                p.iter().map(|v| v * v).sum::<f64>().sqrt() < 11.0
            })
            .collect();
        let mean = |v: &[f32]| inside.iter().map(|i| v[*i] as f64).sum::<f64>() / inside.len() as f64;
        let (ma, mb) = (mean(a), mean(b));
        let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
        for &i in &inside {
            let (x, y) = (a[i] as f64 - ma, b[i] as f64 - mb);
            ab += x * y;
            aa += x * x;
            bb += y * y;
        }
        ab / (aa * bb).sqrt()
    }

    /// Views made in real space with the geometry of `tomo::recon` alone:
    /// every voxel of every copy is dropped onto the detector where the
    /// reconstruction says it projects. `ctf`: (defocus at the axis per
    /// view, Å) applied to the whole image.
    fn series(particles: &[Particle], thetas: &[f64], ctf: Option<&[f64]>) -> Series {
        let (w, h) = (128usize, 128usize);
        let grid = Grid { dims: [128, 128, 40], scale: 1.0, z_offset: 2.5 };
        let truth = phantom();
        let c = (N / 2) as f64;
        let mut s = Series {
            stack: vec![0.0; w * h * thetas.len()],
            w,
            h,
            views: thetas.iter().enumerate().map(|(i, t)| View { theta: *t, ctf: ctf.map(|d| [d[i], d[i], 0.0]) }).collect(),
            pixel_a: 8.0,
            gradient: 0,
            optics: [300.0, 2.7, 0.07],
            grid,
        };
        let planes: Vec<Vec<f32>> = (0..thetas.len())
            .into_par_iter()
            .map(|i| {
                let mut plane = vec![0f32; w * h];
                for p in particles {
                    for (j, v) in truth.iter().enumerate() {
                        if *v < 1e-3 {
                            continue;
                        }
                        // A reference point q appears at `at` + R·q.
                        let q = [(j % N) as f64 - c, (j / N % N) as f64 - c, (j / N / N) as f64 - c];
                        let at: [f64; 3] = std::array::from_fn(|a| p.at[a] + p.r[a][0] * q[0] + p.r[a][1] * q[1] + p.r[a][2] * q[2]);
                        let ([x, y], _) = s.place(at, i);
                        let (fx, fy) = (x.floor(), y.floor());
                        let (dx, dy) = ((x - fx) as f32, (y - fy) as f32);
                        let (x0, y0) = (fx as isize, fy as isize);
                        for (yy, wy) in [(y0, 1.0 - dy), (y0 + 1, dy)] {
                            for (xx, wx) in [(x0, 1.0 - dx), (x0 + 1, dx)] {
                                if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                                    plane[yy as usize * w + xx as usize] += v * wx * wy;
                                }
                            }
                        }
                    }
                }
                if let Some(d) = ctf {
                    let fft = Fft::new(w);
                    let (mut re, mut im) = ft2(&fft, w, &plane);
                    let o = crate::ctf::Optics::of(300.0, 2.7, 0.07);
                    for ky in 0..h {
                        for kx in 0..w {
                            let (fx, fy) = (crate::spa::fourier::freq(kx, w) as f64 / (w as f64 * 8.0), crate::spa::fourier::freq(ky, h) as f64 / (h as f64 * 8.0));
                            let t = o.gamma(d[i], fx * fx + fy * fy, 0.0).sin() as f32;
                            re[ky * w + kx] *= t;
                            im[ky * w + kx] *= t;
                        }
                    }
                    plane = crate::spa::fourier::ift2(&fft, w, &re, &im);
                }
                plane
            })
            .collect();
        for (i, p) in planes.into_iter().enumerate() {
            s.stack[i * w * h..][..w * h].copy_from_slice(&p);
        }
        s
    }

    fn particles(count: usize) -> Vec<Particle> {
        let mut rng = Rng(3);
        // On a lattice with room between copies, off the voxel grid.
        (0..count)
            .map(|i| {
                let at = [24.3 + 27.0 * (i % 4) as f64 + rng.unit(), 22.6 + 27.0 * (i / 4 % 4) as f64 + rng.unit(), 17.0 + 4.0 * rng.unit()];
                Particle { at, r: axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 360.0 * rng.unit()), half: i % 2 }
            })
            .collect()
    }

    #[test]
    fn the_tilt_images_give_back_the_particle() {
        let parts = particles(16);
        let thetas: Vec<f64> = (0..21).map(|i| -60.0 + 6.0 * i as f64).collect();
        let s = series(&parts, &thetas, None);
        let mut sums = Sums::new(N, 13.0);
        sums.add(&s, &parts, &|_, _| {}).unwrap();
        let (map, halves, fsc) = sums.finish(11.0);
        let truth = phantom();
        let (c, a, b) = (corr(&map, &truth), corr(&halves[0], &truth), corr(&halves[1], &truth));
        assert!(c > 0.93 && a > 0.88 && b > 0.88, "{c} {a} {b}");
        assert!(fsc[2..8].iter().all(|f| *f > 0.9), "{fsc:?}");
        // Poses of another particle do not: the planes land in the wrong places.
        let mut wrong = parts.clone();
        wrong.rotate_left(1);
        for (w, p) in wrong.iter_mut().zip(&parts) {
            w.at = p.at;
        }
        let mut sums = Sums::new(N, 13.0);
        sums.add(&s, &wrong, &|_, _| {}).unwrap();
        let (map, _, _) = sums.finish(11.0);
        assert!(corr(&map, &truth) < 0.7, "{}", corr(&map, &truth));
    }

    #[test]
    fn particles_settle_where_the_views_have_them() {
        let parts = particles(16);
        let thetas: Vec<f64> = (0..21).map(|i| -60.0 + 6.0 * i as f64).collect();
        let s = series(&parts, &thetas, None);
        // Each one is believed to be up to a voxel and a half away.
        let mut rng = Rng(11);
        let mut moved = parts.clone();
        for p in moved.iter_mut() {
            for a in 0..3 {
                p.at[a] += 3.0 * rng.unit() - 1.5;
            }
        }
        let truth = phantom();
        let weight: Vec<f64> = (0..N / 2).map(|s| if (1..=6).contains(&s) { 1.0 } else { 0.0 }).collect();
        let shifts = settle(&s, &mut moved, &[truth.clone(), truth], N, &weight, 2.5);
        assert_eq!(shifts.len(), parts.len());
        // From up to a voxel and a half to a fraction of one (the copies
        // stand close enough here to show in each other's patches).
        let off: Vec<f64> = moved.iter().zip(&parts).flat_map(|(m, p)| (0..3).map(move |a| m.at[a] - p.at[a])).collect();
        let rms = (off.iter().map(|d| d * d).sum::<f64>() / off.len() as f64).sqrt();
        let worst = off.iter().fold(0.0f64, |w, d| w.max(d.abs()));
        assert!(rms < 0.25 && worst < 0.5, "{rms} {worst}");
    }

    #[test]
    fn each_view_is_weighted_by_its_own_transfer() {
        let parts = particles(16);
        let thetas: Vec<f64> = (0..21).map(|i| -60.0 + 6.0 * i as f64).collect();
        // First zeros between 20 and 28 Å, inside the band (Nyquist 16 Å).
        let defocus: Vec<f64> = (0..21).map(|i| 20000.0 + 1000.0 * i as f64).collect();
        let s = series(&parts, &thetas, Some(&defocus));
        let truth = phantom();
        let run = |s: &Series| {
            let mut sums = Sums::new(N, 13.0);
            sums.add(s, &parts, &|_, _| {}).unwrap();
            corr(&sums.finish(11.0).0, &truth)
        };
        let with = run(&s);
        let mut blind = Series { stack: s.stack.clone(), views: s.views.iter().map(|v| View { ctf: None, ..v.clone() }).collect(), ..series(&[], &thetas, None) };
        blind.grid = s.grid;
        let without = run(&blind);
        assert!(with > 0.85 && with > without + 0.1, "with {with} without {without}");
    }
}
