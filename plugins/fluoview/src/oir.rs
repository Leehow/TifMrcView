//! Olympus OIR (FluoView FV3000 / FV4000, cellSens FV): no public
//! specification; the layout follows Christoph Gohlke's reverse-engineered
//! oirfile (BSD-3-Clause).
//!
//! After a 96-byte header (`OLYMPUSRAWFORMAT`, the declared file size and
//! the position of a block index) the file is a chain of blocks, each a
//! u32 payload length and a u32 type:
//!
//! - 0 metadata: several XML documents (file info, image properties,
//!   per-channel settings, LUTs…), each preceded by a small binary header;
//! - 1 frame properties: one XML document per acquired frame;
//! - 3 UID + 4 pixels: a name such as `z003t002_0_1_<channel uuid>_7`
//!   followed by the bytes of strip 7 of that plane (rows of raw samples).
//!   `REF_LSM0_<uuid>_<n>` names strips of the reference image.
//!
//! We index the strips of every (channel, Z, T, λ) plane and hand them to
//! the host as file chunks. Large acquisitions may continue in companion
//! files (`<name>_00001`…); a plugin only sees the file it opened, so such
//! planes stay empty.

use tifmrcview_plugin_sdk::xml::{self, Node};
use tifmrcview_plugin_sdk::{
    bail, file, le_u32, le_u64, Channel, Context, DType, Group, Info, Recipe, Result, Series,
};

use crate::common::{detail, fnum, num, num_s, raw_rows, tidy_date, Piece, G, DEFAULT_COLORS};

pub const MAGIC: &[u8; 16] = b"OLYMPUSRAWFORMAT";

const BLOCK_METADATA: u32 = 0;
const BLOCK_FRAME: u32 = 1;
const BLOCK_UID: u32 = 3;
const BLOCK_PIXEL: u32 = 4;

/// One strip of pixels.
#[derive(Clone, Copy)]
struct Strip {
    t: u32,
    l: u32,
    z: u32,
    ch: u32,
    n: u32,
    pos: u64,
    len: u64,
}

pub struct Oir {
    info: Info,
    width: u32,
    height: u32,
    row_bytes: u64,
    /// The strips of each page, as pieces of the plane.
    pages: Vec<Vec<Piece>>,
}

/// The blocks of the file that matter.
struct Index {
    strips: Vec<Strip>,
    refs: Vec<Strip>,
    guids: Vec<String>,
    frame: Option<(u64, u64)>,
    meta: Vec<(u64, u64)>,
}

/// Block offsets from the index at the end of the file, else by walking the chain.
fn block_offsets(size: u64) -> Result<Vec<u64>> {
    let h = file::read(0, 96).map_err(|_| "OIR 文件头不完整")?;
    if &h[..16] != MAGIC {
        bail!("不是 OIR 文件");
    }
    let declared = le_u64(&h, 32).min(size);
    let index = le_u64(&h, 40);
    let mut offsets = vec![];
    if index >= 96 && index + 4 <= declared {
        let b = file::read_at_most(index, (declared - index).min(64 << 20) as usize)?;
        if le_u32(&b, 0) == 0xffff_ffff {
            for c in b[4..].chunks_exact(8) {
                let o = le_u64(c, 0);
                if o == 0 || o >= declared {
                    break;
                }
                offsets.push(o);
            }
        }
    }
    if offsets.is_empty() {
        // No usable index (e.g. an interrupted acquisition): walk the blocks.
        let mut pos = 96u64;
        while pos + 8 <= size && offsets.len() < 1 << 24 {
            let b = file::read(pos, 8)?;
            offsets.push(pos);
            pos += 8 + le_u32(&b, 0) as u64;
        }
    }
    Ok(offsets)
}

/// (t, l, z) from the part of a UID before the channel uuid ("z003t002_0_1").
fn uid_axes(s: &str) -> (u32, u32, u32) {
    let b = s.as_bytes();
    let (mut t, mut l, mut z) = (0, 0, 0);
    let mut i = 0;
    while i < b.len() {
        let a = b[i];
        let boundary = i == 0 || b[i - 1].is_ascii_digit() || b[i - 1] == b'_';
        let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if boundary && matches!(a, b't' | b'l' | b'z') && digits > 0 {
            let v: u32 = s[i + 1..i + 1 + digits].parse().unwrap_or(0);
            match a {
                b't' => t = v,
                b'l' => l = v,
                _ => z = v,
            }
            i += 1 + digits;
        } else {
            i += 1;
        }
    }
    (t, l, z)
}

fn is_uuid(s: &str) -> bool {
    s.len() >= 8 && s.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-')
}

fn index(size: u64) -> Result<Index> {
    let offsets = block_offsets(size)?;
    let mut ix = Index { strips: vec![], refs: vec![], guids: vec![], frame: None, meta: vec![] };
    let mut i = 0;
    while i < offsets.len() {
        let off = offsets[i];
        i += 1;
        let head = file::read_at_most(off, 160)?;
        if head.len() < 8 {
            continue;
        }
        let (len, typ) = (le_u32(&head, 0) as u64, le_u32(&head, 4));
        match typ {
            BLOCK_UID if head.len() >= 20 => {
                let ulen = le_u32(&head, 16) as usize;
                if ulen > 4096 {
                    continue;
                }
                let uid_bytes = if 20 + ulen <= head.len() { head[20..20 + ulen].to_vec() } else { file::read(off + 20, ulen)? };
                // The pixel block follows in the index.
                let Some(&next) = offsets.get(i) else { continue };
                let ph = if next >= off && next - off + 8 <= head.len() as u64 {
                    head[(next - off) as usize..(next - off) as usize + 8].to_vec()
                } else {
                    file::read(next, 8)?
                };
                if le_u32(&ph, 4) != BLOCK_PIXEL {
                    continue;
                }
                i += 1;
                let plen = le_u32(&ph, 0) as u64;
                let Ok(uid) = core::str::from_utf8(&uid_bytes) else { continue };
                let mut parts = uid.rsplitn(3, '_');
                let (Some(n), Some(guid)) = (parts.next(), parts.next()) else { continue };
                let rest = parts.next().unwrap_or("");
                let (Ok(n), true) = (n.parse::<u32>(), is_uuid(guid)) else { continue };
                let ch = match ix.guids.iter().position(|g| g == guid) {
                    Some(k) => k,
                    None => {
                        ix.guids.push(guid.to_string());
                        ix.guids.len() - 1
                    }
                } as u32;
                let pos = next + 8;
                let plen = plen.min(size.saturating_sub(pos));
                if rest.starts_with("REF_") {
                    ix.refs.push(Strip { t: 0, l: 0, z: 0, ch, n, pos, len: plen });
                } else {
                    let (t, l, z) = uid_axes(rest);
                    ix.strips.push(Strip { t, l, z, ch, n, pos, len: plen });
                }
            }
            BLOCK_FRAME if ix.frame.is_none() => ix.frame = Some((off + 8, len)),
            BLOCK_METADATA => ix.meta.push((off + 8, len)),
            _ => {}
        }
    }
    Ok(ix)
}

/// The XML documents of a metadata block, with the channel uuid that
/// precedes some of them (LUTs).
fn xml_docs(block: &[u8]) -> Vec<(Option<String>, &[u8])> {
    const MARK: &[u8] = b"<?xml";
    let mut starts = vec![];
    let mut i = 0;
    while let Some(k) = block[i..].iter().position(|&c| c == b'<') {
        let p = i + k;
        if block[p..].starts_with(MARK) {
            starts.push(p);
        }
        i = p + 1;
    }
    let mut out = vec![];
    for (k, &s) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(block.len());
        let doc = &block[s..end];
        let doc = &doc[..doc.iter().rposition(|&c| c == b'>').map_or(0, |e| e + 1)];
        // [u32 36][36-byte uuid][u32 xml length] just before the document.
        let guid = (s >= 44 && le_u32(block, s - 44) == 36)
            .then(|| String::from_utf8_lossy(&block[s - 40..s - 4]).into_owned())
            .filter(|g| is_uuid(g));
        out.push((guid, doc));
    }
    out
}

/// The root element name of a document (without its prefix), cheaply.
fn root_name(doc: &[u8]) -> &str {
    let mut i = 0;
    while let Some(k) = doc[i..].iter().position(|&c| c == b'<') {
        let p = i + k + 1;
        if p < doc.len() && doc[p] != b'?' && doc[p] != b'!' {
            let end = doc[p..].iter().position(|&c| matches!(c, b' ' | b'>' | b'/' | b'\r' | b'\n')).map_or(doc.len(), |e| p + e);
            let name = core::str::from_utf8(&doc[p..end]).unwrap_or("");
            return name.rsplit(':').next().unwrap_or(name);
        }
        i = p;
    }
    ""
}

// ---------------------------------------------------------------- XML helpers

fn child<'d, 'a>(n: Node<'d, 'a>, path: &str) -> Option<Node<'d, 'a>> {
    path.split('/').try_fold(n, |n, name| n.children().find(|c| c.has_tag_name(name)))
}

fn text(n: Node, path: &str) -> Option<String> {
    child(n, path).and_then(|c| c.text()).map(|t| t.trim().to_string()).filter(|s| !s.is_empty())
}

fn children<'d, 'a>(n: Option<Node<'d, 'a>>, name: &'static str) -> impl Iterator<Item = Node<'d, 'a>> {
    n.into_iter().flat_map(move |n| n.children().filter(move |c| c.has_tag_name(name)))
}

/// All elements called `name` below `n`, in document order.
fn descendants<'d, 'a>(n: Node<'d, 'a>, name: &str, out: &mut Vec<Node<'d, 'a>>) {
    for c in n.children() {
        if c.has_tag_name(name) {
            out.push(c);
        }
        descendants(c, name, out);
    }
}

/// What the metadata says, in the parts the reader needs.
#[derive(Default)]
struct Frame {
    width: u32,
    height: u32,
    depth: u32,
    bits: Option<String>,
}

fn parse_frame(xml: &[u8]) -> Frame {
    let doc = xml::Doc::parse(xml);
    let mut f = Frame::default();
    if let Some(def) = doc.root().and_then(|r| child(r, "imageDefinition")) {
        let u = |k| text(def, k).and_then(|v| v.parse().ok()).unwrap_or(0);
        f.width = u("width");
        f.height = u("height");
        f.depth = u("depth");
        f.bits = text(def, "bitCounts");
    }
    f
}

impl Oir {
    pub fn open(series: u32) -> Result<Oir> {
        let size = file::size();
        let ix = index(size)?;
        let (fpos, flen) = ix.frame.context("OIR 文件里没有图像")?;
        let frame = parse_frame(&file::read(fpos, flen.min(16 << 20) as usize).map_err(|_| "OIR 文件被截断")?);
        let dtype = match frame.depth {
            1 => DType::U8,
            2 => DType::U16,
            4 => DType::F32,
            d => bail!("不支持的 OIR 像素深度 {d} 字节"),
        };
        if frame.width == 0 || frame.height == 0 {
            bail!("OIR 图像尺寸无效");
        }

        // Metadata: the first block holds every document we use.
        let meta = match ix.meta.first() {
            Some(&(p, l)) => file::read(p, l.min(256 << 20) as usize).unwrap_or_default(),
            None => vec![],
        };
        let docs = xml_docs(&meta);
        let props = docs.iter().find(|d| root_name(d.1) == "imageProperties").map(|d| xml::Doc::parse(d.1));
        let props_root = props.as_ref().and_then(|d| d.root());

        // Channel order: as the metadata lists them, then any others.
        let mut order: Vec<(i64, usize, u32)> = vec![];
        let mut chan_nodes = vec![];
        if let Some(r) = props_root {
            descendants(r, "channel", &mut chan_nodes);
        }
        let used: Vec<u32> = {
            let mut v: Vec<u32> = if series == 0 { ix.strips.iter() } else { ix.refs.iter() }.map(|s| s.ch).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        for &ch in &used {
            let g = &ix.guids[ch as usize];
            let n = chan_nodes.iter().position(|n| n.attribute("id").as_deref() == Some(g.as_str()));
            let o = n.and_then(|k| chan_nodes[k].attribute("order")).and_then(|o| o.parse().ok()).unwrap_or(i64::MAX);
            order.push((o, n.unwrap_or(usize::MAX), ch));
        }
        order.sort();
        let chans: Vec<u32> = order.iter().map(|o| o.2).collect();

        let mut info_series = vec![];
        if series > 1 || (series == 1 && ix.refs.is_empty()) {
            bail!("这个文件没有第 {} 个图像", series + 1);
        }
        let ref_size = if series == 1 || !ix.refs.is_empty() {
            docs.iter()
                .find(|d| root_name(d.1) == "imageDefinition")
                .map(|d| parse_frame_def(d.1))
                .filter(|s| s.0 > 0 && s.1 > 0)
                .unwrap_or((frame.width, frame.height))
        } else {
            (frame.width, frame.height)
        };
        let width = if series == 1 { ref_size.0 } else { frame.width };
        let row_bytes = width as u64 * frame.depth as u64;

        if !ix.refs.is_empty() {
            let (c, z, t) = dims(&ix.strips);
            let (rc, _, _) = dims(&ix.refs);
            info_series = vec![
                Series { name: "图像".into(), detail: detail(frame.width, frame.height, c, z, t) },
                Series { name: "参考图像".into(), detail: detail(ref_size.0, ref_size.1, rc, 1, 1) },
            ];
        }
        let mut strips = if series == 0 { ix.strips } else { ix.refs };
        if strips.is_empty() {
            bail!("OIR 文件里没有图像数据");
        }
        strips.sort_by_key(|s| (s.t, s.l, s.z, s.ch, s.n));
        let ranks = |f: fn(&Strip) -> u32| {
            let mut v: Vec<u32> = strips.iter().map(f).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (ts, ls, zs) = (ranks(|s| s.t), ranks(|s| s.l), ranks(|s| s.z));
        let nc = chans.len() as u32;
        let size_c = nc * ls.len() as u32;
        let (size_z, size_t) = (zs.len() as u32, ts.len() as u32);
        let mut pages: Vec<Vec<Piece>> = vec![vec![]; (size_c * size_z * size_t) as usize];
        let pos = |v: &[u32], x: u32| v.binary_search(&x).unwrap() as u32;
        for s in &strips {
            let c = chans.iter().position(|&c| c == s.ch).unwrap() as u32 + nc * pos(&ls, s.l);
            let page = c + size_c * (pos(&zs, s.z) + size_z * pos(&ts, s.t));
            let p = &mut pages[page as usize];
            let at = p.last().map_or(0, |q: &Piece| q.at + q.len);
            p.push(Piece { at, file: s.pos, len: s.len });
        }
        // Line scans store more rows than one frame holds.
        let longest = pages.iter().map(|p| p.last().map_or(0, |q| q.at + q.len)).max().unwrap_or(0);
        let height = if series == 1 { ref_size.1 } else { frame.height }.max((longest / row_bytes.max(1)) as u32);

        let mut info = Info::new(width, height, dtype, "Olympus FluoView OIR");
        info.size_c = size_c;
        info.size_z = size_z;
        info.size_t = size_t;
        info.page_count = pages.len() as u32;

        // Lambda wavelengths, when the λ axis is on.
        let lambda_axis = props_root.and_then(|r| axis(r, "LAMBDA"));
        let names: Vec<String> = chans
            .iter()
            .map(|&ch| {
                let g = &ix.guids[ch as usize];
                chan_nodes
                    .iter()
                    .find(|n| n.attribute("id").as_deref() == Some(g.as_str()))
                    .and_then(|n| text(*n, "name"))
                    .unwrap_or_else(|| format!("C{}", ch + 1))
            })
            .collect();
        let luts: Vec<(String, [u8; 3])> = docs
            .iter()
            .filter(|d| root_name(d.1) == "LUT")
            .filter_map(|(g, d)| Some((g.clone()?, lut_color(d)?)))
            .collect();
        for &l in &ls {
            for (i, &ch) in chans.iter().enumerate() {
                let mut name = names[i].clone();
                if ls.len() > 1 {
                    match lambda_axis {
                        Some((start, step)) if step > 0.0 => name = format!("{name} {} nm", num(start + step * (l.max(1) - 1) as f64)),
                        _ => name = format!("{name} λ{l}"),
                    }
                }
                let g = &ix.guids[ch as usize];
                let k = info.channels.len();
                let fallback = if size_c == 1 { [255, 255, 255] } else { DEFAULT_COLORS[k % 7] };
                let color = luts.iter().find(|x| &x.0 == g).map(|x| x.1).unwrap_or(fallback);
                info.channels.push(Channel { name, color });
            }
        }

        if let Some(r) = props_root {
            metadata(r, &docs, &ix.guids, &chans, &frame, series, &mut info);
        }
        info.series = info_series;
        info.series_index = series;
        Ok(Oir { info, width, height, row_bytes, pages })
    }

    pub fn info(&mut self) -> Info {
        std::mem::replace(&mut self.info, Info::new(1, 1, DType::U8, ""))
    }

    pub fn page(&self, page: u32) -> Result<Recipe> {
        let pieces = self.pages.get(page as usize).context("页码超出范围")?;
        let mut r = Recipe::new();
        raw_rows(pieces, self.row_bytes, self.height, self.width, 0, 0, &mut r)?;
        Ok(r)
    }
}

/// (channels × λ, Z, T) of a set of strips.
fn dims(strips: &[Strip]) -> (u32, u32, u32) {
    let count = |f: fn(&Strip) -> u32| {
        let mut v: Vec<u32> = strips.iter().map(f).collect();
        v.sort_unstable();
        v.dedup();
        v.len() as u32
    };
    (count(|s| s.ch) * count(|s| s.l), count(|s| s.z), count(|s| s.t))
}

/// (width, height) of a `base:imageDefinition` document.
fn parse_frame_def(xml: &[u8]) -> (u32, u32) {
    let doc = xml::Doc::parse(xml);
    let Some(r) = doc.root() else { return (0, 0) };
    let u = |k| text(r, k).and_then(|v| v.parse().ok()).unwrap_or(0);
    (u("width"), u("height"))
}

/// Channel colour of a LUT document: the primaries with a non-zero contrast.
/// Only the head of the document is parsed (the table itself is large).
fn lut_color(doc: &[u8]) -> Option<[u8; 3]> {
    let head = &doc[..doc.len().min(4096)];
    let d = xml::Doc::parse(head);
    let r = d.root()?;
    let on = |c: &str| text(r, &format!("{c}/contrast")).and_then(|v| fnum(&v)).is_some_and(|v| v > 0.0);
    let c = [on("red"), on("green"), on("blue")].map(|b| if b { 255 } else { 0 });
    (c != [0, 0, 0]).then_some(c)
}

/// (start, step) of an enabled acquisition axis (ZSTACK, TIMELAPSE, LAMBDA).
fn axis(root: Node, name: &str) -> Option<(f64, f64)> {
    let ip = child(root, "acquisition/imagingParam")?;
    let found = children(Some(ip), "axis")
        .filter(|a| a.attribute("enable").as_deref() == Some("true"))
        .find(|a| text(*a, "axis").as_deref() == Some(name))?;
    let f = |k| text(found, k).and_then(|v| fnum(&v)).unwrap_or(0.0);
    Some((f("startPosition"), f("step")))
}

/// Pixel size in µm from a channel's `length` and `pixelUnit`.
fn length_um(ch: Node, k: &str) -> Option<f64> {
    let v = text(ch, &format!("length/{k}")).and_then(|v| fnum(&v))?;
    let scale = match text(ch, &format!("pixelUnit/{k}")).as_deref() {
        Some("NANO_METER") => 1e-3,
        Some("MILLI_METER") => 1e3,
        _ => 1.0,
    };
    (v > 0.0).then_some(v * scale)
}

/// Pixel size, format name and the info-panel groups.
fn metadata(r: Node, docs: &[(Option<String>, &[u8])], guids: &[String], chans: &[u32], frame: &Frame, series: u32, info: &mut Info) {
    let mut all_ch = vec![];
    descendants(r, "channel", &mut all_ch);
    let image_info = child(r, "imageInfo");
    let mut info_ch = vec![];
    if let Some(ii) = image_info {
        descendants(ii, "channel", &mut info_ch);
    }
    fn by_id<'d, 'a>(list: &[Node<'d, 'a>], g: &str) -> Option<Node<'d, 'a>> {
        list.iter().copied().find(|n| n.attribute("id").as_deref() == Some(g))
    }

    let first = chans.first().and_then(|&c| by_id(&info_ch, &guids[c as usize])).or(info_ch.first().copied());
    if let Some(c) = first {
        if let Some(x) = length_um(c, "x") {
            let y = length_um(c, "y").unwrap_or(x);
            let z = if info.size_z > 1 { length_um(c, "z").unwrap_or(0.0) } else { 0.0 };
            info.pixel_size = Some([x, y, z]);
            info.unit = "µm".into();
        }
    }

    let system = text(r, "system/systemName");
    let version = text(r, "system/systemVersion");
    if let Some(s) = &system {
        info.format = format!("Olympus FluoView OIR ({s}{})", version.as_deref().map(|v| format!(" {v}")).unwrap_or_default());
    }
    let acq = child(r, "acquisition");
    let ip = acq.and_then(|a| child(a, "imagingParam"));

    let mut out: Vec<Group> = vec![];
    let mut g = G::new("FluoView 图像");
    g.add("系统", system.map(|s| format!("{s} {}", version.clone().unwrap_or_default())));
    g.add("显微镜", text(r, "microscope/name"));
    g.add("采集时间", text(r, "general/creationDateTime").map(|d| tidy_date(&d)));
    if series == 1 {
        g.add("内容", Some("参考图像".into()));
    }
    g.add("尺寸", Some(detail(info.width, info.height, info.size_c, info.size_z, info.size_t)));
    g.add("位深", frame.bits.clone());
    if let Some([x, y, z]) = info.pixel_size {
        g.add("像素尺寸 X", Some(format!("{:.4} µm", x)));
        g.add("像素尺寸 Y", Some(format!("{:.4} µm", y)));
        if z > 0.0 {
            g.add("Z 步长", Some(format!("{:.4} µm", z)));
        }
    }
    if info.size_t > 1 {
        g.add("时间间隔 (s)", axis(r, "TIMELAPSE").map(|a| a.1).filter(|s| *s > 0.0).map(num));
    }
    if let Some((start, step)) = axis(r, "LAMBDA") {
        g.add("λ 扫描 (nm)", Some(format!("{} + {} × n", num(start), num(step))));
    }
    g.add("样品", text(r, "general/sampleName"));
    g.push_to(&mut out);

    let mut g = G::new("FluoView 显微镜");
    if let Some(o) = acq.and_then(|a| child(a, "microscopeConfiguration/objectiveLens")) {
        g.add("物镜", text(o, "displayName").or_else(|| text(o, "name")));
        g.add("放大倍数", text(o, "magnification").map(|v| num_s(&v)));
        g.add("数值孔径 NA", text(o, "naValue").map(|v| num_s(&v)));
        g.add("工作距离 (mm)", text(o, "wdValue").map(|v| num_s(&v)));
        g.add("浸没介质", text(o, "immersion"));
    }
    if let Some(ip) = ip {
        g.add("扫描缩放", text(ip, "area/zoom").map(|v| num_s(&v)));
        g.add("旋转 (°)", text(ip, "area/rotation").map(|v| num_s(&v)).filter(|v| v != "0"));
    }
    let scanner = acq.and_then(|a| text(a, "configuration/scannerType"));
    g.add("扫描器", scanner.clone());
    let settings = children(acq, "scannerSettings").find(|s| s.attribute("type").as_deref() == scanner.as_deref());
    if let Some(sp) = settings.and_then(|s| child(s, "param/speed")) {
        g.add("像素驻留 (µs)", text(sp, "speed").map(|v| num_s(&v)));
        g.add("行时间 (ms)", text(sp, "speedInformation/lineSpeed").map(|v| num_s(&v)));
        g.add("帧时间 (s)", text(sp, "speedInformation/frameSpeed").and_then(|v| fnum(&v)).map(|v| num(v / 1000.0)));
        g.add("往返扫描", text(sp, "roundtrip"));
    }
    if let Some(m) = ip.and_then(|ip| child(ip, "method")) {
        g.add("顺序扫描", text(m, "sequentialType"));
        let ty = text(m, "integration/type").filter(|t| t != "None");
        g.add(
            "平均（Kalman / 行）",
            ty.map(|t| match text(m, "integration/count") {
                Some(n) => format!("{t} ×{n}"),
                None => t,
            }),
        );
    }
    g.push_to(&mut out);

    // Per-channel settings: the channel in imageInfo (name, emission range,
    // pinhole, lasers), its lsmChannel document (dye, detector) and the
    // imaging parameters (PMT, laser transmissivity).
    let lsm_docs: Vec<xml::Doc> = docs.iter().filter(|d| root_name(d.1) == "lsmChannel").map(|d| xml::Doc::parse(d.1)).collect();
    // Laser definitions sit in each phase, under imageInfo or acquisition.
    let mut lasers = vec![];
    descendants(r, "imagingMainLaser", &mut lasers);
    // The lasers of the system: (id, name).
    let system_lasers: Vec<(String, String)> = children(child(r, "lsm"), "laser")
        .filter_map(|l| Some((text(l, "id")?, text(l, "name")?)))
        .collect();
    let main_lasers: Vec<Node> = children(ip, "mainLaser").collect();
    let pmts: Vec<Node> = children(ip, "pmt").collect();
    for (k, &c) in chans.iter().enumerate() {
        let gid = guids[c as usize].as_str();
        let ch = by_id(&info_ch, gid).or_else(|| by_id(&all_ch, gid));
        let lsm = lsm_docs.iter().filter_map(|d| d.root()).find(|n| n.attribute("id").as_deref() == Some(gid));
        let name = ch.and_then(|n| text(n, "name")).unwrap_or_else(|| format!("C{}", k + 1));
        let dye = lsm.and_then(|n| text(n, "dyeName")).filter(|d| d != "None");
        let title = match &dye {
            Some(d) => format!("通道 {}：{} ({})", k + 1, name, d),
            None => format!("通道 {}：{}", k + 1, name),
        };
        let mut g = G::new(&title);
        g.add("荧光染料", dye);
        if let Some(d) = lsm.and_then(|n| child(n, "dyeData")) {
            g.add("激发峰 (nm)", text(d, "excitationWavelength"));
            g.add("发射峰 (nm)", text(d, "emissionWavelength"));
        }
        if let Some(n) = ch {
            let range = text(n, "productData/wavelengthRange/startWavelength")
                .zip(text(n, "productData/wavelengthRange/endWavelength"))
                .map(|(a, b)| format!("{}–{}", num_s(&a), num_s(&b)));
            g.add("检测范围 (nm)", range);
            g.add("针孔 (µm)", text(n, "productData/pinholeDiameter").map(|v| num_s(&v)));
        }
        // Lasers of the channel, wherever the file lists them.
        let mut id_nodes = vec![];
        for n in all_ch.iter().filter(|n| n.attribute("id").as_deref() == Some(gid)).chain(lsm.iter()) {
            descendants(*n, "laserDataId", &mut id_nodes);
        }
        let mut ids: Vec<String> = id_nodes.iter().filter_map(|i| i.text()).map(|t| t.trim().to_string()).collect();
        ids.dedup();
        let mut seen: Vec<&str> = vec![];
        for id in &ids {
            if seen.contains(&id.as_str()) {
                continue;
            }
            seen.push(id);
            let def = lasers.iter().find(|l| l.attribute("id").as_deref() == Some(id));
            if def.is_some_and(|d| d.attribute("enable").as_deref() == Some("false")) {
                continue;
            }
            // A tunable (multiphoton) laser has no fixed wavelength: show its name.
            let wl = def.and_then(|d| text(*d, "wavelength")).map(|v| format!("{} nm", num_s(&v))).or_else(|| {
                system_lasers.iter().find(|(lid, _)| !lid.is_empty() && id.starts_with(lid.as_str())).map(|l| l.1.clone())
            });
            let tr = main_lasers
                .iter()
                .find(|l| l.attribute("laserDataId").as_deref() == Some(id))
                .and_then(|l| text(*l, "transmissivity"))
                .map(|v| format!("{}%", num_s(&v)));
            g.add(
                "激光",
                match (wl, tr) {
                    (Some(w), Some(t)) => Some(format!("{w}, {t}")),
                    (w, t) => w.or(t),
                },
            );
        }
        if let Some(n) = lsm {
            g.add("探测器", text(n, "deviceName"));
        }
        if let Some(p) = pmts.iter().find(|p| p.attribute("channelId").as_deref() == Some(gid)) {
            g.add("探测器电压 HV", text(*p, "voltage"));
            g.add("增益", text(*p, "gain").map(|v| num_s(&v)));
            g.add("偏移", text(*p, "offset"));
        }
        g.push_to(&mut out);
    }

    // Everything in the image properties, flattened, for search.
    let mut all = G::new("FluoView 全部参数");
    r.doc().leaves(r, |path, text| all.0.add_str(path, text));
    all.push_to(&mut out);
    info.metadata = out;
}
