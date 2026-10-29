//! Spectra (EELS, EDS…) for the plot: the mean over the chosen pixels of
//! one row (each row a spectrum) or of every Z slice (a spectrum image).

use std::sync::Arc;

use anyhow::{bail, Result};
use rayon::prelude::*;

use crate::image::{OpenImage, Plane};

/// `region` (x, y, w, h) clamped to the image, or an error when empty.
fn clamp(region: [f64; 4], w: usize, h: usize) -> Result<[usize; 4]> {
    let x0 = region[0].max(0.0).floor() as usize;
    let y0 = region[1].max(0.0).floor() as usize;
    let x1 = ((region[0] + region[2]).ceil().max(0.0) as usize).min(w);
    let y1 = ((region[1] + region[3]).ceil().max(0.0) as usize).min(h);
    if x1 <= x0 || y1 <= y0 {
        bail!("区域在图像外");
    }
    Ok([x0, y0, x1 - x0, y1 - y0])
}

fn plane(planes: &[Plane], sample: usize) -> &Plane {
    &planes[sample.min(planes.len() - 1)]
}

/// Mean over the region's pixels of one plane.
fn mean(p: &Plane, [x0, y0, w, h]: [usize; 4]) -> f64 {
    let mut sum = 0.0;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            sum += p.data.get_f64(y * p.w + x);
        }
    }
    sum / (w * h) as f64
}

/// The spectrum of `region` of channel `c`: for axis "x" the mean of the
/// region's rows of slice `z` (all columns), for axis "z" the mean of the
/// region's pixels in every slice.
pub fn values(img: &Arc<OpenImage>, axis: &str, c: usize, z: usize, t: usize, region: [f64; 4]) -> Result<Vec<f64>> {
    let info = img.info();
    let r = clamp(region, info.width, info.height)?;
    match axis {
        "x" => {
            let (page, sample) = info.locate(c, z, t);
            let planes = img.page(page)?;
            let p = plane(&planes, sample);
            let [_, y0, _, h] = r;
            Ok((0..p.w)
                .into_par_iter()
                .map(|x| (y0..y0 + h).map(|y| p.data.get_f64(y * p.w + x)).sum::<f64>() / h as f64)
                .collect())
        }
        "z" => (0..info.size_z)
            .into_par_iter()
            .map(|z| {
                let (page, sample) = info.locate(c, z, t);
                let planes = img.page(page)?;
                Ok(mean(plane(&planes, sample), r))
            })
            .collect(),
        _ => bail!("未知的谱轴 {axis}"),
    }
}
