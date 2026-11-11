#![cfg_attr(not(feature = "pro"), allow(dead_code))]
//! Object proposals for the smart selection tool that need no network:
//! the extremal regions around a click (the connected bright or dark blob
//! at the threshold where its outline is sharpest), blob seeds at a known
//! size, and a description of a region to compare objects with examples.
//! SAM proposals (sam.rs) go through the same description, so "find the
//! others like this one" ranks both kinds alike.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// A region of a `w`×`h` window as pixel indices.
#[derive(Clone, Debug)]
pub struct Region {
    pub pix: Vec<usize>,
    /// Mean step across its outline, in noise sigmas.
    pub edge: f32,
    /// Brighter than its surroundings (false: darker).
    pub bright: bool,
    /// One blob: the flood had not yet run over into a neighbouring one.
    pub alone: bool,
}

/// The region a click most likely means: the sharpest outline of the blob
/// on its own, or of all regions when none stands alone clearly.
pub fn best_region(regions: &[Region]) -> Option<&Region> {
    let top = regions.iter().max_by(|a, b| a.edge.total_cmp(&b.edge))?;
    regions.iter().filter(|r| r.alone && r.edge >= 0.5 * top.edge).max_by(|a, b| a.edge.total_cmp(&b.edge)).or(Some(top))
}

#[derive(PartialEq)]
struct Item(f32, usize);
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        self.0.total_cmp(&o.0).then(self.1.cmp(&o.1))
    }
}

/// Noise of an image: the median step between neighbouring pixels, as a
/// Gaussian sigma. Edges are rare enough not to move the median.
pub fn noise_sigma(v: &[f32], w: usize, h: usize) -> f32 {
    let step = (v.len() / 20_000).max(1);
    let mut d: Vec<f32> = (0..v.len()).step_by(step).filter(|i| i % w + 1 < w && i / w + 1 < h).map(|i| (v[i] - v[i + 1]).abs().max((v[i] - v[i + w]).abs())).collect();
    if d.is_empty() {
        return 1.0;
    }
    let mid = d.len() / 2;
    d.select_nth_unstable_by(mid, f32::total_cmp);
    // The larger of two half-normal steps has its median at 1.09 sigma·√2.
    (d[mid] / 1.54).max(f32::MIN_POSITIVE)
}

/// The flood from `seed`: pixels in the order a rising (or, for dark
/// objects, falling) water level reaches them, and the level at each.
/// Stops at the window border or past `max_area`; only whole levels are kept.
fn flood_order(v: &[f32], w: usize, h: usize, seed: usize, pol: f32, max_area: usize) -> (Vec<usize>, Vec<f32>) {
    let mut queued = vec![false; w * h];
    let mut heap = BinaryHeap::new();
    let (mut order, mut levels) = (vec![], vec![]);
    let mut level = f32::INFINITY;
    queued[seed] = true;
    heap.push(Item(pol * v[seed], seed));
    let mut whole = 0;
    while let Some(Item(val, i)) = heap.pop() {
        if val < level {
            // Everything above this level is in: a whole region.
            whole = order.len();
            level = val;
        }
        let (x, y) = (i % w, i / w);
        if x == 0 || y == 0 || x + 1 == w || y + 1 == h || order.len() >= max_area {
            break;
        }
        order.push(i);
        levels.push(level);
        for j in [i - 1, i + 1, i - w, i + w] {
            if !queued[j] {
                queued[j] = true;
                heap.push(Item(pol * v[j], j));
            }
        }
    }
    order.truncate(whole);
    levels.truncate(whole);
    (order, levels)
}

/// The regions around `seed` (x, y) whose outline is sharper than at the
/// thresholds just above and below, smallest first: the blob under the
/// click, then what it is part of. `v` is one window of the image, lightly
/// smoothed; regions that reach the window border are not objects in it.
pub fn flood(v: &[f32], w: usize, h: usize, seed: (usize, usize), sigma: f32) -> Vec<Region> {
    let mut out = vec![];
    if w < 5 || h < 5 || seed.0 == 0 || seed.1 == 0 || seed.0 + 1 >= w || seed.1 + 1 >= h {
        return out;
    }
    let max_area = w * h / 4;
    let click = seed.1 * w + seed.0;
    for pol in [1.0f32, -1.0] {
        // Uphill from the click to the top of the blob it lies on (a click
        // rarely hits the top, and a flood from the flank would start below
        // the saddle to a brighter neighbour and take both).
        let mut top = click;
        loop {
            let (x, y) = (top % w, top / w);
            if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                break;
            }
            let up = [top - w - 1, top - w, top - w + 1, top - 1, top + 1, top + w - 1, top + w, top + w + 1].into_iter().max_by(|&a, &b| (pol * v[a]).total_cmp(&(pol * v[b]))).unwrap();
            if pol * v[up] <= pol * v[top] {
                break;
            }
            top = up;
        }
        let (order, levels) = flood_order(v, w, h, top, pol, max_area);
        if order.len() < 4 {
            continue;
        }
        let mut rank = vec![u32::MAX; w * h];
        for (t, &i) in order.iter().enumerate() {
            rank[i] = t as u32;
        }
        // Whole regions (the level drops right after them), about 12 %
        // apart in area. Where the area jumps from one
        // whole region to the next, the flood has run over a saddle into a
        // neighbouring blob: the region before the jump is the last one
        // that is this blob alone, and is always looked at.
        let mut ts: Vec<usize> = vec![];
        let mut alone: Vec<usize> = vec![];
        let mut last_whole = 0;
        for t in 4..=order.len() {
            if !(t == order.len() || levels[t] < levels[t - 1]) {
                continue;
            }
            if last_whole > 0 && t as f32 > last_whole as f32 * 1.12 + 6.0 {
                alone.push(last_whole);
                if ts.last() != Some(&last_whole) {
                    ts.push(last_whole);
                }
            }
            if ts.last().is_none_or(|&l| t as f32 >= l as f32 * 1.12) {
                ts.push(t);
            }
            last_whole = t;
        }
        let first_merge = alone.first().copied().unwrap_or(usize::MAX);
        // Outline sharpness of each: the mean step from an inside pixel to
        // its outside neighbours.
        let edges: Vec<f32> = ts
            .iter()
            .map(|&t| {
                let (mut sum, mut n) = (0f64, 0usize);
                for &i in &order[..t] {
                    for j in [i - 1, i + 1, i - w, i + w] {
                        if rank[j] >= t as u32 {
                            sum += (pol * (v[i] - v[j])) as f64;
                            n += 1;
                        }
                    }
                }
                (sum / n.max(1) as f64) as f32 / sigma
            })
            .collect();
        for k in 0..ts.len() {
            let before = if k > 0 { edges[k - 1] } else { f32::MIN };
            let after = if k + 1 < ts.len() { edges[k + 1] } else { f32::MIN };
            if (edges[k] >= before && edges[k] > after || alone.contains(&ts[k])) && edges[k] >= 2.0 {
                out.push(Region { pix: order[..ts[k]].to_vec(), edge: edges[k], bright: pol > 0.0, alone: ts[k] <= first_merge });
            }
        }
    }
    out.sort_by_key(|r| r.pix.len());
    out
}

/// What an object looks like, to compare it with the examples.
#[derive(Clone, Copy, Debug, Default)]
pub struct Look {
    /// Pixels (of the image, not of a reduced window).
    pub area: f32,
    /// Mean inside minus mean just around it, in noise sigmas.
    pub contrast: f32,
    /// Mean inside, in robust standard deviations from the image median.
    pub level: f32,
    /// Long over short axis.
    pub elongation: f32,
    /// Share of its bounding box it fills.
    pub fill: f32,
    /// Centre, in window pixels.
    pub cx: f32,
    pub cy: f32,
}

/// Median and robust standard deviation (from the MAD) of an image.
pub fn median_sd(v: &[f32]) -> (f32, f32) {
    let mut s: Vec<f32> = v.iter().step_by((v.len() / 50_000).max(1)).copied().collect();
    if s.is_empty() {
        return (0.0, 1.0);
    }
    let mid = s.len() / 2;
    s.select_nth_unstable_by(mid, f32::total_cmp);
    let med = s[mid];
    let mut d: Vec<f32> = s.iter().map(|x| (x - med).abs()).collect();
    d.select_nth_unstable_by(mid, f32::total_cmp);
    (med, (d[mid] * 1.4826).max(f32::MIN_POSITIVE))
}

/// Describes the region `pix` of a `w`×`h` window `v`. `scale` is how many
/// image pixels one window pixel covers; `stats` = (median, sd, noise).
pub fn look(v: &[f32], w: usize, h: usize, pix: &[usize], scale: f32, stats: (f32, f32, f32)) -> Look {
    let n = pix.len().max(1) as f64;
    let (mut sx, mut sy, mut sv) = (0f64, 0f64, 0f64);
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for &i in pix {
        let (x, y) = (i % w, i / w);
        sx += x as f64;
        sy += y as f64;
        sv += v[i] as f64;
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let (cx, cy, mean) = (sx / n, sy / n, sv / n);
    let (mut xx, mut yy, mut xy) = (0f64, 0f64, 0f64);
    for &i in pix {
        let (dx, dy) = ((i % w) as f64 - cx, (i / w) as f64 - cy);
        xx += dx * dx;
        yy += dy * dy;
        xy += dx * dy;
    }
    let (xx, yy, xy) = (xx / n + 1.0 / 12.0, yy / n + 1.0 / 12.0, xy / n);
    let d = (((xx - yy) / 2.0).powi(2) + xy * xy).sqrt();
    let (l1, l2) = ((xx + yy) / 2.0 + d, ((xx + yy) / 2.0 - d).max(1e-6));
    // The surroundings: the bounding box grown by half the object's
    // radius, without the object.
    let m = ((n / std::f64::consts::PI).sqrt() * 0.5).ceil().max(2.0) as usize;
    let (bx0, by0, bx1, by1) = (x0.saturating_sub(m), y0.saturating_sub(m), (x1 + m).min(w - 1), (y1 + m).min(h - 1));
    let (bw, bh) = (bx1 - bx0 + 1, by1 - by0 + 1);
    let mut inside = vec![false; bw * bh];
    for &i in pix {
        inside[(i / w - by0) * bw + (i % w - bx0)] = true;
    }
    let (mut ring, mut rn) = (0f64, 0usize);
    for y in 0..bh {
        for x in 0..bw {
            if !inside[y * bw + x] {
                ring += v[(by0 + y) * w + bx0 + x] as f64;
                rn += 1;
            }
        }
    }
    let ring = if rn > 0 { ring / rn as f64 } else { mean };
    Look {
        area: pix.len() as f32 * scale * scale,
        contrast: ((mean - ring) / stats.2 as f64) as f32,
        level: ((mean - stats.0 as f64) / stats.1 as f64) as f32,
        elongation: (l1 / l2).sqrt() as f32,
        fill: (n / ((x1 - x0 + 1) * (y1 - y0 + 1)) as f64) as f32,
        cx: cx as f32,
        cy: cy as f32,
    }
}

/// How much `c` looks like the `examples`, 1 (within their range on every
/// count) down to 0. With several examples the range between them counts
/// as alike, which is how marking a few different ones widens the search.
pub fn alike(c: &Look, examples: &[Look]) -> f32 {
    if examples.iter().all(|e| (e.contrast > 0.0) != (c.contrast > 0.0)) {
        return 0.0;
    }
    // (value, tolerance below the examples' range, tolerance above it) per
    // count; areas and contrasts compare as ratios. An object that stands
    // out more than the examples is still one of them, so contrast and
    // brightness only count against a candidate on the weak side; one much
    // larger is something else (a glow around a group of spots).
    let terms: [(fn(&Look) -> f32, f32, f32); 5] = [
        (|l| l.area.max(1.0).ln(), 0.8, 0.55),
        (|l| l.contrast.abs().max(0.1).ln(), 0.5, 3.0),
        (|l| l.level * l.contrast.signum(), 1.5, 30.0),
        (|l| l.elongation.ln(), 0.5, 0.5),
        (|l| l.fill, 0.25, 0.25),
    ];
    let mut d2 = 0.0;
    for (f, below, above) in terms {
        let x = f(c);
        let (lo, hi) = examples.iter().fold((f32::MAX, f32::MIN), |(lo, hi), e| (lo.min(f(e)), hi.max(f(e))));
        d2 += if x < lo { ((lo - x) / below).powi(2) } else if x > hi { ((x - hi) / above).powi(2) } else { 0.0 };
    }
    (-d2 / 2.0).exp()
}

/// Centres of blobs of about `diameter` pixels, bright or dark, strongest
/// first: local maxima of a difference of Gaussians, with their response.
pub fn blob_seeds(v: &[f32], w: usize, h: usize, diameter: f32, bright: bool) -> Vec<(usize, f32)> {
    let s = (diameter / 2.0 / std::f32::consts::SQRT_2).max(0.7);
    let a = crate::sam::denoise(&[v.to_vec()], w, h, s);
    let b = crate::sam::denoise(&[v.to_vec()], w, h, s * 1.6);
    let pol = if bright { 1.0 } else { -1.0 };
    let d: Vec<f32> = a.iter().zip(&b).map(|(a, b)| pol * (a - b)).collect();
    let r = (diameter / 4.0).round().clamp(1.0, 6.0) as usize;
    let mut out = vec![];
    for y in r..h.saturating_sub(r) {
        'px: for x in r..w.saturating_sub(r) {
            let c = d[y * w + x];
            if c <= 0.0 {
                continue;
            }
            for yy in y - r..=y + r {
                for xx in x - r..=x + r {
                    let o = d[yy * w + xx];
                    if o > c || (o == c && (yy, xx) < (y, x)) {
                        continue 'px;
                    }
                }
            }
            out.push((y * w + x, c));
        }
    }
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// One candidate of a search, in image pixels.
#[derive(Clone, Debug)]
pub struct Found {
    /// Bounding box x0, y0, width, height and its mask, row-major.
    pub rect: [usize; 4],
    pub mask: Vec<bool>,
    /// 0..1, how much it looks like the examples.
    pub score: f32,
    pub look: Look,
}

impl Found {
    pub fn area(&self) -> usize {
        self.mask.iter().filter(|m| **m).count()
    }

    /// From the pixels `pix` of a window at (`ox`, `oy`) in the image.
    pub fn from_pixels(pix: &[usize], w: usize, (ox, oy): (usize, usize), score: f32, look: Look) -> Found {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for &i in pix {
            (x0, y0, x1, y1) = (x0.min(i % w), y0.min(i / w), x1.max(i % w), y1.max(i / w));
        }
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut mask = vec![false; bw * bh];
        for &i in pix {
            mask[(i / w - y0) * bw + (i % w - x0)] = true;
        }
        Found { rect: [ox + x0, oy + y0, bw, bh], mask, score, look }
    }

    fn overlap(&self, o: &Found) -> usize {
        let (ax0, ay0, ax1, ay1) = (self.rect[0], self.rect[1], self.rect[0] + self.rect[2], self.rect[1] + self.rect[3]);
        let (bx0, by0, bx1, by1) = (o.rect[0], o.rect[1], o.rect[0] + o.rect[2], o.rect[1] + o.rect[3]);
        let (x0, y0, x1, y1) = (ax0.max(bx0), ay0.max(by0), ax1.min(bx1), ay1.min(by1));
        let mut n = 0;
        for y in y0..y1.max(y0) {
            for x in x0..x1.max(x0) {
                n += (self.mask[(y - ay0) * self.rect[2] + x - ax0] && o.mask[(y - by0) * o.rect[2] + x - bx0]) as usize;
            }
        }
        n
    }
}

/// Keeps the best of candidates that cover each other (more than 30 % of
/// the smaller one), best first.
pub fn dedupe(mut found: Vec<Found>) -> Vec<Found> {
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Found> = vec![];
    let mut areas: Vec<usize> = vec![];
    for f in found {
        let fa = f.area();
        if fa == 0 {
            continue;
        }
        if keep.iter().zip(&areas).all(|(k, &ka)| k.overlap(&f) * 10 <= fa.min(ka) * 3) {
            keep.push(f);
            areas.push(fa);
        }
    }
    keep
}

/// Blobs like the `examples` (pixel lists of the same window) in one
/// window `v` (lightly smoothed): seeds at the examples' size, each grown
/// to the extremal region nearest that size, scored against the examples.
/// `scale`: image pixels per window pixel. Candidates scoring under `min`
/// or covering `taken` pixels are dropped.
pub fn find_blobs(v: &[f32], w: usize, h: usize, examples: &[Vec<usize>], scale: f32, taken: &[bool], min: f32) -> Vec<Found> {
    if examples.is_empty() {
        return vec![];
    }
    let (med, sd) = median_sd(v);
    let stats = (med, sd, noise_sigma(v, w, h));
    let looks: Vec<Look> = examples.iter().map(|p| look(v, w, h, p, scale, stats)).collect();
    let mean_area = looks.iter().map(|l| l.area / (scale * scale)).sum::<f32>() / looks.len() as f32;
    let diameter = 2.0 * (mean_area / std::f32::consts::PI).sqrt();
    let bright = looks.iter().filter(|l| l.contrast > 0.0).count() * 2 >= looks.len();
    let seeds = blob_seeds(v, w, h, diameter, bright);
    // A window around each seed, large enough for a blob three times the
    // examples' size with room around it.
    let half = (diameter * 4.0).ceil().max(12.0) as usize;
    let mut out = vec![];
    for (seed, _) in seeds.into_iter().take(4000) {
        let (x, y) = (seed % w, seed / w);
        if taken[seed] {
            continue;
        }
        let (x0, y0, x1, y1) = (x.saturating_sub(half), y.saturating_sub(half), (x + half + 1).min(w), (y + half + 1).min(h));
        let (ww, wh) = (x1 - x0, y1 - y0);
        let win: Vec<f32> = (0..wh).flat_map(|r| v[(y0 + r) * w + x0..(y0 + r) * w + x1].iter().copied()).collect();
        let mut best: Option<(f32, Vec<usize>, Look)> = None;
        for r in flood(&win, ww, wh, (x - x0, y - y0), stats.2) {
            if r.bright != bright {
                continue;
            }
            let pix: Vec<usize> = r.pix.iter().map(|&i| (y0 + i / ww) * w + x0 + i % ww).collect();
            let l = look(v, w, h, &pix, scale, stats);
            let s = alike(&l, &looks);
            if best.as_ref().is_none_or(|b| s > b.0) {
                best = Some((s, pix, l));
            }
        }
        if let Some((s, pix, l)) = best.filter(|b| b.0 >= min) {
            if pix.iter().filter(|&&i| taken[i]).count() * 5 <= pix.len() {
                out.push(Found::from_pixels(&pix, w, (0, 0), s, l));
            }
        }
    }
    dedupe(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gaussian blobs of height `a` and width `s` on a sloping, noisy background.
    fn scene(w: usize, h: usize, blobs: &[(f32, f32, f32, f32)]) -> Vec<f32> {
        let mut seed = 12345u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let mut v = 100.0 + 0.2 * x + noise() * 4.0;
                for &(cx, cy, a, s) in blobs {
                    v += a * (-((x - cx).powi(2) + (y - cy).powi(2)) / (2.0 * s * s)).exp();
                }
                v
            })
            .collect()
    }

    #[test]
    fn click_takes_the_blob_under_it() {
        let (w, h) = (96, 96);
        let v = scene(w, h, &[(40.0, 44.0, 80.0, 4.0), (70.0, 30.0, 80.0, 4.0)]);
        let v = crate::sam::denoise(&[v], w, h, 1.0);
        let sigma = noise_sigma(&v, w, h);
        let r = flood(&v, w, h, (42, 45), sigma);
        assert!(!r.is_empty());
        let best = best_region(&r).unwrap();
        assert!(best.bright);
        // Around the half-maximum outline: radius 4·√(2 ln 2) ≈ 4.7, area ≈ 70.
        assert!((35..140).contains(&best.pix.len()), "{}", best.pix.len());
        assert!(best.pix.iter().all(|&i| ((i % w) as f32 - 40.0).hypot((i / w) as f32 - 44.0) < 10.0));
    }

    #[test]
    fn dark_blobs_work_too() {
        let (w, h) = (96, 96);
        let v = scene(w, h, &[(50.0, 50.0, -60.0, 5.0)]);
        let v = crate::sam::denoise(&[v], w, h, 1.0);
        let r = flood(&v, w, h, (50, 51), noise_sigma(&v, w, h));
        let best = best_region(&r).unwrap();
        assert!(!best.bright);
        assert!((50..220).contains(&best.pix.len()), "{}", best.pix.len());
    }

    #[test]
    fn finds_the_other_blobs_of_one_example() {
        let (w, h) = (200, 160);
        let blobs = [(30.0, 30.0, 80.0, 3.5), (90.0, 40.0, 70.0, 3.5), (150.0, 50.0, 90.0, 4.0), (60.0, 110.0, 75.0, 3.0), (140.0, 120.0, 80.0, 3.5), (100.0, 90.0, 85.0, 14.0)];
        let v = scene(w, h, &blobs);
        let v = crate::sam::denoise(&[v], w, h, 1.0);
        let sigma = noise_sigma(&v, w, h);
        let ex = best_region(&flood(&v, w, h, (30, 30), sigma)).unwrap().clone();
        let mut taken = vec![false; w * h];
        for &i in &ex.pix {
            taken[i] = true;
        }
        let found = find_blobs(&v, w, h, &[ex.pix], 1.0, &taken, 0.5);
        let at = |x: f32, y: f32| found.iter().any(|f| (f.rect[0] as f32 + f.rect[2] as f32 / 2.0 - x).hypot(f.rect[1] as f32 + f.rect[3] as f32 / 2.0 - y) < 4.0);
        for b in &blobs[1..5] {
            assert!(at(b.0, b.1), "missed the blob at {}, {}: {found:?}", b.0, b.1);
        }
        // Not the example again, and not the large blob.
        assert!(!at(30.0, 30.0));
        assert!(!at(100.0, 90.0));
        assert_eq!(found.len(), 4);
    }
}
