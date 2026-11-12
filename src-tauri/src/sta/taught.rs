//! "Find again": after the user has dropped a few of the places found and
//! perhaps added a few that were missed, the rest are judged again by
//! what the user did. Every place is compared with the average of the
//! ones kept and with the average of the ones dropped. A few gold beads or
//! membrane edges dropped by hand so take all their look-alikes with them,
//! and a few missed particles added bring theirs in.
//!
//! Only what is clear is changed (`Bar`). One or two dropped cubes are
//! mostly noise, and so is how much any other cube is like them: a place
//! goes only when it is like the dropped far beyond what the kept places
//! scatter by, and comes in only by how much it is like the kept, which
//! many cubes say. (Measured: with the plain difference of the two, two
//! ribosomes dropped among 448 took 144 out and brought 316 in, and the
//! average of what was left fell from 30 to 63 Å.) Dropping more of a kind
//! makes the kind clearer.
//!
//! The cubes are compared as cut and turned back by the orientation the
//! search found (the template's frame), inside the particle's ball. A cube
//! that is itself one of the kept or dropped is compared with the others
//! only, so that it does not vote for itself.

/// Collects the two averages and judges cubes against them.
pub struct Taught {
    n: usize,
    mask: Vec<f32>,
    kept: Vec<f64>,
    dropped: Vec<f64>,
    kept_n: usize,
    dropped_n: usize,
}

/// The part of a cube inside the mask, with zero mean and unit length.
fn prepared(cube: &[f32], mask: &[f32]) -> Vec<f64> {
    let (mut sum, mut weight) = (0f64, 0f64);
    for (x, m) in cube.iter().zip(mask) {
        sum += (*x * *m) as f64;
        weight += *m as f64;
    }
    let mean = sum / weight.max(1e-12);
    let mut out: Vec<f64> = cube.iter().zip(mask).map(|(x, m)| (*x as f64 - mean) * *m as f64).collect();
    let len = out.iter().map(|x| x * x).sum::<f64>().sqrt();
    if len > 1e-12 {
        out.iter_mut().for_each(|x| *x /= len);
    }
    out
}

/// The mean product of `a` with the `count` cubes summed in `sum`, leaving
/// `a` itself out when it is one of them (`own`). Unlike the cosine with
/// their average this does not depend on how many there are: an average of
/// three noisy cubes is mostly noise, and everything would look unlike it.
fn alike(a: &[f64], sum: &[f64], count: usize, own: bool) -> f64 {
    let less = if own { 1.0 } else { 0.0 };
    let others = count as f64 - less;
    if others < 1.0 {
        return 0.0;
    }
    a.iter().zip(sum).map(|(x, s)| x * (s - less * x)).sum::<f64>() / others
}

impl Taught {
    /// For `n`³ cubes with the particle within `radius` voxels of the
    /// centre `n / 2`.
    pub fn new(n: usize, radius: f64) -> Taught {
        let c = (n / 2) as f64;
        let mut mask = vec![0f32; n * n * n];
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let r = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2) + (z as f64 - c).powi(2)).sqrt();
                    // A soft edge two voxels wide.
                    mask[(z * n + y) * n + x] = ((radius + 1.0 - r) / 2.0).clamp(0.0, 1.0) as f32;
                }
            }
        }
        Taught { n, mask, kept: vec![0.0; n * n * n], dropped: vec![0.0; n * n * n], kept_n: 0, dropped_n: 0 }
    }

    /// Adds a cube to the average of the kept or of the dropped.
    pub fn learn(&mut self, cube: &[f32], kept: bool) {
        assert_eq!(cube.len(), self.n * self.n * self.n);
        let p = prepared(cube, &self.mask);
        let (sum, count) = if kept { (&mut self.kept, &mut self.kept_n) } else { (&mut self.dropped, &mut self.dropped_n) };
        sum.iter_mut().zip(&p).for_each(|(s, x)| *s += x);
        *count += 1;
    }

    /// Whether there is something to judge by: some kept and some dropped.
    pub fn ready(&self) -> bool {
        self.kept_n >= 2 && self.dropped_n >= 1
    }

    /// How much the cube is like the kept and like the dropped. `member`:
    /// the cube was given to `learn` as kept (true) or dropped (false).
    pub fn likes(&self, cube: &[f32], member: Option<bool>) -> Likes {
        let p = prepared(cube, &self.mask);
        let kept = alike(&p, &self.kept, self.kept_n, member == Some(true)) as f32;
        // With nothing dropped (or only itself) there is nothing to be like.
        let others = self.dropped_n - usize::from(member == Some(false) && self.dropped_n > 0);
        let dropped = if others == 0 { f32::NEG_INFINITY } else { alike(&p, &self.dropped, self.dropped_n, member == Some(false)) as f32 };
        Likes { kept, dropped }
    }
}

/// How much a cube is like the kept and like the dropped (`Taught::likes`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Likes {
    pub kept: f32,
    pub dropped: f32,
}

/// Kept places needed to know their scatter.
const FEWEST: usize = 8;
/// A place goes when it is like the dropped this many scatters beyond the kept.
const CLEAR: f32 = 3.0;

/// Where a place stands clear of the kept ones: what the kept places
/// themselves score says how much chance alone gives.
#[derive(Clone, Copy, Debug)]
pub struct Bar {
    /// Like the dropped beyond this is no chance (None: too few to know).
    far: Option<f32>,
    /// How much the middle kept place is like the kept.
    middle: f32,
}

impl Bar {
    /// From what the kept places score. How much they are like the dropped
    /// scatters about a middle; both are read from the lower half of them
    /// (the 5th and the 25th of a hundred, for a bell-shaped scatter), so
    /// that look-alikes of the dropped among the kept, up to half of them,
    /// do not move the bar they are measured against.
    pub fn of(kept: &[Likes]) -> Bar {
        let sorted = |f: fn(&Likes) -> f32| {
            let mut v: Vec<f32> = kept.iter().map(f).filter(|x| x.is_finite()).collect();
            v.sort_by(f32::total_cmp);
            v
        };
        let at = |v: &[f32], share: f64| v[((v.len() - 1) as f64 * share).round() as usize];
        let like_kept = sorted(|l| l.kept);
        let like_dropped = sorted(|l| l.dropped);
        let far = (like_dropped.len() >= FEWEST).then(|| {
            let (low, quarter) = (at(&like_dropped, 0.05), at(&like_dropped, 0.25));
            let scatter = (quarter - low) / 0.971;
            quarter + 0.674 * scatter + CLEAR * scatter
        });
        Bar { far, middle: if like_kept.is_empty() { f32::INFINITY } else { at(&like_kept, 0.5) } }
    }

    /// A kept place that is to go: more like the dropped than like the
    /// kept, and clear of chance.
    pub fn goes(&self, l: Likes) -> bool {
        self.far.is_some_and(|far| l.dropped > l.kept && l.dropped > far)
    }

    /// A place that was not kept and is to come in: more like the kept
    /// than the middle kept one is, and not one that would go.
    pub fn comes(&self, l: Likes) -> bool {
        l.kept > self.middle && !self.goes(l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = 20;

    fn noise(seed: &mut u64) -> f32 {
        let mut sum = 0.0;
        for _ in 0..12 {
            *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            sum += (*seed >> 40) as f32 / (1u64 << 24) as f32;
        }
        sum - 6.0
    }

    /// A noisy cube of a particle (three balls) or of a bead (one dense
    /// small ball), both dark.
    fn cube(particle: bool, seed: &mut u64) -> Vec<f32> {
        let balls: &[([f64; 3], f64, f32)] = if particle { &[([10.0, 10.0, 10.0], 4.0, -1.0), ([14.0, 9.0, 11.0], 3.0, -1.0), ([8.0, 13.0, 8.0], 2.5, -1.0)] } else { &[([10.0, 10.0, 10.0], 2.5, -4.0)] };
        let mut v = vec![0f32; N * N * N];
        for z in 0..N {
            for y in 0..N {
                for x in 0..N {
                    let p = [x as f64, y as f64, z as f64];
                    let d: f32 = balls.iter().map(|(c, r, a)| if (0..3).map(|k| (p[k] - c[k]).powi(2)).sum::<f64>().sqrt() < *r { *a } else { 0.0 }).sum();
                    v[(z * N + y) * N + x] = d + 1.5 * noise(seed);
                }
            }
        }
        v
    }

    #[test]
    fn look_alikes_of_the_dropped_go_and_the_rest_stay() {
        let mut seed = 5u64;
        // What the program kept: 40 particles and 20 beads; the user
        // dropped three of the beads.
        let particles: Vec<Vec<f32>> = (0..40).map(|_| cube(true, &mut seed)).collect();
        let beads: Vec<Vec<f32>> = (0..20).map(|_| cube(false, &mut seed)).collect();
        let mut t = Taught::new(N, 8.0);
        assert!(!t.ready());
        particles.iter().for_each(|c| t.learn(c, true));
        beads[3..].iter().for_each(|c| t.learn(c, true));
        beads[..3].iter().for_each(|c| t.learn(c, false));
        assert!(t.ready());
        let kept: Vec<Likes> = particles.iter().chain(&beads[3..]).map(|c| t.likes(c, Some(true))).collect();
        let bar = Bar::of(&kept);
        let stay = particles.iter().filter(|c| !bar.goes(t.likes(c, Some(true)))).count();
        let go = beads[3..].iter().filter(|c| bar.goes(t.likes(c, Some(true)))).count();
        assert_eq!(stay, 40, "particles that stay");
        assert!(go >= 15, "beads that go: {go} of 17");
        // Places the program had not kept are judged the same way.
        let more: Vec<Vec<f32>> = (0..40).map(|_| cube(true, &mut seed)).collect();
        let come = more.iter().filter(|c| bar.comes(t.likes(c, None))).count();
        assert!((8..=32).contains(&come), "missed particles that come in: {come} of 40");
        assert!(!bar.comes(t.likes(&cube(false, &mut seed), None)));
        // Plain noise does not come in.
        let noise: Vec<Vec<f32>> = (0..200).map(|_| (0..N * N * N).map(|_| 1.5 * noise(&mut seed)).collect()).collect();
        let come = noise.iter().filter(|c| bar.comes(t.likes(c, None))).count();
        assert!(come <= 2, "noise that comes in: {come} of 200");
    }

    /// Dropping one or two of the very kind that is kept tells nothing:
    /// nothing may go and no noise may come in for it.
    #[test]
    fn dropping_one_of_the_kept_kind_changes_nothing() {
        for dropped in [1, 2] {
            let mut seed = 11u64;
            let particles: Vec<Vec<f32>> = (0..300).map(|_| cube(true, &mut seed)).collect();
            let mut t = Taught::new(N, 8.0);
            particles[dropped..].iter().for_each(|c| t.learn(c, true));
            particles[..dropped].iter().for_each(|c| t.learn(c, false));
            let kept: Vec<Likes> = particles[dropped..].iter().map(|c| t.likes(c, Some(true))).collect();
            let bar = Bar::of(&kept);
            let go = kept.iter().filter(|l| bar.goes(**l)).count();
            assert!(go <= 2, "{dropped} dropped: {go} of 299 went");
            // The plain difference, as it was judged before, is chance here.
            let plain = kept.iter().filter(|l| l.kept <= l.dropped).count();
            assert!(plain > 20 * go.max(1), "{dropped} dropped: by the plain difference {plain} would go");
            let noise: Vec<Vec<f32>> = (0..300).map(|_| (0..N * N * N).map(|_| 1.5 * noise(&mut seed)).collect()).collect();
            let come = noise.iter().filter(|c| bar.comes(t.likes(c, None))).count();
            assert!(come <= 2, "{dropped} dropped: {come} of 300 noise cubes came in");
        }
    }
}
