//! GPU evolution of the native NAD tensor field, using the existing CubeCL
//! device. Tensor eigensystems stay in f64 on the CPU: Metal has no native f64,
//! and f32 eigensystems change the nearly degenerate reference tensor.
use anyhow::{bail, Result};
use burn::tensor::{Device, Tensor as BurnTensor};
use burn_cubecl::{cubecl, CubeBackend};
use cubecl::{client::Client, prelude::*, server::Handle};
use rayon::prelude::*;

// Compensated two-float arithmetic for the reference's double intermediates in
// stencil weights. No fast-math reassociation/contraction is enabled. Explicit
// fma is used only to recover the residual of a rounded multiply/square root.
#[cube]
fn add_wide(ah: f32, al: f32, bh: f32, bl: f32) -> (f32, f32) {
    let s = ah + bh;
    let v = s - ah;
    let error = (ah - (s - v)) + (bh - v);
    let t = error + (al + bl);
    let hi = s + t;
    (hi, t - (hi - s))
}

#[cube]
fn mul_wide(hi: f32, lo: f32, factor: f32) -> (f32, f32) {
    let product = hi * factor;
    let error = fma(hi, factor, -product) + lo * factor;
    let result = product + error;
    (result, error - (result - product))
}

#[cube]
fn root_wide(value: f32) -> (f32, f32) {
    let root = value.sqrt();
    let mut error = 0.0;
    if value > 0.0 {
        error = fma(-root, root, value) / (2.0 * root);
    }
    (root, error)
}

#[cube]
fn weight(tensor: &[f32], i: usize, j: usize, dx: i32, dy: i32, dz: i32) -> (f32, f32, f32) {
    let count = usize::cast_from(dx != 0) + usize::cast_from(dy != 0) + usize::cast_from(dz != 0);
    if count == 1 {
        let axis = if dx != 0 {
            0usize
        } else if dy != 0 {
            1usize
        } else {
            2usize
        };
        let mut hi = 0.05f32 * (tensor[i * 6 + axis] + tensor[j * 6 + axis]);
        let mut lo = 0.0f32;
        #[unroll]
        for off in 3usize..6usize {
            if (axis == 0 && off != 5) || (axis == 1 && off != 4) || (axis == 2 && off != 3) {
                let a = tensor[i * 6 + off];
                let b = tensor[j * 6 + off];
                let (ah, al) = root_wide(a * a);
                let (bh, bl) = root_wide(b * b);
                let (sum, residual) = add_wide(ah, al, bh, bl);
                let (term, tail) = mul_wide(sum, residual, -0.025f32);
                let (result, rest) = add_wide(hi, lo, term, tail);
                hi = result;
                lo = rest;
            }
        }
        (hi + lo, hi + (lo - 1.0e-12), hi + (lo + 1.0e-12))
    } else {
        let off = if dz == 0 {
            3usize
        } else if dy == 0 {
            4usize
        } else {
            5usize
        };
        let sign = if dz == 0 {
            dx * dy
        } else if dy == 0 {
            dx * dz
        } else {
            dy * dz
        };
        let a = tensor[i * 6 + off];
        let b = tensor[j * 6 + off];
        let (ah, al) = root_wide(a * a);
        let (bh, bl) = root_wide(b * b);
        let (sum, residual) = add_wide(f32::cast_from(sign) * (a + b), 0.0, ah, al);
        let (sum, residual) = add_wide(sum, residual, bh, bl);
        let (result, rest) = mul_wide(sum, residual, 0.025f32);
        (
            result + rest,
            result + (rest - 1.0e-12),
            result + (rest + 1.0e-12),
        )
    }
}

#[cube(launch)]
fn tmv_nad_eed_evolve(
    image: &[f32],
    tensor: &[f32],
    output: &mut [f32],
    repairs: &mut [u32],
    w: usize,
    h: usize,
    d: usize,
) {
    let i = ABSOLUTE_POS;
    if i < image.len() {
        let x = i % w;
        let y = i / w % h;
        let z = i / (w * h);
        let center = image[i];
        let mut value = center;
        let mut repair = center.abs() < 1.0e-20;
        #[unroll]
        for n in 0..18 {
            let dx = if n == 0 || n == 6 || n == 9 || n == 12 || n == 13 {
                1
            } else if n == 1 || n == 7 || n == 8 || n == 14 || n == 15 {
                -1
            } else {
                0
            };
            let dy = if n == 2 || n == 6 || n == 8 || n == 16 || n == 17 {
                1
            } else if n == 3 || n == 7 || n == 9 || n == 10 || n == 11 {
                -1
            } else {
                0
            };
            let dz = if n == 5 || n == 11 || n == 13 || n == 15 || n == 17 {
                1
            } else if n == 4 || n == 10 || n == 12 || n == 14 || n == 16 {
                -1
            } else {
                0
            };
            let px = i32::cast_from(x) + dx;
            let py = i32::cast_from(y) + dy;
            let pz = i32::cast_from(z) + dz;
            let ghost = z + 1 == d && n == 11;
            if !(z + 1 == d && n == 10)
                && px >= 0
                && px < i32::cast_from(w)
                && py >= 0
                && py < i32::cast_from(h)
                && ((pz >= 0 && pz < i32::cast_from(d)) || ghost)
            {
                let j = (usize::cast_from(pz) % d * h + usize::cast_from(py)) * w
                    + usize::cast_from(px);
                let (weight, lower, upper) = weight(tensor, i, j, dx, dy, dz);
                let difference = image[j] - center;
                let updated = value + weight * difference;
                // The compensated weight's conservative absolute interval is
                // far wider than its rounding error for |tensor| <= 2. If its
                // ends could change this rounded fp32 update, recompute this
                // voxel on the CPU before it can enter the next eigensystem.
                repair = repair
                    || value + lower * difference != updated
                    || value + upper * difference != updated;
                value = updated;
            }
        }
        output[i] = value;
        repairs[i] = u32::cast_from(repair);
    }
}

pub(crate) struct Evolution {
    client: Client,
    output: Handle,
    repairs: Handle,
    dims: [usize; 3],
    len: usize,
    pub repaired_voxels: usize,
}

impl Evolution {
    pub(crate) fn new(device: &Device, dims: [usize; 3]) -> Result<Self> {
        // The application selects one shared GPU. Qualify its actual compiler
        // and driver once; other backends fall back before processing user data.
        static QUALIFIED: std::sync::OnceLock<std::result::Result<(), String>> =
            std::sync::OnceLock::new();
        let qualification = QUALIFIED.get_or_init(|| {
            (|| -> Result<()> {
                let dims = [11, 9, 7];
                let input: Vec<f32> = (0..11 * 9 * 7)
                    .map(|i| ((i * 37 % 101) as f32 - 50.0) * 0.02)
                    .collect();
                let expected = crate::nad_eed::diffuse(input.clone(), dims, 0.8, 3, &|| Ok(()))?;
                let mut gpu = Self::allocate(device, dims)?;
                let actual = crate::nad_eed::diffuse_with_step(
                    input,
                    dims,
                    0.8,
                    3,
                    &|| Ok(()),
                    &mut |image, tensor, next| gpu.step(image, tensor, next),
                )?;
                if actual != expected {
                    bail!("NAD GPU 数值自检未通过");
                }
                Ok(())
            })()
            .map_err(|error| format!("{error:#}"))
        });
        if let Err(error) = qualification {
            bail!("{error}");
        }
        Self::allocate(device, dims)
    }

    fn allocate(device: &Device, dims: [usize; 3]) -> Result<Self> {
        let len = dims
            .iter()
            .try_fold(1usize, |n, &v| n.checked_mul(v))
            .ok_or_else(|| anyhow::anyhow!("NAD GPU 体尺寸过大"))?;
        // At most 512 MiB of device working storage; larger inputs retain the
        // CPU implementation instead of risking the shared training device.
        if dims.contains(&0) || len > (512 << 20) / 36 {
            bail!("NAD GPU 工作区超过内存预算");
        }
        let primitive = BurnTensor::<1>::zeros([1], device)
            .try_into_primitive::<CubeBackend>()
            .map_err(|_| anyhow::anyhow!("NAD GPU 后端不支持自定义计算"))?;
        let client = primitive.client;
        let output = client.empty(len * 4);
        let repairs = client.empty(len * 4);
        Ok(Self {
            client,
            output,
            repairs,
            dims,
            len,
            repaired_voxels: 0,
        })
    }

    pub(crate) fn step(
        &mut self,
        image: &[f32],
        tensor: &[[f32; 6]],
        output: &mut [f32],
    ) -> Result<()> {
        if image.len() != self.len || tensor.len() != self.len || output.len() != self.len {
            bail!("NAD GPU 数据尺寸不一致");
        }
        if tensor
            .as_flattened()
            .iter()
            .any(|v| !v.is_finite() || v.abs() > 2.0)
        {
            bail!("NAD GPU 张量超出精度保护范围");
        }
        let src = self.client.create_from_slice(f32::as_bytes(image));
        let fields = self
            .client
            .create_from_slice(f32::as_bytes(tensor.as_flattened()));
        let groups = self.len.div_ceil(128) as u32;
        tmv_nad_eed_evolve::launch(
            &self.client,
            CubeCount::Static(groups.min(1024), groups.div_ceil(1024), 1),
            CubeDim::new_1d(128),
            // Each handle contains exactly the contiguous f32 count declared
            // here. The kernel bounds-checks its absolute index before access.
            unsafe { BufferArg::from_raw_parts(src, self.len) },
            unsafe { BufferArg::from_raw_parts(fields, self.len * 6) },
            unsafe { BufferArg::from_raw_parts(self.output.clone(), self.len) },
            unsafe { BufferArg::from_raw_parts(self.repairs.clone(), self.len) },
            self.dims[0],
            self.dims[1],
            self.dims[2],
        );
        let buffers = self
            .client
            .read(vec![self.output.clone(), self.repairs.clone()])?;
        if buffers.len() != 2 || buffers.iter().any(|b| b.len() != self.len * 4) {
            bail!("NAD GPU 读取结果尺寸不一致");
        }
        for (out, bytes) in output.iter_mut().zip(buffers[0].chunks_exact(4)) {
            *out = f32::from_ne_bytes(bytes.try_into().unwrap());
        }
        let flags = &buffers[1];
        self.repaired_voxels += flags
            .chunks_exact(4)
            .filter(|b| b.iter().any(|v| *v != 0))
            .count();
        output.par_iter_mut().enumerate().for_each(|(i, out)| {
            if flags[i * 4..i * 4 + 4].iter().any(|v| *v != 0) {
                *out = crate::nad_eed::cpu_point(image, tensor, self.dims, i);
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path, time::Instant};

    fn floats(path: &Path, offset: usize) -> Vec<f32> {
        fs::read(path).unwrap()[offset..]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect()
    }

    #[test]
    #[ignore = "requires an actual GPU; run explicitly for native acceptance"]
    fn hardware_evolution_matches_original_volumes() {
        let (device, name) = crate::ctf_gpu::device().expect("a real GPU is required");
        eprintln!("NAD GPU hardware backend: {name}");
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nad3d");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        let mut exact = true;
        for case in manifest["cases"].as_array().unwrap() {
            let dims = std::array::from_fn(|i| case["shape"][i].as_u64().unwrap() as usize);
            let input = floats(&root.join(case["input"].as_str().unwrap()), 0);
            let mut gpu = Evolution::new(device, dims).unwrap();
            for reference in case["references"].as_array().unwrap() {
                let expected = floats(&root.join(reference["output"].as_str().unwrap()), 0);
                let out = crate::nad_eed::diffuse_with_step(
                    input.clone(),
                    dims,
                    (reference["kAbsolute"].as_f64().unwrap() as f32).max(f32::MIN_POSITIVE),
                    reference["iterations"].as_u64().unwrap() as usize,
                    &|| Ok(()),
                    &mut |image, tensor, next| gpu.step(image, tensor, next),
                )
                .unwrap();
                let max = out
                    .iter()
                    .zip(&expected)
                    .map(|(&a, &b)| (a - b).abs())
                    .fold(0.0, f32::max);
                eprintln!("{} {} maxdelta={max}", case["name"], reference["output"]);
                exact &= out == expected;
            }
        }
        if let Ok(directory) = std::env::var("TMV_NAD_GPU_REAL_FIXTURE") {
            let directory = Path::new(&directory);
            let input = floats(&directory.join("real-roi.mrc"), 1024);
            let dims = [128, 128, 32];
            let mut gpu = Evolution::new(device, dims).unwrap();
            for (strength, n) in [(0.1f32, 5), (0.8, 5), (3.5, 2), (0.1, 20)] {
                let expected = floats(
                    &directory.join(format!("real-imod-k{strength}-n{n}.mrc")),
                    1024,
                );
                let start = Instant::now();
                let out = crate::nad_eed::diffuse_with_step(
                    input.clone(),
                    dims,
                    strength * 0.9518798,
                    n,
                    &|| Ok(()),
                    &mut |image, tensor, next| gpu.step(image, tensor, next),
                )
                .unwrap();
                let elapsed = start.elapsed().as_secs_f64();
                let max = out
                    .iter()
                    .zip(&expected)
                    .map(|(&a, &b)| (a - b).abs())
                    .fold(0.0, f32::max);
                let rms = (out
                    .iter()
                    .zip(&expected)
                    .map(|(&a, &b)| ((a - b) as f64).powi(2))
                    .sum::<f64>()
                    / out.len() as f64)
                    .sqrt();
                eprintln!(
                    "REAL NAD GPU k={strength} n={n} maxdelta={max} rms={rms} seconds={elapsed}"
                );
                fs::write(
                    directory.join(format!("real-gpu-k{strength}-n{n}.f32")),
                    f32::as_bytes(&out),
                )
                .unwrap();
                exact &= out == expected;
            }
        }
        assert!(
            exact,
            "GPU outputs differ from the original-program goldens; inspect all recorded deltas"
        );
    }
}
