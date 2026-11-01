//! Registry plans are independent of Win32 calls.
#[cfg(windows)]
use std::collections::BTreeSet;

pub(super) const CLSID: &str = crate::edition::THUMB_CLSID_STR;
pub(super) const SLOT: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
const RECORD: &str = if crate::edition::PRO {
    r"Software\com.leehow.tifmrcview.pro\PluginFormats"
} else {
    r"Software\com.leehow.tifmrcview\PluginFormats"
};
// Tauri NSIS APP_ASSOCIATE uses fileAssociations[].name as FILECLASS.
const PROGID: &str = "MRC Image";

#[derive(Debug, Default, PartialEq, Eq)]
struct Owned {
    open_with: bool,
    thumbnail: bool,
    backup: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct Edit {
    key: String,
    name: String,
    value: Option<String>,
}

fn open_key(ext: &str) -> String {
    format!(r"Software\Classes\.{ext}\OpenWithProgids")
}
fn thumb_key(ext: &str) -> String {
    format!(r"Software\Classes\.{ext}\ShellEx\{SLOT}")
}

fn plan(
    ext: &str,
    wanted: bool,
    owned: &Owned,
    open_present: bool,
    user_thumb: Option<&str>,
    effective_thumb: Option<&str>,
    dll_registered: bool,
) -> (Vec<Edit>, Owned) {
    let mut edits = Vec::new();
    let mut next = Owned {
        open_with: owned.open_with,
        thumbnail: owned.thumbnail,
        backup: owned.backup.clone(),
    };
    if wanted {
        if !open_present {
            edits.push(Edit {
                key: open_key(ext),
                name: PROGID.into(),
                value: Some(String::new()),
            });
            next.open_with = true;
        }
        // Check HKCR too: do not hide a machine-wide or another user's provider.
        if dll_registered && effective_thumb.is_none_or(str::is_empty) {
            next.thumbnail = true;
            if !owned.thumbnail {
                next.backup = user_thumb.map(str::to_string);
            }
            edits.push(Edit {
                key: thumb_key(ext),
                name: String::new(),
                value: Some(CLSID.into()),
            });
        }
    } else {
        if owned.open_with && open_present {
            edits.push(Edit {
                key: open_key(ext),
                name: PROGID.into(),
                value: None,
            });
        }
        if owned.thumbnail && user_thumb == Some(CLSID) {
            edits.push(Edit {
                key: thumb_key(ext),
                name: String::new(),
                value: owned.backup.clone(),
            });
        }
        next = Owned::default();
    }
    (edits, next)
}

#[cfg(windows)]
pub(super) fn sync(desired: &BTreeSet<String>) -> anyhow::Result<()> {
    use windows_sys::Win32::System::Registry::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
    let previous = reg::subkeys(RECORD)?;
    let installed = super::installer_extensions();
    let mut changed = false;
    let dll = reg::read(
        HKEY_CURRENT_USER,
        &format!(r"Software\Classes\CLSID\{CLSID}\InprocServer32"),
        "",
    )?
    .is_some_and(|p| std::path::Path::new(&p).is_file());
    // An uninstalled/portable executable must not advertise a dangling ProgID.
    let command = reg::read(
        HKEY_CURRENT_USER,
        &format!(r"Software\Classes\{PROGID}\shell\open\command"),
        "",
    )?;
    let expected = format!("\"{}\" \"%1\"", std::env::current_exe()?.display());
    if !command.is_some_and(|c| c.eq_ignore_ascii_case(&expected)) {
        return Ok(()); // A dev/portable copy must not alter the installed app's entries.
    }
    for ext in previous.iter().chain(desired).collect::<BTreeSet<_>>() {
        if !super::valid_ext(ext) {
            continue;
        }
        let record = format!(r"{RECORD}\{ext}");
        let mut owned = Owned {
            open_with: reg::read(HKEY_CURRENT_USER, &record, "OpenWith")?.as_deref() == Some("1"),
            thumbnail: reg::read(HKEY_CURRENT_USER, &record, "Thumbnail")?.as_deref() == Some("1"),
            backup: reg::read(HKEY_CURRENT_USER, &record, "ThumbnailBackup")?,
        };
        // A newer installer may take over a previously dynamic extension.
        // Hand its thumbnail slot to the installer rather than deleting it.
        if installed.contains(ext) {
            owned.thumbnail = false;
        }
        let user_thumb = reg::read(HKEY_CURRENT_USER, &thumb_key(ext), "")?;
        let effective = reg::read(HKEY_CLASSES_ROOT, &format!(r".{ext}\ShellEx\{SLOT}"), "")?;
        let present = reg::read(HKEY_CURRENT_USER, &open_key(ext), PROGID)?.is_some();
        let (edits, next) = plan(
            ext,
            desired.contains(ext),
            &owned,
            present,
            user_thumb.as_deref(),
            effective.as_deref(),
            dll,
        );
        // Journal ownership before writing shell entries, so interrupted writes
        // remain removable on the next launch or by NSIS.
        if desired.contains(ext) && next != owned {
            reg::write(
                &record,
                "OpenWith",
                Some(if next.open_with { "1" } else { "0" }),
            )?;
            reg::write(
                &record,
                "Thumbnail",
                Some(if next.thumbnail { "1" } else { "0" }),
            )?;
            reg::write(&record, "ThumbnailBackup", next.backup.as_deref())?;
        }
        for edit in edits {
            reg::write(&edit.key, &edit.name, edit.value.as_deref())?;
            changed = true;
        }
        if !desired.contains(ext) {
            reg::delete_record(&record)?;
        }
    }
    if changed {
        unsafe {
            windows_sys::Win32::UI::Shell::SHChangeNotify(
                0x08000000,
                0,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
    }
    Ok(())
}

#[cfg(windows)]
mod reg {
    use anyhow::{bail, Result};
    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND,
    };
    use windows_sys::Win32::System::Registry::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn check(code: u32) -> Result<()> {
        if code != 0 {
            bail!(
                "registry: {}",
                std::io::Error::from_raw_os_error(code as i32)
            );
        }
        Ok(())
    }
    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
    fn open(root: HKEY, path: &str) -> Result<Option<Key>> {
        let mut key = std::ptr::null_mut();
        let code = unsafe {
            RegOpenKeyExW(
                root,
                wide(path).as_ptr(),
                0,
                KEY_READ | KEY_WOW64_64KEY,
                &mut key,
            )
        };
        if code == ERROR_FILE_NOT_FOUND || code == ERROR_PATH_NOT_FOUND {
            return Ok(None);
        }
        check(code)?;
        Ok(Some(Key(key)))
    }
    pub(super) fn read(root: HKEY, path: &str, name: &str) -> Result<Option<String>> {
        let Some(key) = open(root, path)? else {
            return Ok(None);
        };
        let mut size = 0;
        let name = wide(name);
        let code = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(code)?;
        // OpenWithProgids also permits REG_NONE/REG_BINARY. Existence is enough.
        let mut bytes = vec![0u16; (size as usize).div_ceil(2)];
        check(unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                bytes.as_mut_ptr().cast(),
                &mut size,
            )
        })?;
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        Ok(Some(String::from_utf16_lossy(&bytes)))
    }
    pub(super) fn write(path: &str, name: &str, value: Option<&str>) -> Result<()> {
        let mut key = std::ptr::null_mut();
        if value.is_none() && open(HKEY_CURRENT_USER, path)?.is_none() {
            return Ok(());
        }
        check(unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(path).as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_WRITE | KEY_WOW64_64KEY,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        })?;
        let key = Key(key);
        if let Some(value) = value {
            let data = wide(value);
            check(unsafe {
                RegSetValueExW(
                    key.0,
                    wide(name).as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            })
        } else {
            let code = unsafe { RegDeleteValueW(key.0, wide(name).as_ptr()) };
            if code == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                check(code)
            }
        }
    }
    pub(super) fn subkeys(path: &str) -> Result<Vec<String>> {
        let Some(key) = open(HKEY_CURRENT_USER, path)? else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let code = unsafe {
                RegEnumKeyExW(
                    key.0,
                    result.len() as u32,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if code == ERROR_NO_MORE_ITEMS {
                break;
            }
            check(code)?;
            result.push(String::from_utf16_lossy(&name[..len as usize]));
        }
        Ok(result)
    }
    pub(super) fn delete_record(path: &str) -> Result<()> {
        // Our journal has values only; never remove file-type trees.
        let code =
            unsafe { RegDeleteKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), KEY_WOW64_64KEY, 0) };
        if code == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            check(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_plan_preserves_providers_and_preexisting_open_with() {
        let (edits, next) = plan(
            "dm4",
            true,
            &Owned::default(),
            true,
            None,
            Some("other"),
            true,
        );
        assert!(edits.is_empty());
        assert_eq!(next, Owned::default());
        let (edits, next) = plan("dm4", true, &Owned::default(), false, None, None, true);
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].key, r"Software\Classes\.dm4\OpenWithProgids");
        assert_eq!(edits[0].name, PROGID);
        assert!(next.thumbnail && next.open_with);
        assert!(
            plan("dm4", true, &next, true, Some(CLSID), Some(CLSID), true)
                .0
                .is_empty()
        );
        let removed = plan("dm4", false, &next, true, Some(CLSID), Some(CLSID), true).0;
        assert_eq!(removed.len(), 2);
        assert!(removed.iter().all(|e| e.value.is_none()));
        assert_eq!(
            plan(
                "dm4",
                false,
                &next,
                true,
                Some("new-provider"),
                Some("new-provider"),
                true
            )
            .0
            .len(),
            1
        );
    }
    #[test]
    fn restores_empty_value_and_requires_installed_dll() {
        let (edits, owned) = plan(
            "dm4",
            true,
            &Owned::default(),
            false,
            Some(""),
            Some(""),
            true,
        );
        assert_eq!(edits.len(), 2);
        // A journal can be written before an interrupted shell write.
        let retry = plan("dm4", true, &owned, true, Some(""), Some(""), true);
        assert_eq!(retry.0.len(), 1);
        assert_eq!(retry.1.backup.as_deref(), Some(""));
        let removed = plan("dm4", false, &owned, true, Some(CLSID), Some(CLSID), true).0;
        assert_eq!(removed[1].value.as_deref(), Some(""));
        assert_eq!(
            plan("dm4", true, &Owned::default(), false, None, None, false)
                .0
                .len(),
            1
        );
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert!(config["bundle"]["fileAssociations"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |a| a["name"] == PROGID && a["ext"].as_array().unwrap().iter().any(|e| e == "mrc")
            ));
        assert!(
            include_str!("../../../packaging/windows/thumbnail-hooks.nsh")
                .contains(&format!("!define TMV_PLUGIN_PROGID \"{PROGID}\""))
        );
        assert!(RECORD.ends_with("PluginFormats"));
    }
}
