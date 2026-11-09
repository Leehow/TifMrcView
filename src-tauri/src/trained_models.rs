//! Persistence adapters for the four models trained interactively in Pro.
//! These records restore inference; no optimizer-state resume is implied.
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use burn::tensor::Device;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::image::OpenImage;
use crate::model_store::{self, Record, Session};

pub const WEDGE: &str = "tmv-wedge-unet3d-v1";
pub const DENOISE: &str = "tmv-denoise-unet2d-base32-levels2-v1";
pub const UNET: &str = "tmv-label-unet2d-base16-levels3-v1";
pub const CLASSIFIER: &str = "tmv-pixel-forest-features-v1";

pub fn architecture(kind: &str) -> Result<&'static str> {
    match kind {
        "wedge" => Ok(WEDGE),
        "denoise" => Ok(DENOISE),
        "unet" => Ok(UNET),
        "classifier" => Ok(CLASSIFIER),
        _ => bail!("未知的训练模型类型"),
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub record: Record,
    pub record_path: String,
    pub weights_path: String,
    pub summary: Value,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Persistence {
    pub saved: Option<Saved>,
    pub error: Option<String>,
}

impl Persistence {
    pub fn from_result(result: Result<Saved>) -> Self {
        match result {
            Ok(saved) => Self { saved: Some(saved), error: None },
            Err(e) => Self { saved: None, error: Some(format!("{e:#}")) },
        }
    }
}

fn saved(record: Record, path: PathBuf) -> Saved {
    let weights = path.parent().unwrap().join(&record.payload);
    let model = &record.parameters["model"];
    let summary = match record.kind.as_str() {
        "wedge" => {
            json!({"dims": model["dims"], "cube": record.parameters["effective"]["config"]["cube"], "loss": model["loss"], "backend": "已恢复", "seconds": record.elapsed_seconds})
        }
        "denoise" => {
            json!({"mode": model["mode"], "images": record.parameters["trainingImages"], "loss": model["loss"], "backend": "已恢复", "seconds": record.elapsed_seconds, "warning": null})
        }
        "unet" => {
            json!({"labels": model["labels"], "slices": record.parameters["trainingSlices"].as_array().map_or(0, Vec::len), "loss": model["loss"], "accuracy": model["accuracy"], "backend": "已恢复", "seconds": record.elapsed_seconds})
        }
        "classifier" => json!({"labels": model["labels"], "samples": model["samples"], "accuracy": model["accuracy"], "backend": "已恢复", "seconds": record.elapsed_seconds}),
        _ => Value::Null,
    };
    Saved { record, record_path: path.to_string_lossy().into(), weights_path: weights.to_string_lossy().into(), summary }
}

fn finite(x: f32) -> Option<f32> {
    x.is_finite().then_some(x)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WedgeModel {
    geometry: crate::wedge::Geometry,
    bin: usize,
    mean: f32,
    std: f32,
    loss: Option<f32>,
    pixel_a: f64,
    dims: [usize; 3],
}

#[derive(Serialize, Deserialize)]
struct DenoiseModel {
    mode: crate::denoise_learn::Mode,
    bin: usize,
    mean: f32,
    std: f32,
    loss: Option<f32>,
    source: String,
}

#[derive(Serialize, Deserialize)]
struct UnetModel {
    labels: Vec<u16>,
    lo: f32,
    hi: f32,
    loss: Option<f32>,
    accuracy: Option<f32>,
}

pub fn save_wedge(session: &Session, model: &crate::wedge::Trained, complete: bool, progress: Value, backend: &str) -> Result<Saved> {
    let mut shot = session.clone();
    shot.record.parameters["model"] = serde_json::to_value(WedgeModel {
        geometry: model.geometry,
        bin: model.bin,
        mean: model.mean,
        std: model.std,
        loss: finite(model.loss),
        pixel_a: model.pixel_a,
        dims: model.vol.n,
    })?;
    let record = shot.save(complete, progress, json!({"loss": finite(model.loss)}), backend, "safetensors", |p| model_store::save_network(&model.net, p))?;
    Ok(saved(record, shot.record_path(complete)))
}

pub fn save_denoise(session: &Session, model: &crate::denoise_learn::Learned, complete: bool, progress: Value, backend: &str) -> Result<Saved> {
    let mut shot = session.clone();
    shot.record.parameters["model"] =
        serde_json::to_value(DenoiseModel { mode: model.mode, bin: model.bin, mean: model.mean, std: model.std, loss: finite(model.loss), source: model.source.clone() })?;
    let record = shot.save(complete, progress, json!({"loss": finite(model.loss)}), backend, "safetensors", |p| model_store::save_network(&model.net, p))?;
    Ok(saved(record, shot.record_path(complete)))
}

pub fn save_unet(session: &Session, model: &crate::unet::Trained, complete: bool, progress: Value, backend: &str) -> Result<Saved> {
    let mut shot = session.clone();
    shot.record.parameters["model"] =
        serde_json::to_value(UnetModel { labels: model.labels.clone(), lo: model.lo, hi: model.hi, loss: finite(model.loss), accuracy: finite(model.accuracy) })?;
    let record = shot
        .save(complete, progress, json!({"loss": finite(model.loss), "accuracy": finite(model.accuracy)}), backend, "safetensors", |p| model_store::save_network(&model.net, p))?;
    Ok(saved(record, shot.record_path(complete)))
}

pub fn save_classifier(session: &Session, model: &crate::classify::Classifier, backend: &str) -> Result<Saved> {
    model.validate_checkpoint()?;
    let mut shot = session.clone();
    shot.record.parameters["model"] = json!({"labels": model.labels, "samples": model.samples, "lo": model.lo, "hi": model.hi, "accuracy": model.forest.oob_accuracy});
    let record = shot.save(true, json!({"status": "complete"}), json!({"accuracy": model.forest.oob_accuracy}), backend, "json", |p| {
        let mut file = std::io::BufWriter::new(std::fs::File::create(p)?);
        serde_json::to_writer(&mut file, model)?;
        file.flush()?;
        Ok(())
    })?;
    Ok(saved(record, shot.record_path(true)))
}

pub enum Model {
    Wedge(crate::wedge::Trained),
    Denoise(crate::denoise_learn::Learned),
    Unet(crate::unet::Trained),
    Classifier(crate::classify::Classifier),
}

fn normalization(mean: f32, std: f32) -> Result<()> {
    if !mean.is_finite() || !std.is_finite() || std <= 0.0 {
        bail!("模型归一化参数无效");
    }
    Ok(())
}

pub fn restore(img: &OpenImage, kind: &str, c: usize, t: usize) -> Result<Option<(Saved, Model)>> {
    let Some((record, path)) = model_store::load(img, kind, c, t, architecture(kind)?)? else { return Ok(None) };
    if img.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
        bail!("已取消");
    }
    let dev = Device::flex();
    let model = match kind {
        "wedge" => {
            let m: WedgeModel = serde_json::from_value(record.parameters["model"].clone())?;
            let cfg: crate::wedge::TrainConfig = serde_json::from_value(record.parameters["effective"]["config"].clone())?;
            normalization(m.mean, m.std)?;
            m.geometry.check()?;
            if !(1..=4).contains(&m.bin) || !(1..=64).contains(&cfg.base) || !(1..=4).contains(&cfg.levels) || !cfg.smooth.is_finite() || !(0.0..=10.0).contains(&cfg.smooth) {
                bail!("缺失楔模型结构或预处理参数无效");
            }
            let mut net = crate::wedge::Net::new(cfg.base, cfg.levels, &dev);
            model_store::load_network(&mut net, &path)?;
            let (vol, _, _) = crate::wedge::volume(img, c, t, m.bin)?;
            if vol.n != m.dims {
                bail!("缺失楔模型体尺寸不匹配");
            }
            let vol = if cfg.smooth > 0.0 { Arc::new(crate::wedge::smooth(&vol, cfg.smooth)) } else { vol };
            Model::Wedge(crate::wedge::Trained {
                net,
                vol,
                geometry: m.geometry,
                bin: m.bin,
                mean: m.mean,
                std: m.std,
                loss: m.loss.unwrap_or(f32::NAN),
                token: img.token,
                pixel_a: m.pixel_a,
                c,
                t,
            })
        }
        "denoise" => {
            let m: DenoiseModel = serde_json::from_value(record.parameters["model"].clone())?;
            normalization(m.mean, m.std)?;
            if !(1..=8).contains(&m.bin) {
                bail!("降噪模型分箱无效");
            }
            let mut net = crate::denoise_learn::Net::new(&dev);
            model_store::load_network(&mut net, &path)?;
            Model::Denoise(crate::denoise_learn::Learned { net, mode: m.mode, bin: m.bin, mean: m.mean, std: m.std, loss: m.loss.unwrap_or(f32::NAN), source: m.source })
        }
        "unet" => {
            let m: UnetModel = serde_json::from_value(record.parameters["model"].clone())?;
            if !(2..=4096).contains(&m.labels.len()) || m.labels[0] == 0 || !m.labels.windows(2).all(|w| w[0] < w[1]) || !m.lo.is_finite() || !m.hi.is_finite() {
                bail!("分割模型类别或归一化参数无效");
            }
            let mut net = crate::unet::UNet::new(m.labels.len(), &dev);
            model_store::load_network(&mut net, &path)?;
            Model::Unet(crate::unet::Trained { net, labels: m.labels, lo: m.lo, hi: m.hi, loss: m.loss.unwrap_or(f32::NAN), accuracy: m.accuracy.unwrap_or(0.0) })
        }
        "classifier" => {
            if std::fs::metadata(&path)?.len() > 512 * 1024 * 1024 {
                bail!("分类器模型文件过大");
            }
            let model: crate::classify::Classifier = serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(&path)?))?;
            model.validate_checkpoint()?;
            if serde_json::to_value(&model.labels)? != record.parameters["model"]["labels"] {
                bail!("分类器类别记录不匹配");
            }
            Model::Classifier(model)
        }
        _ => unreachable!(),
    };
    let record_path = model_store::directory(img, kind, c, t)?.join(if record.complete { "model.json" } else { "checkpoint.json" });
    Ok(Some((saved(record, record_path), model)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reopened(path: &str) -> Arc<OpenImage> {
        OpenImage::new(900_001, Box::new(crate::formats::mrc::MrcSource::open(std::path::Path::new(path)).unwrap()), 0)
    }

    fn same(a: &[f32], b: &[f32]) {
        assert_eq!(a.len(), b.len());
        let delta = a.iter().zip(b).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(a.iter().chain(b).all(|v| v.is_finite()) && delta < 1e-6, "restored inference differs by {delta}");
    }

    #[test]
    fn trained_wedge_restores_weights_geometry_and_preprocessing_without_retraining() {
        let temp = tempfile::tempdir().unwrap();
        let img = model_store::tests::image(temp.path(), "wedge.mrc");
        let cfg = crate::wedge::TrainConfig { cube: 16, count: 1, iterations: 1, steps: 1, batch: 1, base: 2, levels: 1, smooth: 0.7, ..Default::default() };
        let g = crate::wedge::Geometry { beam: 2, tilt: 1, max_tilt: 60.0, min_tilt: Some(-55.0) };
        let session = Session::start(&img, "wedge", 0, 0, WEDGE, json!({"effective": {"config": cfg, "geometry": g, "bin": 1}, "trainingSlices": [0,15]})).unwrap();
        let (vol, mean, std) = crate::wedge::volume(&img, 0, 0, 1).unwrap();
        let vol = Arc::new(crate::wedge::smooth(&vol, cfg.smooth));
        let dev = Device::flex();
        let (net, loss) = crate::wedge::train(&dev, &vol, &g, &cfg, |_| {}).unwrap();
        let model = crate::wedge::Trained { net, vol, geometry: g, bin: 1, mean, std, loss, token: img.token, pixel_a: 2.5, c: 0, t: 0 };
        let expected = crate::wedge::apply(&dev, &model.net, &model.vol, [0, 0, 7], [16, 16, 1], &mut |_, _| Ok(())).unwrap();
        save_wedge(&session, &model, true, json!({"iteration": 1, "step": 1}), "CPU").unwrap();
        let path = img.info().path.clone();
        drop(model);
        drop(img);
        let new_image = reopened(&path);
        let (saved, Model::Wedge(restored)) = restore(&new_image, "wedge", 0, 0).unwrap().unwrap() else { panic!("wrong model kind") };
        assert_eq!(restored.token, new_image.token);
        assert_eq!(restored.geometry.min_tilt, Some(-55.0));
        assert_eq!(saved.record.parameters["effective"]["config"]["seed"], cfg.seed);
        same(&expected, &crate::wedge::apply(&dev, &restored.net, &restored.vol, [0, 0, 7], [16, 16, 1], &mut |_, _| Ok(())).unwrap());
    }

    #[test]
    fn trained_denoiser_restores_parameters_and_identical_inference() {
        let temp = tempfile::tempdir().unwrap();
        let img = model_store::tests::image(temp.path(), "denoise.mrc");
        let input = crate::denoise::input(&img, 0, 0, 0, 0).unwrap();
        let cfg = crate::denoise_learn::TrainConfig { steps: 2, batch: 1, patch: 8, ..Default::default() };
        let mode = crate::denoise_learn::Mode::N2v;
        let session =
            Session::start(&img, "denoise", 0, 0, DENOISE, json!({"effective": {"config": cfg, "mode": mode, "bin": 1, "slab": 0}, "trainingSlices": [0], "trainingImages": 1}))
                .unwrap();
        let dev = Device::flex();
        let (net, mean, std, loss) = crate::denoise_learn::train(&dev, &[(input.clone(), None)], 16, 16, mode, &cfg, |_, _, _| {}).unwrap();
        let model = crate::denoise_learn::Learned { net, mode, mean, std, loss, source: img.info().name.clone(), bin: 1 };
        let expected = model.apply(&dev, &input, 16, 16);
        save_denoise(&session, &model, true, json!({"step": 2, "totalSteps": 2}), "CPU").unwrap();
        let path = img.info().path.clone();
        drop(model);
        drop(img);
        let (saved, Model::Denoise(restored)) = restore(&reopened(&path), "denoise", 0, 0).unwrap().unwrap() else { panic!("wrong model kind") };
        assert_eq!(saved.record.progress["step"], 2);
        assert_eq!(restored.mode, mode);
        same(&expected, &restored.apply(&dev, &input, 16, 16));
    }

    #[test]
    fn trained_unet_restores_class_mapping_and_identical_segmentation() {
        let temp = tempfile::tempdir().unwrap();
        let img = model_store::tests::image(temp.path(), "unet.mrc");
        let input = crate::denoise::input(&img, 0, 0, 0, 0).unwrap();
        let labels: Vec<u16> = (0..256).map(|i| if i % 16 < 8 { 3 } else { 8 }).collect();
        let cfg = crate::unet::TrainConfig { steps: 2, batch: 1, patch: 16, ..Default::default() };
        let session = Session::start(&img, "unet", 0, 0, UNET, json!({"effective": {"config": cfg}, "trainingSlices": [0]})).unwrap();
        let dev = Device::flex();
        let model = crate::unet::train(&dev, &[(input.clone(), labels)], 16, 16, 0.0, 1.0, &cfg, |_, _, _| {}).unwrap();
        let expected = model.predict(&dev, &input, 16, 16);
        save_unet(&session, &model, true, json!({"step": 2}), "CPU").unwrap();
        let path = img.info().path.clone();
        drop(model);
        drop(img);
        let (_, Model::Unet(restored)) = restore(&reopened(&path), "unet", 0, 0).unwrap().unwrap() else { panic!("wrong model kind") };
        assert_eq!(restored.labels, [3, 8]);
        let actual = restored.predict(&dev, &input, 16, 16);
        assert_eq!(expected.0, actual.0);
        same(&expected.1, &actual.1);
    }

    #[test]
    fn trained_classifier_restores_forest_labels_and_probabilities() {
        let temp = tempfile::tempdir().unwrap();
        let img = model_store::tests::image(temp.path(), "forest.mrc");
        let input = crate::denoise::input(&img, 0, 0, 0, 0).unwrap();
        let labels: Vec<u16> = (0..256).map(|i| if i % 16 < 8 { 2 } else { 9 }).collect();
        let cfg = crate::classify::ForestConfig { trees: 3, max_depth: 4, bins: 4, ..Default::default() };
        let session = Session::start(&img, "classifier", 0, 0, CLASSIFIER, json!({"effective": {"config": cfg}, "trainingSlices": [0]})).unwrap();
        let dev = Device::flex();
        let model = crate::classify::train(&dev, &[(input.clone(), &labels)], 16, 16, 0.0, 1.0, &cfg).unwrap();
        let features = crate::classify::features(&dev, &input, 16, 16);
        let expected = model.forest.predict_cpu(&features, 256);
        save_classifier(&session, &model, "CPU").unwrap();
        let path = img.info().path.clone();
        drop(model);
        drop(img);
        let (_, Model::Classifier(restored)) = restore(&reopened(&path), "classifier", 0, 0).unwrap().unwrap() else { panic!("wrong model kind") };
        assert_eq!(restored.labels, [2, 9]);
        let actual = restored.forest.predict_cpu(&features, 256);
        assert_eq!(expected.0, actual.0);
        same(&expected.1, &actual.1);
    }
}
