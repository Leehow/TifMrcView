//! Native EER grouping. TIFF parses directories only; shared event decoding
//! reads mapped strips in parallel, at most four fractions at once. Raw
//! fractions are never materialised as full-resolution images.
use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use rayon::prelude::*;
use std::{fs::File, io::Cursor, path::Path};
use tiff::{decoder::Decoder, tags::Tag};
#[path = "../../../plugins/thermo/src/eer_decode.rs"]
pub mod events;
struct Fraction {
    strips: Vec<(usize, usize)>,
    rows: usize,
    compression: u32,
    bits: (u32, u32, u32),
}
pub struct Eer {
    map: Mmap,
    pub w: usize,
    pub h: usize,
    frames: Vec<Fraction>,
}
impl Eer {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        // SAFETY: the input is read-only and must not be truncated while open.
        let map = unsafe { Mmap::map(&file)? };
        let mut d = Decoder::new(Cursor::new(&map[..]))?;
        let (w, h) = d.dimensions()?;
        let (w, h) = (w as usize, h as usize);
        let mut frames = vec![];
        if w == 0 || h == 0 || w.saturating_mul(h) > 1 << 28 {
            bail!("EER 尺寸无效");
        }
        loop {
            if d.dimensions()? != (w as u32, h as u32) {
                bail!("EER 分帧尺寸不一致");
            }
            let c = d.get_tag_u32(Tag::Compression)?;
            let tag = |d: &mut Decoder<Cursor<&[u8]>>, n| d.get_tag_u32(Tag::Unknown(n));
            let bits = match c {
                65000 => (8, 2, 2),
                65001 => (7, 2, 2),
                65002 => (
                    tag(&mut d, 65007).unwrap_or(7),
                    tag(&mut d, 65008).unwrap_or(2),
                    tag(&mut d, 65009).unwrap_or(2),
                ),
                1 => (0, 0, 0),
                _ => bail!("不支持 EER 压缩 {c}"),
            };
            let offsets = d.get_tag_u64_vec(Tag::StripOffsets)?;
            let lengths = d.get_tag_u64_vec(Tag::StripByteCounts)?;
            if offsets.is_empty() || offsets.len() != lengths.len() {
                bail!("EER strips 无效");
            }
            let mut strips = vec![];
            for (o, n) in offsets.into_iter().zip(lengths) {
                let end = o.checked_add(n).context("EER strip 溢出")?;
                if end > map.len() as u64 {
                    bail!("EER strip 截断");
                }
                strips.push((o as usize, n as usize));
            }
            let rows = d.get_tag_u32(Tag::RowsPerStrip).unwrap_or(h as u32) as usize;
            if rows == 0 {
                bail!("EER RowsPerStrip 为 0");
            }
            frames.push(Fraction {
                strips,
                rows,
                compression: c,
                bits,
            });
            if !d.more_images() {
                break;
            }
            d.next_image()?;
        }
        Ok(Self { map, w, h, frames })
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    pub fn group(&self, start: usize, count: usize, up: usize) -> Result<Vec<f32>> {
        if ![1, 2].contains(&up) || start >= self.len() || count == 0 {
            bail!("EER 分组或超分辨率参数无效");
        }
        let end = (start + count).min(self.len());
        let rw = self.w * up;
        let mut out = vec![0.0; rw * self.h * up];
        for chunk in self.frames[start..end].chunks(4) {
            super::check_cancel()?;
            let decoded: Vec<Vec<usize>> = chunk
                .par_iter()
                .map(|f| -> Result<Vec<usize>> {
                    let mut hits = vec![];
                    for (j, &(off, len)) in f.strips.iter().enumerate() {
                        let y0 = j * f.rows;
                        if y0 >= self.h {
                            bail!("EER strip 行数超限");
                        }
                        let rows = f.rows.min(self.h - y0);
                        let src = &self.map[off..off + len];
                        if f.compression == 1 {
                            if src.len() < rows * self.w {
                                bail!("EER 原始 strip 截断");
                            }
                            for (i, &n) in src[..rows * self.w].iter().enumerate() {
                                for _ in 0..n {
                                    hits.push(((y0 + i / self.w) * up) * rw + (i % self.w) * up);
                                }
                            }
                        } else {
                            events::events(src, rows * self.w, f.bits, |p, sx, sy| {
                                let (dx, dy) = if up == 2 {
                                    (
                                        events::subpixel_2x(sx, f.bits.1),
                                        events::subpixel_2x(sy, f.bits.2),
                                    )
                                } else {
                                    (0, 0)
                                };
                                hits.push(
                                    ((y0 + p / self.w) * up + dy) * rw + (p % self.w) * up + dx,
                                );
                            })
                            .map_err(anyhow::Error::msg)?;
                        }
                    }
                    Ok(hits)
                })
                .collect::<Result<_>>()?;
            for hits in decoded {
                for i in hits {
                    out[i] += 1.0;
                }
            }
        }
        Ok(out)
    }
}
