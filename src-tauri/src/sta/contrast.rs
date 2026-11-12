//! What the examples have that their look-alikes have not.
//!
//! A search with the picture of an example asks where the tomogram looks
//! like it. For a small thing on a membrane the answer is every piece of
//! membrane bent the same way: most of the picture is membrane, and what
//! the user circled it for is a small share of it. Those places are no
//! failure of the search, they are what tells the thing apart: they show
//! what in the picture is common (the membrane) and how it varies.
//!
//! So, after the first search, the pictures of the places it found (turned
//! into the examples' frame) are taken as what the examples are not, and
//! the template becomes the direction that parts the two best: the
//! examples' mean less the look-alikes' mean, with the ways the look-alikes
//! vary among themselves weighed down (Fisher's linear discriminant; with
//! one positive it is the exemplar detector of Hariharan, Malik and
//! Ramanan, ECCV 2012). It is a template like any other and is searched
//! for in the same way.
//!
//! Whether this helps is not assumed but measured on the examples: each in
//! turn is left out, the template made from the others, and the one left
//! out ranked among look-alikes that were held back too. When what was
//! found first is mostly the thing itself (particles in solvent), the
//! look-alikes are the examples' own kind, the template is noise and the
//! one left out ranks anywhere: the first search stands. When the examples
//! share something the look-alikes lack, each ranks at the top.

use super::cc::ball;
use rayon::prelude::*;

/// A left-out example has to rank within this share of the held-back
/// look-alikes, every one of them. By chance alone all `k` do with a
/// probability of this to the power `k`.
pub const TOP: f64 = 0.02;
/// Fewest look-alikes: half of them make the template, half are held back.
pub const FEWEST: usize = 200;
/// The pictures are smoothed by this (voxels, Gaussian) before anything is
/// learned from them: a template made of two or three examples is mostly
/// their own noise, which is white, while what they share is not.
const SMOOTH: f64 = 0.6;
/// The ridge tried, as shares of the look-alikes' mean variance.
const RIDGES: [f64; 3] = [0.1, 1.0, 10.0];

/// The template that tells the examples from their look-alikes.
pub struct Told {
    /// `n`³, to be searched for as the examples' pictures are.
    pub template: Vec<f32>,
    /// Per example: where it ranks among held-back look-alikes when it is
    /// left out of the template (a share, 0 the top), and the same with the
    /// plain mean of the other examples as the template.
    pub ranks: Vec<(f64, f64)>,
    /// Whether every left-out example ranks within `TOP`.
    pub holds: bool,
}

/// The values of a cube, smoothed, under a soft ball, their weighted mean
/// taken off, unit length: what a normalised correlation sees of it.
fn under(cube: &[f32], mask: &[f32], support: &[usize]) -> Vec<f64> {
    let n = (cube.len() as f64).cbrt().round() as usize;
    let cube = &super::pick::gauss(cube, [n, n, n], SMOOTH)[..];
    let (mut sum, mut weight) = (0.0, 0.0);
    for &i in support {
        sum += (cube[i] * mask[i]) as f64;
        weight += mask[i] as f64;
    }
    let mean = sum / weight;
    let mut out: Vec<f64> = support.iter().map(|&i| (cube[i] as f64 - mean) * mask[i] as f64).collect();
    let norm = out.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-12);
    out.iter_mut().for_each(|x| *x /= norm);
    out
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn mean_of(rows: &[&Vec<f64>], p: usize) -> Vec<f64> {
    let mut m = vec![0.0; p];
    for r in rows {
        m.iter_mut().zip(r.iter()).for_each(|(a, b)| *a += b);
    }
    m.iter_mut().for_each(|a| *a /= rows.len().max(1) as f64);
    m
}

/// The covariance of `rows` about `mean`, p × p, row-major.
fn covariance(rows: &[&Vec<f64>], mean: &[f64]) -> Vec<f64> {
    let p = mean.len();
    let centred: Vec<Vec<f64>> = rows.iter().map(|r| r.iter().zip(mean).map(|(a, b)| a - b).collect()).collect();
    let n = rows.len().max(1) as f64;
    let mut out = vec![0.0; p * p];
    out.par_chunks_mut(p).enumerate().for_each(|(i, row)| {
        for c in &centred {
            let ci = c[i];
            if ci != 0.0 {
                for j in i..p {
                    row[j] += ci * c[j];
                }
            }
        }
        row.iter_mut().for_each(|x| *x /= n);
    });
    for i in 0..p {
        for j in 0..i {
            out[i * p + j] = out[j * p + i];
        }
    }
    out
}

/// The lower Cholesky factor of the symmetric positive matrix `a` (p × p),
/// in place; false when it is not positive.
fn cholesky(a: &mut [f64], p: usize) -> bool {
    for j in 0..p {
        let (head, tail) = a.split_at_mut((j + 1) * p);
        let row_j = &mut head[j * p..];
        let d = row_j[j] - row_j[..j].iter().map(|x| x * x).sum::<f64>();
        if d <= 0.0 || !d.is_finite() {
            return false;
        }
        let d = d.sqrt();
        row_j[j] = d;
        let row_j = &row_j[..j];
        tail.par_chunks_mut(p).for_each(|row_i| {
            let s: f64 = row_i[..j].iter().zip(row_j).map(|(x, y)| x * y).sum();
            row_i[j] = (row_i[j] - s) / d;
        });
    }
    true
}

/// x with L·Lᵀ·x = b, `l` the factor of `cholesky`.
fn solve(l: &[f64], p: usize, b: &[f64]) -> Vec<f64> {
    let mut y = b.to_vec();
    for i in 0..p {
        let s: f64 = (0..i).map(|j| l[i * p + j] * y[j]).sum();
        y[i] = (y[i] - s) / l[i * p + i];
    }
    for i in (0..p).rev() {
        let s: f64 = (i + 1..p).map(|j| l[j * p + i] * y[j]).sum();
        y[i] = (y[i] - s) / l[i * p + i];
    }
    y
}

/// The factor of `cov + ridge · (mean variance) · I`.
fn factor(cov: &[f64], p: usize, ridge: f64) -> Option<Vec<f64>> {
    let trace = (0..p).map(|i| cov[i * p + i]).sum::<f64>() / p as f64;
    let mut a = cov.to_vec();
    for i in 0..p {
        a[i * p + i] += ridge * trace.max(1e-12);
    }
    cholesky(&mut a, p).then_some(a)
}

/// The share of `held` that scores at least what `x` does with `w`.
fn rank(w: &[f64], x: &[f64], held: &[&Vec<f64>]) -> f64 {
    let s = dot(w, x);
    let above = held.iter().filter(|h| dot(w, h) >= s).count();
    above as f64 / held.len().max(1) as f64
}

/// The template that tells `ours` (the examples' pictures) from `others`
/// (the pictures of what a search with them found, best first), all `n`³
/// cubes in one frame with the thing `2 · radius` voxels across. None with
/// fewer than two examples or `FEWEST` look-alikes.
pub fn tell(ours: &[Vec<f32>], others: &[Vec<f32>], n: usize, radius: f64) -> Option<Told> {
    if ours.len() < 2 || others.len() < FEWEST {
        return None;
    }
    let mask = ball(n, radius, 2.0);
    let support: Vec<usize> = (0..mask.len()).filter(|i| mask[*i] > 0.0).collect();
    let p = support.len();
    let pos: Vec<Vec<f64>> = ours.iter().map(|c| under(c, &mask, &support)).collect();
    let neg: Vec<Vec<f64>> = others.par_iter().map(|c| under(c, &mask, &support)).collect();
    // Best first, so every other one makes two halves alike.
    let made: Vec<&Vec<f64>> = neg.iter().step_by(2).collect();
    let held: Vec<&Vec<f64>> = neg.iter().skip(1).step_by(2).collect();
    let mean = mean_of(&made, p);
    let cov = covariance(&made, &mean);
    let k = pos.len();
    let without = |b: usize| -> Vec<f64> {
        let rest: Vec<&Vec<f64>> = (0..k).filter(|e| *e != b).map(|e| &pos[e]).collect();
        mean_of(&rest, p)
    };
    // The plain template, for comparison.
    let plain: Vec<f64> = (0..k).map(|b| rank(&without(b), &pos[b], &held)).collect();
    let mut best: Option<(f64, f64, Vec<f64>)> = None;
    for ridge in RIDGES {
        let Some(l) = factor(&cov, p, ridge) else { continue };
        let ranks: Vec<f64> = (0..k)
            .map(|b| {
                let d: Vec<f64> = without(b).iter().zip(&mean).map(|(a, m)| a - m).collect();
                rank(&solve(&l, p, &d), &pos[b], &held)
            })
            .collect();
        let worst = ranks.iter().copied().fold(0.0, f64::max);
        // On a tie the stronger ridge: the plainer template.
        if best.as_ref().is_none_or(|b| worst <= b.0) {
            best = Some((worst, ridge, ranks));
        }
    }
    let (worst, ridge, ranks) = best?;
    let holds = worst <= TOP;
    // From all the look-alikes and all the examples.
    let all: Vec<&Vec<f64>> = neg.iter().collect();
    let mean = mean_of(&all, p);
    let l = factor(&covariance(&all, &mean), p, ridge)?;
    let ours_mean = mean_of(&pos.iter().collect::<Vec<_>>(), p);
    let d: Vec<f64> = ours_mean.iter().zip(&mean).map(|(a, m)| a - m).collect();
    let w = solve(&l, p, &d);
    let mut template = vec![0f32; n * n * n];
    for (i, x) in support.iter().zip(&w) {
        template[*i] = *x as f32;
    }
    Some(Told { template, ranks: ranks.into_iter().zip(plain).collect(), holds })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spa::Rng;

    const N: usize = 16;

    /// A piece of a bent sheet through the middle of the cube, as a search
    /// would have put it (its normal along z within a few degrees, its bend
    /// and its height a little different each time), and on it, when
    /// `with`, a small ring lying on one side of it: the thing the user
    /// circled.
    fn piece(rng: &mut Rng, with: bool, noise: f64) -> Vec<f32> {
        let c = (N / 2) as f64;
        let bend = 0.02 + 0.03 * rng.unit();
        let lift = rng.unit() - 0.5;
        let (tx, ty) = (0.2 * (rng.unit() - 0.5), 0.2 * (rng.unit() - 0.5));
        let (bx, by) = (0.8 * (rng.unit() - 0.5), 0.8 * (rng.unit() - 0.5));
        (0..N * N * N)
            .map(|i| {
                let (x, y, z) = ((i % N) as f64 - c, ((i / N) % N) as f64 - c, (i / (N * N)) as f64 - c);
                let sheet = z - lift - tx * x - ty * y - bend * (x * x + y * y);
                let mut v = -(-sheet * sheet / 2.0).exp();
                if with {
                    // A ring lying on the sheet.
                    let ring = ((x - bx).hypot(y - by) - 2.2).powi(2) + (z - lift - 2.5).powi(2);
                    v -= 0.5 * (-ring / 1.6).exp();
                }
                (v + noise * rng.gauss()) as f32
            })
            .collect()
    }

    /// Three sheets with a blob among a thousand without: the plain mean
    /// ranks a fourth with a blob among the bare sheets, the template that
    /// tells them puts it at the top, and says so of the examples.
    #[test]
    fn a_small_thing_on_a_sheet_is_told_from_the_bare_sheet() {
        let mut rng = Rng(3);
        let noise = 0.35;
        let ours: Vec<Vec<f32>> = (0..3).map(|_| piece(&mut rng, true, noise)).collect();
        let others: Vec<Vec<f32>> = (0..1200).map(|_| piece(&mut rng, false, noise)).collect();
        let told = tell(&ours, &others, N, 4.5).unwrap();
        eprintln!("left out: {:?}", told.ranks);
        assert!(told.holds, "{:?}", told.ranks);
        // New ones of both kinds, never seen.
        let mask = ball(N, 4.5, 2.0);
        let support: Vec<usize> = (0..mask.len()).filter(|i| mask[*i] > 0.0).collect();
        let w = under(&told.template, &mask, &support);
        let plain = {
            let rows: Vec<Vec<f64>> = ours.iter().map(|c| under(c, &mask, &support)).collect();
            mean_of(&rows.iter().collect::<Vec<_>>(), support.len())
        };
        let with: Vec<Vec<f64>> = (0..40).map(|_| under(&piece(&mut rng, true, noise), &mask, &support)).collect();
        let bare: Vec<Vec<f64>> = (0..800).map(|_| under(&piece(&mut rng, false, noise), &mask, &support)).collect();
        let top = |t: &[f64]| -> usize {
            // Of the 40 best by `t`, how many have the blob.
            let mut all: Vec<(f64, bool)> = with.iter().map(|x| (dot(t, x), true)).chain(bare.iter().map(|x| (dot(t, x), false))).collect();
            all.sort_by(|a, b| b.0.total_cmp(&a.0));
            all[..40].iter().filter(|a| a.1).count()
        };
        let (told_top, plain_top) = (top(&w), top(&plain));
        eprintln!("of the best 40: told {told_top}, plain {plain_top}");
        assert!(told_top >= 28 && told_top > plain_top + 10, "told {told_top}, plain {plain_top}");
    }

    /// When what was found is the examples' own kind there is nothing to
    /// tell, and the examples say so: left out, they rank anywhere.
    #[test]
    fn examples_among_their_own_kind_do_not_hold() {
        let mut rng = Rng(9);
        let ours: Vec<Vec<f32>> = (0..3).map(|_| piece(&mut rng, true, 0.35)).collect();
        let others: Vec<Vec<f32>> = (0..1200).map(|_| piece(&mut rng, true, 0.35)).collect();
        let told = tell(&ours, &others, N, 4.5).unwrap();
        eprintln!("left out: {:?}", told.ranks);
        assert!(!told.holds, "{:?}", told.ranks);
        // Too few of either: nothing is said.
        assert!(tell(&ours[..1], &others, N, 4.5).is_none());
        assert!(tell(&ours, &others[..50], N, 4.5).is_none());
    }
}
