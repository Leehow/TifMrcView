//! Auto Slice & View projects (FIB-SEM serial sectioning on Helios, Scios,
//! Aquilos… DualBeams): `ProjectData.dat` in the project folder, the slice
//! images in subfolders below it.
//!
//! `ProjectData.dat` is XML. What is used here is what public tools read
//! from it: `Name`, `TimeCreated` and `ExecutionResult` under the root, and
//! one `Slice` element per milled slice under `Results`, with the target
//! and measured thickness in metres (`TargetThickness`, `MeasuredThickness`,
//! `NaN` when not measured). Other simple elements are listed as they are.
//!
//! The slices are the TIFFs written into the project's subfolders, one
//! folder per detector or image type (`… - SliceImage - 001.tif`); the
//! `LogImages` folder holds alignment snapshots and is left out. Each
//! folder with a run of alike-named TIFFs is one stack; with several, the
//! one with the most slices comes first. Pixel sizes come from the FEI tags
//! of the slices themselves ([`crate::feitiff`]).

use tifmrcview_plugin_sdk::{bail, file, xml, Group, Result, Stack, StackSeries};

use crate::show_length;

/// The file as text: UTF-8 (with or without BOM) or UTF-16 with BOM.
fn text(bytes: &[u8]) -> String {
    match bytes {
        [0xff, 0xfe, rest @ ..] => {
            let u: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&u)
        }
        [0xfe, 0xff, rest @ ..] => {
            let u: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&u)
        }
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// "001" style digit runs replaced by `#`, lower-cased.
fn pattern(name: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    for c in name.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('#');
            }
            digits = true;
        } else {
            out.extend(c.to_lowercase());
            digits = false;
        }
    }
    out
}

fn is_tiff(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".tif") || l.ends_with(".tiff")
}

/// One run of slices: its folder, file-name pattern and files (in order).
struct Run<'a> {
    dir: &'a str,
    pattern: String,
    files: Vec<&'a str>,
}

fn runs<'a>(listing: &[&'a str]) -> Vec<Run<'a>> {
    let mut runs: Vec<Run<'a>> = Vec::new();
    for &f in listing {
        let (dir, name) = f.rsplit_once('/').unwrap_or(("", f));
        if !is_tiff(name) || dir.is_empty() {
            continue;
        }
        if dir.split('/').any(|d| d.eq_ignore_ascii_case("LogImages")) {
            continue;
        }
        let p = pattern(name);
        match runs.iter_mut().find(|r| r.dir == dir && r.pattern == p) {
            Some(r) => r.files.push(f),
            None => runs.push(Run { dir, pattern: p, files: vec![f] }),
        }
    }
    runs.retain(|r| r.files.len() >= 2);
    // The listing is in natural order already. Biggest run first; ties
    // keep the listing order.
    runs.sort_by(|a, b| b.files.len().cmp(&a.files.len()));
    runs
}

struct Thickness {
    target: Vec<f64>,
    measured: Vec<f64>,
    slices: usize,
}

fn mean_sd(v: &[f64]) -> Option<(f64, f64)> {
    if v.is_empty() {
        return None;
    }
    let n = v.len() as f64;
    let m = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n;
    Some((m, var.sqrt()))
}

pub fn stack(listing: &[&str], series: u32) -> Result<Stack> {
    let src = text(&file::read(0, file::size() as usize)?);
    let doc = xml::Doc::parse(src.as_bytes());
    let Some(root) = doc.root() else { bail!("ProjectData.dat 不是 XML 文件") };

    let mut project = Group::new("Auto Slice & View 项目");
    let mut th = Thickness { target: Vec::new(), measured: Vec::new(), slices: 0 };
    for c in root.children() {
        let name = c.name();
        if c.has_tag_name("Results") {
            for s in c.children().filter(|s| s.has_tag_name("Slice")) {
                th.slices += 1;
                let num = |k: &str| s.attribute(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0);
                if let Some(t) = num("TargetThickness") {
                    th.target.push(t);
                }
                if let Some(m) = num("MeasuredThickness") {
                    th.measured.push(m);
                }
            }
        } else if c.children().next().is_none() {
            if let Some(t) = c.text().filter(|t| !t.trim().is_empty()) {
                let label = match name.as_ref() {
                    "Name" => "项目名称",
                    "TimeCreated" => "创建时间",
                    "ExecutionResult" => "运行结果",
                    other => other,
                };
                project.push(label, t.trim());
            }
        }
    }
    if th.slices > 0 {
        project.push("切片数（项目记录）", &th.slices.to_string());
    }
    let target = mean_sd(&th.target);
    let measured = mean_sd(&th.measured);
    if let Some((m, sd)) = target {
        let range = if sd > m * 1e-3 { format!("（平均，±{}）", show_length(sd)) } else { String::new() };
        project.push("目标切片厚度", &(show_length(m) + &range));
    }
    if let Some((m, sd)) = measured {
        project.push(
            "实测切片厚度",
            &format!("{} ± {}（{} 片的平均）", show_length(m), show_length(sd), th.measured.len()),
        );
        let total: f64 = th.measured.iter().sum();
        project.push("实测总深度", &show_length(total));
    }

    let runs = runs(listing);
    if runs.is_empty() {
        bail!("Auto Slice & View 项目文件夹里没有找到切片图像（应在项目文件夹下的子文件夹里）");
    }
    let i = (series as usize).min(runs.len() - 1);
    let run = &runs[i];
    let mut out = Stack {
        files: run.files.iter().map(|f| f.to_string()).collect(),
        format: Some("Thermo Auto Slice & View".into()),
        ..Default::default()
    };
    // Nominal spacing: even, as the slices are shown; the measured
    // thicknesses are listed for reference.
    if let Some((m, _)) = target.or(measured) {
        out.z_spacing = Some(crate::tidy(m * 1e9));
        out.unit = "nm".into();
    }
    if th.slices > 0 && th.slices != run.files.len() {
        project.push("说明", &format!("项目记录 {} 片，文件夹里有 {} 张图像", th.slices, run.files.len()));
    }
    project.push("图像文件夹", run.dir);
    project.push_to(&mut out.metadata);
    if runs.len() > 1 {
        out.series = runs
            .iter()
            .map(|r| StackSeries {
                name: r.dir.rsplit('/').next().unwrap_or(r.dir).to_string(),
                detail: format!("{} 张 · {}", r.files.len(), r.pattern.replace('#', "N")),
                dir: r.dir.to_string(),
            })
            .collect();
        out.series_index = i as u32;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_skip_logs_and_singles() {
        let l = [
            "LogImages/EBeam_001.tif",
            "LogImages/EBeam_002.tif",
            "ProjectData.dat",
            "Images/SEM Image/run - SliceImage - 001.tif",
            "Images/SEM Image/run - SliceImage - 002.tif",
            "Images/SEM Image/run - SliceImage - 010.tif",
            "Images/SEM Image 2/run - SliceImage - 001.tif",
            "Images/SEM Image 2/run - SliceImage - 002.tif",
            "Images/overview.tif",
        ];
        let r = runs(&l);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].dir, "Images/SEM Image");
        assert_eq!(r[0].files.len(), 3);
        assert_eq!(r[1].dir, "Images/SEM Image 2");
    }

    #[test]
    fn text_handles_boms() {
        assert_eq!(text(b"\xef\xbb\xbf<a/>"), "<a/>");
        assert_eq!(text(b"\xff\xfe<\0a\0/\0>\0"), "<a/>");
        assert_eq!(text(b"<a/>"), "<a/>");
    }
}
