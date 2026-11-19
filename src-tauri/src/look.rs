//! A picture of one slice for an agent to read coordinates from: the region
//! asked for, contrast-stretched (1st–99th percentile), enlarged or reduced
//! to about `size` pixels, the label layers drawn as coloured outlines, and a (blue)
//! grid with image-pixel numbers on the margins. An agent that segments by
//! looking (it may see better than our tools) reads x and y off the grid
//! and gives marks back (label_marks.rs). Used by `tmv look` and by the
//! window's `look` action, so both show the same picture.

use std::sync::Arc;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::image::OpenImage;
use crate::labels::{Labels, Values};

const MARGIN_L: usize = 34;
const MARGIN_T: usize = 16;

// 3×5 digits, one row per 3 bits (top first).
const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7], [2, 6, 2, 2, 7], [7, 1, 7, 4, 7], [7, 1, 7, 1, 7], [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7], [7, 4, 7, 5, 7], [7, 1, 1, 1, 1], [7, 5, 7, 5, 7], [7, 5, 7, 1, 7],
];

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[u8; 3]>,
}

impl Canvas {
    fn set(&mut self, x: usize, y: usize, c: [u8; 3]) {
        if x < self.w && y < self.h {
            self.px[y * self.w + x] = c;
        }
    }
    fn blend(&mut self, x: usize, y: usize, c: [u8; 3], a: f32) {
        if x < self.w && y < self.h {
            let p = &mut self.px[y * self.w + x];
            for i in 0..3 {
                p[i] = (p[i] as f32 * (1.0 - a) + c[i] as f32 * a) as u8;
            }
        }
    }
    /// A number in the 3×5 font at scale 2, its top left at (x, y).
    fn number(&mut self, x: usize, y: usize, n: usize, c: [u8; 3]) {
        for (i, ch) in n.to_string().bytes().enumerate() {
            let g = DIGITS[(ch - b'0') as usize];
            for (row, bits) in g.iter().enumerate() {
                for col in 0..3 {
                    if bits >> (2 - col) & 1 == 1 {
                        for dy in 0..2 {
                            for dx in 0..2 {
                                self.set(x + i * 8 + col * 2 + dx, y + row * 2 + dy, c);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A round grid step giving about 6–12 lines across `span` pixels.
fn grid_step(span: f64) -> usize {
    let raw = span / 8.0;
    let p = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * p).find(|s| *s >= raw).unwrap_or(10.0 * p);
    (step.round() as usize).max(1)
}

fn color_of(l: &Labels, k: u16) -> [u8; 3] {
    l.materials.iter().find(|m| m.label == k).and_then(|m| m.color).unwrap_or(crate::label_marks::palette(k))
}

/// The PNG and what it shows: `region` [x, y, w, h] in image pixels, the
/// scale (screen pixels per image pixel) and the grid step.
pub fn look(img: &Arc<OpenImage>, labels: Option<&Labels>, z: usize, region: Option<[usize; 4]>, size: usize) -> Result<(Vec<u8>, Value)> {
    let info = img.info();
    let (w, h) = (info.width, info.height);
    if z >= info.size_z {
        bail!("z={z} 超出范围（0–{}）", info.size_z.saturating_sub(1));
    }
    let [rx, ry, rw, rh] = region.unwrap_or([0, 0, w, h]);
    let (rx, ry) = (rx.min(w - 1), ry.min(h - 1));
    let (rw, rh) = (rw.clamp(1, w - rx), rh.clamp(1, h - ry));
    let vals = crate::labels::ChannelValues { img: img.clone(), c: 0, t: 0 }.slice(z)?;
    let mut s: Vec<f32> = (ry..ry + rh).flat_map(|y| vals[y * w + rx..y * w + rx + rw].iter().copied()).filter(|v| v.is_finite()).collect();
    s.sort_by(f32::total_cmp);
    let (lo, hi) = if s.is_empty() { (0.0, 1.0) } else { (s[s.len() / 100], s[(s.len() - 1) * 99 / 100]) };
    let span = if hi > lo { hi - lo } else { 1.0 };
    let scale = (size.clamp(64, 4096) as f64 / rw.max(rh) as f64).min(32.0);
    let (ow, oh) = (((rw as f64 * scale).round() as usize).max(1), ((rh as f64 * scale).round() as usize).max(1));
    let mut c = Canvas { w: ow + MARGIN_L, h: oh + MARGIN_T, px: vec![[24, 24, 24]; (ow + MARGIN_L) * (oh + MARGIN_T)] };
    let at = |ox: usize, oy: usize| -> (usize, usize) {
        (rx + ((ox as f64 + 0.5) / scale) as usize, ry + ((oy as f64 + 0.5) / scale) as usize)
    };
    for oy in 0..oh {
        for ox in 0..ow {
            let (x, y) = at(ox, oy);
            let v = (((vals[y.min(h - 1) * w + x.min(w - 1)] - lo) / span).clamp(0.0, 1.0) * 255.0) as u8;
            c.set(MARGIN_L + ox, MARGIN_T + oy, [v, v, v]);
        }
    }
    // Label layers: a light fill and a solid outline.
    if let Some(slice) = labels.and_then(|l| l.slice(z)) {
        let l = labels.unwrap();
        let k = |ox: usize, oy: usize| {
            let (x, y) = at(ox, oy);
            slice[y.min(h - 1) * w + x.min(w - 1)]
        };
        for oy in 0..oh {
            for ox in 0..ow {
                let v = k(ox, oy);
                if v == 0 {
                    continue;
                }
                let edge = ox == 0 || oy == 0 || ox + 1 == ow || oy + 1 == oh || k(ox - 1, oy) != v || k(ox + 1, oy) != v || k(ox, oy - 1) != v || k(ox, oy + 1) != v;
                c.blend(MARGIN_L + ox, MARGIN_T + oy, color_of(l, v), if edge { 0.95 } else { 0.25 });
            }
        }
    }
    // Grid and numbers.
    let step = grid_step(rw.max(rh) as f64);
    let first = |a: usize| a.div_ceil(step) * step;
    let mut gx = first(rx);
    while gx < rx + rw {
        let ox = ((gx - rx) as f64 * scale) as usize;
        for oy in 0..oh {
            c.blend(MARGIN_L + ox, MARGIN_T + oy, [90, 200, 255], 0.35);
        }
        c.number(MARGIN_L + ox + 1, 2, gx, [90, 200, 255]);
        gx += step;
    }
    let mut gy = first(ry);
    while gy < ry + rh {
        let oy = ((gy - ry) as f64 * scale) as usize;
        for ox in 0..ow {
            c.blend(MARGIN_L + ox, MARGIN_T + oy, [90, 200, 255], 0.35);
        }
        c.number(1, MARGIN_T + oy + 1, gy, [90, 200, 255]);
        gy += step;
    }
    let mut png = Vec::new();
    {
        use image::ImageEncoder;
        let raw: Vec<u8> = c.px.iter().flatten().copied().collect();
        image::codecs::png::PngEncoder::new(&mut png).write_image(&raw, c.w as u32, c.h as u32, image::ExtendedColorType::Rgb8)?;
    }
    let meta = json!({
        "z": z, "region": [rx, ry, rw, rh], "scale": (scale * 1000.0).round() / 1000.0, "gridStep": step,
        "imageSize": [w, h, info.size_z],
        "howToRead": "blue numbers are image pixels (x along the top, y down the left); a point at picture pixel (px, py) is image x = region[0] + (px - 34) / scale, y = region[1] + (py - 16) / scale",
    });
    Ok((png, meta))
}

/// Where a picture for an agent goes: a temporary folder the tools read from.
pub fn out_path(stem: &str, z: usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("tifmrcview-agent");
    let _ = std::fs::create_dir_all(&dir);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    dir.join(format!("look-{stem}-z{z}-{stamp}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_steps_are_round() {
        assert_eq!(grid_step(160.0), 20);
        assert_eq!(grid_step(4096.0), 1000);
        assert_eq!(grid_step(8.0), 1);
        assert_eq!(grid_step(900.0), 200);
    }
}
