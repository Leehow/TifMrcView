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
