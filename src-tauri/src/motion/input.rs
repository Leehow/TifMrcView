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
    /// A group, and for EER (whole, finite, non-negative counts by
    /// construction) its largest value, so the frame need not be checked.
    fn group_checked(&self, start: usize, count: usize, up: usize) -> Result<(Vec<f32>, Option<f32>)> {
        match self {
            Self::Eer(e) => e.group_max(start, count, up).map(|(v, m)| (v, Some(m))),
            _ => Ok((self.group(start, count, up)?, None)),
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
                    // Retain mmap as an explicit diagnostic comparison. The
                    // sequential source hook leaves non-MRC decoders unchanged.
                    let data = if std::env::var("TMV_MOTION_MRC_IO").as_deref() == Ok("mmap") {
                        img.page(page)?
                    } else {
                        img.movie_page(page)?
                    };
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

/// A full-size frame kept for the final sum: in memory, as counts when
/// they are whole numbers (EER and counting movies without gain: one or
/// two bytes a pixel), else as floats; on disk once the memory budget is used.
enum Stored {
    U8(Vec<u8>),
    U16(Vec<u16>),
    F32(Vec<f32>),
    Disk,
}

/// Memory for full-size frames before they go to disk.
const MEMORY: usize = 6 << 30;
/// Pixels of a movie's frames kept on the GPU (a byte each for counts).
const RESIDENT: usize = 2 << 30;

/// How many frames `prepare` will group a movie into.
fn groups_hint(n: usize, p: &Params, movie: &Movie) -> usize {
    if p.group == 0 && matches!(movie, Movie::Eer(_)) {
        n.min(eer_frames(p.total_dose))
    } else {
        n.div_ceil(p.group.max(1))
    }
}

enum Keep<'a> {
    U8(&'a mut [u8]),
    U16(&'a mut [u16]),
    F32(&'a mut [f32]),
}

impl Stored {
    /// Room for a frame of `len` pixels, in the narrowest type its values
    /// (`whole` numbers up to `max`) fit, or on disk past `MEMORY`.
    fn empty(len: usize, whole: bool, max: f32, used: &mut usize) -> Stored {
        let bytes = if whole && max <= 255.0 { 1 } else if whole && max <= 65535.0 { 2 } else { 4 } * len;
        if *used + bytes > MEMORY {
            return Stored::Disk;
        }
        *used += bytes;
        match bytes / len.max(1) {
            1 => Stored::U8(vec![0; len]),
            2 => Stored::U16(vec![0; len]),
            _ => Stored::F32(vec![0.0; len]),
        }
    }
}

pub struct Spool {
    dir: PathBuf,
    stored: Vec<Stored>,
    used: usize,
    pub w: usize,
    pub h: usize,
    pub frames: Vec<Vec<f32>>,
    pub bin: usize,
    pub counts: Vec<usize>,
    pub times: Vec<f64>,
    pub original: Vec<f32>,
    /// Gain reference the stored frames are multiplied by when read: frames
    /// are kept as recorded (counts fit in a byte), a quarter of the gained
    /// floats in memory and on the way to the GPU.
    pub gain: Option<std::sync::Arc<Vec<f32>>>,
    /// The stored frames already on the GPU, uploaded while the movie before
    /// is being corrected (the way MotionCor keeps a movie on the card): the
    /// sum then cuts its tiles from them without waiting for an upload.
    pub resident: Vec<super::sum_gpu::Resident>,
    pub decode_ms: f64,
    pub prepare_ms: f64,
}
/// A stored full-size frame as recorded, before the gain.
pub enum Raw<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
    F32(std::borrow::Cow<'a, [f32]>),
}
impl Drop for Spool {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
impl Spool {
    /// Frame i with the gain applied.
    pub fn read(&self, i: usize) -> Result<Vec<f32>> {
        let gain = |v: f32, k: usize| self.gain.as_ref().map_or(v, |g| v * g[k]);
        Ok(match self.raw(i)? {
            Raw::U8(v) => v.par_iter().enumerate().map(|(k, &x)| gain(x as f32, k)).collect(),
            Raw::U16(v) => v.par_iter().enumerate().map(|(k, &x)| gain(x as f32, k)).collect(),
            Raw::F32(v) => v.par_iter().enumerate().map(|(k, &x)| gain(x, k)).collect(),
        })
    }
    /// Frame i as recorded, before the gain.
    pub fn raw(&self, i: usize) -> Result<Raw<'_>> {
        match &self.stored[i] {
            Stored::U8(v) => return Ok(Raw::U8(v)),
            Stored::U16(v) => return Ok(Raw::U16(v)),
            Stored::F32(v) => return Ok(Raw::F32(std::borrow::Cow::Borrowed(v))),
            Stored::Disk => {}
        }
        let bytes = fs::read(self.dir.join(format!("frame-{i}.f32")))?;
        if bytes.len() != self.w * self.h * 4 {
            bail!("电影缓存截断");
        }
        Ok(Raw::F32(std::borrow::Cow::Owned(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect())))
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
                n.div_ceil(eer_frames(p.total_dose))
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
        let preview = p.preview_bin > 0;
        let bin = if preview { p.preview_bin } else { w.min(h).div_ceil(p.working_size).max(1) };
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
        let mut spool = Self {
            dir,
            stored: Vec::new(),
            used: 0,
            w,
            h,
            bin,
            frames: vec![],
            counts: vec![],
            times: vec![],
            // The gained full-size sum is only for the diagnostics images.
            original: if preview || !p.diagnostics { vec![] } else { vec![0.0; w * h] },
            gain: None,
            resident: vec![],
            decode_ms: 0.0,
            prepare_ms: 0.0,
        };
        let gain = load_gain(p, w, h, up)?;
        // Whole frames go to the GPU here only for a full correction on the
        // GPU, and only while they take no more than RESIDENT bytes.
        let mut card = (!preview && !p.benchmark && p.backend != "cpu")
            .then(crate::ctf_gpu::device)
            .flatten()
            .map(|(dev, _)| dev)
            .filter(|_| w * h * groups_hint(n, p, movie) <= RESIDENT);
        // Auto grouping balances the remainder: a one-fraction last frame
        // otherwise has much lower SNR than the rest of a 600–1000-frame EER.
        let groups: Vec<_> = if p.group == 0 && matches!(movie, Movie::Eer(_)) {
            let frames = n.min(eer_frames(p.total_dose));
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
        let (mut check_ms, mut fused_ms, mut upload_ms) = (0.0f64, 0.0f64, 0.0f64);
        for (start, count) in groups {
            check_cancel()?;
            let decode = std::time::Instant::now();
            let (mut frame, counted) = movie.group_checked(start, count, up)?;
            spool.decode_ms += decode.elapsed().as_secs_f64() * 1000.0;
            // One pass for the checks, one for everything else: each pass
            // over a 4096² frame is a 64 MB walk through memory.
            let t_check = std::time::Instant::now();
            let (finite, whole, max) = match counted {
                Some(max) => (true, true, max),
                None => frame
                    .par_chunks(1 << 16)
                    .map(|c| c.iter().fold((true, true, 0.0f32), |(f, wh, m), &v| (f && v.is_finite(), wh && v >= 0.0 && v.fract() == 0.0, m.max(v))))
                    .reduce(|| (true, true, 0.0), |a, b| (a.0 && b.0, a.1 && b.1, a.2.max(b.2))),
            };
            if !finite {
                bail!("电影或 gain 含 NaN/Inf");
            }
            // Kept as recorded; the gain is applied when read.
            let mut stored = (!preview).then(|| Stored::empty(frame.len(), whole, max, &mut spool.used));
            if matches!(stored, Some(Stored::Disk)) {
                if !spool.dir.exists() {
                    fs::create_dir(&spool.dir).context("无法创建电影临时目录")?;
                }
                let bytes: Vec<u8> = frame.par_chunks(4096).flat_map_iter(|v| v.iter().flat_map(|v| v.to_le_bytes())).collect();
                fs::write(spool.dir.join(format!("frame-{}.f32", spool.stored.len())), bytes)?;
            }
            check_ms += t_check.elapsed().as_secs_f64() * 1e3; let t_fused = std::time::Instant::now();
            // Rows of `bin` lines: stored, gained, added to the original and
            // binned together while they are in cache.
            let slab = bin * w;
            let mut work = if bin == 1 { vec![] } else { vec![0.0f32; ww * hh] };
            // The frame mean for the GPU upload, summed while the rows are in cache.
            let mut frame_sum = 0.0f64;
            {
                let mut original: &mut [f32] = if preview { &mut [] } else { &mut spool.original };
                let gain = gain.as_deref();
                let mut keep: Option<(&mut [u8], &mut [u16], &mut [f32])> = match stored.as_mut() {
                    Some(Stored::U8(v)) => Some((v, &mut [], &mut [])),
                    Some(Stored::U16(v)) => Some((&mut [], v, &mut [])),
                    Some(Stored::F32(v)) => Some((&mut [], &mut [], v)),
                    _ => None,
                };
                // Split everything into per-slab pieces, then work on them in parallel.
                let rows = frame.len().div_ceil(slab);
                let mut jobs = Vec::with_capacity(rows);
                let mut rest_frame: &mut [f32] = &mut frame;
                let mut rest_work: &mut [f32] = &mut work;
                for r in 0..rows {
                    let take = slab.min(rest_frame.len());
                    let (f, tail) = std::mem::take(&mut rest_frame).split_at_mut(take);
                    rest_frame = tail;
                    let o = if original.is_empty() { None } else {
                        let (o, tail) = std::mem::take(&mut original).split_at_mut(take);
                        original = tail;
                        Some(o)
                    };
                    let k = match keep.as_mut() {
                        Some((a, _, _)) if !a.is_empty() => { let (x, t) = std::mem::take(a).split_at_mut(take); *a = t; Some(Keep::U8(x)) }
                        Some((_, b, _)) if !b.is_empty() => { let (x, t) = std::mem::take(b).split_at_mut(take); *b = t; Some(Keep::U16(x)) }
                        Some((_, _, c)) if !c.is_empty() => { let (x, t) = std::mem::take(c).split_at_mut(take); *c = t; Some(Keep::F32(x)) }
                        _ => None,
                    };
                    let wr = if bin > 1 && r < hh && take == slab {
                        let (x, t) = std::mem::take(&mut rest_work).split_at_mut(ww);
                        rest_work = t;
                        Some(x)
                    } else {
                        None
                    };
                    jobs.push((r * slab, f, o, k, wr));
                }
                frame_sum = jobs.into_par_iter().map(|(at, f, o, k, wr)| {
                    match k {
                        Some(Keep::U8(d)) => d.iter_mut().zip(f.iter()).for_each(|(d, v)| *d = *v as u8),
                        Some(Keep::U16(d)) => d.iter_mut().zip(f.iter()).for_each(|(d, v)| *d = *v as u16),
                        Some(Keep::F32(d)) => d.copy_from_slice(f),
                        None => {}
                    }
                    if let Some(g) = gain {
                        let n = f.len();
                        f.iter_mut().zip(&g[at..at + n]).for_each(|(x, g)| *x *= g);
                    }
                    if let Some(o) = o {
                        o.iter_mut().zip(f.iter()).for_each(|(o, v)| *o += v);
                    }
                    if let Some(row) = wr {
                        for (x, out) in row.iter_mut().enumerate() {
                            let mut sum = 0.0;
                            for dy in 0..bin {
                                for dx in 0..bin {
                                    sum += f[dy * w + x * bin + dx];
                                }
                            }
                            *out = sum / (bin * bin) as f32;
                        }
                    }
                    f.iter().map(|&v| v as f64).sum::<f64>()
                }).sum::<f64>();
            }
            fused_ms += t_fused.elapsed().as_secs_f64() * 1e3; let t_upload = std::time::Instant::now();
            if let Some(stored) = stored {
                spool.stored.push(stored);
                if let Some(dev) = card {
                    let i = spool.stored.len() - 1;
                    let mean = (frame_sum / frame.len() as f64) as f32;
                    let up = spool.raw(i).and_then(|raw| {
                        crate::ctf_gpu::guarded(|| super::sum_gpu::Resident::upload(dev, &raw, mean)).and_then(|r| r)
                    });
                    match up {
                        Ok(r) => spool.resident.push(r),
                        Err(e) => {
                            eprintln!("电影帧常驻 GPU 失败，求和时再上传：{e:#}");
                            spool.resident.clear();
                            card = None;
                        }
                    }
                }
            }
            upload_ms += t_upload.elapsed().as_secs_f64() * 1e3;
            if bin == 1 {
                // Nothing to bin: keep the frame itself, not a copy.
                spool.frames.push(frame);
                spool.counts.push(count);
                spool
                    .times
                    .push(2.0 * (start as f64 + count as f64 / 2.0) / n as f64 - 1.0);
                progress("读取分组", spool.len(), total);
                continue;
            }
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
        if !preview {
            spool.gain = gain;
        }
        spool.prepare_ms = now.elapsed().as_secs_f64() * 1000.0;
        if std::env::var_os("TMV_GPU_PROFILE").is_some() { eprintln!("PREP_PROFILE decode={:.0}ms check={check_ms:.0}ms fused={fused_ms:.0}ms mean+upload={upload_ms:.0}ms total={:.0}ms", spool.decode_ms, spool.prepare_ms); }
        Ok(spool)
    }
}

/// Dose a frame needs for its shift to be measured, e⁻/Å².
const FRAME_DOSE: f64 = 0.4;

/// How many frames an EER movie's fractions are grouped into: about
/// `FRAME_DOSE` each when the movie's dose is known (a tilt image of
/// 3 e⁻/Å² gives 8; in 40 frames each would hold a thirteenth of an electron
/// per Å², and its measured shift would be noise), at least 4 and at most 40;
/// 40 when it is not known.
pub fn eer_frames(total_dose: Option<f64>) -> usize {
    match total_dose {
        Some(d) if d > 0.0 => ((d / FRAME_DOSE).round() as usize).clamp(4, 40),
        _ => 40,
    }
}

/// The gain of a series is the same file for every movie: read, oriented
/// and checked once, then shared.
fn load_gain(p: &Params, w: usize, h: usize, up: usize) -> Result<Option<std::sync::Arc<Vec<f32>>>> {
    use std::sync::{Arc, LazyLock, Mutex};
    type Key = (String, Option<std::time::SystemTime>, u64, usize, usize, usize, u8, bool, bool);
    static LAST: LazyLock<Mutex<Option<(Key, Arc<Vec<f32>>)>>> = LazyLock::new(|| Mutex::new(None));
    let Some(path) = &p.gain_path else {
        return Ok(None);
    };
    let meta = fs::metadata(path).ok();
    let key: Key = (path.clone(), meta.as_ref().and_then(|m| m.modified().ok()), meta.as_ref().map_or(0, |m| m.len()), w, h, up, p.gain_rotate as u8, p.gain_flip_x, p.gain_flip_y);
    if let Some((k, g)) = LAST.lock().unwrap().as_ref() {
        if *k == key {
            return Ok(Some(g.clone()));
        }
    }
    let g = Arc::new(read_gain(path, p, w, h, up)?);
    *LAST.lock().unwrap() = Some((key, g.clone()));
    Ok(Some(g))
}

fn read_gain(path: &str, p: &Params, w: usize, h: usize, up: usize) -> Result<Vec<f32>> {
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
    Ok(out)
}
