//! XDG file generation and narrowly owned Added Associations.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const DESKTOP: &str = if crate::edition::PRO { "tifmrcview-pro.desktop" } else { "tifmrcview.desktop" };
const PREFIX: &str = "application/x-tifmrcview-";

#[derive(Default, Deserialize, Serialize)]
struct Record {
    added: BTreeSet<String>,
    desktop: Option<String>,
}

fn mime_types(extensions: &BTreeSet<String>, globs: &str) -> BTreeMap<String, String> {
    extensions
        .iter()
        .map(|ext| {
            let pattern = format!("*.{ext}");
            let known = globs
                .lines()
                .filter(|l| !l.starts_with('#'))
                .filter_map(|line| {
                    let mut parts = line.split(':');
                    let weight = parts.next()?.parse::<u32>().ok()?;
                    let mime = parts.next()?;
                    let glob = parts.next()?;
                    (glob == pattern
                        && !mime.starts_with(PREFIX)
                        && mime
                            .split_once('/')
                            .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
                        && mime
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"/+.-_".contains(&b)))
                    .then_some((weight, mime))
                })
                .max_by_key(|(weight, _)| *weight)
                .map(|(_, mime)| mime.to_string());
            (
                ext.clone(),
                known.unwrap_or_else(|| format!("{PREFIX}{ext}")),
            )
        })
        .collect()
}

fn mime_xml(types: &BTreeMap<String, String>) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n");
    for (ext, mime) in types {
        xml.push_str(&format!(
            "  <mime-type type=\"{mime}\"><glob pattern=\"*.{ext}\"/></mime-type>\n"
        ));
    }
    xml.push_str("</mime-info>\n");
    xml
}

fn thumbnailer(mimes: &BTreeSet<String>) -> String {
    let exec = include_str!("../../../packaging/linux/tifmrcview.thumbnailer")
        .lines()
        .find(|line| line.starts_with("Exec="))
        .unwrap();
    let exec = exec.replacen("Exec=tifmrcview ", &format!("Exec={} ", crate::edition::BIN), 1);
    format!(
        "[Thumbnailer Entry]\nTryExec={}\n{exec}\nMimeType={};\n",
        crate::edition::BIN,
        mimes.iter().cloned().collect::<Vec<_>>().join(";")
    )
}

/// Preserve defaults, comments, other applications and their preference order.
fn mimeapps(
    input: &str,
    desired: &BTreeSet<String>,
    previous: &BTreeSet<String>,
) -> (String, BTreeSet<String>) {
    let mut section = "";
    let mut removed = BTreeSet::new();
    for line in input.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line;
        }
        if section == "[Removed Associations]" {
            if let Some((mime, apps)) = line.split_once('=') {
                if apps.split(';').any(|a| a == DESKTOP) {
                    removed.insert(mime.to_string());
                }
            }
        }
    }
    let desired: BTreeSet<_> = desired.difference(&removed).cloned().collect();
    let mut output = String::new();
    let mut seen = BTreeSet::new();
    let mut owned = BTreeSet::new();
    let mut in_added = false;
    let mut has_added = false;
    // Insert new keys before leaving the Added section (or at EOF).
    let append = |output: &mut String, seen: &BTreeSet<String>, owned: &mut BTreeSet<String>| {
        for mime in desired.difference(seen) {
            if !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!("{mime}={DESKTOP};\n"));
            owned.insert(mime.clone());
        }
    };
    for raw in input.split_inclusive('\n') {
        let line = raw.trim();
        if line.starts_with('[') {
            if in_added {
                append(&mut output, &seen, &mut owned);
            }
            in_added = line == "[Added Associations]";
            has_added |= in_added;
        }
        if in_added && !line.starts_with('#') {
            if let Some((mime, apps)) = line.split_once('=') {
                let was_present = apps.split(';').any(|a| a == DESKTOP);
                let want = desired.contains(mime);
                let remove = previous.contains(mime) && !want;
                seen.insert(mime.to_string());
                if want && (!was_present || previous.contains(mime)) {
                    owned.insert(mime.to_string());
                }
                if (remove && was_present) || (want && !was_present) {
                    let mut apps: Vec<_> = apps
                        .split(';')
                        .filter(|a| !a.is_empty() && (!remove || *a != DESKTOP))
                        .collect();
                    if want && !was_present {
                        apps.push(DESKTOP);
                    }
                    if !apps.is_empty() {
                        output.push_str(&format!("{mime}={};\n", apps.join(";")));
                    }
                    continue;
                }
            }
        }
        output.push_str(raw);
    }
    if !has_added && !desired.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("[Added Associations]\n");
        append(&mut output, &seen, &mut owned);
    } else if in_added {
        append(&mut output, &seen, &mut owned);
    }
    (output, owned)
}

fn desktop(exe: &Path, mimes: &BTreeSet<String>) -> String {
    // Desktop Exec quoting is not shell quoting. % is a field-code escape.
    let executable = exe
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%")
        // Desktop string escapes are decoded before Exec argument escapes.
        .replace('\\', "\\\\");
    format!("[Desktop Entry]\nType=Application\nName={}\nExec=\"{executable}\" %U\nIcon={}\nTerminal=false\nNoDisplay=true\nMimeType={};\n",
        crate::edition::NAME, crate::edition::BIN, mimes.iter().cloned().collect::<Vec<_>>().join(";"))
}

fn read_optional(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

fn write_changed(path: &Path, contents: Option<&str>) -> Result<bool> {
    let existing = read_optional(path)?;
    if let Some(contents) = contents {
        if existing == contents {
            return Ok(false);
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("tmv-tmp");
        std::fs::write(&tmp, contents)?;
        std::fs::rename(tmp, path)?;
    } else {
        match std::fs::remove_file(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

#[cfg(target_os = "linux")]
pub(super) fn sync(extensions: &BTreeSet<String>) -> Result<()> {
    let data = dirs::data_dir().ok_or_else(|| anyhow::anyhow!("no XDG data directory"))?;
    let config = dirs::config_dir().ok_or_else(|| anyhow::anyhow!("no XDG config directory"))?;
    let mut globs = String::new();
    let data_dirs =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for dir in std::iter::once(data.clone()).chain(std::env::split_paths(&data_dirs)) {
        globs.push_str(&read_optional(&dir.join("mime/globs2"))?);
        globs.push('\n');
    }
    sync_at(
        &data,
        &config,
        extensions,
        &globs,
        &std::env::current_exe()?,
    )
}

fn sync_at(
    data: &Path,
    config: &Path,
    extensions: &BTreeSet<String>,
    globs: &str,
    exe: &Path,
) -> Result<()> {
    let record_path = data.join(crate::edition::APP_ID).join("plugin-formats.json");
    let record = read_optional(&record_path)?;
    let previous: Record = if record.is_empty() {
        Record::default()
    } else {
        serde_json::from_str(&record)?
    };
    let types = mime_types(extensions, globs);
    let mimes: BTreeSet<_> = types.values().cloned().collect();
    let apps_path = config.join("mimeapps.list");
    let (apps, added) = mimeapps(&read_optional(&apps_path)?, &mimes, &previous.added);
    let desktop_path = data.join("applications").join(DESKTOP);
    let old_desktop = read_optional(&desktop_path)?;
    let own_desktop = previous.desktop.as_deref() == Some(old_desktop.as_str());
    let next_desktop = if !mimes.is_empty() && (old_desktop.is_empty() || own_desktop) {
        Some(desktop(exe, &mimes))
    } else {
        None
    };
    // Keep a removal journal before writing any shared associations.
    let journal = Record {
        added: previous.added.union(&added).cloned().collect(),
        desktop: next_desktop.clone(),
    };
    if !mimes.is_empty() || !previous.added.is_empty() || previous.desktop.is_some() {
        write_changed(&record_path, Some(&serde_json::to_string(&journal)?))?;
    }
    let mime_dir = data.join("mime");
    let xml_path = mime_dir.join(format!("packages/{}-plugins.xml", crate::edition::BIN));
    let xml = mime_xml(&types);
    let mime_changed = write_changed(&xml_path, (!types.is_empty()).then_some(xml.as_str()))?;
    let thumb = thumbnailer(&mimes);
    write_changed(
        &data.join(format!("thumbnailers/{}-plugins.thumbnailer", crate::edition::BIN)),
        (!mimes.is_empty()).then_some(thumb.as_str()),
    )?;
    if !apps.is_empty() {
        write_changed(&apps_path, Some(&apps))?;
    }
    let mut desktop_changed = false;
    if let Some(text) = &next_desktop {
        if old_desktop.is_empty() || own_desktop {
            desktop_changed = write_changed(&desktop_path, Some(text))?;
        }
    } else if own_desktop {
        desktop_changed = write_changed(&desktop_path, None)?;
    }
    let next = Record {
        added,
        desktop: next_desktop,
    };
    write_changed(
        &record_path,
        (!mimes.is_empty() || !next.added.is_empty() || next.desktop.is_some())
            .then(|| serde_json::to_string(&next))
            .transpose()?
            .as_deref(),
    )?;
    // Cache tools are optional. Never run xdg-mime default.
    #[cfg(target_os = "linux")]
    {
        if mime_changed {
            refresh("update-mime-database", &mime_dir);
        }
        if desktop_changed {
            refresh("update-desktop-database", &data.join("applications"));
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (mime_changed, desktop_changed);
    Ok(())
}

#[cfg(target_os = "linux")]
fn refresh(tool: &str, directory: &Path) {
    match std::process::Command::new(tool).arg(directory).status() {
        Ok(status) if status.success() => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        result => eprintln!("{tool}: {result:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn uses_shared_mime_and_generates_exact_globs_and_exec() {
        let types = mime_types(
            &set(&["dm4", "png"]),
            "50:image/png:*.png\n50:application/x-tifmrcview-dm4:*.dm4\n",
        );
        assert_eq!(types["png"], "image/png");
        assert_eq!(types["dm4"], "application/x-tifmrcview-dm4");
        let xml = mime_xml(&types);
        assert!(xml.contains("<mime-type type=\"image/png\"><glob pattern=\"*.png\"/>"));
        assert!(thumbnailer(&types.values().cloned().collect())
            .contains(&format!("Exec={} --thumbnail %i %o --size %s\n", crate::edition::BIN)));
    }
    #[test]
    fn added_associations_preserve_defaults_order_and_user_entries() {
        // This edition's desktop file (tifmrcview-pro.desktop in Pro).
        let original = "# keep\n[Default Applications]\nimage/png=other.desktop;\n[Added Associations]\nimage/png=before.desktop;tifmrcview.desktop;after.desktop;\napplication/x-tifmrcview-dm3=other.desktop;\n[Removed Associations]\nimage/jpeg=tifmrcview.desktop;\n"
            .replace("tifmrcview.desktop", DESKTOP);
        let (out, owned) = mimeapps(
            &original,
            &set(&["image/png", "application/x-tifmrcview-dm4", "image/jpeg"]),
            &set(&["application/x-tifmrcview-dm3"]),
        );
        assert!(out.starts_with("# keep\n[Default Applications]\nimage/png=other.desktop;\n"));
        assert!(out.contains(&format!("image/png=before.desktop;{DESKTOP};after.desktop;")));
        assert!(out.contains("application/x-tifmrcview-dm3=other.desktop;"));
        assert_eq!(owned, set(&["application/x-tifmrcview-dm4"]));
        assert_eq!(
            mimeapps(
                &out,
                &set(&["image/png", "application/x-tifmrcview-dm4", "image/jpeg"]),
                &owned
            )
            .0,
            out
        );
        let (removed, _) = mimeapps(&out, &BTreeSet::new(), &owned);
        assert_eq!(removed, original);
    }
    #[test]
    fn sync_install_repeat_remove_in_xdg_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let config = dir.path().join("config");
        let exe = Path::new("/opt/Tif Mrc/tifmrcview");
        let run = |exts| sync_at(&data, &config, &set(exts), "", exe).unwrap();
        run(&["dm4", "dm3"]);
        let apps = std::fs::read(config.join("mimeapps.list")).unwrap();
        run(&["dm3", "dm4"]);
        assert_eq!(std::fs::read(config.join("mimeapps.list")).unwrap(), apps);
        run(&["dm4"]);
        let bin = crate::edition::BIN;
        let xml = std::fs::read_to_string(data.join(format!("mime/packages/{bin}-plugins.xml"))).unwrap();
        assert!(!xml.contains("dm3"));
        assert!(xml.contains("dm4"));
        run(&[]);
        assert!(!data.join(format!("thumbnailers/{bin}-plugins.thumbnailer")).exists());
        assert!(!data.join(format!("mime/packages/{bin}-plugins.xml")).exists());
        assert!(!data.join("applications").join(DESKTOP).exists());
        assert!(!data.join(crate::edition::APP_ID).join("plugin-formats.json").exists());
        assert_eq!(
            std::fs::read_to_string(config.join("mimeapps.list")).unwrap(),
            "[Added Associations]\n"
        );
        let launcher = data.join("applications").join(DESKTOP);
        std::fs::write(&launcher, "[Desktop Entry]\nName=User launcher\n").unwrap();
        run(&["dm4"]);
        run(&[]);
        assert_eq!(
            std::fs::read_to_string(launcher).unwrap(),
            "[Desktop Entry]\nName=User launcher\n"
        );
    }
}
