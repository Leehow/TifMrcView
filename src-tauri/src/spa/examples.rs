//! "Paint a few, find the rest" for single particles. The user paints two or
//! three particles on a micrograph or two with the viewer's label brush (a
//! layer of 小颗粒, refined to the particle's outline; 找同类 may already have
//! added more on that micrograph) and ticks the layer in the workspace. Each
//! painted object is an example: its picture, a disc as large as the paint,
//! is searched for at every in-plane angle in every micrograph, and the
//! places that look like one of the examples are shown for the user to check
//! before anything is extracted (`Found`, `examples.json`).
//!
//! Nothing here knows what a particle is: no blob size, no darkness, no shape
//! model. What follows from the examples alone:
//! - The scale: micrographs are binned so a painted particle is about
//!   `ACROSS` pixels across, and band-passed round that size.
//! - How alike is alike enough: each example is also found by the others
//!   with some score (`Found::mutual`, in robust deviations of that
//!   example's own scores on the micrograph, so a long side view and a round
//!   top view are measured alike); places that score about as much as the
//!   examples typically do count as one more (`suggested`). With one example, or examples
//!   that do not find each other (different views), the scores of all places
//!   are split where they fall into two groups (Otsu). The user moves the
//!   cut-off with a slider, clicks a wrong place away and clicks a missed one
//!   in; 再找一轮 searches again with the added places as more examples and the
//!   dropped ones as counter-examples (a place more like a counter-example
//!   than like any example is left out).
//!
//! The score is the normalised cross-correlation under the example's disc
//! (zero-mean template, local mean and energy of the micrograph under the
//! same disc), computed with FFTs, two rotated templates per complex
//! transform.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::pick::{bin, gauss, robust};
use super::project::{read_mic, Project};
use super::{check_cancel, Progress};
use crate::fft::Fft;

/// Pixels across a painted particle after binning.
const ACROSS: f64 = 24.0;
/// The disc searched for is this much wider than the paint.
const WIDER: f64 = 1.1;
/// Places kept per micrograph at most (best first).
const MOST: usize = 3000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    pub mic: usize,
    /// Centre and radius, micrograph pixels.
    pub x: f64,
    pub y: f64,
    pub radius: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub x: f32,
    pub y: f32,
    /// Best score with an example, in robust deviations above that example's
    /// typical score on the micrograph (−1 for a place the user clicked in).
    pub score: f32,
    /// Best score with a counter-example (−1: none).
    pub neg: f32,
    /// The user's word: 1 = keep, −1 = drop, 0 = by the cut-off.
    #[serde(default)]
    pub user: i8,
}

impl Place {
    pub fn kept(&self, cutoff: f32) -> bool {
        match self.user {
            1 => true,
            -1 => false,
            _ => self.score >= cutoff && self.score > self.neg,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub examples: Vec<Example>,
    pub negatives: Vec<Example>,
    pub mics: Vec<Vec<Place>>,
    /// The cut-off in use and the one the examples suggest.
    pub cutoff: f32,
    pub suggested: f32,
    /// Each example's best score with the other examples.
    pub mutual: Vec<f32>,
    pub bin: usize,
    /// Particle diameter (Å) for the box and the mask: the paint's size.
    pub diameter_a: f64,
}

impl Found {
    pub fn kept(&self) -> usize {
        self.mics.iter().flatten().filter(|p| p.kept(self.cutoff)).count()
    }
    pub fn radius_px(&self) -> f64 {
        median(self.examples.iter().map(|e| e.radius).collect())
    }
}

fn file(p: &Project) -> PathBuf {
    Path::new(&p.dir).join("examples.json")
}

pub fn load(p: &Project) -> Result<Found> {
    serde_json::from_slice(&std::fs::read(file(p)).context("还没有按例子找颗粒")?).context("examples.json")
}

pub fn save(p: &Project, f: &Found) -> Result<()> {
    std::fs::write(file(p), serde_json::to_vec(f)?)?;
    Ok(())
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// The examples of the ticked label layers ([image, label]) of the project's
/// micrographs.
pub fn examples_of(p: &Project, ticked: &[(String, u16)]) -> Result<Vec<Example>> {
    let mut out = vec![];
    for (image, label) in ticked {
        let Some(mic) = p.mics.iter().position(|m| &m.path == image) else { bail!("{image} 不是这个工程的显微图") };
        for o in crate::sta::layers::painted(image, &[*label])? {
            if o.half >= 2.0 {
                out.push(Example { mic, x: o.centre[0], y: o.centre[1], radius: o.half });
            }
        }
    }
    if out.is_empty() {
        bail!("勾选的图层上没有涂到颗粒");
    }
    Ok(out)
}

// ---------- the search ----------

/// 2D transform in place (rows, then columns); `inverse` conjugates around it
/// and scales by 1/n².
fn fft2(fft: &Fft, n: usize, re: &mut [f32], im: &mut [f32], inverse: bool) {
    if inverse {
        im.iter_mut().for_each(|v| *v = -*v);
    }
    for (r, i) in re.chunks_mut(n).zip(im.chunks_mut(n)) {
        fft.run(r, i);
    }
    let (mut cr, mut ci) = (vec![0f32; n], vec![0f32; n]);
    for x in 0..n {
        for y in 0..n {
            cr[y] = re[y * n + x];
            ci[y] = im[y * n + x];
        }
        fft.run(&mut cr, &mut ci);
        for y in 0..n {
            re[y * n + x] = cr[y];
            im[y * n + x] = ci[y];
        }
    }
    if inverse {
        let s = 1.0 / (n * n) as f32;
        re.iter_mut().for_each(|v| *v *= s);
        im.iter_mut().for_each(|v| *v *= -s);
    }
}

/// A micrograph as it is searched: binned by `f`, band-passed round the
/// particle size `d` (binned pixels), robustly standardised and clipped.
fn prepared(img: &[f32], w: usize, h: usize, f: usize, d: f64) -> (Vec<f32>, usize, usize) {
    let (b, bw, bh) = bin(img, w, h, f);
    let fine = gauss(&b, bw, bh, 0.8);
    let coarse = gauss(&b, bw, bh, d.max(4.0));
    let v: Vec<f32> = fine.iter().zip(&coarse).map(|(a, c)| a - c).collect();
    let (m, s) = robust(&v);
    (v.iter().map(|x| ((x - m) / s).clamp(-4.0, 4.0)).collect(), bw, bh)
}

fn sample(v: &[f32], w: usize, h: usize, x: f64, y: f64) -> f32 {
    let (x, y) = (x.clamp(0.0, (w - 1) as f64), y.clamp(0.0, (h - 1) as f64));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let a = v[y0 * w + x0] * (1.0 - fx) + v[y0 * w + x1] * fx;
    let b = v[y1 * w + x0] * (1.0 - fx) + v[y1 * w + x1] * fx;
    a * (1.0 - fy) + b * fy
}

/// A template: the values under a disc of radius `r` (binned pixels) about
/// its centre, turned by an angle, zero mean and unit norm over the disc, as
/// (dx, dy, value).
type Template = Vec<(i32, i32, f32)>;

fn template(v: &[f32], w: usize, h: usize, c: [f64; 2], r: f64, angle: f64) -> Template {
    let (s, k) = angle.sin_cos();
    let ri = r.ceil() as i32;
    let mut t: Template = vec![];
    for dy in -ri..=ri {
        for dx in -ri..=ri {
            if ((dx * dx + dy * dy) as f64) <= r * r {
                let (u, w2) = (dx as f64, dy as f64);
                t.push((dx, dy, sample(v, w, h, c[0] + k * u - s * w2, c[1] + s * u + k * w2)));
            }
        }
    }
    let mean = t.iter().map(|x| x.2).sum::<f32>() / t.len() as f32;
    let norm = t.iter().map(|x| (x.2 - mean).powi(2)).sum::<f32>().sqrt().max(1e-6);
    t.iter().map(|&(a, b, x)| (a, b, (x - mean) / norm)).collect()
}

fn disc(r: f64) -> Template {
    let ri = r.ceil() as i32;
    let mut t = vec![];
    for dy in -ri..=ri {
        for dx in -ri..=ri {
            if ((dx * dx + dy * dy) as f64) <= r * r {
                t.push((dx, dy, 1.0));
            }
        }
    }
    t
}

/// Wraps template values onto an n×n grid with its centre at index 0.
fn put(t: &Template, n: usize, out: &mut [f32]) {
    for &(dx, dy, v) in t {
        let (x, y) = (dx.rem_euclid(n as i32) as usize, dy.rem_euclid(n as i32) as usize);
        out[y * n + x] += v;
    }
}

/// What one example is searched with: its templates at every angle and its radius.
struct Searched {
    templates: Vec<Template>,
    radius: f64,
}

/// The best normalised correlation of each of `searched` at every pixel of
/// a prepared micrograph (n×n, valid on bw×bh).
fn scores(img: &[f32], bw: usize, bh: usize, searched: &[Searched]) -> (usize, Vec<Vec<f32>>) {
    let n = bw.max(bh).next_power_of_two();
    let fft = Fft::new(n);
    let mut re = vec![0f32; n * n];
    for y in 0..bh {
        re[y * n..y * n + bw].copy_from_slice(&img[y * bw..(y + 1) * bw]);
    }
    let mut sq: Vec<f32> = re.iter().map(|v| v * v).collect();
    let mut im = vec![0f32; n * n];
    let mut im2 = vec![0f32; n * n];
    // One transform for the image (real) and its square (imaginary).
    let (fi_re, fi_im, f2_re, f2_im) = {
        let mut a = re.clone();
        let mut b = std::mem::take(&mut sq);
        std::mem::swap(&mut im, &mut b);
        fft2(&fft, n, &mut a, &mut im, false);
        // Z = F(I) + i F(I²): separate by conjugate symmetry.
        let (mut r1, mut i1, mut r2, mut i2) = (vec![0f32; n * n], vec![0f32; n * n], vec![0f32; n * n], vec![0f32; n * n]);
        for ky in 0..n {
            for kx in 0..n {
                let k = ky * n + kx;
                let c = ((n - ky) % n) * n + (n - kx) % n;
                let (zr, zi, cr, ci) = (a[k], im[k], a[c], -im[c]);
                r1[k] = 0.5 * (zr + cr);
                i1[k] = 0.5 * (zi + ci);
                r2[k] = 0.5 * (zi - ci);
                i2[k] = -0.5 * (zr - cr);
            }
        }
        im2.fill(0.0);
        (r1, i1, r2, i2)
    };
    // Local energy under each distinct disc.
    let mut radii: Vec<i64> = searched.iter().map(|s| (s.radius * 4.0).round() as i64).collect();
    radii.sort_unstable();
    radii.dedup();
    let energy: Vec<(i64, Vec<f32>)> = radii
        .par_iter()
        .map(|&r4| {
            let d = disc(r4 as f64 / 4.0);
            let cnt = d.len() as f32;
            let mut m = vec![0f32; n * n];
            put(&d, n, &mut m);
            let mut mi = vec![0f32; n * n];
            fft2(&fft, n, &mut m, &mut mi, false);
            // conj(M) times F(I) and F(I²), packed as one inverse: S1 + i S2.
            let mut pr = vec![0f32; n * n];
            let mut pi = vec![0f32; n * n];
            for k in 0..n * n {
                let (mr, mim) = (m[k], -mi[k]);
                let (ar, ai) = (fi_re[k] * mr - fi_im[k] * mim, fi_re[k] * mim + fi_im[k] * mr);
                let (br, bi) = (f2_re[k] * mr - f2_im[k] * mim, f2_re[k] * mim + f2_im[k] * mr);
                pr[k] = ar - bi;
                pi[k] = ai + br;
            }
            fft2(&fft, n, &mut pr, &mut pi, true);
            let e: Vec<f32> = pr.iter().zip(&pi).map(|(s1, s2)| (s2 - s1 * s1 / cnt).max(1e-3).sqrt()).collect();
            (r4, e)
        })
        .collect();
    let _ = im2;
    let out = searched
        .iter()
        .map(|s| {
            let r4 = (s.radius * 4.0).round() as i64;
            let e = &energy.iter().find(|x| x.0 == r4).unwrap().1;
            let pairs: Vec<(&Template, Option<&Template>)> = s.templates.chunks(2).map(|c| (&c[0], c.get(1))).collect();
            pairs
                .par_iter()
                .fold(
                    || vec![f32::MIN; n * n],
                    |mut best, (a, b)| {
                        let mut tr = vec![0f32; n * n];
                        let mut ti = vec![0f32; n * n];
                        put(a, n, &mut tr);
                        if let Some(b) = b {
                            put(b, n, &mut ti);
                        }
                        fft2(&fft, n, &mut tr, &mut ti, false);
                        // F(I)·conj(Ta + i Tb) → ca − i cb after the inverse.
                        for k in 0..n * n {
                            let (zr, zi) = (tr[k], -ti[k]);
                            let (ar, ai) = (fi_re[k], fi_im[k]);
                            tr[k] = ar * zr - ai * zi;
                            ti[k] = ar * zi + ai * zr;
                        }
                        fft2(&fft, n, &mut tr, &mut ti, true);
                        for k in 0..n * n {
                            let mut v = tr[k];
                            if b.is_some() {
                                v = v.max(-ti[k]);
                            }
                            best[k] = best[k].max(v / e[k]);
                        }
                        best
                    },
                )
                .reduce(|| vec![f32::MIN; n * n], |a, b| a.iter().zip(&b).map(|(x, y)| x.max(*y)).collect())
        })
        .collect();
    (n, out)
}

/// Angles at which a disc of radius `r` (binned pixels) is searched: its rim
/// moves about 1.5 pixels from one to the next, 12 to 48 of them.
fn angles(r: f64) -> Vec<f64> {
    let k = ((std::f64::consts::TAU * r / 1.5).round() as usize).clamp(12, 48);
    (0..k).map(|i| i as f64 * std::f64::consts::TAU / k as f64).collect()
}

/// Splits `v` into two groups where the variance between them is largest.
fn otsu(v: &[f32]) -> f32 {
    if v.len() < 4 {
        return v.iter().copied().fold(f32::MAX, f32::min);
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    let total: f64 = s.iter().map(|&x| x as f64).sum();
    let n = s.len() as f64;
    let (mut best, mut at, mut acc) = (f64::MIN, s[0], 0f64);
    for i in 1..s.len() {
        acc += s[i - 1] as f64;
        let (w0, w1) = (i as f64, n - i as f64);
        let (m0, m1) = (acc / w0, (total - acc) / w1);
        let between = w0 * w1 * (m0 - m1).powi(2);
        if between > best {
            best = between;
            at = s[i];
        }
    }
    at
}

/// The search of all micrographs for `examples` (counter-examples
/// `negatives`). Places the user marked in `before` keep their mark.
pub fn find(p: &Project, examples: Vec<Example>, negatives: Vec<Example>, before: Option<&Found>, progress: Progress) -> Result<Found> {
    if examples.is_empty() {
        bail!("还没有例子");
    }
    let px = p.optics.pixel_a;
    let radius = median(examples.iter().map(|e| e.radius).collect());
    // Binned at least so far that a micrograph fits a 1024² transform: one of 2048² costs four times as much.
    let side = p.mics.iter().map(|m| m.width.max(m.height)).max().unwrap_or(1024);
    let f = ((2.0 * radius / ACROSS).floor() as usize).max(side.div_ceil(1024)).max(1);
    let d = 2.0 * radius / f as f64;
    let nmics = p.mics.len();
    // The templates come from the prepared micrographs the examples are on.
    let all: Vec<(&Example, bool)> = examples.iter().map(|e| (e, true)).chain(negatives.iter().map(|e| (e, false))).collect();
    let mut on: Vec<usize> = all.iter().map(|e| e.0.mic).collect();
    on.sort_unstable();
    on.dedup();
    let mut searched: Vec<Searched> = vec![];
    let mut order: Vec<usize> = vec![];
    for &m in &on {
        check_cancel()?;
        let (img, w, h) = read_mic(&p.mics[m].path)?;
        let (v, bw, bh) = prepared(&img, w, h, f, d);
        for (k, (e, _)) in all.iter().enumerate().filter(|(_, e)| e.0.mic == m) {
            let r = (e.radius * WIDER / f as f64).max(3.0);
            let c = [e.x / f as f64, e.y / f as f64];
            searched.push(Searched { templates: angles(r).into_iter().map(|a| template(&v, bw, bh, c, r, a)).collect(), radius: r });
            order.push(k);
        }
    }
    // Back in the order of `all`.
    let mut by: Vec<Option<Searched>> = (0..all.len()).map(|_| None).collect();
    for (s, k) in searched.into_iter().zip(order) {
        by[k] = Some(s);
    }
    let searched: Vec<Searched> = by.into_iter().map(|s| s.unwrap()).collect();
    let npos = examples.len();
    let smallest = searched[..npos].iter().map(|s| s.radius).fold(f64::MAX, f64::min);
    let mut mutual = vec![f32::MIN; npos];
    let mut mics: Vec<Vec<Place>> = vec![vec![]; nmics];
    for m in 0..nmics {
        check_cancel()?;
        progress("按例子找颗粒", m, nmics);
        if p.mics[m].excluded {
            continue;
        }
        let (img, w, h) = read_mic(&p.mics[m].path)?;
        let (v, bw, bh) = prepared(&img, w, h, f, d);
        let (n, mut maps) = scores(&v, bw, bh, &searched);
        // Each example's scores in units of its own spread on this micrograph: an elongated
        // example (a side view) correlates less with anything than a round one, so raw scores
        // of different examples are not comparable, while how far a place stands out is.
        {
            let margin = smallest.ceil() as usize;
            for map in maps.iter_mut() {
                let valid: Vec<f32> = (margin..bh.saturating_sub(margin)).flat_map(|y| (margin..bw.saturating_sub(margin)).map(move |x| y * n + x)).map(|k| map[k]).collect();
                let (med, sd) = robust(&valid);
                map.iter_mut().for_each(|v| *v = (*v - med) / sd);
            }
        }
        // How each example on this micrograph scores with the others.
        for (i, e) in examples.iter().enumerate().filter(|(_, e)| e.mic == m) {
            let (cx, cy) = ((e.x / f as f64).round() as i64, (e.y / f as f64).round() as i64);
            for (j, map) in maps[..npos].iter().enumerate() {
                if j == i {
                    continue;
                }
                for dy in -2..=2i64 {
                    for dx in -2..=2i64 {
                        let (x, y) = (cx + dx, cy + dy);
                        if x >= 0 && y >= 0 && (x as usize) < bw && (y as usize) < bh {
                            mutual[i] = mutual[i].max(map[y as usize * n + x as usize]);
                        }
                    }
                }
            }
        }
        let pos: Vec<f32> = (0..n * n).map(|k| maps[..npos].iter().map(|m| m[k]).fold(f32::MIN, f32::max)).collect();
        let neg = |k: usize| maps[npos..].iter().map(|m| m[k]).fold(-1f32, f32::max);
        // Peaks: local maxima, a disc's radius in from the edge, then apart by most of a diameter.
        let margin = smallest.ceil() as usize;
        let win = (0.3 * 2.0 * smallest).round().max(1.0) as i64;
        let valid: Vec<f32> = (margin..bh.saturating_sub(margin)).flat_map(|y| (margin..bw.saturating_sub(margin)).map(move |x| (x, y))).map(|(x, y)| pos[y * n + x]).collect();
        let (med, sd) = robust(&valid);
        let floor = (med + 2.0 * sd).max(2.0);
        let mut peaks: Vec<(f32, usize, usize)> = vec![];
        for y in margin..bh.saturating_sub(margin) {
            for x in margin..bw.saturating_sub(margin) {
                let s = pos[y * n + x];
                if s <= floor {
                    continue;
                }
                let mut top = true;
                'w: for dy in -win..=win {
                    for dx in -win..=win {
                        let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                        if (dx, dy) != (0, 0) && xx >= 0 && yy >= 0 && (xx as usize) < bw && (yy as usize) < bh && pos[yy as usize * n + xx as usize] > s {
                            top = false;
                            break 'w;
                        }
                    }
                }
                if top {
                    peaks.push((s, x, y));
                }
            }
        }
        peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
        let apart = (0.8 * 2.0 * smallest).powi(2);
        let mut kept: Vec<(f32, usize, usize)> = vec![];
        for q in peaks {
            if kept.len() >= MOST {
                break;
            }
            if kept.iter().all(|k| ((k.1 as f64 - q.1 as f64).powi(2) + (k.2 as f64 - q.2 as f64).powi(2)) >= apart) {
                kept.push(q);
            }
        }
        mics[m] = kept
            .iter()
            .map(|&(s, x, y)| Place { x: ((x as f64 + 0.5) * f as f64) as f32, y: ((y as f64 + 0.5) * f as f64) as f32, score: s, neg: neg(y * n + x), user: 0 })
            .collect();
    }
    progress("按例子找颗粒", nmics, nmics);
    // Places the user marked before keep their mark (the nearest place within a radius, or as clicked).
    if let Some(b) = before {
        let r2 = (radius * radius) as f32;
        for (m, list) in b.mics.iter().enumerate() {
            for q in list.iter().filter(|q| q.user != 0) {
                let Some(here) = mics.get_mut(m) else { continue };
                match here.iter_mut().filter(|o| (o.x - q.x).powi(2) + (o.y - q.y).powi(2) <= r2).min_by(|a, b| ((a.x - q.x).powi(2) + (a.y - q.y).powi(2)).total_cmp(&((b.x - q.x).powi(2) + (b.y - q.y).powi(2)))) {
                    Some(o) => o.user = q.user,
                    None if q.user == 1 => here.push(Place { user: 1, ..q.clone() }),
                    None => {}
                }
            }
        }
    }
    let scores: Vec<f32> = mics.iter().flatten().filter(|q| q.score >= 0.0).map(|q| q.score).collect();
    let mutual: Vec<f32> = mutual.into_iter().map(|m| if m == f32::MIN { -1.0 } else { m }).collect();
    let alike: Vec<f32> = mutual.iter().copied().filter(|&m| m > 0.0).collect();
    let split = otsu(&scores);
    // What the examples typically score with each other, a little below (the median: one
    // example unlike the rest, a rare view, would otherwise let everything in).
    let suggested = if alike.len() >= 2 { median(alike.iter().map(|&m| m as f64).collect()) as f32 * 0.9 } else { split };
    let diameter_a = ((2.0 * radius * px * 1.15) / 5.0).round() * 5.0;
    Ok(Found { examples, negatives, mics, cutoff: suggested, suggested, mutual, bin: f, diameter_a })
}

/// Searches again with the places the user clicked in as more examples and
/// the ones clicked away as counter-examples.
pub fn again(p: &Project, f: &Found, progress: Progress) -> Result<Found> {
    let r = f.radius_px();
    let mut examples = f.examples.clone();
    let mut negatives = f.negatives.clone();
    for (m, list) in f.mics.iter().enumerate() {
        for q in list {
            let e = Example { mic: m, x: q.x as f64, y: q.y as f64, radius: r };
            match q.user {
                1 if q.score < f.cutoff => examples.push(e),
                -1 if q.score >= f.cutoff => negatives.push(e),
                _ => {}
            }
        }
    }
    find(p, examples, negatives, Some(f), progress)
}

/// A click at (x, y) on micrograph `mic`: on a place, flips whether it is kept;
/// elsewhere, adds a place there.
pub fn click(f: &mut Found, mic: usize, x: f32, y: f32) -> Result<()> {
    let cutoff = f.cutoff;
    let r = f.radius_px() as f32;
    let list = f.mics.get_mut(mic).context("没有这张显微图")?;
    let near = list
        .iter_mut()
        .filter(|q| (q.x - x).powi(2) + (q.y - y).powi(2) <= r * r)
        .min_by(|a, b| ((a.x - x).powi(2) + (a.y - y).powi(2)).total_cmp(&((b.x - x).powi(2) + (b.y - y).powi(2))));
    match near {
        Some(q) => q.user = if q.kept(cutoff) { -1 } else { 1 },
        None => list.push(Place { x, y, score: -1.0, neg: -1.0, user: 1 }),
    }
    Ok(())
}

/// The kept places become the picks (`picks.json`), and the paint's size the
/// particle diameter.
pub fn use_found(p: &mut Project, f: &Found) -> Result<usize> {
    let picks: Vec<Vec<[f32; 3]>> = f.mics.iter().map(|l| l.iter().filter(|q| q.kept(f.cutoff)).map(|q| [q.x, q.y, q.score.max(0.0)]).collect()).collect();
    let n: usize = picks.iter().map(Vec::len).sum();
    if n == 0 {
        bail!("一个颗粒也没有留下");
    }
    for (m, l) in p.mics.iter_mut().zip(&picks) {
        m.picks = l.len();
    }
    std::fs::write(Path::new(&p.dir).join("picks.json"), serde_json::to_vec(&picks)?)?;
    p.diameter_a = f.diameter_a;
    p.save()?;
    Ok(n)
}

/// The workspace's steps (`spa_run`, here and on an SSH server): "examples"
/// searches from the ticked label layers ({ticked: [[image, label]…]}) or
/// again from the last search with the user's clicks ({again: true});
/// "examplesUse" makes the kept places the picks.
pub fn step(p: &mut Project, step: &str, v: &serde_json::Value, progress: Progress) -> Result<serde_json::Value> {
    Ok(match step {
        "examples" => {
            let f = if v.get("again").and_then(|b| b.as_bool()).unwrap_or(false) {
                again(p, &load(p)?, progress)?
            } else {
                let ticked: Vec<(String, u16)> = serde_json::from_value(v["ticked"].clone()).context("勾选的图层")?;
                find(p, examples_of(p, &ticked)?, vec![], None, progress)?
            };
            save(p, &f)?;
            serde_json::json!({"kept": f.kept(), "cutoff": f.cutoff, "suggested": f.suggested, "examples": f.examples.len(), "diameterA": f.diameter_a})
        }
        "examplesUse" => {
            let f = load(p)?;
            serde_json::json!(use_found(p, &f)?)
        }
        other => bail!("未知步骤 {other}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Discs of two kinds (a ring and a bar) on noise: examples of the ring find the rings, not the bars.
    #[test]
    fn finds_what_is_like_the_examples() {
        let (w, h) = (256usize, 256usize);
        let mut rng = super::super::Rng(7);
        let mut img: Vec<f32> = (0..w * h).map(|_| rng.gauss() as f32 * 0.6).collect();
        let rings = [(40.0, 40.0), (120.0, 60.0), (200.0, 50.0), (60.0, 150.0), (180.0, 200.0)];
        let bars = [(120.0, 140.0), (210.0, 120.0), (40.0, 220.0)];
        for y in 0..h {
            for x in 0..w {
                for &(cx, cy) in &rings {
                    let r = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
                    if (6.0..10.0).contains(&r) {
                        img[y * w + x] += 1.5;
                    }
                }
                for &(cx, cy) in &bars {
                    if (x as f64 - cx).abs() < 10.0 && (y as f64 - cy).abs() < 3.0 {
                        img[y * w + x] += 1.5;
                    }
                }
            }
        }
        let (v, bw, bh) = prepared(&img, w, h, 1, 20.0);
        let ex = [(40.0, 40.0), (120.0, 60.0)];
        let searched: Vec<Searched> = ex.iter().map(|&(x, y)| Searched { templates: angles(11.0).into_iter().map(|a| template(&v, bw, bh, [x, y], 11.0, a)).collect(), radius: 11.0 }).collect();
        let (n, maps) = scores(&v, bw, bh, &searched);
        let at = |x: f64, y: f64| maps.iter().map(|m| m[y as usize * n + x as usize]).fold(f32::MIN, f32::max);
        let ring_low = rings[2..].iter().map(|&(x, y)| at(x, y)).fold(f32::MAX, f32::min);
        let bar_high = bars.iter().map(|&(x, y)| at(x, y)).fold(f32::MIN, f32::max);
        assert!(ring_low > bar_high + 0.1, "rings {ring_low} bars {bar_high}");
        assert!(ring_low > 0.4, "{ring_low}");
    }

    /// On real data, not in the repository: `TMV_SPA_DIR` (a project folder),
    /// `TMV_EXAMPLES` ("mic,x,y,radius;…" marked by eye, micrograph pixels),
    /// `TMV_OUT` (a JSON file for what was found). Scoring is done outside.
    #[test]
    #[ignore]
    fn real_examples() {
        let dir = std::env::var("TMV_SPA_DIR").unwrap();
        let p = Project::load(&dir).unwrap();
        let examples: Vec<Example> = std::env::var("TMV_EXAMPLES")
            .unwrap()
            .split(';')
            .map(|e| {
                let v: Vec<f64> = e.split(',').map(|x| x.trim().parse().unwrap()).collect();
                Example { mic: v[0] as usize, x: v[1], y: v[2], radius: v[3] }
            })
            .collect();
        let t = std::time::Instant::now();
        let f = find(&p, examples, vec![], None, &|_, _, _| {}).unwrap();
        eprintln!("{:.1} s, bin {}, mutual {:?}, suggested {}, kept {}", t.elapsed().as_secs_f64(), f.bin, f.mutual, f.suggested, f.kept());
        std::fs::write(std::env::var("TMV_OUT").unwrap(), serde_json::to_vec(&f).unwrap()).unwrap();
    }

    #[test]
    fn otsu_splits_two_groups() {
        let v: Vec<f32> = (0..50).map(|i| 0.1 + i as f32 * 0.001).chain((0..10).map(|i| 0.6 + i as f32 * 0.001)).collect();
        let s = otsu(&v);
        assert!(s > 0.2 && s <= 0.6, "{s}");
    }
}
