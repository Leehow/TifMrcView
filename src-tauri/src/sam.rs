#![cfg_attr(not(feature = "pro"), allow(dead_code))]
//! Pro: SAM 2.1 click segmentation. The model file is downloaded once into
//! the app's data folder; the model is loaded on first use and the features
//! of the last segmented slice are kept, so further clicks only run the
//! small mask decoder.

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

#[derive(Deserialize, Clone, Copy)]
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

/// The mask of the last prediction, kept for `sam_apply`.
pub struct LastMask {
    pub token: u64,
    pub z: usize,
    pub mask: Vec<bool>,
}

#[derive(Default)]
pub struct Sam {
    pub downloading: AtomicBool,
    inner: Mutex<Inner>,
    pub last: Mutex<Option<LastMask>>,
}

#[derive(Default)]
struct Inner {
    #[cfg(feature = "pro")]
    predictor: Option<Arc<tmv_sam::Predictor>>,
    backend: Option<String>,
    /// (token, z, c, t, lo, hi) of `encoded`.
    #[cfg(feature = "pro")]
    key: Option<(u64, usize, usize, usize, u32, u32)>,
    #[cfg(feature = "pro")]
    encoded: Option<Arc<tmv_sam::Encoded>>,
}

impl Sam {
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

    /// Mask of the prompt on slice `req.z` as one byte per pixel (1 = inside),
    /// plus the predicted IoU. `values` reads the slice's raw values.
    #[cfg(feature = "pro")]
    pub fn predict(&self, req: &PredictReq, w: usize, h: usize, values: impl FnOnce() -> Result<Vec<f32>>) -> Result<(Vec<u8>, f32)> {
        let path = model_path().filter(|p| p.exists());
        let Some(path) = path else { bail!("还没有下载 SAM 模型") };
        let key = (req.token, req.z, req.c, req.t, req.lo.to_bits(), req.hi.to_bits());
        let (predictor, encoded) = {
            let mut inner = self.inner.lock().unwrap();
            if inner.predictor.is_none() {
                let (device, name) = tmv_sam::pick_device();
                let p = tmv_sam::Predictor::load(&path, &tmv_sam::Sam2Config::tiny(), &device)?;
                inner.predictor = Some(Arc::new(p));
                inner.backend = Some(name.to_string());
            }
            let p = inner.predictor.clone().unwrap();
            if inner.key != Some(key) {
                let v = values()?;
                let span = (req.hi - req.lo).abs().max(f32::MIN_POSITIVE);
                let lo = req.lo.min(req.hi);
                let gray: Vec<u8> = v.iter().map(|&x| ((x - lo) / span * 255.0).clamp(0.0, 255.0) as u8).collect();
                inner.encoded = Some(Arc::new(p.encode_gray(&gray, w, h)));
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
        *self.last.lock().unwrap() = Some(LastMask { token: req.token, z: req.z, mask });
        Ok((bytes, r.iou))
    }

    #[cfg(not(feature = "pro"))]
    pub fn predict(&self, _: &PredictReq, _: usize, _: usize, _: impl FnOnce() -> Result<Vec<f32>>) -> Result<(Vec<u8>, f32)> {
        bail!("SAM 分割只在 TifMrcView Pro 里有")
    }
}

/// Checks a downloaded model file against the known size and SHA-256.
pub fn verify(bytes_len: u64, sha256_hex: &str) -> Result<()> {
    if bytes_len != MODEL_SIZE || sha256_hex != MODEL_SHA256 {
        bail!("下载的模型文件不完整或已损坏，请重试");
    }
    Ok(())
}
