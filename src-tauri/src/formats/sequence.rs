//! Image sequences: a folder of slices (FIB-SEM, serial-block-face SEM,
//! array tomography…) shown as one Z stack.
//!
//! The slices can be in any format the app opens; each one is opened only
//! when one of its pages is needed, and a few stay open for the pages next
//! to it. Which files belong to the stack, in which order and how far apart
//! they are comes either from the folder itself (the largest group of files
//! named alike, in natural order) or from an acquisition program's project
//! file, read by a plugin (`Registry::stack`).

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};

use crate::image::{DimOrder, ImageInfo, MetaGroup, Plane, Source};
use crate::plugins::registry::{Registry, StackPlan};

/// Opened slices kept for the pages around the one being read.
const KEEP_OPEN: usize = 16;
/// Folders deeper than this below a project file are not listed for its plugin.
pub const LIST_DEPTH: usize = 4;
/// Most files listed for a plugin.
pub const LIST_MAX: usize = 200_000;

/// Sorts "img2" before "img10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let nb = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let (da, db) = (&a[..na], &b[..nb]);
                let ta = da.iter().skip_while(|&&c| c == b'0').count();
                let tb = db.iter().skip_while(|&&c| c == b'0').count();
                let ord = ta.cmp(&tb).then_with(|| da[na - ta..].cmp(&db[nb - tb..]));
                if ord != Ordering::Equal {
                    return ord;
                }
                a = &a[na..];
                b = &b[nb..];
            }
            (Some(x), Some(y)) => {
                let ord = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

/// A file name with every run of digits replaced by `#`, lower-cased:
/// slices of one acquisition share it ("run - SliceImage - #.tif").
pub fn name_pattern(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut digits = false;
    for c in name.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('#');
            }
            digits = true;
        } else {
            out.extend(c.to_lowercase());
            digits = false;
        }
    }
    out
}

fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// Image files directly in `dir` that the app opens, in natural order.
pub fn images_in(dir: &Path, reg: &Registry) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("无法读取文件夹 {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|n| !hidden(n)))
        .map(|e| e.path())
        .filter(|p| p.is_file() && super::is_supported_with(p, reg))
        .collect();
    files.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    Ok(files)
}

/// The slices of the stack in a folder: the largest group of images named
/// alike. Ties go to the group whose first file sorts first.
pub fn slices_in(dir: &Path, reg: &Registry) -> Result<Vec<PathBuf>> {
    let files = images_in(dir, reg)?;
    let mut groups: Vec<(String, Vec<PathBuf>)> = vec![];
    for f in files {
        let key = name_pattern(&f.file_name().unwrap_or_default().to_string_lossy());
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => g.1.push(f),
            None => groups.push((key, vec![f])),
        }
    }
    let best = groups
        .into_iter()
        .fold(None::<Vec<PathBuf>>, |best, (_, g)| match best {
            Some(b) if b.len() >= g.len() => Some(b),
            _ => Some(g),
        });
    match best {
        Some(g) if g.len() >= 2 => Ok(g),
        Some(_) => bail!("文件夹里只有一张同名规则的图像，组不成堆叠"),
        None => bail!("文件夹里没有能打开的图像"),
    }
}

/// Every file below `dir` (up to [`LIST_DEPTH`] folders down, hidden ones
/// skipped) as `/`-separated paths relative to it, in natural order: what a
/// plugin sees of an acquisition folder.
pub fn list_tree(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, rel: &str, depth: usize, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(base.join(rel)) else {
            return;
        };
        let mut entries: Vec<(String, bool)> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_string();
                let ty = e.file_type().ok()?;
                (!hidden(&name)).then_some((name, ty.is_dir()))
            })
            .collect();
        entries.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        for (name, is_dir) in entries {
            if out.len() >= LIST_MAX {
                return;
            }
            let path = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if is_dir {
                if depth < LIST_DEPTH {
                    walk(base, &path, depth + 1, out);
                }
            } else {
                out.push(path);
            }
        }
    }
    let mut out = vec![];
    walk(dir, "", 0, &mut out);
    out
}

/// Metres per unit, for the units pixel sizes come in.
pub fn unit_metres(unit: &str) -> Option<f64> {
    Some(match unit.trim() {
        "m" => 1.0,
        "mm" => 1e-3,
        "µm" | "μm" | "um" | "micron" | "microns" => 1e-6,
        "nm" => 1e-9,
        "Å" | "A" | "angstrom" => 1e-10,
        "pm" => 1e-12,
        _ => return None,
    })
}

/// A folder of slices (or the files a project file names) as one Z stack.
pub struct Sequence {
    files: Vec<PathBuf>,
    /// Opens the other slices with the plugins the first was opened with.
    reg: Registry,
    /// Pages of one slice file: its sizeC (or 1 with interleaved channels) × sizeZ.
    first: ImageInfo,
    info: ImageInfo,
    open: Mutex<VecDeque<(usize, Arc<dyn Source>)>>,
    /// Slices that could not be opened, so each error is reported once.
    failed: Mutex<HashMap<usize, String>>,
}

impl Sequence {
    /// The stack of the slices in `dir`, or the stack a plugin builds from a
    /// project file in `dir` or the two folders above it.
    pub fn open_dir(dir: &Path, series: usize, reg: &Registry) -> Result<Sequence> {
        // "slices/" and "slices" are the same stack.
        let dir: &Path = &dir.components().collect::<PathBuf>();
        if let Some(config) = reg.find_stack_file(dir) {
            // A project file above the folder only counts when the stack it
            // describes is this folder's (not, say, its log images).
            let here = config.parent() == Some(dir);
            match reg.stack(&config, series, Some(dir)) {
                Some(Ok(plan)) if here || plan.files.iter().all(|f| f.parent() == Some(dir)) => {
                    return Sequence::from_plan(plan, reg)
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => eprintln!("{}: {e:#}", config.display()),
                None => {}
            }
        }
        if series > 0 {
            bail!("这个文件夹只有一个图像序列");
        }
        let files = slices_in(dir, reg)?;
        Sequence::new(files, dir, reg, None)
    }

    /// The stack a plugin describes in a project file.
    pub fn open_project(config: &Path, series: usize, reg: &Registry) -> Result<Sequence> {
        match reg.stack(config, series, None) {
            Some(plan) => Sequence::from_plan(plan?, reg),
            None => bail!("没有插件能读这个项目文件：{}", config.display()),
        }
    }

    fn from_plan(plan: StackPlan, reg: &Registry) -> Result<Sequence> {
        let config = plan.config.clone();
        let mut seq = Sequence::new(plan.files.clone(), &config, reg, Some(&plan))?;
        seq.info.series = plan.series;
        seq.info.series_index = plan.series_index;
        Ok(seq)
    }

    /// `path` is what the stack is opened as (a folder or a project file).
    fn new(
        files: Vec<PathBuf>,
        path: &Path,
        reg: &Registry,
        plan: Option<&StackPlan>,
    ) -> Result<Sequence> {
        let first_path = files.first().context("序列里没有图像")?.clone();
        // The first slice with what metadata plugins add (pixel size,
        // instrument settings); the others only for their pixels.
        let first_src = super::open_with(&first_path, reg)
            .with_context(|| format!("无法打开第一张 {}", first_path.display()))?;
        let first = first_src.info().clone();
        if first.size_t > 1 {
            bail!("{} 有时间维度，不能作为堆叠的一层", first.name);
        }
        let n = files.len();
        let mut info = first.clone();
        info.path = path.to_string_lossy().into_owned();
        info.name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| info.path.clone());
        info.size_z = first.size_z * n;
        info.size_t = 1;
        info.order = DimOrder::CZT;
        info.page_count = if first.samples_per_page > 1 {
            info.size_z
        } else {
            info.size_c * info.size_z
        };
        info.series = vec![];
        info.series_index = 0;
        let kind = plan
            .and_then(|p| p.format.clone())
            .unwrap_or_else(|| "图像序列".to_string());
        info.format = format!("{kind} · {}", first.format);

        let mut group = MetaGroup {
            name: "图像序列".into(),
            items: vec![],
        };
        let mut add = |k: &str, v: String| group.items.push([k.to_string(), v]);
        if let Some(p) = plan {
            add("项目文件", p.config.to_string_lossy().into_owned());
        }
        let dir = first_path
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        info.folder = Some(dir.clone());
        add("文件夹", dir);
        add("文件数", n.to_string());
        if first.size_z > 1 {
            add("每个文件的层数", first.size_z.to_string());
        }
        let file_name = |p: &PathBuf| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        add("第一张", file_name(&files[0]));
        add("最后一张", file_name(&files[n - 1]));

        // Z spacing: from the project file, converted to the slices' unit.
        let mut z_note = "未知（可在「工具」里填写）".to_string();
        if let Some(z) = plan.and_then(|p| p.z_spacing.clone()) {
            let (z, z_unit) = z;
            let slice_unit = info.pixel_size.map(|_| info.unit.clone());
            match (
                slice_unit.as_deref().and_then(unit_metres),
                unit_metres(&z_unit),
            ) {
                (Some(to), Some(from)) => {
                    let ps = info.pixel_size.as_mut().unwrap();
                    ps[2] = tidy(z * from / to);
                    z_note = format!("{} {}（项目文件）", trim_num(ps[2]), info.unit);
                }
                (None, Some(_)) => {
                    // No XY calibration in the slices: keep the Z one alone.
                    z_note = format!("{} {z_unit}（项目文件）", trim_num(z));
                }
                _ => {}
            }
        } else if let Some(ps) = info.pixel_size.filter(|p| p[2] > 0.0 && first.size_z > 1) {
            z_note = format!("{} {}（文件）", trim_num(ps[2]), info.unit);
        } else if let Some(ps) = info.pixel_size.as_mut() {
            ps[2] = 0.0;
        }
        add("Z 间距", z_note);
        let mut metadata = vec![group];
        if let Some(p) = plan {
            metadata.extend(p.metadata.iter().cloned());
        }
        metadata.extend(first.metadata.iter().cloned());
        info.metadata = metadata;

        let open: VecDeque<(usize, Arc<dyn Source>)> = VecDeque::from([(0, Arc::from(first_src))]);
        Ok(Sequence {
            files,
            reg: reg.snapshot(),
            first,
            info,
            open: Mutex::new(open),
            failed: Mutex::new(HashMap::new()),
        })
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    fn slice(&self, i: usize) -> Result<Arc<dyn Source>> {
        if let Some((_, s)) = self.open.lock().unwrap().iter().find(|(j, _)| *j == i) {
            return Ok(s.clone());
        }
        if let Some(e) = self.failed.lock().unwrap().get(&i) {
            bail!("{e}");
        }
        let path = &self.files[i];
        let opened = super::open_pixels_with(path, 0, &self.reg)
            .map(|(s, _)| s)
            .and_then(|s| {
                let a = s.info();
                let b = &self.first;
                if (
                    a.width,
                    a.height,
                    a.dtype,
                    a.size_c,
                    a.samples_per_page,
                    a.size_z,
                ) != (
                    b.width,
                    b.height,
                    b.dtype,
                    b.size_c,
                    b.samples_per_page,
                    b.size_z,
                ) {
                    bail!(
                        "{} 是 {}×{} {}，和第一张（{}×{} {}）不一样，不能放进同一个堆叠",
                        a.name,
                        a.width,
                        a.height,
                        dtype_name(a.dtype),
                        b.width,
                        b.height,
                        dtype_name(b.dtype)
                    );
                }
                Ok(Arc::<dyn Source>::from(s))
            });
        let src = match opened {
            Ok(s) => s,
            Err(e) => {
                let msg = format!(
                    "第 {} 层（{}）：{e:#}",
                    i + 1,
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
                self.failed.lock().unwrap().insert(i, msg.clone());
                bail!("{msg}");
            }
        };
        let mut open = self.open.lock().unwrap();
        if !open.iter().any(|(j, _)| *j == i) {
            if open.len() >= KEEP_OPEN {
                // Slot 0 holds the first slice, whose metadata the stack shows.
                open.remove(1);
            }
            open.push_back((i, src.clone()));
        }
        Ok(src)
    }
}

/// Rounds away the noise of a unit conversion (10 significant digits).
fn tidy(v: f64) -> f64 {
    format!("{v:.9e}").parse().unwrap_or(v)
}

fn dtype_name(d: crate::image::DType) -> String {
    format!("{d:?}").to_lowercase()
}

fn trim_num(v: f64) -> String {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

impl Source for Sequence {
    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn read_page(&self, page: usize) -> Result<Vec<Plane>> {
        let page = page.min(self.info.page_count - 1);
        let (c, z) = if self.info.samples_per_page > 1 {
            (0, page)
        } else {
            (page % self.info.size_c, page / self.info.size_c)
        };
        let per_file = self.first.size_z;
        let (file, inner_z) = (z / per_file, z % per_file);
        let src = self.slice(file)?;
        let (inner, _) = src.info().locate(c, inner_z, 0);
        src.read_page(inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tiff::encoder::{colortype, TiffEncoder};

    fn write_tif(path: &Path, w: u32, h: u32, value: u16) {
        let mut enc = TiffEncoder::new(File::create(path).unwrap()).unwrap();
        let px = vec![value; (w * h) as usize];
        enc.write_image::<colortype::Gray16>(w, h, &px).unwrap();
    }

    fn reg() -> Registry {
        Registry::new(None, "0.7.0")
    }

    #[test]
    fn patterns_group_slices() {
        assert_eq!(
            name_pattern("Run - SliceImage - 012.tif"),
            "run - sliceimage - #.tif"
        );
        assert_eq!(
            name_pattern("2020-08-04_17_04_11_1_0_mode_0.tif"),
            "#-#-#_#_#_#_#_#_mode_#.tif"
        );
        assert_eq!(natural_cmp("s9.tif", "s10.tif"), Ordering::Less);
    }

    #[test]
    fn folder_becomes_a_stack_of_its_largest_group() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        for i in [1, 2, 10] {
            write_tif(&d.join(format!("slice_{i}.tif")), 4, 3, i as u16);
        }
        // An overview image and a hidden file stay out of the stack.
        write_tif(&d.join("overview.tif"), 8, 8, 99);
        write_tif(&d.join("._slice_3.tif"), 4, 3, 3);
        std::fs::write(d.join("notes.txt"), "x").unwrap();
        let s = Sequence::open_dir(d, 0, &reg()).unwrap();
        let i = s.info();
        assert_eq!((i.width, i.height, i.size_z, i.page_count), (4, 3, 3, 3));
        assert!(i.format.starts_with("图像序列"), "{}", i.format);
        let v: Vec<f64> = (0..3)
            .map(|p| s.read_page(p).unwrap()[0].data.get_f64(0))
            .collect();
        assert_eq!(v, [1.0, 2.0, 10.0], "natural order");
        assert_eq!(i.metadata[0].name, "图像序列");
        assert!(Sequence::open_dir(d, 1, &reg()).is_err());
    }

    #[test]
    fn mismatched_slices_fail_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write_tif(&d.join("s1.tif"), 4, 3, 1);
        write_tif(&d.join("s2.tif"), 5, 3, 2);
        write_tif(&d.join("s3.tif"), 4, 3, 3);
        let s = Sequence::open_dir(d, 0, &reg()).unwrap();
        assert!(s.read_page(0).is_ok());
        let e = format!("{:#}", s.read_page(1).err().unwrap());
        assert!(e.contains("第 2 层") && e.contains("不一样"), "{e}");
        assert!(s.read_page(1).is_err(), "remembered");
        assert_eq!(s.read_page(2).unwrap()[0].data.get_f64(0), 3.0);
    }

    #[test]
    fn single_or_no_images_are_not_a_stack() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(format!(
            "{:#}",
            Sequence::open_dir(tmp.path(), 0, &reg()).err().unwrap()
        )
        .contains("没有能打开的图像"));
        write_tif(&tmp.path().join("a.tif"), 2, 2, 1);
        assert!(Sequence::open_dir(tmp.path(), 0, &reg()).is_err());
    }

    #[test]
    fn many_slices_keep_few_open() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..40 {
            write_tif(&tmp.path().join(format!("z{i:03}.tif")), 2, 2, i);
        }
        let s = Sequence::open_dir(tmp.path(), 0, &reg()).unwrap();
        for p in (0..40).rev() {
            assert_eq!(s.read_page(p).unwrap()[0].data.get_f64(0), p as f64);
        }
        assert!(s.open.lock().unwrap().len() <= KEEP_OPEN);
        assert_eq!(s.open.lock().unwrap()[0].0, 0);
    }

    #[test]
    fn tree_listing_is_relative_and_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::create_dir_all(d.join("a/b/c/d/e")).unwrap();
        std::fs::create_dir_all(d.join(".hidden")).unwrap();
        for f in [
            "top.dat",
            "a/x2.tif",
            "a/x10.tif",
            "a/b/c/d/deep.tif",
            "a/b/c/d/e/too-deep.tif",
            ".hidden/h.tif",
        ] {
            std::fs::write(d.join(f), "").unwrap();
        }
        assert_eq!(
            list_tree(d),
            ["a/b/c/d/deep.tif", "a/x2.tif", "a/x10.tif", "top.dat"]
        );
    }
}
