//! From tomograms to a coarse average with no parameters to set.
//!
//! 1. Each tomogram is binned to about `SEARCH_VOXEL` Å and searched for
//!    dark blobs of a ladder of sizes (`pick`). The sizes found are the
//!    "kinds" offered to the user; gold beads, ice and dense debris make
//!    kinds of their own that are simply not chosen.
//! 2. For the chosen size the tomograms are binned again so that size
//!    spans about `SPAN` voxels of a `BOX`³ cube, the cubes are cut at the
//!    blob centres and brought to zero mean and unit variance.
//! 3. The strongest `FIRST` blobs make a first model (`refine::first_model`),
//!    every particle is then placed on it by a global search, and the two
//!    halves are aligned locally (`refine::align`).
//!
//! Results go to `<out>/`: `average.mrc`, `half1.mrc`, `half2.mrc`,
//! `particles.csv` (tomogram, position in tomogram voxels, RELION-style
//! rot/tilt/psi, score) and `sta.json`.

use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::pick::{blobs, Blob};
use super::refine::{align, first_model, Live, Progress, Set};
use super::{cc, check_cancel, Pose};
use crate::spa::geom::{angles, axis_angle, grid, mul};
use crate::spa::Rng;
use crate::tomo::{output, pipeline};

/// Voxel size the tomograms are searched at, Å.
const SEARCH_VOXEL: f64 = 16.0;
/// Blob diameters looked for, Å.
const LADDER: [f64; 7] = [90.0, 120.0, 160.0, 210.0, 280.0, 370.0, 490.0];
/// Robust z-score a blob needs.
const MIN_Z: f32 = 5.0;
/// Side of the cubes, and how many voxels of it the particle spans.
const BOX: usize = 32;
const SPAN: f64 = 13.0;
/// Particles in the first model, and most particles averaged here.
const FIRST: usize = 200;
const MOST: usize = 3000;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// A tomogram (X, Y = tilt axis, Z = beam).
    pub path: String,
    /// First and last tilt used for it, degrees.
    pub tilts: [f64; 2],
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Kind {
    pub diameter_a: f64,
    pub count: usize,
    /// Median robust z-score of its blobs.
    pub contrast: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub kinds: Vec<Kind>,
    /// None when the user has to choose a kind first.
    pub diameter_a: Option<f64>,
    pub particles: usize,
    pub voxel_a: f64,
    /// Resolution at FSC 0.143 between the halves, Å.
    pub resolution_a: Option<f64>,
    pub fsc: Vec<f64>,
    pub map: Option<String>,
    pub halves: Option<[String; 2]>,
    pub table: Option<String>,
    pub ms: u128,
}

/// A tomogram averaged over `bin`³ voxels: values, size, voxel size (Å).
pub fn binned(path: &str, voxel_a: f64) -> Result<(Vec<f32>, [usize; 3], f64, usize)> {
    let img = pipeline::open(path)?;
    let info = img.info();
    let px = output::pixel_a(&img);
    let bin = ((voxel_a / px).round() as usize).max(1);
    let (w, h, nz) = (info.width / bin, info.height / bin, info.size_z / bin);
    if w < BOX || h < BOX || nz < 4 {
        bail!("断层图太小：{}", path);
    }
    let mut out = vec![0f32; w * h * nz];
    let scale = 1.0 / (bin * bin * bin) as f32;
    for z in 0..nz * bin {
        check_cancel()?;
        let (page, s) = info.locate(0, z, 0);
        let p = img.page(page)?;
        let p = &p[s.min(p.len() - 1)];
        let plane = &mut out[(z / bin) * w * h..(z / bin + 1) * w * h];
        plane.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for j in 0..bin {
                let src = (y * bin + j) * info.width;
                for (x, o) in row.iter_mut().enumerate() {
                    for i in 0..bin {
                        *o += p.data.get_f64(src + x * bin + i) as f32 * scale;
                    }
                }
            }
        });
    }
    Ok((out, [w, h, nz], px * bin as f64, bin))
}

/// The sizes found, most common first; sizes with very few blobs are left out.
pub fn kinds(found: &[Blob], voxel_a: f64) -> Vec<Kind> {
    let mut out: Vec<Kind> = LADDER
        .iter()
        .filter_map(|&d| {
            let mut z: Vec<f32> = found.iter().filter(|b| ((b.diameter as f64 * voxel_a) - d).abs() < 1.0).map(|b| b.score).collect();
            if z.len() < 8 {
                return None;
            }
            z.sort_by(f32::total_cmp);
            Some(Kind { diameter_a: d, count: z.len(), contrast: z[z.len() / 2] as f64 })
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count));
    out
}

struct Cut {
    source: usize,
    /// Centre in voxels of the source tomogram (unbinned).
    at: [f64; 3],
    /// Source voxels per working voxel.
    bin: usize,
    score: f32,
    cube: Vec<f32>,
}

fn cube(v: &[f32], dims: [usize; 3], c: [usize; 3]) -> Option<Vec<f32>> {
    let h = BOX / 2;
    if (0..3).any(|i| c[i] < h || c[i] + h > dims[i]) {
        return None;
    }
    let mut out = Vec::with_capacity(BOX * BOX * BOX);
    for z in c[2] - h..c[2] + h {
        for y in c[1] - h..c[1] + h {
            let row = (z * dims[1] + y) * dims[0];
            out.extend_from_slice(&v[row + c[0] - h..row + c[0] + h]);
        }
    }
    let n = out.len() as f64;
    let mean = out.iter().map(|x| *x as f64).sum::<f64>() / n;
    let sd = (out.iter().map(|x| (*x as f64 - mean).powi(2)).sum::<f64>() / n).sqrt();
    if sd <= 0.0 {
        return None;
    }
    out.iter_mut().for_each(|x| *x = ((*x as f64 - mean) / sd) as f32);
    Some(out)
}

/// `diameter_a` None finds the kinds only (the report then has no map).
pub fn run(sources: &[Source], diameter_a: Option<f64>, out_dir: &str, progress: Progress, live: Live) -> Result<Report> {
    let started = Instant::now();
    if sources.is_empty() {
        bail!("没有断层图");
    }
    // 1. Kinds.
    let mut all: Vec<Blob> = vec![];
    for (i, s) in sources.iter().enumerate() {
        progress("找颗粒", i, sources.len());
        let (v, dims, voxel, _) = binned(&s.path, SEARCH_VOXEL)?;
        let sizes: Vec<f64> = LADDER.iter().map(|d| d / voxel).collect();
        let mut found = blobs(&v, dims, &sizes, true, MIN_Z)?;
        // Report sizes on the ladder exactly, whatever the voxel size.
        for b in &mut found {
            let k = sizes.iter().position(|d| (*d as f32 - b.diameter).abs() < 1e-3).unwrap_or(0);
            b.diameter = (LADDER[k] / SEARCH_VOXEL) as f32;
        }
        all.extend(found);
    }
    progress("找颗粒", sources.len(), sources.len());
    let kinds = kinds(&all, SEARCH_VOXEL);
    let mut report = Report {
        kinds: kinds.clone(),
        diameter_a: None,
        particles: 0,
        voxel_a: 0.0,
        resolution_a: None,
        fsc: vec![],
        map: None,
        halves: None,
        table: None,
        ms: 0,
    };
    let Some(want) = diameter_a else {
        report.ms = started.elapsed().as_millis();
        return Ok(report);
    };
    // The ladder size nearest to what was asked for.
    let size = *LADDER.iter().min_by(|a, b| (*a - want).abs().total_cmp(&(*b - want).abs())).unwrap();

    // 2. Cubes at the working voxel size.
    let mut cuts: Vec<Cut> = vec![];
    let mut voxel_a = size / SPAN;
    for (i, s) in sources.iter().enumerate() {
        progress("裁出颗粒", i, sources.len());
        let (v, dims, voxel, bin) = binned(&s.path, size / SPAN)?;
        voxel_a = voxel;
        // Blobs again at this sampling, at the one size: centres are finer.
        let found = blobs(&v, dims, &[size / voxel], true, MIN_Z)?;
        for b in found {
            let c = [b.pos[0].round() as usize, b.pos[1].round() as usize, b.pos[2].round() as usize];
            if let Some(cube) = cube(&v, dims, c) {
                // The centre of working voxel c in source voxels.
                let at = c.map(|v| v as f64 * bin as f64 + (bin as f64 - 1.0) / 2.0);
                cuts.push(Cut { source: i, at, bin, score: b.score, cube });
            }
        }
    }
    progress("裁出颗粒", sources.len(), sources.len());
    if cuts.len() < 20 {
        bail!("这种尺寸的颗粒只找到 {} 个，太少了", cuts.len());
    }
    cuts.sort_by(|a, b| b.score.total_cmp(&a.score));
    cuts.truncate(MOST);
    let wedges: Vec<Vec<f32>> = sources.iter().map(|s| cc::wedge(BOX, s.tilts[0], s.tilts[1])).collect();
    // Sizes from the scale space are rough and tend to be small: masks
    // and search ranges allow for a particle 1.6 times as large.
    let radius = 0.8 * size / voxel_a;

    // 3. First model from the strongest, then everything.
    let first = cuts.len().min(FIRST);
    let particles: Vec<Vec<f32>> = cuts.iter().map(|c| c.cube.clone()).collect();
    let wedge_of: Vec<usize> = cuts.iter().map(|c| c.source).collect();
    let head = Set { n: BOX, particles: &particles[..first], wedges: &wedges, wedge_of: &wedge_of[..first], radius };
    let (model, mut poses) = first_model(&head, 1, progress, live)?;
    let set = Set { n: BOX, particles: &particles, wedges: &wedges, wedge_of: &wedge_of, radius };
    if cuts.len() > first {
        let engine = cc::Engine::new(BOX, (BOX as f64 / (2.0 * radius) * 4.0).min(BOX as f64 / 2.0 - 3.0), 1.0, radius + 2.0);
        let rest: Vec<_> = (first..cuts.len()).into_par_iter().map(|i| engine.prepare(&particles[i], &wedges[wedge_of[i]])).collect();
        let mut rng = Rng(2);
        let turn = axis_angle([rng.gauss(), rng.gauss(), rng.gauss()], 360.0 * rng.unit());
        let rotations: Vec<_> = grid(30.0).iter().map(|r| mul(r, &turn)).collect();
        let shift = (radius / 2.0).ceil().max(2.0) as usize;
        let placed = engine.global(&rest, &wedges, &wedge_of[first..], &model, &rotations, shift, &|a, b| progress("放置全部颗粒", a, b))?;
        poses.extend(placed);
    }
    let out = align(&set, &model, &poses, progress, live)?;

    // Results.
    std::fs::create_dir_all(out_dir).with_context(|| format!("无法创建 {out_dir}"))?;
    let file = |name: &str| Path::new(out_dir).join(name).to_string_lossy().into_owned();
    let shape = [BOX, BOX, BOX];
    output::write_mrc(&file("average.mrc"), &out.map, shape, voxel_a, false)?;
    output::write_mrc(&file("half1.mrc"), &out.halves[0], shape, voxel_a, false)?;
    output::write_mrc(&file("half2.mrc"), &out.halves[1], shape, voxel_a, false)?;
    let mut csv = String::from("tomogram,x,y,z,rot,tilt,psi,score,half\n");
    for (i, (c, p)) in cuts.iter().zip(&out.poses).enumerate() {
        let Pose { r, t, score } = *p;
        let [rot, tilt, psi] = angles(&r);
        let k = c.bin as f64;
        csv += &format!(
            "{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.4},{}\n",
            sources[c.source].path,
            c.at[0] + t[0] * k,
            c.at[1] + t[1] * k,
            c.at[2] + t[2] * k,
            rot,
            tilt,
            psi,
            score,
            i % 2 + 1
        );
    }
    std::fs::write(file("particles.csv"), csv)?;
    report.diameter_a = Some(size);
    report.particles = cuts.len();
    report.voxel_a = voxel_a;
    report.resolution_a = (out.shell > 0.0).then(|| BOX as f64 * voxel_a / out.shell);
    report.fsc = out.fsc;
    report.map = Some(file("average.mrc"));
    report.halves = Some([file("half1.mrc"), file("half2.mrc")]);
    report.table = Some(file("particles.csv"));
    report.ms = started.elapsed().as_millis();
    std::fs::write(file("sta.json"), serde_json::to_string_pretty(&report)?)?;
    Ok(report)
}
