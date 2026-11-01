//! The projectors of `recon.rs` on the GPU through Burn. A slab of Y rows
//! is one upload; per view, the detector (back projection) or depth-row
//! (forward projection) sampling positions are computed on the device and
//! applied to all rows of the slab with two gathers. SIRT iterates on the
//! device; one download per slab.

use super::recon::{ramp, Geometry, Method, Params, COVERED, SEEN};
use anyhow::{bail, Result};
use burn::tensor::{Device, Int, Tensor, TensorData};

/// Two-point linear interpolation: indices and weights (zero outside).
struct Taps {
    i0: Tensor<1, Int>,
    i1: Tensor<1, Int>,
    w0: Tensor<2>,
    w1: Tensor<2>,
}

/// Taps for positions `pos` along lines of length `len` whose samples are
/// `stride` apart in the flattened source, starting at `base`.
fn taps(pos: Tensor<1>, len: usize, stride: usize, base: Option<Tensor<1>>) -> Taps {
    let w = len;
    let lo = pos.clone().floor();
    let f = pos - lo.clone();
    let inside = |t: Tensor<1>| {
        t.clone()
            .greater_equal_elem(0.0)
            .bool_and(t.lower_equal_elem((w - 1) as f32))
            .float()
    };
    let hi = lo.clone() + 1.0;
    let w0 = (f.clone().neg() + 1.0) * inside(lo.clone());
    let w1 = f * inside(hi.clone());
    let index = |t: Tensor<1>| {
        let t = t.clamp(0.0, (w - 1) as f32).mul_scalar(stride as f32);
        match &base {
            Some(b) => (t + b.clone()).int(),
            None => t.int(),
        }
    };
    Taps {
        i0: index(lo),
        i1: index(hi),
        w0: w0.unsqueeze_dim(0),
        w1: w1.unsqueeze_dim(0),
    }
}

fn gather(src: Tensor<2>, t: &Taps) -> Tensor<2> {
    src.clone().select(1, t.i0.clone()) * t.w0.clone() + src.select(1, t.i1.clone()) * t.w1.clone()
}

pub struct Plan {
    dev: Device,
    g: Geometry,
    p: Params,
    /// u = x − (w−1)/2 and depth v of every voxel, flattened [z][x].
    u: Tensor<1>,
    v: Tensor<1>,
    /// Column-stepping views: per (step m, detector j) the column k and
    /// j − (w−1)/2, as floats; k outside the slice is masked.
    cols: Vec<Option<(Tensor<1>, Tensor<1>, usize)>>,
    ramp: Tensor<3>,
    /// 1 / A1 and 1 / B1 where covered, else 0.
    inv_rows: Tensor<2>,
    inv_cols: Tensor<2>,
}

impl Plan {
    pub fn new(dev: &Device, g: &Geometry, p: &Params) -> Result<Plan> {
        let (w, z) = (g.w, g.z);
        let h = (w - 1) as f32 / 2.0;
        let up = |v: Vec<f32>| Tensor::<1>::from_data(TensorData::new(v.clone(), [v.len()]), dev);
        let u = up((0..z * w).map(|i| (i % w) as f32 - h).collect());
        let v = up((0..z * w).map(|i| g.depth(i / w) as f32).collect());
        let n = (2 * w).next_power_of_two();
        let r = ramp(n, p.filter);
        let len = r.len();
        let mut plan = Plan {
            dev: dev.clone(),
            g: g.clone(),
            p: p.clone(),
            cols: (0..g.views())
                .map(|v| {
                    (!g.by_rows(v)).then(|| {
                        let m = g.columns(v, 0).1;
                        let k: Vec<f32> = (0..m * w)
                            .map(|i| (g.columns(v, i % w).0 + (i / w) as isize) as f32)
                            .collect();
                        (
                            up(k),
                            up((0..m * w).map(|i| (i % w) as f32 - h).collect()),
                            m,
                        )
                    })
                })
                .collect(),
            u,
            v,
            ramp: Tensor::<1>::from_data(TensorData::new(r, [len]), dev).reshape([1, 1, len]),
            inv_rows: Tensor::zeros([1, 1], dev),
            inv_cols: Tensor::zeros([1, 1], dev),
        };
        if p.method != Method::Wbp {
            let inv = |t: Tensor<2>, least: f32| {
                let short = t.clone().lower_elem(least.max(COVERED));
                t.clamp_min(COVERED).recip().mask_fill(short, 0.0)
            };
            let rows = plan
                .forward(Tensor::ones([1, z * w], dev))
                .reshape([g.views(), w]);
            let longest = rows.clone().max_dim(1).mul_scalar(SEEN).clamp_min(COVERED);
            let short = rows.clone().lower(longest.expand([g.views(), w]));
            plan.inv_rows = rows
                .clamp_min(COVERED)
                .recip()
                .mask_fill(short, 0.0)
                .reshape([1, g.views() * w]);
            let ones = vec![1.0; g.views()];
            let cols = plan.back(Tensor::ones([1, g.views() * w], dev), &ones);
            let seen = SEEN * cols.clone().max().into_scalar::<f32>();
            plan.inv_cols = inv(cols, seen);
        }
        Ok(plan)
    }

    /// B: [n][views·w] → [n][z·w].
    fn back(&self, sino: Tensor<2>, scale: &[f64]) -> Tensor<2> {
        let (w, n) = (self.g.w, sino.dims()[0]);
        let mut out = Tensor::<2>::zeros([n, self.g.z * w], &self.dev);
        for k in 0..self.g.views() {
            out = out
                + self
                    .back_view(k, sino.clone().narrow(1, k * w, w))
                    .mul_scalar(scale[k] as f32);
        }
        out
    }

    /// B for one view: [n][w] → [n][z·w].
    fn back_view(&self, k: usize, row: Tensor<2>) -> Tensor<2> {
        let w = self.g.w;
        let [c, s] = self.g.trig[k];
        let d = self.u.clone().mul_scalar(c as f32)
            + self.v.clone().mul_scalar(s as f32)
            + (w - 1) as f32 / 2.0;
        gather(row, &taps(d, w, 1, None))
    }

    /// A: [n][z·w] → [n][views·w]; rows of `u`/`v` are reused for the
    /// (depth row z, detector j) pairs since both are z × w grids.
    fn forward(&self, slice: Tensor<2>) -> Tensor<2> {
        let views = (0..self.g.views())
            .map(|k| self.forward_view(k, slice.clone()))
            .collect();
        Tensor::cat(views, 1)
    }

    /// A for one view: [n][z·w] → [n][w].
    fn forward_view(&self, k: usize, slice: Tensor<2>) -> Tensor<2> {
        let (w, z, n) = (self.g.w, self.g.z, slice.dims()[0]);
        let h = (w - 1) as f32 / 2.0;
        let rows = self.row_starts();
        let [c, s] = self.g.trig[k];
        let cols = &self.cols[k];
        {
            if let Some((k, t, m)) = cols {
                // Step through columns: depth = (j − h − c·(k − h))/s, as a row index.
                let depth = (t.clone() - (k.clone() - h).mul_scalar(c as f32)).div_scalar(s as f32);
                let row = depth - self.g.z_offset as f32 + (z - 1) as f32 / 2.0;
                let inside = k
                    .clone()
                    .greater_equal_elem(0.0)
                    .bool_and(k.clone().lower_elem(w as f32))
                    .float()
                    .unsqueeze_dim(0);
                let mut tp = taps(row, z, w, Some(k.clone().clamp(0.0, (w - 1) as f32)));
                tp.w0 = tp.w0 * inside.clone();
                tp.w1 = tp.w1 * inside;
                gather(slice.clone(), &tp)
                    .reshape([n, *m, w])
                    .sum_dim(1)
                    .reshape([n, w])
                    .div_scalar(s.abs() as f32)
            } else {
                // Step through depth rows: x = (j − h − s·v)/c + h.
                let x =
                    (self.u.clone() - self.v.clone().mul_scalar(s as f32)).div_scalar(c as f32) + h;
                let t = taps(x, w, 1, Some(rows.clone()));
                gather(slice.clone(), &t)
                    .reshape([n, z, w])
                    .sum_dim(1)
                    .reshape([n, w])
                    .div_scalar(c.abs() as f32)
            }
        }
    }

    /// Start of each depth row, per (z, j).
    fn row_starts(&self) -> Tensor<1> {
        let w = self.g.w as f32;
        (self.v.clone() - self.g.z_offset as f32 + (self.g.z - 1) as f32 / 2.0)
            .mul_scalar(w)
            .round()
    }

    /// One slab of `n` rows, [n][views][w] → [n][z][w].
    pub fn slab(&self, sino: &[f32], n: usize) -> Result<Vec<f32>> {
        let (w, nv) = (self.g.w, self.g.views());
        if sino.len() != n * nv * w {
            bail!("投影尺寸不一致");
        }
        let b = Tensor::<1>::from_data(TensorData::new(sino.to_vec(), [sino.len()]), &self.dev);
        let x = match self.p.method {
            Method::Wbp => {
                let len = (2 * w).next_power_of_two();
                let (re, im) = burn::signal::rfft(b.reshape([n, nv, w]), 2, Some(len));
                let f = burn::signal::irfft(
                    re * self.ramp.clone(),
                    im * self.ramp.clone(),
                    2,
                    Some(len),
                );
                let f = f.narrow(2, 0, w).reshape([n, nv * w]);
                let scale: Vec<f64> = self.g.weight.iter().map(|k| k / nv as f64).collect();
                self.back(f, &scale)
            }
            Method::Sart => {
                let b = b.reshape([n, nv * w]);
                let mut x = Tensor::<2>::zeros([n, self.g.z * w], &self.dev);
                let order = super::recon::sart_order(nv);
                for _ in 0..self.p.iterations {
                    super::check_cancel()?;
                    for &k in &order {
                        let e = (b.clone().narrow(1, k * w, w) - self.forward_view(k, x.clone()))
                            * self.inv_rows.clone().narrow(1, k * w, w);
                        x = x + self.back_view(k, e).mul_scalar(self.p.lambda);
                        if self.p.non_negative {
                            x = x.clamp_min(0.0);
                        }
                    }
                }
                x
            }
            Method::Sirt => {
                let b = b.reshape([n, nv * w]);
                let ones = vec![1.0; nv];
                let mut x = Tensor::<2>::zeros([n, self.g.z * w], &self.dev);
                for _ in 0..self.p.iterations {
                    super::check_cancel()?;
                    let e = (b.clone() - self.forward(x.clone())) * self.inv_rows.clone();
                    x = x + (self.back(e, &ones) * self.inv_cols.clone()).mul_scalar(self.p.lambda);
                    if self.p.non_negative {
                        x = x.clamp_min(0.0);
                    }
                }
                x
            }
        };
        Ok(x.into_data()
            .convert::<f32>()
            .try_into_vec::<f32>()
            .unwrap())
    }
}
