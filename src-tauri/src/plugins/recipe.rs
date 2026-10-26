//! Page recipes: how a format plugin tells the host to build a page.
//!
//! A plugin answers `tmv_page` with a list of chunks, each a byte range of
//! the file (or of the recipe itself) plus its compression and the rectangle
//! of the page it fills. The host reads the memory-mapped file, decompresses
//! the chunks in parallel and copies the pixels into the planes, so a
//! plugin-provided format decodes at native speed. The binary layout is
//! specified in `docs/format-plugins.md`.

use std::borrow::Cow;
use std::io::Read;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use crate::image::{DType, ImageInfo, Plane, PlaneData};

pub const MAGIC: &[u8; 4] = b"TMVR";
const HEADER: usize = 16;
const CHUNK: usize = 48;
/// Chunk dtype code meaning "the page's dtype".
const DTYPE_OF_PAGE: u8 = 0xff;

pub mod flags {
    pub const BIG_ENDIAN: u8 = 1;
    pub const HILO: u8 = 2;
    pub const BGR: u8 = 4;
    pub const PREDICTOR: u8 = 8;
    pub const ALL: u8 = BIG_ENDIAN | HILO | BGR | PREDICTOR;
}

/// Compression codes shared by recipes and the `decompress` import.
pub mod codec {
    pub const RAW: u8 = 0;
    pub const ZLIB: u8 = 1;
    pub const DEFLATE: u8 = 2;
    pub const LZW: u8 = 3;
    pub const ZSTD: u8 = 4;
    pub const JPEG: u8 = 5;
    pub const PACKBITS: u8 = 6;
}

pub fn dtype_code(d: DType) -> u8 {
    match d {
        DType::U8 => 0,
        DType::I8 => 1,
        DType::U16 => 2,
        DType::I16 => 3,
        DType::U32 => 4,
        DType::I32 => 5,
        DType::F32 => 6,
        DType::F64 => 7,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub inline: bool,
    pub codec: u8,
    pub dtype: u8,
    pub flags: u8,
    pub samples: usize,
    pub offset: u64,
    pub length: u64,
    pub rect: [usize; 4],
    pub plane: usize,
    pub stride: usize,
}

#[derive(Debug)]
pub struct Recipe {
    pub chunks: Vec<Chunk>,
    pub payload: Vec<u8>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Parses the binary recipe a plugin returned.
pub fn parse(mut bytes: Vec<u8>) -> Result<Recipe> {
    if bytes.len() < HEADER || &bytes[..4] != MAGIC {
        bail!("插件返回的页面描述格式不对");
    }
    if u32_at(&bytes, 4) != 1 {
        bail!("插件返回的页面描述版本不支持（{}）", u32_at(&bytes, 4));
    }
    let n = u32_at(&bytes, 8) as usize;
    let table = n.checked_mul(CHUNK).and_then(|t| t.checked_add(HEADER)).filter(|&t| t <= bytes.len());
    let table = table.context("插件返回的页面描述被截断")?;
    let mut chunks = Vec::with_capacity(n);
    for i in 0..n {
        let c = &bytes[HEADER + i * CHUNK..HEADER + (i + 1) * CHUNK];
        let rect = [u32_at(c, 24), u32_at(c, 28), u32_at(c, 32), u32_at(c, 36)].map(|v| v as usize);
        let chunk = Chunk {
            inline: match c[0] {
                0 => false,
                1 => true,
                s => bail!("页面描述里的数据来源 {s} 不支持"),
            },
            codec: c[1],
            dtype: c[2],
            flags: c[3],
            samples: u32_at(c, 4) as usize,
            offset: u64_at(c, 8),
            length: u64_at(c, 16),
            rect,
            plane: u32_at(c, 40) as usize,
            stride: u32_at(c, 44) as usize,
        };
        if chunk.codec > codec::PACKBITS {
            bail!("页面描述里的压缩方式 {} 不支持", chunk.codec);
        }
        if chunk.flags & !flags::ALL != 0 {
            bail!("页面描述里有未知的标志 {:#x}", chunk.flags);
        }
        if chunk.samples == 0 || chunk.samples > 64 {
            bail!("页面描述里的样本数不对（{}）", chunk.samples);
        }
        chunks.push(chunk);
    }
    let payload = bytes.split_off(table);
    Ok(Recipe { chunks, payload })
}

/// Decompresses `src`, producing at most `max_out` bytes.
pub fn decompress(codec: u8, src: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let limit = max_out as u64;
    let mut out = Vec::with_capacity(max_out.min(256 << 20));
    match codec {
        codec::RAW => out.extend_from_slice(&src[..src.len().min(max_out)]),
        codec::ZLIB => {
            flate2::read::ZlibDecoder::new(src).take(limit).read_to_end(&mut out)?;
        }
        codec::DEFLATE => {
            flate2::read::DeflateDecoder::new(src).take(limit).read_to_end(&mut out)?;
        }
        codec::LZW => {
            let mut d = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
            let v = d.decode(src).or_else(|_| weezl::decode::Decoder::new(weezl::BitOrder::Msb, 8).decode(src))?;
            out = v;
            out.truncate(max_out);
        }
        codec::ZSTD => {
            let dec = ruzstd::decoding::StreamingDecoder::new(src).map_err(|e| anyhow::anyhow!("zstd: {e}"))?;
            dec.take(limit).read_to_end(&mut out)?;
        }
        codec::PACKBITS => {
            let mut i = 0;
            while i < src.len() && out.len() < max_out {
                let c = src[i] as i8;
                i += 1;
                if c >= 0 {
                    let n = (c as usize + 1).min(src.len() - i).min(max_out - out.len());
                    out.extend_from_slice(&src[i..i + n]);
                    i += c as usize + 1;
                } else if c != -128 && i < src.len() {
                    let n = ((1 - c as isize) as usize).min(max_out - out.len());
                    out.extend(std::iter::repeat_n(src[i], n));
                    i += 1;
                }
            }
        }
        c => bail!("不支持的压缩方式 {c}"),
    }
    Ok(out)
}

/// The decoded bytes of one chunk: `rows` rows of `stride` bytes.
struct Decoded<'a> {
    bytes: Cow<'a, [u8]>,
    stride: usize,
}

fn decode_chunk<'a>(c: &Chunk, i: usize, file: &'a [u8], payload: &'a [u8], size: usize) -> Result<Decoded<'a>> {
    let [_, _, w, h] = c.rect;
    let row = w * c.samples * size;
    let stride = if c.stride == 0 { row } else { c.stride };
    if stride < row {
        bail!("第 {} 块的行宽不对", i + 1);
    }
    let want = stride.checked_mul(h).context("图像块太大")?;
    let src_all = if c.inline { payload } else { file };
    let src = usize::try_from(c.offset)
        .ok()
        .zip(usize::try_from(c.length).ok())
        .and_then(|(o, l)| src_all.get(o..o.checked_add(l)?))
        .with_context(|| format!("第 {} 块的数据位置超出了文件", i + 1))?;
    let mut bytes: Cow<[u8]> = match c.codec {
        codec::RAW => Cow::Borrowed(src),
        codec::JPEG => {
            if size != 1 || !(c.samples == 1 || c.samples == 3) {
                bail!("JPEG 块只能是 8 位灰度或 RGB");
            }
            let img = image::load_from_memory_with_format(src, image::ImageFormat::Jpeg)?;
            Cow::Owned(if c.samples == 1 { img.into_luma8().into_raw() } else { img.into_rgb8().into_raw() })
        }
        codec => Cow::Owned(decompress(codec, src, want)?),
    };
    if c.codec == codec::JPEG && stride != row {
        bail!("JPEG 块不能指定行宽");
    }
    if bytes.len() < want.saturating_sub(stride - row) {
        bail!("图像数据不完整（第 {} 块）", i + 1);
    }
    if c.flags & flags::HILO != 0 {
        if size != 2 {
            bail!("hi/lo 排列只适用于 16 位数据");
        }
        let v = &bytes[..want.min(bytes.len())];
        let half = v.len() / 2;
        let mut packed = vec![0u8; half * 2];
        packed.par_chunks_mut(2).enumerate().for_each(|(k, p)| {
            p[0] = v[k];
            p[1] = v[half + k];
        });
        bytes = Cow::Owned(packed);
    }
    if c.flags & flags::PREDICTOR != 0 {
        let mut v = bytes.into_owned();
        undo_predictor(&mut v, stride, w, c.samples, size, c.flags & flags::BIG_ENDIAN != 0)?;
        bytes = Cow::Owned(v);
    }
    Ok(Decoded { bytes, stride })
}

/// Undoes TIFF horizontal differencing (predictor 2) in place.
fn undo_predictor(v: &mut [u8], stride: usize, w: usize, spp: usize, size: usize, big: bool) -> Result<()> {
    macro_rules! run {
        ($t:ty) => {
            v.par_chunks_mut(stride).for_each(|row| {
                let n = (w * spp).min(row.len() / size);
                let get = |r: &[u8], i: usize| {
                    let b: [u8; size_of::<$t>()] = r[i * size..i * size + size].try_into().unwrap();
                    if big { <$t>::from_be_bytes(b) } else { <$t>::from_le_bytes(b) }
                };
                for i in spp..n {
                    let s = get(row, i).wrapping_add(get(row, i - spp));
                    let b = if big { s.to_be_bytes() } else { s.to_le_bytes() };
                    row[i * size..i * size + size].copy_from_slice(&b);
                }
            })
        };
    }
    match size {
        1 => run!(u8),
        2 => run!(u16),
        4 => run!(u32),
        8 => run!(u64),
        _ => bail!("预测器不支持这种数据类型"),
    }
    Ok(())
}

/// Copies one sample of `src` rows into the bytes of a plane.
#[allow(clippy::too_many_arguments)]
fn place(
    dst: &mut [u8],
    page_w: usize,
    src: &Decoded,
    rows: usize,
    cols: usize,
    x: usize,
    y: usize,
    spp: usize,
    sample: usize,
    size: usize,
    swap: bool,
) {
    let line = page_w * size;
    let work = |(r, out): (usize, &mut [u8])| {
        let s = &src.bytes[r * src.stride..];
        let out = &mut out[x * size..(x + cols) * size];
        if spp == 1 {
            out.copy_from_slice(&s[..cols * size]);
        } else {
            let px = spp * size;
            for (o, i) in out.chunks_exact_mut(size).zip(s.chunks_exact(px)) {
                o.copy_from_slice(&i[sample * size..sample * size + size]);
            }
        }
        if swap {
            for o in out.chunks_exact_mut(size) {
                o.reverse();
            }
        }
    };
    let dst = &mut dst[y * line..(y + rows) * line];
    // Small tiles are not worth splitting across threads.
    if rows * cols * size >= 1 << 16 {
        dst.par_chunks_mut(line).enumerate().for_each(work);
    } else {
        dst.chunks_mut(line).enumerate().for_each(work);
    }
}

/// Builds the planes of one page from `recipe`, reading `file`.
pub fn execute(recipe: &Recipe, file: &[u8], info: &ImageInfo) -> Result<Vec<Plane>> {
    let (w, h) = (info.width, info.height);
    let planes_n = info.samples_per_page.max(1);
    let size = info.dtype.size();
    let page_code = dtype_code(info.dtype);
    for (i, c) in recipe.chunks.iter().enumerate() {
        if c.dtype != DTYPE_OF_PAGE && c.dtype != page_code {
            bail!("第 {} 块的数据类型和图像不一致", i + 1);
        }
    }
    // Chunks entirely off the page are ignored; the rest are clipped.
    let visible: Vec<(usize, &Chunk)> = recipe
        .chunks
        .iter()
        .enumerate()
        .filter(|(_, c)| c.rect[0] < w && c.rect[1] < h && c.rect[2] > 0 && c.rect[3] > 0)
        .collect();
    let decoded: Vec<Decoded> =
        visible.par_iter().map(|&(i, c)| decode_chunk(c, i, file, &recipe.payload, size)).collect::<Result<_>>()?;

    let target_of = |c: &Chunk, s: usize| if c.flags & flags::BGR != 0 && s < 3 { c.plane + 2 - s } else { c.plane + s };

    // One chunk filling the whole page (the usual case): convert straight
    // from the decoded bytes, without first zeroing the planes.
    if let ([(_, c)], [d]) = (visible.as_slice(), decoded.as_slice()) {
        let row = w * c.samples * size;
        if c.rect == [0, 0, w, h] && d.stride == row && c.flags & !(flags::BIG_ENDIAN | flags::BGR) == 0 {
            let little = c.flags & flags::BIG_ENDIAN == 0;
            let split = crate::formats::raw_to_plane(&d.bytes[..row * h], info.dtype, little).deinterleave(c.samples);
            let mut planes: Vec<Option<PlaneData>> = (0..planes_n).map(|_| None).collect();
            for (s, p) in split.into_iter().enumerate() {
                if let Some(slot) = planes.get_mut(target_of(c, s)) {
                    *slot = Some(p);
                }
            }
            return Ok(planes
                .into_iter()
                .map(|p| Plane { w, h, data: p.unwrap_or_else(|| PlaneData::zeroed(info.dtype, w * h)) })
                .collect());
        }
    }

    let mut planes: Vec<PlaneData> = (0..planes_n).map(|_| PlaneData::zeroed(info.dtype, w * h)).collect();
    for (&(_, c), d) in visible.iter().zip(&decoded) {
        let [x, y, cw, ch] = c.rect;
        // decode_chunk checked that all `ch` rows are there.
        let rows = ch.min(h - y);
        let cols = cw.min(w - x);
        let swap = (c.flags & flags::BIG_ENDIAN != 0) != cfg!(target_endian = "big");
        for s in 0..c.samples {
            let target = target_of(c, s);
            if target >= planes_n {
                continue;
            }
            place(planes[target].as_bytes_mut(), w, d, rows, cols, x, y, c.samples, s, size, swap);
        }
    }
    Ok(planes.into_iter().map(|data| Plane { w, h, data }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::tests::fake_info;

    /// Writes a recipe the way the SDK does.
    pub(crate) fn build(chunks: &[Chunk], payload: &[u8]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for c in chunks {
            out.extend_from_slice(&[c.inline as u8, c.codec, c.dtype, c.flags]);
            out.extend_from_slice(&(c.samples as u32).to_le_bytes());
            out.extend_from_slice(&c.offset.to_le_bytes());
            out.extend_from_slice(&c.length.to_le_bytes());
            for v in c.rect {
                out.extend_from_slice(&(v as u32).to_le_bytes());
            }
            out.extend_from_slice(&(c.plane as u32).to_le_bytes());
            out.extend_from_slice(&(c.stride as u32).to_le_bytes());
        }
        out.extend_from_slice(payload);
        out
    }

    fn chunk(offset: u64, length: u64, rect: [usize; 4]) -> Chunk {
        Chunk { inline: false, codec: 0, dtype: DTYPE_OF_PAGE, flags: 0, samples: 1, offset, length, rect, plane: 0, stride: 0 }
    }

    fn run(info: &ImageInfo, chunks: &[Chunk], file: &[u8], payload: &[u8]) -> Result<Vec<Plane>> {
        execute(&parse(build(chunks, payload))?, file, info)
    }

    fn values(p: &Plane) -> Vec<f64> {
        (0..p.data.len()).map(|i| p.data.get_f64(i)).collect()
    }

    #[test]
    fn parse_roundtrip_and_rejects_garbage() {
        let c = Chunk { inline: true, codec: codec::ZSTD, dtype: 2, flags: flags::HILO, samples: 3, offset: 5, length: 9, rect: [1, 2, 3, 4], plane: 1, stride: 20 };
        let r = parse(build(std::slice::from_ref(&c), b"xyz")).unwrap();
        assert_eq!(r.chunks, std::slice::from_ref(&c));
        assert_eq!(r.payload, b"xyz");
        assert!(parse(b"nope".to_vec()).is_err());
        let mut bad = build(std::slice::from_ref(&c), b"");
        bad.truncate(40);
        assert!(parse(bad).is_err());
        let mut bad = c.clone();
        bad.codec = 99;
        assert!(parse(build(&[bad], b"")).is_err());
        let mut bad = c;
        bad.samples = 0;
        assert!(parse(build(&[bad], b"")).is_err());
    }

    #[test]
    fn tiles_with_clipping_and_overlap() {
        let info = fake_info(4, 3, 1, 1, 1, DType::U16);
        // File: 2x2 tile of 1..4 at offset 0, 3x3 tile of 9s at offset 8.
        let mut file: Vec<u8> = [1u16, 2, 3, 4].iter().flat_map(|v| v.to_le_bytes()).collect();
        file.extend([9u16; 9].iter().flat_map(|v| v.to_le_bytes()));
        let planes =
            run(&info, &[chunk(0, 8, [0, 0, 2, 2]), chunk(8, 18, [2, 1, 3, 3]), chunk(0, 8, [9, 9, 2, 2])], &file, b"").unwrap();
        assert_eq!(planes.len(), 1);
        #[rustfmt::skip]
        assert_eq!(values(&planes[0]), [
            1.0, 2.0, 0.0, 0.0,
            3.0, 4.0, 9.0, 9.0,
            0.0, 0.0, 9.0, 9.0,
        ]);
    }

    #[test]
    fn interleaved_bgr_big_endian_and_inline() {
        let mut info = fake_info(2, 1, 3, 1, 1, DType::U16);
        info.samples_per_page = 3;
        // BGRA, big-endian, inline payload; alpha is dropped.
        let px: Vec<u8> = [10u16, 20, 30, 99, 11, 21, 31, 99].iter().flat_map(|v| v.to_be_bytes()).collect();
        let mut c = chunk(0, px.len() as u64, [0, 0, 2, 1]);
        c.inline = true;
        c.samples = 4;
        c.flags = flags::BGR | flags::BIG_ENDIAN;
        let planes = run(&info, &[c], b"", &px).unwrap();
        assert_eq!(values(&planes[0]), [30.0, 31.0]);
        assert_eq!(values(&planes[1]), [20.0, 21.0]);
        assert_eq!(values(&planes[2]), [10.0, 11.0]);
    }

    #[test]
    fn codecs_hilo_and_predictor() {
        let info = fake_info(3, 2, 1, 1, 1, DType::U16);
        let vals = [1u16, 258, 3, 65535, 0, 7];
        let raw: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
        let want: Vec<f64> = vals.iter().map(|&v| v as f64).collect();

        let z = ruzstd::encoding::compress_to_vec(&raw[..], ruzstd::encoding::CompressionLevel::Fastest);
        let mut c = chunk(0, z.len() as u64, [0, 0, 3, 2]);
        c.codec = codec::ZSTD;
        assert_eq!(values(&run(&info, &[c.clone()], &z, b"").unwrap()[0]), want);

        // hi/lo: all low bytes, then all high bytes.
        let mut hl: Vec<u8> = vals.iter().map(|v| *v as u8).collect();
        hl.extend(vals.iter().map(|v| (v >> 8) as u8));
        let z = ruzstd::encoding::compress_to_vec(&hl[..], ruzstd::encoding::CompressionLevel::Fastest);
        c.length = z.len() as u64;
        c.flags = flags::HILO;
        assert_eq!(values(&run(&info, &[c], &z, b"").unwrap()[0]), want);

        let mut zl = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        std::io::Write::write_all(&mut zl, &raw).unwrap();
        let zl = zl.finish().unwrap();
        let mut c = chunk(0, zl.len() as u64, [0, 0, 3, 2]);
        c.codec = codec::ZLIB;
        assert_eq!(values(&run(&info, &[c], &zl, b"").unwrap()[0]), want);

        // Horizontal differencing per row.
        let diff: Vec<u8> = [1u16, 257, (3u16).wrapping_sub(258), 65535, 1, 7].iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut c = chunk(0, 12, [0, 0, 3, 2]);
        c.flags = flags::PREDICTOR;
        let got = values(&run(&info, &[c], &diff, b"").unwrap()[0]);
        assert_eq!(got, [1.0, 258.0, 3.0, 65535.0, 0.0, 7.0]);

        let pb = [2u8, 1, 2, 3, 0xfe, 9]; // literal 1 2 3, then 9 x3
        assert_eq!(decompress(codec::PACKBITS, &pb, 100).unwrap(), [1, 2, 3, 9, 9, 9]);
    }

    #[test]
    fn errors_instead_of_panics() {
        let info = fake_info(2, 2, 1, 1, 1, DType::U8);
        // Out of the file.
        assert!(run(&info, &[chunk(2, 4, [0, 0, 2, 2])], &[0; 4], b"").is_err());
        assert!(run(&info, &[chunk(u64::MAX, 4, [0, 0, 2, 2])], &[0; 4], b"").is_err());
        // Too short.
        assert!(run(&info, &[chunk(0, 3, [0, 0, 2, 2])], &[0; 4], b"").is_err());
        // Wrong dtype.
        let mut c = chunk(0, 4, [0, 0, 2, 2]);
        c.dtype = 2;
        assert!(run(&info, &[c], &[0; 8], b"").is_err());
        // Corrupt zstd.
        let mut c = chunk(0, 4, [0, 0, 2, 2]);
        c.codec = codec::ZSTD;
        assert!(run(&info, &[c], &[1, 2, 3, 4], b"").is_err());
        // Decompression is capped at what the chunk needs.
        let big = ruzstd::encoding::compress_to_vec(&vec![7u8; 1 << 20][..], ruzstd::encoding::CompressionLevel::Fastest);
        assert_eq!(decompress(codec::ZSTD, &big, 10).unwrap().len(), 10);
    }
}
