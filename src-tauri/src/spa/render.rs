//! PNG previews for the workspace: a micrograph with its picks circled,
//! and the 2D class averages as a gallery.

use anyhow::Result;

use super::pick::{bin, gauss, robust};

fn png(gray: &[u8], rgb_marks: &[(usize, u8, u8, u8)], w: usize, h: usize) -> Result<Vec<u8>> {
    let mut rgb: Vec<u8> = gray.iter().flat_map(|&g| [g, g, g]).collect();
    for &(i, r, g, b) in rgb_marks {
        rgb[3 * i] = r;
        rgb[3 * i + 1] = g;
        rgb[3 * i + 2] = b;
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_raw(w as u32, h as u32, rgb)
        .ok_or_else(|| anyhow::anyhow!("png"))?
        .write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

fn to_gray(v: &[f32]) -> Vec<u8> {
    let (m, s) = robust(v);
    v.iter()
        .map(|&x| (((x - m) / s / 6.0 + 0.5) * 255.0).clamp(0.0, 255.0) as u8)
        .collect()
}

/// The micrograph binned to at most `max_side` pixels (lightly smoothed),
/// with a circle of `radius_px` (micrograph pixels) at each pick.
pub fn mic_preview(
    img: &[f32],
    w: usize,
    h: usize,
    picks: &[[f32; 3]],
    radius_px: f64,
    max_side: usize,
) -> Result<(Vec<u8>, f64)> {
    let f = w.max(h).div_ceil(max_side.max(64)).max(1);
    let (b, bw, bh) = bin(img, w, h, f);
    let b = gauss(&b, bw, bh, 1.0);
    let gray = to_gray(&b);
    let mut marks = vec![];
    let r = radius_px / f as f64;
    for q in picks {
        let (cx, cy) = (q[0] as f64 / f as f64, q[1] as f64 / f as f64);
        let steps = (2.0 * std::f64::consts::PI * r).ceil().max(12.0) as usize * 2;
        for s in 0..steps {
            let a = s as f64 / steps as f64 * 2.0 * std::f64::consts::PI;
            for dr in [0.0, 1.0] {
                let (x, y) = (
                    (cx + (r + dr) * a.cos()).round(),
                    (cy + (r + dr) * a.sin()).round(),
                );
                if x >= 0.0 && y >= 0.0 && (x as usize) < bw && (y as usize) < bh {
                    marks.push((y as usize * bw + x as usize, 40, 220, 90));
                }
            }
        }
    }
    Ok((png(&gray, &marks, bw, bh)?, f as f64))
}

/// Class averages in a grid of `cols` columns, each scaled to `cell` pixels.
pub fn gallery(avgs: &[Vec<f32>], b: usize, cols: usize, cell: usize) -> Result<Vec<u8>> {
    let rows = avgs.len().div_ceil(cols.max(1)).max(1);
    let (w, h) = (cols * cell, rows * cell);
    let mut gray = vec![0u8; w * h];
    for (n, a) in avgs.iter().enumerate() {
        let g = to_gray(a);
        let (ox, oy) = ((n % cols) * cell, (n / cols) * cell);
        for y in 0..cell - 2 {
            for x in 0..cell - 2 {
                let (sx, sy) = (x * b / (cell - 2), y * b / (cell - 2));
                gray[(oy + y + 1) * w + ox + x + 1] = g[sy * b + sx];
            }
        }
    }
    png(&gray, &[], w, h)
}
