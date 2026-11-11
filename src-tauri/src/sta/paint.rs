//! "Paint the membrane" (`Kind::Membrane` of `marks`): the user scribbles
//! over a membrane and the membrane segmentation next to the file says which
//! voxels that is (`sheet::touched`); `sheet::seeds` spreads the sampling
//! points on it.
//!
//! Everything here is in voxels of the file the strokes were drawn on (the
//! mark's `tomogram`: a tomogram or its look file), the same grid the label
//! map beside that file has. The loaded mask and the last results are kept,
//! so the overlay's previews and section highlights do not read the label
//! file or search the sheet again for every change of section.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::marks::Mark;
use super::shape::Seed;
use super::sheet;
use crate::labels::{labels_path, Labels};
use crate::membrane::{MATERIAL, VESICLE_MATERIAL};

/// A freehand stroke over a membrane, in voxels of the mark's tomogram. A
/// cut stroke does not take a membrane: it ends the sheet where it crosses.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub points: Vec<[f64; 3]>,
    #[serde(default)]
    pub cut: bool,
}

/// The membrane (and vesicle) voxels of a file's label map.
pub struct Mask {
    pub n: [usize; 3],
    /// Voxel size of the file, Å.
    pub px: f64,
    pub mask: Vec<bool>,
}

/// A label file as it was when read: path, modification time, length.
type Key = (String, Option<SystemTime>, u64);

fn key_of(path: &str) -> Option<Key> {
    let m = std::fs::metadata(path).ok()?;
    Some((path.to_string(), m.modified().ok(), m.len()))
}

static MASK: Mutex<Option<(Key, Option<Arc<Mask>>)>> = Mutex::new(None);

fn load_mask(tomogram: &str, label_file: &str) -> Result<Option<Mask>> {
    let img = crate::tomo::pipeline::open(tomogram)?;
    let info = img.info();
    let n = [info.width, info.height, info.size_z];
    let px = crate::tomo::output::pixel_a(&img);
    let labels = Labels::load(std::path::Path::new(label_file), n[0], n[1], n[2])?;
    let wanted: Vec<u16> = labels.materials.iter().filter(|m| m.name == MATERIAL || m.name == VESICLE_MATERIAL).map(|m| m.label).collect();
    if wanted.is_empty() {
        return Ok(None);
    }
    let plane = n[0] * n[1];
    let mut mask = vec![false; plane * n[2]];
    for z in 0..n[2] {
        if let Some(s) = labels.slice(z) {
            for (o, v) in mask[z * plane..(z + 1) * plane].iter_mut().zip(s) {
                *o = wanted.contains(v);
            }
        }
    }
    Ok(Some(Mask { n, px, mask }))
}

/// The membrane voxels of the label map beside `tomogram`; `None` when there
/// is no label map or it has no membrane material (not segmented yet).
pub fn mask_of(tomogram: &str) -> Result<Option<Arc<Mask>>> {
    let file = labels_path(tomogram, 0);
    let Some(key) = key_of(&file) else { return Ok(None) };
    if let Some((k, m)) = MASK.lock().unwrap().as_ref() {
        if *k == key {
            return Ok(m.clone());
        }
    }
    let m = load_mask(tomogram, &file)?.map(Arc::new);
    *MASK.lock().unwrap() = Some((key, m.clone()));
    Ok(m)
}

/// A few results kept, the oldest dropped first.
struct Recent<K, V>(Vec<(K, V)>);

impl<K: PartialEq, V: Clone> Recent<K, V> {
    const MOST: usize = 8;
    fn get(&mut self, k: &K) -> Option<V> {
        let i = self.0.iter().position(|(x, _)| x == k)?;
        let e = self.0.remove(i);
        let v = e.1.clone();
        self.0.push(e);
        Some(v)
    }
    fn put(&mut self, k: K, v: V) {
        if self.0.len() >= Self::MOST {
            self.0.remove(0);
        }
        self.0.push((k, v));
    }
}

type TouchedKey = (Key, Vec<Stroke>);
static TOUCHED: Mutex<Recent<TouchedKey, Arc<Vec<u32>>>> = Mutex::new(Recent(Vec::new()));
type SeedsKey = (Key, Vec<Stroke>, u64);
static SEEDS: Mutex<Recent<SeedsKey, Arc<Vec<Seed>>>> = Mutex::new(Recent(Vec::new()));

/// The mask of the mark's file and the voxels its strokes take (sorted voxel
/// indices); `None` when the file has no segmented membrane.
pub fn touched_of(m: &Mark) -> Result<Option<(Arc<Mask>, Arc<Vec<u32>>)>> {
    let Some(mask) = mask_of(&m.tomogram)? else { return Ok(None) };
    let Some(key) = key_of(&labels_path(&m.tomogram, 0)) else { return Ok(None) };
    let k = (key, m.strokes.clone());
    if let Some(v) = TOUCHED.lock().unwrap().get(&k) {
        return Ok(Some((mask, v)));
    }
    let side = |cut: bool| -> Vec<Vec<[f64; 3]>> { m.strokes.iter().filter(|s| s.cut == cut && !s.points.is_empty()).map(|s| s.points.clone()).collect() };
    let v = Arc::new(sheet::touched(&mask.mask, mask.n, &side(false), &side(true)));
    TOUCHED.lock().unwrap().put(k, v.clone());
    Ok(Some((mask, v)))
}

/// Whether the mark has a stroke that takes a membrane (a cut alone does not).
pub fn has_stroke(m: &Mark) -> bool {
    m.strokes.iter().any(|s| !s.cut && !s.points.is_empty())
}

/// The seeds of a membrane mark for particles `size_a` Å across, in voxels
/// of the mark's own file (spaced half a particle apart), and how many
/// voxels the strokes took. No seeds and 0 voxels when there is no
/// segmented membrane or the strokes touch none. Normals face away from
/// the middle of what was taken; `Mark::flip` is not applied here.
pub fn seeds_of(m: &Mark, size_a: f64) -> Result<(usize, Arc<Vec<Seed>>)> {
    let Some((mask, voxels)) = touched_of(m)? else { return Ok((0, Arc::new(vec![]))) };
    if voxels.is_empty() {
        return Ok((0, Arc::new(vec![])));
    }
    let spacing = 0.5 * size_a / mask.px;
    let Some(key) = key_of(&labels_path(&m.tomogram, 0)) else { return Ok((0, Arc::new(vec![]))) };
    let k = (key, m.strokes.clone(), spacing.to_bits());
    if let Some(s) = SEEDS.lock().unwrap().get(&k) {
        return Ok((voxels.len(), s));
    }
    let s = Arc::new(sheet::seeds(&voxels, mask.n, spacing));
    SEEDS.lock().unwrap().put(k, s.clone());
    Ok((voxels.len(), s))
}

/// The error a run gives when a membrane mark has no seeds.
pub fn nothing_touched() -> anyhow::Error {
    anyhow::anyhow!("这一笔没有碰到分割出的膜：请先在这张断层图上自动找膜，并把笔画在膜上")
}

/// The runs `[y, x0, x1)` (x1 excluded) of the voxels the membrane mark
/// takes on section `z` of its file, for the highlight. Empty for other
/// kinds, for a mark of another file than `tomogram`, and when nothing is
/// taken.
pub fn section_runs(m: &Mark, tomogram: &str, z: usize) -> Result<Vec<[usize; 3]>> {
    if m.kind != super::marks::Kind::Membrane || m.tomogram != tomogram {
        return Ok(vec![]);
    }
    let Some((mask, voxels)) = touched_of(m)? else { return Ok(vec![]) };
    let [w, h, d] = mask.n;
    if z >= d {
        return Ok(vec![]);
    }
    let plane = w * h;
    let lo = voxels.partition_point(|&i| (i as usize) < z * plane);
    let hi = voxels.partition_point(|&i| (i as usize) < (z + 1) * plane);
    let mut out: Vec<[usize; 3]> = vec![];
    for &i in &voxels[lo..hi] {
        let r = i as usize - z * plane;
        let (y, x) = (r / w, r % w);
        match out.last_mut() {
            Some(l) if l[0] == y && l[2] == x => l[2] = x + 1,
            _ => out.push([y, x, x + 1]),
        }
    }
    Ok(out)
}

/// {ready, model}: whether the label map beside `tomogram` has the membrane
/// material (the automatic segmentation was run), and whether the
/// segmentation model is set up.
pub fn status(tomogram: &str) -> Result<Value> {
    Ok(json!({ "ready": mask_of(tomogram)?.is_some(), "model": crate::membrane::model_path().is_some() }))
}

/// Segments the membranes of `tomogram` into the label map beside it.
/// `progress` gets (stage, done, total). Returns {ready, voxels, components,
/// vesicles, seconds}.
pub fn segment(tomogram: &str, progress: &(dyn Fn(&str, usize, usize) + Sync)) -> Result<Value> {
    let Some(model) = crate::membrane::model_path() else {
        bail!("还没有膜分割模型：请先在标签面板的「自动分割膜」里载入一次权重");
    };
    let (r, _) = crate::classify::on_device(|dev| crate::membrane::segment_file(dev, &model, tomogram, progress))?;
    Ok(json!({ "ready": mask_of(tomogram)?.is_some(), "voxels": r.voxels, "components": r.components, "vesicles": r.vesicles.len(), "seconds": r.seconds }))
}
