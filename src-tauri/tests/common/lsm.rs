//! Writes small synthetic Zeiss LSM files (TIFF with a CZ_LSMINFO tag, image
//! and thumbnail IFDs in pairs) for tests.

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Comp {
    None,
    Lzw,
    /// LZW with the horizontal-differencing predictor.
    LzwPredictor,
}

#[derive(Clone)]
pub struct Spec {
    pub w: u32,
    pub h: u32,
    pub c: usize,
    pub z: usize,
    pub t: usize,
    /// 8 or 16.
    pub bits: u16,
    pub comp: Comp,
    pub rows_per_strip: u32,
    /// One strip set per channel (PlanarConfiguration 2) instead of interleaved samples.
    pub planar: bool,
    pub thumbnails: bool,
    /// LSMINFO ScanType.
    pub scan_type: u16,
    /// DimensionP and DimensionM (0 = not used).
    pub positions: i32,
    pub tiles: i32,
    /// Channel names and colours, scan information, time stamps, positions.
    pub metadata: bool,
    /// How many image/thumbnail pairs are really written (acquisition aborted).
    pub pairs_written: Option<usize>,
    /// Put the pixel data at this file offset (beyond 4 GiB, with wrapped 32-bit offsets).
    pub data_at: Option<u64>,
}

impl Spec {
    pub fn new(w: u32, h: u32, c: usize, z: usize, t: usize) -> Spec {
        Spec {
            w,
            h,
            c,
            z,
            t,
            bits: 8,
            comp: Comp::None,
            rows_per_strip: h,
            planar: true,
            thumbnails: true,
            scan_type: if t > 1 { 6 } else { 0 },
            positions: 0,
            tiles: 0,
            metadata: true,
            pairs_written: None,
            data_at: None,
        }
    }
}

/// Pixel value of channel `c` at (x, y) of the image with this index (T outer, Z inner).
pub fn pattern(img: usize, c: usize, x: u32, y: u32, bits: u16) -> u32 {
    let v = (img as u32 * 1009 + c as u32 * 4001 + x * 17 + y * 257) % 65521;
    if bits == 8 {
        v % 251
    } else {
        v
    }
}

pub fn plane_bytes(s: &Spec, img: usize, c: usize) -> Vec<u8> {
    let mut b = vec![];
    for y in 0..s.h {
        for x in 0..s.w {
            let v = pattern(img, c, x, y, s.bits);
            if s.bits == 8 {
                b.push(v as u8);
            } else {
                b.extend_from_slice(&(v as u16).to_le_bytes());
            }
        }
    }
    b
}

fn put(out: &mut Vec<u8>, at: usize, bytes: &[u8]) {
    if out.len() < at + bytes.len() {
        out.resize(at + bytes.len(), 0);
    }
    out[at..at + bytes.len()].copy_from_slice(bytes);
}

/// The CZ_LSMINFO structure (512 bytes), with the offsets of the blocks.
#[allow(clippy::too_many_arguments)]
fn lsminfo(s: &Spec, offs: &Offsets) -> Vec<u8> {
    let mut b = vec![0u8; 512];
    put(&mut b, 0, &67127628u32.to_le_bytes());
    put(&mut b, 4, &512i32.to_le_bytes());
    for (o, v) in [
        (8, s.w),
        (12, s.h),
        (16, s.z as u32),
        (20, s.c as u32),
        (24, s.t as u32),
    ] {
        put(&mut b, o, &(v as i32).to_le_bytes());
    }
    put(
        &mut b,
        28,
        &(if s.bits == 8 { 1i32 } else { 2 }).to_le_bytes(),
    );
    put(&mut b, 32, &(s.w as i32 / 8).to_le_bytes());
    put(&mut b, 36, &(s.h as i32 / 8).to_le_bytes());
    for (o, v) in [(40, 0.25e-6f64), (48, 0.25e-6), (56, 1.5e-6)] {
        put(&mut b, o, &v.to_le_bytes());
    }
    put(&mut b, 88, &s.scan_type.to_le_bytes());
    put(&mut b, 112, &2.5f64.to_le_bytes());
    put(&mut b, 108, &offs.colors.to_le_bytes());
    put(&mut b, 124, &offs.scan_info.to_le_bytes());
    put(&mut b, 132, &offs.stamps.to_le_bytes());
    put(&mut b, 264, &s.positions.to_le_bytes());
    put(&mut b, 268, &s.tiles.to_le_bytes());
    put(&mut b, 336, &offs.tiles.to_le_bytes());
    put(&mut b, 376, &offs.positions.to_le_bytes());
    b
}

#[derive(Default, Clone, Copy)]
struct Offsets {
    colors: u32,
    scan_info: u32,
    stamps: u32,
    positions: u32,
    tiles: u32,
}

pub const CHANNEL_NAMES: [&str; 3] = ["Ch1-T1", "Ch2-T2", "ChS1-T3"];
pub const CHANNEL_COLORS: [[u8; 3]; 3] = [[0, 255, 0], [255, 0, 0], [10, 20, 30]];

fn channel_colors(c: usize) -> Vec<u8> {
    let names: Vec<Vec<u8>> = (0..c)
        .map(|i| {
            let mut n = CHANNEL_NAMES[i % 3].as_bytes().to_vec();
            n.push(0);
            n
        })
        .collect();
    let (coff, noff) = (40usize, 40 + c * 4);
    let size = noff + names.iter().map(|n| 4 + n.len()).sum::<usize>();
    let mut b = vec![];
    for v in [size, c, c, coff, noff, 0, 0, 0, 0, 0] {
        b.extend_from_slice(&(v as u32).to_le_bytes());
    }
    for i in 0..c {
        let col = CHANNEL_COLORS[i % 3];
        b.extend_from_slice(&[col[0], col[1], col[2], 0]);
    }
    for n in names {
        b.extend_from_slice(&(n.len() as u32).to_le_bytes());
        b.extend_from_slice(&n);
    }
    assert_eq!(b.len(), size);
    b
}

/// A scan information tree like ZEN black writes it: two acquired tracks (one
/// detector each), and a third, switched-off track.
pub fn scan_info() -> Vec<u8> {
    let mut b = vec![];
    let mut rec = |entry: u32, typ: u32, payload: &[u8]| {
        b.extend_from_slice(&entry.to_le_bytes());
        b.extend_from_slice(&typ.to_le_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        b.extend_from_slice(payload);
    };
    let s = |v: &str| {
        let mut x = v.as_bytes().to_vec();
        x.push(0);
        x
    };
    let end = 0xFFFF_FFFFu32;
    rec(0x1000_0000, 0, &[]);
    rec(0x1000_0001, 2, &s("Synthetic recording"));
    rec(0x1000_0004, 2, &s("Plan-Apochromat 63x/1.40 Oil"));
    rec(0x1000_0016, 5, &2.0f64.to_le_bytes());
    rec(0x1000_001F, 5, &127.5f64.to_le_bytes());
    rec(0x1000_0020, 5, &127.5f64.to_le_bytes());
    // 2005-05-19 12:00:00
    rec(0x1000_0036, 5, &38491.5f64.to_le_bytes());
    rec(0x1000_0047, 2, &s("tester"));
    rec(0x1000_0058, 4, &7i32.to_le_bytes()); // an entry without a name
    rec(0x3000_0000, 0, &[]);
    for (name, on, power) in [("Argon/2", -1, 25.5f64), ("HeNe1", 0, 1.0)] {
        rec(0x5000_0000, 0, &[]);
        rec(0x5000_0001, 2, &s(name));
        rec(0x5000_0002, 4, &(on as i32).to_le_bytes());
        rec(0x5000_0003, 5, &power.to_le_bytes());
        rec(end, 0, &[]);
    }
    rec(end, 0, &[]);
    rec(0x2000_0000, 0, &[]);
    // (name, acquire, detector name, channel name, pinhole, gain, laser nm, transmission, spectral)
    let tracks = [
        (
            "Track A", -1, "Pmt2", "Ch1", 123.0f64, 812.0f64, 488.0f64, 2.0f64,
        ),
        ("Track B", -1, "Pmt3", "Ch2", 454.0, 690.0, 543.0, 22.0),
        ("Off", 0, "Pmt1", "Ch3", 0.0, 0.0, 633.0, 10.0),
    ];
    for (name, acquire, det, ch, pinhole, gain, wl, tr) in tracks {
        rec(0x4000_0000, 0, &[]);
        rec(0x4000_0003, 4, &2i32.to_le_bytes());
        rec(0x4000_0004, 4, &1i32.to_le_bytes());
        rec(0x4000_0005, 4, &4i32.to_le_bytes());
        rec(0x4000_0006, 4, &(acquire as i32).to_le_bytes());
        rec(0x4000_000C, 2, &s(name));
        rec(0x4000_0019, 5, &1.28f64.to_le_bytes());
        rec(0x6000_0000, 0, &[]);
        // A detector that is not acquired, then the real one.
        for on in [0i32, -1] {
            rec(0x7000_0000, 0, &[]);
            rec(
                0x7000_0003,
                5,
                &(if on != 0 { gain } else { 0.0 }).to_le_bytes(),
            );
            rec(0x7000_0009, 5, &pinhole.to_le_bytes());
            rec(0x7000_000B, 4, &on.to_le_bytes());
            rec(0x7000_000C, 2, &s(if on != 0 { det } else { "PmtOther" }));
            rec(0x7000_000F, 2, &s("EF2"));
            rec(0x7000_0010, 2, &s("BP 500-550 IR"));
            rec(0x7000_0014, 2, &s(if on != 0 { ch } else { "ChX" }));
            rec(0x7000_0026, 2, &s(" "));
            rec(end, 0, &[]);
        }
        rec(end, 0, &[]);
        rec(0x8000_0000, 0, &[]);
        rec(0x9000_0000, 0, &[]);
        rec(0x9000_0001, 2, &s("488"));
        rec(0x9000_0002, 5, &tr.to_le_bytes());
        rec(0x9000_0003, 5, &wl.to_le_bytes());
        rec(0x9000_0004, 4, &(-1i32).to_le_bytes());
        rec(end, 0, &[]);
        rec(end, 0, &[]);
        rec(0xA000_0000, 0, &[]);
        rec(0xB000_0000, 0, &[]);
        rec(0xB000_0001, 2, &s("HT"));
        rec(0xB000_0002, 2, &s("HFT UV/488/543/633"));
        rec(end, 0, &[]);
        rec(end, 0, &[]);
        rec(0xC000_0000, 0, &[]);
        rec(0xD000_0000, 0, &[]);
        rec(0xD000_0006, 4, &8i32.to_le_bytes());
        rec(end, 0, &[]);
        rec(end, 0, &[]);
        rec(end, 0, &[]); // the track
    }
    rec(end, 0, &[]); // Tracks
    rec(end, 0, &[]); // Recording
    b
}

fn encode(s: &Spec, plane: &[u8], spp_in_data: usize) -> Vec<u8> {
    let mut data = plane.to_vec();
    if s.comp == Comp::LzwPredictor {
        // Horizontal differencing per row, from the right.
        let size = (s.bits / 8) as usize;
        let row = s.w as usize * spp_in_data * size;
        for r in data.chunks_mut(row) {
            let n = r.len() / size;
            for i in (spp_in_data..n).rev() {
                if size == 1 {
                    r[i] = r[i].wrapping_sub(r[i - spp_in_data]);
                } else {
                    let a = u16::from_le_bytes([r[i * 2], r[i * 2 + 1]]);
                    let b = u16::from_le_bytes([
                        r[(i - spp_in_data) * 2],
                        r[(i - spp_in_data) * 2 + 1],
                    ]);
                    r[i * 2..i * 2 + 2].copy_from_slice(&a.wrapping_sub(b).to_le_bytes());
                }
            }
        }
    }
    match s.comp {
        Comp::None => data,
        _ => weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
            .encode(&data)
            .unwrap(),
    }
}

/// One IFD to write.
struct Page {
    main: bool,
    /// Stored strips, plane by plane.
    strips: Vec<Vec<u8>>,
    /// Uncompressed size of each strip (what LSM stores as StripByteCounts).
    counts: Vec<u32>,
    dummy: bool,
}

/// Cuts rows into strips, compresses them, and records their uncompressed sizes.
fn split(
    s: &Spec,
    data: &[u8],
    row_bytes: usize,
    spp: usize,
    strips: &mut Vec<Vec<u8>>,
    counts: &mut Vec<u32>,
) {
    for k in 0..s.h.div_ceil(s.rows_per_strip) {
        let y0 = (k * s.rows_per_strip) as usize;
        let y1 = ((k + 1) * s.rows_per_strip).min(s.h) as usize;
        let part = &data[y0 * row_bytes..y1 * row_bytes];
        counts.push(part.len() as u32);
        strips.push(encode(s, part, spp));
    }
}

/// All IFDs in file order.
fn pages(s: &Spec) -> Vec<Page> {
    let n_img = s.z * s.t * (s.positions.max(1) * s.tiles.max(1)) as usize;
    let written = s.pairs_written.unwrap_or(n_img).min(n_img);
    let size = (s.bits / 8) as u32;
    let mut out = vec![];
    for img in 0..written {
        let mut strips = vec![];
        let mut counts = vec![];
        let dummy;
        if s.planar {
            for c in 0..s.c {
                let plane = plane_bytes(s, img, c);
                split(
                    s,
                    &plane,
                    (s.w * size) as usize,
                    1,
                    &mut strips,
                    &mut counts,
                );
            }
            dummy = s.c == 2;
        } else {
            // Interleaved samples.
            let planes: Vec<Vec<u8>> = (0..s.c).map(|c| plane_bytes(s, img, c)).collect();
            let sz = size as usize;
            let mut inter = vec![];
            for i in 0..(s.w * s.h) as usize {
                for p in &planes {
                    inter.extend_from_slice(&p[i * sz..(i + 1) * sz]);
                }
            }
            split(
                s,
                &inter,
                s.w as usize * s.c * sz,
                s.c,
                &mut strips,
                &mut counts,
            );
            dummy = false;
        }
        out.push(Page {
            main: true,
            strips,
            counts,
            dummy,
        });
        if s.thumbnails {
            let (tw, th) = (s.w / 8, s.h / 8);
            let plane = vec![0xEEu8; (tw * th) as usize];
            out.push(Page {
                main: false,
                strips: vec![plane.clone(), plane.clone(), plane],
                counts: vec![tw * th; 3],
                dummy: false,
            });
        }
    }
    out
}

struct Ent {
    tag: u16,
    typ: u16,
    count: u32,
    data: Vec<u8>,
}

fn ent_short(tag: u16, v: &[u16]) -> Ent {
    Ent {
        tag,
        typ: 3,
        count: v.len() as u32,
        data: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}
fn ent_long(tag: u16, v: &[u32]) -> Ent {
    Ent {
        tag,
        typ: 4,
        count: v.len() as u32,
        data: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}

/// Writes the file. Layout: header, metadata blocks, the IFD chain, the pixel data.
pub fn write(path: &Path, s: &Spec) {
    let pages = pages(s);
    let size = s.bits / 8;
    // As LSM writes two channels: two samples, but BitsPerSample, StripOffsets and
    // StripByteCounts have a third, empty entry.
    let spp = s.c;

    let build = |data_start: u64| -> (Vec<u8>, u64) {
        let mut out = vec![0u8; 8];
        out[..4].copy_from_slice(b"II\x2a\x00");
        let mut offs = Offsets::default();
        let align = |o: &mut Vec<u8>| {
            if o.len() % 2 == 1 {
                o.push(0);
            }
        };
        if s.metadata {
            offs.colors = out.len() as u32;
            out.extend(channel_colors(s.c));
            align(&mut out);
            offs.scan_info = out.len() as u32;
            out.extend(scan_info());
            align(&mut out);
            let stamps: Vec<f64> = (0..s.z * s.t).map(|i| 1.0 + i as f64 * 2.5).collect();
            offs.stamps = out.len() as u32;
            out.extend_from_slice(&((8 + 8 * stamps.len()) as i32).to_le_bytes());
            out.extend_from_slice(&(stamps.len() as i32).to_le_bytes());
            for v in &stamps {
                out.extend_from_slice(&v.to_le_bytes());
            }
            for (list, field) in [(s.positions, 0), (s.tiles, 1)] {
                if list > 0 {
                    let o = out.len() as u32;
                    if field == 0 {
                        offs.positions = o;
                    } else {
                        offs.tiles = o;
                    }
                    out.extend_from_slice(&(list as u32).to_le_bytes());
                    for i in 0..list {
                        for v in [100.0 * (field + 1) as f64 + i as f64, 5.5, 0.0] {
                            out.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                }
            }
        }
        let info = lsminfo(s, &offs);

        // IFDs; data offsets are assigned in physical order as we go.
        let mut cursor = data_start;
        let mut first_ifd_at = 0usize;
        let mut pending_next: Option<usize> = None;
        let mut max_end = data_start;
        for (n, page) in pages.iter().enumerate() {
            align(&mut out);
            let ifd_at = out.len();
            if n == 0 {
                first_ifd_at = ifd_at;
            }
            if let Some(p) = pending_next {
                put(&mut out, p, &(ifd_at as u32).to_le_bytes());
            }
            let (w, h) = if page.main {
                (s.w, s.h)
            } else {
                (s.w / 8, s.h / 8)
            };
            let page_spp = if page.main { spp } else { 3 };
            let planar = if page.main { s.planar } else { true };
            let compression = if !page.main || s.comp == Comp::None {
                1
            } else {
                5
            };
            let rps = if page.main { s.rows_per_strip } else { h };
            let bits = if page.main { size * 8 } else { 8 };
            let mut bps: Vec<u16> = vec![
                bits;
                if page.main && s.c < 3 && planar {
                    s.c
                } else {
                    page_spp
                }
            ];
            if page.main && s.c == 2 && planar {
                bps.push(0);
            }
            // Strip offsets in physical order; the empty surplus strip points at the end of the data.
            let mut so: Vec<u32> = vec![];
            let mut sc: Vec<u32> = vec![];
            for (strip, count) in page.strips.iter().zip(&page.counts) {
                so.push(cursor as u32); // wraps like the real files
                sc.push(*count);
                cursor += strip.len() as u64;
            }
            if page.dummy {
                so.push(cursor as u32);
                sc.push(0);
            }
            max_end = max_end.max(cursor);
            let mut ents = vec![
                ent_long(254, &[if page.main { 0 } else { 1 }]),
                ent_long(256, &[w]),
                ent_long(257, &[h]),
                ent_short(258, &bps),
                ent_short(259, &[compression]),
                ent_short(262, &[if page_spp >= 3 { 2 } else { 1 }]),
                ent_long(273, &so),
                ent_short(277, &[page_spp as u16]),
                ent_long(278, &[rps]),
                ent_long(279, &sc),
                ent_short(284, &[if planar { 2 } else { 1 }]),
            ];
            if page.main && s.comp == Comp::LzwPredictor {
                ents.push(ent_short(317, &[2]));
            }
            if n == 0 {
                ents.push(Ent {
                    tag: 34412,
                    typ: 1,
                    count: info.len() as u32,
                    data: info.clone(),
                });
            }
            let table = 2 + ents.len() * 12 + 4;
            let blobs_at = ifd_at + table;
            out.extend_from_slice(&(ents.len() as u16).to_le_bytes());
            let mut blobs: Vec<u8> = vec![];
            for e in &ents {
                out.extend_from_slice(&e.tag.to_le_bytes());
                out.extend_from_slice(&e.typ.to_le_bytes());
                out.extend_from_slice(&e.count.to_le_bytes());
                if e.data.len() <= 4 {
                    let mut v = e.data.clone();
                    v.resize(4, 0);
                    out.extend_from_slice(&v);
                } else {
                    out.extend_from_slice(&((blobs_at + blobs.len()) as u32).to_le_bytes());
                    blobs.extend_from_slice(&e.data);
                    if blobs.len() % 2 == 1 {
                        blobs.push(0);
                    }
                }
            }
            pending_next = Some(out.len());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend(blobs);
        }
        put(&mut out, 4, &(first_ifd_at as u32).to_le_bytes());
        (out, max_end)
    };

    // First pass for the size of the metadata, second with the real data offsets.
    let (probe, _) = build(0);
    let data_start = s.data_at.unwrap_or(probe.len() as u64);
    let (head, end) = build(data_start);
    assert!((head.len() as u64) <= data_start);
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&head).unwrap();
    f.seek(SeekFrom::Start(data_start)).unwrap();
    for page in &pages {
        for strip in &page.strips {
            f.write_all(strip).unwrap();
        }
    }
    f.set_len(end).unwrap();
}
