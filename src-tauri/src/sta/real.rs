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
    eprintln!("{} particles, {} Å, voxel {} Å, {:.0} s", r["particles"], r["resolutionA"], r["voxelA"], started.elapsed().as_secs_f64());
    eprintln!("fsc {}", r["fsc"]);
}
