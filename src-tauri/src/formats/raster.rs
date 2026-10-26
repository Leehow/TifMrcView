//! Everyday image formats (PNG, JPEG, GIF, BMP, WebP) via the `image` crate.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{DynamicImage, ImageReader};

use crate::image::{ChannelInfo, DType, DimOrder, ImageInfo, Plane, PlaneData, Source};

pub struct RasterSource {
    path: PathBuf,
    info: ImageInfo,
}

fn reader(path: &Path) -> Result<ImageReader<std::io::BufReader<std::fs::File>>> {
    let mut r = ImageReader::open(path)
        .with_context(|| format!("无法打开 {}", path.display()))?
        .with_guessed_format()?;
    r.no_limits();
    Ok(r)
}

impl RasterSource {
    pub fn open(path: &Path) -> Result<RasterSource> {
        let r = reader(path)?;
        let format = r.format().map(|f| format!("{f:?}").to_uppercase()).unwrap_or_else(|| "IMAGE".into());
        let mut dec = r.into_decoder()?;
        let (w, h) = image::ImageDecoder::dimensions(&dec);
        let color = image::ImageDecoder::color_type(&dec);
        let _ = image::ImageDecoder::icc_profile(&mut dec);
        let rgb = color.has_color();
        let dtype = match color.bytes_per_pixel() / color.channel_count() {
            1 => DType::U8,
            2 => DType::U16,
            _ => DType::F32,
        };
        let is_rgb = rgb && dtype == DType::U8;
        let channels = if rgb {
            ["R", "G", "B"]
                .iter()
                .zip([[255, 0, 0], [0, 255, 0], [0, 0, 255]])
                .map(|(n, c)| ChannelInfo { name: n.to_string(), color: c })
                .collect()
        } else {
            vec![ChannelInfo { name: "灰度".into(), color: [255, 255, 255] }]
        };
        let size_c = channels.len();
        let info = ImageInfo {
            path: path.to_string_lossy().into_owned(),
            name: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            format,
            width: w as usize,
            height: h as usize,
            size_c,
            size_z: 1,
            size_t: 1,
            dtype,
            is_rgb,
            channels,
            pixel_size: None,
            unit: String::new(),
            metadata: vec![],
            samples_per_page: size_c,
            order: DimOrder::CZT,
            page_count: 1,
            series: vec![],
            series_index: 0,
        };
        Ok(RasterSource { path: path.to_path_buf(), info })
    }
}

impl Source for RasterSource {
    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn read_page(&self, _page: usize) -> Result<Vec<Plane>> {
        let img: DynamicImage = reader(&self.path)?.decode()?;
        let (w, h) = (self.info.width, self.info.height);
        let data = match (self.info.size_c > 1, self.info.dtype) {
            (true, DType::U8) => PlaneData::U8(img.into_rgb8().into_raw()),
            (true, DType::U16) => PlaneData::U16(img.into_rgb16().into_raw()),
            (true, _) => PlaneData::F32(img.into_rgb32f().into_raw()),
            (false, DType::U8) => PlaneData::U8(img.into_luma8().into_raw()),
            (false, DType::U16) => PlaneData::U16(img.into_luma16().into_raw()),
            (false, _) => PlaneData::F32(img.to_luma32f().into_raw()),
        };
        Ok(data
            .deinterleave(self.info.size_c)
            .into_iter()
            .map(|data| Plane { w, h, data })
            .collect())
    }
}
