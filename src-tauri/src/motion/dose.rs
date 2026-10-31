//! Grant & Grigorieff, eLife 4:e06980 (2015): critical exposure in e−/Å²,
//! Ne(k)=0.245 k^(-1.665)+2.81 at 300 kV, k in Å⁻¹. The published 200 kV
//! scaling is 0.8. We reject unvalidated voltages. Integrate amplitude
//! exp[-D/(2 Ne)] over each grouped exposure, including pre-exposure.
use anyhow::{bail, Result};
pub fn critical(k: f64, kv: f64) -> Result<f64> {
    if !k.is_finite() || k < 0.0 {
        bail!("空间频率无效");
    }
    let scale = if (kv - 300.0).abs() < 1.0 {
        1.0
    } else if (kv - 200.0).abs() < 1.0 {
        0.8
    } else {
        bail!("剂量加权仅支持已验证的 200/300 kV");
    };
    Ok(if k == 0.0 {
        f64::INFINITY
    } else {
        scale * (0.245 * k.powf(-1.665) + 2.81)
    })
}
pub fn weight(k: f64, kv: f64, start: f64, dose: f64) -> Result<f64> {
    let n = critical(k, kv)?;
    if k == 0.0 {
        return Ok(1.0);
    }
    let rate = 1.0 / (2.0 * n);
    Ok((-start * rate).exp()
        * if dose > 0.0 {
            -(-dose * rate).exp_m1() / (dose * rate)
        } else {
            1.0
        })
}
