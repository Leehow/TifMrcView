//! EER (Electron Event Representation) movies of Thermo Fisher Falcon 4,
//! 4i and C cameras.
//!
//! An EER file is a BigTIFF with one directory per frame. Each frame is a
//! bit stream of detected electrons: a run length (how many pixels to skip
//! before the next event) followed by the sub-pixel position of the event,
//! packed least-significant bit first. Compression 65000 uses 8-bit runs,
//! 65001 7-bit runs, and 65002 the bit widths in tags 65007–65009; a run
//! of all ones only skips, without an event. The acquisition settings are
//! XML in tag 65001 of the first directory. (Written from the description
//! in the EER file format documentation and the BSD-licensed decoder of
//! imagecodecs.)
//!
//! A single frame holds only a few hundred thousand events, so the default
//! view sums consecutive frames into at most [`MAX_FRACTIONS`] fractions;
//! the second series has every frame. Events are counted at the physical
//! resolution (the sub-pixel bits are not used).

use crate::{fmt, show_length};
use tifmrcview_plugin_sdk::{bail, file, tiff, Codec, DType, Group, Info, Recipe, Result, Series};

/// Most pages of the summed view.
const MAX_FRACTIONS: u32 = 60;
/// Most encoded bytes summed into one page (about 6·10⁶ events), to stay
/// well inside the time a page may take.
const MAX_PAGE_BYTES: u64 = 8 << 20;

struct Frame {
    /// (offset, length) of each strip.
    strips: Vec<(u64, u64)>,
    rows_per_strip: u32,
    compression: u16,
    bits: (u32, u32, u32),
}

pub struct Eer {
    width: u32,
    height: u32,
    frames: Vec<Frame>,
    /// Frames summed per page (1 = every frame).
    per_page: u32,
    items: Vec<(String, String, String)>,
    series: u32,
}

/// (name, unit, value) of every `<item>` of the acquisition XML.
fn items(xml: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(p) = rest.find("<item") {
        rest = &rest[p + 5..];
        let Some(close) = rest.find('>') else { break };
        let head = &rest[..close];
        let attr = |name: &str| -> String {
            let key = format!("{name}=\"");
            head.find(&key)
                .and_then(|i| head[i + key.len()..].split('"').next())
                .unwrap_or("")
                .to_string()
        };
        let body = &rest[close + 1..];
        let value = body
            .split("</item>")
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        out.push((attr("name"), attr("unit"), value));
        rest = body;
    }
    out
}

fn item<'a>(
    items: &'a [(String, String, String)],
    name: &str,
) -> Option<&'a (String, String, String)> {
    items.iter().find(|i| i.0 == name)
}

fn item_num(items: &[(String, String, String)], name: &str) -> Option<f64> {
    item(items, name)
        .and_then(|i| i.2.parse().ok())
        .filter(|v: &f64| v.is_finite())
}

/// Adds the events of one strip to `out`, the rows of the strip as
/// little-endian u16 counts (kept as bytes so the page needs no conversion).
pub fn decode(
    src: &[u8],
    out: &mut [u8],
    (skip_bits, h_bits, v_bits): (u32, u32, u32),
) -> Result<()> {
    let nbits = (skip_bits + h_bits + v_bits) as usize;
    if !(9..=16).contains(&nbits) || skip_bits < 4 {
        bail!("EER 文件的位宽不对（{skip_bits}+{h_bits}+{v_bits}）");
    }
    let mask = (1u32 << skip_bits) - 1;
    let total = src.len() * 8;
    let end = total.saturating_sub(nbits);
    let n = out.len() / 2;
    let mut bit = 0usize;
    let mut px = 0usize;
    while bit < end {
        let b = bit >> 3;
        // Little-endian word from byte `b`; zero past the end.
        let word = if b + 4 <= src.len() {
            u32::from_le_bytes([src[b], src[b + 1], src[b + 2], src[b + 3]])
        } else {
            let mut w = [0u8; 4];
            w[..src.len() - b].copy_from_slice(&src[b..]);
            u32::from_le_bytes(w)
        } >> (bit & 7);
        let skip = word & mask;
        px += skip as usize;
        if px >= n {
            if px == n {
                break;
            }
            bail!("EER 帧数据损坏（事件位置超出图像）");
        }
        if skip == mask {
            bit += skip_bits as usize;
            continue;
        }
        let c = u16::from_le_bytes([out[2 * px], out[2 * px + 1]]).saturating_add(1);
        out[2 * px..2 * px + 2].copy_from_slice(&c.to_le_bytes());
        px += 1;
        bit += nbits;
    }
    Ok(())
}

impl Eer {
    pub fn open(series: u32) -> Result<Eer> {
        let size = file::size();
        let t = tiff::parse(&mut tiff::file_reader, size, 0)?;
        let first = &t.ifds[0];
        let width = first.uint(&t, 256).unwrap_or(0) as u32;
        let height = first.uint(&t, 257).unwrap_or(0) as u32;
        if width == 0 || height == 0 {
            bail!("EER 文件里图像尺寸不对");
        }
        let xml = first
            .get(65001)
            .map(|e| String::from_utf8_lossy(&e.value).into_owned())
            .unwrap_or_default();
        let mut frames = Vec::with_capacity(t.ifds.len());
        for ifd in &t.ifds {
            if ifd.uint(&t, 256) != Some(width as u64) || ifd.uint(&t, 257) != Some(height as u64) {
                continue;
            }
            let compression = ifd.uint(&t, 259).unwrap_or(1) as u16;
            let bits = match compression {
                65000 => (8, 2, 2),
                65001 => (7, 2, 2),
                65002 => (
                    ifd.uint(&t, 65007).unwrap_or(7) as u32,
                    ifd.uint(&t, 65008).unwrap_or(2) as u32,
                    ifd.uint(&t, 65009).unwrap_or(2) as u32,
                ),
                1 => (0, 0, 0),
                c => bail!("EER 文件用了不认识的压缩方式 {c}"),
            };
            let offs = ifd.uints(&t, 273);
            let lens = ifd.uints(&t, 279);
            if offs.is_empty() || offs.len() != lens.len() {
                bail!("EER 帧缺少数据位置");
            }
            let rows_per_strip = ifd
                .uint(&t, 278)
                .unwrap_or(height as u64)
                .min(height as u64)
                .max(1) as u32;
            frames.push(Frame {
                strips: offs.into_iter().zip(lens).collect(),
                rows_per_strip,
                compression,
                bits,
            });
        }
        if frames.is_empty() {
            bail!("EER 文件里没有帧");
        }
        let n = frames.len() as u32;
        let per_page = if series == 1 {
            1
        } else {
            let mean: u64 = frames
                .iter()
                .map(|f| f.strips.iter().map(|s| s.1).sum::<u64>())
                .sum::<u64>()
                / n as u64;
            n.div_ceil(MAX_FRACTIONS)
                .min((MAX_PAGE_BYTES / mean.max(1)).max(1) as u32)
                .max(1)
        };
        if series > 1 {
            bail!("这个文件只有 2 种视图（请求第 {} 种）", series + 1);
        }
        Ok(Eer {
            width,
            height,
            frames,
            per_page,
            items: items(&xml),
            series,
        })
    }

    pub fn info(&mut self) -> Result<Info> {
        let n = self.frames.len() as u32;
        let pages = n.div_ceil(self.per_page);
        let mut info = Info::new(self.width, self.height, DType::U16, "Thermo Fisher EER");
        info.size_t = pages;
        info.page_count = pages;
        let it = &self.items;
        // Newer files give the sensor pixel size in metres.
        let px = item_num(it, "sensorPixelSize.width")
            .or_else(|| item_num(it, "pixelSize.width"))
            .filter(|&v| v > 0.0 && v < 1e-3);
        if let Some(px) = px {
            let (x, unit) = crate::length(px);
            let py = item_num(it, "sensorPixelSize.height")
                .or_else(|| item_num(it, "pixelSize.height"))
                .unwrap_or(px);
            info.pixel_size = Some([x, crate::tidy(py * x / px), 0.0]);
            info.unit = unit.into();
        }
        let mut g = Group::new("EER 电子计数");
        g.add_str("帧数", &n.to_string());
        g.add_str(
            "显示方式",
            &if self.per_page == 1 {
                "逐帧显示".to_string()
            } else {
                format!(
                    "每 {} 帧相加为一页，共 {pages} 页（在序列里可切换到逐帧）",
                    self.per_page
                )
            },
        );
        g.add_str("尺寸", &format!("{} × {}", self.width, self.height));
        let c = self.frames[0].compression;
        let (s, h, v) = self.frames[0].bits;
        g.add_str("编码", &format!("{c}（游程 {s} 位，亚像素 {h}+{v} 位）"));
        let bytes: u64 = self
            .frames
            .iter()
            .map(|f| f.strips.iter().map(|s| s.1).sum::<u64>())
            .sum();
        g.add_str(
            "平均每帧事件数（估计）",
            &fmt(((bytes * 8) as f64 / (s + h + v) as f64 / n as f64).round()),
        );
        if let Some(px) = px {
            g.add_str("像素尺寸", &show_length(px));
        }
        g.push_to(&mut info.metadata);
        let mut all = Group::new("EER 采集参数");
        for (name, unit, value) in &self.items {
            let v = if unit.is_empty() {
                value.clone()
            } else {
                format!("{value} {unit}")
            };
            all.push(name, &v);
        }
        all.push_to(&mut info.metadata);
        info.series = vec![
            Series {
                name: format!("按 {} 帧相加", self.per_page.max(2)),
                detail: "电子计数总和".into(),
            },
            Series {
                name: "逐帧".into(),
                detail: format!("{n} 帧"),
            },
        ];
        if self.per_page == 1 && self.series == 0 {
            // Few frames: the summed view is the same as the frame view.
            info.series.clear();
        }
        info.series_index = self.series;
        Ok(info)
    }

    pub fn page(&self, page: u32) -> Result<Recipe> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut counts = vec![0u8; w * h * 2];
        let first = page as usize * self.per_page as usize;
        if first >= self.frames.len() {
            bail!("没有第 {} 页", page + 1);
        }
        let last = (first + self.per_page as usize).min(self.frames.len());
        for f in &self.frames[first..last] {
            for (k, &(off, len)) in f.strips.iter().enumerate() {
                let y0 = k * f.rows_per_strip as usize;
                if y0 >= h {
                    break;
                }
                let y1 = (y0 + f.rows_per_strip as usize).min(h);
                let rows = &mut counts[2 * y0 * w..2 * y1 * w];
                let data = file::read(off, len as usize)?;
                if f.compression == 1 {
                    // Uncompressed 8-bit counts (rare).
                    for (o, &b) in rows.chunks_exact_mut(2).zip(&data) {
                        let c = u16::from_le_bytes([o[0], o[1]]).saturating_add(b as u16);
                        o.copy_from_slice(&c.to_le_bytes());
                    }
                } else {
                    decode(&data, rows, f.bits)?;
                }
            }
        }
        let mut r = Recipe::new();
        r.inline(&counts, Codec::Raw, [0, 0, self.width, self.height]);
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Packs (skip, sub-pixel) codes LSB first, as an EER writer does.
    fn pack(codes: &[(u32, u32)], skip_bits: u32) -> Vec<u8> {
        let mut bits: Vec<bool> = vec![];
        for &(skip, sub) in codes {
            for i in 0..skip_bits {
                bits.push(skip >> i & 1 == 1);
            }
            if skip != (1 << skip_bits) - 1 {
                for i in 0..4 {
                    bits.push(sub >> i & 1 == 1);
                }
            }
        }
        let mut out = vec![0u8; bits.len().div_ceil(8) + 4];
        for (i, b) in bits.iter().enumerate() {
            if *b {
                out[i / 8] |= 1 << (i % 8);
            }
        }
        out
    }

    #[test]
    fn decodes_runs_and_overflow() {
        // 7-bit runs: event at 3, event at 4 (run 0), skip 127 without an
        // event, an event 5 further on, then a run to the end of the frame
        // as writers end every frame.
        let src = pack(&[(3, 0b0101), (0, 0), (127, 0), (5, 0b1111), (62, 0)], 7);
        let mut out = vec![0u8; 400];
        decode(&src, &mut out, (7, 2, 2)).unwrap();
        let counts = |o: &[u8]| {
            o.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<u16>>()
        };
        let hits: Vec<usize> = counts(&out)
            .iter()
            .enumerate()
            .filter(|(_, &c)| c > 0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(hits, [3, 4, 3 + 1 + 1 + 127 + 5]);
        // The same events again add up.
        decode(&src, &mut out, (7, 2, 2)).unwrap();
        assert_eq!(counts(&out)[4], 2);
        assert!(decode(&src, &mut [0u8; 20], (7, 2, 2)).is_err());
    }

    #[test]
    fn reads_xml_items() {
        let x = r#"<metadata><item name="exposureTime" unit="s">2.999666</item><item name="numberOfFrames">721</item></metadata>"#;
        let it = items(x);
        assert_eq!(it.len(), 2);
        assert_eq!(
            it[0],
            ("exposureTime".into(), "s".into(), "2.999666".into())
        );
        assert_eq!(item_num(&it, "numberOfFrames"), Some(721.0));
    }
}
