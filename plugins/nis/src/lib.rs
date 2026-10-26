//! Nikon NIS-Elements plugin: reads ND2 files (widefield, spinning disk,
//! A1/AX confocal, brightfield RGB) as a TifMrcView WebAssembly format plugin.
//!
//! An ND2 file (version 2.x and 3.x) is a chain of chunks. Each starts with
//! the magic 0x0ABECEDA, the length of its name, the length of its data and
//! the name ("ImageDataSeq|12!", "ImageAttributesLV!", "ImageMetadataLV!"…).
//! The last 40 bytes of the file point at a chunk map that lists every chunk
//! by name with its offset, so we never walk the chain.
//!
//! - `ImageDataSeq|i` is frame `i` of the acquisition: 8 bytes of time stamp,
//!   then the pixels, all channels interleaved per pixel ("components"),
//!   rows padded to `uiWidthBytes`; either raw or one zlib stream.
//! - `ImageAttributes` gives the frame geometry, sample size and compression.
//! - `ImageMetadata` describes the experiment loops (time, XY positions, Z,
//!   nested in that order or another); frame `i` is the `i`-th combination,
//!   the innermost loop running fastest.
//! - `ImageMetadataSeq|0` holds the calibration, objective and, per channel
//!   ("picture plane"), its name, colour, filters and camera settings.
//! - `ImageTextInfo` is NIS-Elements' own text summary of the acquisition.
//!
//! XY positions become series (one image per stage position, with its own
//! Z and T); channels are the components of every frame, so each page holds
//! all channels of one (Z, T) and the host splits them while decoding.
//!
//! Old JPEG 2000-based ND2 files (version 1, written before NIS-Elements 3)
//! are not supported.
//!
//! The layout of the format was learnt from the BSD-3-licensed Python
//! package `nd2` by Talley Lambert (https://github.com/tlambert03/nd2) and
//! the MIT/Apache-2.0 Rust crate `nd2-rs`; this is our own implementation.

mod variant;

use tifmrcview_plugin_sdk::{
    bail, export_format, file, le_u32, le_u64, Channel, Chunk, Codec, Context, DType, Format, Group, Info, Recipe,
    Result, Series,
};
use variant::{num, V};

const CHUNK_MAGIC: u32 = 0x0ABE_CEDA;
const JP2_MAGIC: [u8; 4] = [0, 0, 0, 0x0c];
const MAP_SIGNATURE: &[u8] = b"ND2 CHUNK MAP SIGNATURE 0000001!";
const FILE_SIGNATURE: &[u8] = b"ND2 FILE SIGNATURE CHUNK NAME01!";

const DEFAULT_COLORS: [[u8; 3]; 7] =
    [[0, 255, 0], [255, 0, 255], [0, 255, 255], [255, 0, 0], [0, 0, 255], [255, 255, 0], [255, 255, 255]];

// ---------------------------------------------------------------- chunk map

/// Where things are in the file.
struct Chunks {
    /// Chunk offset of `ImageDataSeq|i` by `i`; 0 where the frame is missing.
    frames: Vec<u64>,
    /// Every other chunk (a few dozen), by full name including the `!`.
    named: Vec<(String, u64)>,
}

/// Reads the chunk map at the end of the file in one go.
fn chunk_map() -> Result<Chunks> {
    let size = file::size();
    if size < 40 {
        bail!("文件太小，不是 ND2 文件");
    }
    let tail = file::read(size - 40, 40)?;
    if &tail[..32] != MAP_SIGNATURE {
        bail!("ND2 文件不完整：找不到块索引（文件可能没有正常保存完）");
    }
    let pos = le_u64(&tail, 32);
    let (start, len) = chunk_data(pos).map_err(|_| "ND2 块索引损坏")?;
    if len > 1 << 30 {
        bail!("ND2 块索引损坏");
    }
    let map = file::read(start, len as usize).map_err(|_| "ND2 块索引被截断")?;

    // Entries are "name!" followed by two u64: offset and size. Image frames
    // (and per-frame custom data) can number tens of thousands, so they are
    // recognised in place without building strings.
    let mut frames: Vec<u64> = vec![];
    let mut named = vec![];
    let mut cur = 0;
    while cur < map.len() {
        let Some(k) = map[cur..].iter().position(|&c| c == b'!') else { break };
        let end = cur + k + 1;
        let name = &map[cur..end];
        if name == MAP_SIGNATURE || end + 16 > map.len() {
            break;
        }
        let offset = le_u64(&map, end);
        if let Some(n) = name.strip_prefix(b"ImageDataSeq|") {
            let digits = &n[..n.len() - 1];
            if !digits.is_empty() && digits.len() <= 9 && digits.iter().all(u8::is_ascii_digit) {
                let i = digits.iter().fold(0usize, |a, &d| a * 10 + (d - b'0') as usize);
                if i < 1 << 24 {
                    if frames.len() <= i {
                        frames.resize(i + 1, 0);
                    }
                    frames[i] = offset;
                }
            }
        } else if !name.starts_with(b"CustomDataSeq|") {
            named.push((String::from_utf8_lossy(name).into_owned(), offset));
        }
        cur = end + 16;
    }
    Ok(Chunks { frames, named })
}

/// Data range (start, length) of the chunk at `pos`.
fn chunk_data(pos: u64) -> Result<(u64, u64)> {
    let h = file::read(pos, 16)?;
    if le_u32(&h, 0) != CHUNK_MAGIC {
        bail!("ND2 文件损坏：位置 {pos} 处不是数据块");
    }
    let start = pos + 16 + le_u32(&h, 4) as u64;
    let len = le_u64(&h, 8);
    if start.checked_add(len).is_none_or(|e| e > file::size()) {
        bail!("ND2 数据块被截断（位置 {pos}）");
    }
    Ok((start, len))
}

impl Chunks {
    /// Decoded metadata chunk: the first of `names` that the file has.
    fn decode(&self, names: &[&str]) -> Option<V> {
        let pos = names.iter().find_map(|n| self.named.iter().find(|(k, _)| k == n).map(|(_, p)| *p))?;
        let (start, len) = chunk_data(pos).ok()?;
        let data = file::read(start, usize::try_from(len).ok().filter(|&l| l < 1 << 28)?).ok()?;
        Some(variant::decode(&data))
    }
}

// ---------------------------------------------------------------- experiment loops

#[derive(Clone, Copy, PartialEq, Debug)]
enum Axis {
    T,
    P,
    Z,
}

const TIME_LOOP: u64 = 1;
const XY_LOOP: u64 = 2;
const Z_LOOP: u64 = 4;
const SPECT_LOOP: u64 = 6;
const CUSTOM_LOOP: u64 = 7;
const NE_TIME_LOOP: u64 = 8;
const MAN_TIME_LOOP: u64 = 9;

#[derive(Clone, Debug)]
struct Point {
    name: String,
    pos: [f64; 3],
}

#[derive(Clone, Debug, Default)]
struct Loop {
    kind: u64,
    count: usize,
    level: i32,
    /// Z step in µm (Z loops).
    step: f64,
    /// Z drive (Z loops).
    device: String,
    /// Period in ms (time loops).
    period: f64,
    /// Stage positions (XY loops).
    points: Vec<Point>,
}

impl Loop {
    fn axis(&self) -> Option<Axis> {
        match self.kind {
            XY_LOOP => Some(Axis::P),
            Z_LOOP => Some(Axis::Z),
            TIME_LOOP | NE_TIME_LOOP | CUSTOM_LOOP | MAN_TIME_LOOP => Some(Axis::T),
            _ => None,
        }
    }

    fn label(&self) -> String {
        let name = match self.kind {
            TIME_LOOP => "T",
            NE_TIME_LOOP => "T′",
            XY_LOOP => "XY",
            Z_LOOP => "Z",
            CUSTOM_LOOP => "自定义",
            _ => "T",
        };
        format!("{name}({})", self.count)
    }
}

fn get_f(v: &V, k: &str) -> f64 {
    v.get(k).and_then(V::f64).unwrap_or(0.0)
}

fn get_n(v: &V, k: &str) -> usize {
    v.get(k).and_then(V::u64).unwrap_or(0) as usize
}

/// One loop of the experiment tree, or `None` (also for empty loops).
fn single_loop(src: &V) -> Option<Loop> {
    let src = src.unwrap_single();
    let kind = src.get("eType").and_then(V::u64).unwrap_or(0);
    let mut pars = src.get("uLoopPars")?;
    if pars.items().is_empty() || kind > MAN_TIME_LOOP {
        return None;
    }
    // Older files wrap the parameters in a one-item list.
    if let [(k, inner @ V::Map(_))] = pars.items() {
        if k.is_empty() || k == "no_name" {
            pars = inner;
        }
    }
    let mut lp = Loop { kind, ..Loop::default() };
    match kind {
        TIME_LOOP | MAN_TIME_LOOP | CUSTOM_LOOP => {
            lp.count = get_n(pars, "uiCount");
            lp.period = get_f(pars, "dPeriod");
        }
        XY_LOOP => {
            let rel = pars.get("bRelativeXY").is_some_and(V::truthy);
            let (rx, ry) = if rel { (get_f(pars, "dReferenceX"), get_f(pars, "dReferenceY")) } else { (0.0, 0.0) };
            let use_z = pars.get("bUseZ").is_some_and(V::truthy);
            if let Some(points) = pars.get("Points") {
                for (_, p) in points.items() {
                    let name = ["dPosName", "pPosName"].iter().find_map(|k| p.get(k).and_then(V::str)).unwrap_or("");
                    let z = if use_z { get_f(p, "dPosZ") } else { 0.0 };
                    let pos = [rx + get_f(p, "dPosX"), ry + get_f(p, "dPosY"), z];
                    lp.points.push(Point { name: name.trim().to_string(), pos });
                }
            } else if let Some(xs) = pars.get("dPosX") {
                for (i, (_, x)) in xs.items().iter().enumerate() {
                    let at = |k: &str| pars.get(k).and_then(|v| v.items().get(i)).and_then(|(_, v)| v.f64());
                    let pos = [x.f64().unwrap_or(0.0), at("dPosY").unwrap_or(0.0), at("dPosZ").unwrap_or(0.0)];
                    lp.points.push(Point { name: String::new(), pos });
                }
            }
            let valid = src.get("pItemValid").map(V::flags).unwrap_or_default();
            if !valid.is_empty() {
                lp.points = lp.points.iter().zip(&valid).filter(|(_, &ok)| ok).map(|(p, _)| p.clone()).collect();
            }
            lp.count = lp.points.len();
        }
        Z_LOOP => {
            lp.count = get_n(pars, "uiCount");
            let (lo, hi) = (get_f(pars, "dZLow"), get_f(pars, "dZHigh"));
            lp.step = get_f(pars, "dZStep").abs();
            if lp.step == 0.0 && lp.count > 1 {
                lp.step = (hi - lo).abs() / (lp.count - 1) as f64;
            }
            lp.device = pars.get("wsZDevice").and_then(V::str).unwrap_or("").trim().to_string();
        }
        NE_TIME_LOOP => {
            let valid = pars.get("pPeriodValid").map(V::flags).unwrap_or_default();
            if let Some(periods) = pars.get("pPeriod") {
                for ((_, p), &ok) in periods.items().iter().zip(&valid) {
                    if ok {
                        lp.count += get_n(p, "uiCount");
                        if lp.period == 0.0 {
                            lp.period = get_f(p, "dPeriod");
                        }
                    }
                }
            }
        }
        SPECT_LOOP => {
            lp.count = pars.at("pPlanes/uiCount").and_then(V::u64).unwrap_or(get_n(pars, "uiCount") as u64) as usize;
        }
        _ => return None,
    }
    (lp.count > 0).then_some(lp)
}

/// Flattens the experiment tree into its loops, outermost first, the way
/// NIS-Elements does (nested loops of the same kind keep the larger one;
/// spectral loops are the channels and add no axis).
fn load_experiment(src: &V, level: i32, dest: &mut Vec<Loop>) {
    let src = src.unwrap_single();
    let Some(mut lp) = single_loop(src) else { return };
    let mut level = level;
    if lp.kind == SPECT_LOOP {
        level -= 1;
    } else if dest.last().is_none_or(|d| d.level < level) {
        lp.level = level;
        dest.push(lp);
    } else if let Some(prev) = dest.last_mut() {
        if prev.level == level && prev.kind == lp.kind && prev.count < lp.count {
            lp.level = level;
            *prev = lp;
        }
    }
    // Time loops with sub-loops keep their nested experiment one level down.
    let sub = src
        .at("uLoopPars/pSubLoops")
        .and_then(|s| s.items().first())
        .and_then(|(_, s)| s.get("ppNextLevelEx"))
        .and_then(|n| n.items().first())
        .and_then(|(_, n)| n.get("SLxExperiment"));
    if let Some(e) = sub {
        let mut more = vec![];
        load_experiment(e, 0, &mut more);
        dest.extend(more);
    }
    if let Some(next) = src.get("ppNextLevelEx") {
        for (_, item) in next.items() {
            load_experiment(item, level + 1, dest);
        }
    }
}

// ---------------------------------------------------------------- the file

pub struct Nd2 {
    info: Info,
    width: u32,
    height: u32,
    components: u32,
    /// Bytes per sample.
    sample_size: u32,
    /// Bytes per row of a frame.
    stride: u32,
    compressed: bool,
    frames: Vec<u64>,
    /// Frame (sequence index) of each page of this series; `u32::MAX` = none.
    pages: Vec<u32>,
}

impl Format for Nd2 {
    fn open(path: &str) -> Result<Nd2> {
        Nd2::open_series(path, 0)
    }

    fn open_series(_path: &str, series: u32) -> Result<Nd2> {
        Nd2::load(series as usize)
    }

    fn info(&mut self) -> Result<Info> {
        Ok(std::mem::replace(&mut self.info, Info::new(1, 1, DType::U8, "")))
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        let mut r = Recipe::new();
        let seq = *self.pages.get(page as usize).context("页码超出范围")?;
        let Some(&pos) = self.frames.get(seq as usize).filter(|&&p| p != 0) else {
            // Frames the acquisition never recorded (it was stopped early) stay black.
            return Ok(r);
        };
        let (start, len) = chunk_data(pos)?;
        if len < 8 {
            bail!("ND2 图像数据不完整（第 {} 帧）", seq + 1);
        }
        let (w, h, comp) = (self.width, self.height, self.components);
        let packed = w * comp * self.sample_size;
        let stride = if self.stride == packed { 0 } else { self.stride };
        // The 8 bytes before the pixels are the frame's time stamp.
        let mut c = Chunk::file(start + 8, len - 8, [0, 0, w, h]).samples(comp).stride(stride);
        if self.compressed {
            c = c.codec(Codec::Zlib);
        } else {
            let need = self.stride as u64 * (h as u64 - 1) + packed as u64;
            if len - 8 < need {
                bail!("ND2 图像数据不完整（第 {} 帧）", seq + 1);
            }
            c.length = need;
        }
        r.push(c);
        Ok(r)
    }
}

export_format!(Nd2);

impl Nd2 {
    fn load(series: usize) -> Result<Nd2> {
        let head = file::read_at_most(0, 112)?;
        if head.len() >= 4 && head[..4] == JP2_MAGIC {
            bail!("这是旧版 ND2 文件（基于 JPEG 2000，NIS-Elements 3 之前），暂不支持；请在 NIS-Elements 里另存为新版 ND2 或 OME-TIFF");
        }
        if head.len() < 4 || le_u32(&head, 0) != CHUNK_MAGIC {
            bail!("不是 ND2 文件");
        }
        let version = if head.len() >= 112 && &head[16..48] == FILE_SIGNATURE {
            String::from_utf8_lossy(&head[48..54]).trim_start_matches("Ver").to_string()
        } else {
            String::new()
        };
        let chunks = chunk_map()?;

        let attrs = chunks.decode(&["ImageAttributesLV!", "ImageAttributes!"]);
        let attrs = attrs.context("ND2 文件缺少图像属性（ImageAttributes）")?;
        let attrs = attrs.unwrap_single();
        let width = get_n(attrs, "uiWidth") as u32;
        let height = get_n(attrs, "uiHeight") as u32;
        let comp = get_n(attrs, "uiComp").max(1) as u32;
        let bits = get_n(attrs, "uiBpcInMemory");
        let width_bytes = get_n(attrs, "uiWidthBytes") as u32;
        let seq_count = get_n(attrs, "uiSequenceCount");
        let compression = attrs.get("eCompression").and_then(V::u64).unwrap_or(2);
        if width == 0 || height == 0 {
            bail!("ND2 图像尺寸无效（{width}×{height}）");
        }
        if comp > 64 {
            bail!("ND2 每像素分量太多（{comp}），暂不支持");
        }
        let dtype = match bits {
            8 => DType::U8,
            16 => DType::U16,
            32 => DType::F32,
            b => bail!("不支持的 ND2 位深（每分量 {b} 位）"),
        };
        let packed = width as u64 * comp as u64 * dtype.size() as u64;
        let stride = if (width_bytes as u64) < packed { packed } else { width_bytes as u64 };
        if stride > u32::MAX as u64 {
            bail!("ND2 图像太宽（{width}）");
        }
        let compressed = match compression {
            0 => true,
            1 => bail!("这个 ND2 用有损 JPEG 2000 压缩，暂不支持；请在 NIS-Elements 里另存为不压缩或无损压缩"),
            _ => false,
        };

        // Experiment loops: frame i is the i-th combination, innermost fastest.
        let mut loops = vec![];
        if let Some(exp) = chunks.decode(&["ImageMetadataLV!", "ImageMetadata!"]) {
            load_experiment(&exp, 0, &mut loops);
        }
        let product = loops.iter().fold(1usize, |a, l| a.saturating_mul(l.count));
        if loops.is_empty() || (product <= 1 && seq_count > 1) {
            let n = seq_count.max(chunks.frames.len()).max(1);
            loops = vec![Loop { kind: TIME_LOOP, count: n, ..Loop::default() }];
        }
        let size = |a: Axis| loops.iter().filter(|l| l.axis() == Some(a)).fold(1usize, |n, l| n.saturating_mul(l.count));
        let (size_t, size_p, size_z) = (size(Axis::T), size(Axis::P), size(Axis::Z));
        if size_t.saturating_mul(size_z).saturating_mul(size_p) > 1 << 26 {
            bail!("ND2 帧数太多（{} 个时间点 × {} 层 × {} 个位置）", size_t, size_z, size_p);
        }
        if series >= size_p {
            bail!("ND2 里只有 {size_p} 个位置（请求第 {} 个）", series + 1);
        }

        // Page z + Z·t of this position -> sequence index.
        let mut pages = Vec::with_capacity(size_t * size_z);
        for t in 0..size_t {
            for z in 0..size_z {
                let mut rem = [t, series, z];
                let (mut seq, mut stride_s) = (0usize, 1usize);
                for l in loops.iter().rev() {
                    let Some(a) = l.axis() else { continue };
                    let r = &mut rem[a as usize];
                    seq += (*r % l.count) * stride_s;
                    *r /= l.count;
                    stride_s *= l.count;
                }
                pages.push(u32::try_from(seq).unwrap_or(u32::MAX));
            }
        }

        let meta = chunks.decode(&["ImageMetadataSeqLV|0!", "ImageMetadataSeq|0!"]);
        let empty = V::Map(vec![]);
        let meta = meta.as_ref().map_or(&empty, |m| m.unwrap_single());
        let text = chunks.decode(&["ImageTextInfoLV!", "ImageTextInfo!"]);
        let text = text.as_ref().map_or(&empty, |t| t.unwrap_single());
        let app = chunks.decode(&["CustomDataVar|AppInfo_V1_0!"]);
        let app = app.as_ref().map_or(&empty, |a| a.unwrap_single());
        let calib = chunks.decode(&["ImageCalibrationLV|0!", "ImageCalibration|0!"]);
        let calib = calib.as_ref().map_or(&empty, |c| c.unwrap_single());

        let mut info = Info::new(width, height, dtype, "Nikon ND2");
        info.size_z = size_z as u32;
        info.size_t = size_t as u32;
        info.size_c = comp;
        info.samples_per_page = comp;
        info.page_count = pages.len() as u32;
        let planes = picture_planes(meta);
        info.is_rgb = dtype == DType::U8 && comp == 3 && planes.len() <= 1;
        if !info.is_rgb {
            info.channels = channels(&planes, comp as usize);
        }

        // Pixel size (µm) and Z step.
        let cal = meta.get("dCalibration").or(calib.get("dCalibration")).and_then(V::f64).unwrap_or(0.0);
        let calibrated = meta.get("bCalibrated").or(calib.get("bValid")).is_none_or(V::truthy);
        let zloop = loops.iter().find(|l| l.kind == Z_LOOP);
        let zstep = zloop.map_or(0.0, |l| l.step);
        if cal > 0.0 && cal.is_finite() && calibrated {
            info.pixel_size = Some([cal, cal, zstep]);
            info.unit = "µm".into();
        }

        let sw = app.get("m_SWNameString").and_then(V::str).unwrap_or("").trim();
        let ver = app.get("m_VersionString").and_then(V::str).unwrap_or("").trim();
        if !sw.is_empty() {
            let short = ver.split([' ', ',']).next().unwrap_or("");
            info.format = format!("Nikon ND2 ({sw} {short})").replace(" )", ")");
        }

        // One series per stage position.
        let xy = loops.iter().find(|l| l.kind == XY_LOOP);
        if size_p > 1 {
            info.series = (0..size_p)
                .map(|p| {
                    let name = xy
                        .and_then(|l| l.points.get(p))
                        .map(|pt| pt.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| format!("位置 {}", p + 1));
                    let mut detail = format!("{width}×{height}");
                    for (n, a) in [(comp as usize, "C"), (size_z, "Z"), (size_t, "T")] {
                        if n > 1 {
                            detail.push_str(&format!(" · {n}{a}"));
                        }
                    }
                    Series { name, detail }
                })
                .collect();
            info.series_index = series as u32;
        }

        let ctx = Ctx {
            version: &version,
            attrs,
            meta,
            text,
            app,
            calib,
            loops: &loops,
            planes: &planes,
            series,
            seq_count,
            compressed,
            info: &info,
        };
        let metadata = groups(&ctx);
        info.metadata = metadata;
        let sample_size = dtype.size() as u32;
        let stride = stride as u32;
        Ok(Nd2 { info, width, height, components: comp, sample_size, stride, compressed, frames: chunks.frames, pages })
    }
}

// ---------------------------------------------------------------- channels

/// The picture planes (channels) of the first frame's metadata, in order.
fn picture_planes(meta: &V) -> Vec<&V> {
    let pp = meta.get("sPicturePlanes");
    let planes = pp.and_then(|p| p.get("sPlaneNew").filter(|v| !v.items().is_empty()).or(p.get("sPlane")));
    planes.map(|p| p.items().iter().map(|(_, v)| v).collect()).unwrap_or_default()
}

/// NIS colours are 0xAABBGGRR.
fn abgr(c: u64) -> [u8; 3] {
    [c as u8, (c >> 8) as u8, (c >> 16) as u8]
}

fn channels(planes: &[&V], comp: usize) -> Vec<Channel> {
    let mut out = vec![];
    for p in planes {
        let name = p.get("sDescription").and_then(V::str).unwrap_or("").trim().to_string();
        let n = get_n(p, "uiCompCount").max(1);
        let color = p.get("uiColor").and_then(V::u64).map(abgr).filter(|c| c.iter().any(|&v| v > 0));
        for k in 0..n {
            let (name, color) = if n == 3 {
                let rgb = [[255, 0, 0], [0, 255, 0], [0, 0, 255]][k];
                (format!("{name} {}", ["R", "G", "B"][k]).trim().to_string(), Some(rgb))
            } else if n > 1 {
                (format!("{name} {}", k + 1).trim().to_string(), color)
            } else {
                (name.clone(), color)
            };
            out.push((name, color));
        }
    }
    if out.len() != comp {
        out.clear();
    }
    (0..comp)
        .map(|i| {
            let (name, color) = out.get(i).cloned().unwrap_or_default();
            let fallback = if comp == 1 { [255, 255, 255] } else { DEFAULT_COLORS[i % 7] };
            let name = if name.is_empty() { format!("C{}", i + 1) } else { name };
            Channel { name, color: color.unwrap_or(fallback) }
        })
        .collect()
}

/// Wavelength of the highest point of a spectrum (0 when there is none).
fn spectrum_peak(s: Option<&V>) -> f64 {
    let mut best: Option<(f64, f64)> = None;
    for (_, p) in s.and_then(|s| s.get("pPoint")).map_or(&[][..], V::items) {
        let (t, w) = (get_f(p, "dTValue"), get_f(p, "dWavelength"));
        if best.is_none_or(|(bt, _)| t > bt) {
            best = Some((t, w));
        }
    }
    best.map_or(0.0, |b| b.1)
}

/// (excitation, emission) in nm of a plane, from its fluorescent probe or
/// first filter, as NIS-Elements reports them.
fn wavelengths(plane: &V, k: usize) -> (f64, f64) {
    let probe = plane.get("pFluorescentProbe");
    let filter = plane.at("pFilterPath/m_pFilter").and_then(|f| f.items().first()).map(|(_, f)| f);
    let rgb = get_n(plane, "uiCompCount") == 3;
    let mut ex = spectrum_peak(probe.and_then(|p| p.get("m_ExcitationSpectrum")));
    if ex == 0.0 {
        if let Some(fs) = filter.and_then(|f| f.get("m_ExcitationSpectrum")) {
            let pts = fs.get("pPoint").map_or(&[][..], V::items);
            if get_n(fs, "uiCount") > 1 && pts.iter().all(|(_, p)| get_n(p, "eType") == 4) {
                ex = pts.get(k).map_or(0.0, |(_, p)| get_f(p, "dWavelength"));
            }
            if ex == 0.0 {
                ex = spectrum_peak(Some(fs));
            }
        }
    }
    let mut em = spectrum_peak(probe.and_then(|p| p.get("m_EmissionSpectrum")));
    if em == 0.0 {
        em = spectrum_peak(filter.and_then(|f| f.get("m_EmissionSpectrum")));
    }
    if rgb {
        (0.0, 0.0)
    } else {
        (ex, em)
    }
}

// ---------------------------------------------------------------- info panel

struct Ctx<'a> {
    version: &'a str,
    attrs: &'a V,
    meta: &'a V,
    text: &'a V,
    app: &'a V,
    calib: &'a V,
    loops: &'a [Loop],
    planes: &'a [&'a V],
    series: usize,
    seq_count: usize,
    compressed: bool,
    info: &'a Info,
}

/// NIS-Elements' text fields (`TextInfoItem_N`), with our labels.
const TEXT_ITEMS: [&str; 15] = [
    "图像 ID", "类型", "组", "样品 ID", "作者", "描述", "采集设备", "采样", "位置", "日期", "结论", "信息 1", "信息 2",
    "光学系统", "软件版本",
];

fn text_item(text: &V, i: usize) -> Option<String> {
    let k = format!("TextInfoItem_{i}");
    text.get(&k).and_then(V::str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn positive(v: Option<&V>) -> Option<f64> {
    v.and_then(V::f64).filter(|&x| x > 0.0 && x.is_finite())
}

/// Julian day number (with fraction) -> "2017-06-06 11:15:06".
fn julian(jd: f64) -> Option<String> {
    if !(2_400_000.0..2_600_000.0).contains(&jd) {
        return None;
    }
    let secs = ((jd - 2_440_587.5) * 86_400.0).round() as i64;
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    Some(format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", sod / 3600, sod / 60 % 60, sod % 60))
}

/// Bits of `uiModalityMask` and their names.
const MODALITIES: [(u64, &str); 24] = [
    (0x1, "荧光"),
    (0x2, "明场"),
    (0x10, "相差"),
    (0x20, "微分干涉 DIC"),
    (0x100, "相机"),
    (0x200, "激光扫描共聚焦"),
    (0x400, "转盘共聚焦"),
    (0x800, "扫场共聚焦（狭缝）"),
    (0x1000, "扫场共聚焦（针孔）"),
    (0x2000, "DSD 共聚焦"),
    (0x4000, "SIM"),
    (0x8000, "iSIM"),
    (0x40, "RCM"),
    (0x80, "VCS"),
    (0x4000_0000, "SoRa"),
    (0x40000, "Live-SR"),
    (0x10000, "多光子"),
    (0x20000, "TIRF"),
    (0x100000, "PMT"),
    (0x200000, "光谱"),
    (0x1000000, "透射探测器"),
    (0x2000000, "非扫描探测器 NDD"),
    (0x4000000, "虚拟滤镜"),
    (0x8000000, "GaAsP"),
];

/// The lines of NIS-Elements' description that belong to plane `k` (1-based):
/// from "Plane #k:" to the next line that is not indented.
fn plane_block(desc: &str, k: usize) -> Vec<&str> {
    let head = format!("Plane #{k}:");
    let mut lines = desc.lines();
    if !lines.by_ref().any(|l| l.trim() == head) {
        return vec![];
    }
    lines.take_while(|l| l.starts_with(' ') || l.starts_with('\t')).collect()
}

/// Adds "Key: value" lines of NIS-Elements' text (indented sub-lines under a
/// "Section:" line become items of that section).
fn add_text_lines(g: &mut Group, lines: &[&str], skip: &[&str]) {
    let mut section: Option<(usize, String)> = None;
    for line in lines {
        let indent = line.len() - line.trim_start().len();
        let mut l = line.trim();
        if l.is_empty() {
            continue;
        }
        // Under "ALC, MultiLaser(ALC):", lines such as "Line:1; ExW:405; Power: 0.0; Off"
        // belong to the section; "Binning: 2x2" under "Camera Settings:" is a setting of its own.
        if let Some((d, s)) = &section {
            if indent > *d {
                if l.contains(';') || !l.contains(':') {
                    g.add_str(s, l);
                    continue;
                }
            } else {
                section = None;
            }
        }
        // "Microscope Settings:   Filter (Emission): 4" carries its first setting on the same line.
        while let Some((k, v)) = l.split_once(':') {
            let v = v.trim();
            let first = v.split(':').next().unwrap_or("");
            if v.contains(':') && !first.contains(';') && (k.ends_with("Settings") || k.ends_with("Setting")) {
                l = v;
                continue;
            }
            let k = k.trim();
            if v.is_empty() {
                section = Some((indent, k.to_string()));
            } else if !skip.contains(&k) {
                g.add_str(k, v);
            }
            break;
        }
    }
}

/// "Line:4; ExW:640; Power: 77.0; On" -> "640 nm, 77.0%" for lines that are on.
fn laser(line: &str) -> Option<String> {
    let parts: Vec<&str> = line.split(';').map(str::trim).collect();
    if parts.last() != Some(&"On") {
        return None;
    }
    let field = |k: &str| parts.iter().find_map(|p| p.strip_prefix(k)).map(|v| v.trim().to_string());
    let ex = field("ExW:")?;
    Some(match field("Power:") {
        Some(p) => format!("{ex} nm, {p}%"),
        None => format!("{ex} nm"),
    })
}

fn groups(c: &Ctx) -> Vec<Group> {
    let mut out = vec![];
    let (meta, text, info) = (c.meta, c.text, c.info);

    let mut g = Group::new("NIS 图像");
    let sw = ["m_SWNameString", "m_VersionString"].map(|k| c.app.get(k).and_then(V::str).unwrap_or("").trim());
    g.add_str("软件", format!("{} {}", sw[0], sw[1]).trim());
    g.add_str("ND2 版本", c.version);
    let date = text_item(text, 9).or_else(|| meta.get("dTimeAbsolute").and_then(V::f64).and_then(julian));
    g.add("采集时间", date.map(|d| d.split_whitespace().collect::<Vec<_>>().join(" ")));
    let mut dims = format!("X={}  Y={}", info.width, info.height);
    let positions: usize = c.loops.iter().filter(|l| l.kind == XY_LOOP).map(|l| l.count).product();
    for (n, a) in [(info.size_z, "Z"), (info.size_c, "C"), (info.size_t, "T"), (positions as u32, "XY")] {
        if n > 1 {
            dims.push_str(&format!("  {a}={n}"));
        }
    }
    g.push("尺寸", &dims);
    g.add_str("实验循环", &c.loops.iter().map(Loop::label).collect::<Vec<_>>().join(" › "));
    g.push("帧数", &format!("{}", c.seq_count));
    let bits = get_n(c.attrs, "uiBpcSignificant");
    let mem = get_n(c.attrs, "uiBpcInMemory");
    g.push("像素类型", info.dtype.name());
    if bits > 0 && bits != mem {
        g.push("位深", &format!("{bits}（存储 {mem} 位）"));
    } else {
        g.push("位深", &format!("{mem}"));
    }
    g.push("压缩", if c.compressed { "无损（zlib）" } else { "无" });
    if let Some([x, _, z]) = info.pixel_size {
        g.push("像素尺寸", &format!("{} µm", num(x)));
        if z > 0.0 {
            g.push("Z 步长", &format!("{} µm", num(z)));
        }
    }
    if let Some(l) = c.loops.iter().find(|l| l.axis() == Some(Axis::T) && l.period > 0.0) {
        g.push("时间间隔", &format!("{} ms", num(l.period)));
    }
    let point = c.loops.iter().find(|l| l.kind == XY_LOOP).and_then(|l| l.points.get(c.series));
    if let Some(p) = point {
        if !p.name.is_empty() {
            g.push("位置名称", &p.name);
        }
        g.push("载物台 XY (µm)", &format!("{}, {}", num(p.pos[0]), num(p.pos[1])));
    } else if let (Some(x), Some(y)) = (meta.get("dXPos").and_then(V::f64), meta.get("dYPos").and_then(V::f64)) {
        g.push("载物台 XY (µm)", &format!("{}, {}", num(x), num(y)));
    }
    for i in [0, 1, 2, 3, 4, 8, 10, 11, 12] {
        g.add(TEXT_ITEMS[i], text_item(text, i));
    }
    g.push_to(&mut out);

    // Microscope: from the image metadata, the first channel's devices and the text.
    let sample0 = sample_setting(meta, 0, c.planes.first().copied());
    let mut g = Group::new("NIS 显微镜");
    if let Some(d) = sample0.and_then(|s| s.get("pDeviceSetting")) {
        let full = d.get("m_sMicroscopeFullName").and_then(V::str).unwrap_or("").trim();
        let phys = d.get("m_sMicroscopePhysFullName").and_then(V::str).unwrap_or("").trim();
        let name = if !phys.is_empty() && phys != full { format!("{phys}（{full}）") } else { full.to_string() };
        g.add_str("显微镜", name.trim());
    }
    let obj = sample0.and_then(|s| s.get("pObjectiveSetting"));
    let objective = meta
        .get("wsObjectiveName")
        .and_then(V::str)
        .or(obj.and_then(|o| o.get("wsObjectiveName")).and_then(V::str))
        .or(c.calib.get("sObjective").and_then(V::str))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| text_item(text, 13));
    g.add("物镜", objective.clone());
    let mag = positive(meta.get("dObjectiveMag")).or(positive(obj.and_then(|o| o.get("dObjectiveMag")))).or_else(|| {
        // "Plan Apo λ 20x" -> 20
        let o = objective.as_deref()?;
        o.split_whitespace().find_map(|w| w.strip_suffix(['x', 'X'])?.parse::<f64>().ok())
    });
    g.add("物镜放大倍数", mag.map(|m| format!("{}×", num(m))));
    let na = positive(meta.get("dObjectiveNA")).or(positive(obj.and_then(|o| o.get("dObjectiveNA"))));
    g.add("数值孔径 NA", na.map(num));
    let ri = positive(meta.get("dRefractIndex1")).or(positive(meta.get("dRefractIndex2")));
    g.add("折射率", ri.map(num));
    g.add("变焦", positive(meta.get("dZoom")).map(|z| format!("{}×", num(z))));
    g.add("投影放大倍数", positive(meta.get("dProjectiveMag")).map(|z| format!("{}×", num(z))));
    g.add("针孔直径 (µm)", positive(meta.get("dPinholeRadius")).map(|r| num(r * 2.0)));
    let camera = sample0
        .and_then(|s| s.at("pCameraSetting/CameraUserName"))
        .or(meta.at("sCameraSetting/sCameraName"))
        .and_then(V::str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    g.add("相机", camera.map(String::from));
    g.add_str("采集设备", c.app.get("m_GrabberString").and_then(V::str).unwrap_or("").trim());
    if let Some(z) = c.loops.iter().find(|l| l.kind == Z_LOOP) {
        g.add_str("Z 驱动", &z.device);
    }
    g.add("温度", positive(meta.get("dTemperK")).map(|k| format!("{:.1} °C", k - 273.15)));
    // The capture summary: "Camera Type: …", "Binning: 2x2", "Exposure: 100 ms"…
    if let Some(cap) = text_item(text, 6) {
        let lines: Vec<&str> = cap.lines().collect();
        add_text_lines(&mut g, &lines, &[]);
    }
    g.push_to(&mut out);

    // Channels.
    let desc = text_item(text, 5).unwrap_or_default();
    if !info.is_rgb {
        let mut comp = 0;
        for (i, plane) in c.planes.iter().enumerate() {
            let n = get_n(plane, "uiCompCount").max(1);
            let name = plane.get("sDescription").and_then(V::str).unwrap_or("").trim();
            let mut g = Group::new(format!("通道 {}：{}", i + 1, name).trim_end_matches('：'));
            if n > 1 {
                g.push("分量", &format!("{n}"));
            }
            if let Some(ch) = info.channels.get(comp) {
                g.push("显示颜色", &format!("#{:02X}{:02X}{:02X}", ch.color[0], ch.color[1], ch.color[2]));
            }
            comp += n;
            let mask = plane.get("uiModalityMask").and_then(V::u64).unwrap_or(0);
            let modes: Vec<&str> = MODALITIES.iter().filter(|(b, _)| mask & b != 0).map(|(_, s)| *s).collect();
            g.add_str("模式", &modes.join("、"));
            let sample = sample_setting(meta, i, Some(plane));
            let ch_meta = sample.and_then(|s| s.at("pCameraSetting/Metadata/Channels"));
            let ch_meta = ch_meta.and_then(|c| c.items().first()).map(|c| &c.1);
            let (mut ex, mut em) = wavelengths(plane, i);
            if ex == 0.0 {
                ex = ch_meta.map_or(0.0, |m| get_f(m, "ExWavelength"));
            }
            if em == 0.0 {
                em = ch_meta.map_or(0.0, |m| get_f(m, "EmWavelength"));
            }
            g.add("激发波长 (nm)", (ex > 0.0).then(|| num(ex)));
            g.add("发射波长 (nm)", (em > 0.0).then(|| num(em)));
            let probe = plane.at("pFluorescentProbe/m_sName").and_then(V::str).unwrap_or("").trim();
            g.add_str("荧光探针", probe);
            let filter = plane.at("pFilterPath/m_pFilter").and_then(|f| f.items().first());
            let filter = filter.and_then(|(_, f)| f.get("m_sName")).and_then(V::str).unwrap_or("").trim();
            if filter != name {
                g.add_str("滤镜", filter);
            }
            g.add("针孔直径 (µm)", positive(plane.get("dPinholeDiameter")).map(num));
            if let Some(s) = sample {
                g.add("曝光时间 (ms)", positive(s.get("dExposureTime")).map(num));
                let cam = s.at("pCameraSetting/CameraUserName").and_then(V::str).unwrap_or("").trim();
                if Some(cam) != camera {
                    g.add_str("相机", cam);
                }
                let bx = s.at("pCameraSetting/FormatQuality/fmtDesc/dBinningX").and_then(V::f64);
                let by = s.at("pCameraSetting/FormatQuality/fmtDesc/dBinningY").and_then(V::f64);
                if let (Some(x), Some(y)) = (bx, by) {
                    g.push("分档", &format!("{}×{}", num(x), num(y)));
                }
            }
            let block = plane_block(&desc, i + 1);
            for l in &block {
                if let Some(s) = laser(l) {
                    g.push("激光", &s);
                }
            }
            add_text_lines(&mut g, &block, &["Name", "Component Count"]);
            g.push_to(&mut out);
        }
    }

    // Everything, flattened, for search.
    let mut g = Group::new("NIS 全部参数");
    for (i, label) in TEXT_ITEMS.iter().enumerate() {
        if let Some(t) = text_item(text, i) {
            g.push(&format!("文本信息/{label}"), &t);
        }
    }
    for (prefix, v) in [
        ("AppInfo", c.app),
        ("ImageAttributes", c.attrs),
        ("ImageCalibration", c.calib),
        ("ImageMetadataSeq", meta),
    ] {
        let mut path = String::from(prefix);
        variant::leaves(v, &mut |k, val| {
            path.truncate(prefix.len());
            path.push('/');
            path.push_str(k);
            g.push(&path, val);
        });
    }
    g.push_to(&mut out);
    out
}

/// The camera/device settings (`sSampleSetting/aK`) used by plane `i`.
fn sample_setting<'a>(meta: &'a V, i: usize, plane: Option<&V>) -> Option<&'a V> {
    let ss = meta.at("sPicturePlanes/sSampleSetting")?;
    let k = plane.and_then(|p| p.get("uiSampleIndex")).and_then(V::u64).filter(|&k| k > 0).unwrap_or(i as u64);
    ss.get(&format!("a{k}")).or_else(|| ss.items().get(k as usize).map(|(_, v)| v))
}
