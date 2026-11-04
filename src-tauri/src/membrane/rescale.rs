//! Patch Fourier resizing, matching the published inference convention.
//! The spectrum is centred, cropped/zero-padded, then inverse transformed.
//! No amplitude correction: the inverse is normalized by the output size.

use crate::fft::Fft;
use rayon::prelude::*;

/// Bluestein transform using the existing radix-2 FFT, for non-power-of-two
/// patch sides. Plans are shared between all lines and inference windows.
struct Transform {
    n: usize,
    fft: Fft,
    chirp: Vec<(f32, f32)>,
    kernel: Vec<(f32, f32)>,
}
impl Transform {
    fn new(n: usize) -> Self {
        let size = (2 * n - 1).next_power_of_two().max(2);
        let fft = Fft::new(size);
        let chirp: Vec<_> = (0..n).map(|k| {
            let a = std::f64::consts::PI * (k * k) as f64 / n as f64;
            (a.cos() as f32, -a.sin() as f32)
        }).collect();
        let (mut re, mut im) = (vec![0.0; size], vec![0.0; size]);
        for k in 0..n {
            let (r, i) = chirp[k];
            re[k] = r;
            im[k] = -i;
            if k != 0 { re[size-k] = r; im[size-k] = -i; }
        }
        fft.run(&mut re, &mut im);
        Self { n, fft, chirp, kernel: re.into_iter().zip(im).collect() }
    }
    fn run(&self, input: &[(f32, f32)], inverse: bool) -> Vec<(f32, f32)> {
        let size = self.kernel.len();
        let (mut re, mut im) = (vec![0.0; size], vec![0.0; size]);
        for (k, &(r, i)) in input.iter().enumerate() {
            let i = if inverse { -i } else { i };
            let (c, s) = self.chirp[k];
            re[k] = r*c-i*s;
            im[k] = r*s+i*c;
        }
        self.fft.run(&mut re, &mut im);
        for k in 0..size {
            let (c, s) = self.kernel[k];
            let (r, i) = (re[k], im[k]);
            re[k] = r*c-i*s;
            im[k] = -(r*s+i*c);
        }
        self.fft.run(&mut re, &mut im);
        (0..self.n).map(|k| {
            let (c, s) = self.chirp[k];
            let (r, i) = (re[k]/size as f32, -im[k]/size as f32);
            let (r, i) = (r*c-i*s, r*s+i*c);
            if inverse { (r/self.n as f32, -i/self.n as f32) } else { (r,i) }
        }).collect()
    }
}

pub(super) struct FourierResize {
    source: Transform,
    target: Transform,
}
impl FourierResize {
    pub(super) fn new(source: usize, target: usize) -> Self {
        assert!(source.is_multiple_of(2) && target.is_multiple_of(2));
        Self { source: Transform::new(source), target: Transform::new(target) }
    }
    pub(super) fn run(&self, values: &[f32]) -> Vec<f32> {
        let (n, m) = (self.source.n, self.target.n);
        assert_eq!(values.len(), n*n*n);
        let mut data: Vec<_> = values.iter().map(|&r| (r,0.0)).collect();
        for _ in 0..3 { data = transform_rotate(&data, n, &self.source, false); }
        let indices: Vec<_> = (0..m).map(|k| {
            let shifted = (k+m/2)%m;
            let old = shifted as isize + (n as isize-m as isize)/2;
            (old >= 0 && old < n as isize).then(|| (old as usize+n/2)%n)
        }).collect();
        let mut resized = vec![(0.0,0.0); m*m*m];
        resized.par_chunks_mut(m*m).enumerate().for_each(|(z,plane)| {
            let Some(iz) = indices[z] else { return };
            for y in 0..m {
                let Some(iy) = indices[y] else { continue };
                for x in 0..m {
                    if let Some(ix) = indices[x] { plane[y*m+x] = data[(iz*n+iy)*n+ix]; }
                }
            }
        });
        drop(data);
        for _ in 0..3 { resized = transform_rotate(&resized, m, &self.target, true); }
        resized.into_iter().map(|(r,_)| r).collect()
    }
}

// Transform contiguous rows then rotate the axes. Three iterations transform
// every axis and restore the original axis order.
fn transform_rotate(data: &[(f32,f32)], n: usize, fft: &Transform, inverse: bool) -> Vec<(f32,f32)> {
    let rows: Vec<_> = data.par_chunks(n).map(|r| fft.run(r,inverse)).collect();
    let mut out = vec![(0.0,0.0); data.len()];
    out.par_chunks_mut(n*n).enumerate().for_each(|(x,plane)| {
        for z in 0..n { for y in 0..n { plane[z*n+y] = rows[z*n+y][x]; } }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_fft_matches_direct_dft() {
        for n in [2,6,10,53,106,120,160] {
            let values: Vec<_> = (0..n).map(|k| ((k as f32*0.37).sin(),(k as f32*0.23).cos())).collect();
            let fft = Transform::new(n);
            let actual = fft.run(&values,false);
            for (k,&(r,i)) in actual.iter().enumerate() {
                let (mut er,mut ei) = (0.0,0.0);
                for (j,&(a,b)) in values.iter().enumerate() {
                    let t = -2.0*std::f64::consts::PI*(j*k) as f64/n as f64;
                    er += a as f64*t.cos()-b as f64*t.sin();
                    ei += a as f64*t.sin()+b as f64*t.cos();
                }
                assert!((r as f64-er).abs()<8e-5 && (i as f64-ei).abs()<8e-5, "n={n} k={k}");
            }
            let back=fft.run(&actual,true);
            assert!(back.iter().zip(values).all(|(&(a,b),(c,d))| (a-c).abs()<2e-6 && (b-d).abs()<2e-6));
        }
    }
    #[test]
    fn fourier_resize_dc_uses_output_normalization() {
        for (n,m) in [(6,10),(10,6)] {
            let out=FourierResize::new(n,m).run(&vec![1.0;n*n*n]);
            let expected=(n as f32/m as f32).powi(3);
            assert!(out.iter().all(|v| (v-expected).abs()<3e-6));
        }
    }
    #[test]
    fn fourier_resize_preserves_axis_order_and_nyquist_convention() {
        let (n,m) = (6usize,10usize);
        for axis in 0..3 {
            for frequency in [1,3] {
                let values: Vec<_> = (0..n*n*n).map(|i| {
                    let xyz=[i%n,(i/n)%n,i/(n*n)];
                    (2.0*std::f64::consts::PI*frequency as f64*xyz[axis] as f64/n as f64).cos() as f32
                }).collect();
                let actual=FourierResize::new(n,m).run(&values);
                for (i,&value) in actual.iter().enumerate() {
                    let xyz=[i%m,(i/m)%m,i/(m*m)];
                    let expected=(n as f64/m as f64).powi(3)*(2.0*std::f64::consts::PI*frequency as f64*xyz[axis] as f64/m as f64).cos();
                    assert!((value as f64-expected).abs()<2e-6,"axis {axis}, frequency {frequency}, index {i}");
                }
            }
        }
    }
    #[test]
    fn centred_impulse_agrees_with_explicit_spectral_sum() {
        let (n,m)=(4usize,6usize);
        let mut input=vec![0.0;n*n*n];
        input[(1*n+2)*n+3]=1.0;
        let actual=FourierResize::new(n,m).run(&input);
        for (i,&value) in actual.iter().enumerate() {
            let pos=[i%m,(i/m)%m,i/(m*m)];
            let mut expected=0.0;
            for z in -2..2 { for y in -2..2 { for x in -2..2 {
                let f=[x,y,z];
                let phase: f64=(0..3).map(|a| f[a] as f64*(pos[a] as f64/m as f64-[3,2,1][a] as f64/n as f64)).sum();
                expected+=(2.0*std::f64::consts::PI*phase).cos()/(m*m*m) as f64;
            }}}
            assert!((value as f64-expected).abs()<2e-6);
        }
    }

}
