//! Single-particle analysis (Pro): micrographs → CTF → particle picking →
//! extraction → reference-free 2D classification → ab initio model →
//! gold-standard 3D refinement → FSC and sharpening. Independent Rust
//! implementations of published methods (see each module).

pub mod class2d;
pub mod data;
pub mod export;
pub mod extract;
pub mod fourier;
pub mod geom;
pub mod pick;
pub mod plane;
pub mod project;
pub mod post;
pub mod refine;
pub mod render;
pub mod score;
pub mod volume;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};

pub static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn check_cancel() -> Result<()> {
    if CANCEL.load(Ordering::Relaxed) {
        bail!("已取消");
    }
    Ok(())
}

/// Progress: (stage, done, total).
pub type Progress<'a> = &'a (dyn Fn(&str, usize, usize) + Sync);

/// Small deterministic generator (splitmix64).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn gauss(&mut self) -> f64 {
        let (u, v) = (self.unit().max(1e-300), self.unit());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}
