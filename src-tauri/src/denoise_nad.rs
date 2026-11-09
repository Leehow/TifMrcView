//! Full-volume native NAD/EED filtering and per-image disk-backed result cache.
//! The in-process numerical kernel is in nad_eed.rs. No IMOD runtime is used.
use crate::denoise::Params;
use crate::formats::mrc::MrcSource;
use crate::image::{OpenImage, Source};
use anyhow::{bail, Result};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
static CANCEL_EPOCH: AtomicU64 = AtomicU64::new(0);
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
pub fn cancel() {
    CANCEL_EPOCH.fetch_add(1, Ordering::SeqCst);
}
#[derive(PartialEq)]
struct Key {
    c: usize,
    t: usize,
    strength: f32,
    iterations: usize,
}
pub(crate) struct Filtered {
    // Release mapping before scratch cleanup, including on Windows.
    source: MrcSource,
    _scratch: Scratch,
    key: Key,
}
impl Filtered {
    pub fn plane(&self, z: usize) -> Result<Vec<f32>> {
        if z >= self.source.info().size_z {
            bail!("层号超出范围");
        }
        let page = self.source.read_page(z)?;
        let p = &page[0];
        Ok((0..p.w * p.h).map(|i| p.data.get_f64(i) as f32).collect())
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self> {
        for _ in 0..32 {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("tmv-nad-{}-{id}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        bail!("无法创建 NAD 缓存目录")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
/// Compute allXYZ voxels once, then serve both slice previews and exports from
/// this same result. K is expressed in the original full-volume population SD.
pub(crate) fn prepare(img: &OpenImage, c: usize, t: usize, p: &Params) -> Result<Arc<Filtered>> {
    if !p.strength.is_finite()
        || p.strength <= 0.0
        || !(1..=200).contains(&p.iterations)
        || p.slab != 0
    {
        bail!("NAD 3D 需要 K > 0、迭代 1–200 次；不能同时使用 Z 邻层平均");
    }
    let info = img.info();
    if info.is_rgb || info.size_z < 2 || c >= info.size_c || t >= info.size_t {
        bail!("NAD 3D 需要至少两层的灰度体数据及有效的通道、时间点");
    }
    let epoch = CANCEL_EPOCH.load(Ordering::SeqCst);
    let check = || -> Result<()> {
        if CANCEL_EPOCH.load(Ordering::SeqCst) != epoch || img.cancelled.load(Ordering::SeqCst) {
            bail!("已取消");
        }
        Ok(())
    };
    let key = Key {
        c,
        t,
        strength: p.strength,
        iterations: p.iterations,
    };
    let mut cached = img
        .denoise_nad
        .lock()
        .map_err(|_| anyhow::anyhow!("NAD 缓存不可用"))?;
    check()?;
    if let Some(result) = cached.as_ref().filter(|r| r.key == key) {
        return Ok(result.clone());
    }
    let (w, h, nz) = (info.width, info.height, info.size_z);
    let len = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(nz))
        .ok_or_else(|| anyhow::anyhow!("NAD 体尺寸过大"))?;
    let mut volume = Vec::new();
    volume.try_reserve_exact(len)?;
    let (mut count, mut mean, mut m2) = (0u64, 0f64, 0f64);
    for z in 0..nz {
        check()?;
        for v in crate::denoise::input(img, c, z, t, 0)? {
            if !v.is_finite() {
                bail!("NAD 输入包含非有限数值");
            }
            count += 1;
            let delta = v as f64 - mean;
            mean += delta / count as f64;
            m2 += delta * (v as f64 - mean);
            volume.push(v);
        }
    }
    let sd = (m2 / count as f64).max(0.0).sqrt() as f32;
    let k = (p.strength * sd).max(f32::MIN_POSITIVE);
    if !k.is_finite() {
        bail!("NAD 的 K 值过大，请减小强度或缩放输入数据");
    }
    // A failed/cancelled computation leaves the last usable result intact.
    let filtered = crate::nad_eed::diffuse(volume, [w, h, nz], k, p.iterations, &check)?;
    check()?;
    let scratch = Scratch::new()?;
    let output = scratch.0.join("filtered.mrc");
    let (mut lo, mut hi, mut mean, mut m2) = (f32::INFINITY, f32::NEG_INFINITY, 0f64, 0f64);
    for (i, &v) in filtered.iter().enumerate() {
        lo = lo.min(v);
        hi = hi.max(v);
        let delta = v as f64 - mean;
        mean += delta / (i + 1) as f64;
        m2 += delta * (v as f64 - mean);
    }
    let mut writer = BufWriter::new(File::create(&output)?);
    writer.write_all(&crate::align::mrc_header(
        w,
        h,
        nz,
        crate::align::pixel_a(info),
        [lo, hi, mean as f32, (m2 / len as f64).sqrt() as f32],
        "TifMrcView: native NAD3D / EED",
    ))?;
    for plane in filtered.chunks(w * h) {
        check()?;
        let mut bytes = Vec::with_capacity(plane.len() * 4);
        for v in plane {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        writer.write_all(&bytes)?;
    }
    writer.flush()?;
    drop(writer);
    drop(filtered);
    check()?;
    let source = MrcSource::open(&output)?;
    let result = Arc::new(Filtered {
        source,
        _scratch: scratch,
        key,
    });
    *cached = Some(result.clone());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::denoise::{self, Method, PreviewReq, SaveReq};

    fn params() -> Params {
        Params {
            method: Method::Nad3d,
            strength: 0.8,
            iterations: 3,
            patch: 1,
            search: 7,
            slab: 0,
            device: "cpu".into(),
        }
    }

    fn fixture(dir: &std::path::Path) -> Arc<OpenImage> {
        let (w, h, nz) = (16, 12, 6);
        let values: Vec<f32> = (0..w * h * nz)
            .map(|i| {
                let (x, y, z) = (i % w, i / w % h, i / (w * h));
                (x as f32 * 0.3).sin()
                    + y as f32 * 0.06
                    + z as f32 * 0.4
                    + ((i * 37 % 101) as f32 - 50.0) * 0.02
            })
            .collect();
        let path = dir.join("input.mrc");
        let mut f = File::create(&path).unwrap();
        f.write_all(&crate::align::mrc_header(
            w,
            h,
            nz,
            2.5,
            [-1.0, 5.0, 1.0, 1.0],
            "NAD test",
        ))
        .unwrap();
        for v in values {
            f.write_all(&v.to_le_bytes()).unwrap();
        }
        drop(f);
        OpenImage::new(7, Box::new(MrcSource::open(&path).unwrap()), 0)
    }

    #[test]
    fn native_nad_works_without_imod_installation() {
        let dir = tempfile::tempdir().unwrap();
        let img = fixture(dir.path());
        let result = prepare(&img, 0, 0, &params())
            .expect("NAD must run inside the App, without any IMOD program or environment");
        assert_eq!(result.plane(0).unwrap().len(), 16 * 12);
        assert_eq!(result.plane(5).unwrap().len(), 16 * 12);
    }

    #[test]
    fn rejects_invalid_volume_parameters_instead_of_using_2d() {
        let dir = tempfile::tempdir().unwrap();
        let img = fixture(dir.path());
        let mut p = params();
        p.slab = 1;
        assert!(prepare(&img, 0, 0, &p).is_err());
        p.slab = 0;
        p.strength = f32::NAN;
        assert!(prepare(&img, 0, 0, &p).is_err());
        assert!(denoise::filter(vec![1.0; 16], 4, 4, &params(), 1.0).is_err());
    }

    #[test]
    fn native_preview_all_z_and_export_share_the_filtered_volume() {
        let dir = tempfile::tempdir().unwrap();
        let img = fixture(dir.path());
        let original = fs::read(dir.path().join("input.mrc")).unwrap();
        let p = params();
        let result = prepare(&img, 0, 0, &p).unwrap();
        let all: Vec<f32> = (0..6)
            .flat_map(|z| crate::denoise::input(&img, 0, z, 0, 0).unwrap())
            .collect();
        let mut changed = false;
        for z in 0..6 {
            let preview = denoise::preview(
                &img,
                &PreviewReq {
                    token: 7,
                    z,
                    c: 0,
                    t: 0,
                    region: [0.0, 0.0, 16.0, 12.0],
                    params: p.clone(),
                },
            )
            .unwrap();
            let plane = result.plane(z).unwrap();
            for (i, &v) in preview.values.iter().enumerate() {
                assert!(v.is_finite());
                assert!((v - plane[i]).abs() < 1e-6, "Z={z} pixel={i}");
                changed |= (v - all[z * 192 + i]).abs() > 1e-4;
            }
            assert_eq!(preview.backend, "NAD 3D (CPU)");
        }
        assert!(changed, "Native NAD must actually filter the input");
        assert!(
            Arc::ptr_eq(&result, &prepare(&img, 0, 0, &p).unwrap()),
            "Z changes reuse the full-volume result"
        );
        for zs in [vec![], vec![4]] {
            let path = dir.path().join(format!("saved-{}.mrc", zs.len()));
            denoise::save(
                &img,
                &SaveReq {
                    token: 7,
                    zs: zs.clone(),
                    c: 0,
                    t: 0,
                    path: path.to_string_lossy().into(),
                    params: p.clone(),
                },
                &|_, _| {},
            )
            .unwrap();
            let saved = MrcSource::open(&path).unwrap();
            assert_eq!(saved.info().size_z, if zs.is_empty() { 6 } else { 1 });
            assert_eq!(saved.info().pixel_size.unwrap()[0], 2.5);
            for z in 0..saved.info().size_z {
                let saved_plane = saved.read_page(z).unwrap();
                let expected_plane = result.plane(if zs.is_empty() { z } else { zs[z] }).unwrap();
                for i in 0..192 {
                    assert_eq!(saved_plane[0].data.get_f64(i), expected_plane[i] as f64);
                }
            }
        }
        assert_eq!(fs::read(dir.path().join("input.mrc")).unwrap(), original);
        let cached_path = result._scratch.0.clone();
        drop(result);
        drop(img);
        assert!(
            !cached_path.exists(),
            "closing the image removes its NAD cache"
        );
    }

    #[test]
    fn original_imod_golden_volumes_match_without_external_software() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nad3d");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        for case in manifest["cases"].as_array().unwrap() {
            let shape = case["shape"].as_array().unwrap();
            let (w, h, nz) = (
                shape[0].as_u64().unwrap() as usize,
                shape[1].as_u64().unwrap() as usize,
                shape[2].as_u64().unwrap() as usize,
            );
            let path = dir
                .path()
                .join(format!("{}.mrc", case["name"].as_str().unwrap()));
            let mut f = File::create(&path).unwrap();
            f.write_all(&crate::align::mrc_header(
                w,
                h,
                nz,
                2.5,
                [-10.0, 10.0, 0.0, 1.0],
                "synthetic NAD reference",
            ))
            .unwrap();
            f.write_all(&fs::read(root.join(case["input"].as_str().unwrap())).unwrap())
                .unwrap();
            drop(f);
            let img = OpenImage::new(9, Box::new(MrcSource::open(&path).unwrap()), 0);
            for reference in case["references"].as_array().unwrap() {
                let mut p = params();
                p.strength = reference["strength"].as_f64().unwrap() as f32;
                p.iterations = reference["iterations"].as_u64().unwrap() as usize;
                let actual = prepare(&img, 0, 0, &p).unwrap();
                let bytes = fs::read(root.join(reference["output"].as_str().unwrap())).unwrap();
                let expected: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
                    .collect();
                let values: Vec<f32> = (0..nz).flat_map(|z| actual.plane(z).unwrap()).collect();
                assert_eq!(values.len(), expected.len());
                for (i, (&a, &b)) in values.iter().zip(&expected).enumerate() {
                    assert_eq!(
                        a, b,
                        "case={} K={} n={} voxel={i}",
                        case["name"], p.strength, p.iterations
                    );
                }
            }
        }
    }

    #[test]
    fn parameter_changes_invalidate_cache_and_cancellation_preserves_last_result() {
        let dir = tempfile::tempdir().unwrap();
        let img = fixture(dir.path());
        let first = prepare(&img, 0, 0, &params()).unwrap();
        let mut p = params();
        p.iterations += 1;
        let second = prepare(&img, 0, 0, &p).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_ne!(first.plane(2).unwrap(), second.plane(2).unwrap());
        p.strength *= 2.0;
        let third = prepare(&img, 0, 0, &p).unwrap();
        assert!(!Arc::ptr_eq(&second, &third));
        img.cancelled.store(true, Ordering::SeqCst);
        assert!(prepare(&img, 0, 0, &params()).is_err());
        assert!(Arc::ptr_eq(
            img.denoise_nad.lock().unwrap().as_ref().unwrap(),
            &third
        ));
        assert!(third._scratch.0.is_dir());
    }
}
