//! Movie adapters and a bounded-memory spool of rendered groups. Native EER
//! is decoded once; alignment holds only reduced images. Full groups are
//! read back one at a time for warping, and removed on error or cancellation.
use super::{check_cancel, eer::Eer, Params};
use crate::image::OpenImage;
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

pub enum Movie {
    Eer(Eer),
    Image {
        img: Arc<OpenImage>,
        along_t: bool,
        c: usize,
        z: usize,
        t: usize,
    },
}
impl Movie {
    pub fn open(
        path: &str,
        current: Option<Arc<OpenImage>>,
        p: &Params,
        c: usize,
        z: usize,
        t: usize,
    ) -> Result<Self> {
        if Path::new(path)
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("eer"))
        {
            return Ok(Self::Eer(Eer::open(Path::new(path))?));
        }
        let img = match current {
            Some(i) => i,
            None => OpenImage::new(0, crate::formats::open(Path::new(path))?, 64 << 20),
        };
        let info = img.info();
        let along_t = match p.frame_axis.as_str() {
            "auto" => info.size_z == 1 && info.size_t > 1,
            "t" => true,
            "z" => false,
            _ => bail!("电影轴必须为 auto/z/t"),
        };
        if c >= info.size_c || z >= info.size_z || t >= info.size_t {
            bail!("电影通道/切片/时间索引无效");
        }
        Ok(Self::Image {
            img,
            along_t,
            c,
            z,
            t,
        })
    }
    pub fn shape(&self) -> (usize, usize, usize) {
        match self {
            Self::Eer(e) => (e.w, e.h, e.len()),
            Self::Image { img, along_t, .. } => {
                let i = img.info();
                (
                    i.width,
                    i.height,
                    if *along_t { i.size_t } else { i.size_z },
                )
            }
        }
    }
    pub fn pixel_a(&self) -> Option<f64> {
        match self {
            Self::Image { img, .. } => img
                .info()
                .pixel_size
                .map(|_| crate::align::pixel_a(img.info())),
            _ => None,
        }
    }
    fn group(&self, start: usize, count: usize, up: usize) -> Result<Vec<f32>> {
        match self {
            Self::Eer(e) => e.group(start, count, up),
            Self::Image {
                img,
                along_t,
                c,
                z,
                t,
            } => {
                if up != 1 {
                    bail!("2× 渲染只适用于 EER");
                }
                let (w, h, n) = self.shape();
                let mut out = vec![0.0; w * h];
                for i in start..(start + count).min(n) {
                    check_cancel()?;
                    let (page, s) = img.info().locate(
                        *c,
                        if *along_t { *z } else { i },
                        if *along_t { i } else { *t },
                    );
                    let data = img.page(page)?;
                    let plane = &data[s];
                    out.par_iter_mut()
                        .enumerate()
                        .for_each(|(j, o)| *o += plane.data.get_f64(j) as f32);
                }
                Ok(out)
            }
        }
    }
}

pub struct Spool {
    dir: PathBuf,
    pub w: usize,
    pub h: usize,
    pub frames: Vec<Vec<f32>>,
    pub bin: usize,
    pub counts: Vec<usize>,
    pub times: Vec<f64>,
    pub original: Vec<f32>,
    pub decode_ms: f64,
    pub prepare_ms: f64,
}
impl Drop for Spool {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
impl Spool {
    pub fn read(&self, i: usize) -> Result<Vec<f32>> {
        let bytes = fs::read(self.dir.join(format!("frame-{i}.f32")))?;
        if bytes.len() != self.w * self.h * 4 {
            bail!("电影缓存截断");
        }
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect())
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    pub fn prepare(
        movie: &Movie,
        p: &Params,
        parent: &Path,
        progress: &(dyn Fn(&str, usize, usize) + Sync),
    ) -> Result<Self> {
        let now = std::time::Instant::now();
        let (w, h, n) = movie.shape();
        let up = if matches!(movie, Movie::Eer(_)) {
            p.upsample
        } else {
            1
        };
        if n < 2 {
            bail!("电影至少需要两帧");
        }
        let group = if p.group == 0 {
            if matches!(movie, Movie::Eer(_)) {
                n.div_ceil(40)
            } else {
                1
            }
        } else {
            p.group
        };
        if group > 256 || n.div_ceil(group) > 2048 {
            bail!("分组过大或输出帧数过多");
        }
        let (w, h) = (w * up, h * up);
        if w.saturating_mul(h) > 1 << 27 {
            bail!("电影渲染尺寸过大");
        }
        let bin = w.min(h).div_ceil(p.working_size).max(1);
        let (ww, hh) = (w / bin, h / bin);
        if ww.min(hh) < 80 {
            bail!("电影太小（至少 80 px）");
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = parent.join(format!(
            ".motion-work-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).context("无法创建电影临时目录")?;
        let mut spool = Self {
            dir,
            w,
            h,
            bin,
            frames: vec![],
            counts: vec![],
            times: vec![],
            original: vec![0.0; w * h],
            decode_ms: 0.0,
            prepare_ms: 0.0,
        };
        let gain = load_gain(p, w, h, up)?;
        // Auto grouping balances the remainder: a one-fraction last frame
        // otherwise has much lower SNR than the rest of a 600–1000-frame EER.
        let groups: Vec<_> = if p.group == 0 && matches!(movie, Movie::Eer(_)) {
            let frames = n.min(40);
            (0..frames)
                .map(|i| (i * n / frames, (i + 1) * n / frames - i * n / frames))
                .collect()
        } else {
            (0..n)
                .step_by(group)
                .map(|start| (start, group.min(n - start)))
                .collect()
        };
        let total = groups.len();
        for (start, count) in groups {
            check_cancel()?;
            let decode = std::time::Instant::now();
            let mut frame = movie.group(start, count, up)?;
            spool.decode_ms += decode.elapsed().as_secs_f64() * 1000.0;
            if let Some(g) = &gain {
                frame.par_iter_mut().zip(g).for_each(|(x, g)| *x *= g);
            }
            if frame.par_iter().any(|x| !x.is_finite()) {
                bail!("电影或 gain 含 NaN/Inf");
            }
            spool
                .original
                .par_iter_mut()
                .zip(&frame)
                .for_each(|(o, v)| *o += v);
            let bytes: Vec<u8> = frame
                .par_chunks(4096)
                .flat_map_iter(|v| v.iter().flat_map(|v| v.to_le_bytes()))
                .collect();
            fs::write(spool.dir.join(format!("frame-{}.f32", spool.len())), bytes)?;
            let mut work = vec![0.0; ww * hh];
            work.par_chunks_mut(ww).enumerate().for_each(|(y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    let mut sum = 0.0;
                    for dy in 0..bin {
                        for dx in 0..bin {
                            sum += frame[(y * bin + dy) * w + x * bin + dx];
                        }
                    }
                    *o = sum / (bin * bin) as f32;
                }
            });
            spool.frames.push(work);
            spool.counts.push(count);
            spool
                .times
                .push(2.0 * (start as f64 + count as f64 / 2.0) / n as f64 - 1.0);
            progress("读取分组", spool.len(), total);
        }
        if spool.len() < 2 {
            bail!("分组后至少需要两帧");
        }
        spool.prepare_ms = now.elapsed().as_secs_f64() * 1000.0;
        Ok(spool)
    }
}

fn load_gain(p: &Params, w: usize, h: usize, up: usize) -> Result<Option<Vec<f32>>> {
    let Some(path) = &p.gain_path else {
        return Ok(None);
    };
    let img = OpenImage::new(0, crate::formats::open(Path::new(path))?, 64 << 20);
    let page = img.page(0)?;
    let g = &page[0];
    let (gw, gh) = if p.gain_rotate.is_multiple_of(2) {
        (g.w, g.h)
    } else {
        (g.h, g.w)
    };
    if !w.is_multiple_of(gw)
        || !h.is_multiple_of(gh)
        || w / gw != h / gh
        || ![1, 2].contains(&(w / gw))
    {
        bail!("gain 尺寸与电影不匹配");
    }
    let scale = w / gw;
    if scale == 2 && up != 2 {
        bail!("gain 必须匹配原始像素尺寸");
    }
    let mut out = vec![0.0; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut x = x / scale;
            let mut y = y / scale;
            if p.gain_flip_x {
                x = gw - 1 - x;
            }
            if p.gain_flip_y {
                y = gh - 1 - y;
            }
            let (sx, sy) = match p.gain_rotate {
                0 => (x, y),
                1 => (y, g.h - 1 - x),
                2 => (g.w - 1 - x, g.h - 1 - y),
                _ => (g.w - 1 - y, x),
            };
            *o = g.data.get_f64(sy * g.w + sx) as f32;
        }
    });
    if out.iter().any(|v| !v.is_finite() || *v < 0.0) {
        bail!("gain 必须为有限非负值");
    }
    Ok(Some(out))
}
