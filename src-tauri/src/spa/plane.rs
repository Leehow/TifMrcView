//! The 2D counterpart of `volume`: rotated copies of class averages as
//! interpolated slices of their padded transforms, and class averages
//! accumulated in Fourier space (Σ C·w·X and Σ C²·w at the rotated
//! coefficient positions), so rotating never resamples a noisy image.

use super::data::soft;
use super::fourier::{self, Half};
use crate::fft::Fft;

fn sinc(u: f64) -> f64 {
    if u.abs() < 1e-9 {
        1.0
    } else {
        (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u)
    }
}

/// 2D rotation of a frequency by `deg`.
#[inline]
pub fn rotate(k: [f32; 2], c: f32, s: f32) -> [f32; 2] {
    [c * k[0] - s * k[1], s * k[0] + c * k[1]]
}

pub struct Projector2 {
    n: usize,
    pad: usize,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Projector2 {
    pub fn new(img: &[f32], b: usize, pad: usize) -> Projector2 {
        let n = b * pad;
        let o = (n - b) / 2;
        let corr: Vec<f64> = (0..b)
            .map(|i| sinc((i as f64 - b as f64 / 2.0) / n as f64).powi(2))
            .collect();
        let mut big = vec![0f32; n * n];
        for y in 0..b {
            for x in 0..b {
                big[(y + o) * n + x + o] = (img[y * b + x] as f64 / (corr[x] * corr[y])) as f32;
            }
        }
        let (re, im) = fourier::ft2(&Fft::new(n), n, &big);
        Projector2 { n, pad, re, im }
    }

    #[inline]
    fn at(&self, q: [f32; 2]) -> (f32, f32) {
        let n = self.n;
        let h = (n / 2) as f32 - 1.0;
        if q[0].abs() > h || q[1].abs() > h {
            return (0.0, 0.0);
        }
        let (fx, fy) = (q[0].floor(), q[1].floor());
        let (tx, ty) = (q[0] - fx, q[1] - fy);
        let x0 = fourier::index(fx as i32, n);
        let y0 = fourier::index(fy as i32, n);
        let (x1, y1) = ((x0 + 1) % n, (y0 + 1) % n);
        let mut r = 0f32;
        let mut m = 0f32;
        for (iy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
            for (ix, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                let j = iy * n + ix;
                r += wx * wy * self.re[j];
                m += wx * wy * self.im[j];
            }
        }
        (r, m)
    }

    /// The class rotated by `deg` on `half`: P(k) = A(Rot(−deg)·k).
    pub fn rotated(&self, deg: f64, half: &Half, out: &mut [f32]) {
        let (s, c) = (-deg).to_radians().sin_cos();
        let p = self.pad as f32;
        for (j, &k) in half.k.iter().enumerate() {
            let q = rotate([k[0] * p, k[1] * p], c as f32, s as f32);
            let (a, b) = self.at(q);
            out[2 * j] = a;
            out[2 * j + 1] = b;
        }
    }
}

/// Σ C·w·X and Σ C²·w for one class, on its b × b grid.
#[derive(Clone)]
pub struct Accum2 {
    pub b: usize,
    pub re: Vec<f32>,
    pub im: Vec<f32>,
    pub den: Vec<f32>,
    pub count: usize,
}

impl Accum2 {
    pub fn new(b: usize) -> Accum2 {
        Accum2 {
            b,
            re: vec![0.0; b * b],
            im: vec![0.0; b * b],
            den: vec![0.0; b * b],
            count: 0,
        }
    }

    fn add(&mut self, q: [f32; 2], re: f32, im: f32, w: f32) {
        let n = self.b;
        let h = (n / 2) as f32 - 1.0;
        if q[0].abs() > h || q[1].abs() > h {
            return;
        }
        let (fx, fy) = (q[0].floor(), q[1].floor());
        let (tx, ty) = (q[0] - fx, q[1] - fy);
        let x0 = fourier::index(fx as i32, n);
        let y0 = fourier::index(fy as i32, n);
        let (x1, y1) = ((x0 + 1) % n, (y0 + 1) % n);
        for (iy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
            for (ix, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                let j = iy * n + ix;
                let f = wx * wy;
                self.re[j] += f * re;
                self.im[j] += f * im;
                self.den[j] += f * w;
            }
        }
    }

    /// Inserts a particle matched to the class rotated by `deg`; `x` and
    /// `c2` are its coefficients on `half` (a b-box half plane).
    pub fn insert(&mut self, deg: f64, half: &Half, x: &[f32], c2: &[f32]) {
        let (s, c) = (-deg).to_radians().sin_cos();
        for (j, &k) in half.k.iter().enumerate() {
            let q = rotate(k, c as f32, s as f32);
            self.add(q, x[2 * j], x[2 * j + 1], c2[j]);
            self.add([-q[0], -q[1]], x[2 * j], -x[2 * j + 1], c2[j]);
        }
        self.count += 1;
    }

    pub fn merge(&mut self, o: &Accum2) {
        for i in 0..self.re.len() {
            self.re[i] += o.re[i];
            self.im[i] += o.im[i];
            self.den[i] += o.den[i];
        }
        self.count += o.count;
    }

    /// The class average: Wiener division with `lambda` × the mean
    /// denominator of each shell, cut beyond `r_max` with a cosine edge,
    /// and a soft circular mask of `radius` pixels.
    pub fn average(&self, lambda: f64, r_max: f64, radius: f64) -> Vec<f32> {
        let b = self.b;
        let shells = b / 2 + 2;
        let (mut sum, mut cnt) = (vec![0f64; shells], vec![0f64; shells]);
        for y in 0..b {
            for x in 0..b {
                let (fx, fy) = (fourier::freq(x, b), fourier::freq(y, b));
                let r = ((fx * fx + fy * fy) as f64).sqrt().round() as usize;
                if r < shells {
                    sum[r] += self.den[y * b + x] as f64;
                    cnt[r] += 1.0;
                }
            }
        }
        let (mut re, mut im) = (vec![0f32; b * b], vec![0f32; b * b]);
        for y in 0..b {
            for x in 0..b {
                let (fx, fy) = (fourier::freq(x, b), fourier::freq(y, b));
                let rr = ((fx * fx + fy * fy) as f64).sqrt();
                let r = rr.round() as usize;
                if r >= shells {
                    continue;
                }
                let f = soft(rr, r_max, 2.0) as f64;
                if f == 0.0 {
                    continue;
                }
                let d = self.den[y * b + x] as f64 + lambda * sum[r] / cnt[r].max(1.0) + 1e-20;
                re[y * b + x] = (self.re[y * b + x] as f64 / d * f) as f32;
                im[y * b + x] = (self.im[y * b + x] as f64 / d * f) as f32;
            }
        }
        let mut img = fourier::ift2(&Fft::new(b), b, &re, &im);
        let c = b as f64 / 2.0;
        for y in 0..b {
            for x in 0..b {
                let d = ((x as f64 - c).powi(2) + (y as f64 - c).powi(2)).sqrt();
                img[y * b + x] *= soft(d, radius, 4.0);
            }
        }
        img
    }
}

/// Shift (pixels) that moves the centre of the positive mass of `img` to the box centre.
pub fn centre_of_mass(img: &[f32], b: usize) -> [f32; 2] {
    let (mut sx, mut sy, mut s) = (0f64, 0f64, 0f64);
    let c = b as f64 / 2.0;
    for y in 0..b {
        for x in 0..b {
            let v = img[y * b + x].max(0.0) as f64;
            sx += v * (x as f64 - c);
            sy += v * (y as f64 - c);
            s += v;
        }
    }
    if s <= 0.0 {
        [0.0, 0.0]
    } else {
        [(-sx / s) as f32, (-sy / s) as f32]
    }
}

/// Shifts an image by `d` pixels (Fourier phase ramp).
pub fn shift(img: &[f32], b: usize, d: [f32; 2]) -> Vec<f32> {
    let fft = Fft::new(b);
    let (mut re, mut im) = fourier::ft2(&fft, b, img);
    for y in 0..b {
        for x in 0..b {
            let (fx, fy) = (fourier::freq(x, b) as f32, fourier::freq(y, b) as f32);
            let ph = -2.0 * std::f32::consts::PI * (fx * d[0] + fy * d[1]) / b as f32;
            let (s, c) = ph.sin_cos();
            let j = y * b + x;
            let (a, bb) = (re[j], im[j]);
            re[j] = a * c - bb * s;
            im[j] = a * s + bb * c;
        }
    }
    fourier::ift2(&fft, b, &re, &im)
}
