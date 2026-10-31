//! SerialEM montages: an MRC stack whose sections are pieces of one large
//! image (a grid or atlas map), each with its position. The positions come
//! from the `.mdoc` next to the file (`PieceCoordinates` per `[ZValue]`) or,
//! without one, from the SerialEM extended header. The montage is shown
//! assembled, one plane per montage section; where pieces overlap, each
//! keeps the half nearer its own centre (3dmod shows montages the same way,
//! without blending).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;

use super::mrc::MrcSource;
use crate::image::{DType, ImageInfo, MetaGroup, Plane, PlaneData, SeriesInfo, Source};

/// Largest assembled plane, in bytes; bigger montages open as a piece stack.
const MAX_BYTES: usize = 16 << 30;

/// Where one piece sits and which part of it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    /// Section of the MRC stack.
    pub section: usize,
    /// Top-left corner on the montage plane.
    pub x: usize,
    pub y: usize,
    /// Drawn part, in montage coordinates: [x0, x1) × [y0, y1).
    pub own: [usize; 4],
}

/// Piece positions (x, y, montage section) for every section of the stack,
/// from the `.mdoc` or the extended header.
fn coordinates(path: &Path, mrc: &MrcSource) -> Option<(Vec<[i64; 3]>, &'static str)> {
    let n = mrc.stack_depth();
    let mdoc = std::fs::read_to_string(format!("{}.mdoc", path.display())).ok();
    if let Some(c) = mdoc.as_deref().and_then(|t| from_mdoc(t, n)) {
        return Some((c, ".mdoc"));
    }
    from_extended_header(mrc).map(|c| (c, "MRC 扩展头"))
}

/// `PieceCoordinates` of each `[ZValue = n]` section, when the file is a
/// montage (`Montage = 1`) and every section has them.
pub fn from_mdoc(text: &str, sections: usize) -> Option<Vec<[i64; 3]>> {
    let mut montage = false;
    let mut z: Option<usize> = None;
    let mut out: Vec<Option<[i64; 3]>> = vec![None; sections];
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("[ZValue") {
            z = rest.trim_start_matches([' ', '=']).trim_end_matches(']').trim().parse().ok();
            continue;
        }
        if line.starts_with('[') {
            z = None;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match (key.trim(), z) {
            ("Montage", None) => montage = value.trim() == "1",
            ("PieceCoordinates", Some(z)) if z < sections => {
                let v: Vec<i64> = value.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                if v.len() >= 2 {
                    out[z] = Some([v[0], v[1], v.get(2).copied().unwrap_or(0)]);
                }
            }
            _ => {}
        }
    }
    if !montage {
        return None;
    }
    out.into_iter().collect()
}

/// Piece coordinates stored by SerialEM in its extended header (type
/// "SERI"): `nint` bytes per section, `nreal` flags saying which fields are
/// there. Bit 1 is the tilt angle (2 bytes), bit 2 the piece coordinates
/// (three 16-bit integers). They are read unsigned: montages wider than
/// 32767 pixels overflow the signed field, and pieces never sit below zero.
fn from_extended_header(mrc: &MrcSource) -> Option<Vec<[i64; 3]>> {
    let ext = mrc.extended_header();
    let (nint, flags) = mrc.serial_em_fields()?;
    if flags & 2 == 0 || nint < 8 {
        return None;
    }
    let at = if flags & 1 != 0 { 2 } else { 0 };
    let read = |b: &[u8]| -> i64 { (if mrc.little() { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) }) as i64 };
    (0..mrc.stack_depth())
        .map(|s| {
            let e = ext.get(s * nint + at..s * nint + at + 6)?;
            Some([read(&e[0..2]), read(&e[2..4]), read(&e[4..6])])
        })
        .collect()
}

/// Lays the pieces out: the montage sections in order, the plane size, and
/// for every section its pieces with the part each one draws. None when the
/// coordinates do not describe a montage.
pub fn layout(coords: &[[i64; 3]], w: usize, h: usize) -> Option<(Vec<Vec<Piece>>, usize, usize)> {
    if coords.len() < 2 || coords.iter().any(|c| c[0] < 0 || c[1] < 0 || c[2] < 0) {
        return None;
    }
    let first = coords[0];
    if coords.iter().all(|c| c[0] == first[0] && c[1] == first[1]) {
        return None;
    }
    let width = coords.iter().map(|c| c[0] as usize).max()? + w;
    let height = coords.iter().map(|c| c[1] as usize).max()? + h;
    let mut zs: Vec<i64> = coords.iter().map(|c| c[2]).collect();
    zs.sort_unstable();
    zs.dedup();
    let planes = zs
        .iter()
        .map(|&z| {
            let mut pieces: Vec<Piece> = coords
                .iter()
                .enumerate()
                .filter(|(_, c)| c[2] == z)
                .map(|(section, c)| {
                    let (x, y) = (c[0] as usize, c[1] as usize);
                    Piece { section, x, y, own: [x, x + w, y, y + h] }
                })
                .collect();
            split_overlaps(&mut pieces, w, h);
            pieces
        })
        .collect();
    Some((planes, width, height))
}

/// Cuts every overlap between two pieces at its middle, across the axis
/// along which the pieces are further apart, so each piece keeps the half
/// nearer its centre. Pieces at the same position keep everything; the later
/// one is drawn last.
fn split_overlaps(pieces: &mut [Piece], w: usize, h: usize) {
    let all = pieces.to_vec();
    for p in pieces.iter_mut() {
        for q in &all {
            let (dx, dy) = (q.x as i64 - p.x as i64, q.y as i64 - p.y as i64);
            if (dx == 0 && dy == 0) || dx.unsigned_abs() as usize >= w || dy.unsigned_abs() as usize >= h {
                continue;
            }
            if dx.abs() >= dy.abs() {
                let mid = (p.x.max(q.x) + (p.x + w).min(q.x + w)) / 2;
                if dx > 0 {
                    p.own[1] = p.own[1].min(mid);
                } else {
                    p.own[0] = p.own[0].max(mid);
                }
            } else {
                let mid = (p.y.max(q.y) + (p.y + h).min(q.y + h)) / 2;
                if dy > 0 {
                    p.own[3] = p.own[3].min(mid);
                } else {
                    p.own[2] = p.own[2].max(mid);
                }
            }
        }
    }
}

pub struct Montage {
    pieces: MrcSource,
    info: ImageInfo,
    planes: Vec<Vec<Piece>>,
    /// Every `bin`-th pixel of each piece (1 except for previews).
    bin: usize,
    fill: f64,
    done: AtomicUsize,
}

/// Opens `path` as a montage when it is one: series 0 is the assembled
/// montage, series 1 the stack of single pieces. Anything else opens as a
/// plain MRC stack.
pub fn open(path: &Path, series: usize) -> Result<Box<dyn Source>> {
    open_scaled(path, series, None)
}

/// For thumbnails: the montage subsampled to at most `max_side` pixels a
/// side, so a 37000-pixel map does not have to be assembled in full.
pub fn open_preview(path: &Path, max_side: usize) -> Result<Box<dyn Source>> {
    open_scaled(path, 0, Some(max_side))
}

fn open_scaled(path: &Path, series: usize, max_side: Option<usize>) -> Result<Box<dyn Source>> {
    let mrc = MrcSource::open(path)?;
    let Some((coords, from)) = coordinates(path, &mrc) else {
        anyhow::ensure!(series == 0, "这个文件只有一个图像");
        return Ok(Box::new(mrc));
    };
    let base = mrc.info().clone();
    let assembled = layout(&coords, base.width, base.height)
        .filter(|(_, w, h)| w.saturating_mul(*h).saturating_mul(base.dtype.size()) <= MAX_BYTES);
    let Some((planes, width, height)) = assembled else {
        anyhow::ensure!(series == 0, "这个文件只有一个图像");
        return Ok(Box::new(mrc));
    };
    let list = vec![
        SeriesInfo { name: "拼好的 montage".into(), detail: format!("{width}×{height}") },
        SeriesInfo { name: "单张 piece".into(), detail: format!("{}×{} × {}", base.width, base.height, base.size_z) },
    ];
    let group = MetaGroup {
        name: "Montage 拼图".into(),
        items: vec![
            ["piece 数".into(), coords.len().to_string()],
            ["piece 大小".into(), format!("{}×{}", base.width, base.height)],
            ["拼好的大小".into(), format!("{width}×{height}")],
            ["坐标来源".into(), from.into()],
        ],
    };
    if series == 1 {
        let mut stack = mrc;
        let info = stack.info_mut();
        info.series = list;
        info.series_index = 1;
        info.metadata.insert(0, group);
        return Ok(Box::new(stack));
    }
    anyhow::ensure!(series == 0, "这个文件里没有这个图像");
    let bin = max_side.map_or(1, |m| width.max(height).div_ceil(m.max(1)).max(1));
    let mut info = base;
    info.width = width.div_ceil(bin);
    info.height = height.div_ceil(bin);
    if let Some(ps) = info.pixel_size.as_mut() {
        ps[0] *= bin as f64;
        ps[1] *= bin as f64;
    }
    info.size_z = planes.len();
    info.page_count = planes.len();
    info.series = list;
    info.series_index = 0;
    info.metadata.insert(0, group);
    let fill = mrc.header_mean();
    Ok(Box::new(Montage { pieces: mrc, info, planes, bin, fill, done: AtomicUsize::new(0) }))
}

impl Source for Montage {
    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn progress(&self) -> Option<(usize, usize)> {
        let total: usize = self.planes.iter().map(Vec::len).sum();
        let done = self.done.load(Ordering::Relaxed);
        (done < total).then_some((done, total))
    }

    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        use rayon::prelude::*;
        let (w, h, bin) = (self.info.width, self.info.height, self.bin);
        let es = self.info.dtype.size();
        let mut data = filled(self.info.dtype, w * h, self.fill);
        let out = data.as_bytes_mut();
        let pw = self.pieces.info().width;
        for p in &self.planes[page.min(self.planes.len() - 1)] {
            // Output pixels whose source pixel (ox·bin, oy·bin) this piece draws.
            let [x0, x1, y0, y1] = p.own.map(|v| v.div_ceil(bin));
            if x0 < x1 && y0 < y1 {
                // Samples straight from the mapped file when they are stored
                // in native order, else decoded.
                let decoded;
                let src = match self.pieces.raw_page(p.section) {
                    Some((raw, little)) if little == cfg!(target_endian = "little") || es == 1 => raw,
                    _ => {
                        decoded = self.pieces.read_page(p.section)?;
                        decoded[0].data.as_bytes()
                    }
                };
                out[y0 * w * es..y1 * w * es].par_chunks_mut(w * es).enumerate().for_each(|(i, row)| {
                    let s = ((y0 + i) * bin - p.y) * pw + x0 * bin - p.x;
                    if bin == 1 {
                        row[x0 * es..x1 * es].copy_from_slice(&src[s * es..(s + x1 - x0) * es]);
                    } else {
                        for (k, o) in row[x0 * es..x1 * es].chunks_exact_mut(es).enumerate() {
                            let at = (s + k * bin) * es;
                            o.copy_from_slice(&src[at..at + es]);
                        }
                    }
                });
            }
            self.done.fetch_add(1, Ordering::Relaxed);
        }
        Ok(vec![Plane { w, h, data }])
    }
}

/// A plane of `len` samples all set to `v` (where no piece was recorded).
fn filled(dtype: DType, len: usize, v: f64) -> PlaneData {
    let v = if v.is_finite() { v } else { 0.0 };
    match dtype {
        DType::U8 => PlaneData::U8(vec![v.clamp(0.0, 255.0) as u8; len]),
        DType::I8 => PlaneData::I8(vec![v.clamp(-128.0, 127.0) as i8; len]),
        DType::U16 => PlaneData::U16(vec![v.clamp(0.0, 65535.0) as u16; len]),
        DType::I16 => PlaneData::I16(vec![v.clamp(-32768.0, 32767.0) as i16; len]),
        DType::U32 => PlaneData::U32(vec![v.max(0.0) as u32; len]),
        DType::I32 => PlaneData::I32(vec![v as i32; len]),
        DType::F32 => PlaneData::F32(vec![v as f32; len]),
        DType::F64 => PlaneData::F64(vec![v; len]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdoc_piece_coordinates() {
        let text = "PixelSpacing = 14.46\nMontage = 1\n\n[ZValue = 1]\nPieceCoordinates = 2870 0 0\nTiltAngle = 0\n\n[ZValue = 0]\nPieceCoordinates = 0 0 0\n\n[MontSection = 0]\nPieceCoordinates = 9 9 9\n";
        assert_eq!(from_mdoc(text, 2), Some(vec![[0, 0, 0], [2870, 0, 0]]));
        // Not a montage, or a section without coordinates.
        assert_eq!(from_mdoc(&text.replace("Montage = 1", "Montage = 0"), 2), None);
        assert_eq!(from_mdoc(text, 3), None);
    }

    #[test]
    fn overlaps_split_in_the_middle() {
        // 2×2 grid of 100×100 pieces, 80 apart: 20-pixel overlaps.
        let coords = [[0, 0, 0], [80, 0, 0], [0, 80, 0], [80, 80, 0]];
        let (planes, w, h) = layout(&coords, 100, 100).unwrap();
        assert_eq!((planes.len(), w, h), (1, 180, 180));
        let own: Vec<[usize; 4]> = planes[0].iter().map(|p| p.own).collect();
        assert_eq!(own, [[0, 90, 0, 90], [90, 180, 0, 90], [0, 90, 90, 180], [90, 180, 90, 180]]);
    }

    #[test]
    fn not_a_montage() {
        assert!(layout(&[[0, 0, 0]], 10, 10).is_none());
        assert!(layout(&[[0, 0, 0], [0, 0, 1]], 10, 10).is_none());
        assert!(layout(&[[0, 0, 0], [-5, 0, 0]], 10, 10).is_none());
    }

    #[test]
    fn montage_sections_become_planes() {
        let coords = [[0, 0, 0], [8, 0, 0], [0, 0, 1], [8, 0, 1]];
        let (planes, w, h) = layout(&coords, 10, 10).unwrap();
        assert_eq!((planes.len(), w, h), (2, 18, 10));
        assert_eq!(planes[1].iter().map(|p| p.section).collect::<Vec<_>>(), [2, 3]);
    }

    /// A 2×1 montage written as an MRC with the coordinates in a SerialEM
    /// extended header, assembled and as the piece stack.
    #[test]
    fn assembles_from_extended_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.mrc");
        let (pw, ph) = (4usize, 2usize);
        let mut b = vec![0u8; 1024];
        for (i, v) in [pw as i32, ph as i32, 2, 1].iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[84..88].copy_from_slice(&5f32.to_le_bytes());
        b[92..96].copy_from_slice(&16i32.to_le_bytes());
        b[104..108].copy_from_slice(b"SERI");
        b[128..130].copy_from_slice(&8i16.to_le_bytes());
        b[130..132].copy_from_slice(&3i16.to_le_bytes());
        b[212] = 0x44;
        for (x, y) in [(0u16, 0u16), (3, 1)] {
            b.extend_from_slice(&0i16.to_le_bytes());
            for v in [x, y, 0] {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        for v in [1i16, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(&path, &b).unwrap();

        let src = open(&path, 0).unwrap();
        let info = src.info();
        assert_eq!((info.width, info.height, info.size_z, info.series.len()), (7, 3, 1, 2));
        let p = src.read_page(0).unwrap();
        let v: Vec<f64> = (0..21).map(|i| p[0].data.get_f64(i)).collect();
        // The overlap (x 3) splits at x 3: the first piece keeps x < 3.
        #[rustfmt::skip]
        assert_eq!(v, [1., 1., 1., 5., 5., 5., 5.,
                       1., 1., 1., 2., 2., 2., 2.,
                       5., 5., 5., 2., 2., 2., 2.]);
        assert_eq!(src.progress(), None);

        // Preview at most 4 pixels a side: every other pixel.
        let small = open_preview(&path, 4).unwrap();
        assert_eq!((small.info().width, small.info().height), (4, 2));
        let p = small.read_page(0).unwrap();
        let v: Vec<f64> = (0..8).map(|i| p[0].data.get_f64(i)).collect();
        assert_eq!(v, [1., 1., 5., 5., 5., 5., 2., 2.]);

        let stack = open(&path, 1).unwrap();
        assert_eq!((stack.info().width, stack.info().size_z, stack.info().series_index), (4, 2, 1));
        assert!(open(&path, 2).is_err());
    }
}
