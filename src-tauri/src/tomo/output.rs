//! Bilinear affine resampling into a vertical-axis stack; float32 MRC and
//! IMOD affine sidecars. .xf/.tlt rows follow included, sorted tilt order;
//! .views records the corresponding zero-based source sections explicitly.

use super::{check_cancel, geometry, recon, track::sample, validate_bin, Alignment};
use crate::image::OpenImage;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
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
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconReq {
    pub token: u64,
    pub c: usize,
    pub t: usize,
    pub alignment: Alignment,
    pub bin: usize,
    pub params: recon::Params,
    /// One output per input token: [main, optional even/odd inputs...].
    pub paths: Vec<String>,
    #[serde(default)]
    pub additional_tokens: Vec<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconResult {
    pub paths: Vec<String>,
    pub backend: Vec<String>,
    pub ms: f64,
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
    validate(img, a, bin)?;
    let info = img.info();
    if c >= info.size_c || t >= info.size_t {
        bail!("通道或时间点无效");
    }
    let (w, h) = (info.width / bin, info.height / bin);
    if w.saturating_mul(h).saturating_mul(info.size_z) > 1 << 29 {
        bail!("对齐叠层超过 2 GiB，请提高 binning");
    }
    let r = geometry::rotation(a.axis);
    let mut stack = Vec::new();
    let mut angles = Vec::new();
    for (i, v) in a.views.iter().filter(|v| !v.excluded).enumerate() {
        check_cancel()?;
        let (page, s) = info.locate(c, v.file_index, t);
        let pp = img.page(page)?;
        let pp = &pp[s.min(pp.len() - 1)];
        let src: Vec<f32> = (0..pp.w * pp.h)
            .map(|i| pp.data.get_f64(i) as f32)
            .collect();
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
                        sum += sample(&src, pp.w, pp.h, sx, sy);
                    }
                }
                *o = sum / (bin * bin) as f32;
            }
        });
        stack.extend(plane);
        angles.push(v.angle);
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
    let r = geometry::rotation(a.axis);
    let mut xf = String::new();
    let mut tlt = String::new();
    let mut map = String::new();
    for v in a.views.iter().filter(|v| !v.excluded) {
        let aa = [
            r[0][0] / v.mag,
            r[1][0] / v.mag,
            r[0][1] / v.mag,
            r[1][1] / v.mag,
        ];
        let dx = -aa[0] * v.shift[0] - aa[1] * v.shift[1];
        let dy = -aa[2] * v.shift[0] - aa[3] * v.shift[1];
        xf.push_str(&format!(
            "{:.9} {:.9} {:.9} {:.9} {:.6} {:.6}\n",
            aa[0], aa[1], aa[2], aa[3], dx, dy
        ));
        tlt.push_str(&format!("{:.6}\n", v.angle));
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

pub fn reconstruct(
    imgs: &[Arc<OpenImage>],
    req: &ReconReq,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<ReconResult> {
    let now = std::time::Instant::now();
    if imgs.len() != req.paths.len() || imgs.is_empty() {
        bail!("每个输入叠层必须有一个输出路径");
    }
    for (i, path) in req.paths.iter().enumerate() {
        if req.paths[..i].contains(path) {
            bail!("输出路径重复");
        }
        for img in imgs {
            protect_source(img, path)?;
        }
    }
    let mut backends = Vec::new();
    for (img, path) in imgs.iter().zip(&req.paths) {
        validate(img, &req.alignment, req.bin)?;
        let (stack, w, h, angles) = aligned(img, &req.alignment, req.bin, req.c, req.t, progress)?;
        let nv = angles.len();
        let mut sino = vec![0.0; stack.len()];
        for y in 0..h {
            for i in 0..nv {
                sino[(y * nv + i) * w..(y * nv + i + 1) * w]
                    .copy_from_slice(&stack[(i * h + y) * w..(i * h + y + 1) * w]);
            }
        }
        drop(stack);
        let op = recon::Operator::new(w, req.params.thickness, &angles, req.params.z_offset)?;
        let r = recon::reconstruct(&sino, h, &op, &req.params, progress)?;
        write_mrc(
            path,
            &r.data,
            [w, h, req.params.thickness],
            pixel_a(img) * req.bin as f64,
            true,
        )?;
        export(&req.alignment, path)?;
        backends.push(r.backend);
    }
    Ok(ReconResult {
        paths: req.paths.clone(),
        backend: backends,
        ms: now.elapsed().as_secs_f64() * 1000.0,
    })
}
