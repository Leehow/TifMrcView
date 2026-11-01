//! Export of a dataset for other software.
//!
//! - "imod": an etomo-style folder `<name>/` with the stack (`<name>.st`,
//!   a hard link when the file system allows, else a copy), the nominal and
//!   refined angles (`.rawtlt`, `.tlt`), the transforms (`.xf`, every view
//!   in stack order, IMOD's convention), the views left out
//!   (`<name>_exclude.txt`, IMOD's list syntax, 1-based) and the CTF as a
//!   ctfplotter defocus file (`.defocus`, version 3). IMOD reads it as its
//!   own alignment; Warp (`WarpTools ts_import_alignments`) and RELION 5's
//!   tomography import take IMOD alignments from such a folder.
//! - "aligned": the aligned views (`<name>_ali.mrc`, tilt axis vertical,
//!   at the chosen binning, excluded views left out, sorted by tilt) and
//!   their refined angles (`<name>_ali.tlt`), for programs that start from
//!   an aligned tilt series (AreTomo with alignment off, EMAN2, Dynamo …).
//! - "relion5": a RELION 5 tomography project start, `<name>_relion5/`:
//!   `tilt_series.star` (the global table) and `tilt_series/<name>.star`
//!   (one row per tilt kept, with CTF and alignment), plus each tilt as its
//!   own MRC in `frames/` (and the even/odd halves when there are some, as
//!   rlnMicrographNameEven/Odd). The alignment follows RELION 5's own
//!   conversion of IMOD results (relion `align_tilt_series/imod`):
//!   XTilt 0, YTilt the refined tilt, ZRot the in-plane angle of the `.xf`
//!   matrix (atan2(−A21, A11), i.e. IMOD's rotation angle), and the shifts
//!   the specimen shifts −A⁻¹·(DX, DY) in Å. RELION has no per-tilt
//!   magnification, so it is left out (ours stay within a percent).
//!   rlnTomoHand is written as −1, RELION's usual value for Krios data;
//!   check it against one tomogram before averaging.
//! - "table": `<name>_cryoet.csv`, one row per view with everything the
//!   dataset knows about it, for scripts and spreadsheets.

use super::{dataset::Dataset, geometry, Alignment, View};
use anyhow::{bail, Result};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One `.xf` row: the transform taking the raw view to the aligned one.
pub fn xf_row(a: &Alignment, v: &View) -> [f64; 6] {
    let r = geometry::rotation(a.axis + v.rotation);
    let m = [
        r[0][0] / v.mag,
        r[1][0] / v.mag,
        r[0][1] / v.mag,
        r[1][1] / v.mag,
    ];
    [
        m[0],
        m[1],
        m[2],
        m[3],
        -m[0] * v.shift[0] - m[1] * v.shift[1],
        -m[2] * v.shift[0] - m[3] * v.shift[1],
    ]
}

/// 1-based IMOD list (`1,4-6`).
fn ranges(mut v: Vec<usize>) -> String {
    v.sort_unstable();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let mut j = i;
        while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
            j += 1;
        }
        out.push(if i == j {
            format!("{}", v[i] + 1)
        } else {
            format!("{}-{}", v[i] + 1, v[j] + 1)
        });
        i = j + 1;
    }
    out.join(",")
}

fn stem(d: &Dataset) -> String {
    Path::new(&d.stack)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("stack")
        .to_string()
}

/// Views of the alignment in stack order.
fn by_section(a: &Alignment) -> Vec<&View> {
    let mut v: Vec<&View> = a.views.iter().collect();
    v.sort_by_key(|v| v.file_index);
    v
}

pub fn imod(d: &Dataset, dir: &str) -> Result<Vec<String>> {
    let Some(a) = d.alignment.as_ref() else {
        bail!("先完成倾转对齐")
    };
    let name = stem(d);
    let folder = PathBuf::from(dir).join(&name);
    std::fs::create_dir_all(&folder)?;
    let file = |ext: &str| folder.join(format!("{name}.{ext}"));
    let mut written = Vec::new();
    let mut put = |path: PathBuf, text: String| -> Result<()> {
        std::fs::write(&path, text)?;
        written.push(path.to_string_lossy().into_owned());
        Ok(())
    };
    let views = by_section(a);
    let (mut xf, mut tlt, mut raw) = (String::new(), String::new(), String::new());
    for v in &views {
        let x = xf_row(a, v);
        writeln!(
            xf,
            "{:12.7} {:12.7} {:12.7} {:12.7} {:12.3} {:12.3}",
            x[0], x[1], x[2], x[3], x[4], x[5]
        )?;
        writeln!(tlt, "{:.2}", v.refined())?;
        writeln!(raw, "{:.2}", v.angle)?;
    }
    put(file("xf"), xf)?;
    put(file("tlt"), tlt)?;
    put(file("rawtlt"), raw)?;
    let excluded: Vec<usize> = views
        .iter()
        .filter(|v| v.excluded)
        .map(|v| v.file_index)
        .collect();
    put(
        folder.join(format!("{name}_exclude.txt")),
        format!("{}\n", ranges(excluded)),
    )?;
    if let Some(ctf) = &d.ctf {
        // Version 3: a header line, then per view
        // start end tilt-start tilt-end defocus1 defocus2 (nm) astigmatism angle (°).
        let mut text = String::from("1  0 0. 0. 0  3\n");
        for f in &ctf.tilts {
            let t = d.angles.get(f.z).copied().unwrap_or(0.0);
            writeln!(
                text,
                "{}  {}  {:.2}  {:.2}  {:.1}  {:.1}  {:.2}",
                f.z + 1,
                f.z + 1,
                t,
                t,
                f.df1 / 10.0,
                f.df2 / 10.0,
                f.angle
            )?;
        }
        put(file("defocus"), text)?;
    }
    let st = file("st");
    if !st.exists() {
        if std::fs::hard_link(&d.stack, &st).is_err() {
            std::fs::copy(&d.stack, &st)?;
        }
    }
    written.push(st.to_string_lossy().into_owned());
    Ok(written)
}

pub fn table(d: &Dataset, dir: &str) -> Result<Vec<String>> {
    let Some(a) = d.alignment.as_ref() else {
        bail!("先完成倾转对齐")
    };
    let px = d.optics.pixel_a.unwrap_or(1.0);
    let mut text = String::from("section,tilt_nominal_deg,tilt_refined_deg,excluded,shift_x_px,shift_y_px,shift_x_A,shift_y_A,axis_deg,rotation_deg,magnification,residual_px,defocus1_A,defocus2_A,astigmatism_angle_deg,ctf_fit_resolution_A\n");
    for v in by_section(a) {
        let ctf = d
            .ctf
            .as_ref()
            .and_then(|c| c.tilts.iter().find(|f| f.z == v.file_index));
        let c = |f: fn(&super::dataset::TiltCtf) -> f64| {
            ctf.map(|t| format!("{:.1}", f(t))).unwrap_or_default()
        };
        writeln!(
            text,
            "{},{:.3},{:.3},{},{:.3},{:.3},{:.2},{:.2},{:.3},{:.3},{:.5},{:.3},{},{},{},{}",
            v.file_index + 1,
            v.angle,
            v.refined(),
            v.excluded as u8,
            v.shift[0],
            v.shift[1],
            v.shift[0] * px,
            v.shift[1] * px,
            a.axis,
            v.rotation,
            v.mag,
            v.residual,
            c(|t| t.df1),
            c(|t| t.df2),
            c(|t| t.angle),
            c(|t| t.fit_res),
        )?;
    }
    let path = PathBuf::from(dir).join(format!("{}_cryoet.csv", stem(d)));
    std::fs::write(&path, text)?;
    Ok(vec![path.to_string_lossy().into_owned()])
}

pub fn aligned(
    d: &Dataset,
    dir: &str,
    bin: usize,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Vec<String>> {
    let Some(a) = d.alignment.as_ref() else {
        bail!("先完成倾转对齐")
    };
    let img = super::pipeline::open(&d.stack)?;
    let px = d
        .optics
        .pixel_a
        .unwrap_or_else(|| super::output::pixel_a(&img));
    let (data, w, h, angles) = super::output::aligned(&img, a, bin, 0, 0, progress)?;
    let base = PathBuf::from(dir).join(format!("{}_ali", stem(d)));
    let mrc = base.with_extension("mrc").to_string_lossy().into_owned();
    super::output::write_mrc(&mrc, &data, [w, h, angles.len()], px * bin as f64, false)?;
    let tlt = base.with_extension("tlt");
    std::fs::write(
        &tlt,
        angles
            .iter()
            .map(|t| format!("{t:.2}\n"))
            .collect::<String>(),
    )?;
    Ok(vec![mrc, tlt.to_string_lossy().into_owned()])
}

/// RELION 5's view of one tilt: (ZRot °, X and Y specimen shifts in Å).
pub fn relion_pose(a: &Alignment, v: &View, pixel_a: f64) -> [f64; 3] {
    let x = xf_row(a, v);
    let z = (-x[2]).atan2(x[0]).to_degrees();
    // −A⁻¹·D, with A = R/m: our stored shift, in Å.
    [z, v.shift[0] * pixel_a, v.shift[1] * pixel_a]
}

/// A STAR loop: `data_<block>`, the labels, the rows.
fn star(block: &str, labels: &[&str], rows: &[Vec<String>]) -> String {
    let mut s = format!("# version 50001\n\ndata_{block}\n\nloop_\n");
    for (i, l) in labels.iter().enumerate() {
        let _ = writeln!(s, "_{l} #{}", i + 1);
    }
    for r in rows {
        s.push_str(&r.join("  "));
        s.push('\n');
    }
    s.push('\n');
    s
}

pub fn relion5(
    d: &Dataset,
    dir: &str,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Vec<String>> {
    let Some(a) = d.alignment.as_ref() else {
        bail!("先完成倾转对齐")
    };
    let Some(px) = d.optics.pixel_a else {
        bail!("先在 CTF 一步填上像素大小")
    };
    let name = stem(d);
    let root = PathBuf::from(dir).join(format!("{name}_relion5"));
    std::fs::create_dir_all(root.join("frames"))?;
    std::fs::create_dir_all(root.join("tilt_series"))?;
    let sources: Vec<(&str, String)> = std::iter::once(("", d.stack.clone()))
        .chain(d.even.clone().map(|p| ("_even", p)))
        .chain(d.odd.clone().map(|p| ("_odd", p)))
        .filter(|(_, p)| Path::new(p).exists())
        .collect();
    let kept: Vec<&View> = by_section(a).into_iter().filter(|v| !v.excluded).collect();
    let frame = |v: &View, suffix: &str| {
        format!(
            "frames/{name}_{:03}_{:.1}{suffix}.mrc",
            v.file_index, v.angle
        )
    };
    let total = kept.len() * sources.len();
    let mut done = 0;
    for (suffix, path) in &sources {
        let img = super::pipeline::open(path)?;
        let info = img.info();
        for v in &kept {
            super::check_cancel()?;
            let (page, s) = info.locate(0, v.file_index, 0);
            let p = img.page(page)?;
            let p = &p[s.min(p.len() - 1)];
            let data: Vec<f32> = (0..p.w * p.h).map(|i| p.data.get_f64(i) as f32).collect();
            super::output::write_mrc(
                &root.join(frame(v, suffix)).to_string_lossy(),
                &data,
                [p.w, p.h, 1],
                px,
                false,
            )?;
            done += 1;
            progress(done, total);
        }
    }
    let halves = sources.len() == 3;
    let mut labels = vec![
        "rlnMicrographName",
        "rlnTomoNominalStageTiltAngle",
        "rlnTomoNominalTiltAxisAngle",
        "rlnMicrographPreExposure",
        "rlnDefocusU",
        "rlnDefocusV",
        "rlnDefocusAngle",
        "rlnCtfMaxResolution",
        "rlnTomoXTilt",
        "rlnTomoYTilt",
        "rlnTomoZRot",
        "rlnTomoXShiftAngst",
        "rlnTomoYShiftAngst",
    ];
    if halves {
        labels.extend(["rlnMicrographNameEven", "rlnMicrographNameOdd"]);
    }
    let rows: Vec<Vec<String>> = kept
        .iter()
        .map(|v| {
            let [z, sx, sy] = relion_pose(a, v, px);
            let ctf = d
                .ctf
                .as_ref()
                .and_then(|c| c.tilts.iter().find(|f| f.z == v.file_index));
            let (du, dv, da, res) =
                ctf.map_or((0.0, 0.0, 0.0, 0.0), |f| (f.df1, f.df2, f.angle, f.fit_res));
            let mut r = vec![
                frame(v, ""),
                format!("{:.3}", v.angle),
                format!("{z:.3}"),
                // The dataset keeps no dose; RELION's dose weighting then does nothing.
                "0.000".into(),
                format!("{du:.1}"),
                format!("{dv:.1}"),
                format!("{da:.2}"),
                format!("{res:.2}"),
                "0.000".into(),
                format!("{:.3}", v.refined()),
                format!("{z:.3}"),
                format!("{sx:.2}"),
                format!("{sy:.2}"),
            ];
            if halves {
                r.extend([frame(v, "_even"), frame(v, "_odd")]);
            }
            r
        })
        .collect();
    let series = root.join("tilt_series").join(format!("{name}.star"));
    std::fs::write(&series, star(&name, &labels, &rows))?;
    let img = super::pipeline::open(&d.stack)?;
    let (w, h) = (img.info().width, img.info().height);
    let depth = d
        .tomograms
        .iter()
        .rev()
        .find(|t| t.kind == "full")
        .map_or(w.min(h) / 4, |t| t.thickness * t.bin);
    let global = star(
        "global",
        &[
            "rlnTomoName",
            "rlnTomoTiltSeriesStarFile",
            "rlnVoltage",
            "rlnSphericalAberration",
            "rlnAmplitudeContrast",
            "rlnMicrographOriginalPixelSize",
            "rlnTomoHand",
            "rlnOpticsGroupName",
            "rlnTomoTiltSeriesPixelSize",
            "rlnTomoSizeX",
            "rlnTomoSizeY",
            "rlnTomoSizeZ",
        ],
        &[vec![
            name.clone(),
            format!("tilt_series/{name}.star"),
            format!("{:.3}", d.optics.kv),
            format!("{:.3}", d.optics.cs_mm),
            format!("{:.3}", d.optics.amplitude),
            format!("{px:.4}"),
            "-1".into(),
            "optics1".into(),
            format!("{px:.4}"),
            w.to_string(),
            h.to_string(),
            depth.to_string(),
        ]],
    );
    let top = root.join("tilt_series.star");
    std::fs::write(&top, global)?;
    Ok(vec![
        top.to_string_lossy().into_owned(),
        series.to_string_lossy().into_owned(),
    ])
}

pub fn run(
    d: &Dataset,
    format: &str,
    dir: &str,
    bin: usize,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Vec<String>> {
    match format {
        "relion5" => relion5(d, dir, progress),
        "imod" => imod(d, dir),
        "aligned" => aligned(d, dir, bin, progress),
        "table" => table(d, dir),
        _ => bail!("不支持的导出格式：{format}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn imod_lists() {
        assert_eq!(super::ranges(vec![5, 0, 3, 4]), "1,4-6");
        assert_eq!(super::ranges(vec![]), "");
    }
}
