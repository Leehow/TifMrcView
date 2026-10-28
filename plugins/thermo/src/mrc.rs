//! The FEI extended header of MRC files written by Thermo Fisher software
//! (EPU, Tomography, Velox, Maps): `EXTTYP` "FEI1" or "FEI2" in the MRC
//! header, then one metadata block per image with the microscope, high
//! tension, dose, stage, pixel size, optics and camera settings. Two
//! bitmasks say which fields are valid. Layout from Thermo Fisher's public
//! "MRC2014 file format, FEI extended header" description.
//!
//! The core opens the MRC; this adds the settings and the pixel size.

use crate::{fmt, length, show_length};
use tifmrcview_plugin_sdk::{file, le_i32, le_u32, le_u64, Group, Meta, Result};

fn f64_at(b: &[u8], o: usize) -> f64 {
    f64::from_bits(le_u64(b, o))
}

fn text(b: &[u8], o: usize) -> Option<String> {
    let s = &b[o..o + 16];
    let end = s.iter().position(|&c| c == 0).unwrap_or(16);
    let t: String = s[..end].iter().map(|&c| c as char).collect();
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// Days since 1899-12-30 (an OLE date) as "YYYY-MM-DD HH:MM:SS".
fn ole_date(d: f64) -> Option<String> {
    if !(1.0..200_000.0).contains(&d) {
        return None;
    }
    let secs = ((d - 25569.0) * 86400.0).round() as i64;
    crate::unix_time(secs).map(|s| s.trim_end_matches(" UTC").to_string())
}

fn degrees(v: f64) -> String {
    format!("{}°", fmt((v * 100.0).round() / 100.0))
}

pub fn metadata() -> Result<Option<Meta>> {
    if file::size() < 1024 {
        return Ok(None);
    }
    let h = file::read(0, 1024)?;
    let ext = &h[104..108];
    let nsymbt = le_i32(&h, 92);
    if !(ext == b"FEI1" || ext == b"FEI2") || nsymbt < 300 {
        return Ok(None);
    }
    let nz = le_i32(&h, 8).max(1) as usize;
    let first = file::read_at_most(1024, 1024.min(nsymbt as usize))?;
    let size = le_i32(&first, 0);
    if !(300..=1 << 20).contains(&size) || first.len() < 475 {
        return Ok(None);
    }
    let size = size as usize;
    let b = &first;
    let m1 = le_u32(b, 8);
    let m2 = le_u32(b, 297);
    let has1 = |bit: u32| m1 & (1 << bit) != 0;
    let has2 = |bit: u32| m2 & (1 << bit) != 0;
    let d = |o: usize| f64_at(b, o);

    let mut g = Group::new("Thermo Fisher MRC");
    g.add_str(
        "扩展头",
        &format!(
            "{}，版本 {}，每帧 {size} 字节",
            String::from_utf8_lossy(ext),
            le_i32(b, 4)
        ),
    );
    if has1(0) {
        g.add("时间", ole_date(d(12)));
    }
    if has1(1) {
        g.add("显微镜", text(b, 20));
    }
    if has1(2) {
        g.add("仪器编号", text(b, 36));
    }
    if has1(3) {
        let app = [text(b, 52), if has1(4) { text(b, 68) } else { None }]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        g.add("软件", Some(app).filter(|a| !a.is_empty()));
    }
    if has1(5) {
        g.add_str("加速电压", &format!("{} kV", fmt(d(84) / 1000.0)));
    }
    if has1(6) && d(92) > 0.0 {
        // e/m² → e/Å².
        g.add_str("剂量", &format!("{} e/Å²", fmt(d(92) * 1e-20)));
    }
    if has1(7) {
        g.add_str("α 倾转", &degrees(d(100)));
    }
    if has1(8) {
        g.add_str("β 倾转", &degrees(d(108)));
    }
    for (bit, o, label) in [
        (9, 116, "样品台 X"),
        (10, 124, "样品台 Y"),
        (11, 132, "样品台 Z"),
    ] {
        if has1(bit) {
            g.add_str(label, &show_length(d(o)));
        }
    }
    if has1(12) {
        g.add_str("倾转轴角度", &degrees(d(140)));
    }
    let (px, py) = (d(156), d(164));
    let real = has1(14) && px > 0.0 && px < 1e-3;
    if has1(14) && px > 0.0 {
        if real {
            g.add_str("像素尺寸", &show_length(px));
        } else {
            g.add_str("像素尺寸（倒空间）", &format!("{} 1/nm", fmt(px * 1e-9)));
        }
    }
    if has1(22) {
        g.add_str("散焦", &show_length(d(220)));
    }
    if has1(24) && d(236) != 0.0 {
        g.add_str("设定散焦", &show_length(d(236)));
    }
    if has1(25) {
        g.add_str("模式", if le_i32(b, 244) == 1 { "TEM" } else { "STEM" });
    }
    if has1(26) {
        g.add_str(
            "投影模式",
            if le_i32(b, 248) == 1 {
                "成像"
            } else {
                "衍射"
            },
        );
    }
    if has1(27) {
        g.add("物镜模式", text(b, 252));
    }
    if has1(28) {
        g.add("高倍模式", text(b, 268));
    }
    if has1(29) {
        g.add_str(
            "探针模式",
            if le_i32(b, 284) == 1 {
                "纳米探针"
            } else {
                "微探针"
            },
        );
    }
    if has1(30) {
        g.add_str("能量过滤 (EFTEM)", if b[288] != 0 { "开" } else { "关" });
    }
    if has1(31) && d(289) > 0.0 {
        g.add_str("放大倍数", &format!("{}×", fmt(d(289).round())));
    }
    if has2(0) && d(301) > 0.0 {
        g.add_str("相机长度", &format!("{} mm", fmt(d(301) * 1000.0)));
    }
    if has2(1) {
        g.add_str("束斑", &le_i32(b, 309).to_string());
    }
    if has2(2) && d(313) > 0.0 {
        g.add_str("照明区域", &show_length(d(313)));
    }
    if has2(4) && d(329) > 0.0 {
        g.add_str("会聚半角", &format!("{} mrad", fmt(d(329) * 1000.0)));
    }
    if has2(5) {
        g.add("照明模式", text(b, 337));
    }
    if has2(16) && d(419) > 0.0 {
        g.add_str("积分时间", &format!("{} s", fmt(d(419))));
    }
    if has2(17) {
        g.add_str(
            "像素合并",
            &format!("{} × {}", le_i32(b, 427), le_i32(b, 431)),
        );
    }
    if has2(19) {
        g.add("相机", text(b, 435));
    }
    // The tilt range of a tilt series.
    if has1(7) && nz > 1 && (nsymbt as usize) >= nz * size {
        let n = nz.min(4096);
        if let Ok(all) = file::read(1024, n * size) {
            let tilts: Vec<f64> = (0..n)
                .map(|k| f64_at(&all, k * size + 100))
                .filter(|t| t.is_finite())
                .collect();
            let lo = tilts.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = tilts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if hi > lo {
                g.add_str(
                    "倾转范围",
                    &format!("{} 到 {}（{} 张）", degrees(lo), degrees(hi), n),
                );
            }
        }
    }
    let pixel = real.then(|| {
        let (x, unit) = length(px);
        (
            [x, crate::tidy(if py > 0.0 { py * x / px } else { x }), 0.0],
            unit,
        )
    });
    Ok(Some(Meta {
        format: None,
        pixel_size: pixel.map(|p| p.0),
        unit: pixel.map(|p| p.1.into()),
        groups: vec![g],
    }))
}
