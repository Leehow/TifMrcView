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
    /// Set for 8-bit palette images, which the tiff crate does not decode.
    palette: Option<Palette>,
}

/// `lut` is None when the colour map is (essentially) a gray ramp: the
/// indices are then the intensities, as in Zeiss SmartSEM images whose
/// palette only recolours a few entries for overlays.
struct Palette {
    lut: Option<Vec<[u8; 3]>>,
}

type Dec<'a> = Decoder<Cursor<&'a [u8]>>;

impl TiffSource {
    pub fn open(path: &Path) -> Result<TiffSource> {
        Self::open_inner(path).map_err(unsupported_msg)
    }

    fn open_inner(path: &Path) -> Result<TiffSource> {
        let file = File::open(path).with_context(|| format!("无法打开 {}", path.display()))?;
        // SAFETY: the file is opened read-only; concurrent truncation by another
        // process is the usual mmap caveat and is accepted for a viewer.
        let mmap = unsafe { Mmap::map(&file)? };
        let mut dec = Decoder::new(Cursor::new(&mmap[..]))?.with_limits(Limits::unlimited());

        let (w, h) = dec.dimensions()?;
        let photometric = dec.find_tag_unsigned::<u16>(Tag::PhotometricInterpretation)?.unwrap_or(1);
        let mut palette = None;
        let color = if photometric == 3 {
            let bits = dec.find_tag_unsigned::<u16>(Tag::BitsPerSample)?.unwrap_or(1);
            if bits != 8 {
                bail!("不支持 {bits} 位的调色板 TIFF");
            }
            let map = dec.find_tag_unsigned_vec::<u16>(Tag::ColorMap)?.unwrap_or_default();
            palette = Some(Palette { lut: color_lut(&map) });
            match palette.as_ref().and_then(|p| p.lut.as_ref()) {
                Some(_) => ColorType::RGB(8),
                None => ColorType::Gray(8),
            }
        } else {
            dec.colortype()?
        };
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
            metadata: vec![],
            samples_per_page: spp.max(1),
            order: DimOrder::CZT,
            page_count: ifds.len(),
            series: vec![],
            series_index: 0,
            folder: None,
            spectrum: None,
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
        Ok(TiffSource { mmap, info, ifds, contiguous, planar: planar == 2, palette })
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
        if let Some(p) = &self.palette {
            return self.decode_palette(&mut dec, p);
        }
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

    /// Reads the strips of an 8-bit palette image ourselves and maps the
    /// indices through the colour map unless it is a gray ramp.
    fn decode_palette(&self, dec: &mut Dec<'_>, pal: &Palette) -> Result<PlaneData> {
        let (w, h) = (self.info.width, self.info.height);
        if dec.find_tag(Tag::TileOffsets)?.is_some() {
            bail!("分块存储的调色板 TIFF 暂不支持");
        }
        let offsets = dec.find_tag(Tag::StripOffsets)?.ok_or_else(|| anyhow!("缺少 StripOffsets"))?.into_u64_vec()?;
        let counts = dec.find_tag(Tag::StripByteCounts)?.ok_or_else(|| anyhow!("缺少 StripByteCounts"))?.into_u64_vec()?;
        let rps = (dec.find_tag_unsigned::<u32>(Tag::RowsPerStrip)?.unwrap_or(u32::MAX) as usize).clamp(1, h);
        let compression = dec.find_tag_unsigned::<u16>(Tag::Compression)?.unwrap_or(1);
        let predictor = dec.find_tag_unsigned::<u16>(Tag::Predictor)?.unwrap_or(1);
        let mut idx = vec![0u8; w * h];
        idx.par_chunks_mut(rps * w).enumerate().try_for_each(|(i, out)| -> Result<()> {
            let (off, n) = (*offsets.get(i).ok_or_else(|| anyhow!("条带缺失"))? as usize, *counts.get(i).unwrap_or(&0) as usize);
            let src = self.mmap.get(off..off + n).ok_or_else(|| anyhow!("条带超出文件末尾"))?;
            inflate_strip(src, compression, out)?;
            if predictor == 2 {
                for row in out.chunks_mut(w) {
                    for x in 1..row.len() {
                        row[x] = row[x].wrapping_add(row[x - 1]);
                    }
                }
            }
            Ok(())
        })?;
        Ok(match &pal.lut {
            None => PlaneData::U8(idx),
            Some(lut) => PlaneData::U8(idx.par_iter().flat_map_iter(|&i| lut[i as usize]).collect()),
        })
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
        let bits = dec.colortype()?.bit_depth();
        if let (PlaneData::U8(packed), true) = (&data, bits < 8 && self.info.samples_per_page == 1) {
            // Sub-byte samples come back packed MSB first, rows padded to a byte;
            // the library has already applied WhiteIsZero inversion.
            let (w, h) = (self.info.width, self.info.height);
            let row = (w * bits as usize).div_ceil(8);
            if packed.len() >= row * h {
                let max = (1u8 << bits) - 1;
                let mut out = Vec::with_capacity(w * h);
                for y in 0..h {
                    let src = &packed[y * row..(y + 1) * row];
                    for x in 0..w {
                        let bit = x * bits as usize;
                        let v = (src[bit / 8] >> (8 - bits as usize - bit % 8)) & max;
                        out.push(v);
                    }
                }
                return Ok(PlaneData::U8(out));
            }
        }
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

    fn raw_page(&self, page: usize) -> Option<(&[u8], bool)> {
        let (off, little) = self.contiguous?;
        if self.info.samples_per_page > 1 || self.palette.is_some() {
            return None;
        }
        let n = self.info.page_bytes();
        let start = off + page.min(self.info.page_count - 1) * n;
        Some((&self.mmap[start..start + n], little))
    }

    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        let (w, h, spp) = (self.info.width, self.info.height, self.info.samples_per_page);
        let data = if let Some((off, little)) = self.contiguous {
            let n = self.info.page_bytes();
            let start = off + page * n;
            raw_to_plane(&self.mmap[start..start + n], self.info.dtype, little)
        } else {
            let ifd = *self.ifds.get(page).ok_or_else(|| anyhow!("页码越界"))?;
            self.decode_ifd(ifd).map_err(unsupported_msg)?
        };
        let planes = if self.planar && self.contiguous.is_none() {
            split_planar(data, w * h, spp)
        } else {
            data.deinterleave(spp)
        };
        Ok(planes.into_iter().map(|data| Plane { w, h, data }).collect())
    }
}

/// Replaces the library's English "unsupported" errors with a user-facing one.
fn unsupported_msg(e: anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<tiff::TiffError>() {
        Some(tiff::TiffError::UnsupportedError(u)) => anyhow!("不支持的 TIFF 位深或颜色类型（{u}）"),
        _ => e,
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

/// Gray ramps give None, anything else an 8-bit RGB lookup table. Zeiss
/// SmartSEM recolours every ninth entry (26 of 256) for its overlays and
/// keeps image data off those indices, so up to a quarter may differ.
fn color_lut(map: &[u16]) -> Option<Vec<[u8; 3]>> {
    let n = map.len() / 3;
    if n == 0 {
        return None;
    }
    let (r, g, b) = (&map[..n], &map[n..2 * n], &map[2 * n..3 * n]);
    let gray = (0..n).filter(|&i| r[i] == g[i] && g[i] == b[i] && (r[i] >> 8) as usize == i * 255 / (n - 1).max(1)).count();
    if gray * 4 >= n * 3 {
        return None;
    }
    let mut lut = vec![[0u8; 3]; 256];
    for (i, e) in lut.iter_mut().enumerate().take(n) {
        *e = [(r[i] >> 8) as u8, (g[i] >> 8) as u8, (b[i] >> 8) as u8];
    }
    Some(lut)
}

/// Decompresses one strip into `out` (which is exactly the strip's size).
fn inflate_strip(src: &[u8], compression: u16, out: &mut [u8]) -> Result<()> {
    use std::io::Read;
    match compression {
        1 => {
            let n = src.len().min(out.len());
            out[..n].copy_from_slice(&src[..n]);
        }
        5 => {
            let mut d = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
            let v = d.decode(src).or_else(|_| {
                // Old-style LZW without the early code size switch.
                weezl::decode::Decoder::new(weezl::BitOrder::Msb, 8).decode(src)
            })?;
            let n = v.len().min(out.len());
            out[..n].copy_from_slice(&v[..n]);
        }
        8 | 32946 => {
            let mut z = flate2::read::ZlibDecoder::new(src);
            let mut filled = 0;
            while filled < out.len() {
                let k = z.read(&mut out[filled..])?;
                if k == 0 {
                    break;
                }
                filled += k;
            }
        }
        32773 => {
            let (mut i, mut o) = (0, 0);
            while i < src.len() && o < out.len() {
                let c = src[i] as i8;
                i += 1;
                if c >= 0 {
                    let n = (c as usize + 1).min(src.len() - i).min(out.len() - o);
                    out[o..o + n].copy_from_slice(&src[i..i + n]);
                    i += c as usize + 1;
                    o += n;
                } else if c != -128 && i < src.len() {
                    let n = ((1 - c as isize) as usize).min(out.len() - o);
                    out[o..o + n].fill(src[i]);
                    i += 1;
                    o += n;
                }
            }
        }
        c => bail!("调色板 TIFF 的压缩方式 {c} 暂不支持"),
    }
    Ok(())
}

#[cfg(test)]
mod format_fix_tests {
    use super::*;
    use crate::image::PlaneData;

    /// Minimal little-endian single-strip uncompressed gray TIFF.
    fn build(w: u32, h: u32, bits: u16, photometric: u16, data: &[u8]) -> Vec<u8> {
        let entries: [(u16, u16, u32, u32); 9] = [
            (256, 4, 1, w),
            (257, 4, 1, h),
            (258, 3, 1, bits as u32),
            (259, 3, 1, 1),
            (262, 3, 1, photometric as u32),
            (273, 4, 1, 8),
            (277, 3, 1, 1),
            (278, 4, 1, h),
            (279, 4, 1, data.len() as u32),
        ];
        let mut b = b"II\x2a\x00".to_vec();
        b.extend(((8 + data.len() + data.len() % 2) as u32).to_le_bytes());
        b.extend(data);
        if data.len() % 2 == 1 {
            b.push(0);
        }
        b.extend((entries.len() as u16).to_le_bytes());
        for (tag, ty, n, v) in entries {
            b.extend(tag.to_le_bytes());
            b.extend(ty.to_le_bytes());
            b.extend(n.to_le_bytes());
            b.extend(v.to_le_bytes());
        }
        b.extend(0u32.to_le_bytes());
        b
    }

    fn read(name: &str, bytes: &[u8]) -> Result<(ImageInfo, PlaneData)> {
        let dir = std::env::temp_dir().join(format!("tmv_fmtfix_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let p = dir.join("t.tif");
        std::fs::write(&p, bytes)?;
        let r = crate::formats::open(&p).and_then(|s| {
            let pl = s.read_page(0)?;
            Ok((s.info().clone(), pl.into_iter().next().unwrap().data))
        });
        let _ = std::fs::remove_dir_all(&dir);
        r
    }

    #[test]
    fn bilevel_black_is_zero() {
        // 10x2, rows padded to 2 bytes.
        let (info, d) = read("b1", &build(10, 2, 1, 1, &[0b1010_0000, 0b0100_0000, 0xff, 0xc0])).unwrap();
        assert_eq!(info.dtype, DType::U8);
        let PlaneData::U8(v) = d else { panic!() };
        let mut want = vec![1, 0, 1, 0, 0, 0, 0, 0, 0, 1];
        want.extend([1u8; 10]);
        assert_eq!(v, want);
    }

    #[test]
    fn bilevel_white_is_zero_inverts() {
        let (_, d) = read("b1w", &build(8, 1, 1, 0, &[0b1010_0000])).unwrap();
        let PlaneData::U8(v) = d else { panic!() };
        assert_eq!(v, vec![0, 1, 0, 1, 1, 1, 1, 1]);
    }

    #[test]
    fn four_bit_gray() {
        // 3x2, rows padded to 2 bytes.
        let (_, d) = read("b4", &build(3, 2, 4, 1, &[0x12, 0x30, 0xfe, 0xd0])).unwrap();
        let PlaneData::U8(v) = d else { panic!() };
        assert_eq!(v, vec![1, 2, 3, 15, 14, 13]);
    }

    #[test]
    fn twelve_bit_gray_clear_error() {
        // 2x1, 12-bit packed: 0xABC 0x123.
        // The tiff crate cannot decode 12-bit samples: expect a clear error.
        let e = read("b12", &build(2, 1, 12, 1, &[0xab, 0xc1, 0x23])).err().unwrap();
        assert!(e.to_string().contains("不支持的 TIFF 位深"), "{e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats;
    use crate::image::tests::fake_info;
    use tiff::encoder::{colortype, Rational, TiffEncoder, TiffValue};
    use tiff::tags::ResolutionUnit;

    /// Writes one IFD per entry of `pages`; the description goes on the first.
    fn write_tiff<C: colortype::ColorType>(
        path: &Path,
        (w, h): (u32, u32),
        pages: &[Vec<C::Inner>],
        desc: Option<&str>,
        res: Option<(ResolutionUnit, u32)>,
    ) where
        [C::Inner]: TiffValue,
    {
        let mut enc = TiffEncoder::new(File::create(path).unwrap()).unwrap();
        for (i, data) in pages.iter().enumerate() {
            let mut img = enc.new_image::<C>(w, h).unwrap();
            if let (0, Some(d)) = (i, desc) {
                img.encoder().write_tag(Tag::ImageDescription, d).unwrap();
            }
            if let (0, Some((unit, n))) = (i, res) {
                img.resolution(unit, Rational { n, d: 1 });
            }
            img.write_data(data).unwrap();
        }
    }

    fn open(path: &Path) -> Box<dyn Source> {
        formats::open(path).unwrap()
    }

    fn page_values(src: &dyn Source, page: usize, sample: usize) -> Vec<f64> {
        let planes = src.read_page(page).unwrap();
        let p = &planes[sample].data;
        (0..p.len()).map(|i| p.get_f64(i)).collect()
    }

    /// `n` pages of 3x2 pixels where page p holds p*10 + pixel index.
    fn gray8_pages(n: usize) -> Vec<Vec<u8>> {
        (0..n).map(|p| (0..6).map(|i| (p * 10 + i) as u8).collect()).collect()
    }

    #[test]
    fn gray8_stack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        write_tiff::<colortype::Gray8>(&path, (3, 2), &gray8_pages(4), None, None);
        let src = open(&path);
        let info = src.info();
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!((info.size_c, info.size_z, info.size_t, info.page_count), (1, 4, 1, 4));
        assert_eq!(info.dtype, DType::U8);
        assert!(!info.is_rgb);
        assert_eq!(info.format, "TIFF");
        assert_eq!(info.pixel_size, None);
        assert_eq!(page_values(&*src, 2, 0), [20.0, 21.0, 22.0, 23.0, 24.0, 25.0]);
        assert_eq!(info.locate(0, 3, 0), (3, 0));
    }

    #[test]
    fn gray16_stack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        let pages: Vec<Vec<u16>> = (0..3).map(|p| (0..6).map(|i| (p * 20000 + i * 1000) as u16).collect()).collect();
        write_tiff::<colortype::Gray16>(&path, (3, 2), &pages, None, None);
        let src = open(&path);
        assert_eq!(src.info().dtype, DType::U16);
        assert_eq!(src.info().size_z, 3);
        assert_eq!(page_values(&*src, 2, 0), [40000.0, 41000.0, 42000.0, 43000.0, 44000.0, 45000.0]);
    }

    #[test]
    fn float32_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        let vals = vec![-1.5f32, 0.0, 0.25, 1e6, f32::MIN_POSITIVE, 3.0];
        write_tiff::<colortype::Gray32Float>(&path, (3, 2), std::slice::from_ref(&vals), None, None);
        let src = open(&path);
        assert_eq!(src.info().dtype, DType::F32);
        assert_eq!(page_values(&*src, 0, 0), vals.iter().map(|&v| v as f64).collect::<Vec<_>>());
    }

    #[test]
    fn rgb8_is_interleaved_into_channels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        // Two pixels per row, two rows: (r,g,b) = (p, p+100, p+200).
        let page: Vec<u8> = (0..4u8).flat_map(|p| [p, p + 100, p + 200]).collect();
        write_tiff::<colortype::RGB8>(&path, (2, 2), &[page.clone(), page], None, None);
        let src = open(&path);
        let info = src.info();
        assert!(info.is_rgb);
        assert_eq!((info.size_c, info.size_z, info.samples_per_page, info.page_count), (3, 2, 3, 2));
        assert_eq!(info.channels.len(), 3);
        assert_eq!(src.read_page(1).unwrap().len(), 3);
        assert_eq!(page_values(&*src, 0, 0), [0.0, 1.0, 2.0, 3.0]);
        assert_eq!(page_values(&*src, 0, 1), [100.0, 101.0, 102.0, 103.0]);
        assert_eq!(page_values(&*src, 0, 2), [200.0, 201.0, 202.0, 203.0]);
        assert_eq!(info.locate(2, 1, 0), (1, 2));
    }

    #[test]
    fn page_out_of_range_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        write_tiff::<colortype::Gray8>(&path, (3, 2), &gray8_pages(2), None, None);
        assert!(open(&path).read_page(5).is_err());
    }

    #[test]
    fn multiple_strips_per_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        let (w, h) = (4u32, 9u32);
        let page: Vec<u8> = (0..w * h).map(|i| i as u8).collect();
        let mut enc = TiffEncoder::new(File::create(&path).unwrap()).unwrap();
        let mut img = enc.new_image::<colortype::Gray8>(w, h).unwrap();
        img.rows_per_strip(2).unwrap();
        img.write_data(&page).unwrap();
        let src = open(&path);
        assert_eq!(page_values(&*src, 0, 0), page.iter().map(|&v| v as f64).collect::<Vec<_>>());
    }

    #[test]
    fn imagej_hyperstack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        let desc = "ImageJ=1.53t\nimages=6\nchannels=2\nslices=3\nunit=micron\nspacing=0.5\n";
        write_tiff::<colortype::Gray8>(&path, (3, 2), &gray8_pages(6), Some(desc), None);
        let src = open(&path);
        let info = src.info();
        assert_eq!(info.format, "ImageJ TIFF");
        assert_eq!((info.size_c, info.size_z, info.size_t, info.page_count), (2, 3, 1, 6));
        assert_eq!(info.unit, "µm");
        // The z spacing comes from the description.
        assert_eq!(info.pixel_size.map(|p| p[2]), Some(0.5));
        assert_eq!(info.channels.len(), 2);
        // ImageJ order is CZT: channel varies fastest.
        assert_eq!(info.locate(1, 0, 0).0, 1);
        let (page, _) = info.locate(1, 2, 0);
        assert_eq!(page, 5);
        assert_eq!(page_values(&*src, page, 0)[0], 50.0);
    }

    #[test]
    fn imagej_resolution_gives_pixel_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        let desc = "ImageJ=1.53t\nimages=1\nunit=micron\n";
        write_tiff::<colortype::Gray8>(&path, (3, 2), &gray8_pages(1), Some(desc), Some((ResolutionUnit::None, 4)));
        let info = open(&path).info().clone();
        assert_eq!(info.unit, "µm");
        assert_eq!(info.pixel_size, Some([0.25, 0.25, 0.0]));
    }

    #[test]
    fn plain_tiff_resolution_in_centimetres() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        write_tiff::<colortype::Gray8>(&path, (3, 2), &gray8_pages(1), None, Some((ResolutionUnit::Centimeter, 10_000)));
        let info = open(&path).info().clone();
        assert_eq!(info.unit, "µm");
        assert_eq!(info.pixel_size, Some([1.0, 1.0, 0.0]));
    }

    #[test]
    fn imagej_contiguous_planes_after_first_ifd() {
        // A single IFD whose description claims 3 images: all planes are read
        // from consecutive bytes after the first strip offset.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        let desc = "ImageJ=1.53t\nimages=3\nslices=3\n";
        write_tiff::<colortype::Gray8>(&path, (3, 2), &gray8_pages(1), Some(desc), None);
        let mut bytes = std::fs::read(&path).unwrap();
        let off = bytes.windows(6).position(|w| w == [0, 1, 2, 3, 4, 5]).unwrap();
        bytes.extend_from_slice(&[0; 64]);
        std::fs::write(&path, &bytes).unwrap();
        let src = open(&path);
        assert_eq!((src.info().page_count, src.info().size_z), (3, 3));
        assert_eq!(page_values(&*src, 0, 0), [0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        let want: Vec<f64> = bytes[off + 12..off + 18].iter().map(|&b| b as f64).collect();
        assert_eq!(page_values(&*src, 2, 0), want);
    }

    #[test]
    fn imagej_inconsistent_metadata_falls_back_to_stack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.tif");
        let desc = "ImageJ=1.53t\nimages=6\nchannels=4\nslices=3\n";
        write_tiff::<colortype::Gray8>(&path, (3, 2), &gray8_pages(6), Some(desc), None);
        let info = open(&path).info().clone();
        assert_eq!((info.size_c, info.size_z, info.size_t), (1, 6, 1));
        assert_eq!(info.channels.len(), 1);
    }

    #[test]
    fn ome_tiff_dimension_order_and_scale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.ome.tif");
        let desc = r#"<?xml version="1.0"?><OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06"><Image ID="Image:0"><Pixels ID="Pixels:0" DimensionOrder="XYZCT" Type="uint8" SizeX="3" SizeY="2" SizeZ="3" SizeC="2" SizeT="1" PhysicalSizeX="0.2" PhysicalSizeY="0.3" PhysicalSizeZ="1.5" PhysicalSizeXUnit="nm"><Channel ID="Channel:0:0" Name="DAPI" Color="-16776961"/><Channel ID="Channel:0:1" Fluor="GFP"/></Pixels></Image></OME>"#;
        write_tiff::<colortype::Gray8>(&path, (3, 2), &gray8_pages(6), Some(desc), None);
        let src = open(&path);
        let info = src.info();
        assert_eq!(info.format, "OME-TIFF");
        assert_eq!((info.size_c, info.size_z, info.size_t), (2, 3, 1));
        assert_eq!(info.pixel_size, Some([0.2, 0.3, 1.5]));
        assert_eq!(info.unit, "nm");
        assert_eq!(info.channels[0].name, "DAPI");
        assert_eq!(info.channels[0].color, [255, 0, 0]);
        assert_eq!(info.channels[1].name, "GFP");
        // XYZCT: z varies fastest, so (c=1, z=2) is page 5 and (c=1, z=0) is page 3.
        assert_eq!(info.locate(1, 2, 0).0, 5);
        assert_eq!(info.locate(1, 0, 0).0, 3);
        assert_eq!(page_values(&*src, info.locate(1, 0, 0).0, 0)[0], 30.0);
    }

    #[test]
    fn ome_tiff_with_rgb_pages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.ome.tif");
        let desc = r#"<OME><Image><Pixels DimensionOrder="XYCZT" SizeZ="2" SizeC="3" SizeT="1"/></Image></OME>"#;
        let page: Vec<u8> = (0..12).collect();
        write_tiff::<colortype::RGB8>(&path, (2, 2), &[page.clone(), page], Some(desc), None);
        let info = open(&path).info().clone();
        assert_eq!((info.size_c, info.size_z, info.size_t, info.samples_per_page), (3, 2, 1, 3));
    }

    // ---- parsers on strings ----------------------------------------------

    fn info() -> ImageInfo {
        fake_info(3, 2, 1, 6, 1, DType::U8)
    }

    #[test]
    fn parse_imagej_fields() {
        let mut i = info();
        let n = parse_imagej("ImageJ=1.53\nimages=24\nchannels=2\nslices=3\nframes=4\nspacing=0.7\nunit=um\n", &mut i);
        assert_eq!(n, Some(24));
        assert_eq!((i.size_c, i.size_z, i.size_t), (2, 3, 4));
        assert_eq!(i.pixel_size, Some([0.0, 0.0, 0.7]));
        assert_eq!(i.unit, "µm");
        assert_eq!(i.format, "ImageJ TIFF");
    }

    #[test]
    fn parse_imagej_only_images_means_z_stack() {
        let mut i = info();
        assert_eq!(parse_imagej("ImageJ=1.53\nimages=9\n", &mut i), Some(9));
        assert_eq!((i.size_c, i.size_z, i.size_t), (1, 9, 1));
        let mut i = info();
        assert_eq!(parse_imagej("ImageJ=1.53\n", &mut i), None);
        assert_eq!(i.size_z, 6);
    }

    #[test]
    fn parse_imagej_keeps_other_units_and_ignores_garbage() {
        let mut i = info();
        parse_imagej("ImageJ=1\nunit=nm\nspacing=abc\nchannels=x\n", &mut i);
        assert_eq!(i.unit, "nm");
        assert_eq!(i.pixel_size, None);
        assert_eq!(i.size_c, 1);
        // Key prefixes must match whole keys.
        let mut i = info();
        parse_imagej("ImageJ=1\nmyslices=5\nslices=2\n", &mut i);
        assert_eq!(i.size_z, 2);
    }

    #[test]
    fn parse_imagej_rgb_keeps_sample_channels() {
        let mut i = info();
        i.samples_per_page = 3;
        i.size_c = 3;
        parse_imagej("ImageJ=1\nimages=2\nchannels=5\nslices=2\n", &mut i);
        assert_eq!(i.size_c, 3);
    }

    #[test]
    fn parse_ome_defaults_and_order() {
        let mut i = info();
        parse_ome(r#"<OME><Pixels SizeZ="6" DimensionOrder="XYTZC"/></OME>"#, &mut i);
        assert_eq!((i.size_c, i.size_z, i.size_t), (1, 6, 1));
        assert!(matches!(i.order, DimOrder::TZC));
        assert_eq!(i.pixel_size, None);
        assert_eq!(i.format, "OME-TIFF");
    }

    #[test]
    fn parse_ome_physical_size_prefix_does_not_confuse_size() {
        let mut i = info();
        parse_ome(r#"<OME><Pixels PhysicalSizeZ="2" SizeZ="4" PhysicalSizeX="0.1"/></OME>"#, &mut i);
        assert_eq!(i.size_z, 4);
        assert_eq!(i.pixel_size, Some([0.1, 0.1, 2.0]));
        assert_eq!(i.unit, "µm");
    }

    #[test]
    fn parse_ome_channel_colors_and_names() {
        let mut i = info();
        parse_ome(
            r#"<OME><Pixels SizeC="3" SizeZ="2"><Channel Name="A" Color="-65536"/><Channel Color="0"/><Channel Fluor="F" Color="16711935"/></Pixels></OME>"#,
            &mut i,
        );
        let ch: Vec<_> = i.channels.iter().map(|c| (c.name.as_str(), c.color)).collect();
        // -65536 = 0xFFFF0000 (RGBA) -> yellow; 0 is "no colour" -> default; 0x00FF00FF -> (0,255,0).
        assert_eq!(ch[0], ("A", [255, 255, 0]));
        assert_eq!(ch[1], ("C2", crate::image::DEFAULT_COLORS[1]));
        assert_eq!(ch[2], ("F", [0, 255, 0]));
    }

    #[test]
    fn parse_ome_single_channel_is_white() {
        let mut i = info();
        parse_ome(r#"<OME><Pixels SizeZ="1"><Channel Name="A" Color="-65536"/></Pixels></OME>"#, &mut i);
        assert_eq!(i.channels[0].color, [255, 255, 255]);
    }

    #[test]
    fn parse_ome_without_pixels_changes_nothing() {
        let mut i = info();
        parse_ome("<OME></OME>", &mut i);
        assert_eq!((i.size_z, i.format.as_str()), (6, "fake"));
    }
}
