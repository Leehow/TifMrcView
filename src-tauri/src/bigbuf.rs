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

/// Floats in memory, or in a mapped scratch file (already unlinked).
pub enum Buf {
    Mem(Vec<f32>),
    Map(memmap2::MmapMut, usize),
}

/// Buffers smaller than this are always in memory.
const SMALL: u64 = 256 << 20;

/// Where scratch files go: beside the job's results (the big disk the user
/// chose), else the system's temporary folder.
fn scratch_dir() -> PathBuf {
    std::env::var_os("TMV_SCRATCH")
        .or_else(|| std::env::var_os("TMV_OUTPUT_ROOT"))
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
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

fn mapped(len: usize) -> std::io::Result<memmap2::MmapMut> {
    let dir = scratch_dir();
    let name = format!(".tmv-scratch-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let path = dir.join(name);
    let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
    // Gone from the folder at once; the mapping keeps the space until dropped.
    let _ = std::fs::remove_file(&path);
    file.set_len((len.max(1) * 4) as u64)?;
    unsafe { memmap2::MmapMut::map_mut(&file) }
}

static NEXT: AtomicU64 = AtomicU64::new(0);

impl Buf {
    /// `len` zeros, in memory when they fit, else in a scratch file.
    pub fn zeros(len: usize) -> Buf {
        let bytes = (len * 4) as u64;
        if !fits(bytes) {
            match mapped(len) {
                Ok(m) => return Buf::Map(m, len),
                Err(e) => eprintln!("scratch file unavailable ({e}); keeping {} MB in memory", bytes >> 20),
            }
        }
        Buf::from(vec![0f32; len])
    }

    /// `len` values `f(i)`, computed in parallel.
    pub fn from_fn(len: usize, f: impl Fn(usize) -> f32 + Sync) -> Buf {
        let mut b = Buf::zeros(len);
        b.par_chunks_mut(1 << 16).enumerate().for_each(|(k, chunk)| {
            for (j, v) in chunk.iter_mut().enumerate() {
                *v = f(k * (1 << 16) + j);
            }
        });
        b
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
        if let Buf::Mem(v) = self {
            IN_MEMORY.fetch_sub((v.len() * 4) as u64, Ordering::Relaxed);
        }
    }
}

impl Deref for Buf {
    type Target = [f32];
    fn deref(&self) -> &[f32] {
        match self {
            Buf::Mem(v) => v,
            // A mapping starts on a page boundary, so the floats are aligned.
            Buf::Map(m, n) => unsafe { std::slice::from_raw_parts(m.as_ptr() as *const f32, *n) },
        }
    }
}

impl DerefMut for Buf {
    fn deref_mut(&mut self) -> &mut [f32] {
        match self {
            Buf::Mem(v) => v,
            Buf::Map(m, n) => unsafe { std::slice::from_raw_parts_mut(m.as_mut_ptr() as *mut f32, *n) },
        }
    }
}

impl Clone for Buf {
    fn clone(&self) -> Buf {
        let mut b = Buf::zeros(self.len());
        b.par_chunks_mut(1 << 16).zip(self.par_chunks(1 << 16)).for_each(|(d, s)| d.copy_from_slice(s));
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
        let m = Buf::Map(
            {
                std::env::set_var("TMV_SCRATCH", dir.path());
                mapped(1000).unwrap()
            },
            1000,
        );
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
