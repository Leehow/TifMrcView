//! Reader for IMOD binary model files (.mod / .fid), enough to overlay
//! objects, contours and points on the image.
//!
//! Format reference: IMOD `binspec.html`. All values are big-endian.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub path: String,
    pub name: String,
    /// Image size the model was made on, used to spot a binning mismatch.
    pub size: [i32; 3],
    pub objects: Vec<Object>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Object {
    pub name: String,
    pub color: [u8; 3],
    /// Contours are open lines rather than closed outlines.
    pub open: bool,
    /// Scattered points: draw each point as a sphere, no connecting lines.
    pub scattered: bool,
    pub hidden: bool,
    /// Sphere radius for points, in pixels.
    pub point_size: f32,
    pub line_width: u8,
    pub contours: Vec<Contour>,
}

#[derive(Serialize)]
pub struct Contour {
    /// Flattened x, y, z triples in image pixel coordinates.
    pub pts: Vec<f32>,
    /// Per-point sphere sizes, when the model stores them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Vec<f32>>,
}

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.p + n > self.b.len() {
            bail!("模型文件被截断");
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn id(&mut self) -> Result<[u8; 4]> {
        Ok(self.take(4)?.try_into().unwrap())
    }
    fn text(&mut self, n: usize) -> Result<String> {
        let s = self.take(n)?;
        let end = s.iter().position(|&c| c == 0).unwrap_or(n);
        Ok(String::from_utf8_lossy(&s[..end]).trim().to_string())
    }
}

const OBJ_OFF: u32 = 1 << 1;
const OBJ_OPEN: u32 = 1 << 3;
const OBJ_SCAT: u32 = 1 << 9;

pub fn load(path: &Path) -> Result<Model> {
    let bytes = std::fs::read(path).with_context(|| format!("无法读取 {}", path.display()))?;
    let mut r = Rd { b: &bytes, p: 0 };
    if r.take(4)? != b"IMOD" {
        bail!("不是 IMOD 模型文件");
    }
    r.take(4)?; // version, e.g. "V1.2"
    let name = r.text(128)?;
    let size = [r.i32()?, r.i32()?, r.i32()?];
    // Rest of the fixed header: objsize .. alpha/beta/gamma.
    r.take(232 - 128 - 12)?;

    let mut objects: Vec<Object> = Vec::new();
    // Keep whatever parsed before an unknown or damaged chunk.
    let _ = read_chunks(&mut r, &mut objects);
    Ok(Model {
        path: path.to_string_lossy().into_owned(),
        name: if name.is_empty() {
            path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            name
        },
        size,
        objects,
    })
}

fn read_chunks(r: &mut Rd<'_>, objects: &mut Vec<Object>) -> Result<()> {
    loop {
        let Ok(id) = r.id() else { break };
        match &id {
            b"IEOF" => break,
            b"OBJT" => {
                let name = r.text(64)?;
                r.take(64)?; // extra
                let _contsize = r.i32()?;
                let flags = r.u32()?;
                let _axis = r.i32()?;
                let _drawmode = r.i32()?;
                let rgb = [r.f32()?, r.f32()?, r.f32()?];
                let pdrawsize = r.i32()?;
                let sym = r.take(8)?; // symbol, symsize, linewidth2, linewidth, linesty, symflags, sympad, trans
                let _meshsize = r.i32()?;
                let _surfsize = r.i32()?;
                objects.push(Object {
                    name,
                    color: rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
                    open: flags & OBJ_OPEN != 0,
                    scattered: flags & OBJ_SCAT != 0,
                    hidden: flags & OBJ_OFF != 0,
                    point_size: pdrawsize.max(0) as f32,
                    line_width: sym[3].max(1),
                    contours: vec![],
                });
            }
            b"CONT" => {
                let n = r.i32()?.max(0) as usize;
                r.take(12)?; // flags, time, surf
                let mut pts = Vec::with_capacity(n * 3);
                for _ in 0..n * 3 {
                    pts.push(r.f32()?);
                }
                if let Some(o) = objects.last_mut() {
                    o.contours.push(Contour { pts, sizes: None });
                }
            }
            b"MESH" => {
                let vsize = r.i32()?.max(0) as usize;
                let lsize = r.i32()?.max(0) as usize;
                r.take(8)?; // flag, time, surf
                r.take(vsize * 12 + lsize * 4)?;
            }
            b"SIZE" => {
                let n = r.i32()?.max(0) as usize / 4;
                let mut s = Vec::with_capacity(n);
                for _ in 0..n {
                    s.push(r.f32()?);
                }
                if let Some(c) = objects.last_mut().and_then(|o| o.contours.last_mut()) {
                    c.sizes = Some(s);
                }
            }
            _ => {
                // Every other chunk is an id followed by its byte count.
                let n = r.i32()?;
                if n < 0 {
                    bail!("模型文件格式无法识别");
                }
                r.take(n as usize)?;
            }
        }
    }
    Ok(())
}

/// Model files that probably belong to an image: same stem with an IMOD
/// model extension, e.g. `tomo.rec` → `tomo.mod`, `tilt.st` → `tilt.fid`.
pub fn find_for(image: &Path) -> Vec<PathBuf> {
    let Some(dir) = image.parent() else { return vec![] };
    let name = image.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut stems = vec![];
    if let Some(i) = name.find('.') {
        stems.push(name[..i].to_string());
    }
    if let Some(i) = name.rfind('.') {
        stems.push(name[..i].to_string());
    }
    stems.dedup();
    let mut out = vec![];
    for stem in stems {
        for ext in ["mod", "fid", "imod", "seed", "3dmod"] {
            let p = dir.join(format!("{stem}.{ext}"));
            if p.is_file() && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// Set TIFMRCVIEW_TEST_MOD to a model file to run.
    #[test]
    fn reads_model_from_env() {
        let Ok(p) = std::env::var("TIFMRCVIEW_TEST_MOD") else { return };
        let m = super::load(p.as_ref()).unwrap();
        println!("{} objects, size {:?}", m.objects.len(), m.size);
        for o in &m.objects {
            let n: usize = o.contours.iter().map(|c| c.pts.len() / 3).sum();
            println!("  {} color {:?} open {} scat {} contours {} points {} first {:?}",
                o.name, o.color, o.open, o.scattered, o.contours.len(), n,
                o.contours.first().map(|c| &c.pts[..3.min(c.pts.len())]));
        }
        assert!(!m.objects.is_empty());
    }
}
