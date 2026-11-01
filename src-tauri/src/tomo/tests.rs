//! Deterministic phantoms exercise image tracking, model fitting, the
//! transpose identity, reconstruction quality and the actual GPU backend.
use super::*;
use recon::{Filter, Geometry, Method, Params};

fn rng(state: &mut u64) -> f64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 32) as f64 / u32::MAX as f64
}
fn corr(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len() as f64;
    let ma = a.iter().map(|&v| v as f64).sum::<f64>() / n;
    let mb = b.iter().map(|&v| v as f64).sum::<f64>() / n;
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (&a, &b) in a.iter().zip(b) {
        let (a, b) = (a as f64 - ma, b as f64 - mb);
        ab += a * b;
        aa += a * a;
        bb += b * b;
    }
    ab / (aa * bb).sqrt()
}

fn bead_series() -> (Stack, Vec<[f64; 2]>) {
    bead_series_shape(128, 128)
}

fn bead_series_shape(w: usize, h: usize) -> (Stack, Vec<[f64; 2]>) {
    bead_series_with(w, h, &|_, _| (0.0, 0.0, 1.0))
}

/// Beads seen through per-view distortions `(rotation °, tilt error °, mag)`
/// given by the view index and its nominal angle.
fn bead_series_with(
    w: usize,
    h: usize,
    distort: &dyn Fn(usize, f64) -> (f64, f64, f64),
) -> (Stack, Vec<[f64; 2]>) {
    let mut seed = 20261030;
    let angles: Vec<f64> = (-20..=20).map(|i| i as f64 * 3.0).collect();
    let mut beads = Vec::new();
    for y in 0..4 {
        for x in 0..w / 32 {
            beads.push((
                [
                    (x as f64 + 0.5) * 32.0 - w as f64 / 2.0 + (rng(&mut seed) - 0.5) * 4.0,
                    (y as f64 + 0.5) * 32.0 - h as f64 / 2.0 + (rng(&mut seed) - 0.5) * 4.0,
                    (rng(&mut seed) - 0.5) * 10.0,
                ],
                2.0 + rng(&mut seed),
            ));
        }
    }
    let shifts: Vec<[f64; 2]> = angles
        .iter()
        .map(|a| {
            if *a == 0.0 {
                [0.0; 2]
            } else {
                [(rng(&mut seed) - 0.5) * 3.0, (rng(&mut seed) - 0.5) * 3.0]
            }
        })
        .collect();
    let mut views = Vec::new();
    for (i, &a) in angles.iter().enumerate() {
        let (rot, err, mag) = distort(i, a);
        let p = geometry::projection(a + err, 84.6 + rot);
        let mut v = vec![0.0; w * h];
        for (x, amp) in &beads {
            let q: [f64; 2] = std::array::from_fn(|k| {
                mag * (0..3).map(|j| p[k][j] * x[j]).sum::<f64>()
                    + ([w, h][k] - 1) as f64 / 2.0
                    + shifts[i][k]
            });
            for y in (q[1] as isize - 8).max(0)..(q[1] as isize + 9).min(h as isize) {
                for x in (q[0] as isize - 8).max(0)..(q[0] as isize + 9).min(w as isize) {
                    let r2 = (x as f64 - q[0]).powi(2) + (y as f64 - q[1]).powi(2);
                    v[y as usize * w + x as usize] += (amp * (-r2 / 5.0).exp()) as f32;
                }
            }
        }
        for x in &mut v {
            *x += ((rng(&mut seed) - 0.5) * 0.02) as f32;
        }
        views.push(v);
    }
    // Dose-symmetric source order is represented in the map; numerical views
    // are intentionally sorted and must never be tracked in acquisition order.
    let file_order = (0..angles.len())
        .map(|i| {
            if i <= 20 {
                40 - 2 * i
            } else {
                2 * (i - 21) + 1
            }
        })
        .collect();
    (
        Stack {
            w,
            h,
            views,
            angles,
            file_order,
        },
        shifts,
    )
}

#[test]
fn grouped_rotation_tilt_and_magnification() {
    // A slow in-plane rotation drift, a tilt scale error and a magnification
    // step in the positive half: what the shared groups are for.
    let distort = |i: usize, a: f64| {
        (
            0.04 * i as f64 - 0.8,
            0.02 * a,
            if a > 20.0 { 1.01 } else { 1.0 },
        )
    };
    let (st, _) = bead_series_with(256, 128, &distort);
    let patch = track::PatchConfig {
        nx: 8,
        ny: 4,
        size: 30,
    };
    let tr = track::track(&st, 84.6, patch, &|_, _| {}).unwrap();
    let plain = model::solve(&tr, &st.angles, 84.6, model::Groups::NONE).unwrap();
    let groups = model::Groups {
        rotation: 3,
        tilt: 3,
        mag: 3,
    };
    let s = model::solve(&tr, &st.angles, 84.6, groups).unwrap();
    eprintln!(
        "residual plain {:.3} grouped {:.3}",
        plain.residual, s.residual
    );
    assert!(
        s.residual < 0.5 * plain.residual,
        "{} vs {}",
        s.residual,
        plain.residual
    );
    assert!(s.residual < 0.3, "grouped residual {}", s.residual);
    let zero = 20;
    for (i, &a) in st.angles.iter().enumerate() {
        // A tilt scale error trades with the depth scale of the points (a
        // gauge at small angles), so only the fit shows it, not the angles.
        let (rot, _, mag) = distort(i, a);
        let (rot0, _, _) = distort(zero, 0.0);
        // Within a group of three the truth varies by up to ±0.04°.
        assert!(
            (s.rotations[i] - (rot - rot0)).abs() < 0.2,
            "view {i} rotation {} vs {}",
            s.rotations[i],
            rot - rot0
        );
        assert!(
            (s.magnifications[i] - mag).abs() < 0.004,
            "view {i} mag {} vs {mag}",
            s.magnifications[i]
        );
    }
}

#[test]
fn phantom_tracking_axis_sweep_and_shifts() {
    let (st, truth) = bead_series();
    let (tr, s) = track_and_solve(
        &st,
        88.6,
        track::PatchConfig {
            nx: 4,
            ny: 4,
            size: 30,
        },
        &[],
        true,
        &|_, _| {},
    )
    .unwrap();
    eprintln!(
        "tracking axis={} residual={} counts={:?}",
        s.axis, s.residual, tr.counts
    );
    assert!((s.axis - 84.6).abs() <= 0.3, "axis {}", s.axis);
    assert!(s.residual < 0.5, "residual {}", s.residual);
    // x/z translation gauges are fixed at zero tilt. Remove their remaining
    // depth-origin gauge before judging the raw per-view translations.
    let r = geometry::rotation(s.axis);
    let mut num = 0.0;
    let mut den = 0.0;
    for (i, a) in st.angles.iter().enumerate() {
        let sn = a.to_radians().sin();
        for (k, row) in r.iter().enumerate() {
            num += sn * row[0] * (s.shifts[i][k] - truth[i][k]);
            den += (sn * row[0]).powi(2);
        }
    }
    let dz = num / den;
    let rms = (s
        .shifts
        .iter()
        .zip(&truth)
        .enumerate()
        .map(|(i, (a, b))| {
            (0..2)
                .map(|k| (a[k] - b[k] - dz * st.angles[i].to_radians().sin() * r[k][0]).powi(2))
                .sum::<f64>()
        })
        .sum::<f64>()
        / (2 * truth.len()) as f64)
        .sqrt();
    assert!(rms < 0.5, "shift rms {rms}");
}

/// Three Gaussian blobs per XZ slice, slightly different on each row;
/// returns the geometry, the truth [rows][z][w] and its sinograms.
fn recon_phantom() -> (Geometry, Vec<f32>, Vec<f32>, usize) {
    let (w, z, rows) = (48, 40, 5);
    let angles: Vec<_> = (-20..=20).map(|i| i as f64 * 3.0).collect();
    let g = Geometry::new(w, z, &angles, 0.0).unwrap();
    let mut truth = vec![0.0; rows * w * z];
    for y in 0..rows {
        for k in 0..w * z {
            let (x, zz) = ((k % w) as f32, (k / w) as f32);
            truth[y * w * z + k] = [(-8.0, -5.0, 2.0), (9.0, 7.0, 3.0), (-2.0, 10.0, 1.5)]
                .iter()
                .map(|(dx, dz, s)| {
                    let r2 = (x - 23.5 - dx).powi(2) + (zz - 19.5 - dz).powi(2);
                    (-r2 / (2.0 * s * s)).exp()
                })
                .sum::<f32>()
                * (1.0 + y as f32 * 0.1);
        }
    }
    let b = truth
        .chunks(w * z)
        .flat_map(|x| recon::forward(&g, x))
        .collect();
    (g, truth, b, rows)
}

fn sirt_error(g: &Geometry, b: &[f32], x: &[f32]) -> f64 {
    let n = g.views() * g.w;
    b.chunks(n)
        .zip(x.chunks(g.z * g.w))
        .map(|(b, x)| {
            recon::forward(g, x)
                .iter()
                .zip(b)
                .map(|(a, b)| ((a - b) as f64).powi(2))
                .sum::<f64>()
        })
        .sum::<f64>()
        .sqrt()
}

#[test]
fn projector_geometry_and_reconstruction_quality() {
    let (g, truth, b, _) = recon_phantom();
    // A blob at (x, depth v) projects to (w−1)/2 + cos θ·(x − (w−1)/2) + sin θ·v.
    let slice: Vec<f32> = (0..g.z * g.w)
        .map(|i| {
            let (x, z) = (
                (i % g.w) as f32 - 30.0,
                (i / g.w) as f32 - (g.z / 2 + 6) as f32,
            );
            (-(x * x + z * z) / 4.5).exp()
        })
        .collect();
    let sino = recon::forward(&g, &slice);
    for (v, [c, s]) in g.trig.iter().enumerate() {
        let row = &sino[v * g.w..(v + 1) * g.w];
        let centroid = row
            .iter()
            .enumerate()
            .map(|(j, a)| j as f64 * *a as f64)
            .sum::<f64>()
            / row.iter().map(|&a| a as f64).sum::<f64>();
        let want = c * (30.0 - 23.5) + s * (g.depth(g.z / 2 + 6)) + 23.5;
        assert!(
            (centroid - want).abs() < 0.05,
            "view {v}: {centroid} vs {want}"
        );
    }
    for (method, iterations) in [(Method::Wbp, 15), (Method::Sirt, 15), (Method::Sart, 3)] {
        let p = Params {
            thickness: g.z,
            method,
            iterations,
            lambda: if method == Method::Sart { 0.5 } else { 1.0 },
            backend: "cpu".into(),
            ..Default::default()
        };
        let x = recon::cpu_slab(&g, &b, &p).unwrap();
        let cc = corr(&x, &truth);
        eprintln!("{method:?} phantom cc={cc}");
        assert!(
            cc > if method == Method::Wbp { 0.80 } else { 0.85 },
            "{method:?} cc {cc}"
        );
    }
    let errors: Vec<f64> = [2, 5, 15]
        .iter()
        .map(|&iterations| {
            let p = Params {
                thickness: g.z,
                method: Method::Sirt,
                iterations,
                backend: "cpu".into(),
                ..Default::default()
            };
            sirt_error(&g, &b, &recon::cpu_slab(&g, &b, &p).unwrap())
        })
        .collect();
    assert!(
        errors.windows(2).all(|e| e[1] < e[0]),
        "SIRT residuals {errors:?}"
    );
    for f in [Filter::Cosine, Filter::Hamming] {
        assert!(recon::filtered(&b, g.w, f).iter().all(|v| v.is_finite()));
    }
}

#[test]
fn streaming_slabs_match_one_slab() {
    let (g, _, b, rows) = recon_phantom();
    let data: Vec<f32> = (0..g.views())
        .flat_map(|v| (0..rows).flat_map(move |y| (0..g.w).map(move |x| (y, v, x))))
        .map(|(y, v, x)| b[(y * g.views() + v) * g.w + x])
        .collect();
    let views = recon::Views {
        data: &data,
        w: g.w,
        h: rows,
        views: g.views(),
    };
    let p = Params {
        thickness: g.z,
        backend: "cpu".into(),
        ..Default::default()
    };
    let mut out = vec![0.0; rows * g.z * g.w];
    recon::reconstruct(
        &views,
        &g,
        &p,
        &mut |y0, n, slab| {
            out[y0 * g.z * g.w..(y0 + n) * g.z * g.w].copy_from_slice(&slab);
            Ok(())
        },
        &|_, _| {},
    )
    .unwrap();
    // Streaming subtracts each view's mean over all rows first.
    let mut means = vec![0f64; g.views()];
    for (i, v) in b.iter().enumerate() {
        means[(i / g.w) % g.views()] += *v as f64 / (rows * g.w) as f64;
    }
    let centred: Vec<f32> = b
        .iter()
        .enumerate()
        .map(|(i, v)| v - means[(i / g.w) % g.views()] as f32)
        .collect();
    let want = recon::cpu_slab(&g, &centred, &p).unwrap();
    assert!(out.iter().zip(&want).all(|(a, b)| (a - b).abs() < 1e-5));
}

#[test]
fn gpu_matches_cpu() {
    let Some((dev, name)) = crate::ctf_gpu::device() else {
        eprintln!("SKIP: no GPU device");
        return;
    };
    let (g, _, b, rows) = recon_phantom();
    for method in [Method::Wbp, Method::Sirt, Method::Sart] {
        let p = Params {
            thickness: g.z,
            method,
            iterations: if method == Method::Sart { 3 } else { 15 },
            lambda: if method == Method::Sart { 0.5 } else { 1.0 },
            ..Default::default()
        };
        let cpu = recon::cpu_slab(&g, &b, &p).unwrap();
        let gpu = crate::ctf_gpu::guarded(|| recon_gpu::Plan::new(dev, &g, &p)?.slab(&b, rows))
            .unwrap()
            .unwrap();
        let e = cpu
            .iter()
            .zip(&gpu)
            .map(|(a, b)| (*a as f64 - *b as f64).powi(2))
            .sum::<f64>();
        let n = cpu.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
        let rel = (e / n).sqrt();
        eprintln!("{method:?} {name} relative L2={rel}");
        assert!(rel < 1e-4);
    }
}

#[test]
fn patch_spacing_and_quality() {
    let p = track::PatchConfig::default_for(2048, 2048);
    assert_eq!(p.nx, 20);
    for p in track::PatchConfig::candidates(1024, 1024) {
        assert!(p.size <= 1024 / p.nx);
    }
    assert!(track::PatchConfig {
        nx: 8,
        ny: 8,
        size: 150
    }
    .validate(1024, 1024)
    .is_err());
    assert_eq!(quality(2.9), "<3 px");
    assert_eq!(quality(4.1), "4–7 px");
}

#[test]
fn sorted_input_and_shared_even_odd_geometry() {
    let dir = tempfile::tempdir().unwrap();
    let angles = [30.0, -30.0, 0.0, -15.0, 15.0];
    let w = 16;
    let h = 16;
    let frames: Vec<f32> = (0..5 * w * h)
        .map(|i| ((i % w) as f32 * 0.2).sin() + (i / (w * h)) as f32)
        .collect();
    let mut imgs = Vec::new();
    for (i, (name, factor)) in [("ts", 1.0), ("ts_even", 1.0), ("ts_odd", 2.0)]
        .iter()
        .enumerate()
    {
        let path = dir.path().join(format!("{name}.mrc"));
        output::write_mrc(
            path.to_str().unwrap(),
            &frames.iter().map(|v| v * factor).collect::<Vec<_>>(),
            [w, h, 5],
            2.276,
            false,
        )
        .unwrap();
        imgs.push(OpenImage::new(
            i as u64 + 1,
            crate::formats::open(&path).unwrap(),
            1 << 20,
        ));
    }
    let st = read_stack(&imgs[0], 0, 0, 1, &angles).unwrap();
    assert_eq!(st.file_order, [1, 3, 2, 4, 0]);
    assert_eq!(st.angles, [-30.0, -15.0, 0.0, 15.0, 30.0]);
    assert_eq!(st.views[0], frames[w * h..2 * w * h]);
    let a = Alignment {
        width: w,
        height: h,
        bin: 1,
        axis: 90.0,
        patch: track::PatchConfig {
            nx: 2,
            ny: 2,
            size: 8,
        },
        residual: 0.0,
        quality: quality(0.0).into(),
        views: st
            .file_order
            .iter()
            .zip(&st.angles)
            .map(|(&file_index, &angle)| View {
                file_index,
                angle,
                residual: 0.0,
                shift: [0.0; 2],
                mag: 1.0,
                rotation: 0.0,
                tilt: angle,
                cc: 1.0,
                contours: 10,
                flagged: false,
                excluded: file_index == 4,
            })
            .collect(),
        tracks: track::Tracks {
            observations: vec![],
            points: 0,
            cc: vec![],
            counts: vec![],
            ms: 0.0,
            cpu_pair_ms: 0.0,
            gpu_pair_ms: None,
            backend: String::new(),
        },
        ms: 0.0,
        tilt_offset: 0.0,
    };
    // The dataset finds the half stacks motion correction leaves beside the
    // stack; one alignment reconstructs all three.
    let mut d = dataset::Dataset::open(&imgs[0].info().path, 5, Some(2.276)).unwrap();
    assert!(d.even.as_deref().unwrap().ends_with("ts_even.mrc"));
    assert!(d.odd.as_deref().unwrap().ends_with("ts_odd.mrc"));
    d.angles = angles.to_vec();
    d.alignment = Some(a.clone());
    let step = pipeline::ReconStep {
        bin: 2,
        params: Params {
            thickness: 8,
            backend: "cpu".into(),
            ..Default::default()
        },
        halves: true,
        phase_flip: false,
        ctf_3d: false,
    };
    let out = pipeline::reconstruct(&mut d, &step, &|_, _, _| {}).unwrap();
    assert_eq!(out.paths.len(), 3);
    assert!(
        out.paths[0].ends_with("ts_wbp_b2.mrc") && out.paths[1].ends_with("ts_wbp_b2_even.mrc")
    );
    let open =
        |path: &str| OpenImage::new(9, crate::formats::open(Path::new(path)).unwrap(), 1 << 20);
    use std::path::Path;
    let e = open(&out.paths[1]);
    let o = open(&out.paths[2]);
    assert_eq!(
        (e.info().width, e.info().height, e.info().size_z),
        (8, 8, 8)
    );
    assert!((output::pixel_a(&e) - 4.552).abs() < 1e-5);
    for z in 0..8 {
        let ep = e.page(z).unwrap();
        let op = o.page(z).unwrap();
        for i in 0..64 {
            assert!((2.0 * ep[0].data.get_f64(i) - op[0].data.get_f64(i)).abs() < 1e-5);
        }
    }
    let again = dataset::Dataset::open(&imgs[0].info().path, 5, None).unwrap();
    assert_eq!(again.tomograms.len(), 3);
    assert_eq!(again.tomograms[0].halves.as_ref().unwrap()[1], out.paths[2]);
    d.optics.pixel_a = Some(2.276);
    let files = export::run(&d, "relion5", dir.path().to_str().unwrap(), 1, &|_, _| {}).unwrap();
    let series = std::fs::read_to_string(&files[1]).unwrap();
    // Four tilts kept (one excluded), each with its frame and the halves.
    assert_eq!(
        series.lines().filter(|l| l.starts_with("frames/")).count(),
        4
    );
    assert!(series.contains("_rlnTomoZRot #11") && series.contains("_rlnMicrographNameOdd #15"));
    assert!(dir
        .path()
        .join("ts_relion5/frames/ts_001_-30.0.mrc")
        .exists());
    assert!(std::fs::read_to_string(&files[0])
        .unwrap()
        .contains("tilt_series/ts.star"));
    let files = export::run(&d, "imod", dir.path().to_str().unwrap(), 1, &|_, _| {}).unwrap();
    let xf = std::fs::read_to_string(files.iter().find(|f| f.ends_with(".xf")).unwrap()).unwrap();
    assert_eq!(xf.lines().count(), 5);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ts/ts_exclude.txt")).unwrap(),
        "5\n"
    );
    assert!(dir.path().join("ts/ts.st").exists());
    // Running again replaces its own outputs instead of numbering new ones.
    let out2 = pipeline::reconstruct(&mut d, &step, &|_, _, _| {}).unwrap();
    assert_eq!(out2.paths, out.paths);
    assert!(output::save(
        &imgs[0],
        &output::SaveReq {
            token: 1,
            c: 0,
            t: 0,
            alignment: a,
            bin: 1,
            path: imgs[0].info().path.clone(),
            transforms_only: true
        },
        &|_, _| {}
    )
    .is_err());
}

#[test]
fn rectangular_tracking_and_working_pixel_configs() {
    let (st, _) = bead_series_shape(384, 128);
    let patch = track::PatchConfig {
        nx: 12,
        ny: 4,
        size: 30,
    };
    let (_, s) = track_and_solve(&st, 88.6, patch, &[], true, &|_, _| {}).unwrap();
    assert!((s.axis - 84.6).abs() <= 0.3, "rectangular axis {}", s.axis);
    assert!(s.residual < 0.5, "rectangular residual {}", s.residual);
    let p = track::PatchConfig::default_for(959, 256);
    assert!((60..=150).contains(&p.size));
    assert!(p.nx > p.ny);
    assert_eq!(
        track::PatchConfig::default_for(1024, 1024),
        track::PatchConfig {
            nx: 10,
            ny: 10,
            size: 80
        }
    );
    assert_eq!(
        track::PatchConfig::default_for(2048, 2048),
        track::PatchConfig {
            nx: 20,
            ny: 20,
            size: 80
        }
    );
    for b in [1, 2, 4, 8] {
        let p = track::PatchConfig::default_for(8192 / b, 8192 / b);
        assert_eq!(p.size, 80);
    }
    assert_eq!(auto_bin(3838, 1024), 1);
    assert_eq!(auto_bin(4096, 2048), 2);
    assert_eq!(auto_bin(8192, 8192), 8);
}

#[test]
fn batched_patch_cc_matches_cached_cpu() {
    let Some((dev, _)) = crate::ctf_gpu::device() else {
        return;
    };
    let n = 64;
    let mut seed = 42;
    let pairs: Vec<_> = (0..8)
        .map(|_| {
            let a: Vec<f32> = (0..n * n).map(|_| rng(&mut seed) as f32).collect();
            let b = (0..n * n)
                .map(|i| a[((i / n + 2) % n) * n + (i % n + 3) % n])
                .collect();
            (a, b)
        })
        .collect();
    let gpu =
        crate::ctf_gpu::guarded(|| correlation_gpu::Batch::new(dev, n).correlate(&pairs, 10.0))
            .unwrap()
            .unwrap();
    for ((a, b), g) in pairs.iter().zip(gpu) {
        let c = crate::align::patch_shift(a, b, n, 10.0);
        assert!((c.0[0] - g.0[0]).hypot(c.0[1] - g.0[1]) < 0.01);
        assert!((c.1 - g.1).abs() < 1e-4);
    }
}

#[test]
fn relion_pose_is_the_relion_projection() {
    // RELION 5 projects X to Rz(ZRot)·[cos YTilt·x + sin YTilt·z, y] + shift
    // (its rlnTomoProjX/Y rows); ours is m·R(axis + ρ)·[…] + shift.
    let v = View {
        file_index: 0,
        angle: 30.0,
        residual: 0.0,
        shift: [3.5, -2.0],
        mag: 1.0,
        rotation: 0.7,
        tilt: 30.4,
        cc: 1.0,
        contours: 1,
        flagged: false,
        excluded: false,
    };
    let a = Alignment {
        width: 64,
        height: 64,
        bin: 1,
        axis: 84.6,
        patch: track::PatchConfig {
            nx: 2,
            ny: 2,
            size: 8,
        },
        residual: 0.0,
        quality: String::new(),
        views: vec![v.clone()],
        tracks: track::Tracks {
            observations: vec![],
            points: 0,
            cc: vec![],
            counts: vec![],
            ms: 0.0,
            cpu_pair_ms: 0.0,
            gpu_pair_ms: None,
            backend: String::new(),
        },
        ms: 0.0,
        tilt_offset: 0.0,
    };
    let [z, sx, sy] = export::relion_pose(&a, &v, 2.0);
    let x = [10.0, -7.0, 4.0];
    let (s, c) = v.tilt.to_radians().sin_cos();
    let q = [c * x[0] + s * x[2], x[1]];
    let (zs, zc) = z.to_radians().sin_cos();
    let relion = [
        zc * q[0] - zs * q[1] + sx / 2.0,
        zs * q[0] + zc * q[1] + sy / 2.0,
    ];
    let p = geometry::projection(v.tilt, a.axis + v.rotation);
    let ours: [f64; 2] =
        std::array::from_fn(|k| (0..3).map(|j| p[k][j] * x[j]).sum::<f64>() + v.shift[k]);
    assert!(
        (relion[0] - ours[0]).abs() < 1e-9 && (relion[1] - ours[1]).abs() < 1e-9,
        "{relion:?} vs {ours:?}"
    );
}

#[test]
fn phase_flip_turns_the_ctf_into_its_magnitude() {
    // An image seen through sin γ (one defocus, untilted); flipping must give
    // the same image seen through |sin γ|.
    let (n, pixel, df) = (64usize, 4.0, 15000.0);
    let l = crate::ctf::wavelength(300.0);
    let (a, b, w) = (
        std::f64::consts::PI * l,
        0.5 * std::f64::consts::PI * 2.7e7 * l.powi(3),
        (0.07f64 / (1.0f64 - 0.0049).sqrt()).atan(),
    );
    let mut seed = 5;
    let texture: Vec<f32> = (0..n * n).map(|_| rng(&mut seed) as f32).collect();
    let fft = crate::motion::fourier::Cpu::new(n, n);
    let through = |transfer: &dyn Fn(f64) -> f64| {
        let (mut re, mut im) = (texture.clone(), vec![0.0; n * n]);
        fft.transform(&mut re, &mut im, false);
        for i in 0..n * n {
            let (fx, fy) = (
                crate::motion::sum::freq(i % n, n),
                crate::motion::sum::freq(i / n, n),
            );
            let k2 = (fx * fx + fy * fy) / (n as f64 * pixel).powi(2);
            let g = transfer(a * df * k2 - b * k2 * k2 + w) as f32;
            re[i] *= g;
            im[i] *= g;
        }
        fft.transform(&mut re, &mut im, true);
        re
    };
    let mut seen = through(&|g| g.sin());
    let want = through(&|g| g.sin().abs());
    ctf::flip(
        &mut seen,
        n,
        n,
        &ctf::Flip {
            optics: crate::ctf::Optics::of(300.0, 2.7, 0.07),
            df1: df,
            df2: df,
            angle: 0.0,
            theta: 0.0,
            gradient: 0,
            pixel_a: pixel,
            offset: 0.0,
        },
    );
    let err = seen
        .iter()
        .zip(&want)
        .map(|(a, b)| ((a - b) as f64).powi(2))
        .sum::<f64>();
    let norm = want.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
    assert!(
        (err / norm).sqrt() < 1e-4,
        "relative error {}",
        (err / norm).sqrt()
    );
}

#[test]
fn depth_and_strip_defocus_make_the_beam_height() {
    // A point at lateral u, depth v (specimen frame) in a view tilted by θ sits
    // at detector d = u cos θ + v sin θ and height h = −u sin θ + v cos θ;
    // strip gradient at d plus the slab's depth term is −gradient·h (pixels → Å).
    for &(u, v, theta, g) in &[
        (12.0, 5.0, 40.0, 1i8),
        (-30.0, -8.0, -55.0, -1),
        (7.0, 20.0, 10.0, 1),
    ] {
        let (s, c) = f64::to_radians(theta).sin_cos();
        let d = u * c + v * s;
        let h = -u * s + v * c;
        let strip = g as f64 * 2.0 * f64::to_radians(theta).tan() * d;
        let total = strip + ctf::depth_defocus(v, theta, g, 2.0);
        assert!(
            (total + g as f64 * 2.0 * h).abs() < 1e-9,
            "{total} vs {}",
            -(g as f64) * 2.0 * h
        );
    }
}
