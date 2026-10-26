//! Olympus OIB (FluoView FV1000 / FV1200 / FV10i): an OLE2 compound file
//! holding the files of an OIF folder. `OibInfo.txt` maps the container's
//! stream names (`Stream00001`…) to the original file names; the main
//! `.oif` settings file holds the axes and acquisition settings; every
//! plane is one small TIFF named after its indices (`s_C001Z003T002.tif`),
//! with a `.pty` settings file next to it.
//!
//! Plane TIFFs are parsed when their page is asked for; their strips are
//! mapped through the stream's sector chain to file ranges.

use std::collections::BTreeMap;

use tifmrcview_plugin_sdk::tiff::{self, tag};
use tifmrcview_plugin_sdk::{bail, flags, Channel, Chunk, Codec, Context, DType, Group, Info, Recipe, Result};

use crate::cfb::{self, Cfb};
use crate::common::{detail, merge_chunks, num, raw_rows, Ini, Piece, G, DEFAULT_COLORS};

/// One plane: its stream and its position.
struct Plane {
    stream: String,
    /// Original file name (`s_C001Z001.tif`).
    name: String,
    c: u32,
    z: u32,
    t: u32,
    l: u32,
}

pub struct Oib {
    cfb: Cfb,
    info: Info,
    dtype: DType,
    width: u32,
    height: u32,
    /// Stream of each page (None: the plane is missing; shown as zeros).
    pages: Vec<Option<String>>,
}

/// Original file name → stream name, from OibInfo.txt.
fn stream_map(cfb: &Cfb) -> Result<(Ini, BTreeMap<String, String>)> {
    let e = cfb.stream("OibInfo.txt").context("不是 OIB 文件（缺少 OibInfo.txt）")?;
    let info = Ini::parse(&cfb.read_stream(e, 1 << 20)?);
    let mut map = BTreeMap::new();
    for (k, v) in info.section("OibSaveInfo").unwrap_or(&[]) {
        if k.starts_with("Stream") {
            let base = v.rsplit(['/', '\\']).next().unwrap_or(v);
            map.insert(base.to_string(), k.clone());
        }
    }
    Ok((info, map))
}

/// `s_C001Z002T003.tif` -> ("s", [('C', 1), ('Z', 2), ('T', 3)]).
fn plane_indices(name: &str) -> Option<(&str, Vec<(u8, u32)>)> {
    let stem = name.strip_suffix(".tif").or_else(|| name.strip_suffix(".TIF"))?;
    let (prefix, axes) = stem.rsplit_once('_')?;
    let b = axes.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let a = b[i];
        if !a.is_ascii_uppercase() {
            return None;
        }
        let j = i + 1 + b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if j == i + 1 {
            return None;
        }
        out.push((a, axes[i + 1..j].parse().ok()?));
        i = j;
    }
    (!out.is_empty()).then_some((prefix, out))
}

fn rank(v: &mut Vec<u32>) {
    v.sort_unstable();
    v.dedup();
}

impl Oib {
    pub fn open() -> Result<Oib> {
        let cfb = Cfb::open()?;
        let (oibinfo, names) = stream_map(&cfb)?;
        let main_name = oibinfo.s("OibSaveInfo", "MainFileName").context("OIB 文件损坏（没有主设置文件）")?;
        let main = match cfb.stream(&main_name) {
            Some(e) => Ini::parse(&cfb.read_stream(e, 16 << 20)?),
            None => bail!("OIB 文件损坏（找不到 {main_name}）"),
        };

        // Plane TIFFs, grouped by name prefix; the largest group is the image.
        let mut groups: BTreeMap<&str, Vec<Plane>> = BTreeMap::new();
        for (name, stream) in &names {
            let Some((prefix, axes)) = plane_indices(name) else { continue };
            let mut p = Plane { stream: stream.clone(), name: name.clone(), c: 0, z: 0, t: 0, l: 0 };
            let mut other = false;
            for (a, i) in axes {
                match a {
                    b'C' => p.c = i,
                    b'Z' => p.z = i,
                    b'T' => p.t = i,
                    b'L' => p.l = i,
                    // Other axes (angle, point…): first index only.
                    _ => other |= i > 1,
                }
            }
            if !other {
                groups.entry(prefix).or_default().push(p);
            }
        }
        let planes = groups.into_values().max_by_key(|g| g.len()).context("OIB 文件里没有图像")?;

        let (mut cs, mut zs, mut ts, mut ls): (Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>) = (
            planes.iter().map(|p| p.c).collect(),
            planes.iter().map(|p| p.z).collect(),
            planes.iter().map(|p| p.t).collect(),
            planes.iter().map(|p| p.l).collect(),
        );
        for v in [&mut cs, &mut zs, &mut ts, &mut ls] {
            rank(v);
        }
        // Lambda (spectral) planes become extra channels.
        let size_c = (cs.len() * ls.len()) as u32;
        let (size_z, size_t) = (zs.len() as u32, ts.len() as u32);
        let mut pages: Vec<Option<String>> = vec![None; (size_c * size_z * size_t) as usize];
        let pos = |v: &[u32], x: u32| v.binary_search(&x).unwrap() as u32;
        for p in &planes {
            let c = pos(&cs, p.c) + cs.len() as u32 * pos(&ls, p.l);
            let page = c + size_c * (pos(&zs, p.z) + size_z * pos(&ts, p.t));
            pages[page as usize].get_or_insert_with(|| p.stream.clone());
        }

        // Size and pixel type from the first plane.
        let first = planes.iter().min_by_key(|p| (p.t, p.z, p.l, p.c)).unwrap();
        let mut probe = Oib { cfb, info: Info::new(1, 1, DType::U8, ""), dtype: DType::U8, width: 0, height: 0, pages };
        let (tf, _) = probe.plane_tiff(&first.stream)?;
        let ifd = &tf.ifds[0];
        let width = ifd.uint(&tf, tag::IMAGE_WIDTH).context("OIB 图像缺少宽度")? as u32;
        let height = ifd.uint(&tf, tag::IMAGE_LENGTH).context("OIB 图像缺少高度")? as u32;
        let dtype = tiff_dtype(&tf, ifd)?;
        if ifd.uint(&tf, tag::SAMPLES_PER_PIXEL).unwrap_or(1) != 1 {
            bail!("暂不支持彩色（RGB）OIB 图像");
        }
        probe.width = width;
        probe.height = height;
        probe.dtype = dtype;

        let mut info = Info::new(width, height, dtype, "Olympus FluoView OIB");
        info.size_c = size_c;
        info.size_z = size_z;
        info.size_t = size_t;
        info.page_count = probe.pages.len() as u32;
        let channel_names: Vec<(u32, u32)> =
            (0..ls.len()).flat_map(|l| cs.iter().map(move |&c| (c, l as u32))).collect();
        metadata(&probe.cfb, &names, &main, &planes, &channel_names, ls.len() > 1, &mut info);
        probe.info = info;
        Ok(probe)
    }

    pub fn info(&mut self) -> Info {
        std::mem::replace(&mut self.info, Info::new(1, 1, DType::U8, ""))
    }

    /// Parses the TIFF of a plane stream; also returns where its bytes are.
    fn plane_tiff(&self, stream: &str) -> Result<(tiff::Tiff, Vec<Piece>)> {
        let e = self.cfb.stream(stream).context("OIB 文件损坏（找不到图像流）")?;
        let pieces = self.cfb.pieces(e)?;
        let size = e.size;
        let mut read = |o: u64, l: usize| -> Result<Vec<u8>> {
            if o.checked_add(l as u64).is_none_or(|end| end > size) {
                bail!("OIB 图像流被截断");
            }
            cfb::read_pieces(&pieces, o, l)
        };
        let t = tiff::parse(&mut read, size, 1)?;
        Ok((t, pieces))
    }

    pub fn page(&self, page: u32) -> Result<Recipe> {
        let mut r = Recipe::new();
        let Some(stream) = self.pages.get(page as usize).context("页码超出范围")? else { return Ok(r) };
        let (tf, pieces) = self.plane_tiff(stream)?;
        let ifd = &tf.ifds[0];
        let w = ifd.uint(&tf, tag::IMAGE_WIDTH).unwrap_or(0) as u32;
        let h = ifd.uint(&tf, tag::IMAGE_LENGTH).unwrap_or(0) as u32;
        if (w, h) != (self.width, self.height) || tiff_dtype(&tf, ifd)? != self.dtype {
            bail!("OIB 里各平面的尺寸或像素类型不一致");
        }
        if ifd.uint(&tf, tag::SAMPLES_PER_PIXEL).unwrap_or(1) != 1 || ifd.get(tag::TILE_OFFSETS).is_some() {
            bail!("暂不支持这种 OIB 平面布局");
        }
        let codec = match ifd.uint(&tf, tag::COMPRESSION).unwrap_or(1) {
            1 => Codec::Raw,
            5 => Codec::Lzw,
            8 | 32946 => Codec::Zlib,
            32773 => Codec::PackBits,
            c => bail!("不支持的 OIB 压缩方式 {c}"),
        };
        let mut fl = if tf.little { 0 } else { flags::BIG_ENDIAN };
        if ifd.uint(&tf, tag::PREDICTOR) == Some(2) {
            fl |= flags::PREDICTOR;
        }
        let offs = ifd.uints(&tf, tag::STRIP_OFFSETS);
        let counts = ifd.uints(&tf, tag::STRIP_BYTE_COUNTS);
        let rps = (ifd.uint(&tf, tag::ROWS_PER_STRIP).unwrap_or(h as u64).min(h as u64).max(1)) as u32;
        if offs.is_empty() || offs.len() != counts.len() {
            bail!("OIB 图像缺少像素位置");
        }
        let rb = w as u64 * self.dtype.size() as u64;
        let size: u64 = pieces.last().map_or(0, |p| p.at + p.len);
        for (i, (&o, &n)) in offs.iter().zip(&counts).enumerate() {
            let y = i as u32 * rps;
            if y >= h {
                break;
            }
            let rows = rps.min(h - y);
            let n = if codec == Codec::Raw { n.min(rb * rows as u64) } else { n };
            if o.checked_add(n).is_none_or(|e| e > size) {
                bail!("OIB 图像流被截断");
            }
            let sub = cfb::sub_pieces(&pieces, o, n);
            if codec == Codec::Raw {
                raw_rows(&sub, rb, rows, w, y, fl, &mut r)?;
            } else if sub.len() == 1 {
                r.push(Chunk::file(sub[0].file, n, [0, y, w, rows]).codec(codec).flags(fl));
            } else {
                // A compressed strip split over sectors: hand it over whole.
                let bytes = cfb::read_pieces(&pieces, o, n as usize)?;
                r.inline(&bytes, codec, [0, y, w, rows]).flags |= fl;
            }
        }
        merge_chunks(&mut r);
        Ok(r)
    }
}

fn tiff_dtype(tf: &tiff::Tiff, ifd: &tiff::Ifd) -> Result<DType> {
    let bits = ifd.uint(tf, tag::BITS_PER_SAMPLE).unwrap_or(1);
    let fmt = ifd.uint(tf, tag::SAMPLE_FORMAT).unwrap_or(1);
    Ok(match (bits, fmt) {
        (8, 2) => DType::I8,
        (8, _) => DType::U8,
        (16, 2) => DType::I16,
        (16, _) => DType::U16,
        (32, 3) => DType::F32,
        (32, 2) => DType::I32,
        (32, _) => DType::U32,
        _ => bail!("不支持的 OIB 像素位数 {bits}"),
    })
}

// ---------------------------------------------------------------- metadata

/// Reads a small settings stream by its original name (`s_LUT1.lut`).
fn settings(cfb: &Cfb, names: &BTreeMap<String, String>, name: &str, max: usize) -> Option<Ini> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let e = cfb.stream(names.get(base)?)?;
    cfb.read_stream(e, max).ok().map(|b| Ini::parse(&b))
}

/// Channel colour from a LUT file: the primaries with a non-zero contrast.
fn lut_color(lut: &Ini) -> Option<[u8; 3]> {
    let on = |s: &str| lut.f(s, "Contrast").is_some_and(|v| v > 0.0);
    let c = [on("Red"), on("Green"), on("Blue")].map(|b| if b { 255 } else { 0 });
    (c != [0, 0, 0]).then_some(c)
}

/// Step of an axis in µm (or ms for T): from its start/end positions, else its interval.
fn axis_step(main: &Ini, code: &str) -> Option<(f64, String)> {
    for i in 0..16 {
        let s = format!("Axis {i} Parameters Common");
        if main.get(&s, "AxisCode") != Some(code) {
            continue;
        }
        let n = main.f(&s, "MaxSize").unwrap_or(0.0);
        let unit = main.s(&s, "PixUnit").unwrap_or_default();
        let span = main.f(&s, "EndPosition").zip(main.f(&s, "StartPosition")).map(|(e, b)| (e - b).abs());
        let step = match span {
            Some(d) if n > 1.0 && d > 0.0 => d / (n - 1.0),
            _ => main.f(&s, "Interval").unwrap_or(0.0).abs(),
        };
        return (step > 0.0).then_some((step, unit));
    }
    None
}

fn to_um(v: f64, unit: &str) -> Option<f64> {
    match unit {
        "um" | "µm" | "micron" => Some(v),
        "nm" => Some(v / 1000.0),
        "mm" => Some(v * 1000.0),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn metadata(
    cfb: &Cfb,
    names: &BTreeMap<String, String>,
    main: &Ini,
    planes: &[Plane],
    channels: &[(u32, u32)],
    lambda: bool,
    info: &mut Info,
) {
    const ACQ: &str = "Acquisition Parameters Common";
    const REF: &str = "Reference Image Parameter";

    // Pixel size.
    let xy = |k: &str, u: &str, axis: &str| {
        main.f(REF, k).and_then(|v| to_um(v, main.get(REF, u).unwrap_or("um"))).filter(|v| *v > 0.0).or_else(|| {
            axis_step(main, axis).and_then(|(v, u)| to_um(v, &u))
        })
    };
    let px = xy("WidthConvertValue", "WidthUnit", "X");
    let py = xy("HeightConvertValue", "HeightUnit", "Y").or(px);
    let pz = if info.size_z > 1 { axis_step(main, "Z").and_then(|(v, u)| to_um(v, &u)) } else { None };
    if let Some(x) = px {
        info.pixel_size = Some([x, py.unwrap_or(x), pz.unwrap_or(0.0)]);
        info.unit = "µm".into();
    }

    let system = main.s("Version Info", "SystemName");
    let version = main.s("Version Info", "SystemVersion");
    if let Some(s) = &system {
        info.format = format!("Olympus FluoView OIB ({}{})", s.trim_start_matches("FLUOVIEW ").trim(), version.as_deref().map(|v| format!(" {v}")).unwrap_or_default());
    }

    // Settings of the first plane of each channel (.pty next to the .tif).
    let pty = |c: u32| -> Option<Ini> {
        let p = planes.iter().filter(|p| p.c == c).min_by_key(|p| (p.t, p.z, p.l))?;
        settings(cfb, names, &p.name.replace(".tif", ".pty").replace(".TIF", ".pty"), 1 << 20)
    };
    let first_c = channels.first().map_or(1, |c| c.0);
    let pty0 = pty(first_c);

    // Channels: names, dyes and LUT colours. Channel N of the main file is
    // the Nth C index present.
    let mut cs: Vec<u32> = channels.iter().map(|c| c.0).collect();
    rank(&mut cs);
    for (i, &(c, l)) in channels.iter().enumerate() {
        let k = cs.binary_search(&c).unwrap_or(0) + 1;
        let sec = format!("Channel {k} Parameters");
        let base = main.s(&sec, "DyeName").filter(|d| d != "None").or_else(|| main.s(&sec, "CH Name")).unwrap_or_else(|| format!("C{k}"));
        let name = if lambda { format!("{base} λ{}", l + 1) } else { base };
        let lut = main
            .s("ProfileSaveInfo", &format!("LutFileName{}", k - 1))
            .and_then(|f| settings(cfb, names, &f, 4096))
            .and_then(|l| lut_color(&l));
        let fallback = if channels.len() == 1 { [255, 255, 255] } else { DEFAULT_COLORS[i % 7] };
        info.channels.push(Channel { name, color: lut.unwrap_or(fallback) });
    }

    let mut out: Vec<Group> = vec![];
    let mut g = G::new("FluoView 图像");
    g.add("系统", system.map(|s| format!("{s} {}", version.clone().unwrap_or_default())));
    g.add("采集设备", main.s(ACQ, "Acquisition Device"));
    g.add("采集时间", main.s(ACQ, "ImageCaputreDate"));
    g.add("文件类型", main.s("File Info", "FileType"));
    g.add("尺寸", Some(detail(info.width, info.height, info.size_c, info.size_z, info.size_t)));
    g.add("有效位数", main.s(REF, "ValidBitCounts"));
    if let Some([x, y, z]) = info.pixel_size {
        g.add("像素尺寸 X", Some(format!("{:.4} µm", x)));
        g.add("像素尺寸 Y", Some(format!("{:.4} µm", y)));
        if z > 0.0 {
            g.add("Z 步长", Some(format!("{:.4} µm", z)));
        }
    }
    if info.size_t > 1 {
        if let Some((v, u)) = axis_step(main, "T") {
            let s = if u == "ms" { v / 1000.0 } else { v };
            g.add("时间间隔 (s)", Some(num(s)));
        }
    }
    g.add("用户", main.s("File Info", "UserName"));
    g.add("原文件名", main.s("File Info", "DataName"));
    g.push_to(&mut out);

    let mut g = G::new("FluoView 显微镜");
    if let Some(p) = &pty0 {
        g.add("物镜", p.s(ACQ, "ObjectiveLens Name").map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")));
        g.add("放大倍数", p.f(ACQ, "Magnification").map(num));
        g.add("数值孔径 NA", p.f(ACQ, "ObjectiveLens NAValue").map(num));
        g.add("工作距离 (mm)", p.f(ACQ, "ObjectiveLens WDValue").filter(|v| *v != 9999.0).map(num));
    }
    g.add("扫描缩放", main.f(ACQ, "ZoomValue").map(num));
    g.add("扫描模式", main.s(ACQ, "ScanMode"));
    g.add("扫描方向", main.s(ACQ, "ScanDirection"));
    if let Some(p) = &pty0 {
        g.add("像素驻留 (µs)", p.f(ACQ, "Time Per Pixel").map(num));
        g.add("行时间 (ms)", p.f(ACQ, "Time Per Line").map(|v| num(v / 1000.0)));
        g.add("帧时间 (s)", p.f(ACQ, "Time Per Frame").map(|v| num(v / 1e6)));
    }
    let integ = main.s(ACQ, "IntegrationType").filter(|t| t != "None");
    g.add(
        "平均（Kalman / 行）",
        integ.map(|t| match main.s(ACQ, "IntegrationCount") {
            Some(n) => format!("{t} ×{n}"),
            None => t,
        }),
    );
    g.add("顺序扫描", main.s(ACQ, "SequentialMode"));
    g.push_to(&mut out);

    // Lasers by wavelength, with the transmissivity in percent.
    let lasers: Vec<(f64, Option<f64>)> = (1..=16)
        .filter_map(|i| {
            let wl = main.f(ACQ, &format!("LaserWavelength{i:02}"))?;
            Some((wl, main.f(ACQ, &format!("LaserTransmissivity{i:02}"))))
        })
        .collect();
    for (k, &c) in cs.iter().enumerate() {
        let sec = format!("Channel {} Parameters", k + 1);
        let dye = main.s(&sec, "DyeName");
        let label = dye.clone().filter(|d| d != "None").or_else(|| main.s(&sec, "CH Name")).unwrap_or_default();
        let mut g = G::new(&format!("通道 {}：{}", k + 1, label));
        g.add("荧光染料", dye);
        g.add("探测通道", main.s(&sec, "CH Name"));
        let ex = main.f(&sec, "ExcitationWavelength");
        g.add("激发波长 (nm)", ex.map(num));
        g.add("发射波长 (nm)", main.f(&sec, "EmissionWavelength").map(num));
        if let Some(ex) = ex {
            let tr = lasers.iter().find(|l| (l.0 - ex).abs() < 0.5).and_then(|l| l.1);
            g.add("激光", Some(match tr {
                Some(t) => format!("{} nm, {}%", num(ex), num(t)),
                None => format!("{} nm", num(ex)),
            }));
        }
        g.add("发射滤光片", main.s(&sec, "BF Name"));
        g.add("二向色镜", main.s(&sec, "EmissionDM Name").or_else(|| main.s(&sec, "ExcitationDM Name")));
        let p = pty(c);
        let analog = main.f(&sec, "PMTDetectingMode").unwrap_or(0.0) == 0.0;
        g.add("检测模式", Some(if analog { "Analog" } else { "Photon counting" }.to_string()));
        let ptyv = |k: &str| p.as_ref().and_then(|p| p.f(ACQ, k));
        g.add("探测器电压 HV", ptyv("PMTVoltage").or_else(|| main.f(&sec, "AnalogPMTVoltage")).map(num));
        g.add("增益", ptyv("AnalogPMTGain").map(num));
        g.add("偏移 (%)", ptyv("AnalogPMTOffset").map(num));
        g.add("针孔 (µm)", ptyv("PinholeDiameter").map(|v| num(v / 1000.0)));
        g.add("共聚焦", main.s(&sec, "Confocal"));
        g.push_to(&mut out);
    }

    // Every setting of the main file, for search.
    let mut all = Group::new("FluoView 全部参数");
    let mut key = String::new();
    for (s, kv) in &main.sections {
        for (k, v) in kv {
            key.clear();
            key.push_str(s);
            key.push('/');
            key.push_str(k);
            all.add_str(&key, v);
        }
    }
    all.push_to(&mut out);
    info.metadata = out;
}
