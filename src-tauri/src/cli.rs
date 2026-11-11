//! `tmv`: TifMrcView Pro's processing without a window, for Linux servers
//! that have a GPU but no display. Coding agents (Claude Code, Codex) copy
//! data there over SSH, run one command per purpose, and copy the results
//! back to open them in the app.
//!
//! The commands are the app's one-click flows with the same code and the
//! same automatic choices: no method switches or tuning knobs. Results are
//! written in the input's project folder (as the app writes them: step
//! folders 1_motion … 4_sta, flat beside the input for an old project), so copying the folder
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
                                 (--pixel is needed when the file does not say its pixel size)
  tmv sta <tilt-series|tomogram> [MORE …]
                                 Subtomogram averaging: makes the cards, then STOPS
        [--pick N | --points marks.json | --marks marks.json [--size small|medium|large]
         | --template map.mrc | --auto] [--out DIR] [--pixel Å]
  tmv sta <tomogram> [MORE …] --kinds [--keep 1,3] [--out DIR]
                                 Sort the particles of a finished average into kinds
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
  does) or tomograms that tmv tomo --for sta wrote. What to average is the
  user's choice, never the program's, so without --pick, --points or --auto it
  only makes the class-average "cards" of the blobs found (<out>/cards/card_N.mrc,
  32³; --out defaults to 4_sta/ in the project folder of the first input,
  or <first input>_sta beside it for a tomogram outside a project) and stops with exit status
  2: {"needsChoice": true, "cards": […], "next": …}, one line per card on stderr.
  Show the cards (and the clear "look" tomogram) to the user, let them choose,
  then rerun with --pick N (average the card they chose) or --points marks.json
  (average what they marked: the point marks of the workspace's marks file), or
  --marks marks.json (every kind of mark: points, lines, surfaces, spheres given
  as centre + a point on the surface, painted membranes; shapes are averaged on
  seeds along them with their directions, into <out>/drawn/; --size small|medium|
  large is the particle size of lines, surfaces and membranes; a sphere takes it
  from its radius; clicks into pits of a membrane, kind "pit", are examples:
  every pit of every membrane in the inputs is found and all are averaged, the
  membranes segmented first where they are not yet), or --template map.mrc|.em (a density map the user already
  has is the template: everything like it is found and averaged without
  stopping, into <out>/own/). --kinds (on a folder that already holds an
  average; give the same tomograms and --out) sorts its particles into kinds
  (<out>/…/kinds/kind_N.mrc, kinds.json; JSON "kinds": [{id, count, share, map}],
  empty when the particles are all of one kind) and prints them; --kinds --keep 1,3
  then averages only those kinds at their poses into …/kinds/kept/ and prints the usual summary. Everything after
  the choice is automatic. The hints per card (likely a particle, line/layer-like)
  are only hints. --auto is the unattended batch mode, only for when the user
  asked for it: every card (at most 12) is tried as the template of a coarse run
  (<out>/trials/card_N/, kept in cards/trials.json) and judged by what it gives
  (a card that is a layer, a line or a small blob finds no group of particles
  that stands out, a particle gives an average whose two halves agree far out:
  the "agreement", the area under their FSC, 1/Å); when the cards within 0.75
  of the best agreement show one kind of particle it takes the best of them,
  naming the others. The average of everything that looks like card N goes to
  <out>/ (average.mrc: weighted by the half agreement, sharpened and cut at the
  resolution; average_raw.mrc: as averaged; half1.mrc, half2.mrc, particles.csv,
  sta.json). With --auto, when no card gave an average it also stops with exit
  status 2 and the trials in the JSON: look at the cards, then rerun with --pick N.
  Progress counts the steps of the whole run ("第 k/N 步"). The JSON has
  trials (per card: ok, reason, particles, agreement, fsc), resolutionA, particles, fsc, diameterA (1.33 × the card's size, as
  diameterFrom "card" says), searched ("look" or "raw": the tomogram the template
  was searched on) with searchVoxelA, ctfLimited (true when the FSC has a dip
  that rises again, a local minimum below 0.5 followed by a value 0.15 higher:
  a zero of the contrast transfer; resolutionA is then the dip and signalA
  is where the halves last agree, finer), sharpenB (the B factor in Å², negative,
  applied to average.mrc; left out when no fit was possible) and seconds (the whole run).

OUTPUT
  Files are written in the input's folder, each step in a subfolder of its own
  (1_motion, 2_tomograms, 3_clean, 4_sta; an older project stays flat beside the
  input); copy the folder back and open them in
  TifMrcView Pro (label maps open with their tomogram). Progress goes to
  stderr. The result is one JSON object on stdout ({"ok":true,…}), also
  saved as <input>.tmv.json (tmv tomo --for sta|survey: <input>.sta.tmv.json,
  <input>.survey.tmv.json; tmv sta: <out>/sta.tmv.json). Exit status 0 on success, 1 on error
  ({"ok":false,"error":…}), 2 when a choice is needed (spa: pick a kind;
  sta: pick a card).

  The GPU is found automatically (CUDA with an NVIDIA driver, else Vulkan,
  else CPU). On a shared server pick the card with CUDA_VISIBLE_DEVICES=N;
  TMV_GPU=cuda|vulkan|cpu forces a backend. A job that needs the GPU waits for
  another tmv job using it to finish (TMV_GPU_SHARE=1: run alongside it). Long runs:
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
    // Held until this job ends.
    let _turn = if matches!(cmd, "run" | "tomo" | "sta" | "membranes" | "membrane" | "spa" | "ctf" | "denoise") { gpu_turn() } else { None };
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
        const FLAGS: [&str; 4] = ["--sam", "--fresh", "--auto", "--kinds"];
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

/// This job's turn on the GPU: two jobs at once on one GPU run it out of
/// memory, and the one that fails may be hours in. So a job waits for the
/// one before it (a lock file in the temporary folder, one per choice of
/// CUDA_VISIBLE_DEVICES, released when the job ends however it ends).
/// None when there is no GPU, when TMV_GPU_SHARE is set, or when the lock
/// cannot be had at all (the job then runs as before).
fn gpu_turn() -> Option<std::fs::File> {
    gpu()?;
    if std::env::var_os("TMV_GPU_SHARE").is_some() {
        return None;
    }
    let which: String = std::env::var("CUDA_VISIBLE_DEVICES").unwrap_or_default().chars().filter(char::is_ascii_alphanumeric).collect();
    let name = if which.is_empty() { "tifmrcview-gpu.lock".to_string() } else { format!("tifmrcview-gpu-{which}.lock") };
    take_turn(&std::env::temp_dir().join(name), &|| {
        eprintln!("tmv: another tmv job is using the GPU: waiting for it to finish (two at once run out of GPU memory). TMV_GPU_SHARE=1 runs alongside it instead.")
    })
}

/// The lock on `path`, waiting for whoever holds it (`waiting` is called
/// once before the wait). None when the file cannot be opened or locked.
fn take_turn(path: &Path, waiting: &dyn Fn()) -> Option<std::fs::File> {
    // Another user's file can be read, and a lock needs no more.
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).or_else(|_| std::fs::File::open(path)).ok()?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            waiting();
            file.lock().ok()?;
        }
        Err(_) => return None,
    }
    Some(file)
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

/// The options of this run that the same run with another choice needs
/// again: where the results go and the pixel size the files do not say.
fn again(o: &Opts, out: &Path) -> String {
    let mut s = String::new();
    if o.get("out").is_some() {
        s += &format!(" --out {}", out.display());
    }
    if let Some(p) = o.get("pixel") {
        s += &format!(" --pixel {p}");
    }
    s
}

/// Makes sure the pixel size of the tomogram or tilt series `path` is known
/// for `cmd`: the file's own (header or mdoc) always wins; for a file that
/// does not say it, `--pixel` is remembered for it (`tomo::output::set_pixel`),
/// and without `--pixel` this is an error. Returns the pixel size in Å.
fn require_pixel(cmd: &str, path: &str, img: &std::sync::Arc<OpenImage>, pixel: Option<f64>) -> Result<f64> {
    use crate::tomo::output::{pixel_known, set_pixel};
    match (pixel_known(img), pixel) {
        (Some(known), given) => {
            if let Some(g) = given.filter(|g| (g - known).abs() > 0.01 * known) {
                eprintln!("tmv {cmd}: {path} says its pixel size is {known:.4} Å, not the {g} Å of --pixel: the file's value is used");
            }
            Ok(known)
        }
        (None, Some(g)) => {
            set_pixel(path, Some(g));
            Ok(g)
        }
        (None, None) => bail!("{path} does not say its pixel size (the header has 0 or 1): pass it with --pixel <Å>"),
    }
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
    let px = require_pixel("tomo", &stack, &img, pixel)?;
    let mut d = Dataset::open(&info.path, info.size_z, Some(px))?;
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

/// `--out`, else the project's `4_sta/` for a tomogram of a new project (or
/// a raw tilt series of one: `tmv sta` first makes its tomogram there), else
/// `<stem>_sta` beside the first input (old flat project, or a tomogram from elsewhere).
fn sta_out(o: &Opts, first: &str) -> Result<PathBuf> {
    if let Some(d) = o.get("out") {
        return std::path::absolute(d).with_context(|| format!("--out {d}"));
    }
    // A tilt series not yet processed (no dataset beside it, none of an old
    // flat project) belongs to a new project: its tomogram goes to `2_tomograms/`.
    let series = if ext(first) == "mdoc" {
        Some(format!("{}.mrc", first.strip_suffix(".mdoc").map(|s| s.strip_suffix(".mrc").unwrap_or(s)).unwrap_or(first)))
    } else if auto_sta::dataset_of(first).is_none() && Path::new(first).is_file() && open_image(first).is_ok_and(|img| has_tilts(first, img.info().size_z)) {
        Some(first.to_string())
    } else {
        None
    };
    if let Some(stack) = series.filter(|s| !crate::project_dirs::is_legacy(s)) {
        return Ok(crate::project_dirs::root_of(Path::new(&stack)).join(crate::project_dirs::STA));
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

/// The stderr line saying a repeating pattern was found in the layers of a drawn surface and used to align, when so.
fn pattern_line(rows_a: f64, inside_a: f64, fold: u64, symmetric: bool) -> String {
    let side = if inside_a < 0.0 { "outside" } else { "inside" };
    let axes = match (fold > 1, symmetric) {
        (true, true) => format!("; it repeats itself {fold} times round: the particles are centred on those axes and the average is made with those turns (C{fold})"),
        (true, false) => format!("; it repeats itself {fold} times round, and the particles are centred on those axes"),
        _ => String::new(),
    };
    format!("tmv sta: found a repeating pattern {:.0} Å {side} the surface (rows {:.0} Å apart) and aligned by it{axes}", inside_a.abs(), rows_a)
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
        Note::Marked { clicked, members, size_a } => eprintln!("tmv sta: you marked {clicked} particles; the template is the average of {members} like them (about {size_a:.0} Å across)"),
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

/// What `tmv sta` does after the cards: nothing (the user chooses), the card
/// the user named, the marks the user made, or (`--auto`, unattended batch
/// runs only) whatever the program picks.
#[derive(Debug, PartialEq)]
enum StaMode {
    Find,
    Pick(usize),
    Points(String),
    /// Every kind of mark (points, lines, surfaces, spheres, painted membranes), as the workspace runs them.
    Marks(String),
    /// A density map (MRC or EM) the user brings as the template.
    Template(String),
    /// The kinds among the particles of a finished average, and (`--keep`) the average of some of them.
    Kinds(Option<Vec<usize>>),
    Auto,
}

fn sta_mode(o: &Opts) -> Result<StaMode> {
    let pick = o.get("pick").map(|s| s.parse::<usize>().with_context(|| format!("--pick: `{s}` is not a card number (0, 1, 2, …)"))).transpose()?;
    let points = o.get("points");
    let marks = o.get("marks");
    let template = o.get("template");
    let auto = o.flag("auto");
    let kinds = o.flag("kinds");
    if o.get("keep").is_some() && !kinds {
        bail!("--keep goes with --kinds (tmv sta --kinds --keep 1,3 …)");
    }
    if [pick.is_some(), points.is_some(), marks.is_some(), template.is_some(), auto, kinds].iter().filter(|b| **b).count() > 1 {
        bail!("--pick, --points, --marks, --template, --kinds and --auto exclude each other (--pick N / --points or --marks marks.json / --template map.mrc: what the user chose; --kinds: look at a finished average; --auto: the program chooses)");
    }
    if kinds {
        return Ok(StaMode::Kinds(o.get("keep").map(crate::sta::placed::parse_keep).transpose()?));
    }
    Ok(match (pick, points) {
        (Some(n), _) => StaMode::Pick(n),
        (_, Some(f)) => StaMode::Points(f.to_string()),
        _ if marks.is_some() => StaMode::Marks(marks.unwrap().to_string()),
        _ if template.is_some() => StaMode::Template(template.unwrap().to_string()),
        _ if auto => StaMode::Auto,
        _ => StaMode::Find,
    })
}

/// `tmv sta --marks marks.json`: what the user marked (points, lines,
/// surfaces, spheres, painted membranes), averaged as the workspace's
/// "average what I marked" does (`sta::marks::run_marks`): points make a
/// template, shapes give seeds with their directions. The marks name the
/// tomogram (or its look file) they were made on, which must be among the
/// inputs. `--size small|medium|large` (or the `size` saved with the marks)
/// is the particle size for lines, surfaces and membranes.
fn sta_marks(o: &Opts, file: &str, found: &[auto_sta::Resolved], out: &Path, progress: &Progress, tomograms: Vec<Value>) -> Result<Value> {
    use crate::sta::marks::{run_marks, Mark, Saved};
    let bytes = std::fs::read(file).with_context(|| format!("--marks {file}: cannot read"))?;
    let (marks, saved_size): (Vec<Mark>, Option<String>) = serde_json::from_slice::<Saved>(&bytes)
        .map(|s| (s.marks, s.size))
        .or_else(|_| serde_json::from_slice::<Vec<Mark>>(&bytes).map(|m| (m, None)))
        .with_context(|| format!("--marks {file}: not a marks.json"))?;
    if marks.is_empty() {
        bail!("--marks {file}: no marks in it");
    }
    let size = o.get("size").map(str::to_string).or(saved_size);
    let inputs: Vec<String> = found.iter().map(|f| f.tomogram.clone()).collect();
    let emit = |name: &str, v: Value| match name {
        "sta-progress" => progress.report(v["label"].as_str().unwrap_or(""), v["done"].as_u64().unwrap_or(0) as usize, v["total"].as_u64().unwrap_or(0) as usize),
        "sta-note" => eprintln!("tmv sta: {v}"),
        _ => {}
    };
    let dev = || Ok(device());
    let mut v = run_marks(&inputs, &marks, size.as_deref(), Some(&out.to_string_lossy()), &dev, &emit, &|_| String::new())?;
    progress.done();
    let r = &v["report"];
    let map = r["map"].as_str().unwrap_or("average.mrc");
    match r["resolutionA"].as_f64() {
        Some(a) => eprintln!("tmv sta: done: {a:.1} Å resolution from {} particles; the average is {map}", r["particles"]),
        None => eprintln!("tmv sta: done: {} particles averaged, no resolution from the halves; the average is {map}", r["particles"]),
    }
    if let (Some(rows), Some(inside)) = (r["pattern"]["rowsA"].as_f64(), r["pattern"]["insideA"].as_f64()) {
        eprintln!("{}", pattern_line(rows, inside, r["pattern"]["fold"].as_u64().unwrap_or(1), r["pattern"]["symmetric"].as_bool().unwrap_or(false)));
    }
    if let Value::Object(m) = &mut v {
        m.remove("result");
        m.remove("cards");
        m.remove("trials");
        m.remove("chosen");
    }
    v["marks"] = json!(marks.len());
    v["tomograms"] = json!(tomograms);
    v["open"] = v["report"]["map"].clone();
    Ok(v)
}

/// `tmv sta --template map.mrc|.em`: the map the user already has is the
/// template (`sta::review::own_map`), everything like it is found in the
/// inputs (`find_own`) and all that is found is averaged (`average_found`),
/// without stopping, into `<out>/own/`; prints the report as the other routes do.
fn sta_template(map: &str, found: &[auto_sta::Resolved], out: &Path, progress: &Progress, tomograms: Vec<Value>) -> Result<Value> {
    use crate::sta::review;
    let map = std::fs::canonicalize(map).with_context(|| format!("--template {map}: no such file"))?.to_string_lossy().into_owned();
    let inputs: Vec<String> = found.iter().map(|f| f.tomogram.clone()).collect();
    let out_s = out.to_string_lossy().into_owned();
    let emit = |name: &str, v: Value| match name {
        "sta-progress" => progress.report(v["label"].as_str().unwrap_or(""), v["done"].as_u64().unwrap_or(0) as usize, v["total"].as_u64().unwrap_or(0) as usize),
        "sta-note" => eprintln!("tmv sta: {v}"),
        _ => {}
    };
    let dev = || Ok(device());
    let b64 = |_: &[u8]| String::new();
    let own = review::own_map(&inputs, &map, &out_s, &b64).with_context(|| format!("--template {map}"))?;
    if own["assumed"] == true {
        eprintln!("tmv sta: the map does not say its voxel size; the tomogram's is taken");
    }
    review::find_own(&inputs, &out_s, &dev, &emit, &b64)?;
    let dir = review::own_dir(out);
    let mut v = review::average_found(&dir, &dev, &emit, &b64)?;
    progress.done();
    let r = v["report"].clone();
    let shown = r["map"].as_str().unwrap_or("average.mrc");
    match r["resolutionA"].as_f64() {
        Some(a) => eprintln!("tmv sta: done: {a:.1} Å resolution (FSC 0.143 between the two halves) from {} particles; the average is {shown}", r["particles"]),
        None => eprintln!("tmv sta: done: {} particles averaged, the halves did not agree well enough to give a resolution; the average is {shown}", r["particles"]),
    }
    if let (Some(rows), Some(inside)) = (r["pattern"]["rowsA"].as_f64(), r["pattern"]["insideA"].as_f64()) {
        eprintln!("{}", pattern_line(rows, inside, r["pattern"]["fold"].as_u64().unwrap_or(1), r["pattern"]["symmetric"].as_bool().unwrap_or(false)));
    }
    if let Value::Object(m) = &mut v {
        m.remove("result");
        m.remove("cards");
        m.remove("trials");
        m.remove("chosen");
    }
    v["template"] = json!(map);
    v["tomograms"] = json!(tomograms);
    v["open"] = r["map"].clone();
    Ok(v)
}

/// `tmv sta --kinds [--keep 1,3]`: the particles of the finished average in
/// `out` are sorted into kinds (`sta::placed::run_kinds`; kept in
/// `kinds/kinds.json`), listed, and with `--keep` only those kinds are
/// averaged (`run_keep`; the kinds already saved for this average are used).
fn sta_kinds(keep: Option<&[usize]>, found: &[auto_sta::Resolved], out: &Path, progress: &Progress, tomograms: Vec<Value>) -> Result<Value> {
    use crate::sta::placed;
    let inputs: Vec<String> = found.iter().map(|f| f.tomogram.clone()).collect();
    let out_s = out.to_string_lossy().into_owned();
    let emit = |name: &str, v: Value| match name {
        "sta-progress" => progress.report(v["label"].as_str().unwrap_or(""), v["done"].as_u64().unwrap_or(0) as usize, v["total"].as_u64().unwrap_or(0) as usize),
        "sta-note" => eprintln!("tmv sta: {v}"),
        _ => {}
    };
    let b64 = |_: &[u8]| String::new();
    let saved = if keep.is_some() { placed::kinds_saved(&out_s, None, &b64).ok().flatten() } else { None };
    let mut v = match saved {
        Some(v) => v,
        None => {
            let v = placed::run_kinds(&inputs, &out_s, None, &emit, &b64)?;
            progress.done();
            v
        }
    };
    let list = v["kinds"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        eprintln!("tmv sta: the particles are all of one kind as far as can be told (no clearly different kinds found)");
    } else {
        eprintln!("tmv sta: {} kinds among {} particles:", list.len(), v["particles"]);
        for k in &list {
            eprintln!("  kind {}: {} particles ({:.0} %), average {}", k["id"], k["count"], 100.0 * k["share"].as_f64().unwrap_or(0.0), k["map"].as_str().unwrap_or(""));
        }
    }
    for k in v["kinds"].as_array_mut().into_iter().flatten() {
        if let Value::Object(m) = k {
            m.remove("png");
        }
    }
    v["tomograms"] = json!(tomograms);
    if let Some(keep) = keep {
        let have: Vec<usize> = list.iter().filter_map(|k| k["id"].as_u64().map(|i| i as usize)).collect();
        if have.is_empty() {
            bail!("--keep: there is only one kind, nothing to choose from");
        }
        if let Some(bad) = keep.iter().find(|k| !have.contains(k)) {
            bail!("--keep: there is no kind {bad} (the kinds are {})", have.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", "));
        }
        let mut kept = placed::run_keep(&inputs, &out_s, None, keep, &emit, &b64)?;
        progress.done();
        let r = kept["report"].clone();
        let shown = r["map"].as_str().unwrap_or("average.mrc");
        match r["resolutionA"].as_f64() {
            Some(a) => eprintln!("tmv sta: done: {a:.1} Å resolution (FSC 0.143 between the two halves) from {} particles of kinds {}; the average is {shown}", r["particles"], keep.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")),
            None => eprintln!("tmv sta: done: {} particles averaged, the halves did not agree well enough to give a resolution; the average is {shown}", r["particles"]),
        }
        if kept["fromTiltImages"] == true {
            eprintln!("tmv sta: the first average came from the tilt images and may be sharper; this one is from the tomograms at the same sampling");
        }
        if let Value::Object(m) = &mut kept {
            m.remove("result");
        }
        v["open"] = r["map"].clone();
        v["resolutionA"] = r["resolutionA"].clone();
        v["particles"] = r["particles"].clone();
        v["map"] = r["map"].clone();
        v["kept"] = kept;
    }
    Ok(v)
}

/// The hints shown with a card on stderr; only hints, they never choose.
fn card_hints(c: &crate::sta::project::Card) -> Vec<&'static str> {
    let mut h = vec![];
    if c.likely {
        h.push("likely a particle");
    }
    if c.elongation >= crate::sta::project::ELONGATION {
        h.push("line-like or layer-like");
    }
    h
}

fn card_line(c: &crate::sta::project::Card) -> String {
    let hints = card_hints(c);
    format!("  card {}: about {:.0} Å, {} members{}", c.id, c.diameter_a, c.members, if hints.is_empty() { String::new() } else { format!(" ({})", hints.join(", ")) })
}

/// `tmv sta`: the tomogram of each input (made as `tmv tomo --for sta` does),
/// class-average cards of the blobs in them, then (when the user has chosen:
/// `--pick`, `--points`; or `--auto`) the average of everything that looks
/// like the chosen card (`sta::auto::auto`). Without a choice it stops at the cards.
fn sta(o: &Opts) -> Result<Value> {
    use crate::sta::project as sp;
    let inputs: Vec<String> = o.input.iter().chain(&o.more).cloned().collect();
    if inputs.is_empty() {
        bail!("give at least one tilt series or tomogram: tmv sta [--pick N | --points marks.json | --template map.mrc | --auto] [--out DIR] [--pixel Å] INPUT [MORE …]");
    }
    let mode = sta_mode(o)?;
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
            let dataset = auto_sta::dataset_of(input);
            if dataset.is_none() {
                require_pixel("sta", input, &open_image(input)?, o.number("pixel")?)?;
            }
            (input.clone(), dataset)
        };
        found.push(auto_sta::resolve(input, tomogram, dataset, &note));
    }
    let sources: Vec<sp::Source> = found.iter().map(auto_sta::Resolved::source).collect();
    let tomograms: Vec<Value> = found
        .iter()
        .map(|f| json!({ "input": f.input, "tomogram": f.tomogram, "look": f.look.as_ref().map(|l| &l.path), "tiltsDeg": f.tilts, "tiltsFrom": f.tilts_from }))
        .collect();

    if let StaMode::Find = mode {
        let cards = auto_sta::find(&sources, &out, &|| Ok(device()), &|s, i, n| progress.report(s, i, n), &|_| {}, &note)?;
        progress.done();
        let cards_dir = out.join("cards");
        let tomos: Vec<&str> = found.iter().map(|f| f.tomogram.as_str()).collect();
        let next = format!("tmv sta --pick N{} {}", again(o, &out), tomos.join(" "));
        eprintln!("tmv sta: {} cards made in {}:", cards.len(), cards_dir.display());
        for c in &cards {
            eprintln!("{}", card_line(c));
        }
        eprintln!(
            "tmv sta: nothing is averaged until the user chooses. Look at {}/card_N.mrc (32³ class averages; open in TifMrcView Pro, best on the clear look tomogram), show them to the user and ask which one; then rerun with --pick N ({next}), or --points marks.json for what the user marked. (--auto lets the program choose, for unattended batch runs only.)",
            cards_dir.display()
        );
        return Ok(json!({
            "needsChoice": true,
            "cardsDir": cards_dir,
            "cards": cards.iter().map(card_json).collect::<Vec<_>>(),
            "tomograms": tomograms,
            "next": next,
        }));
    }
    if let StaMode::Kinds(keep) = &mode {
        return sta_kinds(keep.as_deref(), &found, &out, &progress, tomograms);
    }
    if let StaMode::Marks(file) = &mode {
        return sta_marks(o, file, &found, &out, &progress, tomograms);
    }
    if let StaMode::Template(map) = &mode {
        return sta_template(map, &found, &out, &progress, tomograms);
    }
    if let StaMode::Points(file) = &mode {
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
        if let Some(p) = &d.report.pattern {
            eprintln!("{}", pattern_line(p.rows_a, p.inside_a, p.fold as u64, p.symmetric));
        }
        let mut v = serde_json::to_value(&d.report)?;
        v["card"] = card_json(&d.card);
        v["points"] = json!(points.len());
        v["out"] = json!(dir);
        v["tomograms"] = json!(tomograms);
        v["backend"] = json!(d.backend);
        v["open"] = json!(d.report.map);
        return Ok(v);
    }
    let outcome = auto_sta::auto(&sources, if let StaMode::Pick(n) = mode { Some(n) } else { None }, &out, &|| Ok(device()), &|s, i, n| progress.report(s, i, n), &|_| {}, &note)?;
    let cards_dir = out.join("cards");
    let d = match outcome {
        auto_sta::Outcome::Done(d) => d,
        auto_sta::Outcome::NeedsChoice { cards, trials, good, cards_dir } => {
            let tomos: Vec<&str> = found.iter().map(|f| f.tomogram.as_str()).collect();
            let rerun = format!("tmv sta --pick N{} {}", again(o, &out), tomos.join(" "));
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
    if let Some(p) = &r.pattern {
        eprintln!("{}", pattern_line(p.rows_a, p.inside_a, p.fold as u64, p.symmetric));
    }
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
    if let Some(note) = &r.note {
        eprintln!("tmv membranes: {note}");
    }
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
        "copies": r.copies,
        "note": r.note,
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

    /// A second job waits for the first one's turn on the GPU to end.
    #[test]
    fn a_job_waits_for_the_turn_of_the_one_before() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu.lock");
        let first = super::take_turn(&path, &|| panic!("nobody to wait for")).expect("the lock");
        let waited = std::sync::Arc::new(AtomicBool::new(false));
        let done = std::sync::Arc::new(AtomicBool::new(false));
        let (w, d, p) = (waited.clone(), done.clone(), path.clone());
        let second = std::thread::spawn(move || {
            let turn = super::take_turn(&p, &|| w.store(true, Ordering::SeqCst));
            d.store(true, Ordering::SeqCst);
            turn.is_some()
        });
        while !waited.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!done.load(Ordering::SeqCst), "the second job ran while the first held the GPU");
        drop(first);
        assert!(second.join().unwrap());
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
            pattern: None,
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
        let plan = |cards, trials| Plan { cards, trials, average: true };
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

    #[test]
    fn sta_stops_at_the_cards_unless_told() {
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let mode = |extra: &[&str]| sta_mode(&Opts::parse(&[vec![dir.clone()], extra.iter().map(|s| s.to_string()).collect()].concat()).unwrap());
        // No choice: the cards only.
        assert_eq!(mode(&[]).unwrap(), StaMode::Find);
        assert_eq!(mode(&["--out", "x"]).unwrap(), StaMode::Find);
        assert_eq!(mode(&["--pick", "2"]).unwrap(), StaMode::Pick(2));
        assert_eq!(mode(&["--pick=0"]).unwrap(), StaMode::Pick(0));
        assert_eq!(mode(&["--points", "m.json"]).unwrap(), StaMode::Points("m.json".into()));
        assert_eq!(mode(&["--marks", "m.json"]).unwrap(), StaMode::Marks("m.json".into()));
        assert!(mode(&["--marks", "m.json", "--points", "m.json"]).is_err());
        assert_eq!(mode(&["--template", "map.mrc"]).unwrap(), StaMode::Template("map.mrc".into()));
        assert_eq!(mode(&["--template=map.em"]).unwrap(), StaMode::Template("map.em".into()));
        assert!(mode(&["--template", "map.mrc", "--auto"]).is_err());
        assert!(mode(&["--template", "map.mrc", "--pick", "1"]).is_err());
        // --kinds is a flag; --keep goes with it only.
        assert_eq!(mode(&["--kinds"]).unwrap(), StaMode::Kinds(None));
        assert_eq!(mode(&["--kinds", "--keep", "1,3"]).unwrap(), StaMode::Kinds(Some(vec![1, 3])));
        assert_eq!(mode(&["--keep=2", "--kinds"]).unwrap(), StaMode::Kinds(Some(vec![2])));
        assert!(mode(&["--keep", "1"]).is_err());
        assert!(mode(&["--kinds", "--keep", "0"]).is_err());
        assert!(mode(&["--kinds", "--pick", "1"]).is_err());
        assert!(mode(&["--kinds", "--auto"]).is_err());
        // --auto is a flag without a value and goes before or after the input.
        assert_eq!(mode(&["--auto"]).unwrap(), StaMode::Auto);
        let o = Opts::parse(&["--auto".into(), dir.clone()]).unwrap();
        assert_eq!((sta_mode(&o).unwrap(), o.input.is_some()), (StaMode::Auto, true));
        // One way to choose at a time; a card number is a number.
        assert!(mode(&["--auto", "--pick", "1"]).is_err());
        assert!(mode(&["--auto", "--points", "m.json"]).is_err());
        assert!(mode(&["--pick", "1", "--points", "m.json"]).is_err());
        assert!(mode(&["--pick", "x"]).is_err());
    }

    #[test]
    fn cards_are_listed_with_hints_only() {
        let mut c = card(3);
        assert_eq!(card_line(&c), "  card 3: about 210 Å, 50 members");
        c.likely = true;
        assert_eq!(card_line(&c), "  card 3: about 210 Å, 50 members (likely a particle)");
        c.likely = false;
        c.elongation = 2.4;
        assert_eq!(card_line(&c), "  card 3: about 210 Å, 50 members (line-like or layer-like)");
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
    fn pixel_is_required_only_for_files_that_do_not_say_it() {
        let dir = std::env::temp_dir().join(format!("tmv_reqpix_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mk = |name: &str, px: f64| {
            let f = dir.join(name).to_string_lossy().into_owned();
            crate::tomo::output::write_mrc(&f, &vec![0.5; 4 * 4 * 2], [4, 4, 2], px, false).unwrap();
            f
        };
        let (unset, set) = (mk("unset.mrc", 0.0), mk("set.mrc", 2.7));
        let o = Opts::parse(&[unset.clone(), "--pixel".into(), "3.5".into()]).unwrap();
        assert_eq!(o.number("pixel").unwrap(), Some(3.5));
        let err = require_pixel("sta", &unset, &open_image(&unset).unwrap(), None).unwrap_err().to_string();
        assert!(err.contains("does not say its pixel size") && err.contains("--pixel <Å>"), "{err}");
        assert_eq!(require_pixel("sta", &unset, &open_image(&unset).unwrap(), Some(3.5)).unwrap(), 3.5);
        assert_eq!(crate::tomo::output::pixel_a(&open_image(&unset).unwrap()), 3.5);
        // A file that states its pixel size keeps it, whatever --pixel says.
        let got = require_pixel("sta", &set, &open_image(&set).unwrap(), Some(9.0)).unwrap();
        assert!((got - 2.7).abs() < 1e-4);
        assert!((crate::tomo::output::pixel_a(&open_image(&set).unwrap()) - 2.7).abs() < 1e-4);
        crate::tomo::output::set_pixel(&unset, None);
        std::fs::remove_dir_all(&dir).ok();
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
    fn sta_works_in_the_project_folder_of_a_new_project_and_beside_an_old_one() {
        let o = Opts::default();
        // A tomogram (or its look file) of a new project: the project's own folder.
        assert_eq!(sta_out(&o, "/data/p/2_tomograms/X_sart3.mrc").unwrap(), Path::new("/data/p/4_sta"));
        assert_eq!(sta_out(&o, "/data/p/3_clean/X_look.mrc").unwrap(), Path::new("/data/p/4_sta"));
        // A tilt series not yet processed, given as its mdoc: the folder of the frames.
        assert_eq!(sta_out(&o, "/data/p/ts_01.mrc.mdoc").unwrap(), Path::new("/data/p/4_sta"));
        // The same series of an old flat project (its dataset beside the stack) stays flat.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ts_01.tomo.json"), b"{}").unwrap();
        let mdoc = dir.path().join("ts_01.mrc.mdoc").to_string_lossy().into_owned();
        assert_eq!(sta_out(&o, &mdoc).unwrap(), dir.path().join("ts_01.mrc_sta"));
        let tomogram = dir.path().join("ts_01_sart3.mrc").to_string_lossy().into_owned();
        assert_eq!(sta_out(&o, &tomogram).unwrap(), dir.path().join("ts_01_sart3_sta"));
    }

    #[test]
    fn errors_come_as_json_with_hints() {
        assert!(hint("不知道像素大小").is_some());
        assert!(hint("no MemBrain-seg membrane model set up").is_some());
        assert_eq!(main(vec!["nonsense".into()]), 1);
        assert_eq!(main(vec!["--version".into()]), 0);
    }
}
