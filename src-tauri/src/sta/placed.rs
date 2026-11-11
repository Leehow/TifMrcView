//! Looking at a finished average (Pro): the kinds its particles fall into,
//! with a small picture each (`kinds_of` / `keep_kinds` of `project`), and
//! where its particles lie on a tomogram, with the way each one stands
//! (`placed`). The workspace, the desktop commands and `tmv sta --kinds` use these.
//!
//! The folder of a result is the one `auto::results_of` lists (`which`:
//! "auto", "points", "drawn", "own"; none: the newest). `kinds_of` writes
//! `<dir>/kinds/kinds.json`; the average of the kept kinds goes to `<dir>/kinds/kept`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::auto::{self, resolve_tomogram, result_value, slices_png, Emit};
use super::project as sp;
use super::review::{events, Frame, State};
use crate::tomo::dataset::Look;

/// The folder of the result `which` of the output folder `out`, else the newest one.
pub fn result_dir(out: &Path, which: Option<&str>) -> Result<PathBuf> {
    let all = auto::results_of(out);
    match which.and_then(|w| all.iter().find(|(n, _)| *n == w)).or(all.first()) {
        Some((_, dir)) => Ok(dir.clone()),
        None => bail!("这里还没有平均结果"),
    }
}

fn json_of(path: &Path) -> Option<Value> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Whether the first average of `dir` came from the tilt images (it can then be sharper than one from the tomograms).
fn from_tilt_images(dir: &Path) -> bool {
    json_of(&dir.join("sta.json")).is_some_and(|r| r["averagedFrom"] == "tilt images")
}

// ---- kinds ----

/// `kinds.json` (what `kinds_of` wrote) as the workspace shows it: per kind
/// `{id, count, share (of all the particles in kinds, 0–1), png}` (the three
/// central sections of the kind's map; `png` null without a picture maker
/// or a map), without the rows; plus `apart`, `particles`, `voxelA`,
/// `dir` and `fromTiltImages`. `kinds` is empty when the particles are of one kind.
pub fn kinds_shown(listed: &Value, dir: &Path, b64: &dyn Fn(&[u8]) -> String) -> Value {
    let list: Vec<&Value> = listed["kinds"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let total: u64 = list.iter().filter_map(|k| k["count"].as_u64()).sum();
    let kinds: Vec<Value> = list
        .iter()
        .map(|k| {
            let count = k["count"].as_u64().unwrap_or(0);
            let png = k["map"]
                .as_str()
                .and_then(|m| crate::spa::data::read_volume(m).ok())
                .and_then(|(vol, dims, _)| slices_png(&vol, dims[0], 96).ok())
                .map(|p| b64(&p))
                .filter(|s| !s.is_empty());
            json!({ "id": k["id"], "count": count, "share": if total > 0 { count as f64 / total as f64 } else { 0.0 }, "map": k["map"], "png": png })
        })
        .collect();
    json!({
        "kinds": kinds, "apart": listed["apart"], "particles": listed["particles"], "voxelA": listed["voxelA"],
        "dir": dir, "fromTiltImages": from_tilt_images(dir),
    })
}

/// The kinds saved for the result in `dir`, or None when there are none or
/// the average was made again since (then they describe other particles).
pub fn kinds_saved_in(dir: &Path, b64: &dyn Fn(&[u8]) -> String) -> Option<Value> {
    fresh_kinds(dir).map(|listed| kinds_shown(&listed, dir, b64))
}

/// `kinds.json` of `dir` as it is on disk, when it is not older than the average.
fn fresh_kinds(dir: &Path) -> Option<Value> {
    let file = dir.join("kinds").join("kinds.json");
    let listed = json_of(&file)?;
    if modified(&dir.join("sta.json")).zip(modified(&file)).is_some_and(|(avg, kinds)| avg > kinds) {
        return None;
    }
    Some(listed)
}

/// `kinds_saved_in` for the result `which` of `out`.
pub fn kinds_saved(out: &str, which: Option<&str>, b64: &dyn Fn(&[u8]) -> String) -> Result<Option<Value>> {
    Ok(kinds_saved_in(&result_dir(Path::new(out), which)?, b64))
}

fn sources_of(inputs: &[String], note: &(dyn Fn(auto::Note) + Sync)) -> Vec<sp::Source> {
    inputs.iter().map(|i| resolve_tomogram(i, note).source()).collect()
}

/// Sorts the particles of the result `which` of `out` into kinds
/// (`project::kinds_of`) and returns `kinds_shown` of it. Events as `sta_run`.
pub fn run_kinds(inputs: &[String], out: &str, which: Option<&str>, emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    let dir = result_dir(Path::new(out), which)?;
    events(emit, b64, |progress, _live, note| {
        let sources = sources_of(inputs, note);
        let listed = sp::kinds_of(&sources, &dir.to_string_lossy(), progress)?;
        Ok(kinds_shown(&listed, &dir, b64))
    })
}

/// Averages only the kinds `keep` of the result `which` of `out`
/// (`project::keep_kinds`) into `<dir>/kinds/kept`. Returns `{report, result
/// (as `auto::result_value` of the kept average), ids, fromTiltImages, dir}`.
pub fn run_keep(inputs: &[String], out: &str, which: Option<&str>, keep: &[usize], emit: Emit, b64: &(dyn Fn(&[u8]) -> String + Sync)) -> Result<Value> {
    if keep.is_empty() {
        bail!("请至少选一个种类");
    }
    let dir = result_dir(Path::new(out), which)?;
    events(emit, b64, |progress, live, note| {
        let sources = sources_of(inputs, note);
        let report = sp::keep_kinds(&sources, &dir.to_string_lossy(), keep, progress, live)?;
        let kept = dir.join("kinds").join("kept");
        Ok(json!({
            "report": serde_json::to_value(&report)?, "result": result_value(&kept, b64)?, "ids": keep,
            "fromTiltImages": from_tilt_images(&dir), "dir": dir,
        }))
    })
}

/// The kind ids from a `--keep` list: "1,3" (spaces allowed), at least one, from 1.
pub fn parse_keep(text: &str) -> Result<Vec<usize>> {
    let mut ids = vec![];
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let id: usize = part.parse().ok().filter(|n| *n >= 1).with_context(|| format!("--keep: `{part}` is not a kind number (1, 2, 3, …)"))?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        bail!("--keep needs kind numbers, e.g. --keep 1,3");
    }
    Ok(ids)
}

// ---- where the particles lie ----

/// One row of `particles.csv`.
#[derive(Clone, Debug, PartialEq)]
pub struct Particle {
    pub tomogram: String,
    /// Centre, voxels of the tomogram.
    pub at: [f64; 3],
    /// Where the particle's Z axis points (the third column of its rotation), in the tomogram's x, y, z.
    pub axis: [f64; 3],
    pub score: f64,
}

/// The particles of a saved result, in the order of the rows of `particles.csv`
/// (the numbers `kinds.json` calls rows).
pub fn read_particles(dir: &Path) -> Result<Vec<Particle>> {
    let text = std::fs::read_to_string(dir.join("particles.csv")).context("读不到颗粒表")?;
    let mut out = vec![];
    for line in text.lines().skip(1) {
        // The tomogram's path may hold commas: the numbers are the last eleven fields.
        let f: Vec<&str> = line.rsplitn(12, ',').collect();
        if f.len() < 12 {
            continue;
        }
        let n = |k: usize| f[k].trim().parse::<f64>().with_context(|| format!("颗粒表里有读不懂的一行：{line}"));
        let r = crate::spa::geom::euler(n(7)?, n(6)?, n(5)?);
        out.push(Particle { tomogram: f[11].to_string(), at: [n(10)?, n(9)?, n(8)?], axis: [r[0][2], r[1][2], r[2][2]], score: n(4)? });
    }
    Ok(out)
}

/// Kind id per row of `particles.csv` and `(id, count)` of each kind, from `kinds.json` when it is up to date.
pub fn kind_ids(dir: &Path) -> (HashMap<usize, usize>, Vec<(usize, usize)>) {
    let mut of_row = HashMap::new();
    let mut kinds = vec![];
    if let Some(listed) = fresh_kinds(dir) {
        for k in listed["kinds"].as_array().into_iter().flatten() {
            let Some(id) = k["id"].as_u64().map(|i| i as usize) else { continue };
            let rows: Vec<usize> = k["rows"].as_array().into_iter().flatten().filter_map(|r| r.as_u64().map(|r| r as usize)).collect();
            for r in &rows {
                of_row.insert(*r, id);
            }
            kinds.push((id, rows.len()));
        }
    }
    (of_row, kinds)
}

fn same(a: &str, b: &str) -> bool {
    a == b || std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(x, y)| x == y)
}

/// The places of the particles of tomogram `raw` as the open file shows them
/// (`look`: the open file is its clear companion): `[{i (row), pos: [x, y, z],
/// axis: [x, y, z], kind (id or null), score}]`.
pub fn places_on(parts: &[Particle], kind_of: &HashMap<usize, usize>, raw: &str, look: Option<&Look>) -> Vec<Value> {
    let frame = Frame { source: 0, look };
    let r1 = |x: f64| (x * 10.0).round() / 10.0;
    let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
    parts
        .iter()
        .enumerate()
        .filter(|(_, p)| same(&p.tomogram, raw))
        .map(|(i, p)| {
            let at = State::to_open(&frame, p.at);
            json!({ "i": i, "pos": [r1(at[0]), r1(at[1]), r1(at[2])], "axis": p.axis.map(r3), "kind": kind_of.get(&i), "score": (p.score * 1e4).round() / 1e4 })
        })
        .collect()
}

/// The particles of the result `which` of `out` on the open file `open` (a
/// tomogram of the result or its clear companion): `{places, radius (of a
/// particle, in the open file's voxels, or null), open (the file is one of the
/// result's), look (it is the clear companion), perTomogram: [{name, path,
/// look, n}], kinds: [{id, count}], particles}`. `inputs` are the workspace's tomograms.
pub fn placed(out: &str, which: Option<&str>, inputs: &[String], open: &str) -> Result<Value> {
    let dir = result_dir(Path::new(out), which)?;
    let parts = read_particles(&dir)?;
    // The tomograms of the result in order of first appearance, each with its clear companion.
    let mut paths: Vec<String> = vec![];
    for p in &parts {
        if !paths.iter().any(|q| same(q, &p.tomogram)) {
            paths.push(p.tomogram.clone());
        }
    }
    let nothing = |_: auto::Note| {};
    let known: Vec<(String, Option<Look>)> = inputs.iter().map(|i| resolve_tomogram(i, &nothing)).map(|r| (r.tomogram, r.look)).collect();
    let looks: Vec<Option<Look>> = paths
        .iter()
        .map(|p| match known.iter().find(|(k, _)| same(k, p)) {
            Some((_, l)) => l.clone(),
            None => resolve_tomogram(p, &nothing).look,
        })
        .collect();
    let (of_row, kinds) = kind_ids(&dir);
    let per: Vec<Value> = paths
        .iter()
        .zip(&looks)
        .map(|(p, l)| {
            let name = Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
            json!({ "name": name, "path": p, "look": l.as_ref().map(|l| &l.path), "n": parts.iter().filter(|q| same(&q.tomogram, p)).count() })
        })
        .collect();
    let found = paths.iter().zip(&looks).find_map(|(p, l)| {
        if same(p, open) {
            Some((p, None))
        } else {
            l.as_ref().filter(|l| same(&l.path, open)).map(|l| (p, Some(l)))
        }
    });
    let (places, radius) = match found {
        Some((raw, look)) => {
            let radius = json_of(&dir.join("sta.json"))
                .and_then(|r| r["diameterA"].as_f64())
                .zip(sp::pixel_of(raw).ok().filter(|p| *p > 0.0))
                .map(|(d, px)| ((0.5 * d / px / look.map_or(1.0, |l| l.scale)) * 10.0).round() / 10.0);
            (places_on(&parts, &of_row, raw, look), json!(radius))
        }
        None => (vec![], Value::Null),
    };
    Ok(json!({
        "places": places, "radius": radius, "open": found.is_some(), "look": found.is_some_and(|f| f.1.is_some()),
        "perTomogram": per, "kinds": kinds.iter().map(|(id, n)| json!({ "id": id, "count": n })).collect::<Vec<_>>(),
        "particles": parts.len(), "dir": dir,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tmv_placed_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const HEAD: &str = "tomogram,x,y,z,rot,tilt,psi,score,half,blob,strength,darkest\n";

    #[test]
    fn particles_are_read_with_the_way_they_stand() {
        let d = scratch("read");
        // A path with a comma; the second particle is tilted 90° so its Z axis lies along X.
        let csv = format!("{HEAD}/data/a,b.mrc,10.00,20.00,30.00,0.00,0.00,0.00,0.5000,1,1.00,0.1000,2.00\n/data/c.mrc,5.00,6.00,7.00,0.00,90.00,0.00,0.4000,2,1.00,0.1000,2.00\nshort,line\n");
        std::fs::write(d.join("particles.csv"), csv).unwrap();
        let p = read_particles(&d).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].tomogram, "/data/a,b.mrc");
        assert_eq!(p[0].at, [10.0, 20.0, 30.0]);
        assert!((p[0].axis[2] - 1.0).abs() < 1e-9 && p[0].axis[0].abs() < 1e-9);
        assert!((p[1].axis[0].abs() - 1.0).abs() < 1e-9 && p[1].axis[2].abs() < 1e-9);
        assert_eq!(p[1].score, 0.4);
        assert!(read_particles(&d.join("nothing")).is_err());
    }

    #[test]
    fn places_follow_the_shown_file() {
        let parts = vec![
            Particle { tomogram: "/t/a.mrc".into(), at: [100.0, 50.0, 20.0], axis: [0.0, 0.0, 1.0], score: 0.5 },
            Particle { tomogram: "/t/b.mrc".into(), at: [1.0, 2.0, 3.0], axis: [1.0, 0.0, 0.0], score: 0.25 },
            Particle { tomogram: "/t/a.mrc".into(), at: [10.0, 10.0, 10.0], axis: [0.0, 1.0, 0.0], score: 0.1 },
        ];
        let kinds: HashMap<usize, usize> = [(0, 2), (2, 1)].into();
        // On the tomogram itself nothing moves; the other tomogram's particles are left out.
        let raw = places_on(&parts, &kinds, "/t/a.mrc", None);
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[0]["pos"], json!([100.0, 50.0, 20.0]));
        assert_eq!(raw[0]["kind"], json!(2));
        assert_eq!((raw[1]["i"].clone(), raw[1]["kind"].clone()), (json!(2), json!(1)));
        // On its clear companion (raw = 2 × look + offset) places are look voxels.
        let look = Look { path: "/t/a_look.mrc".into(), scale: 2.0, offset: [10.0, 0.0, -4.0] };
        let on_look = places_on(&parts, &kinds, "/t/a.mrc", Some(&look));
        assert_eq!(on_look[0]["pos"], json!([45.0, 25.0, 12.0]));
        assert_eq!(on_look[0]["axis"], json!([0.0, 0.0, 1.0]));
        let none = places_on(&parts, &HashMap::new(), "/t/b.mrc", None);
        assert_eq!(none[0]["kind"], Value::Null);
    }

    #[test]
    fn kinds_are_read_and_go_stale_with_the_average() {
        let d = scratch("kinds");
        std::fs::write(d.join("sta.json"), r#"{"averagedFrom":"tilt images"}"#).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::create_dir_all(d.join("kinds")).unwrap();
        let listed = json!({ "kinds": [{ "id": 1, "count": 3, "map": "/none.mrc", "rows": [0, 1, 4] }, { "id": 2, "count": 1, "map": "/none2.mrc", "rows": [2] }], "apart": 0.4, "particles": 5, "voxelA": 12.0 });
        std::fs::write(d.join("kinds").join("kinds.json"), serde_json::to_vec(&listed).unwrap()).unwrap();
        let shown = kinds_saved_in(&d, &|_| "x".into()).unwrap();
        assert_eq!(shown["kinds"].as_array().unwrap().len(), 2);
        assert_eq!(shown["kinds"][0]["share"], json!(0.75));
        assert_eq!(shown["kinds"][0]["png"], Value::Null);
        assert!(shown["kinds"][0].get("rows").is_none());
        assert_eq!(shown["fromTiltImages"], json!(true));
        let (of_row, kinds) = kind_ids(&d);
        assert_eq!((of_row.get(&4), of_row.get(&2), of_row.get(&3)), (Some(&1), Some(&2), None));
        assert_eq!(kinds, vec![(1, 3), (2, 1)]);
        // One kind only: an empty list, said plainly by the workspace.
        std::fs::write(d.join("kinds").join("kinds.json"), br#"{"kinds":[],"apart":null,"particles":5,"voxelA":12.0}"#).unwrap();
        assert!(kinds_saved_in(&d, &|_| String::new()).unwrap()["kinds"].as_array().unwrap().is_empty());
        // The average made again afterwards: the kinds no longer fit.
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(d.join("sta.json"), "{}").unwrap();
        assert!(kinds_saved_in(&d, &|_| String::new()).is_none());
        assert!(kinds_saved_in(&d.join("nothing"), &|_| String::new()).is_none());
    }

    #[test]
    fn the_result_folder_is_found_by_name_or_newest() {
        let d = scratch("dir");
        assert!(result_dir(&d, None).is_err());
        std::fs::write(d.join("average.mrc"), b"x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::create_dir_all(d.join("own")).unwrap();
        std::fs::write(d.join("own").join("average.mrc"), b"x").unwrap();
        assert_eq!(result_dir(&d, None).unwrap(), d.join("own"));
        assert_eq!(result_dir(&d, Some("auto")).unwrap(), d);
        assert_eq!(result_dir(&d, Some("points")).unwrap(), d.join("own"));
    }

    #[test]
    fn keep_lists_are_read() {
        assert_eq!(parse_keep("1,3").unwrap(), vec![1, 3]);
        assert_eq!(parse_keep(" 2 , 2,1 ").unwrap(), vec![2, 1]);
        assert!(parse_keep("").is_err());
        assert!(parse_keep("0").is_err());
        assert!(parse_keep("a,1").is_err());
    }
}
