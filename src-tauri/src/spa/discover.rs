//! Finds the kinds of particle on a few micrographs, so that the user only
//! has to say which one is theirs (the one-click mode of the workspace).
//!
//! 1. A handful of micrographs with the best CTF fits are binned to about
//!    1000 pixels and denoised with Noise2Void (`denoise_learn`, trained on
//!    them alone).
//! 2. Blobs of every size from 40 to 500 Å are found in the denoised images
//!    as local maxima of a difference-of-Gaussians scale space (fixed ratio,
//!    so the response is comparable across sizes); the scale of a maximum
//!    gives the blob's size.
//! 3. Optionally SAM 2 (prompted with a point on each blob) outlines the
//!    blob, and the outline gives a better size.
//! 4. Each blob is described by its size, its elongation and its radial
//!    profile (rotation-invariant), and the blobs are clustered (k-means).
//!    Every kind gets a strip of its most typical members, its size and how
//!    common it is. Ice, carbon edges and aggregates fall into kinds of their
//!    own that the user simply does not choose.
//!
//! `symmetry` suggests a point group from the 2D class averages: the n-fold
//! rotational autocorrelation of each average (top views of Cn and Dn show
//! n-fold symmetry) and a 2-fold one on other views (side views of Dn).

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::pick::{bin, gauss, robust};
use super::project::{read_mic, Project};
use super::{check_cancel, render, Progress, Rng};

/// Outlines blobs: (denoised image, w, h, blob centres) → equivalent
/// diameter of each outline in pixels, None where there is no good one.
pub type Outliner<'a> = &'a (dyn Fn(&[f32], usize, usize, &[[f32; 2]]) -> Result<Vec<Option<f64>>> + Sync);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kind {
    /// Typical diameter, Å.
    pub diameter_a: f64,
    /// Blobs of this kind found, and their share of all blobs.
    pub count: usize,
    pub share: f64,
    /// Length over width.
    pub elongation: f64,
    /// How strongly they stand out (median DoG response).
    pub contrast: f64,
    /// PNG strip of typical members.
    pub png: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub kinds: Vec<Kind>,
    pub micrographs: usize,
    /// Pixel size of the working images, Å.
    pub working_px: f64,
    /// Sizes outlined by SAM (else from the scale space).
    pub outlined: bool,
    pub backend: String,
    /// Kinds the user chose.
    #[serde(default)]
    pub chosen: Vec<usize>,
}

struct Blob {
    img: usize,
    x: f32,
    y: f32,
    resp: f32,
    feat: Vec<f32>,
}

const CROP: usize = 40;
const PROFILE: usize = 12;
const MAX_PER_MIC: usize = 400;

/// Micrographs to look at: up to `n`, the best CTF fits, spread over the set.
fn sample(p: &Project, n: usize) -> Vec<usize> {
    let mut ok: Vec<usize> = (0..p.mics.len()).filter(|&i| !p.mics[i].excluded && p.mics[i].ctf.is_some()).collect();
    if ok.is_empty() {
        ok = (0..p.mics.len()).filter(|&i| !p.mics[i].excluded).collect();
    }
    // Best half by fit resolution, then evenly spaced.
    ok.sort_by(|&a, &b| {
        let r = |i: usize| p.mics[i].ctf.as_ref().map(|c| c.fit_res).unwrap_or(99.0);
        r(a).total_cmp(&r(b))
    });
    let good = &ok[..ok.len().div_ceil(2).max(n.min(ok.len()))];
    let k = n.min(good.len());
    let mut s: Vec<usize> = (0..k).map(|i| good[i * good.len() / k.max(1)]).collect();
    s.sort_unstable();
    s
}

/// Bilinear sample with clamping.
fn at(img: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let a = img[y0 * w + x0] * (1.0 - tx) + img[y0 * w + x1] * tx;
    let b = img[y1 * w + x0] * (1.0 - tx) + img[y1 * w + x1] * tx;
    a * (1.0 - ty) + b * ty
}

/// The blob resampled to CROP² with its diameter at 40% of the crop.
fn crop(img: &[f32], w: usize, h: usize, x: f32, y: f32, r: f32) -> Vec<f32> {
    let scale = 2.0 * r / (0.4 * CROP as f32);
    let c = CROP as f32 / 2.0;
    let mut out = vec![0f32; CROP * CROP];
    for j in 0..CROP {
        for i in 0..CROP {
            out[j * CROP + i] = at(img, w, h, x + (i as f32 - c) * scale, y + (j as f32 - c) * scale);
        }
    }
    out
}

/// Radial profile (to 1.5 radii) and elongation (from second moments of
/// the part above half the peak) of a crop.
fn shape(c: &[f32]) -> (Vec<f32>, f32) {
    let n = CROP as f32;
    let centre = n / 2.0;
    let r = 0.2 * n;
    let (mut sum, mut cnt) = (vec![0f32; PROFILE], vec![0f32; PROFILE]);
    // Background from the corners' ring.
    let ring: Vec<f32> = (0..CROP * CROP)
        .filter(|&k| {
            let (x, y) = ((k % CROP) as f32 - centre, (k / CROP) as f32 - centre);
            (x * x + y * y).sqrt() > 1.6 * r
        })
        .map(|k| c[k])
        .collect();
    let (bg, sd) = robust(&ring);
    let peak = c[CROP / 2 * CROP + CROP / 2].max(bg + sd);
    let (mut m0, mut mx, mut my) = (0f32, 0f32, 0f32);
    let mut pts = vec![];
    for y in 0..CROP {
        for x in 0..CROP {
            let (dx, dy) = (x as f32 - centre, y as f32 - centre);
            let d = (dx * dx + dy * dy).sqrt() / r;
            let v = (c[y * CROP + x] - bg) / sd.max(1e-6);
            let b = (d / 1.5 * PROFILE as f32) as usize;
            if b < PROFILE {
                sum[b] += v;
                cnt[b] += 1.0;
            }
            if d < 1.6 && c[y * CROP + x] - bg > 0.5 * (peak - bg) {
                m0 += 1.0;
                mx += dx;
                my += dy;
                pts.push((dx, dy));
            }
        }
    }
    let prof: Vec<f32> = sum.iter().zip(&cnt).map(|(s, c)| s / c.max(1.0)).collect();
    if m0 < 3.0 {
        return (prof, 1.0);
    }
    let (cx, cy) = (mx / m0, my / m0);
    let (mut sxx, mut syy, mut sxy) = (0f32, 0f32, 0f32);
    for (x, y) in pts {
        sxx += (x - cx) * (x - cx);
        syy += (y - cy) * (y - cy);
        sxy += (x - cx) * (y - cy);
    }
    let (a, b) = (0.5 * (sxx + syy), (0.25 * (sxx - syy).powi(2) + sxy * sxy).sqrt());
    let el = ((a + b) / (a - b).max(1e-3)).sqrt().min(5.0);
    (prof, el)
}

/// Blobs of radius `r` (pixels) on one denoised image (particles
/// positive): local maxima of a difference of Gaussians, as z-scores of
/// that response, at least 0.9 r apart; strongest first.
fn blobs_at(img: &[f32], w: usize, h: usize, r: f64, most: usize) -> Vec<(f32, f32, f32)> {
    let k = 1.25;
    let s = r / std::f64::consts::SQRT_2;
    let a = gauss(img, w, h, s / k);
    let b = gauss(img, w, h, s * k);
    let d: Vec<f32> = a.iter().zip(&b).map(|(a, b)| a - b).collect();
    let (m, sd) = robust(&d);
    let z: Vec<f32> = d.into_iter().map(|v| (v - m) / sd).collect();
    let margin = (1.2 * r).ceil() as usize;
    if 2 * margin + 2 >= w.min(h) {
        return vec![];
    }
    let mut cand = vec![];
    for y in margin..h - margin {
        for x in margin..w - margin {
            let v = z[y * w + x];
            if v < 2.0 {
                continue;
            }
            // 3 × 3 maximum first (cheap), the distance rule below.
            let mut top = true;
            for (dx, dy) in [(-1isize, -1isize), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                if z[(y as isize + dy) as usize * w + (x as isize + dx) as usize] > v {
                    top = false;
                    break;
                }
            }
            if top {
                cand.push((x as f32, y as f32, v));
            }
        }
    }
    cand.sort_by(|a, b| b.2.total_cmp(&a.2));
    let min = (0.9 * r) as f32;
    let mut kept: Vec<(f32, f32, f32)> = vec![];
    for c in cand {
        if kept.iter().all(|k| (k.0 - c.0).powi(2) + (k.1 - c.1).powi(2) > min * min) {
            kept.push(c);
            if kept.len() >= most {
                break;
            }
        }
    }
    kept
}

/// k-means++ (deterministic seed) on rows of `x`; returns assignments.
fn kmeans(x: &[Vec<f32>], k: usize, seed: u64) -> Vec<usize> {
    let n = x.len();
    let d2 = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(p, q)| (p - q) * (p - q)).sum::<f32>();
    let mut rng = Rng(seed);
    let mut cent: Vec<Vec<f32>> = vec![x[rng.below(n)].clone()];
    while cent.len() < k {
        let dist: Vec<f32> = x.iter().map(|v| cent.iter().map(|c| d2(v, c)).fold(f32::MAX, f32::min)).collect();
        let total: f32 = dist.iter().sum();
        if total <= 0.0 {
            break;
        }
        let mut t = rng.unit() as f32 * total;
        let mut pick = n - 1;
        for (i, d) in dist.iter().enumerate() {
            t -= d;
            if t <= 0.0 {
                pick = i;
                break;
            }
        }
        cent.push(x[pick].clone());
    }
    let mut a = vec![0usize; n];
    for _ in 0..50 {
        let mut changed = false;
        for (i, v) in x.iter().enumerate() {
            let best = (0..cent.len()).min_by(|&p, &q| d2(v, &cent[p]).total_cmp(&d2(v, &cent[q]))).unwrap();
            if best != a[i] {
                a[i] = best;
                changed = true;
            }
        }
        for (c, cv) in cent.iter_mut().enumerate() {
            let members: Vec<&Vec<f32>> = x.iter().zip(&a).filter(|(_, &ai)| ai == c).map(|(v, _)| v).collect();
            if members.is_empty() {
                continue;
            }
            for (j, o) in cv.iter_mut().enumerate() {
                *o = members.iter().map(|m| m[j]).sum::<f32>() / members.len() as f32;
            }
        }
        if !changed {
            break;
        }
    }
    a
}

pub struct Config {
    pub micrographs: usize,
    pub kinds: usize,
    pub denoise_steps: usize,
    pub dark: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { micrographs: 5, kinds: 6, denoise_steps: 300, dark: true }
    }
}

/// Denoised working images of a few micrographs (particles positive),
/// their size, pixel size and the backend that trained the denoiser.
pub struct Denoised {
    pub images: Vec<Vec<f32>>,
    pub w: usize,
    pub h: usize,
    pub px: f64,
    pub backend: String,
}

pub fn run(p: &Project, cfg: &Config, outline: Option<Outliner>, progress: Progress) -> Result<Discovery> {
    let d = denoised(p, cfg, progress)?;
    kinds(p, cfg, &d, outline, progress)
}

pub fn denoised(p: &Project, cfg: &Config, progress: Progress) -> Result<Denoised> {
    let ids = sample(p, cfg.micrographs);
    if ids.is_empty() {
        bail!("没有可用的显微图");
    }
    // Working images: about 1000 pixels on the long side.
    let mut imgs = vec![];
    let (mut ww, mut wh, mut kbin) = (0, 0, 1);
    for (n, &i) in ids.iter().enumerate() {
        check_cancel()?;
        let (img, w, h) = read_mic(&p.mics[i].path)?;
        kbin = w.max(h).div_ceil(1024).max(1);
        let (b, bw, bh) = bin(&img, w, h, kbin);
        if n > 0 && (bw, bh) != (ww, wh) {
            continue;
        }
        (ww, wh) = (bw, bh);
        let (m, s) = robust(&b);
        let sign = if cfg.dark { -1.0 } else { 1.0 };
        imgs.push(b.iter().map(|v| (sign * (v - m) / s).clamp(-6.0, 6.0)).collect::<Vec<f32>>());
        progress("读取显微图", n + 1, ids.len());
    }
    let wpx = p.optics.pixel_a * kbin as f64;
    // Noise2Void on these images alone.
    let pairs: Vec<(Vec<f32>, Option<Vec<f32>>)> = imgs.iter().map(|v| (v.clone(), None)).collect();
    let tc = crate::denoise_learn::TrainConfig { steps: cfg.denoise_steps.max(20), ..Default::default() };
    let steps = tc.steps;
    let ((net, mean, std, loss), backend) = crate::classify::on_device(|dev| {
        crate::denoise_learn::train(dev, &pairs, ww, wh, crate::denoise_learn::Mode::N2v, &tc, |s, _, _| progress("去噪训练", s, steps))
    })?;
    let learned = crate::denoise_learn::Learned { net, mode: crate::denoise_learn::Mode::N2v, mean, std, loss, source: String::new(), bin: 1 };
    let dev = crate::classify::device().0;
    let mut dn = vec![];
    for (n, v) in imgs.iter().enumerate() {
        check_cancel()?;
        let d = match crate::ctf_gpu::guarded(|| learned.apply(&dev, v, ww, wh)) {
            Ok(d) => d,
            Err(_) => learned.apply(&burn::tensor::Device::flex(), v, ww, wh),
        };
        dn.push(gauss(&d, ww, wh, 0.7));
        progress("去噪", n + 1, imgs.len());
    }
    Ok(Denoised { images: dn, w: ww, h: wh, px: wpx, backend: backend.into() })
}

/// The kinds of blob on denoised images.
pub fn kinds(p: &Project, cfg: &Config, d: &Denoised, outline: Option<Outliner>, progress: Progress) -> Result<Discovery> {
    let (dn, ww, wh, wpx) = (&d.images, d.w, d.h, d.px);
    // Sizes: diameters from 40 Å (or 8 pixels) to 500 Å (or a fifth of
    // the image), about 1.6× apart. Which one holds whole particles is
    // easy to see in the strips and hard to tell from numbers, so each size
    // gets its card(s) and the user points at theirs.
    let lo = (20.0 / wpx).max(4.0);
    let hi = (250.0 / wpx).min(ww.min(wh) as f64 / 10.0);
    if hi <= lo {
        bail!("像素太大或图像太小，找不到颗粒尺度");
    }
    let n_sizes = (((hi / lo).ln() / 1.6f64.ln()).round() as usize + 1).clamp(1, cfg.kinds.max(1));
    let radii: Vec<f64> = (0..n_sizes).map(|i| if n_sizes == 1 { lo } else { lo * (hi / lo).powf(i as f64 / (n_sizes - 1) as f64) }).collect();
    let mut outlined = outline.is_some();
    let mut kinds = vec![];
    for (si, &r) in radii.iter().enumerate() {
        check_cancel()?;
        let mut all: Vec<Blob> = vec![];
        for (n, img) in dn.iter().enumerate() {
            let most = ((ww * wh) as f64 / (std::f64::consts::PI * r * r) * 0.5).clamp(5.0, MAX_PER_MIC as f64) as usize;
            for q in blobs_at(img, ww, wh, r, most) {
                let c = crop(img, ww, wh, q.0, q.1, r as f32);
                let (prof, el) = shape(&c);
                let mut feat = vec![el.ln() * 2.0];
                feat.extend(prof.iter().map(|v| v * 0.3));
                all.push(Blob { img: n, x: q.0, y: q.1, resp: q.2, feat });
            }
        }
        progress("寻找颗粒", si + 1, radii.len());
        if all.len() < 5 {
            continue;
        }
        // How flat the mean blob is inside: the profile at half the radius
        // over the centre. Maxima of the filter alone (noise, texture, a
        // size that is not the particles') average to the filter's own
        // peaked shape (~0.5 and less); whole particles are flat-topped
        // discs or rings (0.6 and more).
        let np = all[0].feat.len() - 1;
        let mean: Vec<f32> = (0..np).map(|j| all.iter().map(|b| b.feat[j + 1] / 0.3).sum::<f32>() / all.len() as f32).collect();
        let flat = (mean[PROFILE / 3] / mean[0].max(1e-3)) as f64;
        if std::env::var("TMV_SPA_DEBUG").is_ok() {
            eprintln!("{:.0} Å: {} blobs, flatness {flat:.2}", 2.0 * r * wpx, all.len());
        }
        // Size from SAM outlines of the strongest blobs, when it agrees
        // with the scale (else the scale's own).
        let mut order: Vec<usize> = (0..all.len()).collect();
        order.sort_by(|&a, &b| all[b].resp.total_cmp(&all[a].resp));
        let mut diameter = 2.0 * r;
        if let (Some(f), true) = (outline, outlined) {
            let mut sizes = vec![];
            for (n, img) in dn.iter().enumerate() {
                let pts: Vec<[f32; 2]> = order.iter().filter(|&&i| all[i].img == n).take(12).map(|&i| [all[i].x, all[i].y]).collect();
                match f(img, ww, wh, &pts) {
                    Ok(s) => sizes.extend(s.into_iter().flatten().filter(|&dpx| dpx > r && dpx < 4.0 * r)),
                    Err(e) => {
                        eprintln!("discover: outlines failed: {e}");
                        outlined = false;
                        break;
                    }
                }
            }
            if sizes.len() >= 5 {
                sizes.sort_by(f64::total_cmp);
                diameter = sizes[sizes.len() / 2];
            }
        }
        // Two looks when there are enough blobs (e.g. top and side views).
        let feats: Vec<Vec<f32>> = all.iter().map(|b| b.feat.clone()).collect();
        let k = if all.len() >= 60 { 2 } else { 1 };
        let a = kmeans(&feats, k, 0x5eed_0d15 + si as u64);
        for c in 0..k {
            let mut m: Vec<usize> = (0..all.len()).filter(|&i| a[i] == c).collect();
            if k > 1 && (m.len() as f64) < 0.15 * all.len() as f64 {
                continue;
            }
            let med = |mut v: Vec<f32>| -> f32 {
                v.sort_by(f32::total_cmp);
                v[v.len() / 2]
            };
            let elongation = med(m.iter().map(|&i| all[i].feat[0] / 2.0).collect()).exp() as f64;
            let contrast = med(m.iter().map(|&i| all[i].resp).collect()) as f64;
            // Typical members: the middle of the strength range (the
            // strongest are often specks of ice or gold).
            m.sort_by(|&p, &q| all[q].resp.total_cmp(&all[p].resp));
            let from = m.len() / 4;
            m.drain(..from.min(m.len().saturating_sub(8)));
            // Shown a little closer than the features are taken.
            let rr = (0.7 * diameter / 2.0) as f32;
            let tiles: Vec<Vec<f32>> = m.iter().take(8).map(|&i| crop(&dn[all[i].img], ww, wh, all[i].x, all[i].y, rr)).collect();
            let count = a.iter().filter(|&&x| x == c).count();
            kinds.push((Kind { diameter_a: diameter * wpx, count, share: 0.0, elongation, contrast, png: String::new() }, tiles, flat));
        }
    }
    if kinds.is_empty() {
        bail!("没找到颗粒状物体");
    }
    // The particle-like sizes first; at most `cfg.kinds` cards, and the
    // clearly not particle-like ones only to fill up to three.
    kinds.sort_by(|a, b| b.2.total_cmp(&a.2));
    let good = kinds.iter().filter(|k| k.2 >= 0.6).count();
    kinds.truncate(good.max(3).min(cfg.kinds.max(1)));
    let total: usize = kinds.iter().map(|k| k.0.count).sum();
    let mut out = vec![];
    for (i, (mut k, tiles, _)) in kinds.into_iter().enumerate() {
        k.share = k.count as f64 / total as f64;
        k.png = p.path(&format!("kind{}.png", i + 1));
        std::fs::write(&k.png, render::gallery(&tiles, CROP, 8, 72)?)?;
        out.push(k);
    }
    Ok(Discovery { kinds: out, micrographs: dn.len(), working_px: wpx, outlined, backend: d.backend.clone(), chosen: vec![] })
}

/// Correlation of an image with itself rotated by 360°/n, over a ring
/// between 0.15 and 0.9 of `radius` (pixels) around the centre.
pub(crate) fn rot_corr(img: &[f32], b: usize, radius: f64, n: usize) -> f64 {
    let c = b as f64 / 2.0;
    let angles = 360;
    let rings: Vec<f64> = (0..16).map(|i| radius * (0.15 + 0.75 * i as f64 / 15.0)).collect();
    let mut polar = vec![0f64; rings.len() * angles];
    for (ri, &r) in rings.iter().enumerate() {
        for t in 0..angles {
            let a = t as f64 / angles as f64 * std::f64::consts::TAU;
            polar[ri * angles + t] = at(img, b, b, (c + r * a.cos()) as f32, (c + r * a.sin()) as f32) as f64;
        }
        let row = &mut polar[ri * angles..(ri + 1) * angles];
        let m = row.iter().sum::<f64>() / angles as f64;
        row.iter_mut().for_each(|v| *v -= m);
    }
    let shift = angles / n;
    let (mut num, mut den) = (0f64, 0f64);
    for ri in 0..rings.len() {
        let row = &polar[ri * angles..(ri + 1) * angles];
        // Rings weighted by circumference.
        let w = rings[ri];
        for t in 0..angles {
            num += w * row[t] * row[(t + shift) % angles];
            den += w * row[t] * row[t];
        }
    }
    if den <= 0.0 { 0.0 } else { num / den }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymmetryGuess {
    pub symmetry: String,
    /// n-fold correlation of the best top view, and the best 2-fold of the others.
    pub score: f64,
    pub two_fold: f64,
}

/// Point group suggested by class averages (`b` × `b`, particle radius in
/// pixels), from those in `use_classes`. Each average is centred on its
/// mass first. A top view of Cn or Dn correlates with itself turned by
/// 360°/n clearly more than by 360°/(n ± 1); a featureless disc or ring
/// correlates more and more as the angle shrinks, with no such peak. Dn
/// also has side views with a 2-fold axis in the plane.
pub fn symmetry(avgs: &[Vec<f32>], b: usize, radius: f64, use_classes: &[usize]) -> SymmetryGuess {
    let centred: Vec<Vec<f32>> = avgs.iter().map(|a| super::plane::shift(a, b, super::plane::centre_of_mass(a, b))).collect();
    let mut best = (1usize, 0f64, usize::MAX);
    for &c in use_classes {
        let s: Vec<f64> = (0..=13).map(|n| if n < 2 { 1.0 } else { rot_corr(&centred[c], b, radius, n) }).collect();
        for n in 3..=12 {
            let peak = s[n] - s[n - 1].max(s[n + 1]);
            if s[n] >= 0.5 && peak >= 0.1 && s[n] > best.1 {
                best = (n, s[n], c);
            }
        }
    }
    if best.0 < 3 {
        return SymmetryGuess { symmetry: "C1".into(), score: 0.0, two_fold: 0.0 };
    }
    let two = use_classes
        .iter()
        .filter(|&&c| c != best.2)
        .map(|&c| {
            let a = &centred[c];
            let (s2, s3) = (rot_corr(a, b, radius, 2), rot_corr(a, b, radius, 3));
            if s2 - s3 >= 0.2 { s2 } else { 0.0 }
        })
        .fold(0f64, f64::max);
    let symmetry = if two >= 0.6 { format!("D{}", best.0) } else { format!("C{}", best.0) };
    SymmetryGuess { symmetry, score: best.1, two_fold: two }
}

/// Trilinear sample of a b³ map at (x, y, z) voxels (0 outside).
fn at3(v: &[f32], b: usize, x: f64, y: f64, z: f64) -> f32 {
    if x < 0.0 || y < 0.0 || z < 0.0 || x > (b - 1) as f64 || y > (b - 1) as f64 || z > (b - 1) as f64 {
        return 0.0;
    }
    let (x0, y0, z0) = (x.floor() as usize, y.floor() as usize, z.floor() as usize);
    let (x1, y1, z1) = ((x0 + 1).min(b - 1), (y0 + 1).min(b - 1), (z0 + 1).min(b - 1));
    let (tx, ty, tz) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32, (z - z0 as f64) as f32);
    let i = |x: usize, y: usize, z: usize| v[(z * b + y) * b + x];
    let c00 = i(x0, y0, z0) * (1.0 - tx) + i(x1, y0, z0) * tx;
    let c10 = i(x0, y1, z0) * (1.0 - tx) + i(x1, y1, z0) * tx;
    let c01 = i(x0, y0, z1) * (1.0 - tx) + i(x1, y0, z1) * tx;
    let c11 = i(x0, y1, z1) * (1.0 - tx) + i(x1, y1, z1) * tx;
    let c0 = c00 * (1.0 - ty) + c10 * ty;
    let c1 = c01 * (1.0 - ty) + c11 * ty;
    c0 * (1.0 - tz) + c1 * tz
}

/// The map turned by 180° about the axis in the xy plane at `phi` degrees
/// from x, `dz` voxels above the centre (or, with `about_z`, turned by
/// `phi` about z and moved by −dz along z).
fn turned(v: &[f32], b: usize, phi: f64, dz: f64, about_z: bool) -> Vec<f32> {
    let c = b as f64 / 2.0;
    let (s, co) = phi.to_radians().sin_cos();
    let mut out = vec![0f32; b * b * b];
    out.par_chunks_mut(b * b).enumerate().for_each(|(z, plane)| {
        for y in 0..b {
            for x in 0..b {
                let (px, py, pz) = (x as f64 - c, y as f64 - c, z as f64 - c);
                let (qx, qy, qz) = if about_z {
                    // Sample at R(−phi)·p, so the map turns by +phi.
                    (co * px + s * py, -s * px + co * py, pz + dz)
                } else {
                    // R = 2uuᵀ − I with u = (cos, sin, 0): its own inverse.
                    let d = co * px + s * py;
                    (2.0 * d * co - px, 2.0 * d * s - py, 2.0 * dz - pz)
                };
                plane[y * b + x] = at3(v, b, qx + c, qy + c, qz + c);
            }
        }
    });
    out
}

fn corr3(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().map(|&v| v as f64).sum::<f64>() / n, b.iter().map(|&v| v as f64).sum::<f64>() / n);
    let (mut sab, mut saa, mut sbb) = (0f64, 0f64, 0f64);
    for (&x, &y) in a.iter().zip(b) {
        let (x, y) = (x as f64 - ma, y as f64 - mb);
        sab += x * y;
        saa += x * x;
        sbb += y * y;
    }
    sab / (saa * sbb).sqrt().max(1e-30)
}

/// Whether a Cn map (axis z) is in fact Dn: the best 2-fold axis in the
/// xy plane (degrees from x, and its height in voxels: a Cn refinement
/// leaves the particle anywhere along z) if the map turned about it
/// matches itself: correlation ≥ 0.85 and well above the worst angle
/// (by 0.05 and by half of what the worst one lacks; low-pass maps all
/// correlate highly, so the margin is relative).
pub fn dihedral(v: &[f32], b: usize, n: usize) -> Option<(f64, f64, f64)> {
    let span = 180.0 / n.max(1) as f64;
    let reach = (b as f64 / 8.0).round();
    // Height first at a coarse angle step, then the angle at that height.
    let mut best = (0.0, 0.0, f64::MIN);
    let mut dz = -reach;
    while dz <= reach {
        let mut phi = 0.0;
        while phi < span {
            let c = corr3(v, &turned(v, b, phi, dz, false));
            if c > best.2 {
                best = (phi, dz, c);
            }
            phi += 4.0;
        }
        dz += 1.0;
    }
    let dz = best.1;
    let mut scores = vec![];
    let mut phi = 0.0;
    while phi < span {
        scores.push((phi, corr3(v, &turned(v, b, phi, dz, false))));
        phi += 1.0;
    }
    let top = scores.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1))?;
    let worst = scores.iter().map(|s| s.1).fold(f64::MAX, f64::min);
    eprintln!("dihedral test D{n}: best {:.2} at {:.0}°, {dz:+.0} voxels; worst {:.2}", top.1, top.0, worst);
    let gap = top.1 - worst;
    (top.1 >= 0.85 && gap >= 0.05 && gap >= 0.5 * (1.0 - worst)).then_some((top.0, dz, top.1))
}

/// The map turned about z and moved along z so that the 2-fold at `phi`,
/// `dz` lies along x through the centre (the Dn convention of
/// `geom::symmetry`).
pub fn align_dihedral(v: &[f32], b: usize, phi: f64, dz: f64) -> Vec<f32> {
    turned(v, b, -phi, dz, true)
}

/// Correlation of a map with itself turned by `deg` about axis `ax` (0 x, 1 y, 2 z) — a check.
pub fn axis_corr(v: &[f32], b: usize, ax: usize, deg: f64) -> f64 {
    let c = b as f64 / 2.0;
    let (s, co) = deg.to_radians().sin_cos();
    let mut out = vec![0f32; b * b * b];
    for z in 0..b {
        for y in 0..b {
            for x in 0..b {
                let p = [x as f64 - c, y as f64 - c, z as f64 - c];
                let (i, j) = match ax { 0 => (1, 2), 1 => (2, 0), _ => (0, 1) };
                let mut q = p;
                q[i] = co * p[i] + s * p[j];
                q[j] = -s * p[i] + co * p[j];
                out[(z * b + y) * b + x] = at3(v, b, q[0] + c, q[1] + c, q[2] + c);
            }
        }
    }
    corr3(v, &out)
}

type M3 = [[f64; 3]; 3];

/// out(p) = v(m·p + t) about the box centre (trilinear).
fn sample_by(v: &[f32], b: usize, m: &M3, t: [f64; 3]) -> Vec<f32> {
    let c = b as f64 / 2.0;
    let mut out = vec![0f32; b * b * b];
    out.par_chunks_mut(b * b).enumerate().for_each(|(z, plane)| {
        for y in 0..b {
            for x in 0..b {
                let p = [x as f64 - c, y as f64 - c, z as f64 - c];
                let q: Vec<f64> = (0..3).map(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + t[i] + c).collect();
                plane[y * b + x] = at3(v, b, q[0], q[1], q[2]);
            }
        }
    });
    out
}

/// Rotation by `deg` about the unit axis `a` (Rodrigues).
fn rot(a: [f64; 3], deg: f64) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    let t = 1.0 - c;
    let [x, y, z] = a;
    [[t * x * x + c, t * x * y - s * z, t * x * z + s * y], [t * x * y + s * z, t * y * y + c, t * y * z - s * x], [t * x * z - s * y, t * y * z + s * x, t * z * z + c]]
}

fn transpose(m: &M3) -> M3 {
    [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]]
}

/// The map moved so that its centre of (positive) mass is at the box centre.
pub fn centred3(v: &[f32], b: usize) -> Vec<f32> {
    let c = b as f64 / 2.0;
    let (mut s, mut m) = ([0f64; 3], 0f64);
    for z in 0..b {
        for y in 0..b {
            for x in 0..b {
                let w = v[(z * b + y) * b + x].max(0.0) as f64;
                s[0] += w * (x as f64 - c);
                s[1] += w * (y as f64 - c);
                s[2] += w * (z as f64 - c);
                m += w;
            }
        }
    }
    if m <= 0.0 {
        return v.to_vec();
    }
    let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    sample_by(v, b, &id, [s[0] / m, s[1] / m, s[2] / m])
}

/// The n-fold axis of a C1 map (centred first): the direction whose
/// 360°/n turn matches the map best, searched on a 32³ copy over the
/// hemisphere, then refined at full size. Returns (axis, correlation,
/// correlation of the half-way turn) or None when nothing matches.
pub fn find_axis(v: &[f32], b: usize, n: usize) -> Option<([f64; 3], f64, f64)> {
    let small = if b > 32 { super::fourier::resample3(v, b, 32) } else { v.to_vec() };
    let bs = (small.len() as f64).cbrt().round() as usize;
    let step = 360.0 / n as f64;
    // Fibonacci points on the upper hemisphere, about 3° apart.
    let count = 2400;
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    let dirs: Vec<[f64; 3]> = (0..count)
        .map(|i| {
            let z = 1.0 - i as f64 / count as f64;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let t = golden * i as f64;
            [r * t.cos(), r * t.sin(), z]
        })
        .collect();
    let score = |vol: &[f32], bb: usize, a: [f64; 3], deg: f64| corr3(vol, &sample_by(vol, bb, &rot(a, deg), [0.0; 3]));
    let mut coarse: Vec<([f64; 3], f64)> = dirs.iter().map(|&a| (a, score(&small, bs, a, step))).collect();
    coarse.sort_by(|a, b| b.1.total_cmp(&a.1));
    // Refine the best few at full size on a ±3° grid.
    let mut best = ([0.0, 0.0, 1.0], f64::MIN);
    for &(a, _) in coarse.iter().take(4) {
        let (mut cur, mut cs) = (a, score(v, b, a, step));
        for d in [2.0f64, 1.0, 0.5] {
            loop {
                let mut moved = false;
                // Two directions perpendicular to the axis.
                let u = if cur[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
                let e1 = norm(cross(cur, u));
                let e2 = cross(cur, e1);
                for (p, q) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                    let t = d.to_radians();
                    let cand = norm([cur[0] + t * (p * e1[0] + q * e2[0]), cur[1] + t * (p * e1[1] + q * e2[1]), cur[2] + t * (p * e1[2] + q * e2[2])]);
                    let s = score(v, b, cand, step);
                    if s > cs {
                        (cur, cs) = (cand, s);
                        moved = true;
                    }
                }
                if !moved {
                    break;
                }
            }
        }
        if cs > best.1 {
            best = (cur, cs);
        }
    }
    let half = score(v, b, best.0, step / 2.0);
    eprintln!("axis search C{n}: {:.2} (half turn {:.2}) along {:.2?}", best.1, half, best.0);
    // The n comes from the 2D classes; at low resolution the n-fold turn
    // hardly beats a half-way one (the map is mostly a cylinder), but the
    // best direction is still the axis. A poor match means no axis.
    (best.1 >= 0.75).then_some((best.0, best.1, half))
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn norm(a: [f64; 3]) -> [f64; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

/// The map turned so that `axis` points along z.
pub fn align_axis(v: &[f32], b: usize, axis: [f64; 3]) -> Vec<f32> {
    let z = [0.0, 0.0, 1.0];
    let k = cross(axis, z);
    let s = (k[0] * k[0] + k[1] * k[1] + k[2] * k[2]).sqrt();
    if s < 1e-9 {
        return if axis[2] > 0.0 { v.to_vec() } else { sample_by(v, b, &rot([1.0, 0.0, 0.0], 180.0), [0.0; 3]) };
    }
    let angle = s.atan2(axis[2]).to_degrees();
    // R takes axis to z; out(p) = v(Rᵀ p).
    let r = rot(norm(k), angle);
    sample_by(v, b, &transpose(&r), [0.0; 3])
}
