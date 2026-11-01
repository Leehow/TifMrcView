//! Export of a single-particle project for other software.
//!
//! - "relion": a folder `<name>_relion/` that RELION (3.1 and later) and
//!   cryoSPARC import directly:
//!   - `micrographs_ctf.star`: optics group and one row per micrograph used,
//!     with its CTF (as RELION's CtfFind job writes it);
//!   - `coords/<micrograph>_autopick.star` and `autopick.star` (RELION 4's
//!     list of coordinate files): every pick, to re-extract elsewhere at
//!     another box or pixel size;
//!   - `particles.mrcs` (a hard link when the file system allows, else a
//!     copy) with `particles.star` (all particles, their 2D class) and,
//!     after a choice of 2D classes, `particles_selected.star`;
//!   - after refinement `refine.star` (angles, origins in Å, half set as
//!     rlnRandomSubset) and the maps: `map.mrc`, `half1.mrc`, `half2.mrc`,
//!     `mask.mrc`, `sharpened.mrc`; the initial model as `initial.mrc`.
//!   Micrograph paths stay absolute; particle images are named relative to
//!   the STAR files, so the folder can be moved together.
//!   cryoSPARC: "Import Particle Stack" with `refine.star` (or
//!   `particles_selected.star`) and the folder as data path; "Import 3D
//!   Volumes" for the maps; "Import Micrographs" + "Import Particles" for
//!   the coordinates.
//! - "table": `<name>_particles.csv`, one row per particle with everything
//!   the project knows about it, for scripts and spreadsheets.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::project::{self, Project};

fn name(p: &Project) -> String {
    Path::new(&p.source)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("spa")
        .to_string()
}

fn stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("mic")
        .to_string()
}

/// A hard link when possible, else a copy.
fn place(src: &str, dst: &Path) -> Result<()> {
    if dst.exists() {
        std::fs::remove_file(dst)?;
    }
    if std::fs::hard_link(src, dst).is_err() {
        std::fs::copy(src, dst).with_context(|| format!("复制 {src} 失败"))?;
    }
    Ok(())
}

/// A STAR loop block.
fn star(block: &str, labels: &[&str], rows: &[Vec<String>]) -> String {
    let mut s = format!("\ndata_{block}\n\nloop_\n");
    for (i, l) in labels.iter().enumerate() {
        let _ = writeln!(s, "_rln{l} #{}", i + 1);
    }
    for r in rows {
        s.push_str(&r.join(" "));
        s.push('\n');
    }
    s
}

pub fn relion(
    p: &Project,
    dir: &str,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Vec<String>> {
    let out = Path::new(dir).join(format!("{}_relion", name(p)));
    std::fs::create_dir_all(out.join("coords"))?;
    let mut files: Vec<PathBuf> = vec![];
    let o = &p.optics;

    // Micrographs with their CTF.
    let mut s = String::from("# TifMrcView Pro\n");
    s += &star(
        "optics",
        &[
            "OpticsGroup",
            "OpticsGroupName",
            "MicrographOriginalPixelSize",
            "Voltage",
            "SphericalAberration",
            "AmplitudeContrast",
            "MicrographPixelSize",
        ],
        &[vec![
            "1".into(),
            "opticsGroup1".into(),
            format!("{:.5}", o.pixel_a),
            format!("{:.1}", o.kv),
            format!("{:.3}", o.cs_mm),
            format!("{:.4}", o.amplitude),
            format!("{:.5}", o.pixel_a),
        ]],
    );
    let used: Vec<usize> = (0..p.mics.len())
        .filter(|&i| !p.mics[i].excluded && p.mics[i].ctf.is_some())
        .collect();
    if used.is_empty() {
        bail!("还没有拟合 CTF 的显微图");
    }
    let rows: Vec<Vec<String>> = used
        .iter()
        .map(|&i| {
            let m = &p.mics[i];
            let c = m.ctf.as_ref().unwrap();
            vec![
                m.path.clone(),
                "1".into(),
                format!("{:.1}", c.df1),
                format!("{:.1}", c.df2),
                format!("{:.1}", (c.df1 - c.df2).abs()),
                // RELION measures the defocus angle with y down in the image.
                format!("{:.2}", (-c.angle).rem_euclid(180.0)),
                format!("{:.2}", c.fit_res),
                format!("{:.5}", c.score),
            ]
        })
        .collect();
    s += &star(
        "micrographs",
        &[
            "MicrographName",
            "OpticsGroup",
            "DefocusU",
            "DefocusV",
            "CtfAstigmatism",
            "DefocusAngle",
            "CtfMaxResolution",
            "CtfFigureOfMerit",
        ],
        &rows,
    );
    let f = out.join("micrographs_ctf.star");
    std::fs::write(&f, s)?;
    files.push(f);

    // Coordinates of every pick.
    let picks = p.picks()?;
    let mut list = vec![];
    for (k, &i) in used.iter().enumerate() {
        let Some(q) = picks.get(i).filter(|q| !q.is_empty()) else {
            continue;
        };
        let rows: Vec<Vec<String>> = q
            .iter()
            .map(|c| {
                vec![
                    format!("{:.2}", c[0]),
                    format!("{:.2}", c[1]),
                    format!("{:.4}", c[2]),
                ]
            })
            .collect();
        let f = out
            .join("coords")
            .join(format!("{}_autopick.star", stem(&p.mics[i].path)));
        std::fs::write(
            &f,
            format!(
                "# TifMrcView Pro\n{}",
                star(
                    "",
                    &["CoordinateX", "CoordinateY", "AutopickFigureOfMerit"],
                    &rows
                )
            ),
        )?;
        list.push(vec![
            p.mics[i].path.clone(),
            format!("coords/{}", f.file_name().unwrap().to_string_lossy()),
        ]);
        progress(k + 1, used.len() + 1);
    }
    if !list.is_empty() {
        let f = out.join("autopick.star");
        std::fs::write(
            &f,
            format!(
                "# TifMrcView Pro\n{}",
                star(
                    "coordinate_files",
                    &["MicrographName", "MicrographCoordinates"],
                    &list
                )
            ),
        )?;
        files.push(f);
    }

    // Particles and maps.
    if let Some(st) = &p.stack {
        let parts = p.particles()?;
        let stack = out.join("particles.mrcs");
        place(&st.path, &stack)?;
        files.push(stack.clone());
        let s = stack.to_string_lossy();
        let f = out.join("particles.star");
        project::write_star(p, &parts, &s, st.box_px, st.pixel_a, None, false, &f)?;
        files.push(f);
        if p.class2d.as_ref().is_some_and(|c| !c.selected.is_empty()) {
            let ids = project::selected(p, &parts);
            let sub: Vec<_> = ids.iter().map(|&i| parts[i].clone()).collect();
            let f = out.join("particles_selected.star");
            project::write_star(p, &sub, &s, st.box_px, st.pixel_a, Some(&ids), false, &f)?;
            files.push(f);
        }
        if let Some(c) = &p.class2d {
            let f = out.join("class2d.mrcs");
            place(&c.path, &f)?;
            files.push(f);
        }
        if let Some(r) = &p.refine {
            let ids = project::selected(p, &parts);
            let sub: Vec<_> = ids.iter().map(|&i| parts[i].clone()).collect();
            let f = out.join("refine.star");
            project::write_star(p, &sub, &s, st.box_px, st.pixel_a, Some(&ids), true, &f)?;
            files.push(f);
            for (src, dst) in [
                (&r.map, "map.mrc"),
                (&r.half1, "half1.mrc"),
                (&r.half2, "half2.mrc"),
                (&r.mask, "mask.mrc"),
                (&r.sharpened, "sharpened.mrc"),
            ] {
                let f = out.join(dst);
                place(src, &f)?;
                files.push(f);
            }
        }
    }
    if let Some(m) = &p.initial {
        let f = out.join("initial.mrc");
        place(&m.path, &f)?;
        files.push(f);
    }
    progress(used.len() + 1, used.len() + 1);
    Ok(files.iter().map(|f| f.to_string_lossy().into()).collect())
}

pub fn table(p: &Project, dir: &str) -> Result<Vec<String>> {
    let parts = p.particles()?;
    let px = p
        .stack
        .as_ref()
        .map(|s| s.pixel_a)
        .unwrap_or(p.optics.pixel_a);
    let used = project::selected(p, &parts);
    let keep: std::collections::HashSet<usize> = used.into_iter().collect();
    let mut s = String::from("index,micrograph,x_px,y_px,defocus_u_A,defocus_v_A,defocus_angle_deg,pick_score,class2d,selected,rot_deg,tilt_deg,psi_deg,origin_x_A,origin_y_A,half_set\n");
    for (i, q) in parts.iter().enumerate() {
        let mic = &p.mics[q.mic as usize].path;
        let oriented = p.refine.is_some() && keep.contains(&i);
        let _ = write!(
            s,
            "{},\"{}\",{:.2},{:.2},{:.1},{:.1},{:.2},{:.4},{},{}",
            i + 1,
            mic.replace('"', "\"\""),
            q.x,
            q.y,
            q.df1,
            q.df2,
            q.angle,
            q.fom,
            if q.class2d >= 0 {
                (q.class2d + 1).to_string()
            } else {
                String::new()
            },
            keep.contains(&i) as u8
        );
        if oriented {
            let _ = writeln!(
                s,
                ",{:.3},{:.3},{:.3},{:.3},{:.3},{}",
                q.euler[0],
                q.euler[1],
                q.euler[2],
                -q.shift[0] as f64 * px,
                -q.shift[1] as f64 * px,
                q.half
            );
        } else {
            s.push_str(",,,,,,\n");
        }
    }
    std::fs::create_dir_all(dir)?;
    let f = Path::new(dir).join(format!("{}_particles.csv", name(p)));
    std::fs::write(&f, s)?;
    Ok(vec![f.to_string_lossy().into()])
}

pub fn run(
    p: &Project,
    format: &str,
    dir: &str,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Vec<String>> {
    match format {
        "relion" => relion(p, dir, progress),
        "table" => table(p, dir),
        other => bail!("未知导出格式 {other}"),
    }
}
