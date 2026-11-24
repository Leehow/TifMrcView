//! Format and metadata plugins: which installed packages handle which file
//! extensions, their compiled WebAssembly modules, and the [`Source`] that
//! reads a file through one.
//!
//! The registry is global because file opening (`formats::open`) happens in
//! many places: the viewer, thumbnails, the probe tool. It is configured
//! once with the plugin folder and told when packages change (install,
//! uninstall, reload) or are switched on and off in the UI.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use anyhow::{anyhow, bail, Context, Result};
use memmap2::Mmap;
use serde::Deserialize;
use wasmi::Module;

use super::package::{self, Manifest};
use super::recipe;
use super::wasm::{self, Exports, Instance};
use crate::image::{ChannelInfo, DType, DimOrder, ImageInfo, MetaGroup, Plane, SeriesInfo, Source, SpectrumInfo};

/// Largest page a plugin may describe (bytes).
const MAX_PAGE_BYTES: u64 = 1 << 34;
/// Where the UI's on/off switches are remembered, inside the plugin folder.
const DISABLED_FILE: &str = ".disabled.json";

/// Vendor formats the official plugins open, so a missing plugin gets a
/// helpful message: (extension, plugin id, plugin name).
const KNOWN: &[(&str, &str, &str)] = &[
    ("czi", "zeiss", "Zeiss"),
    ("lsm", "zeiss", "Zeiss"),
    ("lif", "lasx", "Leica LAS X"),
    ("lof", "lasx", "Leica LAS X"),
    ("nd2", "nis", "Nikon NIS-Elements"),
    ("oib", "fluoview", "Olympus FluoView"),
    ("oir", "fluoview", "Olympus FluoView"),
    ("ims", "imaris", "Imaris"),
    ("dm3", "gatan", "Gatan"),
    ("dm4", "gatan", "Gatan"),
    ("ser", "thermo", "Thermo Fisher"),
    ("emi", "thermo", "Thermo Fisher"),
    ("emd", "thermo", "Thermo Fisher"),
    ("eer", "thermo", "Thermo Fisher"),
    ("tvips", "tvips", "TVIPS"),
];

/// Acquisition folders the official plugins read (`stacks` in their
/// manifests), so a folder can suggest the plugin before it is installed:
/// (project file names as in `stacks`, plugin id, plugin name).
const KNOWN_PROJECTS: &[(&[&str], &str, &str)] = &[
    (&["*.lif", "*.lof", "*.xlef", "*.xlif", "MetaData/*.lif_*.xml"], "lasx", "Leica LAS X"),
    (&["ProjectData.dat"], "thermo", "Thermo Fisher"),
];

/// What the folder of an opened image says about it (`Registry::companion`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Companion {
    /// The image belongs to a stack an enabled plugin reads: open `path`
    /// (image `series`) to see the whole acquisition.
    #[serde(rename_all = "camelCase")]
    Open {
        plugin: String,
        path: PathBuf,
        series: usize,
        /// The image in the acquisition (series name or file name).
        title: String,
        detail: String,
        /// Whether the opened image is one of `files` (else the folder just
        /// holds the project).
        matched: bool,
        /// The other files of the same image, for the UI to reuse the answer.
        files: Vec<PathBuf>,
    },
    /// A plugin that is not installed (or switched off, or too old) would
    /// read the project file `found`.
    #[serde(rename_all = "camelCase")]
    Plugin { id: String, plugin: String, found: String, action: &'static str },
}

/// One installed package with a WebAssembly module.
struct Pkg {
    manifest: Manifest,
    dir: PathBuf,
    module: OnceLock<std::result::Result<(Module, Exports), String>>,
}

/// Enabled local format package for the remote WASM preview sandbox.
pub struct PreviewPackage {
    pub manifest: serde_json::Value,
    pub wasm: Vec<u8>,
    pub app_version: String,
}

impl Pkg {
    fn module(&self) -> Result<&Module> {
        let r = self.module.get_or_init(|| {
            let path = self.dir.join(package::safe_rel(self.manifest.wasm.as_deref().unwrap_or("")).unwrap_or_default());
            let bytes = std::fs::read(&path).map_err(|e| format!("读不到 {}：{e}", path.display()))?;
            wasm::compile(&bytes).map_err(|e| format!("{e:#}"))
        });
        match r {
            Ok((m, _)) => Ok(m),
            Err(e) => bail!("插件「{}」无法加载：{e}", self.manifest.name),
        }
    }

    fn opens(&self, ext: &str) -> Option<&str> {
        self.manifest.formats.iter().find(|f| f.ext.iter().any(|e| e == ext)).map(|f| f.name.as_str())
    }

    fn reads_metadata(&self, ext: &str) -> bool {
        self.manifest.metadata.iter().any(|e| e == ext)
    }

    /// The `stacks` entry for a project file `path` (its name, and the name
    /// of its folder for entries that point into a subfolder).
    fn stack_decl(&self, path: &Path) -> Option<&package::StackDecl> {
        let (folder, name) = folder_and_name(path)?;
        self.manifest.stacks.iter().find(|d| d.matches_in(&folder, &name))
    }
}

#[derive(Default)]
struct Index {
    /// Valid packages that have a wasm module, sorted by id.
    wasm: Vec<Arc<Pkg>>,
    /// Every installed package folder, with its error if it is broken.
    installed: HashMap<String, Option<String>>,
}

#[derive(Default)]
pub struct Registry {
    dir: RwLock<Option<PathBuf>>,
    app_version: RwLock<String>,
    index: RwLock<Option<Arc<Index>>>,
    disabled: RwLock<HashSet<String>>,
}

impl Registry {
    pub fn new(dir: Option<PathBuf>, app_version: &str) -> Registry {
        let r = Registry::default();
        r.configure(dir, app_version);
        r
    }

    /// Another handle on the same plugins and switches, for a source that
    /// opens more files later (an image sequence).
    pub fn snapshot(&self) -> Registry {
        Registry {
            dir: RwLock::new(self.dir.read().unwrap().clone()),
            app_version: RwLock::new(self.app_version.read().unwrap().clone()),
            index: RwLock::new(Some(self.index())),
            disabled: RwLock::new(self.disabled.read().unwrap().clone()),
        }
    }

    /// The registry `formats::open` uses.
    pub fn global() -> &'static Registry {
        static GLOBAL: OnceLock<Registry> = OnceLock::new();
        GLOBAL.get_or_init(Registry::default)
    }

    /// Points the registry at a plugin folder and loads the on/off switches.
    pub fn configure(&self, dir: Option<PathBuf>, app_version: &str) {
        let disabled = dir
            .as_ref()
            .and_then(|d| std::fs::read(d.join(DISABLED_FILE)).ok())
            .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
            .unwrap_or_default();
        *self.disabled.write().unwrap() = disabled.into_iter().collect();
        *self.dir.write().unwrap() = dir;
        *self.app_version.write().unwrap() = app_version.to_string();
        self.invalidate();
    }

    /// Forgets the scanned packages and compiled modules (after install,
    /// uninstall or a reload).
    pub fn invalidate(&self) {
        *self.index.write().unwrap() = None;
    }

    /// Sets which packages the user switched off, and remembers it so files
    /// opened before the UI has loaded follow it too.
    pub fn set_disabled(&self, ids: Vec<String>) -> Result<()> {
        let ids: HashSet<String> = ids.into_iter().filter(|i| package::valid_id(i)).collect();
        if *self.disabled.read().unwrap() == ids {
            return Ok(());
        }
        if let Some(dir) = self.dir.read().unwrap().as_ref() {
            let mut list: Vec<&String> = ids.iter().collect();
            list.sort();
            std::fs::create_dir_all(dir)?;
            std::fs::write(dir.join(DISABLED_FILE), serde_json::to_vec(&list)?)?;
        }
        *self.disabled.write().unwrap() = ids;
        Ok(())
    }

    fn enabled(&self, id: &str) -> bool {
        !self.disabled.read().unwrap().contains(id)
    }

    fn index(&self) -> Arc<Index> {
        if let Some(i) = self.index.read().unwrap().as_ref() {
            return i.clone();
        }
        let mut idx = Index::default();
        if let Some(dir) = self.dir.read().unwrap().as_ref() {
            let version = self.app_version.read().unwrap().clone();
            for p in package::scan(dir, &version) {
                idx.installed.insert(p.id.clone(), p.error.clone());
                // A plugin that another installed one replaces (e.g. the old
                // per-product Zeiss plugins next to "zeiss") never runs.
                if p.error.is_some() || p.replaced_by.is_some() {
                    continue;
                }
                let pkg = PathBuf::from(&p.dir);
                if let Ok(m) = package::read_installed(&pkg, &version) {
                    if m.wasm.is_some() {
                        idx.wasm.push(Arc::new(Pkg { manifest: m, dir: pkg, module: OnceLock::new() }));
                    }
                }
            }
        }
        let idx = Arc::new(idx);
        *self.index.write().unwrap() = Some(idx.clone());
        idx
    }

    /// Enabled packages, in id order.
    fn active(&self) -> Vec<Arc<Pkg>> {
        self.index().wasm.iter().filter(|p| self.enabled(&p.manifest.id)).cloned().collect()
    }

    pub fn preview_package(&self, path: &Path) -> Result<Option<PreviewPackage>> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let Some(pkg) = self.active().into_iter().find(|p| p.opens(&ext).is_some()) else {
            return Ok(None);
        };
        let wasm_path = pkg.dir.join(
            package::safe_rel(pkg.manifest.wasm.as_ref().unwrap()).context("无效格式插件路径")?,
        );
        if wasm_path.metadata()?.len() > 16 * 1024 * 1024 {
            bail!("远程预览插件超过 16 MB");
        }
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(pkg.dir.join("plugin.json"))?)?;
        manifest["main"] = "index.js".into();
        manifest["wasm"] = "format.wasm".into();
        Ok(Some(PreviewPackage {
            manifest,
            wasm: std::fs::read(wasm_path)?,
            app_version: self.app_version.read().unwrap().clone(),
        }))
    }

    /// Whether an enabled plugin opens files with this (lower-case) extension.
    pub fn opens(&self, ext: &str) -> bool {
        self.active().iter().any(|p| p.opens(ext).is_some())
    }

    /// Extensions enabled plugins can open, sorted.
    pub fn extensions(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.active().iter().flat_map(|p| p.manifest.formats.iter().flat_map(|f| f.ext.clone())).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Opens image `series` of `path` with the first enabled plugin that
    /// handles `ext`; also returns that plugin's id.
    pub fn open(&self, path: &Path, ext: &str, series: usize) -> Option<Result<(Box<dyn Source>, String)>> {
        let pkg = self.active().into_iter().find(|p| p.opens(ext).is_some())?;
        Some(WasmSource::open(pkg.clone(), path, series).map(|s| (Box::new(s) as Box<dyn Source>, pkg.manifest.id.clone())))
    }

    fn stack_pkg(&self, path: &Path) -> Option<(Arc<Pkg>, String)> {
        self.active().into_iter().find_map(|p| {
            let d = p.stack_decl(path)?.name.clone();
            Some((p, d))
        })
    }

    /// Whether an enabled plugin reads `path` as an acquisition project
    /// file. A file a plugin opens as an image (a Leica LIF) stays an image
    /// when opened; it is a project file only for the folder around it.
    pub fn is_stack_file(&self, path: &Path) -> bool {
        path.is_file() && self.stack_pkg(path).is_some() && !self.opens_as_image(path)
    }

    fn opens_as_image(&self, path: &Path) -> bool {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        self.active().iter().any(|p| p.opens(&ext).is_some())
    }

    /// Project files enabled plugins read for `dir`: in `dir` or one or two
    /// folders above it (programs keep the slices in subfolders), from the
    /// nearest folder that has any. The best first: the plugin's earlier
    /// `stacks` names, then the file whose name begins the opened file's.
    pub fn find_stack_files(&self, dir: &Path, opened: Option<&Path>) -> Vec<PathBuf> {
        let active = self.active();
        if !active.iter().any(|p| !p.manifest.stacks.is_empty()) {
            return vec![];
        }
        let subs: Vec<String> =
            active.iter().flat_map(|p| p.manifest.stacks.iter().flat_map(|d| d.subfolders().map(str::to_string))).collect();
        let opened_name = opened.and_then(|o| o.file_name()).map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        for d in dir.ancestors().take(3) {
            let mut found: Vec<(usize, bool, PathBuf)> = vec![];
            let mut look = |folder: &Path| {
                let Ok(rd) = std::fs::read_dir(folder) else { return };
                for e in rd.filter_map(|e| e.ok()) {
                    let p = e.path();
                    if !p.is_file() {
                        continue;
                    }
                    let Some((fname, name)) = folder_and_name(&p) else { continue };
                    let rank = active.iter().filter_map(|k| k.manifest.stacks.iter().find_map(|s| s.priority(&fname, &name))).min();
                    if let Some(rank) = rank {
                        let stem = name.rsplit_once('.').map_or(name.as_str(), |s| s.0).to_lowercase();
                        let begins = !opened_name.is_empty() && opened_name.starts_with(&stem);
                        found.push((rank, begins, p));
                    }
                }
            };
            look(d);
            for sub in &subs {
                if let Ok(rd) = std::fs::read_dir(d) {
                    for e in rd.filter_map(|e| e.ok()) {
                        if e.file_name().to_string_lossy().eq_ignore_ascii_case(sub) && e.path().is_dir() {
                            look(&e.path());
                        }
                    }
                }
            }
            if !found.is_empty() {
                found.sort_by(|a, b| {
                    a.0.cmp(&b.0)
                        .then(b.1.cmp(&a.1))
                        .then_with(|| crate::formats::sequence::natural_cmp(&a.2.to_string_lossy(), &b.2.to_string_lossy()))
                });
                return found.into_iter().map(|f| f.2).collect();
            }
        }
        vec![]
    }

    /// The best project file for `dir` (see [`Self::find_stack_files`]).
    pub fn find_stack_file(&self, dir: &Path) -> Option<PathBuf> {
        self.find_stack_files(dir, None).into_iter().next()
    }

    /// The stack the first readable project file for `dir` describes, and
    /// that file; `opened` (the folder or a file in it) picks the series.
    pub fn stack_for(&self, dir: &Path, series: usize, opened: Option<&Path>) -> Option<(PathBuf, Result<StackPlan>)> {
        let mut last = None;
        for config in self.find_stack_files(dir, opened) {
            match self.stack(&config, series, opened) {
                Some(Ok(plan)) => return Some((config, Ok(plan))),
                Some(Err(e)) => {
                    eprintln!("{}: {e:#}", config.display());
                    last = Some((config, Err(e)));
                }
                None => {}
            }
        }
        last
    }

    /// Asks the plugin that reads project file `config` for image stack
    /// `series`. When the stack is opened as folder `opened`, or `opened`
    /// is one of its slices, that series is picked instead.
    pub fn stack(&self, config: &Path, series: usize, opened: Option<&Path>) -> Option<Result<StackPlan>> {
        let (pkg, name) = self.stack_pkg(config)?;
        Some(read_stack(self, &pkg, &name, config, series, opened))
    }

    /// What the folder of the opened image `file` adds to it: the whole
    /// acquisition it belongs to (a Leica project next to its exported
    /// TIFFs…), or the plugin that would read that acquisition.
    pub fn companion(&self, file: &Path) -> Option<Companion> {
        let dir = file.parent()?;
        if !file.is_file() || self.is_stack_file(file) {
            return None;
        }
        if let Some((config, Ok(plan))) = self.stack_for(dir, 0, Some(file)) {
            let same = |a: &Path| std::fs::canonicalize(a).ok() == std::fs::canonicalize(file).ok();
            let matched = plan.files.iter().any(|f| same(f));
            let title = plan.series.get(plan.series_index).map(|s| s.name.clone());
            let detail = plan.series.get(plan.series_index).map(|s| s.detail.clone()).unwrap_or_default();
            let plugin = plan.format.clone().unwrap_or_default();
            if let Some((target, series)) = plan.open.clone() {
                if same(&target) {
                    return None;
                }
                let title = title.unwrap_or_else(|| target.file_name().unwrap_or_default().to_string_lossy().into_owned());
                return Some(Companion::Open { plugin, path: target, series, title, detail, matched, files: plan.files });
            }
            if matched && plan.files.len() > 1 {
                let title = title.unwrap_or_else(|| format!("{} 层", plan.files.len()));
                return Some(Companion::Open { plugin, path: config, series: plan.series_index, title, detail, matched, files: plan.files });
            }
            return None;
        }
        self.missing_project(dir, file)
    }

    /// A project file in `dir` that an official plugin would read, when
    /// that plugin is not ready to.
    fn missing_project(&self, dir: &Path, file: &Path) -> Option<Companion> {
        let idx = self.index();
        let mut names: Vec<(String, String)> = vec![];
        let entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        for p in &entries {
            if p.is_file() && p != file {
                names.push((String::new(), p.file_name()?.to_string_lossy().into_owned()));
            } else if p.is_dir() {
                let sub = p.file_name()?.to_string_lossy().into_owned();
                if KNOWN_PROJECTS.iter().any(|k| k.0.iter().any(|f| f.split_once('/').is_some_and(|(d, _)| d.eq_ignore_ascii_case(&sub)))) {
                    for e in std::fs::read_dir(p).ok()?.filter_map(|e| e.ok()) {
                        names.push((sub.clone(), e.file_name().to_string_lossy().into_owned()));
                    }
                }
            }
        }
        names.sort();
        for &(files, id, name) in KNOWN_PROJECTS {
            let decl = package::StackDecl { files: files.iter().map(|f| f.to_string()).collect(), name: String::new() };
            let Some((sub, found)) = names.iter().filter(|(d, n)| !n.starts_with('.') && decl.matches_in(d, n)).min_by_key(|(d, n)| (decl.priority(d, n), d.clone(), n.clone())) else {
                continue;
            };
            let ready = idx.wasm.iter().find(|p| p.manifest.id == id);
            let action = match (ready, idx.installed.get(id)) {
                (Some(p), _) if self.enabled(id) && !p.manifest.stacks.is_empty() => continue,
                (Some(_), _) if !self.enabled(id) => "enable",
                (_, Some(Some(_))) | (Some(_), _) => "update",
                (None, Some(None)) => "update",
                (None, None) => "install",
            };
            let found = if sub.is_empty() { found.clone() } else { format!("{sub}/{found}") };
            return Some(Companion::Plugin { id: id.into(), plugin: name.into(), found, action });
        }
        None
    }

    /// Why a file with this extension cannot be opened, when a plugin could help.
    pub fn missing(&self, ext: &str) -> Option<String> {
        let idx = self.index();
        if let Some(p) = idx.wasm.iter().find(|p| p.opens(ext).is_some()) {
            return Some(format!("「{}」插件已停用，请在「插件」里启用后再打开 .{ext} 文件", p.manifest.name));
        }
        let &(_, id, name) = KNOWN.iter().find(|k| k.0 == ext)?;
        Some(match idx.installed.get(id) {
            Some(Some(err)) => format!("「{name}」插件无法加载：{err}"),
            Some(None) => format!("请把「{name}」插件更新到最新版本，才能打开 .{ext} 文件"),
            None => format!("需要安装「{name}」插件才能打开 .{ext} 文件"),
        })
    }

    /// The plugin a file needs and what to do about it (install, update,
    /// switch on), when it cannot be opened for want of one: what the
    /// one-click install button offers.
    pub fn missing_plugin(&self, path: &Path) -> Option<Companion> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let found = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let idx = self.index();
        if let Some(p) = idx.wasm.iter().find(|p| p.opens(&ext).is_some()) {
            if self.enabled(&p.manifest.id) {
                return None;
            }
            return Some(Companion::Plugin { id: p.manifest.id.clone(), plugin: p.manifest.name.clone(), found, action: "enable" });
        }
        let &(_, id, name) = KNOWN.iter().find(|k| k.0 == ext)?;
        let action = if idx.installed.contains_key(id) { "update" } else { "install" };
        Some(Companion::Plugin { id: id.into(), plugin: name.into(), found, action })
    }

    /// Lets metadata plugins add to what the opener found. Plugin errors
    /// are logged and otherwise ignored: the image itself is fine.
    pub fn add_metadata(&self, path: &Path, ext: &str, src: Box<dyn Source>, skip: Option<&str>) -> Box<dyn Source> {
        let pkgs: Vec<Arc<Pkg>> =
            self.active().into_iter().filter(|p| p.reads_metadata(ext) && Some(p.manifest.id.as_str()) != skip).collect();
        if pkgs.is_empty() {
            return src;
        }
        let mut info = src.info().clone();
        let mut changed = false;
        for p in pkgs {
            match read_metadata(&p, path) {
                Ok(Some(m)) => {
                    m.apply(&mut info);
                    changed = true;
                }
                Ok(None) => {}
                Err(e) => eprintln!("plugin {}: {}: {e:#}", p.manifest.id, path.display()),
            }
        }
        if changed {
            Box::new(Augmented { inner: src, info })
        } else {
            src
        }
    }
}

/// Checks a package before it is installed: the module compiles, has the
/// exports its plugin.json promises and speaks ABI v1.
pub fn validate_package(dir: &Path, m: &Manifest) -> Result<()> {
    let Some(w) = &m.wasm else { return Ok(()) };
    let bytes = std::fs::read(dir.join(package::safe_rel(w).context("wasm 路径不对")?))?;
    let (module, ex) = wasm::compile(&bytes)?;
    if !m.formats.is_empty() && !ex.format {
        bail!("plugin.json 声明了 formats，但 {w} 没有导出 tmv_open / tmv_info / tmv_page / tmv_close");
    }
    if !m.metadata.is_empty() && !ex.metadata {
        bail!("plugin.json 声明了 metadata，但 {w} 没有导出 tmv_metadata");
    }
    if !m.stacks.is_empty() && !ex.stack {
        bail!("plugin.json 声明了 stacks，但 {w} 没有导出 tmv_stack");
    }
    Instance::new(&module, Arc::new(Vec::new()), &m.id)?;
    Ok(())
}

fn map_file(path: &Path) -> Result<Arc<Mmap>> {
    let file = File::open(path).with_context(|| format!("无法打开 {}", path.display()))?;
    // SAFETY: read-only mapping; see TiffSource::open.
    Ok(Arc::new(unsafe { Mmap::map(&file)? }))
}

// ---------------------------------------------------------------- formats

fn one() -> usize {
    1
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InfoJson {
    width: usize,
    height: usize,
    #[serde(default = "one")]
    size_c: usize,
    #[serde(default = "one")]
    size_z: usize,
    #[serde(default = "one")]
    size_t: usize,
    dtype: String,
    #[serde(default)]
    is_rgb: bool,
    #[serde(default = "one")]
    samples_per_page: usize,
    #[serde(default)]
    order: String,
    page_count: Option<usize>,
    #[serde(default)]
    channels: Vec<ChannelJson>,
    pixel_size: Option<Vec<f64>>,
    #[serde(default)]
    unit: String,
    format: Option<String>,
    #[serde(default)]
    metadata: Vec<GroupJson>,
    #[serde(default)]
    series: Vec<SeriesJson>,
    #[serde(default)]
    series_index: usize,
    spectrum: Option<SpectrumJson>,
}

#[derive(Deserialize)]
struct SpectrumJson {
    axis: String,
    #[serde(default)]
    start: f64,
    step: f64,
    #[serde(default)]
    unit: String,
    #[serde(default)]
    kind: String,
}

#[derive(Deserialize)]
struct SeriesJson {
    #[serde(default)]
    name: String,
    #[serde(default)]
    detail: String,
}

#[derive(Deserialize)]
struct ChannelJson {
    #[serde(default)]
    name: String,
    color: Option<[u8; 3]>,
}

#[derive(Deserialize)]
struct GroupJson {
    name: String,
    #[serde(default)]
    items: Vec<[String; 2]>,
}

impl GroupJson {
    fn into_group(self) -> MetaGroup {
        MetaGroup { name: self.name, items: self.items }
    }
}

fn parse_dtype(s: &str) -> Result<DType> {
    Ok(match s {
        "u8" => DType::U8,
        "i8" => DType::I8,
        "u16" => DType::U16,
        "i16" => DType::I16,
        "u32" => DType::U32,
        "i32" => DType::I32,
        "f32" => DType::F32,
        "f64" => DType::F64,
        _ => bail!("插件给出的数据类型不对：{s}"),
    })
}

fn pixel_size(v: Option<Vec<f64>>) -> Option<[f64; 3]> {
    let v = v?;
    let get = |i: usize| v.get(i).copied().filter(|x| x.is_finite() && *x > 0.0).unwrap_or(0.0);
    let x = get(0);
    (x > 0.0).then(|| [x, if get(1) > 0.0 { get(1) } else { x }, get(2)])
}

/// Turns `tmv_info` JSON into an [`ImageInfo`], checking it is consistent.
fn parse_info(json: &[u8], path: &Path, default_format: &str) -> Result<ImageInfo> {
    let j: InfoJson = serde_json::from_slice(json).map_err(|e| anyhow!("插件返回的图像信息不对：{e}"))?;
    if j.width == 0 || j.height == 0 || j.size_c == 0 || j.size_z == 0 || j.size_t == 0 {
        bail!("插件返回的图像尺寸不对");
    }
    let dtype = parse_dtype(&j.dtype)?;
    let spp = j.samples_per_page;
    if spp != 1 && spp != j.size_c {
        bail!("插件返回的 samplesPerPage 必须是 1 或 sizeC");
    }
    let default_pages = if spp > 1 { j.size_z * j.size_t } else { j.size_c * j.size_z * j.size_t };
    let page_count = j.page_count.unwrap_or(default_pages);
    let page_bytes = (j.width as u64)
        .checked_mul(j.height as u64)
        .and_then(|n| n.checked_mul((spp * dtype.size()) as u64))
        .filter(|&n| n <= MAX_PAGE_BYTES);
    if page_bytes.is_none() || page_count == 0 || page_count > 1 << 24 {
        bail!("插件返回的图像太大或没有页面（{}×{}，{page_count} 页）", j.width, j.height);
    }
    let mut info = ImageInfo {
        path: path.to_string_lossy().into_owned(),
        name: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        format: j.format.filter(|f| !f.is_empty()).unwrap_or_else(|| default_format.to_string()),
        width: j.width,
        height: j.height,
        size_c: j.size_c,
        size_z: j.size_z,
        size_t: j.size_t,
        dtype,
        is_rgb: j.is_rgb && dtype == DType::U8 && spp >= 3,
        channels: vec![],
        pixel_size: pixel_size(j.pixel_size),
        unit: j.unit,
        metadata: j.metadata.into_iter().map(GroupJson::into_group).collect(),
        samples_per_page: spp,
        order: DimOrder::parse(&j.order),
        page_count,
        series: vec![],
        series_index: 0,
        folder: None,
        spectrum: j
            .spectrum
            .filter(|s| (s.axis == "x" || s.axis == "z") && s.start.is_finite() && s.step.is_finite() && s.step != 0.0)
            .map(|s| SpectrumInfo { axis: s.axis, start: s.start, step: s.step, unit: s.unit, kind: s.kind }),
    };
    if j.series.len() > 1 && j.series_index < j.series.len() {
        info.series_index = j.series_index;
        info.series = j
            .series
            .into_iter()
            .enumerate()
            .map(|(i, s)| SeriesInfo { name: if s.name.is_empty() { format!("图像 {}", i + 1) } else { s.name }, detail: s.detail })
            .collect();
    }
    if info.pixel_size.is_none() {
        info.unit.clear();
    }
    let n = info.size_c;
    for (i, c) in j.channels.into_iter().take(n).enumerate() {
        let fallback = if n == 1 { [255, 255, 255] } else { crate::image::DEFAULT_COLORS[i % 7] };
        let name = if c.name.is_empty() { format!("C{}", i + 1) } else { c.name };
        info.channels.push(ChannelInfo { name, color: c.color.unwrap_or(fallback) });
    }
    info.fill_default_channels();
    Ok(info)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PyramidJson {
    #[serde(default)]
    series_index: usize,
    #[serde(default)]
    pyramid: Vec<[u64; 3]>,
}

fn is_pyramid(json: &[u8]) -> bool {
    serde_json::from_slice::<PyramidJson>(json).is_ok_and(|j| j.pyramid.len() > 1)
}

/// The smaller levels of a full-resolution image whose plugin lists its
/// series as resolution levels: each narrower than the one before, and the
/// first one this image.
fn pyramid(json: &[u8], info: &ImageInfo) -> Vec<[usize; 3]> {
    let Ok(j) = serde_json::from_slice::<PyramidJson>(json) else { return vec![] };
    let sizes: Vec<[usize; 3]> = j.pyramid.iter().map(|s| s.map(|v| usize::try_from(v).unwrap_or(0))).collect();
    let ok = j.series_index == 0
        && sizes.len() > 1
        && sizes[0] == [info.width, info.height, info.size_z]
        && sizes.iter().all(|s| s[0] > 0 && s[1] > 0 && s[2] > 0)
        && sizes.windows(2).all(|p| p[1][0] < p[0][0] && p[1][1] <= p[0][1] && p[1][2] <= p[0][2]);
    if ok { sizes[1..].to_vec() } else { vec![] }
}

/// A file read through a format plugin. The plugin instance is used only
/// briefly per page (to get the recipe), behind a lock; decoding happens
/// outside it, so prefetch threads still decode in parallel.
pub struct WasmSource {
    file: Arc<Mmap>,
    info: ImageInfo,
    pkg: Arc<Pkg>,
    path: PathBuf,
    /// The image at lower resolutions (width, height, Z), when the plugin
    /// says its series are that (`pyramid`); entry i is series i + 1.
    levels: Vec<[usize; 3]>,
    plugin: Mutex<(Instance, i32)>,
    /// Pages stored as one uncompressed run of the file: (offset, little endian).
    raw: Mutex<HashMap<usize, Option<(usize, bool)>>>,
}

impl WasmSource {
    fn open(pkg: Arc<Pkg>, path: &Path, series: usize) -> Result<WasmSource> {
        let module = pkg.module()?;
        let file = map_file(path)?;
        let mut inst = Instance::new(module, file.clone(), &pkg.manifest.id)?;
        let series = u32::try_from(series).context("图像序号太大")?;
        let handle = inst.open(&path.to_string_lossy(), series)?;
        let json = inst.info(handle)?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let name = pkg.opens(&ext).filter(|n| !n.is_empty()).unwrap_or(&pkg.manifest.name).to_string();
        let mut info = parse_info(&json, path, &name)?;
        let levels = pyramid(&json, &info);
        if !levels.is_empty() || series > 0 && is_pyramid(&json) {
            // Resolution levels are read by the viewer, not picked.
            info.series.clear();
            info.series_index = 0;
        }
        Ok(WasmSource {
            file,
            info,
            pkg,
            path: path.to_path_buf(),
            levels,
            plugin: Mutex::new((inst, handle)),
            raw: Mutex::new(HashMap::new()),
        })
    }

    fn recipe(&self, page: usize) -> Result<recipe::Recipe> {
        let page = page.min(self.info.page_count - 1);
        let bytes = {
            let mut p = self.plugin.lock().unwrap_or_else(|e| e.into_inner());
            let h = p.1;
            p.0.page(h, page as u32)?
        };
        recipe::parse(bytes)
    }
}

impl Source for WasmSource {
    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn levels(&self) -> &[[usize; 3]] {
        &self.levels
    }

    fn open_level(&self, level: usize) -> Option<Result<Box<dyn Source>>> {
        self.levels.get(level.checked_sub(1)?)?;
        Some(WasmSource::open(self.pkg.clone(), &self.path, level).map(|s| Box::new(s) as Box<dyn Source>))
    }

    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        recipe::execute(&self.recipe(page)?, &self.file, &self.info)
    }

    fn raw_page(&self, page: usize) -> Option<(&[u8], bool)> {
        if self.info.samples_per_page != 1 {
            return None;
        }
        let page = page.min(self.info.page_count - 1);
        let n = self.info.page_bytes();
        let cached = self.raw.lock().unwrap().get(&page).copied();
        let found = match cached {
            Some(f) => f,
            None => {
                let f = self.recipe(page).ok().and_then(|r| match r.chunks.as_slice() {
                    [c] if !c.inline
                        && c.codec == recipe::codec::RAW
                        && c.skip == 0
                        && c.samples == 1
                        && c.flags & !recipe::flags::BIG_ENDIAN == 0
                        && c.rect == [0, 0, self.info.width, self.info.height]
                        && (c.stride == 0 || c.stride == self.info.width * self.info.dtype.size())
                        && c.length as usize >= n =>
                    {
                        Some((usize::try_from(c.offset).ok()?, c.flags & recipe::flags::BIG_ENDIAN == 0))
                    }
                    _ => None,
                });
                self.raw.lock().unwrap().insert(page, f);
                f
            }
        };
        let (off, little) = found?;
        Some((self.file.get(off..off.checked_add(n)?)?, little))
    }
}

impl Drop for WasmSource {
    fn drop(&mut self) {
        let p = self.plugin.get_mut().unwrap_or_else(|e| e.into_inner());
        let h = p.1;
        p.0.close(h);
    }
}

// ---------------------------------------------------------------- stacks

/// What a plugin read from an acquisition project file: the slice files of
/// one image stack, in Z order, and what to show about them.
#[derive(Clone)]
pub struct StackPlan {
    pub config: PathBuf,
    pub files: Vec<PathBuf>,
    pub format: Option<String>,
    /// Distance between slices and its unit.
    pub z_spacing: Option<(f64, String)>,
    /// X/Y pixel size the project knows, replacing the slices' own.
    pub pixel_size: Option<([f64; 2], String)>,
    pub metadata: Vec<MetaGroup>,
    pub series: Vec<SeriesInfo>,
    pub series_index: usize,
    /// A file that holds this same image (image `.1` of it), better opened
    /// than the slices: the vendor container next to its exported TIFFs.
    pub open: Option<(PathBuf, usize)>,
    /// The slices of every series, when the plugin lists them (parallel to
    /// `series`): the files exported from each image of a container.
    pub series_files: Vec<Vec<PathBuf>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StackJson {
    #[serde(default)]
    files: Vec<String>,
    format: Option<String>,
    z_spacing: Option<f64>,
    pixel_size: Option<Vec<f64>>,
    open: Option<String>,
    #[serde(default)]
    open_series: usize,
    #[serde(default)]
    unit: String,
    #[serde(default)]
    metadata: Vec<GroupJson>,
    #[serde(default)]
    series: Vec<StackSeriesJson>,
    #[serde(default)]
    series_index: usize,
}

#[derive(Deserialize)]
struct StackSeriesJson {
    #[serde(default)]
    name: String,
    #[serde(default)]
    detail: String,
    /// The folder holding this series' slices, relative to the project file.
    #[serde(default)]
    dir: String,
    /// Its slices, when the plugin lists them for every series.
    #[serde(default)]
    files: Vec<String>,
}

fn read_stack(reg: &Registry, pkg: &Pkg, name: &str, config: &Path, series: usize, opened: Option<&Path>) -> Result<StackPlan> {
    let decl = pkg.stack_decl(config).context("项目文件不对")?;
    let parent = config.parent().context("项目文件没有所在文件夹")?;
    // A project file in a subfolder (`MetaData/*.xml`) describes the folder above.
    let in_sub = folder_and_name(config).is_some_and(|(f, n)| {
        !f.is_empty() && decl.priority(&f, &n).is_some_and(|i| decl.files[i].contains('/'))
    });
    let base = if in_sub { parent.parent().context("项目文件没有上级文件夹")? } else { parent }.to_path_buf();
    let listing = crate::formats::sequence::list_tree(&base);
    let mut inst = Instance::new(pkg.module()?, map_file(config)?, &pkg.manifest.id)?;
    let path = config.to_string_lossy();
    let joined = listing.join("\n");
    let call = |inst: &mut Instance, series: usize| -> Result<StackJson> {
        let json = inst.stack(&path, &joined, u32::try_from(series).context("图像序号太大")?)?;
        serde_json::from_slice(&json).map_err(|e| anyhow!("插件返回的堆叠信息不对：{e}"))
    };
    let mut j = call(&mut inst, series)?;
    // Opened as one of the project's folders, or as one of its slices:
    // show that stack.
    if let (Some(o), 0) = (opened, series) {
        let canon = std::fs::canonicalize(o).ok();
        let same = |rel: &str| !rel.is_empty() && std::fs::canonicalize(base.join(rel)).ok() == canon;
        let pick = if o.is_dir() {
            j.series.iter().position(|s| same(&s.dir))
        } else {
            j.series.iter().position(|s| s.files.iter().any(|f| same(f)))
        };
        if let Some(i) = pick {
            if i != j.series_index {
                j = call(&mut inst, i)?;
            }
        }
    }
    let known: HashSet<&str> = listing.iter().map(String::as_str).collect();
    let mut files = Vec::with_capacity(j.files.len());
    for f in &j.files {
        if !known.contains(f.as_str()) {
            bail!("插件给出的切片文件不在项目文件夹里：{f}");
        }
        let p = base.join(f);
        if !crate::formats::is_supported_with(&p, reg) {
            bail!("插件给出的切片文件打不开：{f}");
        }
        files.push(p);
    }
    let open = match j.open.as_deref().filter(|o| !o.is_empty()) {
        Some(o) if o == "." => Some((config.to_path_buf(), j.open_series)),
        Some(o) if known.contains(o) => Some((base.join(o), j.open_series)),
        Some(o) => bail!("插件给出的图像文件不在项目文件夹里：{o}"),
        None => None,
    };
    if files.is_empty() && open.is_none() {
        bail!("项目里没有找到切片图像");
    }
    let pixel_size = j
        .pixel_size
        .as_deref()
        .and_then(|p| match p {
            [x, y, ..] if x.is_finite() && *x > 0.0 => Some([*x, if y.is_finite() && *y > 0.0 { *y } else { *x }]),
            [x] if x.is_finite() && *x > 0.0 => Some([*x, *x]),
            _ => None,
        })
        .filter(|_| !j.unit.is_empty())
        .map(|p| (p, j.unit.clone()));
    let mut plan = StackPlan {
        config: config.to_path_buf(),
        files,
        format: j.format.filter(|f| !f.is_empty()).or_else(|| (!name.is_empty()).then(|| name.to_string())),
        z_spacing: j.z_spacing.filter(|z| z.is_finite() && *z > 0.0).map(|z| (z, j.unit.clone())),
        pixel_size,
        metadata: j.metadata.into_iter().map(GroupJson::into_group).collect(),
        series: vec![],
        series_index: 0,
        open,
        series_files: vec![],
    };
    if !j.series.is_empty() && j.series_index < j.series.len() {
        plan.series_index = j.series_index;
        plan.series_files = j
            .series
            .iter()
            .map(|s| s.files.iter().filter(|f| known.contains(f.as_str())).map(|f| base.join(f)).collect())
            .collect();
        plan.series = j
            .series
            .into_iter()
            .enumerate()
            .map(|(i, s)| SeriesInfo { name: if s.name.is_empty() { format!("序列 {}", i + 1) } else { s.name }, detail: s.detail })
            .collect();
    }
    Ok(plan)
}

/// The name of the folder holding `path`, and the file name.
fn folder_and_name(path: &Path) -> Option<(String, String)> {
    let name = path.file_name()?.to_str()?.to_string();
    let folder = path.parent().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    Some((folder, name))
}

// ---------------------------------------------------------------- metadata

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetaJson {
    format: Option<String>,
    pixel_size: Option<Vec<f64>>,
    unit: Option<String>,
    #[serde(default)]
    groups: Vec<GroupJson>,
}

impl MetaJson {
    fn apply(self, info: &mut ImageInfo) {
        if let Some(f) = self.format.filter(|f| !f.is_empty()) {
            info.format = f;
        }
        if let (Some(ps), Some(unit)) = (pixel_size(self.pixel_size), self.unit.filter(|u| !u.is_empty())) {
            info.pixel_size = Some(ps);
            info.unit = unit;
        }
        info.metadata.extend(self.groups.into_iter().map(GroupJson::into_group));
    }
}

fn read_metadata(pkg: &Pkg, path: &Path) -> Result<Option<MetaJson>> {
    let mut inst = Instance::new(pkg.module()?, map_file(path)?, &pkg.manifest.id)?;
    let json = inst.metadata(&path.to_string_lossy())?;
    let m: Option<MetaJson> = serde_json::from_slice(&json).map_err(|e| anyhow!("插件返回的元数据不对：{e}"))?;
    Ok(m)
}

/// A source whose info a metadata plugin extended.
struct Augmented {
    inner: Box<dyn Source>,
    info: ImageInfo,
}

impl Source for Augmented {
    fn info(&self) -> &ImageInfo {
        &self.info
    }
    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        self.inner.read_page(page)
    }
    fn read_movie_page(&self, page: usize) -> Option<Result<Vec<Plane>>> {
        self.inner.read_movie_page(page)
    }
    fn raw_page(&self, page: usize) -> Option<(&[u8], bool)> {
        self.inner.raw_page(page)
    }
    fn progress(&self) -> Option<(usize, usize)> {
        self.inner.progress()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_levels_from_the_pyramid_list() {
        let p = Path::new("/x/a.ims");
        let j = br#"{"width":8,"height":6,"sizeZ":4,"dtype":"u16","series":[{"name":"a"},{"name":"b"},{"name":"c"}],"seriesIndex":0,"pyramid":[[8,6,4],[4,3,2],[2,1,1]]}"#;
        let info = parse_info(j, p, "IMS").unwrap();
        assert_eq!(pyramid(j, &info), vec![[4, 3, 2], [2, 1, 1]]);
        assert!(is_pyramid(j));
        // Not this image's size, not shrinking, or another series open: none.
        for bad in [
            &br#"{"width":8,"height":6,"sizeZ":4,"dtype":"u16","seriesIndex":0,"pyramid":[[9,6,4],[4,3,2]]}"#[..],
            br#"{"width":8,"height":6,"sizeZ":4,"dtype":"u16","seriesIndex":0,"pyramid":[[8,6,4],[8,3,2]]}"#,
            br#"{"width":8,"height":6,"sizeZ":4,"dtype":"u16","seriesIndex":1,"pyramid":[[8,6,4],[4,3,2]]}"#,
            br#"{"width":8,"height":6,"sizeZ":4,"dtype":"u16"}"#,
        ] {
            assert!(pyramid(bad, &parse_info(bad, p, "IMS").unwrap()).is_empty());
        }
    }

    #[test]
    fn info_json_is_checked_and_completed() {
        let p = Path::new("/x/a.abc");
        let i = parse_info(br#"{"width":4,"height":3,"dtype":"u16"}"#, p, "ABC").unwrap();
        assert_eq!((i.width, i.height, i.size_c, i.page_count, i.format.as_str()), (4, 3, 1, 1, "ABC"));
        assert_eq!(i.name, "a.abc");
        assert_eq!(i.channels.len(), 1);
        let i = parse_info(
            br#"{"width":4,"height":3,"sizeC":2,"sizeZ":5,"dtype":"f32","order":"ZCT","channels":[{"name":"GFP","color":[1,2,3]}],
                "pixelSize":[0.5,0,2],"unit":"um","format":"F","metadata":[{"name":"g","items":[["k","v"]]}]}"#,
            p,
            "ABC",
        )
        .unwrap();
        assert_eq!(i.page_count, 10);
        assert_eq!(i.pixel_size, Some([0.5, 0.5, 2.0]));
        assert_eq!(i.channels[0].name, "GFP");
        assert_eq!(i.channels[1].name, "C2");
        assert_eq!(i.metadata[0].items[0], ["k".to_string(), "v".to_string()]);
        assert_eq!(i.locate(1, 0, 0).0, 5, "ZCT order");
        assert_eq!(i.spectrum, None);
        let i = parse_info(
            br#"{"width":2048,"height":1,"dtype":"f32","spectrum":{"axis":"x","start":-100,"step":0.5,"unit":"eV","kind":"EELS"}}"#,
            p,
            "ABC",
        )
        .unwrap();
        let sp = i.spectrum.unwrap();
        assert_eq!((sp.axis.as_str(), sp.start, sp.step, sp.unit.as_str(), sp.kind.as_str()), ("x", -100.0, 0.5, "eV", "EELS"));
        // A spectrum axis the viewer cannot plot is dropped, not an error.
        for odd in [r#"{"axis":"y","step":1}"#, r#"{"axis":"z","step":0}"#] {
            let j = format!(r#"{{"width":4,"height":3,"dtype":"u8","spectrum":{odd}}}"#);
            assert_eq!(parse_info(j.as_bytes(), p, "x").unwrap().spectrum, None, "{odd}");
        }
        for bad in [
            &br#"{"width":0,"height":3,"dtype":"u8"}"#[..],
            br#"{"width":4,"height":3,"dtype":"u12"}"#,
            br#"{"width":4,"height":3,"dtype":"u8","sizeC":3,"samplesPerPage":2}"#,
            br#"{"width":4,"height":3,"dtype":"u8","pageCount":0}"#,
            br#"{"width":4000000000,"height":4000000000,"dtype":"f64"}"#,
            b"[]",
        ] {
            assert!(parse_info(bad, p, "x").is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn metadata_json_merges() {
        let mut info = crate::image::tests::fake_info(4, 4, 1, 1, 1, DType::U8);
        info.metadata.push(MetaGroup { name: "core".into(), items: vec![] });
        let m: Option<MetaJson> = serde_json::from_str(
            r#"{"format":"Vendor TIFF","pixelSize":[2.5,2.5,0],"unit":"nm","groups":[{"name":"V","items":[["a","b"]]}]}"#,
        )
        .unwrap();
        m.unwrap().apply(&mut info);
        assert_eq!(info.format, "Vendor TIFF");
        assert_eq!((info.pixel_size, info.unit.as_str()), (Some([2.5, 2.5, 0.0]), "nm"));
        assert_eq!(info.metadata.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["core", "V"]);
        let m: Option<MetaJson> = serde_json::from_str("null").unwrap();
        assert!(m.is_none());
    }

    /// End to end on a real Leica folder: TMV_LEICA_DIR (a LAS X folder
    /// with a LIF and TIFFs exported from it) and TMV_LASX_WASM (the built
    /// plugin). `cargo test leica_folder -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn leica_folder() {
        let (Ok(dir), Ok(wasm)) = (std::env::var("TMV_LEICA_DIR"), std::env::var("TMV_LASX_WASM")) else { return };
        let dir = PathBuf::from(dir);
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("lasx");
        std::fs::create_dir_all(&pkg).unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/lasx");
        for f in ["plugin.json", "index.js"] {
            std::fs::copy(src.join(f), pkg.join(f)).unwrap();
        }
        std::fs::copy(&wasm, pkg.join("lasx.wasm")).unwrap();
        let empty = Registry::new(Some(tmp.path().join("none")), "0.7.334");
        let mut tifs: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "tif")).collect();
        tifs.sort();
        let tif = &tifs[tifs.len() / 2];
        println!("without plugin: {:?}", empty.companion(tif));
        assert!(matches!(empty.companion(tif), Some(Companion::Plugin { action: "install", .. })));
        let reg = Registry::new(Some(tmp.path().to_path_buf()), "0.7.334");
        let c = reg.companion(tif);
        match &c {
            Some(Companion::Open { path, series, title, matched, files, .. }) => {
                println!("open {} series {series} ({title}), matched {matched}, {} files", path.display(), files.len())
            }
            other => panic!("{other:?}"),
        }
        let started = std::time::Instant::now();
        let src = crate::formats::open_with(&dir, &reg).unwrap();
        let i = src.info();
        println!("folder opens as {} {}x{} c{} z{} series {}/{} ({:?})", i.format, i.width, i.height, i.size_c, i.size_z, i.series_index, i.series.len(), started.elapsed());
        // Without the LIF only the exports and MetaData remain.
        let xml = std::fs::read_dir(dir.join("MetaData")).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            n.ends_with(".xml") && !n.contains(".lif_") && !n.contains("roperties")
        });
        if let Some(xml) = xml {
            let plan = reg.stack(&xml, 0, None).unwrap().unwrap();
            println!("MetaData stack: {} files, px {:?}, z {:?}, open {:?}", plan.files.len(), plan.pixel_size, plan.z_spacing, plan.open);
        }
    }

    /// Every image of a real LIF: TMV_LIF (the file) and TMV_LASX_WASM.
    /// `cargo test lif_every_series -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lif_every_series() {
        let (Ok(lif), Ok(wasm)) = (std::env::var("TMV_LIF"), std::env::var("TMV_LASX_WASM")) else { return };
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("lasx");
        std::fs::create_dir_all(&pkg).unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/lasx");
        for f in ["plugin.json", "index.js"] {
            std::fs::copy(src.join(f), pkg.join(f)).unwrap();
        }
        std::fs::copy(&wasm, pkg.join("lasx.wasm")).unwrap();
        let reg = Registry::new(Some(tmp.path().to_path_buf()), "0.7.335");
        let path = PathBuf::from(lif);
        if let Some(Ok(plan)) = reg.stack(&path, 0, None) {
            let exported: Vec<usize> = plan.series_files.iter().map(Vec::len).collect();
            println!("exports per image: {exported:?}");
        }
        let first = crate::formats::open_series_with(&path, 0, &reg).unwrap();
        let series = first.info().series.clone();
        for (i, s) in series.iter().enumerate() {
            match crate::formats::open_series_with(&path, i, &reg) {
                Ok(src) => {
                    let inf = src.info();
                    let page = src.read_page(inf.page_count.saturating_sub(1)).map(|_| "ok").unwrap_or("read failed");
                    println!("{i} {} [{}] -> {}x{} c{} z{} pages {} last page {page}", s.name, s.detail, inf.width, inf.height, inf.size_c, inf.size_z, inf.page_count);
                }
                Err(e) => println!("{i} {} [{}] -> ERROR {e:#}", s.name, s.detail),
            }
        }
    }

    #[test]
    fn missing_plugin_messages_and_switches() {
        let tmp = tempfile::tempdir().unwrap();
        let r = Registry::new(Some(tmp.path().to_path_buf()), "0.4.0");
        assert!(r.missing("czi").unwrap().contains("需要安装「Zeiss」插件"));
        assert!(r.missing("lsm").unwrap().contains("需要安装「Zeiss」插件"));
        assert!(r.missing("xyz").is_none());
        // A JavaScript-only zeiss package.
        std::fs::create_dir_all(tmp.path().join("zeiss")).unwrap();
        std::fs::write(tmp.path().join("zeiss/plugin.json"), r#"{"id":"zeiss","name":"Zeiss","version":"1.0.0"}"#).unwrap();
        std::fs::write(tmp.path().join("zeiss/index.js"), "").unwrap();
        r.invalidate();
        assert!(r.missing("czi").unwrap().contains("更新到最新版本"));

        r.set_disabled(vec!["zeiss".into(), "../bad".into()]).unwrap();
        let again = Registry::new(Some(tmp.path().to_path_buf()), "0.4.0");
        assert!(!again.enabled("zeiss") && again.enabled("nis"));
        assert!(r.extensions().is_empty() && !r.opens("czi"));
    }

    #[test]
    fn replaced_plugins_never_run() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = |id: &str, extra: &str| {
            let d = tmp.path().join(id);
            std::fs::create_dir_all(&d).unwrap();
            let m = format!(r#"{{"id":"{id}","name":"{id}","version":"1.0.0","wasm":"m.wasm"{extra}}}"#);
            std::fs::write(d.join("plugin.json"), m).unwrap();
            std::fs::write(d.join("index.js"), "").unwrap();
            std::fs::write(d.join("m.wasm"), "").unwrap();
        };
        pkg("zen", r#","formats":[{"ext":["czi"]}]"#);
        pkg("smartsem", r#","metadata":{"ext":["tif"]}"#);
        let r = Registry::new(Some(tmp.path().to_path_buf()), "0.6.0");
        assert_eq!(r.extensions(), ["czi"]);
        assert_eq!(r.active().len(), 2);
        pkg("zeiss", r#","formats":[{"ext":["lsm"]}],"replaces":["zen","smartsem"]"#);
        r.invalidate();
        assert_eq!(r.extensions(), ["lsm"]);
        assert_eq!(r.active().iter().map(|p| p.manifest.id.as_str()).collect::<Vec<_>>(), ["zeiss"]);
        let by: Vec<_> = package::scan(tmp.path(), "0.6.0").into_iter().map(|p| (p.id, p.replaced_by)).collect();
        assert_eq!(by, [("smartsem".into(), Some("zeiss".into())), ("zeiss".into(), None), ("zen".into(), Some("zeiss".into()))]);
    }
}
