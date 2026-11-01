//! Wiener-like deconvolution of a phase-flipped tomogram (the filter
//! Tegunov's tom_deconv introduced and Warp and IsoNet use): each spatial
//! frequency is scaled by |CTF| / (CTF² + 1/SNR), with an SNR that falls
//! exponentially with frequency and a gentle high-pass at the lowest
//! frequencies. It restores the low-resolution contrast the CTF takes away
//! and damps the frequencies near its zeros, where there is mostly noise,
//! so particles and membranes stand out. The CTF is isotropic at the
//! series' median defocus.

use crate::ctf::Optics;
use rayon::prelude::*;

/// Strength of the filter (log₁₀ SNR scale / 3) and how fast the SNR falls.
pub const STRENGTH: f64 = 1.0;
pub const FALLOFF: f64 = 1.0;
/// Below this fraction of Nyquist the high-pass ramps up.
const HIGHPASS: f64 = 0.02;

/// Deconvolves a volume [z][y][x] in place.
pub(crate) fn deconvolve(
    vol: &mut [f32],
    [w, h, z]: [usize; 3],
    pixel_a: f64,
    defocus_a: f64,
    optics: &Optics,
) {
    let (nx, ny, nz) = (
        w.next_power_of_two(),
        h.next_power_of_two(),
        z.next_power_of_two().max(2),
    );
    let mean = (vol.par_iter().map(|&v| v as f64).sum::<f64>() / vol.len() as f64) as f32;
    let mut re = vec![0f32; nx * ny * nz];
    let mut im = vec![0f32; nx * ny * nz];
    for k in 0..z {
        for y in 0..h {
            let (s, d) = ((k * h + y) * w, (k * ny + y) * nx);
            for x in 0..w {
                re[d + x] = vol[s + x] - mean;
            }
        }
    }
    fft3(&mut re, &mut im, [nx, ny, nz], false);
    let f = |i: usize, n: usize| crate::motion::sum::freq(i, n) / n as f64;
    re.par_chunks_mut(nx)
        .zip(im.par_chunks_mut(nx))
        .enumerate()
        .for_each(|(line, (r, m))| {
            let (y, k) = (line % ny, line / ny);
            for x in 0..nx {
                // Spatial frequency in cycles per pixel, then per Å.
                let kp = (f(x, nx).powi(2) + f(y, ny).powi(2) + f(k, nz).powi(2)).sqrt();
                let ka = kp / pixel_a;
                let ctf = optics.gamma(defocus_a, ka * ka, 0.0).sin().abs();
                let ramp = (kp / (HIGHPASS * 0.5)).min(1.0) * std::f64::consts::PI;
                let snr = (-kp * 100.0 * FALLOFF / pixel_a).exp()
                    * 10f64.powf(3.0 * STRENGTH)
                    * (1.0 - ramp.cos());
                let g = if snr > 0.0 {
                    (ctf / (ctf * ctf + 1.0 / snr)) as f32
                } else {
                    0.0
                };
                r[x] *= g;
                m[x] *= g;
            }
        });
    fft3(&mut re, &mut im, [nx, ny, nz], true);
    for k in 0..z {
        for y in 0..h {
            let (s, d) = ((k * ny + y) * nx, (k * h + y) * w);
            vol[d..d + w].copy_from_slice(&re[s..s + w]);
        }
    }
}

/// In-place complex 3D FFT (power-of-two sides); the inverse is scaled.
fn fft3(re: &mut [f32], im: &mut [f32], [nx, ny, nz]: [usize; 3], inverse: bool) {
    if inverse {
        im.par_iter_mut().for_each(|v| *v = -*v);
    }
    let run = |re: &mut [f32], im: &mut [f32], n: usize, stride: usize, lines: Vec<usize>| {
        let fft = crate::fft::Fft::new(n);
        let out: Vec<(Vec<f32>, Vec<f32>)> = lines
            .par_iter()
            .map(|&b| {
                let mut r: Vec<f32> = (0..n).map(|k| re[b + k * stride]).collect();
                let mut i: Vec<f32> = (0..n).map(|k| im[b + k * stride]).collect();
                fft.run(&mut r, &mut i);
                (r, i)
            })
            .collect();
        for (&b, (r, i)) in lines.iter().zip(out) {
            for k in 0..n {
                re[b + k * stride] = r[k];
                im[b + k * stride] = i[k];
            }
        }
    };
    run(re, im, nx, 1, (0..ny * nz).map(|l| l * nx).collect());
    run(
        re,
        im,
        ny,
        nx,
        (0..nx * nz).map(|l| (l / nx) * nx * ny + l % nx).collect(),
    );
    run(re, im, nz, nx * ny, (0..nx * ny).collect());
    if inverse {
        let n = (nx * ny * nz) as f32;
        re.par_iter_mut().for_each(|v| *v /= n);
        im.par_iter_mut().for_each(|v| *v = -*v / n);
    }
}
