//! A whole Z stack as one small 8-bit volume for the Pro 3D view. The stack
//! is box-averaged down to a voxel budget so it fits in a GPU 3D texture.

use std::sync::Arc;

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::Deserialize;

use crate::image::{OpenImage, Px};
use crate::with_plane;

#[derive(Deserialize)]
pub struct VolChan {
    pub index: usize,
    pub min: Option<f32>,
    pub max: Option<f32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeReq {
    pub token: u64,
    pub t: usize,
    pub channels: Vec<VolChan>,
    /// Largest number of voxels per channel.
    pub max_voxels: usize,
}

/// Largest side of a 3D texture that every desktop GPU accepts.
const MAX_SIDE: usize = 2048;

/// XY and Z reduction factors: shrink XY first, and Z only while the stack
/// keeps at least 64 slices.
pub fn factors(w: usize, h: usize, d: usize, max_voxels: usize) -> (usize, usize) {
    let fits = |f: usize, fz: usize| {
        let (a, b, c) = (w.div_ceil(f), h.div_ceil(f), d.div_ceil(fz));
        a <= MAX_SIDE && b <= MAX_SIDE && c <= MAX_SIDE && a * b * c <= max_voxels.max(1)
    };
    let mut f = 1;
    loop {
        let fz = f.min(d.div_ceil(64)).max(1);
        if fits(f, fz) || (w.div_ceil(f) == 1 && h.div_ceil(f) == 1) {
            return (f, fz);
        }
        f += 1;
    }
}

/// Layout of the reply: `u32 width, height, depth, channels`, `f32` voxel
/// size x, y, z (relative; 1 when the file does not say), then per channel
/// `f32 min, max` used for scaling followed by `width*height*depth` bytes.
pub fn volume(img: &Arc<OpenImage>, req: &VolumeReq) -> Result<Vec<u8>> {
    let info = img.info();
    let (w, h, d) = (info.width, info.height, info.size_z);
    if d < 2 {
        bail!("这张图只有一层，没有三维数据");
    }
    let (f, fz) = factors(w, h, d, req.max_voxels);
    let (ow, oh, od) = (w.div_ceil(f), h.div_ceil(f), d.div_ceil(fz));
    let px = info.pixel_size.unwrap_or([1.0, 1.0, 1.0]);
    let pz = if px[2] > 0.0 { px[2] } else { px[0] };
    let mut out = Vec::with_capacity(28 + req.channels.len() * (8 + ow * oh * od));
    for v in [ow, oh, od, req.channels.len()] {
        out.extend_from_slice(&(v as u32).to_le_bytes());
    }
    for v in [px[0] * f as f64, px[1] * f as f64, pz * fz as f64] {
        out.extend_from_slice(&(v as f32).to_le_bytes());
    }
    for c in &req.channels {
        // Default range: the auto contrast of the middle slice.
        let (mid, sample) = info.locate(c.index, d / 2, req.t);
        let stats = img.stats(mid, sample, &img.page(mid)?);
        let (alo, ahi) = if info.is_rgb { (0.0, 255.0) } else { stats.auto_range() };
        let lo = c.min.unwrap_or(alo);
        let hi = c.max.unwrap_or(ahi);
        let scale = 255.0 / (hi - lo).max(f32::MIN_POSITIVE);
        out.extend_from_slice(&lo.to_le_bytes());
        out.extend_from_slice(&hi.to_le_bytes());
        let mut voxels = vec![0u8; ow * oh * od];
        voxels.par_chunks_mut(ow * oh).enumerate().try_for_each(|(oz, slab)| -> Result<()> {
            let mut acc = vec![0f32; ow * oh];
            let mut count = vec![0u32; ow * oh];
            for z in oz * fz..((oz + 1) * fz).min(d) {
                if img.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    bail!("cancelled");
                }
                let (page, sample) = info.locate(c.index, z, req.t);
                let planes = img.page(page)?;
                let plane = &planes[sample.min(planes.len() - 1)];
                with_plane!(&plane.data, v => {
                    for y in 0..h.min(plane.h) {
                        let row = &v[y * plane.w..y * plane.w + w.min(plane.w)];
                        let base = (y / f) * ow;
                        for (x, &p) in row.iter().enumerate() {
                            let i = base + x / f;
                            acc[i] += p.to_f32();
                            count[i] += 1;
                        }
                    }
                });
            }
            for ((o, a), n) in slab.iter_mut().zip(&acc).zip(&count) {
                let mean = if *n > 0 { a / *n as f32 } else { 0.0 };
                *o = ((mean - lo) * scale).clamp(0.0, 255.0) as u8;
            }
            Ok(())
        })?;
        out.extend_from_slice(&voxels);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::tests::{fake_info, FakeSource};
    use crate::image::DType;

    #[test]
    fn stack_becomes_scaled_volume() {
        // 4x4x3 stack, slice z filled with 50 * z.
        let pages = (0..3).map(|z| vec![vec![50 * z as u8; 16]]).collect();
        let (src, _) = FakeSource::new(fake_info(4, 4, 1, 3, 1, DType::U8), pages);
        let img = OpenImage::new(1, Box::new(src), 1 << 20);
        let req = VolumeReq { token: 1, t: 0, channels: vec![VolChan { index: 0, min: Some(0.0), max: Some(100.0) }], max_voxels: 1 << 20 };
        let out = volume(&img, &req).unwrap();
        let u = |i: usize| u32::from_le_bytes(out[i..i + 4].try_into().unwrap());
        assert_eq!((u(0), u(4), u(8), u(12)), (4, 4, 3, 1));
        let data = &out[28 + 8..];
        assert_eq!(data.len(), 48);
        assert_eq!((data[0], data[16], data[32]), (0, 127, 255));
    }

    #[test]
    fn single_slice_is_an_error() {
        let (src, _) = FakeSource::new(fake_info(4, 4, 1, 1, 1, DType::U8), vec![vec![vec![0; 16]]]);
        let img = OpenImage::new(1, Box::new(src), 1 << 20);
        let req = VolumeReq { token: 1, t: 0, channels: vec![VolChan { index: 0, min: None, max: None }], max_voxels: 1 << 20 };
        assert!(volume(&img, &req).is_err());
    }

    #[test]
    fn small_stacks_are_kept_whole() {
        assert_eq!(factors(256, 256, 100, 64 << 20), (1, 1));
    }

    #[test]
    fn large_stacks_shrink_xy_before_z() {
        let (f, fz) = factors(4096, 4096, 300, 128 << 20);
        assert!(f >= 2);
        assert!(300usize.div_ceil(fz) >= 64);
        assert!(4096usize.div_ceil(f).pow(2) * 300usize.div_ceil(fz) <= 128 << 20);
    }

    #[test]
    fn texture_side_limit() {
        let (f, _) = factors(10000, 100, 10, usize::MAX);
        assert!(10000usize.div_ceil(f) <= MAX_SIDE);
    }
}
