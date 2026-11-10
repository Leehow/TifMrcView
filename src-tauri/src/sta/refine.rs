//! A first model without a reference, and coarse gold-standard alignment,
//! both on the 3D engine (`cc`, `average`).
//!
//! First model. A few particles as they lie, averaged with their wedge
//! weights, are the seed (the usual start of reference-free subtomogram
//! averaging, e.g. in PEET and Dynamo). Then, round by round, every
//! particle is aligned to the current average over all orientations and
//! the average is rebuilt, while the resolution limit rises. The
//! orientation grid is turned by a random rotation each round, so the
//! rounds together sample more finely than one grid and no grid direction
//! is favoured.
//!
//! Alignment. The particles are split into two halves that never meet
//! (Scheres & Chen, Nat Methods 2012); each half is searched locally
//! around its poses against its own average, in ever smaller steps, and
//! the shell where the Fourier shell correlation of the two averages falls
//! below 0.143 sets the next resolution limit.

use anyhow::{bail, Result};
use rayon::prelude::*;

use super::average::Sum;
use super::cc::Engine;
use super::{check_cancel, Pose, IDENTITY};
use crate::spa::geom::{axis_angle, grid, local, mul, Mat};
use crate::spa::{post, Rng};

/// Subtomograms of side `n` with the wedge mask each one has.
pub struct Set<'a> {
    pub n: usize,
    pub particles: &'a [Vec<f32>],
    pub wedges: &'a [Vec<f32>],
    pub wedge_of: &'a [usize],
    /// Particle radius in voxels.
    pub radius: f64,
}

/// (stage, done, total).
pub type Progress<'a> = &'a (dyn Fn(&str, usize, usize) + Sync);
/// Called with every new average, for the viewer.
pub type Live<'a> = &'a (dyn Fn(&[f32]) + Sync);

impl Set<'_> {
    fn check(&self) -> Result<()> {
        if self.particles.len() < 4 {
            bail!("颗粒太少，无法平均");
        }
        if self.particles.len() != self.wedge_of.len() || self.wedge_of.iter().any(|w| *w >= self.wedges.len()) {
            bail!("颗粒和缺失楔数目不一致");
        }
        Ok(())
    }

    fn wedge(&self, i: usize) -> &[f32] {
        &self.wedges[self.wedge_of[i]]
    }

    /// The average of the particles `keep` picks, at their poses.
    pub fn average(&self, poses: &[Pose], keep: impl Fn(usize) -> bool + Sync) -> Sum {
        let ids: Vec<usize> = (0..self.particles.len()).filter(|i| keep(*i)).collect();
        ids.par_chunks(8)
            .map(|chunk| {
                let mut s = Sum::new(self.n);
                for &i in chunk {
                    s.add(&self.particles[i], self.wedge(i), &poses[i]);
                }
                s
            })
            .reduce(
                || Sum::new(self.n),
                |mut a, b| {
                    a.merge(&b);
                    a
                },
            )
    }

    fn engine(&self, low_pass: f64) -> Engine {
        Engine::new(self.n, low_pass, 1.0, (self.radius + 2.0).min(self.n as f64 / 2.0 - 3.0))
    }

    fn shift(&self) -> usize {
        (self.radius / 2.0).ceil().clamp(2.0, self.n as f64 / 4.0) as usize
    }
}

fn random_rotation(rng: &mut Rng) -> Mat {
    let axis = [rng.gauss(), rng.gauss(), rng.gauss()];
    axis_angle(axis, 360.0 * rng.unit())
}

/// Rounds of the first model and where its resolution limit starts and
/// ends, as a share of the particle: features down to the particle's
/// radius first, down to a quarter of it at the end.
const ROUNDS: usize = 6;
/// Rounds of `classes`: more than the first model, as the kinds have to
/// settle as well as the poses.
const CLASS_ROUNDS: usize = 10;
/// The resolution limit rises over the first of them and then stays.
const CLASS_RAMP: usize = 6;
/// Orientation step of the global search, degrees.
const STEP: f64 = 30.0;

/// A model from the particles alone, with their poses on it. The
/// resolution limit (Fourier voxels) rises from `n / (2·radius)`-ish to
/// about four times that.
pub fn first_model(set: &Set, seed: u64, progress: Progress, live: Live) -> Result<(Vec<f32>, Vec<Pose>)> {
    set.check()?;
    let m = set.particles.len();
    let mut rng = Rng(seed);
    let mut poses = vec![Pose { r: IDENTITY, t: [0.0; 3], score: 0.0 }; m];
    // The seed: a few particles as they lie.
    let few = (m / 10).clamp(2, 8);
    let mut chosen = vec![false; m];
    for _ in 0..few {
        chosen[rng.below(m)] = true;
    }
    let mut model = set.average(&poses, |i| chosen[i]).finish();
    live(&model);
    let n = set.n as f64;
    let (lo, hi) = (n / (2.0 * set.radius) * 1.5, (n / (2.0 * set.radius) * 5.0).min(n / 2.0 - 3.0));
    let rotations = grid(STEP);
    for round in 0..ROUNDS {
        check_cancel()?;
        let low_pass = (lo + (hi - lo) * round as f64 / (ROUNDS - 1) as f64).max(2.0);
        let engine = set.engine(low_pass);
        let prepared: Vec<_> = (0..m).into_par_iter().map(|i| engine.prepare(&set.particles[i], set.wedge(i))).collect();
        let turn = random_rotation(&mut rng);
        let turned: Vec<Mat> = rotations.iter().map(|r| mul(r, &turn)).collect();
        let stage = format!("起始模型 第 {}/{} 轮", round + 1, ROUNDS);
        poses = engine.global(&prepared, &model, &turned, set.shift(), &|a, b| progress(&stage, a, b))?;
        let average = set.average(&poses, |_| true).finish();
        model = centre(set, &mut poses, average);
        live(&model);
    }
    Ok((model, poses))
}

pub struct Class {
    pub model: Vec<f32>,
    /// Indices of its particles in the set, and their poses on `model`.
    pub members: Vec<usize>,
    pub poses: Vec<Pose>,
}

/// Sorts the particles into `k` kinds, each with its own model, from the
/// particles alone (multi-reference alignment: every round each particle
/// is searched against every model and goes to the one it matches best;
/// Scheres et al., J Mol Biol 348, 2005, in its hard-assignment form).
/// What was found by size alone is a mixture (the particle, gold, ice,
/// borders); one model for all would be pulled about by the densest of
/// them, while here each kind draws its own. Most populous first.
pub fn classes(set: &Set, k: usize, seed: u64, progress: Progress, live: Live) -> Result<Vec<Class>> {
    set.check()?;
    let m = set.particles.len();
    let k = k.clamp(1, m / 4);
    let mut rng = Rng(seed);
    // Start: k random groups, averaged as they lie.
    let mut class: Vec<usize> = (0..m).map(|_| rng.below(k)).collect();
    let mut poses = vec![Pose { r: IDENTITY, t: [0.0; 3], score: 0.0 }; m];
    let mut models: Vec<Vec<f32>> = (0..k).map(|c| set.average(&poses, |i| class[i] == c).finish()).collect();
    let n = set.n as f64;
    let (lo, hi) = (n / (2.0 * set.radius) * 1.5, (n / (2.0 * set.radius) * 5.0).min(n / 2.0 - 3.0));
    let rotations = grid(STEP);
    for round in 0..CLASS_ROUNDS {
        check_cancel()?;
        let low_pass = (lo + (hi - lo) * round.min(CLASS_RAMP - 1) as f64 / (CLASS_RAMP - 1) as f64).max(2.0);
        let engine = set.engine(low_pass);
        let prepared: Vec<_> = (0..m).into_par_iter().map(|i| engine.prepare(&set.particles[i], set.wedge(i))).collect();
        let turn = random_rotation(&mut rng);
        let turned: Vec<Mat> = rotations.iter().map(|r| mul(r, &turn)).collect();
        let mut best = vec![Pose { r: IDENTITY, t: [0.0; 3], score: f32::MIN }; m];
        for (c, model) in models.iter().enumerate() {
            let stage = format!("分类 第 {}/{} 轮", round + 1, CLASS_ROUNDS);
            let found = engine.global(&prepared, model, &turned, set.shift(), &|a, b| progress(&stage, c * b + a, k * b))?;
            for i in 0..m {
                if found[i].score > best[i].score {
                    best[i] = found[i];
                    class[i] = c;
                }
            }
        }
        poses = best;
        for c in 0..k {
            if (0..m).filter(|i| class[*i] == c).count() < 4 {
                // A kind that drew almost nothing starts again from the
                // particles that fit their own kind worst.
                let mut worst: Vec<usize> = (0..m).collect();
                worst.sort_by(|a, b| poses[*a].score.total_cmp(&poses[*b].score));
                for &i in worst.iter().take((m / (2 * k)).max(4)) {
                    class[i] = c;
                }
            }
            let ids: Vec<usize> = (0..m).filter(|i| class[*i] == c).collect();
            let average = set.average(&poses, |i| class[i] == c).finish();
            // `centre` moves every pose; only this kind's are its own.
            let mut own: Vec<Pose> = ids.iter().map(|i| poses[*i]).collect();
            models[c] = centre(set, &mut own, average);
            for (j, i) in ids.iter().enumerate() {
                poses[*i] = own[j];
            }
            live(&models[c]);
        }
    }
    let mut out: Vec<Class> = (0..k)
        .map(|c| {
            let members: Vec<usize> = (0..m).filter(|i| class[*i] == c).collect();
            Class { model: models[c].clone(), poses: members.iter().map(|i| poses[*i]).collect(), members }
        })
        .filter(|c| !c.members.is_empty())
        .collect();
    out.sort_by(|a, b| b.members.len().cmp(&a.members.len()));
    Ok(out)
}

/// Rounds of `first_model_known`: the start is already close.
const KNOWN_ROUNDS: usize = 4;

/// A first model for particles cut along a drawn shape, where each one's
/// Z axis is known (`axes`: along the filament or out of the surface;
/// `both` when its sense is not, as for a filament). The start is every
/// particle with its axis upright and a random turn about it; each round
/// then searches the turn about the axis and the shift only.
pub fn first_model_known(set: &Set, axes: &[[f64; 3]], both: bool, seed: u64, progress: Progress, live: Live) -> Result<(Vec<f32>, Vec<Pose>)> {
    set.check()?;
    let m = set.particles.len();
    assert_eq!(axes.len(), m);
    let mut rng = Rng(seed);
    let spin = |deg: f64| super::cc_axis(deg);
    let mut poses: Vec<Pose> =
        axes.iter().map(|a| Pose { r: mul(&super::shape::frame(*a), &spin(360.0 * rng.unit())), t: [0.0; 3], score: 0.0 }).collect();
    let mut model = set.average(&poses, |_| true).finish();
    live(&model);
    let n = set.n as f64;
    let (lo, hi) = (n / (2.0 * set.radius) * 1.5, (n / (2.0 * set.radius) * 5.0).min(n / 2.0 - 3.0));
    for round in 0..KNOWN_ROUNDS {
        check_cancel()?;
        let low_pass = (lo + (hi - lo) * round as f64 / (KNOWN_ROUNDS - 1) as f64).max(2.0);
        let engine = set.engine(low_pass);
        // A different set of turns each round, so the grid leaves no mark.
        let offset = spin(STEP * rng.unit());
        let reference = engine.reference(&model);
        let done = std::sync::atomic::AtomicUsize::new(0);
        let stage = format!("起始模型 第 {}/{} 轮", round + 1, KNOWN_ROUNDS);
        poses = (0..m)
            .into_par_iter()
            .map(|i| {
                let mut best = Pose { score: f32::MIN, ..poses[i] };
                if super::CANCEL.load(std::sync::atomic::Ordering::Relaxed) {
                    return poses[i];
                }
                let x = engine.prepare(&set.particles[i], set.wedge(i));
                for r in super::shape::turns(axes[i], STEP, both) {
                    let r = mul(&r, &offset);
                    let (score, t) = engine.score(&x, &engine.turn(&reference, &r), set.shift());
                    if score > best.score {
                        best = Pose { r, t, score };
                    }
                }
                progress(&stage, done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, m);
                best
            })
            .collect();
        check_cancel()?;
        let average = set.average(&poses, |_| true).finish();
        model = centre(set, &mut poses, average);
        live(&model);
    }
    Ok((model, poses))
}

/// Moves the model's centre of mass (of what stands out from the
/// background) to the box centre and the poses with it, so the particle
/// does not drift out of the mask over the rounds.
fn centre(set: &Set, poses: &mut [Pose], model: Vec<f32>) -> Vec<f32> {
    let n = set.n;
    let c = (n / 2) as f64;
    let mean = model.iter().map(|v| *v as f64).sum::<f64>() / model.len() as f64;
    let sd = (model.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / model.len() as f64).sqrt();
    // Density of either sign counts (cryo data are dark on light).
    let (mut w, mut g) = (0.0, [0.0; 3]);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let p = [x as f64 - c, y as f64 - c, z as f64 - c];
                if p.iter().map(|v| v * v).sum::<f64>() > (set.radius + 2.0).powi(2) {
                    continue;
                }
                let v = ((model[(z * n + y) * n + x] as f64 - mean).abs() - sd).max(0.0);
                w += v;
                for i in 0..3 {
                    g[i] += v * p[i];
                }
            }
        }
    }
    if w <= 0.0 {
        return model;
    }
    let g = [g[0] / w, g[1] / w, g[2] / w];
    if g.iter().map(|v| v * v).sum::<f64>().sqrt() < 0.25 {
        return model;
    }
    // new(p) = model(p + g); a particle then sits at t + R·g.
    for p in poses.iter_mut() {
        for i in 0..3 {
            p.t[i] += p.r[i][0] * g[0] + p.r[i][1] * g[1] + p.r[i][2] * g[2];
        }
    }
    super::cc::transform(&model, n, &IDENTITY, g)
}

pub struct Aligned {
    /// The average of both halves.
    pub map: Vec<f32>,
    pub halves: [Vec<f32>; 2],
    pub poses: Vec<Pose>,
    /// FSC of the halves per shell (0..=n/2), inside the particle sphere.
    pub fsc: Vec<f64>,
    /// The shell where it falls below 0.143 (Fourier voxels; the
    /// resolution is n·voxel/shell).
    pub shell: f64,
}

/// The shell (fractional) where the FSC first falls below 0.143 after
/// the first two.
fn cut(fsc: &[f64]) -> f64 {
    for s in 2..fsc.len() {
        if fsc[s] < 0.143 {
            let (a, b) = (fsc[s - 1], fsc[s]);
            return (s - 1) as f64 + if a > b { (a - 0.143) / (a - b) } else { 0.0 };
        }
    }
    (fsc.len() - 1) as f64
}

/// The last shell the halves still agree in: with every particle at about
/// the same defocus the curve falls through 0.143 at a zero of the contrast
/// transfer and rises again behind it, and that signal is as good for
/// aligning as the rest (the halves are independent). A shell counts only
/// with a neighbour above the line too, so a single noisy value does not.
fn extent(fsc: &[f64]) -> f64 {
    let ok = |s: usize| fsc.get(s).is_some_and(|v| *v >= 0.143);
    (2..fsc.len()).rev().find(|&s| ok(s) && (ok(s - 1) || ok(s + 1))).map_or(1.0, |s| s as f64)
}

/// Local search ranges of the rounds, degrees; each is covered in two
/// steps per side.
const RANGES: [f64; 3] = [12.0, 6.0, 3.0];

/// Gold-standard local alignment from the poses of a first model. Half A
/// is the even particles, half B the odd ones; callers that oversample
/// positions put neighbours in the same half by ordering them so.
pub fn align(set: &Set, model: &[f32], start: &[Pose], progress: Progress, live: Live) -> Result<Aligned> {
    set.check()?;
    let m = set.particles.len();
    let mut poses = start.to_vec();
    let mut refs = [model.to_vec(), model.to_vec()];
    // The shared start is trusted only at low resolution.
    let mut low_pass = (set.n as f64 / (2.0 * set.radius) * 3.0).clamp(2.0, set.n as f64 / 2.0 - 3.0);
    let mut out = None;
    for (round, range) in RANGES.iter().enumerate() {
        check_cancel()?;
        let engine = set.engine(low_pass);
        let steps = local(*range, 2);
        let against = [engine.reference(&refs[0]), engine.reference(&refs[1])];
        let done = std::sync::atomic::AtomicUsize::new(0);
        let stage = format!("对齐 第 {}/{} 轮", round + 1, RANGES.len());
        poses = (0..m)
            .into_par_iter()
            .map(|i| {
                if super::CANCEL.load(std::sync::atomic::Ordering::Relaxed) {
                    return poses[i];
                }
                let x = engine.prepare(&set.particles[i], set.wedge(i));
                let p = engine.local(&x, &against[i % 2], &poses[i], &steps, set.shift());
                progress(&stage, done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, m);
                p
            })
            .collect();
        check_cancel()?;
        let (a, b) = (set.average(&poses, |i| i % 2 == 0), set.average(&poses, |i| i % 2 == 1));
        let halves = [a.finish(), b.finish()];
        let fsc = post::fsc(&halves[0], &halves[1], set.n, set.radius + 2.0);
        let shell = cut(&fsc);
        low_pass = (shell.max(extent(&fsc)) + 1.0).clamp(2.0, set.n as f64 / 2.0 - 3.0);
        let mut both = a;
        both.merge(&b);
        let map = both.finish();
        live(&map);
        refs = [halves[0].clone(), halves[1].clone()];
        out = Some(Aligned { map, halves, poses: poses.clone(), fsc, shell });
    }
    Ok(out.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signal_behind_a_zero_counts_but_a_lone_value_does_not() {
        let dip = [1.0, 0.99, 0.9, 0.6, 0.147, 0.107, 0.19, 0.3, 0.37, 0.17, 0.06, 0.01, 0.2, 0.0];
        assert_eq!(cut(&dip).floor(), 4.0);
        assert_eq!(extent(&dip), 9.0);
        assert_eq!(extent(&[1.0, 0.9, 0.5, 0.1, 0.0, 0.0]), 2.0);
    }
}
