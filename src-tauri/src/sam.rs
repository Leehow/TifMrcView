#![cfg_attr(not(feature = "pro"), allow(dead_code))]
//! Pro: SAM 2.1 click segmentation. The model file is downloaded once into
//! the app's data folder; the model is loaded on first use and the features
//! of the last segmented slice are kept, so further clicks only run the
//! small mask decoder. A mask can be carried along Z: each next slice is
//! prompted with the previous mask's box and its innermost point.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(feature = "pro")]
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const MODEL_FILE: &str = "sam2.1_hiera_tiny.pt";
pub const MODEL_SIZE: u64 = 156_008_466;
pub const MODEL_SHA256: &str = "7402e0d864fa82708a20fbd15bc84245c2f26dff0eb43a4b5b93452deb34be69";
/// Tried in order; the mirror helps where huggingface.co is blocked.
pub const MODEL_URLS: [&str; 2] = [
    "https://huggingface.co/facebook/sam2.1-hiera-tiny/resolve/main/sam2.1_hiera_tiny.pt",
    "https://hf-mirror.com/facebook/sam2.1-hiera-tiny/resolve/main/sam2.1_hiera_tiny.pt",
];

pub fn model_path() -> Option<PathBuf> {
    Some(crate::plugins::paths::user_dir()?.parent()?.join("models").join(MODEL_FILE))
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct PromptPoint {
    pub x: f32,
    pub y: f32,
    pub positive: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PredictReq {
    pub token: u64,
    pub z: usize,
    pub c: usize,
    pub t: usize,
    /// Display range the slice is mapped to 0..255 with.
    pub lo: f32,
    pub hi: f32,
    pub points: Vec<PromptPoint>,
    pub bbox: Option<[f32; 4]>,
    #[serde(default)]
    pub denoise: Denoise,
}

/// Denoising before encoding: average `slab` slices on each side, then a
/// Gaussian blur of `sigma` pixels. On cryo-ET slices this makes SAM follow
/// whole membranes instead of short straight pieces.
#[derive(Deserialize, Default, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct Denoise {
    pub slab: usize,
    pub sigma: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub available: bool,
    pub downloaded: bool,
    pub loaded: bool,
    pub backend: Option<String>,
    pub size: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropagateReq {
    pub token: u64,
    pub z: usize,
    pub c: usize,
    pub t: usize,
    pub lo: f32,
    pub hi: f32,
    pub up: bool,
    pub down: bool,
    /// Most slices to go in each direction.
    pub max: usize,
    #[serde(default)]
    pub denoise: Denoise,
}

/// The mask of the last prediction, kept for `sam_apply`.
pub struct LastMask {
    pub token: u64,
    pub z: usize,
    pub mask: Vec<bool>,
    /// The model's own quality estimate for it.
    pub iou: f32,
    /// The model's low-resolution logits, the mask prompt for the next slice.
    pub low_res: Vec<f32>,
}

#[derive(Default)]
pub struct Sam {
    pub downloading: AtomicBool,
    /// Set to stop a running propagation.
    pub cancel: AtomicBool,
    inner: Mutex<Inner>,
    pub last: Mutex<Option<LastMask>>,
}

#[derive(Default)]
struct Inner {
    #[cfg(feature = "pro")]
    predictor: Option<Arc<tmv_sam::Predictor>>,
    backend: Option<String>,
    /// (token, z, c, t, lo, hi, slab, sigma) of `encoded`.
    #[cfg(feature = "pro")]
    key: Option<(u64, usize, usize, usize, u32, u32, usize, u32)>,
    #[cfg(feature = "pro")]
    encoded: Option<Arc<tmv_sam::Encoded>>,
}

impl Sam {
    /// A Sam with an already loaded model (developer tools, tests).
    #[cfg(feature = "pro")]
    pub fn with_predictor(p: tmv_sam::Predictor) -> Self {
        let sam = Sam::default();
        sam.inner.lock().unwrap().predictor = Some(Arc::new(p));
        sam
    }

    pub fn status(&self) -> Status {
        let inner = self.inner.lock().unwrap();
        #[cfg(feature = "pro")]
        let loaded = inner.predictor.is_some();
        #[cfg(not(feature = "pro"))]
        let loaded = false;
        Status {
            available: cfg!(feature = "pro"),
            downloaded: model_path().is_some_and(|p| p.metadata().is_ok_and(|m| m.len() == MODEL_SIZE)),
            loaded,
            backend: inner.backend.clone(),
            size: MODEL_SIZE,
        }
    }

    #[cfg(feature = "pro")]
    fn predictor(inner: &mut Inner) -> Result<Arc<tmv_sam::Predictor>> {
        if inner.predictor.is_none() {
            let Some(path) = model_path().filter(|p| p.exists()) else { bail!("还没有下载 SAM 模型") };
            #[cfg(target_os = "linux")]
            use_bundled_libdevice();
            let t = std::time::Instant::now();
            let (p, name) = tmv_sam::load_best(&path, &tmv_sam::Sam2Config::tiny(), std::time::Duration::from_secs(90))?;
            eprintln!("SAM: model ready on {name} in {:.1?}", t.elapsed());
            inner.predictor = Some(Arc::new(p));
            inner.backend = Some(name.to_string());
        }
        Ok(inner.predictor.clone().unwrap())
    }

    /// Mask of the prompt on slice `req.z` as one byte per pixel (1 = inside),
    /// plus the predicted IoU. `values` reads a slice's raw values.
    #[cfg(feature = "pro")]
    pub fn predict(&self, req: &PredictReq, (w, h, d): (usize, usize, usize), values: impl Fn(usize) -> Result<Vec<f32>>) -> Result<(Vec<u8>, f32)> {
        let dn = req.denoise;
        let key = (req.token, req.z, req.c, req.t, req.lo.to_bits(), req.hi.to_bits(), dn.slab, dn.sigma.to_bits());
        let (predictor, encoded) = {
            let mut inner = self.inner.lock().unwrap();
            let p = Self::predictor(&mut inner)?;
            if inner.key != Some(key) {
                let t = std::time::Instant::now();
                let gray = slice_gray(&values, req.z, (w, h, d), (req.lo, req.hi), dn)?;
                inner.encoded = Some(Arc::new(p.encode_gray(&gray, w, h)));
                eprintln!("SAM: encoded slice {} ({w}x{h}) in {:.1?}", req.z, t.elapsed());
                inner.key = Some(key);
            }
            (p, inner.encoded.clone().unwrap())
        };
        let points: Vec<tmv_sam::Point> = req.points.iter().map(|p| tmv_sam::Point { x: p.x, y: p.y, positive: p.positive }).collect();
        if points.is_empty() && req.bbox.is_none() {
            bail!("请先点一下要分割的物体");
        }
        let r = predictor.predict(&encoded, &points, req.bbox, None);
        let mask: Vec<bool> = r.logits.iter().map(|&v| v > 0.0).collect();
        let bytes = mask.iter().map(|&m| m as u8).collect();
        *self.last.lock().unwrap() = Some(LastMask { token: req.token, z: req.z, mask, iou: r.iou, low_res: r.low_res_values() });
        Ok((bytes, r.iou))
    }

    /// Carries the last mask (on `req.z`) up and/or down the stack, one slice
    /// at a time, until the object is lost or `cancel` is set. Returns the
    /// masks by slice, the starting one first.
    #[cfg(feature = "pro")]
    pub fn propagate(
        &self,
        req: &PropagateReq,
        (w, h, d): (usize, usize, usize),
        values: impl Fn(usize) -> Result<Vec<f32>>,
        progress: impl Fn(usize, usize),
    ) -> Result<Vec<(usize, Vec<bool>)>> {
        use std::sync::atomic::Ordering::SeqCst;
        let (start, start_iou, start_low) = match self.last.lock().unwrap().as_ref() {
            Some(m) if m.token == req.token && m.z == req.z => (m.mask.clone(), m.iou, m.low_res.clone()),
            _ => bail!("先在这一层点出一个分割，再沿 Z 传播"),
        };
        if too_big(&start) {
            bail!("这个分割几乎盖满了整层，先用 Alt+点击去掉背景再传播");
        }
        let predictor = Self::predictor(&mut self.inner.lock().unwrap())?;
        // A mask that has grown to three times the starting one has leaked
        // into its surroundings a little at a time. (Shrinking is normal
        // towards the ends of an object.)
        let start_area = start.iter().filter(|m| **m).count() as f32;
        self.cancel.store(false, SeqCst);
        let mut dirs = vec![];
        if req.up {
            dirs.push((-1isize, req.z.min(req.max)));
        }
        if req.down {
            dirs.push((1, (d - 1 - req.z).min(req.max)));
        }
        let total: usize = dirs.iter().map(|d| d.1).sum();
        let (mut out, mut done) = (vec![(req.z, start.clone())], 0);
        for (dir, steps) in dirs {
            let (mut prev, mut prev_low, base) = (start.clone(), start_low.clone(), done);
            for k in 1..=steps {
                if self.cancel.load(SeqCst) {
                    return Ok(out);
                }
                let z = (req.z as isize + dir * k as isize) as usize;
                let Some(prompt) = follow_prompt(&prev, w, h) else { break };
                let enc = predictor.encode_gray(&slice_gray(&values, z, (w, h, d), (req.lo, req.hi), req.denoise)?, w, h);
                let pts: Vec<tmv_sam::Point> = prompt.points.iter().map(|p| tmv_sam::Point { x: p.x, y: p.y, positive: true }).collect();
                // Encoding is the slow part; the decoder is cheap, so each
                // way of prompting is tried and the one that keeps closest
                // to the previous slice wins.
                let mut best: Option<(f32, Vec<bool>, Vec<f32>)> = None;
                let mut why = vec![];
                for (bbox, use_mask) in prompt.variants() {
                    let r = predictor.predict(&enc, &pts, bbox, use_mask.then(|| predictor.mask_prompt(&prev_low)));
                    let mask: Vec<bool> = r.logits.iter().map(|&v| v > 0.0).collect();
                    let grown = mask.iter().filter(|m| **m).count() as f32 / start_area;
                    if grown > 3.0 || !still_same(&prev, &mask, w, h, r.iou, min_iou(start_iou)) {
                        why.push(format!("box {} mask {use_mask}: {}, grown {grown:.2}", bbox.is_some(), compare(&prev, &mask, w, h, r.iou)));
                        continue;
                    }
                    let score = likeness(&prev, &mask, w, h);
                    if best.as_ref().is_none_or(|b| score > b.0) {
                        best = Some((score, mask, r.low_res_values()));
                    }
                }
                let Some((_, mask, low)) = best else {
                    eprintln!("SAM: propagation stops at slice {z} (needs quality {:.2}): {}", min_iou(start_iou), why.join("; "));
                    break;
                };
                done += 1;
                progress(done, total);
                out.push((z, mask.clone()));
                prev = mask;
                prev_low = low;
            }
            // The slices left in this direction are skipped.
            done = base + steps;
            progress(done, total);
        }
        Ok(out)
    }

    #[cfg(not(feature = "pro"))]
    pub fn predict(&self, _: &PredictReq, _: (usize, usize, usize), _: impl Fn(usize) -> Result<Vec<f32>>) -> Result<(Vec<u8>, f32)> {
        bail!("SAM 分割只在 TifMrcView Pro 里有")
    }

    #[cfg(not(feature = "pro"))]
    pub fn propagate(
        &self,
        _: &PropagateReq,
        _: (usize, usize, usize),
        _: impl Fn(usize) -> Result<Vec<f32>>,
        _: impl Fn(usize, usize),
    ) -> Result<Vec<(usize, Vec<bool>)>> {
        bail!("SAM 分割只在 TifMrcView Pro 里有")
    }
}

/// The 8-bit image SAM encodes for slice `z`, denoised as asked.
fn slice_gray(values: &impl Fn(usize) -> Result<Vec<f32>>, z: usize, (w, h, d): (usize, usize, usize), (lo, hi): (f32, f32), dn: Denoise) -> Result<Vec<u8>> {
    if dn.slab == 0 && dn.sigma <= 0.0 {
        return Ok(to_gray(&values(z)?, lo, hi));
    }
    let slices = (z.saturating_sub(dn.slab)..=(z + dn.slab).min(d.max(1) - 1)).map(values).collect::<Result<Vec<_>>>()?;
    Ok(to_gray(&denoise(&slices, w, h, dn.sigma), lo, hi))
}

/// Raw values to 0..255 over the display range.
pub fn to_gray(v: &[f32], lo: f32, hi: f32) -> Vec<u8> {
    let span = (hi - lo).abs().max(f32::MIN_POSITIVE);
    let lo = lo.min(hi);
    v.iter().map(|&x| ((x - lo) / span * 255.0).clamp(0.0, 255.0) as u8).collect()
}

/// Denoising for noisy slices (cryo-ET): the mean of the given neighbouring
/// slices, then a 2D Gaussian blur of `sigma` pixels (0 = none).
pub fn denoise(slices: &[Vec<f32>], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let n = slices.len().max(1) as f32;
    let mut a = vec![0f32; w * h];
    for s in slices {
        for (o, v) in a.iter_mut().zip(s) {
            *o += v / n;
        }
    }
    if sigma <= 0.0 {
        return a;
    }
    let r = (sigma * 3.0).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    let mut b = vec![0f32; w * h];
    // Rows, then columns: (samples per line, stride along it, lines, line step).
    for (len, stride, lines, step) in [(w, 1, h, w), (h, w, w, 1)] {
        for line in 0..lines {
            let base = line * step;
            for i in 0..len {
                let mut acc = 0.0;
                for (j, kv) in k.iter().enumerate() {
                    let p = (i as isize + j as isize - r).clamp(0, len as isize - 1) as usize;
                    acc += kv * a[base + p * stride];
                }
                b[base + i * stride] = acc / sum;
            }
        }
        std::mem::swap(&mut a, &mut b);
    }
    a
}

/// What carries a mask on to the next slice.
pub struct Prompt {
    pub bbox: Option<[f32; 4]>,
    pub points: Vec<PromptPoint>,
    /// Give the previous slice's mask as a mask prompt too.
    pub use_mask: bool,
}

impl Prompt {
    /// The (box, mask prompt) pairs worth trying: the preferred one first.
    /// A thin outline that is nearly closed looks open, and its box then
    /// asks for the whole cell, so thin objects also go without a box.
    /// Blobs keep to their box: the previous mask would hold back the
    /// shrinking towards their ends.
    pub fn variants(&self) -> Vec<(Option<[f32; 4]>, bool)> {
        let alt = if self.points.len() > 1 { vec![(None, true), (None, false), (self.bbox, true)] } else { vec![] };
        let mut v = vec![(self.bbox, self.use_mask)];
        for a in alt {
            if !v.iter().any(|b: &(Option<[f32; 4]>, bool)| (b.0.is_some(), b.1) == (a.0.is_some(), a.1)) {
                v.push(a);
            }
        }
        v
    }
}

/// The prompt for the next slice from the mask on this one: its box grown
/// by 10 % and the innermost pixel. A thin object (a membrane, a filament)
/// gets two more points spread along it. A thin object that closes on
/// itself (a cell outline) gets no box, which would ask for the whole cell,
/// but five points along it plus the previous mask as a mask prompt.
pub fn follow_prompt(mask: &[bool], w: usize, h: usize) -> Option<Prompt> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, _) in mask.iter().enumerate().filter(|(_, m)| **m) {
        let (x, y) = (i % w, i / w);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    if x0 == usize::MAX {
        return None;
    }
    let outside: Vec<bool> = mask.iter().map(|m| !m).collect();
    let depth = crate::labels::edt(&outside, w, h);
    let (i, &deepest) = depth.iter().enumerate().filter(|(i, _)| mask[*i]).max_by(|a, b| a.1.total_cmp(b.1))?;
    let point = |i: usize| PromptPoint { x: (i % w) as f32 + 0.5, y: (i / w) as f32 + 0.5, positive: true };
    let (gx, gy) = ((x1 - x0 + 1) as f32 * 0.1, (y1 - y0 + 1) as f32 * 0.1);
    let bbox = [
        (x0 as f32 - gx).max(0.0),
        (y0 as f32 - gy).max(0.0),
        ((x1 + 1) as f32 + gx).min(w as f32),
        ((y1 + 1) as f32 + gy).min(h as f32),
    ];
    let span = (x1 - x0 + 1).max(y1 - y0 + 1) as f32;
    if span <= 8.0 * deepest {
        return Some(Prompt { bbox: Some(bbox), points: vec![point(i)], use_mask: false });
    }
    let closed = enclosed(mask, w, h) > 2 * mask.iter().filter(|m| **m).count();
    // Farthest-point picks among the pixels on the middle line.
    let mid: Vec<usize> = (0..mask.len()).filter(|&j| mask[j] && depth[j] >= 0.5 * deepest).collect();
    let mut pts = vec![i];
    for _ in 0..if closed { 4 } else { 2 } {
        let far = mid.iter().copied().max_by_key(|&j| {
            pts.iter().map(|&p| (j % w).abs_diff(p % w).pow(2) + (j / w).abs_diff(p / w).pow(2)).min().unwrap_or(0)
        });
        if let Some(f) = far.filter(|f| !pts.contains(f)) {
            pts.push(f);
        }
    }
    let points = pts.into_iter().map(point).collect();
    Some(if closed { Prompt { bbox: None, points, use_mask: true } } else { Prompt { bbox: Some(bbox), points, use_mask: false } })
}

/// Pixels outside `mask` that the image border cannot reach: the holes a
/// closed outline encloses.
fn enclosed(mask: &[bool], w: usize, h: usize) -> usize {
    let mut seen = mask.to_vec();
    let mut stack: Vec<usize> = (0..w).flat_map(|x| [x, (h - 1) * w + x]).chain((0..h).flat_map(|y| [y * w, y * w + w - 1])).collect();
    while let Some(i) = stack.pop() {
        if seen[i] {
            continue;
        }
        seen[i] = true;
        let (x, y) = (i % w, i / w);
        if x > 0 { stack.push(i - 1); }
        if x + 1 < w { stack.push(i + 1); }
        if y > 0 { stack.push(i - w); }
        if y + 1 < h { stack.push(i + w); }
    }
    seen.iter().filter(|s| !**s).count()
}

/// Points the CUDA backend at the libdevice.10.bc that the Linux packages
/// carry (/usr/lib/tifmrcview-pro/cuda, also inside the AppImage), unless
/// CUDA_PATH already names a toolkit.
#[cfg(all(feature = "pro", target_os = "linux"))]
fn use_bundled_libdevice() {
    if std::env::var_os("CUDA_PATH").is_some() {
        return;
    }
    let lib = std::path::Path::new("usr/lib").join(crate::edition::BIN).join("cuda");
    let roots = [std::env::var_os("APPDIR").map(PathBuf::from), Some(PathBuf::from("/"))];
    let found = roots.into_iter().flatten().map(|r| r.join(&lib)).find(|d| d.join("nvvm/libdevice/libdevice.10.bc").is_file());
    if let Some(dir) = found {
        eprintln!("SAM: CUDA_PATH={}", dir.display());
        std::env::set_var("CUDA_PATH", dir);
    }
}

/// More than 60 % of the slice: background, not an object worth carrying on.
fn too_big(mask: &[bool]) -> bool {
    mask.iter().filter(|m| **m).count() * 10 > mask.len() * 6
}

/// The model's quality estimate a slice needs to go on: 0.5, or less when
/// the user accepted a lower one on the first slice (thin, faint objects
/// score low throughout), but never under 0.3.
pub fn min_iou(start: f32) -> f32 {
    (start * 0.8).clamp(0.3, 0.5)
}

/// Whether `next` still looks like the object of `prev` on the slice before:
/// confident, of similar size, and overlapping it. Thin objects shift by
/// about their own width from slice to slice, so pixels within 2 px of the
/// other mask also count as overlapping.
pub fn still_same(prev: &[bool], next: &[bool], w: usize, h: usize, iou_pred: f32, min_iou: f32) -> bool {
    let (a, b) = (prev.iter().filter(|m| **m).count(), next.iter().filter(|m| **m).count());
    let both = prev.iter().zip(next).filter(|(p, n)| **p && **n).count();
    if b == 0 || iou_pred < min_iou || too_big(next) {
        return false;
    }
    let ratio = b as f32 / a.max(1) as f32;
    if !(0.4..=2.5).contains(&ratio) {
        return false;
    }
    if both as f32 / (a + b - both) as f32 >= 0.3 {
        return true;
    }
    likeness(prev, next, w, h) >= 0.6
}

/// What `still_same` looks at, for the log.
fn compare(prev: &[bool], next: &[bool], w: usize, h: usize, iou_pred: f32) -> String {
    let (a, b) = (prev.iter().filter(|m| **m).count(), next.iter().filter(|m| **m).count());
    let both = prev.iter().zip(next).filter(|(p, n)| **p && **n).count();
    let iou = both as f32 / (a + b - both).max(1) as f32;
    format!("quality {iou_pred:.2}, {b} px (x{:.2}), IoU {iou:.2}, near {:.2}", b as f32 / a.max(1) as f32, likeness(prev, next, w, h))
}

/// The smaller of the shares of each mask lying within 2 px of the other.
pub fn likeness(prev: &[bool], next: &[bool], w: usize, h: usize) -> f32 {
    let near = |from: &[bool], to: &[bool]| {
        let n = from.iter().filter(|m| **m).count().max(1);
        let dist = crate::labels::edt(to, w, h);
        from.iter().zip(&dist).filter(|(m, d)| **m && **d <= 2.0).count() as f32 / n as f32
    };
    near(next, prev).min(near(prev, next))
}

/// Checks a downloaded model file against the known size and SHA-256.
pub fn verify(bytes_len: u64, sha256_hex: &str) -> Result<()> {
    if bytes_len != MODEL_SIZE || sha256_hex != MODEL_SHA256 {
        bail!("下载的模型文件不完整或已损坏，请重试");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> Vec<bool> {
        (0..w * h).map(|i| ((i % w) as f32 + 0.5 - cx).hypot((i / w) as f32 + 0.5 - cy) < r).collect()
    }

    /// A 3 px thick wavy line across the image, shifted down by `dy`.
    fn band(w: usize, h: usize, dy: f32) -> Vec<bool> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                (y - (h as f32 / 2.0 + dy + 6.0 * (x / 10.0).sin())).abs() < 1.5
            })
            .collect()
    }

    #[test]
    fn prompt_follows_the_mask() {
        let m = disc(64, 48, 30.0, 20.0, 10.0);
        let Prompt { bbox, points: p, .. } = follow_prompt(&m, 64, 48).unwrap();
        let b = bbox.unwrap();
        assert_eq!(p.len(), 1);
        assert!((p[0].x - 30.0).abs() <= 1.0 && (p[0].y - 20.0).abs() <= 1.0);
        assert!(b[0] < 20.0 && b[0] > 16.0 && b[2] > 40.0 && b[2] < 44.0);
        assert!(follow_prompt(&vec![false; 64 * 48], 64, 48).is_none());
    }

    #[test]
    fn thin_objects_get_points_along_them() {
        let m = band(96, 48, 0.0);
        let Prompt { bbox, points: p, use_mask } = follow_prompt(&m, 96, 48).unwrap();
        assert!(bbox.is_some() && !use_mask);
        assert_eq!(p.len(), 3);
        let at = |q: &PromptPoint| m[q.y as usize * 96 + q.x as usize];
        assert!(p.iter().all(at), "{p:?}");
        let mut xs: Vec<f32> = p.iter().map(|q| q.x).collect();
        xs.sort_by(f32::total_cmp);
        assert!(xs.windows(2).all(|p| p[1] - p[0] > 20.0), "{xs:?}");
    }

    #[test]
    fn closed_outlines_go_on_by_points_and_mask() {
        let (w, h) = (96, 64);
        let ring: Vec<bool> = (0..w * h)
            .map(|i| ((i % w) as f32 + 0.5 - 48.0).hypot((i / w) as f32 + 0.5 - 32.0) - 24.0)
            .map(|r| r.abs() < 1.5)
            .collect();
        let Prompt { bbox, points, use_mask } = follow_prompt(&ring, w, h).unwrap();
        assert!(bbox.is_none() && use_mask);
        assert_eq!(points.len(), 5);
        assert!(points.iter().all(|q| ring[q.y as usize * w + q.x as usize]));
    }

    #[test]
    fn stops_when_the_object_is_lost() {
        let same = |a: &[bool], b: &[bool], iou: f32| still_same(a, b, 64, 48, iou, 0.5);
        let a = disc(64, 48, 30.0, 20.0, 10.0);
        assert!(same(&a, &disc(64, 48, 31.0, 20.0, 9.0), 0.9));
        assert!(!same(&a, &disc(64, 48, 31.0, 20.0, 9.0), 0.3));
        assert!(!same(&a, &disc(64, 48, 31.0, 20.0, 3.0), 0.9));
        assert!(!same(&a, &disc(64, 48, 55.0, 40.0, 10.0), 0.9));
        assert!(!same(&a, &vec![false; 64 * 48], 0.9));
        assert!(!same(&vec![true; 64 * 48], &vec![true; 64 * 48], 0.9));
    }

    #[test]
    fn thin_objects_may_shift_by_their_width() {
        let a = band(96, 48, 0.0);
        // Moved by its own width: no plain overlap left, but still the same membrane.
        assert!(still_same(&a, &band(96, 48, 2.5), 96, 48, 0.45, min_iou(0.48)));
        assert!(!still_same(&a, &band(96, 48, 10.0), 96, 48, 0.45, min_iou(0.48)));
        assert!(!still_same(&a, &band(96, 48, 2.5), 96, 48, 0.45, min_iou(0.9)));
        assert_eq!(min_iou(0.9), 0.5);
        assert_eq!(min_iou(0.2), 0.3);
    }

    #[test]
    fn denoise_averages_and_blurs() {
        let a = vec![vec![0.0, 0.0, 9.0, 0.0, 0.0], vec![0.0, 0.0, 3.0, 0.0, 0.0]];
        assert_eq!(denoise(&a, 5, 1, 0.0), vec![0.0, 0.0, 6.0, 0.0, 0.0]);
        let b = denoise(&a, 5, 1, 1.0);
        assert!(b[2] < 6.0 && b[1] > 0.0 && b[2] > b[1] && b[1] > b[0]);
    }

    /// A sphere in a 160x120x21 stack: one click on the middle slice, then
    /// propagation should reach the slices the sphere is on and stop.
    /// TMV_SAM_MODEL=sam2.1_hiera_tiny.pt cargo test --release --features pro sphere -- --ignored
    #[cfg(feature = "pro")]
    #[test]
    #[ignore]
    fn sphere_propagates_and_stops() {
        let path = std::env::var("TMV_SAM_MODEL").expect("TMV_SAM_MODEL");
        let (w, h, d) = (160, 120, 21);
        let r = 8.0f32 * 6.0;
        let slice = |z: usize| -> Vec<f32> {
            let dz = (z as f32 - 10.0) * 6.0;
            (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32 - 80.0, (i / w) as f32 - 60.0);
                    if x * x + y * y + dz * dz < r * r { 200.0 } else { 30.0 }
                })
                .collect()
        };
        let sam = Sam::default();
        let (p, backend) = tmv_sam::load_best(path.as_ref(), &tmv_sam::Sam2Config::tiny(), std::time::Duration::from_secs(90)).unwrap();
        println!("backend {backend}");
        sam.inner.lock().unwrap().predictor = Some(Arc::new(p));
        let req = PredictReq { token: 1, z: 10, c: 0, t: 0, lo: 0.0, hi: 255.0, points: vec![PromptPoint { x: 80.0, y: 60.0, positive: true }], bbox: None, denoise: Denoise::default() };
        sam.predict(&req, (w, h, d), |z| Ok(slice(z))).unwrap();
        let along = PropagateReq { token: 1, z: 10, c: 0, t: 0, lo: 0.0, hi: 255.0, up: true, down: true, max: 100, denoise: Denoise::default() };
        let masks = sam.propagate(&along, (w, h, d), |z| Ok(slice(z)), |_, _| {}).unwrap();
        let mut zs: Vec<usize> = masks.iter().map(|m| m.0).collect();
        zs.sort();
        println!("slices {zs:?}");
        // The sphere covers z 3..=17; the outer slices are tiny caps.
        assert!(zs.contains(&5) && zs.contains(&15), "{zs:?}");
        assert!(!zs.contains(&0) && !zs.contains(&20), "{zs:?}");
        for (z, m) in &masks {
            let truth: Vec<bool> = slice(*z).iter().map(|&v| v > 100.0).collect();
            let both = truth.iter().zip(m).filter(|(a, b)| **a && **b).count();
            let any = truth.iter().zip(m).filter(|(a, b)| **a || **b).count();
            assert!(both as f32 / any as f32 > 0.8, "z {z}: IoU {}", both as f32 / any as f32);
        }
    }

    /// A dark, 3 px thick wavy membrane that drifts 1.5 px per slice, as a
    /// tilted membrane does in a tomogram, should be followed through the
    /// stack from one click.
    #[cfg(feature = "pro")]
    #[test]
    #[ignore]
    fn membrane_propagates() {
        let path = std::env::var("TMV_SAM_MODEL").expect("TMV_SAM_MODEL");
        let (w, h, d) = (192, 128, 21);
        let on = |z: usize, i: usize| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            (y - (50.0 + (z as f32 - 10.0) * 1.5 + 8.0 * (x / 20.0).sin())).abs() < 1.5
        };
        let slice = |z: usize| -> Vec<f32> { (0..w * h).map(|i| if on(z, i) { 40.0 } else { 160.0 }).collect() };
        let (p, _) = tmv_sam::load_best(path.as_ref(), &tmv_sam::Sam2Config::tiny(), std::time::Duration::from_secs(90)).unwrap();
        let sam = Sam::with_predictor(p);
        let x = 96.0f32;
        let y = 50.0 + 8.0 * (x / 20.0).sin();
        let req = PredictReq { token: 1, z: 10, c: 0, t: 0, lo: 0.0, hi: 255.0, points: vec![PromptPoint { x, y, positive: true }], bbox: None, denoise: Denoise::default() };
        let (_, iou) = sam.predict(&req, (w, h, d), |z| Ok(slice(z))).unwrap();
        let along = PropagateReq { token: 1, z: 10, c: 0, t: 0, lo: 0.0, hi: 255.0, up: true, down: true, max: 100, denoise: Denoise::default() };
        let masks = sam.propagate(&along, (w, h, d), |z| Ok(slice(z)), |_, _| {}).unwrap();
        let mut zs: Vec<usize> = masks.iter().map(|m| m.0).collect();
        zs.sort();
        println!("start quality {iou:.2}, slices {zs:?}");
        for (z, m) in &masks {
            let n = m.iter().filter(|v| **v).count();
            let hit = m.iter().enumerate().filter(|(i, v)| **v && on(*z, *i)).count();
            println!("z {z}: {n} px, {:.0} % on the membrane", 100.0 * hit as f32 / n as f32);
        }
        assert!(zs.len() >= 15, "{zs:?}");
    }

    /// A closed, noisy membrane (a cell outline) that shrinks away from the
    /// middle slice, segmented with denoising as tomograms are: its box holds
    /// the whole cell, so it must be followed by points and mask alone.
    #[test]
    fn thin_objects_try_more_prompts() {
        let blob = Prompt { bbox: Some([0.0, 0.0, 9.0, 9.0]), points: vec![PromptPoint { x: 4.5, y: 4.5, positive: true }], use_mask: false };
        assert_eq!(blob.variants().len(), 1);
        let thin = Prompt { points: vec![PromptPoint { x: 4.5, y: 4.5, positive: true }; 3], ..blob };
        let v = thin.variants();
        assert_eq!(v.len(), 4);
        assert!(v[0].0.is_some() && !v[0].1 && v[1].0.is_none() && v[1].1);
    }

    #[cfg(feature = "pro")]
    #[test]
    #[ignore]
    fn closed_membrane_propagates() {
        outline_propagates(false);
    }

    /// A cell outline with a gap on its left: not closed, but its box would
    /// still ask for the whole cell.
    #[cfg(feature = "pro")]
    #[test]
    #[ignore]
    fn open_outline_propagates() {
        outline_propagates(true);
    }

    #[cfg(feature = "pro")]
    fn outline_propagates(gap: bool) {
        let path = std::env::var("TMV_SAM_MODEL").expect("TMV_SAM_MODEL");
        let (w, h, d) = (192, 128, 21);
        let ring = |z: usize, i: usize| {
            let s = 1.0 - (z as f32 - 10.0).abs() * 0.02;
            let (x, y) = (((i % w) as f32 + 0.5 - 96.0) / (60.0 * s), ((i / w) as f32 + 0.5 - 64.0) / (40.0 * s));
            ((x * x + y * y).sqrt() - 1.0).abs() * 40.0 * s < 1.5 && !(gap && x < -0.7)
        };
        let slice = |z: usize| -> Vec<f32> {
            let mut seed = 0x9e37_79b9u32.wrapping_mul(z as u32 + 1);
            (0..w * h)
                .map(|i| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let noise = (seed % 1000) as f32 / 1000.0 * 120.0 - 60.0;
                    (if ring(z, i) { 70.0 } else { 150.0 }) + noise
                })
                .collect()
        };
        let (p, _) = tmv_sam::load_best(path.as_ref(), &tmv_sam::Sam2Config::tiny(), std::time::Duration::from_secs(90)).unwrap();
        let sam = Sam::with_predictor(p);
        let denoise = Denoise { slab: 2, sigma: 1.5 };
        let req = PredictReq { token: 1, z: 10, c: 0, t: 0, lo: 0.0, hi: 255.0, points: vec![PromptPoint { x: 96.0, y: 24.5, positive: true }], bbox: None, denoise };
        let (start, iou) = sam.predict(&req, (w, h, d), |z| Ok(slice(z))).unwrap();
        let n = start.iter().filter(|v| **v != 0).count();
        println!("start quality {iou:.2}, {n} px");
        let along = PropagateReq { token: 1, z: 10, c: 0, t: 0, lo: 0.0, hi: 255.0, up: true, down: true, max: 100, denoise };
        let masks = sam.propagate(&along, (w, h, d), |z| Ok(slice(z)), |_, _| {}).unwrap();
        let mut zs: Vec<usize> = masks.iter().map(|m| m.0).collect();
        zs.sort();
        println!("slices {zs:?}");
        for (z, m) in &masks {
            let n = m.iter().filter(|v| **v).count();
            let near = m.iter().enumerate().filter(|(i, v)| **v && (0..9).any(|k| ring(*z, (*i + w * 4 + k).saturating_sub(4 + w * 4).min(w * h - 1)))).count();
            println!("z {z}: {n} px, {:.0} % near the membrane", 100.0 * near as f32 / n as f32);
        }
        assert!(zs.len() >= 15, "{zs:?}");
    }
}
