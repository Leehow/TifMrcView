//! Side views of a Z stack: XZ (one row of every slice) and YZ (one column of
//! every slice), served as an ordinary image so the viewer can show them.
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// Z slices read so far for the page being built, shown as progress.
    done: AtomicUsize,
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
        Reslice { base, axis, info, pages, done: AtomicUsize::new(0) }
    }
}

impl Reslice {
    /// The row (XZ) or column (YZ) of one slice that this view shows.
    fn pick(&self, src: &[u8], index: usize, es: usize) -> Vec<u8> {
        let b = self.base.info();
        match self.axis {
            Axis::Xz => src[index * b.width * es..(index + 1) * b.width * es].to_vec(),
            Axis::Yz => {
                let mut row = Vec::with_capacity(b.height * es);
                for y in 0..b.height {
                    let o = (y * b.width + index) * es;
                    row.extend_from_slice(&src[o..o + es]);
                }
                row
            }
        }
    }
}

impl Source for Reslice {
    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn progress(&self) -> Option<(usize, usize)> {
        let b = self.base.info();
        let total = b.size_z * if b.samples_per_page > 1 { b.size_c } else { 1 };
        Some((self.done.load(Ordering::Relaxed).min(total), total))
    }

    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        let b = self.base.info();
        let (c0, index, t) = self.pages[page];
        let es = b.dtype.size();
        let (w, h) = (self.info.width, self.info.height);
        // One plane per sample, as the base file stores them.
        let channels: Vec<usize> = if b.samples_per_page > 1 { (0..b.size_c).collect() } else { vec![c0] };
        let mut out: Vec<PlaneData> = channels.iter().map(|_| PlaneData::zeroed(b.dtype, w * h)).collect();
        self.done.store(0, Ordering::Relaxed);
        for (k, &c) in channels.iter().enumerate() {
            // Rows of the side view are the Z slices; read them in parallel.
            // A slice already decoded is used as is; an uncompressed file is
            // read straight from the mapping, which touches only the bytes
            // this view needs (one row per slice for XZ); anything else is
            // decoded, which also caches it for the next side slice.
            let rows: Vec<Result<Vec<u8>>> = (0..b.size_z)
                .into_par_iter()
                .map(|z| {
                    let (p, s) = b.locate(c, z, t);
                    let row = match (self.base.cached(p), self.base.source.raw_page(p)) {
                        (None, Some((src, little))) => {
                            let mut row = self.pick(src, index, es);
                            if little != cfg!(target_endian = "little") {
                                row.chunks_exact_mut(es).for_each(|v| v.reverse());
                            }
                            row
                        }
                        (cached, _) => {
                            let planes = match cached {
                                Some(planes) => planes,
                                None => self.base.page(p)?,
                            };
                            self.pick(planes[s.min(planes.len() - 1)].data.as_bytes(), index, es)
                        }
                    };
                    self.done.fetch_add(1, Ordering::Relaxed);
                    Ok(row)
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
