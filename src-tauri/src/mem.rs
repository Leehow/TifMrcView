//! Physical memory size, used to size the decoded-page cache.

use std::sync::OnceLock;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// Decoded pages kept in memory, shared by the files open in all windows.
///
/// A quarter of RAM leaves room for the OS, the webview and whatever the user
/// runs next to the viewer. At least 512 MiB so a few pages of a large stack
/// still fit on small machines; at most 8 GiB because prefetching more takes
/// long to fill and adds little. 3 GiB when the size cannot be read.
pub fn cache_budget() -> usize {
    static BUDGET: OnceLock<usize> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let b = total_ram().map_or(3 * GIB, |r| (r / 4).clamp(512 * MIB, 8 * GIB));
        usize::try_from(b).unwrap_or(usize::MAX)
    })
}

/// Total physical memory in bytes.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn total_ram() -> Option<u64> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

/// Reads `MemTotal:  16314544 kB` from /proc/meminfo.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn parse_meminfo(s: &str) -> Option<u64> {
    let line = s.lines().find(|l| l.starts_with("MemTotal:"))?;
    let mut it = line["MemTotal:".len()..].split_whitespace();
    let n: u64 = it.next()?.parse().ok()?;
    let k = match it.next() {
        Some("kB") | None => 1024,
        _ => return None,
    };
    Some(n * k).filter(|&b| b > 0)
}

#[cfg(target_os = "macos")]
fn total_ram() -> Option<u64> {
    let mut v: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: the name is NUL-terminated and `v` / `len` describe a valid u64.
    let r = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            &mut v as *mut u64 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (r == 0 && v > 0).then_some(v)
}

#[cfg(windows)]
fn total_ram() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: MEMORYSTATUSEX is plain data; dwLength is set as the API requires.
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut m) != 0 && m.ullTotalPhys > 0).then_some(m.ullTotalPhys)
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
fn total_ram() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo() {
        let s = "MemTotal:       16314544 kB\nMemFree:         1234 kB\n";
        assert_eq!(parse_meminfo(s), Some(16314544 * 1024));
        assert_eq!(parse_meminfo("MemFree: 1 kB\n"), None);
        assert_eq!(parse_meminfo("MemTotal: x kB\n"), None);
        assert_eq!(parse_meminfo("MemTotal: 0 kB\n"), None);
    }

    #[test]
    fn budget_is_clamped() {
        let b = cache_budget() as u64;
        assert!((512 * MIB..=8 * GIB).contains(&b));
    }
}
