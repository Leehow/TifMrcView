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
use crate::image::{ChannelInfo, DType, DimOrder, ImageInfo, MetaGroup, Plane, Source};

/// Largest page a plugin may describe (bytes).
const MAX_PAGE_BYTES: u64 = 1 << 34;
/// Where the UI's on/off switches are remembered, inside the plugin folder.
const DISABLED_FILE: &str = ".disabled.json";

/// Vendor formats that used to be built in, so a missing plugin gets a
/// helpful message: (extension, plugin id, plugin name).
const KNOWN: &[(&str, &str, &str)] = &[("czi", "zen", "Zeiss ZEN")];

/// One installed package with a WebAssembly module.
struct Pkg {
    manifest: Manifest,
    dir: PathBuf,
    module: OnceLock<std::result::Result<(Module, Exports), String>>,
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
                if p.error.is_some() {
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

    /// Opens `path` with the first enabled plugin that handles `ext`;
    /// also returns that plugin's id.
    pub fn open(&self, path: &Path, ext: &str) -> Option<Result<(Box<dyn Source>, String)>> {
        let pkg = self.active().into_iter().find(|p| p.opens(ext).is_some())?;
        Some(WasmSource::open(&pkg, path).map(|s| (Box::new(s) as Box<dyn Source>, pkg.manifest.id.clone())))
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
            Some(None) => format!("请把「{name}」插件更新到 2.0 或更新版本，才能打开 .{ext} 文件"),
            None => format!("需要安装「{name}」插件才能打开 .{ext} 文件"),
        })
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
    };
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

/// A file read through a format plugin. The plugin instance is used only
/// briefly per page (to get the recipe), behind a lock; decoding happens
/// outside it, so prefetch threads still decode in parallel.
pub struct WasmSource {
    file: Arc<Mmap>,
    info: ImageInfo,
    plugin: Mutex<(Instance, i32)>,
    /// Pages stored as one uncompressed run of the file: (offset, little endian).
    raw: Mutex<HashMap<usize, Option<(usize, bool)>>>,
}

impl WasmSource {
    fn open(pkg: &Pkg, path: &Path) -> Result<WasmSource> {
        let module = pkg.module()?;
        let file = map_file(path)?;
        let mut inst = Instance::new(module, file.clone(), &pkg.manifest.id)?;
        let handle = inst.open(&path.to_string_lossy())?;
        let json = inst.info(handle)?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let name = pkg.opens(&ext).filter(|n| !n.is_empty()).unwrap_or(&pkg.manifest.name).to_string();
        let info = parse_info(&json, path, &name)?;
        Ok(WasmSource { file, info, plugin: Mutex::new((inst, handle)), raw: Mutex::new(HashMap::new()) })
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

    #[test]
    fn missing_plugin_messages_and_switches() {
        let tmp = tempfile::tempdir().unwrap();
        let r = Registry::new(Some(tmp.path().to_path_buf()), "0.4.0");
        assert!(r.missing("czi").unwrap().contains("需要安装「Zeiss ZEN」插件"));
        assert!(r.missing("xyz").is_none());
        // An old, JavaScript-only zen package.
        std::fs::create_dir_all(tmp.path().join("zen")).unwrap();
        std::fs::write(tmp.path().join("zen/plugin.json"), r#"{"id":"zen","name":"Zeiss ZEN","version":"1.0.0"}"#).unwrap();
        std::fs::write(tmp.path().join("zen/index.js"), "").unwrap();
        r.invalidate();
        assert!(r.missing("czi").unwrap().contains("更新到 2.0"));

        r.set_disabled(vec!["zen".into(), "../bad".into()]).unwrap();
        let again = Registry::new(Some(tmp.path().to_path_buf()), "0.4.0");
        assert!(!again.enabled("zen") && again.enabled("smartsem"));
        assert!(r.extensions().is_empty() && !r.opens("czi"));
    }
}
