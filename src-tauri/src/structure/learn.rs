//! 沿 Z 找全 and 找所有类似 learnt from the user's own marks, in 3D at once.
//!
//! The way ilastik and Ais work: the pixels the user labelled on one slice
//! are the examples of the structure, a ring around them (the cytoplasm, ice
//! or lumen next to it) the examples of "not this", and the Pro pixel
//! classifier (`classify`: filter responses + random forest) learns the
//! difference on the spot. It labels every slice in the layer's range (each
//! read as the mean of three neighbours, which keeps a membrane and loses
//! noise), the answers are smoothed across Z, and the 3D pieces are read off
//! at once, as TomoSegMemTV does with its membrane filter: a membrane is one
//! connected sheet, whatever its slices look like one by one.
//!
//! 沿 Z 找全 keeps the pieces the user's marks touch; 找所有类似 offers the
//! others, best first. A piece the user takes out is a "not this" example
//! for the next search (see `NOT`).
//!
//! What gets written on each slice is drawn by the same engine as a type
//! click (`refine`), with the piece as the paint, so a found membrane looks
//! like the user's.

use super::follow::{carry_inside, extended, rasterize, Cached, CANCEL};
use super::{refine, Outcome};
use crate::classify::{self, ForestConfig};
use crate::labels::{components_where, edt, Mark, SearchRange, Values};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

/// One connected 3D piece: its pixels (image indices) per slice, and how
/// sure the classifier is of it.
#[derive(Clone, Debug)]
pub struct Piece {
    pub px: BTreeMap<usize, Vec<u32>>,
    /// Mean membrane probability (0.5..1).
    pub prob: f32,
}

impl Piece {
    pub fn voxels(&self) -> usize {
        self.px.values().map(Vec::len).sum()
    }
    pub fn zs(&self) -> (usize, usize) {
        (*self.px.keys().next().unwrap_or(&0), *self.px.keys().next_back().unwrap_or(&0))
    }
    /// 0..0.8, as the 找同类 bar reads scores (0.8: as clear as the examples).
    pub fn score(&self) -> f32 {
        let (a, b) = self.zs();
        let sure = ((self.prob - 0.5) * 2.0).clamp(0.0, 1.0);
        let deep = ((b - a + 1) as f32 / 10.0).min(1.0);
        0.8 * sure.sqrt() * deep
    }
}

/// Pixels the user took out of earlier candidates, per (image token,
/// label): (slice, pixels) learnt as "not this" next time.
pub static NOT: Mutex<Vec<((u64, u16), usize, Vec<u32>)>> = Mutex::new(vec![]);

/// The examples: the layer's pixels on slice `z` (`pos`), their thickness,
/// and pixels known not to be the structure (slice, image indices).
pub struct Examples<'a> {
    pub z: usize,
    pub pos: &'a [bool],
    pub thick: f32,
    pub not: Vec<(usize, Vec<u32>)>,
}

/// The mean of slices z-1..=z+1 (fewer at the ends).
fn mean3(vals: &dyn Values, z: usize, d: usize) -> Result<Vec<f32>> {
    let mut acc = vals.slice(z)?;
    let mut n = 1.0;
    for zz in [z.wrapping_sub(1), z + 1] {
        if zz < d {
            let v = vals.slice(zz)?;
            acc.iter_mut().zip(&v).for_each(|(a, b)| *a += b);
            n += 1.0;
        }
    }
    acc.iter_mut().for_each(|a| *a /= n);
    Ok(acc)
}

/// Rows and columns [x0, y0, x1, y1) that hold the range (with a margin for
/// the filters), or the whole slice.
fn bounds(range: Option<&SearchRange>, w: usize, h: usize) -> [usize; 4] {
    let Some(r) = range.filter(|r| r.poly.len() >= 3) else { return [0, 0, w, h] };
    let m = 24.0;
    let (mut a, mut b, mut c, mut d) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &r.poly {
        a = a.min(p[0]);
        b = b.min(p[1]);
        c = c.max(p[0]);
        d = d.max(p[1]);
    }
    let cl = |v: f32, n: usize| (v.max(0.0) as usize).min(n);
    [cl(a - m, w), cl(b - m, h), cl(c + m + 1.0, w), cl(d + m + 1.0, h)]
}

fn crop(v: &[f32], w: usize, b: [usize; 4]) -> Vec<f32> {
    let mut out = Vec::with_capacity((b[2] - b[0]) * (b[3] - b[1]));
    for y in b[1]..b[3] {
        out.extend_from_slice(&v[y * w + b[0]..y * w + b[2]]);
    }
    out
}

struct Sets(Vec<u32>);

impl Sets {
    fn add(&mut self) -> u32 {
        self.0.push(self.0.len() as u32);
        self.0.len() as u32 - 1
    }
    fn root(&mut self, mut a: u32) -> u32 {
        while self.0[a as usize] != a {
            self.0[a as usize] = self.0[self.0[a as usize] as usize];
            a = self.0[a as usize];
        }
        a
    }
    fn join(&mut self, a: u32, b: u32) {
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            self.0[a.max(b) as usize] = a.min(b);
        }
    }
}

/// What was learnt, for one image and layer: the classifier and where it
/// applies. Kept between searches (the user's next 找所有类似 or 沿 Z on
/// the same examples needs no new training).
pub struct Learnt {
    clf: classify::Classifier,
    dims: (usize, usize, usize),
    /// Box [x0, y0, x1, y1) and slices searched.
    b: [usize; 4],
    zs: (usize, usize),
    inside: Option<Vec<f32>>,
    pub thick: f32,
}

/// Learns the structure from `ex` (in the layer's range, all slices when it
/// has none).
pub fn learn(vals: &dyn Values, dims: (usize, usize, usize), ex: &Examples, range: Option<&SearchRange>) -> Result<Learnt> {
    let (w, h, d) = dims;
    let zs = range.map_or((0, d - 1), |r| (r.z0.min(d - 1), r.z1.min(d - 1).max(r.z0.min(d - 1))));
    let b = bounds(range, w, h);
    if b[2] - b[0] < 8 || b[3] - b[1] < 8 {
        bail!("范围太小");
    }
    let inside = range.filter(|r| r.poly.len() >= 3).map(|r| crop(&r.mask(w, h).iter().map(|&v| v as u8 as f32).collect::<Vec<_>>(), w, b));
    // Examples on their slices: 2 = the structure, 1 = not it.
    let t = ex.thick.max(1.5);
    let (r0, r1) = (2.0 * t + 3.0, 2.0 * t + 3.0 + (3.0 * t).max(10.0));
    let near = edt(ex.pos, w, h);
    let mut marks0 = vec![0u16; w * h];
    for i in 0..w * h {
        if ex.pos[i] {
            marks0[i] = 2;
        } else if near[i] >= r0 && near[i] <= r1 {
            marks0[i] = 1;
        }
    }
    let mut train_z: BTreeMap<usize, Vec<u16>> = BTreeMap::new();
    train_z.insert(ex.z, marks0);
    for (z, px) in &ex.not {
        if *z >= d {
            continue;
        }
        let s = train_z.entry(*z).or_insert_with(|| vec![0u16; w * h]);
        for &i in px {
            if (i as usize) < s.len() && s[i as usize] != 2 {
                s[i as usize] = 1;
            }
        }
    }
    let slices: Vec<(Vec<f32>, Vec<u16>)> = train_z.into_iter().map(|(z, m)| Ok((mean3(vals, z, d)?, m))).collect::<Result<_>>()?;
    let (med, sd) = crate::segpick::median_sd(&mean3(vals, ex.z, d)?);
    let (lo, hi) = (med - 3.0 * sd, med + 3.0 * sd);
    let cfg = ForestConfig { trees: 32, max_depth: 12, ..Default::default() };
    let refs: Vec<(Vec<f32>, &[u16])> = slices.iter().map(|(v, m)| (v.clone(), m.as_slice())).collect();
    let (clf, _) = classify::on_device(|dev| classify::train(dev, &refs, w, h, lo, hi, &cfg))?;
    Ok(Learnt { clf, dims, b, zs, inside, thick: t })
}

/// Reads probabilities from a `Learnt`, each slice once.
pub struct Reader<'a> {
    l: &'a Learnt,
    vals: Cached<'a>,
    raw: Mutex<HashMap<usize, std::sync::Arc<Vec<f32>>>>,
}

impl<'a> Reader<'a> {
    pub fn new(l: &'a Learnt, vals: &'a dyn Values) -> Self {
        Reader { l, vals: Cached { vals, keep: Mutex::new(vec![]) }, raw: Mutex::new(HashMap::new()) }
    }

    fn box_dims(&self) -> (usize, usize) {
        (self.l.b[2] - self.l.b[0], self.l.b[3] - self.l.b[1])
    }

    /// Probability of the structure on slice `z`, in the box.
    fn raw(&self, z: usize) -> Result<std::sync::Arc<Vec<f32>>> {
        if let Some(p) = self.raw.lock().unwrap().get(&z) {
            return Ok(p.clone());
        }
        let (bw, bh) = self.box_dims();
        let v = crop(&mean3(&self.vals, z, self.l.dims.2)?, self.l.dims.0, self.l.b);
        let ((lab, conf), _) = classify::on_device(|dev| Ok(self.l.clf.predict(dev, &v, bw, bh)))?;
        let p: Vec<f32> = lab.iter().zip(&conf).map(|(&l, &p)| if l == 2 { p } else { 1.0 - p }).collect();
        let p = std::sync::Arc::new(p);
        let mut m = self.raw.lock().unwrap();
        // The pieces pass walks the stack in order: older slices go.
        m.retain(|k, _| k.abs_diff(z) <= 4);
        m.insert(z, p.clone());
        Ok(p)
    }

    /// Probability on slice `z` smoothed (1 2 1) across Z, in the box, 0
    /// outside the range.
    pub fn prob(&self, z: usize) -> Result<Vec<f32>> {
        let mid = self.raw(z)?;
        let lo = if z > self.l.zs.0 { Some(self.raw(z - 1)?) } else { None };
        let hi = if z < self.l.zs.1 { Some(self.raw(z + 1)?) } else { None };
        let mut p = mid.to_vec();
        for i in 0..p.len() {
            let a = lo.as_ref().map_or(mid[i], |s| s[i]);
            let c = hi.as_ref().map_or(mid[i], |s| s[i]);
            p[i] = (a + 2.0 * mid[i] + c) / 4.0;
            if self.l.inside.as_ref().is_some_and(|m| m[i] < 0.5) {
                p[i] = 0.0;
            }
        }
        Ok(p)
    }

    /// `prob` over the whole slice (0 outside the box).
    pub fn prob_image(&self, z: usize) -> Result<Vec<f32>> {
        let (w, h, _) = self.l.dims;
        let b = self.l.b;
        let bw = b[2] - b[0];
        let p = self.prob(z)?;
        let mut out = vec![0f32; w * h];
        for y in b[1]..b[3] {
            out[y * w + b[0]..y * w + b[2]].copy_from_slice(&p[(y - b[1]) * bw..(y - b[1] + 1) * bw]);
        }
        Ok(out)
    }

    /// The 3D pieces of the structure: pixels above 0.5, joined across
    /// slices; a piece must reach 0.75 somewhere (one weak speck chain is
    /// noise). `progress(slices done, of)`.
    pub fn pieces(&self, progress: &dyn Fn(usize, usize)) -> Result<Vec<Piece>> {
        let (w, _, _) = self.l.dims;
        let b = self.l.b;
        let (bw, bh) = self.box_dims();
        let (zlo, zhi) = self.l.zs;
        let t = self.l.thick;
        let min_px = ((t * t * 2.0) as usize).max(12);
        let mut sets = Sets(vec![]);
        // Per 2D piece: its slice, pixels (image indices), probability sum, peak.
        let mut parts: Vec<(usize, Vec<u32>, f32, f32)> = vec![];
        let mut prev_lab: Vec<u32> = vec![0; bw * bh];
        let total = zhi - zlo + 1;
        for z in zlo..=zhi {
            if CANCEL.load(Ordering::Relaxed) {
                bail!("已取消");
            }
            let p = self.prob(z)?;
            let comps = components_where(bw, bh, |i| p[i] > 0.5);
            let mut lab = vec![0u32; bw * bh];
            for c in comps.into_iter().filter(|c| c.len() >= min_px) {
                let id = sets.add();
                let (mut sum, mut peak) = (0.0, 0f32);
                let mut px = Vec::with_capacity(c.len());
                for &i in &c {
                    lab[i] = id + 1;
                    sum += p[i];
                    peak = peak.max(p[i]);
                    px.push(((b[1] + i / bw) * w + b[0] + i % bw) as u32);
                    // Joined to the pieces it touches on the slice before.
                    let (x, y) = ((i % bw) as i64, (i / bw) as i64);
                    for dy in -1..=1i64 {
                        for dx in -1..=1i64 {
                            let (xx, yy) = (x + dx, y + dy);
                            if xx >= 0 && yy >= 0 && (xx as usize) < bw && (yy as usize) < bh {
                                let q = prev_lab[yy as usize * bw + xx as usize];
                                if q > 0 {
                                    sets.join(id, q - 1);
                                }
                            }
                        }
                    }
                }
                parts.push((z, px, sum, peak));
            }
            prev_lab = lab;
            progress(z - zlo + 1, total);
        }
        let mut by_root: HashMap<u32, Piece> = HashMap::new();
        let mut sums: HashMap<u32, (f32, usize, f32)> = HashMap::new();
        for (k, (z, px, sum, peak)) in parts.into_iter().enumerate() {
            let r = sets.root(k as u32);
            let s = sums.entry(r).or_default();
            s.0 += sum;
            s.1 += px.len();
            s.2 = s.2.max(peak);
            by_root.entry(r).or_insert_with(|| Piece { px: BTreeMap::new(), prob: 0.0 }).px.entry(z).or_default().extend(px);
        }
        let mut out: Vec<Piece> = by_root
            .into_iter()
            .filter(|(r, _)| sums[r].2 >= 0.75)
            .map(|(r, mut p)| {
                let (s, n, _) = sums[&r];
                p.prob = s / n.max(1) as f32;
                p
            })
            .collect();
        out.sort_by(|a, b| b.voxels().cmp(&a.voxels()));
        Ok(out)
    }

    /// 沿 Z 找全, line by line: each refined line on `z0` is carried to the
    /// next slice where the learnt probability still holds it: the paint
    /// there is what of the line's band (the line a little longer, a little
    /// wider) the classifier calls structure, drawn by the type-click
    /// engine. A line stops where too little of it is left: that one test,
    /// learnt from the user's own example, replaces the hand-set rules.
    /// Filled types are carried as their probable pixels near the object.
    #[allow(clippy::too_many_arguments)]
    pub fn follow(&self, z0: usize, start: &[Mark], start_px: &[bool], structure: &str, existing: &BTreeMap<usize, Vec<bool>>, progress: &dyn Fn(usize)) -> Result<(BTreeMap<usize, Vec<bool>>, Vec<Mark>)> {
        let (w, h, _) = self.l.dims;
        let dims = self.l.dims;
        let (zlo, zhi) = self.l.zs;
        let t = self.l.thick;
        let line = matches!(structure, "membrane" | "membrane2" | "fibre" | "shell");
        let gap = (2.0 * t).max(6.0);
        let reach = t + 3.0;
        let mut out: BTreeMap<usize, Vec<bool>> = BTreeMap::new();
        let mut found = vec![];
        let mut steps = 0;
        // One run per line (lines of one membrane stay apart); a filled
        // object is one run.
        let runs: Vec<Vec<Mark>> = if line { start.iter().filter(|m| m.path.len() >= 2).map(|m| vec![m.clone()]).collect() } else { vec![vec![]] };
        for run in runs {
            let first_px = if line { rasterize(w, h, &run) } else { start_px.to_vec() };
            let n0 = first_px.iter().filter(|&&v| v).count().max(1);
            for dir in [-1i64, 1] {
                let (mut prev, mut prev_px) = (run.clone(), first_px.clone());
                let mut z = z0 as i64 + dir;
                while z >= zlo as i64 && z <= zhi as i64 {
                    if CANCEL.load(Ordering::Relaxed) {
                        bail!("已取消");
                    }
                    let zu = z as usize;
                    let p = self.prob_image(zu)?;
                    // Where the object may be now.
                    let band: Vec<bool> = if line {
                        let longer: Vec<Mark> = prev.iter().map(|m| extended(m, EXTEND)).collect();
                        edt(&rasterize(w, h, &longer), w, h).iter().map(|&d| d <= reach).collect()
                    } else {
                        edt(&prev_px, w, h).iter().map(|&d| d <= reach).collect()
                    };
                    let paint: Vec<bool> = band.iter().zip(&p).map(|(&b, &q)| b && q > 0.5).collect();
                    // How much of the object before is still held up.
                    let near_paint = edt(&paint, w, h);
                    let held = prev_px.iter().zip(&near_paint).filter(|(&v, &d)| v && d <= reach).count();
                    let before = prev_px.iter().filter(|&&v| v).count().max(1);
                    if held * 10 < before * 4 || held * 10 < n0 * 3 {
                        break;
                    }
                    let (mut new, px) = if line {
                        let ms: Vec<Mark> = match refine(&self.vals, dims, zu, &paint, gap, structure)? {
                            Outcome::Done(_, ms, _) => ms
                                .into_iter()
                                .filter(|m| {
                                    let on = m.path.iter().filter(|q| (q[0] as usize) < w && (q[1] as usize) < h && near_paint[q[1] as usize * w + q[0] as usize] <= reach).count();
                                    on * 10 >= m.path.len() * 7
                                })
                                .collect(),
                            Outcome::Rough(_) => vec![],
                        };
                        let px = rasterize(w, h, &ms);
                        (ms, px)
                    } else {
                        (vec![], paint.clone())
                    };
                    if line && new.is_empty() {
                        break;
                    }
                    for m in new.iter_mut() {
                        m.z = zu;
                    }
                    if structure != "shell" {
                        carry_inside(&mut new, &prev);
                    }
                    // Already there (the user's paint, an earlier run, another
                    // line of this run): followed on, not drawn twice.
                    let mut taken = existing.get(&zu).cloned().unwrap_or_else(|| vec![false; w * h]);
                    if let Some(o) = out.get(&zu) {
                        taken.iter_mut().zip(o).for_each(|(a, &b)| *a |= b);
                    }
                    let (draw_px, fresh) = if line && taken.iter().any(|&v| v) {
                        let dt = edt(&taken, w, h);
                        let lim = (2.0 * t).max(4.0) + 2.0;
                        let fresh: Vec<Mark> = new
                            .iter()
                            .filter(|m| {
                                let on = m.path.iter().filter(|q| (q[0] as usize) < w && (q[1] as usize) < h && dt[q[1] as usize * w + q[0] as usize] <= lim).count();
                                2 * on < m.path.len()
                            })
                            .cloned()
                            .collect();
                        (rasterize(w, h, &fresh), fresh)
                    } else if line {
                        (px.clone(), new.clone())
                    } else {
                        (px.iter().zip(&taken).map(|(&a, &b)| a && !b).collect(), vec![])
                    };
                    if draw_px.iter().any(|&v| v) {
                        let slot = out.entry(zu).or_insert_with(|| vec![false; w * h]);
                        slot.iter_mut().zip(&draw_px).for_each(|(a, &b)| *a |= b);
                    }
                    found.extend(fresh);
                    steps += 1;
                    progress(steps);
                    prev = new;
                    prev_px = px;
                    z += dir;
                }
            }
        }
        Ok((out, found))
    }

    /// Takes a found piece as the user's own: its pixels on slice `z` drawn
    /// by the type-click engine, then followed through Z like the examples.
    pub fn adopt(&self, z: usize, px: &[u32], structure: &str, existing: &BTreeMap<usize, Vec<bool>>, progress: &dyn Fn(usize)) -> Result<(BTreeMap<usize, Vec<bool>>, Vec<Mark>)> {
        let (w, h, _) = self.l.dims;
        let line = matches!(structure, "membrane" | "membrane2" | "fibre" | "shell");
        let mut paint = vec![false; w * h];
        px.iter().for_each(|&i| paint[i as usize] = true);
        let (marks, own) = if line {
            let near = edt(&paint, w, h);
            let reach = self.l.thick + 3.0;
            let mut ms: Vec<Mark> = match refine(&self.vals, self.l.dims, z, &paint, (2.0 * self.l.thick).max(6.0), structure)? {
                Outcome::Done(_, ms, _) => ms
                    .into_iter()
                    .filter(|m| {
                        let on = m.path.iter().filter(|q| (q[0] as usize) < w && (q[1] as usize) < h && near[q[1] as usize * w + q[0] as usize] <= reach).count();
                        on * 10 >= m.path.len() * 7
                    })
                    .collect(),
                Outcome::Rough(_) => vec![],
            };
            ms.iter_mut().for_each(|m| m.z = z);
            let own = rasterize(w, h, &ms);
            (ms, own)
        } else {
            (vec![], paint.clone())
        };
        if !own.iter().any(|&v| v) {
            return Ok((BTreeMap::new(), vec![]));
        }
        let (mut out, mut found) = self.follow(z, &marks, &own, structure, existing, progress)?;
        let slot = out.entry(z).or_insert_with(|| vec![false; w * h]);
        slot.iter_mut().zip(&own).for_each(|(a, &b)| *a |= b);
        found.extend(marks);
        Ok((out, found))
    }
}

/// How far (pixels) a line may grow at each end from one slice to the next.
const EXTEND: f32 = 8.0;

/// Whether piece `p` touches the example pixels (within `reach`) on slice `z`.
pub fn touches(p: &Piece, z: usize, near_pos: &[f32], reach: f32) -> bool {
    p.px.get(&z).is_some_and(|px| px.iter().any(|&i| near_pos[i as usize] <= reach))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two dark tilted lines on noise: one through z 5..30, one far away
    /// through z 8..25; a bright line through all slices.
    struct Two {
        w: usize,
        h: usize,
    }

    impl Values for Two {
        fn slice(&self, z: usize) -> Result<Vec<f32>> {
            let (w, h) = (self.w, self.h);
            let mut v = vec![0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    let n = (((x * 7919 + y * 104729 + z * 1299709) % 1000) as f32 / 1000.0 - 0.5) * 0.6;
                    v[y * w + x] = 1.0 + n;
                }
            }
            let mut line = |yc: &dyn Fn(f32) -> f32, amp: f32| {
                for x in 10..w - 10 {
                    let c = yc(x as f32);
                    for y in 0..h {
                        let dy = y as f32 + 0.5 - c;
                        v[y * w + x] += amp * (-dy * dy / (2.0 * 1.2 * 1.2)).exp();
                    }
                }
            };
            let off = z as f32 * 0.5;
            if (5..30).contains(&z) {
                line(&|x| 20.0 + off + x * 0.2, -1.5);
            }
            if (8..25).contains(&z) {
                line(&|x| 100.0 - off + x * 0.1, -1.5);
            }
            line(&|x| 60.0 + x * 0.05, 1.5);
            Ok(v)
        }
    }

    #[test]
    fn learns_the_dark_lines_in_3d() {
        let (w, h, d) = (128usize, 140usize, 36usize);
        let img = Two { w, h };
        let z0 = 15;
        let off = z0 as f32 * 0.5;
        let path: Vec<[f32; 2]> = (12..116).step_by(3).map(|x| [x as f32 + 0.5, 20.0 + off + x as f32 * 0.2]).collect();
        let mark = Mark { z: z0, path, thickness: 3.0, inside: 1, closed: false };
        let pos = rasterize(w, h, std::slice::from_ref(&mark));
        CANCEL.store(false, Ordering::Relaxed);
        let ex = Examples { z: z0, pos: &pos, thick: 3.0, not: vec![] };
        let l = learn(&img, (w, h, d), &ex, None).unwrap();
        let r = Reader::new(&l, &img);
        let ps = r.pieces(&|_, _| {}).unwrap();
        let near = edt(&pos, w, h);
        let own: Vec<&Piece> = ps.iter().filter(|p| touches(p, z0, &near, 4.0)).collect();
        assert!(!own.is_empty(), "the example's own piece");
        let (a, b) = own.iter().fold((usize::MAX, 0), |(a, b), p| (a.min(p.zs().0), b.max(p.zs().1)));
        assert!(a <= 8 && b >= 27, "own piece spans {a}..{b}");
        // The other dark line is a separate piece, found; the bright one is not.
        let others: Vec<&Piece> = ps.iter().filter(|p| !touches(p, z0, &near, 4.0) && p.zs().1 - p.zs().0 >= 3).collect();
        let at = |p: &Piece, z: usize| p.px.get(&z).map(|px| px.iter().map(|&i| (i as usize / w) as f32).sum::<f32>() / px.len() as f32);
        assert!(others.iter().any(|p| at(p, 16).is_some_and(|y| (95.0..115.0).contains(&y))), "other dark line");
        assert!(!ps.iter().any(|p| p.voxels() > 200 && at(p, 16).is_some_and(|y| (58.0..66.0).contains(&y))), "bright line left out");
        // Drawn as lines on the other slices, inside side kept.
        let (out, marks) = r.follow(z0, std::slice::from_ref(&mark), &pos, "membrane", &BTreeMap::new(), &|_| {}).unwrap();
        assert!(out.len() >= 15, "slices drawn {}", out.len());
        let (a, b) = (*out.keys().next().unwrap(), *out.keys().next_back().unwrap());
        assert!(a >= 3 && b <= 31, "stops where the line ends: {a}..{b}");
        for m in marks.iter().filter(|m| (8..27).contains(&m.z)) {
            let off = m.z as f32 * 0.5;
            let err: f32 = m.path.iter().map(|p| (p[1] - (20.0 + off + (p[0] - 0.5) * 0.2)).abs()).sum::<f32>() / m.path.len() as f32;
            assert!(err < 2.5, "z {} err {err}", m.z);
            assert_eq!(m.inside, 1, "inside at z {}", m.z);
        }
    }

    /// On real data, as `follow::tests::real`: TMV_FOLLOW_IMG=<tomogram>
    /// [TMV_FOLLOW_Z] [TMV_FOLLOW_OUT=<folder>] cargo test --release
    /// --features pro --lib structure::learn::tests::real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real() {
        use crate::labels::Labels;
        let Ok(path) = std::env::var("TMV_FOLLOW_IMG") else { return };
        let img = crate::image::OpenImage::new(1, crate::formats::open(std::path::Path::new(&path)).unwrap(), 256 << 20);
        let info = img.info();
        let (w, h, d) = (info.width, info.height, info.size_z);
        let labels = Labels::load(std::path::Path::new(&crate::labels::labels_path(&path, 0)), w, h, d).unwrap();
        let m = labels.materials.iter().find(|m| !m.structure.is_empty() && !m.marks.is_empty()).expect("a typed layer with refined objects");
        let z = std::env::var("TMV_FOLLOW_Z").ok().and_then(|v| v.parse().ok()).unwrap_or(m.marks[0].z);
        let marks: Vec<Mark> = m.marks.iter().filter(|k| k.z == z).cloned().collect();
        let pos: Vec<bool> = labels.slice(z).unwrap().iter().map(|&v| v == m.label).collect();
        let thick = super::super::median(marks.iter().map(|k| k.thickness).collect()).max(1.5);
        println!("{path}: {w}x{h}x{d}, layer {} ({}) z {z}: {} marks, {} px, thick {thick}", m.name, m.structure, marks.len(), pos.iter().filter(|&&b| b).count());
        let vals = crate::labels::ChannelValues { img: img.clone(), c: 0, t: 0 };
        CANCEL.store(false, Ordering::Relaxed);
        let t0 = std::time::Instant::now();
        let ex = Examples { z, pos: &pos, thick, not: vec![] };
        let l = learn(&vals, (w, h, d), &ex, None).unwrap();
        let r = Reader::new(&l, &vals);
        let ps = r.pieces(&|_, _| {}).unwrap();
        println!("pieces: {} in {:.1} s ({:.2} s/slice)", ps.len(), t0.elapsed().as_secs_f32(), t0.elapsed().as_secs_f32() / d as f32);
        let near = edt(&pos, w, h);
        let own: Vec<&Piece> = ps.iter().filter(|p| touches(p, z, &near, thick + 2.0)).collect();
        for p in &own {
            println!("  own: z {:?}, {} voxels, p {:.2}", p.zs(), p.voxels(), p.prob);
        }
        let mut others: Vec<&Piece> = ps.iter().filter(|p| !touches(p, z, &near, thick + 2.0) && p.zs().1 - p.zs().0 >= 2).collect();
        others.sort_by(|a, b| b.score().total_cmp(&a.score()));
        println!("others: {} (z-extent >= 3)", others.len());
        for p in others.iter().take(15) {
            println!("  z {:?}, {} voxels, p {:.2}, score {:.2}", p.zs(), p.voxels(), p.prob, p.score());
        }
        let t1 = std::time::Instant::now();
        let (out, found) = r.follow(z, &marks, &pos, &m.structure, &BTreeMap::new(), &|_| {}).unwrap();
        let zr = (out.keys().next().copied(), out.keys().next_back().copied());
        println!("draw: {} slices {:?}, {} lines, {:.1} s", out.len(), zr, found.len(), t1.elapsed().as_secs_f32());
        if let Ok(dir) = std::env::var("TMV_FOLLOW_OUT") {
            let dir = format!("{dir}/learn");
            std::fs::create_dir_all(&dir).unwrap();
            // Grey slice; red: drawn lines; green: own pieces; yellow: the 15 best others.
            for zz in (0..d).step_by(6).chain([z]) {
                let v = vals.slice(zz).unwrap();
                let (med, sd) = crate::segpick::median_sd(&v);
                let mut im = image::RgbImage::new(w as u32, h as u32);
                for (i, p) in v.iter().enumerate() {
                    let g = (((p - med) / sd * 40.0) + 128.0).clamp(0.0, 255.0) as u8;
                    im.put_pixel((i % w) as u32, (i / w) as u32, image::Rgb([g, g, g]));
                }
                let mut tint = |px: &[u32], c: [u8; 3]| {
                    for &i in px {
                        let (x, y) = (i as usize % w, i as usize / w);
                        let o = im.get_pixel(x as u32, y as u32).0;
                        im.put_pixel(x as u32, y as u32, image::Rgb([((o[0] as u16 + c[0] as u16) / 2) as u8, ((o[1] as u16 + c[1] as u16) / 2) as u8, ((o[2] as u16 + c[2] as u16) / 2) as u8]));
                    }
                };
                for p in &own {
                    if let Some(px) = p.px.get(&zz) {
                        tint(px, [0, 255, 0]);
                    }
                }
                for p in others.iter().take(15) {
                    if let Some(px) = p.px.get(&zz) {
                        tint(px, [255, 220, 0]);
                    }
                }
                let lines: Vec<u32> = if zz == z { pos.iter().enumerate().filter(|(_, &b)| b).map(|(i, _)| i as u32).collect() } else { out.get(&zz).map(|s| s.iter().enumerate().filter(|(_, &b)| b).map(|(i, _)| i as u32).collect()).unwrap_or_default() };
                for i in lines {
                    im.put_pixel((i as usize % w) as u32, (i as usize / w) as u32, image::Rgb([255, 0, 0]));
                }
                im.save(format!("{dir}/z{zz:03}.png")).unwrap();
            }
            println!("pictures in {dir}");
        }
    }
}
