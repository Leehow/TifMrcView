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
//! - "table": `<name>_cryoet.csv`, one row per view with everything the
//!   dataset knows about it, for scripts and spreadsheets.

use super::{dataset::Dataset, geometry, Alignment, View};
use anyhow::{bail, Result};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// One `.xf` row: the transform taking the raw view to the aligned one.
pub fn xf_row(a: &Alignment, v: &View) -> [f64; 6] {
    let r = geometry::rotation(a.axis + v.rotation);
    let m = [r[0][0] / v.mag, r[1][0] / v.mag, r[0][1] / v.mag, r[1][1] / v.mag];
    [m[0], m[1], m[2], m[3], -m[0] * v.shift[0] - m[1] * v.shift[1], -m[2] * v.shift[0] - m[3] * v.shift[1]]
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
        out.push(if i == j { format!("{}", v[i] + 1) } else { format!("{}-{}", v[i] + 1, v[j] + 1) });
        i = j + 1;
    }
    out.join(",")
}

fn stem(d: &Dataset) -> String {
    Path::new(&d.stack).file_stem().and_then(|s| s.to_str()).unwrap_or("stack").to_string()
}

/// Views of the alignment in stack order.
fn by_section(a: &Alignment) -> Vec<&View> {
    let mut v: Vec<&View> = a.views.iter().collect();
    v.sort_by_key(|v| v.file_index);
    v
}

pub fn imod(d: &Dataset, dir: &str) -> Result<Vec<String>> {
    let Some(a) = d.alignment.as_ref() else { bail!("先完成倾转对齐") };
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
        writeln!(xf, "{:12.7} {:12.7} {:12.7} {:12.7} {:12.3} {:12.3}", x[0], x[1], x[2], x[3], x[4], x[5])?;
        writeln!(tlt, "{:.2}", v.refined())?;
        writeln!(raw, "{:.2}", v.angle)?;
    }
    put(file("xf"), xf)?;
    put(file("tlt"), tlt)?;
    put(file("rawtlt"), raw)?;
    let excluded: Vec<usize> = views.iter().filter(|v| v.excluded).map(|v| v.file_index).collect();
    put(folder.join(format!("{name}_exclude.txt")), format!("{}\n", ranges(excluded)))?;
    if let Some(ctf) = &d.ctf {
        // Version 3: a header line, then per view
        // start end tilt-start tilt-end defocus1 defocus2 (nm) astigmatism angle (°).
        let mut text = String::from("1  0 0. 0. 0  3\n");
        for f in &ctf.tilts {
            let t = d.angles.get(f.z).copied().unwrap_or(0.0);
            writeln!(text, "{}  {}  {:.2}  {:.2}  {:.1}  {:.1}  {:.2}", f.z + 1, f.z + 1, t, t, f.df1 / 10.0, f.df2 / 10.0, f.angle)?;
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
    let Some(a) = d.alignment.as_ref() else { bail!("先完成倾转对齐") };
    let px = d.optics.pixel_a.unwrap_or(1.0);
    let mut text = String::from("section,tilt_nominal_deg,tilt_refined_deg,excluded,shift_x_px,shift_y_px,shift_x_A,shift_y_A,axis_deg,rotation_deg,magnification,residual_px,defocus1_A,defocus2_A,astigmatism_angle_deg,ctf_fit_resolution_A\n");
    for v in by_section(a) {
        let ctf = d.ctf.as_ref().and_then(|c| c.tilts.iter().find(|f| f.z == v.file_index));
        let c = |f: fn(&super::dataset::TiltCtf) -> f64| ctf.map(|t| format!("{:.1}", f(t))).unwrap_or_default();
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

pub fn aligned(d: &Dataset, dir: &str, bin: usize, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Vec<String>> {
    let Some(a) = d.alignment.as_ref() else { bail!("先完成倾转对齐") };
    let img = super::pipeline::open(&d.stack)?;
    let px = d.optics.pixel_a.unwrap_or_else(|| super::output::pixel_a(&img));
    let (data, w, h, angles) = super::output::aligned(&img, a, bin, 0, 0, progress)?;
    let base = PathBuf::from(dir).join(format!("{}_ali", stem(d)));
    let mrc = base.with_extension("mrc").to_string_lossy().into_owned();
    super::output::write_mrc(&mrc, &data, [w, h, angles.len()], px * bin as f64, false)?;
    let tlt = base.with_extension("tlt");
    std::fs::write(&tlt, angles.iter().map(|t| format!("{t:.2}\n")).collect::<String>())?;
    Ok(vec![mrc, tlt.to_string_lossy().into_owned()])
}

pub fn run(d: &Dataset, format: &str, dir: &str, bin: usize, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Vec<String>> {
    match format {
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
