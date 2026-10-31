//! Drift and motion correction (Pro): finds how far each slice of a stack
//! moved and writes the aligned stack or its sum.
//!
//! Each slice is binned to a working size of at most `n` × `n`, its mean
//! removed and its edges tapered, and Fourier transformed. Only the inner
//! half of the frequencies is kept, under a Gaussian low-pass and a weak
//! high-pass that ignores slow gradients. Shifts come from the peak of the
//! cross-correlation, located on the coarse grid of an inverse FFT and then
//! refined by evaluating the correlation directly at finer offsets (a small
//! inverse DFT, two matrix products), with a parabola through the best point.
//!
//! Two ways to align:
//! - "movie": frames of a dose-fractionated movie. Every frame is aligned
//!   to the sum of all the others, as they are aligned so far, repeated
//!   until the shifts settle. Leaving the frame out of its reference keeps
//!   its own noise from pulling the peak to zero.
//! - "series": tilt series, FIB-SEM and serial-section stacks, where only
//!   neighbours look alike. Each slice is aligned to the one before, and
//!   the shifts are summed along the stack.
//!
//! Shifts are reported in pixels of the full image, relative to the middle
//! slice: slice i shows at x what the reference shows at x − d_i, so the
//! aligned slice is a_i(x) = s_i(x + d_i).

use std::f64::consts::PI;
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::fft::Fft;
use crate::image::OpenImage;

/// Set to stop a running alignment or save; cleared when one starts.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// Most slices whose spectra are held at once in "series" mode.
const CHUNK: usize = 64;
/// Shift change (binned pixels) below which "movie" iterations stop.
const SETTLED: f64 = 0.01;
const MAX_ITER: usize = 12;
/// Series steps with a peak below this fraction of the chunk's median are not trusted.
const LOW_STEP: f64 = 0.5;

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AlignReq {
    pub token: u64,
    /// Slices to align, in order; empty means all.
    #[serde(default)]
    pub zs: Vec<usize>,
    pub c: usize,
    pub t: usize,
    /// Region used to measure the shifts ([x, y, w, h]); None is the whole image.
    pub region: Option<[f64; 4]>,
    /// "movie" or "series".
    pub mode: String,
    /// Working size (power of two) the region is binned down to.
    pub n: usize,
    /// Largest shift searched, in full-image pixels; 0 is a quarter of the region.
    #[serde(default)]
    pub max_shift: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Aligned {
    pub zs: Vec<usize>,
    /// [dx, dy] per slice, full-image pixels, relative to the middle slice.
    pub shifts: Vec<[f64; 2]>,
    /// Normalised correlation at each slice's peak (0–1).
    pub cc: Vec<f64>,
    pub bin: usize,
    pub n: usize,
    pub iterations: usize,
    pub ms: f64,
}

/// Kept frequencies of one binned slice: m × m, row = ky + m/2, column = kx + m/2.
#[derive(Clone)]
struct Spec {
    re: Vec<f32>,
    im: Vec<f32>,
    /// Σ|F|², for normalising correlations.
    energy: f64,
}

/// Binning and geometry of the working images.
#[derive(Clone, Copy)]
struct Geom {
    /// Top-left corner of the square that is binned, full-image pixels.
    x0: usize,
    y0: usize,
    bin: usize,
    /// Working size and kept frequencies (m = n/2).
    n: usize,
    m: usize,
    /// Image size, to keep moved squares inside it.
    w: usize,
    h: usize,
}

impl Geom {
    fn new(region: [usize; 4], want_n: usize, img_w: usize, img_h: usize) -> Result<Geom> {
        let [x, y, w, h] = region;
        let side = w.min(h);
        if side < 80 {
            bail!("区域太小（至少 80 × 80 像素）");
        }
        // Leave a margin of 1/16 on each side, so the second pass can move
        // the square by the shift it found.
        let avail = side - side / 8;
        let want = want_n.clamp(64, 4096).next_power_of_two();
        let n = want.min(1 << avail.ilog2());
        let bin = avail / n;
        let used = n * bin;
        Ok(Geom { x0: x + (w - used) / 2, y0: y + (h - used) / 2, bin, n, m: n / 2, w: img_w, h: img_h })
    }

    /// The whole-pixel move nearest to a shift (binned pixels) that keeps
    /// the square inside the image.
    fn offset(&self, d: [f64; 2]) -> [isize; 2] {
        let used = (self.n * self.bin) as isize;
        let clamp = |v: f64, start: usize, size: usize| ((v * self.bin as f64).round() as isize).clamp(-(start as isize), size as isize - start as isize - used);
        [clamp(d[0], self.x0, self.w), clamp(d[1], self.y0, self.h)]
    }
}

/// Bins, tapers and transforms one slice; keeps the inner frequencies, filtered.
/// `off` moves the binned square (full-image pixels; the caller keeps it inside).
fn spectrum(g: &Geom, fft: &Fft, filter: &[f32], off: [isize; 2], values: impl Fn(usize, usize) -> f32 + Sync) -> Spec {
    let (n, b) = (g.n, g.bin);
    let (x0, y0) = ((g.x0 as isize + off[0]) as usize, (g.y0 as isize + off[1]) as usize);
    let mut re: Vec<f32> = (0..n * n)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % n, i / n);
            let mut s = 0f64;
            for dy in 0..b {
                for dx in 0..b {
                    s += values(x0 + x * b + dx, y0 + y * b + dy) as f64;
                }
            }
            (s / (b * b) as f64) as f32
        })
        .collect();
    let mean = re.iter().map(|&v| v as f64).sum::<f64>() / (n * n) as f64;
    // Cosine taper over the outer 1/16 on each side, after removing the mean.
    let edge = (n / 16).max(2);
    let taper: Vec<f32> = (0..n)
        .map(|i| {
            let d = i.min(n - 1 - i);
            if d >= edge { 1.0 } else { (0.5 - 0.5 * (PI * (d as f64 + 0.5) / edge as f64).cos()) as f32 }
        })
        .collect();
    re.par_chunks_mut(n).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            *v = (*v - mean as f32) * taper[x] * taper[y];
        }
    });
    let mut im = vec![0f32; n * n];
    fft2(fft, &mut re, &mut im, n);
    // Keep |kx|, |ky| < m/2, centred.
    let m = g.m;
    let mut kr = vec![0f32; m * m];
    let mut ki = vec![0f32; m * m];
    let mut energy = 0f64;
    for r in 0..m {
        let ky = r as isize - (m / 2) as isize;
        let sy = ky.rem_euclid(n as isize) as usize;
        for c in 0..m {
            let kx = c as isize - (m / 2) as isize;
            let sx = kx.rem_euclid(n as isize) as usize;
            let f = filter[r * m + c];
            let (a, b) = (re[sy * n + sx] * f, im[sy * n + sx] * f);
            kr[r * m + c] = a;
            ki[r * m + c] = b;
            energy += (a as f64).powi(2) + (b as f64).powi(2);
        }
    }
    Spec { re: kr, im: ki, energy }
}

/// 2D forward FFT in place, row major (row = y).
fn fft2(fft: &Fft, re: &mut [f32], im: &mut [f32], n: usize) {
    re.par_chunks_mut(n).zip(im.par_chunks_mut(n)).for_each(|(r, i)| fft.run(r, i));
    let (mut tr, mut ti) = (transpose(re, n), transpose(im, n));
    tr.par_chunks_mut(n).zip(ti.par_chunks_mut(n)).for_each(|(r, i)| fft.run(r, i));
    re.copy_from_slice(&transpose(&tr, n));
    im.copy_from_slice(&transpose(&ti, n));
}

fn transpose(a: &[f32], n: usize) -> Vec<f32> {
    let mut out = vec![0f32; n * n];
    out.par_chunks_mut(n).enumerate().for_each(|(x, row)| {
        for (y, o) in row.iter_mut().enumerate() {
            *o = a[y * n + x];
        }
    });
    out
}

/// Gaussian low-pass (0.14 at the edge of the kept band) times a high-pass
/// that drops the slowest few frequencies.
fn make_filter(m: usize) -> Vec<f32> {
    let sigma = m as f64 / 4.0;
    (0..m * m)
        .map(|i| {
            let ky = (i / m) as f64 - (m / 2) as f64;
            let kx = (i % m) as f64 - (m / 2) as f64;
            let r2 = kx * kx + ky * ky;
            ((-0.5 * r2 / (sigma * sigma)).exp() * (1.0 - (-0.5 * r2 / 4.0).exp())) as f32
        })
        .collect()
}

/// Phase ramp of a shift d (binned pixels): A(k) = F(k)·e^{+2πi k·d/n}.
fn shifted(s: &Spec, g: &Geom, d: [f64; 2], out_re: &mut [f32], out_im: &mut [f32], sign: f32) {
    let m = g.m;
    let w = 2.0 * PI / g.n as f64;
    let ex: Vec<(f32, f32)> = (0..m).map(|c| ((c as f64 - (m / 2) as f64) * d[0] * w).sin_cos()).map(|(s, c)| (c as f32, s as f32)).collect();
    let ey: Vec<(f32, f32)> = (0..m).map(|r| ((r as f64 - (m / 2) as f64) * d[1] * w).sin_cos()).map(|(s, c)| (c as f32, s as f32)).collect();
    for r in 0..m {
        let (cy, sy) = ey[r];
        for c in 0..m {
            let (cx, sx) = ex[c];
            // e^{i(a+b)} = (cx·cy − sx·sy) + i(sx·cy + cx·sy)
            let (pr, pi) = (cx * cy - sx * sy, sx * cy + cx * sy);
            let i = r * m + c;
            let (a, b) = (s.re[i], s.im[i]);
            out_re[i] += sign * (a * pr - b * pi);
            out_im[i] += sign * (a * pi + b * pr);
        }
    }
}

/// Cross-power F·conj(R) of a slice against its reference.
fn cross(f: &Spec, rr: &[f32], ri: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let n = f.re.len();
    let mut pr = vec![0f32; n];
    let mut pi = vec![0f32; n];
    for i in 0..n {
        let (a, b, c, d) = (f.re[i], f.im[i], rr[i], ri[i]);
        pr[i] = a * c + b * d;
        pi[i] = b * c - a * d;
    }
    (pr, pi)
}

/// Correlation Re Σ P(k)·e^{+2πi k·(x, y)/n} at offsets xs × ys (binned
/// pixels), as [y][x]: first over kx for each row, then over ky.
fn correlate_at(pr: &[f32], pi: &[f32], g: &Geom, xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let m = g.m;
    let w = 2.0 * PI / g.n as f64;
    let phase = |k: usize, d: f64| ((k as f64 - (m / 2) as f64) * d * w).sin_cos();
    let ex: Vec<(f64, f64)> = (0..m).flat_map(|c| xs.iter().map(move |&x| phase(c, x))).map(|(s, c)| (c, s)).collect();
    // T[ky][a] = Σ_kx P[ky][kx]·e^{i kx x_a}
    let nx = xs.len();
    let t: Vec<(f64, f64)> = (0..m)
        .into_par_iter()
        .flat_map_iter(|r| {
            let mut row = vec![(0f64, 0f64); nx];
            for c in 0..m {
                let (a, b) = (pr[r * m + c] as f64, pi[r * m + c] as f64);
                for (k, o) in row.iter_mut().enumerate() {
                    let (cs, sn) = ex[c * nx + k];
                    o.0 += a * cs - b * sn;
                    o.1 += a * sn + b * cs;
                }
            }
            row
        })
        .collect();
    let mut out = vec![0f64; ys.len() * nx];
    for (j, &y) in ys.iter().enumerate() {
        for r in 0..m {
            let (sn, cs) = phase(r, y);
            for k in 0..nx {
                let (a, b) = t[r * nx + k];
                out[j * nx + k] += a * cs - b * sn;
            }
        }
    }
    out
}

/// Best shift on the inverse FFT's grid (every n/m binned pixels), within `max`.
fn coarse_peak(pr: &[f32], pi: &[f32], g: &Geom, fft_m: &Fft, max: f64) -> [f64; 2] {
    let m = g.m;
    // Re Σ P e^{+i…} = Re FFT(conj P), with k moved to index k mod m.
    let mut re = vec![0f32; m * m];
    let mut im = vec![0f32; m * m];
    for r in 0..m {
        let sy = (r + m / 2) % m;
        for c in 0..m {
            let sx = (c + m / 2) % m;
            re[sy * m + sx] = pr[r * m + c];
            im[sy * m + sx] = -pi[r * m + c];
        }
    }
    fft2(fft_m, &mut re, &mut im, m);
    let step = (g.n / m) as f64;
    let mut best = (f32::MIN, [0f64; 2]);
    for y in 0..m {
        let dy = if y >= m / 2 { y as f64 - m as f64 } else { y as f64 } * step;
        for x in 0..m {
            let dx = if x >= m / 2 { x as f64 - m as f64 } else { x as f64 } * step;
            if dx.hypot(dy) <= max && re[y * m + x] > best.0 {
                best = (re[y * m + x], [dx, dy]);
            }
        }
    }
    best.1
}

/// Peak near `start`, to a fraction of a binned pixel: the best of a 5 × 5
/// grid (moving the grid if the best is on its edge), then a parabola in x
/// and in y through it and its neighbours. Returns the shift and the peak.
fn refine_peak(pr: &[f32], pi: &[f32], g: &Geom, start: [f64; 2]) -> ([f64; 2], f64) {
    let mut c = [start[0].round(), start[1].round()];
    for _ in 0..4 {
        let xs: Vec<f64> = (-2..=2).map(|i| c[0] + i as f64).collect();
        let ys: Vec<f64> = (-2..=2).map(|i| c[1] + i as f64).collect();
        let v = correlate_at(pr, pi, g, &xs, &ys);
        let (bi, _) = v.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
        let (bx, by) = (bi % 5, bi / 5);
        if (1..=3).contains(&bx) && (1..=3).contains(&by) {
            let at = |x: usize, y: usize| v[y * 5 + x];
            let para = |a: f64, b: f64, c: f64| {
                let den = a - 2.0 * b + c;
                if den < 0.0 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 }
            };
            let fx = para(at(bx - 1, by), at(bx, by), at(bx + 1, by));
            let fy = para(at(bx, by - 1), at(bx, by), at(bx, by + 1));
            return ([xs[bx] + fx, ys[by] + fy], at(bx, by));
        }
        c = [xs[bx], ys[by]];
    }
    (c, correlate_at(pr, pi, g, &[c[0]], &[c[1]])[0])
}

/// Normalised correlation from a peak value and the two spectra's energies.
fn norm_cc(peak: f64, ea: f64, eb: f64) -> f64 {
    let d = (ea * eb).sqrt();
    if d > 0.0 { (peak / d).clamp(-1.0, 1.0) } else { 0.0 }
}

/// Aligns spectra of a movie: each frame to the sum of the others. Shifts in binned pixels.
fn align_movie(specs: &[Spec], g: &Geom, fft_m: &Fft, max: f64, progress: &(dyn Fn(usize) + Sync)) -> Result<(Vec<[f64; 2]>, Vec<f64>, usize)> {
    let len = g.m * g.m;
    let mut d = vec![[0f64; 2]; specs.len()];
    let mut cc = vec![0f64; specs.len()];
    let mut iterations = 0;
    for it in 0..MAX_ITER {
        if CANCEL.load(Ordering::SeqCst) {
            bail!("已取消");
        }
        iterations = it + 1;
        let (mut sr, mut si) = (vec![0f32; len], vec![0f32; len]);
        for (s, &di) in specs.iter().zip(&d) {
            shifted(s, g, di, &mut sr, &mut si, 1.0);
        }
        let next: Vec<([f64; 2], f64)> = specs
            .par_iter()
            .zip(&d)
            .map(|(s, &di)| {
                // The reference without this frame.
                let (mut rr, mut ri) = (sr.clone(), si.clone());
                shifted(s, g, di, &mut rr, &mut ri, -1.0);
                let er: f64 = rr.iter().zip(&ri).map(|(&a, &b)| (a as f64).powi(2) + (b as f64).powi(2)).sum();
                let (pr, pi) = cross(s, &rr, &ri);
                let start = if it == 0 { coarse_peak(&pr, &pi, g, fft_m, max) } else { di };
                let (p, v) = refine_peak(&pr, &pi, g, start);
                (p, norm_cc(v, s.energy, er))
            })
            .collect();
        let change = next.iter().zip(&d).map(|(a, b)| (a.0[0] - b[0]).hypot(a.0[1] - b[1])).fold(0f64, f64::max);
        for (i, (p, v)) in next.into_iter().enumerate() {
            d[i] = p;
            cc[i] = v;
        }
        progress(it + 1);
        if change < SETTLED {
            break;
        }
    }
    Ok((d, cc, iterations))
}

/// Shift between neighbours a (earlier) and b: where b shows what a shows.
fn pair_shift(a: &Spec, b: &Spec, g: &Geom, fft_m: &Fft, max: f64) -> ([f64; 2], f64) {
    let (pr, pi) = cross(b, &a.re, &a.im);
    let start = coarse_peak(&pr, &pi, g, fft_m, max);
    let (p, v) = refine_peak(&pr, &pi, g, start);
    (p, norm_cc(v, a.energy, b.energy))
}

/// Spectrum of slice i (index into the slices aligned), its square moved by an offset.
type SpecOf<'a> = dyn Fn(usize, [isize; 2]) -> Result<Spec> + Sync + 'a;

/// Shifts (binned pixels) and correlations of `count` slices, with the
/// squares moved by `offs`. Series: each to its neighbour, summed.
fn pass(g: &Geom, count: usize, offs: &[[isize; 2]], series: bool, max: f64, spec_of: &SpecOf, tick: &(dyn Fn() + Sync)) -> Result<(Vec<[f64; 2]>, Vec<f64>, usize)> {
    let fft_m = Fft::new(g.m);
    let get = |i: usize| -> Result<Spec> {
        let s = spec_of(i, offs[i]);
        tick();
        s
    };
    if !series {
        let specs: Vec<Spec> = (0..count).map(get).collect::<Result<_>>()?;
        return align_movie(&specs, g, &fft_m, max, &|_| tick());
    }
    // Neighbours, a chunk of slices at a time.
    let mut d = vec![[0f64; 2]; count];
    let mut cc = vec![1f64; count];
    let mut prev: Option<Spec> = None;
    // The last slice placed by a good step, for bridging weak ones.
    let mut anchor: Option<(usize, Spec)> = None;
    for start in (0..count).step_by(CHUNK) {
        let specs: Vec<Spec> = (start..(start + CHUNK).min(count)).map(get).collect::<Result<_>>()?;
        if start == 0 {
            anchor = Some((0, specs[0].clone()));
        }
        let steps: Vec<(usize, ([f64; 2], f64))> = (0..specs.len())
            .into_par_iter()
            .filter_map(|k| {
                let a = if k == 0 { prev.as_ref()? } else { &specs[k - 1] };
                Some((start + k, pair_shift(a, &specs[k], g, &fft_m, max)))
            })
            .collect();
        // A step whose peak is much weaker than the others' (a slice at high
        // tilt, or damaged) is often a wrong peak tens of pixels away. Then
        // measure from the last slice that was placed by a good step (the
        // anchor) straight to this one, and place the slices between on the
        // line; if that is weak too, keep the slice where its neighbour is
        // and leave its correlation low so the panel flags it.
        let mut good: Vec<f64> = steps.iter().map(|(_, (_, v))| *v).collect();
        good.sort_by(f64::total_cmp);
        let weak = good.get(good.len() / 2).copied().unwrap_or(0.0) * LOW_STEP;
        let moved = |a: usize, b: usize| [(offs[b][0] - offs[a][0]) as f64 / g.bin as f64, (offs[b][1] - offs[a][1]) as f64 / g.bin as f64];
        for (i, (s, v)) in steps {
            let k = i - start;
            let held_before = anchor.as_ref().is_some_and(|(a, _)| *a + 1 < i);
            let mut placed = None;
            if v < weak || held_before {
                if let Some((a, sa)) = anchor.as_ref().filter(|(a, _)| *a + 1 < i) {
                    let (bs, bv) = pair_shift(sa, &specs[k], g, &fft_m, max);
                    if bv >= weak {
                        let m = moved(*a, i);
                        let end = [d[*a][0] + m[0] + bs[0], d[*a][1] + m[1] + bs[1]];
                        for j in *a + 1..i {
                            let f = (j - a) as f64 / (i - a) as f64;
                            d[j] = [d[*a][0] + f * (end[0] - d[*a][0]), d[*a][1] + f * (end[1] - d[*a][1])];
                        }
                        placed = Some((end, bv));
                    }
                }
            }
            let (pos, c) = match placed {
                Some(p) => p,
                None if v < weak => (d[i - 1], v),
                None => {
                    let m = moved(i - 1, i);
                    ([d[i - 1][0] + m[0] + s[0], d[i - 1][1] + m[1] + s[1]], v)
                }
            };
            d[i] = pos;
            cc[i] = c;
            if c >= weak {
                anchor = Some((i, specs[k].clone()));
            }
        }
        prev = specs.into_iter().last();
    }
    if count > 1 {
        cc[0] = cc[1];
    }
    Ok((d, cc, 1))
}

/// Two passes: the second with each square moved by the shift the first
/// found, so the tapered edges no longer pull the peak toward zero shift.
/// Returns full-image pixel shifts relative to the middle slice.
fn measure(g: &Geom, count: usize, series: bool, max: f64, spec_of: &SpecOf, tick: &(dyn Fn() + Sync)) -> Result<(Vec<[f64; 2]>, Vec<f64>, usize)> {
    let zero = vec![[0isize; 2]; count];
    let (d1, _, it1) = pass(g, count, &zero, series, max, spec_of, tick)?;
    let mid = d1[count / 2];
    let offs: Vec<[isize; 2]> = d1.iter().map(|d| g.offset([d[0] - mid[0], d[1] - mid[1]])).collect();
    // What is left is small; search a few binned pixels around it.
    let (d2, cc, it2) = pass(g, count, &offs, series, (max / 4.0).max(8.0), spec_of, tick)?;

    let mut d: Vec<[f64; 2]> = if series {
        d2.iter().map(|v| [v[0] * g.bin as f64 + offs[0][0] as f64, v[1] * g.bin as f64 + offs[0][1] as f64]).collect()
    } else {
        d2.iter().zip(&offs).map(|(v, o)| [v[0] * g.bin as f64 + o[0] as f64, v[1] * g.bin as f64 + o[1] as f64]).collect()
    };
    // Series: relative to the middle slice (the untilted one of a tilt
    // series). Movie: to the mean, the least noisy reference.
    let origin = if series {
        d[count / 2]
    } else {
        let k = count as f64;
        [d.iter().map(|v| v[0]).sum::<f64>() / k, d.iter().map(|v| v[1]).sum::<f64>() / k]
    };
    for v in &mut d {
        *v = [v[0] - origin[0], v[1] - origin[1]];
    }
    Ok((d, cc, it1 + it2))
}

/// Measures the shift of every slice.
pub fn estimate(img: &Arc<OpenImage>, req: &AlignReq, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Aligned> {
    let start = std::time::Instant::now();
    CANCEL.store(false, Ordering::SeqCst);
    let info = img.info();
    let zs: Vec<usize> = if req.zs.is_empty() { (0..info.size_z).collect() } else { req.zs.iter().copied().filter(|&z| z < info.size_z).collect() };
    if zs.len() < 2 {
        bail!("至少需要两层");
    }
    let region = match req.region {
        Some(r) => {
            let x0 = r[0].max(0.0).floor() as usize;
            let y0 = r[1].max(0.0).floor() as usize;
            let x1 = ((r[0] + r[2]).ceil() as usize).min(info.width);
            let y1 = ((r[1] + r[3]).ceil() as usize).min(info.height);
            if x1 <= x0 || y1 <= y0 {
                bail!("区域在图像外");
            }
            [x0, y0, x1 - x0, y1 - y0]
        }
        None => [0, 0, info.width, info.height],
    };
    let g = Geom::new(region, req.n, info.width, info.height)?;
    let fft_n = Fft::new(g.n);
    let filter = make_filter(g.m);
    let max = if req.max_shift > 0.0 { req.max_shift / g.bin as f64 } else { g.n as f64 / 4.0 };
    let series = match req.mode.as_str() {
        "movie" => false,
        "series" => true,
        m => bail!("未知的对齐方式 {m}"),
    };
    let spec_of = |i: usize, off: [isize; 2]| -> Result<Spec> {
        if CANCEL.load(Ordering::SeqCst) {
            bail!("已取消");
        }
        let (page, sample) = info.locate(req.c, zs[i], req.t);
        let planes = img.page(page)?;
        let p = &planes[sample.min(planes.len() - 1)];
        Ok(spectrum(&g, &fft_n, &filter, off, |x, y| p.data.get_f64(y * p.w + x) as f32))
    };
    // Progress: two passes of spectra, plus movie iterations (estimated).
    let total = 2 * zs.len() + if series { 0 } else { 8 };
    let done = std::sync::atomic::AtomicUsize::new(0);
    let tick = || progress((done.fetch_add(1, Ordering::Relaxed) + 1).min(total - 1), total);
    let (shifts, cc, iterations) = measure(&g, zs.len(), series, max, &spec_of, &tick)?;
    progress(total, total);
    Ok(Aligned { zs, shifts, cc, bin: g.bin, n: g.n, iterations, ms: start.elapsed().as_secs_f64() * 1e3 })
}

/// Writes the aligned slices (or their mean) as an MRC file of 32-bit floats.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReq {
    pub token: u64,
    pub zs: Vec<usize>,
    pub shifts: Vec<[f64; 2]>,
    pub c: usize,
    pub t: usize,
    pub path: String,
    /// "stack" or "sum".
    pub what: String,
}

/// Catmull–Rom weights for a fraction t of a pixel.
fn cubic(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [-0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1.0, -1.5 * t3 + 2.0 * t2 + 0.5 * t, 0.5 * t3 - 0.5 * t2]
}

/// out(x, y) = v(x + dx, y + dy), Catmull–Rom; `fill` where that falls off
/// the plane. The shift is the same everywhere, so this is a whole-pixel
/// move plus a 4-tap filter along x and then along y.
fn shift_plane(v: &[f32], w: usize, h: usize, d: [f64; 2], fill: f32) -> Vec<f32> {
    let split = |s: f64| {
        let i = s.floor();
        (i as isize, cubic((s - i) as f32), s > i)
    };
    let ((ix, kx, fx), (iy, ky, fy)) = (split(d[0]), split(d[1]));
    // Where the sample point itself lies on the plane.
    let inside = |c: isize, len: usize, frac: bool| c >= 0 && (c as usize) < len && !(frac && c as usize == len - 1);
    let clamp = |c: isize, len: usize| c.clamp(0, len as isize - 1) as usize;
    let mut a = vec![0f32; w * h];
    a.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &v[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            let c = x as isize + ix;
            *o = if inside(c, w, fx) { (0..4).map(|j| kx[j] * src[clamp(c + j as isize - 1, w)]).sum() } else { f32::NAN };
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let c = y as isize + iy;
        if !inside(c, h, fy) {
            row.fill(fill);
            return;
        }
        let rows: [&[f32]; 4] = std::array::from_fn(|j| {
            let r = clamp(c + j as isize - 1, h);
            &a[r * w..(r + 1) * w]
        });
        for (x, o) in row.iter_mut().enumerate() {
            let s = ky[0] * rows[0][x] + ky[1] * rows[1][x] + ky[2] * rows[2][x] + ky[3] * rows[3][x];
            *o = if s.is_nan() { fill } else { s };
        }
    });
    out
}

pub fn save(img: &Arc<OpenImage>, req: &SaveReq, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<()> {
    CANCEL.store(false, Ordering::SeqCst);
    if req.zs.len() != req.shifts.len() || req.zs.is_empty() {
        bail!("位移和层数对不上");
    }
    let info = img.info();
    let (w, h) = (info.width, info.height);
    let sum = match req.what.as_str() {
        "sum" => true,
        "stack" => false,
        x => bail!("未知的输出 {x}"),
    };
    let px = pixel_a(info);
    let nz = if sum { 1 } else { req.zs.len() };
    let tmp = format!("{}.part", req.path);
    let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    f.write_all(&[0u8; 1024])?;
    let mut acc = if sum { vec![0f64; w * h] } else { Vec::new() };
    let (mut lo, mut hi, mut total, mut total2) = (f64::MAX, f64::MIN, 0f64, 0f64);
    for (i, (&z, d)) in req.zs.iter().zip(&req.shifts).enumerate() {
        if CANCEL.load(Ordering::SeqCst) {
            drop(f);
            let _ = std::fs::remove_file(&tmp);
            bail!("已取消");
        }
        let (page, s) = info.locate(req.c, z, req.t);
        let planes = img.page(page)?;
        let p = &planes[s.min(planes.len() - 1)];
        let v: Vec<f32> = (0..p.w * p.h).into_par_iter().map(|i| p.data.get_f64(i) as f32).collect();
        let mean = (v.par_iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64) as f32;
        let out = shift_plane(&v, p.w, p.h, *d, mean);
        if sum {
            acc.par_iter_mut().zip(&out).for_each(|(a, &v)| *a += v as f64);
        } else {
            let (a, b, c, d) = out
                .par_iter()
                .fold(|| (f64::MAX, f64::MIN, 0f64, 0f64), |(a, b, c, d), &v| {
                    let v = v as f64;
                    (a.min(v), b.max(v), c + v, d + v * v)
                })
                .reduce(|| (f64::MAX, f64::MIN, 0f64, 0f64), |x, y| (x.0.min(y.0), x.1.max(y.1), x.2 + y.2, x.3 + y.3));
            lo = lo.min(a);
            hi = hi.max(b);
            total += c;
            total2 += d;
            let mut bytes = vec![0u8; out.len() * 4];
            bytes.par_chunks_mut(4).zip(&out).for_each(|(b, v)| b.copy_from_slice(&v.to_le_bytes()));
            f.write_all(&bytes)?;
        }
        progress(i + 1, req.zs.len());
    }
    if sum {
        let k = req.zs.len() as f64;
        let bytes: Vec<u8> = acc
            .iter()
            .flat_map(|&a| {
                let v = a / k;
                lo = lo.min(v);
                hi = hi.max(v);
                total += v;
                total2 += v * v;
                (v as f32).to_le_bytes()
            })
            .collect();
        f.write_all(&bytes)?;
    }
    let count = (w * h * nz) as f64;
    let mean = total / count;
    let rms = (total2 / count - mean * mean).max(0.0).sqrt();
    f.seek(SeekFrom::Start(0))?;
    let frames = req.zs.len();
    let label = if sum { format!("TifMrcView: aligned sum of {frames} frames") } else { format!("TifMrcView: aligned stack of {frames} slices") };
    f.write_all(&mrc_header(w, h, nz, px, [lo as f32, hi as f32, mean as f32, rms as f32], &label))?;
    f.flush()?;
    drop(f);
    std::fs::rename(&tmp, &req.path)?;
    Ok(())
}

/// Pixel size in Å, or 1 when the file does not say.
pub(crate) fn pixel_a(info: &crate::image::ImageInfo) -> f64 {
    let unit_a = match info.unit.as_str() {
        "nm" => 10.0,
        "µm" | "um" => 1e4,
        "Å" | "A" | "angstrom" => 1.0,
        _ => 0.0,
    };
    info.pixel_size.map(|p| p[0] * unit_a).filter(|&p| p > 0.0).unwrap_or(1.0)
}

/// MRC2014 header for 32-bit floats (mode 2), little-endian; `stats` is
/// min, max, mean, rms; `label` goes into the first of the ten labels.
pub(crate) fn mrc_header(w: usize, h: usize, nz: usize, pixel_a: f64, stats: [f32; 4], label: &str) -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    let mut put_i = |off: usize, v: i32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    for (k, v) in [w as i32, h as i32, nz as i32, 2, 0, 0, 0, w as i32, h as i32, nz as i32].into_iter().enumerate() {
        put_i(k * 4, v);
    }
    put_i(64, 1);
    put_i(68, 2);
    put_i(72, 3);
    // Image stack (0) for a stack, single image (0) as well; nsymbt 0.
    put_i(88, 0);
    put_i(92, 0);
    put_i(108, 20140);
    put_i(220, 1);
    let mut put_f = |off: usize, v: f32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    put_f(40, (pixel_a * w as f64) as f32);
    put_f(44, (pixel_a * h as f64) as f32);
    put_f(48, (pixel_a * nz as f64) as f32);
    for off in [52, 56, 60] {
        put_f(off, 90.0);
    }
    put_f(76, stats[0]);
    put_f(80, stats[1]);
    put_f(84, stats[2]);
    put_f(216, stats[3]);
    b[208..212].copy_from_slice(b"MAP ");
    b[212..216].copy_from_slice(&[0x44, 0x44, 0, 0]);
    let l = label.as_bytes();
    b[224..224 + l.len().min(80)].copy_from_slice(&l[..l.len().min(80)]);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix(mut z: u64) -> u64 {
        // splitmix64
        z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn unit(z: u64) -> f64 {
        (mix(z) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Random texture, the same everywhere: a few thousand Gaussian blobs.
    fn texture(w: usize, h: usize, seed: u64) -> impl Fn(f64, f64) -> f32 {
        let count = w * h / 40;
        let blobs: Vec<(f64, f64, f64, f64)> = (0..count as u64)
            .map(|i| {
                let k = seed * 1_000_003 + i * 4;
                (unit(k) * w as f64 * 1.4 - 0.2 * w as f64, unit(k + 1) * h as f64 * 1.4 - 0.2 * h as f64, 1.5 + unit(k + 2) * 5.0, unit(k + 3) - 0.5)
            })
            .collect();
        move |x, y| {
            blobs
                .iter()
                .filter(|b| (x - b.0).abs() < 4.0 * b.2 && (y - b.1).abs() < 4.0 * b.2)
                .map(|&(bx, by, r, a)| a * (-((x - bx).powi(2) + (y - by).powi(2)) / (2.0 * r * r)).exp())
                .sum::<f64>() as f32
        }
    }

    /// Roughly normal noise, from a hash of (seed, i).
    fn noise(seed: u64) -> impl FnMut() -> f32 {
        let mut i = 0u64;
        move || {
            i += 1;
            (0..4).map(|k| unit(seed.wrapping_mul(0x1000_0000_01b3) ^ (i * 4 + k)) as f32 - 0.5).sum::<f32>() * 1.7
        }
    }

    /// Frames of one texture moved by `d`, with noise.
    fn frames(w: usize, h: usize, d: &[[f64; 2]], sigma: f32) -> Vec<Vec<f32>> {
        let tex = texture(w, h, 99);
        d.iter()
            .enumerate()
            .map(|(i, s)| {
                let mut nz = noise(7 + i as u64);
                (0..w * h).map(|k| tex((k % w) as f64 - s[0], (k / w) as f64 - s[1]) + sigma * nz()).collect()
            })
            .collect()
    }

    fn estimate_frames(fr: &[Vec<f32>], w: usize, h: usize, series: bool, n: usize) -> (Vec<[f64; 2]>, Vec<f64>) {
        let g = Geom::new([0, 0, w, h], n, w, h).unwrap();
        let (fft_n, filter) = (Fft::new(g.n), make_filter(g.m));
        let spec_of = |i: usize, off: [isize; 2]| Ok(spectrum(&g, &fft_n, &filter, off, |x, y| fr[i][y * w + x]));
        let (d, cc, _) = measure(&g, fr.len(), series, g.n as f64 / 4.0, &spec_of, &|| ()).unwrap();
        (d, cc)
    }

    /// Shifts agree up to a common offset (which slice is the reference).
    fn check(got: &[[f64; 2]], truth: &[[f64; 2]], tol: f64) {
        let k = got.len() as f64;
        let err: Vec<[f64; 2]> = got.iter().zip(truth).map(|(g, t)| [g[0] - t[0], g[1] - t[1]]).collect();
        let mean = [err.iter().map(|e| e[0]).sum::<f64>() / k, err.iter().map(|e| e[1]).sum::<f64>() / k];
        for (i, e) in err.iter().enumerate() {
            let e = (e[0] - mean[0]).hypot(e[1] - mean[1]);
            assert!(e < tol, "slice {i}: got {:?}, want {:?} up to an offset (error {e:.3})", got[i], truth[i]);
        }
    }

    #[test]
    fn finds_subpixel_drift_in_a_noisy_movie() {
        // A drifting movie: large first jumps, then slow, as stage drift does.
        let truth: Vec<[f64; 2]> = (0..12).map(|i| { let t = i as f64; [6.0 * (1.0 - (-t / 3.0).exp()) + 0.13 * t, -4.0 * (1.0 - (-t / 4.0).exp()) - 0.07 * t] }).collect();
        let fr = frames(256, 256, &truth, 0.5);
        let (d, cc) = estimate_frames(&fr, 256, 256, false, 256);
        check(&d, &truth, 0.25);
        assert!(cc.iter().all(|&c| c > 0.2), "cc {cc:?}");
    }

    #[test]
    fn follows_a_series_with_large_steps() {
        let truth: Vec<[f64; 2]> = (0..9).map(|i| [11.3 * i as f64 - 30.0, 25.0 * (i as f64 * 0.7).sin()]).collect();
        let fr = frames(300, 260, &truth, 0.05);
        let (d, _) = estimate_frames(&fr, 300, 260, true, 256);
        // Errors add up along a series.
        check(&d, &truth, 0.35);
    }

    /// One slice buried in noise (a high tilt): its step stays near its neighbours'.
    #[test]
    fn tracks_through_a_weak_slice() {
        let truth: Vec<[f64; 2]> = (0..9).map(|i| [5.0 * i as f64, -3.0 * i as f64]).collect();
        let mut fr = frames(300, 300, &truth, 0.05);
        let mut nz = noise(1234);
        for v in &mut fr[6] {
            *v = 0.04 * *v + 3.0 * nz();
        }
        let (d, cc) = estimate_frames(&fr, 300, 300, true, 256);
        assert!(cc[6] < cc[5], "cc {cc:?}");
        // The weak slice may be off by a few pixels, the rest must follow.
        let mid = truth[4];
        for (i, (g, t)) in d.iter().zip(&truth).enumerate() {
            let e = (g[0] - (t[0] - mid[0])).hypot(g[1] - (t[1] - mid[1]));
            assert!(e < if i == 6 { 4.0 } else { 0.5 }, "slice {i}: {g:?} vs {:?}", [t[0] - mid[0], t[1] - mid[1]]);
        }
    }

    /// The last slices all buried in noise (the highest tilts): they stay
    /// put instead of running off on wrong peaks.
    #[test]
    fn holds_weak_slices_at_the_end() {
        let truth: Vec<[f64; 2]> = (0..9).map(|i| [5.0 * i as f64, -3.0 * i as f64]).collect();
        let mut fr = frames(300, 300, &truth, 0.05);
        for (k, f) in fr.iter_mut().enumerate().skip(6) {
            let mut nz = noise(500 + k as u64);
            for v in f.iter_mut() {
                *v = 0.04 * *v + 3.0 * nz();
            }
        }
        let (d, cc) = estimate_frames(&fr, 300, 300, true, 256);
        for i in 6..9 {
            let step = (d[i][0] - d[i - 1][0]).hypot(d[i][1] - d[i - 1][1]);
            assert!(step < 8.0, "slice {i} jumped {step:.1} px: {d:?}");
            assert!(cc[i] < cc[4], "cc {cc:?}");
        }
    }

    #[test]
    fn bins_a_large_image() {
        let truth = vec![[0.0, 0.0], [7.0, -3.0], [13.5, -6.2]];
        let fr = frames(600, 520, &truth, 0.05);
        let (d, _) = estimate_frames(&fr, 600, 520, false, 128);
        // Binned by 4 for 128: tolerance in full pixels.
        check(&d, &truth, 0.3);
    }

    #[test]
    fn interpolates_shifts() {
        let (w, h) = (40, 30);
        let v: Vec<f32> = (0..w * h).map(|i| ((i % w) as f32 * 0.3).sin() + (i / w) as f32 * 0.1).collect();
        // A whole-pixel move gives the pixels themselves.
        let a = shift_plane(&v, w, h, [2.0, 3.0], -9.0);
        assert_eq!(a[7 * w + 5], v[10 * w + 7]);
        // Linear in y is reproduced exactly by Catmull–Rom.
        let b = shift_plane(&v, w, h, [0.0, 0.25], -9.0);
        assert!((b[7 * w + 5] - (v[7 * w + 5] + 0.025)).abs() < 1e-4, "{}", b[7 * w + 5]);
        // Off the plane: the fill value.
        assert_eq!(a[(h - 1) * w + 5], -9.0);
        assert_eq!(a[7 * w + w - 1], -9.0);
        let c = shift_plane(&v, w, h, [-0.5, 0.0], -9.0);
        assert_eq!(c[0], -9.0);
        assert!(c[1] != -9.0);
    }

    #[test]
    fn writes_a_readable_mrc() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.mrc");
        let mut bytes = mrc_header(3, 2, 1, 1.5, [0.0, 5.0, 2.5, 1.0], "TifMrcView: aligned sum of 4 frames");
        for v in 0..6 {
            bytes.extend_from_slice(&(v as f32).to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        let src = crate::formats::open(&path).unwrap();
        let info = src.info();
        assert_eq!((info.width, info.height, info.size_z), (3, 2, 1));
        assert!((info.pixel_size.unwrap()[0] - 1.5).abs() < 1e-6);
        let planes = src.read_page(0).unwrap();
        assert_eq!(planes[0].data.get_f64(5), 5.0);
    }
}

/// Timings on a synthetic 30-frame 3072² movie written to a temporary MRC:
/// cargo test --release --lib align::bench -- --ignored --nocapture
#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore]
    fn movie() {
        let (w, frames) = (3072usize, 30usize);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mrc");
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        f.write_all(&mrc_header(w, w, frames, 1.0, [0.0, 1.0, 0.5, 0.2], "test")).unwrap();
        let t = Instant::now();
        // A blurred noise image, sampled at moving positions, plus fresh noise.
        let bw = w + 128;
        let hash = |k: u64| {
            let mut z = k.wrapping_add(0x9e37_79b9_7f4a_7c15);
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            ((z ^ (z >> 31)) >> 40) as f32 / 16_777_216.0 - 0.5
        };
        let mut base: Vec<f32> = (0..bw * bw).into_par_iter().map(|k| hash(k as u64)).collect();
        for _ in 0..2 {
            let rows: Vec<f32> = (0..bw * bw).into_par_iter().map(|k| { let (x, y) = (k % bw, k / bw); (x.saturating_sub(2)..(x + 3).min(bw)).map(|xx| base[y * bw + xx]).sum::<f32>() / 5.0 }).collect();
            base = (0..bw * bw).into_par_iter().map(|k| { let (x, y) = (k % bw, k / bw); (y.saturating_sub(2)..(y + 3).min(bw)).map(|yy| rows[yy * bw + x]).sum::<f32>() / 5.0 }).collect();
        }
        for i in 0..frames {
            let (dx, dy) = (0.37 * i as f64, -0.21 * i as f64);
            let row: Vec<u8> = (0..w * w)
                .into_par_iter()
                .flat_map_iter(|k| {
                    let (x, y) = ((k % w) as f64 - dx + 64.0, (k / w) as f64 - dy + 64.0);
                    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
                    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
                    let at = |xx: usize, yy: usize| base[yy * bw + xx];
                    let v = (1.0 - fy) * ((1.0 - fx) * at(x0, y0) + fx * at(x0 + 1, y0)) + fy * ((1.0 - fx) * at(x0, y0 + 1) + fx * at(x0 + 1, y0 + 1));
                    (v + 0.3 * hash(k as u64 ^ ((i as u64 + 1) << 40))).to_le_bytes()
                })
                .collect();
            f.write_all(&row).unwrap();
        }
        drop(f);
        eprintln!("wrote {frames} × {w}² in {:?}", t.elapsed());
        let img = Arc::new(OpenImage::new(1, crate::formats::open(&path).unwrap(), 4 << 30));
        for n in [512, 1024] {
            let req = AlignReq { token: 1, zs: vec![], c: 0, t: 0, region: None, mode: "movie".into(), n, max_shift: 0.0 };
            let t = Instant::now();
            let a = estimate(&img, &req, &|_, _| ()).unwrap();
            let last = a.shifts[frames - 1];
            let first = a.shifts[0];
            eprintln!("n {n}: estimate {:?}, bin {}, {} iterations, total drift {:.2}, {:.2} (want {:.2}, {:.2})", t.elapsed(), a.bin, a.iterations, last[0] - first[0], last[1] - first[1], 0.37 * (frames - 1) as f64, -0.21 * (frames - 1) as f64);
            if n == 1024 {
                for what in ["sum", "stack"] {
                    let out = dir.path().join(format!("{what}.mrc"));
                    let t = Instant::now();
                    save(&img, &SaveReq { token: 1, zs: a.zs.clone(), shifts: a.shifts.clone(), c: 0, t: 0, path: out.to_string_lossy().into(), what: what.into() }, &|_, _| ()).unwrap();
                    eprintln!("save {what}: {:?}", t.elapsed());
                }
            }
        }
    }

    /// Real data: TMV_ALIGN_FILE=x.mrc [TMV_ALIGN_MODE=series] cargo test --release --lib align::bench::file -- --ignored --nocapture
    #[test]
    #[ignore]
    fn file() {
        let path = std::env::var("TMV_ALIGN_FILE").unwrap();
        let mode = std::env::var("TMV_ALIGN_MODE").unwrap_or_else(|_| "movie".into());
        let img = Arc::new(OpenImage::new(1, crate::formats::open(std::path::Path::new(&path)).unwrap(), 4 << 30));
        let info = img.info();
        eprintln!("{path}: {} × {} × {}", info.width, info.height, info.size_z);
        for n in [512, 1024] {
            let req = AlignReq { token: 1, zs: vec![], c: 0, t: 0, region: None, mode: mode.clone(), n, max_shift: 0.0 };
            let t = Instant::now();
            let a = estimate(&img, &req, &|_, _| ()).unwrap();
            eprintln!("n {n} {mode}: {:?}, bin {}, {} iterations", t.elapsed(), a.bin, a.iterations);
            for (i, (s, c)) in a.shifts.iter().zip(&a.cc).enumerate() {
                eprintln!("  {i:3} {:8.2} {:8.2}  cc {c:.3}", s[0], s[1]);
            }
        }
        let out = std::env::temp_dir().join("tmv-align-sum.mrc");
        let a = estimate(&img, &AlignReq { token: 1, zs: vec![], c: 0, t: 0, region: None, mode, n: 1024, max_shift: 0.0 }, &|_, _| ()).unwrap();
        let t = Instant::now();
        save(&img, &SaveReq { token: 1, zs: a.zs, shifts: a.shifts, c: 0, t: 0, path: out.to_string_lossy().into(), what: "sum".into() }, &|_, _| ()).unwrap();
        eprintln!("sum written to {} in {:?}", out.display(), t.elapsed());
    }
}

/// Cross-correlation of two already sampled square patches. Shared by tilt
/// tracking; the returned displacement takes a centre in `a` to one in `b`.
#[cfg(feature = "pro")]
struct PatchPlan { g: Geom, fft: Fft, small: Fft, filter: Vec<f32> }
#[cfg(feature = "pro")]
impl PatchPlan {
    fn new(n: usize) -> Self {
        Self { g: Geom { x0: 0, y0: 0, bin: 1, n, m: n / 2, w: n, h: n }, fft: Fft::new(n), small: Fft::new(n/2), filter: make_filter(n/2) }
    }
}
#[cfg(feature = "pro")]
thread_local! { static PATCH_PLANS: std::cell::RefCell<std::collections::HashMap<usize, Arc<PatchPlan>>> = std::cell::RefCell::new(std::collections::HashMap::new()); }

#[cfg(feature = "pro")]
fn patch_plan(n: usize) -> Arc<PatchPlan> {
    PATCH_PLANS.with(|plans| plans.borrow_mut().entry(n).or_insert_with(|| Arc::new(PatchPlan::new(n))).clone())
}

#[cfg(feature = "pro")]
pub(crate) fn patch_shift(a: &[f32], b: &[f32], n: usize, max: f64) -> ([f64; 2], f64) {
    let plan=patch_plan(n);
    let sa=spectrum(&plan.g,&plan.fft,&plan.filter,[0,0],|x,y|a[y*n+x]);
    let sb=spectrum(&plan.g,&plan.fft,&plan.filter,[0,0],|x,y|b[y*n+x]);
    pair_shift(&sa,&sb,&plan.g,&plan.small,max)
}

#[cfg(feature = "pro")]
pub(crate) fn patch_cross_peak(pr: &[f32], pi: &[f32], n: usize, energies: [f64;2], max: f64) -> ([f64;2],f64) {
    let p=patch_plan(n);
    let start=coarse_peak(pr,pi,&p.g,&p.small,max);
    let (d,v)=refine_peak(pr,pi,&p.g,start);
    (d,norm_cc(v,energies[0],energies[1]))
}

#[cfg(feature = "pro")]
pub(crate) fn patch_window(n: usize) -> (Vec<f32>, Vec<f32>) {
    let edge=(n/16).max(2);
    let win:Vec<f32>=(0..n).map(|i| {let d=i.min(n-1-i); if d>=edge {1.0} else {(0.5-0.5*(PI*(d as f64+0.5)/edge as f64).cos()) as f32}}).collect();
    ((0..n*n).map(|i|win[i/n]*win[i%n]).collect(), make_filter(n/2))
}

/// Motion workspace: the same iterative movie solver on reduced groups.
#[cfg(feature = "pro")]
pub(crate) fn movie_planes(frames: &[Vec<f32>], w: usize, h: usize, progress: &(dyn Fn(usize,usize)+Sync)) -> Result<Aligned> {
    let start=std::time::Instant::now();
    if frames.len()<2||frames.iter().any(|v|v.len()!=w*h){bail!("电影尺寸不一致");}
    let g=Geom::new([0,0,w,h],1024,w,h)?;let f=Fft::new(g.n);let filter=make_filter(g.m);
    let spec_of=|i:usize,off:[isize;2]|->Result<Spec>{
        if CANCEL.load(Ordering::SeqCst){bail!("已取消");}
        Ok(spectrum(&g,&f,&filter,off,|x,y|frames[i][y*w+x]))
    };
    let done=std::sync::atomic::AtomicUsize::new(0);let total=2*frames.len()+24;
    let (mut shifts,mut cc,mut iterations)=measure(&g,frames.len(),false,g.n as f64/4.0,&spec_of,&||progress((done.fetch_add(1,Ordering::Relaxed)+1).min(total),total))?;
    let median=|v:&[f64]| {let mut a=v.to_vec();a.sort_by(f64::total_cmp);a[a.len()/2]};
    if median(&cc)<0.08 {
        // Low-contrast movies need the full field and a narrower frequency band.
        let n=(1usize<<w.min(h).ilog2()).min(1024);let bin=w.min(h)/n;
        let wide=Geom{x0:(w-n*bin)/2,y0:(h-n*bin)/2,bin,n,m:n/2,w,h};
        let fft=Fft::new(n);let mut low=make_filter(n/2);
        for (i,v) in low.iter_mut().enumerate(){let x=(i%(n/2)) as f64-(n/4) as f64;let y=(i/(n/2)) as f64-(n/4) as f64;*v*=(-0.5*(x*x+y*y)/(n as f64*0.04).powi(2)).exp() as f32;}
        let specs:Vec<_>=frames.par_iter().map(|v|spectrum(&wide,&fft,&low,[0,0],|x,y|v[y*w+x])).collect();
        let (mut d,c,it)=align_movie(&specs,&wide,&Fft::new(n/2),n as f64/16.0,&|i|progress(i,MAX_ITER))?;
        if median(&c)>median(&cc){
            let mean:[f64;2]=std::array::from_fn(|k|d.iter().map(|v|v[k]).sum::<f64>()/d.len() as f64);
            for v in &mut d{for k in 0..2{v[k]=(v[k]-mean[k])*bin as f64;}}
            shifts=d;cc=c;iterations+=it;
        }
        if median(&cc)<0.12{bail!("电影全局相关度过低（中位 CC={:.3}），无法可靠测量运动；请增加分组或核对原始电影",median(&cc));}
    }
    Ok(Aligned {zs:(0..frames.len()).collect(),shifts,cc,bin:g.bin,n:g.n,iterations,ms:start.elapsed().as_secs_f64()*1000.0})
}

/// Untapered image warping shared with native motion correction.
#[cfg(feature = "pro")]
pub(crate) fn movie_shift(v:&[f32],w:usize,h:usize,d:[f64;2])->Vec<f32> {shift_plane(v,w,h,d,0.0)}
