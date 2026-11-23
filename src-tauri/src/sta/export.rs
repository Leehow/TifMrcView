//! Export STA particle tables (`particles.csv`) to RELION 5 (`particles.star`)
//! and Dynamo (`particles.tbl`) so averages can continue seamlessly into
//! external high-resolution workflows (RELION 5, Warp/M, Dynamo).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::tomo::{output, pipeline};

#[derive(Clone, Debug)]
struct Row {
    tomogram: String,
    x: f64,
    y: f64,
    z: f64,
    rot: f64,
    tilt: f64,
    psi: f64,
    score: f64,
    half: usize,
}

fn parse_csv(text: &str) -> Result<Vec<Row>> {
    let mut rows = vec![];
    for line in text.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.rsplitn(12, ',').collect();
        if cols.len() != 12 {
            continue;
        }
        // Columns in order: tomogram,x,y,z,rot,tilt,psi,score,half,blob,strength,darkest
        let tomogram = cols[11].to_string();
        let parse = |i: usize| -> Result<f64> { cols[i].parse::<f64>().with_context(|| format!("颗粒表数字无效: {}", cols[i])) };
        let half: usize = cols[3].parse().unwrap_or(1).clamp(1, 2);
        rows.push(Row {
            tomogram,
            x: parse(10)?,
            y: parse(9)?,
            z: parse(8)?,
            rot: parse(7)?,
            tilt: parse(6)?,
            psi: parse(5)?,
            score: parse(4)?,
            half,
        });
    }
    if rows.is_empty() {
        bail!("颗粒表里没有颗粒");
    }
    Ok(rows)
}

fn tomo_name(path: &str) -> String {
    Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("tomo").to_string()
}

/// Exports `<result_dir>/particles.csv` into `dest_dir`:
/// - `format == "relion"` -> `particles.star` (RELION 5 tomography STAR format)
/// - `format == "dynamo"` -> `particles.tbl` (Dynamo 35-column table + `tomograms.vll`)
/// - `format == "both"` -> both files
pub fn export(result_dir: &Path, dest_dir: &Path, format: &str) -> Result<Vec<String>> {
    let csv_path = result_dir.join("particles.csv");
    let text = std::fs::read_to_string(&csv_path).with_context(|| format!("找不到颗粒表 {}", csv_path.display()))?;
    let rows = parse_csv(&text)?;
    let sta_meta: serde_json::Value = std::fs::read(result_dir.join("sta.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let fallback_voxel = sta_meta["voxelA"].as_f64().filter(|v| v.is_finite() && *v > 0.0).unwrap_or(1.0);

    // Resolve each tomogram's pixel size and dimensions (when the file is on disk).
    let mut meta: HashMap<String, (f64, [f64; 3])> = HashMap::new();
    for r in &rows {
        meta.entry(r.tomogram.clone()).or_insert_with(|| {
            if let Ok(img) = pipeline::open(&r.tomogram) {
                let info = img.info();
                let px = output::pixel_a(&img);
                let px = if px.is_finite() && px > 0.0 { px } else { fallback_voxel };
                (px, [info.width as f64, info.height as f64, info.size_z as f64])
            } else {
                (fallback_voxel, [0.0, 0.0, 0.0])
            }
        });
    }

    std::fs::create_dir_all(dest_dir).with_context(|| format!("无法创建导出目录 {}", dest_dir.display()))?;
    let mut written = vec![];

    if format == "relion" || format == "both" {
        let mut s = String::new();
        let optics_px = meta.values().next().map_or(fallback_voxel, |m| m.0);
        s.push_str("\n# Exported by TifMrcView Pro STA (RELION 5 tomography format)\n");
        s.push_str("\ndata_optics\n\nloop_\n");
        s.push_str("_rlnOpticsGroup #1\n_rlnOpticsGroupName #2\n_rlnTomoSubtomogramBinning #3\n_rlnImagePixelSize #4\n");
        s.push_str(&format!("1 optics1 1 {:.4}\n", optics_px));

        s.push_str("\ndata_particles\n\nloop_\n");
        let cols = [
            "_rlnTomoName",
            "_rlnCoordinateX",
            "_rlnCoordinateY",
            "_rlnCoordinateZ",
            "_rlnCenteredCoordinateXAngst",
            "_rlnCenteredCoordinateYAngst",
            "_rlnCenteredCoordinateZAngst",
            "_rlnAngleRot",
            "_rlnAngleTilt",
            "_rlnAnglePsi",
            "_rlnOriginXAngst",
            "_rlnOriginYAngst",
            "_rlnOriginZAngst",
            "_rlnRandomSubset",
            "_rlnOpticsGroup",
            "_rlnLogLikeliContribution",
        ];
        for (i, c) in cols.iter().enumerate() {
            s.push_str(&format!("{c} #{}\n", i + 1));
        }
        for r in &rows {
            let (px, dims) = meta[&r.tomogram];
            let cx = if dims[0] > 0.0 { (r.x - dims[0] / 2.0) * px } else { r.x * px };
            let cy = if dims[1] > 0.0 { (r.y - dims[1] / 2.0) * px } else { r.y * px };
            let cz = if dims[2] > 0.0 { (r.z - dims[2] / 2.0) * px } else { r.z * px };
            s.push_str(&format!(
                "{} {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} 0.000 0.000 0.000 {} 1 {:.5}\n",
                tomo_name(&r.tomogram),
                r.x,
                r.y,
                r.z,
                cx,
                cy,
                cz,
                r.rot,
                r.tilt,
                r.psi,
                r.half,
                r.score
            ));
        }
        let path = dest_dir.join("particles.star");
        std::fs::write(&path, s)?;
        written.push(path.to_string_lossy().into_owned());
    }

    if format == "dynamo" || format == "both" {
        let mut tomo_ids: Vec<String> = vec![];
        for r in &rows {
            if !tomo_ids.contains(&r.tomogram) {
                tomo_ids.push(r.tomogram.clone());
            }
        }
        let mut tbl = String::new();
        for (idx, r) in rows.iter().enumerate() {
            let tomo_id = tomo_ids.iter().position(|t| t == &r.tomogram).unwrap_or(0) + 1;
            // Dynamo ZXZ convention vs RELION ZYZ: tdrot = -psi - 90, tilt = -tilt, narot = -rot + 90
            // (inverse passive convention mapping reference -> particle).
            let tdrot = -r.psi - 90.0;
            let tilt = -r.tilt;
            let narot = -r.rot + 90.0;
            // 35 columns (Dynamo .tbl):
            // 1:tag 2:aligned 3:averaged 4-6:dx,dy,dz 7-9:tdrot,tilt,narot 10:cc 11-12:cc2,cc3
            // 13:ftype 14-19:wedge 20:tomo 21:reg 22:class 23:annotation 24-26:x,y,z (1-based)
            // 27-33:zeros 34:ref 35:half
            tbl.push_str(&format!(
                "{} 1 1 0 0 0 {:.3} {:.3} {:.3} {:.5} 0 0 1 -60 60 0 0 0 0 {} 0 1 0 {:.3} {:.3} {:.3} 0 0 0 0 0 0 0 1 {}\n",
                idx + 1,
                tdrot,
                tilt,
                narot,
                r.score,
                tomo_id,
                r.x + 1.0,
                r.y + 1.0,
                r.z + 1.0,
                r.half
            ));
        }
        let tbl_path = dest_dir.join("particles.tbl");
        std::fs::write(&tbl_path, tbl)?;
        written.push(tbl_path.to_string_lossy().into_owned());

        let vll_path = dest_dir.join("tomograms.vll");
        let vll = tomo_ids.join("\n") + "\n";
        std::fs::write(&vll_path, vll)?;
        written.push(vll_path.to_string_lossy().into_owned());
    }

    if written.is_empty() {
        bail!("不支持的导出格式：{format}");
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_relion5_star_and_dynamo_tbl() {
        let dir = tempfile::tempdir().unwrap();
        let res = dir.path().join("res");
        let out = dir.path().join("exp");
        std::fs::create_dir_all(&res).unwrap();
        std::fs::write(
            res.join("particles.csv"),
            "tomogram,x,y,z,rot,tilt,psi,score,half,blob,strength,darkest\n/data/ts01.mrc,100.50,200.25,50.00,12.50,45.00,-30.00,0.4210,1,0.50,0.10,1.20\n/data/ts02.mrc,40.00,80.00,20.00,-90.00,15.00,60.00,0.5123,2,0.60,0.20,1.50\n",
        )
        .unwrap();
        std::fs::write(res.join("sta.json"), r#"{"voxelA": 4.5}"#).unwrap();

        let files = export(&res, &out, "both").unwrap();
        assert_eq!(files.len(), 3);
        let star = std::fs::read_to_string(out.join("particles.star")).unwrap();
        assert!(star.contains("data_optics"));
        assert!(star.contains("data_particles"));
        assert!(star.contains("_rlnCenteredCoordinateXAngst"));
        assert!(star.contains("ts01 100.500 200.250 50.000"));
        assert!(star.contains("12.500 45.000 -30.000 0.000 0.000 0.000 1 1 0.42100"));
        assert!(star.contains("ts02 40.000 80.000 20.000"));

        let tbl = std::fs::read_to_string(out.join("particles.tbl")).unwrap();
        let lines: Vec<&str> = tbl.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].split_whitespace().count(), 35);
        assert_eq!(lines[1].split_whitespace().count(), 35);
        let vll = std::fs::read_to_string(out.join("tomograms.vll")).unwrap();
        assert!(vll.contains("/data/ts01.mrc") && vll.contains("/data/ts02.mrc"));
    }
}
