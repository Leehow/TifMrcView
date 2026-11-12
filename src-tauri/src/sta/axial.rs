//! The axis of a thing read from what is around it, with no reference.
//!
//! A small thing on something larger (a structure on a membrane, a portal
//! on a vesicle) stands with its axis along the larger thing's normal there.
//! Twenty such particles are too few to find three angles each against
//! their own noisy average; read off the surroundings, two of the three are
//! known for each particle by itself, and only the turn about the axis is
//! left to search (`refine::first_model_known`).
//!
//! Two readings of a cube cut wide about the thing (dense is dark):
//! - `sheet`: the normal of the sheet the thing sits on: the direction the
//!   density changes most in (the principal axis of the structure tensor of
//!   the smoothed cube, weighted towards its middle), and how much of the
//!   change is along it (1 for a flat sheet, 0 when every direction is alike).
//! - `lean`: where the mass around the thing lies, from its middle: on a
//!   vesicle the body lies on one side of what sits on it, along its axis.
//!
//! Cubes are n³ with X fastest; a voxel index is (z·n + y)·n + x.

type P = [f64; 3];

fn norm(a: P) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// The weight of a voxel `d` voxels from the middle: a Gaussian of `sigma`.
fn weight(d2: f64, sigma: f64) -> f64 {
    (-d2 / (2.0 * sigma * sigma)).exp()
}

/// Smoothed copy of a cube (separable Gaussian, edges repeated); NaN (outside
/// the tomogram) counts as the mean.
fn smooth(cube: &[f32], n: usize, sigma: f64) -> Vec<f64> {
    let finite: Vec<f64> = cube.iter().filter(|v| v.is_finite()).map(|v| *v as f64).collect();
    let mean = if finite.is_empty() { 0.0 } else { finite.iter().sum::<f64>() / finite.len() as f64 };
    let mut a: Vec<f64> = cube.iter().map(|v| if v.is_finite() { *v as f64 } else { mean }).collect();
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f64> = (-r..=r).map(|i| weight((i * i) as f64, sigma)).collect();
    let s: f64 = k.iter().sum();
    let k: Vec<f64> = k.iter().map(|x| x / s).collect();
    let at = |i: isize| i.clamp(0, n as isize - 1) as usize;
    for axis in 0..3 {
        let mut b = vec![0.0; a.len()];
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let mut sum = 0.0;
                    for (j, w) in k.iter().enumerate() {
                        let o = j as isize - r;
                        let (xx, yy, zz) = match axis {
                            0 => (at(x as isize + o), y, z),
                            1 => (x, at(y as isize + o), z),
                            _ => (x, y, at(z as isize + o)),
                        };
                        sum += w * a[(zz * n + yy) * n + xx];
                    }
                    b[(z * n + y) * n + x] = sum;
                }
            }
        }
        a = b;
    }
    a
}

/// The normal of the sheet in the middle of `cube` and how sheet-like it is
/// there (0…1). `sigma`: how far around the middle counts, voxels.
pub fn sheet(cube: &[f32], n: usize, sigma: f64) -> (P, f64) {
    let g = smooth(cube, n, 1.5);
    let c = (n as f64 - 1.0) / 2.0;
    let mut t = [[0.0f64; 3]; 3];
    for z in 1..n - 1 {
        for y in 1..n - 1 {
            for x in 1..n - 1 {
                let i = (z * n + y) * n + x;
                let d = [0.5 * (g[i + 1] - g[i - 1]), 0.5 * (g[i + n] - g[i - n]), 0.5 * (g[i + n * n] - g[i - n * n])];
                let w = weight((x as f64 - c).powi(2) + (y as f64 - c).powi(2) + (z as f64 - c).powi(2), sigma);
                for a in 0..3 {
                    for b in 0..3 {
                        t[a][b] += w * d[a] * d[b];
                    }
                }
            }
        }
    }
    let (values, vectors) = super::sheet::axes(t);
    let top = values[2].max(1e-30);
    (vectors[2], ((values[2] - values[1]) / top).clamp(0.0, 1.0))
}

/// Where the mass around the middle of `cube` lies from it (a unit
/// direction) and how far, as a share of `sigma`. Dense is dark.
pub fn lean(cube: &[f32], n: usize, sigma: f64) -> (P, f64) {
    let g = smooth(cube, n, 1.5);
    let c = (n as f64 - 1.0) / 2.0;
    let mut sorted = g.clone();
    sorted.sort_by(f64::total_cmp);
    let level = sorted[sorted.len() / 2];
    let (mut m, mut sum) = ([0.0f64; 3], 0.0f64);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let p = [x as f64 - c, y as f64 - c, z as f64 - c];
                let mass = (level - g[(z * n + y) * n + x]).max(0.0) * weight(p[0] * p[0] + p[1] * p[1] + p[2] * p[2], sigma);
                for k in 0..3 {
                    m[k] += mass * p[k];
                }
                sum += mass;
            }
        }
    }
    if sum <= 0.0 {
        return ([0.0, 0.0, 1.0], 0.0);
    }
    let m = m.map(|x| x / sum);
    let d = norm(m);
    if d < 1e-9 {
        return ([0.0, 0.0, 1.0], 0.0);
    }
    (m.map(|x| x / d), d / sigma)
}

/// The angle between two axes taken without their sense, degrees (0…90).
pub fn apart(a: P, b: P) -> f64 {
    let c = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).abs() / (norm(a) * norm(b)).max(1e-30);
    c.clamp(0.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn unit(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn gauss(&mut self) -> f64 {
            let (a, b) = (self.unit().max(1e-12), self.unit());
            (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos()
        }
    }

    fn unit(a: P) -> P {
        let d = norm(a);
        a.map(|x| x / d)
    }

    /// A cube of side `n` with a dense shell of a ball (`centre`, `radius`,
    /// 2 voxels thick) and a dense blob at the middle, plus noise.
    fn on_a_ball(n: usize, centre: P, radius: f64, noise: f64, rng: &mut Rng) -> Vec<f32> {
        let c = (n as f64 - 1.0) / 2.0;
        let mut v = vec![0f32; n * n * n];
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let p = [x as f64 - c, y as f64 - c, z as f64 - c];
                    let r = norm([p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]]);
                    let mut d = if (r - radius).abs() <= 1.0 { -1.0 } else { 0.0 };
                    if norm(p) <= 3.0 {
                        d -= 1.0;
                    }
                    v[(z * n + y) * n + x] = (d + noise * rng.gauss()) as f32;
                }
            }
        }
        v
    }

    #[test]
    fn the_sheet_a_thing_sits_on_gives_its_axis() {
        let mut rng = Rng(0x5eed);
        let n = 32;
        let mut worst = 0.0f64;
        for _ in 0..12 {
            let axis = unit([rng.gauss(), rng.gauss(), rng.gauss()]);
            // A ball of radius 24 whose shell passes through the middle: the
            // thing sits on it, its axis along the ball's radius there.
            let v = on_a_ball(n, axis.map(|a| -24.0 * a), 24.0, 0.8, &mut rng);
            let (normal, flat) = sheet(&v, n, 6.0);
            let (towards, far) = lean(&v, n, 8.0);
            worst = worst.max(apart(normal, axis));
            assert!(apart(normal, axis) < 12.0, "sheet {:.1}°", apart(normal, axis));
            assert!(flat > 0.3, "{flat:.2}");
            // The ball lies on the inner side.
            let along = towards.iter().zip(axis).map(|(a, b)| a * b).sum::<f64>();
            assert!(along < -0.8 && far > 0.1, "lean {along:.2}, {far:.2}");
        }
        eprintln!("worst {worst:.1}°");
    }

    #[test]
    fn noise_alone_is_not_a_sheet() {
        let mut rng = Rng(7);
        let n = 32;
        let v: Vec<f32> = (0..n * n * n).map(|_| rng.gauss() as f32).collect();
        let (_, flat) = sheet(&v, n, 6.0);
        assert!(flat < 0.3, "{flat:.2}");
    }
}
