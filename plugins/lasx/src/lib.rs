//! Leica LAS X plugin: reads LIF projects and LOF exports (LAS X, LAS AF) as
//! a TifMrcView WebAssembly format plugin.
//!
//! The file layout follows the public description in Christoph Gohlke's
//! liffile (BSD-3-Clause, https://github.com/cgohlke/liffile); the code is
//! written from scratch.
//!
//! A LIF file is a header block holding one UTF-16 XML document, followed by
//! memory blocks (a small header with the block's id, then raw bytes). The
//! XML describes every image of the project: its dimensions and channels with
//! their byte strides (`BytesInc`), and which memory block holds the pixels.
//! A LOF file is one memory block followed by the XML of its single image.
//! Pixels are never compressed, so each plane is one chunk of the file.
//!
//! Every image that has pixels becomes a series (named by its path in the
//! element tree). Z, T and any further dimensions (wavelength, loop…) are
//! folded into the Z and T axes; mosaic tiles (`DimID` 10) are stitched
//! using the positions of the `TileScanInfo` attachment when they form a
//! simple grid, otherwise only the first tile is shown. The XML becomes the
//! pixel size, channel colours (from LUT names) and acquisition settings.

use std::borrow::Cow;

use tifmrcview_plugin_sdk::xml::{self, Node};
use tifmrcview_plugin_sdk::{
    bail, export_format, file, flags, le_u32, le_u64, Channel, Chunk, Context, DType, Format, Group, Info, Recipe,
    Result, Series,
};

const DEFAULT_COLORS: [[u8; 3]; 7] =
    [[0, 255, 0], [255, 0, 255], [0, 255, 255], [255, 0, 0], [0, 0, 255], [255, 255, 0], [255, 255, 255]];

/// Most items the flattened "all settings" group will hold.
const MAX_ITEMS: usize = 40_000;

// ------------------------------------------------------------------ container

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Lif,
    Lof,
}

/// The header XML and where the memory blocks start.
struct Container {
    kind: Kind,
    /// The header document, UTF-8.
    xml: Vec<u8>,
    /// LIF: first memory block. LOF: unused.
    blocks: u64,
    /// LOF: (offset, size) of the one memory block.
    lof_block: Option<(u64, u64)>,
}

/// UTF-16LE to UTF-8, with a fast path for ASCII (almost all of the XML).
fn utf16_to_utf8(b: &[u8]) -> Vec<u8> {
    let n = b.len() & !1;
    let mut out = Vec::with_capacity(n / 2 + 16);
    let mut i = 0;
    while i < n {
        let u = b[i] as u32 | (b[i + 1] as u32) << 8;
        i += 2;
        if u < 0x80 {
            out.push(u as u8);
            continue;
        }
        let mut c = u;
        if (0xD800..0xDC00).contains(&u) && i + 1 < n {
            let lo = b[i] as u32 | (b[i + 1] as u32) << 8;
            if (0xDC00..0xE000).contains(&lo) {
                c = 0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00);
                i += 2;
            }
        }
        let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
    out
}

/// One `0x70 len 0x2A chars <utf-16>` XML block at `pos`: (xml, end of block).
fn read_xml_block(pos: u64) -> Result<(Vec<u8>, u64)> {
    let h = file::read(pos, 13).map_err(|_| "不是徕卡 LIF/LOF 文件")?;
    if le_u32(&h, 0) != 0x70 || h[8] != 0x2A {
        bail!("不是徕卡 LIF/LOF 文件");
    }
    let chars = le_u32(&h, 9) as u64;
    let end = pos + 13 + chars * 2;
    if end > file::size() {
        bail!("文件被截断：XML 头不完整");
    }
    let raw = file::read(pos + 13, (chars * 2) as usize)?;
    Ok((utf16_to_utf8(&raw), end))
}

fn container() -> Result<Container> {
    let (xml, end) = read_xml_block(0)?;
    if xml == b"LMS_Object_File" {
        // 0x2A, version, 0x2A, version, then the memory block: 0x2A, u64 size.
        let h = file::read(end, 19).map_err(|_| "LOF 文件被截断")?;
        if h[0] != 0x2A || h[5] != 0x2A || h[10] != 0x2A {
            bail!("LOF 文件损坏：数据块标记不对");
        }
        let size = le_u64(&h, 11);
        let data = end + 19;
        if data.checked_add(size).is_none_or(|e| e > file::size()) {
            bail!("LOF 文件被截断：像素数据不完整");
        }
        let (xml, _) = read_xml_block(data + size).map_err(|_| "LOF 文件损坏：找不到图像描述")?;
        return Ok(Container { kind: Kind::Lof, xml, blocks: 0, lof_block: Some((data, size)) });
    }
    Ok(Container { kind: Kind::Lif, xml, blocks: end, lof_block: None })
}

/// (offset, size) of the memory block called `id` in a LIF file, found by
/// walking the block headers (the data between them is skipped).
fn find_block(c: &Container, version: u32, id: &str) -> Result<(u64, u64)> {
    let want: Vec<u8> = id.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    // 0x70, length, 0x2A, size (u32 in version 1, u64 later), 0x2A, id length.
    let (hdr, size_len) = if version >= 2 { (22usize, 8usize) } else { (18, 4) };
    let fsize = file::size();
    let mut pos = c.blocks;
    while pos + hdr as u64 <= fsize {
        let b = file::read_at_most(pos, hdr + want.len())?;
        if b.len() < hdr || le_u32(&b, 0) != 0x70 || b[8] != 0x2A || b[9 + size_len] != 0x2A {
            bail!("LIF 文件损坏：数据块头不对");
        }
        let size = if size_len == 8 { le_u64(&b, 9) } else { le_u32(&b, 9) as u64 };
        let id_len = le_u32(&b, 10 + size_len) as u64 * 2;
        let data = pos + hdr as u64 + id_len;
        if id_len == want.len() as u64 && b.len() >= hdr + want.len() && b[hdr..] == want[..] {
            return Ok((data, size));
        }
        pos = data.saturating_add(size);
    }
    bail!("LIF 文件里找不到图像数据块 {id}")
}

// ------------------------------------------------------------------ XML helpers

fn child<'d, 'a>(n: Node<'d, 'a>, path: &str) -> Option<Node<'d, 'a>> {
    path.split('/').try_fold(n, |n, name| n.children().find(|c| c.has_tag_name(name)))
}

fn children<'d, 'a>(n: Option<Node<'d, 'a>>, name: &'static str) -> impl Iterator<Item = Node<'d, 'a>> {
    n.into_iter().flat_map(move |n| n.children().filter(move |c| c.has_tag_name(name)))
}

fn attr(n: Node, name: &str) -> Option<String> {
    n.attribute(name).map(|a| a.into_owned())
}

/// Attribute with blanks trimmed and empty values dropped.
fn val(n: Node, name: &str) -> Option<String> {
    n.attribute(name).map(|a| a.trim().to_string()).filter(|s| !s.is_empty())
}

fn fnum(n: Node, name: &str) -> Option<f64> {
    n.attribute(name).and_then(|a| a.trim().parse::<f64>().ok())
}

fn unum(n: Node, name: &str) -> u64 {
    n.attribute(name).and_then(|a| a.trim().parse::<u64>().ok()).unwrap_or(0)
}

fn text(n: Node, path: &str) -> Option<String> {
    child(n, path).and_then(|c| c.text()).map(|t| t.trim().to_string()).filter(|s| !s.is_empty())
}

/// Trims float noise such as 0.030000000000000138 to a few decimals.
fn num(v: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    let r = format!("{v:.4}");
    let r = r.trim_end_matches('0').trim_end_matches('.');
    if (r == "0" || r == "-0") && v != 0.0 {
        format!("{v:.3e}")
    } else {
        r.to_string()
    }
}

fn numstr(s: &str) -> String {
    s.trim().parse::<f64>().map(num).unwrap_or_else(|_| s.to_string())
}

/// Collapses runs of blanks ("HC PL APO CS2    63x/1.40 OIL " -> one space each).
fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Metres per unit name, for lengths.
fn unit_scale(unit: &str) -> Option<f64> {
    Some(match unit.trim() {
        "m" => 1.0,
        "mm" => 1e-3,
        "µm" | "μm" | "um" => 1e-6,
        "nm" => 1e-9,
        _ => return None,
    })
}

// ------------------------------------------------------------------ layout

#[derive(Clone, Debug)]
struct Dim {
    id: u32,
    n: u64,
    length: f64,
    unit: String,
    inc: u64,
}

impl Dim {
    /// Spacing in µm between samples (Length covers n-1 steps), if the unit is a length.
    fn step_um(&self) -> Option<f64> {
        let s = unit_scale(&self.unit)?;
        (self.n > 1 && self.length != 0.0).then(|| self.length.abs() / (self.n - 1) as f64 * s * 1e6)
    }
}

struct Chan {
    tag: u32,
    bits: u32,
    float: bool,
    lut: String,
    inc: u64,
    detector: Option<String>,
    dye: Option<String>,
}

impl Chan {
    fn dtype(&self) -> Result<DType> {
        Ok(match (self.float, self.bits) {
            (false, 1..=8) => DType::U8,
            (false, 9..=16) => DType::U16,
            (false, 17..=32) => DType::U32,
            (true, 17..=32) => DType::F32,
            (true, 33..=64) => DType::F64,
            (f, b) => bail!("不支持的 LIF 像素类型（{b} 位{}）", if f { "浮点" } else { "整数" }),
        })
    }
}

/// Where the planes of one image sit inside its memory block.
struct Layout {
    w: u32,
    h: u32,
    x_inc: u64,
    y_inc: u64,
    dtype: DType,
    bits: u32,
    /// Sorted by byte offset.
    chans: Vec<Chan>,
    /// All channels interleaved pixel by pixel (RGB and the like).
    interleaved: bool,
    bgr: bool,
    z: Option<Dim>,
    /// T and any other dimensions, fastest (smallest stride) first.
    t_axes: Vec<Dim>,
    tile: Option<Dim>,
    x: Dim,
    y: Dim,
}

impl Layout {
    fn parse(data: Node) -> Result<Layout> {
        let desc = child(data, "Image/ImageDescription").context("缺少 ImageDescription")?;
        let mut chans = vec![];
        for c in children(child(desc, "Channels"), "ChannelDescription") {
            let (mut detector, mut dye) = (None, None);
            for p in children(Some(c), "ChannelProperty") {
                let (Some(k), Some(v)) = (text(p, "Key"), text(p, "Value")) else { continue };
                match k.as_str() {
                    "DetectorName" => detector = Some(v),
                    "DyeName" => dye = Some(v.trim_start_matches("Leica/").trim().to_string()),
                    _ => {}
                }
            }
            chans.push(Chan {
                tag: unum(c, "ChannelTag") as u32,
                bits: unum(c, "Resolution") as u32,
                float: unum(c, "DataType") == 1,
                lut: attr(c, "LUTName").unwrap_or_default(),
                inc: unum(c, "BytesInc"),
                detector,
                dye: dye.filter(|d| !d.is_empty()),
            });
        }
        if chans.is_empty() {
            bail!("LIF 图像没有通道描述");
        }
        chans.sort_by_key(|c| c.inc);
        let dtype = chans[0].dtype()?;
        for c in &chans {
            if c.dtype()? != dtype {
                bail!("暂不支持各通道像素类型不同的 LIF 图像");
            }
        }

        let (mut x, mut y, mut z, mut tile) = (None, None, None, None);
        let mut others = vec![];
        for d in children(child(desc, "Dimensions"), "DimensionDescription") {
            let d = Dim {
                id: unum(d, "DimID") as u32,
                n: unum(d, "NumberOfElements"),
                length: fnum(d, "Length").unwrap_or(0.0),
                unit: attr(d, "Unit").unwrap_or_default(),
                inc: unum(d, "BytesInc"),
            };
            match d.id {
                1 => x = Some(d),
                2 => y = Some(d),
                3 if d.n > 1 => z = Some(d),
                10 if d.n > 1 => tile = Some(d),
                0 | 3 | 10 => {}
                _ if d.n > 1 => others.push(d),
                _ => {}
            }
        }
        let (x, y) = (x.context("LIF 图像缺少 X 维度")?, y.context("LIF 图像缺少 Y 维度")?);
        if x.n == 0 || y.n == 0 || x.n > u32::MAX as u64 || y.n > u32::MAX as u64 {
            bail!("LIF 图像尺寸不对");
        }
        others.sort_by_key(|d| d.inc);

        // Channels either sit in separate blocks (planar) or inside each pixel.
        let item = dtype.size() as u64;
        let nch = chans.len() as u64;
        let interleaved =
            nch > 1 && x.inc == nch * item && chans.iter().enumerate().all(|(i, c)| c.inc == i as u64 * item);
        if !interleaved && x.inc != item {
            bail!("不支持的 LIF 像素排列（X 方向步长 {}，每像素 {} 字节）", x.inc, item);
        }
        let row = x.n * if interleaved { x.inc } else { item };
        if y.inc < row {
            bail!("不支持的 LIF 像素排列（Y 方向步长 {} 小于一行 {}）", y.inc, row);
        }
        let bgr = interleaved && nch == 3 && chans[0].tag == 3 && chans[1].tag == 2;
        let bits = chans[0].bits;
        Ok(Layout {
            w: x.n as u32,
            h: y.n as u32,
            x_inc: x.inc,
            y_inc: y.inc,
            dtype,
            bits,
            chans,
            interleaved,
            bgr,
            z,
            t_axes: others,
            tile,
            x,
            y,
        })
    }

    fn size_z(&self) -> u64 {
        self.z.as_ref().map_or(1, |d| d.n)
    }

    fn size_t(&self) -> u64 {
        self.t_axes.iter().map(|d| d.n).product()
    }

    fn size_c(&self) -> usize {
        self.chans.len()
    }

    fn pages(&self) -> u64 {
        (if self.interleaved { 1 } else { self.chans.len() as u64 }) * self.size_z() * self.size_t()
    }

    fn is_rgb(&self) -> bool {
        self.interleaved && self.chans.len() == 3 && self.dtype == DType::U8
    }

    fn tiles(&self) -> u64 {
        self.tile.as_ref().map_or(1, |d| d.n)
    }

    /// "1024×1024 · 2C · 20Z · 16 bit", for the series picker.
    fn detail(&self) -> String {
        let mut s = format!("{}×{}", self.w, self.h);
        if self.is_rgb() {
            s.push_str(" · RGB");
        } else if self.size_c() > 1 {
            s.push_str(&format!(" · {}C", self.size_c()));
        }
        if self.size_z() > 1 {
            s.push_str(&format!(" · {}Z", self.size_z()));
        }
        if self.size_t() > 1 {
            s.push_str(&format!(" · {}T", self.size_t()));
        }
        if self.tiles() > 1 {
            s.push_str(&format!(" · {} 拼块", self.tiles()));
        }
        s.push_str(&format!(" · {} bit", self.bits));
        s
    }
}

// ------------------------------------------------------------------ images of the project

/// One element of the tree that holds pixels.
struct Img<'d, 'a> {
    /// Path in the element tree, without the root element ("Experiment/Series003").
    path: String,
    /// The `Data` element (holds `Image`).
    data: Node<'d, 'a>,
    mem_id: String,
}

fn walk<'d, 'a>(e: Node<'d, 'a>, prefix: &str, depth: u32, lof_size: Option<u64>, out: &mut Vec<Img<'d, 'a>>) {
    let name = attr(e, "Name").unwrap_or_default();
    let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
    let mem = child(e, "Memory");
    let size = lof_size.or(mem.map(|m| unum(m, "Size"))).unwrap_or(0);
    if let Some(data) = child(e, "Data").filter(|d| child(*d, "Image/ImageDescription").is_some()) {
        if size > 0 {
            let path = if path.is_empty() { "图像".to_string() } else { path.clone() };
            out.push(Img { path, data, mem_id: mem.and_then(|m| attr(m, "MemoryBlockID")).unwrap_or_default() });
        }
    }
    if depth < 64 {
        // The root element's own name is not part of the path.
        let next = if depth == 0 { "" } else { path.as_str() };
        for c in children(child(e, "Children"), "Element") {
            walk(c, next, depth + 1, None, out);
        }
    }
}

fn images<'d, 'a>(doc: &'d xml::Doc<'a>, lof_size: Option<u64>) -> Vec<Img<'d, 'a>> {
    let mut out = vec![];
    let Some(root) = doc.root() else { return out };
    if root.has_tag_name("Data") {
        // Old LOF files: the document is just <Data><Image>…
        if child(root, "Image/ImageDescription").is_some() {
            out.push(Img { path: "图像".into(), data: root, mem_id: String::new() });
        }
        return out;
    }
    for e in children(Some(root), "Element") {
        walk(e, "", 0, lof_size, &mut out);
    }
    out
}

// ------------------------------------------------------------------ the plugin

pub struct Lasx {
    info: Info,
    layout: Layout,
    /// Start and end of the image's memory block in the file.
    mem: (u64, u64),
    /// Position of each tile in the stitched page.
    tiles: Vec<(u32, u32)>,
}

impl Format for Lasx {
    fn open(_path: &str) -> Result<Lasx> {
        Lasx::build(0)
    }

    fn open_series(_path: &str, series: u32) -> Result<Lasx> {
        Lasx::build(series as usize)
    }

    fn info(&mut self) -> Result<Info> {
        Ok(std::mem::replace(&mut self.info, Info::new(1, 1, DType::U8, "")))
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        let l = &self.layout;
        let nch = l.chans.len() as u64;
        let page = page as u64;
        let (c, zt) = if l.interleaved { (0, page) } else { ((page % nch) as usize, page / nch) };
        let sz = l.size_z();
        let (z, mut t) = (zt % sz, zt / sz);
        let mut base = self.mem.0 + z * l.z.as_ref().map_or(0, |d| d.inc);
        for d in &l.t_axes {
            base += (t % d.n) * d.inc;
            t /= d.n;
        }
        if !l.interleaved {
            base += l.chans[c].inc;
        }
        let samples = if l.interleaved { nch as u32 } else { 1 };
        let item = l.dtype.size() as u64;
        let row = l.w as u64 * if l.interleaved { l.x_inc } else { item };
        let len = l.y_inc * (l.h as u64 - 1) + row;
        let stride = if l.y_inc == row { 0 } else { l.y_inc as u32 };
        let tile_inc = l.tile.as_ref().map_or(0, |d| d.inc);
        let mut r = Recipe::new();
        for (m, &(x, y)) in self.tiles.iter().enumerate() {
            let start = base + m as u64 * tile_inc;
            // A scan that was stopped early leaves the last planes out: they stay blank.
            if start.checked_add(len).is_none_or(|e| e > self.mem.1) {
                continue;
            }
            let mut ch = Chunk::file(start, len, [x, y, l.w, l.h]).samples(samples).stride(stride);
            if l.bgr {
                ch = ch.flags(flags::BGR);
            }
            r.push(ch);
        }
        Ok(r)
    }
}

impl Lasx {
    fn build(series: usize) -> Result<Lasx> {
        let c = container()?;
        let doc = xml::Doc::parse(&c.xml);
        let version = doc.root().and_then(|r| r.attribute("Version")).and_then(|v| v.trim().parse().ok()).unwrap_or(2);
        let imgs = images(&doc, c.lof_block.map(|b| b.1));
        if imgs.is_empty() {
            bail!("这个文件里没有带像素的图像");
        }
        let img = imgs.get(series).context("文件里没有这个图像")?;
        let layout = Layout::parse(img.data)?;

        let (off, size) = match c.lof_block {
            Some(b) => b,
            None => find_block(&c, version, &img.mem_id)?,
        };
        let mem = (off, off + size.min(file::size().saturating_sub(off)));

        let mut info =
            Info::new(layout.w, layout.h, layout.dtype, if c.kind == Kind::Lof { "Leica LOF" } else { "Leica LIF" });
        info.size_z = layout.size_z() as u32;
        info.size_t = layout.size_t() as u32;
        if layout.pages() > u32::MAX as u64 / 4 {
            bail!("LIF 图像的平面数太多");
        }
        info.page_count = layout.pages() as u32;
        if layout.interleaved {
            info.size_c = layout.size_c() as u32;
            info.samples_per_page = layout.size_c() as u32;
            info.is_rgb = layout.is_rgb();
        } else {
            info.size_c = layout.size_c() as u32;
        }

        // Tiles: stitched by position when they form a simple grid.
        let tinfo = TileInfo::new(&layout, img.data);
        let (tiles, w, h) = tinfo.place(&layout);
        info.width = w;
        info.height = h;

        let hw = Hardware::new(img.data);
        if !info.is_rgb {
            for (i, ch) in layout.chans.iter().enumerate() {
                let det = hw.as_ref().and_then(|h| h.detector_for(i, ch));
                let name = ch
                    .dye
                    .clone()
                    .or_else(|| det.as_ref().and_then(|d| val(d.node, "DyeName")).map(|n| n.trim_start_matches("Leica/").trim().to_string()))
                    .filter(|n| !n.is_empty())
                    .or_else(|| ch.detector.clone())
                    .or_else(|| det.as_ref().and_then(|d| val(d.node, "Name")))
                    .unwrap_or_else(|| format!("C{}", i + 1));
                info.channels.push(Channel { name, color: lut_color(&ch.lut).unwrap_or(DEFAULT_COLORS[i % 7]) });
            }
            if info.channels.len() == 1 && lut_color(&layout.chans[0].lut).is_none() {
                info.channels[0].color = [255, 255, 255];
            }
        }

        let (px, py, pz) = (layout.x.step_um(), layout.y.step_um(), layout.z.as_ref().and_then(|d| d.step_um()));
        if let Some(px) = px {
            info.pixel_size = Some([px, py.unwrap_or(px), pz.unwrap_or(0.0)]);
            info.unit = "µm".into();
        }
        if let Some(sw) = hw.as_ref().and_then(|h| val(h.setting, "Software")) {
            info.format = format!("{} ({sw})", info.format);
        }

        info.metadata = groups(&c, &layout, img, hw.as_ref(), &tinfo, (w, h), (px, py, pz));
        info.series =
            imgs.iter().map(|i| Series { name: i.path.clone(), detail: Layout::parse(i.data).map_or_else(|e| e.0, |l| l.detail()) }).collect();
        info.series_index = series as u32;
        Ok(Lasx { info, layout, mem, tiles })
    }
}

export_format!(Lasx);

// ------------------------------------------------------------------ tiles

struct TileInfo {
    /// (x, y) stage position in metres of each tile, when the file gives them.
    pos: Vec<(f64, f64)>,
    swap_xy: bool,
}

impl TileInfo {
    fn new(l: &Layout, data: Node) -> TileInfo {
        let mut t = TileInfo { pos: vec![], swap_xy: false };
        if l.tiles() < 2 {
            return t;
        }
        let att = children(child(data, "Image"), "Attachment").find(|a| a.attribute("Name").as_deref() == Some("TileScanInfo"));
        if let Some(a) = att {
            t.swap_xy = a.attribute("SwapXY").is_some_and(|v| v.trim() == "1");
            t.pos = children(Some(a), "Tile")
                .map(|n| (fnum(n, "PosX").unwrap_or(f64::NAN), fnum(n, "PosY").unwrap_or(f64::NAN)))
                .collect();
        }
        t
    }

    /// Tile offsets in the stitched page and the page size. Without usable
    /// positions (or when the grid would be unreasonably large) only tile 0.
    fn place(&self, l: &Layout) -> (Vec<(u32, u32)>, u32, u32) {
        let single = (vec![(0, 0)], l.w, l.h);
        let n = l.tiles() as usize;
        if n < 2 || self.pos.len() < n || self.swap_xy || self.pos[..n].iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
            return single;
        }
        let (Some(sx), Some(sy)) = (unit_scale(&l.x.unit), unit_scale(&l.y.unit)) else { return single };
        let pos = &self.pos[..n];
        let min_x = pos.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let min_y = pos.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        // Leica writes Length either as (n-1) or as n pixel steps; take the
        // one that puts the tiles on whole pixels.
        let cands = |d: &Dim, s: f64| [d.length.abs() * s / (d.n - 1).max(1) as f64, d.length.abs() * s / d.n as f64];
        let (cx, cy) = (cands(&l.x, sx), cands(&l.y, sy));
        let miss = |step: [f64; 2], k: usize, get: fn(&(f64, f64)) -> f64, min: f64| -> f64 {
            if step[k] <= 0.0 {
                return f64::INFINITY;
            }
            pos.iter().map(|p| ((get(p) - min) / step[k] - ((get(p) - min) / step[k]).round()).abs()).sum()
        };
        let score = |k: usize| miss(cx, k, |p| p.0, min_x) + miss(cy, k, |p| p.1, min_y);
        let k = if score(1) < score(0) { 1 } else { 0 };
        if cx[k] <= 0.0 || cy[k] <= 0.0 {
            return single;
        }
        let mut out = Vec::with_capacity(n);
        let (mut w, mut h) = (0u64, 0u64);
        for p in pos {
            let (ox, oy) = (((p.0 - min_x) / cx[k]).round(), ((p.1 - min_y) / cy[k]).round());
            if ox > 1e6 || oy > 1e6 {
                return single;
            }
            let (ox, oy) = (ox as u64, oy as u64);
            w = w.max(ox + l.w as u64);
            h = h.max(oy + l.h as u64);
            out.push((ox as u32, oy as u32));
        }
        if w.saturating_mul(h) > 1 << 32 || w > u32::MAX as u64 || h > u32::MAX as u64 {
            return single;
        }
        (out, w as u32, h as u32)
    }
}

// ------------------------------------------------------------------ hardware settings

/// A detector of the scanner's settings, with the settings block it lives in.
struct Det<'d, 'a> {
    node: Node<'d, 'a>,
    def: Node<'d, 'a>,
}

/// The `HardwareSetting` attachment of an image.
struct Hardware<'d, 'a> {
    /// The attachment itself (software, system type).
    setting: Node<'d, 'a>,
    /// The main settings block.
    main: Option<Node<'d, 'a>>,
    /// One block per sequential scan step, when there are several.
    seq: Vec<Node<'d, 'a>>,
}

const DEF_TAGS: [&str; 2] = ["ATLConfocalSettingDefinition", "ATLCameraSettingDefinition"];

impl<'d, 'a> Hardware<'d, 'a> {
    fn new(data: Node<'d, 'a>) -> Option<Hardware<'d, 'a>> {
        let setting = children(child(data, "Image"), "Attachment").find(|a| a.attribute("Name").as_deref() == Some("HardwareSetting"))?;
        let main = DEF_TAGS.iter().find_map(|t| child(setting, t));
        let seq = children(child(setting, "LDM_Block_Sequential/LDM_Block_Sequential_List"), "ATLConfocalSettingDefinition").collect();
        Some(Hardware { setting, main, seq })
    }

    /// The blocks that hold the channels' settings.
    fn defs(&self) -> Vec<Node<'d, 'a>> {
        if self.seq.len() > 1 {
            self.seq.clone()
        } else {
            self.main.into_iter().chain(self.seq.iter().copied()).take(1).collect()
        }
    }

    /// An attribute of the first settings block, else of the main one.
    fn get(&self, name: &str) -> Option<String> {
        self.defs().first().and_then(|d| val(*d, name)).or_else(|| self.main.and_then(|m| val(m, name)))
    }

    fn active_detectors(&self) -> Vec<Det<'d, 'a>> {
        let mut v = vec![];
        for def in self.defs() {
            for d in children(child(def, "DetectorList"), "Detector") {
                if d.attribute("IsActive").as_deref().map(str::trim) == Some("1") {
                    v.push(Det { node: d, def });
                }
            }
        }
        v
    }

    /// An attribute of a detector; sequential steps that leave it out inherit
    /// it from the same detector of the main settings block.
    fn det_attr(&self, d: &Det<'d, 'a>, name: &str) -> Option<String> {
        val(d.node, name).or_else(|| {
            let dname = val(d.node, "Name")?;
            let main = self.main?;
            children(child(main, "DetectorList"), "Detector")
                .find(|m| val(*m, "Name").as_deref() == Some(dname.as_str()))
                .and_then(|m| val(m, name))
        })
    }

    /// The detector that recorded channel `i`: by name when the channel says
    /// which one, else the i-th active one.
    fn detector_for(&self, i: usize, ch: &Chan) -> Option<Det<'d, 'a>> {
        let mut act = self.active_detectors();
        if let Some(name) = &ch.detector {
            if let Some(k) = act.iter().position(|d| val(d.node, "Name").as_deref() == Some(name.as_str())) {
                return Some(act.swap_remove(k));
            }
        }
        (i < act.len()).then(|| act.swap_remove(i))
    }
}

/// Colour for a LUT name.
fn lut_color(name: &str) -> Option<[u8; 3]> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "red" => [255, 0, 0],
        "green" => [0, 255, 0],
        "blue" => [0, 0, 255],
        "cyan" => [0, 255, 255],
        "magenta" => [255, 0, 255],
        "yellow" => [255, 255, 0],
        "gray" | "grey" | "grays" | "greys" | "white" => [255, 255, 255],
        "orange" => [255, 128, 0],
        "violet" | "purple" => [128, 0, 255],
        _ => return None,
    })
}

// ------------------------------------------------------------------ info groups

/// FILETIME ticks (hex) to "2026-10-15 10:12:03 UTC".
fn filetime(hex: &str) -> Option<String> {
    let t = u64::from_str_radix(hex.trim(), 16).ok()?;
    let secs = (t / 10_000_000) as i64 - 11_644_473_600;
    if !(0..=253_402_300_799).contains(&secs) {
        return None;
    }
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    Some(format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60))
}

fn um(v: f64) -> String {
    format!("{:.4} µm", v)
}

fn groups(
    c: &Container,
    l: &Layout,
    img: &Img,
    hw: Option<&Hardware>,
    tiles: &TileInfo,
    size: (u32, u32),
    px: (Option<f64>, Option<f64>, Option<f64>),
) -> Vec<Group> {
    let mut out = vec![];
    let image = child(img.data, "Image");

    // --- image
    let mut g = Group::new("LAS X 图像");
    g.add_str("文件类型", if c.kind == Kind::Lof { "Leica LOF" } else { "Leica LIF" });
    g.add_str("图像", &img.path);
    g.add("软件", hw.and_then(|h| val(h.setting, "Software")));
    g.add("系统", hw.and_then(|h| val(h.setting, "SystemTypeName")));
    g.add("数据来源", hw.and_then(|h| val(h.setting, "DataSourceTypeName")));
    let stamps = image.and_then(|i| child(i, "TimeStampList"));
    g.add("采集时间", stamps.and_then(|s| s.text()).and_then(|t| t.split_whitespace().next().and_then(filetime)));
    g.add_str("像素类型", &format!("{} 位{}", l.bits, if l.dtype == DType::F32 || l.dtype == DType::F64 { "浮点" } else { "" }));
    let mut dims = format!("X={}  Y={}", l.w, l.h);
    if l.is_rgb() {
        dims.push_str("  RGB");
    } else if l.size_c() > 1 {
        dims.push_str(&format!("  C={}", l.size_c()));
    }
    for d in l.z.iter().chain(l.t_axes.iter()).chain(l.tile.iter()) {
        let label = match d.id {
            3 => "Z",
            4 => "T",
            5 => "λ",
            6 => "A",
            7 => "N",
            8 => "Q",
            9 => "Λ",
            10 => "M",
            11 => "L",
            _ => "?",
        };
        dims.push_str(&format!("  {label}={}", d.n));
    }
    g.add_str("尺寸", &dims);
    if let Some(p) = px.0 {
        g.add_str("像素尺寸 X", &um(p));
        g.add_str("像素尺寸 Y", &um(px.1.unwrap_or(p)));
    }
    if let Some(p) = px.2 {
        g.add_str("Z 步长", &um(p));
    }
    if let Some(t) = l.t_axes.iter().find(|d| d.id == 4) {
        if t.n > 1 && t.length > 0.0 && t.unit.trim() == "s" {
            g.add_str("时间间隔", &format!("{} s", num(t.length / (t.n - 1) as f64)));
            g.add_str("总时长", &format!("{} s", num(t.length)));
        }
    }
    if l.tiles() > 1 {
        let stitched = size != (l.w, l.h);
        g.add_str(
            "拼块",
            &if stitched {
                format!("{} 个拼块，已按位置拼成 {}×{}", l.tiles(), size.0, size.1)
            } else if tiles.swap_xy {
                format!("{} 个拼块（拼块扫描交换了 XY 轴，只显示第 1 个）", l.tiles())
            } else {
                format!("{} 个拼块（位置信息不全，只显示第 1 个）", l.tiles())
            },
        );
    }
    g.push_to(&mut out);

    // --- microscope
    if let Some(h) = hw {
        let mut g = Group::new("LAS X 显微镜");
        g.add("显微镜", h.get("MicroscopeModel"));
        g.add("物镜", h.get("ObjectiveName").map(|s| tidy(&s)));
        g.add("放大倍数", h.get("Magnification").map(|v| numstr(&v)));
        g.add("数值孔径 NA", h.get("NumericalAperture").map(|v| numstr(&v)));
        g.add("浸没介质", h.get("Immersion"));
        g.add("折射率", h.get("RefractionIndex").map(|v| numstr(&v)));
        g.add("扫描模式", h.get("ScanMode"));
        let (a, b) = (h.get("InDimension"), h.get("OutDimension"));
        if let (Some(a), Some(b)) = (a, b) {
            g.add_str("扫描格式", &format!("{a}×{b}"));
        }
        g.add("变焦", h.get("Zoom").map(|v| numstr(&v)));
        g.add("扫描速度 (Hz)", h.get("ScanSpeed").map(|v| numstr(&v)));
        g.add("扫描方向", h.get("ScanDirectionXName"));
        g.add("针孔 (µm)", h.get("Pinhole").and_then(|v| v.parse::<f64>().ok()).map(|v| num(v * 1e6)));
        g.add("针孔 (AU)", h.get("PinholeAiry").map(|v| numstr(&v)));
        g.add("行平均", h.get("LineAverage"));
        g.add("帧平均", h.get("FrameAverage"));
        g.add("行累加", h.get("Line_Accumulation"));
        g.add("帧累加", h.get("FrameAccumulation"));
        g.add("像素驻留 (µs)", h.get("PixelDwellTime").and_then(|v| v.parse::<f64>().ok()).map(|v| num(v * 1e6)));
        g.add("帧时间 (s)", h.get("FrameTime").map(|v| numstr(&v)));
        g.add("位深", h.get("BitSize"));
        g.add("视场旋转 (°)", h.get("RotatorAngle").map(|v| numstr(&v)));
        if h.seq.len() > 1 {
            g.add_str("顺序扫描", &format!("{} 步", h.seq.len()));
        }
        g.push_to(&mut out);

        // --- channels
        if !l.is_rgb() {
            for (i, ch) in l.chans.iter().enumerate() {
                let det = h.detector_for(i, ch);
                let name = ch.dye.clone().or_else(|| det.as_ref().and_then(|d| val(d.node, "DyeName"))).or_else(|| ch.detector.clone());
                let mut g = Group::new(&match name {
                    Some(n) if !n.is_empty() => format!("通道 {}：{}", i + 1, n.trim_start_matches("Leica/").trim()),
                    _ => format!("通道 {}", i + 1),
                });
                g.add("颜色 (LUT)", Some(ch.lut.clone()));
                if let Some(d) = &det {
                    g.add("探测器", val(d.node, "Name").map(|n| match val(d.node, "Type") {
                        Some(t) => format!("{n} ({t})"),
                        None => n,
                    }));
                    g.add("采集模式", val(d.node, "AcquisitionModeName"));
                    g.add("增益", h.det_attr(d, "Gain").map(|v| numstr(&v)));
                    g.add("偏移", h.det_attr(d, "Offset").map(|v| numstr(&v)));
                    // Emission range: the spectral band of the detector's channel number.
                    let band = val(d.node, "Channel").and_then(|n| {
                        children(child(d.def, "Spectro"), "MultiBand").find(|m| val(*m, "Channel").as_deref() == Some(n.as_str()))
                    });
                    let range = match band {
                        Some(m) => fnum(m, "LeftWorld").zip(fnum(m, "RightWorld")),
                        None => fnum(d.node, "DetectionRangeBegin").zip(fnum(d.node, "DetectionRangeEnd")).filter(|r| r.1 > r.0),
                    };
                    g.add("检测范围 (nm)", range.filter(|r| r.1 > r.0).map(|r| format!("{} – {}", num(r.0), num(r.1))));
                    for aotf in children(child(d.def, "AotfList"), "Aotf") {
                        for line in children(Some(aotf), "LaserLineSetting") {
                            let inten = fnum(line, "IntensityDev").unwrap_or(0.0);
                            if inten <= 0.0 || val(line, "IsVisible").as_deref() == Some("0") {
                                continue;
                            }
                            let mut s = format!("{} nm, {}%", val(line, "LaserLine").unwrap_or_default(), num(inten));
                            if let Some(a) = fnum(line, "AOBSIntensityDev").filter(|a| *a >= 0.0) {
                                s.push_str(&format!("（AOBS {}%）", num(a)));
                            }
                            g.add_str("激光", &s);
                        }
                    }
                }
                g.add_str("位深", &format!("{}", ch.bits));
                g.push_to(&mut out);
            }
        }
    }

    // --- everything of this image, flattened
    let mut g = Group::new("LAS X 全部参数");
    let id = (!img.mem_id.is_empty()).then_some(img.mem_id.as_str());
    let range = find_data(&c.xml, id).unwrap_or((0, c.xml.len()));
    flatten(&c.xml[range.0..range.1], &mut g);
    g.push_to(&mut out);
    out
}

// ------------------------------------------------------------------ flat listing of the raw XML
//
// The SDK's XML tree does not list attributes, which is where Leica keeps
// nearly everything, so the "all settings" group is made with a small
// tokenizer of its own over the bytes of one image's `Data` element.

enum Ev<'a> {
    Open { name: &'a [u8], attrs: &'a [u8], empty: bool, start: usize },
    Close { end: usize },
    Text(&'a [u8]),
}

struct Tok<'a> {
    b: &'a [u8],
    i: usize,
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let mut i = from;
    while i + needle.len() <= b.len() {
        i += b[i..].iter().position(|&c| c == needle[0])?;
        if b[i..].starts_with(needle) {
            return Some(i);
        }
        i += 1;
    }
    None
}

impl<'a> Tok<'a> {
    fn next(&mut self) -> Option<Ev<'a>> {
        let b = self.b;
        loop {
            if self.i >= b.len() {
                return None;
            }
            if b[self.i] != b'<' {
                let s = self.i;
                let e = b[s..].iter().position(|&c| c == b'<').map_or(b.len(), |p| s + p);
                self.i = e;
                return Some(Ev::Text(&b[s..e]));
            }
            let rest = &b[self.i..];
            if rest.starts_with(b"<!--") {
                self.i = find(b, self.i + 4, b"-->").map_or(b.len(), |p| p + 3);
            } else if rest.starts_with(b"<![CDATA[") {
                let s = self.i + 9;
                let e = find(b, s, b"]]>").unwrap_or(b.len());
                self.i = (e + 3).min(b.len());
                return Some(Ev::Text(&b[s..e]));
            } else if rest.starts_with(b"<?") {
                self.i = find(b, self.i + 2, b"?>").map_or(b.len(), |p| p + 2);
            } else if rest.starts_with(b"<!") {
                self.i = b[self.i..].iter().position(|&c| c == b'>').map_or(b.len(), |p| self.i + p + 1);
            } else if rest.starts_with(b"</") {
                self.i = b[self.i..].iter().position(|&c| c == b'>').map_or(b.len(), |p| self.i + p + 1);
                return Some(Ev::Close { end: self.i });
            } else {
                let start = self.i;
                let mut j = start + 1;
                while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>') {
                    j += 1;
                }
                let mut name = &b[start + 1..j];
                if let Some(p) = name.iter().position(|&c| c == b':') {
                    name = &name[p + 1..];
                }
                let attrs_at = j;
                let mut q = 0u8;
                while j < b.len() {
                    let c = b[j];
                    if q != 0 {
                        if c == q {
                            q = 0;
                        }
                    } else if c == b'"' || c == b'\'' {
                        q = c;
                    } else if c == b'>' {
                        break;
                    }
                    j += 1;
                }
                let mut attrs_end = j.min(b.len());
                let empty = attrs_end > attrs_at && b[attrs_end - 1] == b'/';
                if empty {
                    attrs_end -= 1;
                }
                self.i = (j + 1).min(b.len());
                return Some(Ev::Open { name, attrs: &b[attrs_at..attrs_end], empty, start });
            }
        }
    }
}

/// Calls `f(key, value)` for every attribute in a start tag's attribute text.
fn for_attrs<'a>(a: &'a [u8], mut f: impl FnMut(&'a [u8], &'a [u8])) {
    let mut i = 0;
    while i < a.len() {
        while i < a.len() && a[i].is_ascii_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < a.len() && a[i] != b'=' && !a[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = &a[ks..i];
        while i < a.len() && (a[i].is_ascii_whitespace() || a[i] == b'=') {
            i += 1;
        }
        let Some(&q) = a.get(i).filter(|&&q| q == b'"' || q == b'\'') else {
            i += 1;
            continue;
        };
        let vs = i + 1;
        let ve = a[vs..].iter().position(|&c| c == q).map_or(a.len(), |e| vs + e);
        if !key.is_empty() {
            f(key, &a[vs..ve]);
        }
        i = ve + 1;
    }
}

fn unescape(v: &[u8]) -> Cow<'_, str> {
    let s = String::from_utf8_lossy(v);
    if !s.contains('&') {
        return s;
    }
    let mut out = String::with_capacity(s.len());
    let mut rest: &str = &s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let Some(e) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..e];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(ch) => {
                out.push(ch);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Byte range of the `Data` element of the image whose memory block is
/// `mem_id` (any image when `None`): the `Data` precedes its `Memory`
/// sibling inside the same `Element`.
fn find_data(xml: &[u8], mem_id: Option<&str>) -> Option<(usize, usize)> {
    struct Frame {
        /// 1 = Element, 2 = Data directly inside an Element.
        kind: u8,
        start: usize,
        data: Option<(usize, usize)>,
    }
    let mut stack: Vec<Frame> = vec![];
    let mut tok = Tok { b: xml, i: 0 };
    while let Some(ev) = tok.next() {
        match ev {
            Ev::Open { name, attrs, empty, start } => {
                let top = stack.last().map_or(0, |f| f.kind);
                if name == b"Memory" && top == 1 {
                    let mut id = None;
                    for_attrs(attrs, |k, v| {
                        if k == b"MemoryBlockID" {
                            id = Some(v);
                        }
                    });
                    if mem_id.is_none_or(|m| id == Some(m.as_bytes())) {
                        if let Some(d) = stack.last().and_then(|f| f.data) {
                            return Some(d);
                        }
                    }
                }
                let kind = if name == b"Element" {
                    1
                } else if name == b"Data" && top == 1 {
                    2
                } else {
                    0
                };
                if empty {
                    if kind == 2 {
                        if let Some(p) = stack.last_mut() {
                            p.data = Some((start, tok.i));
                        }
                    }
                } else {
                    stack.push(Frame { kind, start, data: None });
                }
            }
            Ev::Close { end } => {
                if let Some(f) = stack.pop() {
                    if f.kind == 2 {
                        if let Some(p) = stack.last_mut() {
                            p.data = Some((f.start, end));
                        }
                    }
                }
            }
            Ev::Text(_) => {}
        }
    }
    None
}

/// Adds every attribute and every leaf text below the first element of
/// `xml` to `g`, keyed by a path like `Image/Attachment[HardwareSetting]/ATLConfocalSettingDefinition@Zoom`.
fn flatten(xml: &[u8], g: &mut Group) {
    struct Frame<'a> {
        /// Length of the path before this element was appended.
        before: usize,
        has_child: bool,
        text: Option<&'a [u8]>,
        /// Seen children by tag, for numbering repeats.
        kids: Vec<(&'a [u8], u32)>,
        /// Pending `Key` of a Key/Value property pair.
        key: Option<String>,
        is_key: bool,
        is_value: bool,
    }
    // Lists that would only add noise (one long number per frame…).
    const SKIP: [&[u8]; 2] = [b"TimeStampList", b"ImageXMLDocument"];
    const MAX_REPEAT: u32 = 200;
    let clip = |s: Cow<str>| -> String {
        let s = s.trim();
        match s.char_indices().nth(300) {
            Some((i, _)) => format!("{}…", &s[..i]),
            None => s.to_string(),
        }
    };
    let mut tok = Tok { b: xml, i: 0 };
    let mut path = String::new();
    let mut stack: Vec<Frame> = vec![];
    let mut skip = 0u32;
    while let Some(ev) = tok.next() {
        match ev {
            Ev::Open { name, attrs, empty, .. } => {
                if skip > 0 {
                    if !empty {
                        skip += 1;
                    }
                    continue;
                }
                let n = stack.last_mut().map_or(1, |p| {
                    p.has_child = true;
                    match p.kids.iter_mut().find(|k| k.0 == name) {
                        Some(k) => {
                            k.1 += 1;
                            k.1
                        }
                        None => {
                            p.kids.push((name, 1));
                            1
                        }
                    }
                });
                if SKIP.contains(&name) || n > MAX_REPEAT || g.len() >= MAX_ITEMS {
                    if !empty {
                        skip = 1;
                    }
                    continue;
                }
                let tag = String::from_utf8_lossy(name);
                // Label: Name, else Id.
                let (mut label, mut label_key): (Option<Cow<str>>, &[u8]) = (None, b"");
                for_attrs(attrs, |k, v| {
                    if k == b"Name" && !v.is_empty() {
                        (label, label_key) = (Some(unescape(v)), b"Name");
                    } else if k == b"Id" && label.is_none() && !v.is_empty() {
                        (label, label_key) = (Some(unescape(v)), b"Id");
                    }
                });
                let before = path.len();
                // The root element (Data) is not part of the keys.
                if !stack.is_empty() {
                    if !path.is_empty() {
                        path.push('/');
                    }
                    path.push_str(&tag);
                    match &label {
                        Some(l) => {
                            path.push('[');
                            path.push_str(l);
                            path.push(']');
                        }
                        None if n > 1 && name != b"ChannelProperty" => path.push_str(&format!("#{n}")),
                        None => {}
                    }
                }
                for_attrs(attrs, |k, v| {
                    if k == label_key || v.is_empty() {
                        return;
                    }
                    let mut key = path.clone();
                    key.push('@');
                    key.push_str(&String::from_utf8_lossy(k));
                    g.add_str(&key, &clip(unescape(v)));
                });
                if empty {
                    path.truncate(before);
                } else {
                    stack.push(Frame {
                        before,
                        has_child: false,
                        text: None,
                        kids: vec![],
                        key: None,
                        is_key: name == b"Key",
                        is_value: name == b"Value",
                    });
                }
            }
            Ev::Close { .. } => {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let Some(f) = stack.pop() else { continue };
                if !f.has_child {
                    if let Some(t) = f.text {
                        let t = clip(unescape(t));
                        if f.is_key {
                            if let Some(p) = stack.last_mut() {
                                p.key = Some(t);
                            }
                        } else if f.is_value && stack.last().is_some_and(|p| p.key.is_some()) {
                            let key = stack.last_mut().and_then(|p| p.key.take()).unwrap_or_default();
                            g.add_str(&format!("{}[{key}]", &path[..f.before]), &t);
                        } else {
                            g.add_str(&path, &t);
                        }
                    }
                }
                path.truncate(f.before);
            }
            Ev::Text(t) => {
                if skip == 0 {
                    if let Some(f) = stack.last_mut() {
                        if f.text.is_none() && !t.iter().all(|c| c.is_ascii_whitespace()) {
                            f.text = Some(t);
                        }
                    }
                }
            }
        }
    }
}
