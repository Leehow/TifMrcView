//! Native half of the recorder plugin: RGBA frames in, H.264 MP4 out.
//!
//! Methods: `begin {path, width, height, fps, quality}`, `frame` (body =
//! RGBA, `{repeat}` optional), `finish`, `cancel`.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use anyhow::{anyhow, bail, Context, Result};
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, QpRange,
    RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::{RgbaSliceU8, YUVBuffer};
use openh264::OpenH264API;
use serde::Deserialize;
use serde_json::{json, Value};

use super::mp4::{Mp4Writer, TIMESCALE};
use super::NativePlugin;

/// OpenH264 refuses anything larger.
pub const MAX_LONG: u32 = 3840;
pub const MAX_SHORT: u32 = 2160;

struct Session {
    path: PathBuf,
    width: u32,
    height: u32,
    frame_ticks: f64,
    ticks_done: f64,
    frames_in: u64,
    encoder: Encoder,
    yuv: YUVBuffer,
    mp4: Mp4Writer,
}

#[derive(Default)]
pub struct Recorder {
    session: Mutex<Option<Session>>,
}

#[derive(Deserialize)]
struct Begin {
    path: String,
    width: u32,
    height: u32,
    fps: f64,
    /// 0 (small file) .. 1 (near lossless).
    #[serde(default = "default_quality")]
    quality: f64,
}

fn default_quality() -> f64 {
    0.6
}

#[derive(Deserialize, Default)]
struct Frame {
    #[serde(default)]
    repeat: Option<u32>,
}

impl Recorder {
    /// The session survives a panic in the encoder; the next `begin` replaces it.
    fn session(&self) -> MutexGuard<'_, Option<Session>> {
        self.session.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn begin(&self, a: Begin) -> Result<Value> {
        let (w, h) = (a.width, a.height);
        if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
            bail!("frame size must be even, got {w}×{h}");
        }
        if w.max(h) > MAX_LONG || w.min(h) > MAX_SHORT {
            bail!("frame size {w}×{h} exceeds {MAX_LONG}×{MAX_SHORT}");
        }
        if !(a.fps > 0.0 && a.fps <= 120.0) {
            bail!("fps must be in (0, 120]");
        }
        let q = a.quality.clamp(0.0, 1.0);
        // Microscopy data is mostly fine-grained noise, which the default
        // rate control smears into blotches. Budget generously (bits per
        // pixel per frame 0.15 .. 1.5) and cap the quantiser (31 .. 20).
        let bpp = 0.15 * (10f64).powf(q);
        let bps = (w as f64 * h as f64 * a.fps * bpp).clamp(2_000_000.0, 200_000_000.0) as u32;
        let max_qp = (36.0 - 16.0 * q).round() as u8;
        let cfg = EncoderConfig::new()
            .usage_type(UsageType::CameraVideoRealTime)
            .rate_control_mode(RateControlMode::Quality)
            .bitrate(BitRate::from_bps(bps))
            .max_frame_rate(FrameRate::from_hz(a.fps as f32))
            .skip_frames(false)
            .complexity(Complexity::High)
            .qp(QpRange::new(10, max_qp))
            .background_detection(false)
            .adaptive_quantization(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(
                (a.fps * 2.0).ceil().max(1.0) as u32,
            ))
            .vui(VuiConfig::bt601());
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), cfg)
            .map_err(|e| anyhow!("{e}"))?;
        let path = PathBuf::from(&a.path);
        let mp4 = Mp4Writer::create(&path, w, h)
            .with_context(|| format!("cannot write {}", path.display()))?;
        let mut s = self.session();
        if let Some(old) = s.take() {
            let _ = std::fs::remove_file(&old.path);
        }
        *s = Some(Session {
            path,
            width: w,
            height: h,
            frame_ticks: TIMESCALE as f64 / a.fps,
            ticks_done: 0.0,
            frames_in: 0,
            encoder,
            yuv: YUVBuffer::new(w as usize, h as usize),
            mp4,
        });
        Ok(json!({ "bitrate": bps }))
    }

    fn frame(&self, a: Frame, rgba: &[u8]) -> Result<Value> {
        let mut guard = self.session();
        let s = guard
            .as_mut()
            .ok_or_else(|| anyhow!("recording not started"))?;
        let (w, h) = (s.width as usize, s.height as usize);
        if rgba.len() != w * h * 4 {
            bail!(
                "expected {}×{} RGBA ({} bytes), got {}",
                w,
                h,
                w * h * 4,
                rgba.len()
            );
        }
        s.yuv.read_rgba8(RgbaSliceU8::new(rgba, (w, h)));
        let bits = s
            .encoder
            .encode(&s.yuv)
            .map_err(|e| anyhow!("{e}"))?
            .to_vec();
        // Keep timestamps exact for fractional frame rates.
        let repeat = a.repeat.unwrap_or(1).max(1) as f64;
        let start = s.ticks_done.round();
        s.ticks_done += s.frame_ticks * repeat;
        let ticks = (s.ticks_done.round() - start).max(1.0) as u32;
        s.mp4.add(&bits, ticks)?;
        s.frames_in += 1;
        Ok(json!({ "frames": s.frames_in }))
    }

    fn finish(&self) -> Result<Value> {
        let s = self
            .session()
            .take()
            .ok_or_else(|| anyhow!("recording not started"))?;
        let frames = s.mp4.frames();
        let path = s.path.clone();
        match s.mp4.finish() {
            Ok(bytes) => Ok(json!({ "path": path, "frames": frames, "bytes": bytes })),
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                Err(e)
            }
        }
    }

    fn cancel(&self) -> Result<Value> {
        if let Some(s) = self.session().take() {
            drop(s.mp4);
            let _ = std::fs::remove_file(&s.path);
        }
        Ok(Value::Null)
    }
}

impl NativePlugin for Recorder {
    fn id(&self) -> &'static str {
        "recorder"
    }

    fn call(&self, method: &str, args: Value, body: &[u8]) -> Result<Value> {
        match method {
            "begin" => self.begin(serde_json::from_value(args)?),
            "frame" => self.frame(serde_json::from_value(args).unwrap_or_default(), body),
            "finish" => self.finish(),
            "cancel" => self.cancel(),
            "limits" => Ok(json!({ "maxLong": MAX_LONG, "maxShort": MAX_SHORT })),
            _ => bail!("unknown method {method}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_playable_mp4() {
        let dir = std::env::temp_dir().join(format!("tmv-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.mp4");
        let r = Recorder::default();
        let (w, h) = (320u32, 240u32);
        r.call(
            "begin",
            json!({"path": path, "width": w, "height": h, "fps": 7.5}),
            &[],
        )
        .unwrap();
        for i in 0..20u32 {
            let mut px = vec![0u8; (w * h * 4) as usize];
            for (k, p) in px.chunks_mut(4).enumerate() {
                let x = k as u32 % w;
                p.copy_from_slice(&[((x + i * 8) % 256) as u8, (i * 12) as u8, 128, 255]);
            }
            r.call("frame", Value::Null, &px).unwrap();
        }
        let res = r.call("finish", Value::Null, &[]).unwrap();
        assert_eq!(res["frames"], 20);
        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[4..8], b"ftyp");
        assert_eq!(res["bytes"].as_u64().unwrap(), data.len() as u64);
        if let Ok(keep) = std::env::var("TMV_KEEP_MP4") {
            std::fs::copy(&path, keep).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
