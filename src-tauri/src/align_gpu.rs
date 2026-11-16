//! Same leave-one-out movie solver as the CPU reference. Spectral sums,
//! cross powers, coarse FFT and small inverse-DFT grids stay on the GPU;
//! only grid maxima and parabolic offsets are evaluated on the CPU.
use super::*;
use burn::tensor::{Device, Int, Tensor, TensorData};
fn upload<const D: usize>(v: Vec<f32>, s: [usize; D], d: &Device) -> Tensor<D> {
    Tensor::from_data(TensorData::new(v, s), d)
}
fn down<const D: usize>(t: Tensor<D>) -> Vec<f32> {
    crate::motion::sum_gpu::download(t)
}
fn refine(
    pr: &Tensor<3>,
    pi: &Tensor<3>,
    g: &Geom,
    start: &[[f64; 2]],
    dev: &Device,
) -> Vec<([f64; 2], f64)> {
    let k = start.len();
    let m = g.m;
    let w = 2. * PI / g.n as f64;
    let mut centres: Vec<_> = start.iter().map(|p| [p[0].round(), p[1].round()]).collect();
    let mut out = vec![None; k];
    for round in 0..5 {
        let phase = |axis: usize, transpose: bool, sine: bool| -> Tensor<3> {
            let mut v = Vec::with_capacity(k * m * 5);
            for c in &centres {
                if transpose {
                    for j in -2..=2 {
                        for f in 0..m {
                            let a = (f as f64 - (m / 2) as f64) * (c[axis] + j as f64) * w;
                            v.push(if sine { a.sin() } else { a.cos() } as f32);
                        }
                    }
                } else {
                    for f in 0..m {
                        for j in -2..=2 {
                            let a = (f as f64 - (m / 2) as f64) * (c[axis] + j as f64) * w;
                            v.push(if sine { a.sin() } else { a.cos() } as f32);
                        }
                    }
                }
            }
            upload(v, if transpose { [k, 5, m] } else { [k, m, 5] }, dev)
        };
        let (cx, sx, cy, sy) = (
            phase(0, false, false),
            phase(0, false, true),
            phase(1, true, false),
            phase(1, true, true),
        );
        let tr = pr.clone().matmul(cx.clone()) - pi.clone().matmul(sx.clone());
        let ti = pr.clone().matmul(sx) + pi.clone().matmul(cx);
        let grid = down(cy.matmul(tr) - sy.matmul(ti));
        for i in 0..k {
            if out[i].is_some() {
                continue;
            }
            let v = &grid[i * 25..(i + 1) * 25];
            if round == 4 {
                out[i] = Some((centres[i], v[12] as f64));
                continue;
            }
            let bi = (0..25).max_by(|&a, &b| v[a].total_cmp(&v[b])).unwrap();
            let (x, y) = (bi % 5, bi / 5);
            if (1..=3).contains(&x) && (1..=3).contains(&y) {
                let at = |x: usize, y: usize| v[y * 5 + x] as f64;
                let para = |a: f64, b: f64, c: f64| {
                    let den = a - 2. * b + c;
                    if den < 0. {
                        (0.5 * (a - c) / den).clamp(-0.5, 0.5)
                    } else {
                        0.
                    }
                };
                out[i] = Some((
                    [
                        centres[i][0] + x as f64 - 2. + para(at(x - 1, y), at(x, y), at(x + 1, y)),
                        centres[i][1] + y as f64 - 2. + para(at(x, y - 1), at(x, y), at(x, y + 1)),
                    ],
                    at(x, y),
                ));
            } else {
                centres[i][0] += x as f64 - 2.;
                centres[i][1] += y as f64 - 2.;
            }
        }
        if out.iter().all(Option::is_some) {
            break;
        }
    }
    out.into_iter().map(Option::unwrap).collect()
}
pub(super) fn movie(
    specs: &[Spec],
    g: &Geom,
    max: f64,
    progress: &(dyn Fn(usize) + Sync),
    dev: &Device,
) -> Result<(Vec<[f64; 2]>, Vec<f64>, usize)> {
    let k = specs.len();
    let m = g.m;
    anyhow::ensure!(k * m * m <= 1 << 25, "电影频谱超过 GPU 安全批次预算");
    let fr = upload(
        specs.iter().flat_map(|s| s.re.iter().copied()).collect(),
        [k, m, m],
        dev,
    );
    let fi = upload(
        specs.iter().flat_map(|s| s.im.iter().copied()).collect(),
        [k, m, m],
        dev,
    );
    let mut d = vec![[0.; 2]; k];
    let mut cc = vec![0.; k];
    let mut iterations = 0;
    for it in 0..MAX_ITER {
        crate::motion::check_cancel()?;
        iterations = it + 1;
        let mut cs = Vec::with_capacity(k * m * m);
        let mut sn = Vec::with_capacity(k * m * m);
        for pos in &d {
            for y in 0..m {
                for x in 0..m {
                    let phase = 2. * PI / g.n as f64
                        * ((x as f64 - (m / 2) as f64) * pos[0]
                            + (y as f64 - (m / 2) as f64) * pos[1]);
                    cs.push(phase.cos() as f32);
                    sn.push(phase.sin() as f32);
                }
            }
        }
        let cs = upload(cs, [k, m, m], dev);
        let sn = upload(sn, [k, m, m], dev);
        let shifted_r = fr.clone() * cs.clone() - fi.clone() * sn.clone();
        let shifted_i = fr.clone() * sn + fi.clone() * cs;
        let rr = shifted_r.clone().sum_dim(0) - shifted_r;
        let ri = shifted_i.clone().sum_dim(0) - shifted_i;
        let energies = down(
            (rr.clone() * rr.clone() + ri.clone() * ri.clone())
                .sum_dim(1)
                .sum_dim(2),
        );
        let pr = fr.clone() * rr.clone() + fi.clone() * ri.clone();
        let pi = fi.clone() * rr - fr.clone() * ri;
        let starts = if it == 0 {
            let keep = Tensor::<1, Int>::from_data(
                TensorData::new((0..m).map(|i| ((i + m / 2) % m) as i32).collect(), [m]),
                dev,
            );
            let r = pr.clone().select(1, keep.clone()).select(2, keep.clone());
            let im = pi.clone().neg().select(1, keep.clone()).select(2, keep);
            let (r, im) = burn::signal::cfft(r, im, 2, None);
            let (r, _) = burn::signal::cfft(r, im, 1, None);
            let grid = down(r);
            let step = (g.n / m) as f64;
            (0..k)
                .map(|i| {
                    let mut best = (f32::MIN, [0.; 2]);
                    for y in 0..m {
                        let dy = if y >= m / 2 {
                            y as f64 - m as f64
                        } else {
                            y as f64
                        } * step;
                        for x in 0..m {
                            let dx = if x >= m / 2 {
                                x as f64 - m as f64
                            } else {
                                x as f64
                            } * step;
                            let v = grid[(i * m + y) * m + x];
                            if dx.hypot(dy) <= max && v > best.0 {
                                best = (v, [dx, dy]);
                            }
                        }
                    }
                    best.1
                })
                .collect()
        } else {
            d.clone()
        };
        let next = refine(&pr, &pi, g, &starts, dev);
        if it == 0 {
            // Validate two subpixel searches against the f64 CPU reference before
            // trusting the GPU solver for this movie, not merely its availability.
            let take = k.min(2);
            let rr = down(pr.clone().narrow(0, 0, take));
            let ii = down(pi.clone().narrow(0, 0, take));
            for j in 0..take {
                let cpu = refine_peak(
                    &rr[j * m * m..(j + 1) * m * m],
                    &ii[j * m * m..(j + 1) * m * m],
                    g,
                    starts[j],
                );
                let error = (cpu.0[0] - next[j].0[0]).hypot(cpu.0[1] - next[j].0[1]);
                anyhow::ensure!(
                    error < 0.02,
                    "GPU 亚像素搜索校准误差 {error:.4} px 超过 0.02 px"
                );
            }
        }
        anyhow::ensure!(
            next.iter()
                .all(|(p, v)| p.iter().all(|v| v.is_finite()) && v.is_finite()),
            "GPU 全局运动返回非有限值"
        );
        let change = next
            .iter()
            .zip(&d)
            .map(|(a, b)| (a.0[0] - b[0]).hypot(a.0[1] - b[1]))
            .fold(0., f64::max);
        for (i, (p, v)) in next.into_iter().enumerate() {
            d[i] = p;
            cc[i] = norm_cc(v, specs[i].energy, energies[i] as f64);
        }
        crate::compute::report(
            "电影迭代运动求解",
            "GPU + CPU",
            Some("GPU 参考叠加、相关与亚像素搜索；CPU 峰值和收敛判断".into()),
        );
        progress(it + 1);
        if change < SETTLED {
            break;
        }
    }
    Ok((d, cc, iterations))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn movie_gpu_solver_matches_cpu() {
        let Some((dev, _)) = crate::ctf_gpu::device() else {
            assert!(
                std::env::var_os("TMV_REQUIRE_GPU").is_none(),
                "GPU required"
            );
            return;
        };
        let n = 128;
        let g = Geom {
            x0: 0,
            y0: 0,
            bin: 1,
            n,
            m: n / 2,
            w: n,
            h: n,
        };
        let f = Fft::new(n);
        let filter = make_filter(n / 2);
        let base: Vec<f32> = (0..n * n)
            .map(|i| ((i * 37 % 197) as f32 / 100. - 1.) + ((i % n) as f32 * 0.07).sin())
            .collect();
        let offsets = [[0., 0.], [1.3, -0.4], [-0.7, 1.8], [2.1, 0.6]];
        let specs: Vec<_> = offsets
            .iter()
            .map(|&d| {
                let v = movie_shift(&base, n, n, d);
                spectrum(&g, &f, &filter, [0, 0], |x, y| v[y * n + x])
            })
            .collect();
        let cpu = align_movie_cpu(&specs, &g, &Fft::new(n / 2), 16., &|_| {}).unwrap();
        let gpu = movie(&specs, &g, 16., &|_| {}, dev).unwrap(); // Absolute movie displacement has an arbitrary common translation;
                                                                 // the production solver removes it before applying any correction.
        let centre = |p: &Vec<[f64; 2]>| {
            [
                p.iter().map(|v| v[0]).sum::<f64>() / p.len() as f64,
                p.iter().map(|v| v[1]).sum::<f64>() / p.len() as f64,
            ]
        };
        let a = centre(&cpu.0);
        let b = centre(&gpu.0);
        let max = cpu
            .0
            .iter()
            .zip(&gpu.0)
            .map(|(x, y)| ((x[0] - a[0]) - (y[0] - b[0])).hypot((x[1] - a[1]) - (y[1] - b[1])))
            .fold(0., f64::max);
        eprintln!("GPU movie solver max shift discrepancy {max}");
        assert!(max < 0.05, "{max}: CPU {:?} GPU {:?}", cpu.0, gpu.0);
    }
}
