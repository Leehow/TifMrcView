//! Making TifMrcView (or Pro) the default app for .tif/.tiff/.mrc.
//!
//! The installers already register TifMrcView as a handler for these types
//! (see `bundle.fileAssociations`). What each OS lets an app do beyond that
//! differs, so this module does the most each platform allows.

use crate::edition::NAME;

#[cfg(target_os = "macos")]
const MRC_UTI: &str = "com.leehow.tifmrcview.mrc";

#[cfg(target_os = "macos")]
pub fn set_default() -> Result<String, String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn LSSetDefaultRoleHandlerForContentType(
            content_type: CFStringRef,
            role: u32,
            handler: CFStringRef,
        ) -> i32;
    }
    const ROLES_ALL: u32 = 0xFFFF_FFFF;
    let handler = CFString::new(crate::edition::APP_ID);
    let mut failed = vec![];
    for uti in ["public.tiff", MRC_UTI] {
        let t = CFString::new(uti);
        // SAFETY: both strings are valid CFStrings for the duration of the call.
        let status = unsafe {
            LSSetDefaultRoleHandlerForContentType(t.as_concrete_TypeRef(), ROLES_ALL, handler.as_concrete_TypeRef())
        };
        if status != 0 {
            failed.push(format!("{uti} ({status})"));
        }
    }
    if failed.is_empty() {
        Ok(format!("已将 {NAME} 设为 .tif / .tiff / .mrc 的默认打开方式"))
    } else {
        Err(format!("部分类型设置失败: {}", failed.join(", ")))
    }
}

#[cfg(target_os = "linux")]
pub fn set_default() -> Result<String, String> {
    use std::process::Command;
    // Teach the per-user MIME database about .mrc in case the package's
    // system-wide definition is missing (e.g. when running the AppImage).
    if let Some(home) = std::env::var_os("HOME") {
        let dir = std::path::Path::new(&home).join(".local/share/mime");
        let pkg = dir.join("packages");
        if std::fs::create_dir_all(&pkg).is_ok() {
            let _ = std::fs::write(pkg.join("tifmrcview-mrc.xml"), include_str!("../../packaging/linux/tifmrcview-mrc.xml"));
            let _ = Command::new("update-mime-database").arg(&dir).status();
        }
    }
    let candidates = [format!("{NAME}.desktop"), format!("{}.desktop", crate::edition::BIN)];
    let desktop = candidates
        .iter()
        .find(|d| {
            ["/usr/share/applications", "/usr/local/share/applications"]
                .iter()
                .any(|p| std::path::Path::new(p).join(d).exists())
        })
        .unwrap_or(&candidates[0]);
    let status = Command::new("xdg-mime")
        .args(["default", desktop, "image/tiff", "application/x-mrc"])
        .status()
        .map_err(|e| format!("无法运行 xdg-mime: {e}"))?;
    if status.success() {
        Ok(format!("已将 {NAME} 设为 .tif / .tiff / .mrc 的默认打开方式"))
    } else {
        Err("xdg-mime 设置失败".into())
    }
}

#[cfg(windows)]
pub fn set_default() -> Result<String, String> {
    // Windows protects the user's choice, so apps may not set it silently.
    // The installer registers TifMrcView; this opens the page where the user
    // confirms it.
    std::process::Command::new("explorer.exe")
        .arg(format!("ms-settings:defaultapps?registeredAppUser={}", NAME.replace(' ', "%20")))
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(format!("已打开系统“默认应用”设置，请在其中把 .tif / .tiff / .mrc 选为 {NAME}"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn set_default() -> Result<String, String> {
    Err("当前系统不支持".into())
}

/// Claims the file types once, on the first launch after install.
pub fn first_run(config_dir: Option<std::path::PathBuf>) {
    if cfg!(windows) {
        return; // the NSIS/MSI installer writes the association
    }
    let Some(dir) = config_dir else { return };
    let marker = dir.join("default-handler-set");
    if marker.exists() {
        return;
    }
    if set_default().is_ok() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(marker, b"1");
    }
}
