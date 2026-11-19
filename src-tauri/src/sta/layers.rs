//! What to average comes from the label layers the user made with the one
//! label brush of the viewer (`labels`, `structure`), not from tools of this
//! workspace: on a tomogram (usually its clear "look" companion) the user
//! paints a layer per thing, gives it a structure type (小颗粒, 球, 空心球,
//! 单层膜, 双层膜, 纤维, 不规则), refines it, finds the alike with 找同类 and
//! carries it along Z. The workspace lists the layers of every tomogram, the
//! user ticks some (only the membranes of ten tomograms, say), and `marks_of`
//! turns each ticked layer into the marks of this workspace (`marks::Mark`),
//! which are averaged on the raw tomogram as marks always are (`marks::to_source`
//! carries voxels of the look file over).
//!
//! Each structure type becomes the mark that means the same:
//! - 小颗粒 / 球 / 不规则 / rough paint: each object (a painted region,
//!   joined across neighbouring sections where it overlaps) is an example,
//!   the tight circle round it, as a painted example of this workspace was:
//!   the alike are found in every ticked tomogram and shown before averaging,
//!   or, when the user marked them all already, just these are averaged.
//! - 空心球: a sphere (its centre and a point at its radius): particles on
//!   the vesicle's surface, normals outward.
//! - 单层膜 / 双层膜: the refined membrane lines (`labels::Mark`, one per
//!   section) as hand-drawn surfaces, one per line. The side the user named
//!   as inside makes the normals point outward (`flip`); a line without a
//!   side keeps the surface's own first normal.
//! - 纤维: the refined lines as filaments.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use super::marks::{Kind, Mark};
use crate::labels::{components_where, labels_path, Material, MATERIALS_KEY};

/// One layer of a tomogram's label file, for the list to tick.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    /// The image the labels belong to (the look file or the tomogram).
    pub image: String,
    pub label: u16,
    pub name: String,
    pub structure: String,
    pub color: Option<[u8; 3]>,
    /// Painted objects (regions joined across sections).
    pub objects: usize,
    /// Sections with paint of this layer.
    pub sections: usize,
    /// Refined lines (membranes, fibres).
    pub lines: usize,
}

/// One painted object of a layer: pixels joined on a section and across
/// neighbouring sections where they overlap.
#[derive(Clone, Debug, Default)]
struct Object {
    label: u16,
    voxels: f64,
    sum: [f64; 3],
    /// Largest half-extent on any one section (pixels).
    half: f64,
    z: [usize; 2],
}

impl Object {
    fn centre(&self) -> [f64; 3] {
        self.sum.map(|s| s / self.voxels.max(1.0))
    }
}

/// The label file of `image`, when there is one.
pub fn file_of(image: &str) -> Option<String> {
    let p = labels_path(image, 0);
    Path::new(&p).is_file().then_some(p)
}

/// Reads a label file section by section (never the whole stack at once): its
/// materials, and every section that has paint handed to `each` (z, values, w, h).
fn scan(path: &str, mut each: impl FnMut(usize, &[u16], usize, usize)) -> Result<Vec<Material>> {
    use tiff::decoder::{Decoder, DecodingResult};
    let file = std::io::BufReader::new(std::fs::File::open(path).with_context(|| format!("打不开 {path}"))?);
    let mut dec = Decoder::new(file).context("标签文件不是 TIFF")?;
    let materials: Vec<Material> = dec
        .get_tag_ascii_string(tiff::tags::Tag::ImageDescription)
        .ok()
        .and_then(|d| d.lines().find_map(|l| l.strip_prefix(MATERIALS_KEY)).and_then(|j| serde_json::from_str(j).ok()))
        .unwrap_or_default();
    let mut z = 0;
    loop {
        let (w, h) = dec.dimensions()?;
        let s: Vec<u16> = match dec.read_image()? {
            DecodingResult::U16(v) => v,
            DecodingResult::U8(v) => v.into_iter().map(u16::from).collect(),
            _ => bail!("标签文件必须是 8 位或 16 位整数"),
        };
        if s.iter().any(|&v| v != 0) {
            each(z, &s, w as usize, h as usize);
        }
        if !dec.more_images() {
            break;
        }
        dec.next_image()?;
        z += 1;
    }
    Ok(materials)
}

/// The objects of every label of a label file (`want`: only these labels).
fn objects(path: &str, want: Option<&[u16]>) -> Result<(Vec<Material>, Vec<Object>, Vec<(u16, usize)>)> {
    // Parts of the previous section: (label, pixel indices, object id).
    let mut prev: Vec<(u16, Vec<usize>, usize)> = vec![];
    let mut prev_z = usize::MAX;
    let mut parent: Vec<usize> = vec![];
    let mut objs: Vec<Object> = vec![];
    // Sections per label.
    let mut sections: Vec<(u16, usize)> = vec![];
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let materials = scan(path, |z, s, w, h| {
        let mut labels: Vec<u16> = s.iter().copied().filter(|&v| v != 0 && want.is_none_or(|l| l.contains(&v))).collect();
        labels.sort_unstable();
        labels.dedup();
        let mut here: Vec<(u16, Vec<usize>, usize)> = vec![];
        // Which part of the previous section covers each pixel.
        let mut below: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        if prev_z.wrapping_add(1) == z {
            for (k, (_, pix, _)) in prev.iter().enumerate() {
                for &i in pix {
                    below.insert(i, k);
                }
            }
        }
        for &l in &labels {
            match sections.iter_mut().find(|x| x.0 == l) {
                Some(x) => x.1 += 1,
                None => sections.push((l, 1)),
            }
            for pix in components_where(w, h, |i| s[i] == l) {
                if pix.len() < 4 {
                    continue;
                }
                let id = objs.len();
                parent.push(id);
                let (mut sx, mut sy) = (0f64, 0f64);
                let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0, usize::MAX, 0);
                for &i in &pix {
                    let (x, y) = (i % w, i / w);
                    sx += x as f64;
                    sy += y as f64;
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
                let n = pix.len() as f64;
                objs.push(Object { label: l, voxels: n, sum: [sx, sy, z as f64 * n], half: ((x1 - x0).max(y1 - y0) + 1) as f64 / 2.0, z: [z, z] });
                // Joined to the parts of the same label it overlaps on the section below.
                let mut joined: Vec<usize> = pix.iter().filter_map(|i| below.get(i)).filter(|&&k| prev[k].0 == l).map(|&k| prev[k].2).collect();
                joined.sort_unstable();
                joined.dedup();
                for o in joined {
                    let (a, b) = (root(&mut parent, o), root(&mut parent, id));
                    if a != b {
                        parent[b] = a;
                    }
                }
                here.push((l, pix, id));
            }
        }
        prev = here;
        prev_z = z;
    })?;
    // Merge each object into its root.
    let mut merged: Vec<Option<Object>> = vec![None; objs.len()];
    for i in 0..objs.len() {
        let r = root(&mut parent, i);
        let o = &objs[i];
        match &mut merged[r] {
            Some(m) => {
                m.voxels += o.voxels;
                for k in 0..3 {
                    m.sum[k] += o.sum[k];
                }
                m.half = m.half.max(o.half);
                m.z = [m.z[0].min(o.z[0]), m.z[1].max(o.z[1])];
            }
            slot => *slot = Some(o.clone()),
        }
    }
    Ok((materials, merged.into_iter().flatten().collect(), sections))
}

/// One painted object of a label file, for other workspaces (single
/// particles take the objects of micrographs as examples): its label, its
/// centre (x, y, section) and its largest half-extent on a section (pixels).
#[derive(Clone, Copy, Debug)]
pub struct Painted {
    pub label: u16,
    pub centre: [f64; 3],
    pub half: f64,
}

/// The painted objects of the labels `want` of `image`'s label file.
pub fn painted(image: &str, want: &[u16]) -> Result<Vec<Painted>> {
    let path = file_of(image).with_context(|| format!("{} 没有标签图层", name(image)))?;
    let (_, objs, _) = objects(&path, Some(want))?;
    Ok(objs.iter().map(|o| Painted { label: o.label, centre: o.centre(), half: o.half }).collect())
}

/// The layers of the label file of each of `images` (images without one are
/// left out), with how much each holds.
pub fn layers_of(images: &[String]) -> Vec<Layer> {
    let mut out = vec![];
    for image in images {
        let Some(path) = file_of(image) else { continue };
        let Ok((materials, objs, sections)) = objects(&path, None) else { continue };
        let mut labels: Vec<u16> = sections.iter().map(|s| s.0).collect();
        // Layers with refined lines but no paint left still count.
        labels.extend(materials.iter().filter(|m| !m.marks.is_empty()).map(|m| m.label));
        labels.sort_unstable();
        labels.dedup();
        for l in labels {
            let m = materials.iter().find(|m| m.label == l);
            out.push(Layer {
                image: image.clone(),
                label: l,
                name: m.map(|m| m.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("标签 {l}")),
                structure: m.map(|m| m.structure.clone()).unwrap_or_default(),
                color: m.and_then(|m| m.color),
                objects: objs.iter().filter(|o| o.label == l).count(),
                sections: sections.iter().find(|s| s.0 == l).map_or(0, |s| s.1),
                lines: m.map_or(0, |m| m.marks.iter().filter(|k| k.path.len() >= 2).count()),
            });
        }
    }
    out
}

/// Points of a refined line, about `step` pixels apart (the last kept).
fn thinned(path: &[[f32; 2]], step: f64) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = vec![];
    for p in path {
        let p = [p[0] as f64, p[1] as f64];
        if out.last().is_none_or(|q| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt() >= step) {
            out.push(p);
        }
    }
    if let (Some(l), Some(p)) = (out.last().copied(), path.last()) {
        let p = [p[0] as f64, p[1] as f64];
        if l != p {
            out.push(p);
        }
    }
    out
}

/// The marks of the ticked layers: (image, label) pairs. Errors name a layer
/// that gives nothing to average (a membrane never refined, say).
pub fn marks_of(ticked: &[(String, u16)]) -> Result<Vec<Mark>> {
    let mut out = vec![];
    let mut images: Vec<&str> = ticked.iter().map(|t| t.0.as_str()).collect();
    images.dedup();
    for image in images {
        let want: Vec<u16> = ticked.iter().filter(|t| t.0 == image).map(|t| t.1).collect();
        let path = file_of(image).with_context(|| format!("{} 没有标签图层", name(image)))?;
        let (materials, objs, _) = objects(&path, Some(&want))?;
        for &l in &want {
            let m = materials.iter().find(|m| m.label == l).cloned().unwrap_or_default();
            let layer = if m.name.is_empty() { format!("标签 {l}") } else { m.name.clone() };
            let mine: Vec<&Object> = objs.iter().filter(|o| o.label == l).collect();
            let before = out.len();
            let mark = |kind: Kind, points: Vec<[f64; 3]>, flip: bool| Mark { kind, tomogram: image.to_string(), points, flip, strokes: vec![], deep: false };
            match m.structure.as_str() {
                "shell" => out.extend(mine.iter().map(|o| {
                    let c = o.centre();
                    mark(Kind::Sphere, vec![c, [c[0] + o.half, c[1], c[2]]], false)
                })),
                "membrane" | "membrane2" | "fibre" => {
                    let kind = if m.structure == "fibre" { Kind::Line } else { Kind::Surface };
                    for k in m.marks.iter().filter(|k| k.path.len() >= 2) {
                        let pts: Vec<[f64; 3]> = thinned(&k.path, 4.0).into_iter().map(|p| [p[0], p[1], k.z as f64]).collect();
                        if pts.len() >= 2 {
                            // A one-section surface's normal is (ty, −tx) for the direction t of the line,
                            // which is the outward side when +1 (left) is the inside; −1 flips it.
                            out.push(mark(kind, pts, k.inside < 0));
                        }
                    }
                }
                _ => out.extend(mine.iter().map(|o| {
                    let c = o.centre();
                    mark(Kind::Example, vec![c, [c[0] + o.half.max(2.0), c[1], c[2]]], false)
                })),
            }
            if out.len() == before {
                match m.structure.as_str() {
                    "membrane" | "membrane2" | "fibre" => bail!("图层「{layer}」还没有精修出线：在标签画笔里选中这层，点一下它的结构类型"),
                    _ => bail!("图层「{layer}」上没有涂到东西"),
                }
            }
        }
    }
    Ok(out)
}

fn name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::Labels;

    fn write(dir: &Path, materials: Vec<Material>, paint: impl Fn(usize, usize, usize) -> u16) -> String {
        let image = dir.join("t.mrc").to_string_lossy().to_string();
        let (w, h, d) = (40, 30, 6);
        let mut l = Labels::new(w, h, d);
        for z in 0..d {
            for label in 1..=2u16 {
                let mask: Vec<bool> = (0..w * h).map(|i| paint(i % w, i / w, z) == label).collect();
                if mask.iter().any(|&b| b) {
                    l.apply_mask(z, &mask, label, 0, false);
                }
            }
        }
        l.set_materials(materials);
        l.save(Path::new(&labels_path(&image, 0))).unwrap();
        image
    }

    #[test]
    fn objects_join_across_sections_and_become_marks() {
        let dir = tempfile::tempdir().unwrap();
        let particle = Material { label: 1, name: "核糖体".into(), structure: "particle".into(), ..Default::default() };
        let membrane = Material {
            label: 2,
            name: "膜".into(),
            structure: "membrane".into(),
            marks: vec![crate::labels::Mark { z: 3, path: vec![[0.0, 20.0], [10.0, 20.0], [20.0, 20.0]], thickness: 3.0, inside: -1, closed: false }],
            ..Default::default()
        };
        // Two blobs of label 1: one on sections 1–3 (joined), one on section 4 only.
        let image = write(dir.path(), vec![particle, membrane], |x, y, z| {
            if (5..9).contains(&x) && (5..9).contains(&y) && (1..4).contains(&z) {
                1
            } else if (25..30).contains(&x) && (5..10).contains(&y) && z == 4 {
                1
            } else if y == 20 && z == 3 {
                2
            } else {
                0
            }
        });
        let layers = layers_of(&[image.clone()]);
        assert_eq!(layers.len(), 2);
        assert_eq!((layers[0].objects, layers[0].sections), (2, 4));
        assert_eq!(layers[1].lines, 1);

        let marks = marks_of(&[(image.clone(), 1)]).unwrap();
        assert_eq!(marks.len(), 2);
        assert!(marks.iter().all(|m| m.kind == Kind::Example && m.tomogram == image));
        let m = marks.iter().find(|m| m.points[0][0] < 10.0).unwrap();
        let c = m.points[0];
        assert!((c[0] - 6.5).abs() < 1e-9 && (c[2] - 2.0).abs() < 1e-9, "{c:?}");
        assert!((m.points[1][0] - c[0] - 2.0).abs() < 1e-9, "radius: half of the 4-pixel side");

        let marks = marks_of(&[(image.clone(), 2)]).unwrap();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].kind, Kind::Surface);
        assert!(marks[0].flip);
        assert!(marks[0].points.iter().all(|p| p[2] == 3.0));
    }

    #[test]
    fn an_unrefined_membrane_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let membrane = Material { label: 1, name: "外膜".into(), structure: "membrane".into(), ..Default::default() };
        let image = write(dir.path(), vec![membrane], |_, y, z| u16::from(y == 3 && z == 0));
        let e = marks_of(&[(image, 1)]).unwrap_err().to_string();
        assert!(e.contains("外膜") && e.contains("精修"), "{e}");
    }
}
