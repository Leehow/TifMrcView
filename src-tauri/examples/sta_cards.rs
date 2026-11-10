//! The cards to choose a template from, for tests on real data:
//!
//!   cargo run --release --no-default-features --features cli --example sta_cards -- \
//!     [--tilts -60,60] [--out DIR] [--look LOOK.mrc --look-map scale,ox,oy,oz] TOMOGRAM.mrc [MORE.mrc …]
//!
//! Class averages of the blobs of each size go to `<out>/card_<id>.mrc` and
//! `<out>/cards.json`; the JSON is printed too. Tomograms are those of
//! `tmv tomo --for sta` (X, Y = tilt axis, Z = beam). `--look` finds the blobs
//! and makes the cards on the clear companion tomogram instead (single
//! tomogram use): its `"look"` entry in the JSON of `tmv tomo --for sta` gives
//! the file and `scale`, `offset` (raw voxel = scale × look voxel + offset).

use tifmrcview_lib::sta::project::{cards, LookSource, Source};

fn main() -> anyhow::Result<()> {
    let mut tilts = [-60.0, 60.0];
    let mut out = String::from("sta");
    let mut paths = vec![];
    let mut look_path = None;
    let mut look_map = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--tilts" => {
                let v = args.next().unwrap_or_default();
                let p: Vec<f64> = v.split(',').filter_map(|s| s.parse().ok()).collect();
                anyhow::ensure!(p.len() == 2, "--tilts lo,hi");
                tilts = [p[0], p[1]];
            }
            "--out" => out = args.next().unwrap_or_default(),
            "--look" => look_path = Some(args.next().unwrap_or_default()),
            "--look-map" => {
                let v = args.next().unwrap_or_default();
                let p: Vec<f64> = v.split(',').filter_map(|s| s.parse().ok()).collect();
                anyhow::ensure!(p.len() == 4, "--look-map scale,ox,oy,oz");
                look_map = Some((p[0], [p[1], p[2], p[3]]));
            }
            _ => paths.push(a),
        }
    }
    anyhow::ensure!(!paths.is_empty(), "give at least one tomogram");
    let look = match (look_path, look_map) {
        (Some(path), Some((scale, offset))) => Some(LookSource { path, scale, offset }),
        (None, None) => None,
        _ => anyhow::bail!("--look and --look-map go together"),
    };
    anyhow::ensure!(look.is_none() || paths.len() == 1, "--look is for a single tomogram");
    let sources: Vec<Source> = paths.into_iter().map(|path| Source { path, tilts, look: look.clone() }).collect();
    let last = std::sync::Mutex::new(String::new());
    let progress = |stage: &str, done: usize, total: usize| {
        let mut l = last.lock().unwrap();
        if *l != stage || done == total {
            eprintln!("{stage} {done}/{total}");
            *l = stage.to_string();
        }
    };
    let found = cards(&sources, &out, &progress, &|_| {})?;
    println!("{}", serde_json::to_string_pretty(&found)?);
    Ok(())
}
