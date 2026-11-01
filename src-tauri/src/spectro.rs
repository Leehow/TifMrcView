//! Spectrum quantification (Pro): EDS by the Cliff–Lorimer ratio method
//! and EELS core-loss edges by power-law background subtraction.
//!
//! EDS
//! - Background: SNIP clipping on a log-log transformed spectrum, with a
//!   window that follows the detector resolution at each energy.
//! - Peaks: maxima of the smoothed net spectrum above 3σ of counting noise.
//! - Elements: the given ones, or picked greedily by how much of the peak
//!   list their main lines explain.
//! - Quantification: net counts of each element's line family over windows
//!   of ±1.2 FWHM, weighted by theoretical k-factors (Bethe ionisation
//!   cross-section × fluorescence yield, thin film, no absorption or
//!   detector efficiency) unless the user gives k-factors.
//!
//! EELS
//! - Each edge's pre-edge window is fitted with A·E^−r (least squares in
//!   log–log) and the background extrapolated under the edge; the signal
//!   is the net count over a window above the onset.
//! - t/λ from the log-ratio of total to zero-loss counts when the
//!   spectrum starts below zero energy loss.
//! - Ratios of areal densities need partial cross-sections, which the
//!   user gives (σ per edge); without them only edge intensities are given.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::xray_data::{Element, ELEMENTS};

fn element(sym: &str) -> Option<&'static Element> {
    ELEMENTS.iter().find(|e| e.sym.eq_ignore_ascii_case(sym))
}

fn edge(el: &Element, name: &str) -> Option<(f64, f64)> {
    el.edges
        .iter()
        .find(|e| e.name == name)
        .map(|e| (e.energy, e.fyield))
}

// ---- EDS ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdsReq {
    pub values: Vec<f64>,
    /// Energy of channel 0 and per channel, eV.
    pub start: f64,
    pub step: f64,
    /// Beam energy, kV.
    pub e0_kv: f64,
    /// Elements to quantify; empty finds them.
    #[serde(default)]
    pub elements: Vec<String>,
    /// Detector resolution at Mn Kα, eV.
    #[serde(default = "default_fwhm")]
    pub fwhm_mn: f64,
    /// k-factors given by the user (relative to any one element), by symbol.
    #[serde(default)]
    pub k: Vec<(String, f64)>,
}

fn default_fwhm() -> f64 {
    130.0
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EdsPeak {
    pub energy: f64,
    pub net: f64,
    pub snr: f64,
    /// "Fe Kα" when an element's line explains it.
    pub label: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EdsQuant {
    pub element: String,
    /// "K", "L" or "M".
    pub family: String,
    /// Energy of the family's strongest line, eV.
    pub energy: f64,
    pub net: f64,
    /// One standard deviation of `net`, counts.
    pub err: f64,
    pub k: f64,
    /// Whether k came from the user.
    pub k_given: bool,
    pub wt: f64,
    pub at: f64,
    /// Relative error of the concentrations from counting (1σ).
    pub rel_err: f64,
    /// Other quantified elements with a line inside this element's windows.
    pub overlaps: Vec<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EdsResult {
    pub background: Vec<f64>,
    pub peaks: Vec<EdsPeak>,
    pub quant: Vec<EdsQuant>,
    /// Elements were found rather than given.
    pub auto: bool,
}

/// Detector FWHM at energy e (eV): the Fano-limited form scaled from Mn Kα.
fn fwhm(e: f64, at_mn: f64) -> f64 {
    (at_mn * at_mn + 2.49 * (e - 5894.4))
        .max(40.0 * 40.0)
        .sqrt()
}

/// SNIP background with a channel-dependent half window.
fn snip(y: &[f64], half: &[usize]) -> Vec<f64> {
    let n = y.len();
    // Log-log-square-root transform flattens peaks of very different heights.
    let mut v: Vec<f64> = y
        .iter()
        .map(|&a| ((a.max(0.0) + 1.0).sqrt() + 1.0).ln().ln_1p())
        .collect();
    let pmax = half.iter().copied().max().unwrap_or(1);
    let mut w = v.clone();
    // Decreasing windows give a smoother result than increasing ones.
    for p in (1..=pmax).rev() {
        for i in 0..n {
            w[i] = v[i];
            if p <= half[i] && i >= p && i + p < n {
                let m = (v[i - p] + v[i + p]) / 2.0;
                if m < v[i] {
                    w[i] = m;
                }
            }
        }
        std::mem::swap(&mut v, &mut w);
    }
    v.iter()
        .map(|&t| ((t.exp() - 1.0).exp() - 1.0).powi(2) - 1.0)
        .map(|b| b.max(0.0))
        .collect()
}

fn gauss_smooth(y: &[f64], sigma: f64) -> Vec<f64> {
    if sigma < 0.5 {
        return y.to_vec();
    }
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f64> = (-r..=r)
        .map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp())
        .collect();
    let n = y.len() as isize;
    (0..n)
        .map(|i| {
            let (mut s, mut w) = (0.0, 0.0);
            for (j, kk) in (-r..=r).zip(&k) {
                let t = i + j;
                if t >= 0 && t < n {
                    s += y[t as usize] * kk;
                    w += kk;
                }
            }
            s / w
        })
        .collect()
}

/// Subshell whose vacancy gives a line (Siegbahn names).
fn initial_level(name: &str) -> &'static str {
    match name {
        n if n.starts_with('K') => "K",
        "La1" | "La2" | "Lb2,15" | "Ll" | "Lb5" | "Lb6" => "L3",
        "Lb1" | "Ln" | "Lg1" | "Lg5" | "Lg6" => "L2",
        n if n.starts_with('L') => "L1",
        "Mb" => "M4",
        "Mg" => "M3",
        _ => "M5",
    }
}

/// Bethe ionisation (∝ n·ln U/(U·Ec²)) times fluorescence yield of a subshell.
fn shell_yield(el: &Element, shell: &str, e0: f64) -> f64 {
    let n = match shell {
        "K" | "L1" | "L2" | "M1" | "M2" => 2.0,
        "L3" | "M3" | "M4" => 4.0,
        _ => 6.0,
    };
    let b = if shell == "K" { 0.35 } else { 0.25 };
    match edge(el, shell) {
        Some((ec, w)) if e0 > ec => {
            let u = e0 / ec;
            n * b * u.ln() / (u * ec * ec) * w
        }
        _ => 0.0,
    }
}

/// The lines of one family of an element that can be excited at `e0` eV
/// and fall inside [lo, hi] eV, with weights relative to the family's
/// strongest line (line intensities within a subshell times the subshell's
/// share of the emission).
fn family_lines(
    el: &Element,
    fam: &str,
    e0: f64,
    lo: f64,
    hi: f64,
) -> Vec<(f64, f64, &'static str)> {
    let all: Vec<(f64, f64, &'static str)> = el
        .lines
        .iter()
        .filter(|l| l.family == fam)
        .map(|l| {
            (
                l.energy,
                l.intensity * shell_yield(el, initial_level(l.name), e0),
                l.name,
            )
        })
        .filter(|l| l.1 > 0.0)
        .collect();
    let top = all.iter().map(|l| l.1).fold(0.0, f64::max);
    all.into_iter()
        .filter(|l| l.0 >= lo && l.0 <= hi && l.1 >= 0.03 * top)
        .map(|l| (l.0, l.1 / top, l.2))
        .collect()
}

/// The family used to quantify an element: K while its lines are in the
/// spectrum, well excited and under 25 keV; else L; else M.
fn pick_family(el: &Element, e0: f64, lo: f64, hi: f64) -> Option<&'static str> {
    for fam in ["K", "L", "M"] {
        let lines = family_lines(el, fam, e0, lo.max(150.0), hi);
        let Some(main) = lines.iter().map(|l| l.0).reduce(f64::max) else {
            continue;
        };
        let ec = edge(
            el,
            match fam {
                "K" => "K",
                "L" => "L3",
                _ => "M5",
            },
        )
        .unwrap()
        .0;
        if fam == "K" && (main > 25_000.0 || e0 / ec < 1.5) {
            continue;
        }
        return Some(fam);
    }
    None
}

/// Strongest line of a family, by intensity.
fn main_line(lines: &[(f64, f64, &'static str)]) -> Option<(f64, &'static str)> {
    lines
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|l| (l.0, l.2))
}

/// Theoretical k-factor (up to a common constant): A / Σ Q_s·ω_s over the
/// subshells whose vacancies give the family's lines, with the Bethe
/// cross-section Q ∝ n·b·ln(U)/(U·Ec²) (Coster–Kronig transfers ignored).
fn theoretical_k(el: &Element, fam: &str, e0: f64) -> Option<f64> {
    let shells: &[&str] = match fam {
        "K" => &["K"],
        "L" => &["L1", "L2", "L3"],
        _ => &["M4", "M5"],
    };
    let y: f64 = shells.iter().map(|s| shell_yield(el, s, e0)).sum();
    (y > 0.0).then(|| el.mass / y)
}

/// Radioactive elements and noble gases (Ar from ion milling excepted):
/// never picked automatically, only when named.
const RARE: &[&str] = &[
    "Ne", "Kr", "Xe", "Tc", "Pm", "Po", "At", "Rn", "Fr", "Ra", "Ac", "Pa", "Th", "U",
];

fn p_off(a: f64, b: f64) -> f64 {
    (a - b).abs()
}

/// Pretty line name: "Ka1" → "Kα1".
fn greek(name: &str) -> String {
    name.replace('a', "α")
        .replace('b', "β")
        .replace('g', "γ")
        .replace('l', "ℓ")
        .replace('n', "η")
}

/// "Fe Kα1": the nearest visible line of the chosen elements within `tol`.
fn label_peak(
    chosen: &[&'static Element],
    e: f64,
    e0: f64,
    lo: f64,
    hi: f64,
    tol: &dyn Fn(f64) -> f64,
) -> Option<String> {
    // The line most likely to make the peak: strong and close.
    let mut best: Option<(f64, String)> = None;
    for el in chosen {
        for fam in ["K", "L", "M"] {
            for l in family_lines(el, fam, e0, lo, hi) {
                let d = (l.0 - e) / tol(e);
                let score = l.1 * (-2.0 * d * d).exp();
                if d.abs() < 1.0 && best.as_ref().is_none_or(|b| score > b.0) {
                    // "Kα1" and "Kα2" are one peak: drop the number.
                    let name = greek(l.2)
                        .trim_end_matches(|c: char| c.is_ascii_digit() || c == ',')
                        .to_string();
                    best = Some((score, format!("{} {}", el.sym, name)));
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// One element's line family as a peak shape of unit area.
struct Comp {
    el: &'static Element,
    fam: &'static str,
    /// First channel and values.
    at: usize,
    shape: Vec<f64>,
}

struct Shapes {
    start: f64,
    step: f64,
    n: usize,
    fwhm_mn: f64,
    e0: f64,
    lo: f64,
    hi: f64,
}

/// Inverse of the normal matrix of a fit.
struct Cov(Vec<Vec<f64>>);

impl Cov {
    fn var(&self, i: usize) -> f64 {
        self.0.get(i).map_or(f64::NAN, |r| r[i].max(0.0))
    }
    fn corr(&self, i: usize, j: usize) -> f64 {
        let d = (self.var(i) * self.var(j)).sqrt();
        if d > 0.0 {
            self.0[i][j] / d
        } else {
            0.0
        }
    }
}

impl Shapes {
    fn comps(&self, chosen: &[&'static Element]) -> Vec<Comp> {
        let mut out = Vec::new();
        for el in chosen {
            for fam in ["K", "L", "M"] {
                let lines = family_lines(el, fam, self.e0, self.lo, self.hi);
                if lines.is_empty() {
                    continue;
                }
                let e_lo = lines.iter().map(|l| l.0).fold(f64::MAX, f64::min);
                let e_hi = lines.iter().map(|l| l.0).fold(f64::MIN, f64::max);
                let pad = 3.0 * fwhm(e_hi, self.fwhm_mn) / 2.355;
                let a = (((e_lo - pad - self.start) / self.step).floor().max(0.0)) as usize;
                let b = ((((e_hi + pad - self.start) / self.step).ceil()).max(0.0) as usize)
                    .min(self.n - 1);
                if b <= a {
                    continue;
                }
                let mut shape = vec![0.0; b - a + 1];
                for (le, w, _) in &lines {
                    let sg = fwhm(*le, self.fwhm_mn) / 2.355;
                    for (k, v) in shape.iter_mut().enumerate() {
                        let e = self.start + (a + k) as f64 * self.step;
                        *v += w * (-(e - le).powi(2) / (2.0 * sg * sg)).exp() * self.step
                            / (sg * (2.0 * std::f64::consts::PI).sqrt());
                    }
                }
                let tot: f64 = lines.iter().map(|l| l.1).sum();
                shape.iter_mut().for_each(|v| *v /= tot);
                out.push(Comp {
                    el,
                    fam,
                    at: a,
                    shape,
                });
            }
        }
        out
    }

    /// χ² of the best non-negative fit.
    fn fit(
        &self,
        chosen: &[&'static Element],
        net: &[f64],
        y: &[f64],
    ) -> (Vec<Comp>, Vec<f64>, f64) {
        let (c, a, chi, _) = self.fit_full(chosen, net, y);
        (c, a, chi)
    }

    /// Weighted (1/counts) least squares with the coefficients kept ≥ 0.
    fn fit_full(
        &self,
        chosen: &[&'static Element],
        net: &[f64],
        y: &[f64],
    ) -> (Vec<Comp>, Vec<f64>, f64, Cov) {
        let comps = self.comps(chosen);
        let m = comps.len();
        let w: Vec<f64> = y.iter().map(|v| 1.0 / v.max(1.0)).collect();
        let dot = |i: &Comp, j: &Comp| {
            let (a, b) = (
                i.at.max(j.at),
                (i.at + i.shape.len()).min(j.at + j.shape.len()),
            );
            (a..b)
                .map(|t| i.shape[t - i.at] * j.shape[t - j.at] * w[t])
                .sum::<f64>()
        };
        let mut mat = vec![vec![0.0; m]; m];
        let mut rhs = vec![0.0; m];
        for i in 0..m {
            for j in i..m {
                let d = dot(&comps[i], &comps[j]);
                mat[i][j] = d;
                mat[j][i] = d;
            }
            rhs[i] = comps[i]
                .shape
                .iter()
                .enumerate()
                .map(|(k, s)| s * net[comps[i].at + k] * w[comps[i].at + k])
                .sum();
        }
        // Active set: drop the most negative coefficient and solve again.
        let mut on = vec![true; m];
        let mut coef = vec![0.0; m];
        let mut inv = vec![vec![0.0; m]; m];
        loop {
            let idx: Vec<usize> = (0..m).filter(|&i| on[i]).collect();
            let sub: Vec<Vec<f64>> = idx
                .iter()
                .map(|&i| idx.iter().map(|&j| mat[i][j]).collect())
                .collect();
            let Some(si) = invert(&sub) else { break };
            coef = vec![0.0; m];
            for (r, &i) in idx.iter().enumerate() {
                coef[i] = idx
                    .iter()
                    .enumerate()
                    .map(|(c, &j)| si[r][c] * rhs[j])
                    .sum();
            }
            inv = vec![vec![0.0; m]; m];
            for (r, &i) in idx.iter().enumerate() {
                for (c, &j) in idx.iter().enumerate() {
                    inv[i][j] = si[r][c];
                }
            }
            match (0..m)
                .filter(|&i| on[i] && coef[i] < 0.0)
                .min_by(|&a, &b| coef[a].total_cmp(&coef[b]))
            {
                Some(i) => on[i] = false,
                None => break,
            }
        }
        let mut model = vec![0.0; self.n];
        for (c, a) in comps.iter().zip(&coef) {
            for (k, s) in c.shape.iter().enumerate() {
                model[c.at + k] += a * s;
            }
        }
        let i0 = (((self.lo - self.start) / self.step).max(0.0)) as usize;
        let chi = (i0..self.n)
            .map(|i| (net[i] - model[i]).powi(2) * w[i])
            .sum();
        (comps, coef, chi, Cov(inv))
    }
}

/// Gauss–Jordan inverse of a small matrix; None when singular.
fn invert(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let mut m: Vec<Vec<f64>> = a
        .iter()
        .enumerate()
        .map(|(i, r)| {
            r.iter()
                .copied()
                .chain((0..n).map(|j| if i == j { 1.0 } else { 0.0 }))
                .collect()
        })
        .collect();
    for c in 0..n {
        let p = (c..n).max_by(|&x, &y| m[x][c].abs().total_cmp(&m[y][c].abs()))?;
        if m[p][c].abs() < 1e-300 {
            return None;
        }
        m.swap(c, p);
        let d = m[c][c];
        m[c].iter_mut().for_each(|v| *v /= d);
        for r in 0..n {
            if r != c {
                let f = m[r][c];
                if f != 0.0 {
                    let row = m[c].clone();
                    m[r].iter_mut().zip(&row).for_each(|(v, s)| *v -= f * s);
                }
            }
        }
    }
    Some(m.into_iter().map(|r| r[n..].to_vec()).collect())
}

pub fn eds(req: &EdsReq) -> Result<EdsResult> {
    let n = req.values.len();
    if n < 16 || !(req.step > 0.0) {
        bail!("谱太短或能量刻度不对");
    }
    if !(req.e0_kv > 0.0) {
        bail!("需要加速电压");
    }
    let e0 = req.e0_kv * 1e3;
    let en = |i: f64| req.start + i * req.step;
    let (lo, hi) = (en(0.0), en((n - 1) as f64));
    let half: Vec<usize> = (0..n)
        .map(|i| ((1.5 * fwhm(en(i as f64), req.fwhm_mn) / req.step).round() as usize).max(2))
        .collect();
    // SNIP on raw counts follows the noise minima; smooth over about a third of the resolution first.
    let mid = fwhm(5894.4, req.fwhm_mn) / req.step;
    let bg = snip(&gauss_smooth(&req.values, mid / 2.355 * 0.7), &half);
    let net: Vec<f64> = req.values.iter().zip(&bg).map(|(y, b)| y - b).collect();

    // Peaks.
    let sm = gauss_smooth(&net, mid / 2.355 * 0.5);
    let mut peaks: Vec<EdsPeak> = Vec::new();
    for i in 1..n - 1 {
        let e = en(i as f64);
        if e < 100.0 || sm[i] <= 0.0 || sm[i] < sm[i - 1] || sm[i] < sm[i + 1] {
            continue;
        }
        let w = (0.6 * fwhm(e, req.fwhm_mn) / req.step).round().max(1.0) as usize;
        let (a, b) = (i.saturating_sub(w), (i + w).min(n - 1));
        if (a..=b).any(|j| sm[j] > sm[i]) {
            continue;
        }
        let area: f64 = net[a..=b].iter().sum();
        let noise = req.values[a..=b]
            .iter()
            .map(|v| v.max(0.0))
            .sum::<f64>()
            .max(1.0)
            .sqrt();
        let snr = area / noise;
        if snr >= 3.0 {
            peaks.push(EdsPeak {
                energy: e,
                net: area,
                snr,
                label: None,
            });
        }
    }

    // Elements.
    let tol = |e: f64| (0.5 * fwhm(e, req.fwhm_mn)).max(2.0 * req.step);
    let auto = req.elements.is_empty();
    let mut chosen: Vec<&'static Element> = Vec::new();
    if auto {
        // Each element is scored by the peak under the main line of the
        // family it would be quantified with (K lines of light elements, L
        // or M of heavy ones), so a heavy element's weak low-energy lines do
        // not claim a light element's K peak. The best is taken until no
        // clear peak is left unexplained.
        let mut explained = vec![false; peaks.len()];
        for _ in 0..12 {
            let best = ELEMENTS
                .iter()
                .filter(|el| {
                    el.z >= 5 && !RARE.contains(&el.sym) && !chosen.iter().any(|c| c.z == el.z)
                })
                .filter_map(|el| {
                    let fam = pick_family(el, e0, lo, hi)?;
                    let (me, _) = main_line(&family_lines(el, fam, e0, lo.max(150.0), hi))?;
                    let j = peaks.iter().position(|p| (p.energy - me).abs() < tol(me))?;
                    (!explained[j] && peaks[j].snr >= 8.0)
                        .then(|| (peaks[j].net, el, (p_off(peaks[j].energy, me), j)))
                })
                // The larger peak; on one peak, the closer line.
                .max_by(|a, b| a.0.total_cmp(&b.0).then(b.2 .0.total_cmp(&a.2 .0)));
            let Some((_, el, _)) = best else { break };
            chosen.push(el);
            for (j, p) in peaks.iter().enumerate() {
                if ["K", "L", "M"].iter().any(|f| {
                    family_lines(el, f, e0, lo, hi)
                        .iter()
                        .any(|l| (l.0 - p.energy).abs() < tol(p.energy))
                }) {
                    explained[j] = true;
                }
            }
        }
    } else {
        for s in &req.elements {
            match element(s.trim()) {
                Some(el) => chosen.push(el),
                None => bail!("不认识的元素：{s}"),
            }
        }
    }

    for p in peaks.iter_mut() {
        p.label = label_peak(&chosen, p.energy, e0, lo, hi, &tol);
    }

    // Quantify: fit every visible line family of the chosen elements to
    // the net spectrum at once, so overlapping lines are shared out by
    // their shapes.
    let sh = Shapes {
        start: req.start,
        step: req.step,
        n,
        fwhm_mn: req.fwhm_mn,
        e0,
        lo: lo.max(100.0),
        hi,
    };
    if auto {
        // Peaks a few eV apart can be either of two elements (Zr Lα / Pt Mα,
        // Si Kα / W Mα…): keep the one whose line shapes fit best.
        for i in 0..chosen.len() {
            let Some(me) = pick_family(chosen[i], e0, lo, hi)
                .and_then(|f| main_line(&family_lines(chosen[i], f, e0, lo.max(150.0), hi)))
                .map(|m| m.0)
            else {
                continue;
            };
            let mut best = (sh.fit(&chosen, &net, &req.values).2, chosen[i]);
            for el in ELEMENTS.iter().filter(|el| {
                el.z >= 5 && !RARE.contains(&el.sym) && !chosen.iter().any(|c| c.z == el.z)
            }) {
                let Some(alt) = pick_family(el, e0, lo, hi)
                    .and_then(|f| main_line(&family_lines(el, f, e0, lo.max(150.0), hi)))
                    .map(|m| m.0)
                else {
                    continue;
                };
                if (alt - me).abs() > tol(me) {
                    continue;
                }
                let mut trial = chosen.clone();
                trial[i] = el;
                let chi = sh.fit(&trial, &net, &req.values).2;
                if chi < best.0 {
                    best = (chi, el);
                }
            }
            chosen[i] = best.1;
        }
        // Relabel the peaks after swaps.
        for p in peaks.iter_mut() {
            p.label = label_peak(&chosen, p.energy, e0, lo, hi, &tol);
        }
    }
    let (mut comps, mut coef, _, mut cov) = sh.fit_full(&chosen, &net, &req.values);
    if auto {
        // Found elements whose lines the fit gives no clear share (another
        // element's lines took the peak) are dropped.
        let weak = |el: &&Element, comps: &[Comp], coef: &[f64], cov: &Cov| {
            let fam = pick_family(el, e0, lo, hi);
            comps
                .iter()
                .position(|c| c.el.z == el.z && Some(c.fam) == fam)
                .is_none_or(|j| coef[j] < 3.0 * cov.var(j).sqrt())
        };
        let before = chosen.len();
        chosen.retain(|el| !weak(el, &comps, &coef, &cov));
        if chosen.len() != before {
            (comps, coef, _, cov) = sh.fit_full(&chosen, &net, &req.values);
            for p in peaks.iter_mut() {
                p.label = label_peak(&chosen, p.energy, e0, lo, hi, &tol);
            }
        }
    }
    let mut quant: Vec<EdsQuant> = Vec::new();
    for el in &chosen {
        let Some(fam) = pick_family(el, e0, lo, hi) else {
            continue;
        };
        let Some(j) = comps.iter().position(|c| c.el.z == el.z && c.fam == fam) else {
            continue;
        };
        let me = main_line(&family_lines(el, fam, e0, lo.max(150.0), hi)).map_or(0.0, |m| m.0);
        let given = req
            .k
            .iter()
            .find(|(e, _)| e.eq_ignore_ascii_case(el.sym))
            .map(|k| k.1);
        if given.is_none() && !req.k.is_empty() {
            bail!("k 因子要么都给，要么都不给：缺 {}", el.sym);
        }
        // Theoretical k relative to Fe K (Si K below 7.1 kV), the usual references.
        let reference = theoretical_k(element("Fe").unwrap(), "K", e0)
            .or_else(|| theoretical_k(element("Si").unwrap(), "K", e0))
            .unwrap_or(1.0);
        let k = given
            .or_else(|| theoretical_k(el, fam, e0).map(|k| k / reference))
            .unwrap_or(f64::NAN);
        // Other elements whose lines share channels with this family.
        let mut overlaps: Vec<String> = Vec::new();
        for (o, c) in comps.iter().enumerate() {
            if c.el.z != el.z
                && cov.corr(j, o).abs() > 0.15
                && !overlaps.iter().any(|s| s == c.el.sym)
            {
                overlaps.push(c.el.sym.to_string());
            }
        }
        quant.push(EdsQuant {
            element: el.sym.to_string(),
            family: fam.to_string(),
            energy: me,
            net: coef[j],
            err: cov.var(j).sqrt(),
            k,
            k_given: given.is_some(),
            wt: 0.0,
            at: 0.0,
            rel_err: 0.0,
            overlaps,
        });
    }
    // Cliff–Lorimer: C ∝ k·I, normalised; atoms ∝ C / A.
    let tot: f64 = quant
        .iter()
        .filter(|q| q.k.is_finite())
        .map(|q| q.k * q.net.max(0.0))
        .sum();
    if tot > 0.0 {
        for q in quant.iter_mut() {
            q.wt = if q.k.is_finite() {
                100.0 * q.k * q.net.max(0.0) / tot
            } else {
                0.0
            };
        }
        let atoms: f64 = quant
            .iter()
            .map(|q| q.wt / element(&q.element).unwrap().mass)
            .sum();
        for q in quant.iter_mut() {
            q.at = 100.0 * q.wt / element(&q.element).unwrap().mass / atoms;
            q.rel_err = if q.net > 0.0 {
                q.err / q.net
            } else {
                f64::INFINITY
            };
        }
    }
    quant.sort_by(|a, b| b.wt.total_cmp(&a.wt));
    Ok(EdsResult {
        background: bg,
        peaks,
        quant,
        auto,
    })
}

// ---- EELS ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EelsReq {
    pub values: Vec<f64>,
    /// Energy loss of channel 0 and per channel, eV.
    pub start: f64,
    pub step: f64,
    /// Edges to measure, "C K", "Fe L3"…; empty finds them.
    #[serde(default)]
    pub edges: Vec<String>,
    /// Pre-edge fit window width, eV (0: a fifth of the onset energy, 20–100 eV).
    #[serde(default)]
    pub pre: f64,
    /// Integration window above the onset, eV.
    #[serde(default = "default_window")]
    pub window: f64,
    /// Partial cross-sections given by the user, by edge label ("C K").
    #[serde(default)]
    pub sigma: Vec<(String, f64)>,
}

fn default_window() -> f64 {
    50.0
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EelsEdge {
    /// "C K", "Fe L3"…
    pub label: String,
    pub onset: f64,
    /// Pre-edge fit range and the power-law exponent r of A·E^−r.
    pub fit: [f64; 2],
    pub r: f64,
    /// Net counts over [onset, onset + window] and its 1σ (counting and
    /// background extrapolation).
    pub signal: f64,
    pub err: f64,
    /// Counts over the window divided by the extrapolated background.
    pub jump: f64,
    /// Step at the onset relative to the background there, and its significance.
    pub rise: f64,
    pub rise_snr: f64,
    /// Fitted background from the fit start to the end of the window: [eV, counts].
    pub background: Vec<[f64; 2]>,
    /// Atomic fraction, %, when every edge has a cross-section.
    pub at: Option<f64>,
    /// Other edges whose onset is as close to this one (found edges only).
    pub alt: Vec<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EelsResult {
    pub edges: Vec<EelsEdge>,
    /// Relative thickness t/λ, when the zero-loss peak is in the spectrum.
    pub t_lambda: Option<f64>,
    /// Zero-loss range used, eV.
    pub zlp: Option<[f64; 2]>,
    pub auto: bool,
}

/// Fits ln y = ln A − r ln E over channels [a, b]; returns (ln A, r).
fn power_law(e: &[f64], y: &[f64], a: usize, b: usize) -> Option<(f64, f64)> {
    let pts: Vec<(f64, f64)> = (a..=b)
        .filter(|&i| y[i] > 0.0 && e[i] > 0.0)
        .map(|i| (e[i].ln(), y[i].ln()))
        .collect();
    if pts.len() < 4 {
        return None;
    }
    let n = pts.len() as f64;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |s, p| (s.0 + p.0, s.1 + p.1));
    let (mx, my) = (sx / n, sy / n);
    let (sxx, sxy) = pts.iter().fold((0.0, 0.0), |s, p| {
        (s.0 + (p.0 - mx).powi(2), s.1 + (p.0 - mx) * (p.1 - my))
    });
    if sxx <= 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    Some((my - slope * mx, -slope))
}

/// Edges in the table an EELS spectrum can show: K, L3 and M5 onsets.
fn candidate_edges(lo: f64, hi: f64) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for el in ELEMENTS.iter().filter(|el| !RARE.contains(&el.sym)) {
        for name in ["K", "L3", "M5"] {
            if let Some((e, _)) = edge(el, name) {
                if e >= lo && e <= hi {
                    out.push((format!("{} {}", el.sym, name), e));
                }
            }
        }
    }
    out
}

/// Onsets of light-element K edges as seen in EELS of solids, which sit
/// several eV below the free-atom binding energies of the X-ray table.
const EELS_ONSET: &[(&str, f64)] = &[
    ("B K", 188.0),
    ("C K", 284.0),
    ("N K", 401.0),
    ("O K", 532.0),
    ("F K", 685.0),
    ("Ne K", 867.0),
];

fn table_onset(label: &str, x_ray: f64) -> f64 {
    EELS_ONSET
        .iter()
        .find(|o| o.0 == label)
        .map_or(x_ray, |o| o.1)
}

/// Moves a tabulated onset to the steepest relative rise of the smoothed
/// spectrum nearby (chemical shifts move edges by a few eV).
/// None when the steepest rise is at the low end of the search range,
/// which is the tail of an edge below.
fn refine_onset(req: &EelsReq, ys: &[f64], onset: f64) -> Option<f64> {
    let n = ys.len();
    let d = ((2.0 / req.step).round() as usize).max(1);
    let lo = onset - (0.02 * onset).max(8.0);
    let hi = onset + (0.01 * onset).max(4.0);
    let ia = (((lo - req.start) / req.step).ceil().max(d as f64)) as usize;
    let ib = (((hi - req.start) / req.step).floor() as usize).min(n.saturating_sub(d + 1));
    let mut best = (f64::MIN, 0);
    for i in ia..=ib.max(ia) {
        if i + d >= n || i < d || ys[i - d] <= 0.0 {
            continue;
        }
        let rise = (ys[i + d] - ys[i - d]) / ys[i - d];
        if rise > best.0 {
            best = (rise, i);
        }
    }
    (best.0 > f64::MIN && best.1 > ia).then(|| req.start + best.1 as f64 * req.step)
}

fn measure_edge(req: &EelsReq, e: &[f64], label: &str, onset: f64) -> Option<EelsEdge> {
    let n = e.len();
    let idx = |v: f64| (((v - req.start) / req.step).round().max(0.0) as usize).min(n - 1);
    // The default pre-edge window shrinks to what the spectrum holds, down to 10 eV.
    let room = onset - 3.0 - e[0].max(1.0);
    let pre = if req.pre > 0.0 {
        req.pre
    } else {
        (onset * 0.2).clamp(20.0, 100.0).min(room)
    };
    let (fa, fb) = (onset - pre - 3.0, onset - 3.0);
    if pre < 10.0 || fa < e[0].max(1.0) - 1e-9 {
        return None;
    }
    let (ia, ib) = (idx(fa), idx(fb));
    let (ln_a, r) = power_law(e, &req.values, ia, ib)?;
    // The window shrinks to the end of the spectrum, down to 10 eV.
    let window = req.window.min(e[n - 1] - onset);
    let (ja, jb) = (idx(onset), idx(onset + window));
    if window < 10.0 || jb <= ja + 2 {
        return None;
    }
    let bgv = |x: f64| (ln_a - r * x.ln()).exp();
    let (mut sig, mut tot, mut bsum) = (0.0, 0.0, 0.0);
    for i in ja..=jb {
        let b = bgv(e[i]);
        sig += req.values[i] - b;
        tot += req.values[i].max(0.0);
        bsum += b;
    }
    // Background extrapolation error grows with the ratio of window to fit width (Egerton's h factor, simplified).
    let h = 1.0 + (window / pre).powi(2) * 2.0;
    let err = (tot + h * bsum).sqrt();
    // The rise at the onset: mean residual just after minus just before.
    let ds = (0.02 * onset).max(5.0);
    let mean = |a: f64, b: f64| {
        let (i, j) = (idx(a), idx(b));
        let (mut r, mut c, mut k) = (0.0, 0.0, 0.0);
        for t in i..=j.max(i) {
            r += req.values[t] - bgv(e[t]);
            c += req.values[t].max(0.0);
            k += 1.0;
        }
        (r / k, c.max(1.0).sqrt() / k)
    };
    let (before, sb) = mean(onset - ds, onset - req.step);
    let (after, sa) = mean(onset + req.step, onset + ds);
    let rise = after - before;
    let rise_snr = rise / (sa * sa + sb * sb).sqrt();
    let rel = rise / bgv(onset).max(1e-9);
    let background = (ia..=jb)
        .step_by(((jb - ia) / 200).max(1))
        .map(|i| [e[i], bgv(e[i])])
        .collect();
    Some(EelsEdge {
        label: label.to_string(),
        onset,
        fit: [fa, fb],
        r,
        signal: sig,
        err,
        jump: if bsum > 0.0 { tot / bsum } else { 0.0 },
        rise: rel,
        rise_snr,
        background,
        at: None,
        alt: vec![],
    })
}

pub fn eels(req: &EelsReq) -> Result<EelsResult> {
    let n = req.values.len();
    if n < 16 || !(req.step > 0.0) {
        bail!("谱太短或能量刻度不对");
    }
    let e: Vec<f64> = (0..n).map(|i| req.start + i as f64 * req.step).collect();
    let (lo, hi) = (e[0], e[n - 1]);

    // Zero loss: from the start to the first minimum past the peak near 0 eV.
    let (mut t_lambda, mut zlp) = (None, None);
    if lo < -2.0 && hi > 20.0 {
        let z0 = e.iter().position(|&v| v >= 0.0).unwrap_or(0);
        let w = ((5.0 / req.step).round() as usize).max(1);
        let peak = (z0.saturating_sub(w)..(z0 + w).min(n))
            .max_by(|&a, &b| req.values[a].total_cmp(&req.values[b]))
            .unwrap_or(z0);
        let mut end = peak;
        while end + 1 < n && req.values[end + 1] < req.values[end] {
            end += 1;
        }
        let i0: f64 = req.values[..=end].iter().sum();
        let it: f64 = req.values.iter().sum();
        if i0 > 0.0 && it > i0 {
            t_lambda = Some((it / i0).ln());
            zlp = Some([lo, e[end]]);
        }
    }

    let auto = req.edges.is_empty();
    let mut edges: Vec<EelsEdge> = Vec::new();
    let sm = {
        let sig = ((1.0 / req.step).max(1.0)) as f64;
        gauss_smooth(&req.values, sig)
    };
    if auto {
        // Edges under 100 eV sit on the plasmon tail, where a power law
        // does not hold: name those by hand.
        let from = zlp.map_or(lo + 13.0, |z| z[1] + 30.0).max(100.0);
        let mut found: Vec<(EelsEdge, f64)> = Vec::new();
        for (label, x_ray) in candidate_edges(from, hi - 10.0) {
            let table = table_onset(&label, x_ray);
            let Some(onset) = refine_onset(req, &sm, table) else {
                continue;
            };
            if let Some(m) = measure_edge(req, &e, &label, onset) {
                if m.rise_snr > 8.0 && m.rise > 0.05 && m.signal > 3.0 * m.err {
                    found.push((m, (onset - table).abs()));
                }
            }
        }
        // Candidates that land on the same rise compete for it: keep the
        // one whose tabulated onset is nearest, name the others as alternatives.
        found.sort_by(|a, b| a.0.onset.total_cmp(&b.0.onset));
        let mut kept: Vec<(EelsEdge, f64)> = Vec::new();
        for (m, off) in found {
            match kept.last_mut() {
                Some(k) if (m.onset - k.0.onset).abs() <= (0.01 * m.onset).max(4.0) => {
                    if off < k.1 {
                        let old = std::mem::replace(k, (m, off));
                        k.0.alt.push(old.0.label);
                        k.0.alt.extend(old.0.alt);
                    } else {
                        k.0.alt.push(m.label);
                    }
                }
                _ => kept.push((m, off)),
            }
        }
        edges = kept.into_iter().map(|k| k.0).collect();
    } else {
        for lab in &req.edges {
            let mut it = lab.split_whitespace();
            let (Some(sym), Some(shell)) = (it.next(), it.next()) else {
                bail!("边要写成「元素 壳层」，如 C K、Fe L3：{lab}")
            };
            let Some(el) = element(sym) else {
                bail!("不认识的元素：{sym}")
            };
            let shell = shell.to_ascii_uppercase();
            let shell = if shell == "L" || shell == "L23" {
                "L3".to_string()
            } else if shell == "M" || shell == "M45" {
                "M5".to_string()
            } else {
                shell
            };
            let Some((onset, _)) = edge(el, &shell) else {
                bail!("{sym} 没有 {shell} 边")
            };
            let label = format!("{} {}", el.sym, shell);
            let table = table_onset(&label, onset);
            let onset = refine_onset(req, &sm, table).unwrap_or(table);
            match measure_edge(req, &e, &label, onset) {
                Some(m) => edges.push(m),
                None => bail!("{lab} 的边前窗口不在谱里"),
            }
        }
    }
    // Areal densities ∝ I / σ, when every edge has σ.
    let sig: Vec<Option<f64>> = edges
        .iter()
        .map(|m| {
            req.sigma
                .iter()
                .find(|(l, _)| l.eq_ignore_ascii_case(&m.label))
                .map(|s| s.1)
                .filter(|&s| s > 0.0)
        })
        .collect();
    if !edges.is_empty() && sig.iter().all(|s| s.is_some()) {
        let dens: Vec<f64> = edges
            .iter()
            .zip(&sig)
            .map(|(m, s)| m.signal.max(0.0) / s.unwrap())
            .collect();
        let tot: f64 = dens.iter().sum();
        if tot > 0.0 {
            for (m, d) in edges.iter_mut().zip(dens) {
                m.at = Some(100.0 * d / tot);
            }
        }
    }
    Ok(EelsResult {
        edges,
        t_lambda,
        zlp,
        auto,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic EDS spectrum: bremsstrahlung-like background plus
    /// Gaussian lines of the given elements' K or L families, with counts
    /// in proportion to amount / k so Cliff–Lorimer should give `amount`
    /// back (in wt%).
    fn eds_spectrum(parts: &[(&str, f64)], e0: f64, step: f64) -> Vec<f64> {
        let n = (20_000.0 / step) as usize;
        let mut y: Vec<f64> = (0..n)
            .map(|i| {
                let e = (i as f64 + 0.5) * step;
                // Kramers-like, cut by a soft low-energy absorption.
                200.0 * ((e0 - e).max(0.0) / e) * (1.0 - (-(e / 800.0).powi(3)).exp())
            })
            .collect();
        for (sym, wt) in parts {
            let el = element(sym).unwrap();
            let fam = pick_family(el, e0, 0.0, 20_000.0).unwrap();
            let k = theoretical_k(el, fam, e0).unwrap();
            let total = 4e5 * wt
                / k
                / theoretical_k(element("Fe").unwrap(), "K", e0)
                    .unwrap()
                    .recip();
            let lines = family_lines(el, fam, e0, 150.0, 20_000.0);
            let sum: f64 = lines.iter().map(|l| l.1).sum();
            for (le, li, _) in lines {
                let s = fwhm(le, 130.0) / 2.355;
                for (i, v) in y.iter_mut().enumerate() {
                    let e = (i as f64 + 0.5) * step;
                    *v += total * li / sum * step / (s * (2.0 * std::f64::consts::PI).sqrt())
                        * (-(e - le).powi(2) / (2.0 * s * s)).exp();
                }
            }
        }
        y
    }

    #[test]
    fn eds_recovers_composition() {
        let e0 = 200_000.0;
        let y = eds_spectrum(&[("Fe", 70.0), ("Cr", 20.0), ("Ni", 10.0)], e0, 10.0);
        let req = EdsReq {
            values: y,
            start: 5.0,
            step: 10.0,
            e0_kv: 200.0,
            elements: vec![],
            fwhm_mn: 130.0,
            k: vec![],
        };
        let r = eds(&req).unwrap();
        let get = |s: &str| {
            r.quant.iter().find(|q| q.element == s).unwrap_or_else(|| {
                panic!(
                    "{s} not found: {:?}",
                    r.quant.iter().map(|q| &q.element).collect::<Vec<_>>()
                )
            })
        };
        for (s, want) in [("Fe", 70.0), ("Cr", 20.0), ("Ni", 10.0)] {
            let q = get(s);
            assert!(
                (q.wt - want).abs() < 2.0,
                "{s}: {:.2} wt% (want {want}) {:?}",
                q.wt,
                r.quant
            );
            assert_eq!(q.family, "K");
        }
        assert!(
            r.quant.len() <= 4,
            "{:?}",
            r.quant.iter().map(|q| &q.element).collect::<Vec<_>>()
        );
    }

    #[test]
    fn eds_uses_given_elements_and_k() {
        let y = eds_spectrum(&[("Si", 50.0), ("O", 50.0)], 200_000.0, 10.0);
        let req = EdsReq {
            values: y,
            start: 5.0,
            step: 10.0,
            e0_kv: 200.0,
            elements: vec!["Si".into(), "O".into()],
            fwhm_mn: 130.0,
            k: vec![("Si".into(), 1.0), ("O".into(), 1.0)],
        };
        let r = eds(&req).unwrap();
        assert!(!r.auto);
        assert_eq!(r.quant.len(), 2);
        assert!(r.quant.iter().all(|q| q.k_given));
    }

    #[test]
    fn eels_measures_edges_and_thickness() {
        // Zero loss at 0, plasmon at 22 eV, power-law background, a C K edge
        // (284 eV) and an O K edge (532 eV) as saw-tooth steps.
        let step = 0.5;
        let start = -20.0;
        let n = ((800.0 - start) / step) as usize;
        let e: Vec<f64> = (0..n).map(|i| start + i as f64 * step).collect();
        let y: Vec<f64> = e
            .iter()
            .map(|&x| {
                let mut v = 1e6 * (-(x * x) / (2.0 * 0.6f64.powi(2))).exp();
                v += 4e4 * (-((x - 22.0).powi(2)) / (2.0 * 4.0f64.powi(2))).exp();
                if x > 15.0 {
                    v += 1e8 * x.powf(-3.0);
                }
                for (on, h) in [(284.0, 120.0), (532.0, 40.0)] {
                    if x >= on {
                        v += h * (on / x).powi(2);
                    }
                }
                v
            })
            .collect();
        let req = EelsReq {
            values: y,
            start,
            step,
            edges: vec![],
            pre: 0.0,
            window: 50.0,
            sigma: vec![("C K".into(), 1.0), ("O K".into(), 1.0)],
        };
        let r = eels(&req).unwrap();
        let labels: Vec<&str> = r.edges.iter().map(|m| m.label.as_str()).collect();
        assert!(
            labels.contains(&"C K") && labels.contains(&"O K"),
            "{labels:?}"
        );
        let c = r.edges.iter().find(|m| m.label == "C K").unwrap();
        // Expected net over [284, 334]: ∫ 120·(284/E)² dE ≈ 120·284·(1 − 284/334) per eV / step.
        let want = 120.0 * 284.0 * (1.0 - 284.0 / 334.0) / step;
        assert!(
            (c.signal - want).abs() / want < 0.1,
            "C K {} want {want}",
            c.signal
        );
        assert!((c.r - 3.0).abs() < 0.2, "r {}", c.r);
        assert!(
            r.t_lambda.is_some_and(|t| t > 0.0 && t < 1.0),
            "{:?}",
            r.t_lambda
        );
        assert!(
            c.at.is_some(),
            "{:?}",
            r.edges
                .iter()
                .map(|m| (&m.label, m.onset, m.signal / m.err, m.jump))
                .collect::<Vec<_>>()
        );
    }

    /// Runs on a spectrum file: TMV_SPEC (one value per line), TMV_SPEC_KIND
    /// (EDS or EELS), TMV_SPEC_START and _STEP (eV), TMV_SPEC_KV, TMV_SPEC_EL
    /// (elements or edges, comma separated). Prints the result as JSON and
    /// writes it whole to TMV_SPEC_OUT if set.
    #[test]
    #[ignore]
    fn bench_file() {
        let var = |k: &str| std::env::var(k).ok();
        let values: Vec<f64> = std::fs::read_to_string(var("TMV_SPEC").unwrap())
            .unwrap()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect();
        let start: f64 = var("TMV_SPEC_START").unwrap().parse().unwrap();
        let step: f64 = var("TMV_SPEC_STEP").unwrap().parse().unwrap();
        let list: Vec<String> = var("TMV_SPEC_EL")
            .map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let t = std::time::Instant::now();
        let json = if var("TMV_SPEC_KIND").as_deref() == Some("EELS") {
            let r = eels(&EelsReq {
                values,
                start,
                step,
                edges: list,
                pre: 0.0,
                window: 50.0,
                sigma: vec![],
            })
            .unwrap();
            if let Some(out) = var("TMV_SPEC_OUT") {
                std::fs::write(out, serde_json::to_string(&r).unwrap()).unwrap();
            }
            serde_json::to_string_pretty(&serde_json::json!({ "edges": r.edges.iter().map(|m| serde_json::json!({"label": m.label, "onset": m.onset, "signal": m.signal, "err": m.err, "jump": m.jump, "rise": m.rise, "riseSnr": m.rise_snr, "r": m.r, "alt": m.alt})).collect::<Vec<_>>(), "tLambda": r.t_lambda })).unwrap()
        } else {
            let kv = var("TMV_SPEC_KV").map_or(200.0, |v| v.parse().unwrap());
            let r = eds(&EdsReq {
                values,
                start,
                step,
                e0_kv: kv,
                elements: list,
                fwhm_mn: 130.0,
                k: vec![],
            })
            .unwrap();
            if let Some(out) = var("TMV_SPEC_OUT") {
                std::fs::write(out, serde_json::to_string(&r).unwrap()).unwrap();
            }
            serde_json::to_string_pretty(&serde_json::json!({ "peaks": r.peaks, "quant": r.quant }))
                .unwrap()
        };
        eprintln!("{:.1} ms", t.elapsed().as_secs_f64() * 1e3);
        println!("{json}");
    }
}
