//! TVIPS plugin: the formats of TVIPS cameras (TemCam, XF416, XF1000…) in
//! one TifMrcView WebAssembly plugin.
//!
//! - `.tvips` movie streams recorded by EM-MENU / EMPLIfied ([`Stream`]):
//!   a 256-byte series header (size, version, image size, bits per pixel,
//!   offset, binning, pixel size in nm, high tension in kV, total
//!   magnification, frame header size), then frames, each a frame header
//!   (12 bytes in version 1) and the pixels, little-endian. Long recordings
//!   continue in `name_001.tvips`, `name_002.tvips`… without a series
//!   header; a plugin only sees the file being opened, so those show what
//!   the first file holds. (Layout as in LiberTEM, MIT licence.)
//! - the TemData structure EM-MENU writes into its TIFFs ([`emmenu`]),
//!   added to files the core opens.

mod emmenu;

use tifmrcview_plugin_sdk::{
    bail, export_format, export_metadata, file, le_u32, Chunk, DType, Format, Group, Info, Recipe,
    Result,
};

const HEADER: u64 = 256;

fn i32_at(b: &[u8], o: usize) -> i32 {
    le_u32(b, o) as i32
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(le_u32(b, o))
}

pub(crate) fn fmt(v: f64) -> String {
    let v: f64 = format!("{}", v as f32).parse().unwrap_or(v);
    format!("{v}")
}

/// Seconds since 1970 as "YYYY-MM-DD HH:MM:SS UTC".
pub(crate) fn unix_time(t: i64) -> Option<String> {
    if !(1..=32503680000).contains(&t) {
        return None;
    }
    let days = t.div_euclid(86400);
    let s = t.rem_euclid(86400);
    let (y, m, d) = civil(days);
    Some(format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        s / 3600,
        s / 60 % 60,
        s % 60
    ))
}

/// Year, month, day of a day count since 1970-01-01 (proleptic Gregorian).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

pub struct Stream {
    width: u32,
    height: u32,
    dtype: DType,
    frame_header: u64,
    frames: u32,
    header: Vec<u8>,
    first_frame: Vec<u8>,
}

impl Stream {
    fn frame_bytes(&self) -> u64 {
        self.width as u64 * self.height as u64 * self.dtype.size() as u64
    }
}

impl Format for Stream {
    fn open(path: &str) -> Result<Stream> {
        let size = file::size();
        if size < HEADER {
            bail!("不是 TVIPS 影像文件（文件太短）");
        }
        let h = file::read(0, HEADER as usize)?;
        let version = i32_at(&h, 4);
        if i32_at(&h, 0) != HEADER as i32 || !(1..=2).contains(&version) {
            let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
            let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
            let n = stem.len();
            let part = n > 4
                && stem.is_char_boundary(n - 4)
                && stem[n - 4..].starts_with('_')
                && stem[n - 3..].bytes().all(|b| b.is_ascii_digit());
            if part && !stem.ends_with("_000") {
                bail!("这是一段 TVIPS 录像的后续文件，没有文件头；请打开同一录像编号为 _000 的第一个文件");
            }
            bail!("不是 TVIPS 影像文件");
        }
        let (w, h_) = (i32_at(&h, 8), i32_at(&h, 12));
        let dtype = match i32_at(&h, 16) {
            8 => DType::U8,
            16 => DType::U16,
            b => bail!("不支持每像素 {b} 位的 TVIPS 文件"),
        };
        if w <= 0 || h_ <= 0 {
            bail!("TVIPS 文件头里的图像尺寸不对（{w} × {h_}）");
        }
        let frame_header = if version == 1 {
            12
        } else {
            i32_at(&h, 48).max(0) as u64
        };
        let mut s = Stream {
            width: w as u32,
            height: h_ as u32,
            dtype,
            frame_header,
            frames: 0,
            header: h,
            first_frame: vec![],
        };
        let per = frame_header + s.frame_bytes();
        let frames = (size - HEADER) / per;
        if frames == 0 {
            bail!("这个 TVIPS 文件里没有完整的帧");
        }
        s.frames = frames.min(u32::MAX as u64) as u32;
        s.first_frame = file::read(HEADER, frame_header.min(1024) as usize)?;
        Ok(s)
    }

    fn info(&mut self) -> Result<Info> {
        let h = &self.header;
        let mut info = Info::new(self.width, self.height, self.dtype, "TVIPS");
        info.size_t = self.frames;
        info.page_count = self.frames;
        let px = i32_at(h, 36);
        if px > 0 {
            info.pixel_size = Some([px as f64, px as f64, 0.0]);
            info.unit = "nm".into();
        }
        let mut g = Group::new("TVIPS 影像");
        g.add_str("文件版本", &i32_at(h, 4).to_string());
        g.add_str(
            "尺寸",
            &format!("{} × {}，{} 帧", self.width, self.height, self.frames),
        );
        g.add_str("位深", &format!("{} 位", i32_at(h, 16)));
        g.add_str("相机偏移", &format!("{}, {}", i32_at(h, 20), i32_at(h, 24)));
        g.add_str(
            "像素合并",
            &format!("{} × {}", i32_at(h, 28), i32_at(h, 32)),
        );
        if px > 0 {
            g.add_str("像素尺寸", &format!("{px} nm"));
        }
        let ht = i32_at(h, 40);
        if ht > 0 {
            g.add_str("加速电压", &format!("{ht} kV"));
        }
        let mag = i32_at(h, 44);
        if mag > 0 {
            g.add_str("总放大倍数", &format!("{mag}×"));
        }
        g.add_str("帧头", &format!("{} 字节", self.frame_header));
        let f = &self.first_frame;
        if f.len() >= 48 && i32_at(h, 4) == 2 {
            g.add(
                "第 1 帧时间",
                unix_time(le_u32(f, 4) as i64).map(|t| format!("{t}（+{} ms）", le_u32(f, 8))),
            );
            g.add_str("第 1 帧编号", &le_u32(f, 0).to_string());
            let mag = le_u32(f, 20);
            if mag > 0 {
                g.add_str("放大倍数", &format!("{mag}×"));
            }
            g.add_str(
                "模式",
                if le_u32(f, 24) == 2 {
                    "衍射"
                } else {
                    "成像"
                },
            );
            let st: Vec<String> = (0..3).map(|k| fmt(f32_at(f, 28 + 4 * k) as f64)).collect();
            g.add_str("样品台 X / Y / Z", &format!("{} nm", st.join(" / ")));
            g.add_str(
                "α / β 倾转",
                &format!(
                    "{}° / {}°",
                    fmt(f32_at(f, 40) as f64),
                    fmt(f32_at(f, 44) as f64)
                ),
            );
            if f.len() >= 76 {
                g.add_str(
                    "扫描位置（第 1 帧）",
                    &format!(
                        "{}, {}",
                        fmt(f32_at(f, 68) as f64),
                        fmt(f32_at(f, 72) as f64)
                    ),
                );
            }
        }
        g.add_str(
            "说明",
            "长录像会接着写到 _001、_002… 文件里，插件只能读取打开的这个文件",
        );
        g.push_to(&mut info.metadata);
        Ok(info)
    }

    fn page(&self, page: u32) -> Result<Recipe> {
        if page >= self.frames {
            bail!("没有第 {} 帧", page + 1);
        }
        let per = self.frame_header + self.frame_bytes();
        let at = HEADER + page as u64 * per + self.frame_header;
        let mut r = Recipe::new();
        r.push(Chunk::file(
            at,
            self.frame_bytes(),
            [0, 0, self.width, self.height],
        ));
        Ok(r)
    }
}

export_format!(Stream);
export_metadata!(emmenu::EmMenu);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(
            unix_time(1456073429).as_deref(),
            Some("2016-02-21 16:50:29 UTC")
        );
        assert_eq!(civil(17268), (2017, 4, 12));
    }
}
