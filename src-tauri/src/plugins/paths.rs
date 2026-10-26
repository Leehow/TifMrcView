//! User plugin location shared by the app and headless hosts.
use std::path::PathBuf;

pub fn user_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let data = {
        // HOME belongs to the extension container inside Quick Look. The
        // account's home is where Tauri installs the user's plugins.
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut buffer = vec![0u8; 16384];
        let mut result = std::ptr::null_mut();
        unsafe {
            if libc::getpwuid_r(
                libc::getuid(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            ) != 0
                || result.is_null()
            {
                return None;
            }
            use std::os::unix::ffi::OsStrExt;
            PathBuf::from(std::ffi::OsStr::from_bytes(
                std::ffi::CStr::from_ptr((*result).pw_dir).to_bytes(),
            ))
            .join("Library/Application Support")
        }
    };
    #[cfg(not(target_os = "macos"))]
    let data = dirs::data_dir()?;
    Some(data.join("com.leehow.tifmrcview/plugins"))
}
