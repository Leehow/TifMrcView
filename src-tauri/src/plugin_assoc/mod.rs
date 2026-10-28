//! Per-user shell registrations for enabled format plugins.
#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(any(windows, test))]
mod windows;

#[cfg(any(windows, target_os = "linux", test))]
use std::collections::BTreeSet;

#[cfg(any(windows, target_os = "linux", test))]
fn valid_ext(ext: &str) -> bool {
    !ext.is_empty()
        && ext.len() <= 16
        && ext
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

#[cfg(any(windows, target_os = "linux", test))]
fn installer_extensions() -> BTreeSet<String> {
    // This is the installer's list, not CORE_EXTENSIONS (raster types are
    // currently readable but not all registered by the installer).
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    config["bundle"]["fileAssociations"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| {
            a["ext"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
        })
        .collect()
}

#[cfg(any(windows, target_os = "linux", test))]
fn extra_extensions(extensions: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    let installed = installer_extensions();
    extensions
        .into_iter()
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| valid_ext(e) && !installed.contains(e.as_str()))
        .collect()
}

/// Never changes the user's default application. Helpers do not call this.
pub fn sync_plugin_formats() {
    #[cfg(any(windows, target_os = "linux"))]
    {
        // Install, reload and startup can overlap; rescan inside this lock.
        static SYNC: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = SYNC.lock().unwrap_or_else(|e| e.into_inner());
        let registry = crate::plugins::registry::Registry::global();
        registry.invalidate();
        let extensions = extra_extensions(registry.extensions());
        #[cfg(windows)]
        let result = windows::sync(&extensions);
        #[cfg(target_os = "linux")]
        let result = linux::sync(&extensions);
        if let Err(e) = result {
            eprintln!("plugin file associations: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extras_are_normalized_safe_and_not_installer_types() {
        let set = extra_extensions(
            [
                "tif",
                "czi",
                "ZVI",
                "zvi",
                "png",
                "../x",
                "a;b",
                "",
                "toooolongextension",
            ]
            .map(str::to_string),
        );
        assert_eq!(set, BTreeSet::from(["png".into(), "zvi".into()]));
        assert!(extra_extensions(Vec::new()).is_empty());
    }
}
