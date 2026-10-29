//! TIA series files (`.ser`), written by FEI / Thermo Fisher TIA (TEM
//! Imaging & Analysis) on Tecnai, Titan and Talos microscopes.
//!
//! Layout, from the public description of the format: a header (byte
//! order 0x4949, series id 0x0197, version 0x0210 with 32-bit or 0x0220
//! with 64-bit offsets, data type 0x4120 for spectra or 0x4122 for images,
//! tag type, element counts, offset of the offset arrays), the series
//! dimensions (size, calibration, description, units), then two arrays
//! with the file offset of every element and of its tag. A 2D element is
//! its X and Y calibration, a pixel type, its size and the pixels; a tag
//! holds the acquisition time (and, for scans, the beam position).
//!
//! TIA stores image rows bottom-up, so each page is built from one raw
//! chunk per row, flipped. The microscope settings live in the `.emi`
//! file next to the `.ser`, which a plugin cannot open; opening the `.emi`
//! itself says which file to open instead.

use crate::{fmt, length, unix_time};
use tifmrcview_plugin_sdk::{
    bail, file, le_u16, le_u32, le_u64, Chunk, DType, Group, Info, Recipe, Result,
};

const DATA_1D: u32 = 0x4120;
const DATA_2D: u32 = 0x4122;
const TAG_TIME: u16 = 0x4152;
const TAG_TIME_POSITION: u16 = 0x4142;

fn f64_at(b: &[u8], o: usize) -> f64 {
    f64::from_bits(le_u64(b, o))
}

fn pixel_type(t: u16) -> Result<DType> {
    Ok(match t {
        1 => DType::U8,
        2 => DType::U16,
        3 => DType::U32,
        4 => DType::I8,
        5 => DType::I16,
        6 => DType::I32,
        7 => DType::F32,
        8 => DType::F64,
        9 | 10 => bail!("不支持复数像素的 TIA 文件"),
        t => bail!("TIA 文件的像素类型不对（{t}）"),
    })
}

fn pixel_type_name(t: u16) -> &'static str {
    [
        "",
        "8 位无符号整数",
        "16 位无符号整数",
        "32 位无符号整数",
        "8 位有符号整数",
        "16 位有符号整数",
        "32 位有符号整数",
        "32 位浮点",
        "64 位浮点",
    ]
    .get(t as usize)
    .copied()
    .unwrap_or("")
}

/// One axis of the series (e.g. the scan positions of a STEM map).
struct Dim {
    size: u32,
    offset: f64,
    delta: f64,
    description: String,
    units: String,
}

pub struct Ser {
    version: u16,
    width: u32,
    height: u32,
    pixel: u16,
    dtype: DType,
    /// File offset of each element's pixels.
    data: Vec<u64>,
    /// (offset, delta) of X and Y.
    cal: [(f64, f64); 2],
    dims: Vec<Dim>,
    tag_type: u16,
    first_tag: Option<(u32, Option<(f64, f64)>)>,
    total: u32,
}

fn text(b: &[u8]) -> String {
    b.iter()
        .map(|&c| c as char)
        .collect::<String>()
        .trim_end_matches('\0')
        .trim()
        .to_string()
}

impl Ser {
    pub fn open(path: &str) -> Result<Ser> {
        if path
            .rsplit('.')
            .next()
            .is_some_and(|e| e.eq_ignore_ascii_case("emi"))
        {
            bail!("TIA 的 .emi 文件只保存显示设置和显微镜参数，图像在旁边的 .ser 文件里（例如「名称_1.ser」），请打开那个文件");
        }
        let h = file::read(0, 34)?;
        if le_u16(&h, 0) != 0x4949 || le_u16(&h, 2) != 0x0197 {
            bail!("不是 TIA SER 文件");
        }
        let version = le_u16(&h, 4);
        let wide = version >= 0x0220;
        let data_type = le_u32(&h, 6);
        let tag_type = le_u32(&h, 10) as u16;
        let total = le_u32(&h, 14);
        let valid = le_u32(&h, 18);
        let (arrays, ndim, mut at) = if wide {
            (le_u64(&h, 22), le_u32(&h, 30), 34u64)
        } else {
            (le_u32(&h, 22) as u64, le_u32(&h, 26), 30)
        };
        if data_type == DATA_1D {
            bail!("这个 TIA 文件保存的是谱图（EDS / EELS 谱或谱图像），不是图像，暂不支持");
        }
        if data_type != DATA_2D {
            bail!("不认识的 TIA 数据类型 0x{data_type:04x}");
        }
        if valid == 0 {
            bail!("这个 TIA 文件里没有图像（采集被中断？）");
        }
        if ndim > 16 || total < valid {
            bail!("TIA 文件头损坏");
        }
        let mut dims = Vec::new();
        for _ in 0..ndim {
            let b = file::read(at, 28)?;
            let dl = le_u32(&b, 24) as usize;
            let description = text(&file::read(at + 28, dl.min(4096))?);
            at += 28 + dl as u64;
            let ul = le_u32(&file::read(at, 4)?, 0) as usize;
            let units = text(&file::read(at + 4, ul.min(4096))?);
            at += 4 + ul as u64;
            dims.push(Dim {
                size: le_u32(&b, 0),
                offset: f64_at(&b, 4),
                delta: f64_at(&b, 12),
                description,
                units,
            });
        }
        let w = if wide { 8 } else { 4 };
        let raw = file::read(arrays, 2 * total as usize * w)?;
        let off = |i: usize| {
            if wide {
                le_u64(&raw, i * 8)
            } else {
                le_u32(&raw, i * 4) as u64
            }
        };
        let data: Vec<u64> = (0..valid as usize).map(off).collect();
        let tag_at = off(total as usize);
        let e = file::read(data[0], 50)?;
        let pixel = le_u16(&e, 40);
        let dtype = pixel_type(pixel)?;
        let (width, height) = (le_u32(&e, 42), le_u32(&e, 46));
        if width == 0 || height == 0 {
            bail!("TIA 图像尺寸不对（{width} × {height}）");
        }
        let cal = [
            (f64_at(&e, 0), f64_at(&e, 8)),
            (f64_at(&e, 20), f64_at(&e, 28)),
        ];
        let first_tag = file::read(tag_at, 24)
            .ok()
            .filter(|t| le_u16(t, 0) == tag_type)
            .map(|t| {
                let pos = (tag_type == TAG_TIME_POSITION).then(|| (f64_at(&t, 8), f64_at(&t, 16)));
                (le_u32(&t, 4), pos)
            });
        Ok(Ser {
            version,
            width,
            height,
            pixel,
            dtype,
            data,
            cal,
            dims,
            tag_type,
            first_tag,
            total,
        })
    }

    pub fn info(&mut self) -> Result<Info> {
        let mut info = Info::new(self.width, self.height, self.dtype, "TIA SER");
        info.size_t = self.data.len() as u32;
        info.page_count = info.size_t;
        let (dx, dy) = (self.cal[0].1.abs(), self.cal[1].1.abs());
        // Real-space images are calibrated in metres; diffraction patterns in 1/m.
        let real = dx > 0.0 && dx < 1e-2;
        if real {
            let (x, unit) = length(dx);
            let f = x / dx;
            info.pixel_size = Some([x, if dy > 0.0 { dy * f } else { x }, 0.0]);
            info.unit = unit.into();
        } else if dx > 0.0 {
            let (x, unit) = crate::reciprocal(dx);
            let y = if dy > 0.0 { crate::reciprocal(dy).0 } else { x };
            info.pixel_size = Some([x, y, 0.0]);
            info.unit = unit.into();
        }
        let mut g = Group::new("TIA 图像");
        g.add_str(
            "文件格式",
            &format!("TIA SER（版本 0x{:04x}）", self.version),
        );
        g.add_str("尺寸", &format!("{} × {}", self.width, self.height));
        g.add_str("像素类型", pixel_type_name(self.pixel));
        g.add_str(
            "图像数",
            &format!("{}（共预留 {}）", self.data.len(), self.total),
        );
        let axis = |(o, d): (f64, f64)| {
            if real {
                let (v, u) = length(d.abs());
                format!("{} {u}/像素", fmt(v))
            } else if d != 0.0 {
                let (v, u) = length(1.0 / d.abs());
                format!("{} 1/m（1/{} {u}），原点 {}", fmt(d), fmt(v), fmt(o))
            } else {
                String::new()
            }
        };
        g.add("X 标定", Some(axis(self.cal[0])).filter(|s| !s.is_empty()));
        g.add("Y 标定", Some(axis(self.cal[1])).filter(|s| !s.is_empty()));
        if !real && dx > 0.0 {
            g.add_str("标定类型", "倒空间（衍射）");
        }
        for (k, d) in self.dims.iter().enumerate() {
            let mut v = format!("{} 个", d.size);
            if d.delta != 0.0 && d.delta != 1.0 || !d.units.is_empty() {
                v.push_str(&format!(
                    "，步长 {} {}，起点 {}",
                    fmt(d.delta),
                    d.units,
                    fmt(d.offset)
                ));
            }
            let name = if d.description.is_empty() {
                format!("序列维度 {}", k + 1)
            } else {
                d.description.clone()
            };
            g.add_str(&format!("序列维度 {}：{name}", k + 1), &v);
        }
        if let Some((t, pos)) = self.first_tag {
            g.add("采集时间（第 1 张）", unix_time(t as i64));
            if let Some((x, y)) = pos {
                let (vx, u) = length(x.abs().max(y.abs()).max(f64::MIN_POSITIVE));
                let f = vx / x.abs().max(y.abs()).max(f64::MIN_POSITIVE);
                g.add_str(
                    "束斑位置（第 1 张）",
                    &format!("{}, {} {u}", fmt(x * f), fmt(y * f)),
                );
            }
        }
        g.add_str(
            "标签类型",
            if self.tag_type == TAG_TIME {
                "时间"
            } else {
                "时间 + 位置"
            },
        );
        g.add_str(
            "说明",
            "显微镜参数保存在同名的 .emi 文件里，插件只能读取打开的这个文件",
        );
        g.push_to(&mut info.metadata);
        Ok(info)
    }

    pub fn page(&self, page: u32) -> Result<Recipe> {
        let Some(&at) = self.data.get(page as usize) else {
            bail!("没有第 {} 张图像", page + 1)
        };
        let row = self.width as u64 * self.dtype.size() as u64;
        let start = at + 50;
        let mut r = Recipe::new();
        // Rows are stored bottom-up.
        for y in 0..self.height {
            let src = start + (self.height - 1 - y) as u64 * row;
            r.push(Chunk::file(src, row, [0, y, self.width, 1]));
        }
        Ok(r)
    }
}
