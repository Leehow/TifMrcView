//! Whole-tomogram template search, for tests on real data:
//!
//!   cargo run --release --no-default-features --features cli --example sta_search -- \
//!     --template average.mrc [--tilts -60,60] [--diameter 250] [--step 30] \
//!     [--gpu] [--coarse 2] [--exclude] [--out DIR]
//!     [--look LOOK.mrc --look-map scale,ox,oy,oz] TOMOGRAM.mrc
//!
//! The template is a 32³ cube (`average.mrc` of `sta_coarse`). The tomogram
//! is binned to its voxel size. Writes `<out>/peaks.csv` (x, y, z in voxels
//! of the original tomogram, score, kept) and prints a summary. `--exclude`
//! keeps the search off the X/Y borders and gold markers. `--look` searches
//! the clear companion tomogram of `tmv tomo --for sta` instead (its `"look"`
//! JSON gives the file and `scale`, `offset`: raw voxel = scale × look voxel
//! + offset) and reports the places in the raw tomogram; with `--average` the
//! cubes are then cut from the raw one (single tomogram use).

use tifmrcview_lib::sta::project::{binned, to_raw, LookSource};
use tifmrcview_lib::sta::search::{allowed, cutoff, peaks, search};

/// Side of the template cube (`BOX` of `sta::project`).
const BOX: usize = 32;

fn main() -> anyhow::Result<()> {
    let mut tilts = [-60.0, 60.0];
    let mut template = None;
    let mut diameter = 250.0;
    let mut step = 30.0;
    let mut exclude = false;
    // --gpu: the convolution search on the GPU; --coarse 2: at half the template's sampling.
    let mut gpu = false;
    let mut coarse = 1usize;
    let mut out = String::from("sta");
    // --average: search, cut and average in one go (all tomograms given), as the app will.
    let mut average = false;
    // --fine: after the first alignment, again at twice the sampling.
    let mut fine = false;
    let mut more: Vec<String> = vec![];
    let mut look_path = None;
    let mut look_map = None;
    let mut path = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--tilts" => {
                let v = args.next().unwrap_or_default();
                let p: Vec<f64> = v.split(',').filter_map(|s| s.parse().ok()).collect();
                anyhow::ensure!(p.len() == 2, "--tilts lo,hi");
                tilts = [p[0], p[1]];
            }
            "--template" => template = Some(args.next().unwrap_or_default()),
            "--diameter" => diameter = args.next().unwrap_or_default().parse::<f64>()?,
            "--step" => step = args.next().unwrap_or_default().parse::<f64>()?,
            "--gpu" => gpu = true,
            "--coarse" => coarse = args.next().unwrap_or_default().parse::<usize>()?,
            "--exclude" => exclude = true,
            "--out" => out = args.next().unwrap_or_default(),
            "--average" => average = true,
            "--fine" => fine = true,
            "--look" => look_path = Some(args.next().unwrap_or_default()),
            "--look-map" => {
                let v = args.next().unwrap_or_default();
                let p: Vec<f64> = v.split(',').filter_map(|s| s.parse().ok()).collect();
                anyhow::ensure!(p.len() == 4, "--look-map scale,ox,oy,oz");
                look_map = Some((p[0], [p[1], p[2], p[3]]));
            }
            _ if path.is_some() => more.push(a),
            _ => path = Some(a),
        }
    }
    let template = template.ok_or_else(|| anyhow::anyhow!("give --template average.mrc"))?;
    let path = path.ok_or_else(|| anyhow::anyhow!("give a tomogram"))?;
    anyhow::ensure!(diameter > 0.0 && step > 0.0, "--diameter and --step must be positive");
    let look = match (look_path, look_map) {
        (Some(path), Some((scale, offset))) => Some(LookSource { path, scale, offset }),
        (None, None) => None,
        _ => anyhow::bail!("--look and --look-map go together"),
    };
    anyhow::ensure!(look.is_none() || more.is_empty(), "--look is for a single tomogram");
    let start = std::time::Instant::now();

    // The template, read like a tomogram.
    let (cube, voxel) = tifmrcview_lib::sta::project::read_card(&template)?;

    if average {
        use tifmrcview_lib::sta::project::{run_template, Source};
        let (dev, name) = tifmrcview_lib::classify::device();
        eprintln!("device: {name}");
        let mut sources = vec![Source { path: path.clone(), tilts, look: look.clone(), series: None }];
        sources.extend(more.iter().map(|p| Source { path: p.clone(), tilts, look: None, series: None }));
        let last = std::sync::Mutex::new(String::new());
        let progress = |stage: &str, done: usize, total: usize| {
            let mut l = last.lock().unwrap();
            if *l != stage || done == total {
                eprintln!("{stage} {done}/{total}");
                *l = stage.to_string();
            }
        };
        let report = run_template(&dev, &sources, &cube, voxel, diameter, fine, &out, &progress, &|_| {})?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    anyhow::ensure!(coarse == 1 || coarse == 2, "--coarse 1 or 2");
    let side = BOX / coarse;
    let cube = tifmrcview_lib::spa::fourier::resample3(&cube, BOX, side);
    let voxel = voxel * coarse as f64;
    // With a look tomogram the search runs on it; places are mapped to `path`.
    let (v, dims, got, bin) = binned(look.as_ref().map_or(&path, |l| &l.path), voxel)?;
    if (got / voxel - 1.0).abs() > 0.02 {
        eprintln!("warning: the tomogram is binned to {got:.2} A, the template has {voxel:.2} A");
    }
    let radius = 0.5 * diameter / voxel;
    let turns = tifmrcview_lib::spa::geom::grid(step);
    eprintln!("tomogram {}x{}x{} at {got:.2} A, {} orientations", dims[0], dims[1], dims[2], turns.len());
    let mask = exclude.then(|| allowed(&v, dims, radius));
    let excluded = mask.as_ref().map(|m| m.iter().filter(|ok| !**ok).count() as f64 / m.len() as f64);
    let tick = |done: usize, total: usize| eprintln!("search {done}/{total}");
    let found = if gpu {
        let (dev, name) = tifmrcview_lib::classify::device();
        eprintln!("device: {name}");
        tifmrcview_lib::sta::search_gpu::search(&dev, &v, dims, &cube, side, radius, tilts, &turns, mask.as_deref(), &tick)?
    } else {
        anyhow::ensure!(coarse == 1, "--coarse needs --gpu");
        search(&v, dims, &cube, BOX, radius, tilts, &turns, mask.as_deref(), &tick)?
    };
    let all = peaks(&found, 2.0 * radius, usize::MAX);
    let cut = cutoff(&all);

    // Binned voxel p lies at p * bin + (bin - 1) / 2 in the original; a
    // look tomogram's places are mapped through its scale and offset.
    let at = |p: [usize; 3]| -> [f64; 3] {
        match &look {
            Some(l) => to_raw(p.map(|x| x as f64), bin, 1, l),
            None => p.map(|x| (x * bin + (bin - 1) / 2) as f64),
        }
    };
    let mut csv = String::from("x,y,z,score,kept\n");
    let mut kept = 0;
    for p in &all {
        let k = cut.is_some_and(|c| p.score >= c);
        kept += k as usize;
        let a = at(p.pos);
        csv.push_str(&format!("{},{},{},{:.5},{}\n", a[0], a[1], a[2], p.score, k as u8));
    }
    std::fs::create_dir_all(&out)?;
    std::fs::write(format!("{out}/peaks.csv"), csv)?;

    let ranks: serde_json::Map<String, serde_json::Value> =
        [1usize, 10, 50, 100, 200, 400].iter().filter_map(|&r| all.get(r - 1).map(|p| (r.to_string(), serde_json::json!(p.score)))).collect();
    let summary = serde_json::json!({
        "voxelA": got,
        "dims": dims,
        "orientations": turns.len(),
        "peaks": all.len(),
        "excluded": excluded,
        "cutoff": cut,
        "kept": kept,
        "scoreAtRank": ranks,
        "seconds": start.elapsed().as_secs_f64(),
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
