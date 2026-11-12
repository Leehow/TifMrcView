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
use crate::spa::fourier::{ft2, ift2, index, resize2, Half};
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

/// What is found to be wrong with a view (or with the whole tomogram) beyond
/// what the alignment and the CTF fit say, shared by all particles.
#[derive(Clone, Copy, Default, Debug)]
pub struct Fix {
    /// Where the particles really sit in the view, relative to where
    /// `Series::place` would put them: pixels of the views (those of
    /// `centre`, columns then rows). Added to `place`'s centre.
    pub shift: [f64; 2],
    /// Defocus beyond what the CTF fit and the particle's place give, Å
    /// (the unit of `place`'s defocus and of `transfer`'s `extra`). Added to
    /// `place`'s defocus. One value for the tomogram, the same in every view.
    pub defocus: f64,
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
    /// Pixels of the views per pixel of a patch as it is used: patches are
    /// cut `fold` times larger and finer and brought down by cropping their
    /// transform. (Views binned all the way by averaging pixels fold the
    /// noise from beyond the new limit back into the band: a quarter more
    /// noise at half of it.)
    pub fold: usize,
    /// Radius of the disc of a patch that is kept, pixels of the views
    /// (0: all of it).
    pub window: f64,
    /// One `Fix` per view, zeros as opened; `views` finds more of them.
    pub fixes: Vec<Fix>,
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
    pub fn open(dataset: &crate::tomo::dataset::Dataset, tomogram: &crate::tomo::dataset::Tomogram, dims: [usize; 3], bin: usize, fold: usize, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Series> {
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
            fold,
            window: 0.0,
            fixes: vec![Fix::default(); kept.len()],
        })
    }

    /// Where the tomogram point `at` (voxels) falls in view `i` (column,
    /// row) and the defocus it has there beyond the view's at the axis, Å,
    /// both with the view's `Fix` added.
    pub fn place(&self, at: [f64; 3], i: usize) -> ([f64; 2], f64) {
        let g = &self.grid;
        let t = self.views[i].theta.to_radians();
        let u = (at[0] - (g.dims[0] as f64 - 1.0) / 2.0) * g.scale;
        let v = (at[2] - (g.dims[2] as f64 - 1.0) / 2.0 + g.z_offset) * g.scale;
        let across = t.cos() * u + t.sin() * v;
        let row = (at[1] - (g.dims[1] as f64 - 1.0) / 2.0) * g.scale + (self.h as f64 - 1.0) / 2.0;
        // As `tomo::ctf::flip`: the strip's defocus, then the depth's.
        let defocus = self.gradient as f64 * self.pixel_a * (t.tan() * across - v / t.cos());
        let fix = self.fixes.get(i).copied().unwrap_or_default();
        ([across + (self.w as f64 - 1.0) / 2.0 + fix.shift[0], row + fix.shift[1]], defocus + fix.defocus)
    }

    /// The b × b patch of view `i` about `centre`, mean 0 and deviation 1,
    /// its edge tapered, and how far the centre lies from pixel b/2 of it
    /// (a fraction of a pixel); None when it leaves the image.
    fn patch_at(&self, i: usize, centre: [f64; 2], b: usize) -> Option<(Vec<f32>, [f64; 2])> {
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
        let off = [centre[0] - whole[0], centre[1] - whole[1]];
        if self.window > 0.0 {
            // Only the disc the particle projects into, softly: what lies
            // beside it (its neighbours, where they stand close) is not part
            // of it, and a reconstruction on a grid lets some of that in.
            let soft = (self.window / 4.0).max(3.0);
            for y in 0..b {
                for x in 0..b {
                    let d = ((x as f64 - (b / 2) as f64 - off[0]).powi(2) + (y as f64 - (b / 2) as f64 - off[1]).powi(2)).sqrt();
                    let w = if d <= self.window { 1.0 } else if d >= self.window + soft { 0.0 } else { 0.5 + 0.5 * (std::f64::consts::PI * (d - self.window) / soft).cos() };
                    out[y * b + x] *= w as f32;
                }
            }
        }
        Some((out, off))
    }

    /// The transform of the patch of `b` pixels (each `fold` of the view's)
    /// about `centre` in view `i`, and how far the centre lies from pixel
    /// b/2 of it, in those pixels.
    fn patch(&self, i: usize, centre: [f64; 2], b: usize, fft: &Fft) -> Option<(Vec<f32>, Vec<f32>, [f64; 2])> {
        let f = self.fold.max(1);
        let (cut, off) = self.patch_at(i, centre, f * b)?;
        let (re, im) = ft2(fft, f * b, &cut);
        let (re, im) = if f == 1 { (re, im) } else { resize2(&re, &im, f * b, b) };
        Some((re, im, [off[0] / f as f64, off[1] / f as f64]))
    }

    /// sin γ at the half plane's frequencies for view `i` with `extra` Å of
    /// defocus (1 everywhere without a CTF).
    fn transfer(&self, i: usize, extra: f64, half: &Half) -> Vec<f32> {
        let Some([df1, df2, angle]) = self.views[i].ctf else { return vec![1.0; half.len()] };
        let o = crate::ctf::Optics::of(self.optics[0], self.optics[1], self.optics[2]);
        let scale = 1.0 / (half.b as f64 * self.pixel_a * self.fold.max(1) as f64);
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
        let Some((re, im, off)) = series.patch(i, centre, b, fft) else { continue };
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

/// The correlation maps are made FINER times finer than the pixels: read
/// between the pixels of a coarse one, the best place is always a pixel.
const FINER: usize = 4;

/// One patch of one particle against the projection of its half reference.
struct Parts {
    /// X · conj(P) per point of the half plane, over the power of the patch
    /// in its shell and times the shell's weight (0 where the shell does not
    /// count): the correlation before the view's transfer is put in.
    xp: Vec<[f32; 2]>,
    /// |P|² with the same factor: what the model's power under a transfer C
    /// is, Σ C²·pp, which the defocus scores divide by.
    pp: Vec<f32>,
    /// The transfer the patch has (at the defocus `place` says, `defocus`).
    c: Vec<f32>,
    /// The defocus beyond the view's at the axis, Å, with the view's fix.
    defocus: f64,
}

/// What `settle` and `views` compare patches with: the half references and
/// the points and shells they compare at.
struct Matcher {
    projectors: [Projector; 2],
    half: Half,
    top: usize,
    fft: Fft,
    wide: usize,
    fft_wide: Fft,
    weight: Vec<f64>,
}

impl Matcher {
    fn new(series: &Series, refs: &[Vec<f32>; 2], b: usize, weight: &[f64]) -> Matcher {
        let pad = pad_for(b);
        let top = weight.iter().rposition(|w| *w > 0.0).unwrap_or(0).min(b / 2 - 2);
        Matcher {
            projectors: [Projector::new(&refs[0], b, pad), Projector::new(&refs[1], b, pad)],
            half: Half::new(b, top as f64),
            top,
            fft: Fft::new(series.fold.max(1) * b),
            wide: FINER * b,
            fft_wide: Fft::new(FINER * b),
            weight: weight.to_vec(),
        }
    }

    /// The patch of view `i` of particle `p`, cut `shift` (pixels of the
    /// view) beyond where `place` puts it, against the model; None when it
    /// leaves the image.
    fn parts(&self, series: &Series, p: &Particle, i: usize, shift: [f64; 2]) -> Option<Parts> {
        let (half, top) = (&self.half, self.top);
        let b = half.b;
        let k = half.len();
        let (centre, defocus) = series.place(p.at, i);
        let (re, im, off) = series.patch(i, [centre[0] + shift[0], centre[1] + shift[1]], b, &self.fft)?;
        let c = series.transfer(i, defocus, half);
        let e = frame(series.views[i].theta);
        let mut slice = vec![0f32; 2 * k];
        self.projectors[p.half].slice(&mul(&e, &p.r), half, &mut slice);
        // Per shell: the power of the patch and of the model.
        let mut power = vec![[0f64; 2]; top + 2];
        for j in 0..k {
            let s = (half.shell[j] as usize).min(top + 1);
            let (r, m) = (re[half.idx[j] as usize] as f64, im[half.idx[j] as usize] as f64);
            power[s][0] += r * r + m * m;
            power[s][1] += ((slice[2 * j] as f64).powi(2) + (slice[2 * j + 1] as f64).powi(2)) * (c[j] as f64).powi(2);
        }
        let mut xp = vec![[0f32; 2]; k];
        let mut pp = vec![0f32; k];
        for j in 0..k {
            let s = (half.shell[j] as usize).min(top + 1);
            let w = self.weight.get(s).copied().unwrap_or(0.0);
            // By the patch's power alone (nearly all noise): the
            // model keeps its own fall, so shells it hardly reaches
            // count for little. Whitened as well, those shells
            // decide, and what a particle finds there is its own
            // noise in the reference, exactly where it already is.
            let norm = power[s][0];
            if w <= 0.0 || norm <= 0.0 || power[s][1] <= 0.0 {
                continue;
            }
            let [kx, ky] = half.k[j];
            let a = 2.0 * std::f64::consts::PI * (kx as f64 * off[0] + ky as f64 * off[1]) / b as f64;
            let (sn, cs) = (a.sin() as f32, a.cos() as f32);
            let (r, m) = (re[half.idx[j] as usize], im[half.idx[j] as usize]);
            let (xr, xi) = (r * cs - m * sn, r * sn + m * cs);
            let (pr, pi) = (slice[2 * j], slice[2 * j + 1]);
            // X · conj(P).
            let f = (w / norm) as f32;
            xp[j] = [(xr * pr + xi * pi) * f, (xi * pr - xr * pi) * f];
            pp[j] = (pr * pr + pi * pi) * f;
        }
        Some(Parts { xp, pp, c, defocus })
    }

    /// The cross-spectrum X · conj(C·P) of a patch with the model, on the
    /// half plane's points: what `place` turns into a map. Linear, so the
    /// spectra of many patches can be added before the one transform.
    fn spectrum(&self, parts: &Parts) -> Vec<[f32; 2]> {
        parts.xp.iter().zip(&parts.c).map(|(x, c)| [x[0] * c, x[1] * c]).collect()
    }

    /// The cross-correlation map over shifts, `wide` × `wide`, FINER times
    /// finer than the patch's pixels, shift 0 at pixel wide/2: the transform
    /// of a (summed) `spectrum` and its mirror at −k.
    fn place(&self, spec: &[[f32; 2]]) -> Vec<f32> {
        let wide = self.wide;
        let (mut cr, mut ci) = (vec![0f32; wide * wide], vec![0f32; wide * wide]);
        for (j, &[kx, ky]) in self.half.k.iter().enumerate() {
            let [vr, vi] = spec[j];
            let at = index(ky as i32, wide) * wide + index(kx as i32, wide);
            let mate = index(-(ky as i32), wide) * wide + index(-(kx as i32), wide);
            cr[at] = vr;
            ci[at] = vi;
            cr[mate] = vr;
            ci[mate] = -vi;
        }
        ift2(&self.fft_wide, wide, &cr, &ci)
    }

    /// The cross-correlation map of one patch (see `place`).
    fn map(&self, parts: &Parts) -> Vec<f32> {
        self.place(&self.spectrum(parts))
    }

    /// Per view, the sum over `particles` (cut at the views' places, `shift`
    /// beyond) of the correlation maps; empty for a view no patch of which
    /// lies in the image. The spectra are summed on the half plane and each
    /// view is transformed once, not each patch.
    fn summed_maps(&self, series: &Series, particles: &[Particle]) -> Vec<Vec<f32>> {
        let nv = series.views.len();
        let k = self.half.len();
        // Per view: the spectrum so far (empty until a patch comes).
        let add = |sum: &mut Vec<[f32; 2]>, spec: &[[f32; 2]]| {
            if sum.is_empty() {
                sum.extend_from_slice(spec);
            } else {
                sum.iter_mut().zip(spec).for_each(|(a, b)| {
                    a[0] += b[0];
                    a[1] += b[1];
                });
            }
        };
        let sums: Vec<Vec<[f32; 2]>> = particles
            .par_iter()
            .fold(
                || vec![Vec::<[f32; 2]>::new(); nv],
                |mut acc, p| {
                    for (i, sum) in acc.iter_mut().enumerate() {
                        if let Some(parts) = self.parts(series, p, i, [0.0, 0.0]) {
                            add(sum, &self.spectrum(&parts));
                        }
                    }
                    acc
                },
            )
            .reduce(
                || vec![Vec::<[f32; 2]>::new(); nv],
                |mut a, b| {
                    for (x, y) in a.iter_mut().zip(&b) {
                        if !y.is_empty() {
                            add(x, y);
                        }
                    }
                    a
                },
            );
        debug_assert!(sums.iter().all(|s| s.is_empty() || s.len() == k));
        sums.par_iter().map(|s| if s.is_empty() { vec![] } else { self.place(s) }).collect()
    }
}

/// Moves each particle to where the tilt images themselves put it.
///
/// The poses come from subtomograms: a tomogram resamples every view, and a
/// particle found in it can sit a fraction of a voxel from where the views
/// have it. Here every patch is compared with the projection of the
/// particle's own half reference (`refs`, b³) under that view's transfer: the
/// cross-correlation of the two is a map over shifts in the view
/// (`Matcher::map`), and a shift t of the particle in the tomogram moves it
/// by (e_u·t, e_v·t) there. The t with the largest sum over the views is
/// taken, searched within `reach` voxels. Shells count by `weight` (0 where
/// the reference is not trusted) over the power of the patch there. Returns
/// the shifts made, in voxels.
pub fn settle(series: &Series, particles: &mut [Particle], refs: &[Vec<f32>; 2], b: usize, weight: &[f64], reach: f64) -> Vec<[f64; 3]> {
    let m = Matcher::new(series, refs, b, weight);
    let wide = m.wide;
    let steps = (2.0 * reach).ceil() as i32;
    let shifts: Vec<[f64; 3]> = particles
        .par_iter()
        .map(|p| {
            let mut maps: Vec<(f64, f64, Vec<f32>)> = vec![];
            for i in 0..series.views.len() {
                let Some(parts) = m.parts(series, p, i, [0.0, 0.0]) else { continue };
                let e = frame(series.views[i].theta);
                maps.push((e[0][0], e[0][2], m.map(&parts)));
            }
            let g = series.grid.scale / series.fold.max(1) as f64 * FINER as f64;
            let read = |map: &[f32], x: f64, y: f64| -> f64 {
                let (x, y) = (x + (wide / 2) as f64, y + (wide / 2) as f64);
                let (fx, fy) = (x.floor(), y.floor());
                if fx < 0.0 || fy < 0.0 || fx as usize + 1 >= wide || fy as usize + 1 >= wide {
                    return 0.0;
                }
                let (dx, dy) = (x - fx, y - fy);
                let j = fy as usize * wide + fx as usize;
                (map[j] as f64 * (1.0 - dx) + map[j + 1] as f64 * dx) * (1.0 - dy) + (map[j + wide] as f64 * (1.0 - dx) + map[j + wide + 1] as f64 * dx) * dy
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

/// A view's shift counts only when the peak of the summed map stands this many
/// standard deviations of the map away from its mean. A high tilt with little
/// dose has no peak to speak of, and the largest of ~10⁴ noise values in the
/// search disc already reaches 4 deviations: 5 keeps those views at 0
/// instead of at a shift made of noise.
const SURE: f64 = 5.0;

/// The defocus offsets tried, Å: ±`DEFOCUS_REACH` in `DEFOCUS_STEPS` equal
/// steps (500 Å). At 300 kV that moves the phase of the transfer by 0.1 to
/// 0.3 radians at 15 to 10 Å, about the last shell the halves trust: enough
/// to tell neighbours apart where there is signal, not more than the
/// parabola through the best three can read between.
const DEFOCUS_REACH: f64 = 5000.0;
const DEFOCUS_STEPS: usize = 21;

/// At most this many particles (evenly spaced) score the defocus offsets:
/// every candidate needs the transfer of every point of every view of every
/// particle, and a few hundred particles hold all the signal there is for
/// the one number.
const DEFOCUS_PARTICLES: usize = 300;

/// The offset curve counts as flat when its best value stands out from its
/// median by less than this many times the scatter of the curve (found from
/// two halves of the particles, see `views`).
const FLAT: f64 = 3.0;

/// Refines what all particles of a tomogram share: for each view its image
/// shift, and for the tomogram one defocus. One particle has too little
/// signal to settle its own place (`settle`), hundreds of them have enough
/// for these few numbers.
///
/// Every patch is compared, as in `settle`, with the projection of the
/// particle's own half reference (`refs`, b³) under the view's transfer, and
/// the cross-correlation maps of one view are summed over all particles. A
/// peak at +d in that map means the particles stand d beyond where `place`
/// puts them (the map is the transform of X · conj(C·P), so a patch whose
/// content is shifted by d correlates best at d), so the fix is +d. The peak
/// is searched within `reach` voxels, read between map pixels by a parabola,
/// and kept only when it is `SURE` deviations above the rest of the map.
///
/// The defocus: with the views at their shifts, the agreement of the patches
/// with the model summed over views and particles is read for each offset
/// in a range, and the best (by a parabola through its neighbours) is taken,
/// unless it lies at the end of the range or the curve is flat.
///
/// The series is not changed; what is returned is the fix to add to
/// `series.fixes` (shift per view, the same defocus in all). Shells count by
/// `weight`, as in `settle`.
pub fn views(series: &Series, particles: &[Particle], refs: &[Vec<f32>; 2], b: usize, weight: &[f64], reach: f64) -> Vec<Fix> {
    let m = Matcher::new(series, refs, b, weight);
    let (mut fixes, sure) = shifts_of(series, particles, &m, reach);
    if let Some(d) = defocus_offset(series, particles, &m, &fixes, &sure) {
        fixes.iter_mut().for_each(|f| f.defocus = d);
    }
    fixes
}

/// `views` without the defocus: only the shift per view (`defocus` 0 in all
/// fixes). The defocus search re-cuts the patches of up to
/// `DEFOCUS_PARTICLES` particles in every view for each candidate and costs
/// a good part of the run, so a caller that does not use it calls this.
pub fn view_shifts(series: &Series, particles: &[Particle], refs: &[Vec<f32>; 2], b: usize, weight: &[f64], reach: f64) -> Vec<Fix> {
    let m = Matcher::new(series, refs, b, weight);
    shifts_of(series, particles, &m, reach).0
}

/// The shift per view and whether it is sure.
fn shifts_of(series: &Series, particles: &[Particle], m: &Matcher, reach: f64) -> (Vec<Fix>, Vec<bool>) {
    let nv = series.views.len();
    let wide = m.wide;
    let sums = m.summed_maps(series, particles);
    // Map pixels per pixel of the view: the map's pixels are 1/FINER of the
    // patch's, and a patch pixel is `fold` pixels of the view.
    let to_view = series.fold.max(1) as f64 / FINER as f64;
    let r = series.grid.scale / series.fold.max(1) as f64 * FINER as f64 * reach;
    let mut fixes = vec![Fix::default(); nv];
    let mut sure = vec![false; nv];
    let mid = (wide / 2) as i64;
    for (i, map) in sums.iter().enumerate() {
        if map.is_empty() {
            continue;
        }
        let reach_px = r.ceil() as i64;
        let mut best = (f64::MIN, [0i64; 2]);
        let (mut n, mut s1, mut s2) = (0usize, 0f64, 0f64);
        for y in 0..wide as i64 {
            for x in 0..wide as i64 {
                let (dx, dy) = (x - mid, y - mid);
                let v = map[y as usize * wide + x as usize] as f64;
                if ((dx * dx + dy * dy) as f64) <= r * r {
                    // The parabola needs both neighbours on the map.
                    if dx.abs() <= reach_px && dy.abs() <= reach_px && x > 0 && y > 0 && x < wide as i64 - 1 && y < wide as i64 - 1 && v > best.0 {
                        best = (v, [dx, dy]);
                    }
                } else {
                    n += 1;
                    s1 += v;
                    s2 += v * v;
                }
            }
        }
        if n < 2 || best.0 == f64::MIN {
            continue;
        }
        let mean = s1 / n as f64;
        let sd = (s2 / n as f64 - mean * mean).max(0.0).sqrt();
        if sd <= 0.0 || (best.0 - mean) / sd < SURE {
            continue;
        }
        let at = |dx: i64, dy: i64| map[((mid + dy) as usize) * wide + (mid + dx) as usize] as f64;
        let peak = |lo: f64, mid_v: f64, hi: f64| -> f64 {
            let d = lo - 2.0 * mid_v + hi;
            if d < 0.0 { (0.5 * (lo - hi) / d).clamp(-0.5, 0.5) } else { 0.0 }
        };
        let [px, py] = best.1;
        let fx = px as f64 + peak(at(px - 1, py), best.0, at(px + 1, py));
        let fy = py as f64 + peak(at(px, py - 1), best.0, at(px, py + 1));
        fixes[i].shift = [fx * to_view, fy * to_view];
        sure[i] = true;
    }
    (fixes, sure)
}

/// The defocus offset (Å) for the whole series, or None when the series has
/// no CTF, no view has a trusted shift, or the curve does not show one.
/// `fixes` holds the shifts found (the views `sure` of are cut there; the
/// others carry too little to count).
fn defocus_offset(series: &Series, particles: &[Particle], m: &Matcher, fixes: &[Fix], sure: &[bool]) -> Option<f64> {
    if series.views.iter().all(|v| v.ctf.is_none()) || !sure.iter().any(|s| *s) || particles.is_empty() {
        return None;
    }
    // The subset, evenly spaced; its odd and even members make two curves,
    // whose difference shows how much of the curve is noise.
    let count = particles.len().min(DEFOCUS_PARTICLES);
    let subset: Vec<(usize, &Particle)> = (0..count).map(|k| (k, &particles[k * particles.len() / count])).collect();
    let step = 2.0 * DEFOCUS_REACH / (DEFOCUS_STEPS - 1) as f64;
    let offsets: Vec<f64> = (0..DEFOCUS_STEPS).map(|c| -DEFOCUS_REACH + step * c as f64).collect();
    // Per half and offset: Σ Re(X·conj P)·C and Σ |P|²·C².
    let zero = [vec![[0f64; 2]; DEFOCUS_STEPS], vec![[0f64; 2]; DEFOCUS_STEPS]];
    let curves = subset
        .par_iter()
        .fold(
            || zero.clone(),
            |mut acc, &(k, p)| {
                for i in (0..series.views.len()).filter(|i| sure[*i]) {
                    let Some(parts) = m.parts(series, p, i, fixes[i].shift) else { continue };
                    for (c, d) in offsets.iter().enumerate() {
                        let t = series.transfer(i, parts.defocus + d, &m.half);
                        let cell = &mut acc[k % 2][c];
                        cell[0] += parts.xp.iter().zip(&t).map(|(x, t)| (x[0] * t) as f64).sum::<f64>();
                        cell[1] += parts.pp.iter().zip(&t).map(|(q, t)| (q * t * t) as f64).sum::<f64>();
                    }
                }
                acc
            },
        )
        .reduce(
            || zero.clone(),
            |mut a, b| {
                for h in 0..2 {
                    a[h].iter_mut().zip(&b[h]).for_each(|(x, y)| {
                        x[0] += y[0];
                        x[1] += y[1];
                    });
                }
                a
            },
        );
    let median = |v: &[f64]| {
        let mut s = v.to_vec();
        s.sort_by(f64::total_cmp);
        s[s.len() / 2]
    };
    // Divided by the size of the model under that transfer, or the offsets
    // that make the transfer larger (not better matched) would win.
    let score = |num: f64, den: f64| if den > 0.0 { num / den.sqrt() } else { 0.0 };
    let curve = |h: usize| -> Vec<f64> { curves[h].iter().map(|c| score(c[0], c[1])).collect() };
    let total: Vec<f64> = (0..DEFOCUS_STEPS).map(|c| score(curves[0][c][0] + curves[1][c][0], curves[0][c][1] + curves[1][c][1])).collect();
    let (ca, cb) = (curve(0), curve(1));
    let (ma, mb) = (median(&ca), median(&cb));
    // Independent halves: the curve of all is about their mean times √2, and
    // its noise is that of the difference of the two over √2 (what the two
    // have in common, the shape, drops out of the difference).
    let scatter = ((0..DEFOCUS_STEPS).map(|c| ((ca[c] - ma) - (cb[c] - mb)).powi(2)).sum::<f64>() / (2 * DEFOCUS_STEPS) as f64).sqrt();
    let top = (0..DEFOCUS_STEPS).max_by(|a, b| total[*a].total_cmp(&total[*b]))?;
    if top == 0 || top + 1 == DEFOCUS_STEPS || total[top] - median(&total) < FLAT * scatter {
        return None;
    }
    let (lo, mid, hi) = (total[top - 1], total[top], total[top + 1]);
    let d = lo - 2.0 * mid + hi;
    let frac = if d < 0.0 { (0.5 * (lo - hi) / d).clamp(-0.5, 0.5) } else { 0.0 };
    Some(offsets[top] + frac * step)
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
        let fft = Fft::new(series.fold.max(1) * self.b);
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
        series_at(particles, thetas, ctf, 1)
    }

    /// The same with views `fine` times finer than the tomogram's voxels.
    fn series_at(particles: &[Particle], thetas: &[f64], ctf: Option<&[f64]>, fine: usize) -> Series {
        let (w, h) = (128 * fine, 128 * fine);
        let px = 8.0 / fine as f64;
        let grid = Grid { dims: [128, 128, 40], scale: fine as f64, z_offset: 2.5 };
        let truth = phantom();
        let c = (N / 2) as f64;
        let mut s = Series {
            stack: vec![0.0; w * h * thetas.len()],
            w,
            h,
            views: thetas.iter().enumerate().map(|(i, t)| View { theta: *t, ctf: ctf.map(|d| [d[i], d[i], 0.0]) }).collect(),
            pixel_a: px,
            gradient: 0,
            optics: [300.0, 2.7, 0.07],
            grid,
            fold: fine,
            window: 0.0,
            fixes: vec![Fix::default(); thetas.len()],
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
                            let (fx, fy) = (crate::spa::fourier::freq(kx, w) as f64 / (w as f64 * px), crate::spa::fourier::freq(ky, h) as f64 / (h as f64 * px));
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
    fn finer_views_are_brought_down_by_cropping_their_transform() {
        let parts = particles(16);
        let thetas: Vec<f64> = (0..21).map(|i| -60.0 + 6.0 * i as f64).collect();
        let defocus: Vec<f64> = (0..21).map(|i| 20000.0 + 1000.0 * i as f64).collect();
        let s = series_at(&parts, &thetas, Some(&defocus), 2);
        let mut sums = Sums::new(N, 13.0);
        sums.add(&s, &parts, &|_, _| {}).unwrap();
        let (map, _, _) = sums.finish(11.0);
        let c = corr(&map, &phantom());
        assert!(c > 0.9, "{c}");
        // And a particle believed elsewhere is found where it is.
        let mut moved = parts.clone();
        moved[3].at = [parts[3].at[0] + 1.3, parts[3].at[1] - 0.8, parts[3].at[2]];
        let truth = phantom();
        let weight: Vec<f64> = (0..N / 2).map(|s| if (1..=6).contains(&s) { 1.0 } else { 0.0 }).collect();
        settle(&s, &mut moved, &[truth.clone(), truth], N, &weight, 2.5);
        let d = [moved[3].at[0] - parts[3].at[0], moved[3].at[1] - parts[3].at[1]];
        assert!(d[0].abs() < 0.3 && d[1].abs() < 0.3, "{d:?}");
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
        assert!(rms < 0.12 && worst < 0.3, "{rms} {worst}");
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

    #[test]
    fn views_are_put_back_where_the_particles_have_them() {
        let parts = particles(16);
        let thetas: Vec<f64> = (0..21).map(|i| -60.0 + 6.0 * i as f64).collect();
        let mut s = series(&parts, &thetas, None);
        // The images of three views sit d beyond where the series says: made
        // by cutting the patches d the other way (fix −d), which is the same.
        let moved = [(2usize, [1.5, -0.75]), (9, [-1.0, 1.25]), (16, [0.5, 1.0])];
        for (i, d) in moved {
            s.fixes[i].shift = [-d[0], -d[1]];
        }
        let truth = phantom();
        let weight: Vec<f64> = (0..N / 2).map(|s| if (1..=6).contains(&s) { 1.0 } else { 0.0 }).collect();
        let found = views(&s, &parts, &[truth.clone(), truth], N, &weight, 3.0);
        assert_eq!(found.len(), thetas.len());
        for (i, f) in found.iter().enumerate() {
            let want = moved.iter().find(|m| m.0 == i).map_or([0.0, 0.0], |m| m.1);
            assert!((f.shift[0] - want[0]).abs() < 0.2 && (f.shift[1] - want[1]).abs() < 0.2, "view {i}: {:?} want {want:?}", f.shift);
        }
    }

    #[test]
    fn the_defocus_of_a_tomogram_is_found_from_its_particles() {
        let parts = particles(16);
        let thetas: Vec<f64> = (0..21).map(|i| -60.0 + 6.0 * i as f64).collect();
        // Close to focus: at several µm a 32-pixel patch is smaller than what
        // the transfer spreads, and the best offset is read a few thousand Å
        // off, which was seen here and not looked into further).
        let defocus: Vec<f64> = (0..21).map(|i| 5000.0 + 250.0 * i as f64).collect();
        let mut s = series(&parts, &thetas, Some(&defocus));
        let truth = phantom();
        let weight: Vec<f64> = (0..N / 2).map(|s| if (1..=12).contains(&s) { 1.0 } else { 0.0 }).collect();
        let step = 2.0 * DEFOCUS_REACH / (DEFOCUS_STEPS - 1) as f64;
        for offset in [0.0, 1500.0, -2000.0] {
            // Declared offset beyond the true defocus: the answer is minus it.
            s.fixes.iter_mut().for_each(|f| f.defocus = offset);
            let found = views(&s, &parts, &[truth.clone(), truth.clone()], N, &weight, 3.0);
            let d = found[0].defocus;
            eprintln!("declared {offset}: found {d}");
            assert!((d + offset).abs() <= step, "{d} for {offset}");
            assert!(found.iter().all(|f| f.defocus == d));
        }
    }

    /// The way `views` summed before: one inverse transform per patch.
    fn summed_per_patch(m: &Matcher, series: &Series, particles: &[Particle]) -> Vec<Vec<f32>> {
        let nv = series.views.len();
        let add = |sum: &mut Vec<f32>, map: &[f32]| {
            if sum.is_empty() {
                sum.extend_from_slice(map);
            } else {
                sum.iter_mut().zip(map).for_each(|(a, b)| *a += *b);
            }
        };
        particles
            .par_iter()
            .fold(
                || vec![Vec::<f32>::new(); nv],
                |mut acc, p| {
                    for (i, sum) in acc.iter_mut().enumerate() {
                        if let Some(parts) = m.parts(series, p, i, [0.0, 0.0]) {
                            add(sum, &m.map(&parts));
                        }
                    }
                    acc
                },
            )
            .reduce(
                || vec![Vec::<f32>::new(); nv],
                |mut a, b| {
                    for (x, y) in a.iter_mut().zip(&b) {
                        add(x, y);
                    }
                    a
                },
            )
    }

    #[test]
    fn one_transform_per_view_gives_the_sum_of_the_maps() {
        let parts = particles(40);
        let thetas: Vec<f64> = (0..21).map(|i| -60.0 + 6.0 * i as f64).collect();
        let defocus: Vec<f64> = (0..21).map(|i| 5000.0 + 250.0 * i as f64).collect();
        let s = series(&parts, &thetas, Some(&defocus));
        let truth = phantom();
        let weight: Vec<f64> = (0..N / 2).map(|s| if (1..=6).contains(&s) { 1.0 } else { 0.0 }).collect();
        let m = Matcher::new(&s, &[truth.clone(), truth], N, &weight);
        let t = std::time::Instant::now();
        let old = summed_per_patch(&m, &s, &parts);
        let t_old = t.elapsed();
        let t = std::time::Instant::now();
        let new = m.summed_maps(&s, &parts);
        let t_new = t.elapsed();
        eprintln!("40 particles x 21 views, box {N}: per-patch maps {t_old:?}, summed spectra {t_new:?}, factor {:.1}", t_old.as_secs_f64() / t_new.as_secs_f64());
        assert_eq!(old.len(), new.len());
        for (i, (o, n)) in old.iter().zip(&new).enumerate() {
            assert_eq!(o.len(), n.len(), "view {i}");
            let peak = o.iter().fold(0f32, |a, v| a.max(v.abs())).max(1e-20);
            let worst = o.iter().zip(n).fold(0f32, |a, (x, y)| a.max((x - y).abs()));
            assert!(worst / peak < 1e-3, "view {i}: {worst} of {peak}");
        }
    }
}
