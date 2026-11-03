//! One orthogonal plane of a Z stack (XY at a Z, XZ at a Y, YZ at an X),
//! rendered with the same contrast as the main view. This is what the
//! four-pane XYZ window shows; it reads one line from each slice instead of
//! building a whole side-view stack.

use std::sync::Arc;

use anyhow::Result;

use crate::image::{ImageInfo, OpenImage, Plane, PlaneData, Source};
use crate::render::{self, RenderReq};

/// `plane` is "xz" (fixed Y) or "yz" (fixed X). `index` is that coordinate.
/// `req.region` is in the plane's pixels: XZ is (X, Z), YZ is (Y, Z).
pub fn render_plane(img: &Arc<OpenImage>, plane: &str, req: &RenderReq) -> Result<Vec<u8>> {
    let (w, h, pages) = extract(img, plane, req.z, req.t)?;
    let mut info = img.info().clone();
    info.width = w;
    info.height = h;
    info.size_z = 1;
    info.size_t = 1;
    info.page_count = pages.len();
    info.order = crate::image::DimOrder::CZT;
    let mem = OpenImage::new(0, Box::new(Mem { info, pages }), 1 << 30);
    let view = RenderReq {
        token: 0,
        z: 0,
        t: 0,
        channels: req.channels.clone(),
        region: req.region,
        out_w: req.out_w,
        out_h: req.out_h,
        gamma: req.gamma,
        invert: req.invert,
        plane: String::new(),
    };
    render::render(&mem, &view)
}

/// The plane's pixels, one page per channel (or one page of every sample).
fn extract(img: &Arc<OpenImage>, plane: &str, index: usize, t: usize) -> Result<(usize, usize, Vec<Vec<Plane>>)> {
    let info = img.info();
    let xz = plane == "xz";
    let (w, h) = if xz { (info.width, info.size_z) } else { (info.height, info.size_z) };
    let index = if xz { index.min(info.height.saturating_sub(1)) } else { index.min(info.width.saturating_sub(1)) };
    let channels: Vec<usize> = (0..info.size_c).collect();
    let es = info.dtype.size();
    // One output plane per channel. Rows are Z.
    let built: Vec<Plane> = channels
        .iter()
        .map(|&c| {
            let mut data = PlaneData::zeroed(info.dtype, w * h);
            let dst = data.as_bytes_mut();
            for z in 0..info.size_z {
                let (page, sample) = info.locate(c, z, t);
                let planes = img.page(page)?;
                let src = &planes[sample.min(planes.len() - 1)];
                let bytes = src.data.as_bytes();
                let row = if xz { row_bytes(bytes, es, src.w, index) } else { col_bytes(bytes, es, src.w, src.h, index) };
                let n = (w * es).min(row.len());
                dst[z * w * es..z * w * es + n].copy_from_slice(&row[..n]);
            }
            Ok(Plane { w, h, data })
        })
        .collect::<Result<Vec<_>>>()?;
    let pages = if info.samples_per_page > 1 {
        vec![built]
    } else {
        built.into_iter().map(|p| vec![p]).collect()
    };
    Ok((w, h, pages))
}

fn row_bytes(src: &[u8], es: usize, width: usize, y: usize) -> Vec<u8> {
    let start = y.saturating_mul(width).saturating_mul(es);
    let end = (start + width * es).min(src.len());
    src.get(start..end).unwrap_or(&[]).to_vec()
}

fn col_bytes(src: &[u8], es: usize, width: usize, height: usize, x: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(height * es);
    for y in 0..height {
        let o = (y * width + x) * es;
        if o + es <= src.len() {
            out.extend_from_slice(&src[o..o + es]);
        }
    }
    out
}

struct Mem {
    info: ImageInfo,
    pages: Vec<Vec<Plane>>,
}

impl Source for Mem {
    fn info(&self) -> &ImageInfo {
        &self.info
    }
    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        let Some(planes) = self.pages.get(page) else { anyhow::bail!("no page") };
        Ok(planes
            .iter()
            .map(|p| {
                let mut data = PlaneData::zeroed(self.info.dtype, p.data.len());
                let n = data.as_bytes().len().min(p.data.as_bytes().len());
                data.as_bytes_mut()[..n].copy_from_slice(&p.data.as_bytes()[..n]);
                Plane { w: p.w, h: p.h, data }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::tests::{fake_info, FakeSource};
    use crate::image::DType;

    #[test]
    fn xz_and_yz_take_the_right_line_from_each_slice() {
        // 3×2, two slices. Slice 0 is 0..6, slice 1 is 10..16, row-major.
        let mut info = fake_info(3, 2, 1, 2, 1, DType::U8);
        info.page_count = 2;
        let pages = vec![vec![(0..6).collect::<Vec<u8>>()], vec![(10..16).collect()]];
        let (src, _) = FakeSource::new(info, pages);
        let img = OpenImage::new(1, Box::new(src), 1 << 20);
        let (w, h, pages) = extract(&img, "xz", 1, 0).unwrap();
        assert_eq!((w, h), (3, 2));
        let row = |z: usize| -> Vec<f64> { (0..3).map(|x| pages[0][0].data.get_f64(z * 3 + x)).collect() };
        assert_eq!(row(0), vec![3.0, 4.0, 5.0]);
        assert_eq!(row(1), vec![13.0, 14.0, 15.0]);
        let (w, h, pages) = extract(&img, "yz", 0, 0).unwrap();
        assert_eq!((w, h), (2, 2));
        let col = |z: usize| -> Vec<f64> { (0..2).map(|y| pages[0][0].data.get_f64(z * 2 + y)).collect() };
        assert_eq!(col(0), vec![0.0, 3.0]);
        assert_eq!(col(1), vec![10.0, 13.0]);
    }
}
