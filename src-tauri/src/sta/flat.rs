//! Particles on a curved surface, cut "unbent". A lattice on a membrane or
//! a vesicle is found by letting each sampling point slide along the
//! surface and turn about its normal until the pattern under it agrees
//! with the pattern under the others. In cubes cut straight from the
//! tomogram that does not work: the surface is curved in the box, so a
//! slide tilts it, the strong layers of the membrane punish every slide,
//! and the weak pattern in the layer never gets its say (what was seen on
//! the vesicles of the Dynamo walkthrough: rings, no lattice).
//!
//! Here each cube is cut along the surface instead: its x and y run in
//! the tangent plane at the sampling point, and every column of voxels
//! along z starts from the surface itself (the surface about the point is
//! taken as a paraboloid fitted to the neighbouring points). The layers
//! are then flat sheets in every cube, a slide along the surface is a
//! plain shift in x and y and the turn about the normal a plain turn
//! about z, neither of which the layers notice: only the pattern decides.
//! What was found is carried back to the tomogram with `Patch::lift` and
//! `Patch::frame_at`; averages are made from cubes cut straight there.
//!
//! Two things have to be right first, and neither is asked of the user:
//!
//! * The surface. A sphere clicked by eye is a few voxels and a few
//!   degrees off the membrane it stands for, and an unbent cube of a
//!   surface that is not the real one still has the layers tilted in it.
//!   `snap` moves the sampling points onto the layer that is really there
//!   (its profile across the surface, compared between all the points) and
//!   takes the normals from the points themselves.
//! * What repeats, and where. `lattice` reads it from the data: the power
//!   spectrum of the unbent sections at every depth, averaged over the
//!   points, has a ring where a pattern repeats in the layer (on the
//!   vesicles of the Dynamo walkthrough at 70 Å, 90 Å inside the membrane,
//!   which is where their tutorial tells the user to move the boxes by
//!   hand). It gives the depth the boxes are centred on and the detail
//!   the alignment has to see.

use super::shape::{frame, Seed};
use crate::fft::Fft;
use crate::spa::fourier::{freq, ft2};
use crate::spa::geom::Mat;
use rayon::prelude::*;

type P = [f64; 3];

/// The surface about one sampling point.
#[derive(Clone, Copy, Debug)]
pub struct Patch {
    /// The point, voxels of the volume (x, y, z).
    pub at: P,
    /// Columns: two directions in the tangent plane and the normal.
    pub frame: Mat,
    /// The surface's height above the tangent plane at (x, y) there is
    /// `bend[0]·x² + 2·bend[1]·x·y + bend[2]·y²`.
    pub bend: [f64; 3],
}

fn col(m: &Mat, k: usize) -> P {
    [m[0][k], m[1][k], m[2][k]]
}

/// Solves the symmetric 3×3 system a·x = b; None when it has no one answer.
fn solve(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: &[[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = det(&a);
    let scale = a[0][0].abs().max(a[1][1].abs()).max(a[2][2].abs());
    if d.abs() < 1e-9 * scale.powi(3).max(1e-30) {
        return None;
    }
    let mut out = [0.0; 3];
    for k in 0..3 {
        let mut m = a;
        for row in 0..3 {
            m[row][k] = b[row];
        }
        out[k] = det(&m) / d;
    }
    Some(out)
}

/// The patches of the seeds of one surface (or several: only seeds within
/// `reach` of each other and facing the same side are taken together).
pub fn patches(seeds: &[Seed], reach: f64) -> Vec<Patch> {
    seeds
        .iter()
        .map(|s| {
            let f = frame(s.axis);
            let (e1, e2, n) = (col(&f, 0), col(&f, 1), col(&f, 2));
            // Least squares for z = a·x² + 2b·x·y + c·y² over the neighbours.
            let mut m = [[0.0; 3]; 3];
            let mut r = [0.0; 3];
            let mut count = 0;
            for o in seeds {
                let d: P = std::array::from_fn(|k| o.pos[k] - s.pos[k]);
                let len = d.iter().map(|x| x * x).sum::<f64>().sqrt();
                let facing = (0..3).map(|k| o.axis[k] * n[k]).sum::<f64>();
                if len < 1e-9 || len > reach || facing < 0.5 {
                    continue;
                }
                let dot = |e: P| (0..3).map(|k| d[k] * e[k]).sum::<f64>();
                let (x, y, z) = (dot(e1), dot(e2), dot(n));
                let row = [x * x, 2.0 * x * y, y * y];
                for i in 0..3 {
                    for j in 0..3 {
                        m[i][j] += row[i] * row[j];
                    }
                    r[i] += row[i] * z;
                }
                count += 1;
            }
            let bend = if count >= 5 { solve(m, r).unwrap_or([0.0; 3]) } else { [0.0; 3] };
            Patch { at: s.pos, frame: f, bend }
        })
        .collect()
}

/// The same surfaces sampled about `to` apart where the seeds are about
/// `from` apart (all in voxels; `reach` as for `patches`): each seed's own
/// stretch of its surface is sampled on a grid of `to`, and of points that
/// come closer than that from neighbouring seeds one stays. Normals are
/// the surfaces' own there. With `to` larger than `from` the seeds are
/// thinned.
pub fn respaced(seeds: &[Seed], reach: f64, from: f64, to: f64) -> Vec<Seed> {
    let m = (0.5 * from / to).floor() as i64;
    let mut out: Vec<Seed> = vec![];
    let mut scores: Vec<f32> = vec![];
    for p in patches(seeds, reach) {
        for j in -m..=m {
            for i in -m..=m {
                let (x, y) = (i as f64 * to, j as f64 * to);
                out.push(Seed { pos: p.lift([x, y, 0.0]), axis: col(&p.frame_at(x, y), 2) });
                // A seed's own place before what its neighbours put near it.
                scores.push(-((i * i + j * j) as f32));
            }
        }
    }
    let at: Vec<P> = out.iter().map(|s| s.pos).collect();
    let mut keep = super::shape::merge(&at, &scores, 0.7 * to);
    keep.sort_unstable();
    keep.into_iter().map(|k| out[k]).collect()
}

impl Patch {
    fn height(&self, x: f64, y: f64) -> f64 {
        self.bend[0] * x * x + 2.0 * self.bend[1] * x * y + self.bend[2] * y * y
    }

    /// Where the place (x, y, z) of the unbent cube (from its centre) lies
    /// in the volume.
    pub fn lift(&self, q: P) -> P {
        let h = q[2] + self.height(q[0], q[1]);
        std::array::from_fn(|k| self.at[k] + self.frame[k][0] * q[0] + self.frame[k][1] * q[1] + self.frame[k][2] * h)
    }

    /// The frame of the surface above (x, y): what `frame` is at (0, 0),
    /// tilted to the surface's normal there, without a turn about it.
    pub fn frame_at(&self, x: f64, y: f64) -> Mat {
        // Slopes of the paraboloid.
        let (sx, sy) = (2.0 * (self.bend[0] * x + self.bend[1] * y), 2.0 * (self.bend[1] * x + self.bend[2] * y));
        let (e1, e2, n) = (col(&self.frame, 0), col(&self.frame, 1), col(&self.frame, 2));
        let unit = |v: P| {
            let l = v.iter().map(|x| x * x).sum::<f64>().sqrt();
            v.map(|x| x / l)
        };
        let normal = unit(std::array::from_fn(|k| n[k] - sx * e1[k] - sy * e2[k]));
        // The first tangent stays as near e1 as the new normal lets it.
        let along = (0..3).map(|k| e1[k] * normal[k]).sum::<f64>();
        let t1 = unit(std::array::from_fn(|k| e1[k] - along * normal[k]));
        let t2 = [normal[1] * t1[2] - normal[2] * t1[1], normal[2] * t1[0] - normal[0] * t1[2], normal[0] * t1[1] - normal[1] * t1[0]];
        let _ = e2;
        [[t1[0], t2[0], normal[0]], [t1[1], t2[1], normal[1]], [t1[2], t2[2], normal[2]]]
    }
}

/// The volume at `p` (x, y, z), between its voxels; None outside it.
fn sample(v: &[f32], dims: [usize; 3], p: P) -> Option<f32> {
    let f = p.map(|x| x.floor());
    if (0..3).any(|k| f[k] < 0.0 || f[k] + 1.0 > (dims[k] - 1) as f64) {
        return None;
    }
    let (ix, iy, iz) = (f[0] as usize, f[1] as usize, f[2] as usize);
    let (tx, ty, tz) = ((p[0] - f[0]) as f32, (p[1] - f[1]) as f32, (p[2] - f[2]) as f32);
    let at = |dx: usize, dy: usize, dz: usize| v[((iz + dz) * dims[1] + iy + dy) * dims[0] + ix + dx];
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    Some(lerp(
        lerp(lerp(at(0, 0, 0), at(1, 0, 0), tx), lerp(at(0, 1, 0), at(1, 1, 0), tx), ty),
        lerp(lerp(at(0, 0, 1), at(1, 0, 1), tx), lerp(at(0, 1, 1), at(1, 1, 1), tx), ty),
        tz,
    ))
}

/// How well a surface with the normal `n` shows in a tomogram of a tilt
/// series from `lo` to `hi` degrees (tilt axis Y), 0 to 1: its layers are
/// detail along `n`, and detail along the beam is what the missing wedge
/// takes. The top and bottom of a vesicle do not show at all.
pub fn shows(n: P, lo: f64, hi: f64) -> f64 {
    let across = n[0].hypot(n[2]);
    if across < 1e-9 {
        return 1.0;
    }
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    if hi - lo >= 180.0 - 1e-6 {
        return 1.0;
    }
    let (a, b) = if n[0] < 0.0 { (-n[0], -n[2]) } else { (n[0], n[2]) };
    let angle = b.atan2(a).to_degrees();
    let inside = ((angle - lo).min(hi - angle) / 10.0).clamp(0.0, 1.0);
    // A normal mostly along the tilt axis shows whatever the rest of it does.
    inside.max(1.0 - 2.0 * across).clamp(0.0, 1.0)
}

/// Solves the n×n system a·x = b (rows of `a` with `b` appended); None when
/// it has no one answer.
fn solve_n<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    let scale = (0..N).map(|i| a[i][i].abs()).fold(0.0, f64::max).max(1e-300);
    for col in 0..N {
        let pivot = (col..N).max_by(|i, j| a[*i][col].abs().total_cmp(&a[*j][col].abs()))?;
        if a[pivot][col].abs() < 1e-10 * scale {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..N {
            let f = a[row][col] / a[col][col];
            for k in col..N {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; N];
    for row in (0..N).rev() {
        let rest: f64 = (row + 1..N).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - rest) / a[row][row];
    }
    Some(x)
}

/// Steps of `snap`'s profiles, voxels.
const SNAP_STEP: f64 = 0.5;
/// Rounds in which the profiles are compared with their common one.
const SNAP_ROUNDS: usize = 4;

/// The seeds of drawn surfaces moved onto the layer that is really there,
/// with the normals of that layer.
///
/// Each seed has a profile across the surface (the volume along its
/// normal, averaged over a disc of `disc` voxels in the tangent plane).
/// The profiles are shifted (at most `range` voxels) until each agrees
/// with the average of all, which sharpens as they do; the strongest
/// layer of that average is the one the seeds are put on. A seed alone can
/// be wrong (noise, the other layer, a surface the missing wedge hides,
/// `shows`), so its place is not taken from its own shift: a smooth piece
/// of surface is fitted through the shifted seeds within `reach` voxels
/// (further where too few of them show), the ones that agree least
/// counting least, and the seed is put on that with its normal.
pub fn snap(v: &[f32], dims: [usize; 3], seeds: &[Seed], disc: f64, range: f64, reach: f64, tilts: [f64; 2]) -> Vec<Seed> {
    let m = seeds.len();
    let most = (range / SNAP_STEP).ceil() as isize;
    let window = most;
    let half = window + most;
    let len = (2 * half + 1) as usize;
    let r = disc.ceil() as i32;
    let spots: Vec<(f64, f64)> =
        (-r..=r).flat_map(|y| (-r..=r).map(move |x| (x as f64, y as f64))).filter(|(x, y)| x * x + y * y <= disc * disc + 1e-9).collect();
    let profiles: Vec<Option<Vec<f32>>> = seeds
        .par_iter()
        .map(|s| {
            let f = frame(s.axis);
            let (e1, e2, n) = (col(&f, 0), col(&f, 1), col(&f, 2));
            let mut out = vec![0f32; len];
            for (j, o) in out.iter_mut().enumerate() {
                let off = (j as isize - half) as f64 * SNAP_STEP;
                let (mut sum, mut count) = (0f64, 0usize);
                for (x, y) in &spots {
                    if let Some(value) = sample(v, dims, std::array::from_fn(|k| s.pos[k] + x * e1[k] + y * e2[k] + off * n[k])) {
                        sum += value as f64;
                        count += 1;
                    }
                }
                if count * 2 < spots.len() {
                    return None;
                }
                *o = (sum / count as f64) as f32;
            }
            Some(out)
        })
        .collect();
    let seen: Vec<f64> = seeds.iter().map(|s| shows(s.axis, tilts[0], tilts[1])).collect();
    let mut shift = vec![0f64; m];
    let mut score = vec![1f64; m];
    let wide = (2 * window + 1) as usize;
    let mut layer = vec![0f64; wide];
    for _ in 0..SNAP_ROUNDS {
        let mut common = vec![0f64; wide];
        for i in 0..m {
            let Some(p) = &profiles[i] else { continue };
            let w = seen[i] * score[i].max(0.0);
            let s = shift[i].round() as isize;
            for (k, c) in common.iter_mut().enumerate() {
                *c += w * p[(k as isize + most + s) as usize] as f64;
            }
        }
        let mean = common.iter().sum::<f64>() / wide as f64;
        common.iter_mut().for_each(|c| *c -= mean);
        let norm = common.iter().map(|c| c * c).sum::<f64>().sqrt();
        if norm <= 0.0 {
            break;
        }
        for i in 0..m {
            let Some(p) = &profiles[i] else { continue };
            let like = |s: isize| -> f64 {
                let part = &p[(most + s) as usize..][..wide];
                let mean = part.iter().map(|x| *x as f64).sum::<f64>() / wide as f64;
                let (mut dot, mut sq) = (0.0, 0.0);
                for (x, c) in part.iter().zip(&common) {
                    dot += (*x as f64 - mean) * c;
                    sq += (*x as f64 - mean).powi(2);
                }
                if sq > 0.0 { dot / (sq.sqrt() * norm) } else { 0.0 }
            };
            let scores: Vec<f64> = (-most..=most).map(like).collect();
            let best = (0..scores.len()).max_by(|a, b| scores[*a].total_cmp(&scores[*b])).unwrap();
            // Between the steps: the top of the parabola through three scores.
            let fine = if best > 0 && best + 1 < scores.len() {
                let (a, b, c) = (scores[best - 1], scores[best], scores[best + 1]);
                let bend = a - 2.0 * b + c;
                if bend < 0.0 { (0.5 * (a - c) / bend).clamp(-0.5, 0.5) } else { 0.0 }
            } else {
                0.0
            };
            shift[i] = best as f64 - most as f64 + fine;
            score[i] = scores[best];
        }
        layer = common;
    }
    // Which layer: the profiles only say how the seeds lie to each other.
    // The seeds are put on the strongest thing in the common profile (the
    // membrane itself, in the tomograms seen so far).
    let onto = (0..layer.len()).max_by(|a, b| layer[*a].abs().total_cmp(&layer[*b].abs())).map_or(0.0, |k| k as f64 - window as f64);
    shift.iter_mut().for_each(|s| *s += onto);
    // Where each seed's layer is, and how much it counts.
    let found: Vec<Option<(P, f64)>> = (0..m)
        .map(|i| {
            profiles[i].as_ref()?;
            let w = seen[i] * score[i].max(0.0);
            (w > 0.0).then(|| (std::array::from_fn(|k| seeds[i].pos[k] + shift[i] * SNAP_STEP * seeds[i].axis[k]), w))
        })
        .collect();
    (0..m)
        .into_par_iter()
        .map(|i| {
            let s = &seeds[i];
            let f = frame(s.axis);
            let (e1, e2, n) = (col(&f, 0), col(&f, 1), col(&f, 2));
            let mut reach = reach;
            for _ in 0..3 {
                // (x, y, height, weight) of the neighbours' layers.
                let near: Vec<[f64; 4]> = (0..m)
                    .filter_map(|j| {
                        let (q, w) = found[j]?;
                        let d: P = std::array::from_fn(|k| q[k] - s.pos[k]);
                        let facing = (0..3).map(|k| seeds[j].axis[k] * n[k]).sum::<f64>();
                        let dot = |e: P| (0..3).map(|k| d[k] * e[k]).sum::<f64>();
                        let (x, y) = (dot(e1), dot(e2));
                        (facing > 0.5 && x * x + y * y <= reach * reach && dot(n).abs() <= reach).then(|| [x, y, dot(n), w])
                    })
                    .collect();
                if near.len() >= 10 && near.iter().map(|p| p[3]).sum::<f64>() >= 3.0 {
                    if let Some(c) = surface_through(&near) {
                        // Height c0, slopes c1 and c2 at the seed.
                        let axis = [n[0] - c[1] * e1[0] - c[2] * e2[0], n[1] - c[1] * e1[1] - c[2] * e2[1], n[2] - c[1] * e1[2] - c[2] * e2[2]];
                        let l = axis.iter().map(|x| x * x).sum::<f64>().sqrt();
                        return Seed { pos: std::array::from_fn(|k| s.pos[k] + c[0] * n[k]), axis: axis.map(|x| x / l) };
                    }
                }
                reach *= 1.5;
            }
            match found[i] {
                Some((q, _)) => Seed { pos: q, axis: s.axis },
                None => *s,
            }
        })
        .collect()
}

/// The surface z = c0 + c1·x + c2·y + c3·x² + 2·c4·x·y + c5·y² through
/// weighted points (x, y, z, weight); points far off it count less the
/// second and third time round.
fn surface_through(points: &[[f64; 4]]) -> Option<[f64; 6]> {
    let mut trust = vec![1.0; points.len()];
    let mut out = None;
    for _ in 0..3 {
        let mut a = [[0.0; 6]; 6];
        let mut b = [0.0; 6];
        for (p, t) in points.iter().zip(&trust) {
            let row = [1.0, p[0], p[1], p[0] * p[0], 2.0 * p[0] * p[1], p[1] * p[1]];
            let w = p[3] * t;
            for i in 0..6 {
                for j in 0..6 {
                    a[i][j] += w * row[i] * row[j];
                }
                b[i] += w * row[i] * p[2];
            }
        }
        let c = solve_n(a, b)?;
        let off: Vec<f64> = points.iter().map(|p| (p[2] - (c[0] + c[1] * p[0] + c[2] * p[1] + c[3] * p[0] * p[0] + 2.0 * c[4] * p[0] * p[1] + c[5] * p[1] * p[1])).abs()).collect();
        let mut sorted = off.clone();
        sorted.sort_by(f64::total_cmp);
        let spread = (1.4826 * sorted[sorted.len() / 2]).max(0.25);
        for (t, o) in trust.iter_mut().zip(&off) {
            let u = o / (3.0 * spread);
            *t = if u < 1.0 { (1.0 - u * u).powi(2) } else { 0.0 };
        }
        out = Some(c);
    }
    out
}

/// A pattern that repeats within the layers of a surface.
#[derive(Clone, Copy, Debug)]
pub struct Lattice {
    /// How far apart its rows are, voxels.
    pub period: f64,
    /// Where it is strongest, voxels along the normal from the surface
    /// (negative: on the inner side).
    pub depth: f64,
    /// How far its ring stands out of the spectrum around it (1: not at all).
    pub strength: f64,
}

/// The least a ring has to stand out to be taken for a pattern.
const RING: f64 = 1.3;
/// At most this many patches are looked at.
const LATTICE_PATCHES: usize = 400;

/// The power of the unbent sections of side `n` at every depth from
/// −`deep` to `deep` voxels, averaged over the patches and over each ring
/// of the spectrum: [depth][ring], and how many sections each depth had.
fn spectra(v: &[f32], dims: [usize; 3], patches: &[Patch], n: usize, deep: usize) -> (Vec<Vec<f64>>, Vec<usize>) {
    let step = patches.len().div_ceil(LATTICE_PATCHES).max(1);
    let c = (n / 2) as f64;
    let hann: Vec<f32> = (0..n).map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * (i as f64 + 0.5) / n as f64).cos()) as f32).collect();
    let depths = 2 * deep + 1;
    let rings = n / 2;
    let empty = || (vec![vec![0f64; rings]; depths], vec![0usize; depths]);
    patches
        .par_iter()
        .step_by(step)
        .fold(empty, |(mut sum, mut count), patch| {
            let fft = Fft::new(n);
            let mut section = vec![0f32; n * n];
            'depth: for d in 0..depths {
                let z = d as f64 - deep as f64;
                for y in 0..n {
                    for x in 0..n {
                        match sample(v, dims, patch.lift([x as f64 - c, y as f64 - c, z])) {
                            Some(value) => section[y * n + x] = value,
                            None => continue 'depth,
                        }
                    }
                }
                let mean = section.iter().sum::<f32>() / section.len() as f32;
                for y in 0..n {
                    for x in 0..n {
                        section[y * n + x] = (section[y * n + x] - mean) * hann[x] * hann[y];
                    }
                }
                let (re, im) = ft2(&fft, n, &section);
                let (mut power, mut cells) = (vec![0f64; rings], vec![0usize; rings]);
                for y in 0..n {
                    for x in 0..n {
                        let k = ((freq(x, n) as f64).hypot(freq(y, n) as f64)).round() as usize;
                        if k < rings {
                            power[k] += (re[y * n + x] as f64).powi(2) + (im[y * n + x] as f64).powi(2);
                            cells[k] += 1;
                        }
                    }
                }
                for k in 0..rings {
                    sum[d][k] += power[k] / cells[k].max(1) as f64;
                }
                count[d] += 1;
            }
            (sum, count)
        })
        .reduce(empty, |(mut a, mut ca), (b, cb)| {
            for d in 0..depths {
                for k in 0..rings {
                    a[d][k] += b[d][k];
                }
                ca[d] += cb[d];
            }
            (a, ca)
        })
}

/// A pattern has at least this many rows across a section. Fewer are not
/// told from what else varies slowly in a layer (a neighbour, the surface
/// itself where the sheet is not quite flat); a pattern that coarse shows
/// at a coarser sampling (`project::pattern_elsewhere`).
const FEWEST_ROWS: usize = 5;

/// The pattern that repeats in the layers about the patches' surface, if
/// one does: unbent sections of side `n` are looked at from `deep` voxels
/// inside the surface to `deep` outside. None when no ring of the spectrum
/// stands out by `RING` at any depth.
pub fn lattice(v: &[f32], dims: [usize; 3], patches: &[Patch], n: usize, deep: usize) -> Option<Lattice> {
    let (sum, count) = spectra(v, dims, patches, n, deep);
    let rings = n / 2;
    // A ring against the rings two to either side of it.
    let out_of = |d: usize, k: usize| -> f64 {
        let around = (sum[d][k - 2] * sum[d][k + 2]).sqrt();
        if count[d] >= 20 && around > 0.0 { sum[d][k] / around } else { 1.0 }
    };
    let mut best = (1.0, 0, 0);
    for d in 0..sum.len() {
        for k in FEWEST_ROWS..rings.saturating_sub(2) {
            let e = out_of(d, k);
            if e > best.0 {
                best = (e, d, k);
            }
        }
    }
    let (strength, d, k) = best;
    if strength < RING {
        return None;
    }
    // Between the rings and between the depths: where the excess has its
    // centre.
    let over = |d: usize, k: usize| (out_of(d, k) - 1.0).max(0.0);
    let ks: Vec<usize> = (k.max(FEWEST_ROWS + 1) - 1..=(k + 1).min(rings - 3)).collect();
    let ring = ks.iter().map(|k| *k as f64 * over(d, *k)).sum::<f64>() / ks.iter().map(|k| over(d, *k)).sum::<f64>();
    let half = 0.5 * (strength - 1.0);
    let (mut lo, mut hi) = (d, d);
    while lo > 0 && over(lo - 1, k) >= half {
        lo -= 1;
    }
    while hi + 1 < sum.len() && over(hi + 1, k) >= half {
        hi += 1;
    }
    let depth = (lo..=hi).map(|d| (d as f64 - deep as f64) * over(d, k)).sum::<f64>() / (lo..=hi).map(|d| over(d, k)).sum::<f64>();
    Some(Lattice { period: n as f64 / ring, depth, strength })
}

/// The unbent cube of side `n` about a patch (z, y, x order, the point at
/// the centre `n / 2`), from the volume `v` of size `dims`. None when the
/// ball of `radius` voxels about the point is not all inside the volume
/// or less than half the cube is. The cube has zero mean and unit spread
/// (as `project::cube`); voxels that fall outside the volume are zero.
pub fn cut(v: &[f32], dims: [usize; 3], patch: &Patch, n: usize, radius: f64) -> Option<Vec<f32>> {
    if (0..3).any(|k| patch.at[k] - radius < 0.0 || patch.at[k] + radius > (dims[k] - 1) as f64) {
        return None;
    }
    let c = (n / 2) as f64;
    let mut out = vec![f32::NAN; n * n * n];
    let (mut sum, mut sq, mut count) = (0f64, 0f64, 0usize);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let Some(value) = sample(v, dims, patch.lift([x as f64 - c, y as f64 - c, z as f64 - c])) else { continue };
                out[(z * n + y) * n + x] = value;
                sum += value as f64;
                sq += (value as f64).powi(2);
                count += 1;
            }
        }
    }
    if count * 2 < out.len() {
        return None;
    }
    let mean = sum / count as f64;
    let sd = (sq / count as f64 - mean * mean).max(0.0).sqrt();
    if sd <= 0.0 {
        return None;
    }
    out.iter_mut().for_each(|x| *x = if x.is_nan() { 0.0 } else { ((*x as f64 - mean) / sd) as f32 });
    Some(out)
}

/// How many times round a pattern may repeat itself about an axis along
/// the normal, most first, each with the fewer times round that the same
/// axis then holds as well.
const FOLDS: [(usize, &[usize]); 4] = [(6, &[3, 2]), (4, &[2]), (3, &[]), (2, &[])];
/// A model has an axis when it agrees with itself turned about it at least
/// this well (correlation).
const SAME: f64 = 0.3;
/// An axis of more turns is also one of fewer: there the model has to
/// agree this nearly as well as about the best axis of the fewer.
const NEARLY: f64 = 0.85;

/// The axis the pattern of an unbent model (`n`³, layers across Z, as
/// `refine::in_layer` averages it) repeats itself about, if it has one:
/// how many times round, and where an axis near the centre stands, voxels
/// from the centre along X and Y. `period` is the pattern's row spacing
/// and `radius` what of the model counts, voxels.
///
/// A place of a pattern is the point of each cube that lies on the model's
/// centre. Where the pattern repeats itself turned (six times round for a
/// hexagonal one), a cube fits the model at each of those turns alike, and
/// its place is the same point of the pattern at every one of them only
/// when the model's centre is on the axis: otherwise the places of one
/// kind of site scatter round it. The model turned about its centre is the
/// model moved by (R − 1)·a when the axis is at `a`, so the move that
/// makes the two agree gives the axis.
///
/// The axis of the most turns is taken that the model shows at all and
/// that is as good an axis of its fewer turns as any (a sixfold axis is a
/// threefold and a twofold one too): a model that shows the more turns
/// only faintly (rows the missing wedge hides) loses nothing by it, and
/// one that has them is not left on an axis of the fewer, of which a cell
/// has several that are not the same site.
pub fn axis(model: &[f32], n: usize, period: f64, radius: f64) -> Option<(usize, [f64; 2])> {
    // Sections twice as fine (in Fourier space): rows a few voxels apart
    // do not survive being turned between the voxels otherwise.
    const FINER: usize = 2;
    let depth = n;
    let model: Vec<f32> = model.par_chunks(n * n).flat_map_iter(|section| crate::spa::fourier::resample2(section, n, FINER * n)).collect();
    let (n, period, radius, tall) = (FINER * n, FINER as f64 * period, FINER as f64 * radius, radius);
    let c = (n / 2) as f64;
    let reach = ((1.2 * period).ceil() as isize).clamp(1, n as isize / 4);
    let side = (2 * reach + 1) as usize;
    let at = |x: isize, y: isize, z: usize| -> Option<f32> { (x >= 0 && y >= 0 && x < n as isize && y < n as isize).then(|| model[(z * n + y as usize) * n + x as usize]) };
    let inside = |x: usize, y: usize, z: usize| (x as f64 - c).powi(2) + (y as f64 - c).powi(2) <= radius * radius && (z as f64 - (depth / 2) as f64).abs() <= tall;
    // The model turned by one step of `fold`: its value at x is the
    // model's at Rᵀ(x − c) + c.
    let turned = |r: &Mat| -> Vec<f32> {
        let mut out = vec![0f32; n * n * depth];
        out.par_chunks_mut(n * n).enumerate().for_each(|(z, plane)| {
            for y in 0..n {
                for x in 0..n {
                    let (u, v) = (x as f64 - c, y as f64 - c);
                    let (sx, sy) = (r[0][0] * u + r[1][0] * v + c, r[0][1] * u + r[1][1] * v + c);
                    let (fx, fy) = (sx.floor(), sy.floor());
                    let (tx, ty) = ((sx - fx) as f32, (sy - fy) as f32);
                    let (ix, iy) = (fx as isize, fy as isize);
                    let (Some(a), Some(b), Some(d), Some(e)) = (at(ix, iy, z), at(ix + 1, iy, z), at(ix, iy + 1, z), at(ix + 1, iy + 1, z)) else { continue };
                    plane[y * n + x] = (a + (b - a) * tx) * (1.0 - ty) + (d + (e - d) * tx) * ty;
                }
            }
        });
        out
    };
    // How well the model moved by (dx, dy) agrees with a turned one.
    let agree = |turned: &[f32], dx: isize, dy: isize| -> f64 {
        let (mut ab, mut aa, mut bb) = (0f64, 0f64, 0f64);
        for z in 0..depth {
            for y in 0..n {
                for x in 0..n {
                    if !inside(x, y, z) {
                        continue;
                    }
                    let Some(a) = at(x as isize - dx, y as isize - dy, z) else { continue };
                    let b = turned[(z * n + y) * n + x];
                    ab += (a * b) as f64;
                    aa += (a * a) as f64;
                    bb += (b * b) as f64;
                }
            }
        }
        if aa > 0.0 && bb > 0.0 { ab / (aa * bb).sqrt() } else { 0.0 }
    };
    // The move (R − 1)·a of an axis at `a`, and back.
    let less = |r: &Mat| [[r[0][0] - 1.0, r[0][1]], [r[1][0], r[1][1] - 1.0]];
    let move_of = |r: &Mat, a: [f64; 2]| {
        let m = less(r);
        [m[0][0] * a[0] + m[0][1] * a[1], m[1][0] * a[0] + m[1][1] * a[1]]
    };
    let axis_of = |r: &Mat, d: [f64; 2]| {
        let m = less(r);
        let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        [(m[1][1] * d[0] - m[0][1] * d[1]) / det, (m[0][0] * d[1] - m[1][0] * d[0]) / det]
    };
    // Each way round: the model turned, how well it agrees at its best
    // move, and the axis that move stands for.
    struct Way {
        fold: usize,
        turn: Mat,
        turned: Vec<f32>,
        best: f64,
        at: [f64; 2],
    }
    let ways: Vec<Way> = FOLDS
        .iter()
        .map(|&(fold, _)| {
            let turn = super::cc_axis(360.0 / fold as f64);
            let turned = turned(&turn);
            let grid: Vec<f64> = (0..side * side).into_par_iter().map(|k| agree(&turned, (k % side) as isize - reach, (k / side) as isize - reach)).collect();
            let value = |dx: isize, dy: isize| grid[((dy + reach) * side as isize + dx + reach) as usize];
            // Of moves that agree alike (the axes of every cell), a short one.
            let (mut top, mut dx, mut dy) = (f64::MIN, 0, 0);
            for y in -reach..=reach {
                for x in -reach..=reach {
                    let v = value(x, y) - 0.02 * ((x * x + y * y) as f64).sqrt() / reach as f64;
                    if v > top {
                        (top, dx, dy) = (v, x, y);
                    }
                }
            }
            // Between the voxels: a parabola through the neighbours.
            let between = |lo: f64, mid: f64, hi: f64| {
                let bend = lo - 2.0 * mid + hi;
                if bend < 0.0 { (0.5 * (lo - hi) / bend).clamp(-0.5, 0.5) } else { 0.0 }
            };
            let best = value(dx, dy);
            let fx = if dx.abs() < reach { between(value(dx - 1, dy), best, value(dx + 1, dy)) } else { 0.0 };
            let fy = if dy.abs() < reach { between(value(dx, dy - 1), best, value(dx, dy + 1)) } else { 0.0 };
            let at = axis_of(&turn, [dx as f64 + fx, dy as f64 + fy]);
            Way { fold, turn, turned, best, at }
        })
        .collect();
    let way = |fold: usize| ways.iter().find(|w| w.fold == fold).expect("every fold of FOLDS");
    // How well the model agrees with itself turned `fold` times round
    // about an axis at `a`.
    let about = |fold: usize, a: [f64; 2]| -> f64 {
        let w = way(fold);
        let d = move_of(&w.turn, a);
        let (x, y) = (d[0].round() as isize, d[1].round() as isize);
        (-1..=1).flat_map(|j| (-1..=1).map(move |i| (i, j))).map(|(i, j)| agree(&w.turned, x + i, y + j)).fold(f64::MIN, f64::max)
    };
    let mut found: Option<(usize, f64, [f64; 2])> = None;
    for (fold, fewer) in FOLDS {
        let w = way(fold);
        if w.best < SAME || found.is_some_and(|f| f.1 >= w.best) {
            continue;
        }
        if fewer.iter().all(|f| about(*f, w.at) >= NEARLY * way(*f).best) {
            found = Some((fold, w.best, w.at));
            if !fewer.is_empty() {
                break;
            }
        }
    }
    found.map(|(fold, _, a)| (fold, a.map(|x| x / FINER as f64)))
}

/// The cube with everything that changes only slowly across each of its
/// sections taken out: the layers themselves, and what a small leftover
/// tilt of them makes. What stays is the pattern within the layers, up to
/// about `scale` voxels across. Each section has a Gaussian blur of itself
/// (σ = `scale` / 3, edges repeated) taken off.
pub fn pattern(cube: &[f32], n: usize, scale: f64) -> Vec<f32> {
    let sigma = (scale / 3.0).max(1.0);
    let r = (3.0 * sigma).ceil() as isize;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-0.5 * (i as f64 / sigma).powi(2)).exp() as f32).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|x| *x /= sum);
    let at = |i: isize| i.clamp(0, n as isize - 1) as usize;
    let mut out = cube.to_vec();
    let mut rows = vec![0f32; n * n];
    for z in 0..n {
        let plane = &cube[z * n * n..(z + 1) * n * n];
        for y in 0..n {
            for x in 0..n {
                rows[y * n + x] = k.iter().enumerate().map(|(j, w)| w * plane[y * n + at(x as isize + j as isize - r)]).sum();
            }
        }
        for y in 0..n {
            for x in 0..n {
                let blurred: f32 = k.iter().enumerate().map(|(j, w)| w * rows[at(y as isize + j as isize - r) * n + x]).sum();
                out[(z * n + y) * n + x] -= blurred;
            }
        }
    }
    out
}

/// How the pattern lies in each unbent cube, read from the cube itself.
///
/// A pattern of rows `period` voxels apart puts its power on a ring of
/// each section's spectrum, in as many spots as it has directions of rows
/// (six for a hexagonal one, with their opposites), and where on the ring
/// the spots are says how the cube's pattern is turned. That needs no
/// model and no other cube, so nothing has to grow out of noise first (a
/// first model averaged from cubes at random turns is the same all the way
/// round, and a small set then settles in patches that disagree). With the
/// turn known, the phases of two of the spots say where the pattern's
/// origin is under the cube (the two strongest: the missing wedge takes
/// rows in some directions).
///
/// Returns for each cube (side `n`, the pattern's layer at its middle) the
/// turn about Z in degrees and the slide in x and y: the cube's pattern is
/// the common one turned by that much and then moved by that much. None
/// when the ring shows no directions (no rows, or too faint in one cube).
pub fn rows(cubes: &[Vec<f32>], n: usize, period: f64) -> Option<Vec<(f64, [f64; 2])>> {
    const FOLDS: [usize; 3] = [2, 4, 6];
    // What noise alone gives is about 0.1.
    const CLEAR: f64 = 0.2;
    let c = (n / 2) as f64;
    let k0 = n as f64 / period;
    let hann: Vec<f32> = (0..n).map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * (i as f64 + 0.5) / n as f64).cos()) as f32).collect();
    // The layer: the middle sections added up.
    let layers: Vec<Vec<f32>> = cubes
        .par_iter()
        .map(|cube| {
            let mut out = vec![0f32; n * n];
            for z in n / 2 - 2..=n / 2 + 2 {
                for (o, v) in out.iter_mut().zip(&cube[z * n * n..(z + 1) * n * n]) {
                    *o += v;
                }
            }
            let mean = out.iter().sum::<f32>() / out.len() as f32;
            for y in 0..n {
                for x in 0..n {
                    out[y * n + x] = (out[y * n + x] - mean) * hann[x] * hann[y];
                }
            }
            out
        })
        .collect();
    // How much of the ring's power turns up `fold` times round, and where.
    let round: Vec<[(f64, f64); 3]> = layers
        .par_iter()
        .map(|layer| {
            let (re, im) = ft2(&Fft::new(n), n, layer);
            let mut sum = [(0.0, 0.0); 3];
            let mut total = 0.0;
            for y in 0..n {
                for x in 0..n {
                    let (kx, ky) = (freq(x, n) as f64, freq(y, n) as f64);
                    if (kx.hypot(ky) - k0).abs() > 1.5 {
                        continue;
                    }
                    let power = (re[y * n + x] as f64).powi(2) + (im[y * n + x] as f64).powi(2);
                    let angle = ky.atan2(kx);
                    total += power;
                    for (s, fold) in sum.iter_mut().zip(FOLDS) {
                        s.0 += power * (fold as f64 * angle).cos();
                        s.1 -= power * (fold as f64 * angle).sin();
                    }
                }
            }
            sum.map(|s| if total > 0.0 { (s.0 / total, s.1 / total) } else { (0.0, 0.0) })
        })
        .collect();
    let clear: Vec<f64> = (0..3).map(|f| round.iter().map(|r| r[f].0.hypot(r[f].1)).sum::<f64>() / round.len().max(1) as f64).collect();
    let most = clear.iter().copied().fold(0.0, f64::max);
    if most < CLEAR {
        return None;
    }
    // Rows in one direction show at every fold: the fewest that is clear.
    let f = (0..3).find(|f| clear[*f] >= 0.8 * most)?;
    let fold = FOLDS[f] as f64;
    // The waves of rows of each cube: direction, strength and phase.
    let waves: Vec<Vec<(f64, f64, f64)>> = layers
        .par_iter()
        .zip(&round)
        .map(|(layer, r)| {
            // Spots at `turn` + j·360°/fold.
            let turn = -r[f].1.atan2(r[f].0) / fold;
            (0..FOLDS[f] / 2)
                .map(|j| {
                    let a = turn + j as f64 * std::f64::consts::TAU / fold;
                    let (kx, ky) = (k0 * a.cos(), k0 * a.sin());
                    let (mut re, mut im) = (0.0, 0.0);
                    for y in 0..n {
                        for x in 0..n {
                            let at = std::f64::consts::TAU * (kx * (x as f64 - c) + ky * (y as f64 - c)) / n as f64;
                            re += layer[y * n + x] as f64 * at.cos();
                            im -= layer[y * n + x] as f64 * at.sin();
                        }
                    }
                    (a, re.hypot(im), im.atan2(re))
                })
                .collect()
        })
        .collect();
    // Where the origin is put, every wave has the same phase. For three
    // directions of rows (at 0°, 60° and 120°, the third wave the second
    // less the first) that phase cannot be chosen: first − second + third
    // is the same wherever the origin is, and it is the phase all three
    // have at a point the pattern is the same six times round (0 for a
    // pattern of light dots, half a turn for dark ones). It is read from
    // all the cubes together.
    let common = if FOLDS[f] == 6 {
        let (re, im) = waves.iter().fold((0.0, 0.0), |(re, im), w| {
            let weight = w[0].1 * w[1].1 * w[2].1;
            let sum = w[0].2 - w[1].2 + w[2].2;
            (re + weight * sum.cos(), im + weight * sum.sin())
        });
        im.atan2(re)
    } else {
        0.0
    };
    Some(
        waves
            .into_iter()
            .map(|mut w| {
                let turn = w[0].0;
                // Strongest first: the missing wedge takes rows in some
                // directions, and a wave it took has no phase.
                w.sort_by(|a, b| b.1.total_cmp(&a.1));
                // A wave cos(2π·k·(p − t)/n + common) has the phase
                // common − 2π·k·t/n: along its direction the pattern is
                // moved by this much.
                let along = |phase: f64| {
                    let turned = (common - phase).rem_euclid(std::f64::consts::TAU);
                    let least = if turned > std::f64::consts::PI { turned - std::f64::consts::TAU } else { turned };
                    least * n as f64 / (std::f64::consts::TAU * k0)
                };
                let a = [w[0].0.cos(), w[0].0.sin()];
                let u = along(w[0].2);
                let slide = match w.get(1) {
                    // t·a = u and t·b = v.
                    Some((second, _, phase)) => {
                        let b = [second.cos(), second.sin()];
                        let v = along(*phase);
                        let det = a[0] * b[1] - a[1] * b[0];
                        [(u * b[1] - v * a[1]) / det, (v * a[0] - u * b[0]) / det]
                    }
                    // Rows in one direction only: along them nothing is known.
                    None => [u * a[0], u * a[1]],
                };
                (turn.to_degrees(), slide)
            })
            .collect(),
    )
}

/// The missing wedge of a tilt series from `lo` to `hi` degrees as a cube
/// cut in `frame` sees it (as `cc::wedge`, which is the case of the
/// identity frame): the cube's frequency k is the volume's `frame`·k.
pub fn wedge(n: usize, frame: &Mat, lo: f64, hi: f64) -> Vec<f32> {
    let (lo, hi) = (lo.min(hi) - 1e-6, lo.max(hi) + 1e-6);
    let mut out = vec![0f32; n * n * n];
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let k = [freq(x, n) as f64, freq(y, n) as f64, freq(z, n) as f64];
                let (mut a, mut b) = ((0..3).map(|j| frame[0][j] * k[j]).sum::<f64>(), (0..3).map(|j| frame[2][j] * k[j]).sum::<f64>());
                if a < 0.0 || (a.abs() < 1e-9 && b < 0.0) {
                    (a, b) = (-a, -b);
                }
                let angle = if a.abs() < 1e-9 && b.abs() < 1e-9 { 0.0 } else { b.atan2(a).to_degrees() };
                out[(z * n + y) * n + x] = if (lo..=hi).contains(&angle) { 1.0 } else { 0.0 };
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sta::shape::ellipsoid;

    const N: usize = 96;

    /// A shell of radius 30 about the middle of a 96³ volume, dark, with
    /// a second fainter shell 5 voxels inside it.
    fn shells() -> Vec<f32> {
        let c = 47.5;
        let mut v = vec![0f32; N * N * N];
        for z in 0..N {
            for y in 0..N {
                for x in 0..N {
                    let r = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2) + (z as f64 - c).powi(2)).sqrt();
                    v[(z * N + y) * N + x] = -(-0.5 * ((r - 30.0) / 1.2).powi(2)).exp() as f32 - 0.5 * (-0.5 * ((r - 25.0) / 1.2).powi(2)).exp() as f32;
                }
            }
        }
        v
    }

    #[test]
    fn a_sphere_is_cut_into_flat_sheets() {
        let v = shells();
        let seeds = ellipsoid([47.5; 3], [30.0; 3], 5.0);
        let patches = patches(&seeds, 14.0);
        // The bend of a sphere of radius 30 seen from outside: −1/(2·30).
        let p = &patches[seeds.len() / 3];
        assert!((p.bend[0] + 1.0 / 60.0).abs() < 0.004 && (p.bend[2] + 1.0 / 60.0).abs() < 0.004 && p.bend[1].abs() < 0.004, "{:?}", p.bend);
        let n = 24;
        let cube = cut(&v, [N; 3], p, n, 8.0).unwrap();
        // Every column within 8 voxels of the middle has the shell at the
        // centre section and the inner shell 5 voxels below it.
        for y in 4..20 {
            for x in 4..20 {
                if ((x as f64 - 12.0).powi(2) + (y as f64 - 12.0).powi(2)).sqrt() > 8.0 {
                    continue;
                }
                let column: Vec<f32> = (0..n).map(|z| cube[(z * n + y) * n + x]).collect();
                let darkest = column.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
                assert_eq!(darkest, 12, "column {x},{y}: {column:?}");
                assert!(column[7] < column[9] && column[7] < column[5], "inner shell at {x},{y}: {column:?}");
            }
        }
        // A place on the surface 6 voxels from the point is on the sphere,
        // and the frame there has the sphere's normal.
        let q = p.lift([6.0, 0.0, 0.0]);
        let r = (0..3).map(|k| (q[k] - 47.5).powi(2)).sum::<f64>().sqrt();
        assert!((r - 30.0).abs() < 0.3, "{r}");
        let f = p.frame_at(6.0, 0.0);
        let radial: P = std::array::from_fn(|k| (q[k] - 47.5) / r);
        assert!((0..3).map(|k| f[k][2] * radial[k]).sum::<f64>() > 0.999);
    }

    #[test]
    fn a_sphere_drawn_off_the_shell_is_put_on_it() {
        let v = shells();
        // Clicked by eye: the centre off by 2 voxels, the radius 3 too large.
        let seeds = ellipsoid([49.0, 46.5, 47.5], [33.0; 3], 5.0);
        let on = snap(&v, [N; 3], &seeds, 3.0, 6.0, 14.0, [-90.0, 90.0]);
        assert_eq!(on.len(), seeds.len());
        let (mut worst, mut tilt) = (0f64, 1f64);
        for s in &on {
            let d: P = std::array::from_fn(|k| s.pos[k] - 47.5);
            let r = d.iter().map(|x| x * x).sum::<f64>().sqrt();
            worst = worst.max((r - 30.0).abs());
            tilt = tilt.min((0..3).map(|k| s.axis[k] * d[k] / r).sum::<f64>());
        }
        assert!(worst < 0.6, "{worst} voxels off the shell");
        assert!(tilt > 0.995, "a normal {:.1} degrees off", tilt.acos().to_degrees());
    }

    /// A flat membrane across z = 40 of a 96 × 96 × 64 volume, and with
    /// `rows` a hexagonal pattern 6 voxels under it whose rows are that far
    /// apart; noise over everything.
    fn sheet(rows: Option<f64>) -> (Vec<f32>, [usize; 3], Vec<Seed>) {
        let dims = [96, 96, 64];
        let mut rng = crate::spa::Rng(7);
        let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let mut value = -(-0.5 * ((z as f64 - 40.0) / 1.2).powi(2)).exp();
                    if let Some(rows) = rows {
                        let k = std::f64::consts::TAU / rows;
                        let wave = |deg: f64| (k * (x as f64 * deg.to_radians().cos() + y as f64 * deg.to_radians().sin())).cos();
                        value -= 0.25 * (wave(10.0) + wave(70.0) + wave(130.0)) * (-0.5 * ((z as f64 - 34.0) / 1.5).powi(2)).exp();
                    }
                    v[(z * dims[1] + y) * dims[0] + x] = (value + 0.5 * rng.gauss()) as f32;
                }
            }
        }
        let mut seeds = vec![];
        for y in (20..=76).step_by(7) {
            for x in (20..=76).step_by(7) {
                seeds.push(Seed { pos: [x as f64, y as f64, 40.0], axis: [0.0, 0.0, 1.0] });
            }
        }
        (v, dims, seeds)
    }

    #[test]
    fn a_pattern_in_a_layer_is_found_with_its_rows_and_its_depth() {
        let (v, dims, seeds) = sheet(Some(4.0));
        let found = lattice(&v, dims, &patches(&seeds, 16.0), 32, 10).expect("the pattern");
        assert!((found.period - 4.0).abs() < 0.3, "{found:?}");
        assert!((found.depth + 6.0).abs() < 1.0, "{found:?}");
        let (v, dims, seeds) = sheet(None);
        assert!(lattice(&v, dims, &patches(&seeds, 16.0), 32, 10).is_none());
    }

    #[test]
    fn the_turn_and_the_slide_of_a_pattern_are_read_from_its_cube() {
        let n = 32;
        let c = (n / 2) as f64;
        let k = std::f64::consts::TAU / 4.0;
        let mut rng = crate::spa::Rng(3);
        let truth: Vec<(f64, [f64; 2])> = (0..12).map(|_| (360.0 * rng.unit(), [3.0 * rng.unit() - 1.5, 3.0 * rng.unit() - 1.5])).collect();
        // The common pattern has rows across 0°, 60° and 120°.
        let cubes: Vec<Vec<f32>> = truth
            .iter()
            .map(|(turn, slide)| {
                let mut cube = vec![0f32; n * n * n];
                for z in 0..n {
                    for y in 0..n {
                        for x in 0..n {
                            let (px, py) = (x as f64 - c - slide[0], y as f64 - c - slide[1]);
                            let wave: f64 = (0..3).map(|j| (turn + 60.0 * j as f64).to_radians()).map(|a| (k * (px * a.cos() + py * a.sin())).cos()).sum();
                            cube[(z * n + y) * n + x] = (wave * (-0.5 * ((z as f64 - c) / 1.5).powi(2)).exp() + 0.7 * rng.gauss()) as f32;
                        }
                    }
                }
                cube
            })
            .collect();
        let found = rows(&cubes, n, 4.0).expect("rows");
        for ((turn, slide), (t, s)) in truth.iter().zip(&found) {
            let off = (turn - t).rem_euclid(60.0);
            assert!(off.min(60.0 - off) < 3.0, "turn {turn:.1} read as {t:.1}");
            // The same place up to whole rows: every wave has the same phase.
            for j in 0..3 {
                let a = (t + 60.0 * j as f64).to_radians();
                let phase = k * ((slide[0] - s[0]) * a.cos() + (slide[1] - s[1]) * a.sin());
                assert!(phase.cos() > 0.9, "slide {slide:?} read as {s:?}");
            }
        }
        // Noise has no rows.
        let noise: Vec<Vec<f32>> = (0..12).map(|_| (0..n * n * n).map(|_| rng.gauss() as f32).collect()).collect();
        assert!(rows(&noise, n, 4.0).is_none());
    }

    #[test]
    fn a_surface_across_the_beam_does_not_show() {
        assert_eq!(shows([0.0, 0.0, 1.0], -60.0, 60.0), 0.0);
        assert_eq!(shows([1.0, 0.0, 0.0], -60.0, 60.0), 1.0);
        assert_eq!(shows([0.0, 1.0, 0.0], -60.0, 60.0), 1.0);
        assert_eq!(shows([0.0, 0.0, 1.0], -90.0, 90.0), 1.0);
    }

    #[test]
    fn the_wedge_of_the_plain_frame_is_the_plain_wedge() {
        let f = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert_eq!(wedge(16, &f, -60.0, 60.0), crate::sta::cc::wedge(16, -60.0, 60.0));
    }

    /// A model of side 32 with waves of period 4 voxels along `degrees` in
    /// its middle layers, all at their crest at `a` from the centre, and
    /// some noise.
    fn waves(degrees: &[f64], a: [f64; 2]) -> Vec<f32> {
        let n = 32;
        let mut rng = crate::spa::Rng(5);
        let mut m = vec![0f32; n * n * n];
        for z in 0..n {
            let layer = (-0.5 * ((z as f64 - 16.0) / 2.0).powi(2)).exp();
            for y in 0..n {
                for x in 0..n {
                    let (u, v) = (x as f64 - 16.0 - a[0], y as f64 - 16.0 - a[1]);
                    let p: f64 = degrees.iter().map(|d| (std::f64::consts::TAU / 4.0 * (u * d.to_radians().cos() + v * d.to_radians().sin())).cos()).sum();
                    m[(z * n + y) * n + x] = (layer * p + 0.2 * rng.gauss()) as f32;
                }
            }
        }
        m
    }

    /// How far `found` is from a crest of every wave, as a part of a period
    /// (0: on one).
    fn off_crest(degrees: &[f64], a: [f64; 2], found: [f64; 2]) -> f64 {
        degrees
            .iter()
            .map(|d| {
                let along = ((found[0] - a[0]) * d.to_radians().cos() + (found[1] - a[1]) * d.to_radians().sin()) / 4.0;
                (along - along.round()).abs()
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn the_axis_of_a_pattern_is_found_off_the_centre() {
        // Hexagonal: six times round, about the point all three waves crest at.
        let hex = [10.0, 70.0, 130.0];
        let (fold, at) = axis(&waves(&hex, [1.3, -0.8]), 32, 4.0, 10.0).expect("an axis");
        assert_eq!(fold, 6);
        assert!(off_crest(&hex, [1.3, -0.8], at) < 0.05, "{at:?}");
        // Square: four times.
        let square = [20.0, 110.0];
        let (fold, at) = axis(&waves(&square, [-0.6, 1.1]), 32, 4.0, 10.0).expect("an axis");
        assert_eq!(fold, 4);
        assert!(off_crest(&square, [-0.6, 1.1], at) < 0.05, "{at:?}");
        // Rows crossing at 50°: only upside down is the same (about a crest
        // or a trough of each).
        let skew = [0.0, 50.0];
        let (fold, at) = axis(&waves(&skew, [0.4, 0.9]), 32, 4.0, 10.0).expect("an axis");
        assert_eq!(fold, 2);
        let half: Vec<f64> = skew.iter().map(|d: &f64| ((at[0] - 0.4) * d.to_radians().cos() + (at[1] - 0.9) * d.to_radians().sin()) / 2.0).collect();
        assert!(half.iter().all(|h| (h - h.round()).abs() < 0.05), "{at:?}");
        // Nothing but noise has none.
        let mut rng = crate::spa::Rng(9);
        let noise: Vec<f32> = (0..32 * 32 * 32).map(|_| rng.gauss() as f32).collect();
        assert!(axis(&noise, 32, 4.0, 10.0).is_none());
    }

    #[test]
    fn a_surface_is_sampled_again_closer_or_wider() {
        let seeds: Vec<Seed> = (0..6).flat_map(|j| (0..6).map(move |i| Seed { pos: [20.0 + 10.0 * i as f64, 30.0, 20.0 + 10.0 * j as f64], axis: [0.0, 1.0, 0.0] })).collect();
        let closer = respaced(&seeds, 25.0, 10.0, 5.0);
        // A place every 5 voxels over the same stretch and half a step beyond.
        assert_eq!(closer.len(), 13 * 13, "{}", closer.len());
        assert!(closer.iter().all(|s| (s.pos[1] - 30.0).abs() < 1e-6 && s.axis[1] > 0.999));
        let wider = respaced(&seeds, 25.0, 10.0, 20.0);
        assert!(wider.len() >= 9 && wider.len() <= 18, "{}", wider.len());
        for (i, a) in wider.iter().enumerate() {
            for b in &wider[i + 1..] {
                assert!((0..3).map(|k| (a.pos[k] - b.pos[k]).powi(2)).sum::<f64>().sqrt() >= 14.0);
            }
        }
    }
}
