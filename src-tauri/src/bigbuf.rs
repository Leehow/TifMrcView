//! Volume-sized float buffers that do not have to fit in memory.
//!
//! A 2048 × 2048 × 600 tomogram is 10 GB as floats, and the missing-wedge
//! network holds several such volumes at once. A buffer that would not fit
//! in the memory still free is kept in a scratch file mapped into memory
//! instead: the system then pages it to and from the disk as it is used, so
//! a big job runs slower on a small server but runs to the end instead of
//! being killed for want of memory.
use rayon::prelude::*;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Bytes of in-memory buffers alive now.
static IN_MEMORY: AtomicU64 = AtomicU64::new(0);

/// Floats in memory, or in a mapped scratch file: unlinked at once where the
/// disk allows it, else (`Some(path)`) removed when the buffer is dropped.
pub enum Buf {
    Mem(Vec<f32>),
    Map(memmap2::MmapMut, usize, Option<PathBuf>),
}

/// Buffers smaller than this are always in memory.
const SMALL: u64 = 256 << 20;

/// Where scratch files may go, best first: beside the job's results (the big
/// disk the user chose), else the system's temporary folder.
fn scratch_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["TMV_SCRATCH", "TMV_OUTPUT_ROOT"].iter().filter_map(std::env::var_os).map(PathBuf::from).filter(|p| p.is_dir()).collect();
    dirs.push(std::env::temp_dir());
    dirs
}

/// Free bytes on the disk of `dir`, if known.
fn free_bytes(dir: &std::path::Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) } != 0 {
            return None;
        }
        let st = unsafe { st.assume_init() };
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        None
    }
}

/// Scratch files left by a process that was killed (so never removed).
fn sweep(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix(".tmv-scratch-")).and_then(|r| r.split('-').next()).and_then(|p| p.parse::<u32>().ok()) else { continue };
        #[cfg(target_os = "linux")]
        if pid != std::process::id() && !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            let _ = std::fs::remove_file(e.path());
        }
        #[cfg(not(target_os = "linux"))]
        let _ = pid;
    }
}

/// Whether `bytes` more fit in memory, keeping a margin for everything else.
fn fits(bytes: u64) -> bool {
    // TMV_SCRATCH_FORCE puts every buffer on disk (to test the slow path).
    if std::env::var_os("TMV_SCRATCH_FORCE").is_some() {
        return false;
    }
    if bytes < SMALL {
        return true;
    }
    let free = crate::resources::memory_available_bytes();
    if free == 0 {
        return true;
    }
    // Zeroed vectors are not yet touched, so what earlier buffers will take
    // is not all out of `free` yet: count them against it.
    let held = IN_MEMORY.load(Ordering::Relaxed);
    let budget = (free.saturating_add(held) as f64 * 0.7) as u64;
    held.saturating_add(bytes) <= budget.saturating_sub(2 << 30)
}

fn mapped(len: usize) -> std::io::Result<(memmap2::MmapMut, Option<PathBuf>)> {
    let bytes = (len.max(1) * 4) as u64;
    let mut last = std::io::Error::other("no scratch folder");
    for dir in scratch_dirs() {
        sweep(&dir);
        // Room for it and a margin: a full system disk breaks everything else.
        if free_bytes(&dir).is_some_and(|f| f < bytes + (2 << 30)) {
            last = std::io::Error::other(format!("{} 空间不够", dir.display()));
            continue;
        }
        let path = dir.join(format!(".tmv-scratch-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let made = (|| -> std::io::Result<memmap2::MmapMut> {
            let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
            // Size and map first, unlink after: a Windows disk under WSL
            // (drvfs, /mnt/e) refuses to size a file that is already unlinked.
            file.set_len(bytes)?;
            unsafe { memmap2::MmapMut::map_mut(&file) }
        })();
        match made {
            Ok(m) => {
                // Gone from the folder at once where the disk allows it; the
                // mapping keeps the space until dropped.
                let keep = std::fs::remove_file(&path).err().map(|_| path);
                return Ok((m, keep));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                last = e;
            }
        }
    }
    Err(last)
}

static NEXT: AtomicU64 = AtomicU64::new(0);

impl Buf {
    /// `len` zeros, in memory when they fit, else in a scratch file.
    pub fn zeros(len: usize) -> Buf {
        let bytes = (len * 4) as u64;
        if !fits(bytes) {
            match mapped(len) {
                Ok((m, keep)) => return Buf::Map(m, len, keep),
                Err(e) => eprintln!("scratch file unavailable ({e}); keeping {} MB in memory", bytes >> 20),
            }
        }
        Buf::from(vec![0f32; len])
    }

    /// `len` values `f(i)`, computed in parallel.
    pub fn from_fn(len: usize, f: impl Fn(usize) -> f32 + Sync) -> Buf {
        let mut b = Buf::zeros(len);
        // A slab at a time, so a scratch file's pages are let go as it fills.
        let slab = 1 << 26;
        for s0 in (0..len).step_by(slab) {
            let end = (s0 + slab).min(len);
            b[s0..end].par_chunks_mut(1 << 16).enumerate().for_each(|(k, chunk)| {
                for (j, v) in chunk.iter_mut().enumerate() {
                    *v = f(s0 + k * (1 << 16) + j);
                }
            });
            b.trim();
        }
        b
    }

    /// For a buffer in a scratch file: writes what changed to the disk and
    /// lets go of its pages, so a mapping as big as the free memory does not
    /// fill it (on a shared server a 10 GB buffer had left 92 MB free).
    /// Later reads page the values back in. Nothing for one in memory.
    pub fn trim(&self) {
        if let Buf::Map(m, ..) = self {
            if m.flush().is_err() {
                return;
            }
            #[cfg(unix)]
            unsafe {
                libc::madvise(m.as_ptr() as *mut libc::c_void, m.len(), libc::MADV_DONTNEED);
            }
        }
    }

    /// Whether the values are in a scratch file.
    pub fn on_disk(&self) -> bool {
        matches!(self, Buf::Map(..))
    }

    /// The values as a vector (copied out of a scratch file).
    pub fn into_vec(mut self) -> Vec<f32> {
        match &mut self {
            Buf::Mem(v) => {
                let out = std::mem::take(v);
                IN_MEMORY.fetch_sub((out.len() * 4) as u64, Ordering::Relaxed);
                out
            }
            Buf::Map(..) => self.to_vec(),
        }
    }
}

impl From<Vec<f32>> for Buf {
    fn from(v: Vec<f32>) -> Buf {
        IN_MEMORY.fetch_add((v.len() * 4) as u64, Ordering::Relaxed);
        Buf::Mem(v)
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        match self {
            Buf::Mem(v) => {
                IN_MEMORY.fetch_sub((v.len() * 4) as u64, Ordering::Relaxed);
            }
            Buf::Map(_, _, Some(path)) => {
                let _ = std::fs::remove_file(path);
            }
            Buf::Map(..) => {}
        }
    }
}

impl Deref for Buf {
    type Target = [f32];
    fn deref(&self) -> &[f32] {
        match self {
            Buf::Mem(v) => v,
            // A mapping starts on a page boundary, so the floats are aligned.
            Buf::Map(m, n, _) => unsafe { std::slice::from_raw_parts(m.as_ptr() as *const f32, *n) },
        }
    }
}

impl DerefMut for Buf {
    fn deref_mut(&mut self) -> &mut [f32] {
        match self {
            Buf::Mem(v) => v,
            Buf::Map(m, n, _) => unsafe { std::slice::from_raw_parts_mut(m.as_mut_ptr() as *mut f32, *n) },
        }
    }
}

impl Clone for Buf {
    fn clone(&self) -> Buf {
        let mut b = Buf::zeros(self.len());
        b.par_chunks_mut(1 << 16).zip(self.par_chunks(1 << 16)).for_each(|(d, s)| d.copy_from_slice(s));
        b.trim();
        self.trim();
        b
    }
}

impl std::fmt::Debug for Buf {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Buf({} floats{})", self.len(), if self.on_disk() { ", on disk" } else { "" })
    }
}

impl FromIterator<f32> for Buf {
    fn from_iter<I: IntoIterator<Item = f32>>(it: I) -> Buf {
        Buf::from(it.into_iter().collect::<Vec<f32>>())
    }
}

impl FromParallelIterator<f32> for Buf {
    fn from_par_iter<I: IntoParallelIterator<Item = f32>>(it: I) -> Buf {
        Buf::from(it.into_par_iter().collect::<Vec<f32>>())
    }
}

impl PartialEq for Buf {
    fn eq(&self, other: &Buf) -> bool {
        self[..] == other[..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mapped_buffer_reads_and_writes_like_a_vector() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("TMV_SCRATCH", dir.path());
        let (map, keep) = mapped(1000).unwrap();
        let m = Buf::Map(map, 1000, keep);
        let mut m = m;
        assert!(m.iter().all(|&v| v == 0.0));
        for (i, v) in m.iter_mut().enumerate() {
            *v = i as f32;
        }
        let c = m.clone();
        assert_eq!(c[999], 999.0);
        assert_eq!(m.into_vec()[10], 10.0);
        // The scratch file is already gone from the folder.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

/// Values whose pages can be let go (`Buf::trim`); nothing for a vector.
pub trait Trim {
    fn trim(&self) {}
}
impl Trim for Vec<f32> {}
impl Trim for Buf {
    fn trim(&self) {
        Buf::trim(self)
    }
}
