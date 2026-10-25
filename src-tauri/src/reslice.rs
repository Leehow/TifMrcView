//! Side views of a Z stack: XZ (one row of every slice) and YZ (one column of
//! every slice), served as an ordinary image so the viewer can show them.
use std::sync::Arc;

use anyhow::Result;
use rayon::prelude::*;

use crate::image::{ImageInfo, OpenImage, Plane, PlaneData, Source};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Width X, height Z; the slider walks Y.
    Xz,
    /// Width Y, height Z; the slider walks X.
    Yz,
}

pub struct Reslice {
    base: Arc<OpenImage>,
    axis: Axis,
    info: ImageInfo,
    /// For each page of the side view, its (channel, slider index, time).
    pages: Vec<(usize, usize, usize)>,
}

impl Reslice {
    pub fn new(base: Arc<OpenImage>, axis: Axis) -> Reslice {
        let b = base.info();
        let mut info = b.clone();
        let (w, depth) = match axis {
            Axis::Xz => (b.width, b.height),
            Axis::Yz => (b.height, b.width),
        };
        info.width = w;
        info.height = b.size_z;
        info.size_z = depth;
        info.pixel_size = b.pixel_size.map(|[x, y, z]| match axis {
            Axis::Xz => [x, z, y],
            Axis::Yz => [y, z, x],
        });
        info.page_count = if b.samples_per_page > 1 {
            depth * b.size_t
        } else {
            b.size_c * depth * b.size_t
        };
        // Invert `locate` once so a page number maps back to (c, index, t).
        let mut pages = vec![(0, 0, 0); info.page_count];
        for t in 0..info.size_t {
            for z in 0..depth {
                for c in 0..info.size_c {
                    let (p, _) = info.locate(c, z, t);
                    pages[p] = (c, z, t);
                }
            }
        }
        Reslice { base, axis, info, pages }
    }
}

impl Source for Reslice {
    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        let b = self.base.info();
        let (c0, index, t) = self.pages[page];
        let es = b.dtype.size();
        let (w, h) = (self.info.width, self.info.height);
        // One plane per sample, as the base file stores them.
        let channels: Vec<usize> = if b.samples_per_page > 1 { (0..b.size_c).collect() } else { vec![c0] };
        let mut out: Vec<PlaneData> = channels.iter().map(|_| PlaneData::zeroed(b.dtype, w * h)).collect();
        for (k, &c) in channels.iter().enumerate() {
            // Rows of the side view are the Z slices; read them in parallel.
            let rows: Vec<Result<Vec<u8>>> = (0..b.size_z)
                .into_par_iter()
                .map(|z| {
                    let (p, s) = b.locate(c, z, t);
                    let planes = self.base.page(p)?;
                    let src = planes[s.min(planes.len() - 1)].data.as_bytes();
                    Ok(match self.axis {
                        Axis::Xz => src[index * b.width * es..(index + 1) * b.width * es].to_vec(),
                        Axis::Yz => {
                            let mut row = Vec::with_capacity(b.height * es);
                            for y in 0..b.height {
                                let o = (y * b.width + index) * es;
                                row.extend_from_slice(&src[o..o + es]);
                            }
                            row
                        }
                    })
                })
                .collect();
            let dst = out[k].as_bytes_mut();
            for (z, row) in rows.into_iter().enumerate() {
                let row = row?;
                dst[z * w * es..(z + 1) * w * es].copy_from_slice(&row);
            }
        }
        Ok(out.into_iter().map(|data| Plane { w, h, data }).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::tests::{fake_info, FakeSource};
    use crate::image::DType;

    /// 3x2 image with two Z slices: slice 0 holds 0..6, slice 1 holds 10..16.
    fn base(channels: usize) -> Arc<OpenImage> {
        let mut info = fake_info(3, 2, channels, 2, 1, DType::U8);
        info.pixel_size = Some([1.0, 2.0, 3.0]);
        // CZT order: page = c + channels * z.
        let mut pages = vec![];
        for z in 0..2 {
            for c in 0..channels {
                let off = (z * 10 + c * 100) as u8;
                pages.push(vec![(0..6).map(|i| off + i).collect()]);
            }
        }
        let (src, _) = FakeSource::new(info, pages);
        OpenImage::new(1, Box::new(src), 1 << 20)
    }

    fn values(r: &Reslice, page: usize) -> Vec<f64> {
        let planes = r.read_page(page).unwrap();
        (0..planes[0].data.len()).map(|i| planes[0].data.get_f64(i)).collect()
    }

    #[test]
    fn xz_view_geometry() {
        let r = Reslice::new(base(1), Axis::Xz);
        let i = r.info();
        assert_eq!((i.width, i.height, i.size_z, i.page_count), (3, 2, 2, 2));
        assert_eq!(i.pixel_size, Some([1.0, 3.0, 2.0]));
    }

    #[test]
    fn xz_rows_are_slices() {
        let r = Reslice::new(base(1), Axis::Xz);
        // Slider at y=0: row 0 of each slice; y=1: row 1.
        assert_eq!(values(&r, 0), [0.0, 1.0, 2.0, 10.0, 11.0, 12.0]);
        assert_eq!(values(&r, 1), [3.0, 4.0, 5.0, 13.0, 14.0, 15.0]);
    }

    #[test]
    fn yz_view_columns() {
        let r = Reslice::new(base(1), Axis::Yz);
        let i = r.info();
        assert_eq!((i.width, i.height, i.size_z, i.page_count), (2, 2, 3, 3));
        assert_eq!(i.pixel_size, Some([2.0, 3.0, 1.0]));
        // Slider at x=1: column 1 of each slice, one entry per y.
        assert_eq!(values(&r, 1), [1.0, 4.0, 11.0, 14.0]);
        assert_eq!(values(&r, 2), [2.0, 5.0, 12.0, 15.0]);
    }

    #[test]
    fn multichannel_pages_map_back_to_channels() {
        let r = Reslice::new(base(2), Axis::Xz);
        assert_eq!((r.info().size_c, r.info().page_count), (2, 4));
        // Page = c + 2*y in the view's CZT order.
        assert_eq!(values(&r, 0), [0.0, 1.0, 2.0, 10.0, 11.0, 12.0]);
        assert_eq!(values(&r, 1), [100.0, 101.0, 102.0, 110.0, 111.0, 112.0]);
        assert_eq!(values(&r, 2), [3.0, 4.0, 5.0, 13.0, 14.0, 15.0]);
    }
}
