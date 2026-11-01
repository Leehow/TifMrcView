//! The tiles the corrected sum is built from. Each frame is cut into
//! overlapping t × t tiles; a tile moves by the frame's motion at the
//! tile's centre (global shift plus the local model there): the whole-pixel
//! part by where it is cut from the frame, the fraction by a Fourier phase
//! shift. Nothing is resampled, so the sum keeps every spatial frequency
//! (interpolation would damp them, most near Nyquist). Within one tile the
//! smooth local model changes by a small fraction of a pixel, which this
//! leaves out. Tiles are summed over frames in Fourier space, transformed
//! back once, and blended with linear ramps where they overlap.

use super::local::Model;

pub struct Plan {
    pub w: usize,
    pub h: usize,
    /// Tile side (a power of two) and spacing.
    pub t: usize,
    pub stride: usize,
    /// Top-left corner of every tile in the output, row-major, may be negative.
    pub corners: Vec<[isize; 2]>,
}

impl Plan {
    pub fn new(w: usize, h: usize) -> Plan {
        let t = 512usize.min(w.min(h).next_power_of_two()).max(16);
        // The overlap is wide enough for the blend and for fractional shifts to wrap harmlessly.
        let stride = t - (t / 8).max(4);
        let margin = (t - stride) as isize / 2;
        let starts = |n: usize| -> Vec<isize> { (0..n.div_ceil(stride).max(1)).map(|k| (k * stride) as isize - margin).collect() };
        let (xs, ys) = (starts(w), starts(h));
        let corners = ys.iter().flat_map(|&y| xs.iter().map(move |&x| [x, y])).collect();
        Plan { w, h, t, stride, corners }
    }

    pub fn len(&self) -> usize {
        self.corners.len()
    }

    /// Displacement of tile k at a frame's time: global + local model at its centre.
    pub fn shift(&self, k: usize, model: &Model, time: f64, global: [f64; 2]) -> [f64; 2] {
        let c = model.spatial(time);
        let centre = [self.corners[k][0] as f64 + self.t as f64 / 2.0, self.corners[k][1] as f64 + self.t as f64 / 2.0];
        let xx = 2.0 * centre[0] / (self.w - 1).max(1) as f64 - 1.0;
        let yy = 2.0 * centre[1] / (self.h - 1).max(1) as f64 - 1.0;
        let s = [1.0, xx, yy, xx * xx, xx * yy, yy * yy];
        std::array::from_fn(|k| global[k] + (0..6).map(|j| c[k][j] as f64 * s[j]).sum::<f64>())
    }

    /// Tile k of `frame` moved by the whole-pixel part of `s`, into `out`
    /// (t × t), `fill` outside the frame.
    pub fn cut(&self, frame: &[f32], k: usize, s: [f64; 2], fill: f32, out: &mut [f32]) {
        let (t, w, h) = (self.t, self.w as isize, self.h as isize);
        let x0 = self.corners[k][0] + s[0].round() as isize;
        let y0 = self.corners[k][1] + s[1].round() as isize;
        for (j, row) in out.chunks_mut(t).enumerate() {
            let y = y0 + j as isize;
            if y < 0 || y >= h {
                row.fill(fill);
                continue;
            }
            let line = &frame[(y * w) as usize..((y + 1) * w) as usize];
            for (i, o) in row.iter_mut().enumerate() {
                let x = x0 + i as isize;
                *o = if x >= 0 && x < w { line[x as usize] } else { fill };
            }
        }
    }

    /// Blends tiles (each t × t, in tile order) into the w × h output.
    pub fn blend(&self, tiles: &[Vec<f32>]) -> Vec<f32> {
        let (t, w, h) = (self.t, self.w, self.h);
        let ramp = t - self.stride;
        // Weight along one axis: rising over the first `ramp` pixels, falling over the last.
        let edge: Vec<f64> = (0..t).map(|i| (((i.min(t - 1 - i)) as f64 + 0.5) / ramp as f64).min(1.0)).collect();
        let mut sum = vec![0.0f64; w * h];
        let mut weight = vec![0.0f64; w * h];
        for (k, tile) in tiles.iter().enumerate() {
            let [x0, y0] = self.corners[k];
            for j in 0..t {
                let y = y0 + j as isize;
                if y < 0 || y >= h as isize {
                    continue;
                }
                for i in 0..t {
                    let x = x0 + i as isize;
                    if x < 0 || x >= w as isize {
                        continue;
                    }
                    let a = edge[i] * edge[j];
                    let p = y as usize * w + x as usize;
                    sum[p] += a * tile[j * t + i] as f64;
                    weight[p] += a;
                }
            }
        }
        sum.iter().zip(&weight).map(|(s, a)| (s / a.max(1e-12)) as f32).collect()
    }
}
