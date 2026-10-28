//! Gatan plugin: Gatan DigitalMicrograph / GMS files (.dm3 and .dm4) as a
//! TifMrcView WebAssembly format plugin.
//!
//! Both versions are one tag tree ([`tags`]); DM4 only widens the counts
//! and sizes to 64 bits. The images are the entries of `ImageList`: each
//! has `ImageData` (`Data`, the pixels as one uncompressed array;
//! `DataType`; `Dimensions`, X first; `Calibrations`) and `ImageTags` (the
//! acquisition settings: microscope, camera, data bar…). DigitalMicrograph
//! puts a small RGB preview first, which is skipped; a file with several
//! real images gets the series picker.
//!
//! The pixels are contiguous, X fastest, so a page (one plane of a 3D or 4D
//! data set) is one raw chunk the app reads in place.

mod tags;

use tags::{path, Node, Tree};
use tifmrcview_plugin_sdk::{
    bail, export_format, flags, Channel, Chunk, DType, Format, Group, Info, Recipe, Result, Series,
};

/// How the pixels of one image are stored.
#[derive(Clone, Copy)]
enum Pixels {
    Gray(DType),
    /// Real and imaginary parts, shown as two channels.
    Complex(DType),
    /// 4 bytes per pixel: blue, green, red, unused.
    Bgra,
}

impl Pixels {
    fn from_dm(t: u32) -> Option<Pixels> {
        Some(match t {
            1 => Pixels::Gray(DType::I16),
            2 => Pixels::Gray(DType::F32),
            3 => Pixels::Complex(DType::F32),
            6 | 14 => Pixels::Gray(DType::U8),
            7 => Pixels::Gray(DType::I32),
            8 | 20..=23 => Pixels::Bgra,
            9 => Pixels::Gray(DType::I8),
            10 => Pixels::Gray(DType::U16),
            11 => Pixels::Gray(DType::U32),
            12 => Pixels::Gray(DType::F64),
            13 => Pixels::Complex(DType::F64),
            _ => return None,
        })
    }
    fn bytes(self) -> u64 {
        match self {
            Pixels::Gray(d) => d.size() as u64,
            Pixels::Complex(d) => 2 * d.size() as u64,
            Pixels::Bgra => 4,
        }
    }
}

fn dm_type_name(t: u32) -> String {
    let s = match t {
        1 => "16 位有符号整数",
        2 => "32 位浮点",
        3 => "复数（2 × 32 位浮点）",
        5 | 27 => "FFT 压缩复数（packed complex）",
        6 => "8 位无符号整数",
        7 => "32 位有符号整数",
        8 => "RGB",
        9 => "8 位有符号整数",
        10 => "16 位无符号整数",
        11 => "32 位无符号整数",
        12 => "64 位浮点",
        13 => "复数（2 × 64 位浮点）",
        14 => "二值",
        20..=23 => "RGBA（8 位）",
        _ => return format!("类型 {t}"),
    };
    format!("{s}（DataType {t}）")
}

/// One image of `ImageList`, checked and ready to read.
struct Image {
    /// Position in `ImageList`.
    index: usize,
    name: String,
    dm_type: u32,
    pixels: Pixels,
    /// X, Y, then any further dimensions.
    dims: Vec<u64>,
    offset: u64,
}

impl Image {
    fn width(&self) -> u64 {
        self.dims[0]
    }
    fn height(&self) -> u64 {
        self.dims.get(1).copied().unwrap_or(1)
    }
    fn size_z(&self) -> u64 {
        self.dims.get(2).copied().unwrap_or(1)
    }
    fn size_t(&self) -> u64 {
        self.dims.iter().skip(3).product()
    }
    fn page_bytes(&self) -> u64 {
        self.width() * self.height() * self.pixels.bytes()
    }
    fn detail(&self) -> String {
        let d: Vec<String> = self.dims.iter().map(|v| v.to_string()).collect();
        let kind = match self.pixels {
            Pixels::Gray(t) => t.name(),
            Pixels::Complex(_) => "complex",
            Pixels::Bgra => "RGB",
        };
        format!("{} · {kind}", d.join("×"))
    }
}

type Dir = [(String, Node)];

fn images(tree: &Tree) -> Result<Vec<Image>> {
    let le = tree.little_endian;
    let Some(list) = tags::get(&tree.root, "ImageList").and_then(Node::dir) else {
        bail!("这个 DigitalMicrograph 文件里没有图像（缺少 ImageList）");
    };
    let mut out = Vec::new();
    let mut first_err = None;
    // Images with pixel data, including the ones we cannot read.
    let mut with_data = 0;
    for (index, (_, node)) in list.iter().enumerate() {
        let Some(img) = node.dir() else { continue };
        match image(img, index, le) {
            Ok(Some(i)) => {
                with_data += 1;
                out.push(i)
            }
            Ok(None) => {}
            Err(e) => {
                with_data += 1;
                first_err.get_or_insert(e);
            }
        }
    }
    // The preview DigitalMicrograph stores in front of the real image; when
    // the real image cannot be read, say why rather than show the preview.
    if with_data > 1 && out.first().is_some_and(|i| matches!(i.pixels, Pixels::Bgra) && i.index == 0) {
        out.remove(0);
    }
    if out.is_empty() {
        return Err(first_err
            .unwrap_or_else(|| tifmrcview_plugin_sdk::Error("这个 DigitalMicrograph 文件里没有图像数据".into())));
    }
    Ok(out)
}

fn image(img: &Dir, index: usize, le: bool) -> Result<Option<Image>> {
    let Some(data) = path(img, &["ImageData", "Data"]).and_then(Node::data) else { return Ok(None) };
    let Some(dm_type) = path(img, &["ImageData", "DataType"]).and_then(|n| n.num(le)) else { return Ok(None) };
    let dm_type = dm_type as u32;
    let Some(pixels) = Pixels::from_dm(dm_type) else {
        bail!("不支持这种像素类型：{}", dm_type_name(dm_type));
    };
    let dims: Vec<u64> = path(img, &["ImageData", "Dimensions"])
        .and_then(Node::dir)
        .map(|d| d.iter().filter_map(|(_, n)| n.num(le)).map(|v| v as u64).collect())
        .unwrap_or_default();
    if dims.is_empty() || dims.contains(&0) {
        return Ok(None);
    }
    if dims[0] > u32::MAX as u64 || dims.get(1).is_some_and(|&h| h > u32::MAX as u64) {
        bail!("图像太大（{} 像素宽）", dims[0]);
    }
    let name = path(img, &["Name"]).and_then(|n| n.text(le)).unwrap_or_default();
    let i = Image { index, name, dm_type, pixels, dims, offset: data.offset };
    let pages = i.size_z().checked_mul(i.size_t()).filter(|&p| p <= u32::MAX as u64);
    let need = pages.and_then(|p| p.checked_mul(i.page_bytes()));
    match need {
        Some(n) if n <= data.len => Ok(Some(i)),
        Some(n) => bail!("图像数据不完整：需要 {n} 字节，文件里只有 {} 字节", data.len),
        None => bail!("图像尺寸不对：{}", i.detail()),
    }
}

// ------------------------------------------------------------- calibration

/// Length units the viewer knows: (factor to nm, unit shown).
fn length_unit(u: &str) -> Option<(f64, &'static str)> {
    Some(match u.trim() {
        "pm" => (1e-3, "Å"),
        "Å" | "A" | "Angstrom" | "Angstroms" => (0.1, "Å"),
        "nm" => (1.0, "nm"),
        "µm" | "μm" | "um" | "micron" | "microns" => (1e3, "µm"),
        "mm" => (1e6, "mm"),
        "m" => (1e9, "mm"),
        _ => return None,
    })
}

fn unit_to_nm(u: &str) -> f64 {
    match u {
        "Å" => 0.1,
        "nm" => 1.0,
        "µm" => 1e3,
        _ => 1e6,
    }
}

/// Rounds away the noise of a value stored as f32.
fn tidy(v: f64) -> f64 {
    format!("{}", v as f32).parse().unwrap_or(v)
}

fn fmt(v: f64) -> String {
    format!("{}", tidy(v))
}

struct Cal {
    scale: f64,
    origin: f64,
    units: String,
}

fn calibrations(img: &Dir, le: bool) -> Vec<Option<Cal>> {
    let Some(dims) = path(img, &["ImageData", "Calibrations", "Dimension"]).and_then(Node::dir) else {
        return vec![];
    };
    dims.iter()
        .map(|(_, n)| {
            let d = n.dir()?;
            Some(Cal {
                scale: tags::get(d, "Scale").and_then(|n| n.num(le))?,
                origin: tags::get(d, "Origin").and_then(|n| n.num(le)).unwrap_or(0.0),
                units: tags::get(d, "Units").and_then(|n| n.text(le)).unwrap_or_default(),
            })
        })
        .collect()
}

/// Pixel size (x, y, z) and its unit, when X is calibrated in a length.
fn pixel_size(cals: &[Option<Cal>]) -> Option<([f64; 3], &'static str)> {
    let cx = cals.first()?.as_ref()?;
    let (fx, unit) = length_unit(&cx.units)?;
    if cx.scale <= 0.0 {
        return None;
    }
    let per = unit_to_nm(unit);
    let axis = |i: usize| -> f64 {
        cals.get(i)
            .and_then(|c| c.as_ref())
            .and_then(|c| length_unit(&c.units).map(|(f, _)| c.scale.abs() * f / per))
            .unwrap_or(0.0)
    };
    let x = cx.scale * fx / per;
    let y = match axis(1) {
        0.0 => x,
        y => y,
    };
    Some(([tidy(x), tidy(y), tidy(axis(2))], unit))
}

// ------------------------------------------------------------- metadata

const AXES: [&str; 4] = ["X", "Y", "Z", "第 4 维"];

fn image_group(tree: &Tree, img: &Dir, i: &Image, cals: &[Option<Cal>]) -> Group {
    let le = tree.little_endian;
    let s = |p: &[&str]| path(img, p).and_then(|n| n.data()).map(|d| d.display(le)).filter(|v| !v.is_empty());
    let mut g = Group::new("Gatan 图像");
    g.add_str("文件格式", &format!("DigitalMicrograph DM{}", tree.version));
    g.add_str("名称", &i.name);
    g.add_str("尺寸", &i.dims.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" × "));
    g.add_str("像素类型", &dm_type_name(i.dm_type));
    if let Pixels::Complex(_) = i.pixels {
        g.add_str("显示", "通道 1 为实部，通道 2 为虚部");
    }
    g.add_str("字节序", if le { "小端" } else { "大端" });
    for (k, c) in cals.iter().enumerate().take(i.dims.len()) {
        let Some(c) = c else { continue };
        let axis = AXES.get(k).copied().unwrap_or("其他维度");
        let unit = if c.units.is_empty() { "像素".into() } else { c.units.clone() };
        let mut v = format!("{} {unit}/像素", fmt(c.scale));
        if c.origin != 0.0 {
            v.push_str(&format!("（原点 {}）", fmt(c.origin)));
        }
        g.add_str(&format!("{axis} 标定"), &v);
    }
    let bright = path(img, &["ImageData", "Calibrations", "Brightness"]).and_then(Node::dir);
    if let Some(b) = bright {
        let scale = tags::get(b, "Scale").and_then(|n| n.num(le)).unwrap_or(1.0);
        let origin = tags::get(b, "Origin").and_then(|n| n.num(le)).unwrap_or(0.0);
        let units = tags::get(b, "Units").and_then(|n| n.text(le)).unwrap_or_default();
        if scale != 1.0 || origin != 0.0 || !units.is_empty() {
            g.add_str("强度标定", format!("× {} + {} {units}", fmt(scale), fmt(origin)).trim());
        }
    }
    g.add("采集日期", s(&["ImageTags", "DataBar", "Acquisition Date"]));
    g.add("采集时间", s(&["ImageTags", "DataBar", "Acquisition Time"]));
    g.add("GMS 版本", s(&["ImageTags", "GMS Version", "Created"]));
    g
}

/// A tag for the summary groups: numbers rounded to what was stored.
fn show(n: &Node, le: bool) -> Option<String> {
    match n.num(le) {
        Some(v) => Some(fmt(v)),
        None => n.data().map(|d| d.display(le)).filter(|v| !v.is_empty()),
    }
}

fn microscope_group(img: &Dir, le: bool) -> Group {
    let mi = |k: &str| path(img, &["ImageTags", "Microscope Info", k]);
    // Unset numbers are stored as 0.
    let s = |k: &str| mi(k).and_then(|n| show(n, le)).filter(|v| v != "0");
    let mut g = Group::new("Gatan 显微镜");
    g.add("显微镜", s("Microscope").or_else(|| s("Name")));
    if let Some(v) = mi("Voltage").and_then(|n| n.num(le)).filter(|&v| v > 0.0) {
        g.add_str("加速电压", &format!("{} kV", fmt(v / 1000.0)));
    }
    g.add("操作模式", s("Operation Mode"));
    g.add("照明模式", s("Illumination Mode"));
    g.add("成像模式", s("Imaging Mode"));
    g.add("标称放大倍数", s("Indicated Magnification").map(|v| format!("{v}×")));
    g.add("实际放大倍数", s("Actual Magnification").map(|v| format!("{v}×")));
    g.add("相机长度", s("STEM Camera Length").map(|v| format!("{v} mm")));
    g.add("视野", s("Field of View (µm)").map(|v| format!("{v} µm")));
    g.add("球差 Cs", s("Cs(mm)").map(|v| format!("{v} mm")));
    g.add("发射电流", s("Emission Current (µA)").map(|v| format!("{v} µA")));
    g.add("束斑", s("Spot Size").or_else(|| s("Spot size")));
    g.add("探针尺寸", s("Probe Size (nm)").map(|v| format!("{v} nm")));
    g.add("探针电流", s("Probe Current (nA)").or_else(|| s("SEM Probe Current (nA)")).map(|v| format!("{v} nA")));
    g.add("工作距离", s("WD (mm)").map(|v| format!("{v} mm")));
    if let Some(st) = mi("Stage Position").and_then(Node::dir) {
        for (k, label) in [
            ("Stage X", "样品台 X"),
            ("Stage Y", "样品台 Y"),
            ("Stage Z", "样品台 Z"),
            ("Stage Alpha", "样品台 α"),
            ("Stage Beta", "样品台 β"),
        ] {
            g.add(label, tags::get(st, k).and_then(|n| n.num(le)).map(fmt));
        }
    }
    g.add("操作者", s("Operator"));
    g.add("样品", s("Specimen"));
    g
}

fn acquisition_group(img: &Dir, le: bool) -> Group {
    let s = |p: &[&str]| path(img, p).and_then(|n| show(n, le));
    let mut g = Group::new("Gatan 采集");
    g.add(
        "相机",
        s(&["ImageTags", "Acquisition", "Device", "Name"]).or_else(|| s(&["ImageTags", "DataBar", "Device Name"])),
    );
    g.add("信号", s(&["ImageTags", "DataBar", "Signal Name"]));
    g.add("相机型号", s(&["ImageTags", "Acquisition", "Device", "Source"]));
    g.add("有效区域", s(&["ImageTags", "Acquisition", "Device", "Active Size (pixels)"]));
    let hl = |k: &str| s(&["ImageTags", "Acquisition", "Parameters", "High Level", k]);
    g.add(
        "曝光时间",
        hl("Exposure (s)").or_else(|| s(&["ImageTags", "DataBar", "Exposure Time (s)"])).map(|v| format!("{v} s")),
    );
    g.add("像素合并", hl("Binning").or_else(|| s(&["ImageTags", "DataBar", "Binning"])));
    g.add("处理方式", hl("Processing"));
    g.add("采集模式", hl("Acquisition Mode"));
    g
}

/// Every tag under `ImageTags`, flattened to "A/B/C".
fn all_group(img: &Dir, le: bool) -> Group {
    fn walk(dir: &Dir, prefix: &str, le: bool, g: &mut Group) {
        for (k, (label, node)) in dir.iter().enumerate() {
            if g.len() >= 50_000 {
                return;
            }
            let name = if label.is_empty() { format!("[{k}]") } else { label.clone() };
            let full = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            match node {
                Node::Dir(d) => walk(d, &full, le, g),
                Node::Data(d) => g.push(&full, &d.display(le)),
            }
        }
    }
    let mut g = Group::new("Gatan 全部参数");
    if let Some(t) = path(img, &["ImageTags"]).and_then(Node::dir) {
        walk(t, "", le, &mut g);
    }
    g
}

// ------------------------------------------------------------- the format

pub struct Gatan {
    tree: Tree,
    images: Vec<Image>,
    current: usize,
}

impl Gatan {
    fn img(&self) -> &Image {
        &self.images[self.current]
    }
    fn dir(&self) -> &Dir {
        let list = tags::get(&self.tree.root, "ImageList").and_then(Node::dir).unwrap();
        list[self.img().index].1.dir().unwrap()
    }
}

impl Format for Gatan {
    fn open(path: &str) -> Result<Gatan> {
        Gatan::open_series(path, 0)
    }

    fn open_series(_path: &str, series: u32) -> Result<Gatan> {
        let tree = tags::parse()?;
        let images = images(&tree)?;
        let current = series as usize;
        if current >= images.len() {
            bail!("这个文件只有 {} 个图像（请求第 {} 个）", images.len(), current + 1);
        }
        Ok(Gatan { tree, images, current })
    }

    fn info(&mut self) -> Result<Info> {
        let le = self.tree.little_endian;
        let i = self.img();
        let dir = self.dir();
        let (w, h) = (i.width() as u32, i.height() as u32);
        let gms = path(dir, &["ImageTags", "GMS Version", "Created"]).and_then(|n| n.text(le));
        let format = match gms {
            Some(v) => format!("Gatan DM{} (GMS {v})", self.tree.version),
            None => format!("Gatan DM{}", self.tree.version),
        };
        let dtype = match i.pixels {
            Pixels::Gray(d) | Pixels::Complex(d) => d,
            Pixels::Bgra => DType::U8,
        };
        let mut info = Info::new(w, h, dtype, &format);
        info.size_z = i.size_z() as u32;
        info.size_t = i.size_t() as u32;
        match i.pixels {
            Pixels::Gray(_) => {}
            Pixels::Complex(_) => {
                info.size_c = 2;
                info.samples_per_page = 2;
                info.channels = vec![
                    Channel { name: "实部".into(), color: [255, 255, 255] },
                    Channel { name: "虚部".into(), color: [0, 255, 255] },
                ];
            }
            Pixels::Bgra => {
                info.size_c = 3;
                info.samples_per_page = 3;
                info.is_rgb = true;
            }
        }
        info.page_count = info.size_z * info.size_t;
        if info.size_c == 1 {
            let name = if i.name.is_empty() { "Gatan".into() } else { i.name.clone() };
            info.channels = vec![Channel { name, color: [255, 255, 255] }];
        }
        let cals = calibrations(dir, le);
        if let Some((ps, unit)) = pixel_size(&cals) {
            info.pixel_size = Some(ps);
            info.unit = unit.into();
        }
        image_group(&self.tree, dir, i, &cals).push_to(&mut info.metadata);
        microscope_group(dir, le).push_to(&mut info.metadata);
        acquisition_group(dir, le).push_to(&mut info.metadata);
        all_group(dir, le).push_to(&mut info.metadata);
        if self.images.len() > 1 {
            info.series = self
                .images
                .iter()
                .enumerate()
                .map(|(k, im)| Series {
                    name: if im.name.is_empty() { format!("图像 {}", k + 1) } else { im.name.clone() },
                    detail: im.detail(),
                })
                .collect();
            info.series_index = self.current as u32;
        }
        Ok(info)
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        let i = self.img();
        let bytes = i.page_bytes();
        let (w, h) = (i.width() as u32, i.height() as u32);
        let mut c = Chunk::file(i.offset + page as u64 * bytes, bytes, [0, 0, w, h]);
        c = match i.pixels {
            Pixels::Gray(_) => c,
            Pixels::Complex(_) => c.samples(2),
            Pixels::Bgra => c.samples(4).flags(flags::BGR),
        };
        if !self.tree.little_endian {
            c = c.flags(flags::BIG_ENDIAN);
        }
        let mut r = Recipe::new();
        r.push(c);
        Ok(r)
    }
}

export_format!(Gatan);
