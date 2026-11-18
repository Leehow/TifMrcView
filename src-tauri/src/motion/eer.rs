//! Native EER grouping. TIFF parses directories only; shared event decoding
//! counts mapped fractions in parallel, each worker into its own frame. Raw
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
    /// Adds the events of one fraction to `acc` (the rendered frame, `up`
    /// times the detector's size).
    fn count(&self, f: &Fraction, up: usize, acc: &mut [u16]) -> Result<()> {
        let rw = self.w * up;
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
                    let at = ((y0 + i / self.w) * up) * rw + (i % self.w) * up;
                    acc[at] = acc[at].saturating_add(n as u16);
                }
            } else {
                events::events(src, rows * self.w, f.bits, |p, sx, sy| {
                    let (dx, dy) = if up == 2 {
                        (events::subpixel_2x(sx, f.bits.1), events::subpixel_2x(sy, f.bits.2))
                    } else {
                        (0, 0)
                    };
                    let at = ((y0 + p / self.w) * up + dy) * rw + (p % self.w) * up + dx;
                    acc[at] = acc[at].saturating_add(1);
                })
                .map_err(anyhow::Error::msg)?;
            }
        }
        Ok(())
    }
    /// The events of one compressed fraction, in detector order: the
    /// detector pixel shifted left by two, the 2× subpixel (dy, dx) below.
    fn events(&self, f: &Fraction, up: usize, out: &mut Vec<u32>) -> Result<()> {
        out.clear();
        for (j, &(off, len)) in f.strips.iter().enumerate() {
            let y0 = j * f.rows;
            if y0 >= self.h {
                bail!("EER strip 行数超限");
            }
            let rows = f.rows.min(self.h - y0);
            let base = y0 * self.w;
            events::events(&self.map[off..off + len], rows * self.w, f.bits, |p, sx, sy| {
                let (dx, dy) = if up == 2 {
                    (events::subpixel_2x(sx, f.bits.1), events::subpixel_2x(sy, f.bits.2))
                } else {
                    (0, 0)
                };
                out.push((((base + p) as u32) << 2) | (dy << 1 | dx) as u32);
            })
            .map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
    pub fn group(&self, start: usize, count: usize, up: usize) -> Result<Vec<f32>> {
        Ok(self.group_max(start, count, up)?.0)
    }
    /// `group`, with the frame's largest count: taken from each band of rows
    /// while it is still in cache, so a reader needs no pass of its own over
    /// the frame (an EER frame holds whole, non-negative counts by
    /// construction).
    pub fn group_max(&self, start: usize, count: usize, up: usize) -> Result<(Vec<f32>, f32)> {
        if ![1, 2].contains(&up) || start >= self.len() || count == 0 {
            bail!("EER 分组或超分辨率参数无效");
        }
        let end = (start + count).min(self.len());
        let rw = self.w * up;
        let len = rw * self.h * up;
        let fractions = &self.frames[start..end];
        if self.w * self.h <= 1 << 30 && fractions.iter().all(|f| f.compression != 1) {
            // Each fraction is decoded on its own (its bit stream is
            // sequential) into a list of events in detector order; then
            // bands of rows of the frame are filled in parallel, each from
            // its slice of every list. No worker allocates a frame of its
            // own: a frame per worker per group (32 MB at 4096², fresh pages
            // every time) used to spend half the time in the kernel.
            let lists: Vec<Vec<u32>> = fractions
                .par_iter()
                .map(|f| -> Result<Vec<u32>> {
                    super::check_cancel()?;
                    // About eleven bits an event in the 7-bit codes.
                    let mut v = Vec::with_capacity(f.strips.iter().map(|s| s.1).sum::<usize>() * 8 / 10);
                    self.events(f, up, &mut v)?;
                    Ok(v)
                })
                .collect::<Result<_>>()?;
            let mut out = vec![0.0f32; len];
            let band = 64usize.min(self.h);
            let max = out.par_chunks_mut(band * up * rw).enumerate().map(|(b, rows)| {
                let (lo, hi) = (((b * band * self.w) as u32) << 2, ((((b + 1) * band).min(self.h) * self.w) as u32) << 2);
                let first = b * band;
                for list in &lists {
                    let a = list.partition_point(|&e| e < lo);
                    let z = list.partition_point(|&e| e < hi);
                    for &e in &list[a..z] {
                        let p = (e >> 2) as usize;
                        let (dy, dx) = ((e >> 1 & 1) as usize, (e & 1) as usize);
                        let at = ((p / self.w - first) * up + dy) * rw + (p % self.w) * up + dx;
                        rows[at] += 1.0;
                    }
                }
                rows.iter().fold(0.0f32, |m, &v| m.max(v))
            }).reduce(|| 0.0, f32::max);
            return Ok((out, max));
        }
        let out = self.group_per_worker(start, count, up)?;
        let max = out.par_iter().copied().reduce(|| 0.0, f32::max);
        Ok((out, max))
    }
    /// The earlier way, kept for uncompressed fractions (and to compare
    /// with): every worker counts its share into a frame of its own (two
    /// bytes a pixel), and the frames are added up. At most about 1 GB of
    /// such frames at once.
    pub fn group_per_worker(&self, start: usize, count: usize, up: usize) -> Result<Vec<f32>> {
        if ![1, 2].contains(&up) || start >= self.len() || count == 0 {
            bail!("EER 分组或超分辨率参数无效");
        }
        let fractions = &self.frames[start..(start + count).min(self.len())];
        let len = self.w * up * self.h * up;
        let mut out = vec![0.0f32; len];
        let workers = rayon::current_num_threads().clamp(1, ((1usize << 30) / (2 * len)).max(1));
        let share = fractions.len().div_ceil(workers).max(1);
        let parts: Vec<Vec<u16>> = fractions
            .par_chunks(share)
            .map(|chunk| -> Result<Vec<u16>> {
                let mut acc = vec![0u16; len];
                for f in chunk {
                    super::check_cancel()?;
                    self.count(f, up, &mut acc)?;
                }
                Ok(acc)
            })
            .collect::<Result<_>>()?;
        out.par_iter_mut().enumerate().for_each(|(i, o)| *o = parts.iter().map(|p| p[i] as u32).sum::<u32>() as f32);
        Ok(out)
    }
}
