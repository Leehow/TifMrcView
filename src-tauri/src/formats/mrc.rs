//! MRC / MRC2014 reader (cryo-EM maps, tomograms and particle stacks).

use std::fs::File;
use std::path::Path;

use anyhow::{bail, Context, Result};
use memmap2::Mmap;

use super::raw_to_plane;
use crate::image::{DType, DimOrder, ImageInfo, Plane, PlaneData, Source};

pub struct MrcSource {
    mmap: Mmap,
    info: ImageInfo,
    data_offset: usize,
    little: bool,
    mode: i32,
}

impl MrcSource {
    pub fn open(path: &Path) -> Result<MrcSource> {
        let file = File::open(path).with_context(|| format!("无法打开 {}", path.display()))?;
        // SAFETY: read-only mapping; see TiffSource::open.
        let mmap = unsafe { Mmap::map(&file)? };
        if mmap.len() < 1024 {
            bail!("MRC 文件头不完整");
        }
        // Machine stamp at byte 212: 0x44 0x41/0x44 = little, 0x11 0x11 = big.
        let mut little = mmap[212] != 0x11;
        let rd = |off: usize, le: bool| {
            let b: [u8; 4] = mmap[off..off + 4].try_into().unwrap();
            if le { i32::from_le_bytes(b) } else { i32::from_be_bytes(b) }
        };
        // Some writers leave the stamp empty; sanity-check the mode instead.
        let plausible = |m: i32| matches!(m, 0 | 1 | 2 | 3 | 4 | 6 | 12 | 101);
        if !plausible(rd(12, little)) && plausible(rd(12, !little)) {
            little = !little;
        }
        let rf = |off: usize| {
            let b: [u8; 4] = mmap[off..off + 4].try_into().unwrap();
            if little { f32::from_le_bytes(b) } else { f32::from_be_bytes(b) }
        };
        let (nx, ny, nz, mode) = (rd(0, little), rd(4, little), rd(8, little), rd(12, little));
        let (mx, my, mz) = (rd(28, little), rd(32, little), rd(36, little));
        let (cx, cy, cz) = (rf(40), rf(44), rf(48));
        let nsymbt = rd(92, little).max(0) as usize;
        if nx <= 0 || ny <= 0 || nz <= 0 {
            bail!("MRC 尺寸无效: {nx}x{ny}x{nz}");
        }
        // IMOD flags: bit 0 set means mode-0 bytes are signed.
        let imod = rd(152, little) == 1146047817;
        let signed_bytes = mode0_signed(imod, rd(156, little), rf(76), rf(80));
        let dtype = match mode {
            0 if signed_bytes => DType::I8,
            0 => DType::U8,
            1 => DType::I16,
            2 | 12 => DType::F32,
            6 => DType::U16,
            101 => DType::U8,
            3 | 4 => DType::F32,
            _ => bail!("不支持的 MRC mode {mode}"),
        };
        let (w, h, d) = (nx as usize, ny as usize, nz as usize);
        let data_offset = 1024 + nsymbt;
        let plane_bytes = mrc_plane_bytes(mode, w, h);
        let available = mmap.len().saturating_sub(data_offset) / plane_bytes.max(1);
        if available == 0 {
            bail!("MRC 数据被截断");
        }
        let depth = d.min(available);
        let px = |c: f32, m: i32| if m > 0 && c > 0.0 { c as f64 / m as f64 } else { 0.0 };
        let pixel_size = (cx > 0.0).then(|| [px(cx, mx), px(cy, my), px(cz, mz)]);
        let mut info = ImageInfo {
            path: path.to_string_lossy().into_owned(),
            name: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            format: format!("MRC (mode {mode})"),
            width: w,
            height: h,
            size_c: 1,
            size_z: depth,
            size_t: 1,
            dtype,
            is_rgb: false,
            channels: vec![],
            pixel_size,
            unit: "Å".into(),
            metadata: vec![],
            samples_per_page: 1,
            order: DimOrder::CZT,
            page_count: depth,
        };
        info.fill_default_channels();
        Ok(MrcSource { mmap, info, data_offset, little, mode })
    }
}

/// Whether mode-0 bytes are signed. The spec says signed, but IMOD marks it
/// with a flag, and older files without the stamp are often unsigned: for
/// those, header DMIN/DMAX above the int8 range prove the data is unsigned.
fn mode0_signed(imod: bool, flags: i32, dmin: f32, dmax: f32) -> bool {
    if imod {
        return flags & 1 == 1;
    }
    let usable = dmin.is_finite() && dmax.is_finite() && dmax >= dmin;
    !(usable && dmin >= 0.0 && dmax > 127.0 && dmax <= 255.0)
}

fn mrc_plane_bytes(mode: i32, w: usize, h: usize) -> usize {
    match mode {
        0 => w * h,
        1 | 6 | 12 => w * h * 2,
        2 => w * h * 4,
        3 => w * h * 4,
        4 => w * h * 8,
        101 => w.div_ceil(2) * h,
        _ => 0,
    }
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1f) as u32;
    let frac = (h & 0x3ff) as u32;
    let bits = match exp {
        0 if frac == 0 => sign,
        0 => {
            // Subnormal: renormalise.
            let mut e = 127 - 15 + 1;
            let mut f = frac;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((f & 0x3ff) << 13)
        }
        0x1f => sign | 0x7f80_0000 | (frac << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (frac << 13),
    };
    f32::from_bits(bits)
}

impl Source for MrcSource {
    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        use rayon::prelude::*;
        let (w, h) = (self.info.width, self.info.height);
        let n = mrc_plane_bytes(self.mode, w, h);
        let start = self.data_offset + page.min(self.info.page_count - 1) * n;
        let bytes = &self.mmap[start..start + n];
        let little = self.little;
        let data = match self.mode {
            12 => PlaneData::F32(
                bytes
                    .par_chunks_exact(2)
                    .map(|b| {
                        let a = [b[0], b[1]];
                        f16_to_f32(if little { u16::from_le_bytes(a) } else { u16::from_be_bytes(a) })
                    })
                    .collect(),
            ),
            3 | 4 => {
                // Complex data: show the amplitude.
                let (sz, half) = if self.mode == 3 { (4, 2) } else { (8, 4) };
                PlaneData::F32(
                    bytes
                        .par_chunks_exact(sz)
                        .map(|b| {
                            let (re, im) = if half == 2 {
                                let f = |s: &[u8]| {
                                    let a = [s[0], s[1]];
                                    (if little { i16::from_le_bytes(a) } else { i16::from_be_bytes(a) }) as f32
                                };
                                (f(&b[..2]), f(&b[2..]))
                            } else {
                                let f = |s: &[u8]| {
                                    let a: [u8; 4] = s.try_into().unwrap();
                                    if little { f32::from_le_bytes(a) } else { f32::from_be_bytes(a) }
                                };
                                (f(&b[..4]), f(&b[4..]))
                            };
                            (re * re + im * im).sqrt()
                        })
                        .collect(),
                )
            }
            101 => {
                let row = w.div_ceil(2);
                let mut v = vec![0u8; w * h];
                v.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
                    let src = &bytes[y * row..(y + 1) * row];
                    for (x, o) in out.iter_mut().enumerate() {
                        let b = src[x / 2];
                        *o = if x % 2 == 0 { b & 0x0f } else { b >> 4 };
                    }
                });
                PlaneData::U8(v)
            }
            _ => raw_to_plane(bytes, self.info.dtype, little),
        };
        Ok(vec![Plane { w, h, data }])
    }

    fn raw_page(&self, page: usize) -> Option<(&[u8], bool)> {
        if !matches!(self.mode, 0 | 1 | 2 | 6) {
            return None;
        }
        let n = mrc_plane_bytes(self.mode, self.info.width, self.info.height);
        let start = self.data_offset + page.min(self.info.page_count - 1) * n;
        Some((&self.mmap[start..start + n], self.little))
    }
}

#[cfg(test)]
mod format_fix_tests {
    use super::*;

    #[test]
    fn mode0_signedness() {
        // IMOD stamp: flag decides, stats ignored.
        assert!(mode0_signed(true, 1, 0.0, 200.0));
        assert!(!mode0_signed(true, 0, -5.0, 100.0));
        // No stamp: unsigned only when stats prove it.
        assert!(!mode0_signed(false, 0, 0.0, 200.0));
        assert!(!mode0_signed(false, 0, 3.0, 255.0));
        assert!(mode0_signed(false, 0, -100.0, 100.0));
        assert!(mode0_signed(false, 0, 0.0, 100.0));
        assert!(mode0_signed(false, 0, 0.0, 300.0));
        // Unusable stats keep the spec default.
        assert!(mode0_signed(false, 0, 0.0, 0.0));
        assert!(mode0_signed(false, 0, 200.0, 10.0));
        assert!(mode0_signed(false, 0, f32::NAN, 200.0));
        assert!(mode0_signed(false, 0, 0.0, f32::INFINITY));
    }

    fn header(dmin: f32, dmax: f32, imod: bool) -> Vec<u8> {
        let mut h = vec![0u8; 1024 + 4];
        h[0..4].copy_from_slice(&2i32.to_le_bytes());
        h[4..8].copy_from_slice(&1i32.to_le_bytes());
        h[8..12].copy_from_slice(&1i32.to_le_bytes());
        h[76..80].copy_from_slice(&dmin.to_le_bytes());
        h[80..84].copy_from_slice(&dmax.to_le_bytes());
        h[212] = 0x44;
        if imod {
            h[152..156].copy_from_slice(&1146047817i32.to_le_bytes());
        }
        h[1024..].copy_from_slice(&[200, 10, 0, 0]);
        h
    }

    fn open_dtype(name: &str, bytes: &[u8]) -> (DType, PlaneData) {
        let dir = std::env::temp_dir().join(format!("tmv_mrcfix_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.mrc");
        std::fs::write(&p, bytes).unwrap();
        let s = MrcSource::open(&p).unwrap();
        let d = s.read_page(0).unwrap().remove(0).data;
        let t = s.info().dtype;
        let _ = std::fs::remove_dir_all(&dir);
        (t, d)
    }

    #[test]
    fn open_uses_header_stats() {
        let (t, d) = open_dtype("u", &header(0.0, 200.0, false));
        assert_eq!(t, DType::U8);
        assert!(matches!(d, PlaneData::U8(ref v) if v[0] == 200));
        let (t, _) = open_dtype("s", &header(0.0, 0.0, false));
        assert_eq!(t, DType::I8);
        // IMOD stamp without the flag: unsigned, as before.
        let (t, _) = open_dtype("i", &header(-5.0, 100.0, true));
        assert_eq!(t, DType::U8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats;

    #[test]
    fn f16_special_values() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert!(f16_to_f32(0x0000).is_sign_positive());
        assert_eq!(f16_to_f32(0x8000), 0.0);
        assert!(f16_to_f32(0x8000).is_sign_negative());
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert!((f16_to_f32(0x3555) - 1.0 / 3.0).abs() < 1e-3);
        assert_eq!(f16_to_f32(0x7BFF), 65504.0);
        assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
        assert_eq!(f16_to_f32(0xFC00), f32::NEG_INFINITY);
        assert!(f16_to_f32(0x7E00).is_nan());
        assert!(f16_to_f32(0xFFFF).is_nan());
    }

    #[test]
    fn f16_subnormals() {
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(f16_to_f32(0x0200), 2f32.powi(-15));
        assert_eq!(f16_to_f32(0x03FF), 1023.0 * 2f32.powi(-24));
        assert_eq!(f16_to_f32(0x8001), -(2f32.powi(-24)));
        // Smallest normal.
        assert_eq!(f16_to_f32(0x0400), 2f32.powi(-14));
    }

    #[test]
    fn plane_bytes_per_mode() {
        assert_eq!(mrc_plane_bytes(0, 5, 3), 15);
        assert_eq!(mrc_plane_bytes(1, 5, 3), 30);
        assert_eq!(mrc_plane_bytes(6, 5, 3), 30);
        assert_eq!(mrc_plane_bytes(12, 5, 3), 30);
        assert_eq!(mrc_plane_bytes(2, 5, 3), 60);
        assert_eq!(mrc_plane_bytes(3, 5, 3), 60);
        assert_eq!(mrc_plane_bytes(4, 5, 3), 120);
        // 4-bit packed: two pixels per byte, rows padded to whole bytes.
        assert_eq!(mrc_plane_bytes(101, 5, 3), 9);
        assert_eq!(mrc_plane_bytes(101, 4, 3), 6);
        assert_eq!(mrc_plane_bytes(5, 5, 3), 0);
    }

    // ---- end-to-end -------------------------------------------------------

    fn put_i32(h: &mut [u8], off: usize, v: i32, le: bool) {
        let b = if le { v.to_le_bytes() } else { v.to_be_bytes() };
        h[off..off + 4].copy_from_slice(&b);
    }

    fn put_f32(h: &mut [u8], off: usize, v: f32, le: bool) {
        let b = if le { v.to_le_bytes() } else { v.to_be_bytes() };
        h[off..off + 4].copy_from_slice(&b);
    }

    /// A 1024-byte header with sampling mx,my,mz = nx,ny,nz and the given cell.
    fn header(dims: [i32; 3], mode: i32, le: bool, cell: [f32; 3]) -> Vec<u8> {
        let mut h = vec![0u8; 1024];
        for (i, d) in dims.iter().enumerate() {
            put_i32(&mut h, 4 * i, *d, le);
            put_i32(&mut h, 28 + 4 * i, *d, le);
            put_f32(&mut h, 40 + 4 * i, cell[i], le);
        }
        put_i32(&mut h, 12, mode, le);
        h[212] = if le { 0x44 } else { 0x11 };
        h[213] = if le { 0x44 } else { 0x11 };
        h
    }

    fn f32_bytes(v: &[f32], le: bool) -> Vec<u8> {
        v.iter().flat_map(|x| if le { x.to_le_bytes() } else { x.to_be_bytes() }).collect()
    }

    fn i16_bytes(v: &[i16], le: bool) -> Vec<u8> {
        v.iter().flat_map(|x| if le { x.to_le_bytes() } else { x.to_be_bytes() }).collect()
    }

    fn u16_bytes(v: &[u16], le: bool) -> Vec<u8> {
        v.iter().flat_map(|x| if le { x.to_le_bytes() } else { x.to_be_bytes() }).collect()
    }

    fn write(dir: &tempfile::TempDir, name: &str, header: Vec<u8>, data: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let mut bytes = header;
        bytes.extend_from_slice(data);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn values(src: &dyn Source, page: usize) -> Vec<f64> {
        let planes = src.read_page(page).unwrap();
        assert_eq!(planes.len(), 1);
        let p = &planes[0].data;
        (0..p.len()).map(|i| p.get_f64(i)).collect()
    }

    fn open(path: &Path) -> Box<dyn Source> {
        formats::open(path).unwrap()
    }

    #[test]
    fn mode2_float_both_endians() {
        let vals: Vec<f32> = (0..12).map(|i| i as f32 * 0.5 - 1.0).collect();
        for le in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let h = header([3, 2, 2], 2, le, [6.0, 5.0, 8.0]);
            let path = write(&dir, "a.mrc", h, &f32_bytes(&vals, le));
            let src = open(&path);
            let info = src.info();
            assert_eq!((info.width, info.height, info.size_z, info.page_count), (3, 2, 2, 2));
            assert_eq!((info.size_c, info.size_t), (1, 1));
            assert_eq!(info.dtype, DType::F32);
            assert_eq!(info.format, "MRC (mode 2)");
            assert_eq!(info.unit, "Å");
            assert_eq!(info.pixel_size, Some([2.0, 2.5, 4.0]));
            assert_eq!(values(&*src, 0), vals[..6].iter().map(|&v| v as f64).collect::<Vec<_>>());
            assert_eq!(values(&*src, 1), vals[6..].iter().map(|&v| v as f64).collect::<Vec<_>>());
        }
    }

    #[test]
    fn mode1_signed_16_bit() {
        for le in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = write(&dir, "a.mrc", header([2, 2, 1], 1, le, [0.0; 3]), &i16_bytes(&[-32768, -1, 0, 32767], le));
            let src = open(&path);
            assert_eq!(src.info().dtype, DType::I16);
            assert_eq!(src.info().pixel_size, None);
            assert_eq!(values(&*src, 0), [-32768.0, -1.0, 0.0, 32767.0]);
        }
    }

    #[test]
    fn mode6_unsigned_16_bit() {
        for le in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = write(&dir, "a.mrc", header([2, 1, 1], 6, le, [0.0; 3]), &u16_bytes(&[0, 65535], le));
            let src = open(&path);
            assert_eq!(src.info().dtype, DType::U16);
            assert_eq!(values(&*src, 0), [0.0, 65535.0]);
        }
    }

    #[test]
    fn mode0_is_signed_without_imod_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "a.mrc", header([4, 1, 1], 0, true, [0.0; 3]), &[0, 127, 128, 255]);
        let src = open(&path);
        assert_eq!(src.info().dtype, DType::I8);
        assert_eq!(values(&*src, 0), [0.0, 127.0, -128.0, -1.0]);
    }

    #[test]
    fn mode0_imod_flag_selects_unsigned() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = header([2, 1, 1], 0, true, [0.0; 3]);
        put_i32(&mut h, 152, 1146047817, true); // "IMOD"
        put_i32(&mut h, 156, 0, true); // flags: bit 0 clear -> unsigned
        let path = write(&dir, "u.mrc", h, &[200, 10]);
        let src = open(&path);
        assert_eq!(src.info().dtype, DType::U8);
        assert_eq!(values(&*src, 0), [200.0, 10.0]);

        let mut h = header([2, 1, 1], 0, true, [0.0; 3]);
        put_i32(&mut h, 152, 1146047817, true);
        put_i32(&mut h, 156, 1, true); // bit 0 set -> signed
        let path = write(&dir, "s.mrc", h, &[200, 10]);
        let src = open(&path);
        assert_eq!(src.info().dtype, DType::I8);
        assert_eq!(values(&*src, 0), [-56.0, 10.0]);
    }

    #[test]
    fn mode12_half_float() {
        for le in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            // 1.0, -2.0, 0.0, 65504
            let path = write(&dir, "a.mrc", header([2, 2, 1], 12, le, [0.0; 3]), &u16_bytes(&[0x3C00, 0xC000, 0, 0x7BFF], le));
            let src = open(&path);
            assert_eq!(src.info().dtype, DType::F32);
            assert_eq!(values(&*src, 0), [1.0, -2.0, 0.0, 65504.0]);
        }
    }

    #[test]
    fn mode101_four_bit_packed() {
        let dir = tempfile::tempdir().unwrap();
        // 3x2 pixels, rows padded to 2 bytes; low nibble is the even pixel.
        let path = write(&dir, "a.mrc", header([3, 2, 1], 101, true, [0.0; 3]), &[0x21, 0x03, 0x54, 0x06]);
        let src = open(&path);
        assert_eq!(src.info().dtype, DType::U8);
        assert_eq!(values(&*src, 0), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn complex_modes_show_amplitude() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c3.mrc", header([2, 1, 1], 3, true, [0.0; 3]), &i16_bytes(&[3, 4, -6, 8], true));
        assert_eq!(values(&*open(&path), 0), [5.0, 10.0]);
        let path = write(&dir, "c4.mrc", header([1, 1, 1], 4, false, [0.0; 3]), &f32_bytes(&[5.0, 12.0], false));
        assert_eq!(values(&*open(&path), 0), [13.0]);
    }

    #[test]
    fn extended_header_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = header([2, 1, 1], 2, true, [0.0; 3]);
        put_i32(&mut h, 92, 8, true);
        let mut data = vec![0xAA; 8];
        data.extend(f32_bytes(&[1.5, 2.5], true));
        let src = open(&write(&dir, "a.mrc", h, &data));
        assert_eq!(values(&*src, 0), [1.5, 2.5]);
    }

    #[test]
    fn missing_machine_stamp_is_detected_from_mode() {
        // Big-endian data with an empty stamp: little-endian reading gives an invalid mode.
        let dir = tempfile::tempdir().unwrap();
        let mut h = header([2, 1, 1], 2, false, [0.0; 3]);
        h[212] = 0;
        h[213] = 0;
        let src = open(&write(&dir, "a.mrc", h, &f32_bytes(&[1.0, -3.0], false)));
        assert_eq!((src.info().width, src.info().size_z), (2, 1));
        assert_eq!(values(&*src, 0), [1.0, -3.0]);
    }

    #[test]
    fn truncated_data_keeps_complete_planes() {
        let dir = tempfile::tempdir().unwrap();
        // Header promises 5 planes of 2 floats, file holds 2 and a half.
        let mut data = f32_bytes(&[1.0, 2.0, 3.0, 4.0, 5.0], true);
        data.truncate(20);
        let src = open(&write(&dir, "a.mrc", header([2, 1, 5], 2, true, [0.0; 3]), &data));
        assert_eq!(src.info().size_z, 2);
        assert_eq!(src.info().page_count, 2);
        assert_eq!(values(&*src, 1), [3.0, 4.0]);
        // Out-of-range pages clamp to the last one.
        assert_eq!(values(&*src, 9), [3.0, 4.0]);
    }

    #[test]
    fn invalid_files_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let short = dir.path().join("short.mrc");
        std::fs::write(&short, vec![0u8; 100]).unwrap();
        assert!(formats::open(&short).is_err());

        // No data at all.
        let p = write(&dir, "nodata.mrc", header([2, 2, 1], 2, true, [0.0; 3]), &[]);
        assert!(formats::open(&p).is_err());
        // Less than one plane.
        let p = write(&dir, "partial.mrc", header([2, 2, 1], 2, true, [0.0; 3]), &[0; 15]);
        assert!(formats::open(&p).is_err());
        // Zero and negative dimensions.
        let p = write(&dir, "zero.mrc", header([0, 2, 1], 2, true, [0.0; 3]), &[0; 64]);
        assert!(formats::open(&p).is_err());
        let p = write(&dir, "neg.mrc", header([2, -2, 1], 2, true, [0.0; 3]), &[0; 64]);
        assert!(formats::open(&p).is_err());
        // Unsupported mode.
        let p = write(&dir, "mode5.mrc", header([2, 2, 1], 5, true, [0.0; 3]), &[0; 64]);
        assert!(formats::open(&p).is_err());
    }

    #[test]
    fn pixel_size_uses_cell_over_sampling() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = header([4, 4, 2], 2, true, [10.0, 20.0, 3.0]);
        put_i32(&mut h, 28, 2, true); // mx = 2
        put_i32(&mut h, 32, 0, true); // my = 0 -> unknown
        let src = open(&write(&dir, "a.mrc", h, &[0; 4 * 4 * 2 * 4]));
        assert_eq!(src.info().pixel_size, Some([5.0, 0.0, 1.5]));
    }
}
