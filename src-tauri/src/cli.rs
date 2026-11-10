//! `tmv`: TifMrcView Pro's processing without a window, for Linux servers
//! that have a GPU but no display. Coding agents (Claude Code, Codex) copy
//! data there over SSH, run one command per purpose, and copy the results
//! back to open them in the app.
//!
//! The commands are the app's one-click flows with the same code and the
//! same automatic choices: no method switches or tuning knobs. Results are
//! written beside the input (as the app writes them), so copying the folder
//! back is enough. Progress goes to stderr; the result is one JSON object
//! on stdout and is also saved as `<input>.tmv.json`.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::{formats, image::OpenImage};

/// The release version (CI sets TMV_VERSION), else the crate's.
pub const VERSION: &str = match option_env!("TMV_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

const HELP: &str = r#"tmv — TifMrcView Pro command line (no window needed)

USAGE
  tmv run <path>                 Detect what <path> is and run the right command
  tmv tomo <tilt-series|.mdoc>   One-click cryo-ET reconstruction
        [--for display|sta|survey] [--pixel Å]
  tmv sta <tilt-series|tomogram> [MORE …]
                                 Subtomogram averaging: cards, then the average
        [--pick N] [--out DIR] [--pixel Å]
  tmv membranes <tomogram.mrc>   Membranes, vesicles and microtubules → label map
        [--model membrain.ckpt]
  tmv spa <folder>               One-click single-particle analysis
        [--kind N[,M]] [--symmetry C1|D7|…] [--pixel Å] [--kv kV]
        [--dose e/Å²] [--gain file] [--fresh]
  tmv ctf <folder>               CTF of every micrograph (movies are motion corrected)
        [--pixel Å] [--kv kV] [--dose e/Å²] [--gain file] [--fresh]
  tmv denoise <image|stack>      Self-supervised denoising → <name>_denoised.mrc
  tmv info                       Version, GPU and models found
  tmv setup [--membrane membrain.ckpt] [--sam]
                                 Remember the MemBrain-seg model; download SAM 2.1

  Every choice is made automatically, as in the app. --for picks what the
  tomogram is for: display (default: clearest picture, denoised, missing
  wedge filled, membranes when a model is set), sta (subtomogram averaging:
  full detail + RELION 5 folder), survey (quick look).

  tmv sta takes tilt series (made into tomograms first, as tmv tomo --for sta
  does) or tomograms that tmv tomo --for sta wrote. It runs in two steps:
  class-average "cards" of the blobs found go to <out>/cards/ (--out defaults
  to <first input>_sta beside it); then, with --pick N (or when card 0 is
  the only likely one), the average of everything that looks like card N
  goes to <out>/ (average.mrc, half1.mrc, half2.mrc, particles.csv, sta.json).

OUTPUT
  Files are written beside the input; copy the folder back and open them in
  TifMrcView Pro (label maps open with their tomogram). Progress goes to
  stderr. The result is one JSON object on stdout ({"ok":true,…}), also
  saved as <input>.tmv.json (tmv tomo --for sta|survey: <input>.sta.tmv.json,
  <input>.survey.tmv.json). Exit status 0 on success, 1 on error
  ({"ok":false,"error":…}), 2 when a choice is needed (spa: pick a kind;
  sta: pick a card).

  The GPU is found automatically (CUDA with an NVIDIA driver, else Vulkan,
  else CPU). On a shared server pick the card with CUDA_VISIBLE_DEVICES=N;
  TMV_GPU=cuda|vulkan|cpu forces a backend. Long runs:
  nohup tmv … > run.log 2>&1 &
"#;

/// Exit status when the user must choose before the command can go on.
const NEEDS_CHOICE: i32 = 2;

/// `tmv sta` asks `run_template` for this times the card's size: the blobs
/// the cards are made from read small (a 210 Å card goes with a 280 Å particle).
const DIAMETER_FACTOR: f64 = 1.33;

/// Runs the tool with `args` (without the program name); returns the exit status.
pub fn main(args: Vec<String>) -> i32 {
    use_bundled_cuda();
    // Whoever picks a card with CUDA_VISIBLE_DEVICES (the usual way on shared
    // servers) gets that card: only CUDA honours it, Vulkan would not.
    // CUDA also comes first wherever the NVIDIA driver is loaded: on an
    // RTX 3090 server it segmented membranes 1.7x faster than Vulkan
    // (3.3 vs 5.5 s per window). The CPU stays the fallback.
    let nvidia = Path::new("/dev/nvidiactl").exists() || Path::new("/dev/dxg").exists();
    if (nvidia || std::env::var_os("CUDA_VISIBLE_DEVICES").is_some()) && std::env::var_os("TMV_GPU").is_none() {
        std::env::set_var("TMV_GPU", "cuda");
    }
    let Some(cmd) = args.first().map(String::as_str) else {
        print!("{HELP}");
        return 0;
    };
    if matches!(cmd, "-h" | "--help" | "help") {
        print!("{HELP}");
        return 0;
    }
    if matches!(cmd, "-V" | "--version" | "version") {
        println!("tmv {VERSION}");
        return 0;
    }
    if args[1..].iter().any(|a| matches!(a.as_str(), "-h" | "--help")) {
        print!("{HELP}");
        return 0;
    }
    let opts = match Opts::parse(&args[1..]) {
        Ok(o) => o,
        Err(e) => return fail(cmd, &e, None),
    };
    if cmd != "sta" {
        if let Some(extra) = opts.more.first() {
            return fail(cmd, &anyhow!("unexpected argument `{extra}` (one input at a time)"), result_file(cmd, &opts));
        }
    }
    let started = Instant::now();
    if matches!(cmd, "run" | "tomo" | "sta" | "membranes" | "membrane" | "spa" | "ctf" | "denoise") {
        let _ = gpu();
    }
    let result = match cmd {
        "run" => run(&opts),
        "tomo" => tomo(&opts),
        "sta" => sta(&opts),
        "membranes" | "membrane" => membranes(&opts),
        "spa" => spa(&opts),
        "ctf" => ctf(&opts),
        "denoise" => denoise(&opts),
        "info" => info(),
        "setup" => setup(&opts),
        other => Err(anyhow!("unknown command `{other}` (see tmv --help)")),
    };
    match result {
        Ok(mut v) => {
            let status = if v.get("needsChoice").and_then(Value::as_bool) == Some(true) { NEEDS_CHOICE } else { 0 };
            if let Value::Object(o) = &mut v {
                let mut head = serde_json::Map::new();
                head.insert("ok".into(), json!(status == 0));
                head.insert("command".into(), json!(o.remove("command").unwrap_or_else(|| json!(cmd))));
                head.insert("version".into(), json!(VERSION));
                head.insert("seconds".into(), json!((started.elapsed().as_secs_f64() * 10.0).round() / 10.0));
                head.extend(std::mem::take(o));
                v = Value::Object(head);
            }
            emit(&v, result_file(cmd, &opts));
            status
        }
        Err(e) => fail(cmd, &e, result_file(cmd, &opts)),
    }
}

fn fail(cmd: &str, e: &anyhow::Error, file: Option<PathBuf>) -> i32 {
    let msg = format!("{e:#}");
    eprintln!("tmv {cmd}: {msg}");
    let mut v = json!({ "ok": false, "command": cmd, "version": VERSION, "error": msg });
    if let Some(h) = hint(&msg) {
        v["hint"] = json!(h);
    }
    emit(&v, file);
    1
}

/// What to do about the common errors, for whoever reads the JSON.
fn hint(msg: &str) -> Option<&'static str> {
    if msg.contains("像素大小和电压") {
        Some("pass the pixel size of the raw data and the acceleration voltage: --pixel <Å> --kv <kV>")
    } else if msg.contains("像素大小") || msg.contains("pixel size") {
        Some("pass the pixel size of the raw data with --pixel <Å>")
    } else if msg.contains("电压") {
        Some("pass the acceleration voltage with --kv <kV>")
    } else if msg.contains("MemBrain") || msg.contains("membrane model") {
        Some("download a MemBrain-seg checkpoint (.ckpt) from https://github.com/teamtomo/membrain-seg, then: tmv setup --membrane <file.ckpt>")
    } else if msg.contains("倾转角") {
        Some("put the .mdoc (or IMOD .tlt/.rawtlt) with the same name beside the tilt series")
    } else {
        None
    }
}

/// Where the result of `cmd` is saved: beside the input (`INPUT.tmv.json`,
/// `tmv.json` in a folder), but `tmv sta` saves `<out>/sta.tmv.json`, as its
/// input may be a tilt series whose `.tmv.json` `tmv tomo` wrote. `tmv tomo`
/// with a purpose other than display adds it (`INPUT.sta.tmv.json`,
/// `INPUT.survey.tmv.json`), so the results for one stack do not overwrite
/// each other.
fn result_file(cmd: &str, o: &Opts) -> Option<PathBuf> {
    let input = o.input.as_deref()?;
    if cmd == "sta" {
        if let Ok(out) = sta_out(o, input) {
            return Some(out.join("sta.tmv.json"));
        }
    }
    let p = Path::new(input);
    let purpose = match (cmd, o.get("for")) {
        ("tomo", Some("sta" | "averaging")) => ".sta",
        ("tomo", Some("survey" | "quick")) => ".survey",
        _ => "",
    };
    Some(if p.is_dir() { p.join("tmv.json") } else { PathBuf::from(format!("{input}{purpose}.tmv.json")) })
}

/// Prints the result and saves it to `file`.
fn emit(v: &Value, file: Option<PathBuf>) {
    let text = serde_json::to_string_pretty(v).unwrap_or_default();
    println!("{text}");
    let _ = std::io::stdout().flush();
    if let Some(file) = file {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, text + "\n");
    }
}

/// Command-line options: one input path and `--key value` pairs.
#[derive(Default)]
struct Opts {
    input: Option<String>,
    /// Further inputs (only `tmv sta` takes them).
    more: Vec<String>,
    named: Vec<(String, Option<String>)>,
}

impl Opts {
    fn parse(args: &[String]) -> Result<Opts> {
        const FLAGS: [&str; 2] = ["--sam", "--fresh"];
        let mut o = Opts::default();
        let mut it = args.iter().peekable();
        while let Some(a) = it.next() {
            if let Some(k) = a.strip_prefix("--") {
                let (k, v) = match k.split_once('=') {
                    Some((k, v)) => (k.to_string(), Some(v.to_string())),
                    None if FLAGS.contains(&a.as_str()) => (k.to_string(), None),
                    None => (k.to_string(), Some(it.next().cloned().with_context(|| format!("{a} needs a value"))?)),
                };
                o.named.push((k, v));
            } else {
                let p = Path::new(a);
                // Absolute, so the paths saved in project files stay valid.
                let abs = std::fs::canonicalize(p).with_context(|| format!("{a}: no such file or folder"))?.to_string_lossy().into_owned();
                if o.input.is_none() {
                    o.input = Some(abs);
                } else {
                    o.more.push(abs);
                }
            }
        }
        Ok(o)
    }

    fn input(&self) -> Result<&str> {
        self.input.as_deref().context("give the input file or folder")
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.named.iter().rev().find(|(k, _)| k == key).and_then(|(_, v)| v.as_deref())
    }

    fn flag(&self, key: &str) -> bool {
        self.named.iter().any(|(k, _)| k == key)
    }

    fn number(&self, key: &str) -> Result<Option<f64>> {
        self.get(key)
            .map(|s| s.parse::<f64>().ok().filter(|v| *v > 0.0).with_context(|| format!("--{key}: `{s}` is not a positive number")))
            .transpose()
    }
}

/// Progress on stderr: on a terminal one updating line, otherwise (logs)
/// a line when the step changes or every 20 s.
struct Progress {
    start: Instant,
    tty: bool,
    last: Mutex<(String, usize, Instant)>,
}

impl Progress {
    fn new() -> Progress {
        Progress { start: Instant::now(), tty: std::io::stderr().is_terminal(), last: Mutex::new((String::new(), usize::MAX, Instant::now())) }
    }

    fn report(&self, stage: &str, done: usize, total: usize) {
        let tenth = if total > 0 { done * 10 / total } else { 0 };
        let mut last = self.last.lock().unwrap();
        let line = if total > 1 { format!("{stage} {done}/{total}") } else { stage.to_string() };
        let t = self.start.elapsed().as_secs_f64();
        if self.tty {
            if last.2.elapsed() > Duration::from_millis(200) || last.0 != stage {
                eprint!("\r\x1b[2K[{t:7.1} s] {line}");
                *last = (stage.to_string(), tenth, Instant::now());
            }
            return;
        }
        // Logs: a line when the step changes ("第 2/8 步 · …" up to the
        // dot; sub-stage texts change too often), else at most every 20 s.
        let step = stage.split(" · ").next().unwrap_or(stage);
        let last_step = last.0.split(" · ").next().unwrap_or(&last.0).to_string();
        if last_step != step || (done == total && total > 1 && last.1 != 10) || last.2.elapsed() > Duration::from_secs(20) {
            eprintln!("[{t:7.1} s] {line}");
            *last = (stage.to_string(), tenth, Instant::now());
        }
    }

    fn done(&self) {
        if self.tty {
            eprintln!();
        }
    }
}

/// Points CUDA at the libdevice.10.bc shipped beside the binary (the
/// tarball's `cuda/`), unless CUDA_PATH is set.
fn use_bundled_cuda() {
    if std::env::var_os("CUDA_PATH").is_some() {
        return;
    }
    let Some(dir) = std::env::current_exe().ok().and_then(|e| std::fs::canonicalize(e).ok()).and_then(|e| e.parent().map(Path::to_path_buf)) else { return };
    for d in [dir.join("cuda"), dir.join("../lib/tmv/cuda"), dir.join("../cuda")] {
        if d.join("nvvm/libdevice/libdevice.10.bc").is_file() {
            std::env::set_var("CUDA_PATH", d);
            return;
        }
    }
}

/// Runs `f` without the panic messages of backends that fail to start
/// (probing CUDA without a driver panics inside its library).
fn quiet<T>(f: impl FnOnce() -> T) -> T {
    if std::env::var_os("TMV_DEBUG").is_some() {
        return f();
    }
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = f();
    std::panic::set_hook(hook);
    r
}

/// The GPU found (Vulkan, CUDA, … see `tmv_sam::backends`), or None.
fn gpu() -> Option<&'static (burn::tensor::Device, &'static str)> {
    quiet(crate::ctf_gpu::device)
}

/// The GPU (CPU with a warning when there is none).
fn device() -> (&'static burn::tensor::Device, &'static str) {
    static CPU: std::sync::OnceLock<burn::tensor::Device> = std::sync::OnceLock::new();
    match gpu() {
        Some((dev, name)) => {
            eprintln!("GPU: {name}");
            (dev, name)
        }
        None => {
            eprintln!("warning: no GPU found (driver missing?), computing on the CPU — this can take many hours");
            (CPU.get_or_init(|| crate::classify::device().0), "CPU")
        }
    }
}

fn open_image(path: &str) -> Result<std::sync::Arc<OpenImage>> {
    Ok(OpenImage::new(1, formats::open(Path::new(path)).with_context(|| format!("cannot open {path}"))?, 256 << 20))
}

fn ext(path: &str) -> String {
    Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// The tilt angles found beside a stack (mdoc, .tlt, .rawtlt) when there
/// is one per section.
fn has_tilts(path: &str, nz: usize) -> bool {
    nz >= 3 && crate::tomo::geometry::metadata(path, nz).map(|m| m.angles.len() == nz).unwrap_or(false)
}

/// `tmv run`: picks the command for what the input is.
fn run(o: &Opts) -> Result<Value> {
    let input = o.input()?;
    let (cmd, why) = if Path::new(input).is_dir() {
        ("spa", "a folder of micrographs or movies")
    } else if ext(input) == "mdoc" {
        ("tomo", "a SerialEM .mdoc of a tilt series")
    } else {
        let img = open_image(input)?;
        let nz = img.info().size_z;
        if has_tilts(input, nz) {
            ("tomo", "a tilt series (tilt angles found beside it)")
        } else if nz >= 8 && crate::membrane::model_path().is_some() && img.info().width >= 64 {
            ("membranes", "a tomogram (3D volume without tilt angles)")
        } else {
            ("denoise", "an image or stack")
        }
    };
    eprintln!("tmv run: {why} → tmv {cmd}");
    let mut v = match cmd {
        "spa" => spa(o)?,
        "tomo" => tomo(o)?,
        "membranes" => membranes(o)?,
        _ => denoise(o)?,
    };
    v["command"] = json!(cmd);
    v["detected"] = json!(why);
    Ok(v)
}

/// `tmv tomo`: motion correction (from an mdoc), then the one-click
/// reconstruction of `tomo::auto` for the chosen purpose.
fn tomo(o: &Opts) -> Result<Value> {
    use crate::tomo::{auto, dataset::Dataset};
    let input = o.input()?;
    let purpose = match o.get("for").unwrap_or("display") {
        "display" | "show" | "presentation" => auto::Purpose::Display,
        "sta" | "averaging" => auto::Purpose::Sta,
        "survey" | "quick" => auto::Purpose::Survey,
        p => bail!("--for {p}: use display, sta or survey"),
    };
    let pixel = o.number("pixel")?;
    let progress = Progress::new();
    let report = |s: &str, i: usize, n: usize| progress.report(s, i, n);
    let mut motion = Value::Null;
    let stack = if ext(input) == "mdoc" {
        let stem = input.strip_suffix(".mdoc").unwrap_or(input);
        let stem = stem.strip_suffix(".mrc").unwrap_or(stem);
        let output = format!("{stem}.mrc");
        crate::motion::begin();
        let req = crate::motion::series::SeriesReq { mdoc: input.into(), folder: None, output, params: Default::default(), drop_tilts: true };
        let r = crate::motion::series::run(&req, &report)?;
        progress.done();
        let Some(path) = r.path.clone() else { bail!("no movies found beside the mdoc") };
        if r.angles.len() < 3 {
            bail!("fewer than 3 tilts found: cannot reconstruct");
        }
        motion = json!({ "stack": path, "even": r.even, "odd": r.odd, "tilts": r.angles.len(), "warnings": r.warnings });
        path
    } else {
        input.to_string()
    };
    let img = crate::tomo::pipeline::open(&stack)?;
    let info = img.info();
    let px = pixel.or_else(|| info.pixel_size.is_some().then(|| crate::align::pixel_a(info)));
    let mut d = Dataset::open(&info.path, info.size_z, px)?;
    drop(img);
    let membranes = purpose == auto::Purpose::Display && crate::membrane::model_path().is_some();
    if purpose == auto::Purpose::Display && !membranes {
        eprintln!("note: no MemBrain-seg model set, membranes are skipped (tmv setup --membrane <file.ckpt>)");
    }
    let (dev, backend) = device();
    let req = auto::AutoReq { pixel_a: pixel, purpose, fresh: o.flag("fresh") };
    let r = crate::ctf_gpu::guarded(|| auto::run(dev, &mut d, &req, &report))??;
    progress.done();
    let mut v = serde_json::to_value(&r)?;
    if let Value::Object(o) = &mut v {
        o.remove("ms");
    }
    // The app's note points at its own advanced settings; say it for here.
    if let Some(note) = &r.note {
        if note.contains("缺失楔") {
            let hours = note.split("约需").nth(1).and_then(|t| t.trim().split(' ').next()).unwrap_or("?");
            v["note"] = json!(format!("missing-wedge filling skipped: this GPU would need about {hours} h for it (a faster GPU does it automatically)"));
        }
    }
    v["backend"] = json!(backend);
    v["dataset"] = json!(crate::tomo::dataset::path_for(&d.stack));
    if !motion.is_null() {
        v["motion"] = motion;
    }
    if let Some(labels) = &r.labels {
        let summary = format!("{}.membranes.json", labels.strip_suffix(".labels.tif").unwrap_or(labels));
        if let Ok(s) = std::fs::read(&summary).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice::<Value>(&b)?)) {
            v["membranes"] = json!({
                "summary": summary,
                "vesiclesCsv": s["vesiclesCsv"],
                "vesicles": s["vesicles"].as_array().map_or(0, Vec::len),
                "microtubules": s["microtubules"].as_array().map_or(0, Vec::len),
            });
        }
    }
    v["open"] = json!(r.path);
    Ok(v)
}

/// What `tmv sta` knows about one input after its tomogram exists.
struct StaInput {
    input: String,
    tomogram: String,
    tilts: [f64; 2],
    tilts_from: &'static str,
    /// The clear companion `tmv tomo --for sta` made (blobs and the template
    /// are searched on it; the averages are of `tomogram`).
    look: Option<crate::tomo::dataset::Look>,
}

/// The look record kept with `tomogram` in its dataset, when its file is
/// there (also if the folder was copied: then the file beside the tomogram).
fn look_of(d: Option<&crate::tomo::dataset::Dataset>, tomogram: &str) -> Option<crate::tomo::dataset::Look> {
    let name = Path::new(tomogram).file_name()?;
    let mut look = d?.tomograms.iter().find(|t| Path::new(&t.path).file_name() == Some(name))?.look.clone()?;
    if !Path::new(&look.path).is_file() {
        let beside = Path::new(tomogram).with_file_name(Path::new(&look.path).file_name()?);
        if beside.is_file() {
            look.path = beside.to_string_lossy().into_owned();
        } else {
            eprintln!("tmv sta: the look tomogram {} of {tomogram} is missing: using the tomogram alone", look.path);
            return None;
        }
    }
    Some(look)
}

/// First and last tilt angle (°) of `angles`; None with fewer than two or all equal.
fn range_of(angles: &[f64]) -> Option<[f64; 2]> {
    let lo = angles.iter().copied().filter(|a| a.is_finite()).fold(f64::INFINITY, f64::min);
    let hi = angles.iter().copied().filter(|a| a.is_finite()).fold(f64::NEG_INFINITY, f64::max);
    (hi > lo).then_some([lo, hi])
}

/// The tilts used for the tomogram: the refined angles (levelling included)
/// of the alignment's views that were not left out, else the series' angles.
fn tilt_range(d: &crate::tomo::dataset::Dataset) -> Option<([f64; 2], &'static str)> {
    if let Some(a) = &d.alignment {
        let used: Vec<f64> = a.views.iter().filter(|v| !v.excluded).map(|v| v.refined()).collect();
        if let Some(r) = range_of(&used) {
            return Some((r, "alignment"));
        }
    }
    range_of(&d.angles).map(|r| (r, "tilt angles"))
}

/// The dataset (`*.tomo.json`) beside `tomogram` that lists it.
fn dataset_of(tomogram: &str) -> Option<crate::tomo::dataset::Dataset> {
    use crate::tomo::dataset::Dataset;
    let p = Path::new(tomogram);
    let name = p.file_name()?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(p.parent()?)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|f| f.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".tomo.json")))
        .collect();
    files.sort();
    files.into_iter().find_map(|f| {
        let d: Dataset = serde_json::from_slice(&std::fs::read(f).ok()?).ok()?;
        d.tomograms.iter().any(|t| Path::new(&t.path).file_name() == Some(name)).then_some(d)
    })
}

/// `--out`, else `<stem>_sta` beside the first input.
fn sta_out(o: &Opts, first: &str) -> Result<PathBuf> {
    if let Some(d) = o.get("out") {
        return std::path::absolute(d).with_context(|| format!("--out {d}"));
    }
    let p = Path::new(first);
    let stem = p.file_stem().unwrap_or_default().to_string_lossy();
    Ok(p.with_file_name(format!("{stem}_sta")))
}

/// Card 0 when it is likely and no other card is; else the user must choose.
fn auto_pick(cards: &[crate::sta::project::Card]) -> Option<usize> {
    (cards.first().is_some_and(|c| c.likely) && cards.iter().filter(|c| c.likely).count() == 1).then_some(0)
}

fn card_json(c: &crate::sta::project::Card) -> Value {
    json!({ "id": c.id, "diameterA": c.diameter_a, "members": c.members, "compact": (c.compact * 10.0).round() / 10.0, "likely": c.likely, "path": c.path })
}

/// An input tomogram as the cards were made from it (`cards/inputs.json`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct InputStamp {
    path: String,
    size: u64,
    /// Modified time, nanoseconds since the epoch (0 when unknown).
    modified_ns: u128,
}

fn input_stamps<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<InputStamp> {
    paths
        .map(|p| {
            let abs = std::path::absolute(p).unwrap_or_else(|_| PathBuf::from(p));
            let meta = std::fs::metadata(&abs).ok();
            let modified_ns = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            InputStamp { path: abs.to_string_lossy().into_owned(), size: meta.map_or(0, |m| m.len()), modified_ns }
        })
        .collect()
}

/// Why saved cards cannot be reused for the present inputs (None: they can,
/// the inputs are the same, in the same order).
fn stale(saved: Option<&[InputStamp]>, now: &[InputStamp]) -> Option<String> {
    let Some(saved) = saved else {
        return Some("cards/inputs.json is missing or unreadable, so it is not known which tomograms they were made from".into());
    };
    if saved.len() != now.len() {
        return Some(format!("they were made from {} tomogram(s), now there are {}", saved.len(), now.len()));
    }
    for (a, b) in saved.iter().zip(now) {
        if a.path != b.path {
            return Some(format!("they were made from {}, now {}", a.path, b.path));
        }
        if a.size != b.size || a.modified_ns != b.modified_ns {
            return Some(format!("{} has changed since they were made", b.path));
        }
    }
    None
}

/// `tmv sta`: the tomogram of each input (made as `tmv tomo --for sta` does),
/// class-average cards of the blobs in them, then the average of everything
/// that looks like the chosen card.
fn sta(o: &Opts) -> Result<Value> {
    use crate::sta::project as sp;
    let inputs: Vec<String> = o.input.iter().chain(&o.more).cloned().collect();
    if inputs.is_empty() {
        bail!("give at least one tilt series or tomogram: tmv sta [--pick N] [--out DIR] [--pixel Å] INPUT [MORE …]");
    }
    let pick = o.get("pick").map(|s| s.parse::<usize>().with_context(|| format!("--pick: `{s}` is not a card number (0, 1, 2, …)"))).transpose()?;
    let out = sta_out(o, &inputs[0])?;
    let progress = Progress::new();
    let report = |s: &str, i: usize, n: usize| progress.report(s, i, n);

    // Step 0: a tomogram and its tilt range for each input.
    let mut found: Vec<StaInput> = vec![];
    for input in &inputs {
        let (tomogram, dataset) = if dataset_of(input).is_none() && (ext(input) == "mdoc" || has_tilts(input, open_image(input)?.info().size_z)) {
            eprintln!("tmv sta: {input} is a tilt series → tmv tomo --for sta");
            let mut named = vec![("for".to_string(), Some("sta".to_string()))];
            if let Some(px) = o.get("pixel") {
                named.push(("pixel".into(), Some(px.into())));
            }
            let v = tomo(&Opts { input: Some(input.clone()), more: vec![], named })?;
            let path = v["open"].as_str().context("tmv tomo made no tomogram")?.to_string();
            let d = v["dataset"].as_str().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).or_else(|| dataset_of(&path));
            (path, d)
        } else {
            (input.clone(), dataset_of(input))
        };
        let (tilts, tilts_from) = match dataset.as_ref().and_then(tilt_range) {
            Some(t) => t,
            None => {
                eprintln!("warning: no tilt series dataset (*.tomo.json) found for {tomogram}: assuming tilts -60…60");
                ([-60.0, 60.0], "default -60…60")
            }
        };
        let look = look_of(dataset.as_ref(), &tomogram);
        found.push(StaInput { input: input.clone(), tomogram, tilts, tilts_from, look });
    }
    let sources: Vec<sp::Source> = found.iter().map(|f| sp::Source { path: f.tomogram.clone(), tilts: f.tilts, look: f.look.clone() }).collect();
    let tomograms: Vec<Value> = found
        .iter()
        .map(|f| json!({ "input": f.input, "tomogram": f.tomogram, "look": f.look.as_ref().map(|l| &l.path), "tiltsDeg": f.tilts, "tiltsFrom": f.tilts_from }))
        .collect();

    // Step 1: the cards (they take minutes, so a finished set is reused).
    let cards_dir = out.join("cards");
    let cards_json = cards_dir.join("cards.json");
    let inputs_json = cards_dir.join("inputs.json");
    // The look tomograms count too: the cards are made on them.
    let stamps = input_stamps(found.iter().map(|f| f.tomogram.as_str()).chain(found.iter().filter_map(|f| f.look.as_ref().map(|l| l.path.as_str()))));
    let mut cards: Vec<sp::Card> = std::fs::read(&cards_json).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if !cards.is_empty() {
        let saved: Option<Vec<InputStamp>> = std::fs::read(&inputs_json).ok().and_then(|b| serde_json::from_slice(&b).ok());
        if let Some(why) = stale(saved.as_deref(), &stamps) {
            eprintln!("tmv sta: making the cards again: {why}");
            cards.clear();
        }
    }
    if cards.is_empty() {
        cards = sp::cards(&sources, &cards_dir.to_string_lossy(), &report, &|_| {})?;
        std::fs::write(&inputs_json, serde_json::to_string_pretty(&stamps)?)?;
        progress.done();
    } else {
        eprintln!("tmv sta: reusing the cards in {} (delete that folder to make them again)", cards_dir.display());
        // The folder may have been copied elsewhere.
        for c in &mut cards {
            if !Path::new(&c.path).is_file() {
                c.path = cards_dir.join(format!("card_{}.mrc", c.id)).to_string_lossy().into_owned();
            }
        }
    }
    if cards.is_empty() {
        bail!("no card: no kind of blob with enough members was found in these tomograms (is it a tomogram made with --for sta, and does it hold particles?)");
    }

    // Choosing.
    let n = match pick {
        Some(n) => n,
        None => match auto_pick(&cards) {
            Some(n) => {
                eprintln!("tmv sta: card {n} is the only likely one, using it (--pick N chooses another)");
                n
            }
            None => {
                let tomos: Vec<&str> = found.iter().map(|f| f.tomogram.as_str()).collect();
                let rerun = format!("tmv sta --pick N{} {}", if o.get("out").is_some() { format!(" --out {}", out.display()) } else { String::new() }, tomos.join(" "));
                eprintln!("tmv sta: choose a card: look at {}/card_N.mrc (32³ class averages; open in TifMrcView Pro), then rerun with --pick N: {rerun}", cards_dir.display());
                return Ok(json!({
                    "needsChoice": true,
                    "cardsDir": cards_dir,
                    "cards": cards.iter().map(card_json).collect::<Vec<_>>(),
                    "tomograms": tomograms,
                    "next": rerun,
                }));
            }
        },
    };
    let card = cards.get(n).with_context(|| format!("--pick {n}: there are only {} cards (0 to {})", cards.len(), cards.len() - 1))?;

    // Step 2: the average.
    let (cube, voxel) = sp::read_card(&card.path)?;
    let (dev, backend) = device();
    let out_s = out.to_string_lossy().into_owned();
    let r = crate::ctf_gpu::guarded(|| sp::run_template(dev, &sources, &cube, voxel, DIAMETER_FACTOR * card.diameter_a, true, &out_s, &report, &|_| {}))??;
    progress.done();
    let mut v = serde_json::to_value(&r)?;
    v["card"] = card_json(card);
    v["out"] = json!(out);
    v["cardsDir"] = json!(cards_dir);
    v["tomograms"] = json!(tomograms);
    v["backend"] = json!(backend);
    v["open"] = json!(r.map);
    Ok(v)
}

/// The MemBrain-seg model: `--model`, else the one set up.
fn membrane_model(o: &Opts) -> Result<PathBuf> {
    if let Some(m) = o.get("model") {
        let p = std::fs::canonicalize(m).with_context(|| format!("--model {m}: not found"))?;
        std::env::set_var("TMV_MEMBRANE_MODEL", &p);
        return Ok(p);
    }
    crate::membrane::model_path().ok_or_else(|| anyhow!("no MemBrain-seg membrane model set up"))
}

/// `tmv membranes`: membranes, vesicles and microtubules of a tomogram.
fn membranes(o: &Opts) -> Result<Value> {
    let input = o.input()?;
    let model = membrane_model(o)?;
    let progress = Progress::new();
    let (dev, backend) = device();
    let r = crate::ctf_gpu::guarded(|| crate::membrane::segment_file(dev, &model, input, &|s, i, n| progress.report(s, i, n)))??;
    progress.done();
    let diam: Vec<f32> = r.vesicles.iter().map(|v| v.diameter).collect();
    let mut v = json!({
        "image": r.image,
        "labels": r.labels,
        "summary": r.summary,
        "vesiclesCsv": r.vesicles_csv,
        "pixelA": r.pixel_a,
        "shape": r.shape,
        "membraneVoxels": r.voxels,
        "membranePieces": r.components,
        "vesicles": r.vesicles.len(),
        "microtubules": r.microtubules.len(),
        "microtubuleLengthNm": r.microtubules,
        "backend": backend,
        "open": r.image,
    });
    if !diam.is_empty() {
        let mut s = diam.clone();
        s.sort_by(f32::total_cmp);
        v["vesicleDiameterNm"] = json!({ "median": s[s.len() / 2], "min": s[0], "max": s[s.len() - 1] });
    }
    Ok(v)
}

/// The SPA project of `input`, with the optics the user gave.
fn spa_project(o: &Opts) -> Result<crate::spa::project::Project> {
    let input = o.input()?;
    if o.flag("fresh") {
        // Start over: motion sums, CTF and particles made before go.
        let old = std::path::Path::new(input).join(crate::spa::project::DIR);
        if old.join("spa.json").is_file() {
            std::fs::remove_dir_all(&old).with_context(|| format!("cannot remove {}", old.display()))?;
        }
    }
    let mut p = crate::spa::project::Project::open(input)?;
    let mut changed = false;
    if let Some(dose) = o.number("dose")? {
        p.dose_total = Some(dose);
        changed = true;
    }
    if let Some(gain) = o.get("gain") {
        let g = std::fs::canonicalize(gain).with_context(|| format!("--gain: no file {gain}"))?;
        p.gain = Some(g.to_string_lossy().into_owned());
        changed = true;
    }
    if let Some(px) = o.number("pixel")? {
        p.optics.pixel_a = px;
        p.optics_found.pixel = true;
        changed = true;
    }
    if let Some(kv) = o.number("kv")? {
        p.optics.kv = kv;
        p.optics_found.kv = true;
        changed = true;
    }
    let no_pixel = !p.optics_found.pixel || p.optics.pixel_a <= 0.0;
    match (no_pixel, !p.optics_found.kv) {
        (true, true) => bail!("不知道像素大小和电压（文件和配置文件里都没有）"),
        (true, false) => bail!("不知道像素大小（文件和配置文件里都没有）"),
        (false, true) => bail!("不知道电压（文件和配置文件里都没有）"),
        _ => {}
    }
    if changed {
        p.save()?;
    }
    Ok(p)
}

/// Motion correction (for movies) and CTF, as the one-click mode does them.
fn spa_prepare(p: &mut crate::spa::project::Project, report: crate::spa::Progress) -> Result<()> {
    use crate::spa::project as sp;
    if p.movies && p.mics.iter().any(|m| m.movie.as_deref() == Some(m.path.as_str())) {
        crate::motion::begin();
        sp::motion(p, &Default::default(), report)?;
    }
    if p.mics.iter().any(|m| m.ctf.is_none() && !m.excluded) {
        let px = p.optics.pixel_a;
        let params = crate::ctf::Params {
            voltage_kv: p.optics.kv,
            cs_mm: p.optics.cs_mm,
            amp_contrast: p.optics.amplitude,
            pixel_a: px,
            res_low_a: 30.0,
            res_high_a: (px * 2.5).max(4.0),
            df_min_a: 3000.0,
            df_max_a: 50000.0,
            max_astig_a: 2000.0,
            phase: false,
            backend: None,
        };
        crate::ctf::CANCEL.store(false, Ordering::SeqCst);
        sp::ctf(p, &params, 512, report)?;
    }
    Ok(())
}

/// How the movies were summed, with what to pass when something was missing.
fn movies_info(p: &crate::spa::project::Project) -> Value {
    if !p.movies {
        return Value::Null;
    }
    let mut warnings = vec![];
    if p.gain.is_none() {
        warnings.push("no gain reference found beside the movies; if they are not gain-corrected already, rerun with --gain <file> --fresh");
    }
    if p.dose_total.is_none() {
        warnings.push("dose unknown, so the sums are not dose-weighted; rerun with --dose <e/Å² per movie> --fresh for better high resolution");
    }
    json!({ "gain": p.gain, "doseTotal": p.dose_total, "pixelA": p.optics.pixel_a, "warnings": warnings })
}

/// Writes `ctf.tsv` in the project folder; returns its path and the defocus range (µm).
fn ctf_table(p: &crate::spa::project::Project) -> Result<(String, Value)> {
    let mut t = String::from("micrograph\tdefocus1_A\tdefocus2_A\tastig_angle_deg\tfit_resolution_A\tscore\n");
    let mut dfs = vec![];
    let mut res = vec![];
    for m in &p.mics {
        if let Some(c) = &m.ctf {
            t += &format!("{}\t{:.0}\t{:.0}\t{:.1}\t{:.2}\t{:.3}\n", m.path, c.df1, c.df2, c.angle, c.fit_res, c.score);
            dfs.push(0.5 * (c.df1 + c.df2) / 1e4);
            res.push(c.fit_res);
        }
    }
    let path = p.path("ctf.tsv");
    std::fs::write(&path, t)?;
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied()
    };
    let stats = json!({
        "micrographs": dfs.len(),
        "defocusUm": { "min": dfs.iter().copied().reduce(f64::min), "median": median(&mut dfs.clone()), "max": dfs.iter().copied().reduce(f64::max) },
        "fitResolutionA": { "median": median(&mut res) },
    });
    Ok((path, stats))
}

/// `tmv ctf`: CTF of every micrograph of a folder.
fn ctf(o: &Opts) -> Result<Value> {
    let mut p = spa_project(o)?;
    let progress = Progress::new();
    spa_prepare(&mut p, &|s, i, n| progress.report(s, i, n))?;
    progress.done();
    let (table, stats) = ctf_table(&p)?;
    Ok(json!({ "project": p.dir, "table": table, "ctf": stats, "pixelA": p.optics.pixel_a, "kv": p.optics.kv, "movies": movies_info(&p) }))
}

/// SAM 2.1, when its model has been downloaded (`tmv setup --sam`).
fn sam() -> Option<crate::sam::Sam> {
    let path = crate::sam::model_path().filter(|p| p.is_file())?;
    match quiet(|| tmv_sam::load_best(&path, &tmv_sam::Sam2Config::tiny(), Duration::from_secs(90))) {
        Ok((pr, name)) => {
            eprintln!("SAM 2.1 on {name}");
            Some(crate::sam::Sam::with_predictor(pr))
        }
        Err(e) => {
            eprintln!("SAM 2.1 not used: {e:#}");
            None
        }
    }
}

/// `tmv spa`: the one-click single-particle mode. Without `--kind` it stops
/// after finding the kinds of particle when there is more than one, so the
/// user (or the agent, looking at the PNG strips) can pick theirs.
fn spa(o: &Opts) -> Result<Value> {
    use crate::spa::project as sp;
    let mut p = spa_project(o)?;
    let progress = Progress::new();
    let report = |s: &str, i: usize, n: usize| progress.report(s, i, n);
    spa_prepare(&mut p, &report)?;
    let sam = sam();
    let outline = |img: &[f32], w: usize, h: usize, pts: &[[f32; 2]]| sam.as_ref().unwrap().outline(img, w, h, pts);
    let outliner: Option<crate::spa::discover::Outliner> = sam.is_some().then_some(&outline);
    let kinds_arg: Option<Vec<usize>> = o
        .get("kind")
        .map(|s| s.split(',').map(|k| k.trim().parse::<usize>().ok().filter(|k| *k >= 1).map(|k| k - 1).with_context(|| format!("--kind {s}: use 1, 2, … (several: 1,3)"))).collect())
        .transpose()?;
    if p.discovery.is_none() || o.flag("fresh") {
        sp::discover(&mut p, outliner, &report)?;
    }
    let d = p.discovery.clone().context("no particles found")?;
    let kinds: Vec<Value> = d
        .kinds
        .iter()
        .enumerate()
        .map(|(i, k)| json!({ "kind": i + 1, "diameterA": k.diameter_a.round(), "share": (k.share * 100.0).round() / 100.0, "count": k.count, "elongation": (k.elongation * 100.0).round() / 100.0, "png": k.png }))
        .collect();
    let chosen = match kinds_arg {
        Some(k) => k,
        None if d.kinds.len() == 1 => vec![0],
        None => {
            progress.done();
            let (table, ctf) = ctf_table(&p)?;
            return Ok(json!({
                "needsChoice": true,
                "project": p.dir,
                "kinds": kinds,
                "ctf": ctf,
                "ctfTable": table,
                "movies": movies_info(&p),
                "next": format!("look at the png strips, then: tmv spa {} --kind N (several views of the same particle: --kind 1,3; never ice, carbon edges or clumps)", o.input()?),
            }));
        }
    };
    if chosen.iter().any(|&k| k >= d.kinds.len()) {
        bail!("--kind: only {} kinds were found", d.kinds.len());
    }
    let diameter = sp::choose(&mut p, chosen.clone())?;
    let symmetry = o.get("symmetry").unwrap_or("").trim().to_uppercase();
    let r = sp::auto(&mut p, &sp::AutoParams { symmetry }, outliner, &report)?;
    progress.done();
    let guessed = p.symmetry.as_ref().map(|s| serde_json::to_value(s).unwrap_or(Value::Null));
    Ok(json!({
        "project": p.dir,
        "kinds": kinds,
        "chosen": chosen.iter().map(|k| k + 1).collect::<Vec<_>>(),
        "diameterA": diameter.round(),
        "resolutionA": (r.res * 100.0).round() / 100.0,
        "resolutionMaskedA": (r.res_masked * 100.0).round() / 100.0,
        "bfactor": r.bfactor.round(),
        "symmetry": r.params.symmetry,
        "symmetryGuess": guessed,
        "particles": r.particles,
        "map": r.sharpened,
        "backend": r.backend,
        "samUsed": sam.is_some(),
        "movies": movies_info(&p),
        "open": r.sharpened,
    }))
}

/// `tmv denoise`: Noise2Void trained on the image itself, then every slice
/// denoised by it (as the app's 自监督 mode).
fn denoise(o: &Opts) -> Result<Value> {
    use crate::denoise_learn::{self as dl, Learned, Mode, MODEL};
    let input = o.input()?;
    let img = open_image(input)?;
    let info = img.info().clone();
    let (w, h, nz) = (info.width, info.height, info.size_z);
    let progress = Progress::new();
    // At most 16 slices and about 64 M pixels, spread over the stack.
    let most = (64_000_000 / (w * h).max(1)).clamp(1, 16);
    let k = nz.min(most);
    let pairs: Vec<(Vec<f32>, Option<Vec<f32>>)> = (0..k).map(|i| Ok((crate::denoise::input(&img, 0, i * nz / k, 0, 0)?, None))).collect::<Result<_>>()?;
    let corr = crate::denoise::noise_correlation(&pairs[0].0, w, h);
    let cfg = dl::TrainConfig::default();
    let ((net, mean, std, loss), backend) = crate::classify::on_device(|dev| {
        crate::ctf_gpu::guarded(|| dl::train(dev, &pairs, w, h, Mode::N2v, &cfg, |s, n, _| progress.report("训练降噪网络", s, n)))?
    })?;
    *MODEL.lock().unwrap() = Some(std::sync::Arc::new(Learned { net, mode: Mode::N2v, mean, std, loss, source: info.name.clone(), bin: 1 }));
    let p = Path::new(input);
    let stem = p.file_stem().unwrap_or_default().to_string_lossy();
    let stem = stem.strip_suffix(".ome").unwrap_or(&stem);
    let out = p.with_file_name(format!("{stem}_denoised.mrc")).to_string_lossy().into_owned();
    let params: crate::denoise::Params = serde_json::from_value(json!({ "method": "learned", "strength": 1.0 }))?;
    let req = crate::denoise::SaveReq { token: 1, zs: vec![], c: 0, t: 0, path: out.clone(), params };
    crate::denoise::save(&img, &req, &|i, n| progress.report("降噪", i, n))?;
    progress.done();
    let mut v = json!({ "output": out, "slices": nz, "trainedOn": k, "loss": loss, "backend": backend, "open": out });
    if corr.abs() > 0.15 {
        v["warning"] = json!(format!("noise is correlated between neighbouring pixels ({corr:+.2}): the data was probably filtered or interpolated, so self-supervised denoising helps less"));
    }
    Ok(v)
}

/// `tmv info`: what this server offers.
fn info() -> Result<Value> {
    let (gpu, card) = match gpu() {
        Some((dev, name)) => (*name, crate::ctf_gpu::guarded(|| dev.identity().map(|i| i.name)).ok().flatten()),
        None => ("CPU", None),
    };
    Ok(json!({
        "gpu": gpu,
        "card": card,
        "cudaPath": std::env::var("CUDA_PATH").ok(),
        "membraneModel": crate::membrane::model_path(),
        "samModel": crate::sam::model_path().filter(|p| p.is_file()),
        "threads": rayon::current_num_threads(),
        "hint": if gpu == "CPU" { Some("no GPU: install the NVIDIA driver (CUDA) or a Vulkan driver; the CPU works but is very slow") } else { None },
    }))
}

/// `tmv setup`: remembers the membrane model, downloads SAM 2.1.
fn setup(o: &Opts) -> Result<Value> {
    let mut v = json!({});
    if let Some(m) = o.get("membrane").or(o.input.as_deref()) {
        let path = std::fs::canonicalize(m).with_context(|| format!("{m}: not found"))?;
        let (dev, _) = device();
        crate::ctf_gpu::guarded(|| crate::membrane::choose(&path, dev))??;
        v["membraneModel"] = json!(path);
    }
    if o.flag("sam") {
        v["samModel"] = json!(download_sam()?);
    }
    if v.as_object().is_some_and(|o| o.is_empty()) {
        bail!("nothing to set up: tmv setup --membrane <MemBrain-seg .ckpt> and/or --sam");
    }
    Ok(v)
}

/// Downloads SAM 2.1 tiny (156 MB) with curl or wget, checking its SHA-256.
fn download_sam() -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let path = crate::sam::model_path().context("no data folder")?;
    let check = |p: &Path| -> Result<bool> {
        let mut f = std::fs::File::open(p)?;
        let mut h = Sha256::new();
        std::io::copy(&mut f, &mut h)?;
        Ok(format!("{:x}", h.finalize()) == crate::sam::MODEL_SHA256)
    };
    if path.is_file() && check(&path)? {
        return Ok(path);
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    let part = path.with_extension("pt.part");
    for url in crate::sam::MODEL_URLS {
        eprintln!("downloading {url}");
        let ok = std::process::Command::new("curl").args(["-fL", "--retry", "3", "-o"]).arg(&part).arg(url).status().map(|s| s.success()).unwrap_or(false)
            || std::process::Command::new("wget").args(["-q", "-O"]).arg(&part).arg(url).status().map(|s| s.success()).unwrap_or(false);
        if ok && check(&part).unwrap_or(false) {
            std::fs::rename(&part, &path)?;
            return Ok(path);
        }
    }
    let _ = std::fs::remove_file(&part);
    bail!("could not download SAM 2.1 (needs curl or wget and access to huggingface.co or hf-mirror.com); without it the SPA steps still run")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_parse() {
        let dir = std::env::temp_dir();
        let o = Opts::parse(&[dir.to_string_lossy().into(), "--for".into(), "sta".into(), "--pixel=1.5".into(), "--fresh".into()]).unwrap();
        assert_eq!(o.get("for"), Some("sta"));
        assert_eq!(o.number("pixel").unwrap(), Some(1.5));
        assert!(o.flag("fresh"));
        assert!(Path::new(o.input().unwrap()).is_absolute());
        assert!(Opts::parse(&["--kind".into()]).is_err());
        assert!(Opts::parse(&["--pixel".into(), "x".into()]).unwrap().number("pixel").is_err());
    }

    #[test]
    fn sta_takes_several_inputs_and_only_sta() {
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let o = Opts::parse(&[dir.clone(), dir.clone(), "--pick".into(), "2".into()]).unwrap();
        assert_eq!(o.more.len(), 1);
        assert_eq!(o.get("pick"), Some("2"));
        assert_eq!(main(vec!["tomo".into(), dir.clone(), dir]), 1);
        assert_eq!(main(vec!["sta".into()]), 1);
    }

    fn card(id: usize, likely: bool) -> crate::sta::project::Card {
        crate::sta::project::Card { id, diameter_a: 210.0, measured_a: 200.0, voxel_a: 16.0, members: 50, elongation: 1.1, blobs: 1, share: 0.3, path: String::new(), compact: 12.0, likely }
    }

    #[test]
    fn the_card_is_picked_only_when_clear() {
        assert_eq!(auto_pick(&[card(0, true), card(1, false), card(2, false)]), Some(0));
        assert_eq!(auto_pick(&[card(0, true)]), Some(0));
        assert_eq!(auto_pick(&[card(0, true), card(1, true)]), None);
        assert_eq!(auto_pick(&[card(0, false), card(1, false)]), None);
        assert_eq!(auto_pick(&[]), None);
    }

    #[test]
    fn stale_cards_are_found() {
        let stamp = |p: &str, size, m| InputStamp { path: p.into(), size, modified_ns: m };
        let now = vec![stamp("/a.mrc", 10, 5), stamp("/b.mrc", 20, 6)];
        assert_eq!(stale(Some(&now), &now), None);
        assert!(stale(None, &now).is_some());
        assert!(stale(Some(&now[..1]), &now).is_some());
        assert!(stale(Some(&[stamp("/c.mrc", 10, 5), now[1].clone()]), &now).is_some());
        assert!(stale(Some(&[stamp("/a.mrc", 11, 5), now[1].clone()]), &now).unwrap().contains("/a.mrc"));
        assert!(stale(Some(&[now[0].clone(), stamp("/b.mrc", 20, 7)]), &now).unwrap().contains("/b.mrc"));
        let swapped = vec![now[1].clone(), now[0].clone()];
        assert!(stale(Some(&swapped), &now).is_some());
    }

    #[test]
    fn input_stamps_follow_the_file() {
        let dir = std::env::temp_dir().join(format!("tmv_stamp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.mrc");
        std::fs::write(&f, b"abc").unwrap();
        let p = f.to_string_lossy().into_owned();
        let a = input_stamps([p.as_str()].into_iter());
        assert_eq!((a[0].size, a[0].path.as_str()), (3, p.as_str()));
        assert_eq!(stale(Some(&a), &input_stamps([p.as_str()].into_iter())), None);
        std::fs::write(&f, b"abcd").unwrap();
        assert!(stale(Some(&a), &input_stamps([p.as_str()].into_iter())).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sta_saves_its_result_in_the_out_folder() {
        let o = Opts { input: Some("/data/t.mrc".into()), named: vec![("out".into(), Some("/tmp/x".into()))], ..Default::default() };
        assert_eq!(result_file("sta", &o), Some(PathBuf::from("/tmp/x/sta.tmv.json")));
        assert_eq!(result_file("tomo", &o), Some(PathBuf::from("/data/t.mrc.tmv.json")));
        let o = Opts { input: Some("/data/t.mrc".into()), ..Default::default() };
        assert_eq!(result_file("sta", &o), Some(PathBuf::from("/data/t_sta/sta.tmv.json")));
        assert_eq!(result_file("sta", &Opts::default()), None);
        let named = |v: &str| Opts { input: Some("/data/t.mrc".into()), named: vec![("for".into(), Some(v.into()))], ..Default::default() };
        assert_eq!(result_file("tomo", &named("sta")), Some(PathBuf::from("/data/t.mrc.sta.tmv.json")));
        assert_eq!(result_file("tomo", &named("survey")), Some(PathBuf::from("/data/t.mrc.survey.tmv.json")));
        assert_eq!(result_file("tomo", &named("display")), Some(PathBuf::from("/data/t.mrc.tmv.json")));
        assert_eq!(result_file("tomo", &o), Some(PathBuf::from("/data/t.mrc.tmv.json")));
    }

    #[test]
    fn tilt_range_and_default_folder() {
        assert_eq!(range_of(&[-3.0, 57.0, 0.0, -57.0, 3.0]), Some([-57.0, 57.0]));
        assert_eq!(range_of(&[10.0]), None);
        assert_eq!(range_of(&[]), None);
        assert_eq!(range_of(&[f64::NAN, -40.0, 60.0]), Some([-40.0, 60.0]));
        let o = Opts::default();
        assert_eq!(sta_out(&o, "/data/a/X_sart3_b2_flip.mrc").unwrap(), Path::new("/data/a/X_sart3_b2_flip_sta"));
        let o = Opts { named: vec![("out".into(), Some("/tmp/x".into()))], ..Default::default() };
        assert_eq!(sta_out(&o, "/data/a/t.mrc").unwrap(), Path::new("/tmp/x"));
    }

    #[test]
    fn errors_come_as_json_with_hints() {
        assert!(hint("不知道像素大小").is_some());
        assert!(hint("no MemBrain-seg membrane model set up").is_some());
        assert_eq!(main(vec!["nonsense".into()]), 1);
        assert_eq!(main(vec!["--version".into()]), 0);
    }
}
