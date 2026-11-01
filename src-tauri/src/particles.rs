//! Pro particle statistics: finds the particles of one image (nanoparticles
//! on a support, precipitates, pores, cells) and measures each of them.
//!
//! The image is smoothed, optionally flattened (a wide Gaussian of it
//! subtracted) and thresholded (Otsu's threshold unless one is given). The
//! particles are the smaller of the two classes unless the caller says
//! whether they are darker or brighter. Holes are filled, and touching
//! particles are split by a watershed on the distance to the background: a
//! peak of the distance map starts a particle only when it stands more than
//! `split_h` pixels above the saddle that joins it to a higher one.
//!
//! Per particle: area, perimeter (Crofton estimate over four directions),
//! area-equivalent diameter, Feret maximum and minimum (rotating calipers on
//! the convex hull), the fitted ellipse, circularity 4πA/P², solidity,
//! centroid, mean intensity and the outline as a polygon on pixel corners.
//! Lengths are in pixels; the panel converts them with the pixel size.

use std::sync::Arc;

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::image::OpenImage;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Req {
    pub token: u64,
    #[serde(default)]
    pub c: usize,
    #[serde(default)]
    pub z: usize,
    #[serde(default)]
    pub t: usize,
    /// [x, y, w, h] in image pixels; the whole image when absent.
    #[serde(default)]
    pub region: Option<[f64; 4]>,
    /// Particles darker than the background; the minority class when absent.
    #[serde(default)]
    pub dark: Option<bool>,
    /// Threshold on the smoothed (and flattened) image; Otsu's when absent.
    #[serde(default)]
    pub threshold: Option<f32>,
    /// Gaussian smoothing before thresholding, pixels.
    #[serde(default = "one")]
    pub smooth: f32,
    /// σ of the Gaussian background to subtract, pixels; 0 = none.
    #[serde(default)]
    pub background: f32,
    #[serde(default = "yes")]
    pub fill_holes: bool,
    #[serde(default = "yes")]
    pub split: bool,
    /// Minimum depth of a distance-map peak that starts its own particle, px.
    #[serde(default = "one")]
    pub split_h: f32,
    /// Particles smaller than this (pixels) are dropped.
    #[serde(default = "ten")]
    pub min_area: usize,
    /// Drop particles that touch the edge of the analysed area.
    #[serde(default = "yes")]
    pub exclude_edge: bool,
}

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}
fn ten() -> usize {
    10
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Particle {
    pub id: usize,
    /// Centroid, image pixels.
    pub x: f64,
    pub y: f64,
    pub area: f64,
    pub perimeter: f64,
    /// Diameter of the disk of the same area.
    pub diameter: f64,
    pub feret_max: f64,
    pub feret_min: f64,
    /// Direction of the Feret maximum, degrees (image x axis, y down).
    pub feret_angle: f64,
    /// Axes of the ellipse with the same second moments.
    pub major: f64,
    pub minor: f64,
    pub angle: f64,
    pub circularity: f64,
    pub aspect: f64,
    pub solidity: f64,
    pub mean: f64,
    /// [x, y, w, h], image pixels.
    pub bbox: [usize; 4],
    pub touches_edge: bool,
    /// Outer outline on pixel corners, image pixels, as x0, y0, x1, y1, …
    pub outline: Vec<f32>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Result2 {
    /// Analysed area [x, y, w, h].
    pub region: [usize; 4],
    pub dark: bool,
    pub threshold: f32,
    /// Range of the smoothed image, for the threshold slider.
    pub min: f32,
    pub max: f32,
    /// Histogram of the smoothed image over [min, max], 256 bins.
    pub hist: Vec<u32>,
    /// Pixels classed as particle (after hole filling), before size filters.
    pub foreground: usize,
    pub particles: Vec<Particle>,
    /// Particles dropped as too small / touching the edge.
    pub dropped_small: usize,
    pub dropped_edge: usize,
    pub ms: f64,
}

pub fn analyze(img: &Arc<OpenImage>, req: &Req) -> Result<Result2> {
    let start = std::time::Instant::now();
    let info = img.info();
    let (page, sample) = info.locate(req.c, req.z, req.t);
    let planes = img.page(page)?;
    let p = &planes[sample.min(planes.len() - 1)];
    let [x0, y0, w, h] = match req.region {
        Some(r) => {
            let x0 = r[0].max(0.0).floor() as usize;
            let y0 = r[1].max(0.0).floor() as usize;
            let x1 = ((r[0] + r[2]).ceil() as usize).min(p.w);
            let y1 = ((r[1] + r[3]).ceil() as usize).min(p.h);
            if x1 <= x0 + 8 || y1 <= y0 + 8 {
                bail!("区域太小");
            }
            [x0, y0, x1 - x0, y1 - y0]
        }
        None => [0, 0, p.w, p.h],
    };
    let raw: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| p.data.get_f64((y0 + i / w) * p.w + x0 + i % w) as f32)
        .collect();
    let mut out = run(&raw, w, h, req)?;
    for q in &mut out.particles {
        q.x += x0 as f64;
        q.y += y0 as f64;
        q.bbox[0] += x0;
        q.bbox[1] += y0;
        for (k, v) in q.outline.iter_mut().enumerate() {
            *v += if k % 2 == 0 { x0 } else { y0 } as f32;
        }
    }
    out.region = [x0, y0, w, h];
    out.ms = start.elapsed().as_secs_f64() * 1e3;
    Ok(out)
}

/// The whole analysis on a w × h image (region coordinates).
pub fn run(raw: &[f32], w: usize, h: usize, req: &Req) -> Result<Result2> {
    if raw.iter().any(|v| !v.is_finite()) {
        bail!("图里有 NaN 或无穷大的值");
    }
    let mut v = raw.to_vec();
    if req.smooth > 0.0 {
        gaussian(&mut v, w, h, req.smooth);
    }
    if req.background > 0.0 {
        let mut bg = v.clone();
        gaussian(&mut bg, w, h, req.background);
        v.par_iter_mut().zip(&bg).for_each(|(a, b)| *a -= b);
    }
    let (min, max) = v
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    if !(max > min) {
        bail!("图是平的，找不到颗粒");
    }
    let hist = histogram(&v, min, max, 256);
    let (threshold, dark) = match req.threshold {
        Some(t) => {
            let below = v.iter().filter(|&&x| x < t).count();
            (t, req.dark.unwrap_or(below * 2 < v.len()))
        }
        None => auto_threshold(&v, &hist, min, max, req.dark),
    };
    let mut mask: Vec<bool> = v
        .par_iter()
        .map(|&x| if dark { x < threshold } else { x > threshold })
        .collect();
    if req.fill_holes {
        fill_holes(&mut mask, w, h);
    }
    let foreground = mask.iter().filter(|&&m| m).count();
    let labels = if req.split {
        let d = edt(&mask, w, h);
        watershed(&mask, &d, w, h, req.split_h.max(0.0))
    } else {
        watershed(&mask, &vec![0.0; w * h], w, h, f32::INFINITY)
    };
    let (mut particles, dropped_small, dropped_edge) =
        measure(&labels, raw, w, h, req.min_area.max(1), req.exclude_edge);
    for (k, q) in particles.iter_mut().enumerate() {
        q.id = k + 1;
    }
    Ok(Result2 {
        region: [0, 0, w, h],
        dark,
        threshold,
        min,
        max,
        hist,
        foreground,
        particles,
        dropped_small,
        dropped_edge,
        ms: 0.0,
    })
}

/// Separable Gaussian blur with edge clamping; wide ones as three box
/// blurs, whose cost does not grow with σ.
pub fn gaussian(v: &mut [f32], w: usize, h: usize, sigma: f32) {
    if sigma > 6.0 {
        // Box widths whose three passes have variance σ² (Kovesi).
        let s2 = (sigma * sigma) as f64;
        let ideal = (4.0 * s2 + 1.0).sqrt();
        let mut wl = ideal.floor() as i64;
        if wl % 2 == 0 {
            wl -= 1;
        }
        let m = ((12.0 * s2 - 3.0 * (wl * wl) as f64 - 12.0 * wl as f64 - 9.0)
            / (-4.0 * wl as f64 - 4.0))
            .round() as i64;
        for pass in 0..3 {
            let r = ((if pass < m { wl } else { wl + 2 }) / 2) as usize;
            box_blur(v, w, h, r);
        }
        return;
    }
    let r = (3.0 * sigma).ceil().max(1.0) as i64;
    let k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let s: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|x| x / s).collect();
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &v[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            let mut a = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let xx = (x as i64 + j as i64 - r).clamp(0, w as i64 - 1) as usize;
                a += kv * src[xx];
            }
            *o = a;
        }
    });
    v.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut a = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let yy = (y as i64 + j as i64 - r).clamp(0, h as i64 - 1) as usize;
                a += kv * tmp[yy * w + x];
            }
            *o = a;
        }
    });
}

/// Box blur of radius r along rows then columns, edges clamped.
fn box_blur(v: &mut [f32], w: usize, h: usize, r: usize) {
    fn line(src: &[f32], out: &mut [f32], r: usize) {
        let n = src.len() as i64;
        let at = |i: i64| src[i.clamp(0, n - 1) as usize] as f64;
        let mut acc: f64 = (-(r as i64)..=r as i64).map(at).sum();
        let k = 1.0 / (2 * r + 1) as f64;
        for i in 0..n {
            out[i as usize] = (acc * k) as f32;
            acc += at(i + r as i64 + 1) - at(i - r as i64);
        }
    }
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w)
        .zip(v.par_chunks(w))
        .for_each(|(o, s)| line(s, o, r));
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let col: Vec<f32> = (0..h).map(|y| tmp[y * w + x]).collect();
            let mut o = vec![0f32; h];
            line(&col, &mut o, r);
            o
        })
        .collect();
    for (x, col) in cols.iter().enumerate() {
        for (y, &c) in col.iter().enumerate() {
            v[y * w + x] = c;
        }
    }
}

fn histogram(v: &[f32], min: f32, max: f32, n: usize) -> Vec<u32> {
    let mut hist = vec![0u32; n];
    let s = n as f32 / (max - min);
    for &x in v {
        hist[(((x - min) * s) as usize).min(n - 1)] += 1;
    }
    hist
}

/// Otsu's threshold, unless it falls inside the noise of the background
/// (within one robust σ of the median: few particles on a flat
/// background, where Otsu splits the noise), then the median ± 3σ on the
/// particles' side. Without a given polarity the particles are the smaller
/// class, or the larger tail beyond 3σ when falling back.
fn auto_threshold(v: &[f32], hist: &[u32], min: f32, max: f32, dark: Option<bool>) -> (f32, bool) {
    let step = (v.len() / 1_000_000).max(1);
    let mut s: Vec<f32> = v.iter().step_by(step).copied().collect();
    let mid = s.len() / 2;
    let med = *s.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
    let mut dev: Vec<f32> = s.iter().map(|x| (x - med).abs()).collect();
    let sigma = 1.4826 * *dev.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
    let t = otsu(hist, min, max);
    if sigma > 0.0 && (t - med).abs() >= sigma {
        let below = s.iter().filter(|&&x| x < t).count();
        return (t, dark.unwrap_or(below * 2 < s.len()));
    }
    let lo = s.iter().filter(|&&x| x < med - 3.0 * sigma).count();
    let hi = s.iter().filter(|&&x| x > med + 3.0 * sigma).count();
    let dark = dark.unwrap_or(lo > hi);
    (
        if dark {
            med - 3.0 * sigma
        } else {
            med + 3.0 * sigma
        },
        dark,
    )
}

/// Otsu's threshold: the bin edge that maximises the between-class variance.
pub fn otsu(hist: &[u32], min: f32, max: f32) -> f32 {
    let n = hist.len();
    let total: f64 = hist.iter().map(|&c| c as f64).sum();
    let sum: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, &c)| i as f64 * c as f64)
        .sum();
    let (mut w0, mut s0, mut best, mut at) = (0.0, 0.0, -1.0, n / 2);
    for (i, &c) in hist.iter().enumerate().take(n - 1) {
        w0 += c as f64;
        s0 += i as f64 * c as f64;
        let w1 = total - w0;
        if w0 == 0.0 || w1 == 0.0 {
            continue;
        }
        let d = s0 / w0 - (sum - s0) / w1;
        let between = w0 * w1 * d * d;
        if between > best {
            best = between;
            at = i + 1;
        }
    }
    min + (max - min) * at as f32 / n as f32
}

/// Background pixels not 4-connected to the border become foreground.
fn fill_holes(mask: &mut [bool], w: usize, h: usize) {
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w {
        stack.push(x);
        stack.push((h - 1) * w + x);
    }
    for y in 0..h {
        stack.push(y * w);
        stack.push(y * w + w - 1);
    }
    while let Some(i) = stack.pop() {
        if mask[i] || outside[i] {
            continue;
        }
        outside[i] = true;
        let (x, y) = (i % w, i / w);
        if x > 0 {
            stack.push(i - 1);
        }
        if x + 1 < w {
            stack.push(i + 1);
        }
        if y > 0 {
            stack.push(i - w);
        }
        if y + 1 < h {
            stack.push(i + w);
        }
    }
    mask.par_iter_mut()
        .zip(&outside)
        .for_each(|(m, &o)| *m = !o);
}

/// Exact Euclidean distance of each foreground pixel to the nearest
/// background pixel (Felzenszwalb–Huttenlocher lower envelopes, rows then
/// columns). Pixels beyond the image edge count as foreground, so particles
/// cut by the edge are not split along it.
pub fn edt(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f64 = 1e20;
    let mut f = vec![0f64; w * h];
    f.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src: Vec<f64> = (0..w)
            .map(|x| if mask[y * w + x] { INF } else { 0.0 })
            .collect();
        row.copy_from_slice(&envelope(&src));
    });
    let cols: Vec<Vec<f64>> = (0..w)
        .into_par_iter()
        .map(|x| envelope(&(0..h).map(|y| f[y * w + x]).collect::<Vec<_>>()))
        .collect();
    let mut out = vec![0f32; w * h];
    for (x, col) in cols.iter().enumerate() {
        for (y, &d) in col.iter().enumerate() {
            out[y * w + x] = if d >= INF / 2.0 {
                (w.max(h)) as f32
            } else {
                d.sqrt() as f32
            };
        }
    }
    out
}

/// 1D squared distance transform of the sampled function f.
fn envelope(f: &[f64]) -> Vec<f64> {
    let n = f.len();
    let mut v = vec![0usize; n];
    let mut z = vec![0f64; n + 1];
    let mut k = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    let sect = |q: usize, p: usize| {
        ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q as f64 - p as f64))
    };
    for q in 1..n {
        let mut s = sect(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = sect(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    let mut j = 0;
    (0..n)
        .map(|q| {
            while z[j + 1] < q as f64 {
                j += 1;
            }
            (q as f64 - v[j] as f64).powi(2) + f[v[j]]
        })
        .collect()
}

/// Watershed on `height` inside `mask` by flooding from the top: pixels
/// join their neighbours' regions in falling order, and two regions that
/// meet merge unless both peaks stand at least `hmin` above the meeting
/// level. Returns a label per pixel (0 = background), labels 1..=n.
pub fn watershed(mask: &[bool], height: &[f32], w: usize, h: usize, hmin: f32) -> Vec<u32> {
    let mut order: Vec<u32> = (0..(w * h) as u32).filter(|&i| mask[i as usize]).collect();
    order.par_sort_unstable_by(|&a, &b| {
        height[b as usize]
            .total_cmp(&height[a as usize])
            .then(a.cmp(&b))
    });
    const NONE: u32 = u32::MAX;
    let mut parent: Vec<u32> = vec![NONE; w * h];
    let mut peak: Vec<f32> = vec![0.0; w * h];
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            let g = parent[parent[i as usize] as usize];
            parent[i as usize] = g;
            i = g;
        }
        i
    }
    let mut roots: Vec<u32> = Vec::with_capacity(8);
    for &i in &order {
        let (x, y) = ((i as usize % w) as i64, (i as usize / w) as i64);
        let lv = height[i as usize];
        roots.clear();
        for dy in -1..=1i64 {
            for dx in -1..=1i64 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let (xx, yy) = (x + dx, y + dy);
                if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 {
                    continue;
                }
                let j = (yy as usize * w + xx as usize) as u32;
                if parent[j as usize] == NONE {
                    continue;
                }
                let r = root(&mut parent, j);
                if !roots.contains(&r) {
                    roots.push(r);
                }
            }
        }
        if roots.is_empty() {
            parent[i as usize] = i;
            peak[i as usize] = lv;
            continue;
        }
        // Highest peak first; lower ones merge into it when shallow.
        roots.sort_by(|&a, &b| {
            peak[b as usize]
                .total_cmp(&peak[a as usize])
                .then(a.cmp(&b))
        });
        let top = roots[0];
        for &r in &roots[1..] {
            if peak[r as usize] - lv < hmin {
                parent[r as usize] = top;
            }
        }
        parent[i as usize] = top;
    }
    // Final labels in raster order of first appearance.
    let mut map = std::collections::HashMap::new();
    let mut labels = vec![0u32; w * h];
    for i in 0..w * h {
        if parent[i] == NONE {
            continue;
        }
        let r = root(&mut parent, i as u32);
        let n = map.len() as u32 + 1;
        labels[i] = *map.entry(r).or_insert(n);
    }
    labels
}

#[derive(Default, Clone)]
struct Acc {
    n: usize,
    sx: f64,
    sy: f64,
    sxx: f64,
    syy: f64,
    sxy: f64,
    sv: f64,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    edge: bool,
    // Boundary crossings along 0°, 90°, 45°, 135°.
    c: [usize; 4],
    first: usize,
}

fn measure(
    labels: &[u32],
    raw: &[f32],
    w: usize,
    h: usize,
    min_area: usize,
    exclude_edge: bool,
) -> (Vec<Particle>, usize, usize) {
    let n = labels.iter().copied().max().unwrap_or(0) as usize;
    let mut acc = vec![
        Acc {
            x0: usize::MAX,
            y0: usize::MAX,
            first: usize::MAX,
            ..Default::default()
        };
        n + 1
    ];
    let at = |x: i64, y: i64| -> u32 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            0
        } else {
            labels[y as usize * w + x as usize]
        }
    };
    for y in 0..h {
        for x in 0..w {
            let l = labels[y * w + x];
            if l == 0 {
                continue;
            }
            let a = &mut acc[l as usize];
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            a.n += 1;
            a.sx += fx;
            a.sy += fy;
            a.sxx += fx * fx;
            a.syy += fy * fy;
            a.sxy += fx * fy;
            a.sv += raw[y * w + x] as f64;
            a.x0 = a.x0.min(x);
            a.y0 = a.y0.min(y);
            a.x1 = a.x1.max(x);
            a.y1 = a.y1.max(y);
            if a.first == usize::MAX {
                a.first = y * w + x;
            }
            if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                a.edge = true;
            }
            let (xi, yi) = (x as i64, y as i64);
            // Each crossing counted once at the particle pixel on either side.
            for (k, (dx, dy)) in [(1, 0), (0, 1), (1, 1), (-1, 1)].iter().enumerate() {
                if at(xi + dx, yi + dy) != l {
                    a.c[k] += 1;
                }
                if at(xi - dx, yi - dy) != l {
                    a.c[k] += 1;
                }
            }
        }
    }
    let (mut small, mut edge) = (0, 0);
    let keep: Vec<usize> = (1..=n)
        .filter(|&l| {
            let a = &acc[l];
            if a.n < min_area {
                small += 1;
                return false;
            }
            if exclude_edge && a.edge {
                edge += 1;
                return false;
            }
            true
        })
        .collect();
    let parts = keep
        .par_iter()
        .map(|&l| {
            let a = &acc[l];
            let nf = a.n as f64;
            let (cx, cy) = (a.sx / nf, a.sy / nf);
            // Second central moments plus the pixel's own 1/12.
            let mxx = a.sxx / nf - cx * cx + 1.0 / 12.0;
            let myy = a.syy / nf - cy * cy + 1.0 / 12.0;
            let mxy = a.sxy / nf - cx * cy;
            let tr = mxx + myy;
            let det = (mxx * myy - mxy * mxy).max(0.0);
            let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
            let (l1, l2) = (tr / 2.0 + disc, (tr / 2.0 - disc).max(0.0));
            let angle = 0.5 * (2.0 * mxy).atan2(mxx - myy);
            let perimeter = std::f64::consts::PI / 8.0
                * (a.c[0] as f64 + a.c[1] as f64 + (a.c[2] + a.c[3]) as f64 / 2f64.sqrt());
            let outline = trace(labels, w, h, l as u32, a.first, a.n);
            let hull = convex_hull(&outline);
            let hull_area = polygon_area(&hull);
            let (fmax, fang, fmin) = calipers(&hull);
            let area = nf;
            Particle {
                id: 0,
                x: cx,
                y: cy,
                area,
                perimeter,
                diameter: (4.0 * area / std::f64::consts::PI).sqrt(),
                feret_max: fmax,
                feret_min: fmin,
                feret_angle: fang.to_degrees(),
                major: 4.0 * l1.sqrt(),
                minor: 4.0 * l2.sqrt(),
                angle: angle.to_degrees(),
                circularity: (4.0 * std::f64::consts::PI * area / (perimeter * perimeter)).min(1.0),
                aspect: if fmin > 0.0 { fmax / fmin } else { 1.0 },
                solidity: if hull_area > 0.0 {
                    (area / hull_area).min(1.0)
                } else {
                    1.0
                },
                mean: a.sv / nf,
                bbox: [a.x0, a.y0, a.x1 + 1 - a.x0, a.y1 + 1 - a.y0],
                touches_edge: a.edge,
                outline: outline
                    .iter()
                    .flat_map(|p| [p.0 as f32, p.1 as f32])
                    .collect(),
            }
        })
        .collect();
    (parts, small, edge)
}

/// Outer outline of label `l` on pixel corners, starting at the top-left
/// corner of its first pixel in raster order, walking with the particle on
/// the right. Diagonal neighbours stay in one outline. Straight runs are
/// collapsed to their end points.
fn trace(labels: &[u32], w: usize, h: usize, l: u32, first: usize, area: usize) -> Vec<(f64, f64)> {
    let inside = |x: i64, y: i64| {
        x >= 0 && y >= 0 && x < w as i64 && y < h as i64 && labels[y as usize * w + x as usize] == l
    };
    // Directions: 0 = +x, 1 = +y, 2 = −x, 3 = −y (y down). At corner (x, y)
    // heading d, the pixels right and left of the next edge decide the turn;
    // heading +x they are pixel (x, y) below and (x, y − 1) above.
    let (sx, sy) = ((first % w) as i64, (first / w) as i64);
    let (mut x, mut y, mut d) = (sx, sy, 0usize);
    let mut pts: Vec<(f64, f64)> = vec![(x as f64, y as f64)];
    // Each step moves along or turns at one of at most 4 edges per pixel.
    for step in 0..8 * area + 16 {
        if step > 0 && x == sx && y == sy && d == 0 {
            break;
        }
        let (ar, al) = match d {
            0 => ((x, y), (x, y - 1)),
            1 => ((x - 1, y), (x, y)),
            2 => ((x - 1, y - 1), (x - 1, y)),
            _ => ((x, y - 1), (x - 1, y - 1)),
        };
        let nd = if inside(al.0, al.1) {
            (d + 3) % 4 // turn left: keeps diagonal neighbours in one outline
        } else if inside(ar.0, ar.1) {
            d
        } else {
            (d + 1) % 4
        };
        if nd != d {
            d = nd;
            pts.push((x as f64, y as f64));
            continue;
        }
        match d {
            0 => x += 1,
            1 => y += 1,
            2 => x -= 1,
            _ => y -= 1,
        }
    }
    // Drop the repeated start and points in the middle of straight runs.
    pts.dedup();
    if pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 {
        return pts;
    }
    (0..n)
        .filter(|&i| {
            let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0) != 0.0
        })
        .map(|i| pts[i])
        .collect()
}

fn cross(o: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Andrew's monotone chain, counter-clockwise in a y-up sense.
pub fn convex_hull(pts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut p = pts.to_vec();
    p.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let mut hull: Vec<(f64, f64)> = Vec::with_capacity(2 * p.len());
    for pass in 0..2 {
        let base = hull.len();
        let it: Box<dyn Iterator<Item = &(f64, f64)>> = if pass == 0 {
            Box::new(p.iter())
        } else {
            Box::new(p.iter().rev())
        };
        for &q in it {
            while hull.len() >= base + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], q) <= 0.0
            {
                hull.pop();
            }
            hull.push(q);
        }
        hull.pop();
    }
    hull
}

fn polygon_area(p: &[(f64, f64)]) -> f64 {
    let n = p.len();
    (0..n)
        .map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1)
        .sum::<f64>()
        .abs()
        / 2.0
}

/// Feret maximum (with its direction) and minimum width of a convex polygon.
pub fn calipers(hull: &[(f64, f64)]) -> (f64, f64, f64) {
    let n = hull.len();
    if n < 2 {
        return (0.0, 0.0, 0.0);
    }
    let (mut dmax, mut ang) = (0.0, 0.0);
    for i in 0..n {
        for j in i + 1..n {
            let (dx, dy) = (hull[j].0 - hull[i].0, hull[j].1 - hull[i].1);
            let d = dx.hypot(dy);
            if d > dmax {
                dmax = d;
                ang = dy.atan2(dx);
            }
        }
    }
    if ang < 0.0 {
        ang += std::f64::consts::PI;
    }
    if n < 3 {
        return (dmax, ang, 0.0);
    }
    // The minimum width is reached with one side of the hull on a caliper.
    let mut dmin = f64::MAX;
    for i in 0..n {
        let (a, b) = (hull[i], hull[(i + 1) % n]);
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        if len == 0.0 {
            continue;
        }
        let far = hull
            .iter()
            .map(|&q| cross(a, b, q).abs() / len)
            .fold(0.0, f64::max);
        dmin = dmin.min(far);
    }
    (dmax, ang, dmin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> Req {
        Req {
            token: 0,
            c: 0,
            z: 0,
            t: 0,
            region: None,
            dark: None,
            threshold: None,
            smooth: 1.0,
            background: 0.0,
            fill_holes: true,
            split: true,
            split_h: 1.0,
            min_area: 10,
            exclude_edge: true,
        }
    }

    /// Dark disks on a bright noisy support.
    fn disks(w: usize, h: usize, d: &[(f64, f64, f64)]) -> Vec<f32> {
        let mut s = 12345u64;
        let mut noise = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as f64 / (1u64 << 31) as f64 - 0.5) as f32
        };
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
                let inside = d.iter().any(|&(cx, cy, r)| (x - cx).hypot(y - cy) < r);
                (if inside { 40.0 } else { 100.0 }) + 10.0 * noise()
            })
            .collect()
    }

    #[test]
    fn measures_separate_disks() {
        let d = [
            (40.0, 40.0, 10.0),
            (100.0, 50.0, 15.0),
            (60.0, 110.0, 8.0),
            (150.0, 140.0, 20.0),
        ];
        let img = disks(200, 180, &d);
        let r = run(&img, 200, 180, &req()).unwrap();
        assert!(r.dark);
        assert_eq!(
            r.particles.len(),
            4,
            "{:?}",
            r.particles.iter().map(|p| p.diameter).collect::<Vec<_>>()
        );
        for p in &r.particles {
            let (_, _, rad) = d
                .iter()
                .copied()
                .min_by(|a, b| {
                    (a.0 - p.x)
                        .hypot(a.1 - p.y)
                        .total_cmp(&(b.0 - p.x).hypot(b.1 - p.y))
                })
                .unwrap();
            assert!(
                (p.diameter - 2.0 * rad).abs() < 1.0,
                "diameter {} vs {}",
                p.diameter,
                2.0 * rad
            );
            assert!(
                (p.feret_max - 2.0 * rad).abs() < 1.5,
                "feret {} vs {}",
                p.feret_max,
                2.0 * rad
            );
            assert!(
                (p.feret_min - 2.0 * rad).abs() < 1.5,
                "feret min {}",
                p.feret_min
            );
            assert!(
                (p.perimeter - 2.0 * std::f64::consts::PI * rad).abs()
                    / (2.0 * std::f64::consts::PI * rad)
                    < 0.06,
                "perimeter {} r {}",
                p.perimeter,
                rad
            );
            assert!(p.circularity > 0.88, "circularity {}", p.circularity);
            assert!(p.solidity > 0.9);
            assert!(p.outline.len() >= 16);
        }
    }

    #[test]
    fn splits_touching_disks() {
        // Two overlapping disks (centres 22 px apart, radius 13) and one alone.
        let d = [(50.0, 60.0, 13.0), (72.0, 60.0, 13.0), (60.0, 120.0, 12.0)];
        let img = disks(140, 160, &d);
        let r = run(&img, 140, 160, &req()).unwrap();
        assert_eq!(r.particles.len(), 3);
        let mut no = req();
        no.split = false;
        assert_eq!(run(&img, 140, 160, &no).unwrap().particles.len(), 2);
    }

    #[test]
    fn edge_and_size_filters() {
        let d = [(5.0, 50.0, 10.0), (60.0, 50.0, 1.5), (60.0, 20.0, 9.0)];
        let img = disks(100, 100, &d);
        let mut q = req();
        q.smooth = 0.0;
        q.dark = Some(true);
        q.threshold = Some(70.0);
        let r = run(&img, 100, 100, &q).unwrap();
        assert_eq!(r.particles.len(), 1);
        assert_eq!(r.dropped_edge, 1);
        assert!(r.dropped_small >= 1);
    }

    #[test]
    fn ellipse_and_feret() {
        // A 41 × 11 axis-aligned rectangle.
        let (w, h) = (80, 40);
        let mut mask = vec![false; w * h];
        for y in 10..21 {
            for x in 20..61 {
                mask[y * w + x] = true;
            }
        }
        let l = watershed(&mask, &vec![0.0; w * h], w, h, f32::INFINITY);
        let raw = vec![1.0; w * h];
        let (p, _, _) = measure(&l, &raw, w, h, 1, true);
        assert_eq!(p.len(), 1);
        let p = &p[0];
        assert!((p.feret_max - (41f64.hypot(11.0))).abs() < 1e-6);
        assert!((p.feret_min - 11.0).abs() < 1e-6);
        assert!((p.solidity - 1.0).abs() < 1e-9);
        assert_eq!(p.outline.len(), 8);
        assert!(p.angle.abs() < 1e-6);
        assert!(p.major > p.minor * 3.0);
    }

    #[test]
    fn distance_transform_is_exact() {
        let (w, h) = (31, 21);
        let mut mask = vec![true; w * h];
        mask[10 * w + 15] = false;
        mask[3 * w + 4] = false;
        let d = edt(&mask, w, h);
        for y in 0..h {
            for x in 0..w {
                let e = ((x as f64 - 15.0).hypot(y as f64 - 10.0))
                    .min((x as f64 - 4.0).hypot(y as f64 - 3.0));
                assert!(
                    (d[y * w + x] as f64 - e).abs() < 1e-4,
                    "({x},{y}) {} vs {e}",
                    d[y * w + x]
                );
            }
        }
    }

    #[test]
    fn wide_gaussian_matches() {
        let (w, h) = (101, 101);
        let mut a = vec![0f32; w * h];
        a[50 * w + 50] = 1.0;
        let mut b = a.clone();
        gaussian(&mut a, w, h, 9.0);
        let sig = 9.0f64;
        let tot: f64 = a.iter().map(|&x| x as f64).sum();
        assert!((tot - 1.0).abs() < 1e-3);
        let var: f64 = (0..w)
            .map(|x| (0..h).map(|y| a[y * w + x] as f64).sum::<f64>() * (x as f64 - 50.0).powi(2))
            .sum();
        assert!((var.sqrt() - sig).abs() < 0.3, "σ {}", var.sqrt());
        gaussian(&mut b, w, h, 3.0);
        assert!(b[50 * w + 50] > 0.0);
    }

    #[test]
    fn few_particles_on_flat_background() {
        // Two small dark disks: Otsu would split the noise.
        let d = [(40.0, 40.0, 6.0), (120.0, 90.0, 7.0)];
        let img: Vec<f32> = disks(200, 150, &d)
            .iter()
            .map(|v| if *v < 70.0 { 85.0 + (v - 40.0) } else { *v })
            .collect();
        let r = run(&img, 200, 150, &req()).unwrap();
        assert!(r.dark);
        assert_eq!(r.particles.len(), 2, "threshold {}", r.threshold);
    }

    #[test]
    fn otsu_splits_two_peaks() {
        let mut v = vec![10.0f32; 500];
        v.extend(vec![90.0f32; 300]);
        let h = histogram(&v, 10.0, 90.0, 256);
        let t = otsu(&h, 10.0, 90.0);
        assert!(t > 10.0 && t < 90.0);
    }

    /// Runs on a raw little-endian f32 image: TMV_NP (path), TMV_NP_W,
    /// TMV_NP_H; optional TMV_NP_DARK (0/1), TMV_NP_BG, TMV_NP_SMOOTH,
    /// TMV_NP_MIN; writes the result as JSON to TMV_NP_OUT.
    #[test]
    #[ignore]
    fn bench_file() {
        let env = |k: &str| std::env::var(k).ok();
        let path = env("TMV_NP").expect("TMV_NP");
        let w: usize = env("TMV_NP_W").unwrap().parse().unwrap();
        let h: usize = env("TMV_NP_H").unwrap().parse().unwrap();
        let bytes = std::fs::read(path).unwrap();
        let img: Vec<f32> = bytes
            .chunks(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let mut q = req();
        q.dark = env("TMV_NP_DARK").map(|v| v == "1");
        if let Some(v) = env("TMV_NP_BG") {
            q.background = v.parse().unwrap();
        }
        if let Some(v) = env("TMV_NP_SMOOTH") {
            q.smooth = v.parse().unwrap();
        }
        if let Some(v) = env("TMV_NP_MIN") {
            q.min_area = v.parse().unwrap();
        }
        let t = std::time::Instant::now();
        let r = run(&img, w, h, &q).unwrap();
        eprintln!(
            "{} particles, dark {}, threshold {}, {:.0} ms",
            r.particles.len(),
            r.dark,
            r.threshold,
            t.elapsed().as_secs_f64() * 1e3
        );
        if let Some(out) = env("TMV_NP_OUT") {
            std::fs::write(out, serde_json::to_string(&r).unwrap()).unwrap();
        }
    }
}
