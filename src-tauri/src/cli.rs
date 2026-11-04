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
  tmv membranes <tomogram.mrc>   Membranes, vesicles and microtubules → label map
        [--model membrain.ckpt]
  tmv spa <folder>               One-click single-particle analysis
        [--kind N[,M]] [--symmetry C1|D7|…] [--pixel Å] [--kv kV]
  tmv ctf <folder|micrograph>    CTF of every micrograph (movies are motion corrected)
        [--pixel Å] [--kv kV]
  tmv denoise <image|stack>      Self-supervised denoising → <name>_denoised.mrc
  tmv info                       Version, GPU and models found
  tmv setup [--membrane membrain.ckpt] [--sam]
                                 Remember the MemBrain-seg model; download SAM 2.1

  Every choice is made automatically, as in the app. --for picks what the
  tomogram is for: display (default: clearest picture, denoised, missing
  wedge filled, membranes when a model is set), sta (subtomogram averaging:
  full detail + RELION 5 folder), survey (quick look).

OUTPUT
  Files are written beside the input; copy the folder back and open them in
  TifMrcView Pro (label maps open with their tomogram). Progress goes to
  stderr. The result is one JSON object on stdout ({"ok":true,…}), also
  saved as <input>.tmv.json. Exit status 0 on success, 1 on error
  ({"ok":false,"error":…}), 2 when a choice is needed (spa: pick a kind).

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
    let opts = match Opts::parse(&args[1..]) {
        Ok(o) => o,
        Err(e) => return fail(cmd, &e, None),
    };
    let started = Instant::now();
    if matches!(cmd, "run" | "tomo" | "membranes" | "membrane" | "spa" | "ctf" | "denoise") {
        let _ = gpu();
    }
    let result = match cmd {
        "run" => run(&opts),
        "tomo" => tomo(&opts),
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
            emit(&v, opts.input.as_deref());
            status
        }
        Err(e) => fail(cmd, &e, opts.input.as_deref()),
    }
}

fn fail(cmd: &str, e: &anyhow::Error, input: Option<&str>) -> i32 {
    let msg = format!("{e:#}");
    eprintln!("tmv {cmd}: {msg}");
    let mut v = json!({ "ok": false, "command": cmd, "version": VERSION, "error": msg });
    if let Some(h) = hint(&msg) {
        v["hint"] = json!(h);
    }
    emit(&v, input);
    1
}

/// What to do about the common errors, for whoever reads the JSON.
fn hint(msg: &str) -> Option<&'static str> {
    if msg.contains("像素大小") || msg.contains("pixel size") {
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

/// Prints the result and saves it beside the input.
fn emit(v: &Value, input: Option<&str>) {
    let text = serde_json::to_string_pretty(v).unwrap_or_default();
    println!("{text}");
    let _ = std::io::stdout().flush();
    if let Some(input) = input {
        let p = Path::new(input);
        let file = if p.is_dir() { p.join("tmv.json") } else { PathBuf::from(format!("{input}.tmv.json")) };
        let _ = std::fs::write(file, text + "\n");
    }
}

/// Command-line options: one input path and `--key value` pairs.
#[derive(Default)]
struct Opts {
    input: Option<String>,
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
            } else if o.input.is_none() {
                let p = Path::new(a);
                // Absolute, so the paths saved in project files stay valid.
                let abs = std::fs::canonicalize(p).with_context(|| format!("{a}: no such file or folder"))?;
                o.input = Some(abs.to_string_lossy().into_owned());
            } else {
                bail!("unexpected argument `{a}` (one input at a time)");
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
    let mut p = crate::spa::project::Project::open(o.input()?)?;
    let mut changed = false;
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
    if !p.optics_found.pixel || p.optics.pixel_a <= 0.0 {
        bail!("不知道像素大小（文件和配置文件里都没有）");
    }
    if !p.optics_found.kv {
        bail!("不知道电压（文件和配置文件里都没有）");
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
    Ok(json!({ "project": p.dir, "table": table, "ctf": stats, "pixelA": p.optics.pixel_a, "kv": p.optics.kv }))
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
    fn errors_come_as_json_with_hints() {
        assert!(hint("不知道像素大小").is_some());
        assert!(hint("no MemBrain-seg membrane model set up").is_some());
        assert_eq!(main(vec!["nonsense".into()]), 1);
        assert_eq!(main(vec!["--version".into()]), 0);
    }
}
