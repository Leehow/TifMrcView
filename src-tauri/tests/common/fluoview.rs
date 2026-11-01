//! Writes small synthetic Olympus FluoView files (OIB and OIR) for tests.

use std::path::Path;

/// The test pattern: a distinct value for every pixel of every plane.
pub fn pattern(c: usize, z: usize, t: usize, x: usize, y: usize) -> u16 {
    (c * 10007 + z * 1009 + t * 101 + y * 37 + x * 3) as u16
}

/// One plane of the pattern, little-endian u16.
pub fn plane(c: usize, z: usize, t: usize, w: usize, h: usize) -> Vec<u16> {
    (0..h)
        .flat_map(|y| (0..w).map(move |x| pattern(c, z, t, x, y)))
        .collect()
}

fn utf16(s: &str) -> Vec<u8> {
    let mut b = vec![0xff, 0xfe];
    for u in s.encode_utf16() {
        b.extend_from_slice(&u.to_le_bytes());
    }
    b
}

// ---------------------------------------------------------------- CFB

const FREE: u32 = 0xffff_ffff;
const END: u32 = 0xffff_fffe;
const FATSECT: u32 = 0xffff_fffd;

/// A stream of a compound file: (storage or "" for the root, name, data).
pub struct Stream {
    pub storage: &'static str,
    pub name: String,
    pub data: Vec<u8>,
}

/// Writes a version 3 compound file (512-byte sectors). With `interleave`,
/// the sectors of the large streams alternate, so none is contiguous.
pub fn write_cfb(path: &Path, streams: &[Stream], interleave: bool) {
    const SEC: usize = 512;
    const MINI: usize = 64;
    const CUTOFF: usize = 4096;
    let mut sectors: Vec<Vec<u8>> = vec![];
    let mut fat: Vec<u32> = vec![];
    let mut start = vec![END; streams.len()];

    // Large streams.
    let big: Vec<usize> = (0..streams.len())
        .filter(|&i| streams[i].data.len() >= CUTOFF)
        .collect();
    let mut chains: Vec<Vec<u32>> = vec![vec![]; streams.len()];
    let order: Vec<(usize, usize)> = if interleave {
        let most = big
            .iter()
            .map(|&i| streams[i].data.len().div_ceil(SEC))
            .max()
            .unwrap_or(0);
        (0..most)
            .flat_map(|k| big.iter().map(move |&i| (i, k)))
            .filter(|&(i, k)| k < streams[i].data.len().div_ceil(SEC))
            .collect()
    } else {
        big.iter()
            .flat_map(|&i| (0..streams[i].data.len().div_ceil(SEC)).map(move |k| (i, k)))
            .collect()
    };
    for (i, k) in order {
        let d = &streams[i].data;
        let mut s = d[k * SEC..((k + 1) * SEC).min(d.len())].to_vec();
        s.resize(SEC, 0);
        chains[i].push(sectors.len() as u32);
        sectors.push(s);
        fat.push(FREE);
    }
    let link = |fat: &mut Vec<u32>, chain: &[u32]| {
        for w in chain.windows(2) {
            fat[w[0] as usize] = w[1];
        }
        if let Some(&l) = chain.last() {
            fat[l as usize] = END;
        }
    };
    for &i in &big {
        link(&mut fat, &chains[i]);
        start[i] = chains[i][0];
    }

    // Small streams in the mini stream.
    let mut ministream: Vec<u8> = vec![];
    let mut minifat: Vec<u32> = vec![];
    for (i, s) in streams.iter().enumerate() {
        if s.data.len() >= CUTOFF || s.data.is_empty() {
            continue;
        }
        let first = (ministream.len() / MINI) as u32;
        let n = s.data.len().div_ceil(MINI);
        for k in 0..n {
            minifat.push(if k + 1 == n {
                END
            } else {
                first + k as u32 + 1
            });
        }
        ministream.extend_from_slice(&s.data);
        ministream.resize(ministream.len().div_ceil(MINI) * MINI, 0);
        start[i] = first;
    }
    let put = |bytes: &[u8], sectors: &mut Vec<Vec<u8>>, fat: &mut Vec<u32>| -> u32 {
        if bytes.is_empty() {
            return END;
        }
        let chain: Vec<u32> = bytes
            .chunks(SEC)
            .map(|c| {
                let mut s = c.to_vec();
                s.resize(SEC, 0);
                sectors.push(s);
                fat.push(FREE);
                (sectors.len() - 1) as u32
            })
            .collect();
        link(fat, &chain);
        chain[0]
    };
    let mini_start = put(&ministream, &mut sectors, &mut fat);
    let mf: Vec<u8> = minifat.iter().flat_map(|v| v.to_le_bytes()).collect();
    let minifat_start = put(&mf, &mut sectors, &mut fat);
    let n_minifat = mf.len().div_ceil(SEC);

    // Directory: root, one storage, then the streams. Siblings are chained
    // to the right (all black), which readers accept.
    let mut dir: Vec<Vec<u8>> = vec![];
    let entry = |name: &str, kind: u8, left: u32, right: u32, child: u32, start: u32, size: u64| {
        let mut e = vec![0u8; 128];
        let n: Vec<u16> = name.encode_utf16().collect();
        for (k, u) in n.iter().enumerate() {
            e[k * 2..k * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        e[64..66].copy_from_slice(&((n.len() as u16 + 1) * 2).to_le_bytes());
        e[66] = kind;
        e[67] = 1;
        e[68..72].copy_from_slice(&left.to_le_bytes());
        e[72..76].copy_from_slice(&right.to_le_bytes());
        e[76..80].copy_from_slice(&child.to_le_bytes());
        e[116..120].copy_from_slice(&start.to_le_bytes());
        e[120..128].copy_from_slice(&size.to_le_bytes());
        e
    };
    let root_streams: Vec<usize> = (0..streams.len())
        .filter(|&i| streams[i].storage.is_empty())
        .collect();
    let stored: Vec<usize> = (0..streams.len())
        .filter(|&i| !streams[i].storage.is_empty())
        .collect();
    // Entry ids: 0 root, 1 storage, then root streams, then stored streams.
    let id_of = |i: usize| -> u32 {
        if let Some(k) = root_streams.iter().position(|&r| r == i) {
            2 + k as u32
        } else {
            2 + root_streams.len() as u32 + stored.iter().position(|&s| s == i).unwrap() as u32
        }
    };
    dir.push(entry(
        "Root Entry",
        5,
        FREE,
        FREE,
        1,
        mini_start,
        ministream.len() as u64,
    ));
    let storage = streams
        .iter()
        .find(|s| !s.storage.is_empty())
        .map_or("Storage00001", |s| s.storage);
    let first_root = root_streams.first().map_or(FREE, |&i| id_of(i));
    let first_stored = stored.first().map_or(FREE, |&i| id_of(i));
    dir.push(entry(storage, 1, FREE, first_root, first_stored, 0, 0));
    for list in [&root_streams, &stored] {
        for (k, &i) in list.iter().enumerate() {
            let right = list.get(k + 1).map_or(FREE, |&j| id_of(j));
            dir.push(entry(
                &streams[i].name,
                2,
                FREE,
                right,
                FREE,
                start[i],
                streams[i].data.len() as u64,
            ));
        }
    }
    let d: Vec<u8> = dir.concat();
    let dir_start = put(&d, &mut sectors, &mut fat);

    // FAT sectors at the end.
    let mut n_fat = 1;
    while (sectors.len() + n_fat) > n_fat * (SEC / 4) {
        n_fat += 1;
    }
    let fat_first = sectors.len() as u32;
    for _ in 0..n_fat {
        fat.push(FATSECT);
    }
    fat.resize(n_fat * SEC / 4, FREE);
    for c in fat.chunks(SEC / 4) {
        sectors.push(c.iter().flat_map(|v| v.to_le_bytes()).collect());
    }
    assert!(n_fat <= 109);

    let mut h = vec![0u8; 512];
    h[..8].copy_from_slice(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
    h[0x18..0x1a].copy_from_slice(&0x3eu16.to_le_bytes());
    h[0x1a..0x1c].copy_from_slice(&3u16.to_le_bytes());
    h[0x1c..0x1e].copy_from_slice(&0xfffeu16.to_le_bytes());
    h[0x1e..0x20].copy_from_slice(&9u16.to_le_bytes());
    h[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    h[0x2c..0x30].copy_from_slice(&(n_fat as u32).to_le_bytes());
    h[0x30..0x34].copy_from_slice(&dir_start.to_le_bytes());
    h[0x38..0x3c].copy_from_slice(&(CUTOFF as u32).to_le_bytes());
    h[0x3c..0x40].copy_from_slice(&minifat_start.to_le_bytes());
    h[0x40..0x44].copy_from_slice(&(n_minifat as u32).to_le_bytes());
    h[0x44..0x48].copy_from_slice(&END.to_le_bytes());
    for k in 0..109 {
        let v = if k < n_fat {
            fat_first + k as u32
        } else {
            FREE
        };
        h[0x4c + k * 4..0x50 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut out = h;
    for s in sectors {
        out.extend_from_slice(&s);
    }
    std::fs::write(path, out).unwrap();
}

// ---------------------------------------------------------------- OIB

/// A plane TIFF: one u16 image in strips of `rps` rows, IFD first so the
/// pixel data starts at an odd offset.
pub fn plane_tiff(px: &[u16], w: usize, h: usize, rps: usize, big_endian: bool) -> Vec<u8> {
    let nstrips = h.div_ceil(rps);
    let tags = 10usize;
    let ifd_len = 2 + tags * 12 + 4;
    let arrays = 8 + ifd_len;
    let data = arrays + nstrips * 8 + 3; // odd on purpose
    let mut b = vec![];
    let u16b = |v: u16| {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let u32b = |v: u32| {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    b.extend_from_slice(if big_endian { b"MM" } else { b"II" });
    b.extend_from_slice(&u16b(42));
    b.extend_from_slice(&u32b(8));
    b.extend_from_slice(&u16b(tags as u16));
    let entry = |b: &mut Vec<u8>, tag: u16, typ: u16, count: u32, v: u32| {
        b.extend_from_slice(&u16b(tag));
        b.extend_from_slice(&u16b(typ));
        b.extend_from_slice(&u32b(count));
        if typ == 3 && count == 1 {
            b.extend_from_slice(&u16b(v as u16));
            b.extend_from_slice(&[0, 0]);
        } else {
            b.extend_from_slice(&u32b(v));
        }
    };
    entry(&mut b, 256, 4, 1, w as u32);
    entry(&mut b, 257, 4, 1, h as u32);
    entry(&mut b, 258, 3, 1, 16);
    entry(&mut b, 259, 3, 1, 1);
    entry(&mut b, 262, 3, 1, 1);
    let offs_at = arrays as u32;
    let counts_at = (arrays + nstrips * 4) as u32;
    if nstrips == 1 {
        entry(&mut b, 273, 4, 1, data as u32);
    } else {
        entry(&mut b, 273, 4, nstrips as u32, offs_at);
    }
    entry(&mut b, 277, 3, 1, 1);
    entry(&mut b, 278, 4, 1, rps as u32);
    if nstrips == 1 {
        entry(&mut b, 279, 4, 1, (w * h * 2) as u32);
    } else {
        entry(&mut b, 279, 4, nstrips as u32, counts_at);
    }
    entry(&mut b, 284, 3, 1, 1);
    b.extend_from_slice(&u32b(0));
    assert_eq!(b.len(), arrays);
    for k in 0..nstrips {
        b.extend_from_slice(&u32b((data + k * rps * w * 2) as u32));
    }
    for k in 0..nstrips {
        let rows = rps.min(h - k * rps);
        b.extend_from_slice(&u32b((rows * w * 2) as u32));
    }
    b.resize(data, 0);
    for &v in px {
        b.extend_from_slice(&u16b(v));
    }
    b
}

/// An OIB with `nc` channels, `nz` Z planes and `nt` time points of w×h
/// u16, plane TIFFs in strips of `rps` rows.
pub struct OibSpec {
    pub w: usize,
    pub h: usize,
    pub nc: usize,
    pub nz: usize,
    pub nt: usize,
    pub rps: usize,
    pub big_endian: bool,
    pub interleave: bool,
    /// Planes (c, z, t) left out of the file.
    pub missing: Vec<(usize, usize, usize)>,
}

impl Default for OibSpec {
    fn default() -> OibSpec {
        OibSpec {
            w: 64,
            h: 48,
            nc: 2,
            nz: 3,
            nt: 1,
            rps: 16,
            big_endian: false,
            interleave: false,
            missing: vec![],
        }
    }
}

fn main_oif(s: &OibSpec) -> String {
    let mut m = String::new();
    m.push_str(
        "[Version Info]\r\nFileVersion=\"1.2.6.0\"\r\nSystemName=\"FLUOVIEW FV1000\"\r\nSystemVersion=\"4.2.4.5\"\r\n",
    );
    m.push_str("[Acquisition Parameters Common]\r\nAcquisition Device=\"FV1000\"\r\nImageCaputreDate='2022-08-24 14:31:02'\r\n");
    m.push_str("IntegrationCount=2\r\nIntegrationType=\"Line Kalman\"\r\nLaserTransmissivity01=5,3.0\r\nLaserTransmissivity02=12,5.0\r\n");
    m.push_str("LaserWavelength01=405\r\nLaserWavelength02=543\r\nScanDirection=\"Oneway\"\r\nScanMode=\"XYZ\"\r\nSequentialMode=\"Line\"\r\nZoomValue=2.0\r\n");
    let axes = [
        ("X", s.w, "um"),
        ("Y", s.h, "um"),
        ("C", s.nc, "Ch"),
        ("Z", s.nz, "nm"),
        ("T", if s.nt > 1 { s.nt } else { 0 }, "ms"),
        ("A", 0, "deg"),
        ("L", 0, "nm"),
        ("P", 0, "mm"),
    ];
    for (i, (code, n, unit)) in axes.iter().enumerate() {
        m.push_str(&format!(
            "[Axis {i} Parameters Common]\r\nAxisCode=\"{code}\"\r\nMaxSize={n}\r\nPixUnit=\"{unit}\"\r\n"
        ));
        match *code {
            "Z" => m.push_str(&format!(
                "StartPosition=-1000.0\r\nEndPosition={}.0\r\nInterval=150\r\n",
                -1000 + 150 * (*n as i64 - 1)
            )),
            "T" => m.push_str(&format!(
                "StartPosition=0.0\r\nEndPosition={}.0\r\n",
                2500 * (*n as i64 - 1).max(0)
            )),
            _ => {}
        }
    }
    let order = if s.nt > 1 { "XYCZT" } else { "XYCZ" };
    m.push_str(&format!(
        "[Axis Parameter Common]\r\nAxisCount={}\r\nAxisOrder=\"{order}\"\r\n",
        order.len()
    ));
    for (c, (dye, ch, ex, em)) in [
        ("DAPI", "CH1", 405, 461),
        ("Alexa Fluor 594", "CH3", 543, 618),
    ]
    .iter()
    .enumerate()
    .take(s.nc)
    {
        m.push_str(&format!(
            "[Channel {} Parameters]\r\nAnalogPMTVoltage={}\r\nBF Name=\"BA430-455\"\r\nCH Name=\"{ch}\"\r\nConfocal=\"ON\"\r\nDyeName=\"{dye}\"\r\nEmissionDM Name=\"SDM473\"\r\nEmissionWavelength={em}\r\nExcitationWavelength={ex}\r\nPMTDetectingMode=0\r\n",
            c + 1,
            500 + c
        ));
    }
    m.push_str("[File Info]\r\nDataName=\"synthetic.oib\"\r\nFileType=\"GrayScale\"\r\nUserName=\"Guest\"\r\n");
    m.push_str("[ProfileSaveInfo]\r\nLutFileName0=\"synthetic.oif.files\\s_LUT1.lut\"\r\nLutFileName1=\"synthetic.oif.files\\s_LUT2.lut\"\r\n");
    m.push_str(&format!(
        "[Reference Image Parameter]\r\nHeightConvertValue=0,125.0\r\nHeightUnit=\"um\"\r\nImageDepth=2\r\nImageHeight={}\r\nImageWidth={}\r\nValidBitCounts=12\r\nWidthConvertValue=0,125.0\r\nWidthUnit=\"um\"\r\n",
        s.h, s.w
    ));
    m
}

fn pty(c: usize) -> String {
    format!(
        "[Acquisition Parameters Common]\r\nAnalogPMTGain=1.0\r\nAnalogPMTOffset={}\r\nMagnification=60.0\r\nObjectiveLens NAValue=1,35.0\r\nObjectiveLens Name=\"UPLSAPO   60X O  NA:1.35\"\r\nObjectiveLens WDValue=9999.0\r\nPMTVoltage={}\r\nPinholeDiameter=110000\r\nTime Per Frame=\"38419200.000\"\r\nTime Per Line=\"37120.000\"\r\nTime Per Pixel=\"8.0\"\r\nZoomValue=2.0\r\n[Image Parameters]\r\nDataType=\"WORD\"\r\n",
        2 + c,
        508 + c
    )
}

fn lut(rgb: [bool; 3]) -> Vec<u8> {
    let c = |on: bool| if on { 100 } else { 0 };
    let mut b = utf16(&format!(
        "[Blue]\r\nContrast={}\r\n[FileInformation]\r\nName=\"LUT\"\r\n[Green]\r\nContrast={}\r\n[Red]\r\nContrast={}\r\n[ColorLUTData]\r\n",
        c(rgb[2]),
        c(rgb[1]),
        c(rgb[0])
    ));
    b.extend(std::iter::repeat_n(0x7fu8, 1024));
    b
}

pub fn write_oib(path: &Path, s: &OibSpec) {
    let mut streams = vec![];
    let mut info = String::from("[OibSaveInfo]\r\nMainFileName=Stream00001\r\nName=COFAImages\r\nStorage00001=synthetic.oif.files\r\nStream00001=synthetic.oif\r\n");
    let mut k = 2;
    let mut add = |storage: &'static str,
                   orig: &str,
                   data: Vec<u8>,
                   info: &mut String,
                   streams: &mut Vec<Stream>| {
        let name = format!("Stream{k:05}");
        let full = if storage.is_empty() {
            orig.to_string()
        } else {
            format!("{storage}/{orig}")
        };
        info.push_str(&format!("{name}={full}\r\n"));
        streams.push(Stream {
            storage,
            name,
            data,
        });
        k += 1;
    };
    streams.push(Stream {
        storage: "",
        name: "Stream00001".into(),
        data: utf16(&main_oif(s)),
    });
    for t in 0..s.nt {
        for z in 0..s.nz {
            for c in 0..s.nc {
                if s.missing.contains(&(c, z, t)) {
                    continue;
                }
                let stem = if s.nt > 1 {
                    format!("s_C{:03}Z{:03}T{:03}", c + 1, z + 1, t + 1)
                } else {
                    format!("s_C{:03}Z{:03}", c + 1, z + 1)
                };
                let tif = plane_tiff(&plane(c, z, t, s.w, s.h), s.w, s.h, s.rps, s.big_endian);
                add(
                    "Storage00001",
                    &format!("{stem}.tif"),
                    tif,
                    &mut info,
                    &mut streams,
                );
                add(
                    "Storage00001",
                    &format!("{stem}.pty"),
                    utf16(&pty(c)),
                    &mut info,
                    &mut streams,
                );
            }
        }
    }
    add(
        "Storage00001",
        "s_LUT1.lut",
        lut([false, false, true]),
        &mut info,
        &mut streams,
    );
    add(
        "Storage00001",
        "s_LUT2.lut",
        lut([true, false, false]),
        &mut info,
        &mut streams,
    );
    // A thumbnail that is not part of the image.
    add(
        "Storage00001",
        "s_Thumb.tif",
        plane_tiff(&[1, 2, 3, 4], 2, 2, 2, false),
        &mut info,
        &mut streams,
    );
    streams.push(Stream {
        storage: "",
        name: "OibInfo.txt".into(),
        data: utf16(&info),
    });
    write_cfb(path, &streams, s.interleave);
}

// ---------------------------------------------------------------- OIR

pub const GUIDS: [&str; 3] = [
    "8bf167bc-288a-4fa1-8817-0814f5e321ed",
    "46ac593d-5fab-4079-a377-8f2df6102d3e",
    "c263395c-ad37-4a8d-bfe5-2603c4c5a39a",
];

pub struct OirSpec {
    pub w: usize,
    pub h: usize,
    pub nc: usize,
    pub nz: usize,
    pub nt: usize,
    pub nl: usize,
    /// Bytes per pixel strip (need not hold whole rows).
    pub strip: usize,
    /// Leave the block index out (readers walk the blocks).
    pub no_index: bool,
    /// Add a reference image (one plane per channel).
    pub reference: bool,
}

impl Default for OirSpec {
    fn default() -> OirSpec {
        OirSpec {
            w: 40,
            h: 30,
            nc: 2,
            nz: 3,
            nt: 2,
            nl: 1,
            strip: 800,
            no_index: false,
            reference: true,
        }
    }
}

fn block(out: &mut Vec<u8>, offsets: &mut Vec<u64>, typ: u32, payload: &[u8]) {
    offsets.push(out.len() as u64);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&typ.to_le_bytes());
    out.extend_from_slice(payload);
}

fn image_properties(s: &OirSpec) -> String {
    let mut x = String::from(
        r#"<?xml version="1.0" encoding="ASCII"?>
<lsmimage:imageProperties xmlns:lsmimage="l" xmlns:commonimage="c" xmlns:base="b" xmlns:commonphase="p" xmlns:commonparam="q" xmlns:lsmparam="r" xmlns:root="s" xmlns:fvCommonphase="t" xmlns:o="u">
  <commonimage:general><base:creationDateTime>2024-12-02T09:52:10.944+08:00</base:creationDateTime></commonimage:general>
  <commonimage:system><base:systemName>FV4000</base:systemName><base:systemVersion>3.1.1.67</base:systemVersion></commonimage:system>
  <commonimage:microscope><base:name>IX83P2ZF</base:name></commonimage:microscope>
  <commonimage:imageInfo>
    <commonimage:phase id="ph">
      <commonphase:group id="g">
"#,
    );
    // Channels listed in reverse order of their `order` attribute.
    for c in (0..s.nc).rev() {
        x.push_str(&format!(
            r#"        <commonphase:channel id="{}" order="{}">
          <root:productData><fvCommonphase:pinholeDiameter>130.0</fvCommonphase:pinholeDiameter>
            <fvCommonphase:wavelengthRange><o:startWavelength>{}</o:startWavelength><o:endWavelength>{}</o:endWavelength></fvCommonphase:wavelengthRange></root:productData>
          <commonphase:name>CH{}</commonphase:name>
          <commonphase:length><commonparam:x>0.25</commonparam:x><commonparam:y>0.25</commonparam:y><commonparam:z>1.5</commonparam:z></commonphase:length>
          <commonphase:pixelUnit><commonparam:x>MICRO_METER</commonparam:x><commonparam:y>MICRO_METER</commonparam:y><commonparam:z>MICRO_METER</commonparam:z></commonphase:pixelUnit>
          <lsmimage:laserDataId>LD488_1Imaging_main_phase_1</lsmimage:laserDataId>
        </commonphase:channel>
"#,
            GUIDS[c],
            c + 1,
            500 + 70 * c,
            540 + 70 * c,
            c + 2
        ));
    }
    x.push_str(
        r#"      </commonphase:group>
      <lsmimage:imagingMainLaser enable="true" id="LD488_1Imaging_main_phase_1" laserId="LD488_1"><commonimage:wavelength>488</commonimage:wavelength></lsmimage:imagingMainLaser>
    </commonimage:phase>
  </commonimage:imageInfo>
  <commonimage:acquisition>
    <commonimage:microscopeConfiguration>
      <commonimage:objectiveLens id="UPLXAPO10X"><base:name>UPLXAPO10X</base:name><base:displayName>UPLXAPO10X</base:displayName><o:magnification>10.0</o:magnification><o:naValue>0.4</o:naValue><o:wdValue>3.1</o:wdValue><o:immersion>DRY</o:immersion></commonimage:objectiveLens>
    </commonimage:microscopeConfiguration>
    <commonimage:imagingParam>
      <commonparam:axis enable="true"><commonparam:axis>ZSTACK</commonparam:axis><commonparam:startPosition>10.0</commonparam:startPosition><commonparam:step>1.5</commonparam:step></commonparam:axis>
      <commonparam:axis enable="true"><commonparam:axis>TIMELAPSE</commonparam:axis><commonparam:startPosition>0.0</commonparam:startPosition><commonparam:step>30.0</commonparam:step></commonparam:axis>
      <commonparam:axis enable="true"><commonparam:axis>LAMBDA</commonparam:axis><commonparam:startPosition>420.0</commonparam:startPosition><commonparam:step>50.0</commonparam:step></commonparam:axis>
      <lsmparam:mainLaser laserDataId="LD488_1Imaging_main_phase_1"><commonparam:power>100.0</commonparam:power><commonparam:transmissivity>0.6</commonparam:transmissivity></lsmparam:mainLaser>
      <lsmparam:area><lsmparam:zoom>2.0</lsmparam:zoom></lsmparam:area>
"#,
    );
    for c in 0..s.nc {
        x.push_str(&format!(
            "      <lsmparam:pmt channelId=\"{}\" detectorId=\"D{c}\"><lsmparam:voltage>{}</lsmparam:voltage><lsmparam:offset>3</lsmparam:offset><lsmparam:gain>1.0</lsmparam:gain></lsmparam:pmt>\n",
            GUIDS[c],
            600 + c
        ));
    }
    x.push_str(
        r#"      <lsmparam:method><lsmparam:sequentialType>Line</lsmparam:sequentialType><lsmparam:integration><commonparam:type>Line</commonparam:type><commonparam:count>4</commonparam:count></lsmparam:integration></lsmparam:method>
    </commonimage:imagingParam>
    <commonimage:configuration><lsmimage:scannerType>Galvano</lsmimage:scannerType></commonimage:configuration>
    <lsmimage:scannerSettings type="Resonant"><lsmimage:param><lsmparam:speed><commonparam:speed>0.033</commonparam:speed></lsmparam:speed></lsmimage:param></lsmimage:scannerSettings>
    <lsmimage:scannerSettings type="Galvano"><lsmimage:param><lsmparam:speed><commonparam:speed>2.0</commonparam:speed><commonparam:speedInformation><commonparam:lineSpeed>2.112</commonparam:lineSpeed><commonparam:frameSpeed>1083.456</commonparam:frameSpeed></commonparam:speedInformation></lsmparam:speed></lsmimage:param></lsmimage:scannerSettings>
  </commonimage:acquisition>
</lsmimage:imageProperties>"#,
    );
    x
}

/// One XML document of a metadata block, with the binary header before it
/// (here: the channel uuid, as LUT documents have it).
fn meta_doc(out: &mut Vec<u8>, guid: Option<&str>, xml: &str) {
    out.extend_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0]);
    if let Some(g) = guid {
        out.extend_from_slice(&36u32.to_le_bytes());
        out.extend_from_slice(g.as_bytes());
    }
    out.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    out.extend_from_slice(xml.as_bytes());
}

pub fn write_oir(path: &Path, s: &OirSpec) {
    let mut out = vec![0u8; 96];
    out[..16].copy_from_slice(b"OLYMPUSRAWFORMAT");
    for (k, v) in [12u32, 0, 1, 2].iter().enumerate() {
        out[16 + k * 4..20 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
    out[72..80].copy_from_slice(b"FLUOVIEW");
    let mut offsets = vec![];

    let uid_block =
        |out: &mut Vec<u8>, offsets: &mut Vec<u64>, uid: &str, at: usize, data: &[u8]| {
            let mut p = vec![];
            p.extend_from_slice(&(at as u32).to_le_bytes());
            p.extend_from_slice(&(data.len() as u32).to_le_bytes());
            p.extend_from_slice(&(uid.len() as u32).to_le_bytes());
            p.extend_from_slice(uid.as_bytes());
            block(out, offsets, 3, &p);
            block(out, offsets, 4, data);
        };
    let bytes = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };

    if s.reference {
        for c in 0..s.nc {
            let d = bytes(&plane(c, 99, 0, s.w, s.h));
            for (n, chunk) in d.chunks(s.strip).enumerate() {
                uid_block(
                    &mut out,
                    &mut offsets,
                    &format!("REF_LSM0_{}_{n}", GUIDS[c]),
                    n * s.strip,
                    chunk,
                );
            }
        }
    }
    // Metadata.
    let mut m = vec![];
    meta_doc(&mut m, None, "<?xml version=\"1.0\"?>\n<fileinfo:fileInfomation xmlns:fileinfo=\"f\"><fileinfo:version>2.1.2.3</fileinfo:version></fileinfo:fileInfomation>");
    meta_doc(&mut m, None, &image_properties(s));
    for c in 0..s.nc {
        meta_doc(
            &mut m,
            None,
            &format!(
                "<?xml version=\"1.0\"?>\n<lsmimage:lsmChannel xmlns:lsmimage=\"l\" xmlns:commonphase=\"p\" xmlns:dye=\"d\" id=\"{}\" order=\"{}\"><commonphase:name>CH{}</commonphase:name><lsmimage:deviceName>BSD{}</lsmimage:deviceName><lsmimage:dyeName>{}</lsmimage:dyeName><lsmimage:dyeData id=\"x\"><dye:excitationWavelength>498</dye:excitationWavelength><dye:emissionWavelength>517</dye:emissionWavelength></lsmimage:dyeData></lsmimage:lsmChannel>",
                GUIDS[c],
                c + 1,
                c + 2,
                c + 1,
                ["FITC", "mCherry", "DAPI"][c]
            ),
        );
    }
    for c in 0..s.nc {
        let (r, g) = if c == 0 { (0.0, 1.0) } else { (1.0, 0.0) };
        let mut lut = format!(
            "<?xml version=\"1.0\"?>\n<lut:LUT xmlns:lut=\"u\" id=\"x\"><lut:name>L</lut:name><lut:red><lut:contrast>{r}</lut:contrast></lut:red><lut:green><lut:contrast>{g}</lut:contrast></lut:green><lut:blue><lut:contrast>0.0</lut:contrast></lut:blue><lut:data>"
        );
        lut.push_str(&"FF000000".repeat(2000));
        lut.push_str("</lut:data></lut:LUT>");
        meta_doc(&mut m, Some(GUIDS[c]), &lut);
    }
    meta_doc(&mut m, None, &format!("<?xml version=\"1.0\"?>\n<base:imageDefinition xmlns:base=\"b\"><base:width>{}</base:width><base:height>{}</base:height></base:imageDefinition>", s.w, s.h));
    block(&mut out, &mut offsets, 0, &m);

    let frame = format!(
        "<?xml version=\"1.0\" encoding=\"ASCII\"?>\n<lsmframe:frameProperties xmlns:lsmframe=\"f\" xmlns:commonframe=\"c\" xmlns:base=\"b\"><commonframe:imageDefinition><base:colorType>GlayScale</base:colorType><base:width>{}</base:width><base:height>{}</base:height><base:depth>2</base:depth><base:bitCounts>12</base:bitCounts></commonframe:imageDefinition></lsmframe:frameProperties>",
        s.w, s.h
    );
    for t in 0..s.nt {
        for z in 0..s.nz {
            for l in 0..s.nl {
                let mut prefix = String::new();
                if s.nl > 1 {
                    prefix.push_str(&format!("l{:03}", l + 1));
                }
                prefix.push_str(&format!("z{:03}", z + 1));
                if s.nt > 1 {
                    prefix.push_str(&format!("t{:03}", t + 1));
                }
                let mut fp = vec![0u8; 8];
                fp.extend_from_slice(frame.as_bytes());
                block(&mut out, &mut offsets, 1, &fp);
                for c in 0..s.nc {
                    // λ planes use the channel index c + nc·l for the pattern.
                    let d = bytes(&plane(c + s.nc * l, z, t, s.w, s.h));
                    for (n, chunk) in d.chunks(s.strip).enumerate() {
                        uid_block(
                            &mut out,
                            &mut offsets,
                            &format!("{prefix}_0_1_{}_{n}", GUIDS[c]),
                            n * s.strip,
                            chunk,
                        );
                    }
                }
                block(&mut out, &mut offsets, 5, &[]);
            }
        }
    }
    let index = out.len() as u64;
    if !s.no_index {
        out.extend_from_slice(&0xffff_ffffu32.to_le_bytes());
        for o in &offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        out.extend_from_slice(&0u64.to_le_bytes());
    }
    let size = out.len() as u64;
    out[32..40].copy_from_slice(&size.to_le_bytes());
    out[40..48].copy_from_slice(&(if s.no_index { 0 } else { index }).to_le_bytes());
    std::fs::write(path, out).unwrap();
}
