//! Writes small synthetic Zeiss CZI files for tests and benchmarks.

use std::path::Path;

/// One subblock: a tile of pixels plus its directory entry.
#[derive(Clone)]
pub struct Block {
    pub pixel: i32,
    pub compression: i32,
    pub pyramid: u8,
    /// (name, start, size, stored size)
    pub dims: Vec<(&'static str, i32, i32, i32)>,
    pub data: Vec<u8>,
}

impl Block {
    /// A full-resolution tile at (x, y) of size w×h with C/Z/T/M indices.
    #[allow(clippy::too_many_arguments)]
    pub fn tile(
        pixel: i32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        c: i32,
        z: i32,
        t: i32,
        m: i32,
        data: Vec<u8>,
    ) -> Block {
        Block {
            pixel,
            compression: 0,
            pyramid: 0,
            dims: vec![
                ("X", x, w, w),
                ("Y", y, h, h),
                ("C", c, 1, 1),
                ("Z", z, 1, 1),
                ("T", t, 1, 1),
                ("M", m, 1, 1),
            ],
            data,
        }
    }

    pub fn compressed(mut self, compression: i32, data: Vec<u8>) -> Block {
        self.compression = compression;
        self.data = data;
        self
    }
}

fn segment(out: &mut Vec<u8>, id: &str, data: &[u8]) -> usize {
    let pos = out.len();
    let mut name = [0u8; 16];
    name[..id.len()].copy_from_slice(id.as_bytes());
    out.extend_from_slice(&name);
    out.extend_from_slice(&(data.len() as i64).to_le_bytes());
    out.extend_from_slice(&(data.len() as i64).to_le_bytes());
    out.extend_from_slice(data);
    pos
}

fn entry(b: &Block, pos: u64) -> Vec<u8> {
    let mut e = b"DV".to_vec();
    e.extend_from_slice(&b.pixel.to_le_bytes());
    e.extend_from_slice(&(pos as i64).to_le_bytes());
    e.extend_from_slice(&0i32.to_le_bytes()); // file part
    e.extend_from_slice(&b.compression.to_le_bytes());
    e.push(b.pyramid);
    e.extend_from_slice(&[0; 5]);
    e.extend_from_slice(&(b.dims.len() as i32).to_le_bytes());
    for &(name, start, size, stored) in &b.dims {
        let mut n = [0u8; 4];
        n[..name.len()].copy_from_slice(name.as_bytes());
        e.extend_from_slice(&n);
        e.extend_from_slice(&start.to_le_bytes());
        e.extend_from_slice(&size.to_le_bytes());
        e.extend_from_slice(&(start as f32).to_le_bytes());
        e.extend_from_slice(&stored.to_le_bytes());
    }
    e
}

/// Writes a CZI with these subblocks and metadata XML.
pub fn write(path: &Path, blocks: &[Block], xml: &str, file_part: i32) {
    let mut out = vec![];
    // File header, patched below once the positions are known.
    let hdr = segment(&mut out, "ZISRAWFILE", &[0u8; 512]) + 32;

    let mut md = vec![];
    md.extend_from_slice(&(xml.len() as i32).to_le_bytes());
    md.extend_from_slice(&[0u8; 252]);
    md.extend_from_slice(xml.as_bytes());
    let meta_pos = segment(&mut out, "ZISRAWMETADATA", &md);

    let mut entries = vec![];
    for b in blocks {
        let pos = out.len() as u64;
        let e = entry(b, pos);
        let mut d = vec![];
        d.extend_from_slice(&0i32.to_le_bytes()); // metadata size
        d.extend_from_slice(&0i32.to_le_bytes()); // attachment size
        d.extend_from_slice(&(b.data.len() as i64).to_le_bytes());
        d.extend_from_slice(&e);
        let fixed = (16 + e.len()).max(256);
        d.resize(fixed, 0);
        d.extend_from_slice(&b.data);
        segment(&mut out, "ZISRAWSUBBLOCK", &d);
        entries.push(e);
    }

    let mut dir = vec![];
    dir.extend_from_slice(&(entries.len() as i32).to_le_bytes());
    dir.extend_from_slice(&[0u8; 124]);
    for e in &entries {
        dir.extend_from_slice(e);
    }
    let dir_pos = segment(&mut out, "ZISRAWDIRECTORY", &dir);

    out[hdr..hdr + 4].copy_from_slice(&1i32.to_le_bytes()); // major
    out[hdr + 48..hdr + 52].copy_from_slice(&file_part.to_le_bytes());
    out[hdr + 52..hdr + 60].copy_from_slice(&(dir_pos as i64).to_le_bytes());
    out[hdr + 60..hdr + 68].copy_from_slice(&(meta_pos as i64).to_le_bytes());
    std::fs::write(path, out).unwrap();
}

/// Metadata XML in the shape ZEN writes, with `extra` repeated parameters
/// under Information (to make the document large).
pub fn xml(channels: &[(&str, &str)], pixel_m: f64, z_m: f64, extra: usize) -> String {
    let mut x = String::from("<ImageDocument><Metadata><Information>");
    x.push_str(
        "<Application><Name>ZEN 3.8 (blue edition)</Name><Version>3.8.2.0</Version></Application>",
    );
    x.push_str(
        "<Document><CreationDate>2026-10-15T10:12:03.98+08:00</CreationDate><UserName>tester</UserName></Document>",
    );
    x.push_str("<Image><PixelType>Gray16</PixelType><ComponentBitCount>12</ComponentBitCount><SizeX>64</SizeX><SizeY>48</SizeY><SizeZ>3</SizeZ><SizeC>2</SizeC><Dimensions><Channels>");
    for (i, (name, _)) in channels.iter().enumerate() {
        x.push_str(&format!(
            "<Channel Id=\"Channel:{i}\" Name=\"{name}\"><Fluor>{name}</Fluor><ExcitationWavelength>488.00000000000006</ExcitationWavelength><DetectorSettings><Detector Id=\"Detector:Airyscan\"/><Voltage>750</Voltage></DetectorSettings><LaserScanInfo><PixelTime>1.03e-06</PixelTime></LaserScanInfo></Channel>"
        ));
    }
    x.push_str("</Channels></Dimensions></Image>");
    x.push_str("<Instrument><Objectives><Objective Name=\"Plan-Apochromat 63x/1.40 Oil\"><LensNA>1.4</LensNA><Immersion>Oil</Immersion></Objective></Objectives></Instrument>");
    x.push_str("<Processing><Airyscan><Channels><Channel Id=\"Channel:0\"><SuperResolutionParameter>5.2</SuperResolutionParameter></Channel></Channels></Airyscan></Processing>");
    x.push_str("<Extra>");
    for i in 0..extra {
        x.push_str(&format!(
            "<Param Id=\"p{i}\"><Value>{}</Value><Unit>µm</Unit></Param>",
            i as f64 * 0.37
        ));
    }
    x.push_str("</Extra></Information>");
    x.push_str(&format!(
        "<Scaling><Items><Distance Id=\"X\"><Value>{pixel_m:e}</Value></Distance><Distance Id=\"Y\"><Value>{pixel_m:e}</Value></Distance><Distance Id=\"Z\"><Value>{z_m:e}</Value></Distance></Items></Scaling>"
    ));
    x.push_str("<DisplaySetting><Channels>");
    for (i, (name, color)) in channels.iter().enumerate() {
        x.push_str(&format!(
            "<Channel Id=\"Channel:{i}\" Name=\"{name}\"><Color>{color}</Color></Channel>"
        ));
    }
    x.push_str("</Channels></DisplaySetting></Metadata></ImageDocument>");
    x
}

/// Deterministic u16 test pattern for (c, z, x, y).
pub fn pattern(c: usize, z: usize, x: usize, y: usize) -> u16 {
    ((x * 7 + y * 13 + z * 101 + c * 1009) % 4096) as u16
}

pub fn u16_tile(c: usize, z: usize, x0: usize, y0: usize, w: usize, h: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(w * h * 2);
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&pattern(c, z, x0 + x, y0 + y).to_le_bytes());
        }
    }
    v
}

pub fn zstd(data: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
}

/// zstd1: header (size 3, hi/lo flag) then the stream of low bytes, high bytes.
pub fn zstd1_hilo(data: &[u8]) -> Vec<u8> {
    let mut split: Vec<u8> = data.chunks_exact(2).map(|p| p[0]).collect();
    split.extend(data.chunks_exact(2).map(|p| p[1]));
    let mut out = vec![3, 1, 1];
    out.extend(zstd(&split));
    out
}

/// zstd1 without hi/lo: a one-byte header.
pub fn zstd1_plain(data: &[u8]) -> Vec<u8> {
    let mut out = vec![1];
    out.extend(zstd(data));
    out
}

/// A Z stack: `channels` × `zs` pages of w×h u16, one tile per page.
pub fn stack(
    path: &Path,
    w: usize,
    h: usize,
    channels: usize,
    zs: usize,
    compression: i32,
    extra_xml: usize,
) {
    let mut blocks = vec![];
    for z in 0..zs {
        for c in 0..channels {
            let raw = u16_tile(c, z, 0, 0, w, h);
            let b = Block::tile(
                1,
                0,
                0,
                w as i32,
                h as i32,
                c as i32,
                z as i32,
                0,
                0,
                vec![],
            );
            blocks.push(match compression {
                0 => b.compressed(0, raw),
                5 => b.compressed(5, zstd(&raw)),
                6 => b.compressed(6, zstd1_hilo(&raw)),
                c => panic!("compression {c}"),
            });
        }
    }
    let names = [
        ("EGFP", "#FF00FF5B"),
        ("mCherry", "#FFFF0000"),
        ("DAPI", "#FF0000FF"),
    ];
    write(
        path,
        &blocks,
        &xml(&names[..channels.min(3)], 4.5e-8, 1.5e-7, extra_xml),
        0,
    );
}
