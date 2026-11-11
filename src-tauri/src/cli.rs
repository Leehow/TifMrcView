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

use crate::sta::auto::{self as auto_sta, card_json, trial_json};
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
        [--pick N | --points marks.json] [--out DIR] [--pixel Å]
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
  does) or tomograms that tmv tomo --for sta wrote. It runs in three steps:
  class-average "cards" of the blobs found go to <out>/cards/ (--out defaults
  to <first input>_sta beside it); then every card (at most 12) is tried as the
  template of a coarse run (<out>/trials/card_N/, kept in cards/trials.json)
  and judged by what it gives: a card that is a layer, a line or a small blob
  finds no group of particles that stands out, a particle gives an average
  whose two halves agree far out (the "agreement": the area under their FSC,
  1/Å). With --pick N (which skips the trials), or by itself when the cards
  within 0.75 of the best agreement show one kind of particle (it takes the
  best of them, naming the others), the average of everything that looks like
  card N goes to <out>/ (average.mrc: weighted by the half agreement, sharpened and cut at the
  resolution; average_raw.mrc: as averaged; half1.mrc, half2.mrc, particles.csv,
  sta.json).
  When no card gave an average it stops with exit status 2 and the trials in
  the JSON: look at the cards, then rerun with --pick N.
  Progress counts the steps of the whole run ("第 k/N 步"). The JSON has
  trials (per card: ok, reason, particles, agreement, fsc), resolutionA, particles, fsc, diameterA (1.33 × the card's size, as
  diameterFrom "card" says), searched ("look" or "raw": the tomogram the template
  was searched on) with searchVoxelA, ctfLimited (true when the FSC has a dip
  that rises again, a local minimum below 0.5 followed by a value 0.15 higher:
  a zero of the contrast transfer; resolutionA is then the dip and signalA
  is where the halves last agree, finer), sharpenB (the B factor in Å², negative,
  applied to average.mrc; left out when no fit was possible) and seconds (the whole run).

OUTPUT
  Files are written beside the input; copy the folder back and open them in
  TifMrcView Pro (label maps open with their tomogram). Progress goes to
  stderr. The result is one JSON object on stdout ({"ok":true,…}), also
  saved as <input>.tmv.json (tmv tomo --for sta|survey: <input>.sta.tmv.json,
  <input>.survey.tmv.json; tmv sta: <out>/sta.tmv.json). Exit status 0 on success, 1 on error
  ({"ok":false,"error":…}), 2 when a choice is needed (spa: pick a kind;
  sta: pick a card).

  The GPU is found automatically (CUDA with an NVIDIA driver, else Vulkan,
  else CPU). On a shared server pick the card with CUDA_VISIBLE_DEVICES=N;
  TMV_GPU=cuda|vulkan|cpu forces a backend. Long runs:
  nohup tmv … > run.log 2>&1 &
"#;

/// Exit status when the user must choose before the command can go on.
const NEEDS_CHOICE: i32 = 2;

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

/// Whether a progress report is printed, given the last one printed
/// (`(stage, tenth of the stage done)`, `since` ago). On a terminal: every
/// 200 ms and when the stage changes. In a log: when the step changes ("第 2/8
/// 步 · …" up to the dot), at the end of a stage, when a stage's text changes
/// (the next round) or another tenth of it is done, at most every 3 s, and at
/// least every 20 s. (Counting only 20 s apart showed a round's first tick over
/// and over when the work before it took longer than the rest of the round.)
fn due(tty: bool, last: (&str, usize), since: Duration, stage: &str, done: usize, total: usize) -> bool {
    if tty {
        return since > Duration::from_millis(200) || last.0 != stage;
    }
    let tenth = if total > 0 { done * 10 / total } else { 0 };
    let step = stage.split(" · ").next().unwrap_or(stage);
    let last_step = last.0.split(" · ").next().unwrap_or(last.0);
    last_step != step
        || (done == total && total > 1 && last.1 != 10)
        || (since >= Duration::from_secs(3) && (last.0 != stage || tenth != last.1))
        || since > Duration::from_secs(20)
}

/// Progress on stderr: on a terminal one updating line, otherwise (logs)
/// lines as `due` says.
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
        if !due(self.tty, (&last.0, last.1), last.2.elapsed(), stage, done, total) {
            return;
        }
        if self.tty {
            eprint!("\r\x1b[2K[{t:7.1} s] {line}");
        } else {
            eprintln!("[{t:7.1} s] {line}");
        }
        *last = (stage.to_string(), tenth, Instant::now());
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

/// `--out`, else `<stem>_sta` beside the first input.
fn sta_out(o: &Opts, first: &str) -> Result<PathBuf> {
    if let Some(d) = o.get("out") {
        return std::path::absolute(d).with_context(|| format!("--out {d}"));
    }
    Ok(auto_sta::default_out(first))
}

/// The closing line of `tmv sta` on stderr.
fn sta_summary(r: &crate::sta::project::Report) -> String {
    let map = r.map.as_deref().unwrap_or("average.mrc");
    match (r.resolution_a, r.signal_a) {
        (Some(a), Some(signal)) if r.ctf_limited => format!(
            "tmv sta: done: {a:.1} Å resolution (limited at a zero of the contrast transfer; the two halves agree again behind it, out to about {signal:.0} Å) from {} particles; the average is {map}",
            r.particles
        ),
        (Some(a), _) => format!("tmv sta: done: {a:.1} Å resolution (FSC 0.143 between the two halves) from {} particles; the average is {map}", r.particles),
        (None, _) => format!("tmv sta: done: {} particles averaged, the halves did not agree well enough to give a resolution; the average is {map}", r.particles),
    }
}

/// "0 and 1", "0, 1 and 3".
fn id_list(ids: &[usize]) -> String {
    match ids {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")),
    }
}

/// The stderr line of one trial.
fn trial_line(t: &crate::sta::project::Trial) -> String {
    if t.ok {
        let runs = if t.runs_on() { "; it runs on beyond the box (a line, a layer or an edge)" } else { "" };
        let long = t.elongation.map_or(String::new(), |e| format!(", elongation {e:.2}"));
        format!("tmv sta: card {} as a template: {} particles, agreement {:.4}{long}{runs}", t.card, t.particles, t.strength)
    } else if t.reason.as_deref().is_some_and(|r| r.contains("分不出两群")) {
        format!("tmv sta: card {} as a template: no group of particles stands out", t.card)
    } else {
        format!("tmv sta: card {} as a template: failed ({})", t.card, t.reason.as_deref().unwrap_or("unknown"))
    }
}

/// The English line of a note of `sta::auto`, printed on stderr.
fn sta_note(n: &auto_sta::Note, progress: &Progress) {
    use auto_sta::Note;
    match n {
        Note::NoDataset { tomogram } => eprintln!("warning: no tilt series dataset (*.tomo.json) found for {tomogram}: assuming tilts -60…60"),
        Note::LookMissing { look, tomogram } => eprintln!("tmv sta: the look tomogram {look} of {tomogram} is missing: using the tomogram alone"),
        Note::OlderVersion { tomogram } => eprintln!("tmv sta: {tomogram} was made by an older version: the average will come from the tomogram only (run tmv tomo --for sta again to use the tilt images)"),
        Note::CardsStale { why } => eprintln!("tmv sta: making the cards again: {why}"),
        Note::CardsReused { dir } => eprintln!("tmv sta: reusing the cards in {dir} (delete that folder to make them again)"),
        Note::CardsMade { .. } => progress.done(),
        Note::TrialsReused { dir } => eprintln!("tmv sta: reusing the trials in {dir} (delete trials.json to try the cards again)"),
        Note::Trial { trial, reused } => {
            if !reused {
                progress.done();
            }
            eprintln!("{}", trial_line(trial));
        }
        Note::TrialsMade => progress.done(),
        Note::CompareFailed { error } => eprintln!("tmv sta: could not compare the cards ({error}): each is taken for a kind of its own"),
        Note::Marked { points, size_a } => eprintln!("tmv sta: made the template from the {points} particles you marked (about {size_a:.0} Å across)"),
        Note::Using { card, same, other, dir } => {
            let why = if same.len() > 1 { format!("cards {} show the same particle", id_list(same)) } else { "the card that gives the best average".to_string() };
            eprintln!(
                "tmv sta: using card {} (about {} Å across, {} members; {why}); the other cards are in {dir}/, choose another with --pick N",
                card.id, card.diameter_a, card.members
            );
            if !other.is_empty() {
                eprintln!("tmv sta: something else averages well too (card {}): choose it with --pick N", other.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", "));
            }
        }
    }
}

/// `tmv sta`: the tomogram of each input (made as `tmv tomo --for sta` does),
/// class-average cards of the blobs in them, then the average of everything
/// that looks like the chosen card (`sta::auto::auto`).
fn sta(o: &Opts) -> Result<Value> {
    use crate::sta::project as sp;
    let inputs: Vec<String> = o.input.iter().chain(&o.more).cloned().collect();
    if inputs.is_empty() {
        bail!("give at least one tilt series or tomogram: tmv sta [--pick N | --points marks.json] [--out DIR] [--pixel Å] INPUT [MORE …]");
    }
    let pick = o.get("pick").map(|s| s.parse::<usize>().with_context(|| format!("--pick: `{s}` is not a card number (0, 1, 2, …)"))).transpose()?;
    let out = sta_out(o, &inputs[0])?;
    let progress = Progress::new();
    let note = |n: auto_sta::Note| sta_note(&n, &progress);

    // Step 0: a tomogram and its tilt range for each input.
    let mut found: Vec<auto_sta::Resolved> = vec![];
    for input in &inputs {
        let (tomogram, dataset) = if auto_sta::dataset_of(input).is_none() && (ext(input) == "mdoc" || has_tilts(input, open_image(input)?.info().size_z)) {
            eprintln!("tmv sta: {input} is a tilt series → tmv tomo --for sta");
            let mut named = vec![("for".to_string(), Some("sta".to_string()))];
            if let Some(px) = o.get("pixel") {
                named.push(("pixel".into(), Some(px.into())));
            }
            let v = tomo(&Opts { input: Some(input.clone()), more: vec![], named })?;
            let path = v["open"].as_str().context("tmv tomo made no tomogram")?.to_string();
            let d = v["dataset"].as_str().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).or_else(|| auto_sta::dataset_of(&path));
            (path, d)
        } else {
            (input.clone(), auto_sta::dataset_of(input))
        };
        found.push(auto_sta::resolve(input, tomogram, dataset, &note));
    }
    let sources: Vec<sp::Source> = found.iter().map(auto_sta::Resolved::source).collect();
    let tomograms: Vec<Value> = found
        .iter()
        .map(|f| json!({ "input": f.input, "tomogram": f.tomogram, "look": f.look.as_ref().map(|l| &l.path), "tiltsDeg": f.tilts, "tiltsFrom": f.tilts_from }))
        .collect();

    if let Some(file) = o.get("points") {
        // The marks the workspace keeps (`marks.json`): its point marks make one template, then the usual template run.
        use crate::sta::marks::{points_of, Mark, Saved};
        let bytes = std::fs::read(file).with_context(|| format!("--points {file}: cannot read"))?;
        let marks: Vec<Mark> = serde_json::from_slice::<Saved>(&bytes).map(|s| s.marks).or_else(|_| serde_json::from_slice::<Vec<Mark>>(&bytes)).with_context(|| format!("--points {file}: not a marks.json"))?;
        let points = points_of(&marks, &inputs, &sources)?;
        let dir = out.join(auto_sta::MARKED);
        let outcome = auto_sta::auto_points(&sources, &points, &out, &|| Ok(device()), &|s, i, n| progress.report(s, i, n), &|_| {}, &note)?;
        progress.done();
        let auto_sta::Outcome::Done(d) = outcome else { bail!("no average") };
        eprintln!("{}", sta_summary(&d.report));
        let mut v = serde_json::to_value(&d.report)?;
        v["card"] = card_json(&d.card);
        v["points"] = json!(points.len());
        v["out"] = json!(dir);
        v["tomograms"] = json!(tomograms);
        v["backend"] = json!(d.backend);
        v["open"] = json!(d.report.map);
        return Ok(v);
    }
    let outcome = auto_sta::auto(&sources, pick, &out, &|| Ok(device()), &|s, i, n| progress.report(s, i, n), &|_| {}, &note)?;
    let cards_dir = out.join("cards");
    let d = match outcome {
        auto_sta::Outcome::Done(d) => d,
        auto_sta::Outcome::NeedsChoice { cards, trials, good, cards_dir } => {
            let tomos: Vec<&str> = found.iter().map(|f| f.tomogram.as_str()).collect();
            let rerun = format!("tmv sta --pick N{} {}", if o.get("out").is_some() { format!(" --out {}", out.display()) } else { String::new() }, tomos.join(" "));
            if good.is_empty() {
                eprintln!("tmv sta: no card gave an average as a template: look at {}/card_N.mrc (32³ class averages; open in TifMrcView Pro), then rerun with --pick N: {rerun}", cards_dir.display());
            } else {
                let list: Vec<String> = trials.iter().filter(|t| good.contains(&t.card)).map(|t| format!("card {} (agreement {:.4})", t.card, t.strength)).collect();
                eprintln!(
                    "tmv sta: several kinds of particle give a good average: {}; look at {}/card_N.mrc (32³ class averages; open in TifMrcView Pro), then rerun with --pick N: {rerun}",
                    list.join(", "),
                    cards_dir.display()
                );
            }
            return Ok(json!({
                "needsChoice": true,
                "cardsDir": cards_dir,
                "cards": cards.iter().map(card_json).collect::<Vec<_>>(),
                "trials": trials.iter().map(trial_json).collect::<Vec<_>>(),
                "tomograms": tomograms,
                "next": rerun,
            }));
        }
    };
    progress.done();
    let r = d.report;
    eprintln!("{}", sta_summary(&r));
    let mut v = serde_json::to_value(&r)?;
    if let Value::Object(m) = &mut v {
        // `seconds` (the whole run) says it; `ms` covered only the average.
        m.remove("ms");
    }
    v["diameterFrom"] = json!("card");
    v["card"] = card_json(&d.card);
    v["out"] = json!(out);
    v["cardsDir"] = json!(cards_dir);
    v["trials"] = json!(d.trials.iter().map(trial_json).collect::<Vec<_>>());
    v["tomograms"] = json!(tomograms);
    v["backend"] = json!(d.backend);
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
    use std::time::Duration;

    /// The lines a log gets from ticks `(second, stage, done, total)`.
    fn logged(ticks: &[(f64, &str, usize, usize)]) -> Vec<String> {
        let (mut last, mut at, mut out) = ((String::new(), usize::MAX), 0.0, vec![]);
        for &(t, stage, done, total) in ticks {
            if super::due(false, (&last.0, last.1), Duration::from_secs_f64(t - at), stage, done, total) {
                out.push(format!("{stage} {done}/{total}"));
                last = (stage.to_string(), if total > 0 { done * 10 / total } else { 0 });
                at = t;
            }
        }
        out
    }

    #[test]
    fn a_log_shows_the_counter_move_and_every_round() {
        // The real shape: 10 rounds, each a quiet 25 s of preparation, then
        // 1656 ticks in 10 s. Printing every 20 s alone caught the first
        // tick of each round (1/1656) again and again.
        let mut ticks = vec![];
        for round in 1..=10 {
            let stage = format!("第 2/5 步 · 分类 第 {round}/10 轮 210 Å");
            let start = 25.0 + 35.0 * (round - 1) as f64;
            for d in 1..=1656 {
                ticks.push((start + 10.0 * d as f64 / 1656.0, stage.clone(), d, 1656));
            }
        }
        let ticks: Vec<_> = ticks.iter().map(|(t, s, d, n)| (*t, s.as_str(), *d, *n)).collect();
        let lines = logged(&ticks);
        for round in 1..=10 {
            let mine: Vec<&String> = lines.iter().filter(|l| l.contains(&format!("第 {round}/10 轮"))).collect();
            assert!(mine.len() >= 3 && mine.iter().any(|l| l.ends_with("1656/1656")), "round {round}: {mine:?}");
            let dones: std::collections::BTreeSet<&str> = mine.iter().map(|l| l.rsplit(' ').next().unwrap()).collect();
            assert!(dones.len() >= 3, "{mine:?}");
        }
        assert!(lines.len() < 100, "{}", lines.len());
    }

    #[test]
    fn the_closing_line_of_sta_says_what_the_dip_means() {
        let mut r = crate::sta::project::Report {
            kinds: vec![],
            diameter_a: None,
            particles: 533,
            candidates: 600,
            distinct: None,
            voxel_a: 13.0,
            searched: Some("look".into()),
            search_voxel_a: Some(34.7),
            resolution_a: Some(27.84),
            signal_a: None,
            sharpen_b: None,
            ctf_limited: false,
            fsc: vec![],
            averaged_from: None,
            fsc_other: None,
            views_refined: None,
            map: Some("o/average.mrc".into()),
            halves: None,
            table: None,
            ms: 0,
        };
        assert_eq!(super::sta_summary(&r), "tmv sta: done: 27.8 Å resolution (FSC 0.143 between the two halves) from 533 particles; the average is o/average.mrc");
        (r.signal_a, r.ctf_limited) = (Some(19.8), true);
        assert_eq!(
            super::sta_summary(&r),
            "tmv sta: done: 27.8 Å resolution (limited at a zero of the contrast transfer; the two halves agree again behind it, out to about 20 Å) from 533 particles; the average is o/average.mrc"
        );
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!((json["ctfLimited"].as_bool(), json["signalA"].as_f64(), json["resolutionA"].as_f64()), (Some(true), Some(19.8), Some(27.84)));
    }

    #[test]
    fn sta_steps_are_counted_over_the_whole_run() {
        let plan = |cards, trials| Plan { cards, trials };
        // With cards and trials: find, cut and classify (both under step 2), the trials, search, coarse and fine alignment.
        let labels = ["找颗粒", "裁出颗粒 210 Å", "分类 第 3/10 轮 210 Å", "试跑卡片 2/9", "搜索 1/2", "粗对齐 第 1/3 轮", "裁出更细的颗粒", "精细对齐 第 2/3 轮"];
        let all: Vec<String> = labels.iter().map(|l| sta_label(l, plan(true, true))).collect();
        assert_eq!(
            all,
            [
                "第 1/6 步 · 找颗粒",
                "第 2/6 步 · 裁出颗粒 210 Å",
                "第 2/6 步 · 分类 第 3/10 轮 210 Å",
                "第 3/6 步 · 试跑卡片 2/9",
                "第 4/6 步 · 搜索 1/2",
                "第 5/6 步 · 粗对齐 第 1/3 轮",
                "第 6/6 步 · 裁出更细的颗粒",
                "第 6/6 步 · 精细对齐 第 2/3 轮"
            ]
        );
        // Cards made, one picked: no trials step.
        assert_eq!(sta_label("搜索 1/2", plan(true, false)), "第 3/5 步 · 搜索 1/2");
        // Cards reused, trials run.
        assert_eq!(sta_label("试跑卡片 1/9", plan(false, true)), "第 1/4 步 · 试跑卡片 1/9");
        assert_eq!(sta_label("搜索 1/2", plan(false, true)), "第 2/4 步 · 搜索 1/2");
        // Everything reused: only the last three.
        assert_eq!(sta_label("搜索 1/1", plan(false, false)), "第 1/3 步 · 搜索 1/1");
        assert_eq!(sta_label("粗对齐 第 3/3 轮", plan(false, false)), "第 2/3 步 · 粗对齐 第 3/3 轮");
        assert_eq!(sta_label("精细对齐 第 1/3 轮", plan(false, false)), "第 3/3 步 · 精细对齐 第 1/3 轮");
        // Not a step of this command: unchanged.
        assert_eq!(sta_label("起始模型 第 1/6 轮", plan(true, true)), "起始模型 第 1/6 轮");
    }

    use super::*;
    use crate::sta::auto::{auto_pick, input_stamps, range_of, sta_label, stale, InputStamp, Plan};

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

    fn card(id: usize) -> crate::sta::project::Card {
        crate::sta::project::Card { id, diameter_a: 210.0, measured_a: 200.0, voxel_a: 16.0, members: 50, elongation: 1.1, blobs: 1, offset: 0.0, share: 0.3, path: String::new(), compact: 12.0, likely: false }
    }

    fn trial(card: usize, strength: Option<f64>) -> crate::sta::project::Trial {
        crate::sta::project::Trial {
            card,
            ok: strength.is_some(),
            reason: strength.is_none().then(|| "没有找到明显像模板的颗粒（40 个候选的分数分不出两群）".to_string()),
            particles: 100,
            candidates: 200,
            fsc: vec![],
            voxel_a: 13.0,
            strength: strength.unwrap_or(0.0),
            seconds: 1.0,
            elongation: None,
        }
    }

    #[test]
    fn the_card_is_picked_only_when_clear() {
        let id = |c: Option<crate::sta::project::Chosen>| c.map(|c| c.id);
        let cards = [card(0), card(1), card(2)];
        // One card works: it is taken, however the others look.
        assert_eq!(id(auto_pick(&cards, &[trial(0, None), trial(1, Some(0.0274)), trial(2, None)], None, &|_| {})), Some(1));
        // One works well, the other is too weak to count (0.018 / 0.0274 < 0.75).
        assert_eq!(id(auto_pick(&cards, &[trial(0, Some(0.018)), trial(1, Some(0.0274)), trial(2, None)], None, &|_| {})), Some(1));
        // Two close ones are compared from their files: unreadable here, so the stronger is taken.
        assert_eq!(id(auto_pick(&cards, &[trial(0, Some(0.0211)), trial(1, Some(0.0274)), trial(2, None)], None, &|_| {})), Some(1));
        // None works; nothing tried.
        assert_eq!(auto_pick(&cards, &[trial(0, None), trial(1, None)], None, &|_| {}), None);
        assert_eq!(auto_pick(&cards, &[], None, &|_| {}), None);
        assert_eq!(auto_pick(&[], &[], None, &|_| {}), None);
    }

    #[test]
    fn trials_are_told_in_a_line() {
        assert_eq!(trial_line(&trial(3, Some(0.02736))), "tmv sta: card 3 as a template: 100 particles, agreement 0.0274");
        assert_eq!(trial_line(&trial(2, None)), "tmv sta: card 2 as a template: no group of particles stands out");
        let mut other = trial(2, None);
        other.reason = Some("只找到 3 个颗粒，太少了".into());
        assert_eq!(trial_line(&other), "tmv sta: card 2 as a template: failed (只找到 3 个颗粒，太少了)");
    }

    #[test]
    fn card_numbers_are_listed() {
        assert_eq!(id_list(&[0]), "0");
        assert_eq!(id_list(&[0, 1]), "0 and 1");
        assert_eq!(id_list(&[0, 2, 3]), "0, 2 and 3");
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
