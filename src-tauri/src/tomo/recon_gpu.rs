//! Burn projection batches. Geometry stays on the device, all Y rows in a
//! memory-bounded slab share gather/scatter indices. Iterations never read
//! back tensors; one volume download per slab, no per-view/iteration sync.

use super::{
    check_cancel,
    recon::{self, Method, Operator, Params, Reconstruction},
};
use anyhow::{bail, Result};
use burn::tensor::{Device, Int, Tensor, TensorData};

struct GpuOperator {
    idx: Tensor<1, Int>,
    local: Tensor<4, Int>,
    w: usize,
    z: usize,
    wt: Tensor<2>,
    views: usize,
    rays: usize,
    vox: usize,
}
impl GpuOperator {
    fn new(dev: &Device, op: &Operator) -> Self {
        Self {
            local: Tensor::from_data(
                TensorData::new(
                    op.indices.iter().map(|&i| (i % op.w) as i32).collect(),
                    [1, op.views, op.z, 2 * op.w],
                ),
                dev,
            ),
            w: op.w,
            z: op.z,
            idx: Tensor::from_data(
                TensorData::new(
                    op.indices.iter().map(|&i| i as i32).collect(),
                    [op.indices.len()],
                ),
                dev,
            ),
            wt: Tensor::from_data(
                TensorData::new(op.weights.clone(), [1, op.weights.len()]),
                dev,
            ),
            views: op.views,
            rays: op.views * op.w,
            vox: op.w * op.z,
        }
    }
    fn forward(&self, x: Tensor<2>, rows: usize) -> Tensor<2> {
        let dev = x.device();
        let v = (x
            .reshape([rows, 1, self.vox, 1])
            .repeat_dim(1, self.views)
            .repeat_dim(3, 2)
            .reshape([rows, self.views * self.vox * 2])
            * self.wt.clone())
        .reshape([rows, self.views, self.z, 2 * self.w]);
        // Scatter only 2X samples per work unit, with Y × view × Z units.
        // select_assign on a flattened volume serialises all voxels per Y.
        Tensor::zeros([rows, self.views, self.z, self.w], &dev)
            .scatter(
                3,
                self.local.clone().repeat_dim(0, rows),
                v,
                burn::tensor::ops::IndexingUpdateOp::Add,
            )
            .sum_dim(2)
            .reshape([rows, self.rays])
    }
    fn back(&self, b: Tensor<2>, rows: usize) -> Tensor<2> {
        (b.select(1, self.idx.clone()) * self.wt.clone())
            .reshape([rows, self.views, self.vox, 2])
            .sum_dim(3)
            .sum_dim(1)
            .reshape([rows, self.vox])
    }
}

pub fn reconstruct(
    dev: &Device,
    b: &[f32],
    rows: usize,
    op: &Operator,
    p: &Params,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Reconstruction> {
    p.validate(op.w, rows, op.views)?;
    let (rays, vox) = (op.w * op.views, op.w * op.z);
    if b.len() != rows * rays {
        bail!("投影尺寸不一致");
    }
    let source = if p.method == Method::Wbp {
        recon::filtered(b, op.w, p.filter, p.iterations)
    } else {
        b.to_vec()
    };
    let g = GpuOperator::new(dev, op);
    let rs = g.forward(Tensor::ones([1, vox], dev), 1).clamp_min(1e-6);
    let cs = g.back(Tensor::ones([1, rays], dev), 1).clamp_min(1e-6);
    // Four large intermediates fit in ~256 MiB when geometry permits it.
    let batch = ((1usize << 24) / (2 * vox * op.views)).clamp(1, rows);
    let mut out = Vec::with_capacity(rows * vox);
    let steps = if p.method == Method::Wbp {
        1
    } else {
        p.iterations
    };
    for start in (0..rows).step_by(batch) {
        check_cancel()?;
        let k = batch.min(rows - start);
        let bb = Tensor::<2>::from_data(
            TensorData::new(source[start * rays..(start + k) * rays].to_vec(), [k, rays]),
            dev,
        );
        let mut x = Tensor::zeros([k, vox], dev);
        if p.method == Method::Wbp {
            x = g.back(bb, k).div_scalar(op.views as f32);
        } else {
            for it in 0..steps {
                check_cancel()?;
                let e = (bb.clone() - g.forward(x.clone(), k)) / rs.clone();
                x = x + (g.back(e, k) / cs.clone()).mul_scalar(p.lambda);
                if p.non_negative {
                    x = x.clamp_min(0.0);
                }
                progress(start * steps + k * (it + 1), rows * steps);
            }
        }
        out.extend(
            x.into_data()
                .convert::<f32>()
                .try_into_vec::<f32>()
                .unwrap(),
        );
        progress((start + k) * steps, rows * steps);
    }
    Ok(Reconstruction {
        data: out,
        residuals: Vec::new(),
        backend: "Burn".into(),
    })
}
