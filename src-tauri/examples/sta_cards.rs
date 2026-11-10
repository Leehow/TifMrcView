//! The cards to choose a template from, for tests on real data:
//!
//!   cargo run --release --no-default-features --features cli --example sta_cards -- \
//!     [--tilts -60,60] [--out DIR] TOMOGRAM.mrc [MORE.mrc …]
//!
//! Class averages of the blobs of each size go to `<out>/card_<id>.mrc` and
//! `<out>/cards.json`; the JSON is printed too. Tomograms are those of
//! `tmv tomo --for sta` (X, Y = tilt axis, Z = beam), the full-detail ones:
//! cards are always made from them, never from the clear `look` tomogram.

use tifmrcview_lib::sta::project::{cards, Source};

fn main() -> anyhow::Result<()> {
    let mut tilts = [-60.0, 60.0];
    let mut out = String::from("sta");
    let mut paths = vec![];
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
            _ => paths.push(a),
        }
    }
    anyhow::ensure!(!paths.is_empty(), "give at least one tomogram");
    let sources: Vec<Source> = paths.into_iter().map(|path| Source { path, tilts, look: None }).collect();
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
