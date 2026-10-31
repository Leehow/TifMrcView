use super::fourier;
use super::geom;
use super::volume::{self, Accum, Projector};

/// An asymmetric pseudo-protein: a few lobes of different sizes filled
/// with "atoms" (Gaussians of σ 0.8 px), so it has detail to Nyquist.
pub fn phantom(b: usize) -> Vec<f32> {
    let c = b as f64 / 2.0;
    let s = b as f64 / 32.0;
    let lobes = [([0.0, 0.0, 0.0], 5.0), ([6.0, 1.0, -2.0], 3.0), ([-3.0, 7.0, 2.0], 2.5), ([1.0, -6.0, 6.0], 2.0), ([-7.0, -2.0, -4.0], 2.5)];
    let mut rng = super::Rng(99);
    let mut atoms = vec![];
    for (q, r) in lobes {
        let n = (r * r * r * 3.0 * s * s * s) as usize;
        while atoms.len() < 4000 && {
            let mut placed = 0;
            for _ in 0..n * 4 {
                let p = [rng.unit() * 2.0 - 1.0, rng.unit() * 2.0 - 1.0, rng.unit() * 2.0 - 1.0];
                if p[0] * p[0] + p[1] * p[1] + p[2] * p[2] <= 1.0 {
                    atoms.push([q[0] * s + p[0] * r * s, q[1] * s + p[1] * r * s, q[2] * s + p[2] * r * s]);
                    placed += 1;
                    if placed >= n {
                        break;
                    }
                }
            }
            false
        } {}
    }
    let mut v = vec![0f32; b * b * b];
    let sig2 = 2.0 * 0.8f64 * 0.8;
    for a in &atoms {
        let (x0, y0, z0) = ((a[0] + c) as isize, (a[1] + c) as isize, (a[2] + c) as isize);
        for z in (z0 - 3).max(0)..(z0 + 4).min(b as isize) {
            for y in (y0 - 3).max(0)..(y0 + 4).min(b as isize) {
                for x in (x0 - 3).max(0)..(x0 + 4).min(b as isize) {
                    let d2 = (x as f64 - c - a[0]).powi(2) + (y as f64 - c - a[1]).powi(2) + (z as f64 - c - a[2]).powi(2);
                    v[(z as usize * b + y as usize) * b + x as usize] += (-d2 / sig2).exp() as f32;
                }
            }
        }
    }
    v
}

/// Real-space projection along z after rotating the volume by `r`
/// (nearest-sample ray sums): the reference for the Fourier slices.
fn project_real(v: &[f32], b: usize, r: &geom::Mat) -> Vec<f32> {
    let c = b as f64 / 2.0;
    let rt = geom::transpose(r);
    let mut out = vec![0f32; b * b];
    for y in 0..b {
        for x in 0..b {
            let mut s = 0.0;
            for z in 0..b {
                let p = geom::apply(&rt, [x as f64 - c, y as f64 - c, z as f64 - c]);
                let (i, j, k) = ((p[0] + c).round(), (p[1] + c).round(), (p[2] + c).round());
                if i >= 0.0 && j >= 0.0 && k >= 0.0 && (i as usize) < b && (j as usize) < b && (k as usize) < b {
                    s += v[((k as usize) * b + j as usize) * b + i as usize];
                }
            }
            out[y * b + x] = s;
        }
    }
    out
}

fn corr(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().map(|&v| v as f64).sum::<f64>() / n, b.iter().map(|&v| v as f64).sum::<f64>() / n);
    let (mut s, mut sa, mut sb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (*x as f64 - ma, *y as f64 - mb);
        s += x * y;
        sa += x * x;
        sb += y * y;
    }
    s / (sa * sb).sqrt()
}

#[test]
fn fourier_slices_match_real_projections() {
    let b = 32;
    let v = phantom(b);
    let p = Projector::new(&v, b, 2);
    for (rot, tilt, psi) in [(0.0, 0.0, 0.0), (30.0, 60.0, 10.0), (200.0, 120.0, 300.0)] {
        let r = geom::euler(rot, tilt, psi);
        let a = volume::project(&p, &r, b);
        let e = project_real(&v, b, &r);
        let c = corr(&a, &e);
        let (sa, se): (f32, f32) = (a.iter().sum(), e.iter().sum());
        assert!(c > 0.98, "{rot} {tilt} {psi}: {c}");
        assert!((sa / se - 1.0).abs() < 0.05, "scale {sa} {se}");
    }
}

#[test]
fn mask_correlation_is_removed_from_fsc() {
    // Half maps: the phantom low-passed to about 1/4 Nyquist plus
    // independent noise; a hard mask makes the noise correlate.
    let b = 64;
    let truth = super::refine::lowpass(&phantom(b), b, 1.0, 10.0);
    let mut rng = super::Rng(5);
    let sd = std_of(&truth) * 2.0;
    let h1: Vec<f32> = truth.iter().map(|v| v + sd * rng.gauss() as f32).collect();
    let h2: Vec<f32> = truth.iter().map(|v| v + sd * rng.gauss() as f32).collect();
    let mut mask = vec![1f32; b * b * b];
    volume::mask_sphere(&mut mask, b, 14.0, 0.0);
    let masked = |v: &[f32]| -> Vec<f32> { v.iter().zip(&mask).map(|(a, m)| a * m).collect() };
    let raw = super::post::fsc_raw(&masked(&h1), &masked(&h2), b);
    let fixed = super::post::corrected_fsc(&h1, &h2, &masked, b);
    let high = |f: &[f64]| f[16..30].iter().sum::<f64>() / 14.0;
    assert!(fixed[2] > 0.8, "low shells kept: {:.3}", fixed[2]);
    eprintln!("mask FSC beyond the signal: raw {:.3}, corrected {:.3}", high(&raw), high(&fixed));
    assert!(high(&raw) > 0.04, "the hard mask should correlate the noise: {:.3}", high(&raw));
    assert!(high(&fixed).abs() < 0.03, "corrected FSC beyond the signal {:.3} (raw {:.3})", high(&fixed), high(&raw));
}

#[test]
fn reconstructs_from_projections() {
    let b = 32;
    let v = phantom(b);
    let p = Projector::new(&v, b, 2);
    let half = fourier::Half::new(b, b as f64 / 2.0 - 1.0);
    let mut acc = Accum::new(b, 2);
    let mut pts = vec![];
    let mut buf = vec![0f32; 2 * half.len()];
    let ones = vec![1f32; half.len()];
    for r in geom::grid(12.0).iter().step_by(7) {
        p.slice(r, &half, &mut buf);
        acc.points(r, &[geom::IDENTITY], &half, &buf, &ones, &mut pts);
    }
    acc.insert(&pts);
    let rec = acc.reconstruct(None);
    let c = corr(&rec, &v);
    assert!(c > 0.95, "{c}");
}

use super::data::{self, CtfEval, Optics, Particle, Stack, StackWriter};
use super::{refine, Rng};

/// Noisy CTF-modulated projections of `vol` at random orientations and
/// shifts; returns the stack path and the true particles.
pub fn synth_stack(dir: &std::path::Path, vol: &[f32], b: usize, px: f64, n: usize, noise: f32, seed: u64) -> (String, Vec<Particle>, Optics) {
    let optics = Optics { kv: 300.0, cs_mm: 2.7, amplitude: 0.1, pixel_a: px };
    let p = Projector::new(vol, b, 2);
    let mut rng = Rng(seed);
    let path = dir.join("synth.mrcs");
    let mut w = StackWriter::create(&path, b, px, "synthetic").unwrap();
    let fft = crate::fft::Fft::new(b);
    let mut truth = vec![];
    for _ in 0..n {
        let r = refine::random_rotation(&mut rng);
        let a = geom::angles(&r);
        let shift = [(rng.unit() * 6.0 - 3.0) as f32, (rng.unit() * 6.0 - 3.0) as f32];
        let df = 10000.0 + rng.unit() * 20000.0;
        let part = Particle { df1: (df + 300.0) as f32, df2: (df - 300.0) as f32, angle: (rng.unit() * 180.0) as f32, euler: [a[0] as f32, a[1] as f32, a[2] as f32], shift, class2d: -1, ..Default::default() };
        let ctf = CtfEval::new(&optics, &part, b, px);
        let (mut re, mut im) = (vec![0f32; b * b], vec![0f32; b * b]);
        for y in 0..b {
            for x in 0..b {
                let (kx, ky) = (fourier::freq(x, b) as f32, fourier::freq(y, b) as f32);
                let mut s = vec![0f32; 2];
                let half = fourier::Half { b, idx: vec![0], k: vec![[kx, ky]], shell: vec![0] };
                p.slice(&r, &half, &mut s);
                let c = ctf.at(kx, ky);
                let ph = -2.0 * std::f32::consts::PI * (kx * shift[0] + ky * shift[1]) / b as f32;
                let (sn, cs) = ph.sin_cos();
                let (a0, b0) = (s[0] * c, s[1] * c);
                re[y * b + x] = a0 * cs - b0 * sn;
                im[y * b + x] = a0 * sn + b0 * cs;
            }
        }
        let mut img = fourier::ift2(&fft, b, &re, &im);
        // `noise` is the noise σ relative to the clean particle's σ.
        let sd = std_of(&img);
        for v in &mut img {
            *v += noise * sd * rng.gauss() as f32;
        }
        data::normalise(&mut img, b, b as f64 * 0.4);
        w.push(&img).unwrap();
        truth.push(part);
    }
    w.finish().unwrap();
    (path.to_string_lossy().into(), truth, optics)
}

fn std_of(v: &[f32]) -> f32 {
    let n = v.len() as f32;
    let m = v.iter().sum::<f32>() / n;
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / n).sqrt()
}

/// Refinement from the true map low-passed to 30 Å recovers orientations
/// and a map that matches the truth (run with --release --ignored).
#[test]
#[ignore]
fn refines_synthetic_particles() {
    let dir = tempfile::tempdir().unwrap();
    let (b, px) = (64, 3.0);
    let truth = phantom(b);
    let n: usize = std::env::var("TMV_SPA_N").ok().and_then(|v| v.parse().ok()).unwrap_or(1500);
    let (path, true_parts, optics) = synth_stack(dir.path(), &truth, b, px, n, 3.0, 3);
    let stack = Stack::open(&path).unwrap();
    let mut parts: Vec<Particle> = true_parts.iter().map(|p| Particle { euler: [0.0; 3], shift: [0.0; 2], ..p.clone() }).collect();
    let ids: Vec<usize> = (0..n).collect();
    let start = refine::Map { b, px, v: truth.clone() };
    let t = std::time::Instant::now();
    let progress = |s: &str, d: usize, t: usize| eprintln!("{s} {d}/{t}");
    let r = refine::refine(&stack, &mut parts, &ids, &optics, 110.0, &start, &refine::RefineParams::default(), &progress).unwrap();
    eprintln!("refine {:?}, res {:.1} Å, history {:?}", t.elapsed(), r.res, r.history);
    eprintln!("fsc {:?}", r.fsc.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>());
    let tf = super::post::fsc(&r.full.v, &truth, b, 25.0);
    eprintln!("fsc vs truth {:?}", tf.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>());
    let errs: Vec<f64> = parts
        .iter()
        .zip(&true_parts)
        .map(|(a, b)| {
            let ra = geom::euler(a.euler[0] as f64, a.euler[1] as f64, a.euler[2] as f64);
            let rb = geom::euler(b.euler[0] as f64, b.euler[1] as f64, b.euler[2] as f64);
            geom::distance(&ra, &rb)
        })
        .collect();
    let mut sorted = errs.clone();
    sorted.sort_by(f64::total_cmp);
    eprintln!("orientation error median {:.2}°, 90% {:.2}°", sorted[n / 2], sorted[n * 9 / 10]);
    let c = corr(&r.full.v, &truth);
    eprintln!("map correlation with truth {c:.3}");
    assert!(sorted[n / 2] < 5.0);
    assert!(c > 0.9);
    assert!(r.res < 15.0);
}

/// Ab initio from random orientations, then refinement; compared with the
/// truth after a rotational search (either hand).
#[test]
#[ignore]
fn ab_initio_synthetic() {
    let dir = tempfile::tempdir().unwrap();
    let (b, px) = (64, 3.0);
    let truth = phantom(b);
    let n: usize = std::env::var("TMV_SPA_N").ok().and_then(|v| v.parse().ok()).unwrap_or(2000);
    let (path, true_parts, optics) = synth_stack(dir.path(), &truth, b, px, n, 3.0, 5);
    let stack = Stack::open(&path).unwrap();
    let mut parts: Vec<Particle> = true_parts.iter().map(|p| Particle { euler: [0.0; 3], shift: [0.0; 2], ..p.clone() }).collect();
    let ids: Vec<usize> = (0..n).collect();
    let progress = |s: &str, d: usize, t: usize| if d % 10 == 0 || d == t { eprintln!("{s} {d}/{t}") };
    let t = std::time::Instant::now();
    let init = refine::initial(&stack, &mut parts, &ids, &optics, 110.0, &refine::InitialParams::default(), &progress).unwrap();
    eprintln!("initial {:?}", t.elapsed());
    let up = fourier::resample3(&init.v, init.b, b);
    let (cc, _) = best_match(&up, &truth, b);
    let (ccf, _) = best_match(&super::post::flip_hand(&up, b), &truth, b);
    eprintln!("ab initio vs truth: {cc:.3} (flipped hand {ccf:.3})");
    data::write_mrc(&std::path::Path::new("/tmp/spa_init.mrc"), &up, [b; 3], px, "init").ok();
    assert!(cc.max(ccf) > 0.8);
}

/// Best correlation of `v` with rotated copies of `truth` (10° grid, low-passed to 20 Å).
fn best_match(v: &[f32], truth: &[f32], b: usize) -> (f64, geom::Mat) {
    use rayon::prelude::*;
    let lp_v = refine::lowpass(v, b, 3.0, 20.0);
    let lp_t = refine::lowpass(truth, b, 3.0, 20.0);
    let grid = geom::grid(10.0);
    let c = b as f64 / 2.0;
    // Points of the model above its mean, sampled.
    let pts: Vec<([f64; 3], f32)> = (0..b * b * b)
        .step_by(3)
        .map(|i| ([(i % b) as f64 - c, ((i / b) % b) as f64 - c, (i / b / b) as f64 - c], lp_v[i]))
        .collect();
    let at = |p: [f64; 3]| -> f32 {
        let (x, y, z) = ((p[0] + c).round(), (p[1] + c).round(), (p[2] + c).round());
        if x < 0.0 || y < 0.0 || z < 0.0 || x >= b as f64 || y >= b as f64 || z >= b as f64 {
            0.0
        } else {
            lp_t[((z as usize) * b + y as usize) * b + x as usize]
        }
    };
    let best = grid
        .par_iter()
        .map(|r| {
            let a: Vec<f32> = pts.iter().map(|p| p.1).collect();
            let t: Vec<f32> = pts.iter().map(|p| at(geom::apply(r, p.0))).collect();
            (corr(&a, &t), *r)
        })
        .reduce(|| (-1.0, geom::IDENTITY), |x, y| if x.0 >= y.0 { x } else { y });
    best
}

/// 2D classes group particles seen from similar directions.
#[test]
#[ignore]
fn classifies_synthetic_particles() {
    let dir = tempfile::tempdir().unwrap();
    let (b, px) = (64, 3.0);
    let truth = phantom(b);
    let n: usize = std::env::var("TMV_SPA_N").ok().and_then(|v| v.parse().ok()).unwrap_or(2000);
    let (path, true_parts, optics) = synth_stack(dir.path(), &truth, b, px, n, 3.0, 9);
    let stack = Stack::open(&path).unwrap();
    let mut parts: Vec<Particle> = true_parts.iter().map(|p| Particle { euler: [0.0; 3], shift: [0.0; 2], ..p.clone() }).collect();
    let ids: Vec<usize> = (0..n).collect();
    let t = std::time::Instant::now();
    let params = super::class2d::Class2dParams { classes: 10, iterations: 15, ..Default::default() };
    let r = super::class2d::run(&stack, &mut parts, &ids, &optics, 110.0, &params, &|_, _, _| {}).unwrap();
    eprintln!("2D {:?}, counts {:?}", t.elapsed(), r.counts);
    let mut all = vec![];
    for a in &r.averages {
        all.extend_from_slice(a);
    }
    data::write_mrc(std::path::Path::new("/tmp/spa_class2d.mrcs"), &all, [r.b, r.b, r.averages.len()], r.pixel_a, "classes").ok();
    // Purity: share of each class's particles within 35° of the class's most common direction.
    let dir_of = |p: &Particle| {
        let m = geom::euler(p.euler[0] as f64, p.euler[1] as f64, p.euler[2] as f64);
        m[2]
    };
    let mut pure = 0usize;
    for c in 0..r.averages.len() {
        let members: Vec<[f64; 3]> = (0..n).filter(|&i| parts[i].class2d == c as i32).map(|i| dir_of(&true_parts[i])).collect();
        let best = members
            .iter()
            .map(|d| members.iter().filter(|e| (d[0] * e[0] + d[1] * e[1] + d[2] * e[2]) > 35f64.to_radians().cos()).count())
            .max()
            .unwrap_or(0);
        pure += best;
    }
    let purity = pure as f64 / n as f64;
    eprintln!("purity {purity:.2}");
    assert!(purity > 0.35);
}

/// Synthetic micrographs: particles at random orientations on a grid,
/// plus "ice" (white noise) under the same CTF, plus shot noise.
fn synth_mics(dir: &std::path::Path, truth: &[f32], b: usize, px: f64, count: usize, side: usize) -> Vec<(f64, Vec<[f32; 2]>)> {
    let optics = Optics { kv: 300.0, cs_mm: 2.7, amplitude: 0.1, pixel_a: px };
    let p = Projector::new(truth, b, 2);
    let mut rng = Rng(77);
    let fft = crate::fft::Fft::new(side);
    let mut out = vec![];
    let spacing = (b as f64 * 1.3) as usize;
    for m in 0..count {
        let df = 12000.0 + 6000.0 * m as f64 / count as f64;
        let mut img = vec![0f32; side * side];
        let mut centres = vec![];
        let mut y = b;
        while y + b < side {
            let mut x = b;
            while x + b < side {
                let (cx, cy) = (x as f32 + rng.unit() as f32 * 6.0, y as f32 + rng.unit() as f32 * 6.0);
                let r = refine::random_rotation(&mut rng);
                let proj = volume::project(&p, &r, b);
                let (x0, y0) = (cx.round() as usize - b / 2, cy.round() as usize - b / 2);
                for j in 0..b {
                    for i in 0..b {
                        img[(y0 + j) * side + x0 + i] -= proj[j * b + i];
                    }
                }
                centres.push([cx.round(), cy.round()]);
                x += spacing;
            }
            y += spacing;
        }
        let sd = std_of(&img);
        for v in &mut img {
            *v += 1.5 * sd * rng.gauss() as f32;
        }
        // CTF over the whole micrograph.
        let part = Particle { df1: (df + 200.0) as f32, df2: (df - 200.0) as f32, angle: 30.0, ..Default::default() };
        let (mut re, mut im) = fourier::ft2(&fft, side, &img);
        let ctf = CtfEval::new(&optics, &part, side, px);
        for y in 0..side {
            for x in 0..side {
                let c = ctf.at(fourier::freq(x, side) as f32, fourier::freq(y, side) as f32);
                re[y * side + x] *= c;
                im[y * side + x] *= c;
            }
        }
        let mut img = fourier::ift2(&fft, side, &re, &im);
        let sd = std_of(&img);
        for v in &mut img {
            *v += 1.0 * sd * rng.gauss() as f32;
        }
        data::write_mrc(&dir.join(format!("mic_{m:03}.mrc")), &img, [side, side, 1], px, "synthetic micrograph").unwrap();
        out.push((df, centres));
    }
    out
}

/// The whole workflow from micrographs (run with --release --ignored).
#[test]
#[ignore]
fn workflow_from_micrographs() {
    use super::project;
    // TMV_SPA_KEEP=<empty folder> keeps the micrographs and the project.
    let tmp = tempfile::tempdir().unwrap();
    let keep = std::env::var("TMV_SPA_KEEP").ok().map(std::path::PathBuf::from);
    let dir = keep.as_deref().unwrap_or(tmp.path());
    std::fs::create_dir_all(dir).unwrap();
    let (b, px) = (64, 3.0);
    let truth = phantom(b);
    let n_mics: usize = std::env::var("TMV_SPA_MICS").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    let made = synth_mics(dir, &truth, b, px, n_mics, 1024);
    let log = |s: &str, d: usize, t: usize| if d == t { eprintln!("{s} done ({t})") };
    let mut p = project::Project::open(&dir.to_string_lossy()).unwrap();
    assert_eq!(p.mics.len(), n_mics);
    assert!((p.optics.pixel_a - px).abs() < 1e-3, "pixel {}", p.optics.pixel_a);
    p.diameter_a = 110.0;
    p.save().unwrap();
    let cp = crate::ctf::Params { voltage_kv: 300.0, cs_mm: 2.7, amp_contrast: 0.1, pixel_a: px, res_low_a: 30.0, res_high_a: 8.0, df_min_a: 3000.0, df_max_a: 50000.0, max_astig_a: 2000.0, phase: false, backend: Some("cpu".into()) };
    project::ctf(&mut p, &cp, 256, &log).unwrap();
    for (m, (df, _)) in p.mics.iter().zip(&made) {
        let c = m.ctf.as_ref().expect("ctf");
        let mean = (c.df1 + c.df2) / 2.0;
        eprintln!("defocus {mean:.0} (true {df:.0}), fit to {:.1} Å", c.fit_res);
        assert!((mean - df).abs() < 0.05 * df);
    }
    let pp = super::pick::PickParams { diameter_a: 110.0, ..Default::default() };
    let total = project::pick_all(&mut p, &pp, &log).unwrap();
    let picks = p.picks().unwrap();
    let (mut hit, mut truth_n) = (0, 0);
    for (q, (_, centres)) in picks.iter().zip(&made) {
        truth_n += centres.len();
        hit += centres.iter().filter(|c| q.iter().any(|k| (k[0] - c[0]).hypot(k[1] - c[1]) < 8.0)).count();
    }
    eprintln!("picked {total}, {hit} of {truth_n} true particles");
    assert!(hit as f64 > 0.8 * truth_n as f64);
    assert!((total as f64) < 1.3 * truth_n as f64);
    let s = project::extract_all(&mut p, 64, true, &log).unwrap();
    assert_eq!(s.count, total);
    let c2 = super::class2d::Class2dParams { classes: 8, iterations: 8, ..Default::default() };
    project::class2d(&mut p, &c2, &log).unwrap();
    let ip = refine::InitialParams { iterations: 40, batch: 200, ..Default::default() };
    project::initial(&mut p, &ip, &log).unwrap();
    let rp = refine::RefineParams { max_iterations: 6, ..Default::default() };
    let r = project::refine(&mut p, &rp, None, &log).unwrap();
    eprintln!("refined: {:.1} Å (masked {:.1} Å), B {:.0}", r.res, r.res_masked, r.bfactor);
    for f in [&r.map, &r.sharpened, &r.half1, &r.half2, &r.mask, &r.star] {
        assert!(std::path::Path::new(f).exists(), "{f}");
    }
    std::fs::copy(&r.sharpened, "/tmp/spa_workflow_sharp.mrc").ok();
    let (v, _, _) = data::read_volume(&r.map).unwrap();
    let (cc, _) = best_match(&v, &truth, b);
    let (ccf, _) = best_match(&super::post::flip_hand(&v, b), &truth, b);
    eprintln!("refined map vs truth (20 Å): {cc:.3} / flipped {ccf:.3}");
    assert!(r.res < 25.0);
}

/// Masked FSC and B factor of a refinement folder (`TMV_SPA_DIR=<…>/tifmrcview-spa`).
#[test]
#[ignore]
fn masked_fsc_of_folder() {
    let Ok(dir) = std::env::var("TMV_SPA_DIR") else { return };
    let read = |n: &str| data::read_volume(&format!("{dir}/{n}")).unwrap();
    let (h1, d, px) = read("refine_half1.mrc");
    let (h2, _, _) = read("refine_half2.mrc");
    let (mask, _, _) = read("refine_mask.mrc");
    let masked = |v: &[f32]| -> Vec<f32> { v.iter().zip(&mask).map(|(a, m)| a * m).collect() };
    let f = super::post::corrected_fsc(&h1, &h2, &masked, d[0]);
    eprintln!("{:?}", f.iter().map(|v| (v * 100.0).round() / 100.0).collect::<Vec<_>>());
    let (_, s) = super::post::sharpen(&h1, &h2, &mask, d[0], px, None);
    eprintln!("B {:.0} Å², masked {:.2} Å", s.bfactor, s.res_masked);
}

/// Writes a real project's RELION export and CSV to `TMV_SPA_OUT`
/// (`TMV_SPA_DIR` = its tifmrcview-spa folder).
#[test]
#[ignore]
fn export_of_folder() {
    let (Ok(dir), Ok(out)) = (std::env::var("TMV_SPA_DIR"), std::env::var("TMV_SPA_OUT")) else { return };
    let p = super::project::Project::load(&dir).unwrap();
    for f in super::export::run(&p, "relion", &out, &|_, _| {}).unwrap().into_iter().chain(super::export::run(&p, "table", &out, &|_, _| {}).unwrap()) {
        eprintln!("{f}");
    }
}
