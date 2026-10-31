//! Shared EER event iterator (native Pro path and wasm Thermo plugin).
//! Runs and subpixel codes are packed LSB-first. The all-ones run has no
//! subpixel code/event. The callback receives detector index and raw codes;
//! physical-resolution rendering intentionally ignores subpixel positions.

pub fn events(
    src: &[u8],
    pixels: usize,
    bits: (u32, u32, u32),
    mut emit: impl FnMut(usize, u32, u32),
) -> Result<(), &'static str> {
    let (skip, h, v) = bits;
    if !(4..=16).contains(&skip) || h > 4 || v > 4 || skip + h + v > 24 {
        return Err("EER 位宽无效");
    }
    let mask = (1u32 << skip) - 1;
    let mut at = 0usize;
    let mut pixel = 0usize;
    let read = |at: usize, n: u32| {
        let byte = at / 8;
        let mut word = [0u8; 4];
        let len = (src.len() - byte).min(4);
        word[..len].copy_from_slice(&src[byte..byte + len]);
        (u32::from_le_bytes(word) >> (at % 8)) & ((1u32 << n) - 1)
    };
    while at + skip as usize <= src.len() * 8 {
        let run = read(at, skip);
        at += skip as usize;
        pixel = pixel.checked_add(run as usize).ok_or("EER 事件溢出")?;
        if pixel >= pixels {
            return if pixel == pixels {
                Ok(())
            } else {
                Err("EER 事件超出图像")
            };
        }
        if run == mask {
            continue;
        }
        if at + (h + v) as usize > src.len() * 8 {
            return Err("EER 亚像素数据截断");
        }
        let sx = read(at, h);
        at += h as usize;
        let sy = read(at, v);
        at += v as usize;
        emit(pixel, sx, sy);
        pixel += 1;
    }
    Ok(())
}

/// EER's high subpixel bit is inverted on each axis. At 2x, use the high
/// bit of the corrected 4x position, discarding the low localization bit.
#[allow(dead_code)] // wasm display is physical-resolution; native Pro uses 2x.
pub fn subpixel_2x(raw: u32, bits: u32) -> usize {
    if bits == 0 {
        0
    } else {
        ((raw ^ (1 << (bits - 1))) >> (bits - 1)) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runs_and_subpixels() {
        let codes = [(3u32, 5u32), (0, 0), (127, 0), (5, 15), (62, 0)];
        let mut src = vec![0u8; 16];
        let mut at = 0;
        for (run, sub) in codes {
            for k in 0..7 {
                src[at / 8] |= (((run >> k) & 1) as u8) << (at % 8);
                at += 1;
            }
            if run != 127 {
                for k in 0..4 {
                    src[at / 8] |= (((sub >> k) & 1) as u8) << (at % 8);
                    at += 1;
                }
            }
        }
        let mut hits = vec![];
        events(&src, 200, (7, 2, 2), |p, x, y| hits.push((p, x, y))).unwrap();
        assert_eq!(hits, [(3, 1, 1), (4, 0, 0), (137, 3, 3)]);
        assert!(events(&src, 10, (7, 2, 2), |_, _, _| {}).is_err());
        assert_eq!((subpixel_2x(0, 2), subpixel_2x(3, 2)), (1, 0));
    }
}
