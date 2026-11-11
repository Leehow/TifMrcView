//! "Use a map I already have": a density map the user brings (an average
//! from earlier work, a map from a data bank) becomes a card like the ones
//! made from the tomograms, so that everything after it is the ordinary
//! flow: find all like it, show them for checking, average.
//!
//! The map may be any size and sampling, bright or dark, centred or not.
//! It is turned into what a card is: a `BOX`³ cube with the particle dark,
//! at the centre, spanning `SPAN` voxels, at the voxel size the tomograms
//! are binned to for a particle of its size.
//!
//! Maps are read as MRC and as EM (the format of TOM and Dynamo: a 512-byte
//! header, then the values). An EM file does not say its voxel size.

use std::path::Path;

use anyhow::{bail, Context, Result};

use super::filaments::blur;
use super::project::{is_likely, shape, Card, BOX, LADDER, SPAN};
use crate::tomo::{output, pipeline};

/// The particle is what lies this far (as a share of the strongest value)
/// from the background.
const PART: f32 = 0.3;
/// Its extent is the ball about its middle that holds this share of it.
const HELD: f64 = 0.97;
/// A card's size is about this share of the particle's full extent: the
/// sizes of the ladder are those of the blobs the particles show as, which
/// read smaller than the particles are (a 280 Å ribosome makes the 210 Å
/// card), and the search mask reaches 1.6 sizes across.
const FILL: f64 = 0.75;
/// The largest particle a card can show is the last ladder size times this.
const OVER: f64 = 1.3;

/// A map: values in z, y, x order, its size (x, y, z) and voxel size, Å
/// (None when the file does not say).
pub struct Map {
    pub v: Vec<f32>,
    pub n: [usize; 3],
    pub voxel_a: Option<f64>,
}

/// Reads an EM file.
pub fn read_em(path: &str) -> Result<Map> {
    let bytes = std::fs::read(path).with_context(|| format!("无法读取 {path}"))?;
    if bytes.len() < 512 {
        bail!("不是 EM 文件：{path}");
    }
    // Byte 0 names the machine: 6 is little-endian (PC); 0, 3 and 5 are
    // big-endian ones. Byte 3 names the value type.
    let little = match bytes[0] {
        6 | 1 | 2 => true,
        0 | 3 | 4 | 5 => false,
        _ => bail!("不是 EM 文件：{path}"),
    };
    let int = |at: usize| {
        let b: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
        if little { i32::from_le_bytes(b) } else { i32::from_be_bytes(b) }
    };
    let n = [int(4), int(8), int(12)];
    if n.iter().any(|&x| x <= 0 || x > 4096) {
        bail!("不是 EM 文件：{path}");
    }
    let n = n.map(|x| x as usize);
    let count = n[0] * n[1] * n[2];
    let width = match bytes[3] {
        1 => 1,
        2 => 2,
        4 | 5 => 4,
        9 => 8,
        t => bail!("EM 文件的数值类型 {t} 读不了：{path}"),
    };
    let data = &bytes[512..];
    if data.len() < count * width {
        bail!("EM 文件不完整：{path}");
    }
    let v: Vec<f32> = data[..count * width]
        .chunks_exact(width)
        .map(|c| match (bytes[3], little) {
            (1, _) => c[0] as i8 as f32,
            (2, true) => i16::from_le_bytes([c[0], c[1]]) as f32,
            (2, false) => i16::from_be_bytes([c[0], c[1]]) as f32,
            (4, true) => i32::from_le_bytes(c.try_into().unwrap()) as f32,
            (4, false) => i32::from_be_bytes(c.try_into().unwrap()) as f32,
            (5, true) => f32::from_le_bytes(c.try_into().unwrap()),
            (5, false) => f32::from_be_bytes(c.try_into().unwrap()),
            (_, true) => f64::from_le_bytes(c.try_into().unwrap()) as f32,
            (_, false) => f64::from_be_bytes(c.try_into().unwrap()) as f32,
        })
        .collect();
    Ok(Map { v, n, voxel_a: None })
}

/// Reads a map: EM by its extension, anything else as the tomograms are read.
pub fn read(path: &str) -> Result<Map> {
    if Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("em")) {
        return read_em(path);
    }
    let img = pipeline::open(path)?;
    let known = img.info().pixel_size.is_some();
    let px = output::pixel_a(&img);
    drop(img);
    let (v, n, _) = pipeline::load(path)?;
    Ok(Map { v, n, voxel_a: (known && px > 0.0).then_some(px) })
}

/// What `cube_of` gives: the cube and what it shows.
pub struct Fitted {
    /// `BOX`³, the particle dark on a background of zero.
    pub cube: Vec<f32>,
    /// Voxel size of the cube, Å.
    pub voxel_a: f64,
    /// The ladder size the particle is taken as, Å.
    pub diameter_a: f64,
    /// The particle's full extent measured on the map, Å.
    pub measured_a: f64,
    /// The map showed the particle bright and was turned over.
    pub inverted: bool,
}

/// The map as a card's cube. `voxel_a`: the map's voxel size, Å;
/// `tomogram_a`: that of the tomograms it will be looked for in (the cube's
/// voxel is a whole multiple of it, as the tomograms are binned).
pub fn cube_of(map: &[f32], n: [usize; 3], voxel_a: f64, tomogram_a: f64) -> Result<Fitted> {
    let count = n[0] * n[1] * n[2];
    if count == 0 || map.len() != count || n.iter().any(|&x| x < 8) {
        bail!("这张图太小，不能当模板");
    }
    let at = |i: usize| [(i % n[0]) as f64, (i / n[0] % n[1]) as f64, (i / (n[0] * n[1])) as f64];
    let mid = [(n[0] as f64 - 1.0) / 2.0, (n[1] as f64 - 1.0) / 2.0, (n[2] as f64 - 1.0) / 2.0];
    // The background is what the outside of the map holds: beyond the
    // largest ball that fits.
    let reach = mid.iter().copied().fold(f64::INFINITY, f64::min);
    let mut outside: Vec<f32> = (0..count).filter(|&i| (0..3).map(|a| (at(i)[a] - mid[a]).powi(2)).sum::<f64>().sqrt() > 0.9 * reach).map(|i| map[i]).collect();
    if outside.is_empty() {
        bail!("这张图太小，不能当模板");
    }
    let k = outside.len() / 2;
    let background = *outside.select_nth_unstable_by(k, f32::total_cmp).1;
    // A little smoothing so that single noisy voxels do not decide.
    let smooth = blur(map, n, 1.0);
    // Bright or dark: the side on which the strongest values lie.
    let (low, high) = smooth.iter().fold((0f32, 0f32), |(l, h), &x| (l.min(x - background), h.max(x - background)));
    let inverted = high > -low;
    let sign = if inverted { -1.0 } else { 1.0 };
    // Darkness: how far below the background, where clearly so.
    let most = if inverted { high } else { -low };
    if most <= 0.0 {
        bail!("这张图里看不出颗粒");
    }
    let dark = |i: usize| {
        let d = -sign * (smooth[i] - background);
        if d > PART * most { d as f64 } else { 0.0 }
    };
    let (mut sum, mut c) = (0.0, [0.0; 3]);
    for i in 0..count {
        let d = dark(i);
        if d > 0.0 {
            sum += d;
            let p = at(i);
            for a in 0..3 {
                c[a] += d * p[a];
            }
        }
    }
    let c = c.map(|x| x / sum);
    // The ball about the middle that holds most of the particle.
    let mut far: Vec<(f64, f64)> = (0..count).filter_map(|i| (dark(i) > 0.0).then(|| ((0..3).map(|a| (at(i)[a] - c[a]).powi(2)).sum::<f64>().sqrt(), dark(i)))).collect();
    far.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut held = 0.0;
    let mut radius = 0.0;
    for (r, d) in far {
        held += d;
        radius = r;
        if held >= HELD * sum {
            break;
        }
    }
    let measured_a = (2.0 * radius + 1.0) * voxel_a;
    let largest = LADDER[LADDER.len() - 1];
    if FILL * measured_a > OVER * largest {
        bail!("图里的颗粒约 {:.0} Å，太大了（最大约 {:.0} Å）；如果体素大小不对，请先改正", measured_a, largest);
    }
    let diameter_a = *LADDER.iter().min_by(|a, b| (*a - FILL * measured_a).abs().total_cmp(&(*b - FILL * measured_a).abs())).unwrap();
    // The voxel the tomograms are binned to for this size.
    let bin = ((diameter_a / SPAN / tomogram_a).round()).max(1.0);
    let cube_a = bin * tomogram_a;
    // Map voxels per cube voxel; smooth first when the cube is coarser.
    let step = cube_a / voxel_a;
    let source = if step > 1.0 { blur(map, n, 0.4 * step) } else { map.to_vec() };
    let value = |p: [f64; 3]| -> f32 {
        let f = p.map(|x| x.floor());
        let mut out = 0.0;
        for k in 0..8 {
            let q = [f[0] as isize + (k & 1) as isize, f[1] as isize + (k >> 1 & 1) as isize, f[2] as isize + (k >> 2) as isize];
            let w: f64 = (0..3).map(|a| 1.0 - (p[a] - q[a] as f64).abs()).product();
            let inside = (0..3).all(|a| q[a] >= 0 && q[a] < n[a] as isize);
            let x = if inside { source[(q[2] as usize * n[1] + q[1] as usize) * n[0] + q[0] as usize] - background } else { 0.0 };
            out += w * x as f64;
        }
        out as f32
    };
    let half = (BOX / 2) as f64;
    let mut cube = vec![0f32; BOX * BOX * BOX];
    for z in 0..BOX {
        for y in 0..BOX {
            for x in 0..BOX {
                let p = [c[0] + (x as f64 - half) * step, c[1] + (y as f64 - half) * step, c[2] + (z as f64 - half) * step];
                cube[(z * BOX + y) * BOX + x] = sign * value(p);
            }
        }
    }
    Ok(Fitted { cube, voxel_a: cube_a, diameter_a, measured_a, inverted })
}

/// The card of a map the user brings, written into `out_dir` as `card.mrc`
/// and `card.json` (where `project::card_from_points` writes its card).
/// `tomogram_a`: the voxel size of the tomograms, Å; a map that does not say
/// its own voxel size is taken to have that one too (`assumed` comes back
/// true, for the user to be told).
pub fn card_from_map(path: &str, tomogram_a: f64, out_dir: &str) -> Result<(Card, bool)> {
    let map = read(path)?;
    let assumed = map.voxel_a.is_none();
    let fitted = cube_of(&map.v, map.n, map.voxel_a.unwrap_or(tomogram_a), tomogram_a)?;
    let ball = 0.6 * fitted.diameter_a / fitted.voxel_a;
    let (compact, elongation, blobs, offset) = shape(&fitted.cube, BOX, ball);
    let mut card = Card {
        id: 0,
        diameter_a: fitted.diameter_a,
        measured_a: fitted.measured_a,
        voxel_a: fitted.voxel_a,
        members: 0,
        share: 0.0,
        path: String::new(),
        compact,
        elongation,
        blobs,
        offset,
        likely: is_likely(compact, elongation, blobs, offset),
    };
    std::fs::create_dir_all(out_dir).with_context(|| format!("无法创建 {out_dir}"))?;
    card.path = Path::new(out_dir).join("card.mrc").to_string_lossy().into_owned();
    output::write_mrc(&card.path, &fitted.cube, [BOX; 3], card.voxel_a, false)?;
    std::fs::write(Path::new(out_dir).join("card.json"), serde_json::to_string_pretty(&card)?)?;
    Ok((card, assumed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: [usize; 3] = [72, 64, 60];

    /// Three balls of unequal size (about 200 Å across at 5 Å a voxel), a
    /// few voxels off the middle of the map, `value` against zero.
    fn map(value: f32) -> Vec<f32> {
        let balls = [([40.0, 34.0, 31.0], 11.0), ([28.0, 30.0, 27.0], 8.0), ([38.0, 22.0, 36.0], 6.0)];
        let mut v = vec![0f32; N[0] * N[1] * N[2]];
        for z in 0..N[2] {
            for y in 0..N[1] {
                for x in 0..N[0] {
                    let p = [x as f64, y as f64, z as f64];
                    let d: f64 = balls.iter().map(|(c, r): &([f64; 3], f64)| (r - (0..3).map(|a| (p[a] - c[a]).powi(2)).sum::<f64>().sqrt()).clamp(0.0, 1.0)).fold(0.0, f64::max);
                    v[(z * N[1] + y) * N[0] + x] = value * d as f32;
                }
            }
        }
        v
    }

    fn corr(a: &[f32], b: &[f32]) -> f64 {
        let (ma, mb) = (a.iter().sum::<f32>() / a.len() as f32, b.iter().sum::<f32>() / b.len() as f32);
        let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
        for (x, y) in a.iter().zip(b) {
            let (x, y) = ((x - ma) as f64, (y - mb) as f64);
            ab += x * y;
            aa += x * x;
            bb += y * y;
        }
        ab / (aa * bb).sqrt()
    }

    #[test]
    fn a_bright_map_becomes_a_dark_centred_cube_of_its_size() {
        let fitted = cube_of(&map(1.0), N, 5.0, 4.0).unwrap();
        assert!(fitted.inverted);
        // About 34 voxels across at 5 Å: the 120 Å card, binned 2×.
        assert!((150.0..=200.0).contains(&fitted.measured_a), "{}", fitted.measured_a);
        assert_eq!(fitted.diameter_a, 120.0);
        assert_eq!(fitted.voxel_a, 8.0);
        // Dark, and its weight at the middle of the cube.
        let (mut sum, mut c) = (0.0, [0.0; 3]);
        for (i, &x) in fitted.cube.iter().enumerate() {
            assert!(x <= 1e-3);
            let d = -x as f64;
            sum += d;
            for (a, p) in [i % BOX, i / BOX % BOX, i / (BOX * BOX)].into_iter().enumerate() {
                c[a] += d * p as f64;
            }
        }
        for a in 0..3 {
            assert!((c[a] / sum - (BOX / 2) as f64).abs() < 0.6, "{:?}", c.map(|x| x / sum));
        }
        // The same map dark gives the same cube.
        let dark = cube_of(&map(-1.0), N, 5.0, 4.0).unwrap();
        assert!(!dark.inverted);
        assert!(corr(&dark.cube, &fitted.cube) > 0.999);
    }

    #[test]
    fn a_map_too_large_or_empty_is_refused() {
        assert!(cube_of(&map(1.0), N, 40.0, 4.0).is_err());
        assert!(cube_of(&vec![0.0; N[0] * N[1] * N[2]], N, 5.0, 4.0).is_err());
    }

    #[test]
    fn em_files_are_read_and_take_the_tomogram_voxel() {
        let dir = tempfile::tempdir().unwrap();
        let v = map(-1.0);
        let mut bytes = vec![0u8; 512];
        bytes[0] = 6;
        bytes[3] = 5;
        for (k, n) in N.iter().enumerate() {
            bytes[4 + 4 * k..8 + 4 * k].copy_from_slice(&(*n as i32).to_le_bytes());
        }
        v.iter().for_each(|x| bytes.extend_from_slice(&x.to_le_bytes()));
        let path = dir.path().join("template.em");
        std::fs::write(&path, bytes).unwrap();
        let read = read(path.to_str().unwrap()).unwrap();
        assert_eq!((read.n, read.voxel_a), (N, None));
        assert_eq!(read.v, v);
        let out = dir.path().join("own");
        let (card, assumed) = card_from_map(path.to_str().unwrap(), 5.0, out.to_str().unwrap()).unwrap();
        assert!(assumed);
        assert_eq!((card.diameter_a, card.voxel_a), (120.0, 10.0));
        let (cube, voxel) = crate::sta::project::read_card(&card.path).unwrap();
        assert_eq!((cube.len(), voxel), (BOX * BOX * BOX, 10.0));
    }
}
