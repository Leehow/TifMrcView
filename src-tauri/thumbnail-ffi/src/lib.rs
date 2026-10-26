//! C boundary used by the Quick Look extensions.
use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

/// # Safety
/// All pointers must be valid; path is NUL-terminated UTF-8. Output memory
/// belongs to this library and must be released once with tmv_free.
#[no_mangle]
pub unsafe extern "C" fn tmv_thumbnail_rgba(
    path_utf8: *const c_char,
    max_size: u32,
    w: *mut u32,
    h: *mut u32,
    data: *mut *mut u8,
    len: *mut usize,
) -> i32 {
    if path_utf8.is_null() || w.is_null() || h.is_null() || data.is_null() || len.is_null() {
        return 1;
    }
    *w = 0;
    *h = 0;
    *data = std::ptr::null_mut();
    *len = 0;
    match catch_unwind(AssertUnwindSafe(|| {
        let path = CStr::from_ptr(path_utf8).to_str().map_err(|_| ())?;
        let (width, height, rgba) = tifmrcview_lib::thumb::thumbnail_rgba(Path::new(path), max_size).map_err(|_| ())?;
        let mut rgba = rgba.into_boxed_slice();
        *w = width;
        *h = height;
        *len = rgba.len();
        *data = rgba.as_mut_ptr();
        std::mem::forget(rgba);
        Ok::<_, ()>(())
    })) {
        Ok(Ok(())) => 0,
        Ok(Err(())) => 1,
        Err(_) => 2,
    }
}

/// # Safety
/// dir is a NUL-terminated UTF-8 path, or null to use the shared user path.
#[no_mangle]
pub unsafe extern "C" fn tmv_configure_plugins(dir_utf8: *const c_char) -> i32 {
    match catch_unwind(|| {
        let dir = if dir_utf8.is_null() {
            tifmrcview_lib::plugins::paths::user_dir()
        } else {
            Some(Path::new(CStr::from_ptr(dir_utf8).to_str().map_err(|_| ())?).to_path_buf())
        };
        tifmrcview_lib::thumb::configure_plugins(dir);
        Ok::<_, ()>(())
    }) {
        Ok(Ok(())) => 0,
        _ => 1,
    }
}

/// # Safety
/// ptr and len must be the exact allocation returned above, used once.
#[no_mangle]
pub unsafe extern "C" fn tmv_free(ptr: *mut u8, len: usize) {
    let _ = catch_unwind(|| {
        if !ptr.is_null() {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_round_trip_and_error_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = std::ffi::CString::new(dir.path().join("plugins").to_str().unwrap()).unwrap();
        // Isolate discovery from plugins installed in the developer account.
        assert_eq!(unsafe { tmv_configure_plugins(plugin_dir.as_ptr()) }, 0);
        let missing_plugin = dir.path().join("missing.dm4");
        std::fs::write(&missing_plugin, b"no installed decoder").unwrap();
        let missing_plugin = std::ffi::CString::new(missing_plugin.to_str().unwrap()).unwrap();
        let path = dir.path().join("test.mrc");
        let mut bytes = vec![0u8; 1024];
        for (off, value) in [(0, 2i32), (4, 2), (8, 1), (12, 6)] {
            bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[208..212].copy_from_slice(b"MAP ");
        bytes[212] = 0x44;
        for v in [108u16, 120, 200, 347] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        let path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        let (mut w, mut h, mut len) = (0, 0, 0);
        let mut data = std::ptr::null_mut();
        unsafe {
            assert_eq!(
                tmv_thumbnail_rgba(path.as_ptr(), 32, &mut w, &mut h, &mut data, &mut len),
                0
            );
            assert_eq!((w, h, len), (2, 2, 16));
            assert_eq!(std::slice::from_raw_parts(data, len)[15], 255);
            tmv_free(data, len);
            assert_ne!(
                tmv_thumbnail_rgba(path.as_ptr(), 0, &mut w, &mut h, &mut data, &mut len),
                0
            );
            assert_eq!((w, h, len), (0, 0, 0));
            assert!(data.is_null());
            tmv_free(data, len);
            assert_eq!(tmv_thumbnail_rgba(missing_plugin.as_ptr(), 256, &mut w, &mut h, &mut data, &mut len), 1);
            assert_eq!((w, h, len), (0, 0, 0));
            assert!(data.is_null());
        }
    }
}
