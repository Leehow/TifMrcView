use super::average::Sum;
use super::cc::{rotate, transform, wedge, Engine};
use super::pick::{blobs, gauss};
use super::Pose;
use crate::spa::fourier::{ft3, ift3};
use crate::spa::geom::{axis_angle, distance, grid, local, mul, transpose, Mat};
use crate::spa::Rng;

const N: usize = 32;

/// A handful of Gaussian balls with no symmetry.
fn phantom() -> Vec<f32> {
    let balls = [
        ([0.0, 0.0, 0.0], 3.0, 1.0),
        ([6.0, 1.0, -2.0], 2.2, 1.2),
        ([-3.0, 5.0, 3.0], 2.0, 0.9),
        ([-4.0, -5.0, 1.0], 2.5, 1.1),
        ([2.0, -3.0, 6.0], 1.8, 1.3),
        ([1.0, 6.0, -5.0], 1.6, 0.8),
    ];
    let c = (N / 2) as f64;
    let mut v = vec![0f32; N * N * N];
    for z in 0..N {
        for y in 0..N {
            for x in 0..N {
                let p = [x as f64 - c, y as f64 - c, z as f64 - c];
                v[(z * N + y) * N + x] = balls
                    .iter()
                    .map(|(q, s, a)| {
                        let d2: f64 = (0..3).map(|i| (p[i] - q[i]).powi(2)).sum();
                        a * (-d2 / (2.0 * s * s)).exp()
                    })
                    .sum::<f64>() as f32;
            }
        }
    }
    v
}

fn corr(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().map(|v| *v as f64).sum::<f64>() / n, b.iter().map(|v| *v as f64).sum::<f64>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (*x as f64 - ma, *y as f64 - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    ab / (aa * bb).sqrt()
}

/// The reference at pose (r, t), with noise, seen through a wedge.
fn particle(reference: &[f32], r: &Mat, t: [f64; 3], w: &[f32], noise: f64, rng: &mut Rng) -> Vec<f32> {
    // particle(x) = reference(Rᵀ·(x − c) + c − Rᵀ·t).
    let rt = transpose(r);
    let s = [
        -(rt[0][0] * t[0] + rt[0][1] * t[1] + rt[0][2] * t[2]),
        -(rt[1][0] * t[0] + rt[1][1] * t[1] + rt[1][2] * t[2]),
        -(rt[2][0] * t[0] + rt[2][1] * t[1] + rt[2][2] * t[2]),
    ];
    let mut v = transform(reference, N, &rt, s);
    v.iter_mut().for_each(|x| *x += (noise * rng.gauss()) as f32);
    let (mut re, mut im) = ft3(N, &v);
    for i in 0..re.len() {
        re[i] *= w[i];
        im[i] *= w[i];
    }
    ift3(N, &re, &im)
}

#[test]
fn wedge_covers_the_tilt_range() {
    let w = wedge(N, -60.0, 60.0);
    let share = w.iter().sum::<f32>() / w.len() as f32;
    assert!((share - 2.0 / 3.0).abs() < 0.05, "{share}");
    // The beam direction (kz) is missing, the plane normal to it is there.
    assert_eq!(w[(5 * N) * N], 0.0);
    assert_eq!(w[5], 1.0);
    assert_eq!(w[5 * N], 1.0);
    // An asymmetric range keeps one side only.
    let a = wedge(N, -30.0, 60.0);
    assert_eq!(a[(4 * N) * N + 4], 1.0); // +45°
    assert_eq!(a[((N - 4) * N) * N + 4], 0.0); // −45°
}

#[test]
fn rotation_turns_points_and_undoes() {
    let p = phantom();
    let r = mul(&axis_angle([0.0, 0.0, 1.0], 90.0), &axis_angle([1.0, 0.0, 0.0], 0.0));
    // A quarter turn about z moves the ball at (6, 1, −2) to (−1, 6, −2).
    let q = rotate(&p, N, &r);
    let c = N / 2;
    let at = |v: &[f32], x: isize, y: isize, z: isize| v[((c as isize + z) as usize * N + (c as isize + y) as usize) * N + (c as isize + x) as usize];
    assert!((at(&q, -1, 6, -2) - at(&p, 6, 1, -2)).abs() < 1e-4);
    let any = mul(&axis_angle([1.0, 2.0, 3.0], 37.0), &axis_angle([0.0, 1.0, 0.0], 11.0));
    let back = rotate(&rotate(&p, N, &any), N, &transpose(&any));
    assert!(corr(&p, &back) > 0.98, "{}", corr(&p, &back));
}

#[test]
fn aligns_noisy_wedged_particles_and_fills_the_wedge() {
    let truth = phantom();
    let w = vec![wedge(N, -60.0, 60.0)];
    let coarse = grid(60.0);
    let mut rng = Rng(7);
    let sd = (truth.iter().map(|v| (v * v) as f64).sum::<f64>() / truth.len() as f64).sqrt();
    // True orientations near (not on) the coarse grid, shifts off the lattice.
    let poses: Vec<(Mat, [f64; 3])> = (0..8)
        .map(|_| {
            let g = coarse[rng.below(coarse.len())];
            let axis = [rng.gauss(), rng.gauss(), rng.gauss()];
            let r = mul(&axis_angle(axis, 6.0), &g);
            (r, [4.0 * rng.unit() - 2.0, 4.0 * rng.unit() - 2.0, 4.0 * rng.unit() - 2.0])
        })
        .collect();
    let parts: Vec<Vec<f32>> = poses.iter().map(|(r, t)| particle(&truth, r, *t, &w[0], 2.0 * sd, &mut rng)).collect();
    let wedge_of = vec![0usize; parts.len()];

    // One wedged, noisy copy is a poor picture of the particle.
    let alone = transform(&parts[0], N, &poses[0].0, poses[0].1);
    let alone = corr(&alone, &truth);

    // Start from a blurred reference, as a first model would be.
    let start = gauss(&truth, [N, N, N], 2.0);
    let engine = Engine::new(N, 6.0, 1.0, 12.0);
    let prepared: Vec<_> = parts.iter().map(|p| engine.prepare(p, &w[0])).collect();
    let found = engine.global(&prepared, &w, &wedge_of, &start, &coarse, 4, &|_, _| {}).unwrap();
    let mut sum = Sum::new(N);
    for (p, f) in parts.iter().zip(&found) {
        sum.add(p, &w[0], f);
    }
    let first = sum.finish();

    // Local search around the coarse result against the new average.
    let fine = Engine::new(N, 9.0, 1.0, 12.0);
    let steps = local(8.0, 2);
    let prepared: Vec<_> = parts.iter().map(|p| fine.prepare(p, &w[0])).collect();
    let refined: Vec<Pose> = prepared.iter().zip(&found).map(|(x, f)| fine.local(x, &w[0], &first, f, &steps, 4)).collect();
    let mut sum = Sum::new(N);
    for (p, f) in parts.iter().zip(&refined) {
        sum.add(p, &w[0], f);
    }
    let second = sum.finish();

    // The average is only defined up to a common motion of all particles;
    // the start was the truth blurred, so that motion is small here.
    for ((r, t), f) in poses.iter().zip(&refined) {
        assert!(distance(r, &f.r) < 6.0, "angle {}", distance(r, &f.r));
        let d = (0..3).map(|i| (t[i] - f.t[i]).powi(2)).sum::<f64>().sqrt();
        assert!(d < 1.0, "shift {d}");
        assert!(f.score > 0.1);
    }
    let (c1, c2) = (corr(&first, &truth), corr(&second, &truth));
    assert!(c2 > 0.8 && c2 > alone + 0.2 && c2 >= c1 - 0.02, "alone {alone} first {c1} second {c2}");
    assert!(sum.coverage() > 0.97, "{}", sum.coverage());
}

#[test]
fn finds_dark_balls_of_two_sizes() {
    let dims = [64usize, 48, 40];
    let centres = [([12.0, 10.0, 12.0], 4.0), ([40.0, 30.0, 20.0], 4.0), ([52.0, 12.0, 28.0], 4.0), ([24.0, 36.0, 26.0], 7.0)];
    let mut rng = Rng(3);
    let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let inside = centres.iter().any(|(c, r): &([f64; 3], f64)| {
                    (x as f64 - c[0]).powi(2) + (y as f64 - c[1]).powi(2) + (z as f64 - c[2]).powi(2) < r * r
                });
                v[(z * dims[1] + y) * dims[0] + x] = if inside { -1.0 } else { 0.0 } + 0.5 * rng.gauss() as f32;
            }
        }
    }
    let found = blobs(&v, dims, &[8.0, 14.0], true, 6.0).unwrap();
    assert!(found.len() >= 4 && found.len() <= 6, "{}", found.len());
    for (c, r) in centres {
        let b = found
            .iter()
            .find(|b| (0..3).map(|i| (b.pos[i] as f64 - c[i]).powi(2)).sum::<f64>().sqrt() < 2.0)
            .unwrap_or_else(|| panic!("no blob at {c:?}"));
        assert_eq!(b.diameter, if r > 5.0 { 14.0 } else { 8.0 });
    }
    // Bright blobs are not reported when dark ones are asked for.
    let bright: Vec<f32> = v.iter().map(|x| -x).collect();
    assert!(blobs(&bright, dims, &[8.0, 14.0], true, 6.0).unwrap().len() <= 2);
}
