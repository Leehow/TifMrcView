//! 沿 Z 找全: carries the objects a layer has on one slice through the stack.
//!
//! No new detector: on each next slice the object found on the slice before
//! is the rough paint, and the same refinement the user's type click runs
//! (`refine`) finds it there. A membrane moves a little from slice to slice,
//! so the band the line engine searches (a paint width beyond the paint)
//! follows it; where the object fades out, shrinks to nothing or jumps away
//! the following stops on that side.

use super::{median, refine, Outcome};
use crate::labels::{edt, Labels, Mark, SearchRange, Values};
use anyhow::Result;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// Types refined into lines: the next slice's paint is the line found,
/// widened a little; the written pixels are the lines only (not the rough
/// paint the engine keeps where a stretch is unclear).
fn is_line(structure: &str) -> bool {
    matches!(structure, "membrane" | "membrane2" | "fibre" | "shell")
}

pub struct Followed {
    /// Pixels to write per slice (only slices other than the start).
    pub slices: BTreeMap<usize, Vec<bool>>,
    /// The objects found on those slices.
    pub marks: Vec<Mark>,
    /// Lowest and highest slice reached.
    pub range: (usize, usize),
}

/// Slices read once: the line engine averages a few neighbours, so each
/// step would otherwise read five slices again.
pub(super) struct Cached<'a> {
    pub(super) vals: &'a dyn Values,
    pub(super) keep: Mutex<Vec<(usize, Vec<f32>)>>,
}

impl Values for Cached<'_> {
    fn slice(&self, z: usize) -> Result<Vec<f32>> {
        if let Some((_, v)) = self.keep.lock().unwrap().iter().find(|(k, _)| *k == z) {
            return Ok(v.clone());
        }
        let v = self.vals.slice(z)?;
        let mut k = self.keep.lock().unwrap();
        if k.len() >= 24 {
            k.remove(0);
        }
        k.push((z, v.clone()));
        Ok(v)
    }
}

/// How far (pixels) a line may grow at each end from one slice to the next.
const EXTEND: f32 = 10.0;

/// An open line made `e` pixels longer at both ends, straight on.
fn extended(m: &Mark, e: f32) -> Mark {
    let p = &m.path;
    if m.closed || p.len() < 2 {
        return m.clone();
    }
    let k = 3.min(p.len() - 1);
    let out = |a: [f32; 2], b: [f32; 2]| {
        let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
        let n = (dx * dx + dy * dy).sqrt().max(1e-6);
        [a[0] + dx / n * e, a[1] + dy / n * e]
    };
    let mut path = vec![out(p[0], p[k])];
    path.extend(p.iter().copied());
    path.push(out(p[p.len() - 1], p[p.len() - 1 - k]));
    Mark { path, ..m.clone() }
}

/// Whether `m` runs along the lines it came from: at most points the
/// nearest old line (within `reach`) has about the same direction.
fn same_course(m: &Mark, old: &[Mark], reach: f32) -> bool {
    if m.path.len() < 2 || old.is_empty() {
        return true;
    }
    let (mut along, mut n) = (0, 0);
    for &p in &m.path {
        let Some((a, _)) = normal_near(m, p) else { continue };
        let Some((b, d)) = old.iter().filter_map(|o| normal_near(o, p)).min_by(|x, y| x.1.total_cmp(&y.1)) else { continue };
        n += 1;
        if d <= reach && (a[0] * b[0] + a[1] * b[1]).abs() >= 0.75 {
            along += 1;
        }
    }
    n == 0 || along * 10 >= n * 7
}

/// The pixels of refined lines.
pub(super) fn rasterize(w: usize, h: usize, marks: &[Mark]) -> Vec<bool> {
    let mut tmp = Labels::new(w, h, 1);
    for m in marks {
        let mut pts = m.path.clone();
        if m.closed && pts.len() > 2 {
            pts.push(pts[0]);
        }
        tmp.stroke(0, &pts, (m.thickness / 2.0).max(0.75), 1, 0);
    }
    tmp.slice(0).map_or_else(|| vec![false; w * h], |s| s.iter().map(|&v| v == 1).collect())
}

fn count(m: &[bool]) -> usize {
    m.iter().filter(|&&b| b).count()
}

/// Inside-normal (unit) of mark `m` near point `p`, and the distance to it.
fn normal_near(m: &Mark, p: [f32; 2]) -> Option<([f32; 2], f32)> {
    if m.path.len() < 2 {
        return None;
    }
    let (k, d) = m.path.iter().enumerate().map(|(k, q)| (k, (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2))).min_by(|a, b| a.1.total_cmp(&b.1))?;
    let (q0, q1) = (m.path[k.saturating_sub(1)], m.path[(k + 1).min(m.path.len() - 1)]);
    let (tx, ty) = (q1[0] - q0[0], q1[1] - q0[1]);
    let n = (tx * tx + ty * ty).sqrt().max(1e-6);
    // Left of the direction of travel (image y down), as the panel reads it.
    Some(([-ty / n, tx / n], d.sqrt()))
}

/// Gives each new open line the inside of the line it came from: the side
/// is a direction in the image, the new path may run the other way. Each
/// point of the new line votes with the nearest old line there (a single
/// point near a bend or an end can read the wrong way).
fn carry_inside(new: &mut [Mark], old: &[Mark]) {
    let olds: Vec<&Mark> = old.iter().filter(|o| o.inside != 0 && !o.closed && o.path.len() >= 2).collect();
    if olds.is_empty() {
        return;
    }
    for m in new.iter_mut().filter(|m| !m.closed && m.path.len() >= 2) {
        let mut vote = 0f32;
        for &p in m.path.iter() {
            let Some((n_new, _)) = normal_near(m, p) else { continue };
            let Some((o, n_old, d)) = olds.iter().filter_map(|o| normal_near(o, p).map(|(n, d)| (*o, n, d))).min_by(|a, b| a.2.total_cmp(&b.2)) else { continue };
            // Far points (an end that grew) say little.
            let wgt = 1.0 / (1.0 + d / 4.0);
            vote += wgt * (n_old[0] * n_new[0] + n_old[1] * n_new[1]) * o.inside as f32;
        }
        if vote != 0.0 {
            m.inside = if vote > 0.0 { 1 } else { -1 };
        }
    }
}

/// Follows every object of `start` (pixels of the layer on slice `z0`,
/// pieces within `gap` are one object; `marks`: the layer's refined objects
/// on `z0`) up and down the stack as `structure`. `progress(slices done)`.
/// `existing`: the layer's pixels on other slices (an earlier run, the
/// user's paint): a line already there is not drawn a second time.
#[allow(clippy::too_many_arguments)]
pub fn follow_z(vals: &dyn Values, dims: (usize, usize, usize), z0: usize, start: &[bool], marks: &[Mark], structure: &str, range: Option<&SearchRange>, existing: &BTreeMap<usize, Vec<bool>>, progress: &dyn Fn(usize)) -> Result<Followed> {
    let (w, h, d) = dims;
    // Only inside the layer's range, when it has one.
    let inside = range.map(|r| r.mask(w, h));
    let (zlo, zhi) = range.map_or((0, d - 1), |r| (r.z0.min(d - 1), r.z1.min(d - 1).max(r.z0.min(d - 1))));
    let vals = Cached { vals, keep: Mutex::new(vec![]) };
    let line = is_line(structure);
    // Two lines of a double membrane are one object: join across their gap.
    let thick = marks.iter().map(|m| m.thickness).fold(2f32, f32::max);
    // The width the user's object has on its own slice.
    let thick0 = if marks.is_empty() { thick } else { median(marks.iter().map(|m| m.thickness).collect()).max(1.5) };
    let spacing = if structure == "membrane2" && marks.len() >= 2 {
        let a = rasterize(w, h, &marks[..1]);
        let da = edt(&a, w, h);
        let b = rasterize(w, h, &marks[1..]);
        let mut ds: Vec<f32> = b.iter().enumerate().filter(|(_, &v)| v).map(|(i, _)| da[i]).filter(|v| *v < 60.0).collect();
        ds.sort_by(f32::total_cmp);
        ds.get(ds.len() / 2).copied().unwrap_or(0.0)
    } else {
        0.0
    };
    let gap = (2.0 * thick).max(6.0).max(spacing + 4.0);
    let near0 = edt(start, w, h);
    let objects = crate::labels::components_where(w, h, |i| near0[i] <= gap / 2.0);

    let mut out: BTreeMap<usize, Vec<bool>> = BTreeMap::new();
    let mut found = vec![];
    let mut range = (z0, z0);
    let mut steps = 0usize;
    for part in objects {
        let px: Vec<usize> = part.into_iter().filter(|&i| start[i]).collect();
        if px.len() < 6 {
            continue;
        }
        let mut obj = vec![false; w * h];
        px.iter().for_each(|&i| obj[i] = true);
        let near_obj = edt(&obj, w, h);
        let own: Vec<Mark> = marks
            .iter()
            .filter(|m| m.path.iter().any(|p| {
                let (x, y) = (p[0] as usize, p[1] as usize);
                x < w && y < h && near_obj[y * w + x] <= gap
            }))
            .cloned()
            .collect();
        // The object on its first slice, as the engine draws it.
        let first = if line && !own.is_empty() { rasterize(w, h, &own) } else { obj.clone() };
        let n0 = count(&first).max(1);
        for dir in [-1i64, 1] {
            let (mut prev, mut prev_marks) = (first.clone(), own.clone());
            let mut z = z0 as i64 + dir;
            while z >= zlo as i64 && z <= zhi as i64 {
                if CANCEL.load(Ordering::Relaxed) {
                    return Ok(Followed { slices: out, marks: found, range });
                }
                let zu = z as usize;
                // A line's paint is the line before, a little wider so the band
                // may slide with it and longer at its ends so it is not cut a
                // little shorter on every slice (the trimmed ends grow back
                // where the line goes on). A filled object is its own outline.
                let paint: Vec<bool> = if line && !prev_marks.is_empty() {
                    // Grown well past its first length: no more growing (a
                    // line that keeps growing wanders into other things).
                    let e = if count(&prev) as f32 > 1.3 * n0 as f32 + 30.0 { 0.0 } else { EXTEND };
                    let longer: Vec<Mark> = prev_marks.iter().map(|m| extended(m, e)).collect();
                    let base = rasterize(w, h, &longer);
                    edt(&base, w, h).iter().map(|&v| v <= 1.5 + spacing / 2.0).collect()
                } else {
                    prev.clone()
                };
                let dist = edt(&paint, w, h);
                let r = refine(&vals, dims, zu, &paint, gap, structure)?;
                let Outcome::Done(mask, mut new_marks, _) = r else { break };
                if line {
                    // A membrane keeps its direction from slice to slice: a
                    // piece running across the old line is something else
                    // (another membrane, a streak) and is left out.
                    new_marks.retain(|m| same_course(m, &prev_marks, (2.0 * thick).max(5.0) + spacing + EXTEND));
                }
                if new_marks.is_empty() && line {
                    break;
                }
                let mut new = if line { rasterize(w, h, &new_marks) } else { mask };
                if let Some(m) = &inside {
                    new.iter_mut().zip(m).for_each(|(v, &k)| *v &= k);
                }
                let n = count(&new);
                // Faded (much less of it left), grown into something else,
                // or jumped away from where it was.
                // A sudden halving counts only when little is left (one of two
                // lines of an object ending is no reason to drop the other).
                if n < (0.35 * n0 as f32) as usize || (2 * n < count(&prev) && 10 * n < 6 * n0) || n > 3 * count(&prev) + 50 {
                    break;
                }
                // A line that turns into a broad smear (end of the lamella,
                // missing-wedge blur) is no longer the membrane.
                if line && !new_marks.is_empty() {
                    let t = median(new_marks.iter().map(|m| m.thickness).collect());
                    if t > (2.0 * thick0).max(thick0 + 3.0) {
                        break;
                    }
                }
                let reach = (2.0 * thick).max(5.0) + spacing;
                let close = new.iter().zip(&dist).filter(|(&v, &dd)| v && dd <= reach).count();
                if (close as f32) < 0.7 * n as f32 {
                    break;
                }
                for m in new_marks.iter_mut() {
                    m.z = zu;
                }
                if structure != "shell" {
                    carry_inside(&mut new_marks, &prev_marks);
                }
                // Already there (an earlier run, the user's paint, another
                // object of this run on the same membrane): followed on, but
                // not drawn twice.
                let mut taken: Vec<bool> = existing.get(&zu).cloned().unwrap_or_else(|| vec![false; w * h]);
                if let Some(o) = out.get(&zu) {
                    taken.iter_mut().zip(o).for_each(|(t, &v)| *t |= v);
                }
                let fresh: Vec<Mark> = if taken.iter().any(|&v| v) {
                    let dt = edt(&taken, w, h);
                    let lim = (2.0 * thick).max(4.0) + 2.0;
                    new_marks
                        .iter()
                        .filter(|m| {
                            let on = m.path.iter().filter(|p| (p[0] as usize) < w && (p[1] as usize) < h && dt[p[1] as usize * w + p[0] as usize] <= lim).count();
                            2 * on < m.path.len()
                        })
                        .cloned()
                        .collect()
                } else {
                    new_marks.clone()
                };
                let draw = if line { rasterize(w, h, &fresh) } else if fresh.is_empty() && !new_marks.is_empty() { vec![false; w * h] } else { new.clone() };
                let slot = out.entry(zu).or_insert_with(|| vec![false; w * h]);
                slot.iter_mut().zip(&draw).zip(inside.iter().flatten().chain(std::iter::repeat(&true))).for_each(|((o, &v), &k)| *o |= v && k);
                found.extend(fresh.iter().cloned());
                range = (range.0.min(zu), range.1.max(zu));
                steps += 1;
                progress(steps);
                prev = new;
                prev_marks = new_marks;
                z += dir;
            }
        }
    }
    Ok(Followed { slices: out, marks: found, range })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark tilted line whose position drifts with z, on noise-free grey.
    struct Tilted {
        w: usize,
        h: usize,
        /// Slices where the line exists.
        zs: std::ops::Range<usize>,
    }

    impl Values for Tilted {
        fn slice(&self, z: usize) -> Result<Vec<f32>> {
            let mut v = vec![0f32; self.w * self.h];
            for y in 0..self.h {
                for x in 0..self.w {
                    // Hash noise so the robust scale is not zero.
                    let n = (((x * 7919 + y * 104729 + z * 1299709) % 1000) as f32 / 1000.0 - 0.5) * 0.4;
                    v[y * self.w + x] = 1.0 + n;
                }
            }
            if self.zs.contains(&z) {
                let off = z as f32 * 0.5;
                for x in 10..self.w - 10 {
                    let yc = 30.0 + off + x as f32 * 0.3;
                    for y in 0..self.h {
                        let dy = y as f32 + 0.5 - yc;
                        v[y * self.w + x] -= 1.5 * (-dy * dy / (2.0 * 1.2 * 1.2)).exp();
                    }
                }
            }
            Ok(v)
        }
    }

    #[test]
    fn membrane_followed_until_it_ends() {
        let (w, h, d) = (120usize, 120usize, 40usize);
        let img = Tilted { w, h, zs: 5..30 };
        let z0 = 15;
        // The refined line on z0 (what a type click leaves).
        let off = z0 as f32 * 0.5;
        let path: Vec<[f32; 2]> = (12..108).step_by(3).map(|x| [x as f32 + 0.5, 30.0 + off + x as f32 * 0.3]).collect();
        let mark = Mark { z: z0, path, thickness: 3.0, inside: 1, closed: false };
        let start = rasterize(w, h, std::slice::from_ref(&mark));
        CANCEL.store(false, Ordering::Relaxed);
        let f = follow_z(&img, (w, h, d), z0, &start, &[mark], "membrane", None, &Default::default(), &|_| {}).unwrap();
        // Reaches near both ends of the line's range, not far past them.
        assert!(f.range.0 <= 8 && f.range.0 >= 3, "low end {:?}", f.range);
        assert!(f.range.1 >= 27 && f.range.1 <= 32, "high end {:?}", f.range);
        // Each slice's line sits on the true line.
        for m in f.marks.iter().filter(|m| (6..29).contains(&m.z)) {
            let off = m.z as f32 * 0.5;
            let err: f32 = m.path.iter().map(|p| (p[1] - (30.0 + off + (p[0] - 0.5) * 0.3)).abs()).sum::<f32>() / m.path.len() as f32;
            assert!(err < 2.0, "z {} err {err}", m.z);
            assert_eq!(m.inside, 1, "inside kept at z {}", m.z);
        }
    }

    /// On real data: TMV_FOLLOW_IMG=<tomogram> [TMV_FOLLOW_Z=<0-based>]
    /// cargo test --release --lib follow::tests::real -- --ignored --nocapture
    /// Follows the first typed layer of the image's label file from the
    /// slice holding its refined objects, and finds lines like them there.
    #[test]
    #[ignore]
    fn real() {
        let Ok(path) = std::env::var("TMV_FOLLOW_IMG") else { return };
        let img = crate::image::OpenImage::new(1, crate::formats::open(std::path::Path::new(&path)).unwrap(), 256 << 20);
        let info = img.info();
        let (w, h, d) = (info.width, info.height, info.size_z);
        let labels = Labels::load(std::path::Path::new(&crate::labels::labels_path(&path, 0)), w, h, d).unwrap();
        let m = labels.materials.iter().find(|m| !m.structure.is_empty() && !m.marks.is_empty()).expect("a typed layer with refined objects");
        let z = std::env::var("TMV_FOLLOW_Z").ok().and_then(|v| v.parse().ok()).unwrap_or(m.marks[0].z);
        let marks: Vec<Mark> = m.marks.iter().filter(|k| k.z == z).cloned().collect();
        let start: Vec<bool> = labels.slice(z).unwrap().iter().map(|&v| v == m.label).collect();
        println!("{path}: {w}x{h}x{d}, layer {} ({}) z {z}: {} marks, {} px", m.name, m.structure, marks.len(), count(&start));
        let vals = crate::labels::ChannelValues { img: img.clone(), c: 0, t: 0 };
        let t0 = std::time::Instant::now();
        let f = follow_z(&vals, (w, h, d), z, &start, &marks, &m.structure, None, &Default::default(), &|_| {}).unwrap();
        println!("follow: {} slices, range {:?}, {:.1} s", f.slices.len(), f.range, t0.elapsed().as_secs_f32());
        for (zz, mk) in f.slices.iter().step_by(5) {
            let ms: Vec<&Mark> = f.marks.iter().filter(|k| k.z == *zz).collect();
            println!("  z {zz}: {} px, {} lines, thick {:?}, inside {:?}", count(mk), ms.len(), ms.iter().map(|k| k.thickness).collect::<Vec<_>>(), ms.iter().map(|k| k.inside).collect::<Vec<_>>());
        }
        // Pictures: TMV_FOLLOW_OUT=<folder> gets every 8th slice with the
        // followed lines in red and their inside side as short blue ticks.
        if let Ok(dir) = std::env::var("TMV_FOLLOW_OUT") {
            use crate::labels::Values;
            std::fs::create_dir_all(&dir).unwrap();
            for zz in (f.range.0..=f.range.1).step_by(8).chain([z]) {
                let v = vals.slice(zz).unwrap();
                let (med, sd) = crate::segpick::median_sd(&v);
                let mut im = image::RgbImage::new(w as u32, h as u32);
                for (i, p) in v.iter().enumerate() {
                    let g = (((p - med) / sd * 40.0) + 128.0).clamp(0.0, 255.0) as u8;
                    im.put_pixel((i % w) as u32, (i / w) as u32, image::Rgb([g, g, g]));
                }
                let own: Vec<&Mark> = if zz == z { marks.iter().collect() } else { f.marks.iter().filter(|k| k.z == zz).collect() };
                for mk in own {
                    for (k, p) in mk.path.iter().enumerate() {
                        let (x, y) = (p[0] as u32, p[1] as u32);
                        if x < w as u32 && y < h as u32 {
                            im.put_pixel(x, y, image::Rgb([255, 0, 0]));
                        }
                        if k % 4 == 2 && mk.inside != 0 {
                            if let Some((n, _)) = normal_near(mk, *p) {
                                for s in 2..8 {
                                    let (qx, qy) = (p[0] + n[0] * s as f32 * mk.inside as f32, p[1] + n[1] * s as f32 * mk.inside as f32);
                                    if qx >= 0.0 && qy >= 0.0 && (qx as usize) < w && (qy as usize) < h {
                                        im.put_pixel(qx as u32, qy as u32, image::Rgb([0, 120, 255]));
                                    }
                                }
                            }
                        }
                    }
                }
                im.save(format!("{dir}/z{zz:03}.png")).unwrap();
            }
        }
        // 找所有类似, leave one out: the object with most lines is the
        // example, the user's other objects on the slice are the answer.
        let near0 = edt(&start, w, h);
        let objs = crate::labels::components_where(w, h, |i| near0[i] <= 6.0);
        let objs: Vec<Vec<usize>> = objs.into_iter().map(|o| o.into_iter().filter(|&i| start[i]).collect::<Vec<_>>()).filter(|o| o.len() >= 6).collect();
        println!("objects on z {z}: {:?} px", objs.iter().map(|o| o.len()).collect::<Vec<_>>());
        for (ei, ex_obj) in objs.iter().enumerate() {
            let mut ex_px = vec![false; w * h];
            ex_obj.iter().for_each(|&i| ex_px[i] = true);
            let dex = edt(&ex_px, w, h);
            let ex_marks: Vec<Mark> = marks.iter().filter(|m| m.path.iter().any(|p| (p[0] as usize) < w && (p[1] as usize) < h && dex[p[1] as usize * w + p[0] as usize] <= 4.0)).cloned().collect();
            let mut answer = start.clone();
            ex_obj.iter().for_each(|&i| answer[i] = false);
            let dans = edt(&answer, w, h);
            let t0 = std::time::Instant::now();
            let found = super::super::similar::lines(&vals, (w, h, d), z, &ex_marks, &ex_px, None, 200).unwrap();
            println!("example = object {ei} ({} lines): {} candidates in {:.1} s", ex_marks.len(), found.len(), t0.elapsed().as_secs_f32());
            for (k, c) in found.iter().enumerate().take(20) {
                let [x0, y0, bw, _] = c.rect;
                let (mut on, mut n) = (0, 0);
                for (j, _) in c.mask.iter().enumerate().filter(|(_, &v)| v) {
                    n += 1;
                    if dans[(y0 + j / bw) * w + x0 + j % bw] <= 4.0 {
                        on += 1;
                    }
                }
                println!("  #{k}: score {:.2}, {} px, on the answer {:.0}%", c.score, n, 100.0 * on as f32 / n.max(1) as f32);
                // TMV_FOLLOW_OUT: a crop around each candidate of the first
                // example, raw on the left, the candidate in red on the right.
                if let (0, Ok(dir)) = (ei, std::env::var("TMV_FOLLOW_OUT")) {
                    use crate::labels::Values;
                    let v = vals.slice(z).unwrap();
                    let (med, sd) = crate::segpick::median_sd(&v);
                    let (cx, cy) = ((x0 + bw / 2) as i64, (y0 + c.rect[3] / 2) as i64);
                    let half = 80i64;
                    let mut im = image::RgbImage::new(4 * half as u32, 2 * half as u32);
                    for yy in 0..2 * half {
                        for xx in 0..2 * half {
                            let (x, y) = (cx - half + xx, cy - half + yy);
                            let g = if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h { (((v[y as usize * w + x as usize] - med) / sd * 40.0) + 128.0).clamp(0.0, 255.0) as u8 } else { 0 };
                            im.put_pixel(xx as u32, yy as u32, image::Rgb([g, g, g]));
                            let on_c = x >= x0 as i64 && y >= y0 as i64 && ((x - x0 as i64) as usize) < bw && ((y - y0 as i64) as usize) < c.rect[3] && c.mask[(y as usize - y0) * bw + x as usize - x0];
                            im.put_pixel((xx + 2 * half) as u32, yy as u32, if on_c { image::Rgb([255, 0, 0]) } else { image::Rgb([g, g, g]) });
                        }
                    }
                    let im = image::imageops::resize(&im, 8 * half as u32, 4 * half as u32, image::imageops::FilterType::Nearest);
                    im.save(format!("{dir}/cand{k:02}.png")).unwrap();
                }
            }
        }
    }
}
