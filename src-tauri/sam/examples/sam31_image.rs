//! Native SAM 3.1 batch probe: <checkpoint.pt> <image.png> <requests.json> <out-dir>.
//! requests: [{"name":"click","points":[[370,172,1]]},
//!            {"name":"box","box":[150,95,385,230]}]
//! Optional "mask" points at a previous native low.f32 output (288x288).
use anyhow::{Context, Result, bail};
use burn::tensor::{Tensor, TensorData};
use std::{path::PathBuf, time::Instant};
use tmv_sam::{Point, sam31};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        bail!("usage: sam31_image <checkpoint.pt> <image.png> <requests.json> <out-dir>");
    }
    let image = image::open(&args[1])?.to_rgb8();
    let (w, h) = (image.width() as usize, image.height() as usize);
    let requests: serde_json::Value = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    let out = PathBuf::from(&args[3]);
    std::fs::create_dir_all(&out)?;
    let (device, backend) = tmv_sam::pick_device();
    eprintln!("SAM 3.1 native backend: {backend}");
    let start = Instant::now();
    let predictor = sam31::Predictor::load(args[0].as_ref(), &device)?;
    let load_s = start.elapsed().as_secs_f32();
    let start = Instant::now();
    let encoded = predictor.encode_rgb(image.as_raw(), w, h)?;
    let _ = encoded.features.embed.clone().sum().into_scalar::<f32>();
    let encode_s = start.elapsed().as_secs_f32();
    let mut rows = vec![];
    for request in requests
        .as_array()
        .context("requests must be a JSON array")?
    {
        let name = request["name"].as_str().context("name")?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            bail!("unsafe result name");
        }
        let mut points = vec![];
        if let Some(pts) = request["points"].as_array() {
            for p in pts {
                points.push(Point {
                    x: p[0].as_f64().context("x")? as f32,
                    y: p[1].as_f64().context("y")? as f32,
                    positive: p[2].as_i64().context("label")? != 0,
                });
            }
        }
        let bbox = if let Some(v) = request["box"].as_array() {
            if v.len() != 4 {
                bail!("box needs four coordinates");
            }
            Some([
                v[0].as_f64().context("x0")? as f32,
                v[1].as_f64().context("y0")? as f32,
                v[2].as_f64().context("x1")? as f32,
                v[3].as_f64().context("y1")? as f32,
            ])
        } else {
            None
        };
        let mask = if let Some(path) = request["mask"].as_str() {
            let bytes = std::fs::read(path)?;
            if bytes.len() != sam31::MASK_SIZE * sam31::MASK_SIZE * 4 {
                bail!("mask must contain 288x288 little-endian f32 values");
            }
            let values: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            Some(Tensor::<4>::from_data(
                TensorData::new(values, [1, 1, sam31::MASK_SIZE, sam31::MASK_SIZE]),
                &device,
            ))
        } else {
            None
        };
        let start = Instant::now();
        let result = predictor.predict(&encoded, &points, bbox, mask)?;
        let elapsed = start.elapsed().as_secs_f32();
        let pixels = result.logits.iter().filter(|&&v| v > 0.0).count();
        let mask: Vec<u8> = result
            .logits
            .iter()
            .map(|&v| if v > 0.0 { 255 } else { 0 })
            .collect();
        image::save_buffer(
            out.join(format!("{name}.png")),
            &mask,
            w as u32,
            h as u32,
            image::ColorType::L8,
        )?;
        let bytes: Vec<u8> = result.logits.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(out.join(format!("{name}.logits.f32")), bytes)?;
        let bytes: Vec<u8> = result
            .low_res_values()
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        std::fs::write(out.join(format!("{name}.low.f32")), bytes)?;
        let row = serde_json::json!({"name": name, "iou_prediction": result.iou, "pixels": pixels, "seconds": elapsed});
        println!("{row}");
        rows.push(row);
    }
    std::fs::write(
        out.join("results.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"backend":backend, "load_s":load_s, "encode_s":encode_s, "width":w, "height":h, "requests":rows}),
        )?,
    )?;
    Ok(())
}
