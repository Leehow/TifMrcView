//! Projection matching: the alignment refined against the tomogram itself,
//! as Warp and AreTomo do after a first reconstruction. Each view is left
//! out, the rest reconstructed (quick WBP) and projected along it; the
//! view's shift to that projection, measured on squares across the view
//! and taken as their median, is added to its alignment. Rounds repeat
//! until the views move less than `STILL` pixels.

use super::{check_cancel, output, pipeline, quality, recon, Alignment};
use anyhow::Result;

/// Views moving less than this (binned pixels, median) end the rounds.
const STILL: f64 = 0.15;

#[derive(Clone, Debug)]
pub struct Refined {
    /// Median shift per round, binned pixels.
    pub rounds: Vec<f64>,
}

/// Shift (binned pixels) by which `view` sits from `proj`: view(x) ≈ proj(x − d),
/// the median over squares along the longer side.
pub fn offset(proj: &[f32], view: &[f32], w: usize, h: usize) -> Option<[f64; 2]> {
    let n = 1usize << w.min(h).ilog2();
    if n < 32 {
        return None;
    }
    let (long, nx) = if w >= h { (w, w / n) } else { (h, h / n) };
    let count = nx.max(1);
    let mut d: Vec<[f64; 2]> = Vec::new();
    for i in 0..count {
        let at = (long - n) * i / (count - 1).max(1);
        let (x0, y0) = if w >= h { (at, (h - n) / 2) } else { ((w - n) / 2, at) };
        let cut = |v: &[f32]| -> Vec<f32> { (0..n * n).map(|k| v[(y0 + k / n) * w + x0 + k % n]).collect() };
        let (s, cc) = crate::align::patch_shift(&cut(proj), &cut(view), n, n as f64 / 4.0);
        if cc.is_finite() && cc > 0.02 {
            d.push(s);
        }
    }
    if d.is_empty() {
        return None;
    }
    let med = |k: usize| {
        let mut v: Vec<f64> = d.iter().map(|s| s[k]).collect();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    Some([med(0), med(1)])
}

/// Refines the shifts of `a` by projection matching at `bin`, with a
/// tomogram `thickness` sections thick (output pixels at `bin`) at `z_offset`.
pub fn refine(
    img: &std::sync::Arc<crate::image::OpenImage>,
    a: &mut Alignment,
    bin: usize,
    thickness: usize,
    z_offset: f64,
    rounds: usize,
    progress: &(dyn Fn(&str, usize, usize) + Sync),
) -> Result<Refined> {
    let params = recon::Params { thickness, z_offset, ..Default::default() };
    let mut done = Refined { rounds: vec![] };
    for round in 0..rounds {
        let kept: Vec<usize> = (0..a.views.len()).filter(|&i| !a.views[i].excluded).collect();
        let (stack, w, h, _) = output::aligned(img, a, bin, 0, 0, &|_, _| {})?;
        let mut moves: Vec<(usize, [f64; 2])> = Vec::new();
        for (j, &k) in kept.iter().enumerate() {
            check_cancel()?;
            progress(&format!("精修对齐 第 {}/{} 轮", round + 1, rounds), j, kept.len());
            let mut rest = a.clone();
            rest.views[k].excluded = true;
            let others: Vec<f32> = kept.iter().enumerate().filter(|(i, _)| *i != j).flat_map(|(i, _)| stack[i * w * h..(i + 1) * w * h].iter().copied()).collect();
            let angles: Vec<f64> = rest.views.iter().filter(|v| !v.excluded).map(|v| v.refined()).collect();
            let how = pipeline::Build { alignment: &rest, bin, params: &params, ctf: None, ctf_3d: false };
            let vol = pipeline::volume(&how, others, w, h, &angles)?;
            let proj = quality::project(&vol, w, h, thickness, a.views[k].refined(), z_offset)?;
            if let Some(d) = offset(&proj, &stack[j * w * h..(j + 1) * w * h], w, h) {
                moves.push((k, d));
            }
        }
        // A common move of all views is a shift of the whole tomogram, not an error.
        let mean: [f64; 2] = std::array::from_fn(|c| moves.iter().map(|m| m.1[c]).sum::<f64>() / moves.len().max(1) as f64);
        let mut sizes: Vec<f64> = Vec::new();
        for (k, d) in moves {
            let d = [d[0] - mean[0], d[1] - mean[1]];
            sizes.push(d[0].hypot(d[1]));
            let v = &mut a.views[k];
            let r = super::geometry::rotation(a.axis + v.rotation);
            let (u, w2) = (d[0] * bin as f64, d[1] * bin as f64);
            v.shift[0] += v.mag * (r[0][0] * u + r[0][1] * w2);
            v.shift[1] += v.mag * (r[1][0] * u + r[1][1] * w2);
        }
        sizes.sort_by(f64::total_cmp);
        let median = sizes.get(sizes.len() / 2).copied().unwrap_or(0.0);
        done.rounds.push(median);
        if median < STILL {
            break;
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    #[test]
    fn offset_reads_a_known_shift() {
        let (w, h) = (192usize, 64usize);
        let mut seed = 3u64;
        let base: Vec<f32> = (0..w * h).map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) % 1000) as f32 / 1000.0
        }).collect();
        // A blurred random image, and the same moved by (+3, −2): view(x) = proj(x − d).
        let blur = |x: isize, y: isize| -> f32 {
            let mut s = 0.0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (xx, yy) = ((x + dx).rem_euclid(w as isize) as usize, (y + dy).rem_euclid(h as isize) as usize);
                    s += base[yy * w + xx];
                }
            }
            s
        };
        let proj: Vec<f32> = (0..w * h).map(|i| blur((i % w) as isize, (i / w) as isize)).collect();
        let view: Vec<f32> = (0..w * h).map(|i| blur((i % w) as isize - 3, (i / w) as isize + 2)).collect();
        let d = super::offset(&proj, &view, w, h).unwrap();
        assert!((d[0] - 3.0).abs() < 0.3 && (d[1] + 2.0).abs() < 0.3, "{d:?}");
    }
}
