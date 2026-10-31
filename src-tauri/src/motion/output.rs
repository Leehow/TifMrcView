//! Float32 MRC products. Create-new staging protects existing temporary
//! files; owned staging is removed on cancellation/errors before rename.
use super::check_cancel;
use anyhow::{bail, Result};
use std::{
    fs::{self, OpenOptions},
    io::{BufWriter, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
pub fn sibling(path: &str, suffix: &str) -> String {
    let p = Path::new(path);
    p.with_file_name(format!(
        "{}_{suffix}.mrc",
        p.file_stem().unwrap_or_default().to_string_lossy()
    ))
    .to_string_lossy()
    .into()
}
pub fn write(
    path: &str,
    w: usize,
    h: usize,
    n: usize,
    px: f64,
    mut plane: impl FnMut(usize) -> Result<Vec<f32>>,
) -> Result<()> {
    if w == 0 || h == 0 || n == 0 {
        bail!("空电影输出");
    }
    let part = PathBuf::from(format!("{path}.motion-part"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)?;
    let temp = Temp(part.clone());
    let mut f = BufWriter::new(file);
    f.write_all(&[0u8; 1024])?;
    let (mut lo, mut hi, mut sum, mut sum2) = (f64::INFINITY, f64::NEG_INFINITY, 0.0, 0.0);
    for i in 0..n {
        check_cancel()?;
        let v = plane(i)?;
        if v.len() != w * h || v.iter().any(|x| !x.is_finite()) {
            bail!("电影输出尺寸或值无效");
        }
        for x in &v {
            let x = *x as f64;
            lo = lo.min(x);
            hi = hi.max(x);
            sum += x;
            sum2 += x * x;
        }
        let bytes: Vec<u8> = v.iter().flat_map(|v| v.to_le_bytes()).collect();
        f.write_all(&bytes)?;
    }
    let count = (w * h * n) as f64;
    let mean = sum / count;
    let sd = (sum2 / count - mean * mean).max(0.0).sqrt();
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&crate::align::mrc_header(
        w,
        h,
        n,
        px,
        [lo as f32, hi as f32, mean as f32, sd as f32],
        "TifMrcView Pro: native motion correction",
    ))?;
    f.flush()?;
    f.get_ref().sync_all()?;
    drop(f);
    check_cancel()?;
    fs::rename(&part, path)?;
    drop(temp);
    Ok(())
}
pub fn single(path: &str, v: &[f32], w: usize, h: usize, px: f64) -> Result<()> {
    write(path, w, h, 1, px, |_| Ok(v.to_vec()))
}
pub fn protect(input: &str, output: &str) -> Result<()> {
    if Path::new(input)
        .canonicalize()
        .ok()
        .zip(Path::new(output).canonicalize().ok())
        .is_some_and(|(a, b)| a == b)
    {
        bail!("不能覆盖输入电影");
    }
    Ok(())
}
pub fn read_plane(path: &str) -> Result<Vec<f32>> {
    let img = crate::image::OpenImage::new(0, crate::formats::open(Path::new(path))?, 64 << 20);
    let p = img.page(0)?;
    Ok((0..p[0].w * p[0].h)
        .map(|i| p[0].data.get_f64(i) as f32)
        .collect())
}
pub fn text(path: &Path, text: &str) -> Result<()> {
    let part = path.with_extension("motion-text-part");
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)?;
    let temp = Temp(part.clone());
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    check_cancel()?;
    fs::rename(&part, path)?;
    drop(temp);
    Ok(())
}

/// Diagnostic pairs share their intensity/log-power scales. PNGs never
/// substitute for an alignment or a scientific-resolution acceptance gate.
pub fn diagnostics(target: &str, before: &[f32], after: &[f32], w: usize, h: usize) -> Result<()> {
    let n = w.min(h).min(1024);
    let (x0, y0) = ((w - n) / 2, (h - n) / 2);
    let crop = |v: &[f32]| {
        (y0..y0 + n)
            .flat_map(|y| v[y * w + x0..y * w + x0 + n].iter().copied())
            .collect::<Vec<_>>()
    };
    let a = crop(before);
    let b = crop(after);
    png_pair(target, "before", "after", &a, &b, n, n)?;
    let side = (1usize << n.ilog2()).min(512);
    let ps = |v: &[f32]| crate::fft::spectrum(|x, y| v[y * n + x], [0, 0, n, n], side);
    let pa = ps(&a)?;
    let pb = ps(&b)?;
    png_pair(
        target,
        "before-ps",
        "after-ps",
        &pa.log,
        &pb.log,
        pa.n,
        pa.n,
    )?;
    let table = pa
        .radial
        .iter()
        .zip(pb.radial)
        .enumerate()
        .map(|(i, (a, b))| format!("{i}\t{a}\t{b}\n"))
        .collect::<String>();
    text(
        Path::new(&format!("{target}.radial.tsv")),
        &format!("frequency_bin\tuncorrected_log10_power\tcorrected_log10_power\n{table}"),
    )
}
fn png_pair(
    target: &str,
    an: &str,
    bn: &str,
    a: &[f32],
    b: &[f32],
    w: usize,
    h: usize,
) -> Result<()> {
    let mut values: Vec<f32> = a.iter().chain(b).copied().collect();
    values.sort_by(f32::total_cmp);
    let lo = values[values.len() / 100];
    let hi = values[values.len() * 99 / 100];
    for (name, v) in [(an, a), (bn, b)] {
        let bytes = v
            .iter()
            .map(|v| ((v - lo) / (hi - lo).max(1e-20) * 255.0).clamp(0.0, 255.0) as u8)
            .collect();
        image::GrayImage::from_raw(w as u32, h as u32, bytes)
            .ok_or_else(|| anyhow::anyhow!("PNG 尺寸无效"))?
            .save(format!("{target}.{name}.png"))?;
    }
    Ok(())
}
