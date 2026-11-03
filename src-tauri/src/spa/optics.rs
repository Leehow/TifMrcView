//! Optics for a single-particle folder, read from the files beside the
//! micrographs. SerialEM `.mdoc`, Thermo/EPU `.xml` and a RELION optics
//! `.star` carry pixel size, voltage, Cs, amplitude contrast and dose.
//! The image header still wins for pixel size when it has one. Anything
//! these files do not say is left empty so the workspace can ask.

use std::path::{Path, PathBuf};

use super::project::DIR;

/// What the folder's config files actually contained.
#[derive(Clone, Debug, Default)]
pub struct FolderOptics {
    pub pixel_a: Option<f64>,
    pub kv: Option<f64>,
    pub cs_mm: Option<f64>,
    pub amplitude: Option<f64>,
    pub dose_total: Option<f64>,
    /// Basename of the file the values came from.
    pub file: String,
}

#[derive(Clone, Debug, Default)]
struct Hit {
    pixel_a: Option<f64>,
    kv: Option<f64>,
    cs_mm: Option<f64>,
    amplitude: Option<f64>,
    dose_total: Option<f64>,
}

fn num(s: &str) -> Option<f64> {
    s.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn as_kv(v: f64) -> Option<f64> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    let kv = if v > 1000.0 { v / 1000.0 } else { v };
    (20.0..500.0).contains(&kv).then_some(kv)
}

fn as_angstrom(v: f64) -> Option<f64> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    // Metres, the unit EPU writes (1 Å = 1e-10 m). Larger than 1e-8 m is not
    // an image pixel (a sensor pitch is micrometres and is not what we want).
    let a = if v < 1e-8 { v * 1e10 } else { v };
    (0.05..50.0).contains(&a).then_some(a)
}

fn as_cs_mm(v: f64) -> Option<f64> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    let mm = if v < 0.05 { v * 1000.0 } else { v };
    (0.2..10.0).contains(&mm).then_some(mm)
}

fn as_amplitude(v: f64) -> Option<f64> {
    (v.is_finite() && (0.0..1.0).contains(&v)).then_some(v)
}

fn as_dose(v: f64) -> Option<f64> {
    (v.is_finite() && (0.0..500.0).contains(&v) && v > 0.0).then_some(v)
}

fn fill(dst: &mut Hit, src: Hit) -> bool {
    let mut took = false;
    let mut keep = |slot: &mut Option<f64>, v: Option<f64>| {
        if slot.is_none() && v.is_some() {
            *slot = v;
            took = true;
        }
    };
    keep(&mut dst.pixel_a, src.pixel_a);
    keep(&mut dst.kv, src.kv);
    keep(&mut dst.cs_mm, src.cs_mm);
    keep(&mut dst.amplitude, src.amplitude);
    keep(&mut dst.dose_total, src.dose_total);
    took
}

fn done(h: &Hit) -> bool {
    h.pixel_a.is_some()
        && h.kv.is_some()
        && h.cs_mm.is_some()
        && h.amplitude.is_some()
        && h.dose_total.is_some()
}

/// SerialEM mdoc. Voltage is kV, PixelSpacing is Å of `ImageSize` (rescaled
/// when the frames are a different size), ExposureDose is e⁻/Å² of that movie.
fn from_mdoc(text: &str, frame: (usize, usize)) -> Hit {
    let mut pixel = None;
    let mut kv = None;
    let mut size = None;
    let mut dose = None;
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = true;
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if !in_section {
            match k {
                "PixelSpacing" => pixel = pixel.or(num(v).filter(|v| *v > 0.0)),
                "Voltage" => kv = kv.or(num(v).and_then(as_kv)),
                "ImageSize" => {
                    let n: Vec<usize> = v
                        .split_whitespace()
                        .filter_map(|x| x.parse().ok())
                        .collect();
                    if n.len() == 2 && n[0] > 0 && n[1] > 0 {
                        size = Some([n[0], n[1]]);
                    }
                }
                _ => {}
            }
        } else if k == "PixelSpacing" {
            pixel = pixel.or(num(v).filter(|v| *v > 0.0));
        } else if k == "ExposureDose" {
            dose = dose.or(num(v).and_then(as_dose));
        }
    }
    let pixel_a = pixel
        .map(|spacing| {
            if frame.0 == 0 || frame.1 == 0 {
                return spacing;
            }
            let doc = crate::motion::series::Doc {
                image_size: size,
                ..Default::default()
            };
            doc.frame_pixel(spacing, frame)
        })
        .and_then(as_angstrom);
    Hit {
        pixel_a,
        kv,
        dose_total: dose,
        ..Default::default()
    }
}

fn tag_number(text: &str, tag: &str) -> Option<f64> {
    let open = format!("<{tag}");
    let mut rest = text;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        let boundary = after.chars().next();
        if !matches!(boundary, Some('>' | '/' | ' ' | '\n' | '\r' | '\t')) {
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        if after[..gt].trim_end().ends_with('/') {
            rest = &after[gt + 1..];
            continue;
        }
        let body = &after[gt + 1..];
        let end = body.find('<').unwrap_or(body.len());
        if let Some(n) = num(body[..end].trim()) {
            return Some(n);
        }
        rest = body;
    }
    None
}

fn item_number(text: &str, name: &str) -> Option<f64> {
    let key = format!("name=\"{name}\"");
    let i = text.find(&key)?;
    let window = &text[i..(i + 500).min(text.len())];
    tag_number(window, "value").or_else(|| {
        let gt = window.find('>')?;
        let body = &window[gt + 1..];
        let end = body.find('<').unwrap_or(body.len());
        num(body[..end].trim())
    })
}

fn xml_pixel(text: &str) -> Option<f64> {
    for name in ["sensorPixelSize.width", "pixelSize.width"] {
        if let Some(a) = item_number(text, name).and_then(as_angstrom) {
            return Some(a);
        }
    }
    let mut rest = text;
    while let Some(i) = rest.to_ascii_lowercase().find("pixelsize") {
        let window = &rest[i..(i + 800).min(rest.len())];
        if let Some(a) = tag_number(window, "numericValue")
            .or_else(|| tag_number(window, "value"))
            .and_then(as_angstrom)
        {
            return Some(a);
        }
        rest = &rest[i + 9..];
    }
    None
}

/// Thermo / EPU sidecar. highTension and AccelerationVoltage are volts when
/// they are large; pixel sizes are metres.
fn from_xml(text: &str) -> Hit {
    let kv = tag_number(text, "highTension")
        .or_else(|| tag_number(text, "AccelerationVoltage"))
        .or_else(|| item_number(text, "AccelerationVoltage"))
        .or_else(|| item_number(text, "highTension"))
        .and_then(as_kv);
    Hit {
        pixel_a: xml_pixel(text),
        kv,
        cs_mm: tag_number(text, "SphericalAberration")
            .or_else(|| item_number(text, "SphericalAberration"))
            .and_then(as_cs_mm),
        amplitude: tag_number(text, "AmplitudeContrast")
            .or_else(|| item_number(text, "AmplitudeContrast"))
            .and_then(as_amplitude),
        dose_total: tag_number(text, "Dose")
            .or_else(|| item_number(text, "Dose"))
            .and_then(as_dose),
    }
}

fn apply_star(hit: &mut Hit, key: &str, v: f64) {
    match key {
        "_rlnImagePixelSize" | "_rlnMicrographPixelSize" | "_rlnMicrographOriginalPixelSize" => {
            hit.pixel_a = hit.pixel_a.or(as_angstrom(v));
        }
        "_rlnVoltage" => hit.kv = hit.kv.or(as_kv(v)),
        "_rlnSphericalAberration" => hit.cs_mm = hit.cs_mm.or(as_cs_mm(v)),
        "_rlnAmplitudeContrast" => hit.amplitude = hit.amplitude.or(as_amplitude(v)),
        _ => {}
    }
}

/// RELION optics group, either `loop_` columns or `_rlnKey value` lines.
fn from_star(text: &str) -> Hit {
    let mut hit = Hit::default();
    let mut labels: Vec<String> = vec![];
    let mut in_loop = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("data_") {
            in_loop = false;
            labels.clear();
            continue;
        }
        if lower == "loop_" {
            in_loop = true;
            labels.clear();
            continue;
        }
        if let Some(key) = line
            .split_whitespace()
            .next()
            .filter(|s| s.starts_with("_rln"))
        {
            let value = line
                .split_whitespace()
                .nth(1)
                .filter(|s| !s.starts_with('#'));
            if let Some(v) = value.and_then(num) {
                apply_star(&mut hit, key, v);
                in_loop = false;
                labels.clear();
            } else if in_loop {
                labels.push(key.to_string());
            }
            continue;
        }
        if in_loop && !labels.is_empty() {
            let cols: Vec<&str> = line.split_whitespace().collect();
            for (i, key) in labels.iter().enumerate() {
                if let Some(v) = cols.get(i).and_then(|s| num(s)) {
                    apply_star(&mut hit, key, v);
                }
            }
            break;
        }
    }
    hit
}

fn with_ext(dir: &Path, ext: &str, limit: usize) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
        .collect();
    v.sort();
    v.truncate(limit);
    v
}

const DATA_EXT: [&str; 6] = ["mrc", "mrcs", "tif", "tiff", "eer", "dm4"];

fn basenames(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    rd.filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| DATA_EXT.contains(&e.to_ascii_lowercase().as_str()))
        })
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .take(400)
        .collect()
}

fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 2 << 20 {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn mentions(path: &Path, names: &[String]) -> bool {
    if names.is_empty() {
        return false;
    }
    read_text(path).is_some_and(|t| {
        names
            .iter()
            .any(|n| !n.is_empty() && t.contains(n.as_str()))
    })
}

fn candidates(folder: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for ext in ["mdoc", "xml", "star"] {
        out.extend(with_ext(folder, ext, 48));
    }
    for sub in ["mdoc", "mdocfolder"] {
        out.extend(with_ext(&folder.join(sub), "mdoc", 48));
    }
    let names = basenames(folder);
    if let Some(parent) = folder
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n != DIR))
    {
        let mut extra = with_ext(parent, "mdoc", 48);
        extra.extend(with_ext(&parent.join("mdoc"), "mdoc", 48));
        extra.extend(with_ext(&parent.join("mdocfolder"), "mdoc", 48));
        for p in extra {
            if !out.iter().any(|q| q == &p) && mentions(&p, &names) {
                out.push(p);
            }
        }
    }
    out
}

fn parse_file(path: &Path, frame: (usize, usize)) -> Hit {
    let Some(text) = read_text(path) else {
        return Hit::default();
    };
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mdoc" => from_mdoc(&text, frame),
        "xml" => from_xml(&text),
        "star" => from_star(&text),
        _ => Hit::default(),
    }
}

/// Config files in `folder` (and a parent `.mdoc` that names one of its
/// movies). Later files only fill fields the earlier ones left empty.
pub fn read(folder: &Path, frame: (usize, usize)) -> FolderOptics {
    let mut hit = Hit::default();
    let mut file = String::new();
    for path in candidates(folder) {
        if done(&hit) {
            break;
        }
        let part = parse_file(&path, frame);
        if fill(&mut hit, part) && file.is_empty() {
            file = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
    }
    FolderOptics {
        pixel_a: hit.pixel_a,
        kv: hit.kv,
        cs_mm: hit.cs_mm,
        amplitude: hit.amplitude,
        dose_total: hit.dose_total,
        file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdoc_voltage_pixel_and_supersample() {
        let h = from_mdoc("Voltage = 200\nPixelSpacing = 2\nImageSize = 100 80\n[ZValue = 0]\nTiltAngle = 0\nExposureDose = 30.5\n", (200, 160));
        assert!(
            (h.pixel_a.unwrap() - 1.0).abs() < 1e-9,
            "pixel {}",
            h.pixel_a.unwrap()
        );
        assert_eq!(h.kv, Some(200.0));
        assert_eq!(h.dose_total, Some(30.5));
    }

    #[test]
    fn epu_xml_is_volts_and_metres() {
        let xml = r#"<MicroscopeImage>
            <microscopeData><highTension>300000</highTension></microscopeData>
            <pixelSize><x><numericValue>1.06e-10</numericValue></x></pixelSize>
            <SphericalAberration>0.0027</SphericalAberration>
            <AmplitudeContrast>0.07</AmplitudeContrast>
            <Dose>40.2</Dose>
        </MicroscopeImage>"#;
        let h = from_xml(xml);
        assert!((h.pixel_a.unwrap() - 1.06).abs() < 1e-9);
        assert_eq!(h.kv, Some(300.0));
        assert!((h.cs_mm.unwrap() - 2.7).abs() < 1e-9);
        assert_eq!(h.amplitude, Some(0.07));
        assert_eq!(h.dose_total, Some(40.2));
    }

    #[test]
    fn eer_items_and_relion_star() {
        let xml = r#"<item name="pixelSize.width" unit="m"><value>2.0e-10</value></item><item name="AccelerationVoltage">200000</item>"#;
        let h = from_xml(xml);
        assert!((h.pixel_a.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(h.kv, Some(200.0));
        let star = "data_optics\n\nloop_\n_rlnVoltage #1\n_rlnSphericalAberration #2\n_rlnAmplitudeContrast #3\n_rlnImagePixelSize #4\n300.0 2.7 0.10 0.825\n";
        let s = from_star(star);
        assert_eq!(s.kv, Some(300.0));
        assert_eq!(s.cs_mm, Some(2.7));
        assert_eq!(s.amplitude, Some(0.10));
        assert!((s.pixel_a.unwrap() - 0.825).abs() < 1e-9);
    }

    #[test]
    fn folder_prefers_its_own_file_and_header_wins_later() {
        let dir = tempfile::tempdir().unwrap();
        let frames = dir.path().join("Frames");
        std::fs::create_dir(&frames).unwrap();
        std::fs::write(frames.join("mic_001.mrc"), b"not-read-here").unwrap();
        std::fs::write(frames.join("mic_001.xml"), r#"<highTension>200000</highTension><pixelSize><x><numericValue>1.5e-10</numericValue></x></pixelSize>"#).unwrap();
        std::fs::write(dir.path().join("other.mdoc"), "Voltage = 300\nPixelSpacing = 9\nImageSize = 4 4\n[ZValue = 0]\nTiltAngle = 0\nSubFramePath = mic_001.mrc\n").unwrap();
        let got = read(&frames, (4, 4));
        assert_eq!(
            got.kv,
            Some(200.0),
            "the xml in the folder wins over the parent mdoc"
        );
        assert!((got.pixel_a.unwrap() - 1.5).abs() < 1e-6);
        assert_eq!(got.file, "mic_001.xml");
    }

    #[test]
    fn open_project_keeps_header_pixel_and_reads_mdoc() {
        let dir = tempfile::tempdir().unwrap();
        crate::spa::data::write_mrc(
            &dir.path().join("optics_hdr_9f3a.mrc"),
            &[0.0; 16],
            [4, 4, 1],
            1.5,
            "m",
        )
        .unwrap();
        std::fs::write(dir.path().join("optics_hdr_9f3a.mdoc"), "Voltage = 200\nPixelSpacing = 9\nImageSize = 4 4\n[ZValue = 0]\nTiltAngle = 0\nExposureDose = 30\n").unwrap();
        let p = crate::spa::project::Project::open(dir.path().to_str().unwrap()).unwrap();
        assert!(
            (p.optics.pixel_a - 1.5).abs() < 1e-3,
            "header pixel wins, got {}",
            p.optics.pixel_a
        );
        assert!((p.optics.kv - 200.0).abs() < 1e-6);
        assert!(p.optics_found.pixel && p.optics_found.kv);
        assert_eq!(p.dose_total, Some(30.0));
        assert_eq!(p.optics_found.file, "optics_hdr_9f3a.mdoc");
    }

    #[test]
    fn open_project_without_config_leaves_optics_unknown() {
        let dir = tempfile::tempdir().unwrap();
        crate::spa::data::write_mrc(
            &dir.path().join("optics_none_9f3a.mrc"),
            &[0.0; 16],
            [4, 4, 1],
            0.0,
            "m",
        )
        .unwrap();
        let p = crate::spa::project::Project::open(dir.path().to_str().unwrap()).unwrap();
        assert!(!p.optics_found.pixel);
        assert!(!p.optics_found.kv);
        assert_eq!(p.optics.pixel_a, 0.0);
        assert!(p.optics_found.file.is_empty());
        assert!(p.dose_total.is_none());
    }
}
