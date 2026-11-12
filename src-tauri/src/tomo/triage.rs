//! Which views of a tilt series can be used, judged before alignment from
//! what each view shows on its own: how bright it is for its tilt, whether
//! something opaque covers part of it, how far the specimen drifted while
//! it was recorded, whether its Thon rings can be seen and whether it still
//! shows the place its neighbours show. The program only proposes: `bad` is
//! its reason for leaving a view out, the user's own choice (`Dataset::picks`)
//! always wins (`excluded`).
//!
//! Brightness. Through a slab of thickness t the unscattered beam falls as
//! exp(−t / (λ cos(θ − θ₀))), θ₀ the tilt at which the slab is seen head on
//! (a lamella milled at an angle is not level at 0°). ln(mean / dose) is
//! fitted with a − b / cos(θ − θ₀): θ₀ by a search, the slope b as the
//! median of the slopes between all pairs of views (Theil–Sen, so the
//! shadowed views do not pull the line down). A view far below the line
//! is shadowed or was recorded with the beam partly off.

use super::dataset::Dataset;
use serde::{Deserialize, Serialize};

/// What one view shows on its own.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ViewCheck {
    /// Section of the stack.
    pub z: usize,
    pub angle: f64,
    /// Mean of the view (counts per pixel for a sum of counted frames).
    pub mean: f64,
    /// Fraction of the field that is opaque (`measure`).
    pub dark: f64,
    /// Dose of the view, e⁻/Å² (None: unknown, taken as equal for all).
    #[serde(default)]
    pub dose: Option<f64>,
    /// Path the specimen moved along during the exposure, Å.
    #[serde(default)]
    pub drift_a: Option<f64>,
    /// Defocus (µm) and the resolution (Å) to which the rings follow the fit.
    #[serde(default)]
    pub defocus_um: Option<f64>,
    #[serde(default)]
    pub fit_res_a: Option<f64>,
    /// Shift to the neighbouring view nearer 0°, as a fraction of the field.
    #[serde(default)]
    pub jump: Option<f64>,
    /// Brightness over what the series' other views predict for this tilt.
    #[serde(default)]
    pub relative: Option<f64>,
    /// Why the program would leave the view out (None: usable).
    #[serde(default)]
    pub bad: Option<String>,
}

/// What the program makes of the whole series.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub usable: bool,
    /// Why not, or what to look at.
    #[serde(default)]
    pub reason: Option<String>,
    /// Usable, but the user should have a look (many views left out, a poor alignment).
    #[serde(default)]
    pub attention: bool,
}

/// Blocks along each side for `measure`.
const BLOCKS: usize = 16;
/// A block darker than this fraction of the view's bright level is opaque:
/// a grid bar or the holder's edge lets nothing through, thick ice at a
/// high tilt still a tenth or more.
const OPAQUE: f64 = 0.08;
/// Opaque part of the field from which a view is left out.
const SHADOWED: f64 = 0.08;
/// Brightness relative to the series' fit below which a view is left out.
const DIM: f64 = 0.6;
/// A shift to the neighbour beyond this fraction of the field: tracking lost.
const JUMP: f64 = 0.3;

/// Mean of a view and the fraction of it that is opaque: of `BLOCKS`²
/// blocks, those below `OPAQUE` of the bright level (the 90th percentile
/// of the block means).
pub fn measure(v: &[f32], w: usize, h: usize) -> (f64, f64) {
    if w == 0 || h == 0 || v.len() != w * h {
        return (0.0, 0.0);
    }
    let n = BLOCKS.min(w).min(h);
    let mut blocks = Vec::with_capacity(n * n);
    let mut total = 0.0;
    for by in 0..n {
        for bx in 0..n {
            let (x0, x1, y0, y1) = (bx * w / n, (bx + 1) * w / n, by * h / n, (by + 1) * h / n);
            let mut sum = 0.0;
            for y in y0..y1 {
                sum += v[y * w + x0..y * w + x1].iter().map(|&x| x as f64).sum::<f64>();
            }
            total += sum;
            blocks.push(sum / ((x1 - x0) * (y1 - y0)) as f64);
        }
    }
    let mut sorted = blocks.clone();
    sorted.sort_by(f64::total_cmp);
    let bright = sorted[sorted.len() * 9 / 10];
    let dark = if bright > 0.0 { blocks.iter().filter(|&&b| b < OPAQUE * bright).count() as f64 / blocks.len() as f64 } else { 1.0 };
    (total / (w * h) as f64, dark)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() { 0.0 } else { v[v.len() / 2] }
}

/// The brightness each view should have for its tilt (see the head of this
/// file), from the views themselves; None with too few views to tell.
fn expected(angle: &[f64], brightness: &[f64]) -> Option<Vec<f64>> {
    let pts: Vec<(f64, f64)> = angle.iter().zip(brightness).filter(|(_, b)| **b > 0.0).map(|(a, b)| (*a, b.ln())).collect();
    if pts.len() < 7 {
        return None;
    }
    let mut best: Option<(f64, f64, f64, f64)> = None;
    for step in -25..=25 {
        let level = step as f64;
        let x = |a: f64| 1.0 / (a - level).to_radians().cos().max(0.05);
        let mut slopes = Vec::with_capacity(pts.len() * (pts.len() - 1) / 2);
        for i in 0..pts.len() {
            for j in i + 1..pts.len() {
                let dx = x(pts[j].0) - x(pts[i].0);
                if dx.abs() > 1e-3 {
                    slopes.push((pts[j].1 - pts[i].1) / dx);
                }
            }
        }
        if slopes.is_empty() {
            continue;
        }
        // Thicker along the beam is never brighter.
        let b = median(&mut slopes).min(0.0);
        let a = median(&mut pts.iter().map(|p| p.1 - b * x(p.0)).collect::<Vec<_>>());
        let spread = median(&mut pts.iter().map(|p| (p.1 - a - b * x(p.0)).abs()).collect::<Vec<_>>());
        // Level wins ties: a slab is level unless the views say otherwise.
        if best.is_none_or(|m| spread < m.3 - 1e-4) {
            best = Some((a, b, level, spread));
        }
    }
    let (a, b, level, _) = best?;
    Some(angle.iter().map(|&t| (a + b / (t - level).to_radians().cos().max(0.05)).exp()).collect())
}

/// Sets `bad` of every view from what was measured and says what the series
/// is good for. Views are judged on their own first, then the series by
/// what is left.
pub fn judge(checks: &mut [ViewCheck]) -> Verdict {
    let angles: Vec<f64> = checks.iter().map(|c| c.angle).collect();
    let same_dose = checks.iter().any(|c| c.dose.is_none_or(|d| d <= 0.0));
    let brightness: Vec<f64> = checks.iter().map(|c| if same_dose { c.mean } else { c.mean / c.dose.unwrap() }).collect();
    let model = expected(&angles, &brightness);
    let mut drifts: Vec<f64> = checks.iter().filter_map(|c| c.drift_a).collect();
    let drift = median(&mut drifts);
    let mut rings: Vec<f64> = checks.iter().filter(|c| c.angle.abs() <= 30.0).filter_map(|c| c.fit_res_a).filter(|r| r.is_finite()).collect();
    let ring = (rings.len() >= 5).then(|| median(&mut rings));
    for (i, c) in checks.iter_mut().enumerate() {
        c.relative = model.as_ref().map(|m| brightness[i] / m[i]).filter(|r| r.is_finite());
        c.bad = if c.mean <= 0.0 {
            Some("没有信号".into())
        } else if c.dark > SHADOWED {
            Some(format!("被挡住了 {:.0}%", c.dark * 100.0))
        } else if c.relative.is_some_and(|r| r < DIM) {
            Some(format!("太暗，只有预期亮度的 {:.0}%", c.relative.unwrap() * 100.0))
        } else if c.jump.is_some_and(|j| j > JUMP) {
            Some("和相邻的图对不上（跟踪丢了）".into())
        } else if c.drift_a.is_some_and(|d| d > (8.0 * drift).max(60.0)) {
            Some(format!("漂移太大（{:.0} Å）", c.drift_a.unwrap()))
        } else if c.angle.abs() <= 30.0 && ring.zip(c.fit_res_a).is_some_and(|(m, r)| !r.is_finite() || r > 2.5 * m.max(8.0)) {
            Some("看不到 Thon 环（可能拍糊了）".into())
        } else {
            None
        };
    }
    verdict(checks, &[])
}

/// What the series is good for with the views in `out` (sections) left out
/// on top of the program's own `bad` ones.
pub fn verdict(checks: &[ViewCheck], out: &[usize]) -> Verdict {
    let good: Vec<&ViewCheck> = checks.iter().filter(|c| c.bad.is_none() && !out.contains(&c.z)).collect();
    let n = checks.len();
    let dropped = n - good.len();
    let low: Vec<&ViewCheck> = checks.iter().filter(|c| c.angle.abs() <= 10.0).collect();
    let (lo, hi) = good.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |m, c| (m.0.min(c.angle), m.1.max(c.angle)));
    let no = |reason: String| Verdict { usable: false, reason: Some(reason), attention: false };
    if good.len() < 3 {
        return no(format!("能用的图只有 {} 张", good.len()));
    }
    if !low.is_empty() && low.iter().all(|c| c.bad.is_some() || out.contains(&c.z)) {
        let why = low.iter().find_map(|c| c.bad.clone()).unwrap_or_else(|| "已去掉".into());
        return no(format!("0° 附近的图不能用：{why}"));
    }
    if 2 * good.len() < n || good.len() < 9 {
        return no(format!("{n} 张里能用的只有 {} 张", good.len()));
    }
    if hi - lo < 60.0 {
        return no(format!("能用的倾角范围只有 {:.0}°", hi - lo));
    }
    if dropped > 4.max(n * 15 / 100) {
        return Verdict { usable: true, reason: Some(format!("去掉了 {dropped} 张")), attention: true };
    }
    Verdict { usable: true, reason: None, attention: false }
}

/// The sections left out of alignment and reconstruction: the program's
/// `bad` ones unless the user kept them, and the ones the user dropped.
pub fn excluded(d: &Dataset) -> Vec<usize> {
    let mut out: Vec<usize> = d.checks.iter().filter(|c| c.bad.is_some() && d.picks.get(&c.z) != Some(&true)).map(|c| c.z).collect();
    out.extend(d.picks.iter().filter(|(z, keep)| !**keep && **z < d.angles.len()).map(|(z, _)| *z));
    out.sort_unstable();
    out.dedup();
    out
}

/// Each view's shift to its neighbour nearer 0° (the view at the lowest
/// tilt has none), as a fraction of the field, from small copies of the
/// views `w` × `h` in section order. A view moved against its neighbour by
/// a third of the field shares too little of it to be tracked from it.
pub fn jumps(small: &[Vec<f32>], w: usize, h: usize, angles: &[f64]) -> Vec<Option<f64>> {
    use rayon::prelude::*;
    let mut order: Vec<usize> = (0..angles.len()).collect();
    order.sort_by(|&a, &b| angles[a].total_cmp(&angles[b]));
    let Some(zero) = (0..order.len()).min_by(|&a, &b| angles[order[a]].abs().total_cmp(&angles[order[b]].abs())) else { return vec![] };
    let pairs: Vec<(usize, usize)> = (0..order.len())
        .filter(|&k| k != zero)
        .map(|k| (order[k], order[if k < zero { k + 1 } else { k - 1 }]))
        .collect();
    // The central square, correlated over its whole range.
    let n = 1usize << w.min(h).max(1).ilog2();
    if n < 32 {
        return vec![None; angles.len()];
    }
    let (x0, y0) = ((w - n) / 2, (h - n) / 2);
    let cut = |v: &[f32]| -> Vec<f32> {
        let mean = (v.iter().map(|&x| x as f64).sum::<f64>() / v.len().max(1) as f64) as f32;
        (0..n * n).map(|k| v[(y0 + k / n) * w + x0 + k % n] - mean).collect()
    };
    let found: Vec<(usize, Option<f64>)> = pairs
        .par_iter()
        .map(|&(view, toward)| {
            let (d, cc) = crate::align::patch_shift(&cut(&small[toward]), &cut(&small[view]), n, n as f64 / 2.0);
            (view, (cc.is_finite() && cc > 0.02).then(|| d[0].hypot(d[1]) / n as f64))
        })
        .collect();
    let mut out = vec![None; angles.len()];
    for (z, j) in found {
        out[z] = j;
    }
    out
}

/// Judges the views of a new tilt series (`d.checks` as measured while its
/// movies were corrected) with small copies of them, and the series.
pub fn look(d: &mut Dataset, small: &[Vec<f32>], w: usize, h: usize) {
    if small.len() == d.checks.len() && small.iter().all(|v| v.len() == w * h) {
        let angles: Vec<f64> = d.checks.iter().map(|c| c.angle).collect();
        for (c, j) in d.checks.iter_mut().zip(jumps(small, w, h, &angles)) {
            c.jump = j;
        }
    }
    d.verdict = Some(judge(&mut d.checks));
}

/// Looks at the views of a stack that has no checks yet (made elsewhere,
/// or before these were kept) and judges them. Such a stack need not hold
/// counts: its darkest values stand for no signal.
pub fn survey(img: &std::sync::Arc<crate::image::OpenImage>, d: &mut Dataset) -> anyhow::Result<()> {
    let info = img.info();
    let (mut small, mut size) = (vec![], (0, 0));
    for z in 0..d.angles.len() {
        super::check_cancel()?;
        let (page, sample) = info.locate(0, z, 0);
        let p = img.page(page)?;
        let p = &p[sample.min(p.len() - 1)];
        let v: Vec<f32> = (0..p.w * p.h).map(|i| p.data.get_f64(i) as f32).map(|x| if x.is_finite() { x } else { 0.0 }).collect();
        let (s, w, h) = crate::motion::series::shrink(&v, p.w, p.h);
        size = (w, h);
        small.push(s);
    }
    let (w, h) = size;
    // The level of no signal: the darkest hundredth of the darkest view.
    let floor = small
        .iter()
        .map(|v| {
            let mut s = v.clone();
            s.sort_by(f32::total_cmp);
            s[s.len() / 100]
        })
        .fold(f32::INFINITY, f32::min)
        .min(0.0);
    d.checks = small
        .iter()
        .enumerate()
        .map(|(z, v)| {
            let lifted: Vec<f32> = v.iter().map(|x| x - floor).collect();
            let (mean, dark) = measure(&lifted, w, h);
            ViewCheck { z, angle: d.angles[z], mean, dark, ..Default::default() }
        })
        .collect();
    look(d, &small, w, h);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(thickness: f64, level: f64) -> Vec<ViewCheck> {
        (0..41)
            .map(|i| {
                let angle = -60.0 + 3.0 * i as f64;
                ViewCheck { z: i, angle, mean: 20.0 * (-thickness / (angle - level).to_radians().cos()).exp(), dose: Some(3.0), drift_a: Some(4.0 + (i % 3) as f64), fit_res_a: Some(12.0), ..Default::default() }
            })
            .collect()
    }

    #[test]
    fn a_clean_series_keeps_every_view_also_when_the_slab_is_not_level() {
        for (thickness, level) in [(0.3, 0.0), (1.0, 0.0), (1.0, 12.0), (1.5, -17.0)] {
            let mut c = series(thickness, level);
            let v = judge(&mut c);
            assert!(c.iter().all(|c| c.bad.is_none()), "{thickness} {level}: {:?}", c.iter().filter(|c| c.bad.is_some()).map(|c| (c.angle, c.bad.clone())).collect::<Vec<_>>());
            assert_eq!(v, Verdict { usable: true, reason: None, attention: false });
        }
    }

    #[test]
    fn shadowed_views_at_one_end_are_left_out_and_the_rest_kept() {
        let mut c = series(0.8, 8.0);
        for v in c.iter_mut().filter(|v| v.angle >= 51.0) {
            v.mean *= 0.35;
            v.dark = 0.5;
        }
        // One dim view with nothing opaque in it.
        c[12].mean *= 0.4;
        let v = judge(&mut c);
        let bad: Vec<f64> = c.iter().filter(|c| c.bad.is_some()).map(|c| c.angle).collect();
        assert_eq!(bad, [-24.0, 51.0, 54.0, 57.0, 60.0]);
        assert!(c[12].bad.as_ref().unwrap().contains("太暗"), "{:?}", c[12].bad);
        assert!(c[40].bad.as_ref().unwrap().contains("挡住"));
        assert_eq!(v, Verdict { usable: true, reason: None, attention: false });
    }

    #[test]
    fn a_series_without_its_low_tilts_or_with_few_views_left_is_not_usable() {
        let mut c = series(0.8, 0.0);
        for v in c.iter_mut().filter(|v| v.angle.abs() <= 10.0) {
            v.dark = 0.9;
        }
        let v = judge(&mut c);
        assert!(!v.usable && v.reason.as_ref().unwrap().contains("0°"), "{v:?}");
        let mut c = series(0.8, 0.0);
        for v in c.iter_mut().filter(|v| v.angle > -30.0 && v.angle.abs() > 10.0) {
            v.dark = 0.9;
        }
        let v = judge(&mut c);
        assert!(!v.usable, "{v:?}");
    }

    #[test]
    fn drift_rings_and_jumps_each_take_one_view_out() {
        let mut c = series(0.5, 0.0);
        c[5].drift_a = Some(300.0);
        c[20].fit_res_a = Some(60.0);
        c[30].jump = Some(0.45);
        // Rings lost at a high tilt are not held against the view.
        c[1].fit_res_a = Some(f64::INFINITY);
        judge(&mut c);
        let bad: Vec<usize> = c.iter().filter(|c| c.bad.is_some()).map(|c| c.z).collect();
        assert_eq!(bad, [5, 20, 30]);
    }

    #[test]
    fn the_users_choice_wins_over_the_programs() {
        let mut d: Dataset = serde_json::from_value(serde_json::json!({ "version": 1, "stack": "/x/ts.mrc", "angles": [-3.0, 0.0, 3.0, 6.0] })).unwrap();
        d.checks = (0..4).map(|z| ViewCheck { z, bad: (z >= 2).then(|| "x".to_string()), ..Default::default() }).collect();
        assert_eq!(excluded(&d), [2, 3]);
        d.picks.insert(3, true);
        d.picks.insert(0, false);
        assert_eq!(excluded(&d), [0, 2]);
    }

    #[test]
    fn opaque_part_of_a_view_is_measured() {
        let (w, h) = (64, 64);
        let v: Vec<f32> = (0..w * h).map(|i| if i % w >= 48 { 0.1 } else { 10.0 + (i % 7) as f32 }).collect();
        let (mean, dark) = measure(&v, w, h);
        assert!((dark - 0.25).abs() < 0.01, "{dark}");
        assert!(mean > 7.0 && mean < 11.0);
        // Thick but not opaque: nothing counted.
        let v: Vec<f32> = (0..w * h).map(|i| if i % w >= 48 { 2.0 } else { 10.0 }).collect();
        assert_eq!(measure(&v, w, h).1, 0.0);
    }

    #[test]
    fn a_view_far_from_its_neighbour_is_found() {
        let (w, h) = (128usize, 128usize);
        let tex = |dx: isize| -> Vec<f32> {
            (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as isize + dx, (i / w) as isize);
                    // Blocks a few pixels wide, so the correlation has a peak with a width.
                    let mut s = ((x.div_euclid(6)) as u64).wrapping_mul(0x9E3779B97F4A7C15) ^ ((y / 6) as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
                    s ^= s >> 29;
                    s = s.wrapping_mul(0xBF58476D1CE4E5B9);
                    s ^= s >> 32;
                    (s % 1000) as f32 / 1000.0
                })
                .collect()
        };
        let small = vec![tex(0), tex(0), tex(48)];
        let j = jumps(&small, w, h, &[-3.0, 0.0, 3.0]);
        assert!(j[1].is_none());
        assert!(j[0].unwrap() < 0.05, "{:?}", j[0]);
        assert!(j[2].unwrap() > 0.3, "{:?}", j[2]);
    }
}
