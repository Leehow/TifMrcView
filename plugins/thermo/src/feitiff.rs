//! Settings of TIFFs saved by Thermo Fisher / FEI SEM and DualBeam (FIB)
//! microscopes (xT software: Helios, Quanta, Scios, Apreo, Verios, Nova…)
//! and by some TEM software.
//!
//! The settings are INI text in TIFF tag 34682 (FEI_HELIOS) or 34680
//! (FEI_SFEG): `[Section]` lines followed by `Key=Value` lines, values in
//! SI units (metres, volts, amperes, seconds). The core opens the image;
//! this adds the settings, the calibrated pixel size and the format name.

use crate::{fmt, length, show_length};
use tifmrcview_plugin_sdk::{file, tiff, Group, Meta, Result};

const FEI_SFEG: u16 = 34680;
const FEI_HELIOS: u16 = 34682;

type Ini = Vec<(String, Vec<(String, String)>)>;

fn parse_ini(text: &str) -> Ini {
    let mut out: Ini = Vec::new();
    for line in text.lines() {
        let l = line.trim_matches(|c: char| c.is_whitespace() || c == '\0');
        if l.is_empty() {
            continue;
        }
        if l.starts_with('[') && l.ends_with(']') {
            out.push((l[1..l.len() - 1].trim().to_string(), vec![]));
        } else if let Some((k, v)) = l.split_once('=') {
            if out.is_empty() {
                out.push((String::new(), vec![]));
            }
            out.last_mut()
                .unwrap()
                .1
                .push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

fn get<'a>(ini: &'a Ini, section: &str, key: &str) -> Option<&'a str> {
    ini.iter()
        .find(|(s, _)| s == section)
        .and_then(|(_, kv)| kv.iter().find(|(k, _)| k == key))
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

fn num(ini: &Ini, section: &str, key: &str) -> Option<f64> {
    get(ini, section, key)
        .and_then(|v| v.parse().ok())
        .filter(|v: &f64| v.is_finite())
}

pub fn metadata() -> Result<Option<Meta>> {
    let size = file::size();
    let Ok(t) = tiff::parse(&mut tiff::file_reader, size, 1) else {
        return Ok(None);
    };
    let ifd = &t.ifds[0];
    let Some(e) = ifd.get(FEI_HELIOS).or_else(|| ifd.get(FEI_SFEG)) else {
        return Ok(None);
    };
    let end = e
        .value
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(e.value.len());
    // Latin-1: every byte is the code point.
    let text: String = e.value[..end].iter().map(|&c| c as char).collect();
    Ok(build(&parse_ini(&text)))
}

fn build(ini: &Ini) -> Option<Meta> {
    if ini.iter().map(|(_, kv)| kv.len()).sum::<usize>() < 3 {
        return None;
    }
    // The beam that took the image names its own section (EBeam, IBeam…).
    let beam = get(ini, "Beam", "Beam").unwrap_or("EBeam").to_string();
    let b = |k: &str| num(ini, &beam, k).or_else(|| num(ini, "Beam", k));
    let mut g = Group::new("Thermo Fisher 主要参数");
    let date = [get(ini, "User", "Date"), get(ini, "User", "Time")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    g.add("日期时间", Some(date).filter(|d| !d.is_empty()));
    g.add(
        "显微镜",
        get(ini, "System", "SystemType").map(|s| s.trim_matches('"').to_string()),
    );
    g.add("系统类型", get(ini, "System", "Type").map(str::to_string));
    g.add(
        "软件版本",
        get(ini, "System", "Software").map(str::to_string),
    );
    g.add("用户", get(ini, "User", "User").map(str::to_string));
    g.add("备注", get(ini, "User", "UserText").map(str::to_string));
    g.add("束流类型", Some(beam.clone()));
    g.add(
        "加速电压",
        b("HV")
            .filter(|&v| v != 0.0)
            .map(|v| format!("{} kV", fmt(v / 1000.0))),
    );
    g.add(
        "束流",
        b("BeamCurrent").filter(|&v| v > 0.0).map(|v| amps(v)),
    );
    g.add(
        "束斑",
        get(ini, &beam, "Spot")
            .or_else(|| get(ini, "Beam", "Spot"))
            .map(str::to_string),
    );
    g.add(
        "工作距离",
        b("WD")
            .or_else(|| num(ini, "Stage", "WorkingDistance"))
            .filter(|&v| v > 0.0)
            .map(show_length),
    );
    g.add(
        "水平视场 (HFW)",
        b("HFW")
            .or_else(|| num(ini, "Scan", "HorFieldsize"))
            .filter(|&v| v > 0.0)
            .map(show_length),
    );
    g.add(
        "驻留时间",
        num(ini, "Scan", "Dwelltime")
            .or_else(|| num(ini, "EScan", "Dwell"))
            .map(|v| format!("{} µs", fmt(v * 1e6))),
    );
    g.add(
        "帧时间",
        num(ini, "Scan", "FrameTime").map(|v| format!("{} s", fmt(v))),
    );
    g.add(
        "像素尺寸",
        num(ini, "Scan", "PixelWidth")
            .filter(|&v| v > 0.0)
            .map(show_length),
    );
    let det = [get(ini, "Detectors", "Name"), get(ini, "Detectors", "Mode")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    g.add("探测器", Some(det).filter(|d| !d.is_empty()));
    g.add(
        "扫描旋转",
        b("ScanRotation")
            .filter(|&v| v != 0.0)
            .map(|v| format!("{}°", fmt((v.to_degrees() * 100.0).round() / 100.0))),
    );
    for (k, label) in [
        ("StageX", "样品台 X"),
        ("StageY", "样品台 Y"),
        ("StageZ", "样品台 Z"),
    ] {
        g.add(label, num(ini, "Stage", k).map(show_length));
    }
    for (k, label) in [("StageR", "样品台旋转"), ("StageT", "样品台倾转")] {
        g.add(
            label,
            num(ini, "Stage", k)
                .map(|v| format!("{}°", fmt((v.to_degrees() * 100.0).round() / 100.0))),
        );
    }
    g.add(
        "真空模式",
        get(ini, "Vacuum", "UserMode").map(str::to_string),
    );
    g.add(
        "样品室压力",
        num(ini, "Vacuum", "ChPressure")
            .filter(|&v| v > 0.0)
            .map(|v| format!("{} Pa", fmt(v))),
    );
    let mut groups = vec![g];
    for (section, kv) in ini {
        if kv.is_empty() || section.starts_with("Private") {
            continue;
        }
        let mut s = Group::new(&format!("Thermo Fisher [{section}]"));
        for (k, v) in kv {
            s.push(k, v);
        }
        s.push_to(&mut groups);
    }
    let px = num(ini, "Scan", "PixelWidth").filter(|&v| v > 0.0 && v < 1.0);
    let pixel = px.map(|px| {
        let (x, unit) = length(px);
        let py = num(ini, "Scan", "PixelHeight")
            .filter(|&v| v > 0.0)
            .unwrap_or(px);
        ([x, crate::tidy(py * x / px), 0.0], unit)
    });
    let model = get(ini, "System", "SystemType").map(|s| s.trim_matches('"').to_string());
    Some(Meta {
        format: Some(match model {
            Some(m) => format!("Thermo Fisher TIFF ({m})"),
            None => "Thermo Fisher TIFF".into(),
        }),
        pixel_size: pixel.map(|p| p.0),
        unit: pixel.map(|p| p.1.into()),
        groups,
    })
}

fn amps(a: f64) -> String {
    let (f, u) = if a < 1e-9 {
        (1e12, "pA")
    } else if a < 1e-6 {
        (1e9, "nA")
    } else {
        (1e6, "µA")
    };
    format!("{} {u}", fmt((a * f * 1000.0).round() / 1000.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helios_ini() {
        let text = "[User]\r\nDate=06/13/2016\r\nTime=05:06:40 PM\r\nUser=supervisor\r\n[System]\r\nSystemType=Helios NanoLab\" 660\r\n[Beam]\r\nHV=5000\r\nBeam=EBeam\r\n[EBeam]\r\nHV=5000\r\nBeamCurrent=6.25e-12\r\nWD=0.00403466\r\n[Scan]\r\nPixelWidth=3.3724e-06\r\nPixelHeight=3.3724e-06\r\nDwelltime=1e-05\r\n";
        let m = build(&parse_ini(text)).unwrap();
        assert_eq!(
            m.format.as_deref(),
            Some("Thermo Fisher TIFF (Helios NanoLab\" 660)")
        );
        assert_eq!(m.pixel_size, Some([3.3724, 3.3724, 0.0]));
        assert_eq!(m.unit.as_deref(), Some("µm"));
        let json = m.to_json();
        assert!(
            json.contains("5 kV") && json.contains("6.25 pA") && json.contains("4.03466 mm"),
            "{json}"
        );
        assert!(build(&parse_ini("[A]\nx=1")).is_none());
    }
}
