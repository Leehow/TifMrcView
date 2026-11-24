//! Leica acquisition folders as stacks (`tmv_stack`): what ties a folder of
//! LAS X exports to the project they came from.
//!
//! * A LIF/LOF next to TIFFs exported from it (`Project_Series002_z000_ch00.tif`,
//!   named `<project>_<image path with / as _>_z…`): every image of the
//!   project is a series, listing its exported files, and the app opens the
//!   container itself (`open: "."`) at that image, with the original bit
//!   depth, channels and calibration.
//! * Only the exports and their `MetaData/<prefix>.xml` (LAS X writes one per
//!   exported image, the image's XML as in the LIF): the TIFFs become a Z
//!   stack with the pixel size, Z step and acquisition settings of the XML;
//!   each channel (or time point) exported separately is a series.
//! * An `.xlef` / `.xlif` project (LAS X 4+): its images are `.lof` files in
//!   the folders below; each is a series, opened as it is.

use tifmrcview_plugin_sdk::xml;
use tifmrcview_plugin_sdk::{bail, file, Group, Result, Stack, StackSeries, Stacks};

use crate::{container, groups, images, Container, Hardware, Kind, Layout, Img, TileInfo};

pub struct LasxStacks;

impl Stacks for LasxStacks {
    fn stack(path: &str, listing: &[&str], series: u32) -> Result<Stack> {
        let name = file_name(path);
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".xml") {
            export_stack(&name, listing, series)
        } else if lower.ends_with(".xlef") || lower.ends_with(".xlif") {
            lof_stack(&name, listing, series)
        } else {
            container_stack(&name, listing, series)
        }
    }
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn is_tiff(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".tif") || l.ends_with(".tiff")
}

fn strip_ext(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |s| s.0)
}

/// TIFFs directly in the folder whose names start with `prefix` + `_`.
fn exported<'a>(listing: &[&'a str], prefix: &str) -> Vec<&'a str> {
    let p = format!("{}_", prefix.to_lowercase());
    listing.iter().copied().filter(|f| !f.contains('/') && is_tiff(f) && f.to_lowercase().starts_with(&p)).collect()
}

/// The Z number of an exported file (`…_z012_…`), if it has one.
fn z_of(name: &str) -> Option<u32> {
    strip_ext(name).split('_').find_map(|t| {
        let d = t.strip_prefix('z').or_else(|| t.strip_prefix('Z'))?;
        (!d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())).then(|| d.parse().ok()).flatten()
    })
}

/// Exported files of one image, grouped by everything but the Z number
/// (one group per exported channel, time point…), Z order inside.
fn runs<'a>(files: &[&'a str], prefix: &str) -> Vec<(String, Vec<&'a str>)> {
    let mut out: Vec<(String, Vec<(u32, &'a str)>)> = vec![];
    for &f in files {
        let Some(z) = z_of(f) else { continue };
        let rest = &strip_ext(f)[prefix.len().min(f.len())..];
        let key: Vec<&str> = rest.split('_').filter(|t| !t.is_empty() && z_of(t).is_none()).collect();
        let key = key.join(" ");
        match out.iter_mut().find(|r| r.0 == key) {
            Some(r) => r.1.push((z, f)),
            None => out.push((key, vec![(z, f)])),
        }
    }
    out.into_iter()
        .filter(|r| r.1.len() >= 2)
        .map(|(k, mut v)| {
            v.sort_by_key(|x| x.0);
            (k, v.into_iter().map(|x| x.1).collect())
        })
        .collect()
}

// ------------------------------------------------------------------ LIF / LOF

fn container_stack(name: &str, listing: &[&str], series: u32) -> Result<Stack> {
    let c = container()?;
    let doc = xml::Doc::parse(&c.xml);
    let imgs = images(&doc, c.lof_block.map(|b| b.1));
    if imgs.is_empty() {
        bail!("这个文件里没有带像素的图像");
    }
    let stem = strip_ext(name);
    let lasts: Vec<&str> = imgs.iter().map(|i| i.path.rsplit('/').next().unwrap_or(&i.path)).collect();
    let mut list = vec![];
    for (k, img) in imgs.iter().enumerate() {
        // LAS X names exports after the project and the image's path.
        let full = format!("{stem}_{}", img.path.replace('/', "_"));
        let mut files = exported(listing, &full);
        if files.is_empty() && lasts.iter().filter(|l| **l == lasts[k]).count() == 1 && img.path.contains('/') {
            files = exported(listing, &format!("{stem}_{}", lasts[k]));
        }
        if files.is_empty() && c.kind == Kind::Lof && imgs.len() == 1 {
            files = exported(listing, stem);
        }
        let detail = Layout::parse(img.data).map_or_else(|e| e.0, |l| l.detail());
        list.push(StackSeries { name: img.path.clone(), detail, dir: String::new(), files: files.iter().map(|f| f.to_string()).collect() });
    }
    // The default is the first image that was exported here.
    let pick = if series == 0 { list.iter().position(|s| !s.files.is_empty()).unwrap_or(0) } else { (series as usize).min(list.len() - 1) };
    Ok(Stack {
        files: list[pick].files.clone(),
        format: Some("Leica LAS X".into()),
        open: Some(".".into()),
        open_series: pick as u32,
        series: list,
        series_index: pick as u32,
        ..Default::default()
    })
}

// ------------------------------------------------------------------ TIFF exports + MetaData

/// The file as text: UTF-8 (with or without BOM) or UTF-16 with BOM.
fn text(bytes: &[u8]) -> Vec<u8> {
    match bytes {
        [0xff, 0xfe, rest @ ..] => {
            let u: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&u).into_bytes()
        }
        [0xef, 0xbb, 0xbf, rest @ ..] => rest.to_vec(),
        _ => bytes.to_vec(),
    }
}

fn export_stack(name: &str, listing: &[&str], series: u32) -> Result<Stack> {
    let stem = strip_ext(name);
    // "Project.lif_Series002.xml" and "Project_Series002.xml" describe the
    // exports "Project_Series002_z…".
    let prefix = match stem.split_once(".lif_").or_else(|| stem.split_once(".LIF_")) {
        Some((p, s)) => format!("{p}_{s}"),
        None => stem.to_string(),
    };
    let size = file::size();
    if size > 64 << 20 {
        bail!("LAS X 元数据文件太大");
    }
    let xml_bytes = text(&file::read(0, size as usize)?);
    let doc = xml::Doc::parse(&xml_bytes);
    let imgs = images(&doc, None);
    let Some(img) = imgs.into_iter().next() else { bail!("不是 LAS X 导出的图像描述：{name}") };
    let layout = Layout::parse(img.data)?;

    let files = exported(listing, &prefix);
    let runs = runs(&files, &prefix);
    if runs.is_empty() {
        bail!("文件夹里没有 {prefix} 导出的 Z 序列");
    }
    let pick = (series as usize).min(runs.len() - 1);
    let series_name = prefix.rsplit_once('_').map_or(prefix.as_str(), |s| s.1).to_string();
    let img = Img { path: series_name.clone(), data: img.data, mem_id: String::new() };
    let c = Container { kind: Kind::Export, xml: xml_bytes.clone(), blocks: 0, lof_block: None };
    let hw = Hardware::new(img.data);
    let tinfo = TileInfo::new(&layout, img.data);
    let (px, py, pz) = (layout.x.step_um(), layout.y.step_um(), layout.z.as_ref().and_then(|d| d.step_um()));
    let mut metadata = groups(&c, &layout, &img, hw.as_ref(), &tinfo, (layout.w, layout.h), (px, py, pz));
    let mut g = Group::new("LAS X 导出");
    g.add_str("元数据文件", &format!("MetaData/{name}"));
    g.add_str("原始图像", &format!("{series_name}（{}）", layout.detail()));
    g.add_str("导出文件", &format!("{} 张（{}）", runs[pick].1.len(), if runs[pick].0.is_empty() { "全部" } else { &runs[pick].0 }));
    metadata.insert(0, g);
    let mut out = Stack {
        files: runs[pick].1.iter().map(|f| f.to_string()).collect(),
        format: Some("Leica LAS X 导出".into()),
        z_spacing: pz,
        pixel_size: px.map(|x| [x, py.unwrap_or(x)]),
        unit: "µm".into(),
        metadata,
        ..Default::default()
    };
    out.series = runs
        .iter()
        .map(|(k, f)| StackSeries {
            name: if k.is_empty() { series_name.clone() } else { format!("{series_name} · {k}") },
            detail: format!("{} 层", f.len()),
            dir: String::new(),
            files: f.iter().map(|x| x.to_string()).collect(),
        })
        .collect();
    out.series_index = pick as u32;
    Ok(out)
}

// ------------------------------------------------------------------ XLEF / XLIF

fn lof_stack(name: &str, listing: &[&str], series: u32) -> Result<Stack> {
    let stem = strip_ext(name).to_lowercase();
    let mut lofs: Vec<&str> = listing.iter().copied().filter(|f| f.to_ascii_lowercase().ends_with(".lof")).collect();
    if lofs.is_empty() {
        bail!("LAS X 项目文件夹里没有找到 .lof 图像");
    }
    // An .xlif first offers the image of the same name.
    if let Some(i) = lofs.iter().position(|f| strip_ext(&file_name(f)).to_lowercase() == stem) {
        let f = lofs.remove(i);
        lofs.insert(0, f);
    }
    let pick = (series as usize).min(lofs.len() - 1);
    Ok(Stack {
        format: Some("Leica LAS X".into()),
        open: Some(lofs[pick].to_string()),
        open_series: 0,
        series: lofs
            .iter()
            .map(|f| StackSeries { name: strip_ext(f).to_string(), detail: "LOF".into(), dir: String::new(), files: vec![] })
            .collect(),
        series_index: pick as u32,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_group_by_channel() {
        let l = [
            "MetaData/Project_Series002.xml",
            "Project.lif",
            "Project_Series002_z000_ch00_SV.tif",
            "Project_Series002_z000_ch01_SV.tif",
            "Project_Series002_z001_ch00_SV.tif",
            "Project_Series002_z001_ch01_SV.tif",
            "Project_Series002_z010_ch00_SV.tif",
            "Project_Series002_z010_ch01_SV.tif",
            "Project_Series0020_z000_ch00.tif",
            "Other_z000.tif",
        ];
        let files = exported(&l, "Project_Series002");
        assert_eq!(files.len(), 6);
        let r = runs(&files, "Project_Series002");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, "ch00 SV");
        assert_eq!(r[0].1, ["Project_Series002_z000_ch00_SV.tif", "Project_Series002_z001_ch00_SV.tif", "Project_Series002_z010_ch00_SV.tif"]);
        assert_eq!(z_of("a_z012_ch00.tif"), Some(12));
        assert_eq!(z_of("a_zone.tif"), None);
    }
}
