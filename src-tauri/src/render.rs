//! Turns raw planes into display RGBA for exactly the on-screen region and
//! resolution, so the cost and the IPC payload scale with the window, not
//! with the image.

use std::sync::Arc;

use anyhow::Result;
use rayon::prelude::*;
use serde::Deserialize;

use crate::image::{DType, OpenImage, Px};
use crate::with_plane;

#[derive(Deserialize, Clone)]
pub struct ChanReq {
    pub index: usize,
    pub color: [u8; 3],
    pub min: Option<f32>,
    pub max: Option<f32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderReq {
    pub token: u64,
    pub z: usize,
    pub t: usize,
    pub channels: Vec<ChanReq>,
    /// Source rectangle in image pixels: x, y, width, height.
    pub region: [f64; 4],
    pub out_w: usize,
    pub out_h: usize,
    #[serde(default = "one")]
    pub gamma: f32,
    #[serde(default)]
    pub invert: bool,
}

fn one() -> f32 {
    1.0
}

/// Layout of the reply: `u32 width, u32 height, u32 channels`, then per
/// channel `f32 min, f32 max, f32 data_min, f32 data_max`, then RGBA bytes.
pub fn render(img: &Arc<OpenImage>, req: &RenderReq) -> Result<Vec<u8>> {
    let info = img.info();
    let (iw, ih) = (info.width, info.height);
    let out_w = req.out_w.clamp(1, 16384);
    let out_h = req.out_h.clamp(1, 16384);
    let [sx, sy, sw, sh] = req.region;
    let sw = sw.max(1e-6);
    let sh = sh.max(1e-6);

    struct Ch {
        planes: crate::image::Page,
        sample: usize,
        lo: f32,
        inv: f32,
        color: [f32; 3],
    }
    let mut header = Vec::with_capacity(12 + req.channels.len() * 16);
    let mut chans = Vec::new();
    for c in &req.channels {
        let (page, sample) = info.locate(c.index, req.z, req.t);
        let planes = img.page(page)?;
        let stats = img.stats(page, sample, &planes);
        let (alo, ahi) = if info.is_rgb {
            (0.0, 255.0)
        } else if matches!(info.dtype, DType::F32 | DType::F64) {
            stats.auto_range()
        } else {
            let (lo, hi) = stats.auto_range();
            (lo.floor(), hi.ceil())
        };
        let lo = c.min.unwrap_or(alo);
        let hi = c.max.unwrap_or(ahi);
        for f in [lo, hi, stats.min, stats.max] {
            header.extend_from_slice(&f.to_le_bytes());
        }
        chans.push(Ch {
            planes,
            sample,
            lo,
            inv: 1.0 / (hi - lo).max(f32::MIN_POSITIVE),
            color: c.color.map(|v| v as f32),
        });
    }

    // Source column span for each output column, with a sampling stride that
    // keeps strong downscales cheap.
    let span = |o: usize, n: usize, s0: f64, sl: f64, lim: usize| {
        let a = (s0 + o as f64 * sl / n as f64).floor().max(0.0) as usize;
        let b = (s0 + (o + 1) as f64 * sl / n as f64).floor() as usize;
        let a = a.min(lim - 1);
        let b = b.clamp(a + 1, lim);
        let step = (b - a).div_ceil(6).max(1);
        (a, b, step)
    };
    let cols: Vec<(usize, usize, usize)> = (0..out_w).map(|o| span(o, out_w, sx, sw, iw)).collect();

    let mut out = vec![0u8; 12 + header.len() + out_w * out_h * 4];
    out[0..4].copy_from_slice(&(out_w as u32).to_le_bytes());
    out[4..8].copy_from_slice(&(out_h as u32).to_le_bytes());
    out[8..12].copy_from_slice(&(chans.len() as u32).to_le_bytes());
    out[12..12 + header.len()].copy_from_slice(&header);
    let pixels = &mut out[12 + header.len()..];
    let gamma = req.gamma;
    let invert = req.invert;

    pixels.par_chunks_mut(out_w * 4).enumerate().for_each_init(
        || (vec![0f32; out_w], vec![0f32; out_w * 3]),
        |(vals, acc), (oy, row)| {
            let (y0, y1, ystep) = span(oy, out_h, sy, sh, ih);
            acc.iter_mut().for_each(|a| *a = 0.0);
            for ch in &chans {
                let plane = &ch.planes[ch.sample.min(ch.planes.len() - 1)];
                with_plane!(&plane.data, v => gather(v, plane.w, y0, y1, ystep, &cols, vals));
                for (x, &val) in vals.iter().enumerate() {
                    let mut f = ((val - ch.lo) * ch.inv).clamp(0.0, 1.0);
                    if gamma != 1.0 {
                        f = f.powf(gamma);
                    }
                    let a = &mut acc[x * 3..x * 3 + 3];
                    a[0] += f * ch.color[0];
                    a[1] += f * ch.color[1];
                    a[2] += f * ch.color[2];
                }
            }
            for (x, px) in row.chunks_exact_mut(4).enumerate() {
                for k in 0..3 {
                    let v = acc[x * 3 + k].min(255.0) as u8;
                    px[k] = if invert { 255 - v } else { v };
                }
                px[3] = 255;
            }
        },
    );
    Ok(out)
}

/// Box-averages the source footprint of each output pixel in one row.
#[inline]
fn gather<T: Px>(
    v: &[T],
    w: usize,
    y0: usize,
    y1: usize,
    ystep: usize,
    cols: &[(usize, usize, usize)],
    out: &mut [f32],
) {
    let single = y1 - y0 == 1 && cols.iter().all(|c| c.1 - c.0 == 1);
    if single {
        let row = &v[y0 * w..(y0 + 1) * w];
        for (o, c) in out.iter_mut().zip(cols) {
            *o = row[c.0].to_f32();
        }
        return;
    }
    for (o, &(x0, x1, xstep)) in out.iter_mut().zip(cols) {
        let mut sum = 0.0f32;
        let mut n = 0u32;
        let mut y = y0;
        while y < y1 {
            let row = &v[y * w..(y + 1) * w];
            let mut x = x0;
            while x < x1 {
                sum += row[x].to_f32();
                n += 1;
                x += xstep;
            }
            y += ystep;
        }
        *o = sum / n.max(1) as f32;
    }
}

/// Raw values of every channel at one pixel.
pub fn pixel_values(img: &Arc<OpenImage>, z: usize, t: usize, x: usize, y: usize) -> Result<Vec<f64>> {
    let info = img.info();
    if x >= info.width || y >= info.height {
        return Ok(vec![]);
    }
    (0..info.size_c)
        .map(|c| {
            let (page, sample) = info.locate(c, z, t);
            let planes = img.page(page)?;
            let p = &planes[sample.min(planes.len() - 1)];
            Ok(p.data.get_f64(y * p.w + x))
        })
        .collect()
}

/// Small preview of the middle Z slice with all channels composited.
pub fn thumbnail(img: &Arc<OpenImage>, size: usize) -> Result<Vec<u8>> {
    let info = img.info();
    let scale = (size as f64 / info.width.max(info.height) as f64).min(1.0);
    let ow = ((info.width as f64 * scale).round() as usize).max(1);
    let oh = ((info.height as f64 * scale).round() as usize).max(1);
    let channels = info
        .channels
        .iter()
        .enumerate()
        .take(if info.is_rgb { 3 } else { 4 })
        .map(|(i, c)| ChanReq { index: i, color: c.color, min: None, max: None })
        .collect();
    let req = RenderReq {
        token: img.token,
        z: info.size_z / 2,
        t: 0,
        channels,
        region: [0.0, 0.0, info.width as f64, info.height as f64],
        out_w: ow,
        out_h: oh,
        gamma: 1.0,
        invert: false,
    };
    render(img, &req)
}
