//! TIFF / OME-TIFF / ImageJ hyperstack reader.
//!
//! Pages are decoded straight from a memory map. Strips and tiles of a page
//! are decompressed in parallel, each worker with its own decoder.

use std::fs::File;
use std::io::Cursor;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use memmap2::Mmap;
use rayon::prelude::*;
use tiff::decoder::{ChunkType, Decoder, DecodingResult, Limits};
use tiff::tags::Tag;
use tiff::ColorType;

use super::{raw_to_plane, xml_attr, xml_tags};
use crate::image::{ChannelInfo, DType, DimOrder, ImageInfo, Plane, PlaneData, Source};

pub struct TiffSource {
    mmap: Mmap,
    info: ImageInfo,
    /// IFD index for each logical page.
    ifds: Vec<usize>,
    /// ImageJ files over 4 GB store all planes contiguously after the first
    /// IFD; in that case this is (offset of plane 0, little endian).
    contiguous: Option<(usize, bool)>,
    planar: bool,
}

type Dec<'a> = Decoder<Cursor<&'a [u8]>>;

impl TiffSource {
    pub fn open(path: &Path) -> Result<TiffSource> {
        let file = File::open(path).with_context(|| format!("无法打开 {}", path.display()))?;
        // SAFETY: the file is opened read-only; concurrent truncation by another
        // process is the usual mmap caveat and is accepted for a viewer.
        let mmap = unsafe { Mmap::map(&file)? };
        let mut dec = Decoder::new(Cursor::new(&mmap[..]))?.with_limits(Limits::unlimited());

        let (w, h) = dec.dimensions()?;
        let color = dec.colortype()?;
        let sample_format = dec
            .find_tag_unsigned_vec::<u16>(Tag::SampleFormat)?
            .and_then(|v| v.first().copied())
            .unwrap_or(1);
        let description = dec
            .find_tag(Tag::ImageDescription)?
            .and_then(|v| v.into_string().ok())
            .unwrap_or_default();
        let (bits, spp, is_rgb) = match color {
            ColorType::Gray(b) | ColorType::Palette(b) => (b, 1, false),
            ColorType::GrayA(b) => (b, 2, false),
            ColorType::RGB(b) => (b, 3, b == 8),
            ColorType::RGBA(b) => (b, 4, b == 8),
            ColorType::CMYK(b) | ColorType::CMYKA(b) => (b, 4, false),
            ColorType::YCbCr(b) | ColorType::Lab(b) => (b, 3, b == 8),
            ColorType::Multiband { bit_depth, num_samples } => (bit_depth, num_samples as usize, false),
            _ => bail!("不支持的 TIFF 颜色类型 {:?}", color),
        };
        let dtype = match (bits, sample_format) {
            (8, 2) => DType::I8,
            (8, _) => DType::U8,
            (16, 2) => DType::I16,
            (16, 3) => DType::F32, // half floats are widened on decode
            (16, _) => DType::U16,
            (32, 2) => DType::I32,
            (32, 3) => DType::F32,
            (32, _) => DType::U32,
            (64, _) => DType::F64,
            (b, _) if b < 8 => DType::U8,
            (b, _) if b < 16 => DType::U16,
            _ => bail!("不支持的位深 {bits}"),
        };
        let little = matches!(dec.byte_order(), tiff::tags::ByteOrder::LittleEndian);
        let first_strip = dec
            .find_tag(Tag::StripOffsets)?
            .and_then(|v| v.into_u64_vec().ok())
            .and_then(|v| v.first().copied());
        let compression = dec.find_tag_unsigned::<u16>(Tag::Compression)?.unwrap_or(1);
        let planar = dec.find_tag_unsigned::<u16>(Tag::PlanarConfiguration)?.unwrap_or(1);
        let rational = |v: Option<tiff::decoder::ifd::Value>| {
            use tiff::decoder::ifd::Value;
            match v? {
                Value::Rational(n, d) if d != 0 => Some(n as f64 / d as f64),
                Value::Float(f) => Some(f as f64),
                Value::Double(f) => Some(f),
                _ => None,
            }
            .filter(|&r| r > 0.0)
        };
        let xres = rational(dec.find_tag(Tag::XResolution).ok().flatten());
        let yres = rational(dec.find_tag(Tag::YResolution).ok().flatten());
        let res_unit = dec.find_tag_unsigned::<u16>(Tag::ResolutionUnit).ok().flatten().unwrap_or(2);

        // Walk the IFD chain, keeping full-resolution pages of the same shape.
        let mut ifds = vec![0usize];
        let mut idx = 0;
        while dec.more_images() {
            idx += 1;
            if dec.next_image().is_err() {
                break;
            }
            let reduced = dec.find_tag_unsigned::<u32>(Tag::NewSubfileType).ok().flatten().unwrap_or(0) & 1 == 1;
            if !reduced && dec.dimensions().ok() == Some((w, h)) {
                ifds.push(idx);
            }
        }

        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut info = ImageInfo {
            path: path.to_string_lossy().into_owned(),
            name,
            format: "TIFF".into(),
            width: w as usize,
            height: h as usize,
            size_c: spp.max(1),
            size_z: ifds.len(),
            size_t: 1,
            dtype,
            is_rgb,
            channels: vec![],
            pixel_size: None,
            unit: String::new(),
            samples_per_page: spp.max(1),
            order: DimOrder::CZT,
            page_count: ifds.len(),
        };
        let mut contiguous = None;

        if description.contains("<OME") {
            parse_ome(&description, &mut info);
        } else if description.starts_with("ImageJ=") {
            let images = parse_imagej(&description, &mut info);
            let plane = info.page_bytes();
            if let (Some(n), Some(off)) = (images, first_strip) {
                if ifds.len() == 1 && n > 1 && compression == 1 && off as usize + n * plane <= mmap.len() {
                    contiguous = Some((off as usize, little));
                    info.page_count = n;
                }
            }
        }
        let consistent = if info.samples_per_page > 1 {
            info.size_c == info.samples_per_page && info.size_z * info.size_t == info.page_count
        } else {
            info.size_c * info.size_z * info.size_t == info.page_count
        };
        if !consistent {
            // Metadata disagrees with what is in the file: fall back to a plain stack.
            info.size_c = info.samples_per_page;
            info.size_z = info.page_count;
            info.size_t = 1;
            info.order = DimOrder::CZT;
            info.channels.clear();
        }
        // Pixel size from the resolution tags: ImageJ stores pixels per unit,
        // other writers usually pixels per centimetre.
        if let Some(xr) = xres {
            let known = info.pixel_size.map(|p| p[0] > 0.0).unwrap_or(false);
            let z = info.pixel_size.map(|p| p[2]).unwrap_or(0.0);
            let yr = yres.unwrap_or(xr);
            if !known && info.format == "ImageJ TIFF" && !info.unit.is_empty() && info.unit != "pixel" {
                info.pixel_size = Some([1.0 / xr, 1.0 / yr, z]);
            } else if !known && res_unit == 3 {
                info.pixel_size = Some([1e4 / xr, 1e4 / yr, z]);
                info.unit = "µm".into();
            }
        }
        info.fill_default_channels();
        Ok(TiffSource { mmap, info, ifds, contiguous, planar: planar == 2 })
    }

    fn decoder_at(&self, ifd: usize) -> Result<Dec<'_>> {
        let mut d = Decoder::new(Cursor::new(&self.mmap[..]))?.with_limits(Limits::unlimited());
        if ifd > 0 {
            d.seek_to_image(ifd)?;
        }
        Ok(d)
    }

    fn decode_ifd(&self, ifd: usize) -> Result<PlaneData> {
        let mut dec = self.decoder_at(ifd)?;
        let (w, h) = (self.info.width, self.info.height);
        let spp = self.info.samples_per_page;
        let bps = self.info.dtype.size();
        let planar = dec.find_tag_unsigned::<u16>(Tag::PlanarConfiguration)?.unwrap_or(1);
        let bits = dec.colortype()?.bit_depth();
        let native = matches!(bits, 8 | 32 | 64) || (bits == 16 && self.info.dtype != DType::F32);
        if planar != 1 || !native {
            return self.decode_fallback(&mut dec);
        }

        let mut data = PlaneData::zeroed(self.info.dtype, w * h * spp);
        let row = w * spp * bps;
        let bytes = data.as_bytes_mut();
        match dec.get_chunk_type() {
            ChunkType::Strip => {
                let rps = (dec.chunk_dimensions().1 as usize).clamp(1, h);
                bytes
                    .par_chunks_mut(rps * row)
                    .enumerate()
                    .try_for_each_init(
                        || self.decoder_at(ifd).ok(),
                        |d, (i, out)| -> Result<()> {
                            let d = d.as_mut().ok_or_else(|| anyhow!("decoder"))?;
                            d.read_chunk_bytes(i as u32, out)?;
                            Ok(())
                        },
                    )?;
            }
            ChunkType::Tile => {
                let (tw, th) = dec.chunk_dimensions();
                let (tw, th) = (tw as usize, th as usize);
                let across = w.div_ceil(tw);
                bytes
                    .par_chunks_mut(th * row)
                    .enumerate()
                    .try_for_each_init(
                        || (self.decoder_at(ifd).ok(), Vec::<u8>::new()),
                        |(d, buf), (ty, band)| -> Result<()> {
                            let d = d.as_mut().ok_or_else(|| anyhow!("decoder"))?;
                            for tx in 0..across {
                                let idx = (ty * across + tx) as u32;
                                let (dw, dh) = d.chunk_data_dimensions(idx);
                                let (dw, dh) = (dw as usize, dh as usize);
                                let trow = dw * spp * bps;
                                buf.resize(trow * dh, 0);
                                d.read_chunk_bytes(idx, buf)?;
                                let x0 = tx * tw * spp * bps;
                                for r in 0..dh.min(band.len() / row) {
                                    band[r * row + x0..r * row + x0 + trow]
                                        .copy_from_slice(&buf[r * trow..(r + 1) * trow]);
                                }
                            }
                            Ok(())
                        },
                    )?;
            }
        }
        Ok(data)
    }

    /// Whole-image decode through the library, for unusual layouts.
    fn decode_fallback(&self, dec: &mut Dec<'_>) -> Result<PlaneData> {
        let mut res = DecodingResult::U8(vec![]);
        dec.read_image_to_buffer(&mut res)?;
        let data = match res {
            DecodingResult::U8(v) => PlaneData::U8(v),
            DecodingResult::I8(v) => PlaneData::I8(v),
            DecodingResult::U16(v) => PlaneData::U16(v),
            DecodingResult::I16(v) => PlaneData::I16(v),
            DecodingResult::U32(v) => PlaneData::U32(v),
            DecodingResult::I32(v) => PlaneData::I32(v),
            DecodingResult::F32(v) => PlaneData::F32(v),
            DecodingResult::F64(v) => PlaneData::F64(v),
            DecodingResult::U64(v) => PlaneData::F64(v.into_iter().map(|x| x as f64).collect()),
            DecodingResult::I64(v) => PlaneData::F64(v.into_iter().map(|x| x as f64).collect()),
            DecodingResult::F16(v) => PlaneData::F32(v.into_iter().map(f32::from).collect()),
        };
        let need = self.info.width * self.info.height * self.info.samples_per_page;
        if data.len() < need {
            bail!("此 TIFF 的像素布局暂不支持");
        }
        Ok(data)
    }
}

impl Source for TiffSource {
    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        let (w, h, spp) = (self.info.width, self.info.height, self.info.samples_per_page);
        let data = if let Some((off, little)) = self.contiguous {
            let n = self.info.page_bytes();
            let start = off + page * n;
            raw_to_plane(&self.mmap[start..start + n], self.info.dtype, little)
        } else {
            let ifd = *self.ifds.get(page).ok_or_else(|| anyhow!("页码越界"))?;
            self.decode_ifd(ifd)?
        };
        let planes = if self.planar && self.contiguous.is_none() {
            split_planar(data, w * h, spp)
        } else {
            data.deinterleave(spp)
        };
        Ok(planes.into_iter().map(|data| Plane { w, h, data }).collect())
    }
}

fn split_planar(data: PlaneData, plane: usize, n: usize) -> Vec<PlaneData> {
    macro_rules! s {
        ($v:expr, $var:ident) => {
            (0..n).map(|i| PlaneData::$var($v[i * plane..(i + 1) * plane].to_vec())).collect()
        };
    }
    match data {
        PlaneData::U8(v) => s!(v, U8),
        PlaneData::I8(v) => s!(v, I8),
        PlaneData::U16(v) => s!(v, U16),
        PlaneData::I16(v) => s!(v, I16),
        PlaneData::U32(v) => s!(v, U32),
        PlaneData::I32(v) => s!(v, I32),
        PlaneData::F32(v) => s!(v, F32),
        PlaneData::F64(v) => s!(v, F64),
    }
}

fn parse_ome(xml: &str, info: &mut ImageInfo) {
    let Some(pixels) = xml_tags(xml, "Pixels").next() else { return };
    let get = |k: &str| xml_attr(pixels, k).and_then(|v| v.parse::<usize>().ok());
    info.format = "OME-TIFF".into();
    info.size_c = get("SizeC").unwrap_or(1).max(1);
    info.size_z = get("SizeZ").unwrap_or(1).max(1);
    info.size_t = get("SizeT").unwrap_or(1).max(1);
    if let Some(o) = xml_attr(pixels, "DimensionOrder") {
        info.order = DimOrder::parse(o);
    }
    let fx = |k: &str| xml_attr(pixels, k).and_then(|v| v.parse::<f64>().ok());
    if let Some(px) = fx("PhysicalSizeX") {
        info.pixel_size = Some([px, fx("PhysicalSizeY").unwrap_or(px), fx("PhysicalSizeZ").unwrap_or(0.0)]);
        info.unit = xml_attr(pixels, "PhysicalSizeXUnit").unwrap_or("µm").to_string();
    }
    info.channels = xml_tags(xml, "Channel")
        .enumerate()
        .map(|(i, ch)| {
            let name = xml_attr(ch, "Name")
                .or_else(|| xml_attr(ch, "Fluor"))
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("C{}", i + 1));
            // OME colors are signed 32-bit RGBA.
            let color = xml_attr(ch, "Color")
                .and_then(|c| c.parse::<i64>().ok())
                .map(|c| {
                    let c = c as u32;
                    [(c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8]
                })
                .filter(|c| c != &[0, 0, 0])
                .unwrap_or(crate::image::DEFAULT_COLORS[i % 7]);
            ChannelInfo { name, color }
        })
        .collect();
    if info.samples_per_page > 1 && info.size_c == info.samples_per_page {
        // Interleaved RGB pages: channels live inside each page.
        if info.size_z * info.size_t != info.page_count {
            info.size_z = info.page_count;
            info.size_t = 1;
        }
        info.size_c = info.samples_per_page;
        return;
    }
    if info.size_c == 1 && info.channels.len() == 1 && info.size_z == 1 && info.size_t == 1 {
        info.channels[0].color = [255, 255, 255];
    }
}

/// Returns the declared number of images.
fn parse_imagej(desc: &str, info: &mut ImageInfo) -> Option<usize> {
    let kv = |k: &str| {
        desc.lines()
            .find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
            .map(|s| s.trim().to_string())
    };
    info.format = "ImageJ TIFF".into();
    let num = |k: &str| kv(k).and_then(|v| v.parse::<usize>().ok());
    let images = num("images");
    if info.samples_per_page == 1 {
        info.size_c = num("channels").unwrap_or(1).max(1);
    }
    info.size_z = num("slices").unwrap_or(1).max(1);
    info.size_t = num("frames").unwrap_or(1).max(1);
    if num("channels").is_none() && num("slices").is_none() && num("frames").is_none() {
        info.size_z = images.unwrap_or(info.page_count);
    }
    info.order = DimOrder::CZT;
    if let Some(sp) = kv("spacing").and_then(|v| v.parse::<f64>().ok()) {
        info.pixel_size = Some([0.0, 0.0, sp]);
    }
    if let Some(u) = kv("unit") {
        info.unit = if u == "micron" || u == "um" || u == "\\u00B5m" { "µm".into() } else { u };
    }
    images
}
