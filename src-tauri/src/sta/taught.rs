//! "Find again": after the user has dropped a few of the places found and
//! perhaps added a few that were missed, the rest are judged again by
//! what the user did. Every place is compared with the average of the
//! ones kept and with the average of the ones dropped; it stays when it
//! is more like the kept. A few gold beads or membrane edges dropped by
//! hand so take all their look-alikes with them, and a few missed
//! particles added bring theirs in.
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

    /// How much more the cube is like the kept than like the dropped
    /// (positive: it stays). `member`: the cube was given to `learn` as
    /// kept (true) or dropped (false).
    pub fn like(&self, cube: &[f32], member: Option<bool>) -> f32 {
        let p = prepared(cube, &self.mask);
        // A dropped cube that is the only one has no others to compare with.
        if member == Some(false) && self.dropped_n < 2 {
            return -1.0;
        }
        let kept = alike(&p, &self.kept, self.kept_n, member == Some(true));
        let dropped = alike(&p, &self.dropped, self.dropped_n, member == Some(false));
        (kept - dropped) as f32
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
        let stay = particles.iter().filter(|c| t.like(c, Some(true)) > 0.0).count();
        let go = beads[3..].iter().filter(|c| t.like(c, Some(true)) <= 0.0).count();
        assert!(stay >= 38, "particles that stay: {stay} of 40");
        assert!(go >= 15, "beads that go: {go} of 17");
        // Places the program had not kept are judged the same way.
        assert!(t.like(&cube(true, &mut seed), None) > 0.0);
        assert!(t.like(&cube(false, &mut seed), None) < 0.0);
        // The dropped stay dropped.
        assert!(beads[..3].iter().all(|c| t.like(c, Some(false)) < 0.0));
    }
}
