//! Writes small synthetic Nikon ND2 files (versions 2.x and 3.x) for tests.
//!
//! The layout follows the ND2 container: a signature chunk, data and
//! metadata chunks (magic, name length, data length, name, data), a chunk map
//! and, in the last 40 bytes, the chunk map's signature and offset. Metadata
//! is written as the binary "lite variant" (version 3) or as XML (version 2).

use std::io::Write;
use std::path::Path;

/// A metadata value.
#[derive(Clone, Debug)]
pub enum Lv {
    Bool(bool),
    I32(i32),
    U32(u32),
    U64(u64),
    F64(f64),
    Str(String),
    Bytes(Vec<u8>),
    Level(Vec<(String, Lv)>),
}

pub fn s(v: &str) -> Lv {
    Lv::Str(v.into())
}

/// A level from (name, value) pairs.
pub fn level(items: Vec<(&str, Lv)>) -> Lv {
    Lv::Level(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// A list: a level whose items have no name.
pub fn list(items: Vec<Lv>) -> Lv {
    Lv::Level(items.into_iter().map(|v| (String::new(), v)).collect())
}

fn utf16(out: &mut Vec<u8>, s: &str) {
    for u in s.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out.extend_from_slice(&[0, 0]);
}

/// One binary record.
fn lite_record(out: &mut Vec<u8>, name: &str, v: &Lv) {
    let start = out.len();
    let ty = match v {
        Lv::Bool(_) => 1,
        Lv::I32(_) => 2,
        Lv::U32(_) => 3,
        Lv::U64(_) => 5,
        Lv::F64(_) => 6,
        Lv::Str(_) => 8,
        Lv::Bytes(_) => 9,
        Lv::Level(_) => 11,
    };
    out.push(ty);
    out.push((name.encode_utf16().count() + 1) as u8);
    utf16(out, name);
    match v {
        Lv::Bool(b) => out.push(*b as u8),
        Lv::I32(i) => out.extend_from_slice(&i.to_le_bytes()),
        Lv::U32(i) => out.extend_from_slice(&i.to_le_bytes()),
        Lv::U64(i) => out.extend_from_slice(&i.to_le_bytes()),
        Lv::F64(f) => out.extend_from_slice(&f.to_le_bytes()),
        Lv::Str(s) => utf16(out, s),
        Lv::Bytes(b) => {
            out.extend_from_slice(&(b.len() as u64).to_le_bytes());
            out.extend_from_slice(b);
        }
        Lv::Level(items) => {
            out.extend_from_slice(&(items.len() as u32).to_le_bytes());
            let len_at = out.len();
            out.extend_from_slice(&0u64.to_le_bytes());
            let mut offsets = vec![];
            for (k, v) in items {
                offsets.push((out.len() - start) as u64);
                lite_record(out, k, v);
            }
            // The level's length runs from its record start to the end of the children.
            let len = (out.len() - start) as u64;
            out[len_at..len_at + 8].copy_from_slice(&len.to_le_bytes());
            for o in offsets {
                out.extend_from_slice(&o.to_le_bytes());
            }
        }
    }
}

/// A chunk's content in the binary encoding: one level called `root`.
pub fn lite(root: &str, v: &Lv) -> Vec<u8> {
    let mut out = vec![];
    lite_record(&mut out, root, v);
    out
}

/// The same, zlib-compressed behind a "compressed" record (type 76).
pub fn lite_compressed(root: &str, v: &Lv) -> Vec<u8> {
    let mut out = vec![76u8, 0];
    out.extend_from_slice(&[0; 10]);
    out.extend(zlib(&lite(root, v)));
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "&#10;")
}

fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn xml_node(out: &mut String, name: &str, v: &Lv) {
    let name = if name.is_empty() { "no_name" } else { name };
    let scalar = |ty: &str, val: String| {
        format!("<{name} runtype=\"{ty}\" value=\"{}\"/>", xml_escape(&val))
    };
    match v {
        Lv::Bool(b) => out.push_str(&scalar("bool", format!("{b}"))),
        Lv::I32(i) => out.push_str(&scalar("lx_int32", format!("{i}"))),
        Lv::U32(i) => out.push_str(&scalar("lx_uint32", format!("{i}"))),
        Lv::U64(i) => out.push_str(&scalar("lx_uint64", format!("{i}"))),
        Lv::F64(f) => out.push_str(&scalar("double", format!("{f:e}"))),
        Lv::Str(s) => out.push_str(&scalar("CLxStringW", s.clone())),
        Lv::Bytes(b) => out.push_str(&scalar("CLxByteArray", base64(b))),
        Lv::Level(items) => {
            out.push_str(&format!("<{name} runtype=\"CLxListVariant\">"));
            for (k, v) in items {
                xml_node(out, k, v);
            }
            out.push_str(&format!("</{name}>"));
        }
    }
}

/// A chunk's content as NIS-Elements 3 writes it: XML.
pub fn xml(v: &Lv) -> Vec<u8> {
    let mut out =
        String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><variant version=\"1.0\">");
    xml_node(&mut out, "", v);
    out.push_str("</variant>");
    out.into_bytes()
}

pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::fast());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn chunk(out: &mut Vec<u8>, name: &str, data: &[u8], name_pad: usize) -> u64 {
    let pos = out.len() as u64;
    let mut n = name.as_bytes().to_vec();
    n.resize(n.len() + name_pad, 0);
    out.extend_from_slice(&0x0ABE_CEDAu32.to_le_bytes());
    out.extend_from_slice(&(n.len() as u32).to_le_bytes());
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(&n);
    out.extend_from_slice(data);
    pos
}

/// Writes an ND2 container: `version` ("3.0", "2.1"), then the chunks in
/// order, then the chunk map. With `pad`, frame chunk names are padded so
/// that the frame data starts 4 KiB into the chunk, as NIS-Elements does.
pub fn container(path: &Path, version: &str, chunks: &[(String, Vec<u8>)], pad: bool) {
    let mut out = vec![];
    let mut ver = format!("Ver{version}").into_bytes();
    ver.resize(64, 0);
    chunk(&mut out, "ND2 FILE SIGNATURE CHUNK NAME01!", &ver, 0);
    let mut map = vec![];
    for (i, (name, data)) in chunks.iter().enumerate() {
        let pad = if !name.starts_with("ImageDataSeq") {
            i % 7
        } else if pad {
            4072 - name.len()
        } else {
            0
        };
        let pos = chunk(&mut out, name, data, pad);
        map.extend_from_slice(name.as_bytes());
        map.extend_from_slice(&pos.to_le_bytes());
        map.extend_from_slice(&(data.len() as u64).to_le_bytes());
    }
    map.extend_from_slice(b"ND2 CHUNK MAP SIGNATURE 0000001!");
    let map_pos = out.len() as u64;
    map.extend_from_slice(&map_pos.to_le_bytes());
    map.extend_from_slice(&0u64.to_le_bytes());
    chunk(&mut out, "ND2 FILEMAP SIGNATURE NAME 0001!", &map, 0);
    out.extend_from_slice(b"ND2 CHUNK MAP SIGNATURE 0000001!");
    out.extend_from_slice(&map_pos.to_le_bytes());
    std::fs::write(path, out).unwrap();
}

/// What a synthetic file holds.
#[derive(Clone)]
pub struct Spec {
    pub w: usize,
    pub h: usize,
    /// Components (channels) interleaved in every frame.
    pub comp: usize,
    /// 8, 16 or 32 (float).
    pub bits: u32,
    /// Extra bytes at the end of every row.
    pub row_pad: usize,
    pub compressed: bool,
    /// Experiment loops, outermost first: (eType, count). XY loops get
    /// `count` points; NE time loops get two periods, the second invalid.
    pub loops: Vec<(u32, usize)>,
    /// XY points that are switched off (left out of the image).
    pub invalid_points: Vec<usize>,
    /// (name, colour 0xBBGGRR) per channel.
    pub channels: Vec<(&'static str, u32)>,
    /// Frames actually written (fewer than the loops promise: stopped early).
    pub frames: Option<usize>,
    /// Version 2 (XML metadata) instead of 3 (binary).
    pub xml: bool,
    /// Wrap the binary metadata in a compressed record.
    pub compressed_meta: bool,
    /// Pad frame chunk names to 4 KiB (as NIS-Elements does).
    pub pad: bool,
}

impl Spec {
    pub fn new(w: usize, h: usize, comp: usize, bits: u32, loops: Vec<(u32, usize)>) -> Spec {
        let names = ["DAPI", "EGFP", "mCherry", "Cy5"];
        let colors = [0xff0000, 0x00ff00, 0x0000ff, 0xff00ff];
        Spec {
            w,
            h,
            comp,
            bits,
            row_pad: 0,
            compressed: false,
            loops,
            invalid_points: vec![],
            channels: (0..comp).map(|c| (names[c % 4], colors[c % 4])).collect(),
            frames: None,
            xml: false,
            compressed_meta: false,
            pad: true,
        }
    }

    pub fn frame_count(&self) -> usize {
        self.loops
            .iter()
            .map(|&(ty, n)| {
                if ty == 2 {
                    n - self.invalid_points.len()
                } else {
                    n
                }
            })
            .product()
    }

    pub fn row_bytes(&self) -> usize {
        self.w * self.comp * (self.bits as usize / 8) + self.row_pad
    }
}

/// Sample value of component `c` at (x, y) of frame `seq`.
pub fn value(seq: usize, c: usize, x: usize, y: usize) -> f64 {
    ((seq * 37 + c * 1000 + x * 3 + y * 11) % 60_000) as f64
}

/// The bytes of frame `seq` (rows padded, components interleaved).
pub fn frame(spec: &Spec, seq: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(spec.row_bytes() * spec.h);
    for y in 0..spec.h {
        for x in 0..spec.w {
            for c in 0..spec.comp {
                let v = value(seq, c, x, y);
                match spec.bits {
                    8 => out.push(v as u8),
                    16 => out.extend_from_slice(&(v as u16).to_le_bytes()),
                    _ => out.extend_from_slice(&(v as f32 / 4.0).to_le_bytes()),
                }
            }
        }
        out.resize(out.len() + spec.row_pad, 0xEE);
    }
    out
}

fn experiment(spec: &Spec, k: usize) -> Lv {
    let Some(&(ty, n)) = spec.loops.get(k) else {
        return level(vec![]);
    };
    let pars = match ty {
        2 => level(vec![
            ("bUseZ", Lv::Bool(false)),
            ("bRelativeXY", Lv::Bool(false)),
            (
                "Points",
                list(
                    (0..n)
                        .map(|i| {
                            level(vec![
                                ("dPosX", Lv::F64(100.0 * i as f64)),
                                ("dPosY", Lv::F64(-50.0 * i as f64)),
                                ("dPosZ", Lv::F64(0.0)),
                                ("dPosName", s(&format!("Well {}", i + 1))),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
        4 => level(vec![
            ("uiCount", Lv::U32(n as u32)),
            ("dZStep", Lv::F64(0.25)),
            ("dZLow", Lv::F64(0.0)),
            ("dZHigh", Lv::F64(0.25 * (n as f64 - 1.0))),
            ("wsZDevice", s("Ti2 ZDrive")),
        ]),
        8 => level(vec![
            (
                "pPeriod",
                list(vec![
                    level(vec![
                        ("uiCount", Lv::U32(n as u32)),
                        ("dPeriod", Lv::F64(2000.0)),
                    ]),
                    level(vec![("uiCount", Lv::U32(99)), ("dPeriod", Lv::F64(1.0))]),
                ]),
            ),
            ("pPeriodValid", Lv::Bytes(vec![1, 0])),
        ]),
        _ => level(vec![
            ("uiCount", Lv::U32(n as u32)),
            ("dPeriod", Lv::F64(500.0)),
            ("dStart", Lv::F64(0.0)),
        ]),
    };
    // NIS-Elements 3 wraps the loop parameters in a one-item list.
    let pars = if spec.xml { list(vec![pars]) } else { pars };
    let mut items = vec![("eType", Lv::U32(ty)), ("uLoopPars", pars)];
    if ty == 2 && !spec.invalid_points.is_empty() {
        items.push((
            "pItemValid",
            Lv::Bytes(
                (0..n)
                    .map(|i| !spec.invalid_points.contains(&i) as u8)
                    .collect(),
            ),
        ));
    }
    if k + 1 < spec.loops.len() {
        items.push(("ppNextLevelEx", list(vec![experiment(spec, k + 1)])));
    }
    level(items)
}

const OBJECTIVE: &str = "Plan Apo λ 60x Oil";

fn two() -> Lv {
    Lv::F64(2.0)
}

fn picture_metadata(spec: &Spec) -> Lv {
    let planes = spec
        .channels
        .iter()
        .enumerate()
        .map(|(i, &(name, color))| {
            let em = 450.0 + 70.0 * i as f64;
            let spectrum = |w: f64| {
                level(vec![
                    ("uiCount", Lv::U32(1)),
                    (
                        "pPoint",
                        level(vec![(
                            "Point0",
                            level(vec![("eType", Lv::U32(4)), ("dWavelength", Lv::F64(w))]),
                        )]),
                    ),
                ])
            };
            (
                format!("a{i}"),
                level(vec![
                    ("uiCompCount", Lv::U32(1)),
                    ("uiSampleIndex", Lv::U32(i as u32)),
                    ("uiModalityMask", Lv::U64(0x401)),
                    ("pFluorescentProbe", level(vec![("m_sName", s(""))])),
                    (
                        "pFilterPath",
                        level(vec![(
                            "m_pFilter",
                            list(vec![level(vec![
                                ("m_sName", s(&format!("{name} filter"))),
                                ("m_ExcitationSpectrum", spectrum(em - 60.0)),
                                ("m_EmissionSpectrum", spectrum(em)),
                            ])]),
                        )]),
                    ),
                    ("uiColor", Lv::U32(color)),
                    ("sDescription", s(name)),
                    ("dPinholeDiameter", Lv::F64(50.0)),
                ]),
            )
        })
        .collect();
    let samples = (0..spec.channels.len())
        .map(|i| {
            (
                format!("a{i}"),
                level(vec![
                    ("dExposureTime", Lv::F64(100.0 + i as f64)),
                    (
                        "pCameraSetting",
                        level(vec![
                            ("CameraUserName", s("Photometrics Prime 95B")),
                            (
                                "FormatQuality",
                                level(vec![(
                                    "fmtDesc",
                                    level(vec![("dBinningX", two()), ("dBinningY", two())]),
                                )]),
                            ),
                        ]),
                    ),
                    (
                        "pObjectiveSetting",
                        level(vec![
                            ("wsObjectiveName", s(OBJECTIVE)),
                            ("dObjectiveMag", Lv::F64(60.0)),
                        ]),
                    ),
                    (
                        "pDeviceSetting",
                        level(vec![
                            ("m_sMicroscopeFullName", s("Ti2 Microscope")),
                            ("m_sMicroscopePhysFullName", s("Nikon Ti2")),
                        ]),
                    ),
                ]),
            )
        })
        .collect();
    level(vec![
        ("dTimeAbsolute", Lv::F64(2_460_000.25)),
        ("dXPos", Lv::F64(12.5)),
        ("dYPos", Lv::F64(-3.0)),
        (
            "sPicturePlanes",
            level(vec![
                ("uiCount", Lv::U32(spec.channels.len() as u32)),
                ("uiCompCount", Lv::U32(spec.comp as u32)),
                ("sPlaneNew", Lv::Level(planes)),
                ("sSampleSetting", Lv::Level(samples)),
            ]),
        ),
        ("dCalibration", Lv::F64(0.108)),
        ("bCalibrated", Lv::Bool(true)),
        ("wsObjectiveName", s(OBJECTIVE)),
        ("dObjectiveMag", Lv::F64(-1.0)),
        ("dObjectiveNA", Lv::F64(1.42)),
        ("dRefractIndex1", Lv::F64(1.515)),
        ("dZoom", Lv::F64(1.0)),
        ("dPinholeRadius", Lv::F64(-1.0)),
        ("dTemperK", Lv::F64(296.15)),
    ])
}

fn text_info(spec: &Spec) -> Lv {
    let mut desc = String::from("Metadata:\r\nDimensions: test\r\n");
    for (i, (name, _)) in spec.channels.iter().enumerate() {
        let (k, power) = (i + 1, 20 + i);
        desc.push_str(&format!(
            "Plane #{k}:\r\n Name: {name}\r\n Component Count: 1\r\n"
        ));
        desc.push_str(" Camera Settings:   \r\n  Binning: 2x2\r\n");
        desc.push_str(&format!(
            " Microscope Settings:   Nikon Ti2, FilterChanger(Turret1): {k} (Quad)\r\n"
        ));
        desc.push_str("  LUN-F, MultiLaser(LUN-F):\r\n     Line:1; ExW:405; Power: 10.0; Off\r\n");
        desc.push_str(&format!(
            "     Line:2; ExW:488; Power: {power}.5; On\r\n  \r\n  Zoom: 1.00x\r\n"
        ));
    }
    desc.push_str("Z Stack Loop: 3\r\n");
    level(vec![
        ("TextInfoItem_0", s("")),
        ("TextInfoItem_4", s("Tester")),
        ("TextInfoItem_5", s(&desc)),
        (
            "TextInfoItem_6",
            s("Photometrics\r\n\r\nCamera Type: Prime 95B\r\nExposure: 100 ms"),
        ),
        ("TextInfoItem_9", s("15/10/2026  10:12:03")),
        ("TextInfoItem_13", s(OBJECTIVE)),
    ])
}

/// Writes the file `spec` describes.
pub fn write(path: &Path, spec: &Spec) {
    let enc = |root: &str, v: &Lv| {
        if spec.xml {
            xml(v)
        } else if spec.compressed_meta {
            lite_compressed(root, v)
        } else {
            lite(root, v)
        }
    };
    let n = spec.frames.unwrap_or_else(|| spec.frame_count());
    let attrs = level(vec![
        ("uiWidth", Lv::U32(spec.w as u32)),
        ("uiWidthBytes", Lv::U32(spec.row_bytes() as u32)),
        ("uiHeight", Lv::U32(spec.h as u32)),
        ("uiComp", Lv::U32(spec.comp as u32)),
        ("uiBpcInMemory", Lv::U32(spec.bits)),
        (
            "uiBpcSignificant",
            Lv::U32(if spec.bits == 16 { 12 } else { spec.bits }),
        ),
        ("uiSequenceCount", Lv::U32(n as u32)),
        ("eCompression", Lv::U32(if spec.compressed { 0 } else { 2 })),
    ]);
    let (sfx, seq_sfx) = if spec.xml { ("", "") } else { ("LV", "LV") };
    let mut chunks = vec![];
    for seq in 0..n {
        let mut data = (1000.0 * seq as f64).to_le_bytes().to_vec();
        let f = frame(spec, seq);
        data.extend(if spec.compressed { zlib(&f) } else { f });
        chunks.push((format!("ImageDataSeq|{seq}!"), data));
    }
    chunks.push((
        format!("ImageAttributes{sfx}!"),
        enc("SLxImageAttributes", &attrs),
    ));
    chunks.push((
        format!("ImageMetadataSeq{seq_sfx}|0!"),
        enc("SLxPictureMetadata", &picture_metadata(spec)),
    ));
    if !spec.loops.is_empty() {
        chunks.push((
            format!("ImageMetadata{sfx}!"),
            enc("SLxExperiment", &experiment(spec, 0)),
        ));
    }
    chunks.push((
        format!("ImageTextInfo{sfx}!"),
        enc("SLxImageTextInfo", &text_info(spec)),
    ));
    let app = level(vec![
        ("m_SWNameString", s("NIS-Elements AR")),
        ("m_VersionString", s("5.42.03 (Build 1234)")),
    ]);
    chunks.push(("CustomDataVar|AppInfo_V1_0!".into(), enc("", &app)));
    // Per-frame custom data, which readers must skip without cost.
    for seq in 0..n.min(3) {
        chunks.push((format!("CustomDataSeq|Stuff|{seq}!"), vec![0; 8]));
    }
    container(
        path,
        if spec.xml { "2.1" } else { "3.0" },
        &chunks,
        spec.pad,
    );
}
