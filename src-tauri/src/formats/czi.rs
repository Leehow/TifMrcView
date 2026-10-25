//! Zeiss CZI reader (ZEN: LSM / Airyscan / Axioscan / Celldiscoverer).
//!
//! A CZI file is a chain of segments, each a 16-byte id (`ZISRAWFILE`,
//! `ZISRAWDIRECTORY`, `ZISRAWSUBBLOCK`, `ZISRAWMETADATA`…) plus sizes. The
//! directory lists every subblock: one tile of pixels with its position
//! along X, Y, C, Z, T, M (mosaic tile), S (scene) and so on. Metadata is
//! one big XML document.
//!
//! We show the full-resolution layer only (pyramid subblocks are skipped),
//! stitching mosaic tiles into one plane per (C, Z, T).

use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use rayon::prelude::*;

use super::raw_to_plane;
use crate::image::{ChannelInfo, DType, DimOrder, ImageInfo, Plane, Source};

const SEG_HEADER: usize = 32;

/// One full-resolution tile.
#[derive(Clone)]
struct Tile {
    /// Offset of the ZISRAWSUBBLOCK segment.
    pos: usize,
    x: i64,
    y: i64,
    w: usize,
    h: usize,
    c: usize,
    z: usize,
    t: usize,
    /// Mosaic index; higher M is drawn on top.
    m: i32,
    compression: i32,
}

#[derive(Clone, Copy, PartialEq)]
enum Pixel {
    Gray(DType),
    /// Interleaved BGR(A) with this sample type and sample count.
    Bgr(DType, usize),
}

impl Pixel {
    fn from_code(code: i32) -> Result<Pixel> {
        Ok(match code {
            0 => Pixel::Gray(DType::U8),
            1 => Pixel::Gray(DType::U16),
            2 => Pixel::Gray(DType::F32),
            12 => Pixel::Gray(DType::I32),
            3 => Pixel::Bgr(DType::U8, 3),
            4 => Pixel::Bgr(DType::U16, 3),
            8 => Pixel::Bgr(DType::F32, 3),
            9 => Pixel::Bgr(DType::U8, 4),
            _ => bail!("不支持的 CZI 像素类型 {code}"),
        })
    }

    fn dtype(self) -> DType {
        match self {
            Pixel::Gray(d) | Pixel::Bgr(d, _) => d,
        }
    }

    fn samples(self) -> usize {
        match self {
            Pixel::Gray(_) => 1,
            Pixel::Bgr(_, n) => n,
        }
    }
}

pub struct CziSource {
    mmap: Mmap,
    info: ImageInfo,
    pixel: Pixel,
    /// Left/top of the stitched plane in CZI coordinates.
    origin: (i64, i64),
    /// Tiles of each page, keyed by (c, z, t) and sorted by M.
    pages: Vec<Vec<Tile>>,
}

fn rd_i32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn rd_i64(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Checks the segment id at `pos` and returns the start of its data.
fn segment(mmap: &[u8], pos: usize, id: &str) -> Result<usize> {
    if pos + SEG_HEADER > mmap.len() || !mmap[pos..pos + 16].starts_with(id.as_bytes()) {
        bail!("CZI 文件损坏：找不到 {id}");
    }
    Ok(pos + SEG_HEADER)
}

pub fn is_czi(bytes: &[u8]) -> bool {
    bytes.starts_with(b"ZISRAWFILE")
}

impl CziSource {
    pub fn open(path: &Path) -> Result<CziSource> {
        let file = File::open(path).with_context(|| format!("无法打开 {}", path.display()))?;
        // SAFETY: read-only mapping; see TiffSource::open.
        let mmap = unsafe { Mmap::map(&file)? };
        let hdr = segment(&mmap, 0, "ZISRAWFILE")?;
        if mmap.len() < hdr + 80 {
            bail!("CZI 文件头不完整");
        }
        if rd_i32(&mmap, hdr + 48) != 0 {
            bail!("这是多文件 CZI 的分卷，请打开第一个文件");
        }
        let dir_pos = rd_i64(&mmap, hdr + 52) as usize;
        let meta_pos = rd_i64(&mmap, hdr + 60) as usize;

        // Directory entries.
        let d = segment(&mmap, dir_pos, "ZISRAWDIRECTORY")?;
        let count = rd_i32(&mmap, d).max(0) as usize;
        let mut p = d + 128;
        let mut tiles = vec![];
        let mut pixel_code = None;
        let mut extra_min: BTreeMap<[u8; 4], i32> = BTreeMap::new();
        let mut raw = vec![];
        for _ in 0..count {
            if p + 32 > mmap.len() {
                bail!("CZI 目录被截断");
            }
            let e = &mmap[p..];
            if &e[..2] != b"DV" {
                bail!("CZI 目录格式未知");
            }
            let ptype = rd_i32(e, 2);
            let pos = rd_i64(e, 6) as usize;
            let compression = rd_i32(e, 18);
            let pyramid = e[22];
            let ndim = rd_i32(e, 28).max(0) as usize;
            p += 32 + ndim * 20;
            if p > mmap.len() {
                bail!("CZI 目录被截断");
            }
            let mut t = Tile { pos, x: 0, y: 0, w: 0, h: 0, c: 0, z: 0, t: 0, m: 0, compression };
            let mut zoomed = pyramid != 0;
            let mut extras = vec![];
            for k in 0..ndim {
                let de = &e[32 + k * 20..32 + k * 20 + 20];
                let name: [u8; 4] = de[..4].try_into().unwrap();
                let start = rd_i32(de, 4);
                let size = rd_i32(de, 8);
                let stored = rd_i32(de, 16);
                match name[0] {
                    b'X' => (t.x, t.w, zoomed) = (start as i64, stored.max(0) as usize, zoomed || size != stored),
                    b'Y' => (t.y, t.h, zoomed) = (start as i64, stored.max(0) as usize, zoomed || size != stored),
                    b'C' => t.c = start.max(0) as usize,
                    b'Z' => t.z = start.max(0) as usize,
                    b'T' => t.t = start.max(0) as usize,
                    b'M' => t.m = start,
                    // Scenes sit at their own XY positions; stitching shows them all.
                    b'S' => {}
                    _ => extras.push((name, start)),
                }
            }
            if zoomed || t.w == 0 || t.h == 0 {
                continue;
            }
            for &(n, s) in &extras {
                let v = extra_min.entry(n).or_insert(s);
                *v = (*v).min(s);
            }
            pixel_code.get_or_insert(ptype);
            raw.push((t, extras, ptype));
        }
        let pixel = Pixel::from_code(pixel_code.context("CZI 文件里没有图像")?)?;
        // Other dimensions (phase H, rotation R, illumination I, view V, block
        // B…): keep their first index only.
        for (t, extras, ptype) in raw {
            if extras.iter().all(|(n, s)| extra_min.get(n) == Some(s)) {
                if Pixel::from_code(ptype).ok() != Some(pixel) {
                    bail!("CZI 里混有不同的像素类型");
                }
                tiles.push(t);
            }
        }

        // Index C, Z and T by rank so gaps or offsets in the indices do not
        // leave empty pages.
        let rank = |f: fn(&Tile) -> usize| {
            let mut v: Vec<usize> = tiles.iter().map(f).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (cs, zs, ts) = (rank(|t| t.c), rank(|t| t.z), rank(|t| t.t));
        let x0 = tiles.iter().map(|t| t.x).min().unwrap();
        let y0 = tiles.iter().map(|t| t.y).min().unwrap();
        let x1 = tiles.iter().map(|t| t.x + t.w as i64).max().unwrap();
        let y1 = tiles.iter().map(|t| t.y + t.h as i64).max().unwrap();
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        if w.saturating_mul(h) > 1 << 32 {
            bail!("CZI 拼接后图像太大（{w}×{h}），暂不支持");
        }

        let samples = pixel.samples();
        if samples > 1 && cs.len() > 1 {
            bail!("暂不支持多通道的彩色 CZI");
        }
        let (size_c, page_c) = if samples > 1 { (samples.min(3), 1) } else { (cs.len(), cs.len()) };
        let mut pages = vec![vec![]; page_c * zs.len() * ts.len()];
        for t in tiles {
            let c = if samples > 1 { 0 } else { cs.binary_search(&t.c).unwrap() };
            let z = zs.binary_search(&t.z).unwrap();
            let tt = ts.binary_search(&t.t).unwrap();
            pages[c + page_c * (z + zs.len() * tt)].push(t);
        }
        for p in &mut pages {
            p.sort_by_key(|t| t.m);
        }

        let mut info = ImageInfo {
            path: path.to_string_lossy().into_owned(),
            name: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            format: "Zeiss CZI".into(),
            width: w,
            height: h,
            size_c,
            size_z: zs.len(),
            size_t: ts.len(),
            dtype: pixel.dtype(),
            is_rgb: samples > 1 && pixel.dtype() == DType::U8,
            channels: vec![],
            pixel_size: None,
            unit: String::new(),
            metadata: vec![],
            samples_per_page: if samples > 1 { size_c } else { 1 },
            order: DimOrder::CZT,
            page_count: pages.len(),
        };
        if let Ok(xml) = metadata_xml(&mmap, meta_pos) {
            apply_metadata(&xml, &mut info, &cs);
        }
        info.fill_default_channels();
        Ok(CziSource { mmap, info, pixel, origin: (x0, y0), pages })
    }

    /// Raw pixel bytes of one tile, decompressed.
    fn tile_bytes(&self, t: &Tile) -> Result<Vec<u8>> {
        let d = segment(&self.mmap, t.pos, "ZISRAWSUBBLOCK")?;
        let meta_size = rd_i32(&self.mmap, d).max(0) as usize;
        let data_size = rd_i64(&self.mmap, d + 8).max(0) as usize;
        // The fixed part holds the directory entry and is padded to 256 bytes.
        let ndim = rd_i32(&self.mmap, d + 16 + 28).max(0) as usize;
        let fixed = (16 + 32 + ndim * 20).max(256);
        let start = d + fixed + meta_size;
        let data = self.mmap.get(start..start + data_size).context("CZI 子块被截断")?;
        let want = t.w * t.h * self.pixel.samples() * self.pixel.dtype().size();
        let out = match t.compression {
            0 => data.to_vec(),
            5 => zstd(data, want)?,
            6 => {
                // zstd1: a small header, then a zstd stream. With the hi/lo
                // flag the low bytes of all 16-bit samples come first.
                let n = *data.first().context("CZI 子块为空")? as usize;
                let hilo = n >= 3 && data.get(1) == Some(&1) && data.get(2).is_some_and(|b| b & 1 == 1);
                let mut v = zstd(data.get(n..).context("CZI 子块被截断")?, want)?;
                if hilo {
                    let half = v.len() / 2;
                    let mut packed = vec![0u8; v.len()];
                    for i in 0..half {
                        packed[2 * i] = v[i];
                        packed[2 * i + 1] = v[half + i];
                    }
                    v = packed;
                }
                v
            }
            1 => {
                let img = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg)?;
                match self.pixel {
                    Pixel::Gray(DType::U8) => img.into_luma8().into_raw(),
                    // JPEG decodes to RGB; CZI stores BGR.
                    _ => img.into_rgb8().into_raw().chunks_exact(3).flat_map(|p| [p[2], p[1], p[0]]).collect(),
                }
            }
            4 => bail!("这个 CZI 用 JPEG XR 压缩，暂不支持；请在 ZEN 里另存为不压缩或 Zstd 压缩"),
            c => bail!("不支持的 CZI 压缩方式 {c}"),
        };
        if out.len() < want {
            bail!("CZI 子块数据不完整");
        }
        Ok(out)
    }
}

fn zstd(data: &[u8], want: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut dec = ruzstd::decoding::StreamingDecoder::new(data).map_err(|e| anyhow::anyhow!("zstd: {e}"))?;
    let mut out = Vec::with_capacity(want);
    dec.read_to_end(&mut out)?;
    Ok(out)
}

/// The metadata XML of a CZI file.
pub fn metadata_xml(mmap: &[u8], pos: usize) -> Result<String> {
    let d = segment(mmap, pos, "ZISRAWMETADATA")?;
    let size = rd_i32(mmap, d).max(0) as usize;
    let xml = mmap.get(d + 256..d + 256 + size).context("CZI 元数据被截断")?;
    Ok(String::from_utf8_lossy(xml).into_owned())
}

impl Source for CziSource {
    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        let tiles = &self.pages[page.min(self.pages.len() - 1)];
        let (w, h) = (self.info.width, self.info.height);
        let spp = self.pixel.samples();
        let bps = self.pixel.dtype().size();
        let px = spp * bps;
        // Decode tiles in parallel, then paste them in M order.
        let decoded: Vec<Vec<u8>> = tiles.par_iter().map(|t| self.tile_bytes(t)).collect::<Result<_>>()?;
        let single = tiles.len() == 1 && tiles[0].w == w && tiles[0].h == h;
        let bytes = if single {
            decoded.into_iter().next().unwrap()
        } else {
            let mut buf = vec![0u8; w * h * px];
            for (t, src) in tiles.iter().zip(&decoded) {
                let ox = (t.x - self.origin.0) as usize;
                let oy = (t.y - self.origin.1) as usize;
                let row = t.w * px;
                buf.par_chunks_mut(w * px).skip(oy).take(t.h).enumerate().for_each(|(r, dst)| {
                    dst[ox * px..ox * px + row].copy_from_slice(&src[r * row..(r + 1) * row]);
                });
            }
            buf
        };
        let data = raw_to_plane(&bytes, self.pixel.dtype(), true);
        let mut planes = data.deinterleave(spp);
        if spp > 1 {
            // BGR(A) -> RGB, dropping alpha.
            planes.truncate(3);
            planes.swap(0, 2);
        }
        Ok(planes.into_iter().map(|data| Plane { w, h, data }).collect())
    }
}

// ---------------------------------------------------------------- metadata

pub type Node<'a, 'i> = roxmltree::Node<'a, 'i>;

pub fn child<'a, 'i>(n: Node<'a, 'i>, path: &str) -> Option<Node<'a, 'i>> {
    path.split('/').try_fold(n, |n, name| n.children().find(|c| c.has_tag_name(name)))
}

pub fn text(n: Node, path: &str) -> Option<String> {
    child(n, path).and_then(|c| c.text()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

pub fn children<'a, 'i>(n: Option<Node<'a, 'i>>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.into_iter().flat_map(move |n| n.children().filter(move |c| c.has_tag_name(name)))
}

/// `#AARRGGBB` -> RGB.
fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    let h = if h.len() == 8 { &h[2..] } else { h };
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Fills pixel size and channel names/colours from the XML. The acquisition
/// settings are shown by the Zeiss ZEN plugin (`plugins/zen.rs`).
/// `cs` are the C indices present in the file, in order.
fn apply_metadata(xml: &str, info: &mut ImageInfo, cs: &[usize]) {
    let Ok(doc) = roxmltree::Document::parse(xml) else { return };
    let Some(meta) = child(doc.root_element(), "Metadata") else { return };
    let inf = child(meta, "Information");
    let image = inf.and_then(|i| child(i, "Image"));

    // Pixel size: metres in the file, shown in µm.
    let mut ps = [0.0f64; 3];
    for d in children(child(meta, "Scaling/Items"), "Distance") {
        let v: f64 = text(d, "Value").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        match d.attribute("Id") {
            Some("X") => ps[0] = v * 1e6,
            Some("Y") => ps[1] = v * 1e6,
            Some("Z") => ps[2] = v * 1e6,
            _ => {}
        }
    }
    if ps[0] > 0.0 {
        if ps[1] <= 0.0 {
            ps[1] = ps[0];
        }
        info.pixel_size = Some(ps);
        info.unit = "µm".into();
    }

    // Channels, matched to C by their "Channel:N" id, else by order.
    let chans: Vec<Node> = children(image.and_then(|i| child(i, "Dimensions/Channels")), "Channel").collect();
    let disp: Vec<Node> = children(child(meta, "DisplaySetting/Channels"), "Channel").collect();
    let find = |list: &[Node<'_, '_>], c: usize, i: usize| -> Option<usize> {
        let id = format!("Channel:{c}");
        list.iter().position(|n| n.attribute("Id") == Some(id.as_str())).or((i < list.len()).then_some(i))
    };
    if !info.is_rgb && info.samples_per_page == 1 {
        for (i, &c) in cs.iter().enumerate() {
            let n = find(&chans, c, i).map(|k| chans[k]);
            let dn = find(&disp, c, i).map(|k| disp[k]);
            let name = n.and_then(|n| n.attribute("Name")).or(dn.and_then(|n| n.attribute("Name")));
            let color = dn
                .and_then(|n| text(n, "Color"))
                .or(n.and_then(|n| text(n, "Color")))
                .and_then(|s| parse_color(&s))
                .filter(|c| c.iter().any(|&v| v > 0));
            let fallback = if cs.len() == 1 { [255, 255, 255] } else { crate::image::DEFAULT_COLORS[i % 7] };
            info.channels.push(ChannelInfo {
                name: name.map(str::to_string).unwrap_or_else(|| format!("C{}", i + 1)),
                color: color.unwrap_or(fallback),
            });
        }
    }

}

/// Reads the metadata XML of the CZI file at `path`.
pub fn metadata_xml_of(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    // SAFETY: read-only mapping; see TiffSource::open.
    let mmap = unsafe { Mmap::map(&file)? };
    let hdr = segment(&mmap, 0, "ZISRAWFILE")?;
    let pos = mmap.get(hdr + 60..hdr + 68).context("CZI 文件头不完整")?;
    metadata_xml(&mmap, i64::from_le_bytes(pos.try_into().unwrap()) as usize)
}
