//! The brush of painted examples (`Kind::Paint` of `marks`): the user paints
//! over what to find on a section, and what was painted is the example's
//! mask. Each stroke is a round brush of the stroke's own `width` (voxels)
//! dragged along its points; a stroke that ends within one brush width of
//! where it began is a closed loop and its inside is filled, so circling a
//! thing works as well as painting it. A cut stroke (Alt) takes away what
//! it covers. Strokes act in the order they were made, per section.
//!
//! Everything is in voxels of the file the strokes were drawn on, a voxel's
//! centre being its integer index (as the clicks are). A raster covers only
//! the strokes' bounding box: nothing here needs the size of the file, and
//! a stroke over the edge is clipped where the voxels are read (`runs`).
//!
//! Only the painted sections are rasterised. The selection through z that
//! follows the thing up and down is a separate step; it will replace the
//! raster of a section per mark, which is why callers ask for one section
//! at a time (`section`).

use super::paint::Stroke;

/// The painted voxels of one section.
#[derive(Clone, Debug, PartialEq)]
pub struct Raster {
    pub z: usize,
    /// Voxel (x, y) of the first cell of `on`.
    x0: i64,
    y0: i64,
    w: usize,
    h: usize,
    on: Vec<bool>,
}

impl Raster {
    fn blank(z: usize, lo: [f64; 2], hi: [f64; 2]) -> Raster {
        let (x0, y0) = (lo[0].floor() as i64, lo[1].floor() as i64);
        let (x1, y1) = (hi[0].ceil() as i64, hi[1].ceil() as i64);
        let (w, h) = ((x1 - x0 + 1).max(1) as usize, (y1 - y0 + 1).max(1) as usize);
        Raster { z, x0, y0, w, h, on: vec![false; w * h] }
    }

    /// Whether voxel (x, y) is painted.
    pub fn contains(&self, x: i64, y: i64) -> bool {
        let (i, j) = (x - self.x0, y - self.y0);
        i >= 0 && j >= 0 && (i as usize) < self.w && (j as usize) < self.h && self.on[j as usize * self.w + i as usize]
    }

    /// How many voxels are painted.
    pub fn voxels(&self) -> usize {
        self.on.iter().filter(|v| **v).count()
    }

    /// The painted voxels (x, y), row by row.
    pub fn cells(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.on.iter().enumerate().filter(|(_, v)| **v).map(|(k, _)| (self.x0 + (k % self.w) as i64, self.y0 + (k / self.w) as i64))
    }

    /// The runs `[y, x0, x1)` (x1 excluded) of the painted voxels, the form
    /// the overlay of the membranes takes (`paint::section_runs`); voxels
    /// left of or above the image are dropped.
    pub fn runs(&self) -> Vec<[usize; 3]> {
        let mut out: Vec<[usize; 3]> = vec![];
        for (x, y) in self.cells().filter(|(x, y)| *x >= 0 && *y >= 0) {
            let (x, y) = (x as usize, y as usize);
            match out.last_mut() {
                Some(l) if l[0] == y && l[2] == x => l[2] = x + 1,
                _ => out.push([y, x, x + 1]),
            }
        }
        out
    }

    /// The mean position of the painted voxels.
    pub fn centroid(&self) -> Option<[f64; 2]> {
        let (mut n, mut sx, mut sy) = (0.0, 0.0, 0.0);
        for (x, y) in self.cells() {
            n += 1.0;
            sx += x as f64;
            sy += y as f64;
        }
        (n > 0.0).then(|| [sx / n, sy / n])
    }
}

/// The smallest brush radius: a dab of any width takes at least the voxel
/// it is on.
const LEAST_RADIUS: f64 = 0.75;

fn radius(width: f64) -> f64 {
    (width / 2.0).max(LEAST_RADIUS)
}

/// The section a point lies on.
fn section_of(p: &[f64; 3]) -> usize {
    p[2].round().max(0.0) as usize
}

/// One stroke's points on one section (a stroke is drawn on one section; if
/// its points are not, each run of points on a section counts on its own).
struct Piece {
    points: Vec<[f64; 2]>,
    width: f64,
    cut: bool,
}

fn pieces_on(strokes: &[Stroke], z: usize) -> Vec<Piece> {
    let mut out = vec![];
    for s in strokes {
        let mut run: Vec<[f64; 2]> = vec![];
        for p in &s.points {
            if section_of(p) == z {
                run.push([p[0], p[1]]);
            } else if !run.is_empty() {
                out.push(Piece { points: std::mem::take(&mut run), width: s.width, cut: s.cut });
            }
        }
        if !run.is_empty() {
            out.push(Piece { points: run, width: s.width, cut: s.cut });
        }
    }
    out
}

/// The sections that have strokes, in order.
pub fn sections(strokes: &[Stroke]) -> Vec<usize> {
    let mut zs: Vec<usize> = strokes.iter().flat_map(|s| s.points.iter().map(section_of)).collect();
    zs.sort_unstable();
    zs.dedup();
    zs
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Distance of point `p` from the segment `a`–`b`.
fn from_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

/// A stroke that ends within one brush width of its start, after more than
/// a dab's worth of points, is a loop.
fn is_loop(p: &Piece) -> bool {
    p.points.len() >= 3 && dist(p.points[0], p.points[p.points.len() - 1]) <= p.width.max(1.0)
}

/// Sets (or clears) the voxels a piece covers: the brush along its points,
/// and the inside of the loop when it is one.
fn stamp(r: &mut Raster, p: &Piece, value: bool) {
    let rad = radius(p.width);
    let n = p.points.len();
    let mut set = |x: i64, y: i64| {
        let (i, j) = (x - r.x0, y - r.y0);
        if i >= 0 && j >= 0 && (i as usize) < r.w && (j as usize) < r.h {
            r.on[j as usize * r.w + i as usize] = value;
        }
    };
    // The brush: every voxel centre within `rad` of a segment (or of the one point of a dab).
    for k in 0..n.max(1) {
        let a = p.points[k];
        let b = p.points[(k + 1).min(n - 1)];
        let (lx, hx) = (a[0].min(b[0]) - rad, a[0].max(b[0]) + rad);
        let (ly, hy) = (a[1].min(b[1]) - rad, a[1].max(b[1]) + rad);
        for y in ly.ceil() as i64..=hy.floor() as i64 {
            for x in lx.ceil() as i64..=hx.floor() as i64 {
                if from_segment([x as f64, y as f64], a, b) <= rad {
                    set(x, y);
                }
            }
        }
    }
    if !is_loop(p) {
        return;
    }
    // The inside of the loop closed by its last point back to its first: even-odd, row by row.
    let (ly, hy) = p.points.iter().fold((f64::MAX, f64::MIN), |(l, h), q| (l.min(q[1]), h.max(q[1])));
    for y in ly.ceil() as i64..=hy.floor() as i64 {
        let yf = y as f64;
        let mut xs: Vec<f64> = vec![];
        for k in 0..n {
            let (a, b) = (p.points[k], p.points[(k + 1) % n]);
            if (a[1] <= yf) != (b[1] <= yf) {
                xs.push(a[0] + (yf - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
            }
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.chunks_exact(2) {
            for x in pair[0].ceil() as i64..=pair[1].floor() as i64 {
                set(x, y);
            }
        }
    }
}

/// What is painted on section `z`; None when nothing is (nothing was
/// stroked there, or all of it was erased).
pub fn section(strokes: &[Stroke], z: usize) -> Option<Raster> {
    let pieces = pieces_on(strokes, z);
    if pieces.is_empty() {
        return None;
    }
    // The strokes' own extent plus a brush radius round it: all a stroke can cover.
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in &pieces {
        let rad = radius(p.width) + 1.0;
        for q in &p.points {
            for c in 0..2 {
                lo[c] = lo[c].min(q[c] - rad);
                hi[c] = hi[c].max(q[c] + rad);
            }
        }
    }
    let mut r = Raster::blank(z, lo, hi);
    for p in &pieces {
        stamp(&mut r, p, !p.cut);
    }
    (r.voxels() > 0).then_some(r)
}

/// Every painted section, in order.
pub fn rasterise(strokes: &[Stroke]) -> Vec<Raster> {
    sections(strokes).into_iter().filter_map(|z| section(strokes, z)).collect()
}

/// The tight circle round what is painted, as the circled search takes an
/// example: the centre (x, y, z) of the painted voxels of the section with
/// most of them (the first of equals), and the distance of the farthest
/// painted voxel on it, with the half voxel it covers. None when nothing is
/// painted.
pub fn circle_of(strokes: &[Stroke]) -> Option<([f64; 3], f64)> {
    let main = rasterise(strokes).into_iter().reduce(|best, r| if r.voxels() > best.voxels() { r } else { best })?;
    let [cx, cy] = main.centroid()?;
    let far = main.cells().map(|(x, y)| dist([x as f64, y as f64], [cx, cy])).fold(0.0, f64::max);
    Some(([cx, cy, main.z as f64], far + 0.5))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: &[[f64; 3]], width: f64, cut: bool) -> Stroke {
        Stroke { points: points.to_vec(), cut, width }
    }
    /// A circle of `r` voxels about (cx, cy) on section z, ending where it began.
    fn ring(cx: f64, cy: f64, z: f64, r: f64, steps: usize) -> Vec<[f64; 3]> {
        (0..=steps).map(|k| k as f64 / steps as f64 * std::f64::consts::TAU).map(|a| [cx + r * a.cos(), cy + r * a.sin(), z]).collect()
    }

    #[test]
    fn a_dab_is_one_brush_wide_disc() {
        let r = section(&[stroke(&[[20.0, 30.0, 5.0]], 8.0, false)], 5).unwrap();
        let pi_r2 = std::f64::consts::PI * 16.0;
        assert!((r.voxels() as f64 - pi_r2).abs() < 0.15 * pi_r2, "{}", r.voxels());
        assert!(r.contains(20, 30) && r.contains(23, 30) && !r.contains(26, 30) && !r.contains(20, 35));
        // Even a very thin brush takes the voxel it is on.
        assert!(section(&[stroke(&[[7.0, 7.0, 0.0]], 0.0, false)], 0).unwrap().contains(7, 7));
    }

    #[test]
    fn a_stroke_that_does_not_close_is_a_line_not_a_fill() {
        let line = stroke(&[[10.0, 10.0, 2.0], [20.0, 10.0, 2.0], [30.0, 14.0, 2.0], [30.0, 24.0, 2.0]], 4.0, false);
        let r = section(&[line], 2).unwrap();
        // About length × width: far less than the box round it.
        assert!(r.voxels() < 4 * 45 + 30, "{}", r.voxels());
        assert!(r.contains(20, 10) && !r.contains(14, 18));
    }

    #[test]
    fn a_closed_loop_fills_inside() {
        let loop_ = stroke(&ring(50.0, 50.0, 3.0, 12.0, 40), 3.0, false);
        let r = section(&[loop_], 3).unwrap();
        let disc = std::f64::consts::PI * (12.0f64 + 1.5).powi(2);
        assert!((r.voxels() as f64 - disc).abs() < 0.1 * disc, "{} vs {disc}", r.voxels());
        assert!(r.contains(50, 50) && r.contains(44, 55) && !r.contains(50, 70));
        // The same ring left open (a quarter short) is only the outline.
        let open = stroke(&ring(50.0, 50.0, 3.0, 12.0, 40)[..30], 3.0, false);
        let o = section(&[open], 3).unwrap();
        assert!(!o.contains(50, 50) && o.voxels() < r.voxels() / 2);
    }

    #[test]
    fn erasing_takes_away_what_it_covers_and_later_paint_comes_back() {
        let fill = stroke(&ring(40.0, 40.0, 1.0, 10.0, 36), 3.0, false);
        let before = section(&[fill.clone()], 1).unwrap();
        // A stripe across the middle.
        let cut = stroke(&[[25.0, 40.0, 1.0], [55.0, 40.0, 1.0]], 4.0, true);
        let after = section(&[fill.clone(), cut.clone()], 1).unwrap();
        assert!(!after.contains(40, 40) && after.contains(40, 33) && after.contains(40, 47));
        assert!(after.voxels() < before.voxels() && after.voxels() > before.voxels() / 2);
        // Painting again over the erased stripe fills it.
        let again = section(&[fill, cut, stroke(&[[40.0, 40.0, 1.0]], 4.0, false)], 1).unwrap();
        assert!(again.contains(40, 40));
        // Erasing everything leaves nothing on the section.
        let all = stroke(&[[40.0, 40.0, 1.0]], 5.0, false);
        let wipe = stroke(&[[30.0, 40.0, 1.0], [50.0, 40.0, 1.0]], 20.0, true);
        assert!(section(&[all, wipe], 1).is_none());
    }

    #[test]
    fn each_section_has_its_own_raster() {
        let strokes = vec![stroke(&[[10.0, 10.0, 4.0]], 6.0, false), stroke(&[[30.0, 30.0, 9.0]], 6.0, false), stroke(&[[11.0, 10.0, 4.2]], 6.0, false)];
        assert_eq!(sections(&strokes), vec![4, 9]);
        let all = rasterise(&strokes);
        assert_eq!(all.iter().map(|r| r.z).collect::<Vec<_>>(), vec![4, 9]);
        assert!(all[0].contains(10, 10) && !all[0].contains(30, 30) && all[1].contains(30, 30) && !all[1].contains(10, 10));
        assert!(section(&strokes, 5).is_none());
    }

    #[test]
    fn runs_cover_the_voxels_row_by_row_and_skip_what_is_off_the_image() {
        let r = section(&[stroke(&[[1.0, 3.0, 0.0], [6.0, 3.0, 0.0]], 1.0, false)], 0).unwrap();
        let runs = r.runs();
        assert!(runs.iter().all(|u| u[1] < u[2]));
        let n: usize = runs.iter().map(|u| u[2] - u[1]).sum();
        assert_eq!(n, r.voxels());
        let row = runs.iter().find(|u| u[0] == 3).unwrap();
        assert!(row[1] <= 1 && row[2] >= 7);
        // A dab over the left edge keeps what is inside.
        let e = section(&[stroke(&[[0.0, 5.0, 0.0]], 6.0, false)], 0).unwrap();
        assert!(e.runs().iter().all(|u| u[1] < u[2]) && e.runs().iter().map(|u| u[2] - u[1]).sum::<usize>() < e.voxels());
    }

    #[test]
    fn the_circle_round_the_paint_is_tight() {
        let strokes = vec![
            stroke(&ring(60.0, 40.0, 7.0, 9.0, 40), 3.0, false),
            // A smaller dab on another section does not move it.
            stroke(&[[10.0, 10.0, 2.0]], 4.0, false),
        ];
        let (c, r) = circle_of(&strokes).unwrap();
        assert!((c[0] - 60.0).abs() < 0.6 && (c[1] - 40.0).abs() < 0.6 && c[2] == 7.0, "{c:?}");
        assert!((r - 10.5).abs() < 1.2, "{r}");
        assert!(circle_of(&[]).is_none());
        // A single dab is a small circle, not nothing.
        assert!(circle_of(&[stroke(&[[3.0, 3.0, 1.0]], 6.0, false)]).unwrap().1 >= 2.5);
    }
}
