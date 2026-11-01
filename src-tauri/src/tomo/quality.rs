//! An objective measure of a tomogram's quality for choosing how the
//! pipeline reconstructs: the Fourier shell correlation (FSC) of two
//! independent halves — tomograms from the even and the odd frames of each
//! movie, or, without them, from alternate tilts. Where both halves agree,
//! there is signal; the frequency where the FSC falls below 0.143 is the
//! usual resolution estimate. Linear filters change both halves alike, so
//! the FSC judges the reconstruction and CTF correction, not display filters.

/// FSC of two volumes [z][y][x] of the same shape, on the central box with
/// power-of-two sides. Returns (spatial frequency in 1/Å, FSC) per shell;
/// shells are one step of the shortest side wide.
pub fn fsc(a: &[f32], b: &[f32], shape: [usize; 3], pixel_a: f64) -> Vec<(f64, f64)> {
    let [w, h, z] = shape;
    let pow = |n: usize| 1usize << n.ilog2();
    let (nx, ny, nz) = (pow(w), pow(h), pow(z));
    let crop = |v: &[f32]| -> Vec<f32> {
        let (x0, y0, z0) = ((w - nx) / 2, (h - ny) / 2, (z - nz) / 2);
        let mut out = Vec::with_capacity(nx * ny * nz);
        for k in 0..nz {
            for y in 0..ny {
                let at = ((z0 + k) * h + y0 + y) * w + x0;
                out.extend_from_slice(&v[at..at + nx]);
            }
        }
        let mean = out.iter().map(|&x| x as f64).sum::<f64>() / out.len() as f64;
        out.iter().map(|&x| x - mean as f32).collect()
    };
    let (fa, fb) = (fft3(&crop(a), [nx, ny, nz]), fft3(&crop(b), [nx, ny, nz]));
    let shells = nx.min(ny).min(nz) / 2;
    let mut num = vec![0f64; shells + 1];
    let mut da = vec![0f64; shells + 1];
    let mut db = vec![0f64; shells + 1];
    let f = |i: usize, n: usize| crate::motion::sum::freq(i, n) / n as f64;
    for k in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let r = (f(x, nx).powi(2) + f(y, ny).powi(2) + f(k, nz).powi(2)).sqrt();
                let s = (r * 2.0 * shells as f64).round() as usize;
                if s > shells {
                    continue;
                }
                let i = (k * ny + y) * nx + x;
                let (p, q) = (fa[i], fb[i]);
                num[s] += p.0 * q.0 + p.1 * q.1;
                da[s] += p.0 * p.0 + p.1 * p.1;
                db[s] += q.0 * q.0 + q.1 * q.1;
            }
        }
    }
    (1..=shells).map(|s| (s as f64 / (2.0 * shells as f64) / pixel_a, num[s] / (da[s] * db[s]).sqrt().max(1e-30))).collect()
}

/// The frequency (1/Å) where the curve first drops below `threshold`.
pub fn crossing(curve: &[(f64, f64)], threshold: f64) -> f64 {
    for w in curve.windows(2) {
        if w[1].1 < threshold {
            let t = (w[0].1 - threshold) / (w[0].1 - w[1].1).max(1e-12);
            return w[0].0 + t * (w[1].0 - w[0].0);
        }
    }
    curve.last().map_or(0.0, |c| c.0)
}

/// Mean FSC between two frequencies (1/Å): a steadier single number than a crossing.
pub fn mean_between(curve: &[(f64, f64)], lo: f64, hi: f64) -> f64 {
    let v: Vec<f64> = curve.iter().filter(|c| c.0 >= lo && c.0 <= hi).map(|c| c.1).collect();
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

/// Complex 3D FFT of a real volume [z][y][x] (power-of-two sides).
fn fft3(v: &[f32], [nx, ny, nz]: [usize; 3]) -> Vec<(f64, f64)> {
    use rayon::prelude::*;
    let mut re = v.to_vec();
    let mut im = vec![0f32; v.len()];
    let fx = crate::fft::Fft::new(nx);
    re.par_chunks_mut(nx).zip(im.par_chunks_mut(nx)).for_each(|(r, i)| fx.run(r, i));
    let along = |re: &mut Vec<f32>, im: &mut Vec<f32>, n: usize, stride: usize, count: usize, base: &(dyn Fn(usize) -> usize + Sync)| {
        let f = crate::fft::Fft::new(n);
        let lines: Vec<(Vec<f32>, Vec<f32>)> = (0..count)
            .into_par_iter()
            .map(|l| {
                let b = base(l);
                let mut r: Vec<f32> = (0..n).map(|k| re[b + k * stride]).collect();
                let mut i: Vec<f32> = (0..n).map(|k| im[b + k * stride]).collect();
                f.run(&mut r, &mut i);
                (r, i)
            })
            .collect();
        for (l, (r, i)) in lines.into_iter().enumerate() {
            let b = base(l);
            for k in 0..n {
                re[b + k * stride] = r[k];
                im[b + k * stride] = i[k];
            }
        }
    };
    along(&mut re, &mut im, ny, nx, nx * nz, &|l| (l / nx) * nx * ny + l % nx);
    if nz > 1 {
        along(&mut re, &mut im, nz, nx * ny, nx * ny, &|l| l);
    }
    re.into_iter().zip(im).map(|(r, i)| (r as f64, i as f64)).collect()
}

/// FRC of two images [y][x] of the same shape, central power-of-two box.
pub fn frc(a: &[f32], b: &[f32], w: usize, h: usize, pixel_a: f64) -> Vec<(f64, f64)> {
    // A one-section volume: the 3D transform with z = 1 is the 2D one.
    fsc_shells(a, b, [w, h, 1], pixel_a)
}

/// `fsc` without the cubic shell limit of the shortest side (for z = 1).
fn fsc_shells(a: &[f32], b: &[f32], shape: [usize; 3], pixel_a: f64) -> Vec<(f64, f64)> {
    let [w, h, z] = shape;
    let pow = |n: usize| 1usize << n.ilog2();
    let (nx, ny, nz) = (pow(w), pow(h), pow(z));
    let crop = |v: &[f32]| -> Vec<f32> {
        let (x0, y0, z0) = ((w - nx) / 2, (h - ny) / 2, (z - nz) / 2);
        let mut out = Vec::with_capacity(nx * ny * nz);
        for k in 0..nz {
            for y in 0..ny {
                let at = ((z0 + k) * h + y0 + y) * w + x0;
                out.extend_from_slice(&v[at..at + nx]);
            }
        }
        let mean = out.iter().map(|&x| x as f64).sum::<f64>() / out.len() as f64;
        out.iter().map(|&x| x - mean as f32).collect()
    };
    let (fa, fb) = (fft3(&crop(a), [nx, ny, nz]), fft3(&crop(b), [nx, ny, nz]));
    let shells = nx.min(ny) / 2;
    let (mut num, mut da, mut db) = (vec![0f64; shells + 1], vec![0f64; shells + 1], vec![0f64; shells + 1]);
    let f = |i: usize, n: usize| crate::motion::sum::freq(i, n) / n as f64;
    for k in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let r = (f(x, nx).powi(2) + f(y, ny).powi(2) + if nz > 1 { f(k, nz).powi(2) } else { 0.0 }).sqrt();
                let s = (r * 2.0 * shells as f64).round() as usize;
                if s > shells {
                    continue;
                }
                let i = (k * ny + y) * nx + x;
                let (p, q) = (fa[i], fb[i]);
                num[s] += p.0 * q.0 + p.1 * q.1;
                da[s] += p.0 * p.0 + p.1 * p.1;
                db[s] += q.0 * q.0 + q.1 * q.1;
            }
        }
    }
    (1..=shells).map(|s| (s as f64 / (2.0 * shells as f64) / pixel_a, num[s] / (da[s] * db[s]).sqrt().max(1e-30))).collect()
}

/// Projects a volume [z][y][x] (thickness z, width w, height h) along a
/// view tilted by θ: one image [y][x], with `recon`'s forward projector.
pub fn project(vol: &[f32], w: usize, h: usize, z: usize, theta: f64, z_offset: f64) -> anyhow::Result<Vec<f32>> {
    use rayon::prelude::*;
    let g = super::recon::Geometry::new(w, z, &[theta, theta + 1e-3, theta + 2e-3], z_offset)?;
    let mut one = g.clone();
    one.trig.truncate(1);
    one.weight.truncate(1);
    let rows: Vec<Vec<f32>> = (0..h)
        .into_par_iter()
        .map(|y| {
            let slice: Vec<f32> = (0..z).flat_map(|k| vol[(k * h + y) * w..(k * h + y + 1) * w].iter().copied()).collect();
            super::recon::forward(&one, &slice)
        })
        .collect();
    Ok(rows.concat())
}

/// Held-out projection score of an alignment (and CTF): views spread over
/// the series are each left out, the rest reconstructed (phase-flipped WBP,
/// quick) and projected along it, and the projection compared with the
/// measured view by FRC. Comparable between alignments of the same series.
#[derive(Clone, Copy, Debug)]
pub struct HeldOut {
    /// Mean resolution where the FRC falls below 0.143, Å.
    pub resolution_a: f64,
    /// Mean FRC between 100 and 20 Å: the steadier number to rank by.
    pub frc: f64,
}

pub fn held_out(
    img: &std::sync::Arc<crate::image::OpenImage>,
    a: &super::Alignment,
    ctf: Option<super::output::FlipWith>,
    bin: usize,
    thickness: usize,
    z_offset: f64,
) -> anyhow::Result<HeldOut> {
    use super::{output, pipeline, recon};
    let pixel = ctf.map(|c| c.2).unwrap_or_else(|| crate::align::pixel_a(img.info()));
    let mut order: Vec<usize> = (0..a.views.len()).filter(|&i| !a.views[i].excluded).collect();
    order.sort_by(|&i, &j| a.views[i].refined().total_cmp(&a.views[j].refined()));
    let mut picks: Vec<usize> = [0.12, 0.3, 0.45, 0.55, 0.7, 0.88].iter().map(|f| order[((order.len() - 1) as f64 * f).round() as usize]).collect();
    picks.dedup();
    let (mut all, w, h, _) = output::aligned(img, a, bin, 0, 0, &|_, _| {})?;
    if let Some(c) = ctf {
        output::flip_views(&mut all, w, h, a, bin, c, 0.0);
    }
    let kept: Vec<usize> = (0..a.views.len()).filter(|&i| !a.views[i].excluded).collect();
    let params = recon::Params { thickness, z_offset, ..Default::default() };
    let (mut res, mut band) = (0.0, 0.0);
    for &k in &picks {
        super::check_cancel()?;
        let j = kept.iter().position(|&i| i == k).unwrap();
        let mut rest = a.clone();
        rest.views[k].excluded = true;
        let others: Vec<f32> = (0..kept.len()).filter(|&i| i != j).flat_map(|i| all[i * w * h..(i + 1) * w * h].iter().copied()).collect();
        let angles: Vec<f64> = rest.views.iter().filter(|v| !v.excluded).map(|v| v.refined()).collect();
        // Already flipped: the build itself does not flip again.
        let how = pipeline::Build { alignment: &rest, bin, params: &params, ctf: None, ctf_3d: false };
        let vol = pipeline::volume(&how, others, w, h, &angles)?;
        let proj = project(&vol, w, h, thickness, a.views[k].refined(), z_offset)?;
        let curve = frc(&proj, &all[j * w * h..(j + 1) * w * h], w, h, pixel * bin as f64);
        res += 1.0 / crossing(&curve, 0.143).max(curve[0].0);
        band += mean_between(&curve, 1.0 / 100.0, 1.0 / 20.0);
    }
    let n = picks.len() as f64;
    Ok(HeldOut { resolution_a: n / res, frc: band / n })
}
