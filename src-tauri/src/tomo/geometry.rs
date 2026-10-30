//! SerialEM metadata and the single-axis coordinate convention. Image X/Y
//! are array coordinates; positive axis angles are counterclockwise. The axis is
//! alpha CCW from X in the displayed image (Y points down in arrays), so
//! the array projection rotation is phi = 90 - alpha degrees.
//! SerialEM header tilt-axis angles are converted using the owner's
//! RotationAngle = 180 - SerialEM convention, then alpha = 90 - RotationAngle.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub angles: Vec<f64>,
    pub axis: Option<f64>,
    pub pixel_a: Option<f64>,
    pub doses: Vec<Option<f64>>,
}

pub fn parse_mdoc(text: &str) -> Result<Metadata> {
    let mut m = Metadata::default();
    let mut sections: Vec<(usize, Option<f64>, Option<f64>)> = Vec::new();
    let mut current = None;
    for line in text.lines().map(str::trim) {
        if m.axis.is_none() {
            if let Some((_, tail)) = line.split_once("Tilt axis angle") {
                if let Some((_, v)) = tail.split_once('=') {
                    if let Some(a) = v
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.trim_end_matches([']', ',']).parse::<f64>().ok())
                    {
                        m.axis = Some(a - 90.0);
                    }
                }
            }
        }
        if line.starts_with('[') {
            current = None;
            if let Some(v) = line
                .strip_prefix("[ZValue")
                .and_then(|s| s.split_once('=').map(|(_, v)| v))
            {
                let z = v.trim().trim_end_matches(']').trim().parse::<usize>()?;
                sections.push((z, None, None));
                current = Some(sections.len() - 1);
            }
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let v = val.trim().parse::<f64>().ok().filter(|v| v.is_finite());
        match key.trim() {
            "TiltAngle" => {
                if let Some(i) = current {
                    sections[i].1 = v;
                }
            }
            "ExposureDose" => {
                if let Some(i) = current {
                    sections[i].2 = v;
                }
            }
            "PixelSpacing" if m.pixel_a.is_none() => m.pixel_a = v.filter(|v| *v > 0.0),
            _ => {}
        }
    }
    sections.sort_by_key(|s| s.0);
    for (i, (z, angle, dose)) in sections.into_iter().enumerate() {
        if i != z {
            bail!("mdoc ZValue 必须连续且从 0 开始");
        }
        m.angles
            .push(angle.ok_or_else(|| anyhow::anyhow!("mdoc 缺少 TiltAngle: {z}"))?);
        m.doses.push(dose);
    }
    if m.axis.is_some_and(|v| !v.is_finite()) {
        bail!("mdoc 倾转轴角无效");
    }
    Ok(m)
}

pub fn metadata(path: &str, nz: usize) -> Result<Metadata> {
    let p = Path::new(path);
    let mut m = Metadata::default();
    for f in [
        p.with_extension("mdoc"),
        std::path::PathBuf::from(format!("{path}.mdoc")),
    ] {
        if f.exists() {
            m = parse_mdoc(&std::fs::read_to_string(f)?)?;
            break;
        }
    }
    if let Some(a) = crate::ctf::tilt_angles(path, nz) {
        m.angles = a;
    }
    if !m.angles.is_empty() && m.angles.len() != nz {
        bail!("倾转角数与叠层不一致");
    }
    Ok(m)
}

pub fn rotation(axis: f64) -> [[f64; 2]; 2] {
    let (s, c) = (90.0 - axis).to_radians().sin_cos();
    [[c, -s], [s, c]]
}

pub fn projection(angle: f64, axis: f64) -> [[f64; 3]; 2] {
    let (s, c) = angle.to_radians().sin_cos();
    let r = rotation(axis);
    [
        [r[0][0] * c, r[0][1], r[0][0] * s],
        [r[1][0] * c, r[1][1], r[1][0] * s],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mdoc_sections_and_header() {
        let m = parse_mdoc("PixelSpacing = 2.276\n[T = SerialEM: Tilt axis angle = 174.6, binning = 1]\n[ZValue = 1]\nTiltAngle = -3\nExposureDose = 2.1\n[ZValue = 0]\nTiltAngle = 3\nExposureDose = 2\n").unwrap();
        assert_eq!(m.angles, [3.0, -3.0]);
        assert_eq!(m.doses, [Some(2.0), Some(2.1)]);
        assert_eq!(m.pixel_a, Some(2.276));
        assert!((m.axis.unwrap() - 84.6).abs() < 1e-9);
    }
}
