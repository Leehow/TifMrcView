//! Zeiss SmartSEM plugin: acquisition settings of TIFFs saved by SmartSEM,
//! the software of Zeiss SEM / FIB-SEM microscopes (Gemini, Sigma,
//! Crossbeam…), as a TifMrcView WebAssembly metadata plugin.
//!
//! SmartSEM writes TIFF tag 34118 (CZ_SEM): a few numbers, then pairs of
//! lines, a parameter code (`AP_WD`, `DP_DETECTOR_CHANNEL`, `SV_VERSION`…)
//! followed by its display text (`WD =  2.7 mm`, `Date: 22 Oct 2026`), in
//! Latin-1. The core opens the image; this plugin adds the settings, the
//! calibrated pixel size and the format name.

use tifmrcview_plugin_sdk::{export_metadata, file, Group, Meta, Metadata, Result};

const CZ_SEM_TAG: u16 = 34118;
const IMAGE_WIDTH_TAG: u16 = 256;

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

/// The first IFD of a classic TIFF: (little endian, entries).
struct Ifd {
    le: bool,
    entries: Vec<u8>,
}

impl Ifd {
    fn read() -> Option<Ifd> {
        let h = file::read(0, 8).ok()?;
        let le = match &h[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let me = Ifd { le, entries: vec![] };
        if me.u16(&h, 2) != 42 {
            return None;
        }
        let at = me.u32(&h, 4) as u64;
        let n = me.u16(&file::read(at, 2).ok()?, 0) as usize;
        let entries = file::read(at + 2, n * 12).ok()?;
        Some(Ifd { le, entries })
    }

    fn u16(&self, b: &[u8], o: usize) -> u16 {
        let a = [b[o], b[o + 1]];
        if self.le {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        }
    }

    fn u32(&self, b: &[u8], o: usize) -> u32 {
        let a = [b[o], b[o + 1], b[o + 2], b[o + 3]];
        if self.le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    }

    /// (type, count, offset of the 4-byte value field within `entries`).
    fn find(&self, tag: u16) -> Option<(u16, u32, usize)> {
        (0..self.entries.len() / 12)
            .map(|k| k * 12)
            .find(|&e| self.u16(&self.entries, e) == tag)
            .map(|e| (self.u16(&self.entries, e + 2), self.u32(&self.entries, e + 4), e + 8))
    }

    /// The raw bytes of a tag (inline or at its offset). Used for private
    /// text tags that are not valid NUL-terminated ASCII (Zeiss writes Latin-1).
    fn bytes(&self, tag: u16) -> Option<Vec<u8>> {
        let (ty, count, at) = self.find(tag)?;
        let unit = match ty {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            _ => 8,
        };
        let size = unit * count as usize;
        if size <= 4 {
            return Some(self.entries[at..at + size].to_vec());
        }
        file::read_at_most(self.u32(&self.entries, at) as u64, size).ok()
    }

    fn unsigned(&self, tag: u16) -> Option<u32> {
        let (ty, _, at) = self.find(tag)?;
        match ty {
            3 => Some(self.u16(&self.entries, at) as u32),
            4 => Some(self.u32(&self.entries, at)),
            _ => None,
        }
    }
}

struct SmartSem;

impl Metadata for SmartSem {
    fn metadata(path: &str) -> Result<Option<Meta>> {
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if ext != "tif" && ext != "tiff" {
            return Ok(None);
        }
        let Some(ifd) = Ifd::read() else { return Ok(None) };
        let Some(tag) = ifd.bytes(CZ_SEM_TAG) else { return Ok(None) };
        let width = ifd.unsigned(IMAGE_WIDTH_TAG).unwrap_or(0) as usize;
        // Latin-1: every byte is the code point.
        let text: String = tag.iter().map(|&c| c as char).collect();
        Ok(parse_zeiss(&text, width))
    }
}

export_metadata!(SmartSem);

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

fn parse_zeiss(text: &str, width: usize) -> Option<Meta> {
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

    let mut main = Group::new("SmartSEM 主要参数");
    for code in ZEISS_MAIN {
        if let Some((_, k, v)) = get(code) {
            main.add_str(k, v);
        }
    }
    // The key settings group is shown even when empty.
    let mut groups = vec![main];
    for (prefix, name) in [("AP_", "全部数值参数 (AP)"), ("DP_", "全部状态 (DP)"), ("SV_", "系统信息 (SV)")] {
        let mut items: Vec<(&String, &String)> = entries.iter().filter(|e| e.0.starts_with(prefix)).map(|e| (&e.1, &e.2)).collect();
        items.sort_by_key(|a| a.0.to_lowercase());
        let mut g = Group::new(name);
        for (k, v) in items {
            g.push(k, v);
        }
        g.push_to(&mut groups);
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
    let pixel = pixel_nm.filter(|&p| p > 0.0);
    Some(Meta {
        format: Some("Zeiss SmartSEM TIFF".into()),
        pixel_size: pixel.map(|p| [p, p, 0.0]),
        unit: pixel.map(|_| "nm".into()),
        groups,
    })
}
