//! Velox `.emd` files: HDF5, as written by Thermo Fisher Velox on Talos,
//! Spectra and Themis microscopes.
//!
//! Each image of the file is a group `/Data/Image/<id>` with a dataset
//! `Data` (rows × columns × frames, so frames are the fastest index) and a
//! dataset `Metadata` (bytes × frames: one zero-padded JSON document per
//! frame with the detector, pixel size, optics, stage…). Several images
//! (HAADF, BF, DF4, the camera…) become series. Spectrum data
//! (`SpectrumImage`, `SpectrumStream`) is not shown.
//!
//! When a dataset chunk holds one frame, a page is that chunk as it is.
//! When frames are interleaved, the frame is picked out as the first of
//! `frames` samples per pixel (the app drops the others), or, for more
//! than 64 frames per chunk, copied out by the plugin.

use crate::hdf5::{self, ChunkRef, Dataset, Layout, Object, H5};
use crate::json::{self, Flat};
use crate::{fmt, length, show_length, unix_time};
use tifmrcview_plugin_sdk::{
    bail, decompress, file, flags, Channel, Chunk, Codec, DType, Error, Group, Info, Recipe,
    Result, Series,
};

/// The file, read through the host.
pub struct HostFile;

impl hdf5::Source for HostFile {
    fn size(&self) -> u64 {
        file::size()
    }
    fn read(&self, off: u64, len: usize) -> Result<Vec<u8>> {
        file::read_at_most(off, len)
    }
}

/// Most samples per pixel a chunk may have in a recipe.
const MAX_SAMPLES: u64 = 64;

/// One stored block of a dataset: where it starts (row, column, frame),
/// its size in elements, and how its bytes are stored.
#[derive(Clone)]
struct Block {
    start: [u64; 3],
    dims: [u64; 3],
    addr: u64,
    size: u64,
    codec: Codec,
    flags: u8,
}

struct Image {
    name: String,
    height: u64,
    width: u64,
    frames: u64,
    dtype: DType,
    big_endian: bool,
    blocks: Vec<Block>,
    meta: Flat,
    filters: String,
}

pub struct Emd {
    images: Vec<Image>,
    current: usize,
    spectra: usize,
}

fn child(h5: &H5<HostFile>, obj: &Object, name: &str) -> Result<Option<u64>> {
    Ok(h5
        .links(obj)?
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, a)| a))
}

fn codec(filters: &[hdf5::Filter], mask: u32) -> Result<(Codec, u8)> {
    let (mut codec, mut fl) = (Codec::Raw, 0);
    for (i, f) in filters.iter().enumerate() {
        if i < 32 && mask & (1 << i) != 0 {
            continue;
        }
        match f.id {
            hdf5::FILTER_SHUFFLE if codec == Codec::Raw => fl |= flags::SHUFFLE,
            hdf5::FILTER_DEFLATE if codec == Codec::Raw => codec = Codec::Zlib,
            hdf5::FILTER_LZ4 if codec == Codec::Raw => codec = Codec::Lz4H5,
            hdf5::FILTER_FLETCHER32 => {}
            id => bail!("这个 EMD 文件用了不支持的 HDF5 过滤器（编号 {id}）"),
        }
    }
    Ok((codec, fl))
}

fn filter_names(filters: &[hdf5::Filter]) -> String {
    let n: Vec<&str> = filters
        .iter()
        .map(|f| match f.id {
            hdf5::FILTER_DEFLATE => "gzip",
            hdf5::FILTER_SHUFFLE => "shuffle",
            hdf5::FILTER_FLETCHER32 => "Fletcher32",
            hdf5::FILTER_LZ4 => "LZ4",
            _ => "?",
        })
        .collect();
    if n.is_empty() {
        "不压缩".into()
    } else {
        n.join(" + ")
    }
}

fn dtype(ds: &Dataset) -> Result<DType> {
    let t = ds.dtype;
    Ok(match (t.class, t.size, t.signed()) {
        (0, 1, false) => DType::U8,
        (0, 1, true) => DType::I8,
        (0, 2, false) => DType::U16,
        (0, 2, true) => DType::I16,
        (0, 4, false) => DType::U32,
        (0, 4, true) => DType::I32,
        (1, 4, _) => DType::F32,
        (1, 8, _) => DType::F64,
        _ => bail!("不支持这种像素类型（HDF5 类 {}，{} 字节）", t.class, t.size),
    })
}

/// The stored blocks of a dataset, padded to three dimensions.
fn blocks(h5: &H5<HostFile>, ds: &Dataset) -> Result<Vec<Block>> {
    // A 2D dataset is one frame: size 1, starting at 0.
    let pad =
        |v: &[u64], fill: u64| -> [u64; 3] { [v[0], v[1], v.get(2).copied().unwrap_or(fill)] };
    let bs = ds.dtype.size as u64;
    let dims = pad(&ds.dims, 1);
    Ok(match &ds.layout {
        Layout::Contiguous(Some(a)) => {
            vec![Block {
                start: [0; 3],
                dims,
                addr: h5.abs(*a),
                size: dims.iter().product::<u64>() * bs,
                codec: Codec::Raw,
                flags: 0,
            }]
        }
        Layout::Contiguous(None) => vec![],
        Layout::Chunked(cd, _) => {
            let cd = pad(cd, 1);
            let mut v = Vec::new();
            for ChunkRef {
                start,
                addr,
                size,
                mask,
            } in h5.chunks(ds)?
            {
                let (codec, fl) = codec(&ds.filters, mask)?;
                v.push(Block {
                    start: pad(&start, 0),
                    dims: cd,
                    addr,
                    size,
                    codec,
                    flags: fl,
                });
            }
            v
        }
        Layout::Compact => bail!("不支持紧凑存储的 EMD 数据集"),
    })
}

/// Bytes of one block as stored on disk, decompressed.
fn block_bytes(b: &Block, bs: u64) -> Result<Vec<u8>> {
    let raw = file::read(b.addr, b.size as usize)?;
    let n = (b.dims.iter().product::<u64>() * bs) as usize;
    let mut out = if b.codec == Codec::Raw {
        raw
    } else {
        decompress(b.codec, &raw, n)?
    };
    if b.flags & flags::SHUFFLE != 0 && bs > 1 {
        let bs = bs as usize;
        let k = out.len() / bs;
        let mut v = vec![0; k * bs];
        for (j, byte) in out[..k * bs].iter().enumerate() {
            v[(j % k) * bs + j / k] = *byte;
        }
        v.extend_from_slice(&out[k * bs..]);
        out = v;
    }
    Ok(out)
}

/// The JSON of frame 0 from the `Metadata` dataset (bytes × frames).
fn frame_metadata(h5: &H5<HostFile>, ds: &Dataset) -> Result<Flat> {
    if ds.dims.len() != 2 || ds.dtype.size != 1 {
        return Ok(vec![]);
    }
    let (rows, frames) = (ds.dims[0], ds.dims[1].max(1));
    let Some(b) = blocks(h5, ds)?.into_iter().find(|b| b.start == [0, 0, 0]) else {
        return Ok(vec![]);
    };
    let cols = b.dims[1].max(1);
    let n = (rows.min(b.dims[0]) * cols).min(64 << 20);
    if b.codec == Codec::Raw && b.flags == 0 && cols == frames && cols > 1 {
        // Contiguous with frames interleaved: read only what frame 0 needs.
        let take = (rows * cols).min(n);
        let bytes = file::read(b.addr, take as usize)?;
        return Ok(parse_json(
            &bytes
                .iter()
                .step_by(cols as usize)
                .copied()
                .collect::<Vec<u8>>(),
        ));
    }
    let bytes = block_bytes(
        &Block {
            dims: [b.dims[0], cols, 1],
            ..b
        },
        1,
    )?;
    let col: Vec<u8> = bytes
        .iter()
        .take(n as usize)
        .step_by(cols as usize)
        .copied()
        .collect();
    Ok(parse_json(&col))
}

fn parse_json(b: &[u8]) -> Flat {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    json::flatten(&b[..end]).unwrap_or_default()
}

fn get<'a>(m: &'a Flat, key: &str) -> Option<&'a str> {
    m.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

fn num(m: &Flat, key: &str) -> Option<f64> {
    get(m, key)
        .and_then(|v| v.parse().ok())
        .filter(|v: &f64| v.is_finite())
}

impl Emd {
    pub fn open(series: u32) -> Result<Emd> {
        let h5 = H5::open(HostFile).map_err(|e| {
            if e.0 == "不是 HDF5 文件" {
                Error("不是 Velox EMD 文件（不是 HDF5 格式）".into())
            } else {
                e
            }
        })?;
        let root = h5.object(h5.root)?;
        let Some(data) = child(&h5, &root, "Data")? else {
            bail!("不是 Velox EMD 文件（没有 /Data 组）；Berkeley EMD 等其他 EMD 文件暂不支持");
        };
        let data = h5.object(data)?;
        let groups = h5.links(&data)?;
        let spectra = groups
            .iter()
            .filter(|(n, _)| n.starts_with("Spectrum"))
            .count();
        let Some((_, img)) = groups.iter().find(|(n, _)| n == "Image") else {
            if spectra > 0 {
                bail!("这个 Velox 文件只有谱图数据（EDS / EELS），没有图像，暂不支持");
            }
            bail!("这个 Velox 文件里没有图像");
        };
        let img = h5.object(*img)?;
        let mut images = Vec::new();
        let mut first_err = None;
        for (id, addr) in h5.links(&img)? {
            match load(&h5, &id, addr) {
                Ok(Some(i)) => images.push(i),
                Ok(None) => {}
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        if images.is_empty() {
            return Err(first_err.unwrap_or_else(|| Error("这个 Velox 文件里没有图像数据".into())));
        }
        let current = series as usize;
        if current >= images.len() {
            bail!(
                "这个文件只有 {} 个图像（请求第 {} 个）",
                images.len(),
                current + 1
            );
        }
        Ok(Emd {
            images,
            current,
            spectra,
        })
    }

    pub fn info(&mut self) -> Result<Info> {
        let i = &self.images[self.current];
        let m = &i.meta;
        let model =
            get(m, "Instrument.InstrumentModel").or_else(|| get(m, "Instrument.InstrumentClass"));
        let format = match get(m, "Instrument.ControlSoftwareVersion") {
            Some(v) => format!("Velox EMD (Velox {v})"),
            None => "Velox EMD".into(),
        };
        let mut info = Info::new(i.width as u32, i.height as u32, i.dtype, &format);
        info.size_t = i.frames as u32;
        info.page_count = info.size_t;
        info.channels = vec![Channel {
            name: i.name.clone(),
            color: [255, 255, 255],
        }];
        let unit_m = get(m, "BinaryResult.PixelUnitX").is_none_or(|u| u == "m");
        if let Some(px) = num(m, "BinaryResult.PixelSize.width").filter(|&v| v > 0.0 && unit_m) {
            let py = num(m, "BinaryResult.PixelSize.height")
                .filter(|&v| v > 0.0)
                .unwrap_or(px);
            let (x, unit) = length(px);
            info.pixel_size = Some([x, crate::tidy(py * x / px), 0.0]);
            info.unit = unit.into();
        } else if get(m, "BinaryResult.PixelUnitX") == Some("1/m") {
            // Diffraction patterns.
            if let Some(px) = num(m, "BinaryResult.PixelSize.width").filter(|&v| v > 0.0) {
                let py = num(m, "BinaryResult.PixelSize.height").filter(|&v| v > 0.0).unwrap_or(px);
                info.pixel_size = Some([crate::reciprocal(px).0, crate::reciprocal(py).0, 0.0]);
                info.unit = "1/nm".into();
            }
        }

        let mut g = Group::new("Velox 图像");
        g.add_str("探测器", &i.name);
        g.add("显微镜", model.map(str::to_string));
        g.add(
            "制造商",
            get(m, "Instrument.Manufacturer").map(str::to_string),
        );
        g.add(
            "仪器编号",
            get(m, "Instrument.InstrumentId").map(str::to_string),
        );
        g.add(
            "Velox 版本",
            get(m, "Instrument.ControlSoftwareVersion").map(str::to_string),
        );
        let t = num(m, "Acquisition.AcquisitionStartDatetime.DateTime")
            .or_else(|| num(m, "Acquisition.AcquisitionDatetime.DateTime"))
            .filter(|&t| t > 0.0);
        g.add("采集时间", t.and_then(|t| unix_time(t as i64)));
        g.add_str(
            "尺寸",
            &format!("{} × {}，{} 帧", i.width, i.height, i.frames),
        );
        g.add_str("像素类型", i.dtype.name());
        g.add_str("压缩", &i.filters);
        if let Some(px) = num(m, "BinaryResult.PixelSize.width") {
            let u = get(m, "BinaryResult.PixelUnitX").unwrap_or("m");
            g.add_str(
                "像素尺寸",
                &if u == "m" {
                    show_length(px)
                } else {
                    format!("{} {u}", fmt(px))
                },
            );
        }
        g.add(
            "加速电压",
            num(m, "Optics.AccelerationVoltage").map(|v| format!("{} kV", fmt(v / 1000.0))),
        );
        g.add(
            "标称放大倍数",
            num(m, "Optics.NominalMagnification").map(|v| format!("{}×", fmt(v.round()))),
        );
        g.add(
            "相机长度",
            num(m, "Optics.CameraLength").map(|v| format!("{} mm", fmt(v * 1000.0))),
        );
        g.add("聚光镜束斑", get(m, "Optics.SpotIndex").map(str::to_string));
        g.add(
            "散焦",
            num(m, "Optics.Defocus")
                .filter(|&v| v != 0.0)
                .map(show_length),
        );
        g.add(
            "会聚半角",
            num(m, "Optics.BeamConvergence").map(|v| format!("{} mrad", fmt(v * 1000.0))),
        );
        g.add(
            "TEM 子模式",
            get(m, "Optics.TemOperatingSubMode").map(str::to_string),
        );
        g.add(
            "物镜模式",
            get(m, "Optics.ObjectiveLensMode").map(str::to_string),
        );
        g.add(
            "高倍模式",
            get(m, "Optics.HighMagnificationMode").map(str::to_string),
        );
        g.add(
            "电子源",
            get(m, "Acquisition.SourceType").map(str::to_string),
        );
        g.add(
            "扫描帧时间",
            num(m, "Scan.FrameTime")
                .filter(|&v| v > 0.0)
                .map(|v| format!("{} s", fmt(v))),
        );
        g.add(
            "驻留时间",
            num(m, "Scan.DwellTime").map(|v| format!("{} µs", fmt(v * 1e6))),
        );
        for (k, label) in [("x", "样品台 X"), ("y", "样品台 Y"), ("z", "样品台 Z")] {
            g.add(
                label,
                num(m, &format!("Stage.Position.{k}")).map(show_length),
            );
        }
        g.add(
            "α 倾转",
            num(m, "Stage.AlphaTilt")
                .map(|v| format!("{}°", fmt((v.to_degrees() * 1000.0).round() / 1000.0))),
        );
        g.add(
            "β 倾转",
            num(m, "Stage.BetaTilt")
                .map(|v| format!("{}°", fmt((v.to_degrees() * 1000.0).round() / 1000.0))),
        );
        g.add("样品杆", get(m, "Stage.HolderType").map(str::to_string));
        if self.spectra > 0 {
            g.add_str("说明", "文件里还有谱图数据（EDS / EELS），不显示");
        }
        g.push_to(&mut info.metadata);

        let mut all = Group::new("Velox 全部参数");
        for (k, v) in m.iter().take(50_000) {
            all.push(k, v);
        }
        all.push_to(&mut info.metadata);

        if self.images.len() > 1 {
            info.series = self
                .images
                .iter()
                .map(|im| Series {
                    name: im.name.clone(),
                    detail: format!(
                        "{}×{} · {} 帧 · {}",
                        im.width,
                        im.height,
                        im.frames,
                        im.dtype.name()
                    ),
                })
                .collect();
            info.series_index = self.current as u32;
        }
        Ok(info)
    }

    pub fn page(&self, page: u32) -> Result<Recipe> {
        let i = &self.images[self.current];
        let f = page as u64;
        let bs = i.dtype.size() as u64;
        let be = if i.big_endian { flags::BIG_ENDIAN } else { 0 };
        let mut r = Recipe::new();
        for b in &i.blocks {
            let [y0, x0, f0] = b.start;
            let [cy, cx, cf] = b.dims;
            if f < f0 || f >= f0 + cf || y0 >= i.height || x0 >= i.width {
                continue;
            }
            let rect = [x0 as u32, y0 as u32, cx as u32, cy as u32];
            let shift = (f - f0) * bs;
            if cf == 1 {
                r.push(
                    Chunk::file(b.addr, b.size, rect)
                        .codec(b.codec)
                        .flags(b.flags | be),
                );
                continue;
            }
            if cf <= MAX_SAMPLES {
                interleaved(&mut r, b, shift, bs, be);
                continue;
            }
            // Too many frames per pixel for a recipe: copy the frame out.
            let mut out = Vec::with_capacity((cy * cx * bs) as usize);
            let row = cx * cf * bs;
            if b.codec == Codec::Raw && b.flags == 0 {
                for y in 0..cy.min(i.height - y0) {
                    let line = file::read(b.addr + y * row, row as usize)?;
                    for x in 0..cx {
                        let s = (x * cf * bs + shift) as usize;
                        out.extend_from_slice(&line[s..s + bs as usize]);
                    }
                }
                let rows = cy.min(i.height - y0);
                r.inline(&out, Codec::Raw, [rect[0], rect[1], rect[2], rows as u32])
                    .flags = be;
            } else {
                let bytes = block_bytes(b, bs)?;
                for p in 0..cy * cx {
                    let s = (p * cf * bs + shift) as usize;
                    out.extend_from_slice(
                        bytes
                            .get(s..s + bs as usize)
                            .ok_or_else(|| Error("EMD 数据块被截断".into()))?,
                    );
                }
                r.inline(&out, Codec::Raw, rect).flags = be;
            }
        }
        Ok(r)
    }
}

/// A block whose pixels hold `cf` frames each: frame k is sample 0 once
/// the data is shifted by k samples. The last pixel is then short of the
/// samples after it, so the last row and the last pixel are separate
/// chunks.
fn interleaved(r: &mut Recipe, b: &Block, shift: u64, bs: u64, be: u8) {
    let [cy, cx, cf] = b.dims;
    let (x0, y0) = (b.start[1] as u32, b.start[0] as u32);
    let px = cf * bs;
    let row = cx * px;
    let raw = b.codec == Codec::Raw && b.flags & flags::SHUFFLE == 0;
    let mut part = |skip: u64, rect: [u32; 4], samples: u32| {
        let c = if raw {
            Chunk::file(b.addr + skip, b.size.saturating_sub(skip), rect)
        } else {
            Chunk::file(b.addr, b.size, rect)
                .codec(b.codec)
                .flags(b.flags)
                .skip(skip)
        };
        r.push(
            c.samples(samples)
                .stride(if samples == 1 { 0 } else { row as u32 })
                .flags(be),
        );
    };
    if cy > 1 {
        part(shift, [x0, y0, cx as u32, cy as u32 - 1], cf as u32);
    }
    let last = (cy - 1) * row + shift;
    let y = y0 + cy as u32 - 1;
    if cx > 1 {
        part(last, [x0, y, cx as u32 - 1, 1], cf as u32);
    }
    part(last + (cx - 1) * px, [x0 + cx as u32 - 1, y, 1, 1], 1);
}

fn load(h5: &H5<HostFile>, id: &str, addr: u64) -> Result<Option<Image>> {
    let g = h5.object(addr)?;
    let Some(d) = child(h5, &g, "Data")? else {
        return Ok(None);
    };
    let ds = h5.dataset(&h5.object(d)?)?;
    if !(2..=3).contains(&ds.dims.len()) || ds.dims.contains(&0) {
        return Ok(None);
    }
    let dtype = dtype(&ds)?;
    let (height, width) = (ds.dims[0], ds.dims[1]);
    let frames = ds.dims.get(2).copied().unwrap_or(1);
    if width > u32::MAX as u64 || height > u32::MAX as u64 || frames > u32::MAX as u64 {
        bail!("EMD 图像太大");
    }
    let meta = match child(h5, &g, "Metadata")? {
        Some(a) => frame_metadata(h5, &h5.dataset(&h5.object(a)?)?).unwrap_or_default(),
        None => vec![],
    };
    let name = get(&meta, "BinaryResult.Detector")
        .map(str::to_string)
        .unwrap_or_else(|| id.chars().take(8).collect());
    let blocks = blocks(h5, &ds)?;
    if blocks.iter().any(|b| {
        b.dims[0] > u32::MAX as u64 || b.dims[1] * b.dims[2] * dtype.size() as u64 > u32::MAX as u64
    }) {
        bail!("EMD 数据块太大");
    }
    Ok(Some(Image {
        name,
        height,
        width,
        frames,
        dtype,
        big_endian: ds.dtype.big_endian(),
        filters: filter_names(&ds.filters),
        blocks,
        meta,
    }))
}
