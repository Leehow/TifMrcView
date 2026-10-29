//! Explorer COM thumbnail server. No Tauri or WebView runtime.
#![cfg(windows)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::core::{implement, Error, IUnknown, Interface, Ref, Result, GUID, HRESULT};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_POINTER, S_FALSE, S_OK,
};
use windows::Win32::Graphics::Gdi::{CreateDIBSection, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP};
use windows::Win32::System::Com::{
    CoTaskMemFree, IClassFactory, IClassFactory_Impl, IStream, STATFLAG_DEFAULT, STATSTG, STREAM_SEEK_SET,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithStream, IInitializeWithStream_Impl};
use windows::Win32::UI::Shell::{IThumbnailProvider, IThumbnailProvider_Impl, WTSAT_RGB, WTS_ALPHATYPE};

const CLSID: GUID = GUID::from_u128(tifmrcview_lib::edition::THUMB_CLSID);
const MAX_STREAM: u64 = 512 << 20;
static OBJECTS: AtomicUsize = AtomicUsize::new(0);
static LOCKS: AtomicUsize = AtomicUsize::new(0);

struct Lifetime;
impl Lifetime {
    fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn guarded(f: impl FnOnce() -> Result<()>) -> Result<()> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(Error::from_hresult(E_FAIL)))
}

#[implement(IThumbnailProvider, IInitializeWithStream)]
struct Provider {
    file: RefCell<Option<(tempfile::TempDir, String)>>,
    _life: Lifetime,
}

impl IInitializeWithStream_Impl for Provider_Impl {
    fn Initialize(&self, stream: Ref<IStream>, _mode: u32) -> Result<()> {
        guarded(|| {
            let mut slot = self.file.borrow_mut();
            if slot.is_some() {
                return Err(Error::from_hresult(E_FAIL));
            }
            let stream = stream.ok()?;
            let mut stat = STATSTG::default();
            unsafe {
                stream.Stat(&mut stat, STATFLAG_DEFAULT)?;
            }
            let name = if stat.pwcsName.is_null() {
                String::new()
            } else {
                let name = unsafe { stat.pwcsName.to_string().unwrap_or_default() };
                unsafe {
                    CoTaskMemFree(Some(stat.pwcsName.0.cast()));
                }
                name
            };
            if stat.cbSize == 0 || stat.cbSize > MAX_STREAM {
                return Err(Error::from_hresult(E_FAIL));
            }
            unsafe {
                stream.Seek(0, STREAM_SEEK_SET, None)?;
            }
            let dir = tempfile::tempdir().map_err(|_| Error::from_hresult(E_FAIL))?;
            let mut out = std::fs::File::create(dir.path().join("source")).map_err(|_| Error::from_hresult(E_FAIL))?;
            let mut buffer = vec![0u8; 1 << 20];
            let mut total = 0u64;
            loop {
                let mut read = 0;
                unsafe {
                    stream
                        .Read(buffer.as_mut_ptr().cast(), buffer.len() as u32, Some(&mut read))
                        .ok()?;
                }
                if read == 0 {
                    break;
                }
                total += u64::from(read);
                if total > MAX_STREAM {
                    return Err(Error::from_hresult(E_FAIL));
                }
                out.write_all(&buffer[..read as usize])
                    .map_err(|_| Error::from_hresult(E_FAIL))?;
            }
            if total != stat.cbSize {
                return Err(Error::from_hresult(E_FAIL));
            }
            let ext = std::path::Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            *slot = Some((dir, ext));
            Ok(())
        })
    }
}

impl IThumbnailProvider_Impl for Provider_Impl {
    fn GetThumbnail(&self, size: u32, bitmap: *mut HBITMAP, alpha: *mut WTS_ALPHATYPE) -> Result<()> {
        if bitmap.is_null() || alpha.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        unsafe {
            *bitmap = HBITMAP::default();
            *alpha = WTSAT_RGB;
        }
        guarded(|| {
            if size == 0 {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            let slot = self.file.borrow();
            let (dir, ext) = slot.as_ref().ok_or_else(|| Error::from_hresult(E_FAIL))?;
            tifmrcview_lib::thumb::configure_plugins(tifmrcview_lib::plugins::paths::user_dir());
            // Stream names are optional. Try installed format decoders on the
            // same disk snapshot when the shell supplies an anonymous stream.
            let mut extensions = tifmrcview_lib::formats::extensions();
            if extensions.contains(ext) {
                extensions = vec![ext.clone()];
            }
            let mut image = None;
            for candidate in extensions {
                let path = dir.path().join(format!("image.{candidate}"));
                std::fs::hard_link(dir.path().join("source"), &path).map_err(|_| Error::from_hresult(E_FAIL))?;
                let result = tifmrcview_lib::thumb::thumbnail_rgba(&path, size.min(tifmrcview_lib::thumb::MAX_SIZE));
                let _ = std::fs::remove_file(path);
                if let Ok(rgba) = result {
                    image = Some(rgba);
                    break;
                }
            }
            let (w, h, mut rgba) = image.ok_or_else(|| Error::from_hresult(E_FAIL))?;
            for pixel in rgba.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let dib = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)? };
            unsafe {
                std::ptr::copy_nonoverlapping(rgba.as_ptr(), bits.cast(), rgba.len());
                *bitmap = dib;
            }
            Ok(())
        })
    }
}

#[implement(IClassFactory)]
struct Factory {
    _life: Lifetime,
}
impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, object: *mut *mut c_void) -> Result<()> {
        if object.is_null() || iid.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        unsafe {
            *object = std::ptr::null_mut();
        }
        guarded(|| {
            if !outer.is_null() {
                return Err(Error::from_hresult(CLASS_E_NOAGGREGATION));
            }
            let provider: IThumbnailProvider = Provider {
                file: RefCell::new(None),
                _life: Lifetime::new(),
            }
            .into();
            unsafe { provider.query(iid, object).ok() }
        })
    }
    fn LockServer(&self, lock: windows::core::BOOL) -> Result<()> {
        if lock.as_bool() {
            LOCKS.fetch_add(1, Ordering::SeqCst);
        } else {
            let _ = LOCKS.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        }
        Ok(())
    }
}

/// # Safety
/// COM supplies valid GUID and output pointers.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    if clsid.is_null() || iid.is_null() || object.is_null() {
        return E_POINTER;
    }
    *object = std::ptr::null_mut();
    guarded(|| {
        if *clsid != CLSID {
            return Err(Error::from_hresult(CLASS_E_CLASSNOTAVAILABLE));
        }
        let factory: IClassFactory = Factory { _life: Lifetime::new() }.into();
        factory.query(iid, object).ok()
    })
    .into()
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::SeqCst) == 0 && LOCKS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP, HGDIOBJ};
    use windows::Win32::UI::Shell::SHCreateMemStream;

    #[test]
    fn anonymous_stream_creates_opaque_bitmap_and_releases_objects() {
        let mut bytes = vec![0u8; 1024];
        for (off, value) in [(0, 16i32), (4, 16), (8, 1), (12, 6)] {
            bytes[off..off + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[208..212].copy_from_slice(b"MAP ");
        bytes[212] = 0x44;
        for n in 0..256u16 {
            bytes.extend_from_slice(&(108 + n % 240).to_le_bytes());
        }
        unsafe {
            let stream = SHCreateMemStream(Some(&bytes)).unwrap();
            let init: IInitializeWithStream = Provider {
                file: RefCell::new(None),
                _life: Lifetime::new(),
            }
            .into();
            init.Initialize(&stream, 0).unwrap();
            assert!(init.Initialize(&stream, 0).is_err());
            let provider: IThumbnailProvider = init.cast().unwrap();
            let mut bitmap = HBITMAP::default();
            let mut alpha = WTS_ALPHATYPE::default();
            provider.GetThumbnail(8, &mut bitmap, &mut alpha).unwrap();
            let mut info = BITMAP::default();
            assert_ne!(
                GetObjectW(
                    HGDIOBJ(bitmap.0),
                    std::mem::size_of::<BITMAP>() as i32,
                    Some((&mut info as *mut BITMAP).cast())
                ),
                0
            );
            assert_eq!((info.bmWidth, info.bmHeight), (8, 8));
            assert_eq!(alpha, WTSAT_RGB);
            let pixels = std::slice::from_raw_parts(info.bmBits.cast::<u8>(), 8 * 8 * 4);
            assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[0] > 100));
            assert!(DeleteObject(HGDIOBJ(bitmap.0)).as_bool());
            assert!(provider.GetThumbnail(8, std::ptr::null_mut(), &mut alpha).is_err());
        }
        assert_eq!(DllCanUnloadNow(), S_OK);
    }
}
