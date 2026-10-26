//! Zeiss ZEN plugin: reads CZI files (LSM / Airyscan / Axioscan /
//! Celldiscoverer) as a TifMrcView WebAssembly format plugin.
//!
//! A CZI file is a chain of segments, each a 16-byte id (`ZISRAWFILE`,
//! `ZISRAWDIRECTORY`, `ZISRAWSUBBLOCK`, `ZISRAWMETADATA`…) plus sizes. The
//! directory lists every subblock: one tile of pixels with its position
//! along X, Y, C, Z, T, M (mosaic tile), S (scene) and so on. Metadata is
//! one big XML document.
//!
//! We show the full-resolution layer only (pyramid subblocks are skipped),
//! stitching mosaic tiles into one plane per (C, Z, T). The plugin only
//! indexes the file; each page is handed to the host as a recipe (tile
//! offsets, compression and position), which decodes and stitches natively.
//! The XML becomes the pixel size, channel names and colours, and the
//! acquisition settings shown in the image-info panel.

use std::collections::BTreeMap;

mod xml;

use tifmrcview_plugin_sdk::{
    bail, export_format, file, flags, le_i32, le_i64, Channel, Chunk, Codec, Context, DType, Format, Group, Info,
    Recipe, Result,
};
use xml::Node;

const SEG_HEADER: u64 = 32;

const DEFAULT_COLORS: [[u8; 3]; 7] =
    [[0, 255, 0], [255, 0, 255], [0, 255, 255], [255, 0, 0], [0, 0, 255], [255, 255, 0], [255, 255, 255]];

/// One full-resolution tile.
#[derive(Clone)]
struct Tile {
    /// Offset of the ZISRAWSUBBLOCK segment.
    pos: u64,
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

/// Reads the segment header at `pos`, checks its id and returns the start of its data.
fn segment(pos: u64, id: &str) -> Result<u64> {
    let ok = pos.checked_add(SEG_HEADER).is_some_and(|e| e <= file::size())
        && file::read(pos, 16).is_ok_and(|b| b.starts_with(id.as_bytes()));
    if !ok {
        bail!("CZI 文件损坏：找不到 {id}");
    }
    Ok(pos + SEG_HEADER)
}

/// Sequential reader over a large region (the directory), one window at a time.
struct Window {
    start: u64,
    buf: Vec<u8>,
}

impl Window {
    /// `len` bytes at `pos`, or `None` past the end of the file.
    fn get(&mut self, pos: u64, len: usize) -> Option<&[u8]> {
        let end = pos.checked_add(len as u64)?;
        if end > file::size() {
            return None;
        }
        if pos < self.start || end > self.start + self.buf.len() as u64 {
            self.buf = file::read_at_most(pos, len.max(1 << 20)).ok()?;
            self.start = pos;
        }
        let o = (pos - self.start) as usize;
        self.buf.get(o..o + len)
    }
}

pub struct Czi {
    info: Info,
    pixel: Pixel,
    /// Left/top of the stitched plane in CZI coordinates.
    origin: (i64, i64),
    /// Tiles of each page, keyed by (c, z, t) and sorted by M.
    pages: Vec<Vec<Tile>>,
}

impl Format for Czi {
    fn open(_path: &str) -> Result<Czi> {
        let hdr = segment(0, "ZISRAWFILE")?;
        let h = file::read(hdr, 80).map_err(|_| "CZI 文件头不完整")?;
        if le_i32(&h, 48) != 0 {
            bail!("这是多文件 CZI 的分卷，请打开第一个文件");
        }
        let dir_pos = le_i64(&h, 52) as u64;
        let meta_pos = le_i64(&h, 60) as u64;

        // Directory entries.
        let d = segment(dir_pos, "ZISRAWDIRECTORY")?;
        let mut win = Window { start: 0, buf: vec![] };
        let count = le_i32(win.get(d, 4).context("CZI 目录被截断")?, 0).max(0) as usize;
        let mut p = d + 128;
        let mut tiles = vec![];
        let mut pixel_code = None;
        let mut extra_min: BTreeMap<[u8; 4], i32> = BTreeMap::new();
        let mut raw = vec![];
        for _ in 0..count {
            let e = win.get(p, 32).context("CZI 目录被截断")?;
            if &e[..2] != b"DV" {
                bail!("CZI 目录格式未知");
            }
            let ptype = le_i32(e, 2);
            let pos = le_i64(e, 6) as u64;
            let compression = le_i32(e, 18);
            let pyramid = e[22];
            let ndim = le_i32(e, 28).max(0) as usize;
            let e = win.get(p, 32 + ndim * 20).context("CZI 目录被截断")?.to_vec();
            p += 32 + ndim as u64 * 20;
            let mut t = Tile { pos, x: 0, y: 0, w: 0, h: 0, c: 0, z: 0, t: 0, m: 0, compression };
            let mut zoomed = pyramid != 0;
            let mut extras = vec![];
            for k in 0..ndim {
                let de = &e[32 + k * 20..32 + k * 20 + 20];
                let name: [u8; 4] = de[..4].try_into().unwrap();
                let start = le_i32(de, 4);
                let size = le_i32(de, 8);
                let stored = le_i32(de, 16);
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
        let (w, h) = ((x1 - x0) as u64, (y1 - y0) as u64);
        if w.saturating_mul(h) > 1 << 32 || w > u32::MAX as u64 || h > u32::MAX as u64 {
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

        let mut info = Info::new(w as u32, h as u32, pixel.dtype(), "Zeiss ZEN CZI");
        info.size_c = size_c as u32;
        info.size_z = zs.len() as u32;
        info.size_t = ts.len() as u32;
        info.is_rgb = samples > 1 && pixel.dtype() == DType::U8;
        info.samples_per_page = if samples > 1 { size_c as u32 } else { 1 };
        info.page_count = pages.len() as u32;
        if let Ok(xml) = metadata_xml(meta_pos) {
            apply_metadata(&xml, &mut info, &cs);
        }
        Ok(Czi { info, pixel, origin: (x0, y0), pages })
    }

    fn info(&mut self) -> Result<Info> {
        // Pages need only the tile index; hand the info over without a copy.
        Ok(std::mem::replace(&mut self.info, Info::new(1, 1, DType::U8, "")))
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        let tiles = &self.pages[(page as usize).min(self.pages.len() - 1)];
        let mut r = Recipe::new();
        for t in tiles {
            r.push(self.tile_chunk(t)?);
        }
        Ok(r)
    }
}

impl Czi {
    /// Where the pixels of one tile are and how they are stored.
    fn tile_chunk(&self, t: &Tile) -> Result<Chunk> {
        let d = segment(t.pos, "ZISRAWSUBBLOCK")?;
        let head = file::read(d, 48).map_err(|_| "CZI 子块被截断")?;
        let meta_size = le_i32(&head, 0).max(0) as u64;
        let data_size = le_i64(&head, 8).max(0) as u64;
        // The fixed part holds the directory entry and is padded to 256 bytes.
        let ndim = le_i32(&head, 16 + 28).max(0) as u64;
        let fixed = (16 + 32 + ndim * 20).max(256);
        let start = d + fixed + meta_size;
        if start.checked_add(data_size).is_none_or(|e| e > file::size()) {
            bail!("CZI 子块被截断");
        }
        let rect = [(t.x - self.origin.0) as u32, (t.y - self.origin.1) as u32, t.w as u32, t.h as u32];
        let samples = self.pixel.samples() as u32;
        let bgr = if samples > 1 { flags::BGR } else { 0 };
        let chunk = Chunk::file(start, data_size, rect).samples(samples);
        Ok(match t.compression {
            0 => chunk.flags(bgr),
            5 => chunk.codec(Codec::Zstd).flags(bgr),
            6 => {
                // zstd1: a small header, then a zstd stream. With the hi/lo
                // flag the low bytes of all 16-bit samples come first.
                let h = file::read_at_most(start, 3.min(data_size as usize))?;
                let n = *h.first().context("CZI 子块为空")? as u64;
                let hilo = n >= 3 && h.get(1) == Some(&1) && h.get(2).is_some_and(|b| b & 1 == 1);
                if n > data_size {
                    bail!("CZI 子块被截断");
                }
                let mut c = Chunk::file(start + n, data_size - n, rect).samples(samples).codec(Codec::Zstd).flags(bgr);
                if hilo {
                    c = c.flags(flags::HILO);
                }
                c
            }
            // JPEG decodes to gray or RGB, already in display order.
            1 => chunk.codec(Codec::Jpeg).samples(if samples > 1 { 3 } else { 1 }),
            4 => bail!("这个 CZI 用 JPEG XR 压缩，暂不支持；请在 ZEN 里另存为不压缩或 Zstd 压缩"),
            c => bail!("不支持的 CZI 压缩方式 {c}"),
        })
    }
}

export_format!(Czi);

/// The metadata XML of the file (UTF-8, not validated as a whole).
fn metadata_xml(pos: u64) -> Result<Vec<u8>> {
    let d = segment(pos, "ZISRAWMETADATA")?;
    let size = le_i32(&file::read(d, 4)?, 0).max(0) as usize;
    Ok(file::read(d + 256, size).map_err(|_| "CZI 元数据被截断")?)
}

// ---------------------------------------------------------------- metadata

fn child<'d, 'a>(n: Node<'d, 'a>, path: &str) -> Option<Node<'d, 'a>> {
    path.split('/').try_fold(n, |n, name| n.children().find(|c| c.has_tag_name(name)))
}

fn text(n: Node, path: &str) -> Option<String> {
    child(n, path).and_then(|c| c.text()).map(|t| t.trim().to_string()).filter(|s| !s.is_empty())
}

fn attr(n: Node, name: &str) -> Option<String> {
    n.attribute(name).map(|a| a.into_owned())
}

fn children<'d, 'a>(n: Option<Node<'d, 'a>>, name: &'static str) -> impl Iterator<Item = Node<'d, 'a>> {
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

/// Fills pixel size, channel names/colours, the format name and the
/// acquisition settings from the XML. `cs` are the C indices present in
/// the file, in order.
fn apply_metadata(xml: &[u8], info: &mut Info, cs: &[usize]) {
    let doc = xml::Doc::parse(xml);
    let Some(meta) = doc.root().and_then(|r| child(r, "Metadata")) else { return };
    let inf = child(meta, "Information");
    let image = inf.and_then(|i| child(i, "Image"));

    // Pixel size: metres in the file, shown in µm.
    let ps = scaling_um(meta);
    if ps[0] > 0.0 {
        info.pixel_size = Some([ps[0], if ps[1] > 0.0 { ps[1] } else { ps[0] }, ps[2]]);
        info.unit = "µm".into();
    }

    // Channels, matched to C by their "Channel:N" id, else by order.
    let chans: Vec<Node> = children(image.and_then(|i| child(i, "Dimensions/Channels")), "Channel").collect();
    let disp: Vec<Node> = children(child(meta, "DisplaySetting/Channels"), "Channel").collect();
    let find = |list: &[Node<'_, '_>], c: usize, i: usize| -> Option<usize> {
        let id = format!("Channel:{c}");
        list.iter().position(|n| n.attribute("Id").as_deref() == Some(id.as_str())).or((i < list.len()).then_some(i))
    };
    if !info.is_rgb && info.samples_per_page == 1 {
        for (i, &c) in cs.iter().enumerate() {
            let n = find(&chans, c, i).map(|k| chans[k]);
            let dn = find(&disp, c, i).map(|k| disp[k]);
            let name = n.and_then(|n| attr(n, "Name")).or(dn.and_then(|n| attr(n, "Name")));
            let color = dn
                .and_then(|n| text(n, "Color"))
                .or(n.and_then(|n| text(n, "Color")))
                .and_then(|s| parse_color(&s))
                .filter(|c| c.iter().any(|&v| v > 0));
            let fallback = if cs.len() == 1 { [255, 255, 255] } else { DEFAULT_COLORS[i % 7] };
            info.channels.push(Channel {
                name: name.unwrap_or_else(|| format!("C{}", i + 1)),
                color: color.unwrap_or(fallback),
            });
        }
    }

    if let Some(v) = text(meta, "Information/Application/Version") {
        info.format = format!("Zeiss ZEN CZI (ZEN {v})");
    }
    info.metadata = groups(meta, &ps);
}

/// (x, y, z) pixel size in µm; 0 where the file does not say.
fn scaling_um(meta: Node) -> [f64; 3] {
    let mut ps = [0.0f64; 3];
    for d in children(child(meta, "Scaling/Items"), "Distance") {
        let v: f64 = text(d, "Value").and_then(|v| v.parse().ok()).unwrap_or(0.0) * 1e6;
        match d.attribute("Id").as_deref() {
            Some("X") => ps[0] = v,
            Some("Y") => ps[1] = v,
            Some("Z") => ps[2] = v,
            _ => {}
        }
    }
    ps
}

/// Trims float noise such as 0.030000000000000138 to a few decimals.
fn num(s: &str) -> String {
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() => {
            let r = format!("{v:.4}");
            let r = r.trim_end_matches('0').trim_end_matches('.');
            if (r == "0" || r == "-0") && v != 0.0 {
                format!("{v:.3e}")
            } else {
                r.to_string()
            }
        }
        _ => s.to_string(),
    }
}

/// "2026-10-15T10:12:03.98+08:00" -> "2026-10-15 10:12:03 +08:00".
fn tidy_date(d: &str) -> String {
    let Some((day, time)) = d.split_once('T') else { return d.to_string() };
    let zone_at = time.find(['+', '-', 'Z']).unwrap_or(time.len());
    let (clock, zone) = time.split_at(zone_at);
    let clock = clock.split('.').next().unwrap_or(clock);
    let zone = if zone == "Z" { "UTC" } else { zone };
    format!("{day} {clock} {zone}").trim_end().to_string()
}

/// Group that also skips ZEN's "NaN" placeholders.
struct G(Group);

impl G {
    fn new(name: &str) -> G {
        G(Group::new(name))
    }
    fn add(&mut self, k: &str, v: Option<String>) {
        self.0.add(k, v.filter(|v| v != "NaN"));
    }
    fn push_to(self, out: &mut Vec<Group>) {
        self.0.push_to(out)
    }
}

/// Groups shown in the info panel, most useful first.
fn groups(meta: Node, ps: &[f64; 3]) -> Vec<Group> {
    let inf = child(meta, "Information");
    let image = inf.and_then(|i| child(i, "Image"));
    let instrument = inf.and_then(|i| child(i, "Instrument"));
    let chans: Vec<Node> = children(image.and_then(|i| child(i, "Dimensions/Channels")), "Channel").collect();

    let mut out = vec![];
    let mut g = G::new("ZEN 图像");
    let app = inf.and_then(|i| child(i, "Application"));
    g.add(
        "软件",
        app.map(|a| format!("{} {}", text(a, "Name").unwrap_or_default(), text(a, "Version").unwrap_or_default()).trim().to_string()),
    );
    if let Some(i) = image {
        // The document date carries the local time zone; the acquisition time is UTC.
        let date = inf.and_then(|i| text(i, "Document/CreationDate")).or_else(|| text(i, "AcquisitionDateAndTime"));
        g.add("采集时间", date.map(|d| tidy_date(&d)));
        g.add("像素类型", text(i, "PixelType"));
        g.add("位深", text(i, "ComponentBitCount"));
        let dims: Vec<String> = ["X", "Y", "Z", "C", "T", "M", "S", "H"]
            .iter()
            .filter_map(|d| text(i, &format!("Size{d}")).filter(|v| v != "1" || matches!(*d, "X" | "Y")).map(|v| format!("{d}={v}")))
            .collect();
        g.add("尺寸", Some(dims.join("  ")));
        g.add("原始压缩", text(i, "OriginalCompressionMethod"));
    }
    if ps[0] > 0.0 {
        let f = |v: f64| format!("{:.4} µm", v);
        g.add("像素尺寸 X", Some(f(ps[0])));
        g.add("像素尺寸 Y", Some(f(ps[1])));
        if ps[2] > 0.0 {
            g.add("Z 步长", Some(f(ps[2])));
        }
    }
    g.add("用户", inf.and_then(|i| text(i, "Document/UserName")));
    g.push_to(&mut out);

    let mut g = G::new("ZEN 显微镜");
    if let Some(ins) = instrument {
        for m in children(child(ins, "Microscopes"), "Microscope") {
            g.add("显微镜", attr(m, "Name"));
            g.add("系统", text(m, "System"));
            g.add("类型", text(m, "Type"));
            g.add("名称", text(m, "UserDefinedName"));
        }
        for o in children(child(ins, "Objectives"), "Objective") {
            g.add("物镜", attr(o, "Name").or_else(|| text(o, "Manufacturer/Model")));
            g.add("放大倍数", text(o, "NominalMagnification"));
            g.add("数值孔径 NA", text(o, "LensNA"));
            g.add("浸没介质", text(o, "Immersion"));
            g.add("工作距离 (µm)", text(o, "WorkingDistance"));
        }
    }
    if let Some(i) = image {
        g.add("总放大倍数", text(i, "MicroscopeSettings/EyepieceSettings/TotalMagnification"));
    }
    g.push_to(&mut out);

    for (i, &n) in chans.iter().enumerate() {
        let name = attr(n, "Name").unwrap_or_default();
        let mut g = G::new(&format!("通道 {}：{}", i + 1, name));
        g.add("荧光染料", text(n, "Fluor"));
        g.add("激发波长 (nm)", text(n, "ExcitationWavelength").map(|v| num(&v)));
        g.add("发射波长 (nm)", text(n, "EmissionWavelength").map(|v| num(&v)));
        g.add("检测范围 (nm)", text(n, "DetectionWavelength/Ranges"));
        g.add("采集模式", text(n, "AcquisitionMode"));
        g.add("对比方式", text(n, "ContrastMethod"));
        g.add("照明", text(n, "IlluminationType"));
        let lights = children(child(n, "LightSourcesSettings"), "LightSourceSettings");
        for l in lights {
            let wl = text(l, "Wavelength").map(|v| num(&v));
            let tr = text(l, "Transmission")
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| format!("{}%", num(&(v * 100.0).to_string())));
            g.add(
                "激光",
                match (wl, tr) {
                    (Some(w), Some(t)) => Some(format!("{w} nm, {t}")),
                    (w, t) => w.map(|w| format!("{w} nm")).or(t),
                },
            );
        }
        g.add("曝光时间 (ms)", text(n, "ExposureTime").and_then(|v| v.parse::<f64>().ok()).map(|v| num(&format!("{}", v / 1e6))));
        g.add("针孔 (AU)", text(n, "PinholeSizeAiry").map(|v| num(&v)));
        g.add("针孔 (µm)", text(n, "PinholeSize").map(|v| num(&v)));
        if let Some(det) = child(n, "DetectorSettings") {
            g.add(
                "探测器",
                child(det, "Detector").and_then(|d| attr(d, "Id")).map(|s| s.trim_start_matches("Detector:").trim().to_string()),
            );
            g.add("探测器电压 (V)", text(det, "Voltage").map(|v| num(&v)));
            g.add("数字增益", text(det, "DigitalGain").map(|v| num(&v)));
            g.add("偏移", text(det, "Offset").map(|v| num(&v)));
            g.add("分档", text(det, "Binning"));
        }
        if let Some(ls) = child(n, "LaserScanInfo") {
            g.add("像素驻留 (µs)", text(ls, "PixelTime").and_then(|v| v.parse::<f64>().ok()).map(|v| num(&format!("{}", v * 1e6))));
            g.add("帧时间 (s)", text(ls, "FrameTime").map(|v| num(&v)));
            g.add("平均次数", text(ls, "Averaging"));
            g.add("扫描速度", text(ls, "ScanSpeed"));
            g.add("扫描缩放", text(ls, "ZoomX").map(|v| num(&v)));
            g.add("扫描方向", text(ls, "ScanDirection"));
        }
        if let Some(a) = child(n, "AiryscanSettings") {
            g.add("Airyscan 模式", text(a, "Mode"));
        }
        g.push_to(&mut out);
    }

    if let Some(proc_) = inf.and_then(|i| child(i, "Processing")) {
        let mut g = G::new("ZEN 处理");
        for c in children(child(proc_, "Airyscan/Channels"), "Channel") {
            // "Channel:0" is the first channel.
            let n = attr(c, "Id").and_then(|id| id.strip_prefix("Channel:")?.parse::<usize>().ok()).unwrap_or(0) + 1;
            g.add(&format!("Airyscan 强度（通道 {n}）"), text(c, "SuperResolutionParameter").map(|v| num(&v)));
            g.add(&format!("Airyscan 参数估计（通道 {n}）"), text(c, "ParameterEstimationType"));
            g.add(&format!("Airyscan 反卷积（通道 {n}）"), text(c, "DeconvolutionType"));
        }
        g.add("采样因子", text(proc_, "LSM/SamplingFactor").map(|v| num(&v)));
        g.push_to(&mut out);
    }

    // Everything under Information, flattened, for search.
    if let Some(i) = inf {
        let mut g = G::new("ZEN 全部参数");
        i.doc().leaves(i, |path, text| g.0.add_str(path, text));
        g.push_to(&mut out);
    }
    out
}

