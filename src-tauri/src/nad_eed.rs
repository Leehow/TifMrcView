//! In-process 3-D edge-enhancing nonlinear anisotropic diffusion.
//!
//! Independent implementation of the tensor diffusion equation in Frangakis &
//! Hegerl, JSB 135 (2001), doi:10.1006/jsbi.2001.4406. Compatibility targets the
//! EED-only IMOD algorithm (sigma=0, dt=0.1); no IMOD code/library is linked.
use anyhow::{bail, Result};
use rayon::prelude::*;

// Symmetric tensor layout: xx, yy, zz, xy, xz, yz.
type Tensor = [f32; 6];
const PAIRS: [(usize, usize, usize); 3] = [(0, 1, 3), (0, 2, 4), (1, 2, 5)];
// Opposite directions are evaluated from the same old iterate (explicit step).
const NEIGHBORS: [[isize; 3]; 18] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [1, 1, 0],
    [-1, -1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [0, -1, -1],
    [0, -1, 1],
    [1, 0, -1],
    [1, 0, 1],
    [-1, 0, -1],
    [-1, 0, 1],
    [0, 1, -1],
    [0, 1, 1],
];

fn workspace<T: Clone>(len: usize, value: T) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len)?;
    v.resize(len, value);
    Ok(v)
}

/// Symmetric eigensystem via Householder tridiagonalization and implicit-shift
/// QL rotations. This is the standard orthogonal similarity algorithm, written
/// here for a fixed3x3 matrix; eigenvectors are returned in descending order.
fn eigensystem(t: Tensor) -> [[f32; 3]; 3] {
    let [xx, yy, zz, xy, xz, yz] = t.map(f64::from);
    let mut q = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let mut diagonal = [xx, yy, zz];
    let mut off = [xy, yz, 0.0];
    let square = xy * xy + xz * xz;
    let first = if xy > 0.0 {
        -square.sqrt()
    } else {
        square.sqrt()
    };
    let denominator = square - first * xy;
    if denominator > 0.0 {
        let u = [xy - first, xz];
        let reciprocal = 1.0 / denominator;
        let product = [yy * u[0] + yz * u[1], yz * u[0] + zz * u[1]];
        let correction = 0.5 * reciprocal * reciprocal * (u[0] * product[0] + u[1] * product[1]);
        let p = [
            reciprocal * product[0] - correction * u[0],
            reciprocal * product[1] - correction * u[1],
        ];
        diagonal[1] -= 2.0 * p[0] * u[0];
        diagonal[2] -= 2.0 * p[1] * u[1];
        off[0] = first;
        off[1] = off[1] - p[0] * u[1] - u[0] * p[1];
        for i in 0..2 {
            for j in 0..2 {
                q[i + 1][j + 1] -= reciprocal * u[j] * u[i];
            }
        }
    }
    for l in 0..2 {
        for _ in 0..40 {
            let mut m = l;
            while m < 2 {
                let magnitude = diagonal[m].abs() + diagonal[m + 1].abs();
                if off[m].abs() + magnitude == magnitude {
                    break;
                }
                m += 1;
            }
            if m == l {
                break;
            }
            let shift = (diagonal[l + 1] - diagonal[l]) / (2.0 * off[l]);
            let radius = (shift * shift + 1.0).sqrt();
            let mut g = diagonal[m] - diagonal[l]
                + off[l] / (shift + if shift >= 0.0 { radius } else { -radius });
            let (mut sine, mut cosine, mut correction) = (1.0, 1.0, 0.0);
            for i in (l..m).rev() {
                let (f, b) = (sine * off[i], cosine * off[i]);
                if f.abs() >= g.abs() {
                    cosine = g / f;
                    let radius = (cosine * cosine + 1.0).sqrt();
                    off[i + 1] = f * radius;
                    sine = 1.0 / radius;
                    cosine *= sine;
                } else {
                    sine = f / g;
                    let radius = (sine * sine + 1.0).sqrt();
                    off[i + 1] = g * radius;
                    cosine = 1.0 / radius;
                    sine *= cosine;
                }
                g = diagonal[i + 1] - correction;
                let delta = (diagonal[i] - g) * sine + 2.0 * cosine * b;
                correction = sine * delta;
                diagonal[i + 1] = g + correction;
                g = cosine * delta - b;
                for row in &mut q {
                    let next = row[i + 1];
                    row[i + 1] = sine * row[i] + cosine * next;
                    row[i] = cosine * row[i] - sine * next;
                }
            }
            diagonal[l] -= correction;
            off[l] = g;
            off[m] = 0.0;
        }
    }
    let mut order = [0, 1, 2];
    for i in 0..2 {
        let mut largest = i;
        for j in i + 1..3 {
            if diagonal[order[j]] >= diagonal[order[largest]] {
                largest = j;
            }
        }
        order.swap(i, largest);
    }
    order.map(|i| [q[0][i] as f32, q[1][i] as f32, q[2][i] as f32])
}

fn diffusivity(g: [f32; 3], k: f32) -> Tensor {
    let norm = ((g[0] * g[0] + g[1] * g[1] + g[2] * g[2]) as f64).sqrt() as f32;
    let lambda = if norm == 0.0 {
        1.0
    } else {
        (1.0 - (-3.31488 / ((norm / k) as f64).powf(16.0)).exp()) as f32
    };
    let s = [
        g[0] * g[0],
        g[1] * g[1],
        g[2] * g[2],
        g[0] * g[1],
        g[0] * g[2],
        g[1] * g[2],
    ];
    let e = eigensystem(s);
    let component = |i: usize, j: usize| {
        lambda * e[0][i] * e[0][j] + lambda * e[1][i] * e[1][j] + e[2][i] * e[2][j]
    };
    [
        component(0, 0),
        component(1, 1),
        component(2, 2),
        component(0, 1),
        component(0, 2),
        component(1, 2),
    ]
}

fn weight(a: Tensor, b: Tensor, direction: [isize; 3]) -> f32 {
    if direction.iter().filter(|&&v| v != 0).count() == 1 {
        let axis = direction.iter().position(|&v| v != 0).unwrap();
        let mut w = (0.05f32 * (a[axis] + b[axis])) as f64;
        for (p, q, off) in PAIRS {
            if p == axis || q == axis {
                w -= 0.025f32 as f64
                    * (((a[off] * a[off]) as f64).sqrt() + ((b[off] * b[off]) as f64).sqrt());
            }
        }
        w as f32
    } else {
        let (p, q, off) = PAIRS
            .into_iter()
            .find(|(p, q, _)| direction[*p] != 0 && direction[*q] != 0)
            .unwrap();
        (0.025f32 as f64
            * (((direction[p] * direction[q]) as f32 * (a[off] + b[off])) as f64
                + ((a[off] * a[off]) as f64).sqrt()
                + ((b[off] * b[off]) as f64).sqrt())) as f32
    }
}

pub(crate) fn diffuse(
    mut image: Vec<f32>,
    dims: [usize; 3],
    k: f32,
    iterations: usize,
    check: &(impl Fn() -> Result<()> + Sync),
) -> Result<Vec<f32>> {
    let [w, h, d] = dims;
    let len = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(d))
        .ok_or_else(|| anyhow::anyhow!("NAD 体尺寸过大"))?;
    if dims.contains(&0)
        || len != image.len()
        || !k.is_finite()
        || k <= 0.0
        || iterations == 0
        || image.iter().any(|v| !v.is_finite())
    {
        bail!("NAD 体尺寸、强度或输入数值无效");
    }
    let mut next = workspace(len, 0f32)?;
    let mut tensor = workspace(len, [0f32; 6])?;
    let at = |x: usize, y: usize, z: usize| (z * h + y) * w + x;
    for _ in 0..iterations {
        check()?;
        tensor
            .par_chunks_mut(w * h)
            .enumerate()
            .try_for_each(|(z, plane)| -> Result<()> {
                for y in 0..h {
                    check()?;
                    for x in 0..w {
                        // Periodic extension for centered derivative estimation;
                        // the evolution stencil separately enforces zero boundary flux.
                        let g = [
                            (image[at((x + 1) % w, y, z)] - image[at((x + w - 1) % w, y, z)]) * 0.5,
                            (image[at(x, (y + 1) % h, z)] - image[at(x, (y + h - 1) % h, z)]) * 0.5,
                            (image[at(x, y, (z + 1) % d)] - image[at(x, y, (z + d - 1) % d)]) * 0.5,
                        ];
                        if !(g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).is_finite() {
                            bail!("NAD 输入动态范围过大，请先缩放数据");
                        }
                        plane[y * w + x] = diffusivity(g, k);
                    }
                }
                Ok(())
            })?;
        next.par_chunks_mut(w * h)
            .enumerate()
            .try_for_each(|(z, plane)| -> Result<()> {
                for y in 0..h {
                    check()?;
                    for x in 0..w {
                        let i = at(x, y, z);
                        let mut value = image[i];
                        for dir in NEIGHBORS {
                            let p = [
                                x as isize + dir[0],
                                y as isize + dir[1],
                                z as isize + dir[2],
                            ];
                            // IMOD5.1's upper-Z face excludes its north/back diagonal
                            // and keeps the north/front periodic ghost. Retain that
                            // boundary convention for reference-result compatibility.
                            let top_diagonal = z + 1 == d && dir == [0, -1, 1];
                            if z + 1 == d && dir == [0, -1, -1] {
                                continue;
                            }
                            if (0..3).any(|a| {
                                (p[a] < 0 || p[a] >= dims[a] as isize) && !(a == 2 && top_diagonal)
                            }) {
                                continue;
                            }
                            let j = at(p[0] as usize, p[1] as usize, p[2] as usize % d);
                            value += weight(tensor[i], tensor[j], dir) * (image[j] - image[i]);
                        }
                        if !value.is_finite() {
                            bail!("NAD 计算超出浮点范围，请先缩放数据");
                        }
                        plane[y * w + x] = value;
                    }
                }
                Ok(())
            })?;
        std::mem::swap(&mut image, &mut next);
    }
    check()?;
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn cancels_during_computation_and_rejects_invalid_inputs() {
        let calls = AtomicUsize::new(0);
        let result = diffuse(vec![1.0; 16 * 16 * 8], [16, 16, 8], 1.0, 100, &|| {
            if calls.fetch_add(1, Ordering::SeqCst) > 25 {
                bail!("cancelled in kernel");
            }
            Ok(())
        });
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cancelled in kernel"));
        assert!(calls.load(Ordering::SeqCst) < 1000);
        for (v, dims, k) in [
            (vec![1.0], [1, 1, 2], 1.0),
            (vec![f32::NAN; 8], [2, 2, 2], 1.0),
            (vec![1.0; 8], [2, 2, 2], 0.0),
            (vec![1.0], [usize::MAX, 3, 3], 1.0),
        ] {
            assert!(diffuse(v, dims, k, 1, &|| Ok(())).is_err());
        }
        let mut extreme = vec![0.0; 27];
        extreme[1] = 1e30;
        assert!(diffuse(extreme, [3, 3, 3], 1.0, 1, &|| Ok(())).is_err());
    }

    #[test]
    fn constant_fields_are_unchanged_and_diffusion_couples_z() {
        let flat = vec![7.0; 4 * 5 * 6];
        assert_eq!(
            diffuse(flat.clone(), [4, 5, 6], 1.0, 20, &|| Ok(())).unwrap(),
            flat
        );
        let mut volume = vec![0.0; 4 * 5 * 6];
        volume[2 * 20..3 * 20].fill(10.0);
        let out = diffuse(volume, [4, 5, 6], 100.0, 1, &|| Ok(())).unwrap();
        assert!(out[20..40].iter().all(|&v| v > 0.0));
        assert!(out[60..80].iter().all(|&v| v > 0.0));
        assert!(out[40..60].iter().all(|&v| v < 10.0));
    }
}
