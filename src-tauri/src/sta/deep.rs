//! 三维选择 of a painted example (`Kind::Paint` with `deep`): the glue
//! between the marks and the engine in `grow`. The strokes of a mark become
//! what was painted and erased on each section (`painted_of`), the file is
//! read a rectangle at a time, and the selection is kept next to the
//! project (`<out>/paint/<hash>.sel`, derived data, safe to delete) so that
//! showing a section, finding alike and averaging never follow it again.
//!
//! The name of a cache file holds everything the selection depends on: the
//! file (path, modification time, length), the strokes and `deep`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::brush::{self, Raster};
use super::grow::{self, End, Engine, Painted, Selection};
use super::marks::{Kind, Mark};
use super::paint::Stroke;

/// The folder of the cached selections in an output folder.
pub const DIR: &str = "paint";

/// Counts the calls of `shape`; the latest call of each slot (the example's
/// place in the list) is the one that goes on, an older one is cancelled.
static COUNT: AtomicU64 = AtomicU64::new(0);
static LATEST: Mutex<Vec<(usize, u64)>> = Mutex::new(Vec::new());

fn is_latest(slot: usize, me: u64) -> bool {
    LATEST.lock().unwrap().iter().any(|(s, n)| *s == slot && *n == me)
}

fn full(r: &Raster, nx: usize, ny: usize) -> Vec<bool> {
    let mut m = vec![false; nx * ny];
    for (x, y) in r.cells() {
        if x >= 0 && y >= 0 && (x as usize) < nx && (y as usize) < ny {
            m[y as usize * nx + x as usize] = true;
        }
    }
    m
}

/// What the strokes painted and erased, per section, as the engine takes
/// it: `take` is what is painted after all the strokes (later paint comes
/// back over an earlier erase), `erase` what an erasing stroke covered and
/// is not painted again. A section whose paint was all erased stays, with
/// nothing taken: it is where the user stopped the thing.
pub fn painted_of(strokes: &[Stroke], dims: [usize; 3]) -> Vec<Painted> {
    let [nx, ny, nz] = dims;
    let erasing: Vec<Stroke> = strokes.iter().filter(|s| s.cut).map(|s| Stroke { cut: false, ..s.clone() }).collect();
    let mut out = vec![];
    for z in brush::sections(strokes).into_iter().filter(|z| *z < nz) {
        let take = brush::section(strokes, z).map_or_else(Vec::new, |r| full(&r, nx, ny));
        let mut erase = brush::section(&erasing, z).map_or_else(Vec::new, |r| full(&r, nx, ny));
        if !take.is_empty() {
            erase.iter_mut().zip(&take).for_each(|(e, t)| *e &= !t);
        }
        if take.iter().chain(erase.iter()).any(|v| *v) {
            out.push(Painted { z, take, erase });
        }
    }
    out
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

/// The name (without extension) of the cache file of mark `m`; `None` when
/// its file cannot be seen.
pub fn hash_of(m: &Mark) -> Option<String> {
    let md = std::fs::metadata(&m.tomogram).ok()?;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    fnv(&mut h, m.tomogram.as_bytes());
    let secs = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    fnv(&mut h, &secs.to_le_bytes());
    fnv(&mut h, &md.len().to_le_bytes());
    fnv(&mut h, &[m.deep as u8]);
    for s in &m.strokes {
        fnv(&mut h, &[s.cut as u8]);
        fnv(&mut h, &s.width.to_bits().to_le_bytes());
        fnv(&mut h, &(s.points.len() as u64).to_le_bytes());
        for p in &s.points {
            for c in p {
                fnv(&mut h, &c.to_bits().to_le_bytes());
            }
        }
    }
    Some(format!("{h:016x}"))
}

pub fn sel_path(out: &Path, m: &Mark) -> Option<PathBuf> {
    Some(out.join(DIR).join(format!("{}.sel", hash_of(m)?)))
}

type Kept = (PathBuf, std::time::SystemTime, Arc<Selection>);
static KEPT: Mutex<Option<Kept>> = Mutex::new(None);

/// The selection of a painted mark with 三维选择 on, when it was made
/// before (and the strokes and the file are as they were).
pub fn cached(out: &Path, m: &Mark) -> Option<Arc<Selection>> {
    if m.kind != Kind::Paint || !m.deep {
        return None;
    }
    let path = sel_path(out, m)?;
    let when = std::fs::metadata(&path).ok()?.modified().ok()?;
    if let Some((p, t, s)) = KEPT.lock().unwrap().as_ref() {
        if *p == path && *t == when {
            return Some(s.clone());
        }
    }
    let s = Arc::new(Selection::load(&path).ok()?);
    *KEPT.lock().unwrap() = Some((path, when, s.clone()));
    Some(s)
}

/// The runs `[y, x0, x1)` of mark `m` on section `z`: what the selection
/// reaches there, or, where it does not, what was painted there. `None`
/// when there is no selection.
pub fn section_runs(out: &Path, m: &Mark, z: usize) -> Option<Vec<[usize; 3]>> {
    let s = cached(out, m)?;
    let runs = s.section(z);
    if !runs.is_empty() {
        return Some(runs.to_vec());
    }
    Some(brush::section(&m.strokes, z).map_or_else(Vec::new, |r| r.runs()))
}

/// Reads rectangles of the sections of an open file.
fn reader(img: Arc<crate::image::OpenImage>) -> impl Fn(usize, [usize; 4]) -> Result<Vec<f32>> + Sync {
    move |z, [x0, y0, w, h]| {
        let info = img.info();
        let (page, s) = info.locate(0, z, 0);
        let p = img.page(page)?;
        let p = &p[s.min(p.len() - 1)];
        let mut out = Vec::with_capacity(w * h);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                out.push(p.data.get_f64(y * info.width + x) as f32);
            }
        }
        Ok(out)
    }
}

/// Follows the paint of mark `m` (or loads what was followed before) and
/// keeps it under `out`. Returns the selection and the file's voxel size, Å.
/// A newer call for the same `slot` cancels this one, and so does
/// `sta::CANCEL` set meanwhile.
pub fn shape(out: &Path, m: &Mark, slot: usize, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<(Arc<Selection>, f64)> {
    if m.kind != Kind::Paint {
        bail!("这不是涂出来的例子");
    }
    let img = crate::tomo::pipeline::open(&m.tomogram)?;
    let px = crate::tomo::output::pixel_a(&img);
    if let Some(s) = cached(out, m) {
        return Ok((s, px));
    }
    let info = img.info();
    let dims = [info.width, info.height, info.size_z];
    let painted = painted_of(&m.strokes, dims);
    if painted.is_empty() {
        bail!("还没有涂");
    }
    let me = COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    {
        let mut l = LATEST.lock().unwrap();
        l.retain(|(s, _)| *s != slot);
        l.push((slot, me));
    }
    let was = super::CANCEL.load(Ordering::Relaxed);
    let cancel = move || !is_latest(slot, me) || (!was && super::CANCEL.load(Ordering::Relaxed));
    let read = reader(img.clone());
    let sel = grow::follow(&read, dims, &painted, None, m.deep, &cancel, progress)?;
    if let Some(path) = sel_path(out, m).filter(|_| m.deep) {
        sel.save(&path).with_context(|| format!("存不了 {}", path.display()))?;
    }
    Ok((Arc::new(sel), px))
}

/// What the program saw at an end, in words.
pub fn end_text(e: End) -> &'static str {
    match e {
        End::Ended => "到头了",
        End::Faded => "渐渐看不清了（缺失楔）",
        End::Joined => "和旁边的东西连在一起，停在这里",
        End::Through => "一直延伸出去，按宽度截取了一段",
        End::Edge => "到断层图的边了",
        End::Stopped => "停在你擦掉的那一层",
        End::Estimated => "厚度按大小估计",
    }
}

fn end_name(e: End) -> Value {
    serde_json::to_value(e).unwrap_or(Value::Null)
}

/// The side view as a PNG.
fn side_png(s: &Selection) -> Option<Vec<u8>> {
    if s.side.w == 0 || s.side.h == 0 {
        return None;
    }
    let (w, h, rgba) = s.side_rgba([255, 140, 0]);
    let mut buf = vec![];
    use ::image::ImageEncoder;
    ::image::codecs::png::PngEncoder::new(&mut buf).write_image(&rgba, w as u32, h as u32, ::image::ExtendedColorType::Rgba8).ok()?;
    Some(buf)
}

/// What the list row says: {z: [z0, z1], used, voxels, sizeNm: [x, y, z],
/// centre, engine, ends: [up, down], endText, estimated, sidePng}. `px` is
/// the voxel size, Å; `b64` encodes bytes.
pub fn summary(s: &Selection, px: f64, b64: &dyn Fn(&[u8]) -> String) -> Value {
    let nm = |v: usize| (v as f64 * px / 10.0 * 10.0).round() / 10.0;
    let flat = s.engine == Engine::Flat;
    let (_, used_h) = s.heights();
    let mut ends = s.ends;
    if flat {
        ends = [End::Estimated; 2];
    }
    let text: Vec<&str> = if flat {
        let t = if s.how.drawn_again > 0.0 { "没能沿上下找到边界，厚度按大小估计" } else { end_text(End::Estimated) };
        vec![t, t]
    } else {
        ends.iter().map(|e| end_text(*e)).collect()
    };
    json!({
        "z": s.z, "used": s.used, "voxels": s.voxels,
        "sizeNm": [nm(s.dims[0]), nm(s.dims[1]), nm(used_h)],
        "centre": s.centre, "radius": s.radius,
        "engine": s.engine, "estimated": flat,
        "ends": [end_name(ends[0]), end_name(ends[1])],
        "endText": text,
        "sidePng": side_png(s).map(|p| format!("data:image/png;base64,{}", b64(&p))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: &[[f64; 3]], width: f64, cut: bool) -> Stroke {
        Stroke { points: points.to_vec(), cut, width }
    }
    fn ring(cx: f64, cy: f64, z: f64, r: f64) -> Vec<[f64; 3]> {
        (0..=40).map(|k| k as f64 / 40.0 * std::f64::consts::TAU).map(|a| [cx + r * a.cos(), cy + r * a.sin(), z]).collect()
    }
    fn paint_mark(path: &str, strokes: Vec<Stroke>, deep: bool) -> Mark {
        let mut m: Mark = serde_json::from_value(json!({ "kind": "paint", "tomogram": path, "points": [] })).unwrap();
        m.strokes = strokes;
        m.deep = deep;
        m
    }

    #[test]
    fn strokes_become_what_is_painted_and_erased_per_section() {
        let dims = [64usize, 64, 20];
        let fill = stroke(&ring(30.0, 30.0, 5.0, 8.0), 3.0, false);
        let cut = stroke(&[[20.0, 30.0, 5.0], [40.0, 30.0, 5.0]], 4.0, true);
        let wipe = stroke(&[[30.0, 30.0, 9.0], [30.0, 55.0, 9.0]], 10.0, true);
        let again = stroke(&[[30.0, 30.0, 9.0]], 6.0, false);
        let all = stroke(&[[10.0, 10.0, 12.0]], 6.0, false);
        let all_gone = stroke(&[[10.0, 10.0, 12.0]], 20.0, true);
        let p = painted_of(&[fill.clone(), cut, wipe, again, all, all_gone], dims);
        let zs: Vec<usize> = p.iter().map(|p| p.z).collect();
        assert_eq!(zs, vec![5, 9, 12]);
        let at = |m: &Vec<bool>, x: usize, y: usize| m[y * 64 + x];
        // Section 5: painted, with a stripe erased across it.
        assert!(at(&p[0].take, 30, 22) && !at(&p[0].take, 30, 30) && at(&p[0].erase, 30, 30) && !at(&p[0].erase, 30, 22));
        // Section 9: the wipe was painted over again where the dab is; the rest stays erased.
        assert!(at(&p[1].take, 30, 30) && !at(&p[1].erase, 30, 30) && at(&p[1].erase, 30, 50));
        // Section 12: all erased, nothing taken, but there: a stop.
        assert!(p[2].take.iter().all(|v| !v) && at(&p[2].erase, 10, 10));
        // Sections beyond the file are left out.
        assert!(painted_of(&[stroke(&[[5.0, 5.0, 25.0]], 4.0, false), fill], dims).iter().all(|p| p.z < 20));
        assert!(painted_of(&[], dims).is_empty());
    }

    #[test]
    fn hash_follows_strokes_file_and_deep() {
        let dir = std::env::temp_dir().join(format!("tmv-deep-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.mrc");
        std::fs::write(&f, b"abc").unwrap();
        let path = f.to_string_lossy().into_owned();
        let mut m = paint_mark(&path, vec![stroke(&[[1.0, 2.0, 3.0]], 4.0, false)], true);
        let h = hash_of(&m).unwrap();
        assert_eq!(h, hash_of(&m.clone()).unwrap());
        m.strokes[0].points[0][0] = 1.5;
        assert_ne!(h, hash_of(&m).unwrap());
        let mut d = m.clone();
        d.deep = false;
        assert_ne!(hash_of(&m).unwrap(), hash_of(&d).unwrap());
        let before = hash_of(&m).unwrap();
        std::fs::write(&f, b"abcd").unwrap();
        assert_ne!(before, hash_of(&m).unwrap());
        m.tomogram = "/nonexistent/x.mrc".into();
        assert!(hash_of(&m).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tomogram with a dark ball of radius 7 voxels in grain, painted on its middle section.
    #[test]
    fn a_painted_ball_is_followed_cached_and_shown_on_its_sections() {
        let dims = [64usize, 64, 40];
        let dir = std::env::temp_dir().join(format!("tmv-deep-ball-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.mrc").to_string_lossy().into_owned();
        let mut v = vec![0f32; dims[0] * dims[1] * dims[2]];
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let d = ((x as f32 - 32.0).powi(2) + (y as f32 - 32.0).powi(2) + (z as f32 - 20.0).powi(2)).sqrt();
                    let grain = ((((x + 64 * y + 4096 * z) as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40) as f32 / (1u64 << 24) as f32) - 0.5;
                    v[(z * dims[1] + y) * dims[0] + x] = 10.0 + 0.2 * grain - if d <= 7.0 { 4.0 } else { 0.0 };
                }
            }
        }
        crate::tomo::output::write_mrc(&path, &v, dims, 10.0, false).unwrap();
        let m = paint_mark(&path, vec![stroke(&ring(32.0, 32.0, 20.0, 5.0), 3.0, false)], true);
        let out = dir.join("out");
        assert!(cached(&out, &m).is_none() && section_runs(&out, &m, 20).is_none());
        let (s, px) = shape(&out, &m, 0, &|_, _| {}).unwrap();
        assert_eq!(px, 10.0);
        assert!(s.z[0] <= 20 && s.z[1] >= 20 && s.z[0] >= 8 && s.z[1] <= 32, "{:?}", s.z);
        // It is on disk, loaded again equal, and the section runs come from it.
        assert!(sel_path(&out, &m).unwrap().exists());
        let c = cached(&out, &m).unwrap();
        // (floats may differ in the last digit after the text form)
        assert_eq!((&c.shown, c.z, c.used, c.engine, c.ends, c.voxels, c.side.w), (&s.shown, s.z, s.used, s.engine, s.ends, s.voxels, s.side.w));
        assert!((c.radius - s.radius).abs() < 1e-9 && (c.width - s.width).abs() < 1e-9);
        assert!(!section_runs(&out, &m, 20).unwrap().is_empty());
        if s.engine == Engine::Cut {
            assert!(!section_runs(&out, &m, s.z[0] + 1).unwrap().is_empty());
        }
        // Far from it: nothing.
        assert!(section_runs(&out, &m, 2).unwrap().is_empty());
        // Changed strokes have no cache.
        let mut m2 = m.clone();
        m2.strokes.push(stroke(&[[32.0, 32.0, 22.0]], 4.0, false));
        assert!(cached(&out, &m2).is_none());
        // 三维选择 off: estimated thickness, not cached, texts say so.
        let off = paint_mark(&path, m.strokes.clone(), false);
        let (f, _) = shape(&out, &off, 0, &|_, _| {}).unwrap();
        assert_eq!(f.engine, Engine::Flat);
        assert!(cached(&out, &off).is_none());
        let j = summary(&f, px, &|b| format!("{}", b.len()));
        assert_eq!(j["engine"], "flat");
        assert_eq!(j["endText"][0], "厚度按大小估计");
        assert!(j["sidePng"].as_str().is_none_or(|s| s.starts_with("data:image/png;base64,")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
