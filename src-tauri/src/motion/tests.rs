use super::*;
use crate::image::OpenImage;
use std::path::Path;
fn rand(s: &mut u64) -> f64 {
    *s = s
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*s >> 32) as f64 / u32::MAX as f64
}
fn poisson(s: &mut u64, lambda: f64) -> f32 {
    let limit = (-lambda).exp();
    let mut p = 1.0;
    let mut k = 0;
    while p > limit {
        p *= rand(s).max(1e-12);
        k += 1;
    }
    (k - 1) as f32
}
fn rel(a: &[f32], b: &[f32]) -> f64 {
    (a.iter()
        .zip(b)
        .map(|(a, b)| (*a as f64 - *b as f64).powi(2))
        .sum::<f64>()
        / a.iter()
            .map(|v| (*v as f64).powi(2))
            .sum::<f64>()
            .max(1e-20))
    .sqrt()
}
fn stack(path: &Path, frames: &[Vec<f32>], w: usize, h: usize) {
    output::write(path.to_str().unwrap(), w, h, frames.len(), 2.0, |i| {
        Ok(frames[i].clone())
    })
    .unwrap();
}
fn texture(w: usize, h: usize) -> Vec<f32> {
    let mut seed = 202611;
    let mut v = vec![4.0; w * h];
    for _ in 0..300 {
        let cx = rand(&mut seed) * w as f64;
        let cy = rand(&mut seed) * h as f64;
        let sigma = 1.0 + rand(&mut seed) * 3.0;
        let amp = 10.0 + rand(&mut seed) * 12.0;
        for y in (cy as isize - 12).max(0)..(cy as isize + 13).min(h as isize) {
            for x in (cx as isize - 12).max(0)..(cx as isize + 13).min(w as isize) {
                v[y as usize * w + x as usize] += (amp
                    * (-((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2))
                        / (2.0 * sigma * sigma))
                        .exp()) as f32;
            }
        }
    }
    v
}

#[test]
fn synthetic_global_and_local_motion_with_poisson_noise() {
    begin();
    let dir = tempfile::tempdir().unwrap();
    let (w, h, n) = (256, 256, 20);
    let reference = texture(w, h);
    let times: Vec<_> = (0..n)
        .map(|i| 2.0 * (i as f64 + 0.5) / n as f64 - 1.0)
        .collect();
    let mut truth = local::Model::zero(&times);
    truth.coeff[0][3] = 0.8;
    truth.coeff[0][9] = 0.2;
    truth.coeff[1][6] = -0.7;
    truth.coeff[1][12] = 0.3;
    let shifts: Vec<_> = times
        .iter()
        .map(|t| [2.5 * t + 0.3 * (t * t - 1.0 / 3.0), -1.7 * t])
        .collect();
    let mut seed = 88;
    let frames: Vec<Vec<f32>> = (0..n)
        .map(|i| {
            (0..w * h)
                .map(|j| {
                    let (x, y) = ((j % w) as f64, (j / w) as f64);
                    let (mut sx, mut sy) = (x, y);
                    for _ in 0..4 {
                        let d = truth.displacement(
                            2.0 * sx / (w - 1) as f64 - 1.0,
                            2.0 * sy / (h - 1) as f64 - 1.0,
                            times[i],
                        );
                        sx = x - shifts[i][0] - d[0];
                        sy = y - shifts[i][1] - d[1];
                    }
                    poisson(
                        &mut seed,
                        crate::tomo::track::sample(&reference, w, h, sx, sy).max(0.0) as f64 * 6.0,
                    ) / 6.0
                })
                .collect()
        })
        .collect();
    let path = dir.path().join("movie.mrc");
    stack(&path, &frames, w, h);
    let p = Params {
        working_size: 256,
        patches: 3,
        backend: "cpu".into(),
        dose_weighted: false,
        ..Default::default()
    };
    let movie = input::Movie::open(path.to_str().unwrap(), None, &p, 0, 0, 0).unwrap();
    let spool = input::Spool::prepare(&movie, &p, dir.path(), &|_, _, _| {}).unwrap();
    let g = crate::align::movie_planes(&spool.frames, w, h, &|_, _| {}).unwrap();
    let rms = (g
        .shifts
        .iter()
        .zip(&shifts)
        .map(|(a, b)| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2))
        .sum::<f64>()
        / (2 * n) as f64)
        .sqrt();
    eprintln!("global drift RMS={rms}");
    assert!(rms < 0.3);
    let l = local::estimate(&spool, &g.shifts, &p, &|_, _, _| {}).unwrap();
    eprintln!(
        "local residual {} -> {}",
        l.global_residual, l.local_residual
    );
    assert!(l.local_residual < 0.5 * l.global_residual);
    let image_error = |model: &local::Model| {
        let corrected: Vec<_> = frames
            .iter()
            .enumerate()
            .map(|(i, f)| sum::warp(f, w, h, g.shifts[i], model, times[i]))
            .collect();
        let mut error = 0.0;
        for y in 24..h - 24 {
            for x in 24..w - 24 {
                let j = y * w + x;
                let avg = corrected.iter().map(|v| v[j] as f64).sum::<f64>() / n as f64;
                error += (avg - reference[j] as f64).powi(2);
            }
        }
        error
    };
    let global_error = image_error(&local::Model::zero(&times));
    let local_error = image_error(&l.model);
    eprintln!("image MSE sums {global_error} -> {local_error}");
    assert!(local_error < global_error);
}

#[test]
fn blank_movie_is_rejected_instead_of_applying_noise_peaks() {
    begin();
    let frames = vec![vec![1.0; 128 * 128]; 4];
    assert!(crate::align::movie_planes(&frames, 128, 128, &|_, _| {}).is_err());
}

#[test]
fn critical_exposure_and_finite_group_dose() {
    assert!((dose::critical(1.0, 300.0).unwrap() - 3.055).abs() < 1e-12);
    assert!((dose::critical(1.0, 200.0).unwrap() - 2.444).abs() < 1e-12);
    for (k, expected) in [
        (0.05, 38.733697205829415),
        (0.1, 14.138335024281877),
        (0.25, 5.2737462140627835),
        (0.5, 3.5869284538780786),
    ] {
        assert!((dose::critical(k, 300.0).unwrap() - expected).abs() < 1e-10);
    }
    assert!(dose::critical(0.0, 300.0).unwrap().is_infinite());
    assert_eq!(dose::weight(0.0, 300.0, 100.0, 2.0).unwrap(), 1.0);
    let expected = 6.11 * (1.0 - (-1.0f64 / 6.11).exp());
    assert!((dose::weight(1.0, 300.0, 0.0, 1.0).unwrap() - expected).abs() < 1e-12);
    assert!(
        dose::weight(0.1, 300.0, 5.0, 1.0).unwrap() > dose::weight(0.5, 300.0, 5.0, 1.0).unwrap()
    );
    assert!(dose::critical(0.1, 100.0).is_err());
}

fn tiny_eer() -> Vec<u8> {
    let codes = [(2u32, 1u32), (4, 14), (8, 0)];
    let mut encoded = vec![0u8; 12];
    let mut at = 0;
    for (run, sub) in codes {
        for b in 0..7 {
            encoded[at / 8] |= (((run >> b) & 1) as u8) << (at % 8);
            at += 1;
        }
        for b in 0..4 {
            encoded[at / 8] |= (((sub >> b) & 1) as u8) << (at % 8);
            at += 1;
        }
    }
    let entry_count = 9;
    let ifd_size = 2 + entry_count * 12 + 4;
    let data = 8 + 2 * ifd_size;
    let mut out = vec![0u8; data + 24];
    out[..8].copy_from_slice(&[b'I', b'I', 42, 0, 8, 0, 0, 0]);
    for f in 0..2 {
        let pos = 8 + f * ifd_size;
        out[pos..pos + 2].copy_from_slice(&(entry_count as u16).to_le_bytes());
        let tags = [
            (256, 4, 4),
            (257, 4, 4),
            (258, 3, 16),
            (259, 3, 65001),
            (262, 3, 1),
            (273, 4, data + f * 12),
            (277, 3, 1),
            (278, 4, 4),
            (279, 4, 12),
        ];
        for (i, (tag, typ, val)) in tags.into_iter().enumerate() {
            let p = pos + 2 + 12 * i;
            out[p..p + 2].copy_from_slice(&(tag as u16).to_le_bytes());
            out[p + 2..p + 4].copy_from_slice(&(typ as u16).to_le_bytes());
            out[p + 4..p + 8].copy_from_slice(&1u32.to_le_bytes());
            out[p + 8..p + 12].copy_from_slice(&(val as u32).to_le_bytes());
        }
        let next = if f == 0 { 8 + ifd_size } else { 0 };
        out[pos + ifd_size - 4..pos + ifd_size].copy_from_slice(&(next as u32).to_le_bytes());
        out[data + f * 12..data + (f + 1) * 12].copy_from_slice(&encoded);
    }
    out
}
#[test]
fn native_eer_group_and_super_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tiny.eer");
    std::fs::write(&path, tiny_eer()).unwrap();
    let e = eer::Eer::open(&path).unwrap();
    assert_eq!((e.w, e.h, e.len()), (4, 4, 2));
    let v = e.group(0, 2, 1).unwrap();
    assert_eq!(v.iter().sum::<f32>(), 4.0);
    assert_eq!(v[2], 2.0);
    assert_eq!(v[7], 2.0);
    let v = e.group(0, 1, 2).unwrap();
    assert_eq!(v.len(), 64);
    assert_eq!(v.iter().sum::<f32>(), 2.0);
    assert_eq!(v[13], 1.0); // (5,1), raw event at detector (2,0).
    assert_eq!(v[22], 1.0); // (6,2), raw event at detector (3,1).
}

#[test]
fn cpu_gpu_fourier_warp_and_dose_sums() {
    begin();
    let Some((dev, _)) = crate::ctf_gpu::device() else {
        eprintln!("SKIP no GPU");
        return;
    };
    let (w, h) = (64, 32);
    let mut re = texture(w, h);
    let input = re.clone();
    let mut im = vec![0.0; w * h];
    fourier::Cpu::new(w, h).transform(&mut re, &mut im, false);
    use burn::tensor::{Tensor, TensorData};
    let gpu = fourier::Gpu::new(w, h, dev);
    let (gr, gi) = gpu.transform(
        Tensor::from_data(TensorData::new(input.clone(), [h, w]), dev),
        Tensor::zeros([h, w], dev),
        false,
    );
    assert!(rel(&re, &sum_gpu::download(gr.clone())) < 1e-4);
    assert!(rel(&im, &sum_gpu::download(gi.clone())) < 1e-4);
    let (back, _) = gpu.transform(gr, gi, true);
    assert!(rel(&input, &sum_gpu::download(back)) < 1e-5);
    let dir = tempfile::tempdir().unwrap();
    let (w, h) = (128, 128);
    let frames = vec![texture(w, h); 4];
    let path = dir.path().join("movie.mrc");
    stack(&path, &frames, w, h);
    let p = Params {
        local: false,
        working_size: 128,
        backend: "cpu".into(),
        ..Default::default()
    };
    let m = input::Movie::open(path.to_str().unwrap(), None, &p, 0, 0, 0).unwrap();
    let sp = input::Spool::prepare(&m, &p, dir.path(), &|_, _, _| {}).unwrap();
    let mut model = local::Model::zero(&sp.times);
    model.coeff[0][3] = 0.1;
    model.coeff[1][6] = 0.2;
    let shifts = vec![[0.3, -0.4]; 4];
    let ex = sum::exposure(&sp.counts, 0.5, 2.0);
    let cpu = sum::cpu(&sp, &shifts, &model, Some(&ex), 2.0, 300.0, &|_, _, _| {}).unwrap();
    let gpu = crate::ctf_gpu::guarded(|| {
        sum_gpu::run(
            dev,
            &sp,
            &shifts,
            &model,
            Some(&ex),
            2.0,
            300.0,
            &|_, _, _| {},
        )
    })
    .unwrap()
    .unwrap();
    for (a, b) in [
        (&cpu.unweighted, &gpu.unweighted),
        (&cpu.even, &gpu.even),
        (&cpu.odd, &gpu.odd),
        (
            cpu.weighted.as_ref().unwrap(),
            gpu.weighted.as_ref().unwrap(),
        ),
    ] {
        let r = rel(a, b);
        eprintln!("sum relative {r}");
        assert!(r < 1e-5);
    }
}

#[test]
fn mdoc_batch_sorts_outputs_but_not_pre_exposure() {
    begin();
    let dir = tempfile::tempdir().unwrap();
    let frames = vec![texture(128, 128); 4];
    for (name, scale) in [("a.mrc", 1.0), ("b.mrc", 2.0)] {
        let data: Vec<Vec<f32>> = frames
            .iter()
            .map(|v| v.iter().map(|x| x * scale).collect())
            .collect();
        stack(&dir.path().join(name), &data, 128, 128);
    }
    let text="PixelSpacing = 2\nVoltage = 300\n[ZValue = 0]\nTiltAngle = 3\nSubFramePath = C:\\frames\\a.mrc\nExposureDose = 2\nDateTime = 13-Oct-25 14:00:02\n[ZValue = 1]\nTiltAngle = -3\nSubFramePath = b.mrc\nExposureDose = 3\nDateTime = 13-Oct-25 14:00:01\n[ZValue = 2]\nTiltAngle = 0\nSubFramePath = missing.eer\nExposureDose = 4\nDateTime = 13-Oct-25 14:00:00\n";
    let mdoc = dir.path().join("input.mdoc");
    std::fs::write(&mdoc, text).unwrap();
    let target = dir.path().join("series.mrc");
    let r = series::run(
        &series::SeriesReq {
            mdoc: mdoc.to_string_lossy().into(),
            folder: None,
            output: target.to_string_lossy().into(),
            params: Params {
                backend: "cpu".into(),
                local: false,
                dose_weighted: false,
                working_size: 128,
                ..Default::default()
            },
            drop_tilts: false,
        },
        &|_, _, _| {},
    )
    .unwrap();
    assert_eq!(r.angles, [-3.0, 3.0]);
    assert_eq!(r.records.iter().map(|r| r.z).collect::<Vec<_>>(), [2, 1, 0]);
    assert_eq!(
        r.records.iter().map(|r| r.pre_exposure).collect::<Vec<_>>(),
        [0.0, 4.0, 7.0]
    );
    assert!(r.records[0].skipped.is_some());
    assert_eq!(
        std::fs::read_to_string(target.with_extension("tlt")).unwrap(),
        "-3.000000\n3.000000\n"
    );
    for (path, scale) in [
        (r.path.unwrap(), 8.0),
        (r.even.unwrap(), 4.0),
        (r.odd.unwrap(), 4.0),
    ] {
        let i = OpenImage::new(0, crate::formats::open(Path::new(&path)).unwrap(), 1 << 20);
        assert_eq!(i.info().size_z, 2);
        let first = i.page(0).unwrap();
        let second = i.page(1).unwrap();
        for (j, &value) in frames[0].iter().enumerate() {
            assert!((first[0].data.get_f64(j) - scale * value as f64).abs() < 1e-3);
            assert!((second[0].data.get_f64(j) - scale * 0.5 * value as f64).abs() < 1e-3);
        }
    }
    assert!(!std::fs::read_dir(dir.path()).unwrap().any(|p| p
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".motion-work")));
}

#[test]
fn frames_on_t_and_oriented_gain() {
    use crate::image::{ImageInfo, Plane, Source};
    struct Time {
        inner: Box<dyn Source>,
        info: ImageInfo,
    }
    impl Source for Time {
        fn info(&self) -> &ImageInfo {
            &self.info
        }
        fn read_page(&self, page: usize) -> anyhow::Result<Vec<Plane>> {
            self.inner.read_page(page)
        }
    }
    begin();
    let dir = tempfile::tempdir().unwrap();
    let (w, h) = (128, 128);
    let path = dir.path().join("time.mrc");
    stack(&path, &vec![vec![1.0; w * h]; 4], w, h);
    let inner = crate::formats::open(&path).unwrap();
    let mut info = inner.info().clone();
    info.size_t = 4;
    info.size_z = 1;
    let img = OpenImage::new(9, Box::new(Time { inner, info }), 64 << 20);
    let gain = dir.path().join("gain.mrc");
    let values = (0..w * h)
        .map(|i| 1.0 + (i % w) as f32 / (w - 1) as f32 + 2.0 * (i / w) as f32 / (h - 1) as f32)
        .collect::<Vec<_>>();
    stack(&gain, &[values], w, h);
    let p = Params {
        gain_path: Some(gain.to_string_lossy().into()),
        gain_rotate: 1,
        gain_flip_x: true,
        working_size: 128,
        local: false,
        ..Default::default()
    };
    let movie = input::Movie::open(path.to_str().unwrap(), Some(img), &p, 0, 0, 0).unwrap();
    assert_eq!(movie.shape(), (w, h, 4));
    let sp = input::Spool::prepare(&movie, &p, dir.path(), &|_, _, _| {}).unwrap();
    assert_eq!(sp.len(), 4);
    let first = sp.read(0).unwrap();
    let expected = 1.0 + 24.0 / 127.0 + 80.0 / 127.0;
    assert!((first[24 * w + 40] as f64 - expected).abs() < 1e-6);
}

#[test]
fn mdoc_pixel_is_rescaled_to_the_frames() {
    // EMPIAR-10164: binned images 924×958 at 5.4 Å, super-resolution frames 7420×7676.
    let doc = series::parse("PixelSpacing = 5.4\nImageSize = 924 958\n[ZValue = 0]\nTiltAngle = 0\nPixelSpacing = 5.4\n").unwrap();
    assert_eq!(doc.image_size, Some([924, 958]));
    assert!((doc.frame_pixel(5.4, (7420, 7676)) - 0.675).abs() < 1e-9);
    assert_eq!(doc.frame_pixel(5.4, (924, 958)), 5.4);
    assert!((doc.frame_pixel(5.4, (3710, 3838)) - 1.35).abs() < 1e-9);
}

#[test]
fn mdoc_batch_finds_the_one_gain_reference() {
    use super::series::find_gain;
    let dir = tempfile::tempdir().unwrap();
    let frames = dir.path().join("frames");
    std::fs::create_dir(&frames).unwrap();
    let mdoc = dir.path().join("ts.mrc.mdoc");
    std::fs::write(&mdoc, "").unwrap();
    let movie = frames.join("a.eer");
    std::fs::write(&movie, "").unwrap();
    std::fs::write(frames.join("notes.txt"), "").unwrap();
    assert_eq!(find_gain(&mdoc, Some(&movie), None), None);
    std::fs::write(frames.join("20260610_gain.gain"), "").unwrap();
    assert_eq!(find_gain(&mdoc, Some(&movie), None), Some(frames.join("20260610_gain.gain")));
    // Two candidates: which one is meant is not known.
    std::fs::write(dir.path().join("CountRef_x.mrc"), "").unwrap();
    assert_eq!(find_gain(&mdoc, Some(&movie), None), None);
}
