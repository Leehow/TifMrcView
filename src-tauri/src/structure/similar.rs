//! 找所有类似 for line-like layers (膜, 纤维, 空心球's ring): the layer's
//! refined lines on a slice are the examples; every other line on the slice
//! (inside the layer's range) of the same polarity, about the same width and
//! a similar strength is a candidate. Each candidate is drawn by the same
//! line engine as a type click (`band_line`), so it looks like the
//! examples; the user takes out the wrong ones and adds the rest.

use super::follow::{rasterize, Cached};
use super::line::{band_line, centre_line};
use super::{blur, median, Outcome};
use crate::labels::{edt, Mark, SearchRange, Values};
use anyhow::Result;
use std::sync::Mutex;

pub struct Candidate {
    /// x, y, width, height of `mask`.
    pub rect: [usize; 4],
    pub mask: Vec<bool>,
    pub marks: Vec<Mark>,
    /// 0..0.8: 0.8 is as strong as the examples.
    pub score: f32,
}

/// The last search, for `apply`.
pub struct Found {
    pub token: u64,
    pub z: usize,
    pub items: Vec<Candidate>,
}

pub static FOUND: Mutex<Option<Found>> = Mutex::new(None);

/// Ridge strength of a `w`×`h` image at scale `sigma` for lines of polarity
/// `sign` (+1 dark, -1 bright): the larger Hessian eigenvalue, scaled.
fn ridge(v: &[f32], w: usize, h: usize, sigma: f32, sign: f32) -> Vec<f32> {
    let b = blur(v, w, h, sigma);
    let at = |x: usize, y: usize| b[y * w + x];
    let mut r = vec![0f32; w * h];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let dxx = at(x + 1, y) - 2.0 * at(x, y) + at(x - 1, y);
            let dyy = at(x, y + 1) - 2.0 * at(x, y) + at(x, y - 1);
            let dxy = (at(x + 1, y + 1) - at(x + 1, y - 1) - at(x - 1, y + 1) + at(x - 1, y - 1)) / 4.0;
            let (a, c) = (sign * dxx, sign * dyy);
            let bb = sign * dxy;
            let m = (a + c) / 2.0;
            let d = (((a - c) / 2.0).powi(2) + bb * bb).sqrt();
            // Across a line the curvature is large, along it small: the
            // larger eigenvalue less the other's magnitude keeps lines, not spots.
            let (l1, l2) = (m + d, m - d);
            r[y * w + x] = sigma * sigma * (l1 - l2.abs()).max(0.0);
        }
    }
    r
}

/// Finds lines like the `examples` on slice `z`. `taken`: pixels already
/// labelled (no candidates there). Best first, at most `max`.
pub fn lines(vals: &dyn Values, dims: (usize, usize, usize), z: usize, examples: &[Mark], taken: &[bool], range: Option<&SearchRange>, max: usize) -> Result<Vec<Candidate>> {
    let (w, h, _) = dims;
    let ex: Vec<&Mark> = examples.iter().filter(|m| m.path.len() >= 2).collect();
    if ex.is_empty() {
        anyhow::bail!("先在这一层涂一条膜并点结构类型精修，它就是要找的例子");
    }
    let vals = Cached { vals, keep: Mutex::new(vec![]) };
    // The slice in robust units, as the line engine reads it.
    let raw = {
        let slab = if dims.2 > 1 { 2 } else { 0 };
        let zs = z.saturating_sub(slab)..(z + slab + 1).min(dims.2);
        let n = zs.len() as f32;
        let mut v = vec![0f32; w * h];
        for zz in zs {
            for (o, p) in v.iter_mut().zip(vals.slice(zz)?) {
                *o += p / n;
            }
        }
        let (med, sd) = crate::segpick::median_sd(&v);
        v.iter_mut().for_each(|p| *p = (*p - med) / sd);
        v
    };
    let thick = median(ex.iter().map(|m| m.thickness).collect()).max(1.0);
    let sigma = (thick / 2.355).clamp(1.0, 5.6);
    let sample = |r: &[f32], m: &Mark| -> Vec<f32> {
        m.path
            .iter()
            .filter_map(|p| {
                let (x, y) = (p[0] as usize, p[1] as usize);
                (x < w && y < h).then(|| r[y * w + x])
            })
            .collect()
    };
    // Polarity: the sign the examples answer to.
    let (_sign, r) = {
        let dark = ridge(&raw, w, h, sigma, 1.0);
        let bright = ridge(&raw, w, h, sigma, -1.0);
        let md = median(ex.iter().flat_map(|m| sample(&dark, m)).collect());
        let mb = median(ex.iter().flat_map(|m| sample(&bright, m)).collect());
        if md >= mb { (1.0, dark) } else { (-1.0, bright) }
    };
    let ex_r = median(ex.iter().flat_map(|m| sample(&r, m)).collect()).max(1e-6);
    let _ = &sample;
    let ex_len = median(ex.iter().map(|m| m.path.len() as f32 * 3.0).collect());
    let min_len = (4.0 * thick).max(20.0).min(ex_len.max(20.0));

    // Not on or right beside what is labelled, only inside the range.
    let inside = range.map(|r| r.mask(w, h));
    let near_taken = edt(taken, w, h);
    let free = |i: usize| near_taken[i] > thick + 3.0 && inside.as_ref().map_or(true, |m| m[i]);
    let low = 0.25 * ex_r;
    let blobs = crate::labels::components_where(w, h, |i| r[i] >= low && free(i));
    let mut blobs: Vec<(f32, Vec<usize>)> = blobs.into_iter().filter(|b| b.len() as f32 >= min_len).map(|b| (b.iter().map(|&i| r[i]).sum::<f32>(), b)).collect();
    blobs.sort_by(|a, b| b.0.total_cmp(&a.0));
    blobs.truncate(4 * max);

    // The examples' cross-section: brightness across the line, from the
    // centre out to a few widths on each side (which side is inside does
    // not matter here: the profile is made symmetric).
    let reach = (3.0 * thick).ceil().max(6.0) as i32;
    let profile = |m: &Mark| -> Vec<f32> {
        let pts = &m.path;
        let mut cols: Vec<Vec<f32>> = vec![vec![]; (2 * reach + 1) as usize];
        for i in 0..pts.len() {
            let (a, b) = (pts[i.saturating_sub(1)], pts[(i + 1).min(pts.len() - 1)]);
            let (tx, ty) = (b[0] - a[0], b[1] - a[1]);
            let n = (tx * tx + ty * ty).sqrt().max(1e-6);
            let (nx, ny) = (-ty / n, tx / n);
            for u in -reach..=reach {
                let (x, y) = (pts[i][0] + nx * u as f32, pts[i][1] + ny * u as f32);
                if x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h {
                    cols[(u + reach) as usize].push(raw[y as usize * w + x as usize]);
                }
            }
        }
        let p: Vec<f32> = cols.into_iter().map(median).collect();
        (0..p.len()).map(|i| (p[i] + p[p.len() - 1 - i]) / 2.0).collect()
    };
    let profiles = |ms: &[&Mark]| -> Vec<f32> {
        let all: Vec<Vec<f32>> = ms.iter().map(|m| profile(m)).collect();
        (0..(2 * reach + 1) as usize).map(|u| median(all.iter().map(|p| p[u]).collect())).collect()
    };
    let ex_prof = profiles(&ex);
    let centred = |p: &[f32]| -> (Vec<f32>, f32) {
        let m = p.iter().sum::<f32>() / p.len() as f32;
        let c: Vec<f32> = p.iter().map(|v| v - m).collect();
        let n = c.iter().map(|v| v * v).sum::<f32>().sqrt();
        (c, n)
    };
    let (ex_c, ex_n) = centred(&ex_prof);
    // How much like the examples' cross-section (correlation) and how
    // strong (contrast relative to theirs).
    let alike = |p: &[f32]| -> (f32, f32) {
        let (c, n) = centred(p);
        let corr = c.iter().zip(&ex_c).map(|(a, b)| a * b).sum::<f32>() / (n * ex_n).max(1e-6);
        (corr, n / ex_n.max(1e-6))
    };

    // First pass on this slice: the line engine draws each candidate; keep
    // those whose cross-section looks like the examples'.
    let mut first = vec![];
    let mut used = vec![false; w * h];
    for (_, b) in blobs {
        if first.len() >= 3 * max {
            break;
        }
        let mut paint = vec![false; w * h];
        b.iter().for_each(|&i| paint[i] = true);
        let Some((centre, _)) = centre_line(&paint, w, h, 2.0) else { continue };
        if (centre.len() as f32) < min_len {
            continue;
        }
        let Outcome::Done(_, marks, _) = band_line(&vals, dims, z, &centre, thick / 2.0 + 2.0, &paint, Some(&used), false)? else { continue };
        let marks: Vec<Mark> = marks.into_iter().filter(|m| m.path.len() as f32 * 3.0 >= min_len && m.thickness <= 2.0 * thick + 1.0 && m.thickness >= 0.5 * thick - 1.0).collect();
        if marks.is_empty() {
            continue;
        }
        let (corr, contrast) = alike(&profiles(&marks.iter().collect::<Vec<_>>()));
        if corr < 0.5 || contrast < 0.35 {
            continue;
        }
        let mask = rasterize(w, h, &marks);
        mask.iter().enumerate().filter(|(_, &v)| v).for_each(|(i, _)| used[i] = true);
        let len: f32 = marks.iter().map(|m| m.path.len() as f32 * 3.0).sum();
        let pre = corr * contrast.min(1.0) * (len / ex_len.max(1.0)).min(1.0).sqrt();
        first.push((pre, mask, marks, corr, contrast.min(1.0)));
    }
    first.sort_by(|a, b| b.0.total_cmp(&a.0));
    first.truncate(max.min(40));

    // A membrane goes on through the neighbouring slices; a streak of noise
    // does not. Each candidate is followed a few slices up and down with the
    // engine of 沿 Z 找全; the share of slices it holds on is its 3D score.
    const SPAN: usize = 4;
    let (zlo, zhi) = (z.saturating_sub(SPAN), (z + SPAN).min(dims.2.saturating_sub(1)));
    let possible = (zhi - zlo).max(1) as f32;
    let mut out = vec![];
    for (_, mask, marks, corr, contrast) in first {
        let persist = if dims.2 > 1 {
            let r = SearchRange { poly: vec![], z0: zlo, z1: zhi };
            let f = super::follow::follow_z(&vals, dims, z, &mask, &marks, "membrane", Some(&r), &|_| {})?;
            f.slices.len() as f32 / possible
        } else {
            1.0
        };
        // Membranes are long; streaks of noise and the rims of small
        // things are short: length against the example's counts fully.
        let len: f32 = marks.iter().map(|m| m.path.len() as f32 * 3.0).sum();
        let long = (len / ex_len.max(1.0)).min(1.0);
        let score = 0.8 * corr.clamp(0.0, 1.0) * contrast * persist * long;
        if score < 0.03 {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for (i, _) in mask.iter().enumerate().filter(|(_, &v)| v) {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        let local: Vec<bool> = (0..bw * bh).map(|j| mask[(y0 + j / bw) * w + x0 + j % bw]).collect();
        out.push(Candidate { rect: [x0, y0, bw, bh], mask: local, marks: marks.into_iter().map(|m| Mark { inside: 0, ..m }).collect(), score });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two dark parallel lines and one bright line on hash noise.
    struct Img {
        w: usize,
        h: usize,
    }

    impl Values for Img {
        fn slice(&self, z: usize) -> Result<Vec<f32>> {
            let mut v = vec![0f32; self.w * self.h];
            for y in 0..self.h {
                for x in 0..self.w {
                    let n = (((x * 7919 + y * 104729 + z * 1299709) % 1000) as f32 / 1000.0 - 0.5) * 0.4;
                    let line = |yc: f32| (-(y as f32 + 0.5 - yc).powi(2) / (2.0 * 1.2 * 1.2)).exp();
                    let mut p = 1.0 + n;
                    if (10..150).contains(&x) {
                        p -= 1.5 * line(30.0 + x as f32 * 0.2);
                        p -= 1.5 * line(90.0 + x as f32 * 0.1);
                        p += 1.5 * line(140.0);
                    }
                    v[y * self.w + x] = p;
                }
            }
            Ok(v)
        }
    }

    #[test]
    fn finds_the_other_dark_line_not_the_bright_one() {
        let (w, h) = (160usize, 170usize);
        let img = Img { w, h };
        let path: Vec<[f32; 2]> = (12..148).step_by(3).map(|x| [x as f32 + 0.5, 30.0 + x as f32 * 0.2]).collect();
        let ex = Mark { z: 0, path, thickness: 3.0, inside: 1, closed: false };
        let taken = rasterize(w, h, std::slice::from_ref(&ex));
        let found = lines(&img, (w, h, 1), 0, &[ex], &taken, None, 20).unwrap();
        let strong: Vec<&Candidate> = found.iter().filter(|c| c.score > 0.4).collect();
        assert!(!strong.is_empty(), "nothing found");
        // The best one is the second dark line (y ≈ 90 + 0.1 x).
        let m = &strong[0].marks[0];
        let err: f32 = m.path.iter().map(|p| (p[1] - (90.0 + (p[0] - 0.5) * 0.1)).abs()).sum::<f32>() / m.path.len() as f32;
        assert!(err < 2.0, "best candidate off the line: {err}");
        // The bright line is no strong candidate.
        assert!(strong.iter().all(|c| c.marks.iter().all(|m| m.path.iter().all(|p| (p[1] - 140.0).abs() > 4.0))));
    }
}
