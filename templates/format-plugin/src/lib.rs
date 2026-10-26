//! R16: a 16-byte header followed by uncompressed little-endian u16 planes.
//! Only the header is read here; the host reads the pixels through recipes.

use tifmrcview_plugin_sdk::{
    bail, export_format, file, le_u32, Chunk, Context, DType, Format, Group, Info, Recipe, Result,
};

#[derive(Debug)]
struct R16 {
    w: u32,
    h: u32,
    depth: u32,
    page_bytes: u64,
}

impl R16 {
    // Kept separate from host imports so the same parser can be tested natively.
    fn parse(head: &[u8], file_size: u64) -> Result<R16> {
        if head.len() < 16 || &head[..4] != b"R16\0" {
            bail!("Not an R16 file (expected a 16-byte R16 header)");
        }
        let (w, h, depth) = (le_u32(head, 4), le_u32(head, 8), le_u32(head, 12));
        if w == 0 || h == 0 || depth == 0 || depth > 1 << 24 {
            bail!("Invalid R16 dimensions");
        }
        let page_bytes = (w as u64)
            .checked_mul(h as u64)
            .and_then(|n| n.checked_mul(2))
            .context("R16 dimensions overflow")?;
        if page_bytes > 16 << 30 {
            bail!("R16 page exceeds the app's 16 GiB limit");
        }
        let end = page_bytes
            .checked_mul(depth as u64)
            .and_then(|n| n.checked_add(16))
            .context("R16 file size overflow")?;
        if end > file_size {
            bail!("R16 pixel data is truncated");
        }
        Ok(R16 {
            w,
            h,
            depth,
            page_bytes,
        })
    }
}

impl Format for R16 {
    fn open(_path: &str) -> Result<R16> {
        R16::parse(&file::read(0, 16)?, file::size())
    }

    fn info(&mut self) -> Result<Info> {
        let mut info = Info::new(self.w, self.h, DType::U16, "R16 stack");
        info.size_z = self.depth;
        info.page_count = self.depth;
        let mut group = Group::new("R16");
        group.add_str("Planes", &self.depth.to_string());
        group.add_str("Storage", "Little-endian u16, uncompressed");
        info.metadata.push(group);
        Ok(info)
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        if page >= self.depth {
            bail!("R16 page is out of range");
        }
        let mut recipe = Recipe::new();
        recipe.push(Chunk::file(
            16 + page as u64 * self.page_bytes,
            self.page_bytes,
            [0, 0, self.w, self.h],
        ));
        Ok(recipe)
    }
}

export_format!(R16);

#[cfg(test)]
#[path = "../examples/make_sample.rs"]
#[allow(dead_code)]
mod sample;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_sample_info_and_pixels() {
        let bytes = sample::sample();
        let mut image = R16::parse(&bytes[..16], bytes.len() as u64).unwrap();
        let info = image.info().unwrap();
        assert_eq!(
            (info.width, info.height, info.size_z, info.page_count),
            (4, 3, 2, 2)
        );
        assert_eq!(info.dtype, DType::U16);
        assert!(info.to_json().contains("Little-endian u16, uncompressed"));
        for page in 0..2 {
            let recipe = image.page(page).unwrap();
            assert_eq!(recipe.chunks.len(), 1);
            let c = &recipe.chunks[0];
            assert_eq!((c.source, c.flags, c.samples), (0, 0, 1));
            assert_eq!(
                (c.offset, c.length, c.rect),
                (16 + page as u64 * 24, 24, [0, 0, 4, 3])
            );
            let pixels: Vec<u16> = bytes[c.offset as usize..(c.offset + c.length) as usize]
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            assert_eq!(
                pixels,
                (0..12)
                    .map(|i| (page * 12 + i) as u16 * 1000)
                    .collect::<Vec<_>>()
            );
            let encoded = recipe.to_bytes();
            assert_eq!(&encoded[..4], b"TMVR");
            assert_eq!(le_u32(&encoded, 4), 1);
            assert_eq!(encoded.len(), 64);
        }
        assert!(image.page(2).is_err());
    }

    #[test]
    fn rejects_bad_headers_dimensions_and_truncated_pixels() {
        let good = sample::sample();
        for len in 0..16 {
            assert!(R16::parse(&good[..len], good.len() as u64).is_err());
        }
        let mut bad = good.clone();
        bad[0] = b'X';
        assert!(R16::parse(&bad[..16], bad.len() as u64).is_err());
        for offset in [4, 8, 12] {
            let mut bad = good.clone();
            bad[offset..offset + 4].copy_from_slice(&0u32.to_le_bytes());
            assert!(R16::parse(&bad[..16], bad.len() as u64).is_err());
        }
        assert!(R16::parse(&good[..16], (good.len() - 1) as u64).is_err());
        for dims in [
            [u32::MAX, u32::MAX, 1],
            [100_000, 100_000, 1],
            [1, 1, (1 << 24) + 1],
        ] {
            let mut head = b"R16\0".to_vec();
            for n in dims {
                head.extend_from_slice(&n.to_le_bytes());
            }
            assert!(R16::parse(&head, u64::MAX).is_err());
        }
    }
}
