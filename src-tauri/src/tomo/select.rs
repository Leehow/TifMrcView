//! Choices judged by the tomogram they give (`quality::held_out_views`),
//! in batches: every candidate move of a batch is scored against the same
//! held-out views, the best is kept only when it beats the current choice
//! by more than twice the standard error of the paired difference, and the
//! next batch starts from it; a batch with no such move ends the search.
//! Residuals alone mislead: on EMPIAR-10988 TS_026 the alignment with the
//! lowest residual (1.14 px) gave the worst tomogram.

use super::{dataset::Dataset, quality, Alignment};
use anyhow::Result;
use serde::Serialize;

/// One batch: each candidate's gain over the current choice and its error.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub base: f64,
    pub moves: Vec<(String, f64, f64)>,
    pub kept: Option<String>,
}

/// Most views tried for leaving out.
const CANDIDATES: usize = 6;
/// Bin of the quick tomograms the score is taken on.
const BIN: usize = 8;

/// Leaves out views that make the tomogram worse: the views fitting the
/// alignment worst (residual above 1.5 × the median, at most `CANDIDATES`),
/// each tried alone per batch on the alignment's own tracks.
pub fn exclude_views(img: &std::sync::Arc<crate::image::OpenImage>, d: &Dataset, a: &Alignment, progress: &(dyn Fn(&str, usize, usize) + Sync)) -> Result<(Alignment, Vec<Batch>)> {
    let ctf = d.ctf.as_ref().map(|c| (c, &d.optics, d.optics.pixel_a.unwrap_or(1.0)));
    let px = d.optics.pixel_a.unwrap_or(1.0);
    let depth = ((6000.0 / (px * BIN as f64)).ceil() as usize).clamp(16, 512);
    let mut residuals: Vec<f64> = a.views.iter().filter(|v| !v.excluded).map(|v| v.residual).collect();
    residuals.sort_by(f64::total_cmp);
    let median = residuals[residuals.len() / 2];
    let mut worst: Vec<&super::View> = a.views.iter().filter(|v| !v.excluded && v.residual > 1.5 * median).collect();
    worst.sort_by(|x, y| y.residual.total_cmp(&x.residual));
    let mut candidates: Vec<usize> = worst.iter().take(CANDIDATES).map(|v| v.file_index).collect();
    let mut current = a.clone();
    let mut batches = vec![];
    if candidates.is_empty() {
        return Ok((current, batches));
    }
    let held = quality::spread_views(a, 6, &candidates);
    let score = |al: &Alignment| quality::held_out_views(img, al, ctf, BIN, depth, 0.0, &held);
    let mut base = score(&current)?;
    let mut done = 0;
    while !candidates.is_empty() {
        let mut moves = vec![];
        let mut best: Option<(usize, Alignment, quality::HeldOut, f64)> = None;
        for (i, &c) in candidates.iter().enumerate() {
            super::check_cancel()?;
            progress("挑选倾角", done, done + candidates.len() - i);
            done += 1;
            let mut excluded: Vec<usize> = current.views.iter().filter(|v| v.excluded).map(|v| v.file_index).collect();
            excluded.push(c);
            let req = super::AlignReq { token: 0, c: 0, t: 0, angles: d.angles.clone(), start: None, step: None, axis: Some(current.axis), bin: current.bin, sweep: false, patch: Some(current.patch), excluded, groups: None, previous: Some(current.clone()) };
            let Ok(al) = super::align(img, &req, &|_, _| {}) else { continue };
            let s = score(&al)?;
            let (gain, se) = s.better(&base);
            let angle = a.views.iter().find(|v| v.file_index == c).map_or(0.0, |v| v.angle);
            moves.push((format!("去掉 {angle:+.0}°"), gain, se));
            if gain > 2.0 * se && best.as_ref().is_none_or(|b| gain > b.3) {
                best = Some((c, al, s, gain));
            }
        }
        let kept = best.as_ref().map(|b| moves.iter().find(|m| m.1 == b.3).unwrap().0.clone());
        batches.push(Batch { base: base.frc, moves, kept });
        let Some((c, al, s, _)) = best else { break };
        candidates.retain(|&x| x != c);
        current = al;
        base = s;
    }
    Ok((current, batches))
}
