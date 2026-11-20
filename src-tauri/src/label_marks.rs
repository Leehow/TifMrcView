//! Label layers from marks given as numbers: an agent (or a script) names
//! points on objects, outlines or lines, and each layer of the label map
//! (`<image>.labels.tif`, the one the Labels brush paints) is filled the way
//! the brush's own tools fill it: a point is a click of the smart selection
//! tool (sam_pick.rs: SAM when it is there, the classical picker otherwise),
//! an outline is the lasso, a line is the brush. The window runs this
//! through `labels_marks` on the open image (undoable, shown at once);
//! `tmv labels IMAGE --marks marks.json` runs it without a window.
//!
//! marks.json:
//! ```json
//! {"layers": [
//!   {"name": "线粒体", "color": [230, 25, 75],
//!    "points": [[120, 88, 40], [300, 210, 41]], "size": 200,
//!    "polygons": [{"z": 40, "pts": [[10, 10], [60, 10], [60, 50]]}],
//!    "strokes": [{"z": 40, "pts": [[0, 0], [90, 90]], "radius": 4}]}
//! ]}
//! ```
//! Coordinates are image pixels (x, y) and the slice z, counted from 0. A
//! layer is found by `label`, else by `name` (made when missing), else it is
//! a new label. `size` (pixels) is about how large the objects are: the
//! picker looks at a square of twice that around each point (default: the
//! whole slice). `erase: true` takes the marks away instead. `"masks":
//! ["mask.tif", …]` next to `"layers"` adds whole masks an agent made itself
//! (label_import.rs, as 导入 mask in the window).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::image::OpenImage;
use crate::labels::{Labels, Values};

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Marks {
    pub layers: Vec<Layer>,
    /// Mask files (any volume the app reads, the image's size or a whole
    /// fraction of it) added as new layers, as 标签 → 导入 mask does.
    pub masks: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Layer {
    pub label: Option<u16>,
    pub name: String,
    pub color: Option<[u8; 3]>,
    /// [x, y, z] on objects: one smart-selection click each.
    pub points: Vec<[f32; 3]>,
    pub size: Option<f32>,
    pub polygons: Vec<Shape>,
    pub strokes: Vec<Shape>,
    pub erase: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Shape {
    pub z: usize,
    pub pts: Vec<[f32; 2]>,
    pub radius: Option<f32>,
}

// The label palette of the window (ui/pro/pro.js LABEL_COLORS), for new named layers.
const COLORS: [[u8; 3]; 16] = [
    [230, 25, 75], [60, 180, 75], [255, 225, 25], [0, 130, 200], [245, 130, 48], [145, 30, 180],
    [70, 240, 240], [240, 50, 230], [210, 245, 60], [250, 190, 212], [0, 128, 128], [220, 190, 255],
    [170, 110, 40], [255, 250, 200], [128, 0, 0], [170, 255, 195],
];

/// The window's default colour of label `k`.
pub fn palette(k: u16) -> [u8; 3] {
    COLORS[(k.max(1) as usize - 1) % COLORS.len()]
}

/// The 1st and 99th percentile of a slice: the contrast the picker sees it with.
fn range(v: &[f32]) -> (f32, f32) {
    let mut s: Vec<f32> = v.iter().step_by((v.len() / 65536).max(1)).copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return (0.0, 1.0);
    }
    s.sort_by(f32::total_cmp);
    let at = |q: f64| s[((s.len() - 1) as f64 * q) as usize];
    let (lo, hi) = (at(0.01), at(0.99));
    if hi > lo { (lo, hi) } else { (lo, lo + 1.0) }
}

/// Which label a layer paints, making or renaming its material as asked.
fn layer_label(l: &mut Labels, layer: &Layer) -> u16 {
    let label = match (layer.label, layer.name.is_empty()) {
        (Some(k), _) if k > 0 => k,
        (_, false) => {
            let k = l.materials.iter().find(|m| m.name == layer.name).map(|m| m.label);
            k.unwrap_or_else(|| {
                let next = l.max_label().max(l.materials.iter().map(|m| m.label).max().unwrap_or(0)) + 1;
                l.material_label(&layer.name, layer.color.unwrap_or(COLORS[(next as usize - 1) % COLORS.len()]))
            })
        }
        _ => l.max_label().max(l.materials.iter().map(|m| m.label).max().unwrap_or(0)) + 1,
    };
    if !layer.name.is_empty() || layer.color.is_some() {
        let mut m = l.materials.clone();
        let i = match m.iter().position(|m| m.label == label) {
            Some(i) => i,
            None => {
                m.push(crate::labels::Material { label, ..Default::default() });
                m.len() - 1
            }
        };
        if !layer.name.is_empty() {
            m[i].name = layer.name.clone();
        }
        if let Some(c) = layer.color {
            m[i].color = Some(c);
        }
        l.set_materials(m);
    }
    label
}

/// Paints `marks` into `l` as one undo step;
/// returns the slices changed and a summary per layer.
pub fn apply(l: &mut Labels, img: &Arc<OpenImage>, sam: &crate::sam::Sam, token: u64, marks: &Marks) -> Result<(Vec<usize>, Value)> {
    let info = img.info();
    let (w, h, d) = (info.width, info.height, info.size_z);
    if l.w != w || l.h != h || l.d != d {
        bail!("标签图和图像大小不一样");
    }
    let vals = crate::labels::ChannelValues { img: img.clone(), c: 0, t: 0 };
    let depth = l.undo_depth();
    let mut ranges: HashMap<usize, (f32, f32)> = HashMap::new();
    let mut zs = vec![];
    let mut out = vec![];
    for layer in &marks.layers {
        let label = layer_label(l, layer);
        let paint = if layer.erase { 0 } else { label };
        let guard = if layer.erase { label } else { 0 };
        let before = l.counts().get(label as usize).copied().unwrap_or(0);
        let mut problems = vec![];
        // Points: a click of the smart tool each, all of a layer as one step.
        let mut masks: Vec<(usize, Vec<bool>)> = vec![];
        for (i, p) in layer.points.iter().enumerate() {
            let (x, y, z) = (p[0], p[1], p[2].round().max(0.0) as usize);
            if z >= d || !(0.0..w as f32).contains(&x) || !(0.0..h as f32).contains(&y) {
                problems.push(format!("第 {} 个点 {:?} 在图像外", i + 1, p));
                continue;
            }
            let (lo, hi) = match ranges.get(&z) {
                Some(r) => *r,
                None => {
                    let r = range(&vals.slice(z)?);
                    ranges.insert(z, r);
                    r
                }
            };
            let view = match layer.size {
                Some(s) if s > 0.0 => [x - s, y - s, x + s, y + s],
                _ => [0.0, 0.0, w as f32, h as f32],
            };
            let req = crate::sam::PickReq { token, z, c: 0, t: 0, lo, hi, denoise: Default::default(), path: vec![[x, y]], view, choice: None, preview: false };
            match sam.pick(&req, (w, h, d), |z| vals.slice(z)) {
                Ok(pk) => {
                    let [x0, y0, rw, rh] = pk.rect;
                    let mut full = vec![false; w * h];
                    for yy in 0..rh {
                        for xx in 0..rw {
                            if pk.mask[yy * rw + xx] != 0 && x0 + xx < w && y0 + yy < h {
                                full[(y0 + yy) * w + x0 + xx] = true;
                            }
                        }
                    }
                    match masks.iter_mut().find(|(mz, _)| *mz == z) {
                        Some((_, m)) => m.iter_mut().zip(&full).for_each(|(a, b)| *a |= *b),
                        None => masks.push((z, full)),
                    }
                }
                Err(e) => problems.push(format!("第 {} 个点 {:?}：{e:#}", i + 1, p)),
            }
        }
        if !masks.is_empty() {
            let refs: Vec<(usize, &[bool])> = masks.iter().map(|(z, m)| (*z, m.as_slice())).collect();
            zs.extend(l.apply_masks(&refs, paint, guard, false));
        }
        for s in &layer.polygons {
            if s.z >= d || s.pts.len() < 3 {
                problems.push(format!("轮廓（第 {} 层）要在图像里、至少 3 个点", s.z));
                continue;
            }
            zs.extend(l.polygon(s.z, &s.pts, paint, guard));
        }
        for s in &layer.strokes {
            if s.z >= d || s.pts.is_empty() {
                problems.push(format!("线（第 {} 层）要在图像里、至少 1 个点", s.z));
                continue;
            }
            zs.extend(l.stroke(s.z, &s.pts, s.radius.unwrap_or(4.0), paint, guard));
        }
        let after = l.counts().get(label as usize).copied().unwrap_or(0);
        let name = l.materials.iter().find(|m| m.label == label).map(|m| m.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("标签 {label}"));
        out.push(json!({"label": label, "name": name, "pixels": after, "changed": after as i64 - before as i64, "problems": problems}));
    }
    for m in &marks.masks {
        let r = crate::label_import::import(l, std::path::Path::new(m))?;
        zs.extend(r.zs);
        for (label, name) in r.layers {
            let pixels = l.counts().get(label as usize).copied().unwrap_or(0);
            out.push(json!({"label": label, "name": name, "pixels": pixels, "from": m}));
        }
    }
    // One request, one undo step.
    l.squash_since(depth);
    zs.sort_unstable();
    zs.dedup();
    Ok((zs, json!(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_parse_with_defaults() {
        let m: Marks = serde_json::from_str(r#"{"layers":[{"name":"膜","points":[[1,2,3]]},{"label":4,"polygons":[{"z":0,"pts":[[0,0],[5,0],[5,5]]}]}]}"#).unwrap();
        assert_eq!(m.layers.len(), 2);
        assert_eq!(m.layers[0].points[0], [1.0, 2.0, 3.0]);
        assert_eq!(m.layers[1].label, Some(4));
        assert!(m.layers[1].strokes.is_empty());
    }

    #[test]
    fn named_layers_are_made_once_and_kept() {
        let mut l = Labels::new(8, 8, 2);
        let a = layer_label(&mut l, &Layer { name: "膜".into(), ..Default::default() });
        let b = layer_label(&mut l, &Layer { name: "膜".into(), ..Default::default() });
        let c = layer_label(&mut l, &Layer { name: "颗粒".into(), color: Some([1, 2, 3]), ..Default::default() });
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(l.materials.iter().find(|m| m.label == c).unwrap().color, Some([1, 2, 3]));
    }

    #[test]
    fn all_marks_are_one_undo_step() {
        let mut l = Labels::new(16, 16, 2);
        let d = l.undo_depth();
        l.polygon(0, &[[1.0, 1.0], [8.0, 1.0], [8.0, 8.0]], 1, 0);
        l.stroke(1, &[[2.0, 2.0], [10.0, 10.0]], 2.0, 2, 0);
        l.squash_since(d);
        assert_eq!(l.undo_depth(), d + 1);
        l.undo();
        assert!(l.is_empty());
    }

    #[test]
    fn contrast_ignores_outliers() {
        let mut v: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        v[0] = -1e9;
        v[999] = 1e9;
        let (lo, hi) = range(&v);
        assert!(lo >= 0.0 && hi <= 999.0 && hi > lo);
    }
}
