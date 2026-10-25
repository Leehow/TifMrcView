//! Native half of the Zeiss SmartSEM plugin (`ui/plugins/smartsem.js`).
//!
//! SmartSEM, the software of Zeiss SEM / FIB-SEM microscopes, writes TIFF
//! tag 34118 (CZ_SEM): a few numbers, then pairs of lines, a parameter code
//! (`AP_WD`, `DP_DETECTOR_CHANNEL`, `SV_VERSION`…) followed by its display
//! text (`WD =  2.7 mm`, `Date: 22 Oct 2026`), in Latin-1.

use std::fs::File;
use std::path::Path;

use anyhow::{bail, Result};
use memmap2::Mmap;
use serde_json::{json, Value};

use super::NativePlugin;
use crate::formats::tiff::first_ifd_tag;
use crate::image::MetaGroup;

const CZ_SEM_TAG: u16 = 34118;

pub struct SmartSem;

impl NativePlugin for SmartSem {
    fn id(&self) -> &'static str {
        "smartsem"
    }

    fn call(&self, method: &str, args: Value, _body: &[u8]) -> Result<Value> {
        match method {
            // {path, width} -> null, or {groups, pixelNm, version}
            "read" => {
                let path = args["path"].as_str().unwrap_or_default();
                let width = args["width"].as_u64().unwrap_or(0) as usize;
                Ok(read(Path::new(path), width)?.map_or(Value::Null, |z| {
                    json!({ "groups": z.groups, "pixelNm": z.pixel_nm, "version": z.version })
                }))
            }
            _ => bail!("unknown method {method}"),
        }
    }
}

fn read(path: &Path, width: usize) -> Result<Option<Zeiss>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext != "tif" && ext != "tiff" {
        return Ok(None);
    }
    let file = File::open(path)?;
    // SAFETY: read-only map, see formats::tiff.
    let mmap = unsafe { Mmap::map(&file)? };
    Ok(first_ifd_tag(&mmap, CZ_SEM_TAG)
        .map(|b| b.iter().map(|&c| c as char).collect::<String>())
        .and_then(|t| parse_zeiss(&t, width)))
}

/// Settings shown first, in this order, by parameter code.
const ZEISS_MAIN: &[&str] = &[
    "AP_DATE",
    "AP_TIME",
    "DP_SEM",
    "SV_SERIAL_NUMBER",
    "AP_ACTUALKV",
    "AP_WD",
    "AP_MAG",
    "AP_IMAGE_PIXEL_SIZE",
    "AP_WIDTH",
    "AP_HEIGHT",
    "DP_IMAGE_STORE",
    "DP_DETECTOR_CHANNEL",
    "DP_IMPLIED_DETECTOR",
    "DP_MIXING",
    "AP_IPROBE",
    "AP_APERTURESIZE",
    "AP_SCM",
    "DP_SCANRATE",
    "DP_DWELL_TIME",
    "AP_LINE_TIME",
    "AP_FRAME_TIME",
    "DP_NOISE_REDUCTION",
    "AP_LINE_AVERAGE_COUNT",
    "AP_FRAME_AVERAGE_COUNT",
    "AP_BRIGHTNESSZ0",
    "AP_CONTRASTZ0",
    "AP_SCANROTATION",
    "AP_STIG_X",
    "AP_STIG_Y",
    "AP_TILT_ANGLE",
    "AP_STAGE_AT_X",
    "AP_STAGE_AT_Y",
    "AP_STAGE_AT_Z",
    "AP_STAGE_AT_T",
    "AP_STAGE_AT_R",
    "AP_SYSTEM_VAC",
    "AP_COLUMN_VAC",
    "AP_CHAMBER_PRESSURE",
    "AP_CRYO_STAGE_ACTUAL_T",
    "SV_USER_NAME",
    "SV_OPERATOR",
    "SV_USER_TEXT",
    "SV_VERSION",
];

struct Zeiss {
    groups: Vec<MetaGroup>,
    /// Pixel size in nm.
    pixel_nm: Option<f64>,
    /// SmartSEM version, e.g. "V07.04.00.00 : 10-May-23".
    version: Option<String>,
}

fn is_code(l: &str) -> bool {
    l.len() > 3
        && matches!(&l[..3], "AP_" | "DP_" | "SV_")
        && l.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// Splits `Label = value` or `Label: value`.
fn split_entry(l: &str) -> (String, String) {
    let (k, v) = l.split_once(" = ").or_else(|| l.split_once(" =")).or_else(|| l.split_once(':')).unwrap_or((l, ""));
    let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
    (k.trim().to_string(), v)
}

/// Parses "15.61 nm" style lengths into nm.
fn length_nm(v: &str) -> Option<f64> {
    let mut it = v.split_whitespace();
    let n: f64 = it.next()?.parse().ok()?;
    let f = match it.next()? {
        "pm" => 1e-3,
        "Å" => 0.1,
        "nm" => 1.0,
        "µm" | "μm" | "um" => 1e3,
        "mm" => 1e6,
        _ => return None,
    };
    Some(n * f).filter(|x| *x > 0.0)
}

fn parse_zeiss(text: &str, width: usize) -> Option<Zeiss> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches(['\r', '\0', ' '])).collect();
    // code -> (label, value), in file order
    let mut entries: Vec<(&str, String, String)> = vec![];
    let mut i = 0;
    while i < lines.len() {
        if is_code(lines[i]) && i + 1 < lines.len() && !is_code(lines[i + 1]) {
            let (k, v) = split_entry(lines[i + 1]);
            entries.push((lines[i], k, v));
            i += 2;
        } else {
            i += 1;
        }
    }
    if entries.len() < 5 {
        return None;
    }
    let get = |code: &str| entries.iter().find(|e| e.0 == code);

    let mut main = vec![];
    for code in ZEISS_MAIN {
        if let Some((_, k, v)) = get(code) {
            if !v.is_empty() {
                main.push([k.clone(), v.clone()]);
            }
        }
    }
    let mut groups = vec![MetaGroup { name: "SmartSEM 主要参数".into(), items: main }];
    for (prefix, name) in [("AP_", "全部数值参数 (AP)"), ("DP_", "全部状态 (DP)"), ("SV_", "系统信息 (SV)")] {
        let mut items: Vec<[String; 2]> =
            entries.iter().filter(|e| e.0.starts_with(prefix)).map(|e| [e.1.clone(), e.2.clone()]).collect();
        items.sort_by_key(|a| a[0].to_lowercase());
        if !items.is_empty() {
            groups.push(MetaGroup { name: name.into(), items });
        }
    }

    // "Image Pixel Size" has four digits. The header's fourth number is the
    // same calibration in metres, at full precision, for a 1024-pixel-wide
    // store; use it when it agrees.
    let nominal = get("AP_IMAGE_PIXEL_SIZE").and_then(|e| length_nm(&e.2));
    let precise = lines
        .get(3)
        .and_then(|l| l.trim().parse::<f64>().ok())
        .map(|m| m * 1e9 * 1024.0 / width.max(1) as f64);
    let pixel_nm = match (nominal, precise) {
        (Some(n), Some(p)) if (p - n).abs() / n < 0.005 => Some(p),
        (Some(n), _) => Some(n),
        (None, _) => get("AP_WIDTH").and_then(|e| length_nm(&e.2)).map(|w| w / width.max(1) as f64),
    };
    let version = get("SV_VERSION").map(|e| e.2.clone()).filter(|v| !v.is_empty());
    Some(Zeiss { groups, pixel_nm, version })
}
