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
    let v = run_marks(&[file.clone()], &marks, None, Some(&out), &dev, &emit, &|_| String::new()).unwrap();
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
