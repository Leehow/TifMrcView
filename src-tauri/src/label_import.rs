//! Masks made elsewhere become label layers: a segmentation from IMOD,
//! membrain-seg, Amira, Fiji or any program that writes a volume the app
//! reads (MRC/REC, TIFF stacks, a PNG for one micrograph). Layers are what
//! the workspaces pick from (STA, single particles), so a mask from another
//! program is as good a way in as painting.
//!
//! How the values are read:
//! - Whole numbers with at most `MOST_LABELS` different non-zero values (a
//!   label map, or a 0/1 or 0/255 mask): each value is one layer.
//! - Anything else (a probability or score map): one layer of the voxels
//!   above half way between the lowest and the highest value.
//!
//! The mask may be the image's size or a whole-number fraction of it on
//! every axis it shrinks (a mask of the binned tomogram): each mask voxel
//! then covers that many voxels of the image. A mask of one section on a
//! single-section image works the same way.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::image::OpenImage;
use crate::labels::Labels;

/// More different values than this are not taken for a label map.
const MOST_LABELS: usize = 255;

/// Colours of the layers made (one after another).
const COLOURS: [[u8; 3]; 8] = [[230, 80, 60], [60, 160, 230], [80, 200, 90], [240, 180, 40], [170, 90, 220], [40, 200, 200], [230, 110, 170], [150, 150, 150]];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    /// The layers made: label number and name.
    pub layers: Vec<(u16, String)>,
    /// How many image voxels each mask voxel stands for, per axis.
    pub scale: [usize; 3],
    /// Sections changed.
    pub zs: Vec<usize>,
}

fn factor(image: usize, mask: usize) -> Option<usize> {
    (mask > 0 && image % mask == 0).then(|| image / mask)
}

/// Adds the mask in `path` to `labels` as new layers named after the file.
pub fn import(labels: &mut Labels, path: &Path) -> Result<Imported> {
    let img = OpenImage::new(0, crate::formats::open(path).with_context(|| format!("打不开 {}", path.display()))?, 256 << 20);
    let info = img.info().clone();
    let (mw, mh, md) = (info.width, info.height, info.size_z.max(1));
    let (Some(fx), Some(fy), Some(fz)) = (factor(labels.w, mw), factor(labels.h, mh), factor(labels.d, md)) else {
        bail!("mask 大小 {mw}×{mh}×{md} 和图像 {}×{}×{} 对不上（要一样大，或者是图像的整数分之一）", labels.w, labels.h, labels.d);
    };
    // The values, section by section (masks are small next to the memory a label map takes).
    let mut values: Vec<f32> = Vec::with_capacity(mw * mh * md);
    for z in 0..md {
        let (page, sample) = info.locate(0, z, 0);
        let planes = img.page(page)?;
        let p = &planes[sample.min(planes.len() - 1)];
        values.extend((0..mw * mh).map(|i| p.data.get_f64(i) as f32));
    }
    let whole = values.iter().all(|v| v.is_finite() && v.fract() == 0.0);
    let mut distinct: Vec<i64> = vec![];
    if whole {
        for &v in &values {
            let v = v as i64;
            if v != 0 && !distinct.contains(&v) {
                distinct.push(v);
                if distinct.len() > MOST_LABELS {
                    break;
                }
            }
        }
    }
    let stem = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let stem = stem.split('.').next().unwrap_or("mask").to_string();
    // Each layer: its name and which mask voxels it takes.
    let groups: Vec<(String, Box<dyn Fn(f32) -> bool>)> = if whole && !distinct.is_empty() && distinct.len() <= MOST_LABELS {
        distinct.sort_unstable();
        let one = distinct.len() == 1;
        distinct.into_iter().map(|v| (if one { stem.clone() } else { format!("{stem} {v}") }, Box::new(move |x: f32| x as i64 == v) as Box<dyn Fn(f32) -> bool>)).collect()
    } else {
        let (lo, hi) = values.iter().filter(|v| v.is_finite()).fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        if !(hi > lo) {
            bail!("mask 里没有东西（所有值都一样）");
        }
        let cut = 0.5 * (lo + hi);
        vec![(stem.clone(), Box::new(move |x: f32| x > cut) as Box<dyn Fn(f32) -> bool>)]
    };
    let (w, h, d) = (labels.w, labels.h, labels.d);
    let mut out = Imported { layers: vec![], scale: [fx, fy, fz], zs: vec![] };
    for (k, (name, takes)) in groups.iter().enumerate() {
        let mut masks: Vec<(usize, Vec<bool>)> = vec![];
        for z in 0..d {
            let mz = z / fz;
            let base = mz * mw * mh;
            let mut m = vec![false; w * h];
            let mut any = false;
            for y in 0..h {
                let row = base + (y / fy) * mw;
                for x in 0..w {
                    if takes(values[row + x / fx]) {
                        m[y * w + x] = true;
                        any = true;
                    }
                }
            }
            if any {
                masks.push((z, m));
            }
        }
        if masks.is_empty() {
            continue;
        }
        // A name already in use gets a number, so an import never paints into an existing layer.
        let mut unique = name.clone();
        let mut n = 2;
        while labels.materials.iter().any(|m| m.name == unique) {
            unique = format!("{name} ({n})");
            n += 1;
        }
        let label = labels.material_label(&unique, COLOURS[k % COLOURS.len()]);
        let refs: Vec<(usize, &[bool])> = masks.iter().map(|(z, m)| (*z, &m[..])).collect();
        out.zs.extend(labels.apply_masks(&refs, label, 0, false));
        out.layers.push((label, unique));
    }
    if out.layers.is_empty() {
        bail!("mask 里没有非零的体素");
    }
    out.zs.sort_unstable();
    out.zs.dedup();
    Ok(out)
}

#[cfg(all(test, feature = "pro"))]
mod tests {
    use super::*;

    fn write_mrc(path: &Path, v: &[f32], dims: [usize; 3]) {
        crate::spa::data::write_mrc(path, v, dims, 1.0, "test mask").unwrap();
    }

    #[test]
    fn a_label_map_becomes_one_layer_per_value() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("seg.mrc");
        // 4×4×2 map of the image's 8×8×2 (binned 2 in X and Y): values 0, 3, 7.
        let mut v = vec![0f32; 4 * 4 * 2];
        v[0] = 3.0;
        v[16 + 5] = 7.0;
        write_mrc(&p, &v, [4, 4, 2]);
        let mut l = Labels::new(8, 8, 2);
        let r = import(&mut l, &p).unwrap();
        assert_eq!(r.scale, [2, 2, 1]);
        assert_eq!(r.layers.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["seg 3", "seg 7"]);
        let (a, b) = (r.layers[0].0, r.layers[1].0);
        assert_eq!(l.get(1, 1, 0), a);
        assert_eq!(l.get(2, 2, 0), 0);
        assert_eq!(l.get(3, 3, 1), b);
    }

    #[test]
    fn a_score_map_is_cut_half_way() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("prob.mrc");
        let v: Vec<f32> = (0..16).map(|i| i as f32 / 15.0 * 0.9 + 0.05).collect();
        write_mrc(&p, &v, [4, 4, 1]);
        let mut l = Labels::new(4, 4, 1);
        let r = import(&mut l, &p).unwrap();
        assert_eq!(r.layers.len(), 1);
        assert_eq!(l.get(0, 0, 0), 0);
        assert_eq!(l.get(3, 3, 0), r.layers[0].0);
        // A second import of the same file makes a layer of its own.
        let r2 = import(&mut l, &p).unwrap();
        assert_ne!(r2.layers[0].0, r.layers[0].0);
        assert_eq!(r2.layers[0].1, "prob (2)");
    }

    #[test]
    fn a_mask_of_another_size_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.mrc");
        write_mrc(&p, &[1.0; 9], [3, 3, 1]);
        let mut l = Labels::new(8, 8, 1);
        assert!(import(&mut l, &p).unwrap_err().to_string().contains("对不上"));
    }
}
