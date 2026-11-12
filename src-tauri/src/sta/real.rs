//! Runs on real data that is not in the repository: each test is ignored
//! and reads where its data lies from the environment, so it only runs
//! when asked for by name on a machine that has the data.

use serde_json::Value;

use super::marks::{run_marks, Kind, Mark};

/// The seven virus-like particles of the Dynamo walkthrough "lattices on
/// vesicles" (v17.rec, 463×463×150 voxels of 8 Å), marked as a user would:
/// a click at the centre and one on the membrane, by eye on section 75.
const V17: [([f64; 3], f64); 7] = [
    ([105.0, 198.0, 75.0], 57.0),
    ([235.0, 235.0, 75.0], 65.0),
    ([333.0, 138.0, 75.0], 55.0),
    ([385.0, 245.0, 75.0], 52.0),
    ([140.0, 310.0, 75.0], 52.0),
    ([318.0, 330.0, 75.0], 55.0),
    ([215.0, 395.0, 75.0], 55.0),
];

/// `TMV_V17=/path/v17.rec TMV_OUT=/path/out cargo test … sta::real::v17_spheres -- --ignored --nocapture`
#[test]
#[ignore]
fn v17_spheres() {
    let file = std::env::var("TMV_V17").expect("TMV_V17 = the path of v17.rec");
    let out = std::env::var("TMV_OUT").expect("TMV_OUT = a folder for the result");
    let marks: Vec<Mark> = V17
        .iter()
        .map(|(c, r)| Mark { kind: Kind::Sphere, tomogram: file.clone(), points: vec![*c, [c[0] + r, c[1], c[2]]], flip: false, strokes: vec![] })
        .collect();
    let started = std::time::Instant::now();
    let emit = |name: &str, v: Value| {
        if name == "sta-progress" && v["done"] == v["total"] {
            eprintln!("{:6.1} s  {}", started.elapsed().as_secs_f64(), v["label"].as_str().unwrap_or(""));
        }
    };
    static CPU: std::sync::OnceLock<burn::tensor::Device> = std::sync::OnceLock::new();
    let dev = || Ok((CPU.get_or_init(|| crate::classify::device().0), "CPU"));
    let v = run_marks(&[file.clone()], &marks, None, false, Some(&out), &dev, &emit, &|_| String::new()).unwrap();
    let r = &v["report"];
    eprintln!("{} particles, {} Å, voxel {} Å, pattern {}, {:.0} s", r["particles"], r["resolutionA"], r["voxelA"], r["pattern"], started.elapsed().as_secs_f64());
    eprintln!("fsc {}", r["fsc"]);
}

/// The kinds among the particles of the average `v17_spheres` left in
/// `TMV_OUT` (same variables).
#[test]
#[ignore]
fn v17_kinds() {
    let file = std::env::var("TMV_V17").expect("TMV_V17 = the path of v17.rec");
    let out = std::env::var("TMV_OUT").expect("TMV_OUT = the folder v17_spheres wrote to");
    let dir = std::path::Path::new(&out).join("drawn");
    let sources = vec![super::project::Source { path: file, tilts: [-60.0, 60.0], look: None, series: None }];
    let started = std::time::Instant::now();
    let v = super::project::kinds_of(&sources, &dir.to_string_lossy(), &|label, done, total| {
        if done == total {
            eprintln!("{:6.1} s  {label}", started.elapsed().as_secs_f64());
        }
    })
    .unwrap();
    let kinds: Vec<String> = v["kinds"].as_array().unwrap().iter().map(|k| format!("kind {}: {}", k["id"], k["count"])).collect();
    eprintln!("{} particles, apart {}, {kinds:?}, {:.0} s", v["particles"], v["apart"], started.elapsed().as_secs_f64());
}

/// The same vesicles given as for particles twice as large (the rows of
/// the lattice are then finer than the sampling the size asks for).
#[test]
#[ignore]
fn v17_spheres_coarse() {
    use super::project::{run_drawn, Drawn, Source};
    let file = std::env::var("TMV_V17").expect("TMV_V17 = the path of v17.rec");
    let out = std::env::var("TMV_OUT").expect("TMV_OUT = a folder for the result");
    let size_a = 450.0;
    let drawn: Vec<Drawn> = V17
        .iter()
        .flat_map(|(c, r)| super::shape::ellipsoid(*c, [*r; 3], 0.5 * size_a / 8.0))
        .map(|s| Drawn { source: 0, pos: s.pos, axis: s.axis })
        .collect();
    let sources = vec![Source { path: file, tilts: [-60.0, 60.0], look: None, series: None }];
    let started = std::time::Instant::now();
    let r = run_drawn(&sources, &drawn, false, size_a, &out, &|label, done, total| {
        if done == total {
            eprintln!("{:6.1} s  {label}", started.elapsed().as_secs_f64());
        }
    }, &|_| {})
    .unwrap();
    eprintln!("{} seeds, {} particles, {:?} Å, voxel {} Å, pattern {:?}, {:.0} s", drawn.len(), r.particles, r.resolution_a, r.voxel_a, r.pattern, started.elapsed().as_secs_f64());
}

/// What the cards of a run (`TMV_CARDS`: its `cards` folder) measure as:
/// the parts of each card's elongation and the hints that follow.
#[test]
#[ignore]
fn card_shapes() {
    use super::project::{elongations, read_card, shape, Card, BOX};
    let dir = std::path::PathBuf::from(std::env::var("TMV_CARDS").expect("TMV_CARDS = the cards folder of a run"));
    let cards: Vec<Card> = serde_json::from_slice(&std::fs::read(dir.join("cards.json")).unwrap()).unwrap();
    for c in cards {
        let path = if std::path::Path::new(&c.path).exists() { std::path::PathBuf::from(&c.path) } else { dir.join(std::path::Path::new(&c.path).file_name().unwrap()) };
        let (avg, voxel) = read_card(&path.to_string_lossy()).unwrap();
        let ball = 0.6 * c.diameter_a / voxel;
        let (compact, elongation, blobs, offset) = shape(&avg, BOX, ball);
        let (all, piece) = elongations(&avg, BOX, ball);
        eprintln!(
            "card {}: {} Å, {} members, voxel {voxel:.2}, ball {ball:.1}; saved: compact {:.2} elongation {:.2} blobs {} likely {}; now: compact {compact:.2} elongation {elongation:.2} blobs {blobs} offset {offset:.2}; all density {all:.2}, centre piece {piece:?} (own elongation, runs on to the edge)",
            c.id, c.diameter_a, c.members, c.compact, c.elongation, c.blobs, c.likely
        );
    }
}

/// How a membrane segmentation with fewer mirrored copies (`TMV_COPIES`,
/// 1 by default) compares with the one already beside `TMV_TOMO` (all
/// eight, from `tmv membranes`): the time, how much of each mask the two
/// share, and how much of each lies within a voxel of the other.
#[test]
#[ignore]
fn membrane_copies() {
    use crate::membrane::{load, model_path, segment, Pass};
    let tomogram = std::env::var("TMV_TOMO").expect("TMV_TOMO = a tomogram segmented by tmv membranes");
    let copies: usize = std::env::var("TMV_COPIES").ok().and_then(|c| c.parse().ok()).unwrap_or(1);
    let all = super::paint::mask_of(&tomogram).unwrap().expect("the membranes of the eight-copy run beside the tomogram");
    let (v, n, px) = crate::tomo::pipeline::load(&tomogram).unwrap();
    assert_eq!(n, all.n);
    let model = model_path().expect("a membrane model set up");
    let started = std::time::Instant::now();
    let (seg, backend) = crate::classify::on_device(|dev| {
        let net = load(&model, dev)?;
        segment(dev, &net, &v, n, px as f32, Pass::Copies(copies), &mut |p| {
            if p.done % 20 == 0 {
                eprintln!("window {}/{}", p.done, p.total);
            }
        })
    })
    .unwrap();
    let seconds = started.elapsed().as_secs_f64();
    let (a, b) = (&seg.mask, &all.mask);
    let count = |m: &[bool]| m.iter().filter(|x| **x).count();
    let both = a.iter().zip(b).filter(|(x, y)| **x && **y).count();
    // A mask grown by one voxel along each axis.
    let grown = |m: &[bool]| -> Vec<bool> {
        (0..m.len())
            .map(|i| {
                let p = [i % n[0], i / n[0] % n[1], i / (n[0] * n[1])];
                m[i] || [(0, 1), (1, n[0]), (2, n[0] * n[1])].iter().any(|(axis, step)| (p[*axis] > 0 && m[i - step]) || (p[*axis] + 1 < n[*axis] && m[i + step]))
            })
            .collect()
    };
    let near = |m: &[bool], other: &[bool]| {
        let g = grown(other);
        m.iter().zip(&g).filter(|(x, y)| **x && **y).count() as f64 / count(m).max(1) as f64
    };
    eprintln!(
        "{copies} copies on {backend}: {seconds:.0} s, {} windows, {} membrane voxels ({} with eight copies), Dice {:.4}; within a voxel of the other: {:.4} of this one, {:.4} of the eight-copy one; cut short: {:?}",
        seg.stats.windows,
        count(a),
        count(b),
        2.0 * both as f64 / (count(a) + count(b)).max(1) as f64,
        near(a, b),
        near(b, a),
        seg.stats.cut_short
    );
}

/// What the user's ticks are worth: a review of circled examples
/// (`TMV_REVIEW`, the folder with its `found.json`) is gone through as a user
/// would, fifty pictures at a time, best first. A place is right when it lies
/// within `TMV_NEAR` voxels (default 45) of one of the positions in
/// `TMV_ANSWERS` (a text file, "x y z" per line, voxels of the first
/// tomogram); the answers stand for the user's eye and for nothing else.
/// After every fifty the places not yet seen are put in a new order by what
/// was ticked and what was not, in several ways, at several widths of the
/// cubes compared; printed is how many of the answers were reached after
/// 50, 100, … pictures. "search" is the order the search gave.
///
/// `TMV_REVIEW=… TMV_ANSWERS=… cargo test … sta::real::ticks_put_the_rest_in_order -- --ignored --nocapture`
#[test]
#[ignore]
fn ticks_put_the_rest_in_order() {
    use super::project::BOX;
    use super::review;
    let dir = std::env::var("TMV_REVIEW").expect("TMV_REVIEW = the folder of a circled review (found.json)");
    let answers: Vec<[f64; 3]> = std::fs::read_to_string(std::env::var("TMV_ANSWERS").expect("TMV_ANSWERS = a text file of x y z"))
        .unwrap()
        .lines()
        .filter_map(|l| {
            let v: Vec<f64> = l.split(|c: char| c.is_whitespace() || c == ',').filter_map(|x| x.parse().ok()).collect();
            (v.len() >= 3).then(|| [v[0], v[1], v[2]])
        })
        .collect();
    let near: f64 = std::env::var("TMV_NEAR").ok().and_then(|x| x.parse().ok()).unwrap_or(45.0);
    let s = review::load(std::path::Path::new(&dir)).expect("no found.json there");
    let sources: Vec<super::project::Source> = s.inputs.iter().map(|i| super::auto::resolve_tomogram(i, &|_| {}).source()).collect();
    let total = s.found.places.len();
    // Which answer a place stands on (of the first tomogram).
    let answer_of: Vec<Option<usize>> = s
        .found
        .places
        .iter()
        .map(|p| {
            let at = p.at();
            (p.source == 0)
                .then(|| answers.iter().enumerate().map(|(k, a)| (k, (0..3).map(|d| (a[d] - at[d]).powi(2)).sum::<f64>().sqrt())).filter(|(_, d)| *d <= near).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(k, _)| k))
                .flatten()
        })
        .collect();
    eprintln!("{} places, {} of them on one of {} answers ({} answers reached at all); the search looked {} times as wide as circled", total, answer_of.iter().flatten().count(), answers.len(), {
        let mut k: Vec<usize> = answer_of.iter().flatten().copied().collect();
        k.sort_unstable();
        k.dedup();
        k.len()
    }, s.found.wide);
    let ways = ["search", "like the ticked", "ticked less unticked", "nearest ticked", "nearest ticked less nearest unticked"];
    for wide in [1.0, 2.0, 3.0] {
        for smooth in [0.0, 1.0] {
            let mut found = s.found.clone();
            found.wide = wide;
            // Every cube inside the ball, zero mean, unit length.
            let c = (BOX / 2) as f64;
            let radius = 0.6 * s.card.diameter_a / s.found.voxel_a;
            let inside: Vec<usize> = (0..BOX * BOX * BOX).filter(|i| (((i % BOX) as f64 - c).powi(2) + ((i / BOX % BOX) as f64 - c).powi(2) + ((i / (BOX * BOX)) as f64 - c).powi(2)).sqrt() <= radius).collect();
            let mut cubes: Vec<Option<Vec<f32>>> = vec![None; total];
            for src in 0..sources.len() {
                for (i, cube) in review::cubes_of(&sources, &found, src, &(0..total).collect::<Vec<_>>()).unwrap() {
                    let cube = if smooth > 0.0 { super::pick::gauss(&cube, [BOX; 3], smooth) } else { cube };
                    let mut v: Vec<f32> = inside.iter().map(|k| cube[*k]).collect();
                    let mean = v.iter().sum::<f32>() / v.len() as f32;
                    v.iter_mut().for_each(|x| *x -= mean);
                    let len = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
                    v.iter_mut().for_each(|x| *x /= len);
                    cubes[i] = Some(v);
                }
            }
            let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
            for (w, way) in ways.iter().enumerate() {
                // The examples are ticked from the start.
                let mut ticked: Vec<usize> = s.mine.clone();
                let mut unticked: Vec<usize> = vec![];
                let mut seen = vec![false; total];
                ticked.iter().for_each(|i| seen[*i] = true);
                let mut reached: Vec<usize> = vec![];
                for round in 0..6 {
                    let score = |i: usize| -> f32 {
                        let Some(x) = &cubes[i] else { return f32::MIN };
                        let like = |set: &[usize], best: bool| {
                            let all = set.iter().filter_map(|k| cubes[*k].as_ref()).map(|k| dot(x, k));
                            if best { all.fold(f32::MIN, f32::max) } else { all.sum::<f32>() / set.len().max(1) as f32 }
                        };
                        match (w, round, unticked.is_empty()) {
                            (0, _, _) => -(i as f32),
                            (1, _, _) | (2, _, true) => like(&ticked, false),
                            (2, _, false) => like(&ticked, false) - like(&unticked, false),
                            (3, _, _) | (4, _, true) => like(&ticked, true),
                            _ => like(&ticked, true) - like(&unticked, true),
                        }
                    };
                    let mut rest: Vec<(usize, f32)> = (0..total).filter(|i| !seen[*i]).map(|i| (i, score(i))).collect();
                    rest.sort_by(|a, b| b.1.total_cmp(&a.1));
                    for (i, _) in rest.into_iter().take(50) {
                        seen[i] = true;
                        match answer_of[i] {
                            Some(k) => {
                                ticked.push(i);
                                if !reached.contains(&k) {
                                    reached.push(k);
                                }
                            }
                            None => unticked.push(i),
                        }
                    }
                    eprint!("{}{}", if round == 0 { format!("cubes {wide}x, smoothed {smooth}: {way:38}") } else { String::new() }, format_args!(" {:2}", reached.len()));
                }
                eprintln!("  of {} (ticked {}, unticked {})", answers.len(), ticked.len(), unticked.len());
            }
        }
    }
}

/// Whether an average has an axis it can be turned about and stay the same
/// (a thing on a membrane often has: the membrane's normal), and what it
/// looks like averaged about it. `TMV_MAP` = a cubic map (an average of
/// `tmv sta`); `TMV_FOLD` = the turn tried is 360°/fold (12 when not given).
/// Writes `<map>_axis.mrc` (the axis along z) and `<map>_turned.mrc` (that,
/// averaged over the fold turns) beside the map.
/// `TMV_MAP=/path/average_raw.mrc cargo test … sta::real::the_axis_of_an_average -- --ignored --nocapture`
#[test]
#[ignore]
fn the_axis_of_an_average() {
    use crate::spa::discover::{align_axis, axis_corr, best_axis, centred3};
    let path = std::env::var("TMV_MAP").expect("TMV_MAP = a cubic map");
    let fold: usize = std::env::var("TMV_FOLD").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let img = crate::tomo::pipeline::open(&path).unwrap();
    let info = img.info();
    let n = info.width;
    assert!(info.height == n && info.size_z == n, "{}x{}x{}", info.width, info.height, info.size_z);
    let voxel = crate::tomo::output::pixel_a(&img);
    let mut v = vec![0f32; n * n * n];
    for z in 0..n {
        let (page, s) = info.locate(0, z, 0);
        let p = img.page(page).unwrap();
        let p = &p[s.min(p.len() - 1)];
        for (i, o) in v[z * n * n..(z + 1) * n * n].iter_mut().enumerate() {
            *o = p.data.get_f64(i) as f32;
        }
    }
    // Dark on light, as tomograms are: the mass is what is below the mean.
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let dark: Vec<f32> = v.iter().map(|x| mean - x).collect();
    let centred = centred3(&dark, n);
    let (axis, at, half) = best_axis(&centred, n, fold);
    let along = align_axis(&centred, n, axis);
    let mut turned = vec![0f32; along.len()];
    for k in 0..fold {
        let r = crate::spa::geom::axis_angle([0.0, 0.0, 1.0], 360.0 * k as f64 / fold as f64);
        for (o, x) in turned.iter_mut().zip(super::cc::rotate(&along, n, &r)) {
            *o += x / fold as f32;
        }
    }
    let same = super::tests::corr(&along, &turned);
    eprintln!("{path}: {n}³ at {voxel:.2} Å; axis {axis:.2?}, the map turned by {:.0}° matches itself at {at:.3} (by half that {half:.3}); about z after aligning: 90° {:.3}, 180° {:.3}; with its average over the turns {same:.3}", 360.0 / fold as f64, axis_corr(&along, n, 2, 90.0), axis_corr(&along, n, 2, 180.0));
    let stem = path.trim_end_matches(".mrc");
    crate::tomo::output::write_mrc(&format!("{stem}_axis.mrc"), &along, [n, n, n], voxel, false).unwrap();
    crate::tomo::output::write_mrc(&format!("{stem}_turned.mrc"), &turned, [n, n, n], voxel, false).unwrap();
}

/// How well the axis of a thing is read off its surroundings with no
/// reference (`axial::sheet`, `axial::lean`), against the axes of a known
/// answer (scoring only). `TMV_TOMO` = the tomogram; `TMV_ANSWERS` = a text
/// file, "x y z tdrot tilt narot" per line (voxels of `TMV_TOMO`, Dynamo's
/// angles in degrees); `TMV_RADIUS_A` = the radius of a circle drawn round
/// one thing, Å; `TMV_VOXEL_A` = the voxel the circled search worked at, Å;
/// `TMV_BY` = how many times as wide the surroundings are cut (2).
/// Dynamo turns its reference by Rz(tdrot)·Rx(tilt)·Rz(narot); which way
/// round its tables mean that is not checked here, so both are scored (one
/// of them is the axis, the other is not). Random axes score about 57°.
///
/// `TMV_TOMO=… TMV_ANSWERS=… TMV_RADIUS_A=… TMV_VOXEL_A=… cargo test … sta::real::axes_from_the_surroundings -- --ignored --nocapture`
#[test]
#[ignore]
fn axes_from_the_surroundings() {
    use super::axial::{apart, lean, sheet};
    use super::project::{binned, cube_around, on_grid, BOX};
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} is not set"));
    let number = |k: &str| env(k).parse::<f64>().unwrap_or_else(|_| panic!("{k} is not a number"));
    let tomo = env("TMV_TOMO");
    let rows: Vec<[f64; 6]> = std::fs::read_to_string(env("TMV_ANSWERS"))
        .unwrap()
        .lines()
        .filter_map(|l| {
            let v: Vec<f64> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() >= 6).then(|| [v[0], v[1], v[2], v[3], v[4], v[5]])
        })
        .collect();
    let (radius_a, voxel_a) = (number("TMV_RADIUS_A"), number("TMV_VOXEL_A"));
    let by = std::env::var("TMV_BY").ok().and_then(|x| x.parse::<f64>().ok()).unwrap_or(2.0);
    let (v, dims, wide_voxel, bin) = binned(&tomo, voxel_a * by).unwrap();
    let r = radius_a / wide_voxel;
    eprintln!("{tomo}: {} things, cubes of {BOX} at {wide_voxel:.2} Å (bin {bin}), the circle {r:.1} voxels across its radius", rows.len());
    // Dynamo's reference Z axis carried into the tomogram, either way round.
    let axis_a = |t: &[f64; 6]| {
        let ((st, ct), (sa, ca)) = (t[4].to_radians().sin_cos(), t[3].to_radians().sin_cos());
        [st * sa, -st * ca, ct]
    };
    let axis_b = |t: &[f64; 6]| {
        let ((st, ct), (sn, cn)) = (t[4].to_radians().sin_cos(), t[5].to_radians().sin_cos());
        [st * sn, st * cn, ct]
    };
    let median = |mut x: Vec<f64>| {
        x.sort_by(f64::total_cmp);
        if x.is_empty() { f64::NAN } else { x[x.len() / 2] }
    };
    for scale in [0.5, 1.0, 1.5] {
        let sigma = scale * r;
        let (mut sa, mut sb, mut la, mut lb, mut flats) = (vec![], vec![], vec![], vec![], vec![]);
        for t in &rows {
            let (at, _) = on_grid([t[0], t[1], t[2]], bin);
            let Some(cube) = cube_around(&v, dims, at, BOX, 4) else { continue };
            let (normal, flat) = sheet(&cube, BOX, sigma);
            let (towards, far) = lean(&cube, BOX, sigma.max(2.0) * 1.5);
            sa.push(apart(normal, axis_a(t)));
            sb.push(apart(normal, axis_b(t)));
            la.push(apart(towards, axis_a(t)));
            lb.push(apart(towards, axis_b(t)));
            flats.push(flat);
            if scale == 1.0 {
                eprintln!("  {:>6.0} {:>6.0} {:>5.0}: sheet {:>4.0}° / {:>4.0}° (flat {flat:.2}), lean {:>4.0}° / {:>4.0}° ({far:.2})", t[0], t[1], t[2], sa.last().unwrap(), sb.last().unwrap(), la.last().unwrap(), lb.last().unwrap());
            }
        }
        eprintln!(
            "window {scale}× the circle: median off, sheet {:.0}° / {:.0}°, lean {:.0}° / {:.0}° (either way round); sheet flatness {:.2}; {} cut",
            median(sa.clone()),
            median(sb),
            median(la),
            median(lb),
            median(flats),
            sa.len()
        );
    }
}
