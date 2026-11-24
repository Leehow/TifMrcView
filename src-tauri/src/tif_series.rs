//! Raw TIFF export of a range of slices, cropped to a region: the recorder
//! plugin's "导出 TIF" next to its MP4. Pixel values are written as stored
//! (no contrast, no overlays), every channel kept, so the result can be
//! analysed further in Fiji / ImageJ.
//!
//! Two layouts: one multi-page TIFF (ImageJ hyperstack, channel fastest),
//! or a folder with one TIFF per slice (its channels as pages).
use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use tiff::encoder::{colortype, ImageEncoder, Rational, TiffEncoder, TiffKind};
use tiff::tags::{ResolutionUnit, Tag};

use crate::image::{ImageInfo, OpenImage, Plane, PlaneData};

#[derive(Deserialize, Debug, Clone)]
pub struct Request {
    pub token: u64,
    /// First and last slider index, both included; `from > to` runs backwards.
    pub from: usize,
    pub to: usize,
    pub t: usize,
    /// [x, y, w, h] in image pixels.
    pub region: [usize; 4],
    /// The chosen `.tif`; for `sequence` the folder is named after it.
    pub path: String,
    /// One file per slice inside a folder instead of one multi-page file.
    #[serde(default)]
    pub sequence: bool,
}

#[derive(serde::Serialize, Debug)]
pub struct Written {
    /// The file, or the folder for a sequence.
    pub path: String,
    pub files: usize,
    pub slices: usize,
    pub channels: usize,
    pub bytes: u64,
}

/// Writes the slices `from..=to` of `img`, cropped to `region`.
pub fn export(img: &OpenImage, req: &Request) -> Result<Written> {
    let info = img.info();
    let [x, y, w, h] = req.region;
    if w == 0 || h == 0 || x + w > info.width || y + h > info.height {
        bail!("导出区域超出图像：{:?}，图像 {}×{}", req.region, info.width, info.height);
    }
    if req.from >= info.size_z || req.to >= info.size_z {
        bail!("层号超出范围（共 {} 层）", info.size_z);
    }
    let slices: Vec<usize> = if req.from <= req.to {
        (req.from..=req.to).collect()
    } else {
        (req.to..=req.from).rev().collect()
    };
    let channels = info.size_c.max(1);
    let read = |z: usize, c: usize| -> Result<PlaneData> {
        let (page, sample) = info.locate(c, z, req.t);
        let planes = img.page(page)?;
        let plane = &planes[sample.min(planes.len() - 1)];
        Ok(crop(plane, req.region))
    };
    let path = PathBuf::from(&req.path);
    if !req.sequence {
        let desc = imagej_description(info, slices.len(), channels);
        let mut enc = encoder(&path)?;
        for (k, &z) in slices.iter().enumerate() {
            for c in 0..channels {
                let first = k == 0 && c == 0;
                write_page(&mut enc, w, h, &read(z, c)?, info, first.then_some(desc.as_str()))?;
            }
        }
        drop(enc);
        let bytes = std::fs::metadata(&path)?.len();
        return Ok(Written { path: req.path.clone(), files: 1, slices: slices.len(), channels, bytes });
    }
    let dir = path.with_extension("");
    std::fs::create_dir_all(&dir).with_context(|| format!("无法建文件夹 {}", dir.display()))?;
    let stem = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "slice".into());
    let digits = (slices.iter().max().copied().unwrap_or(0) + 1).to_string().len().max(3);
    let desc = imagej_description(info, 1, channels);
    let mut bytes = 0;
    for &z in &slices {
        let file = dir.join(format!("{stem}_{:0digits$}.tif", z + 1));
        let mut enc = encoder(&file)?;
        for c in 0..channels {
            write_page(&mut enc, w, h, &read(z, c)?, info, (c == 0).then_some(desc.as_str()))?;
        }
        drop(enc);
        bytes += std::fs::metadata(&file)?.len();
    }
    Ok(Written { path: dir.to_string_lossy().into_owned(), files: slices.len(), slices: slices.len(), channels, bytes })
}

fn encoder(path: &Path) -> Result<TiffEncoder<BufWriter<File>>> {
    let file = File::create(path).with_context(|| format!("无法写入 {}", path.display()))?;
    Ok(TiffEncoder::new(BufWriter::new(file))?)
}

/// The rows `y..y+h`, columns `x..x+w` of a plane.
fn crop(plane: &Plane, [x, y, w, h]: [usize; 4]) -> PlaneData {
    fn rows<T: Copy>(v: &[T], stride: usize, [x, y, w, h]: [usize; 4]) -> Vec<T> {
        let mut out = Vec::with_capacity(w * h);
        for r in y..y + h {
            out.extend_from_slice(&v[r * stride + x..r * stride + x + w]);
        }
        out
    }
    let r = [x, y, w, h];
    match &plane.data {
        PlaneData::U8(v) => PlaneData::U8(rows(v, plane.w, r)),
        PlaneData::I8(v) => PlaneData::I8(rows(v, plane.w, r)),
        PlaneData::U16(v) => PlaneData::U16(rows(v, plane.w, r)),
        PlaneData::I16(v) => PlaneData::I16(rows(v, plane.w, r)),
        PlaneData::U32(v) => PlaneData::U32(rows(v, plane.w, r)),
        PlaneData::I32(v) => PlaneData::I32(rows(v, plane.w, r)),
        PlaneData::F32(v) => PlaneData::F32(rows(v, plane.w, r)),
        PlaneData::F64(v) => PlaneData::F64(rows(v, plane.w, r)),
    }
}

/// ImageJ reads this as a hyperstack with the file's pixel size and Z step.
fn imagej_description(info: &ImageInfo, slices: usize, channels: usize) -> String {
    let mut d = format!("ImageJ=1.11a\nimages={}\n", slices * channels);
    if channels > 1 {
        d += &format!("channels={channels}\n");
    }
    if slices > 1 {
        d += &format!("slices={slices}\n");
    }
    if channels > 1 && slices > 1 {
        d += "hyperstack=true\nmode=composite\n";
    }
    if let Some(unit) = unit_name(info) {
        d += &format!("unit={unit}\n");
        if let Some([_, _, z]) = info.pixel_size.filter(|p| p[2] > 0.0) {
            d += &format!("spacing={z}\n");
        }
    }
    d
}

fn unit_name(info: &ImageInfo) -> Option<&str> {
    info.pixel_size.filter(|p| p[0] > 0.0)?;
    Some(match info.unit.as_str() {
        "µm" | "μm" | "um" => "micron",
        "" => return None,
        u => u,
    })
}

fn write_page<W: Write + Seek>(
    enc: &mut TiffEncoder<W>,
    w: usize,
    h: usize,
    data: &PlaneData,
    info: &ImageInfo,
    desc: Option<&str>,
) -> Result<()> {
    fn put<W: Write + Seek, C: colortype::ColorType, K: TiffKind>(
        mut img: ImageEncoder<'_, W, C, K>,
        data: &[C::Inner],
        info: &ImageInfo,
        desc: Option<&str>,
    ) -> Result<()>
    where
        [C::Inner]: tiff::encoder::TiffValue,
    {
        if let Some(d) = desc {
            img.encoder().write_tag(Tag::ImageDescription, d)?;
            img.encoder().write_tag(Tag::Software, "TifMrcView")?;
        }
        if let (Some([px, py, _]), Some(_)) = (info.pixel_size, unit_name(info)) {
            // Pixels per unit, as ImageJ expects (unit in the description).
            let per = |v: f64| Rational { n: (1e6 / v.max(1e-12)).round().min(u32::MAX as f64) as u32, d: 1_000_000 };
            img.x_resolution(per(px));
            img.y_resolution(per(if py > 0.0 { py } else { px }));
            img.resolution_unit(ResolutionUnit::None);
        }
        img.write_data(data)?;
        Ok(())
    }
    let (w, h) = (w as u32, h as u32);
    match data {
        PlaneData::U8(v) => put(enc.new_image::<colortype::Gray8>(w, h)?, v, info, desc),
        PlaneData::I8(v) => put(enc.new_image::<colortype::GrayI8>(w, h)?, v, info, desc),
        PlaneData::U16(v) => put(enc.new_image::<colortype::Gray16>(w, h)?, v, info, desc),
        PlaneData::I16(v) => put(enc.new_image::<colortype::GrayI16>(w, h)?, v, info, desc),
        PlaneData::U32(v) => put(enc.new_image::<colortype::Gray32>(w, h)?, v, info, desc),
        PlaneData::I32(v) => put(enc.new_image::<colortype::GrayI32>(w, h)?, v, info, desc),
        PlaneData::F32(v) => put(enc.new_image::<colortype::Gray32Float>(w, h)?, v, info, desc),
        PlaneData::F64(v) => put(enc.new_image::<colortype::Gray64Float>(w, h)?, v, info, desc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::tests::{fake_info, FakeSource};
    use crate::image::DType;

    /// 4x3, two channels, three slices: value = 100c + 10z + pixel index.
    fn volume() -> std::sync::Arc<OpenImage> {
        let mut info = fake_info(4, 3, 2, 3, 1, DType::U8);
        info.pixel_size = Some([0.5, 0.5, 2.0]);
        info.unit = "µm".into();
        let mut pages = vec![];
        for z in 0..3 {
            for c in 0..2 {
                pages.push(vec![(0..12).map(|i| (100 * c + 10 * z + i) as u8).collect()]);
            }
        }
        let (src, _) = FakeSource::new(info, pages);
        OpenImage::new(1, Box::new(src), 1 << 20)
    }

    fn read_back(path: &Path) -> (ImageInfo, Vec<Vec<f64>>) {
        let img = OpenImage::new(2, crate::formats::open(path).unwrap(), 1 << 20);
        let info = img.info().clone();
        let pages = (0..info.page_count)
            .map(|p| {
                let planes = img.page(p).unwrap();
                (0..planes[0].data.len()).map(|i| planes[0].data.get_f64(i)).collect()
            })
            .collect();
        (info, pages)
    }

    #[test]
    fn stack_keeps_raw_values_crop_and_channels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.tif");
        let req = Request { token: 1, from: 2, to: 1, t: 0, region: [1, 1, 2, 2], path: path.to_string_lossy().into(), sequence: false };
        let w = export(&volume(), &req).unwrap();
        assert_eq!((w.files, w.slices, w.channels), (1, 2, 2));
        let (info, pages) = read_back(&path);
        assert_eq!((info.width, info.height, info.size_c, info.size_z), (2, 2, 2, 2));
        // Backwards from slice 2; pixels (1,1),(2,1),(1,2),(2,2) = 5, 6, 9, 10.
        assert_eq!(pages.len(), 4);
        let z2c0 = pages.iter().find(|p| p[0] == 25.0).expect("slice 2 channel 0");
        assert_eq!(z2c0, &vec![25.0, 26.0, 29.0, 30.0]);
        assert!(pages.iter().any(|p| p[0] == 115.0), "slice 1 channel 1");
        let ps = info.pixel_size.expect("pixel size");
        assert!((ps[0] - 0.5).abs() < 1e-3 && (ps[2] - 2.0).abs() < 1e-3, "{ps:?}");
    }

    #[test]
    fn sequence_writes_one_file_per_slice_in_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cell_z1-3.tif");
        let req = Request { token: 1, from: 0, to: 2, t: 0, region: [0, 0, 4, 3], path: path.to_string_lossy().into(), sequence: true };
        let w = export(&volume(), &req).unwrap();
        assert_eq!(w.files, 3);
        let folder = dir.path().join("cell_z1-3");
        assert_eq!(w.path, folder.to_string_lossy());
        let (info, pages) = read_back(&folder.join("cell_z1-3_002.tif"));
        assert_eq!((info.width, info.height, info.size_c), (4, 3, 2));
        assert_eq!(pages[0][0], 10.0);
        assert_eq!(pages[1][0], 110.0);
    }

    #[test]
    fn rejects_a_region_outside_the_image() {
        let req = Request { token: 1, from: 0, to: 0, t: 0, region: [3, 0, 2, 1], path: "/nonexistent/x.tif".into(), sequence: false };
        assert!(export(&volume(), &req).is_err());
    }
}
