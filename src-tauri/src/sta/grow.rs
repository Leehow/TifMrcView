//! "Follow it up and down" (三维选择): the user painted a thing on one
//! section, and the program says which voxels above and below belong to it.
//!
//! The program knows nothing about what the thing is. All it has is the
//! paint: what was painted is the thing, and what was left unpainted on that
//! same section is not (the user saw that section and drew where the thing
//! ends). Everything else follows from that and from the file:
//!
//! - Box. Only a box about the paint is looked at: the paint's bounding box
//!   grown by the paint's own width in the section, and one and a half times
//!   its longest extent (stretched by the missing wedge) up and down. A box
//!   longer than `LONGEST` voxels is averaged down by a whole factor.
//! - Scale. The box is smoothed over an eighth of the paint's narrowest
//!   width: detail much finer than the thing is noise to this question.
//! - What it looks like. The values under the paint against the values in a
//!   ring just outside it give, for every voxel, how much more it looks
//!   like the thing than like its surroundings (the logarithm of the ratio
//!   of two histograms, `Likeness`). Darker, lighter or neither: whatever
//!   the paint says. The paint's outermost voxels are left out (there the
//!   hand decided, not the image), and so is what of the ring looks like
//!   the paint rather than like the rest of the ring (a neighbour of the
//!   same kind, a wall the thing sits on). When the values alone separate
//!   badly the local roughness is tried as well, and whichever tells the
//!   paint from its ring best counts.
//! - One surface. The selection is the labelling of the box that agrees
//!   best with those likenesses and has the smallest surface, a surface
//!   being cheap where the image has an edge (a minimal cut, solved in its
//!   continuous form: a primal-dual iteration on the relaxation of Chan,
//!   Esedoglu and Nikolova, every step a stencil, `Problem::solve`). The
//!   paint is inside; the rest of a painted section and whatever was erased
//!   is outside, and so are the sides of the box. A section may reach at
//!   most as far sideways from the nearest painted outline as it is away
//!   from it (the thing may lean and widen, it cannot jump), and has to pay
//!   for it: the farther out for its distance, the more a voxel must look
//!   like the thing (`LEAN`). Only what hangs together with the paint is
//!   kept.
//! - Where it ends, per direction, whichever comes first walking away from
//!   the paint: a section the user erased (`End::Stopped`); nothing is
//!   selected any more (`End::Ended`); a section three times the area of
//!   the paint, or one that grows again after having narrowed, is something
//!   else joined to it (`End::Joined`); the contrast against the ring fell
//!   to half of what the painted section has (`End::Faded`); or the thing
//!   runs out of the box, and as much of it as it is long in the section is
//!   kept (`End::Through`, `End::Edge` at the end of the file). Between
//!   two painted sections nothing is cut.
//! - Can it be trusted. Nobody knows the answer, but the paint is one: the
//!   same engine, given only the middle of the paint and not told where the
//!   paint ends, has to draw the paint again on the painted section. Since
//!   what the thing looks like was learnt from this very paint, the paint is
//!   cut in two and each half has to be drawn from what the other taught.
//!   When that fails (`TRUST`), or when the selection runs into something
//!   else in both directions, it is not used and the thickness is estimated
//!   instead ("flat"): the paint blown up from its own outline, a disc to a
//!   ball, a bar to a rod with round ends.
//! - Missing wedge. What is found is what the file shows, drawn out along z
//!   by the wedge (`elongation`). That is what the user is shown, because it
//!   has to agree with the picture. What is used as the example is cut back
//!   at both ends by half of what the wedge adds to something as wide as the
//!   paint is narrow, but not to less than that width nor to less than half
//!   of what was found (`Selection::used`).
//!
//! What it cannot do, as measured on phantoms (the tests below): a hollow
//! thing whose inside looks like its outside, and a thing that touches a
//! wall of its own density, are not drawn again from their middle and get
//! the estimated thickness. Two things that touch one above the other are
//! one thing in the file (the wedge fills the gap) and are selected as one;
//! the user erases a section between them.
//!
//! Nothing here knows of marks, strokes or the screen: `follow` takes the
//! painted sections as masks and a way to read a rectangle of a section,
//! and gives a `Selection` in voxels of the file painted on. Voxel positions
//! are indices (x, y, z); a voxel's centre is at its index.

use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// What the user painted on one section of the file: masks of the whole
/// section, row after row (`y * nx + x`). An empty mask stands for none.
#[derive(Clone, Debug, Default)]
pub struct Painted {
    pub z: usize,
    /// Painted: this is the thing.
    pub take: Vec<bool>,
    /// Erased: this is not.
    pub erase: Vec<bool>,
}

/// Reads the rectangle `[x0, y0, w, h]` of section z, row after row.
pub type Reader<'a> = &'a (dyn Fn(usize, [usize; 4]) -> Result<Vec<f32>> + Sync);

/// A `Reader` for those who can only read whole sections of width `nx`.
pub fn cropped(whole: impl Fn(usize) -> Result<Vec<f32>> + Sync, nx: usize) -> impl Fn(usize, [usize; 4]) -> Result<Vec<f32>> + Sync {
    move |z, [x0, y0, w, h]| {
        let s = whole(z)?;
        let mut out = Vec::with_capacity(w * h);
        for y in y0..y0 + h {
            out.extend_from_slice(&s[y * nx + x0..y * nx + x0 + w]);
        }
        Ok(out)
    }
}

/// Which way the selection was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// Followed through the sections by the minimal cut.
    Cut,
    /// Thickness estimated from the paint's own outline.
    Flat,
}

/// Why the selection ends where it does, in one direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum End {
    /// The thing ends here.
    Ended,
    /// It became too faint to tell (half the contrast of the painted section).
    Faded,
    /// It runs into something else; cut where that begins.
    Joined,
    /// It goes on and on; a piece as long as the paint is wide was kept.
    Through,
    /// It reaches the first or last section of the file.
    Edge,
    /// A section the user erased.
    Stopped,
    /// Not followed: the thickness is an estimate.
    Estimated,
}

/// How the selection came about: nothing a user reads, but what is needed
/// to see why it came out as it did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct How {
    /// The box that was looked at, `[x0, y0, z0]` and `[w, h, d]`, voxels.
    pub origin: [usize; 3],
    pub dims: [usize; 3],
    /// Voxels averaged along each direction.
    pub bin: usize,
    /// How well paint and ring are told apart, 0.5 (not) to 1 (always).
    pub separation: f32,
    /// "values", "roughness" or "both".
    pub feature: String,
    /// How much of the paint the engine drew again from its middle
    /// (overlap over union); below `TRUST` the selection is not used.
    pub drawn_again: f32,
    /// Rounds of the iteration.
    pub rounds: usize,
    /// Contrast of the painted section against its ring, in noise units.
    pub contrast: f32,
}

/// A side view of the selection: the cut through its middle along x and z.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Side {
    /// The row of the file the cut lies in.
    pub y: usize,
    /// Voxel of the first pixel, and voxels per pixel.
    pub x0: usize,
    pub z0: usize,
    pub step: usize,
    /// Pixels across (x) and down (z).
    pub w: usize,
    pub h: usize,
    pub gray: Vec<u8>,
}

/// The runs `[y, x0, x1)` of one section, as `paint.rs` gives them.
pub type Runs = Vec<[usize; 3]>;

/// A thing selected in three dimensions, in voxels of the file painted on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    pub engine: Engine,
    /// Size of the file, `[nx, ny, nz]`.
    pub file: [usize; 3],
    /// What was found and is shown: the sections it reaches, in order.
    pub shown: Vec<(usize, Runs)>,
    /// First and last section shown.
    pub z: [usize; 2],
    /// First and last section used as the example (`shown` with the
    /// stretch of the missing wedge taken off both ends).
    pub used: [usize; 2],
    /// Why it ends: towards section 0, towards the last section.
    pub ends: [End; 2],
    /// Smallest box holding what is shown: `[x0, y0, z0]`, `[w, h, d]`.
    pub origin: [usize; 3],
    pub dims: [usize; 3],
    /// Of what is used: centre of mass (x, y, z), the radius of the ball
    /// about it that holds it all, and its number of voxels.
    pub centre: [f64; 3],
    pub radius: f64,
    pub voxels: usize,
    /// Narrowest width and longest extent of the paint, voxels.
    pub width: f64,
    pub length: f64,
    /// How much the missing wedge draws things out along z.
    pub elongation: f64,
    pub how: How,
    pub side: Side,
}

/// Longest side of the box that is worked on.
const LONGEST: usize = 128;
/// Least room about the paint in the section.
const ROOM: usize = 12;
/// Bins of the histograms.
const BINS: usize = 32;
/// No voxel counts for more than this, however unlike the other side.
const SURE: f32 = 3.0;
/// The paint has to be drawn again this well (overlap over union).
pub const TRUST: f32 = 0.6;
/// A section this many times the paint's area is something else.
const LEAK: f64 = 3.0;
/// An erased section with at most this share left of the section before
/// it is where the user stopped it.
const GONE: f64 = 0.25;
/// A section under this share of the largest is a frayed tip.
const TIP: f64 = 0.1;
/// Growing again after a narrowest section by this factor …
const REGROWN: f64 = 1.25;
/// … and by this share of the paint's area is something else beginning.
const NECK: f64 = 0.1;
/// Values separate well enough alone above this.
const CLEAR: f32 = 0.85;
/// Tilt range taken when none is known, degrees.
const TILT: f64 = 60.0;
/// What a unit of surface costs against a unit of likeness, per voxel of
/// the paint's width: the volume of a thing grows with its width times its
/// surface, so this keeps the balance the same for small and large things.
const SMOOTH: f32 = 0.025;
/// A surface across the sharpest edge still costs this share.
const EDGE: f32 = 0.1;
/// The box is smoothed over the paint's width divided by this.
const FINE: f32 = 8.0;
/// Paint not deeper inside than this many voxels is its rim.
const RIM: f32 = 1.0;
/// A sample is spread over at least this many bins of the histogram.
const SPREAD: f32 = 1.5;
/// How much a voxel at the limit of leaning counts against the thing, in
/// units of `SURE`.
const LEAN: f32 = 2.0;
/// Room about the paint when it is drawn again, in widths of the paint.
const ROOM_AGAIN: f32 = 0.5;

/// How much a tilt range of ±`deg` draws a point out along z.
pub fn elongation(deg: f64) -> f64 {
    let a = deg.clamp(20.0, 90.0).to_radians();
    let sc = a.sin() * a.cos();
    ((a + sc) / (a - sc)).sqrt().max(1.0)
}

/// Follows the paint through the sections of a file of size `dims`
/// (`[nx, ny, nz]`). `tilt_max_deg` is the tilt range (±) the tomogram was
/// made from; `deep` false gives the estimated thickness at once. `cancel`
/// is asked now and then; `progress` gets (done, total).
pub fn follow(read: Reader, dims: [usize; 3], painted: &[Painted], tilt_max_deg: Option<f64>, deep: bool, cancel: &(dyn Fn() -> bool + Sync), progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Selection> {
    let paint = Paint::of(dims, painted)?;
    let e = elongation(tilt_max_deg.unwrap_or(TILT));
    let mut drawn_again = 0.0;
    if deep {
        match cut(read, &paint, e, cancel, progress)? {
            Ok(s) => return Ok(s),
            Err(again) => drawn_again = again,
        }
    }
    let mut s = flat(read, &paint, e, cancel)?;
    s.how.drawn_again = drawn_again;
    progress(1, 1);
    Ok(s)
}

fn stop(cancel: &(dyn Fn() -> bool + Sync)) -> Result<()> {
    if cancel() {
        bail!("已取消");
    }
    Ok(())
}

// ---------------------------------------------------------------- the paint

/// One painted section, cut down to the rectangle all the paint lies in.
struct Layer {
    z: usize,
    take: Vec<bool>,
    erase: Vec<bool>,
}

/// The paint of one example.
struct Paint {
    file: [usize; 3],
    /// `[x0, y0, w, h]` holding all that was painted or erased.
    rect: [usize; 4],
    /// Sections in order of z; those with paint left have `take` set.
    layers: Vec<Layer>,
    /// Narrowest width and longest extent in the section.
    width: f64,
    length: f64,
}

impl Paint {
    fn of(file: [usize; 3], painted: &[Painted]) -> Result<Paint> {
        let [nx, ny, nz] = file;
        let (mut lo, mut hi) = ([usize::MAX; 2], [0usize; 2]);
        for p in painted {
            if p.z >= nz {
                bail!("涂的那一层不在文件里");
            }
            for m in [&p.take, &p.erase] {
                if !m.is_empty() && m.len() != nx * ny {
                    bail!("涂的范围和文件大小不一致");
                }
                for (i, _) in m.iter().enumerate().filter(|(_, v)| **v) {
                    let (x, y) = (i % nx, i / nx);
                    lo = [lo[0].min(x), lo[1].min(y)];
                    hi = [hi[0].max(x), hi[1].max(y)];
                }
            }
        }
        if lo[0] == usize::MAX {
            bail!("还没有涂");
        }
        let rect = [lo[0], lo[1], hi[0] - lo[0] + 1, hi[1] - lo[1] + 1];
        let mut layers: Vec<Layer> = Vec::new();
        let mut order: Vec<&Painted> = painted.iter().collect();
        order.sort_by_key(|p| p.z);
        for p in order {
            let crop = |m: &Vec<bool>| -> Vec<bool> {
                let mut out = vec![false; rect[2] * rect[3]];
                if !m.is_empty() {
                    for y in 0..rect[3] {
                        let src = (rect[1] + y) * nx + rect[0];
                        out[y * rect[2]..(y + 1) * rect[2]].copy_from_slice(&m[src..src + rect[2]]);
                    }
                }
                out
            };
            let (erase, mut take) = (crop(&p.erase), crop(&p.take));
            take.iter_mut().zip(&erase).for_each(|(t, e)| *t &= !e);
            match layers.last_mut() {
                Some(l) if l.z == p.z => {
                    l.take.iter_mut().zip(&take).for_each(|(a, b)| *a |= b);
                    l.erase.iter_mut().zip(&erase).for_each(|(a, b)| *a |= b);
                    let Layer { take, erase, .. } = l;
                    take.iter_mut().zip(erase.iter()).for_each(|(t, e)| *t &= !e);
                }
                _ => layers.push(Layer { z: p.z, take, erase }),
            }
        }
        let (mut width, mut length) = (0f64, 0f64);
        for l in &layers {
            let d = inside(&l.take, rect[2], rect[3]);
            width = width.max(2.0 * d.iter().fold(0f32, |a, b| a.max(*b)) as f64 + 1.0);
            let (mut a, mut b) = ([usize::MAX; 2], [0usize; 2]);
            for (i, _) in l.take.iter().enumerate().filter(|(_, v)| **v) {
                a = [a[0].min(i % rect[2]), a[1].min(i / rect[2])];
                b = [b[0].max(i % rect[2]), b[1].max(i / rect[2])];
            }
            if a[0] != usize::MAX {
                length = length.max((b[0] - a[0] + 1).max(b[1] - a[1] + 1) as f64);
            }
        }
        if width <= 0.0 {
            bail!("还没有涂");
        }
        Ok(Paint { file, rect, layers, width, length: length.max(width) })
    }

    /// The sections that have paint.
    fn taken(&self) -> impl Iterator<Item = &Layer> {
        self.layers.iter().filter(|l| l.take.iter().any(|v| *v))
    }
}

// ------------------------------------------------------- distances in a plane

/// One pass of the squared distance transform along a line (Felzenszwalb
/// and Huttenlocher): `f` in, the lower envelope of its parabolas out.
fn envelope(f: &[f32], out: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let mut k = 0;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        if f[q] >= FAR {
            continue;
        }
        loop {
            let p = v[k];
            let s = if f[p] >= FAR { f32::NEG_INFINITY } else { ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32)) };
            if s <= z[k] && k > 0 {
                k -= 1;
            } else if s <= z[k] {
                v[0] = q;
                z[1] = f32::INFINITY;
                break;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f32::INFINITY;
                break;
            }
        }
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        out[q] = if f[p] >= FAR { FAR } else { (q as f32 - p as f32).powi(2) + f[p] };
    }
}

const FAR: f32 = 1e20;

/// Distance of every pixel to the nearest pixel of `to` (0 on it).
fn distance(to: &[bool], w: usize, h: usize) -> Vec<f32> {
    let mut d: Vec<f32> = to.iter().map(|t| if *t { 0.0 } else { FAR }).collect();
    let m = w.max(h);
    let (mut f, mut o, mut v, mut z) = (vec![0f32; m], vec![0f32; m], vec![0usize; m], vec![0f32; m + 1]);
    for y in 0..h {
        f[..w].copy_from_slice(&d[y * w..(y + 1) * w]);
        envelope(&f[..w], &mut o[..w], &mut v, &mut z);
        d[y * w..(y + 1) * w].copy_from_slice(&o[..w]);
    }
    for x in 0..w {
        for y in 0..h {
            f[y] = d[y * w + x];
        }
        envelope(&f[..h], &mut o[..h], &mut v, &mut z);
        for y in 0..h {
            d[y * w + x] = o[y];
        }
    }
    d.iter_mut().for_each(|v| *v = v.min(FAR).sqrt());
    d
}

/// How deep inside the mask every pixel lies: its distance to the mask's
/// edge (0 outside, about half a pixel on the rim). Beyond the picture is
/// outside.
fn inside(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    let (pw, ph) = (w + 2, h + 2);
    let mut out = vec![true; pw * ph];
    for y in 0..h {
        for x in 0..w {
            out[(y + 1) * pw + x + 1] = !mask[y * w + x];
        }
    }
    let d = distance(&out, pw, ph);
    let mut r = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            r[y * w + x] = (d[(y + 1) * pw + x + 1] - 0.5).max(0.0);
        }
    }
    r
}

// -------------------------------------------------------- estimated thickness

/// The paint blown up from its own outline: over a pixel that lies d inside
/// a paint whose deepest pixel lies R inside, the thing is taken to reach
/// sqrt(d·(2R − d)) sections up and down. A disc becomes its ball.
fn flat(read: Reader, paint: &Paint, e: f64, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Selection> {
    let [x0, y0, w, h] = paint.rect;
    let nz = paint.file[2];
    // Per painted section: how far up and down each pixel reaches.
    let reach: Vec<(usize, Vec<f32>)> = paint
        .taken()
        .map(|l| {
            let d = inside(&l.take, w, h);
            let r = d.iter().fold(0f32, |a, b| a.max(*b)) + 0.5;
            (l.z, d.iter().zip(&l.take).map(|(d, t)| if *t { (d * (2.0 * r - d)).max(0.0).sqrt() } else { -1.0 }).collect())
        })
        .collect();
    let top = |r: &Vec<f32>| r.iter().fold(0f32, |a, b| a.max(*b)).floor() as usize;
    let z0 = reach.iter().map(|(z, r)| z.saturating_sub(top(r))).min().unwrap_or(0);
    let z1 = reach.iter().map(|(z, r)| (z + top(r)).min(nz - 1)).max().unwrap_or(0);
    let mut shown = Vec::new();
    for z in z0..=z1 {
        let mut runs = Runs::new();
        for y in 0..h {
            let mut start = None;
            for x in 0..=w {
                let on = x < w && reach.iter().any(|(zp, r)| r[y * w + x] >= z.abs_diff(*zp) as f32);
                match (on, start) {
                    (true, None) => start = Some(x),
                    (false, Some(s)) => {
                        runs.push([y0 + y, x0 + s, x0 + x]);
                        start = None;
                    }
                    _ => {}
                }
            }
        }
        if !runs.is_empty() {
            shown.push((z, runs));
        }
    }
    let mut s = Selection::of(Engine::Flat, paint, shown, None, [End::Estimated; 2], e)?;
    stop(cancel)?;
    // The side view: one row of every `step`-th section.
    let room = (paint.width.ceil() as usize / 2).max(4);
    let (sx0, sx1) = (s.origin[0].saturating_sub(room), (s.origin[0] + s.dims[0] + room).min(paint.file[0]));
    let (sz0, sz1) = (s.origin[2].saturating_sub(room), (s.origin[2] + s.dims[2] + room).min(nz));
    let step = (sx1 - sx0).max(sz1 - sz0).div_ceil(LONGEST).max(1);
    let y = (s.centre[1].round() as usize).min(paint.file[1] - 1);
    let (sw, sh) = ((sx1 - sx0) / step, (sz1 - sz0) / step);
    let rows: Vec<Vec<f32>> = (0..sh).into_par_iter().map(|k| read(sz0 + k * step, [sx0, y, sx1 - sx0, 1])).collect::<Result<_>>()?;
    let values: Vec<f32> = rows.iter().flat_map(|r| (0..sw).map(move |i| r[i * step])).collect();
    s.side = Side { y, x0: sx0, z0: sz0, step, w: sw, h: sh, gray: gray(&values) };
    s.how.origin = [x0, y0, z0];
    s.how.dims = [w, h, z1 - z0 + 1];
    s.how.bin = 1;
    Ok(s)
}

/// Values as grey levels between their 1st and 99th hundredths.
fn gray(v: &[f32]) -> Vec<u8> {
    if v.is_empty() {
        return Vec::new();
    }
    let mut s: Vec<f32> = v.iter().map(|x| if x.is_finite() { *x } else { 0.0 }).collect();
    s.sort_by(|a, b| a.total_cmp(b));
    let (lo, hi) = (s[s.len() / 100], s[s.len() - 1 - s.len() / 100]);
    let span = (hi - lo).max(1e-12);
    v.iter().map(|x| (((x - lo) / span).clamp(0.0, 1.0) * 255.0) as u8).collect()
}

// ------------------------------------------------------------- the selection

impl Selection {
    /// Fills in what follows from the sections shown. `used` is the range
    /// of sections used (all when None).
    fn of(engine: Engine, paint: &Paint, shown: Vec<(usize, Runs)>, used: Option<[usize; 2]>, ends: [End; 2], e: f64) -> Result<Selection> {
        let (Some(first), Some(last)) = (shown.first(), shown.last()) else { bail!("没有选到东西") };
        let z = [first.0, last.0];
        let used = used.unwrap_or(z);
        let (mut lo, mut hi) = ([usize::MAX; 2], [0usize; 2]);
        let (mut sum, mut n) = ([0f64; 3], 0usize);
        for (sz, runs) in &shown {
            for [y, a, b] in runs {
                lo = [lo[0].min(*a), lo[1].min(*y)];
                hi = [hi[0].max(*b), hi[1].max(*y + 1)];
                if (used[0]..=used[1]).contains(sz) {
                    let len = b - a;
                    sum[0] += (a + b - 1) as f64 / 2.0 * len as f64;
                    sum[1] += (*y * len) as f64;
                    sum[2] += (*sz * len) as f64;
                    n += len;
                }
            }
        }
        if n == 0 {
            bail!("没有选到东西");
        }
        let centre = [sum[0] / n as f64, sum[1] / n as f64, sum[2] / n as f64];
        let mut far = 0f64;
        for (sz, runs) in shown.iter().filter(|(sz, _)| (used[0]..=used[1]).contains(sz)) {
            for [y, a, b] in runs {
                for x in [*a, b - 1] {
                    let d = [x as f64 - centre[0], *y as f64 - centre[1], *sz as f64 - centre[2]];
                    far = far.max(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
                }
            }
        }
        Ok(Selection {
            engine,
            file: paint.file,
            shown,
            z,
            used,
            ends,
            origin: [lo[0], lo[1], z[0]],
            dims: [hi[0] - lo[0], hi[1] - lo[1], z[1] - z[0] + 1],
            centre,
            radius: far.sqrt() + 0.5,
            voxels: n,
            width: paint.width,
            length: paint.length,
            elongation: e,
            how: How::default(),
            side: Side::default(),
        })
    }

    /// The runs of section z as shown (empty where it does not reach).
    pub fn section(&self, z: usize) -> &[[usize; 3]] {
        match self.shown.binary_search_by_key(&z, |(sz, _)| *sz) {
            Ok(i) => &self.shown[i].1,
            Err(_) => &[],
        }
    }

    /// Whether section z belongs to what is used as the example.
    pub fn is_used(&self, z: usize) -> bool {
        (self.used[0]..=self.used[1]).contains(&z)
    }

    /// The sections used as the example (shown, less the wedge's stretch).
    pub fn used_sections(&self) -> impl Iterator<Item = &(usize, Runs)> {
        self.shown.iter().filter(|(z, _)| self.is_used(*z))
    }

    /// The selection as a block of voxels: its first voxel `[x0, y0, z0]`,
    /// its size `[w, h, d]`, and the mask (z, y, x order). `used` true
    /// gives what is used as the example, false what is shown.
    pub fn mask(&self, used: bool) -> ([usize; 3], [usize; 3], Vec<bool>) {
        let (z0, z1) = if used { (self.used[0], self.used[1]) } else { (self.z[0], self.z[1]) };
        let (o, d) = ([self.origin[0], self.origin[1], z0], [self.dims[0], self.dims[1], z1 - z0 + 1]);
        let mut m = vec![false; d[0] * d[1] * d[2]];
        for (z, runs) in self.shown.iter().filter(|(z, _)| (z0..=z1).contains(z)) {
            for [y, a, b] in runs {
                let row = ((z - z0) * d[1] + (y - o[1])) * d[0];
                m[row + a - o[0]..row + b - o[0]].fill(true);
            }
        }
        (o, d, m)
    }

    /// Height, sections: (shown, used).
    pub fn heights(&self) -> (usize, usize) {
        (self.z[1] - self.z[0] + 1, self.used[1] - self.used[0] + 1)
    }

    /// The side view as a picture (RGBA, `side.w` × `side.h`, x across, z
    /// down): the cut in grey, what is used tinted with `colour` and
    /// outlined, what is shown beyond it outlined only.
    pub fn side_rgba(&self, colour: [u8; 3]) -> (usize, usize, Vec<u8>) {
        let s = &self.side;
        let mut state = vec![0u8; s.w * s.h];
        for k in 0..s.h {
            let z = s.z0 + k * s.step + s.step / 2;
            for [y, a, b] in self.section(z) {
                if *y == s.y {
                    for i in 0..s.w {
                        let x = s.x0 + i * s.step + s.step / 2;
                        if (*a..*b).contains(&x) {
                            state[k * s.w + i] = if self.is_used(z) { 2 } else { 1 };
                        }
                    }
                }
            }
        }
        let mut out = vec![255u8; s.w * s.h * 4];
        for k in 0..s.h {
            for i in 0..s.w {
                let v = state[k * s.w + i];
                let g = s.gray.get(k * s.w + i).copied().unwrap_or(0) as f32;
                let at = |dk: isize, di: isize| -> u8 {
                    let (kk, ii) = (k as isize + dk, i as isize + di);
                    if kk < 0 || ii < 0 || kk >= s.h as isize || ii >= s.w as isize { 0 } else { state[kk as usize * s.w + ii as usize] }
                };
                let rim = v > 0 && [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|(a, b)| at(*a, *b) == 0);
                let mix = if rim { if v == 2 { 1.0 } else { 0.6 } } else if v == 2 { 0.25 } else { 0.0 };
                for c in 0..3 {
                    out[(k * s.w + i) * 4 + c] = (g * (1.0 - mix) + colour[c] as f32 * mix) as u8;
                }
            }
        }
        (s.w, s.h, out)
    }

    /// Writes the selection to a file (JSON), to be read back by `load`.
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("sel.part");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &std::path::Path) -> Result<Selection> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes).with_context(|| format!("读不了 {}", path.display()))
    }
}

// --------------------------------------------------------------- the engine

/// The box worked on: a block of the file averaged over `bin`³ voxels.
struct Block {
    /// First voxel in the file and size in averaged voxels `[nx, ny, nz]`.
    origin: [usize; 3],
    n: [usize; 3],
    bin: usize,
    /// Whether it ends where the file does: below, above.
    edge: [bool; 2],
}

impl Block {
    fn around(paint: &Paint, e: f64) -> Block {
        let [fx, fy, fz] = paint.file;
        let room = (paint.width.ceil() as usize).max(ROOM);
        let x0 = paint.rect[0].saturating_sub(room);
        let y0 = paint.rect[1].saturating_sub(room);
        let x1 = (paint.rect[0] + paint.rect[2] + room).min(fx);
        let y1 = (paint.rect[1] + paint.rect[3] + room).min(fy);
        let (first, last) = (paint.taken().next().map_or(0, |l| l.z), paint.taken().last().map_or(0, |l| l.z));
        let reach = (paint.length * e * 1.5).ceil() as usize;
        let across = (x1 - x0).max(y1 - y0);
        // The section sets the averaging; along z it is averaged only as far
        // as the paint's width allows (eight voxels across it are kept), and
        // the rest is given up in reach.
        let tall = last - first + 1 + 2 * reach;
        let bin = across.div_ceil(LONGEST).max(tall.div_ceil(LONGEST).min((paint.width / 8.0) as usize)).max(1);
        let reach = reach.min((LONGEST * bin).saturating_sub(last - first + 1) / 2).max(2 * bin);
        let z0 = first.saturating_sub(reach);
        let z1 = (last + reach + 1).min(fz);
        let n = [((x1 - x0) / bin).max(1), ((y1 - y0) / bin).max(1), ((z1 - z0) / bin).max(1)];
        Block { origin: [x0, y0, z0], n, bin, edge: [z0 == 0, z0 + n[2] * bin + bin > fz] }
    }

    fn load(&self, read: Reader, cancel: &(dyn Fn() -> bool + Sync), tick: &(dyn Fn() + Sync)) -> Result<Vec<f32>> {
        let [nx, ny, nz] = self.n;
        let b = self.bin;
        let mut v = vec![0f32; nx * ny * nz];
        let scale = 1.0 / (b * b * b) as f32;
        v.par_chunks_mut(nx * ny).enumerate().try_for_each(|(k, plane)| -> Result<()> {
            for dz in 0..b {
                stop(cancel)?;
                let s = read(self.origin[2] + k * b + dz, [self.origin[0], self.origin[1], nx * b, ny * b])?;
                if s.len() != nx * b * ny * b {
                    bail!("读到的范围不对");
                }
                for (y, row) in plane.chunks_mut(nx).enumerate() {
                    for dy in 0..b {
                        let src = &s[(y * b + dy) * nx * b..(y * b + dy + 1) * nx * b];
                        for (x, o) in row.iter_mut().enumerate() {
                            for dx in 0..b {
                                *o += src[x * b + dx] * scale;
                            }
                        }
                    }
                }
            }
            tick();
            Ok(())
        })?;
        // Zero mean, unit deviation; what is not a number counts as the mean.
        let ok = v.iter().filter(|x| x.is_finite()).count().max(1) as f64;
        let mean = v.iter().filter(|x| x.is_finite()).map(|x| *x as f64).sum::<f64>() / ok;
        let var = v.iter().filter(|x| x.is_finite()).map(|x| (*x as f64 - mean).powi(2)).sum::<f64>() / ok;
        let sd = var.sqrt().max(1e-12);
        v.par_iter_mut().for_each(|x| *x = if x.is_finite() { ((*x as f64 - mean) / sd) as f32 } else { 0.0 });
        Ok(v)
    }

    /// A mask of the paint's rectangle on the block's plane: a voxel is on
    /// when at least half of the pixels averaged into it are.
    fn plane(&self, paint: &Paint, mask: &[bool]) -> Vec<bool> {
        let [nx, ny, _] = self.n;
        let b = self.bin;
        let [rx, ry, rw, rh] = paint.rect;
        let mut count = vec![0usize; nx * ny];
        for y in 0..rh {
            for x in 0..rw {
                if mask[y * rw + x] {
                    let (i, j) = ((rx + x - self.origin[0]) / b, (ry + y - self.origin[1]) / b);
                    if i < nx && j < ny {
                        count[j * nx + i] += 1;
                    }
                }
            }
        }
        count.iter().map(|c| 2 * c >= b * b).collect()
    }
}

/// Separable Gaussian blur of a block (z, y, x order; edges repeated).
fn blur(v: &[f32], n: [usize; 3], sigma: f64) -> Vec<f32> {
    if sigma < 0.4 {
        return v.to_vec();
    }
    let [nx, ny, nz] = n;
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp() as f32).collect();
    let s: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|x| x / s).collect();
    let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
    let plane = nx * ny;
    let mut a = vec![0f32; v.len()];
    a.par_chunks_mut(plane).zip(v.par_chunks(plane)).for_each(|(o, src)| {
        let mut t = vec![0f32; plane];
        for y in 0..ny {
            let row = &src[y * nx..(y + 1) * nx];
            for x in 0..nx {
                let mut acc = 0f32;
                for (j, w) in k.iter().enumerate() {
                    acc += w * row[clamp(x as isize + j as isize - r, nx)];
                }
                t[y * nx + x] = acc;
            }
        }
        for y in 0..ny {
            for (j, w) in k.iter().enumerate() {
                let from = clamp(y as isize + j as isize - r, ny) * nx;
                for x in 0..nx {
                    o[y * nx + x] += w * t[from + x];
                }
            }
        }
    });
    let mut out = vec![0f32; v.len()];
    out.par_chunks_mut(plane).enumerate().for_each(|(z, o)| {
        for (j, w) in k.iter().enumerate() {
            let from = clamp(z as isize + j as isize - r, nz) * plane;
            for (o, s) in o.iter_mut().zip(&a[from..from + plane]) {
                *o += w * s;
            }
        }
    });
    out
}

/// How much more a value is the thing's than its surroundings': the
/// logarithm of the ratio of the two smoothed histograms.
struct Likeness {
    lo: f32,
    step: f32,
    llr: [f32; BINS],
}

impl Likeness {
    fn learn(fg: &[f32], bg: &[f32]) -> Likeness {
        let all = || fg.iter().chain(bg);
        let lo = all().fold(f32::INFINITY, |a, b| a.min(*b));
        let hi = all().fold(f32::NEG_INFINITY, |a, b| a.max(*b));
        let pad = (hi - lo).max(1e-6) * 0.05;
        let (lo, hi) = (lo - pad, hi + pad);
        let step = (hi - lo) / BINS as f32;
        let hist = |s: &[f32]| -> [f32; BINS] {
            let n = s.len().max(1) as f32;
            let mean = s.iter().sum::<f32>() / n;
            let sd = (s.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n).sqrt();
            // Each sample is spread by the usual rule for a smooth density
            // from few samples, never by less than `SPREAD` bins, and with its
            // whole tail: between the two sides, where neither has a sample,
            // the nearer side wins.
            let h = (1.06 * sd * n.powf(-0.2) / step).max(knob("GROW_SPREAD", SPREAD));
            let mut out = [0f32; BINS];
            for v in s {
                let c = (v - lo) / step - 0.5;
                for (i, o) in out.iter_mut().enumerate() {
                    *o += (-(i as f32 - c).powi(2) / (2.0 * h * h)).exp();
                }
            }
            let sum = out.iter().sum::<f32>().max(1e-12);
            out.iter_mut().for_each(|o| *o /= sum);
            out
        };
        let (f, b) = (hist(fg), hist(bg));
        let floor = 1e-6;
        let mut llr = [0f32; BINS];
        for i in 0..BINS {
            llr[i] = ((f[i] + floor) / (b[i] + floor)).ln().clamp(-SURE, SURE);
        }
        Likeness { lo, step, llr }
    }

    fn at(&self, v: f32) -> f32 {
        let c = ((v - self.lo) / self.step - 0.5).clamp(0.0, BINS as f32 - 1.0);
        let i = (c.floor() as usize).min(BINS - 2);
        let t = c - i as f32;
        self.llr[i] * (1.0 - t) + self.llr[i + 1] * t
    }
}

/// How often a sample of the paint scores above a sample of the ring.
fn separation(fg: &[f32], bg: &[f32]) -> f32 {
    if fg.is_empty() || bg.is_empty() {
        return 0.5;
    }
    let mut all: Vec<(f32, bool)> = fg.iter().map(|v| (*v, true)).chain(bg.iter().map(|v| (*v, false))).collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut below, mut wins) = (0f64, 0f64);
    let mut i = 0;
    while i < all.len() {
        let mut j = i;
        let (mut f, mut b) = (0f64, 0f64);
        while j < all.len() && all[j].0 == all[i].0 {
            if all[j].1 { f += 1.0 } else { b += 1.0 }
            j += 1;
        }
        wins += f * (below + b / 2.0);
        below += b;
        i = j;
    }
    (wins / (fg.len() as f64 * bg.len() as f64)) as f32
}

/// The labelling problem on a block: `f` what it costs a voxel to be the
/// thing, `g` what a unit of surface costs there, `fix` 1 where a voxel is
/// outside for sure and 2 where it is inside.
struct Problem<'a> {
    n: [usize; 3],
    f: &'a [f32],
    g: &'a [f32],
    fix: &'a [u8],
}

/// Rounds of the iteration at most, and between looks at whether the
/// labelling still changes.
/// A constant of the engine. In the tests it can be set from the
/// environment, to try values on real data without building again.
#[cfg(test)]
fn knob(name: &str, d: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

#[cfg(not(test))]
#[inline]
fn knob(_name: &str, d: f32) -> f32 {
    d
}

const ROUNDS: usize = 600;
const LOOK: usize = 10;

impl Problem<'_> {
    /// Minimises Σ f·u + Σ g·|∇u| over 0 ≤ u ≤ 1 with the fixed voxels held
    /// (Chambolle and Pock's primal-dual iteration with the diagonal steps
    /// of their 2011 preconditioning: 1/6 for a voxel, 1/2 for a gradient),
    /// from the start `u`. Cutting the result at one half is a minimal cut.
    /// Gives the number of rounds made.
    fn solve(&self, u: &mut [f32], rounds: usize, cancel: &(dyn Fn() -> bool + Sync), tick: &(dyn Fn(usize) + Sync)) -> Result<usize> {
        let [nx, ny, nz] = self.n;
        let plane = nx * ny;
        // Per plane, the rectangle outside of which everything is fixed
        // outside (one voxel of it included): nothing happens beyond.
        let rects: Vec<[usize; 4]> = (0..nz)
            .into_par_iter()
            .map(|z| {
                let fix = &self.fix[z * plane..(z + 1) * plane];
                let (mut a, mut b) = ([usize::MAX; 2], [0usize; 2]);
                for (i, _) in fix.iter().enumerate().filter(|(_, v)| **v != 1) {
                    a = [a[0].min(i % nx), a[1].min(i / nx)];
                    b = [b[0].max(i % nx), b[1].max(i / nx)];
                }
                if a[0] == usize::MAX { [0, 0, 0, 0] } else { [a[0].saturating_sub(1), a[1].saturating_sub(1), (b[0] + 2).min(nx), (b[1] + 2).min(ny)] }
            })
            .collect();
        // A plane's rectangle together with its neighbours', so that the
        // differences across planes are all inside.
        let wide: Vec<[usize; 4]> = (0..nz)
            .map(|z| {
                let mut r = [usize::MAX, usize::MAX, 0, 0];
                for q in z.saturating_sub(1)..(z + 2).min(nz) {
                    if rects[q][2] > rects[q][0] {
                        r = [r[0].min(rects[q][0]), r[1].min(rects[q][1]), r[2].max(rects[q][2]), r[3].max(rects[q][3])];
                    }
                }
                if r[0] == usize::MAX { [0, 0, 0, 0] } else { r }
            })
            .collect();
        for (i, v) in u.iter_mut().enumerate() {
            match self.fix[i] {
                1 => *v = 0.0,
                2 => *v = 1.0,
                _ => {}
            }
        }
        let mut bar = u.to_vec();
        let mut p = vec![[0f32; 3]; u.len()];
        let mut before: Vec<bool> = u.iter().map(|v| *v > 0.5).collect();
        let (tau, sigma) = (1.0 / 6.0f32, 0.5f32);
        let mut calm = 0;
        for round in 1..=rounds {
            // The dual step: p follows the gradient of ū, held within g.
            p.par_chunks_mut(plane).enumerate().for_each(|(z, p)| {
                let [x0, y0, x1, y1] = wide[z];
                let here = &bar[z * plane..(z + 1) * plane];
                let above = if z + 1 < nz { Some(&bar[(z + 1) * plane..(z + 2) * plane]) } else { None };
                let g = &self.g[z * plane..(z + 1) * plane];
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = y * nx + x;
                        let c = here[i];
                        let dx = if x + 1 < nx { here[i + 1] - c } else { 0.0 };
                        let dy = if y + 1 < ny { here[i + nx] - c } else { 0.0 };
                        let dz = above.map_or(0.0, |a| a[i] - c);
                        let q = [p[i][0] + sigma * dx, p[i][1] + sigma * dy, p[i][2] + sigma * dz];
                        let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
                        let s = if norm > g[i] { g[i] / norm } else { 1.0 };
                        p[i] = [q[0] * s, q[1] * s, q[2] * s];
                    }
                }
            });
            // The primal step: u follows the divergence of p less the cost.
            u.par_chunks_mut(plane).zip(bar.par_chunks_mut(plane)).enumerate().for_each(|(z, (u, bar))| {
                let [x0, y0, x1, y1] = wide[z];
                let here = &p[z * plane..(z + 1) * plane];
                let below = if z > 0 { Some(&p[(z - 1) * plane..z * plane]) } else { None };
                let f = &self.f[z * plane..(z + 1) * plane];
                let fix = &self.fix[z * plane..(z + 1) * plane];
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = y * nx + x;
                        if fix[i] != 0 {
                            continue;
                        }
                        let mut div = here[i][0] + here[i][1] + here[i][2];
                        if x > 0 {
                            div -= here[i - 1][0];
                        }
                        if y > 0 {
                            div -= here[i - nx][1];
                        }
                        if let Some(b) = below {
                            div -= b[i][2];
                        }
                        let new = (u[i] + tau * (div - f[i])).clamp(0.0, 1.0);
                        bar[i] = 2.0 * new - u[i];
                        u[i] = new;
                    }
                }
            });
            if round % LOOK == 0 {
                stop(cancel)?;
                tick(round);
                let (changed, on): (usize, usize) = before
                    .par_iter_mut()
                    .zip(u.par_iter())
                    .map(|(b, u)| {
                        let now = *u > 0.5;
                        let c = (*b != now) as usize;
                        *b = now;
                        (c, now as usize)
                    })
                    .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
                calm = if changed * 1000 <= on { calm + 1 } else { 0 };
                if calm >= 3 && round >= 40 {
                    return Ok(round);
                }
            }
        }
        Ok(rounds)
    }
}

/// Keeps of `on` what hangs together with a voxel of `from` (by faces).
fn connected(on: &mut [bool], from: &[usize], n: [usize; 3]) {
    let [nx, ny, nz] = n;
    let mut keep = vec![false; on.len()];
    let mut stack: Vec<usize> = from.iter().copied().filter(|i| on[*i]).collect();
    stack.iter().for_each(|i| keep[*i] = true);
    while let Some(i) = stack.pop() {
        let (x, y, z) = (i % nx, i / nx % ny, i / (nx * ny));
        let mut go = |j: usize| {
            if on[j] && !keep[j] {
                keep[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            go(i - 1);
        }
        if x + 1 < nx {
            go(i + 1);
        }
        if y > 0 {
            go(i - nx);
        }
        if y + 1 < ny {
            go(i + nx);
        }
        if z > 0 {
            go(i - nx * ny);
        }
        if z + 1 < nz {
            go(i + nx * ny);
        }
    }
    on.copy_from_slice(&keep);
}

/// The band about a mask in a plane between `near` and `far` pixels away.
fn ring(mask: &[bool], w: usize, h: usize, near: f32, far: f32) -> Vec<bool> {
    distance(mask, w, h).iter().map(|d| *d >= near && *d <= far).collect()
}

/// The engine. Err when it could not draw the paint again (`TRUST`, with how
/// well it did) and the thickness has to be estimated.
fn cut(read: Reader, paint: &Paint, e: f64, cancel: &(dyn Fn() -> bool + Sync), progress: &(dyn Fn(usize, usize) + Sync)) -> Result<std::result::Result<Selection, f32>> {
    let block = Block::around(paint, e);
    let [nx, ny, nz] = block.n;
    let (plane, b) = (nx * ny, block.bin);
    // Progress: the planes read, then the rounds of the two solutions.
    let total = nz + 2 * ROUNDS / LOOK;
    let done = AtomicUsize::new(0);
    let v = block.load(read, cancel, &|| progress(done.fetch_add(1, Ordering::Relaxed) + 1, total))?;
    let width = (paint.width / b as f64).max(1.0);
    let s = blur(&v, block.n, width / knob("GROW_FINE", FINE) as f64);
    stop(cancel)?;

    // The painted and erased planes of the block.
    let mut take: Vec<Option<Vec<bool>>> = vec![None; nz];
    let mut erase: Vec<Option<Vec<bool>>> = vec![None; nz];
    for l in &paint.layers {
        if l.z < block.origin[2] || (l.z - block.origin[2]) / b >= nz {
            continue;
        }
        let k = (l.z - block.origin[2]) / b;
        for (mask, into) in [(&l.take, &mut take), (&l.erase, &mut erase)] {
            let m = block.plane(paint, mask);
            if m.iter().any(|v| *v) {
                match &mut into[k] {
                    Some(old) => old.iter_mut().zip(&m).for_each(|(a, b)| *a |= b),
                    None => into[k] = Some(m),
                }
            }
        }
    }
    for k in 0..nz {
        if let (Some(t), Some(e)) = (&mut take[k], &erase[k]) {
            t.iter_mut().zip(e).for_each(|(t, e)| *t &= !e);
            if !t.iter().any(|v| *v) {
                take[k] = None;
            }
        }
    }
    let painted: Vec<usize> = (0..nz).filter(|k| take[*k].is_some()).collect();
    if painted.is_empty() {
        return Ok(Err(0.0));
    }
    let area = |m: &[bool]| m.iter().filter(|v| **v).count();
    // The plane with the most paint stands for the paint where one is needed.
    let main = *painted.iter().max_by_key(|k| area(take[**k].as_ref().unwrap())).unwrap();

    // What the thing looks like: paint against the ring about it.
    let (near, far) = ((0.25 * width as f32).max(1.0), (0.75 * width as f32).max(2.5));
    let (mut fg, mut bg): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    for k in &painted {
        let t = take[*k].as_ref().unwrap();
        let r = ring(t, nx, ny, near, far);
        let gone = erase[*k].as_ref();
        // The paint's rim is where the user's hand decided, not the image:
        // it is left out when enough paint remains without it.
        let deep = inside(t, nx, ny);
        let rim = knob("GROW_RIM", RIM);
        let rim = if 10 * deep.iter().filter(|d| **d > rim).count() >= 3 * area(t) { rim } else { 0.0 };
        for i in 0..plane {
            if t[i] {
                if deep[i] > rim {
                    fg.push(k * plane + i);
                }
            } else if r[i] && !gone.is_some_and(|e| e[i]) {
                bg.push(k * plane + i);
            }
        }
    }
    if fg.is_empty() || bg.len() < 4 {
        return Ok(Err(0.0));
    }
    let pick = |from: &[f32], at: &[usize]| -> Vec<f32> { at.iter().map(|i| from[*i]).collect() };
    // The ring may hold neighbours that look like the thing itself (the
    // next particle, a wall it sits on). They are not what its surroundings
    // look like: what of the ring lies nearer to the paint's middle value
    // than to the ring's own is left out, unless that is most of the ring.
    {
        let middle = |at: &[usize]| -> f32 {
            let mut v = pick(&s, at);
            v.sort_by(|a, b| a.total_cmp(b));
            v[v.len() / 2]
        };
        let (mf, mb) = (middle(&fg), middle(&bg));
        let own: Vec<usize> = bg.iter().copied().filter(|i| (s[*i] - mb).abs() <= (s[*i] - mf).abs()).collect();
        if 2 * own.len() > bg.len() {
            bg = own;
        }
    }
    let by_value = Likeness::learn(&pick(&s, &fg), &pick(&s, &bg));
    let score = |l: &Likeness, from: &[f32], at: &[usize]| -> Vec<f32> { at.iter().map(|i| l.at(from[*i])).collect() };
    let mut sep = separation(&score(&by_value, &s, &fg), &score(&by_value, &s, &bg));
    let (mut values, mut rough) = (true, None);
    if sep < CLEAR {
        // Roughness: the deviation about the local mean, over twice the scale.
        let wide = (width / 4.0).max(1.0);
        let mean = blur(&v, block.n, wide);
        let sq: Vec<f32> = v.par_iter().map(|x| x * x).collect();
        let sq = blur(&sq, block.n, wide);
        let r: Vec<f32> = sq.par_iter().zip(&mean).map(|(q, m)| (q - m * m).max(0.0).sqrt()).collect();
        let by_rough = Likeness::learn(&pick(&r, &fg), &pick(&r, &bg));
        let sep_r = separation(&score(&by_rough, &r, &fg), &score(&by_rough, &r, &bg));
        let both = |at: &[usize]| -> Vec<f32> { at.iter().map(|i| by_value.at(s[*i]) + by_rough.at(r[*i])).collect() };
        let sep_b = separation(&both(&fg), &both(&bg));
        if sep_r > sep && sep_r >= sep_b {
            (sep, values, rough) = (sep_r, false, Some(r));
        } else if sep_b > sep {
            (sep, rough) = (sep_b, Some(r));
        }
    }
    let feature = match (values, &rough) {
        (true, None) => "values",
        (true, Some(_)) => "both",
        _ => "roughness",
    };
    // The likeness of the planes k0..k1, learnt from the given samples.
    let likeness = |fg: &[usize], bg: &[usize], k0: usize, k1: usize| -> Vec<f32> {
        let a = values.then(|| Likeness::learn(&pick(&s, fg), &pick(&s, bg)));
        let b = rough.as_ref().map(|r| Likeness::learn(&pick(r, fg), &pick(r, bg)));
        (k0 * plane..k1 * plane)
            .into_par_iter()
            .map(|i| (a.as_ref().map_or(0.0, |l| l.at(s[i])) + b.as_ref().map_or(0.0, |l| l.at(rough.as_ref().unwrap()[i]))).clamp(-SURE, SURE))
            .collect()
    };
    let llr = likeness(&fg, &bg, 0, nz);
    stop(cancel)?;

    // What a surface costs: little across an edge of the image. The scale
    // of "an edge" is the mean squared step between neighbours in the box.
    let grad: Vec<f32> = (0..nz * plane)
        .into_par_iter()
        .map(|i| {
            let (x, y, z) = (i % nx, i / nx % ny, i / plane);
            let dx = if x + 1 < nx { s[i + 1] - s[i] } else { 0.0 };
            let dy = if y + 1 < ny { s[i + nx] - s[i] } else { 0.0 };
            let dz = if z + 1 < nz { s[i + plane] - s[i] } else { 0.0 };
            dx * dx + dy * dy + dz * dz
        })
        .collect();
    let beta = (grad.par_iter().map(|g| *g as f64).sum::<f64>() / grad.len() as f64).max(1e-12) as f32;
    let lambda = knob("GROW_SMOOTH", SMOOTH) * width as f32;
    let edge = knob("GROW_EDGE", EDGE);
    let g: Vec<f32> = grad.par_iter().map(|q| lambda * (edge + (1.0 - edge) * (-q / (2.0 * beta)).exp())).collect();
    let f: Vec<f32> = llr.par_iter().map(|l| -l).collect();
    let lean = knob("GROW_LEAN", LEAN);

    // Can it draw the paint again from the paint's middle? A slab about the
    // main painted plane, the middle held inside, the box's sides outside,
    // and nothing said about where the paint ends but that it is not
    // farther out than half the paint's width (room enough to show that it
    // does not stop where the paint does). What the thing looks like was
    // learnt from this very paint, so that it comes out again says little:
    // the paint is cut in two, and each half has to be drawn by what was
    // learnt from the other.
    let again = {
        let t = take[main].as_ref().unwrap();
        let deep = inside(t, nx, ny);
        let most = deep.iter().fold(0f32, |a, b| a.max(*b));
        let core: Vec<bool> = deep.iter().map(|d| *d > 0.0 && *d >= (0.25 * width as f32).min(most)).collect();
        let half = ((width / 2.0).round() as usize).clamp(3, 16);
        let (k0, k1) = (main.saturating_sub(half), (main + half + 1).min(nz));
        let n = [nx, ny, k1 - k0];
        let out = distance(t, nx, ny);
        let room = (knob("GROW_ROOM", ROOM_AGAIN) * width as f32).max(2.0);
        let mut fix = vec![0u8; plane * n[2]];
        for k in k0..k1 {
            let fx = &mut fix[(k - k0) * plane..(k - k0 + 1) * plane];
            for i in 0..plane {
                let (x, y) = (i % nx, i / nx);
                if x == 0 || y == 0 || x + 1 == nx || y + 1 == ny || out[i] > room || erase[k].as_ref().is_some_and(|e| e[i]) {
                    fx[i] = 1;
                }
            }
        }
        let seeds: Vec<usize> = (0..plane).filter(|i| core[*i]).map(|i| (main - k0) * plane + i).collect();
        seeds.iter().for_each(|i| fix[*i] = 2);
        // The two halves: either side of the paint's middle, across its
        // longer direction.
        let (mut lo, mut hi, mut sum, mut count) = ([usize::MAX; 2], [0usize; 2], [0f64; 2], 0f64);
        for i in (0..plane).filter(|i| t[*i]) {
            let q = [i % nx, i / nx];
            for a in 0..2 {
                lo[a] = lo[a].min(q[a]);
                hi[a] = hi[a].max(q[a]);
                sum[a] += q[a] as f64;
            }
            count += 1.0;
        }
        let axis = (hi[1] - lo[1] > hi[0] - lo[0]) as usize;
        let middle = sum[axis] / count;
        let side = |i: usize| -> bool { [i % plane % nx, i % plane / nx][axis] as f64 > middle };
        let (mut and, mut or) = (0usize, 0usize);
        for learnt in [false, true] {
            let part = |at: &[usize]| -> Vec<usize> { at.iter().copied().filter(|i| side(*i) == learnt).collect() };
            let (fg, bg) = (part(&fg), part(&bg));
            if fg.len() < 4 || bg.len() < 4 {
                or += (0..plane).filter(|i| t[*i] && side(*i) != learnt).count();
                continue;
            }
            let f: Vec<f32> = likeness(&fg, &bg, k0, k1).iter().map(|l| -l).collect();
            let problem = Problem { n, f: &f, g: &g[k0 * plane..k1 * plane], fix: &fix };
            let mut u = vec![0f32; fix.len()];
            let base = done.load(Ordering::Relaxed);
            let r = problem.solve(&mut u, ROUNDS, cancel, &|r| progress(base + r / LOOK / 2, total))?;
            done.fetch_add(r / LOOK / 2, Ordering::Relaxed);
            let mut on: Vec<bool> = u.iter().map(|x| *x > 0.5).collect();
            connected(&mut on, &seeds, n);
            let got = &on[(main - k0) * plane..(main - k0 + 1) * plane];
            for i in (0..plane).filter(|i| side(*i) != learnt) {
                and += (got[i] && t[i]) as usize;
                or += (got[i] || t[i]) as usize;
            }
        }
        and as f32 / or.max(1) as f32
    };
    done.store(nz + ROUNDS / LOOK, Ordering::Relaxed);
    progress(nz + ROUNDS / LOOK, total);
    if again < knob("GROW_TRUST", TRUST) {
        return Ok(Err(again));
    }

    // The real question, with all that is known held fixed.
    let mut fix = vec![0u8; nz * plane];
    let reach: Vec<Vec<f32>> = painted.iter().map(|k| distance(take[*k].as_ref().unwrap(), nx, ny)).collect();
    fix.par_chunks_mut(plane).enumerate().for_each(|(k, fx)| {
        // The nearest painted planes (both, when two are equally near).
        let away = painted.iter().map(|p| p.abs_diff(k)).min().unwrap();
        for i in 0..plane {
            let (x, y) = (i % nx, i / nx);
            let side = x == 0 || y == 0 || x + 1 == nx || y + 1 == ny;
            let near = painted.iter().zip(&reach).any(|(p, d)| p.abs_diff(k) == away && d[i] <= away as f32);
            if side || !near || erase[k].as_ref().is_some_and(|e| e[i]) {
                fx[i] = 1;
            } else if take[k].as_ref().is_some_and(|t| t[i]) {
                fx[i] = 2;
            }
        }
    });
    let seeds: Vec<usize> = (0..nz * plane).filter(|i| fix[*i] == 2).collect();
    // Leaning and widening is allowed but has to be paid for: a voxel
    // outside the painted outline counts against the thing by how far out
    // it is for its distance from the painted section, up to `SURE` where
    // it would no longer be allowed at all.
    let mut f = f;
    f.par_chunks_mut(plane).enumerate().for_each(|(k, f)| {
        let away = painted.iter().map(|p| p.abs_diff(k)).min().unwrap();
        if away > 0 {
            for i in 0..plane {
                let d = painted.iter().zip(&reach).filter(|(p, _)| p.abs_diff(k) == away).map(|(_, d)| d[i]).fold(f32::INFINITY, f32::min);
                f[i] += lean * SURE * (d / away as f32).min(1.0);
            }
        }
    });
    let problem = Problem { n: block.n, f: &f, g: &g, fix: &fix };
    let mut u = vec![0f32; fix.len()];
    let base = done.load(Ordering::Relaxed);
    let rounds = problem.solve(&mut u, ROUNDS, cancel, &|r| progress(base + r / LOOK, total))?;
    let mut on: Vec<bool> = u.par_iter().map(|x| *x > 0.5).collect();
    connected(&mut on, &seeds, block.n);

    // Where it ends, walking away from the paint in both directions.
    let areas: Vec<usize> = on.par_chunks(plane).map(area).collect();
    let painted_area = painted.iter().map(|k| areas[*k]).max().unwrap() as f64;
    let contrast = |k: usize| -> (f32, f32) {
        let m = &on[k * plane..(k + 1) * plane];
        let r = ring(m, nx, ny, near, far);
        let sl = &s[k * plane..(k + 1) * plane];
        let (mut a, mut na, mut c, mut c2, mut nc) = (0f64, 0f64, 0f64, 0f64, 0f64);
        for i in 0..plane {
            if m[i] {
                a += sl[i] as f64;
                na += 1.0;
            } else if r[i] {
                c += sl[i] as f64;
                c2 += (sl[i] as f64).powi(2);
                nc += 1.0;
            }
        }
        if na == 0.0 || nc == 0.0 {
            return (0.0, 1.0);
        }
        (((a / na) - (c / nc)).abs() as f32, ((c2 / nc - (c / nc).powi(2)).max(1e-12).sqrt()) as f32)
    };
    let contrasts: Vec<(f32, f32)> = (0..nz).into_par_iter().map(|k| if areas[k] > 0 { contrast(k) } else { (0.0, 1.0) }).collect();
    let full = painted.iter().map(|k| contrasts[*k].0).sum::<f32>() / painted.len() as f32;
    let noise = painted.iter().map(|k| contrasts[*k].1).sum::<f32>() / painted.len() as f32;
    let (lowest, highest) = (painted[0], *painted.last().unwrap());
    let largest = (lowest..=highest).map(|k| areas[k]).max().unwrap() as f64;
    let largest = areas.iter().map(|a| *a as f64).filter(|a| *a <= LEAK * painted_area).fold(largest, f64::max);
    let own = (paint.length / b as f64).ceil() as usize;
    // `walk` gives the last plane kept in a direction and why.
    let walk = |up: bool| -> (usize, End) {
        let start = if up { highest } else { lowest };
        let step = |k: usize| -> Option<usize> { if up { (k + 1 < nz).then_some(k + 1) } else { k.checked_sub(1) } };
        let (mut last, mut k) = (start, start);
        let (mut least, mut least_at) = (areas[start] as f64, start);
        loop {
            let Some(next) = step(k) else {
                // Out of the box with the thing still there.
                let far = last.abs_diff(start);
                let at_edge = block.edge[up as usize];
                if at_edge && far <= own {
                    return (last, End::Edge);
                }
                let keep = far.min(own);
                return (if up { start + keep } else { start - keep }, End::Through);
            };
            k = next;
            let a = areas[k] as f64;
            // Erased, and little left of what the section before has: the
            // user's stop.
            if erase[k].is_some() && take[k].is_none() && a <= GONE * areas[last] as f64 {
                return (last, End::Stopped);
            }
            if a == 0.0 {
                return (last, End::Ended);
            }
            if a > LEAK * painted_area {
                return (last, End::Joined);
            }
            if erase[k].is_some() {
                // What the user took away says nothing about the thing.
            } else if a < least {
                (least, least_at) = (a, k);
            } else if a >= REGROWN * least && a - least >= NECK * painted_area {
                // Cut where the growing began: the last section that is
                // still about as narrow as the narrowest.
                let mut q = last;
                while q != least_at && areas[q] as f64 > (1.0 + (REGROWN - 1.0) / 2.0) * least {
                    q = if up { q - 1 } else { q + 1 };
                }
                return (q, End::Joined);
            }
            // The contrast, steadied over three planes.
            let around = [step(k).filter(|q| areas[*q] > 0).map_or(contrasts[k].0, |q| contrasts[q].0), contrasts[k].0, contrasts[last].0];
            let mid = around[0].min(around[1]).max(around[0].max(around[1]).min(around[2]));
            if mid < 0.5 * full {
                return (last, End::Faded);
            }
            if a < TIP * largest {
                return (last, End::Ended);
            }
            last = k;
        }
    };
    let ((k0, end0), (k1, end1)) = (walk(false), walk(true));
    if end0 == End::Joined && end1 == End::Joined {
        // It runs into something else both ways: it does not know where
        // the thing ends, and its own size is the better guess.
        return Ok(Err(again));
    }
    for k in (0..k0).chain(k1 + 1..nz) {
        on[k * plane..(k + 1) * plane].fill(false);
    }
    connected(&mut on, &seeds, block.n);

    // Back to the file's voxels; the painted sections are the paint itself.
    let (ox, oy, oz) = (block.origin[0], block.origin[1], block.origin[2]);
    let mut shown: Vec<(usize, Runs)> = Vec::new();
    for k in k0..=k1 {
        let m = &on[k * plane..(k + 1) * plane];
        let mut runs = Runs::new();
        for y in 0..ny {
            let mut start = None;
            for x in 0..=nx {
                match (x < nx && m[y * nx + x], start) {
                    (true, None) => start = Some(x),
                    (false, Some(a)) => {
                        runs.push([y, a, x]);
                        start = None;
                    }
                    _ => {}
                }
            }
        }
        for dz in 0..b {
            let z = oz + k * b + dz;
            let exact = paint.layers.iter().find(|l| l.z == z && l.take.iter().any(|v| *v));
            let r: Runs = match exact {
                Some(l) => {
                    let [rx, ry, rw, rh] = paint.rect;
                    let mut out = Runs::new();
                    for y in 0..rh {
                        let mut start = None;
                        for x in 0..=rw {
                            match (x < rw && l.take[y * rw + x], start) {
                                (true, None) => start = Some(x),
                                (false, Some(a)) => {
                                    out.push([ry + y, rx + a, rx + x]);
                                    start = None;
                                }
                                _ => {}
                            }
                        }
                    }
                    out
                }
                None => runs.iter().flat_map(|[y, a, c]| (0..b).map(move |dy| [oy + y * b + dy, ox + a * b, ox + c * b])).collect(),
            };
            if !r.is_empty() {
                shown.push((z, r));
            }
        }
    }
    if shown.is_empty() {
        return Ok(Err(again));
    }

    // The wedge's stretch comes off both ends of what is used, where the
    // end is the thing's own: not below half of what was found, and never a
    // painted section.
    let (z0, z1) = (shown[0].0, shown[shown.len() - 1].0);
    let (first, last) = (paint.taken().next().unwrap().z, paint.taken().last().unwrap().z);
    let own_end = |e: End| matches!(e, End::Ended | End::Faded);
    let off = (0.5 * (e - 1.0) * paint.width).round() as usize;
    let mut off = [if own_end(end0) { off.min(first.saturating_sub(z0)) } else { 0 }, if own_end(end1) { off.min(z1.saturating_sub(last)) } else { 0 }];
    let found = z1 - z0 + 1;
    let spare = found - found.div_ceil(2).max((paint.width.round() as usize).min(found));
    let spare = if off[0] > 0 && off[1] > 0 { spare / 2 * 2 } else { spare };
    while off[0] + off[1] > spare {
        let i = (off[1] > off[0]) as usize;
        off[i] -= 1;
    }
    let mut sel = Selection::of(Engine::Cut, paint, shown, Some([z0 + off[0], z1 - off[1]]), [end0, end1], e)?;

    // The side view, from the block as it was read.
    let row = ((sel.centre[1].round() as usize).saturating_sub(oy) / b).min(ny - 1);
    let values: Vec<f32> = (0..nz).flat_map(|k| v[k * plane + row * nx..k * plane + (row + 1) * nx].iter().copied()).collect();
    sel.side = Side { y: oy + row * b + b / 2, x0: ox, z0: oz, step: b, w: nx, h: nz, gray: gray(&values) };
    sel.how = How { origin: block.origin, dims: [nx * b, ny * b, nz * b], bin: b, separation: sep, feature: feature.into(), drawn_again: again, rounds, contrast: full / noise.max(1e-6) };
    progress(total, total);
    Ok(Ok(sel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f64 {
            let (u, v) = (self.unit().max(1e-300), self.unit());
            (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
        }
    }

    /// In-place transform of a line whose length is a power of two.
    fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
        let n = re.len();
        let mut j = 0;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let a = 2.0 * std::f64::consts::PI / len as f64 * if inverse { 1.0 } else { -1.0 };
            for s in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (c, d) = ((a * k as f64).cos(), (a * k as f64).sin());
                    let (i, j) = (s + k, s + k + len / 2);
                    let (tr, ti) = (re[j] * c - im[j] * d, re[j] * d + im[j] * c);
                    (re[j], im[j]) = (re[i] - tr, im[i] - ti);
                    (re[i], im[i]) = (re[i] + tr, im[i] + ti);
                }
            }
            len <<= 1;
        }
    }

    fn fft3(re: &mut [f64], im: &mut [f64], n: [usize; 3], inverse: bool) {
        let [nx, ny, nz] = n;
        let strides = [(nx, 1), (ny, nx), (nz, nx * ny)];
        for (len, stride) in strides {
            let (mut a, mut b) = (vec![0f64; len], vec![0f64; len]);
            for start in (0..nx * ny * nz).filter(|i| i / stride % len == 0) {
                for k in 0..len {
                    (a[k], b[k]) = (re[start + k * stride], im[start + k * stride]);
                }
                fft(&mut a, &mut b, inverse);
                for k in 0..len {
                    (re[start + k * stride], im[start + k * stride]) = (a[k], b[k]);
                }
            }
        }
        if inverse {
            let s = 1.0 / (nx * ny * nz) as f64;
            re.iter_mut().chain(im.iter_mut()).for_each(|v| *v *= s);
        }
    }

    /// Phantoms are smoothed over this many voxels, as a tomogram made to
    /// look at is.
    const SOFT: f64 = 1.5;

    /// A volume of `n` voxels (powers of two) holding `shape`, as a tilt
    /// series of ±`tilt` degrees about Y would show it, with white noise.
    fn phantom(n: [usize; 3], shape: impl Fn(f64, f64, f64) -> f64, tilt: f64, noise: f64, seed: u64) -> Vec<f32> {
        let [nx, ny, nz] = n;
        let mut re: Vec<f64> = (0..nx * ny * nz).map(|i| shape((i % nx) as f64, (i / nx % ny) as f64, (i / (nx * ny)) as f64)).collect();
        let mut im = vec![0f64; re.len()];
        fft3(&mut re, &mut im, n, false);
        let t = tilt.to_radians().tan();
        let fr = |i: usize, n: usize| (if i < n / 2 { i as f64 } else { i as f64 - n as f64 }) / n as f64;
        for i in 0..re.len() {
            let (kx, kz) = (fr(i % nx, nx), fr(i / (nx * ny), nz));
            let ky = fr(i / nx % ny, ny);
            let low = (-2.0 * std::f64::consts::PI.powi(2) * SOFT * SOFT * (kx * kx + ky * ky + kz * kz)).exp();
            (re[i], im[i]) = (re[i] * low, im[i] * low);
            if kz.abs() > t * kx.abs() + 1e-12 {
                (re[i], im[i]) = (0.0, 0.0);
            }
        }
        fft3(&mut re, &mut im, n, true);
        let mut rng = Rng(seed);
        re.iter().map(|v| (*v + noise * rng.gauss()) as f32).collect()
    }

    fn ball(c: [f64; 3], r: f64) -> impl Fn(f64, f64, f64) -> f64 {
        move |x, y, z| if (x - c[0]).powi(2) + (y - c[1]).powi(2) + (z - c[2]).powi(2) <= r * r { 1.0 } else { 0.0 }
    }

    fn disc(n: [usize; 3], c: [f64; 2], r: f64) -> Vec<bool> {
        (0..n[0] * n[1]).map(|i| ((i % n[0]) as f64 - c[0]).powi(2) + ((i / n[0]) as f64 - c[1]).powi(2) <= r * r).collect()
    }

    fn painted(z: usize, take: Vec<bool>) -> Painted {
        Painted { z, take, erase: Vec::new() }
    }

    fn run(v: &[f32], n: [usize; 3], p: &[Painted], tilt: f64, deep: bool, name: &str) -> Selection {
        let read = cropped(|z| Ok(v[z * n[0] * n[1]..(z + 1) * n[0] * n[1]].to_vec()), n[0]);
        let t = Instant::now();
        let s = follow(&read, n, p, Some(tilt), deep, &|| false, &|_, _| {}).unwrap();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let (a, b) = s.heights();
        eprintln!(
            "{name}: {:?} {ms:.0} ms  z {}..{} ({a}) used {}..{} ({b}) ends {:?}  centre {:.1?} r {:.1} vox {}  box {:?}/{} sep {:.2} {} again {:.2} rounds {} contrast {:.1}",
            s.engine, s.z[0], s.z[1], s.used[0], s.used[1], s.ends, s.centre, s.radius, s.voxels, s.how.dims, s.how.bin, s.how.separation, s.how.feature, s.how.drawn_again, s.how.rounds, s.how.contrast
        );
        s
    }

    /// Area of each section shown.
    fn areas(s: &Selection) -> Vec<usize> {
        s.shown.iter().map(|(_, r)| r.iter().map(|[_, a, b]| b - a).sum()).collect()
    }

    /// With `GROW_SHOW` set: the cuts along x and z and along y and z
    /// through `at`, as text, the values in four shades, selected voxels
    /// as `@` (used) or `+` (shown only).
    fn show(s: &Selection, v: &[f32], n: [usize; 3], at: [usize; 2]) {
        if std::env::var("GROW_SHOW").is_err() {
            return;
        }
        let (o, d, m) = s.mask(false);
        let on = |x: usize, y: usize, z: usize| x >= o[0] && y >= o[1] && z >= o[2] && x < o[0] + d[0] && y < o[1] + d[1] && z < o[2] + d[2] && m[((z - o[2]) * d[1] + y - o[1]) * d[0] + x - o[0]];
        let lo = v.iter().fold(f32::INFINITY, |a, b| a.min(*b));
        let hi = v.iter().fold(f32::NEG_INFINITY, |a, b| a.max(*b));
        for z in s.z[0].saturating_sub(6)..(s.z[1] + 7).min(n[2]) {
            let mut line = format!("{z:4} ");
            for across in 0..2 {
                for i in 0..n[across] {
                    let (x, y) = if across == 0 { (i, at[1]) } else { (at[0], i) };
                    let q = ((v[(z * n[1] + y) * n[0] + x] - lo) / (hi - lo) * 3.99) as usize;
                    let c = [' ', '.', 'o', '#'][q];
                    line.push(if on(x, y, z) { if s.is_used(z) { '@' } else { '+' } } else { c });
                }
                line.push_str(" | ");
            }
            eprintln!("{line}");
        }
    }

    const N: [usize; 3] = [64, 64, 128];

    #[test]
    fn elongation_of_tilt_ranges() {
        assert!((elongation(70.0) - 1.31).abs() < 0.01);
        assert!((elongation(60.0) - 1.55).abs() < 0.01);
        assert!((elongation(50.0) - 1.89).abs() < 0.01);
        assert_eq!(elongation(90.0), 1.0);
    }

    #[test]
    fn distances() {
        let m = disc([21, 21, 1], [10.0, 10.0], 6.0);
        let d = inside(&m, 21, 21);
        assert!((d[10 * 21 + 10] - 6.0).abs() < 0.6, "{}", d[10 * 21 + 10]);
        assert_eq!(d[0], 0.0);
        let o = distance(&m, 21, 21);
        assert_eq!(o[10 * 21 + 10], 0.0);
        assert!((o[10 * 21 + 20] - 4.0).abs() < 1e-3);
    }

    fn bar(n: [usize; 3], c: [f64; 2], half: [f64; 2]) -> Vec<bool> {
        (0..n[0] * n[1]).map(|i| ((i % n[0]) as f64 - c[0]).abs() <= half[0] && ((i / n[0]) as f64 - c[1]).abs() <= half[1]).collect()
    }

    /// A ball of 16 voxels, dark or bright, clean or noisy: followed, about
    /// as tall as the file shows it, and used at about its own size.
    #[test]
    fn a_ball() {
        for (noise, sign) in [(0.0, -1.0), (0.3, -1.0), (0.6, 1.0)] {
            let v = phantom(N, |x, y, z| sign * ball([32.0, 32.0, 64.0], 8.0)(x, y, z), 60.0, noise, 1);
            let s = run(&v, N, &[painted(64, disc(N, [32.0, 32.0], 8.0))], 60.0, true, &format!("ball noise {noise}"));
            show(&s, &v, N, [32, 32]);
            assert_eq!(s.engine, Engine::Cut);
            let (shown, used) = s.heights();
            assert!((17..=25).contains(&shown), "shown {shown}");
            assert!((15..=19).contains(&used), "used {used}");
            assert!(used < shown);
            assert!(s.ends.iter().all(|e| matches!(e, End::Ended | End::Faded)), "{:?}", s.ends);
            for (c, want) in s.centre.iter().zip([32.0, 32.0, 64.0]) {
                assert!((c - want).abs() < 1.0, "centre {:?}", s.centre);
            }
            assert!((8.0..=12.5).contains(&s.radius), "radius {}", s.radius);
            // No section is much larger than the paint, and they get smaller.
            let a = areas(&s);
            assert!(a.iter().all(|x| *x < 240), "{a:?}");
            assert!(a[0] < 150 && a[a.len() - 1] < 150, "{a:?}");
            // The painted section is the paint itself.
            assert_eq!(s.section(64).iter().map(|[_, a, b]| b - a).sum::<usize>(), 197);
        }
    }

    /// Painted above its middle, where it is narrower: it may widen.
    #[test]
    fn a_ball_painted_off_its_middle() {
        let v = phantom(N, |x, y, z| -ball([32.0, 32.0, 64.0], 8.0)(x, y, z), 60.0, 0.3, 1);
        let s = run(&v, N, &[painted(68, disc(N, [32.0, 32.0], 6.9))], 60.0, true, "ball painted at +4");
        show(&s, &v, N, [32, 32]);
        eprintln!("  areas {:?}", areas(&s));
        assert_eq!(s.engine, Engine::Cut);
        assert!((s.centre[2] - 64.0).abs() < 2.0, "{:?}", s.centre);
        assert!((16..=23).contains(&s.heights().0), "{:?}", s.heights());
        assert!(areas(&s).iter().max().unwrap() > &170, "{:?}", areas(&s));
    }

    /// A rod along the tilt axis keeps its thickness; one across it is
    /// drawn out by the wedge, and what is used is cut back.
    #[test]
    fn a_rod_lying() {
        let along = |x: f64, y: f64, z: f64| if (y - 32.0).abs() <= 15.0 && (x - 32.0).powi(2) + (z - 64.0).powi(2) <= 25.0 { -1.0 } else { 0.0 };
        let v = phantom(N, along, 60.0, 0.3, 2);
        let s = run(&v, N, &[painted(64, bar(N, [32.0, 32.0], [5.0, 15.0]))], 60.0, true, "rod along y");
        show(&s, &v, N, [32, 32]);
        assert_eq!(s.engine, Engine::Cut);
        assert!((11..=16).contains(&s.heights().0) && (10..=14).contains(&s.heights().1), "{:?}", s.heights());
        assert!((s.width - 11.0).abs() <= 1.0 && s.length == 31.0, "{} {}", s.width, s.length);
        let across = |x: f64, y: f64, z: f64| if (x - 32.0).abs() <= 15.0 && (y - 32.0).powi(2) + (z - 64.0).powi(2) <= 25.0 { -1.0 } else { 0.0 };
        let v = phantom(N, across, 60.0, 0.3, 2);
        let s = run(&v, N, &[painted(64, bar(N, [32.0, 32.0], [15.0, 5.0]))], 60.0, true, "rod along x");
        show(&s, &v, N, [32, 32]);
        assert_eq!(s.engine, Engine::Cut);
        let (shown, used) = s.heights();
        assert!(shown <= 31 && used >= shown / 2 && used >= 11 && shown - used >= 4, "{shown} {used}");
    }

    /// A rod running through all sections: a piece as long as the paint is
    /// wide is kept. A section erased stops it there.
    #[test]
    fn a_rod_through() {
        let rod = |x: f64, y: f64, _z: f64| if (x - 32.0).powi(2) + (y - 32.0).powi(2) <= 36.0 { -1.0 } else { 0.0 };
        let v = phantom(N, rod, 60.0, 0.3, 3);
        let paint = painted(64, disc(N, [32.0, 32.0], 6.0));
        let s = run(&v, N, &[paint.clone()], 60.0, true, "rod through");
        assert_eq!(s.engine, Engine::Cut);
        assert_eq!(s.ends, [End::Through; 2]);
        assert_eq!((s.z, s.used), ([51, 77], [51, 77]));
        let a = areas(&s);
        assert!(a.iter().all(|x| (95..=135).contains(x)), "{a:?}");
        // Erased on section 70: nothing from there on.
        let wall = Painted { z: 70, take: Vec::new(), erase: disc(N, [32.0, 32.0], 8.0) };
        let s = run(&v, N, &[paint.clone(), wall], 60.0, true, "rod through, erased at 70");
        assert_eq!(s.ends, [End::Through, End::Stopped]);
        assert_eq!((s.z, s.used), ([51, 69], [51, 69]));
        // Erased in part: that part is out, the rest goes on.
        let part = Painted { z: 70, take: Vec::new(), erase: bar(N, [36.0, 32.0], [4.0, 10.0]) };
        let s = run(&v, N, &[paint.clone(), part], 60.0, true, "rod through, partly erased at 70");
        assert_eq!(s.ends, [End::Through; 2]);
        assert!(s.section(70).iter().all(|[_, _, b]| *b <= 32), "{:?}", s.section(70));
        assert!(!s.section(70).is_empty());
        // Painted again on section 80: it goes on from there.
        let s = run(&v, N, &[paint, painted(80, disc(N, [32.0, 32.0], 6.0))], 60.0, true, "rod through, painted again at 80");
        assert_eq!(s.ends, [End::Through; 2]);
        assert_eq!(s.z, [51, 93]);
    }

    /// A hollow shell looks inside as it does outside: the engine does not
    /// know what to follow, and the thickness is estimated (a ball).
    #[test]
    fn a_shell() {
        let shell = |x: f64, y: f64, z: f64| {
            let r = ((x - 32.0).powi(2) + (y - 32.0).powi(2) + (z - 64.0).powi(2)).sqrt();
            if (9.0..=12.0).contains(&r) { -1.0 } else { 0.0 }
        };
        for noise in [0.0, 0.3] {
            let v = phantom(N, shell, 60.0, noise, 4);
            let s = run(&v, N, &[painted(64, disc(N, [32.0, 32.0], 12.0))], 60.0, true, &format!("shell noise {noise}"));
            show(&s, &v, N, [32, 32]);
            let (shown, used) = s.heights();
            assert!((21..=33).contains(&shown) && (21..=27).contains(&used), "{shown} {used}");
            assert!((s.centre[2] - 64.0).abs() < 1.5);
            assert!(areas(&s).iter().all(|a| *a <= 441 * 5 / 4), "{:?}", areas(&s));
        }
    }

    /// A ball on a wall that looks like it: it does not run along the wall.
    #[test]
    fn a_ball_on_a_wall() {
        let both = |x: f64, y: f64, z: f64| if (40.0..=44.0).contains(&x) || ball([32.5, 32.0, 64.0], 7.5)(x, y, z) > 0.0 { -1.0 } else { 0.0 };
        let v = phantom(N, both, 60.0, 0.3, 5);
        let s = run(&v, N, &[painted(64, disc(N, [32.0, 32.0], 7.5))], 60.0, true, "ball on a wall");
        show(&s, &v, N, [32, 32]);
        let ball = 4.0 / 3.0 * std::f64::consts::PI * 7.5f64.powi(3);
        assert!((s.voxels as f64) < 2.0 * ball && (s.voxels as f64) > 0.6 * ball, "{}", s.voxels);
        assert!(s.heights().1 <= 24 && s.heights().1 >= 13, "{:?}", s.heights());
        assert!((s.centre[0] - 32.0).abs() < 4.0 && (s.centre[2] - 64.0).abs() < 2.0, "{:?}", s.centre);
        assert!(areas(&s).iter().all(|a| *a <= 177 * 3), "{:?}", areas(&s));
    }

    /// A stalk that ends in something much larger: cut where that begins.
    #[test]
    fn a_stalk_into_a_body() {
        let both = |x: f64, y: f64, z: f64| {
            let stalk = (40.0..=76.0).contains(&z) && (x - 32.0).powi(2) + (y - 32.0).powi(2) <= 25.0;
            if stalk || ball([32.0, 32.0, 90.0], 16.0)(x, y, z) > 0.0 { -1.0 } else { 0.0 }
        };
        let v = phantom(N, both, 60.0, 0.3, 11);
        let s = run(&v, N, &[painted(56, disc(N, [32.0, 32.0], 5.0))], 60.0, true, "stalk into a body");
        show(&s, &v, N, [32, 32]);
        eprintln!("  areas {:?}", areas(&s));
        assert_eq!(s.engine, Engine::Cut);
        assert_eq!(s.ends[1], End::Joined);
        assert!(s.z[1] < 80 && s.z[1] >= 58, "{:?}", s.z);
        assert!(areas(&s).iter().all(|a| *a <= 81 * 3), "{:?}", areas(&s));
    }

    /// Two balls that touch side by side: the painted one is selected, its
    /// neighbour is not. One above the other with a gap: only the painted
    /// one, until the other is painted as well.
    #[test]
    fn two_balls() {
        let side = |x: f64, y: f64, z: f64| -(ball([24.0, 32.0, 64.0], 8.0)(x, y, z) + ball([40.0, 32.0, 64.0], 8.0)(x, y, z)).min(1.0);
        let v = phantom(N, side, 60.0, 0.3, 6);
        let s = run(&v, N, &[painted(64, disc(N, [24.0, 32.0], 8.0))], 60.0, true, "two balls side by side");
        show(&s, &v, N, [24, 32]);
        eprintln!("  areas {:?} x {}..{}", areas(&s), s.origin[0], s.origin[0] + s.dims[0]);
        assert_eq!(s.engine, Engine::Cut);
        assert!((s.centre[0] - 24.0).abs() < 2.0, "{:?}", s.centre);
        let whole = 4.0 / 3.0 * std::f64::consts::PI * 512.0;
        assert!((s.voxels as f64) < 1.5 * whole, "{}", s.voxels);
        assert!(areas(&s).iter().all(|a| *a <= 197 * 5 / 4), "{:?}", areas(&s));
        let apart = |x: f64, y: f64, z: f64| -(ball([32.0, 32.0, 50.0], 8.0)(x, y, z) + ball([32.0, 32.0, 78.0], 8.0)(x, y, z)).min(1.0);
        let v = phantom(N, apart, 60.0, 0.3, 7);
        let s = run(&v, N, &[painted(50, disc(N, [32.0, 32.0], 8.0))], 60.0, true, "two balls one above the other");
        assert_eq!(s.engine, Engine::Cut);
        assert!(s.z[1] <= 62 && s.z[0] >= 38, "{:?}", s.z);
        let s = run(&v, N, &[painted(50, disc(N, [32.0, 32.0], 8.0)), painted(78, disc(N, [32.0, 32.0], 8.0))], 60.0, true, "two balls one above the other, both painted");
        assert!(s.z[0] <= 43 && s.z[1] >= 85, "{:?}", s.z);
        assert!((s.centre[2] - 64.0).abs() < 2.0);
    }

    /// Nothing there: the thickness is estimated, a disc becomes its ball.
    #[test]
    fn noise_only() {
        for seed in 0..6 {
            let v = phantom(N, |_, _, _| 0.0, 60.0, 0.5, 10 + seed);
            let s = run(&v, N, &[painted(64, disc(N, [32.0, 32.0], 8.0))], 60.0, true, "noise");
            assert_eq!(s.engine, Engine::Flat);
            assert_eq!(s.ends, [End::Estimated; 2]);
            assert!(s.how.drawn_again < TRUST);
            assert_eq!((s.z, s.used), ([56, 72], [56, 72]));
            assert!((s.radius - 8.5).abs() < 0.6, "{}", s.radius);
            let whole = 4.0 / 3.0 * std::f64::consts::PI * 512.0;
            assert!((s.voxels as f64 / whole - 1.0).abs() < 0.1, "{}", s.voxels);
        }
    }

    /// 三维选择 off: no reading but for the side view, same estimate.
    #[test]
    fn not_followed() {
        let v = phantom(N, |x, y, z| -ball([32.0, 32.0, 64.0], 8.0)(x, y, z), 60.0, 0.3, 1);
        let paint = [painted(64, bar(N, [32.0, 32.0], [12.0, 4.0]))];
        let s = run(&v, N, &paint, 60.0, false, "not followed");
        assert_eq!(s.engine, Engine::Flat);
        // A bar becomes a rod with round ends, as thick as it is wide.
        assert_eq!(s.z, [60, 68]);
        assert_eq!(s.section(64).len(), 9);
        assert!(s.section(60).len() < 9 && !s.section(60).is_empty());
        assert_eq!((s.origin, s.dims), ([20, 28, 60], [25, 9, 9]));
        // Near the first section of the file it is cut off there.
        let s = run(&v, N, &[painted(2, disc(N, [32.0, 32.0], 8.0))], 60.0, false, "at the edge");
        assert_eq!(s.z, [0, 10]);
        // Erased paint is no paint.
        let gone = [Painted { z: 64, take: disc(N, [32.0, 32.0], 8.0), erase: disc(N, [32.0, 32.0], 9.0) }];
        let read = cropped(|_| Ok(vec![0f32; 64 * 64]), 64);
        assert!(follow(&read, N, &gone, None, true, &|| false, &|_, _| {}).is_err());
    }

    /// A paint large enough for the box to be averaged down.
    #[test]
    fn a_large_ball() {
        let n = [128, 128, 256];
        let v = phantom(n, |x, y, z| -ball([64.0, 64.0, 128.0], 30.0)(x, y, z), 60.0, 0.3, 12);
        let s = run(&v, n, &[painted(128, disc(n, [64.0, 64.0], 30.0))], 60.0, true, "large ball");
        assert_eq!(s.engine, Engine::Cut);
        assert!(s.how.bin >= 2);
        let (shown, used) = s.heights();
        assert!((60..=90).contains(&shown) && (56..=70).contains(&used), "{shown} {used}");
        assert!((s.centre[2] - 128.0).abs() < 3.0, "{:?}", s.centre);
        assert!(s.side_rgba([255, 200, 0]).2.chunks(4).any(|p| p[0] == 255 && p[1] == 200 && p[2] == 0));
        assert_eq!(s.section(128).iter().map(|[_, a, b]| b - a).sum::<usize>(), disc(n, [64.0, 64.0], 30.0).iter().filter(|v| **v).count());
    }

    /// The selection survives being written and read, gives its mask as a
    /// block and its side view as a picture; a cancelled run is an error.
    #[test]
    fn kept_and_shown() {
        let v = phantom(N, |x, y, z| -ball([32.0, 32.0, 64.0], 8.0)(x, y, z), 60.0, 0.3, 1);
        let paint = [painted(64, disc(N, [32.0, 32.0], 8.0))];
        let s = run(&v, N, &paint, 60.0, true, "ball");
        let path = std::env::temp_dir().join(format!("tmv-grow-{}.sel", std::process::id()));
        s.save(&path).unwrap();
        let back = Selection::load(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(back, s);
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"engine\":\"cut\"") && text.contains("\"faded\""), "{}", &text[..200]);
        let (o, d, shown) = s.mask(false);
        let (o2, d2, used) = s.mask(true);
        assert_eq!((o, d), (s.origin, s.dims));
        assert_eq!((o2[2], d2[2]), (s.used[0], s.used[1] - s.used[0] + 1));
        assert_eq!(used.iter().filter(|v| **v).count(), s.voxels);
        assert!(shown.iter().filter(|v| **v).count() > s.voxels);
        let (w, h, rgba) = s.side_rgba([255, 200, 0]);
        assert_eq!((w, h), (s.side.w, s.side.h));
        assert_eq!(rgba.len(), w * h * 4);
        assert!(rgba.chunks(4).any(|p| p[0] == 255 && p[1] == 200 && p[2] == 0));
        if let Ok(dir) = std::env::var("GROW_SHOW") {
            let mut pgm = format!("P6 {w} {h} 255\n").into_bytes();
            pgm.extend(rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]));
            std::fs::write(std::path::Path::new(&dir).join("side.ppm"), pgm).ok();
        }
        let read = cropped(|z| Ok(v[z * 64 * 64..(z + 1) * 64 * 64].to_vec()), 64);
        let steps = AtomicUsize::new(0);
        let err = follow(&read, N, &paint, Some(60.0), true, &|| steps.fetch_add(1, Ordering::Relaxed) > 100, &|_, _| {}).unwrap_err();
        assert_eq!(err.to_string(), "已取消");
        let last = AtomicUsize::new(0);
        follow(&read, N, &paint, Some(60.0), true, &|| false, &|done, total| {
            assert!(done <= total);
            last.store(done * 1000 / total, Ordering::Relaxed);
        })
        .unwrap();
        assert_eq!(last.load(Ordering::Relaxed), 1000);
    }

    /// Time: a particle's box in under half a second, the largest box the
    /// iteration ever sees in under three.
    #[test]
    fn timings() {
        let n = [64, 64, 64];
        let v = phantom(n, |x, y, z| -ball([32.0, 32.0, 32.0], 10.0)(x, y, z), 60.0, 0.3, 8);
        let read = cropped(|z| Ok(v[z * 64 * 64..(z + 1) * 64 * 64].to_vec()), 64);
        let paint = [painted(32, disc(n, [32.0, 32.0], 10.0))];
        let t = Instant::now();
        let s = follow(&read, n, &paint, Some(60.0), true, &|| false, &|_, _| {}).unwrap();
        let small = t.elapsed().as_secs_f64();
        eprintln!("box {:?}: {:.0} ms ({} rounds)", s.how.dims, small * 1e3, s.how.rounds);
        // 128³ voxels: a ball in the middle held inside at its centre, the
        // cost of each voxel by its distance from the ball's surface.
        let n = [128usize; 3];
        let d: Vec<f32> = (0..n[0] * n[1] * n[2]).map(|i| (((i % 128) as f32 - 64.0).powi(2) + ((i / 128 % 128) as f32 - 64.0).powi(2) + ((i / 16384) as f32 - 64.0).powi(2)).sqrt()).collect();
        let f: Vec<f32> = d.iter().map(|d| (d - 40.0).clamp(-SURE, SURE)).collect();
        let g = vec![1.0f32; f.len()];
        let fix: Vec<u8> = d.iter().map(|d| if *d < 2.0 { 2 } else if *d > 63.0 { 1 } else { 0 }).collect();
        let mut u = vec![0f32; f.len()];
        let t = Instant::now();
        let rounds = Problem { n, f: &f, g: &g, fix: &fix }.solve(&mut u, ROUNDS, &|| false, &|_| {}).unwrap();
        let large = t.elapsed().as_secs_f64();
        let on = u.iter().filter(|u| **u > 0.5).count() as f64;
        let ball = 4.0 / 3.0 * std::f64::consts::PI * 40f64.powi(3);
        eprintln!("128 cubed: {:.0} ms ({rounds} rounds), {:.3} of the ball", large * 1e3, on / ball);
        assert!((on / ball - 1.0).abs() < 0.03, "{}", on / ball);
        if !cfg!(debug_assertions) || std::env::var("GROW_TIMED").is_ok() {
            assert!(small < 0.5, "{small}");
            assert!(large < 3.0, "{large}");
        }
    }
}

// ---- real data
/// On a file that is not in the repository:
///
/// `TMV_GROW_FILE=/path/tomogram.mrc TMV_GROW_PAINT=/path/paint.json
/// [TMV_GROW_OUT=/path/folder] cargo test … sta::grow::real::follows --
/// --ignored --nocapture`
///
/// The paint is JSON, one example or a list of them, each painted as discs
/// `[x, y, radius]` in voxels of the file:
///
/// `{"tilt": 60, "deep": true, "sections": [{"z": 88, "take": [[412, 300,
/// 6]], "erase": []}]}`
///
/// Prints what was selected; with `TMV_GROW_OUT` the side views are written
/// there as PNG and the selections as `.sel`. The constants of the engine
/// can be tried from the environment (`knob`): `GROW_SMOOTH`, `GROW_EDGE`,
/// `GROW_FINE`, `GROW_RIM`, `GROW_SPREAD`, `GROW_LEAN`, `GROW_ROOM`,
/// `GROW_TRUST`.
#[cfg(test)]
mod real {
    use super::*;
    use serde_json::Value;

    #[test]
    #[ignore]
    fn follows() {
        let file = std::env::var("TMV_GROW_FILE").expect("TMV_GROW_FILE = the tomogram painted on");
        let paint = std::env::var("TMV_GROW_PAINT").expect("TMV_GROW_PAINT = the paint, JSON");
        let out = std::env::var("TMV_GROW_OUT").ok();
        let img = crate::tomo::pipeline::open(&file).unwrap();
        let info = img.info();
        let dims = [info.width, info.height, info.size_z];
        let px = crate::tomo::output::pixel_a(&img);
        eprintln!("{file}: {dims:?} voxels of {px:.2} Å");
        let read = |z: usize, [x0, y0, w, h]: [usize; 4]| -> Result<Vec<f32>> {
            let (page, s) = info.locate(0, z, 0);
            let p = img.page(page)?;
            let p = &p[s.min(p.len() - 1)];
            let mut v = Vec::with_capacity(w * h);
            for y in y0..y0 + h {
                v.extend((x0..x0 + w).map(|x| p.data.get_f64(y * dims[0] + x) as f32));
            }
            Ok(v)
        };
        let json: Value = serde_json::from_slice(&std::fs::read(&paint).unwrap()).unwrap();
        let examples = match json {
            Value::Array(a) => a,
            one => vec![one],
        };
        for (i, e) in examples.iter().enumerate() {
            let discs = |v: &Value| -> Vec<bool> {
                let mut m = vec![false; dims[0] * dims[1]];
                for d in v.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                    let (cx, cy, r) = (d[0].as_f64().unwrap(), d[1].as_f64().unwrap(), d[2].as_f64().unwrap());
                    for y in ((cy - r).floor().max(0.0) as usize)..((cy + r).ceil() as usize + 1).min(dims[1]) {
                        for x in ((cx - r).floor().max(0.0) as usize)..((cx + r).ceil() as usize + 1).min(dims[0]) {
                            if (x as f64 - cx).powi(2) + (y as f64 - cy).powi(2) <= r * r {
                                m[y * dims[0] + x] = true;
                            }
                        }
                    }
                }
                m
            };
            let painted: Vec<Painted> = e["sections"].as_array().expect("sections").iter().map(|s| Painted { z: s["z"].as_u64().expect("z") as usize, take: discs(&s["take"]), erase: discs(&s["erase"]) }).collect();
            let started = std::time::Instant::now();
            let s = follow(&read, dims, &painted, e["tilt"].as_f64(), e["deep"].as_bool().unwrap_or(true), &|| false, &|_, _| {}).unwrap();
            let ms = started.elapsed().as_secs_f64() * 1e3;
            let (shown, used) = s.heights();
            eprintln!("example {}: {:?} in {ms:.0} ms", i + 1, s.engine);
            eprintln!("  shown  sections {}..{} ({shown}), ends {:?}", s.z[0], s.z[1], s.ends);
            eprintln!("  used   sections {}..{} ({used}), {} voxels, centre {:.1?}, radius {:.1} ({:.0} Å)", s.used[0], s.used[1], s.voxels, s.centre, s.radius, s.radius * px);
            eprintln!("  paint  width {:.1}, length {:.1}; height shown / width {:.2}, used / width {:.2}; wedge draws out by {:.2}", s.width, s.length, shown as f64 / s.width, used as f64 / s.width, s.elongation);
            eprintln!("  how    box {:?} at {:?} averaged by {}, {} separate {:.2}, drawn again {:.2} (needs {TRUST}), {} rounds, contrast {:.1}", s.how.dims, s.how.origin, s.how.bin, s.how.feature, s.how.separation, s.how.drawn_again, s.how.rounds, s.how.contrast);
            eprintln!("  areas  {:?}", s.shown.iter().map(|(_, r)| r.iter().map(|[_, a, b]| b - a).sum::<usize>()).collect::<Vec<_>>());
            if let Some(out) = &out {
                let dir = std::path::Path::new(out);
                std::fs::create_dir_all(dir).unwrap();
                let (w, h, rgba) = s.side_rgba([255, 200, 0]);
                image::save_buffer(dir.join(format!("example{}-side.png", i + 1)), &rgba, w as u32, h as u32, image::ColorType::Rgba8).unwrap();
                s.save(&dir.join(format!("example{}.sel", i + 1))).unwrap();
            }
        }
    }
}
