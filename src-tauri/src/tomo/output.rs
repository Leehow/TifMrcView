//! Bilinear affine resampling into a vertical-axis stack; float32 MRC and
//! IMOD affine sidecars. .xf/.tlt rows follow included, sorted tilt order;
//! .views records the corresponding zero-based source sections explicitly.

use super::{check_cancel, geometry, track::sample, validate_bin, Alignment};
use crate::image::OpenImage;
use anyhow::{bail, Result};
use serde::Deserialize;
use std::{
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReq {
    pub token: u64,
    pub c: usize,
    pub t: usize,
    pub alignment: Alignment,
    pub bin: usize,
    pub path: String,
    #[serde(default)]
    pub transforms_only: bool,
}
pub fn validate(img: &Arc<OpenImage>, a: &Alignment, bin: usize) -> Result<()> {
    validate_bin(bin)?;
    let info = img.info();
    if a.width != info.width
        || a.height != info.height
        || a.views.len() != info.size_z
        || !a.axis.is_finite()
    {
        bail!("对齐几何与叠层尺寸不匹配");
    }
    let mut seen = vec![false; info.size_z];
    for v in &a.views {
        if v.file_index >= seen.len()
            || seen[v.file_index]
            || !v.angle.is_finite()
            || v.angle.abs() >= 89.5
            || !v.mag.is_finite()
            || !(0.5..=2.0).contains(&v.mag)
            || v.shift.iter().any(|v| !v.is_finite())
        {
            bail!("对齐变换无效");
        }
        seen[v.file_index] = true;
    }
    if a.views.iter().filter(|v| !v.excluded).count() < 3 {
        bail!("至少保留三个倾角");
    }
    Ok(())
}

pub fn aligned(
    img: &Arc<OpenImage>,
    a: &Alignment,
    bin: usize,
    c: usize,
    t: usize,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<(Vec<f32>, usize, usize, Vec<f64>)> {
    aligned_flipped(img, a, bin, c, t, None, progress)
}

/// CTF to phase flip with: the fits, the optics, the stack's pixel size (Å).
pub type FlipWith<'a> = (&'a super::dataset::Ctf, &'a super::dataset::Optics, f64);

/// Aligned views, phase flipped when `ctf` is given.
pub fn aligned_flipped(
    img: &Arc<OpenImage>,
    a: &Alignment,
    bin: usize,
    c: usize,
    t: usize,
    ctf: Option<FlipWith>,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<(Vec<f32>, usize, usize, Vec<f64>)> {
    validate(img, a, bin)?;
    let info = img.info();
    if c >= info.size_c || t >= info.size_t {
        bail!("通道或时间点无效");
    }
    let (w, h) = (info.width / bin, info.height / bin);
    if w.saturating_mul(h).saturating_mul(info.size_z) > 1 << 29 {
        bail!("对齐叠层超过 2 GiB，请提高 binning");
    }
    let mut stack = Vec::new();
    let mut angles = Vec::new();
    for (i, v) in a.views.iter().filter(|v| !v.excluded).enumerate() {
        check_cancel()?;
        let r = geometry::rotation(a.axis + v.rotation);
        let (page, s) = info.locate(c, v.file_index, t);
        let pp = img.page(page)?;
        let pp = &pp[s.min(pp.len() - 1)];
        let src: Vec<f32> = (0..pp.w * pp.h)
            .map(|i| pp.data.get_f64(i) as f32)
            .collect();
        // Outside the recorded image: the view's mean, so moved image edges
        // do not become steps that back-project into streaks along Y.
        let mean = (src.iter().map(|&v| v as f64).sum::<f64>() / src.len() as f64) as f32;
        let fill = |x: f64, y: f64| {
            if x < 0.0 || y < 0.0 || x > (pp.w - 1) as f64 || y > (pp.h - 1) as f64 {
                mean
            } else {
                sample(&src, pp.w, pp.h, x, y)
            }
        };
        let mut plane = vec![0.0; w * h];
        use rayon::prelude::*;
        plane.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let mut sum = 0.0;
                // Box integration in output coordinates prevents binning aliasing.
                for dy in 0..bin {
                    for dx in 0..bin {
                        let u = (x * bin + dx) as f64 + (info.width - w * bin) as f64 / 2.0
                            - (info.width - 1) as f64 / 2.0;
                        let vv = (y * bin + dy) as f64 + (info.height - h * bin) as f64 / 2.0
                            - (info.height - 1) as f64 / 2.0;
                        let sx = v.mag * (r[0][0] * u + r[0][1] * vv)
                            + v.shift[0]
                            + (info.width - 1) as f64 / 2.0;
                        let sy = v.mag * (r[1][0] * u + r[1][1] * vv)
                            + v.shift[1]
                            + (info.height - 1) as f64 / 2.0;
                        sum += fill(sx, sy);
                    }
                }
                *o = sum / (bin * bin) as f32;
            }
        });
        if let Some((fits, optics, pixel_a)) = ctf {
            if let Some(f) = fits.tilts.iter().find(|f| f.z == v.file_index) {
                // The aligned view shows the stack turned by ψ = 90° − axis.
                let psi = 90.0 - (a.axis + v.rotation);
                super::ctf::flip(&mut plane, w, h, &super::ctf::Flip {
                    optics: crate::ctf::Optics::of(optics.kv, optics.cs_mm, optics.amplitude),
                    df1: f.df1,
                    df2: f.df2,
                    angle: (f.angle - psi).to_radians(),
                    theta: v.refined(),
                    gradient: fits.gradient,
                    pixel_a: pixel_a * bin as f64,
                });
            }
        }
        stack.extend(plane);
        angles.push(v.refined());
        progress(i + 1, info.size_z);
    }
    Ok((stack, w, h, angles))
}

/// Remove incomplete files on errors/cancellation. Never truncate the source.
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<std::fs::File>) -> Result<()>,
) -> Result<()> {
    let part = path.with_extension(format!(
        "{}.tomo-part",
        path.extension().and_then(|s| s.to_str()).unwrap_or("out")
    ));
    let mut f = BufWriter::new(
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)?,
    );
    let guard = TempFile(part.clone());
    write(&mut f)?;
    check_cancel()?;
    f.flush()?;
    f.get_ref().sync_all()?;
    drop(f);
    std::fs::rename(&part, path)?;
    drop(guard);
    Ok(())
}

pub fn write_mrc(
    path: &str,
    data: &[f32],
    shape: [usize; 3],
    pixel_a: f64,
    yzx: bool,
) -> Result<()> {
    let [w, h, z] = shape;
    if data.len() != w * h * z || data.iter().any(|v| !v.is_finite()) {
        bail!("输出体积无效");
    }
    let count = data.len() as f64;
    let mean = data.iter().map(|&v| v as f64).sum::<f64>() / count;
    let rms = (data.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / count).sqrt();
    let lo = data.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = data.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    atomic_write(Path::new(path), |f| {
        f.write_all(&crate::align::mrc_header(
            w,
            h,
            z,
            pixel_a,
            [lo, hi, mean as f32, rms as f32],
            "TifMrcView Pro: single-axis tomography",
        ))?;
        for k in 0..z {
            check_cancel()?;
            for y in 0..h {
                for x in 0..w {
                    let i = if yzx {
                        (y * z + k) * w + x
                    } else {
                        (k * h + y) * w + x
                    };
                    f.write_all(&data[i].to_le_bytes())?;
                }
            }
        }
        Ok(())
    })
}

pub fn export(a: &Alignment, path: &str) -> Result<()> {
    let mut xf = String::new();
    let mut tlt = String::new();
    let mut map = String::new();
    for v in a.views.iter().filter(|v| !v.excluded) {
        let x = super::export::xf_row(a, v);
        xf.push_str(&format!("{:.9} {:.9} {:.9} {:.9} {:.6} {:.6}\n", x[0], x[1], x[2], x[3], x[4], x[5]));
        tlt.push_str(&format!("{:.6}\n", v.refined()));
        map.push_str(&format!("{}\n", v.file_index));
    }
    for (ext, txt) in [("xf", xf), ("tlt", tlt), ("views", map)] {
        atomic_write(&Path::new(path).with_extension(ext), |f| {
            f.write_all(txt.as_bytes())?;
            Ok(())
        })?;
    }
    Ok(())
}

/// Prefer the calibrated image header, then the mdoc calibration.
pub fn pixel_a(img: &Arc<OpenImage>) -> f64 {
    if img.info().pixel_size.is_some() {
        return crate::align::pixel_a(img.info());
    }
    geometry::metadata(&img.info().path, img.info().size_z)
        .ok()
        .and_then(|m| m.pixel_a)
        .unwrap_or(1.0)
}

fn protect_source(img: &Arc<OpenImage>, path: &str) -> Result<()> {
    let source = Path::new(&img.info().path).canonicalize()?;
    if Path::new(path).canonicalize().is_ok_and(|p| p == source) {
        bail!("不能覆盖输入叠层");
    }
    let dest = Path::new(path);
    let source_parent = source.parent().unwrap().canonicalize()?;
    let dest_parent = dest.parent().unwrap_or(Path::new(".")).canonicalize()?;
    if source_parent == dest_parent && source.file_stem() == dest.file_stem() {
        bail!("输出必须使用不同文件名，避免覆盖输入倾转角文件");
    }
    Ok(())
}
pub fn save(
    img: &Arc<OpenImage>,
    req: &SaveReq,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<()> {
    validate(img, &req.alignment, req.bin)?;
    protect_source(img, &req.path)?;
    if !req.transforms_only {
        let (data, w, h, angles) = aligned(img, &req.alignment, req.bin, req.c, req.t, progress)?;
        write_mrc(
            &req.path,
            &data,
            [w, h, angles.len()],
            pixel_a(img) * req.bin as f64,
            false,
        )?;
    }
    export(&req.alignment, &req.path)
}

/// A float32 MRC volume written slab by slab: XY sections along Z, filled
/// one band of Y rows at a time. Statistics go into the header at the end.
pub struct VolumeFile {
    file: std::fs::File,
    part: PathBuf,
    path: PathBuf,
    shape: [usize; 3],
    pixel_a: f64,
    stats: [f64; 4],
    count: usize,
    done: bool,
}

const HEADER: u64 = 1024;

impl VolumeFile {
    pub fn create(path: &str, shape: [usize; 3], pixel_a: f64) -> Result<VolumeFile> {
        let path = PathBuf::from(path);
        let part = path.with_extension(format!("{}.tomo-part", path.extension().and_then(|s| s.to_str()).unwrap_or("out")));
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&part)?;
        let [w, h, z] = shape;
        file.set_len(HEADER + (w * h * z * 4) as u64)?;
        Ok(VolumeFile { file, part, path, shape, pixel_a, stats: [f64::INFINITY, f64::NEG_INFINITY, 0.0, 0.0], count: 0, done: false })
    }

    /// Rows y0..y0+n of every section, given as [n][z][w].
    pub fn write_rows(&mut self, y0: usize, n: usize, slab: &[f32]) -> Result<()> {
        use std::io::{Seek, SeekFrom};
        let [w, h, z] = self.shape;
        if slab.len() != n * z * w || y0 + n > h {
            bail!("重构分块尺寸不一致");
        }
        let mut band = vec![0u8; n * w * 4];
        for k in 0..z {
            for y in 0..n {
                for x in 0..w {
                    let v = slab[(y * z + k) * w + x];
                    if !v.is_finite() {
                        bail!("重构结果含 NaN/Inf");
                    }
                    let d = v as f64;
                    self.stats[0] = self.stats[0].min(d);
                    self.stats[1] = self.stats[1].max(d);
                    self.stats[2] += d;
                    self.stats[3] += d * d;
                    band[(y * w + x) * 4..(y * w + x + 1) * 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            self.file.seek(SeekFrom::Start(HEADER + ((k * h + y0) * w * 4) as u64))?;
            self.file.write_all(&band)?;
        }
        self.count += n * z * w;
        Ok(())
    }

    pub fn finish(mut self) -> Result<String> {
        use std::io::{Seek, SeekFrom};
        let [w, h, z] = self.shape;
        if self.count != w * h * z {
            bail!("重构未写完");
        }
        let n = self.count as f64;
        let mean = self.stats[2] / n;
        let rms = (self.stats[3] / n - mean * mean).max(0.0).sqrt();
        let header = crate::align::mrc_header(w, h, z, self.pixel_a, [self.stats[0] as f32, self.stats[1] as f32, mean as f32, rms as f32], "TifMrcView Pro: single-axis tomography");
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header)?;
        self.file.sync_all()?;
        std::fs::rename(&self.part, &self.path)?;
        self.done = true;
        Ok(self.path.to_string_lossy().into_owned())
    }
}

impl Drop for VolumeFile {
    fn drop(&mut self) {
        if !self.done {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}
