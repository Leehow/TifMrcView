use super::average::Sum;
use super::cc::{rotate, transform, wedge, Engine};
use super::pick::{blobs, gauss};
use super::Pose;
use crate::spa::fourier::{ft3, ift3};
use crate::spa::geom::{axis_angle, distance, grid, local, mul, transpose, Mat};
use crate::spa::Rng;

const N: usize = 32;

/// A handful of Gaussian balls with no symmetry.
pub(super) fn phantom() -> Vec<f32> {
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
                        // Spread out and sharp, so a few degrees show.
                        let d2: f64 = (0..3).map(|i| (p[i] - 1.4 * q[i]).powi(2)).sum();
                        a * (-d2 / (2.0 * 0.49 * s * s)).exp()
                    })
                    .sum::<f64>() as f32;
            }
        }
    }
    v
}

pub(super) fn corr(a: &[f32], b: &[f32]) -> f64 {
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
pub(super) fn particle(reference: &[f32], r: &Mat, t: [f64; 3], w: &[f32], noise: f64, rng: &mut Rng) -> Vec<f32> {
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
            let r = mul(&axis_angle(axis, 10.0), &g);
            (r, [4.0 * rng.unit() - 2.0, 4.0 * rng.unit() - 2.0, 4.0 * rng.unit() - 2.0])
        })
        .collect();
    let parts: Vec<Vec<f32>> = poses.iter().map(|(r, t)| particle(&truth, r, *t, &w[0], 1.5 * sd, &mut rng)).collect();

    // One wedged, noisy copy is a poor picture of the particle.
    let alone = transform(&parts[0], N, &poses[0].0, poses[0].1);
    let alone = corr(&alone, &truth);

    // Start from a blurred reference, as a first model would be.
    let start = gauss(&truth, [N, N, N], 2.0);
    let engine = Engine::new(N, 6.0, 1.0, 14.0);
    let prepared: Vec<_> = parts.iter().map(|p| engine.prepare(p, &w[0])).collect();
    let found = engine.global(&prepared, &start, &coarse, 4, &|_, _| {}).unwrap();
    // Two halves, as in `refine::align`: a particle searched against an
    // average it is part of would find its own noise where it already is.
    let mut halves = [Sum::new(N), Sum::new(N)];
    for (i, (p, f)) in parts.iter().zip(&found).enumerate() {
        halves[i % 2].add(p, &w[0], f);
    }
    let mut first = Sum::new(N);
    first.merge(&halves[0]);
    first.merge(&halves[1]);
    let first = first.finish();
    let halves = [halves[0].finish(), halves[1].finish()];

    // Local search around the coarse result, each half against the other's average.
    let fine = Engine::new(N, 9.0, 1.0, 14.0);
    let steps = local(8.0, 2);
    let prepared: Vec<_> = parts.iter().map(|p| fine.prepare(p, &w[0])).collect();
    let against = [fine.reference(&halves[0]), fine.reference(&halves[1])];
    let refined: Vec<Pose> =
        prepared.iter().zip(&found).enumerate().map(|(i, (x, f))| fine.local(x, &against[1 - i % 2], f, &steps, 4)).collect();
    let mut sum = Sum::new(N);
    for (p, f) in parts.iter().zip(&refined) {
        sum.add(p, &w[0], f);
    }
    let second = sum.finish();

    // The average is only defined up to a common motion of all particles;
    // the start was the truth blurred, so that motion is small here.
    let mut worst = 0f64;
    for ((r, t), f) in poses.iter().zip(&refined) {
        worst = worst.max(distance(r, &f.r));
        let d = (0..3).map(|i| (t[i] - f.t[i]).powi(2)).sum::<f64>().sqrt();
        assert!(d < 1.0, "shift {d}");
        assert!(f.score > 0.1);
    }
    let mean = |v: &[Pose]| poses.iter().zip(v).map(|((r, _), f)| distance(r, &f.r)).sum::<f64>() / v.len() as f64;
    assert!(worst < 9.0 && mean(&refined) < 0.7 * mean(&found), "worst {worst} coarse {} refined {}", mean(&found), mean(&refined));
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

#[test]
fn local_search_finds_the_orientation_and_shift() {
    let truth = phantom();
    let w = wedge(N, -60.0, 60.0);
    let mut rng = Rng(7);
    let e = Engine::new(N, 8.0, 1.0, 14.0);
    for k in 0..2 {
        let g = mul(&axis_angle([1.0, 2.0, 0.5], 40.0 * k as f64), &axis_angle([0.0, 0.0, 1.0], 25.0));
        let r = mul(&axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 6.0), &g);
        let t = [1.3, -0.7, 2.0];
        let p = particle(&truth, &r, t, &w, 0.0, &mut rng);
        let x = e.prepare(&p, &w);
        let reference = e.reference(&truth);
        let (right, at) = e.score(&x, &e.turn(&reference, &r), 4);
        let (wrong, _) = e.score(&x, &e.turn(&reference, &transpose(&r)), 4);
        assert!(right > 0.97 && wrong < right - 0.2, "{right} {wrong}");
        assert!((0..3).all(|i| (at[i] - t[i]).abs() < 0.15), "{at:?}");
        let from = Pose { r: g, t: [0.0; 3], score: 0.0 };
        let l = e.local(&x, &reference, &from, &local(8.0, 2), 4);
        assert!(distance(&r, &l.r) < 3.5 && l.score > 0.97, "{} {}", distance(&r, &l.r), l.score);
    }
}

/// Heavy (minutes unoptimised): run with
/// `CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test --features pro --lib sta::tests::first_model -- --ignored --nocapture`.
#[test]
#[ignore]
fn first_model_and_alignment_from_particles_alone() {
    use super::refine::{align, first_model, Set};
    let truth = phantom();
    let wedges = vec![wedge(N, -60.0, 60.0), wedge(N, -54.0, 57.0)];
    let mut rng = Rng(11);
    let sd = (truth.iter().map(|v| (v * v) as f64).sum::<f64>() / truth.len() as f64).sqrt();
    let count = 48;
    let wedge_of: Vec<usize> = (0..count).map(|i| i % 2).collect();
    let parts: Vec<Vec<f32>> = (0..count)
        .map(|i| {
            let r = axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 360.0 * rng.unit());
            let t = [3.0 * rng.unit() - 1.5, 3.0 * rng.unit() - 1.5, 3.0 * rng.unit() - 1.5];
            particle(&truth, &r, t, &wedges[wedge_of[i]], 1.5 * sd, &mut rng)
        })
        .collect();
    let set = Set { n: N, particles: &parts, wedges: &wedges, wedge_of: &wedge_of, radius: 12.0 };
    let (model, poses) = first_model(&set, 5, &|_, _, _| {}, &|_| {}).unwrap();
    let out = align(&set, &model, &poses, &|_, _, _| {}, &|_| {}).unwrap();
    // The result may be turned and moved as a whole: bring it onto the truth.
    let onto = |map: &[f32]| {
        let e = Engine::new(N, 8.0, 1.0, 14.0);
        let x = e.prepare(map, &vec![1.0; N * N * N]);
        let all = vec![vec![1f32; N * N * N]];
        let g = e.global(&[x], &truth, &grid(20.0), 5, &|_, _| {}).unwrap()[0];
        let x = e.prepare(map, &all[0]);
        e.local(&x, &e.reference(&truth), &g, &local(8.0, 2), 5).score
    };
    let (first, last) = (onto(&model), onto(&out.map));
    eprintln!("first model {first:.3}, aligned {last:.3}, FSC shell {:.1}, fsc {:?}", out.shell, out.fsc);
    assert!(first > 0.5 && last > 0.75 && last >= first - 0.02, "first {first} aligned {last}");
    assert!(out.shell > 5.0, "{}", out.shell);
}

/// A made-up tomogram with `per_side`² × 2 dark copies of the phantom (each
/// with a soft body) in noise of `noise` times the phantom's spread, written
/// to a new temporary directory: the directory and the sources.
fn particle_tomogram(per_side: usize, noise: f64) -> (std::path::PathBuf, Vec<super::project::Source>) {
    use super::project::Source;
    let (dims, v, _, _) = planted_tomogram(per_side, noise);
    let dir = std::env::temp_dir().join(format!("tmv-sta-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tomo = dir.join("tomo.mrc").to_string_lossy().into_owned();
    // 17.5 Å voxels, so nothing is binned.
    crate::tomo::output::write_mrc(&tomo, &v, dims, 17.5, false).unwrap();
    (dir, vec![Source { path: tomo, tilts: [-90.0, 90.0], look: None, series: None }])
}

/// The tomogram of `particle_tomogram`: its size, values (dark copies, z-major),
/// the voxel each copy's centre is planted at and the copy's body.
fn planted_tomogram(per_side: usize, noise: f64) -> ([usize; 3], Vec<f32>, Vec<[usize; 3]>, Vec<f32>) {
    let truth = phantom();
    // A particle has a body: a large soft ball with the phantom's balls as detail.
    let c = (N / 2) as f64;
    let body: Vec<f32> = truth
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let (x, y, z) = ((i % N) as f64 - c, (i / N % N) as f64 - c, (i / N / N) as f64 - c);
            0.4 * v + (-(x * x + y * y + z * z) / (2.0 * 5.0 * 5.0)).exp() as f32
        })
        .collect();
    let dims = [38 * per_side + 8, 38 * per_side + 8, 96];
    let mut rng = Rng(21);
    let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
    let sd = (truth.iter().map(|x| (x * x) as f64).sum::<f64>() / truth.len() as f64).sqrt();
    let mut places = vec![];
    for k in 0..2 {
        for j in 0..per_side {
            for i in 0..per_side {
                let r = axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 360.0 * rng.unit());
                let p = rotate(&body, N, &r);
                let o = [22 + 38 * i + rng.below(5), 22 + 38 * j + rng.below(5), 24 + 46 * k + rng.below(5)];
                places.push(o);
                for z in 0..N {
                    for y in 0..N {
                        for x in 0..N {
                            // Dark on light, as cryo data are.
                            v[((o[2] + z - N / 2) * dims[1] + o[1] + y - N / 2) * dims[0] + o[0] + x - N / 2] -= p[(z * N + y) * N + x];
                        }
                    }
                }
            }
        }
    }
    v.iter_mut().for_each(|x| *x += (noise * sd * rng.gauss()) as f32);
    (dims, v, places, body)
}

/// Heavy: a made-up tomogram with 32 dark copies of the phantom, found,
/// cut out and averaged by `project::run` (same command as above with
/// `sta::tests::project`).
#[test]
#[ignore]
fn project_finds_and_averages_particles_in_a_tomogram() {
    use super::project::run;
    let (dir, sources) = particle_tomogram(4, 1.0);
    let out = dir.join("out").to_string_lossy().into_owned();
    let found = run(&sources, None, &out, &|_, _, _| {}, &|_| {}).unwrap();
    eprintln!("kinds {:?}", found.kinds);
    assert!(found.map.is_none() && !found.kinds.is_empty());
    // The user's choice: the card that shows the whole particle (here
    // the kind that stands out most; the phantom's balls are another).
    let size = found.kinds.iter().max_by(|a, b| a.contrast.total_cmp(&b.contrast)).unwrap().diameter_a;
    let done = run(&sources, Some(size), &out, &|_, _, _| {}, &|_| {}).unwrap();
    eprintln!("{} of {} particles (distinct {:.2}), {:?} Å, fsc {:?}", done.particles, done.candidates, done.distinct.unwrap_or(0.0), done.resolution_a, done.fsc);
    assert!(done.particles >= 24 && done.particles <= 40, "{}", done.particles);
    assert!(done.resolution_a.unwrap() < 4.0 * done.voxel_a, "{:?}", done.resolution_a);
    assert!(std::path::Path::new(done.map.as_ref().unwrap()).is_file());
    let table = std::fs::read_to_string(done.table.unwrap()).unwrap();
    assert_eq!(table.lines().count(), done.particles + 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Heavy: copies of the phantom on a sphere, their Z axes pointing out;
/// the sphere is "drawn", sampled twice as densely as the copies sit, and
/// `project::run_drawn` has to end with about one particle per copy
/// (same command as above with `sta::tests::drawn`).
#[test]
#[ignore]
fn drawn_sphere_gives_one_particle_per_copy() {
    use super::project::{run_drawn, Drawn, Source};
    use super::shape::{ellipsoid, frame};
    let truth = phantom();
    let c = (N / 2) as f64;
    let body: Vec<f32> = truth
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let (x, y, z) = ((i % N) as f64 - c, (i / N % N) as f64 - c, (i / N / N) as f64 - c);
            0.4 * v + (-(x * x + y * y + z * z) / (2.0 * 5.0 * 5.0)).exp() as f32
        })
        .collect();
    let dims = [144usize, 144, 144];
    let centre = [72.0, 72.0, 72.0];
    let mut rng = Rng(5);
    let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
    let copies = ellipsoid(centre, [44.0; 3], 26.0);
    let mut places = vec![];
    for s in &copies {
        let r = mul(&frame(s.axis), &axis_angle([0.0, 0.0, 1.0], 360.0 * rng.unit()));
        let p = rotate(&body, N, &r);
        let o = s.pos.map(|x| x.round() as usize);
        places.push(o);
        for z in 0..N {
            for y in 0..N {
                for x in 0..N {
                    v[((o[2] + z - N / 2) * dims[1] + o[1] + y - N / 2) * dims[0] + o[0] + x - N / 2] -= p[(z * N + y) * N + x];
                }
            }
        }
    }
    let sd = (truth.iter().map(|x| (x * x) as f64).sum::<f64>() / truth.len() as f64).sqrt();
    v.iter_mut().for_each(|x| *x += (1.0 * sd * rng.gauss()) as f32);
    let dir = std::env::temp_dir().join(format!("tmv-sta-drawn-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tomo = dir.join("tomo.mrc").to_string_lossy().into_owned();
    crate::tomo::output::write_mrc(&tomo, &v, dims, 17.5, false).unwrap();
    let sources = vec![Source { path: tomo, tilts: [-60.0, 60.0], look: None, series: None }];
    // What the sphere tool gives: a slightly wrong sphere, sampled densely.
    let size_a = 227.5;
    let drawn: Vec<Drawn> =
        ellipsoid([73.0, 71.0, 72.5], [45.0; 3], 0.5 * size_a / 17.5).into_iter().map(|s| Drawn { source: 0, pos: s.pos, axis: s.axis }).collect();
    let out = dir.join("out").to_string_lossy().into_owned();
    let done = run_drawn(&sources, &drawn, false, size_a, &out, &|_, _, _| {}, &|_| {}).unwrap();
    eprintln!("{} copies, {} seeds, {} particles (distinct {:.2}), {:?} Å, fsc {:?}", copies.len(), done.candidates, done.particles, done.distinct.unwrap_or(0.0), done.resolution_a, done.fsc);
    let table = std::fs::read_to_string(done.table.unwrap()).unwrap();
    let at: Vec<[f64; 3]> = table
        .lines()
        .skip(1)
        .map(|l| {
            let f: Vec<f64> = l.split(',').skip(1).take(3).map(|x| x.parse().unwrap()).collect();
            [f[0], f[1], f[2]]
        })
        .collect();
    let near = |o: &[usize; 3]| at.iter().map(|a| (0..3).map(|i| (a[i] - o[i] as f64).powi(2)).sum::<f64>().sqrt()).fold(f64::MAX, f64::min);
    let mut d: Vec<f64> = places.iter().map(|o| near(o)).collect();
    d.sort_by(f64::total_cmp);
    eprintln!("nearest: min {:.1} quartile {:.1} median {:.1} upper {:.1} max {:.1}", d[0], d[d.len() / 4], d[d.len() / 2], d[d.len() * 3 / 4], d[d.len() - 1]);
    let radial: Vec<f64> = at.iter().map(|a| (0..3).map(|i| (a[i] - 72.0).powi(2)).sum::<f64>().sqrt()).collect();
    eprintln!("radius of particles: mean {:.1}", radial.iter().sum::<f64>() / radial.len() as f64);
    let hit = places.iter().filter(|o| near(o) < 3.0).count();
    eprintln!("{hit} of {} copies have a particle within 3 voxels", places.len());
    assert!(hit * 10 >= places.len() * 8, "{hit}");
    assert!(done.particles as f64 <= 1.5 * copies.len() as f64, "{}", done.particles);
    // Few particles: the resolution only has to be a sensible one.
    assert!(done.resolution_a.unwrap() < 6.0 * done.voxel_a, "{:?}", done.resolution_a);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A membrane (the plane y = 40, its normal along the tilt axis) with a
/// hexagonal pattern 6 voxels behind it, rows 4 voxels apart, under noise;
/// the membrane is "drawn" 2 voxels off. `project::run_drawn` has to find
/// the pattern by itself and end with its places: wherever one is, each of
/// the pattern's three waves has the same phase (also the one whose rows
/// the missing wedge hides from the alignment).
#[test]
fn a_pattern_on_a_drawn_membrane_is_found_and_its_places_agree() {
    pattern_on_a_membrane(208.0, 24, 7, "lattice");
}

/// The same when the particle size given is twice too large to show the
/// rows (two voxels a row at its sampling): the pattern is found at the
/// finer sampling and everything goes on there.
#[test]
fn a_pattern_finer_than_the_size_asked_for_is_still_found() {
    pattern_on_a_membrane(416.0, 30, 13, "lattice-coarse");
}

/// The membrane is drawn from `from` voxels off the sides with a place
/// every `step` voxels, for particles of `size_a`.
fn pattern_on_a_membrane(size_a: f64, from: usize, step: usize, name: &str) {
    use super::project::{run_drawn, Drawn, Source};
    let dims = [128usize, 64, 128];
    let waves: Vec<[f64; 2]> = [10f64, 70.0, 130.0].iter().map(|d| [d.to_radians().cos(), d.to_radians().sin()]).collect();
    let k = std::f64::consts::TAU / 4.0;
    let mut rng = Rng(11);
    let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let pattern: f64 = waves.iter().map(|w| (k * (x as f64 * w[0] + z as f64 * w[1])).cos()).sum();
                let value = -(-0.5 * ((y as f64 - 40.0) / 1.2).powi(2)).exp() - 0.25 * pattern * (-0.5 * ((y as f64 - 34.0) / 1.5).powi(2)).exp();
                v[(z * dims[1] + y) * dims[0] + x] = (value + 0.5 * rng.gauss()) as f32;
            }
        }
    }
    let dir = std::env::temp_dir().join(format!("tmv-sta-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tomo = dir.join("tomo.mrc").to_string_lossy().into_owned();
    crate::tomo::output::write_mrc(&tomo, &v, dims, 16.0, false).unwrap();
    let sources = vec![Source { path: tomo, tilts: [-60.0, 60.0], look: None, series: None }];
    let mut drawn = vec![];
    for z in (from..=128 - from).step_by(step) {
        for x in (from..=128 - from).step_by(step) {
            drawn.push(Drawn { source: 0, pos: [x as f64, 42.0, z as f64], axis: [0.0, 1.0, 0.0] });
        }
    }
    let out = dir.join("out").to_string_lossy().into_owned();
    let done = run_drawn(&sources, &drawn, false, size_a, &out, &|_, _, _| {}, &|_| {}).unwrap();
    assert!((done.voxel_a - 16.0).abs() < 1e-6 || done.voxel_a < 16.0, "voxel {}", done.voxel_a);
    assert!(done.pattern.is_some_and(|p| (p.rows_a - 64.0).abs() < 6.0), "{:?}", done.pattern);
    let table = std::fs::read_to_string(done.table.unwrap()).unwrap();
    let at: Vec<[f64; 3]> = table
        .lines()
        .skip(1)
        .map(|l| {
            let f: Vec<f64> = l.split(',').skip(1).take(3).map(|x| x.parse().unwrap()).collect();
            [f[0], f[1], f[2]]
        })
        .collect();
    assert!(at.len() >= 100, "{} places", at.len());
    // On the pattern's layer, not on the membrane.
    let depth = at.iter().map(|p| p[1]).sum::<f64>() / at.len() as f64;
    assert!((depth - 34.0).abs() < 1.5, "places at y = {depth:.1}");
    for w in &waves {
        let (c, s) = at.iter().fold((0.0, 0.0), |(c, s), p| {
            let phase = k * (p[0] * w[0] + p[2] * w[1]);
            (c + phase.cos(), s + phase.sin())
        });
        let together = c.hypot(s) / at.len() as f64;
        assert!(together > 0.7, "the places agree to {together:.2} on a wave");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A saved average of 98 particles, a third of them with a partner bound:
/// `project::kinds_of` finds the two kinds from the files alone, and
/// `project::keep_kinds` averages the kind that is kept.
#[test]
fn the_kinds_of_a_saved_average_are_found_and_one_is_kept() {
    use super::project::{keep_kinds, kinds_of, Source};
    use crate::spa::geom::angles;
    let plain = phantom();
    let c = (N / 2) as f64;
    let bound: Vec<f32> = plain
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let (x, y, z) = ((i % N) as f64 - c, (i / N % N) as f64 - c, (i / N / N) as f64 - c);
            v + 1.2 * (-((x - 5.0).powi(2) + (y + 4.0).powi(2) + (z + 3.0).powi(2)) / (2.0 * 2.5 * 2.5)).exp() as f32
        })
        .collect();
    let dims = [176usize, 176, 72];
    let mut rng = Rng(29);
    let mut v: Vec<f32> = (0..dims[0] * dims[1] * dims[2]).map(|_| rng.gauss() as f32).collect();
    let dir = std::env::temp_dir().join(format!("tmv-sta-kinds-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tomo = dir.join("tomo.mrc").to_string_lossy().into_owned();
    let mut csv = String::from("tomogram,x,y,z,rot,tilt,psi,score,half,blob,strength,darkest\n");
    let mut with = vec![];
    for k in 0..98 {
        let o = [28 + 20 * (k % 7), 28 + 20 * (k / 7 % 7), 20 + 32 * (k / 49)];
        let has = k % 3 == 0;
        let r = axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 360.0 * rng.unit());
        let p = rotate(if has { &bound } else { &plain }, N, &r);
        for z in 0..N {
            for y in 0..N {
                for x in 0..N {
                    v[((o[2] + z - N / 2) * dims[1] + o[1] + y - N / 2) * dims[0] + o[0] + x - N / 2] += p[(z * N + y) * N + x];
                }
            }
        }
        let [rot, tilt, psi] = angles(&r);
        csv += &format!("{tomo},{},{},{},{rot:.2},{tilt:.2},{psi:.2},0.5,{},0,0,0\n", o[0], o[1], o[2], k % 2 + 1);
        with.push(has);
    }
    crate::tomo::output::write_mrc(&tomo, &v, dims, 17.5, false).unwrap();
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("particles.csv"), csv).unwrap();
    std::fs::write(out.join("sta.json"), serde_json::json!({ "diameterA": 350.0, "voxelA": 17.5, "resolutionA": 70.0, "fsc": vec![0.5; N / 2 + 1] }).to_string()).unwrap();
    let sources = vec![Source { path: tomo, tilts: [-90.0, 90.0], look: None, series: None }];
    let out = out.to_string_lossy().into_owned();
    let found = kinds_of(&sources, &out, &|_, _, _| {}).unwrap();
    let kinds = found["kinds"].as_array().unwrap();
    assert_eq!(kinds.len(), 2, "{found}");
    let rows = |k: usize| -> Vec<usize> { kinds[k]["rows"].as_array().unwrap().iter().map(|r| r.as_u64().unwrap() as usize).collect() };
    let right = rows(0).iter().filter(|i| !with[**i]).count() + rows(1).iter().filter(|i| with[**i]).count();
    assert!(right >= 92, "{right} of 98 with their own kind");
    assert!(std::path::Path::new(kinds[1]["map"].as_str().unwrap()).exists());
    let kept = keep_kinds(&sources, &out, &[1], &|_, _, _| {}, &|_| {}).unwrap();
    assert!(kept.particles >= 50 && kept.particles <= rows(0).len(), "{}", kept.particles);
    assert!(std::path::Path::new(&kept.map.unwrap()).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// `n` scores drawn from the distribution with these quantiles (probability,
/// score), linear in between.
fn sample(rng: &mut Rng, n: usize, q: &[(f64, f64)]) -> Vec<f32> {
    (0..n)
        .map(|_| {
            let u = rng.unit();
            let k = (1..q.len()).find(|k| u <= q[*k].0).unwrap_or(q.len() - 1);
            let (a, b) = (q[k - 1], q[k]);
            (a.1 + (b.1 - a.1) * (u - a.0) / (b.0 - a.0)) as f32
        })
        .collect()
}

fn as_peaks(scores: &[f32]) -> Vec<super::search::Peak> {
    scores.iter().map(|s| super::search::Peak { pos: [0; 3], score: *s, turn: 0 }).collect()
}

#[test]
fn threshold_splits_the_two_groups_of_peaks() {
    use super::search::cutoff;
    let mut rng = Rng(5);
    // Real data (EMPIAR-10045): 254 peaks on particles, 1426 on the rest.
    let high = [(0.0, 0.52), (0.05, 0.61), (0.25, 0.72), (0.5, 0.75), (0.75, 0.79), (0.95, 0.84), (1.0, 0.88)];
    let low = [(0.0, 0.05), (0.05, 0.19), (0.25, 0.35), (0.5, 0.46), (0.75, 0.59), (0.95, 0.78), (1.0, 0.85)];
    for seed in 0..5 {
        rng.0 = 5 + seed;
        let mut s = sample(&mut rng, 254, &high);
        s.extend(sample(&mut rng, 1426, &low));
        let t = cutoff(&as_peaks(&s)).expect("two groups");
        assert!((0.55..=0.68).contains(&t), "{t}");
    }
    // One group: bell curve and a skewed one (extreme values).
    for seed in 0..5 {
        rng.0 = 50 + seed;
        let bell: Vec<f32> = (0..800).map(|_| (0.4 + 0.12 * rng.gauss()) as f32).collect();
        assert!(cutoff(&as_peaks(&bell)).is_none());
        let gumbel: Vec<f32> = (0..800).map(|_| (0.3 - 0.08 * (-rng.unit().max(1e-9).ln()).ln()) as f32).collect();
        assert!(cutoff(&as_peaks(&gumbel)).is_none());
    }
    // 32 particles, ~200 others.
    let mut s: Vec<f32> = (0..32).map(|_| 0.67 + 0.09 * rng.unit() as f32).collect();
    s.extend((0..200).map(|_| 0.15 + 0.18 * rng.unit() as f32));
    let t = cutoff(&as_peaks(&s)).expect("two groups");
    assert!((0.33..=0.67).contains(&t), "{t}");
}

/// A tomogram packed with the particle: every peak is one, so the scores
/// are one group and nothing parts them, yet they stand far out of what
/// the rest of the search scores. A search of noise alone has peaks too,
/// and none of them stands out.
#[test]
fn peaks_that_are_all_particles_are_told_from_peaks_that_are_all_noise() {
    use super::search::{above_noise, cutoff, sure, Found};
    let mut rng = Rng(21);
    // What a voxel scores where there is nothing: the best of many
    // orientations (an extreme-value curve), 64³ voxels.
    let dims = [64usize; 3];
    let mut noise = |rng: &mut Rng| (0.10 - 0.012 * (-rng.unit().max(1e-12).ln()).ln()) as f32;
    let score: Vec<f32> = (0..dims[0] * dims[1] * dims[2]).map(|_| noise(&mut rng)).collect();
    let empty = Found { score: score.clone(), turn: vec![0; score.len()], dims };
    let floor = above_noise(&empty).expect("a floor");
    // One voxel of 262144 is expected to pass it by chance.
    let passing = score.iter().filter(|s| **s >= floor).count();
    assert!(passing <= 3, "{passing} voxels of noise above {floor}");
    // Its peaks: the best voxel of each 8³ block.
    let mut tops: Vec<f32> = vec![];
    for z in (0..64).step_by(8) {
        for y in (0..64).step_by(8) {
            for x in (0..64).step_by(8) {
                tops.push((0..512).map(|k| score[((z + k / 64) * 64 + y + k / 8 % 8) * 64 + x + k % 8]).fold(f32::MIN, f32::max));
            }
        }
    }
    assert!(sure(&as_peaks(&tops), &[Some(floor)]).is_none());
    // Packed: a particle in every block, scoring 0.33 ± 0.03.
    let particles: Vec<f32> = (0..512).map(|_| (0.33 + 0.03 * rng.gauss()) as f32).collect();
    let mut packed = score.clone();
    for (k, p) in particles.iter().enumerate() {
        packed[((k / 64 * 8 + 4) * 64 + k / 8 % 8 * 8 + 4) * 64 + k % 8 * 8 + 4] = *p;
    }
    let packed = Found { score: packed, turn: vec![0; score.len()], dims };
    let floor = above_noise(&packed).expect("a floor");
    assert!(cutoff(&as_peaks(&particles)).is_none());
    let limit = sure(&as_peaks(&particles), &[Some(floor)]).expect("they stand out");
    assert!(particles.iter().filter(|p| **p >= limit).count() >= 500, "limit {limit}");
    // A search that has no floor says nothing.
    assert!(sure(&as_peaks(&particles), &[Some(floor), None]).is_none());
}

#[test]
fn exclusion_keeps_off_borders_and_gold() {
    use super::search::allowed;
    let dims = [60usize, 56, 20];
    let mut rng = Rng(8);
    let mut v: Vec<f32> = (0..dims[0] * dims[1] * dims[2]).map(|_| rng.gauss() as f32).collect();
    // A small ball far below the noise.
    let c = [30.0, 28.0, 10.0];
    let mut gold = vec![];
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                if (x as f64 - c[0]).powi(2) + (y as f64 - c[1]).powi(2) + (z as f64 - c[2]).powi(2) <= 4.0 {
                    v[(z * dims[1] + y) * dims[0] + x] = -50.0;
                    gold.push([x as f64, y as f64, z as f64]);
                }
            }
        }
    }
    let r = 5.0;
    let ok = allowed(&v, dims, r);
    let (mut off, mut on) = (0, 0);
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let p = [x as f64, y as f64, z as f64];
                let near = gold.iter().any(|g| (0..3).map(|a| (g[a] - p[a]).powi(2)).sum::<f64>() <= 4.0 * r * r);
                let edge = p[0] < r || p[0] > 59.0 - r || p[1] < r || p[1] > 55.0 - r;
                assert_eq!(ok[(z * dims[1] + y) * dims[0] + x], !(near || edge), "{p:?}");
                off += (near || edge) as usize;
                on += !(near || edge) as usize;
            }
        }
    }
    assert!(off > 0 && on > 0);
    // Z faces are open.
    assert!(ok[(0 * dims[1] + 20) * dims[0] + 10] && ok[(19 * dims[1] + 20) * dims[0] + 10]);
    // Without gold only the borders go.
    let plain: Vec<f32> = v.iter().map(|x| x.max(-5.0)).collect();
    let ok = allowed(&plain, dims, r);
    assert!(ok[(10 * dims[1] + 28) * dims[0] + 30]);
}

/// Heavy: the whole-tomogram search finds 32 copies at random orientations
/// with their template, and the threshold read off the chance peaks keeps
/// them and little else (same command as above with `sta::tests::search`).
#[test]
#[ignore]
fn search_finds_the_copies_and_sets_its_threshold() {
    use super::search::{cutoff, peaks, search};
    let truth = phantom();
    let c = (N / 2) as f64;
    let body: Vec<f32> = truth
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let (x, y, z) = ((i % N) as f64 - c, (i / N % N) as f64 - c, (i / N / N) as f64 - c);
            0.4 * v + (-(x * x + y * y + z * z) / (2.0 * 5.0 * 5.0)).exp() as f32
        })
        .collect();
    let dims = [150usize, 170, 90];
    let mut rng = Rng(33);
    let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
    let mut places = vec![];
    for k in 0..2 {
        for j in 0..4 {
            for i in 0..4 {
                let r = axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 360.0 * rng.unit());
                let p = rotate(&body, N, &r);
                let o = [18 + 36 * i + rng.below(5), 20 + 40 * j + rng.below(5), 20 + 44 * k + rng.below(5)];
                places.push(o);
                for z in 0..N {
                    for y in 0..N {
                        for x in 0..N {
                            v[((o[2] + z - N / 2) * dims[1] + o[1] + y - N / 2) * dims[0] + o[0] + x - N / 2] -= p[(z * N + y) * N + x];
                        }
                    }
                }
            }
        }
    }
    let sd = (truth.iter().map(|x| (x * x) as f64).sum::<f64>() / truth.len() as f64).sqrt();
    v.iter_mut().for_each(|x| *x += (2.0 * sd * rng.gauss()) as f32);
    // Dark on light, like the data.
    let template: Vec<f32> = body.iter().map(|x| -x).collect();
    let turns = grid(30.0);
    let started = std::time::Instant::now();
    let found = search(&v, dims, &template, N, 10.0, [-60.0, 60.0], &turns, None, &|_, _| {}).unwrap();
    // Particles do not overlap: peaks are a diameter apart.
    let real = peaks(&found, 20.0, usize::MAX);
    eprintln!("{} turns, {:.0} s; {} peaks, best {:.3}, 32nd {:.3}, 40th {:.3}", turns.len(), started.elapsed().as_secs_f64(), real.len(), real[0].score, real[31].score, real[39].score);
    let limit = cutoff(&real).unwrap();
    let kept: Vec<_> = real.iter().filter(|p| p.score >= limit).collect();
    let near = |o: &[usize; 3]| kept.iter().map(|p| (0..3).map(|i| (p.pos[i] as f64 - o[i] as f64).powi(2)).sum::<f64>().sqrt()).fold(f64::MAX, f64::min);
    let hit = places.iter().filter(|o| near(o) < 2.5).count();
    eprintln!("threshold {limit:.3}: {} kept, {hit} of 32 copies among them", kept.len());
    assert!(hit >= 30, "{hit}");
    assert!(kept.len() <= 36, "{}", kept.len());
}

/// Heavy: a mixture of the phantom and of a dense ball with a streak (what
/// a gold marker looks like) is sorted into its two kinds
/// (same command as above with `sta::tests::classes`).
#[test]
#[ignore]
fn classes_separate_two_kinds_of_particle() {
    use super::refine::{classes, Set};
    let truth = phantom();
    let c = (N / 2) as f64;
    let gold: Vec<f32> = (0..N * N * N)
        .map(|i| {
            let (x, y, z) = ((i % N) as f64 - c, (i / N % N) as f64 - c, (i / N / N) as f64 - c);
            let ball = (-(x * x + y * y + z * z) / (2.0 * 2.5 * 2.5)).exp();
            let streak = 0.25 * (-(x * x + y * y) / (2.0 * 2.0 * 2.0)).exp() * (-(z * z) / (2.0 * 8.0 * 8.0)).exp();
            (1.5 * ball + streak) as f32
        })
        .collect();
    let w = wedge(N, -60.0, 60.0);
    let mut rng = Rng(11);
    let mut particles = vec![];
    let mut kind = vec![];
    for i in 0..90 {
        // A third are gold; gold streaks all run along the beam.
        let is_gold = i % 3 == 0;
        let r = axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 360.0 * rng.unit());
        let t = [1.5 * rng.gauss(), 1.5 * rng.gauss(), 1.5 * rng.gauss()];
        let id = axis_angle([0.0, 0.0, 1.0], 0.0);
        particles.push(particle(if is_gold { &gold } else { &truth }, if is_gold { &id } else { &r }, t, &w, 2.0, &mut rng));
        kind.push(is_gold);
    }
    let wedges = vec![w];
    let wedge_of = vec![0; particles.len()];
    let set = Set { n: N, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius: 10.0 };
    let found = classes(&set, 2, 3, &|_, _, _| {}, &|_| {}).unwrap();
    let mut right = 0;
    for c in &found {
        let golds = c.members.iter().filter(|i| kind[**i]).count();
        eprintln!("class of {}: {} gold, {} particles", c.members.len(), golds, c.members.len() - golds);
        right += golds.max(c.members.len() - golds);
    }
    assert!(found.len() == 2 && right >= 81, "{right} of 90 with their own kind");
}

/// The progress `classes` reports within one round: (done, total) with total
/// the same throughout, done never going back and ending at total, once per
/// rotation of each kind, so the counter moves at every step.
#[test]
fn classes_count_their_progress_steadily() {
    use super::refine::{classes, Set};
    use std::sync::Mutex;
    let n = 16;
    let w = wedge(n, -60.0, 60.0);
    let mut rng = Rng(5);
    let balls: Vec<f32> = (0..n * n * n)
        .map(|i| {
            let (x, y, z) = ((i % n) as f64 - 8.0, (i / n % n) as f64 - 8.0, (i / n / n) as f64 - 8.0);
            (-((x - 2.0).powi(2) + y * y + z * z) / 8.0).exp() as f32 + (rng.gauss() * 0.05) as f32
        })
        .collect();
    let particles: Vec<Vec<f32>> = (0..8).map(|_| balls.iter().map(|v| v + (rng.gauss() * 0.05) as f32).collect()).collect();
    let wedges = vec![w];
    let wedge_of = vec![0; particles.len()];
    let set = Set { n, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius: 4.0 };
    let seen: Mutex<Vec<(String, usize, usize)>> = Mutex::new(vec![]);
    classes(&set, 2, 1, &|s, a, b| seen.lock().unwrap().push((s.to_string(), a, b)), &|_| {}).unwrap();
    let seen = seen.into_inner().unwrap();
    let rounds: Vec<String> = seen.iter().fold(vec![], |mut v, e| {
        if v.last() != Some(&e.0) {
            v.push(e.0.clone());
        }
        v
    });
    assert_eq!(rounds.len(), 10, "{rounds:?}");
    for round in &rounds {
        let ticks: Vec<(usize, usize)> = seen.iter().filter(|e| &e.0 == round).map(|e| (e.1, e.2)).collect();
        let total = ticks[0].1;
        let each = grid(30.0).len();
        assert_eq!(total, 2 * each, "{round}");
        assert!(ticks.iter().all(|t| t.1 == total), "{round}: the total changes");
        assert!(ticks.windows(2).all(|p| p[1].0 == p[0].0 + 1), "{round}: done does not step by one");
        assert_eq!((ticks[0].0, ticks.last().unwrap().0), (1, total), "{round}");
    }
}

/// Heavy: `project::cards` on the same tomogram; the 210 Å kind has to give
/// cards that hold the particles (same command as above with
/// `sta::tests::cards`).
#[test]
#[ignore]
fn cards_hold_the_particles_of_a_kind() {
    use super::project::cards;
    // 72 copies, as a kind needs 40 blobs to make cards, and noisy enough
    // that the copies are not taken for gold markers.
    let (dir, sources) = particle_tomogram(6, 3.0);
    let out = dir.join("cards").to_string_lossy().into_owned();
    let found = cards(&sources, &out, &|_, _, _| {}, &|_| {}).unwrap();
    for c in &found {
        eprintln!("{c:?}");
    }
    let members: usize = found.iter().filter(|c| c.diameter_a == 210.0).map(|c| c.members).sum();
    assert!(members >= 24, "{members}");
    for c in &found {
        assert!(std::path::Path::new(&c.path).is_file());
        // A 1024-byte header and 32³ floats.
        assert_eq!(std::fs::metadata(&c.path).unwrap().len(), 1024 + 4 * 32 * 32 * 32);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn look_positions_map_onto_the_raw_tomogram_with_the_binning_half_voxels() {
    use super::project::{to_raw, LookSource};
    let look = |scale, o| LookSource { path: String::new(), scale, offset: [o; 3] };
    let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-12);
    // Unbinned on both, scale 2, offset 0.5: look voxel p spans raw 2p and 2p + 1.
    assert!(close(to_raw([3.0; 3], 1, 1, &look(2.0, 0.5)), [6.5; 3]));
    // Raw binned by 2: its voxel q is centred on unbinned 2q + 0.5, so 6.5 is q = 3.
    assert!(close(to_raw([3.0; 3], 1, 2, &look(2.0, 0.5)), [3.0; 3]));
    // Look binned by 2 (voxel p centred on 2p + 0.5), raw by 4: they coincide.
    assert!(close(to_raw([5.0, 0.0, 7.5], 2, 4, &look(2.0, 0.5)), [5.0, 0.0, 7.5]));
    // Both binned by 2, scale 2 and a shift of 3.5: voxel 10 is unbinned 20.5,
    // raw 2 × 20.5 + 3.5 = 44.5, which is binned voxel (44.5 − 0.5) / 2 = 22.
    assert!(close(to_raw([10.0; 3], 2, 2, &look(2.0, 3.5)), [22.0; 3]));
    // Scale 1: only the binnings' centres differ.
    assert!(close(to_raw([4.0; 3], 4, 1, &look(1.0, 0.0)), [4.0 * 4.0 + 1.5; 3]));
    assert!(close(to_raw([4.0; 3], 1, 4, &look(1.0, 0.0)), [(4.0 - 1.5) / 4.0; 3]));
}

#[test]
fn the_look_exclusion_mask_comes_from_the_raw_one_and_keeps_the_look_borders() {
    use super::project::{allowed_on_look, LookSource};
    let dims = [20, 20, 6];
    let at = |x: usize, y: usize, z: usize| (z * dims[1] + y) * dims[0] + x;
    let mut raw = vec![true; dims[0] * dims[1] * dims[2]];
    raw[at(10, 10, 3)] = false;
    let same = LookSource { path: String::new(), scale: 1.0, offset: [0.0; 3] };
    let m = allowed_on_look(&raw, dims, 1, 1, dims, 1, 1, &same, 2.0);
    assert!(!m[at(10, 10, 3)] && m[at(10, 11, 3)] && m[at(9, 10, 3)]);
    // Within 2 voxels of the X and Y faces: out, whatever the raw mask says; Z faces are not borders.
    assert!(!m[at(1, 10, 3)] && m[at(2, 10, 3)] && !m[at(10, 18, 3)] && m[at(10, 17, 3)] && m[at(10, 10, 0)] && m[at(10, 10, 5)]);
    // A look grid shifted by 5 raw voxels: its voxel (5, 10, 3) is the raw one (10, 15, 8): outside in Z.
    let shifted = LookSource { path: String::new(), scale: 1.0, offset: [5.0, 5.0, 5.0] };
    let m = allowed_on_look(&raw, dims, 1, 1, dims, 1, 1, &shifted, 2.0);
    assert!(!m[at(5, 10, 3)] && !m[at(5, 10, 2)] && m[at(5, 10, 0)]);
    // The raw hole seen from there: raw (10, 10, 3) is look (5, 5, -2): nothing to see; but raw (10, 10, 5) is look (5, 5, 0).
    let mut raw2 = raw.clone();
    raw2[at(10, 10, 5)] = false;
    let m = allowed_on_look(&raw2, dims, 1, 1, dims, 1, 1, &shifted, 0.0);
    assert!(!m[at(5, 5, 0)] && m[at(5, 6, 0)] && m[at(6, 5, 0)]);
    // Halved grids (2-fold voxels on both): voxel p is binned voxel 2p + 1/2 on both, so the identity map.
    let m = allowed_on_look(&raw, dims, 2, 1, dims, 2, 1, &same, 0.0);
    assert!(!m[at(10, 10, 3)] && m[at(11, 10, 3)]);
}

/// The tomogram `v` (`dims`) made `f` times as fine by repeating each voxel, the
/// first `skip` voxels of every axis dropped.
fn finer(v: &[f32], dims: [usize; 3], skip: usize, f: usize) -> (Vec<f32>, [usize; 3]) {
    let big = dims.map(|d| f * d - skip);
    let mut out = vec![0f32; big[0] * big[1] * big[2]];
    for z in 0..big[2] {
        for y in 0..big[1] {
            for x in 0..big[0] {
                let s = [(x + skip) / f, (y + skip) / f, (z + skip) / f];
                out[(z * big[1] + y) * big[0] + x] = v[(s[2] * dims[1] + s[1]) * dims[0] + s[0]];
            }
        }
    }
    (out, big)
}

/// `v` binned by 2: the mean of every 2×2×2 voxels.
fn bin2(v: &[f32], dims: [usize; 3]) -> (Vec<f32>, [usize; 3]) {
    let d = dims.map(|n| n / 2);
    let mut out = vec![0f32; d[0] * d[1] * d[2]];
    for z in 0..d[2] {
        for y in 0..d[1] {
            for x in 0..d[0] {
                let mut s = 0.0;
                for k in 0..8 {
                    s += v[((2 * z + k / 4) * dims[1] + 2 * y + k / 2 % 2) * dims[0] + 2 * x + k % 2];
                }
                out[(z * d[1] + y) * d[0] + x] = s / 8.0;
            }
        }
    }
    (out, d)
}

/// `v` binned by 2 and smoothed (Gaussian, σ 0.7 voxel).
fn binned_and_smoothed(v: &[f32], dims: [usize; 3]) -> (Vec<f32>, [usize; 3]) {
    let (mut out, d) = bin2(v, dims);
    let kernel: Vec<f32> = (-2..=2).map(|i: i32| (-(i * i) as f32 / (2.0 * 0.49)).exp()).collect();
    let norm: f32 = kernel.iter().sum();
    for axis in 0..3 {
        let stride = [1, d[0], d[0] * d[1]][axis];
        let src = out.clone();
        for i in 0..out.len() {
            let p = [i % d[0], i / d[0] % d[1], i / d[0] / d[1]][axis] as isize;
            let mut s = 0.0;
            for (j, w) in kernel.iter().enumerate() {
                let q = (p + j as isize - 2).clamp(0, d[axis] as isize - 1);
                s += w * src[(i as isize + (q - p) * stride as isize) as usize];
            }
            out[i] = s / norm;
        }
    }
    (out, d)
}

/// The raw tomogram, its look version, what was planted, the body of a copy
/// and the centres (raw voxels) of the gold clusters put into both.
type LookFixture = (Vec<f32>, [usize; 3], Vec<f32>, [usize; 3], Vec<[usize; 3]>, Vec<f32>, Vec<[f64; 3]>);

fn look_fixture(per_side: usize, noise: f64, f: usize) -> LookFixture {
    let (base_dims, mut base, places, body) = planted_tomogram(per_side, noise);
    // Two gold markers: 4 base voxels (8 raw) across, very dark, each 8 base
    // voxels beside a copy, one in each layer (so those two copies lie well
    // within the exclusion distance, and no other copy does).
    let (median, sd) = super::pick::robust(&base);
    let mut gold = vec![];
    for (copy, d) in [(per_side + 1, [8i64, 0, 0]), (per_side * per_side + 2 * per_side + 2, [0, -8, 0])] {
        let o: [usize; 3] = std::array::from_fn(|k| (places[copy][k] as i64 + d[k]) as usize);
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    base[((o[2] + z) * base_dims[1] + o[1] + y) * base_dims[0] + o[0] + x] = median - 200.0 * sd;
                }
            }
        }
        // The cluster's centre, in raw voxels (base voxel b is raw f b .. f b + f − 1).
        gold.push(o.map(|x| f as f64 * x as f64 + (4.0 * f as f64 - 1.0) / 2.0));
    }
    let (raw, raw_dims) = finer(&base, base_dims, 0, f);
    // The look tomogram: the raw one minus its first 3 voxels, binned by 2 and
    // smoothed. It is left as clean as that makes it (far quieter than the
    // raw one, as a denoised tomogram is): the exclusion rule is not applied
    // to it but to the raw one.
    let shifted = finer(&base, base_dims, 3, f);
    let (look, look_dims) = binned_and_smoothed(&shifted.0, shifted.1);
    (raw, raw_dims, look, look_dims, places, body, gold)
}

/// Heavy, as `look_case(2)`: a made-up tomogram at 8.75 Å voxels with 32 dark copies of a
/// particle, and its "look" version (binned by 2, smoothed, 3 voxels of the
/// raw one's edge dropped first so the map is no pure doubling: raw = 2 × look
/// + 3.5). `run_template` searches the look tomogram with a 17.5 Å template
/// and has to cut and average the raw one at the places planted. Run:
/// `CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test --no-default-features --features cli --lib sta::tests::template_on_the_look -- --ignored --nocapture`.
#[test]
#[ignore]
fn template_on_the_look_tomogram_averages_the_raw_one() {
    look_case(2, 4);
}

/// Heavy, the real case's ratios: raw 5.83 Å voxels (the copies are 3 voxels
/// per template voxel), look 11.67 Å (binned by 2), template 17.5 Å: the
/// template's voxel is 1.5 × the look tomogram's, which cannot be binned to it.
#[test]
#[ignore]
fn template_on_a_look_tomogram_finer_than_the_template_by_one_and_a_half() {
    look_case(3, 4);
}

/// The raw tomogram has `f` voxels per template voxel (17.5 Å), the look
/// tomogram is the raw one binned by 2.
fn look_case(f: usize, per_side: usize) {
    use super::project::{run_template, LookSource, Source};
    let (raw, raw_dims, look, look_dims, places, body, gold) = look_fixture(per_side, 14.0, f);
    let raw_px = 17.5 / f as f64;
    let dir = std::env::temp_dir().join(format!("tmv-sta-look-{}-{f}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |n: &str| dir.join(n).to_string_lossy().into_owned();
    crate::tomo::output::write_mrc(&path("raw.mrc"), &raw, raw_dims, raw_px, false).unwrap();
    crate::tomo::output::write_mrc(&path("look.mrc"), &look, look_dims, 2.0 * raw_px, false).unwrap();
    // Template: the particle dark, at 17.5 Å, zero mean and unit variance.
    let mean = body.iter().map(|v| *v as f64).sum::<f64>() / body.len() as f64;
    let sd = (body.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / body.len() as f64).sqrt();
    let template: Vec<f32> = body.iter().map(|v| (-(*v as f64 - mean) / sd) as f32).collect();
    let sources = vec![Source {
        path: path("raw.mrc"),
        tilts: [-90.0, 90.0],
        look: Some(LookSource { path: path("look.mrc"), scale: 2.0, offset: [3.5; 3] }),
        series: None,
    }];
    let (dev, name) = crate::classify::device();
    eprintln!("device {name}");
    let out = path("out");
    // A copy planted at base voxel o is at raw voxel 2 o + 0.5. Distances
    // (raw voxels) from each particle of a table to the nearest copy.
    let planted: Vec<[f64; 3]> = places.iter().map(|o| o.map(|x| f as f64 * x as f64 + (f as f64 - 1.0) / 2.0)).collect();
    let dist = |a: &[f64; 3], b: &[f64; 3]| (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt();
    let read = |table: String| -> Vec<[f64; 3]> {
        std::fs::read_to_string(table)
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| {
                let f: Vec<f64> = l.split(',').skip(1).take(3).map(|x| x.parse().unwrap()).collect();
                [f[0], f[1], f[2]]
            })
            .collect()
    };
    let errors = |found: &[[f64; 3]]| -> Vec<f64> {
        let mut e: Vec<f64> = found.iter().map(|f| planted.iter().map(|p| dist(f, p)).fold(f64::MAX, f64::min)).collect();
        e.sort_by(f64::total_cmp);
        e
    };
    // The same without the look tomogram, for comparison.
    let plain = vec![Source { look: None, ..sources[0].clone() }];
    let base = run_template(&dev, &plain, &template, 17.5, 220.0, super::project::Pass::Fine, &path("base"), &|_, _, _| {}, &|_| {}).unwrap();
    let base_errors = errors(&read(base.table.unwrap()));
    eprintln!("without look: {} particles, {} candidates, distance to the planted copy: median {:.2}, max {:.2}", base.particles, base.candidates, base_errors[base_errors.len() / 2], base_errors[base_errors.len() - 1]);
    let done = run_template(&dev, &sources, &template, 17.5, 220.0, super::project::Pass::Fine, &out, &|_, _, _| {}, &|_| {}).unwrap();
    // The look tomogram was the one searched, at the voxel `look_search` gives
    // (the look voxel binned by the integer nearest to twice the template's:
    // 2 × 17.5 Å here, as the raw search's).
    assert_eq!(base.searched.as_deref(), Some("raw"));
    assert!((base.search_voxel_a.unwrap() - 35.0).abs() < 1e-3, "{:?}", base.search_voxel_a);
    assert_eq!(done.searched.as_deref(), Some("look"));
    let want = 35.0;
    eprintln!("searched {:?} at {:?} Å (raw search {:?} Å)", done.searched, done.search_voxel_a, base.search_voxel_a);
    assert!((done.search_voxel_a.unwrap() - want).abs() < 1e-3, "{:?} not {want}", done.search_voxel_a);
    eprintln!("{} particles of {} planted, {} candidates, {:?} Å", done.particles, places.len(), done.candidates, done.resolution_a);
    let found = read(done.table.unwrap());
    let errors = errors(&found);
    eprintln!("distance to the nearest planted copy (raw voxels): median {:.2}, max {:.2}", errors[errors.len() / 2], errors[errors.len() - 1]);
    // The exclusion distance is 2 × 0.25 × 220 Å = 220 Å = 25 raw voxels from
    // gold (a little more or less for the binning): nothing is reported
    // within 21 of a gold cluster's centre, and every copy 38 or more from
    // all of them (cluster half-diagonal 7 and the binning on top) is found.
    // (Those distances are for 2 raw voxels per template voxel; × f / 2 here.)
    let k = f as f64 / 2.0;
    let near_gold = |p: &[f64; 3], d: f64| gold.iter().any(|g| dist(p, g) <= d * k);
    assert!(gold.len() == 2 && planted.iter().filter(|p| near_gold(p, 21.0)).count() >= 2, "the fixture puts copies beside the gold");
    let too_close = found.iter().filter(|f| near_gold(f, 21.0)).count();
    let away: Vec<&[f64; 3]> = planted.iter().filter(|p| !near_gold(p, 38.0)).collect();
    let hit = away.iter().filter(|p| found.iter().any(|f| dist(f, p) <= 2.0 * k)).count();
    eprintln!("{too_close} particles within 21 raw voxels of gold; {hit} of the {} copies away from gold found within 2 raw voxels", away.len());
    assert_eq!(too_close, 0);
    // At 3 raw voxels per template voxel the noise is such that even the raw
    // search misses a few copies (a threshold between two crowds does): the
    // look search must find about as many (one fewer is within the noise of
    // locating a peak on a grid of 6 raw voxels), and at least 4 in 5.
    let base_found = read(std::fs::canonicalize(&path("base/particles.csv")).unwrap().to_string_lossy().into_owned());
    let base_hit = away.iter().filter(|p| base_found.iter().any(|f| dist(f, p) <= 2.0 * k)).count();
    eprintln!("the raw search finds {base_hit} of them");
    if f == 2 {
        assert_eq!(hit, away.len(), "copies away from gold that were not found");
    } else {
        assert!(hit + 1 >= base_hit && 5 * hit >= 4 * away.len(), "{hit} found, the raw search {base_hit}, of {}", away.len());
    }
    // Within 1.5 raw voxels for most (as without the look tomogram: the
    // alignment in this much noise and the cutting at 2-voxel steps leave
    // that much); a map that was off by the 3.5 voxels of the offset, or by
    // half a binned voxel, would show as a median beyond that.
    assert!(errors[errors.len() / 2] <= 1.5 * k && errors[errors.len() - 1] <= 2.5 * k, "median {} worst {}", errors[errors.len() / 2], errors[errors.len() - 1]);
    assert!(errors[errors.len() / 2] <= base_errors[base_errors.len() / 2] + 0.3 * k);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Cards are made from the raw tomogram only: a source's look tomogram is not
/// even opened (here it and the raw file are missing; the error names the raw
/// one, which the first thing `cards` does is open).
#[test]
fn cards_ignore_the_look_tomogram() {
    use super::project::{cards, LookSource, Source};
    let look = Some(LookSource { path: "/nonexistent/look_tomogram.mrc".into(), scale: 2.0, offset: [0.0; 3] });
    let sources = vec![Source { path: "/nonexistent/raw_tomogram.mrc".into(), tilts: [-60.0, 60.0], look, series: None }];
    let err = format!("{:#}", cards(&sources, "/nonexistent/cards", &|_, _, _| {}, &|_| {}).unwrap_err());
    assert!(err.contains("raw_tomogram.mrc") && !err.contains("look_tomogram"), "{err}");
}




/// A made-up card: soft balls `(centre Å, radius Å)` seen from orientation
/// `r` on a 32³ grid of `voxel` Å, a little smoothed, with noise.
fn made_up_card(voxel: f64, r: &Mat, balls: &[([f64; 3], f64)], noise: f64, seed: u64) -> Vec<f32> {
    let c = (N / 2) as f64;
    let rt = transpose(r);
    let mut v = vec![0f32; N * N * N];
    for z in 0..N {
        for y in 0..N {
            for x in 0..N {
                let q = [(x as f64 - c) * voxel, (y as f64 - c) * voxel, (z as f64 - c) * voxel];
                let p: [f64; 3] = std::array::from_fn(|i| rt[i][0] * q[0] + rt[i][1] * q[1] + rt[i][2] * q[2]);
                v[(z * N + y) * N + x] = balls
                    .iter()
                    .map(|(b, rad)| {
                        let d = ((p[0] - b[0]).powi(2) + (p[1] - b[1]).powi(2) + (p[2] - b[2]).powi(2)).sqrt();
                        1.0 / (1.0 + ((d - rad) / (0.2 * rad)).exp())
                    })
                    .sum::<f64>() as f32;
            }
        }
    }
    // Smoothed by a [1 2 1] kernel along each axis.
    for axis in [1, N, N * N] {
        let old = v.clone();
        for i in 0..v.len() {
            let (lo, hi) = (if (i / axis) % N > 0 { old[i - axis] } else { old[i] }, if (i / axis) % N < N - 1 { old[i + axis] } else { old[i] });
            v[i] = 0.25 * lo + 0.5 * old[i] + 0.25 * hi;
        }
    }
    let mut rng = Rng(seed);
    v.iter_mut().for_each(|x| *x += (noise * rng.gauss()) as f32);
    v
}

/// `same_kind` tells a card from a turned, smoothed, noisy copy of itself at
/// another voxel size (well above `SAME`), and from a rod or a ball twice as
/// large (well below). The numbers are printed (`--nocapture`). Heavy in a
/// debug build (about 25 s a pair; about a second with
/// `CARGO_PROFILE_TEST_OPT_LEVEL=2 cargo test --no-default-features --features cli --lib sta::tests::cards_of_the_same -- --ignored --nocapture`).
#[test]
#[ignore]
fn cards_of_the_same_particle_correlate() {
    use super::project::{same_kind, CardCube, SAME};
    // Three balls of different sizes, no symmetry: 160 Å card, 12.3 Å voxels.
    let three = [([0.0, 0.0, 0.0], 45.0), ([70.0, 0.0, 0.0], 32.0), ([-25.0, 60.0, 0.0], 24.0)];
    let radius = 0.8 * 160.0;
    let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let a = made_up_card(12.3, &id, &three, 0.0, 1);
    let ca = CardCube { cube: &a, voxel_a: 12.3, radius_a: radius };
    // The same object turned about an odd axis, at 16.1 Å voxels (a 210 Å card), noisy.
    let turned = axis_angle([0.3, -0.5, 0.8], 137.0);
    let b = made_up_card(16.1, &turned, &three, 0.12, 2);
    let started = std::time::Instant::now();
    let same = same_kind(ca, CardCube { cube: &b, voxel_a: 16.1, radius_a: 0.8 * 210.0 });
    eprintln!("one comparison: {:?}", started.elapsed());
    // The other way round, and the same voxel size.
    let same_back = same_kind(CardCube { cube: &b, voxel_a: 16.1, radius_a: 0.8 * 210.0 }, ca);
    let b2 = made_up_card(12.3, &turned, &three, 0.12, 3);
    let same_voxel = same_kind(ca, CardCube { cube: &b2, voxel_a: 12.3, radius_a: radius });
    // A rod (25 Å radius, longer than the mask) and a ball of twice the size.
    let rod: Vec<([f64; 3], f64)> = (-8..=8).map(|i| ([i as f64 * 15.0, 0.0, 0.0], 25.0)).collect();
    let rod = made_up_card(12.3, &id, &rod, 0.12, 4);
    let ball = made_up_card(12.3, &id, &[([0.0; 3], 90.0)], 0.12, 5);
    let vs_rod = same_kind(ca, CardCube { cube: &rod, voxel_a: 12.3, radius_a: radius });
    let vs_ball = same_kind(ca, CardCube { cube: &ball, voxel_a: 12.3, radius_a: radius });
    // A different asymmetric object of the same extent.
    let other = [([0.0, 0.0, 0.0], 30.0), ([60.0, 40.0, 0.0], 38.0), ([-50.0, 0.0, 55.0], 28.0)];
    let other = made_up_card(12.3, &turned, &other, 0.12, 6);
    let vs_other = same_kind(ca, CardCube { cube: &other, voxel_a: 12.3, radius_a: radius });
    eprintln!("same object, other voxel size, turned, noisy: {same:.3} (other way round {same_back:.3}); same voxel size: {same_voxel:.3}; rod: {vs_rod:.3}; 2x ball: {vs_ball:.3}; other 3-ball object: {vs_other:.3}");
    assert!(same >= SAME && same_back >= SAME && same_voxel >= SAME, "{same} {same_back} {same_voxel}");
    // An arrangement of the same kind of parts is more alike than a rod or a ball, but less than the object itself.
    assert!(vs_rod < SAME - 0.1 && vs_ball < SAME - 0.1, "{vs_rod} {vs_ball}");
    assert!(vs_other < same - 0.1, "{vs_other} {same}");
}

/// The made-up tomogram of `planted_tomogram` written to its own folder: the
/// folder, the sources, the centres (voxels) of the copies and the copy's body.
fn marked_tomogram(tag: &str, per_side: usize, noise: f64) -> (std::path::PathBuf, Vec<super::project::Source>, Vec<[usize; 3]>, Vec<f32>) {
    let (dims, v, places, body) = planted_tomogram(per_side, noise);
    let dir = std::env::temp_dir().join(format!("tmv-sta-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let tomo = dir.join("tomo.mrc").to_string_lossy().into_owned();
    crate::tomo::output::write_mrc(&tomo, &v, dims, 17.5, false).unwrap();
    (dir, vec![super::project::Source { path: tomo, tilts: [-90.0, 90.0], look: None, series: None }], places, body)
}

/// Clicks a voxel or two beside the middle of some copies give a card of the
/// copy: its size on the ladder is the one the automatic cards of this
/// tomogram have (210 Å, see `cards_hold_the_particles_of_a_kind`), the
/// clicks snap to the blobs, and the average is the copy (compared over every
/// orientation, as cards of the same particle are) at or above `SAME`.
#[test]
fn clicks_near_particles_make_a_card_of_the_particle() {
    use super::project::{card_from_points, read_card, same_kind, CardCube, SAME};
    let (dir, sources, places, body) = marked_tomogram("marked", 3, 3.0);
    let mut rng = Rng(3);
    // Nine of the 18 copies, each clicked up to 2 voxels off in every direction.
    let points: Vec<(usize, [f64; 3])> =
        places.iter().step_by(2).map(|o| (0, std::array::from_fn(|k| o[k] as f64 + 4.0 * rng.unit() - 2.0))).collect();
    let out = dir.join("marked").to_string_lossy().into_owned();
    let started = std::time::Instant::now();
    let card = card_from_points(&sources, &points, &out, &|_, _, _| {}, &|_| {}).unwrap();
    eprintln!("{:?} in {:?}", card, started.elapsed());
    assert_eq!(card.diameter_a, 210.0);
    assert!(card.measured_a > 120.0 && card.measured_a < 320.0, "{}", card.measured_a);
    assert!(card.members >= 6 && card.members <= points.len(), "{}", card.members);
    assert_eq!(card.path, format!("{out}/card.mrc"));
    let (cube, voxel) = read_card(&card.path).unwrap();
    assert!((voxel - 17.5).abs() < 0.01, "{voxel}");
    assert!(std::path::Path::new(&out).join("card.json").is_file());
    // The copy is bright in `body` and dark in the tomogram.
    let truth: Vec<f32> = body.iter().map(|v| -v).collect();
    let alike = same_kind(CardCube::of(&card, &cube), CardCube { cube: &truth, voxel_a: 17.5, radius_a: 0.8 * card.diameter_a });
    eprintln!("card against the copy: {alike:.3}");
    assert!(alike >= SAME, "{alike}");
    // The fewest clicks allowed still give a card, only a rougher one.
    let four = card_from_points(&sources, &points[..4], &dir.join("four").to_string_lossy(), &|_, _, _| {}, &|_| {}).unwrap();
    let (cube4, _) = read_card(&four.path).unwrap();
    let alike4 = same_kind(CardCube::of(&four, &cube4), CardCube { cube: &truth, voxel_a: 17.5, radius_a: 0.8 * four.diameter_a });
    eprintln!("four clicks: {alike4:.3}");
    assert!(four.members == 4 && alike4 >= SAME, "{} {alike4}", four.members);
    // Two clicks are examples: the card is the class of blobs they fell into.
    match card_from_points(&sources, &points[..2], &dir.join("two").to_string_lossy(), &|_, _, _| {}, &|_| {}) {
        Ok(two) => {
            let (cube2, _) = read_card(&two.path).unwrap();
            let alike2 = same_kind(CardCube::of(&two, &cube2), CardCube { cube: &truth, voxel_a: 17.5, radius_a: 0.8 * two.diameter_a });
            eprintln!("two clicks: {} members, {alike2:.3}", two.members);
            assert!(two.members >= 10 && alike2 >= SAME, "{} {alike2}", two.members);
        }
        // Eighteen copies can split into classes too small for a card: then the message asks for more clicks.
        Err(e) => {
            eprintln!("two clicks: {e:#}");
            assert!(format!("{e:#}").contains("多点几个"), "{e:#}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Clicks where nothing is give the message that says so, few or many.
#[test]
fn clicks_on_empty_places_or_too_few_are_refused() {
    use super::project::{card_from_points, MIN_POINTS};
    let (dir, sources, _, _) = marked_tomogram("empty", 3, 3.0);
    let out = dir.join("marked").to_string_lossy().into_owned();
    // Between the two layers of copies (z 24..29 and 70..75) and clear of every column.
    let empty: Vec<(usize, [f64; 3])> = (0..6).map(|i| (0, [41.0 + 38.0 * (i % 3) as f64, 41.0 + 38.0 * (i / 3) as f64, 48.0])).collect();
    let e = format!("{:#}", card_from_points(&sources, &empty, &out, &|_, _, _| {}, &|_| {}).unwrap_err());
    assert!(e.contains("没有找到颗粒"), "{e}");
    let few = format!("{:#}", card_from_points(&sources, &empty[..MIN_POINTS - 1], "/nonexistent/marked", &|_, _, _| {}, &|_| {}).unwrap_err());
    assert!(few.contains("没有找到颗粒"), "{few}");
    assert!(!std::path::Path::new(&out).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// "Find again" cuts the cube of every place of a search (one tomogram at a
/// time) and turns it back by the orientation found: the cubes are those
/// `run_places` cuts, in the template's frame, and a place whose cube does
/// not fit is left out.
#[test]
fn find_again_cuts_the_places_cubes_and_turns_them_back() {
    use super::project::{cut_places, Found, Place};
    let (dir, sources, centres, _) = marked_tomogram("again", 3, 0.2);
    let turns = grid(30.0);
    let mut places: Vec<Place> = centres.iter().enumerate().map(|(k, c)| Place { source: 0, c: *c, bin: 1, turn: if k % 2 == 0 { 0 } else { 5 }, score: 1.0 - k as f32 * 0.01 }).collect();
    // A place in the corner: no whole cube there.
    places.push(Place { source: 0, c: [2, 2, 2], bin: 1, turn: 0, score: 0.1 });
    let n = places.len();
    let found = Found { places: places.clone(), limit: None, noise: None, voxel_a: 17.5, searched: "raw".into(), search_voxel_a: None };
    let got = super::review::cubes_of(&sources, &found, 0, &(0..n).collect::<Vec<_>>()).unwrap();
    assert_eq!(got.len(), n - 1);
    assert!(got.iter().all(|(i, c)| *i < n - 1 && c.len() == N * N * N));
    // Each is the cut cube turned back.
    let (cuts, _) = cut_places(&sources, &places, 17.5).unwrap();
    assert_eq!(cuts.len(), n - 1);
    for ((i, cube), cut) in got.iter().zip(&cuts) {
        let want = rotate(&cut.cube, N, &transpose(&turns[places[*i].turn]));
        assert!(cube.iter().zip(&want).all(|(a, b)| (a - b).abs() < 1e-6));
    }
    // The copies are dark at their middle.
    let c = N / 2;
    for (_, cube) in &got {
        let mid: f32 = (c - 1..=c + 1).flat_map(|z| (c - 1..=c + 1).flat_map(move |y| (c - 1..=c + 1).map(move |x| (z, y, x)))).map(|(z, y, x)| cube[(z * N + y) * N + x]).sum();
        assert!(mid < 0.0, "{mid}");
    }
    // Only the places asked for, and none of a source with no place.
    assert_eq!(super::review::cubes_of(&sources, &found, 0, &[1, 4]).unwrap().iter().map(|g| g.0).collect::<Vec<_>>(), vec![1, 4]);
    assert!(super::review::cubes_of(&sources, &found, 1, &[1]).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
