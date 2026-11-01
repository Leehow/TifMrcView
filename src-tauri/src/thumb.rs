//! Bounded, headless thumbnails using the viewer's display contrast.
use std::path::Path;
use std::sync::Once;

use ::image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};
use anyhow::{bail, ensure, Context, Result};

use crate::{formats, image::OpenImage, plugins, render};

const CACHE_BYTES: usize = 16 << 20;
const MAX_DECODE_BYTES: usize = 128 << 20;
pub const MAX_SIZE: u32 = 2048;
static PLUGINS: Once = Once::new();

pub fn configure_plugins(dir: Option<std::path::PathBuf>) {
    PLUGINS.call_once(|| plugins::registry::Registry::global().configure(dir, env!("CARGO_PKG_VERSION")));
}

/// Middle Z, first T, all channels. No stack prefetch or metadata plugins.
pub fn thumbnail_rgba(path: &Path, max_size: u32) -> Result<(u32, u32, Vec<u8>)> {
    ensure!(
        (1..=MAX_SIZE).contains(&max_size),
        "thumbnail size must be 1..={MAX_SIZE}"
    );
    configure_plugins(plugins::paths::user_dir());
    let src = formats::open_pixels(path)?;
    let info = src.info();
    ensure!(
        info.width > 0
            && info.height > 0
            && info.size_c > 0
            && info.size_z > 0
            && info.size_t > 0
            && info.page_count > 0,
        "empty image"
    );
    // render holds every selected channel until compositing finishes. A cache
    // budget alone cannot bound these allocations or a single huge plane.
    let decoded = info
        .width
        .checked_mul(info.height)
        .and_then(|n| n.checked_mul(info.dtype.size()))
        .and_then(|n| n.checked_mul(info.samples_per_page.max(info.size_c)))
        .context("image dimensions overflow")?;
    ensure!(
        decoded <= MAX_DECODE_BYTES,
        "thumbnail planes exceed 128 MiB decode limit"
    );
    let img = OpenImage::new(0, src, CACHE_BYTES);
    let out = render::thumbnail(&img, max_size as usize)?;
    let word = |i| u32::from_le_bytes(out[i..i + 4].try_into().unwrap());
    ensure!(out.len() >= 12, "short render header");
    let (w, h, channels) = (word(0), word(4), word(8) as usize);
    let header = 12usize
        .checked_add(channels.checked_mul(16).context("channel header overflow")?)
        .context("header overflow")?;
    let pixels = out.get(header..).context("short render payload")?;
    ensure!(pixels.len() == w as usize * h as usize * 4, "invalid RGBA payload");
    Ok((w, h, pixels.to_vec()))
}

pub fn thumbnail_png(path: &Path, max_size: u32) -> Result<Vec<u8>> {
    let (w, h, rgba) = thumbnail_rgba(path, max_size)?;
    let mut png = Vec::new();
    PngEncoder::new(&mut png).write_image(&rgba, w, h, ExtendedColorType::Rgba8)?;
    Ok(png)
}

/// Returns None for normal app startup, otherwise the process exit status.
pub fn cli(args: impl IntoIterator<Item = std::ffi::OsString>) -> Option<i32> {
    let mut args = args.into_iter().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--thumbnail")) {
        return None;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        let input = args.next().context("missing input")?;
        let output = args.next().context("missing output PNG")?;
        let size = match args.next() {
            None => 256,
            Some(flag) if flag == "--size" => args
                .next()
                .context("missing size")?
                .to_str()
                .context("invalid size")?
                .parse()?,
            _ => bail!("expected --size N"),
        };
        ensure!(args.next().is_none(), "unexpected arguments");
        let png = thumbnail_png(Path::new(&input), size)?;
        std::fs::write(output, png)?;
        Ok(())
    }))
    .unwrap_or_else(|_| Err(anyhow::anyhow!("thumbnail decoder panicked")));
    Some(match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("thumbnail: {e:#}");
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    #[test]
    fn low_signal_tiff_has_visible_contrast_and_plain_rgba() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("low.tiff");
        let mut enc = tiff::encoder::TiffEncoder::new(std::fs::File::create(&path).unwrap()).unwrap();
        let pixels: Vec<u16> = (0..256).map(|n| 108 + n % 240).collect();
        enc.write_image::<tiff::encoder::colortype::Gray16>(16, 16, &pixels)
            .unwrap();
        drop(enc);
        let (w, h, rgba) = thumbnail_rgba(&path, 16).unwrap();
        assert_eq!((w, h, rgba.len()), (16, 16, 1024));
        assert!(rgba.chunks_exact(4).all(|p| p[3] == 255));
        assert!(rgba.chunks_exact(4).any(|p| p[0] > 200));
        assert!(rgba.chunks_exact(4).any(|p| p[0] < 20));
        assert_eq!(&thumbnail_png(&path, 8).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
        assert!(thumbnail_rgba(&path, 0).is_err());
    }

    #[test]
    fn sparse_four_gib_mrc_reads_only_middle_plane() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.mrc");
        let mut f = std::fs::File::create(&path).unwrap();
        let (w, h, z) = (256i32, 256i32, 32768i32);
        let mut hdr = [0u8; 1024];
        for (off, v) in [(0, w), (4, h), (8, z), (12, 6)] {
            hdr[off..off + 4].copy_from_slice(&v.to_le_bytes());
        }
        hdr[208..212].copy_from_slice(b"MAP ");
        hdr[212] = 0x44;
        f.write_all(&hdr).unwrap();
        f.set_len(1024 + w as u64 * h as u64 * z as u64 * 2).unwrap();
        f.seek(SeekFrom::Start(1024 + w as u64 * h as u64 * (z / 2) as u64 * 2))
            .unwrap();
        for n in 0..w * h {
            f.write_all(&((108 + n % 240) as u16).to_le_bytes()).unwrap();
        }
        drop(f);
        let (tw, th, rgba) = thumbnail_rgba(&path, 32).unwrap();
        assert_eq!((tw, th, rgba.len()), (32, 32, 4096));
        assert!(rgba.chunks_exact(4).any(|p| p[0] > 100));
    }

    #[test]
    fn oversized_plane_is_rejected_before_decode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large-plane.mrc");
        let mut f = std::fs::File::create(&path).unwrap();
        let mut hdr = [0u8; 1024];
        for (off, v) in [(0, 16384i32), (4, 8192), (8, 1), (12, 6)] {
            hdr[off..off + 4].copy_from_slice(&v.to_le_bytes());
        }
        hdr[208..212].copy_from_slice(b"MAP ");
        hdr[212] = 0x44;
        f.write_all(&hdr).unwrap();
        f.set_len(1024 + (256 << 20)).unwrap();
        drop(f);
        assert!(thumbnail_rgba(&path, 256)
            .unwrap_err()
            .to_string()
            .contains("decode limit"));
    }
}
