//! The TemData structure TVIPS EM-MENU writes into its TIFFs: tag 37706
//! holds its file offset (tag 37708 a copy of it). Version 1 is a short
//! block of integers; version 2 follows it with UTF-16 names, the image,
//! microscope and camera settings and a magic number. Field layout as
//! documented in tifffile (BSD licence, from the EM-MENU help file).
//!
//! The core opens the image; this adds the settings, the pixel size and
//! the format name.

use crate::fmt;
use tifmrcview_plugin_sdk::{file, tiff, Group, Meta, Metadata, Result};

const TEM_DATA: u16 = 37706;
const TEM_DATA_COPY: u16 = 37708;
const V1_SIZE: usize = 244;
const V2_SIZE: usize = 6312;
const MAGIC: u32 = 0xAAAA_AAAA;

pub struct EmMenu;

struct Rd<'a> {
    b: &'a [u8],
    le: bool,
}

impl Rd<'_> {
    fn u32(&self, o: usize) -> u32 {
        let a = [self.b[o], self.b[o + 1], self.b[o + 2], self.b[o + 3]];
        if self.le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    }
    fn i32(&self, o: usize) -> i32 {
        self.u32(o) as i32
    }
    fn f32(&self, o: usize) -> f64 {
        f32::from_bits(self.u32(o)) as f64
    }
    /// A UTF-16 string of `len` bytes.
    fn text(&self, o: usize, len: usize) -> String {
        let units: Vec<u16> = self.b[o..o + len]
            .chunks_exact(2)
            .map(|c| {
                if self.le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16_lossy(&units).trim().to_string()
    }
}

/// EM-MENU dates: year << 16 | month << 8 | day.
fn date(v: i32) -> Option<String> {
    let (y, m, d) = (v >> 16, (v >> 8) & 0xff, v & 0xff);
    ((1990..2200).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d))
        .then(|| format!("{y:04}-{m:02}-{d:02}"))
}

fn time(v: i32) -> Option<String> {
    (0..86400)
        .contains(&v)
        .then(|| format!("{:02}:{:02}:{:02}", v / 3600, v / 60 % 60, v % 60))
}

impl Metadata for EmMenu {
    fn metadata(path: &str) -> Result<Option<Meta>> {
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if ext != "tif" && ext != "tiff" {
            return Ok(None);
        }
        let Ok(t) = tiff::parse(&mut tiff::file_reader, file::size(), 1) else {
            return Ok(None);
        };
        let ifd = &t.ifds[0];
        let data = match ifd.uint(&t, TEM_DATA) {
            Some(at) if at > 0 => file::read_at_most(at, V2_SIZE)?,
            _ => match ifd.get(TEM_DATA_COPY) {
                Some(e) => e.value.clone(),
                None => return Ok(None),
            },
        };
        Ok(parse(&data, t.little))
    }
}

fn parse(b: &[u8], le: bool) -> Option<Meta> {
    if b.len() < V1_SIZE {
        return None;
    }
    let r = Rd { b, le };
    let version = r.i32(0);
    if !(1..=2).contains(&version) {
        return None;
    }
    let mut g = Group::new("TVIPS EM-MENU");
    let mut pixel = None;
    if version == 2 && b.len() >= V2_SIZE && r.u32(V2_SIZE - 4) == MAGIC {
        g.add_str("图像名称", &r.text(244, 160));
        g.add("日期", date(r.i32(584)));
        g.add("时间", time(r.i32(588)));
        g.add_str("尺寸", &format!("{} × {}", r.i32(564), r.i32(568)));
        g.add_str("显微镜", &r.text(3112, 160));
        let ht = r.f32(3272);
        if ht > 0.0 {
            g.add_str("加速电压", &format!("{} kV", fmt(ht / 1000.0)));
        }
        let mag = r.f32(3536);
        if mag > 0.0 {
            g.add_str("放大倍数", &format!("{}×", fmt(mag)));
        }
        let post = r.f32(3544);
        if post > 0.0 {
            g.add_str("后放大倍数", &format!("{}×", fmt(post)));
        }
        let corr = r.f32(3540);
        if corr > 0.0 {
            g.add_str("放大倍数校正", &fmt(corr));
        }
        g.add_str("模式", if r.i32(3532) == 1 { "衍射" } else { "成像" });
        let st: Vec<String> = (0..5).map(|k| fmt(r.f32(3552 + 4 * k))).collect();
        g.add_str("样品台 X / Y / Z / α / β（原始值）", &st.join(" / "));
        g.add_str(
            "束斑 / 亮度",
            &format!("{} / {}", fmt(r.f32(3624)), fmt(r.f32(3628))),
        );
        g.add_str("相机", &r.text(3768, 160));
        g.add_str("像素合并", &format!("{} × {}", r.i32(3944), r.i32(3948)));
        g.add_str("相机偏移", &format!("{}, {}", r.i32(3936), r.i32(3940)));
        let phys = r.f32(3928);
        if phys > 0.0 {
            g.add_str("物理像素尺寸", &format!("{} µm", fmt(phys / 1000.0)));
        }
        g.add_str("曝光时间", &format!("{} ms", fmt(r.f32(3952))));
        g.add_str("增益", &fmt(r.f32(3956)));
        g.add_str("读出速率", &format!("{} kHz", fmt(r.f32(3960))));
        g.add_str("平场校正", &r.text(3964, 160));
        let dose = r.f32(4128);
        if dose > 0.0 {
            g.add_str("剂量", &fmt(dose));
        }
        g.add_str("备注", &r.text(592, 1024));
        g.add_str("处理历史", &r.text(1616, 1024));
        g.add_str("FEI 显微镜信息", &r.text(4260, 1024));
        g.add_str("FEI 样品信息", &r.text(5284, 1024));
        let (px, py) = (r.f32(2968), r.f32(2972));
        if px > 0.0 && px.is_finite() {
            g.add_str("像素尺寸", &format!("{} nm", fmt(px)));
            let tidy = |v: f64| fmt(v).parse().unwrap_or(v);
            pixel = Some([tidy(px), tidy(if py > 0.0 { py } else { px }), 0.0]);
        }
    } else {
        // Version 1: integers, many of them scaled by 1000.
        g.add_str(
            "加速电压",
            &format!("{} kV", fmt(r.i32(84) as f64 / 1000.0)),
        );
        g.add_str("放大倍数", &format!("{}×", r.i32(96)));
        g.add_str("曝光时间", &format!("{} ms", r.i32(196)));
        g.add_str("像素合并", &r.i32(180).to_string());
        g.add("日期", date(r.i32(224)));
        g.add("时间", time(r.i32(228)));
    }
    let camera = if version == 2 && b.len() >= V2_SIZE {
        r.text(3768, 160)
    } else {
        String::new()
    };
    Some(Meta {
        format: Some(if camera.is_empty() {
            "TVIPS EM-MENU TIFF".into()
        } else {
            format!("TVIPS EM-MENU TIFF ({camera})")
        }),
        pixel_size: pixel,
        unit: pixel.map(|_| "nm".into()),
        groups: vec![g],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times() {
        assert_eq!(date(132187148).as_deref(), Some("2017-04-12"));
        assert_eq!(time(32477).as_deref(), Some("09:01:17"));
        assert!(date(0).is_none());
    }

    #[test]
    fn version_2_fields() {
        let mut b = vec![0u8; V2_SIZE];
        let put = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 0, 2);
        put(&mut b, 2968, 1.4208f32.to_bits());
        put(&mut b, 2972, 1.4208f32.to_bits());
        put(&mut b, 3272, 99000f32.to_bits());
        put(&mut b, 3536, 32000f32.to_bits());
        for (k, c) in "F416".encode_utf16().enumerate() {
            b[3768 + 2 * k..3770 + 2 * k].copy_from_slice(&c.to_le_bytes());
        }
        put(&mut b, V2_SIZE - 4, MAGIC);
        let m = parse(&b, true).unwrap();
        assert_eq!(m.format.as_deref(), Some("TVIPS EM-MENU TIFF (F416)"));
        assert_eq!(m.pixel_size, Some([1.4208, 1.4208, 0.0]));
        let j = m.to_json();
        assert!(j.contains("99 kV") && j.contains("32000×"), "{j}");
    }
}
