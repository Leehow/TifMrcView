//! Imaris plugin: reads Imaris .ims files (Oxford Instruments / Bitplane
//! Imaris 5.5+ format, also written by Andor Dragonfly, Fusion and
//! ImarisFileConverter) as a TifMrcView WebAssembly format plugin.
//!
//! An IMS file is HDF5. The pixels are in
//! `/DataSet/ResolutionLevel 0/TimePoint t/Channel c/Data`: one 3D dataset
//! (Z × Y × X) per channel and time point, chunked (e.g. 16×256×256) and
//! usually gzip-compressed, with its dimensions padded up to whole chunks;
//! the real size is in the `ImageSizeX/Y/Z` attributes of the channel
//! group. Lower resolution levels are skipped. Metadata lives in
//! attributes of the groups under `/DataSetInfo` (Image, Channel N,
//! TimeInfo, ImarisDataSet, vendor groups), stored as arrays of
//! one-character strings.
//!
//! HDF5 is read by our own minimal reader (`hdf5.rs`), written from the
//! public HDF5 File Format Specification. The plugin only indexes the file:
//! each page (one Z plane of one channel and time point) is a recipe with
//! one entry per HDF5 chunk crossing that plane; the host inflates the
//! chunk and keeps the plane's slice (`skip`), natively and in parallel.
//!
//! References used for the layout of IMS files (no code taken): the Imaris
//! file format description published by Bitplane, and the h5py
//! documentation (BSD), used as a test oracle.

mod hdf5;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use hdf5::{ChunkRef, Dataset, Layout, Object, H5};
use tifmrcview_plugin_sdk::{
    bail, export_format, file, flags, Channel, Chunk, Codec, DType, Error, Format, Group, Info, Recipe, Result,
};

const DEFAULT_COLORS: [[u8; 3]; 7] =
    [[0, 255, 0], [255, 0, 255], [0, 255, 255], [255, 0, 0], [0, 0, 255], [255, 255, 0], [255, 255, 255]];

/// The file, read through the host.
struct HostFile;

impl hdf5::Source for HostFile {
    fn size(&self) -> u64 {
        file::size()
    }
    fn read(&self, off: u64, len: usize) -> Result<Vec<u8>> {
        file::read_at_most(off, len)
    }
}

/// Attributes of one group, in file order.
type Attrs = Vec<(String, Option<String>)>;

fn get<'a>(a: &'a Attrs, name: &str) -> Option<&'a str> {
    a.iter().find(|(k, _)| k == name).and_then(|(_, v)| v.as_deref()).map(str::trim).filter(|v| !v.is_empty())
}

fn get_num(a: &Attrs, name: &str) -> Option<f64> {
    get(a, name).and_then(|v| v.parse().ok()).filter(|v: &f64| v.is_finite())
}

/// One channel × time point dataset, indexed for page recipes.
struct Volume {
    /// Z, Y, X of the stored (padded) dataset.
    dims: [u64; 3],
    /// Chunk size (Z, Y, X); the whole dataset when contiguous.
    chunk: [u64; 3],
    /// Contiguous data: absolute file offset.
    contiguous: Option<u64>,
    /// Stored chunks, ordered by their first Z.
    chunks: Vec<ChunkRef>,
    filters: Vec<hdf5::Filter>,
}

pub struct Ims {
    h5: H5<HostFile>,
    info: Info,
    dtype: DType,
    big_endian: bool,
    size_c: usize,
    size_z: usize,
    /// Page size (the real image size, without padding).
    width: u64,
    height: u64,
    /// Object headers of `ResolutionLevel 0/TimePoint t`, by t.
    timepoints: Vec<u64>,
    /// Channel numbers (`Channel N`) in order.
    channels: Vec<u32>,
    /// Indexed datasets by `c + size_c * t`; `None` when absent from the file.
    volumes: RefCell<BTreeMap<usize, Option<Rc<Volume>>>>,
}

/// "Name 12" -> 12.
fn numbered(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.trim().parse().ok()
}

/// Links of `obj` named `prefix N`, sorted by N.
fn numbered_links(h5: &H5<HostFile>, obj: &Object, prefix: &str) -> Result<Vec<(u32, u64)>> {
    let mut v: Vec<(u32, u64)> =
        h5.links(obj)?.into_iter().filter_map(|(n, a)| numbered(&n, prefix).map(|k| (k, a))).collect();
    v.sort_unstable();
    Ok(v)
}

fn child(h5: &H5<HostFile>, obj: &Object, name: &str) -> Result<Option<u64>> {
    Ok(h5.links(obj)?.into_iter().find(|(n, _)| n == name).map(|(_, a)| a))
}

/// Checks the filter pipeline of a chunk (`mask` bit i: filter i skipped)
/// and returns the host codec for it.
fn codec(filters: &[hdf5::Filter], mask: u32) -> Result<Codec> {
    let mut codec = Codec::Raw;
    for (i, f) in filters.iter().enumerate() {
        if i < 32 && mask & (1 << i) != 0 {
            continue;
        }
        match f.id {
            hdf5::FILTER_DEFLATE if codec == Codec::Raw => codec = Codec::Zlib,
            // A checksum after the data; the data itself is unchanged.
            hdf5::FILTER_FLETCHER32 => {}
            hdf5::FILTER_SHUFFLE => bail!("这个 IMS 文件用了 HDF5 字节重排（shuffle）过滤器，暂不支持"),
            hdf5::FILTER_LZ4 => {
                bail!("这个 IMS 文件用 LZ4 压缩，暂不支持；请在 Imaris 里另存为 gzip 压缩或不压缩的 .ims")
            }
            id => bail!("这个 IMS 文件用了不支持的 HDF5 过滤器（编号 {id}）"),
        }
    }
    Ok(codec)
}

fn filter_names(filters: &[hdf5::Filter]) -> String {
    let names: Vec<String> = filters
        .iter()
        .map(|f| match f.id {
            hdf5::FILTER_DEFLATE => match f.values.first() {
                Some(l) => format!("gzip（级别 {l}）"),
                None => "gzip".into(),
            },
            hdf5::FILTER_SHUFFLE => "shuffle".into(),
            hdf5::FILTER_FLETCHER32 => "Fletcher32 校验".into(),
            hdf5::FILTER_LZ4 => "LZ4".into(),
            id => format!("过滤器 {id}"),
        })
        .collect();
    if names.is_empty() {
        "不压缩".into()
    } else {
        names.join(" + ")
    }
}

impl Ims {
    /// Reads and indexes the `Data` dataset of channel `c`, time point `t`.
    fn volume(&self, c: usize, t: usize) -> Result<Option<Rc<Volume>>> {
        let key = c + self.size_c * t;
        if let Some(v) = self.volumes.borrow().get(&key) {
            return Ok(v.clone());
        }
        let v = load_volume(&self.h5, self.timepoints[t], self.channels[c])?.map(Rc::new);
        self.volumes.borrow_mut().insert(key, v.clone());
        Ok(v)
    }
}

fn load_volume(h5: &H5<HostFile>, timepoint: u64, channel: u32) -> Result<Option<Volume>> {
    let tp = h5.object(timepoint)?;
    let Some(ch) = child(h5, &tp, &format!("Channel {channel}"))? else { return Ok(None) };
    let Some(data) = child(h5, &h5.object(ch)?, "Data")? else { return Ok(None) };
    let ds = h5.dataset(&h5.object(data)?)?;
    Ok(Some(index_volume(h5, ds)?))
}

fn index_volume(h5: &H5<HostFile>, ds: Dataset) -> Result<Volume> {
    if ds.dims.len() != 3 {
        bail!("IMS 数据集应为三维（Z×Y×X），这里是 {} 维", ds.dims.len());
    }
    let dims = [ds.dims[0], ds.dims[1], ds.dims[2]];
    let (chunk, contiguous, mut chunks) = match &ds.layout {
        Layout::Contiguous(a) => (dims, a.map(|a| h5.abs(a)), vec![]),
        Layout::Chunked(cd, _) => ([cd[0], cd[1], cd[2]], None, h5.chunks(&ds)?),
        Layout::Compact => bail!("不支持紧凑存储的 IMS 数据集"),
    };
    if chunk.contains(&0) {
        bail!("IMS 数据集的分块大小不对");
    }
    chunks.sort_by_key(|c| c.start[0]);
    Ok(Volume { dims, chunk, contiguous, chunks, filters: ds.filters })
}

impl Format for Ims {
    fn open(_path: &str) -> Result<Ims> {
        let head = file::read_at_most(0, 32)?;
        if head.starts_with(b"II*\0") && head.get(8..21) == Some(b"ImarisDataSet".as_slice()) {
            bail!("这是旧版 Imaris 3 格式（IMS3）的文件，暂不支持；请用 Imaris 另存为 Imaris 5.5 格式的 .ims");
        }
        let h5 = H5::open(HostFile).map_err(|e| {
            if e.0 == "不是 HDF5 文件" {
                Error("不是 Imaris 文件（不是 HDF5 格式）".into())
            } else {
                e
            }
        })?;
        let root = h5.object(h5.root)?;
        let root_attrs = h5.attributes(&root).unwrap_or_default();
        let ds_dir = get(&root_attrs, "DataSetDirectoryName").unwrap_or("DataSet").to_string();
        let info_dir = get(&root_attrs, "DataSetInfoDirectoryName").unwrap_or("DataSetInfo").to_string();
        let Some(ds_addr) = child(&h5, &root, &ds_dir)? else { bail!("不是 Imaris 文件（没有 {ds_dir} 组）") };

        let ds_obj = h5.object(ds_addr)?;
        let levels = numbered_links(&h5, &ds_obj, "ResolutionLevel ")?;
        let Some(&(0, rl0)) = levels.first() else { bail!("IMS 文件里没有 ResolutionLevel 0") };
        let tps = numbered_links(&h5, &h5.object(rl0)?, "TimePoint ")?;
        if tps.is_empty() {
            bail!("IMS 文件里没有时间点");
        }
        let tp0 = h5.object(tps[0].1)?;
        let chans = numbered_links(&h5, &tp0, "Channel ")?;
        let Some(&(_, ch0)) = chans.first() else { bail!("IMS 文件里没有通道") };
        let ch0_obj = h5.object(ch0)?;
        let ch0_attrs = h5.attributes(&ch0_obj).unwrap_or_default();
        let Some(data0) = child(&h5, &ch0_obj, "Data")? else { bail!("IMS 文件里缺少图像数据") };
        let ds = h5.dataset(&h5.object(data0)?)?;
        let dt = ds.dtype;
        let dtype = match (dt.class, dt.size, dt.signed()) {
            (0, 1, false) => DType::U8,
            (0, 1, true) => DType::I8,
            (0, 2, false) => DType::U16,
            (0, 2, true) => DType::I16,
            (0, 4, false) => DType::U32,
            (0, 4, true) => DType::I32,
            (1, 4, _) => DType::F32,
            (1, 8, _) => DType::F64,
            _ => bail!("不支持的 IMS 像素类型（HDF5 类型 {}，{} 字节）", dt.class, dt.size),
        };
        let big_endian = dt.big_endian();
        let filters_text = filter_names(&ds.filters);
        let chunk_text = match &ds.layout {
            Layout::Chunked(cd, _) => Some(cd.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("×")),
            _ => None,
        };
        // Refuse compression the host cannot decode now, not page by page.
        codec(&ds.filters, 0)?;
        let vol0 = index_volume(&h5, ds)?;

        // Metadata: attributes of every group under DataSetInfo. A group we
        // cannot read is left out rather than failing the whole file.
        let mut info_groups: Vec<(String, Attrs)> = vec![];
        if let Some(Ok(o)) = child(&h5, &root, &info_dir).ok().flatten().map(|a| h5.object(a)) {
            let mut links = h5.links(&o).unwrap_or_default();
            links.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, addr) in links {
                if let Ok(attrs) = h5.object(addr).and_then(|o| h5.attributes(&o)) {
                    info_groups.push((name, attrs));
                }
            }
        }
        let empty = Attrs::new();
        let group = |name: &str| info_groups.iter().find(|(n, _)| n == name).map(|(_, a)| a).unwrap_or(&empty);
        let image = group("Image");
        // Real size: the channel group's attributes, else DataSetInfo/Image,
        // else the (padded) dataset.
        let size = |k: usize, attr: &str, img: &str| -> u64 {
            let v = get_num(&ch0_attrs, attr).or(get_num(image, img)).map(|v| v as u64).unwrap_or(vol0.dims[k]);
            v.clamp(1, vol0.dims[k].max(1))
        };
        let (w, h, z) = (size(2, "ImageSizeX", "X"), size(1, "ImageSizeY", "Y"), size(0, "ImageSizeZ", "Z"));
        if w > u32::MAX as u64 || h > u32::MAX as u64 {
            bail!("IMS 图像太大（{w}×{h}）");
        }
        let (size_c, size_t) = (chans.len(), tps.len());
        let pages = size_c as u64 * z * size_t as u64;
        if pages > u32::MAX as u64 {
            bail!("IMS 文件的页数太多（{pages}）");
        }

        let mut info = Info::new(w as u32, h as u32, dtype, "Imaris IMS");
        info.size_c = size_c as u32;
        info.size_z = z as u32;
        info.size_t = size_t as u32;
        info.page_count = pages as u32;

        // Pixel size from the physical extents.
        let unit_scale = match get(image, "Unit").unwrap_or("um") {
            "nm" => Some(("nm", 1.0)),
            "mm" => Some(("mm", 1.0)),
            "m" => Some(("µm", 1e6)),
            "um" | "µm" | "micron" | "microns" => Some(("µm", 1.0)),
            _ => None,
        };
        let extent = |k: usize, n: u64| -> f64 {
            match (get_num(image, &format!("ExtMin{k}")), get_num(image, &format!("ExtMax{k}"))) {
                (Some(a), Some(b)) if n > 0 => (b - a).abs() / n as f64,
                _ => 0.0,
            }
        };
        let img_n = |attr: &str, fallback: u64| get_num(image, attr).map(|v| v as u64).filter(|&v| v > 0).unwrap_or(fallback);
        let mut ps = [extent(0, img_n("X", w)), extent(1, img_n("Y", h)), if z > 1 { extent(2, img_n("Z", z)) } else { 0.0 }];
        if let Some((unit, scale)) = unit_scale {
            if ps[0] > 0.0 {
                ps = ps.map(|v| v * scale);
                info.pixel_size = Some([ps[0], if ps[1] > 0.0 { ps[1] } else { ps[0] }, ps[2]]);
                info.unit = unit.into();
            }
        }

        // Channels.
        for (i, &(n, _)) in chans.iter().enumerate() {
            let a = group(&format!("Channel {n}"));
            let name = get(a, "Name").map(str::to_string).unwrap_or_else(|| format!("C{}", i + 1));
            let fallback = if size_c == 1 { [255, 255, 255] } else { DEFAULT_COLORS[i % 7] };
            let color = channel_color(a).filter(|c| c.iter().any(|&v| v > 0)).unwrap_or(fallback);
            info.channels.push(Channel { name, color });
        }

        let app = application(group("ImarisDataSet"), group("Imaris"), group("CustomData"));
        if let Some(a) = &app {
            info.format = format!("Imaris IMS ({a})");
        }

        // Info panel.
        let mut out = vec![];
        let mut g = Group::new("Imaris 图像");
        g.add("软件", app.clone());
        g.add("文件格式", get(&root_attrs, "ImarisVersion").map(|v| format!("Imaris {v}")));
        g.add("名称", get(image, "Name").map(str::to_string));
        g.add("描述", get(image, "Description").filter(|d| *d != "(description not specified)").map(str::to_string));
        g.add("采集时间", get(image, "RecordingDate").map(str::to_string));
        let mut dims = format!("X={w}  Y={h}");
        for (k, v) in [("Z", z), ("C", size_c as u64), ("T", size_t as u64)] {
            if v > 1 {
                dims.push_str(&format!("  {k}={v}"));
            }
        }
        g.add_str("尺寸", &dims);
        g.add_str("像素类型", dtype.name());
        g.add_str("压缩", &filters_text);
        g.add("分块 (Z×Y×X)", chunk_text);
        if levels.len() > 1 {
            g.add_str("分辨率层级", &format!("{}（显示最高分辨率）", levels.len()));
        }
        if let Some(p) = info.pixel_size {
            let u = &info.unit;
            g.add_str("像素尺寸 X", &format!("{} {u}", num(p[0])));
            g.add_str("像素尺寸 Y", &format!("{} {u}", num(p[1])));
            if p[2] > 0.0 {
                g.add_str("Z 步长", &format!("{} {u}", num(p[2])));
            }
        }
        g.add("显微镜模式", get(image, "MicroscopeMode").map(str::to_string));
        g.add("物镜放大倍数", get(image, "LensPower").or(get(image, "Lenspower")).filter(|v| *v != "not known").map(str::to_string));
        g.add("数值孔径 NA", get(image, "NumericalAperture").map(str::to_string));
        g.add("原始格式", get(image, "OriginalFormat").map(str::to_string));
        time_items(&mut g, group("TimeInfo"), size_t);
        g.push_to(&mut out);

        for (i, &(n, _)) in chans.iter().enumerate() {
            let a = group(&format!("Channel {n}"));
            let mut g = Group::new(&format!("通道 {}：{}", i + 1, info.channels[i].name));
            let nm = |k: &str| get(a, k).map(|v| v.replace("nm nm", "nm"));
            g.add("描述", get(a, "Description").filter(|d| *d != "(description not specified)").map(str::to_string));
            g.add("激发波长", nm("LSMExcitationWavelength"));
            g.add("发射波长", nm("LSMEmissionWavelength"));
            g.add("针孔", get(a, "LSMPinhole").or(get(a, "Pinhole")).filter(|v| *v != "0").map(str::to_string));
            g.add("物镜", get(a, "Objective").map(str::to_string));
            g.add("增益", get(a, "Gain").map(str::to_string));
            g.add("偏移", get(a, "Offset").map(str::to_string));
            g.add("颜色 (RGB)", get(a, "Color").map(str::to_string));
            g.add("显示范围", get(a, "ColorRange").map(str::to_string));
            g.add("伽马", get(a, "GammaCorrection").map(str::to_string));
            if i == 0 {
                if let (Some(lo), Some(hi)) = (get(&ch0_attrs, "HistogramMin"), get(&ch0_attrs, "HistogramMax")) {
                    g.add_str("强度范围（第 1 个时间点）", &format!("{lo} – {hi}"));
                }
            }
            g.push_to(&mut out);
        }

        // Everything, flattened, for search.
        let mut g = Group::new("Imaris 全部参数");
        for (k, v) in &root_attrs {
            g.add_str(k, &short(v.as_deref().unwrap_or("")));
        }
        for (name, attrs) in &info_groups {
            for (k, v) in attrs {
                g.add_str(&format!("{name}/{k}"), &short(v.as_deref().unwrap_or("")));
            }
        }
        g.push_to(&mut out);
        info.metadata = out;

        let mut volumes = BTreeMap::new();
        volumes.insert(0, Some(Rc::new(vol0)));
        Ok(Ims {
            h5,
            info,
            dtype,
            big_endian,
            size_c,
            size_z: z as usize,
            width: w,
            height: h,
            timepoints: tps.iter().map(|&(_, a)| a).collect(),
            channels: chans.iter().map(|&(n, _)| n).collect(),
            volumes: RefCell::new(volumes),
        })
    }

    fn info(&mut self) -> Result<Info> {
        Ok(std::mem::replace(&mut self.info, Info::new(1, 1, DType::U8, "")))
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        let page = page as usize;
        let c = page % self.size_c;
        let z = (page / self.size_c) % self.size_z;
        let t = page / (self.size_c * self.size_z);
        let mut r = Recipe::new();
        if t >= self.timepoints.len() {
            return Ok(r);
        }
        let Some(v) = self.volume(c, t)? else { return Ok(r) };
        let size = self.dtype.size() as u64;
        let be = if self.big_endian { flags::BIG_ENDIAN } else { 0 };
        let z = z as u64;
        if z >= v.dims[0] {
            return Ok(r);
        }
        let [_, cy, cx] = v.chunk;
        let row = cx * size;
        let plane = cy * row;
        if row > u32::MAX as u64 || cx > u32::MAX as u64 || cy > u32::MAX as u64 {
            bail!("IMS 分块太大");
        }
        if let Some(a) = v.contiguous {
            r.push(Chunk::file(a + z * plane, plane, [0, 0, cx as u32, cy as u32]).stride(row as u32).flags(be));
            return Ok(r);
        }
        let first = v.chunks.partition_point(|ch| ch.start[0] + v.chunk[0] <= z);
        for ch in &v.chunks[first..] {
            if ch.start[0] > z {
                break;
            }
            let (x0, y0) = (ch.start[2], ch.start[1]);
            if x0 >= self.width || y0 >= self.height {
                continue;
            }
            let rect = [x0 as u32, y0 as u32, cx as u32, cy as u32];
            let dz = z - ch.start[0];
            let chunk = match codec(&v.filters, ch.mask)? {
                Codec::Raw => {
                    if (dz + 1) * plane > ch.size {
                        bail!("IMS 数据块被截断");
                    }
                    Chunk::file(ch.addr + dz * plane, plane, rect)
                }
                codec => Chunk::file(ch.addr, ch.size, rect).codec(codec).skip(dz * plane),
            };
            r.push(chunk.stride(row as u32).flags(be));
        }
        Ok(r)
    }
}

export_format!(Ims);

// ---------------------------------------------------------------- metadata helpers

/// "r g b" in 0..1 (BaseColor), or the brightest entry of a colour table.
fn channel_color(a: &Attrs) -> Option<[u8; 3]> {
    let parse = |s: &str| -> Option<Vec<f64>> { s.split_whitespace().map(|v| v.parse::<f64>().ok()).collect() };
    let v = match get(a, "ColorMode") {
        Some("TableColor") => {
            let t = parse(get(a, "ColorTable")?)?;
            if t.len() < 3 {
                return None;
            }
            t[t.len() - 3..].to_vec()
        }
        _ => parse(get(a, "Color")?)?,
    };
    if v.len() < 3 {
        return None;
    }
    let max = if v[..3].iter().any(|&x| x > 1.0) { 255.0 } else { 1.0 };
    Some([0, 1, 2].map(|i| (v[i] / max * 255.0).round().clamp(0.0, 255.0) as u8))
}

/// The program that wrote the file, e.g. "Imaris x64 9.0" or "Fusion 2.0.0.13".
fn application(ds: &Attrs, imaris: &Attrs, custom: &Attrs) -> Option<String> {
    if let Some(v) = get(custom, "FusionVersion") {
        return Some(format!("Fusion {v}"));
    }
    let creator = get(ds, "Creator").unwrap_or("");
    let version = get(ds, "Version").or(get(imaris, "Version")).unwrap_or("");
    let s = if creator.contains(version) { creator.to_string() } else { format!("{creator} {version}") };
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Start, end and mean interval of the time points.
fn time_items(g: &mut Group, t: &Attrs, size_t: usize) {
    let first = get(t, "TimePoint1");
    let last = get(t, &format!("TimePoint{size_t}"));
    if size_t > 1 {
        g.add("开始时间", first.map(str::to_string));
        g.add("结束时间", last.map(str::to_string));
        if let (Some(a), Some(b)) = (first.and_then(seconds), last.and_then(seconds)) {
            if b > a {
                g.add_str("时间间隔（平均）", &format!("{} s", num((b - a) / (size_t - 1) as f64)));
            }
        }
    }
}

/// "2018-10-22 13:11:03.295" -> seconds since 1970 (time zone ignored).
fn seconds(s: &str) -> Option<f64> {
    let (date, time) = s.trim().split_once(' ')?;
    let d: Vec<i64> = date.split('-').map(|v| v.parse().ok()).collect::<Option<_>>()?;
    let t: Vec<f64> = time.split(':').map(|v| v.parse().ok()).collect::<Option<_>>()?;
    if d.len() != 3 || t.len() != 3 {
        return None;
    }
    // Days from civil date (proleptic Gregorian).
    let (y, m) = if d[1] <= 2 { (d[0] - 1, d[1] + 9) } else { (d[0], d[1] - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d[2] - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days as f64 * 86400.0 + t[0] * 3600.0 + t[1] * 60.0 + t[2])
}

/// Very long values (embedded documents) cut for the info panel.
fn short(v: &str) -> Cow<'_, str> {
    const MAX: usize = 4000;
    if v.len() <= MAX {
        return Cow::Borrowed(v);
    }
    let mut end = MAX;
    while !v.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}…", &v[..end]))
}

/// Up to four decimals, without trailing zeros.
fn num(v: f64) -> String {
    let r = format!("{v:.4}");
    let r = r.trim_end_matches('0').trim_end_matches('.');
    if (r == "0" || r == "-0") && v != 0.0 {
        format!("{v:.3e}")
    } else {
        r.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hdf5::Filter;

    fn f(id: u16) -> Filter {
        Filter { id, values: vec![] }
    }

    #[test]
    fn filters_map_to_host_codecs() {
        assert_eq!(codec(&[], 0).unwrap(), Codec::Raw);
        assert_eq!(codec(&[f(hdf5::FILTER_DEFLATE)], 0).unwrap(), Codec::Zlib);
        assert_eq!(codec(&[f(hdf5::FILTER_DEFLATE), f(hdf5::FILTER_FLETCHER32)], 0).unwrap(), Codec::Zlib);
        // Filter mask bit set: that filter was skipped for this chunk.
        assert_eq!(codec(&[f(hdf5::FILTER_DEFLATE)], 1).unwrap(), Codec::Raw);
        assert!(codec(&[f(hdf5::FILTER_SHUFFLE), f(hdf5::FILTER_DEFLATE)], 0).unwrap_err().0.contains("shuffle"));
        assert!(codec(&[f(hdf5::FILTER_LZ4)], 0).unwrap_err().0.contains("LZ4"));
        assert!(codec(&[f(307)], 0).unwrap_err().0.contains("307"));
        assert_eq!(filter_names(&[f(hdf5::FILTER_DEFLATE)]), "gzip");
        assert_eq!(filter_names(&[]), "不压缩");
    }

    #[test]
    fn metadata_helpers() {
        let a: Attrs = vec![("Color".into(), Some("0.192 0.878 0.192".into())), ("ColorMode".into(), Some("BaseColor".into()))];
        assert_eq!(channel_color(&a), Some([49, 224, 49]));
        let t: Attrs = vec![("ColorMode".into(), Some("TableColor".into())), ("ColorTable".into(), Some("0 0 0 0 0.5 1".into()))];
        assert_eq!(channel_color(&t), Some([0, 128, 255]));
        let d = seconds("2018-10-22 13:11:26.003").unwrap() - seconds("2018-10-22 13:11:03.295").unwrap();
        assert!((d - 22.708).abs() < 1e-6);
        assert_eq!(seconds("1970-01-02 00:00:00"), Some(86400.0));
        assert_eq!(numbered("TimePoint 12", "TimePoint "), Some(12));
        assert_eq!(numbered("TimePointX", "TimePoint "), None);
        assert_eq!(short(&"é".repeat(3000)).chars().count(), 2001);
        let custom: Attrs = vec![("FusionVersion".into(), Some("2.0.0.13".into()))];
        assert_eq!(application(&vec![], &vec![], &custom).as_deref(), Some("Fusion 2.0.0.13"));
        let ds: Attrs = vec![("Creator".into(), Some("Imaris x64".into())), ("Version".into(), Some("9.0".into()))];
        assert_eq!(application(&ds, &vec![], &vec![]).as_deref(), Some("Imaris x64 9.0"));
    }
}
