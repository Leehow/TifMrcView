//! Minimal NIfTI-1 reader (.nii, .nii.gz) for training patches: 3D scalar
//! volumes, X fastest.

use anyhow::{bail, Result};
use std::io::Read;
use std::path::Path;

pub struct Volume {
    pub n: [usize; 3],
    pub v: Vec<f32>,
    /// Voxel size from the header (pixdim 1–3).
    pub spacing: [f32; 3],
}

pub fn read(path: &Path) -> Result<Volume> {
    let raw = std::fs::read(path)?;
    let bytes = if raw.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::with_capacity(raw.len() * 3);
        flate2::read::MultiGzDecoder::new(&raw[..]).read_to_end(&mut out)?;
        out
    } else {
        raw
    };
    parse(&bytes)
}

pub fn parse(b: &[u8]) -> Result<Volume> {
    if b.len() < 352 {
        bail!("NIfTI 文件太短");
    }
    let le = i32::from_le_bytes(b[0..4].try_into().unwrap()) == 348;
    if !le && i32::from_be_bytes(b[0..4].try_into().unwrap()) != 348 {
        bail!("不是 NIfTI-1 文件");
    }
    let i16_at = |o: usize| {
        let a = [b[o], b[o + 1]];
        if le { i16::from_le_bytes(a) } else { i16::from_be_bytes(a) }
    };
    let f32_at = |o: usize| {
        let a: [u8; 4] = b[o..o + 4].try_into().unwrap();
        if le { f32::from_le_bytes(a) } else { f32::from_be_bytes(a) }
    };
    let rank = i16_at(40).clamp(1, 7) as usize;
    let mut n = [1usize; 3];
    for (a, k) in n.iter_mut().enumerate().take(rank.min(3)) {
        *k = i16_at(42 + 2 * a).max(1) as usize;
    }
    for a in 3..rank {
        if i16_at(42 + 2 * a) > 1 {
            bail!("只支持三维单通道 NIfTI");
        }
    }
    let dtype = i16_at(70);
    let off = (f32_at(108) as usize).max(352);
    let (slope, inter) = (f32_at(112), f32_at(116));
    let spacing = [f32_at(80), f32_at(84), f32_at(88)];
    let count = n[0] * n[1] * n[2];
    let size = match dtype {
        2 | 256 => 1,
        4 | 512 => 2,
        8 | 16 | 768 => 4,
        64 => 8,
        _ => bail!("不支持的 NIfTI 数据类型 {dtype}"),
    };
    let data = b.get(off..off + count * size).ok_or_else(|| anyhow::anyhow!("NIfTI 数据不完整"))?;
    let w = |c: &[u8]| -> [u8; 8] {
        let mut a = [0u8; 8];
        if le {
            a[..c.len()].copy_from_slice(c);
        } else {
            for (i, x) in c.iter().rev().enumerate() {
                a[i] = *x;
            }
        }
        a
    };
    let mut v: Vec<f32> = data
        .chunks_exact(size)
        .map(|c| {
            let a = w(c);
            match dtype {
                2 => a[0] as f32,
                256 => a[0] as i8 as f32,
                4 => i16::from_le_bytes([a[0], a[1]]) as f32,
                512 => u16::from_le_bytes([a[0], a[1]]) as f32,
                8 => i32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f32,
                768 => u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f32,
                16 => f32::from_le_bytes([a[0], a[1], a[2], a[3]]),
                _ => f64::from_le_bytes(a) as f32,
            }
        })
        .collect();
    if slope != 0.0 && slope.is_finite() && (slope != 1.0 || inter != 0.0) {
        v.iter_mut().for_each(|x| *x = *x * slope + inter);
    }
    Ok(Volume { n, v, spacing })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_u8() {
        let mut b = vec![0u8; 352 + 24];
        b[0..4].copy_from_slice(&348i32.to_le_bytes());
        b[40..42].copy_from_slice(&3i16.to_le_bytes());
        for (a, k) in [2i16, 3, 4].iter().enumerate() {
            b[42 + 2 * a..44 + 2 * a].copy_from_slice(&k.to_le_bytes());
        }
        b[70..72].copy_from_slice(&2i16.to_le_bytes());
        b[108..112].copy_from_slice(&352f32.to_le_bytes());
        for i in 0..24 {
            b[352 + i] = i as u8;
        }
        let v = parse(&b).unwrap();
        assert_eq!(v.n, [2, 3, 4]);
        assert_eq!(v.v[23], 23.0);
    }
}
